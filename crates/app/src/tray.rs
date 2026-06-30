//! System tray icon + menu, so Formant can live in the background.

use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

/// Actions the tray can request of the app.
pub enum TrayAction {
    Show,
    ToggleBypass,
    Quit,
}

/// Owns the tray icon (dropping it removes the icon from the tray).
pub struct Tray {
    _icon: TrayIcon,
}

/// Build the tray icon + menu. Returns None if the platform refuses it.
pub fn build() -> Option<Tray> {
    let icon = Icon::from_rgba(include_bytes!("../icon.rgba").to_vec(), 256, 256).ok()?;
    let menu = Menu::new();
    menu.append(&MenuItem::with_id("show", "Show Formant", true, None)).ok()?;
    menu.append(&PredefinedMenuItem::separator()).ok()?;
    menu.append(&MenuItem::with_id("bypass", "Toggle bypass", true, None)).ok()?;
    menu.append(&MenuItem::with_id("quit", "Quit", true, None)).ok()?;

    let icon = TrayIconBuilder::new()
        .with_tooltip("Formant — vocal processor")
        .with_menu(Box::new(menu))
        .with_icon(icon)
        .build()
        .ok()?;
    Some(Tray { _icon: icon })
}

/// Drain tray + menu events into actions for the app to handle this frame.
pub fn poll() -> Vec<TrayAction> {
    let mut actions = Vec::new();
    while let Ok(ev) = TrayIconEvent::receiver().try_recv() {
        if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = ev {
            actions.push(TrayAction::Show);
        }
    }
    while let Ok(ev) = MenuEvent::receiver().try_recv() {
        match ev.id.0.as_str() {
            "show" => actions.push(TrayAction::Show),
            "bypass" => actions.push(TrayAction::ToggleBypass),
            "quit" => actions.push(TrayAction::Quit),
            _ => {}
        }
    }
    actions
}
