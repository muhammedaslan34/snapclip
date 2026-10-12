//! An egui window backed by its own OpenGL context.
//!
//! Windows are created on demand and fully destroyed when closed, so the app
//! holds no GPU resources while it sits in the tray.

use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use egui_glow::glow;
use glutin::context::{
    ContextApi, ContextAttributesBuilder, NotCurrentGlContext, PossiblyCurrentContext,
    PossiblyCurrentGlContext,
};
use glutin::display::{GetGlDisplay, GlDisplay};
use glutin::surface::{GlSurface, Surface, SurfaceAttributesBuilder, SwapInterval, WindowSurface};
use raw_window_handle::HasWindowHandle;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoopProxy};
use winit::window::{Window, WindowAttributes};

use crate::app::{AppEvent, Command, Shared};

/// The content of a window.
pub trait View {
    fn ui(&mut self, ui: &mut egui::Ui, cx: &mut Cx<'_>);

    /// Close the window when it loses keyboard focus (popup-style panels).
    fn close_on_focus_lost(&self) -> bool {
        false
    }

    /// Whether the window grabs keyboard focus when it opens.
    fn takes_focus(&self) -> bool {
        true
    }

    fn kind(&self) -> ViewKind;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewKind {
    Capture,
    Editor,
    Clipboard,
    Settings,
    Pin,
    Toast,
    ScrollControl,
}

/// What a view can touch while it builds its UI.
pub struct Cx<'a> {
    pub shared: &'a mut Shared,
    pub commands: &'a mut Vec<Command>,
    pub close: bool,
}

pub struct HostWindow {
    // Field order matters: the GL objects must drop before the window.
    egui: egui_glow::EguiGlow,
    gl: Arc<glow::Context>,
    gl_surface: Surface<WindowSurface>,
    gl_context: PossiblyCurrentContext,
    pub window: Window,
    pub view: Box<dyn View>,
    pub repaint_at: Option<Instant>,
    shown: bool,
}

impl HostWindow {
    pub fn new(
        event_loop: &ActiveEventLoop,
        attributes: WindowAttributes,
        view: Box<dyn View>,
        proxy: &EventLoopProxy<AppEvent>,
    ) -> anyhow::Result<Self> {
        // Stay hidden until the first frame is painted to avoid a white flash.
        let attributes = attributes
            .with_visible(false)
            .with_window_icon(crate::icon::window_icon());

        let template = glutin::config::ConfigTemplateBuilder::new()
            .prefer_hardware_accelerated(None)
            .with_depth_size(0)
            .with_stencil_size(0)
            .with_transparency(false);
        let (window, gl_config) = glutin_winit::DisplayBuilder::new()
            .with_preference(glutin_winit::ApiPreference::FallbackEgl)
            .with_window_attributes(Some(attributes.clone()))
            .build(event_loop, template, |mut configs| {
                configs.next().expect("no OpenGL config available")
            })
            .map_err(|err| anyhow::anyhow!("cannot create OpenGL display: {err}"))?;
        let gl_display = gl_config.display();
        let window = match window {
            Some(window) => window,
            None => glutin_winit::finalize_window(event_loop, attributes, &gl_config)
                .context("cannot create window")?,
        };

        let raw_handle = window.window_handle()?.as_raw();
        let context = unsafe {
            gl_display
                .create_context(&gl_config, &ContextAttributesBuilder::new().build(Some(raw_handle)))
                .or_else(|_| {
                    let gles = ContextAttributesBuilder::new()
                        .with_context_api(ContextApi::Gles(None))
                        .build(Some(raw_handle));
                    gl_display.create_context(&gl_config, &gles)
                })
                .context("cannot create OpenGL context")?
        };

        let (width, height): (u32, u32) = window.inner_size().into();
        let surface_attributes = SurfaceAttributesBuilder::<WindowSurface>::new().build(
            raw_handle,
            NonZeroU32::new(width).unwrap_or(NonZeroU32::MIN),
            NonZeroU32::new(height).unwrap_or(NonZeroU32::MIN),
        );
        let gl_surface = unsafe { gl_display.create_window_surface(&gl_config, &surface_attributes)? };
        let gl_context = context.make_current(&gl_surface)?;
        let _ = gl_surface.set_swap_interval(&gl_context, SwapInterval::Wait(NonZeroU32::MIN));

        let gl = Arc::new(unsafe {
            glow::Context::from_loader_function_cstr(|name| gl_display.get_proc_address(name))
        });
        let egui = egui_glow::EguiGlow::new(
            event_loop,
            gl.clone(),
            None,
            Some(window.scale_factor() as f32),
            true,
        );
        crate::theme::apply(&egui.egui_ctx);

        let window_id = window.id();
        let proxy = egui::mutex::Mutex::new(proxy.clone());
        egui.egui_ctx.set_request_repaint_callback(move |info| {
            let _ = proxy
                .lock()
                .send_event(AppEvent::Repaint(window_id, info.delay));
        });

        Ok(Self {
            egui,
            gl,
            gl_surface,
            gl_context,
            window,
            view,
            repaint_at: None,
            shown: false,
        })
    }

    pub fn kind(&self) -> ViewKind {
        self.view.kind()
    }

    pub fn schedule_repaint(&mut self, delay: Duration) {
        if delay.is_zero() {
            self.window.request_redraw();
        } else if let Some(at) = Instant::now().checked_add(delay) {
            self.repaint_at = Some(self.repaint_at.map_or(at, |old| old.min(at)));
        }
    }

    /// Feeds a window event to egui. Returns `true` if the window should close.
    pub fn on_event(&mut self, event: &WindowEvent) {
        if let WindowEvent::Resized(size) = event {
            self.gl_surface.resize(
                &self.gl_context,
                NonZeroU32::new(size.width).unwrap_or(NonZeroU32::MIN),
                NonZeroU32::new(size.height).unwrap_or(NonZeroU32::MIN),
            );
        }
        let response = self.egui.on_window_event(&self.window, event);
        if response.repaint {
            self.window.request_redraw();
        }
    }

    /// Runs the UI and paints a frame. Returns `true` if the view asked to close.
    pub fn redraw(&mut self, shared: &mut Shared, commands: &mut Vec<Command>) -> bool {
        self.repaint_at = None;
        if let Err(err) = self.gl_context.make_current(&self.gl_surface) {
            log::error!("make_current failed: {err}");
            return false;
        }

        let mut close = false;
        let Self {
            egui, view, window, ..
        } = self;
        egui.run(window, |ui| {
            let mut cx = Cx {
                shared,
                commands,
                close: false,
            };
            view.ui(ui, &mut cx);
            close |= cx.close;
        });

        unsafe {
            use glow::HasContext as _;
            self.gl.clear_color(0.08, 0.08, 0.09, 1.0);
            self.gl.clear(glow::COLOR_BUFFER_BIT);
        }
        self.egui.paint(&self.window);
        if let Err(err) = self.gl_surface.swap_buffers(&self.gl_context) {
            log::error!("swap_buffers failed: {err}");
        }

        if !self.shown {
            self.shown = true;
            self.window.set_visible(true);
            if self.view.takes_focus() {
                self.window.focus_window();
            }
        }
        close
    }
}

impl Drop for HostWindow {
    fn drop(&mut self) {
        // The painter must be destroyed while its context is current.
        if self.gl_context.make_current(&self.gl_surface).is_ok() {
            self.egui.destroy();
        }
    }
}
