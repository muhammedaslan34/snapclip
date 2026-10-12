// No console window on Windows in release builds.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod capture;
mod clip_panel;
mod clip_watch;
mod host;
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
    // A second launch must not wipe the running instance's log.
    if !cfg!(debug_assertions) {
        let dir = settings::data_dir();
        history::ensure_dir(&dir);
        let first = platform::single_instance_probe();
        let file = if first {
            std::fs::File::create(dir.join("snapclip.log"))
        } else {
            std::fs::OpenOptions::new().append(true).create(true).open(dir.join("snapclip.log"))
        };
        if let Ok(file) = file {
            logger.target(env_logger::Target::Pipe(Box::new(file)));
        }
    }
    logger.init();

    // A crash should never be silent: log it and tell the user.
    std::panic::set_hook(Box::new(|info| {
        log::error!("crash: {info}");
        platform::error_box(&format!(
            "SnapClip crashed:\n\n{info}\n\nDetails are in {}",
            settings::data_dir().join("snapclip.log").display()
        ));
    }));

    let Some(_instance_lock) = platform::single_instance_lock() else {
        // Already running: ask that instance to show itself instead.
        let request = platform::Request::from_args().unwrap_or(platform::Request::Settings);
        if !platform::send_request(request) {
            log::info!("{} is already running", settings::APP_NAME);
        }
        return Ok(());
    };

    let event_loop = EventLoop::<app::AppEvent>::with_user_event().build()?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let mut app = app::App::new(&event_loop);
    event_loop.run_app(&mut app)?;
    Ok(())
}
