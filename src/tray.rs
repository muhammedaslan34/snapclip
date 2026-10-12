//! System tray icon and its menu.

use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{TrayIcon, TrayIconBuilder, TrayIconEvent};
use winit::event_loop::EventLoopProxy;

use crate::app::AppEvent;
use crate::settings::APP_NAME;

pub const MENU_CAPTURE: &str = "capture";
pub const MENU_CLIPBOARD: &str = "clipboard";
pub const MENU_SETTINGS: &str = "settings";
pub const MENU_QUIT: &str = "quit";

/// Forwards tray and menu events into the winit event loop.
pub fn install_event_handlers(proxy: &EventLoopProxy<AppEvent>) {
    let menu_proxy = proxy.clone();
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        let _ = menu_proxy.send_event(AppEvent::Menu(event.id.0));
    }));
    let tray_proxy = proxy.clone();
    TrayIconEvent::set_event_handler(Some(move |event| {
        let _ = tray_proxy.send_event(AppEvent::Tray(event));
    }));
}

fn build() -> anyhow::Result<TrayIcon> {
    let menu = Menu::new();
    menu.append_items(&[
        &MenuItem::with_id(MENU_CAPTURE, "Take screenshot", true, None),
        &MenuItem::with_id(MENU_CLIPBOARD, "Clipboard history", true, None),
        &PredefinedMenuItem::separator(),
        &MenuItem::with_id(MENU_SETTINGS, "Settings…", true, None),
        &PredefinedMenuItem::separator(),
        &MenuItem::with_id(MENU_QUIT, "Quit", true, None),
    ])?;

    let mut builder = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .with_tooltip(APP_NAME);
    if let Some(icon) = crate::icon::tray_icon() {
        builder = builder.with_icon(icon);
    }
    Ok(builder.build()?)
}

/// On Windows the tray must live on the thread that runs the event loop.
#[cfg(not(target_os = "linux"))]
pub fn create() -> Option<TrayIcon> {
    build()
        .inspect_err(|err| log::error!("cannot create tray icon: {err}"))
        .ok()
}

/// On Linux the tray needs a GTK main loop, so it gets its own thread.
#[cfg(target_os = "linux")]
pub fn create() -> Option<TrayIcon> {
    std::thread::Builder::new()
        .name("tray-gtk".into())
        .spawn(|| {
            if let Err(err) = gtk::init() {
                log::error!("cannot init GTK for the tray icon: {err}");
                return;
            }
            match build() {
                Ok(_tray) => gtk::main(),
                Err(err) => log::error!("cannot create tray icon: {err}"),
            }
        })
        .ok();
    None
}
