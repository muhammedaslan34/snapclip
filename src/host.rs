//! An egui window rendered with wgpu (DirectX 12 on Windows, with a software
//! fallback when there is no GPU driver, e.g. in VMs and Remote Desktop).
//!
//! Windows are created on demand and fully destroyed when closed, so the app
//! holds no GPU resources while it sits in the tray.

use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::ViewportId;
use egui_wgpu::winit::Painter;
use egui_wgpu::{RendererOptions, WgpuConfiguration, WgpuSetup};
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
    // Field order matters: the painter (and its surface) must drop before the window.
    painter: Painter,
    egui_ctx: egui::Context,
    egui_winit: egui_winit::State,
    viewport_info: egui::ViewportInfo,
    pub window: Arc<Window>,
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
        let window = Arc::new(event_loop.create_window(attributes)?);

        let egui_ctx = egui::Context::default();
        crate::theme::apply(&egui_ctx);

        let config = WgpuConfiguration {
            wgpu_setup: WgpuSetup::from_display_handle(event_loop.owned_display_handle()),
            ..Default::default()
        };
        let mut painter = pollster::block_on(Painter::new(
            egui_ctx.clone(),
            config,
            false,
            RendererOptions::default(),
        ));
        pollster::block_on(painter.set_window(ViewportId::ROOT, Some(window.clone())))
            .map_err(|err| anyhow::anyhow!("cannot set up graphics for the window: {err}"))?;
        if let Some(state) = painter.render_state() {
            let info = state.adapter.get_info();
            log::info!("rendering with {} ({:?})", info.name, info.backend);
            // wgpu panics on errors by default; a glitchy frame is better than a crash.
            state
                .device
                .on_uncaptured_error(Arc::new(|err| log::error!("graphics error: {err}")));
        }

        let egui_winit = egui_winit::State::new(
            egui_ctx.clone(),
            ViewportId::ROOT,
            event_loop,
            Some(window.scale_factor() as f32),
            event_loop.system_theme(),
            painter.max_texture_side(),
        );

        let window_id = window.id();
        let proxy = egui::mutex::Mutex::new(proxy.clone());
        egui_ctx.set_request_repaint_callback(move |info| {
            let _ = proxy
                .lock()
                .send_event(AppEvent::Repaint(window_id, info.delay));
        });

        Ok(Self {
            painter,
            egui_ctx,
            egui_winit,
            viewport_info: Default::default(),
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

    /// Feeds a window event to egui.
    pub fn on_event(&mut self, event: &WindowEvent) {
        if let WindowEvent::Resized(size) = event {
            if let (Some(w), Some(h)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) {
                self.painter.on_window_resized(ViewportId::ROOT, w, h);
            }
        }
        let response = self.egui_winit.on_window_event(&self.window, event);
        if response.repaint {
            self.window.request_redraw();
        }
    }

    /// Runs the UI and paints a frame. Returns `true` if the view asked to close.
    pub fn redraw(&mut self, shared: &mut Shared, commands: &mut Vec<Command>) -> bool {
        self.repaint_at = None;
        let raw_input = self.egui_winit.take_egui_input(&self.window);

        let mut close = false;
        let view = &mut self.view;
        let output = self.egui_ctx.run_ui(raw_input, |ui| {
            let mut cx = Cx {
                shared,
                commands,
                close: false,
            };
            view.ui(ui, &mut cx);
            close |= cx.close;
        });

        for (_, viewport) in output.viewport_output {
            let mut actions = Default::default();
            egui_winit::process_viewport_commands(
                &self.egui_ctx,
                &mut self.viewport_info,
                viewport.commands,
                &self.window,
                &mut actions,
            );
        }
        self.egui_winit
            .handle_platform_output(&self.window, output.platform_output);

        let primitives = self
            .egui_ctx
            .tessellate(output.shapes, output.pixels_per_point);
        let mut textures = output.textures_delta;
        self.painter.paint_and_update_textures(
            ViewportId::ROOT,
            output.pixels_per_point,
            [0.03, 0.03, 0.035, 1.0],
            &primitives,
            &mut textures,
            Vec::new(),
            &self.window,
        );

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
