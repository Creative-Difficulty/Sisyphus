//! The tray on Windows and macOS, through `tray-icon`. Must be created and
//! updated on the main thread (iced's `update` runs there).

use std::sync::mpsc::Sender;

use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

use super::{Event, Info};

const OPEN: &str = "open";
const QUIT: &str = "quit";

/// Windows cuts tooltips at 127 characters.
const TOOLTIP_MAX: usize = 127;

pub struct Tray {
    icon: TrayIcon,
    info: Info,
}

impl Tray {
    pub fn new(events: Sender<Event>) -> Result<Self, String> {
        let menu_events = events.clone();
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            let _ = match event.id.as_ref() {
                OPEN => menu_events.send(Event::Open),
                QUIT => menu_events.send(Event::Quit),
                _ => Ok(()),
            };
        }));
        // A left click shows the panel; the menu is on the right click.
        TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                rect,
                ..
            } = event
            {
                let _ = events.send(Event::Panel(super::IconRect {
                    x: rect.position.x,
                    y: rect.position.y,
                    width: f64::from(rect.size.width),
                    height: f64::from(rect.size.height),
                }));
            }
        }));
        const SIZE: u32 = 64;
        let rgba = crate::logo::rgba(SIZE).ok_or("could not draw the tray icon")?;
        let image = tray_icon::Icon::from_rgba(rgba, SIZE, SIZE).map_err(|e| e.to_string())?;
        let info = Info::default();
        let icon = TrayIconBuilder::new()
            .with_icon(image)
            .with_tooltip(super::shorten(&info.tooltip(), TOOLTIP_MAX))
            .with_menu(Box::new(menu(&info)?))
            .with_menu_on_left_click(false)
            .build()
            .map_err(|e| format!("could not show the tray icon: {e}"))?;
        Ok(Self { icon, info })
    }

    pub fn set_info(&mut self, info: Info) {
        if info == self.info {
            return;
        }
        let _ = self
            .icon
            .set_tooltip(Some(super::shorten(&info.tooltip(), TOOLTIP_MAX)));
        if let Ok(menu) = menu(&info) {
            self.icon.set_menu(Some(Box::new(menu)));
        }
        self.info = info;
    }
}

/// The info lines (greyed out), then "Open Sisyphus" and "Quit".
fn menu(info: &Info) -> Result<Menu, String> {
    let menu = Menu::new();
    for line in &info.lines {
        menu.append(&MenuItem::new(escape_label(line), false, None))
            .map_err(|e| e.to_string())?;
    }
    if !info.lines.is_empty() {
        menu.append(&PredefinedMenuItem::separator())
            .map_err(|e| e.to_string())?;
    }
    menu.append(&MenuItem::with_id(OPEN, "Open Sisyphus", true, None))
        .map_err(|e| e.to_string())?;
    menu.append(&MenuItem::with_id(QUIT, "Quit", true, None))
        .map_err(|e| e.to_string())?;
    Ok(menu)
}

/// On Windows `&` marks the access key; `&&` shows one `&`.
fn escape_label(text: &str) -> String {
    if cfg!(windows) {
        text.replace('&', "&&")
    } else {
        text.to_string()
    }
}
