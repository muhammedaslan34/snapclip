// No console window on Windows in release builds.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod capture;
mod clip_panel;
mod clip_watch;
mod gl_window;
mod history;
mod hotkeys;
mod icon;
mod pin;
mod platform;
mod scroll;
mod settings;
mod settings_view;
mod shortcuts;
mod theme;
mod toast;
mod tray;

use winit::event_loop::{ControlFlow, EventLoop};

fn main() -> anyhow::Result<()> {
    let mut logger = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"));
    // Release builds have no console, so write the log next to the app data instead.
    if !cfg!(debug_assertions) {
        let dir = settings::data_dir();
        history::ensure_dir(&dir);
        if let Ok(file) = std::fs::File::create(dir.join("snapclip.log")) {
            logger.target(env_logger::Target::Pipe(Box::new(file)));
        }
    }
    logger.init();

    let Some(_instance_lock) = platform::single_instance_lock() else {
        log::info!("{} is already running", settings::APP_NAME);
        return Ok(());
    };

    let event_loop = EventLoop::<app::AppEvent>::with_user_event().build()?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let mut app = app::App::new(&event_loop);
    event_loop.run_app(&mut app)?;
    Ok(())
}
