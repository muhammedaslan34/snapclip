//! The background app: owns the tray, hotkeys, clipboard history and any open windows.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use global_hotkey::{GlobalHotKeyEvent, HotKeyState};
use image::RgbaImage;
use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconEvent};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use winit::event::{StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::monitor::MonitorHandle;
use winit::window::{Fullscreen, WindowAttributes, WindowId, WindowLevel};

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use crate::capture::{Action, CaptureView};
use crate::clip_panel::ClipboardView;
use crate::clip_watch::{self, ClipEvent};
use crate::host::{HostWindow, View, ViewKind};
use crate::history::{ClipContent, History};
use crate::hotkeys::{HotkeyAction, Hotkeys};
use crate::pin::PinView;
use crate::platform::{self, ForeignWindow};
use crate::scroll::{self, Region, ScrollMsg, ScrollStatus};
use crate::settings::{APP_NAME, AfterCapture, Settings};
use crate::settings_view::SettingsView;
use crate::toast::{ScrollControlView, ToastView};
use crate::tray;

#[derive(Debug)]
pub enum AppEvent {
    HotKey(GlobalHotKeyEvent),
    Menu(String),
    Tray(TrayIconEvent),
    Clip(ClipEvent),
    Repaint(WindowId, Duration),
    SaveFolderPicked(PathBuf),
    Scroll(ScrollMsg),
    /// Sent by a second launch of the app.
    Request(platform::Request),
}

/// Requests from views, executed by the app after the frame.
pub enum Command {
    /// Result of the capture overlay: perform `action`, then notify or open the editor.
    Captured {
        image: RgbaImage,
        action: Action,
        origin: (i32, i32),
    },
    OpenEditor(RgbaImage),
    ScrollCapture(Region),
    CopyImage(RgbaImage),
    SaveImage(RgbaImage),
    SaveImageAs(RgbaImage),
    /// Image plus its physical desktop position.
    Pin(RgbaImage, (i32, i32)),
    PasteItem(u64),
    OpenSettings,
    ApplySettings,
    PickSaveFolder,
    OpenPath(PathBuf),
}

/// State shared with views.
pub struct Shared {
    pub settings: Settings,
    pub history: History,
    /// Problems to show in the settings window (e.g. a shortcut already taken).
    pub errors: Vec<String>,
    /// The scrolling capture in progress, if any.
    pub scroll: Option<ScrollStatus>,
}

pub struct App {
    proxy: EventLoopProxy<AppEvent>,
    shared: Shared,
    windows: HashMap<WindowId, HostWindow>,
    /// Windows that have had keyboard focus at least once.
    focused: HashSet<WindowId>,
    hotkeys: Option<Hotkeys>,
    tray: Option<TrayIcon>,
    clipboard: Option<arboard::Clipboard>,
    paste_target: Option<ForeignWindow>,
}

impl App {
    pub fn new(event_loop: &EventLoop<AppEvent>) -> Self {
        let proxy = event_loop.create_proxy();
        let settings = Settings::load();
        let data_dir = crate::settings::data_dir();
        let history = History::load(data_dir, settings.history_limit);
        clip_watch::spawn(history.images_dir(), proxy.clone());

        let mut errors = Vec::new();
        let hotkeys = match Hotkeys::new(&proxy) {
            Ok(mut hotkeys) => {
                errors.extend(hotkeys.apply(&settings));
                Some(hotkeys)
            }
            Err(err) => {
                errors.push(format!("Global shortcuts are unavailable: {err}"));
                None
            }
        };
        if let Err(err) = apply_autostart(settings.autostart) {
            errors.push(format!("Could not update start-up setting: {err}"));
        }
        for err in &errors {
            log::warn!("{err}");
        }

        Self {
            proxy,
            shared: Shared {
                settings,
                history,
                errors,
                scroll: None,
            },
            windows: HashMap::new(),
            focused: HashSet::new(),
            hotkeys,
            tray: None,
            clipboard: None,
            paste_target: None,
        }
    }

    // ---------------------------------------------------------------- windows

    fn find(&self, kind: ViewKind) -> Option<WindowId> {
        self.windows
            .iter()
            .find(|(_, w)| w.kind() == kind)
            .map(|(id, _)| *id)
    }

    fn open(&mut self, event_loop: &ActiveEventLoop, attrs: WindowAttributes, view: Box<dyn View>) {
        match HostWindow::new(event_loop, attrs, view, &self.proxy) {
            Ok(window) => {
                let id = window.window.id();
                window.window.request_redraw();
                self.windows.insert(id, window);
            }
            Err(err) => {
                log::error!("cannot open window: {err:#}");
                platform::error_box(&format!("SnapClip could not open a window:\n\n{err:#}"));
            }
        }
    }

    fn close(&mut self, id: WindowId) {
        self.windows.remove(&id);
        self.focused.remove(&id);
    }

    /// Closes the window of this kind if it is open. Returns whether one was closed.
    fn close_kind(&mut self, kind: ViewKind) -> bool {
        match self.find(kind) {
            Some(id) => {
                self.close(id);
                true
            }
            None => false,
        }
    }

    fn redraw_kind(&self, kind: ViewKind) {
        if let Some(w) = self.find(kind).and_then(|id| self.windows.get(&id)) {
            w.window.request_redraw();
        }
    }

    fn handle_request(&mut self, event_loop: &ActiveEventLoop, request: platform::Request) {
        match request {
            platform::Request::Settings => self.open_settings(event_loop),
            platform::Request::Capture => self.open_capture(event_loop),
            platform::Request::Clipboard => self.toggle_clipboard(event_loop),
        }
    }

    fn open_capture(&mut self, event_loop: &ActiveEventLoop) {
        // The capture shortcut also ends a running scrolling capture.
        if let Some(status) = &self.shared.scroll {
            status.state.store(scroll::FINISH, Ordering::Relaxed);
            return;
        }
        if self.find(ViewKind::Capture).is_some() {
            return;
        }
        self.close_kind(ViewKind::Toast);
        // Keep our own popup out of the screenshot.
        if self.close_kind(ViewKind::Clipboard) {
            std::thread::sleep(Duration::from_millis(150));
        }

        let monitor = platform::cursor_pos()
            .and_then(|(x, y)| xcap::Monitor::from_point(x, y).ok())
            .or_else(|| {
                let all = xcap::Monitor::all().ok()?;
                let primary = all.iter().position(|m| m.is_primary().unwrap_or(false));
                all.into_iter().nth(primary.unwrap_or(0))
            });
        let Some(monitor) = monitor else {
            log::error!("no monitor found to capture");
            return;
        };
        let started = Instant::now();
        let image = match monitor.capture_image() {
            Ok(image) => image,
            // Development aid for sessions where capture is blocked (e.g. nested Wayland).
            Err(_) if cfg!(debug_assertions) && std::env::var_os("SNAPCLIP_FAKE_CAPTURE").is_some() => {
                let (w, h) = (monitor.width().unwrap_or(1280), monitor.height().unwrap_or(800));
                RgbaImage::from_fn(w, h, |x, y| {
                    let band = if (y / 40) % 2 == 0 { 230 } else { 245 };
                    image::Rgba([(x * 255 / w) as u8, band, (y * 255 / h) as u8, 255])
                })
            }
            Err(err) => {
                log::error!("screen capture failed: {err}");
                return;
            }
        };
        log::debug!("captured {}x{} in {:?}", image.width(), image.height(), started.elapsed());

        let pos = (monitor.x().unwrap_or(0), monitor.y().unwrap_or(0));
        let target = event_loop
            .available_monitors()
            .find(|m| m.position() == PhysicalPosition::new(pos.0, pos.1))
            .or_else(|| event_loop.primary_monitor());
        let attrs = popup_attributes("SnapClip capture")
            .with_resizable(false)
            .with_position(PhysicalPosition::new(pos.0, pos.1))
            .with_inner_size(PhysicalSize::new(image.width(), image.height()));
        // On Windows a borderless window placed over the monitor is enough, and winit's
        // fullscreen mode breaks DirectX swap-chain creation on some drivers. Wayland
        // can't position windows, so it needs real fullscreen.
        let attrs = if cfg!(windows) {
            attrs
        } else {
            attrs.with_fullscreen(Some(Fullscreen::Borderless(target)))
        };
        let view = CaptureView::new(image, pos, &self.shared.settings);
        self.open(event_loop, attrs, Box::new(view));
    }

    fn toggle_clipboard(&mut self, event_loop: &ActiveEventLoop) {
        if self.close_kind(ViewKind::Clipboard) {
            return;
        }
        self.paste_target = platform::foreground_window();

        let (w, h) = (440.0, 580.0);
        let mut attrs = popup_attributes("Clipboard history")
            .with_resizable(true)
            .with_inner_size(LogicalSize::new(w, h))
            .with_min_inner_size(LogicalSize::new(320.0, 300.0));
        if let Some((x, y)) = platform::cursor_pos() {
            let monitor = monitor_at(event_loop, x, y);
            let scale = monitor.as_ref().map_or(1.0, MonitorHandle::scale_factor);
            let (pw, ph) = ((w * scale) as i32, (h * scale) as i32);
            let (mut px, mut py) = (x + 12, y + 12);
            if let Some(m) = monitor {
                let (mx, my) = (m.position().x, m.position().y);
                let (mw, mh) = (m.size().width as i32, m.size().height as i32);
                px = px.min(mx + mw - pw - 8).max(mx);
                py = py.min(my + mh - ph - 8).max(my);
            }
            attrs = attrs.with_position(PhysicalPosition::new(px, py));
        }
        self.open(event_loop, attrs, Box::new(ClipboardView::new()));
    }

    fn open_settings(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(w) = self.find(ViewKind::Settings).and_then(|id| self.windows.get(&id)) {
            w.window.focus_window();
            return;
        }
        let attrs = WindowAttributes::default()
            .with_title(format!("{APP_NAME} Settings"))
            .with_inner_size(LogicalSize::new(640.0, 720.0))
            .with_min_inner_size(LogicalSize::new(420.0, 360.0));
        let view = SettingsView::new(&self.shared.settings);
        self.open(event_loop, attrs, Box::new(view));
    }

    fn open_editor(&mut self, event_loop: &ActiveEventLoop, image: RgbaImage) {
        let (iw, ih) = (image.width() as f64, image.height() as f64);
        let mut size = LogicalSize::new(900.0, 640.0);
        if let Some(m) = platform::cursor_pos()
            .and_then(|(x, y)| monitor_at(event_loop, x, y))
            .or_else(|| event_loop.primary_monitor())
        {
            let scale = m.scale_factor();
            let max = m.size().to_logical::<f64>(scale);
            // Image at 1:1 plus room for the toolbar, capped to 85% of the screen.
            let w = (iw / scale + 40.0).clamp(760.0, max.width * 0.85);
            let h = (ih / scale + 140.0).clamp(480.0, max.height * 0.85);
            size = LogicalSize::new(w, h);
        }
        let attrs = WindowAttributes::default()
            .with_title(format!("{APP_NAME} Editor"))
            .with_inner_size(size)
            .with_min_inner_size(LogicalSize::new(640.0, 400.0));
        let view = CaptureView::editor(image, &self.shared.settings);
        self.open(event_loop, attrs, Box::new(view));
    }

    /// Notification in the bottom-right corner that doesn't take focus.
    fn open_toast(&mut self, event_loop: &ActiveEventLoop, image: RgbaImage, title: String, origin: (i32, i32)) {
        self.close_kind(ViewKind::Toast);
        let (w, h) = (390.0, 118.0);
        let monitor = platform::cursor_pos()
            .and_then(|(x, y)| monitor_at(event_loop, x, y))
            .or_else(|| event_loop.primary_monitor());
        let mut attrs = popup_attributes("Screenshot captured")
            .with_active(false)
            .with_resizable(false)
            .with_inner_size(LogicalSize::new(w, h));
        if let Some(m) = monitor {
            let scale = m.scale_factor();
            // Leave room for the taskbar.
            let x = m.position().x + m.size().width as i32 - ((w + 16.0) * scale) as i32;
            let y = m.position().y + m.size().height as i32 - ((h + 64.0) * scale) as i32;
            attrs = attrs.with_position(PhysicalPosition::new(x, y));
        }
        self.open(event_loop, attrs, Box::new(ToastView::new(image, title, origin)));
    }

    fn start_scroll(&mut self, event_loop: &ActiveEventLoop, region: Region) {
        if self.shared.scroll.is_some() {
            return;
        }
        let status = ScrollStatus {
            state: Arc::new(AtomicU8::new(scroll::RUNNING)),
            auto_scroll: Arc::new(AtomicBool::new(cfg!(windows))),
            frames: 1,
            height: region.height,
        };
        scroll::spawn(region, &status, self.proxy.clone());
        self.shared.scroll = Some(status);

        // Put the controls next to the region, never on top of it (they would be captured).
        let (w, h) = (330.0, 104.0);
        let mut attrs = popup_attributes("Scrolling capture")
            .with_active(false)
            .with_resizable(false)
            .with_inner_size(LogicalSize::new(w, h));
        let (mx, my) = region.monitor_pos;
        if let Some(m) = monitor_at(event_loop, mx, my) {
            let scale = m.scale_factor();
            let (pw, ph) = ((w * scale) as i32, (h * scale) as i32);
            let (rx, ry) = (mx + region.x as i32, my + region.y as i32);
            let (rw, rh) = (region.width as i32, region.height as i32);
            let (mw, mh) = (m.size().width as i32, m.size().height as i32);
            let x = (rx + rw - pw).clamp(mx, mx + mw - pw);
            let y = if ry + rh + ph + 8 <= my + mh {
                ry + rh + 8
            } else if ry - ph - 8 >= my {
                ry - ph - 8
            } else {
                // No room above or below: use the screen corner furthest from the region.
                my
            };
            let x = if y == my && rx < mx + mw / 2 { mx + mw - pw } else if y == my { mx } else { x };
            attrs = attrs.with_position(PhysicalPosition::new(x, y));
        }
        self.open(event_loop, attrs, Box::new(ScrollControlView));
    }

    fn open_pin(&mut self, event_loop: &ActiveEventLoop, image: RgbaImage, pos: (i32, i32)) {
        let attrs = popup_attributes("Pinned screenshot")
            .with_resizable(false)
            .with_position(PhysicalPosition::new(pos.0, pos.1))
            .with_inner_size(PhysicalSize::new(image.width().max(24), image.height().max(24)));
        self.open(event_loop, attrs, Box::new(PinView::new(image)));
    }

    // --------------------------------------------------------------- commands

    fn run_commands(&mut self, event_loop: &ActiveEventLoop, commands: Vec<Command>) {
        for command in commands {
            match command {
                Command::Captured {
                    image,
                    action,
                    origin,
                } => {
                    let title = match action {
                        Action::Save => {
                            self.save_now(image.clone());
                            "Screenshot saved"
                        }
                        Action::SaveAs => {
                            self.run_commands(event_loop, vec![Command::SaveImageAs(image.clone())]);
                            "Screenshot captured"
                        }
                        _ => {
                            self.copy_image(image.clone());
                            "Copied to clipboard"
                        }
                    };
                    match self.shared.settings.after_capture {
                        AfterCapture::Notify => {
                            self.open_toast(event_loop, image, title.to_owned(), origin)
                        }
                        AfterCapture::Editor => self.open_editor(event_loop, image),
                        AfterCapture::Nothing => {}
                    }
                }
                Command::OpenEditor(image) => self.open_editor(event_loop, image),
                Command::ScrollCapture(region) => self.start_scroll(event_loop, region),
                Command::CopyImage(image) => self.copy_image(image),
                Command::SaveImage(image) => self.save_now(image),
                Command::SaveImageAs(image) => {
                    let dir = self.shared.settings.save_dir.clone();
                    std::thread::spawn(move || {
                        let _ = std::fs::create_dir_all(&dir);
                        let path = rfd::FileDialog::new()
                            .set_title("Save screenshot")
                            .set_directory(&dir)
                            .set_file_name(screenshot_name())
                            .add_filter("PNG image", &["png"])
                            .add_filter("JPEG image", &["jpg", "jpeg"])
                            .save_file();
                        if let Some(path) = path {
                            save_image(&path, &image);
                        }
                    });
                }
                Command::Pin(image, pos) => self.open_pin(event_loop, image, pos),
                Command::PasteItem(id) => self.paste_item(id),
                Command::OpenSettings => self.open_settings(event_loop),
                Command::ApplySettings => self.apply_settings(),
                Command::PickSaveFolder => {
                    let proxy = self.proxy.clone();
                    let dir = self.shared.settings.save_dir.clone();
                    std::thread::spawn(move || {
                        if let Some(path) = rfd::FileDialog::new().set_directory(&dir).pick_folder() {
                            let _ = proxy.send_event(AppEvent::SaveFolderPicked(path));
                        }
                    });
                }
                Command::OpenPath(path) => open_path(&path),
            }
        }
    }

    fn save_now(&self, image: RgbaImage) {
        let path = self.shared.settings.save_dir.join(screenshot_name());
        std::thread::spawn(move || save_image(&path, &image));
    }

    fn clipboard(&mut self) -> Option<&mut arboard::Clipboard> {
        if self.clipboard.is_none() {
            self.clipboard = arboard::Clipboard::new()
                .inspect_err(|err| log::error!("clipboard unavailable: {err}"))
                .ok();
        }
        self.clipboard.as_mut()
    }

    fn copy_image(&mut self, image: RgbaImage) {
        let (width, height) = image.dimensions();
        let data = arboard::ImageData {
            width: width as usize,
            height: height as usize,
            bytes: image.into_raw().into(),
        };
        if let Some(cb) = self.clipboard() {
            if let Err(err) = cb.set_image(data) {
                log::error!("cannot copy image: {err}");
            }
        }
    }

    fn paste_item(&mut self, id: u64) {
        let Some(item) = self.shared.history.get(id).cloned() else {
            return;
        };
        match &item.content {
            ClipContent::Text { text } => {
                if let Some(cb) = self.clipboard() {
                    if let Err(err) = cb.set_text(text.clone()) {
                        log::error!("cannot copy text: {err}");
                        return;
                    }
                }
            }
            ClipContent::Image { file, .. } => {
                match image::open(self.shared.history.file_path(file)) {
                    Ok(img) => self.copy_image(img.to_rgba8()),
                    Err(err) => {
                        log::error!("cannot load clipboard image: {err}");
                        return;
                    }
                }
            }
        }
        self.shared.history.push(item.hash, item.content);

        if self.shared.settings.auto_paste {
            if let Some(target) = self.paste_target.take() {
                platform::activate(target);
            }
            std::thread::spawn(|| {
                std::thread::sleep(Duration::from_millis(120));
                platform::send_paste();
            });
        }
    }

    fn apply_settings(&mut self) {
        let settings = &self.shared.settings;
        settings.save();
        let mut errors = Vec::new();
        if let Some(hotkeys) = &mut self.hotkeys {
            errors.extend(hotkeys.apply(settings));
        }
        if let Err(err) = apply_autostart(settings.autostart) {
            errors.push(format!("Could not update start-up setting: {err}"));
        }
        self.shared.history.set_limit(settings.history_limit);
        self.shared.errors = errors;
    }
}

impl ApplicationHandler<AppEvent> for App {
    fn new_events(&mut self, event_loop: &ActiveEventLoop, cause: StartCause) {
        if cause == StartCause::Init {
            tray::install_event_handlers(&self.proxy);
            self.tray = tray::create();
            log::info!("{APP_NAME} is running in the background");
            let proxy = self.proxy.clone();
            platform::listen_for_requests(move |request| {
                let _ = proxy.send_event(AppEvent::Request(request));
            });
            // Started by hand (not at sign-in): show something so the user knows it runs.
            let autostart = std::env::args().any(|a| a == "--autostart");
            match platform::Request::from_args() {
                Some(request) => self.handle_request(event_loop, request),
                None if !autostart => self.open_settings(event_loop),
                None => {}
            }
        }
    }

    fn resumed(&mut self, _event_loop: &ActiveEventLoop) {}

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: AppEvent) {
        match event {
            AppEvent::HotKey(event) => {
                if event.state != HotKeyState::Pressed {
                    return;
                }
                match self.hotkeys.as_ref().and_then(|h| h.action_for(event.id)) {
                    Some(HotkeyAction::Capture) => self.open_capture(event_loop),
                    Some(HotkeyAction::Clipboard) => self.toggle_clipboard(event_loop),
                    None => {}
                }
            }
            AppEvent::Menu(id) => match id.as_str() {
                tray::MENU_CAPTURE => self.open_capture(event_loop),
                tray::MENU_CLIPBOARD => self.toggle_clipboard(event_loop),
                tray::MENU_SETTINGS => self.open_settings(event_loop),
                tray::MENU_QUIT => event_loop.exit(),
                _ => {}
            },
            AppEvent::Tray(TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            }) => self.open_capture(event_loop),
            AppEvent::Tray(_) => {}
            AppEvent::Clip(event) => {
                self.shared.history.push(event.hash, event.content);
                self.redraw_kind(ViewKind::Clipboard);
            }
            AppEvent::Repaint(id, delay) => {
                if let Some(w) = self.windows.get_mut(&id) {
                    w.schedule_repaint(delay);
                }
            }
            AppEvent::Scroll(ScrollMsg::Progress { frames, height }) => {
                if let Some(status) = &mut self.shared.scroll {
                    status.frames = frames;
                    status.height = height;
                }
                self.redraw_kind(ViewKind::ScrollControl);
            }
            AppEvent::Scroll(ScrollMsg::Done(image)) => {
                self.shared.scroll = None;
                self.close_kind(ViewKind::ScrollControl);
                if let Some(image) = image {
                    self.open_editor(event_loop, image);
                }
            }
            AppEvent::Request(request) => self.handle_request(event_loop, request),
            AppEvent::SaveFolderPicked(path) => {
                self.shared.settings.save_dir = path;
                self.shared.settings.save();
                self.redraw_kind(ViewKind::Settings);
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let Some(window) = self.windows.get_mut(&id) else {
            return;
        };
        match &event {
            WindowEvent::CloseRequested => {
                self.close(id);
                return;
            }
            WindowEvent::Focused(true) => {
                self.focused.insert(id);
            }
            WindowEvent::Focused(false) => {
                if window.view.close_on_focus_lost() && self.focused.contains(&id) {
                    self.close(id);
                    return;
                }
            }
            WindowEvent::RedrawRequested => {
                let mut commands = Vec::new();
                if window.redraw(&mut self.shared, &mut commands) {
                    self.close(id);
                }
                self.run_commands(event_loop, commands);
                return;
            }
            _ => {}
        }
        window.on_event(&event);
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        let mut next: Option<Instant> = None;
        for window in self.windows.values_mut() {
            match window.repaint_at {
                Some(at) if at <= now => {
                    window.repaint_at = None;
                    window.window.request_redraw();
                }
                Some(at) => next = Some(next.map_or(at, |n| n.min(at))),
                None => {}
            }
        }
        event_loop.set_control_flow(match next {
            Some(at) => ControlFlow::WaitUntil(at),
            None => ControlFlow::Wait,
        });
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        self.windows.clear();
        self.tray = None;
    }
}

/// Borderless, always-on-top window that does not show in the taskbar.
fn popup_attributes(title: &str) -> WindowAttributes {
    let attrs = WindowAttributes::default()
        .with_title(title)
        .with_decorations(false)
        .with_window_level(WindowLevel::AlwaysOnTop);
    #[cfg(windows)]
    let attrs = {
        use winit::platform::windows::WindowAttributesExtWindows as _;
        attrs.with_skip_taskbar(true)
    };
    attrs
}

fn monitor_at(event_loop: &ActiveEventLoop, x: i32, y: i32) -> Option<MonitorHandle> {
    event_loop.available_monitors().find(|m| {
        let (p, s) = (m.position(), m.size());
        x >= p.x && y >= p.y && x < p.x + s.width as i32 && y < p.y + s.height as i32
    })
}

fn screenshot_name() -> String {
    chrono::Local::now()
        .format("Screenshot_%Y-%m-%d_%H-%M-%S.png")
        .to_string()
}

fn save_image(path: &std::path::Path, image: &RgbaImage) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let is_jpeg = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("jpg") || e.eq_ignore_ascii_case("jpeg"));
    let result = if is_jpeg {
        image::DynamicImage::ImageRgba8(image.clone()).to_rgb8().save(path)
    } else {
        image.save(path)
    };
    match result {
        Ok(()) => log::info!("saved {}", path.display()),
        Err(err) => log::error!("cannot save {}: {err}", path.display()),
    }
}

fn open_path(path: &std::path::Path) {
    let _ = std::fs::create_dir_all(path);
    let program = if cfg!(windows) { "explorer" } else { "xdg-open" };
    if let Err(err) = std::process::Command::new(program).arg(path).spawn() {
        log::error!("cannot open {}: {err}", path.display());
    }
}

/// Registers or removes the "start at login" entry. Skipped in debug builds so
/// `cargo run` doesn't register a development binary.
fn apply_autostart(enable: bool) -> anyhow::Result<()> {
    if cfg!(debug_assertions) {
        log::debug!("autostart is not changed in debug builds");
        return Ok(());
    }
    let exe = std::env::current_exe()?;
    let launcher = auto_launch::AutoLaunchBuilder::new()
        .set_app_name(APP_NAME)
        .set_app_path(&exe.to_string_lossy())
        .set_args(&["--autostart"])
        .set_windows_enable_mode(auto_launch::WindowsEnableMode::CurrentUser)
        .build()?;
    if enable {
        launcher.enable()?;
    } else if launcher.is_enabled()? {
        launcher.disable()?;
    }
    Ok(())
}
