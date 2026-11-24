//! The tray on Linux and other freedesktop systems: a StatusNotifierItem
//! served over D-Bus by `ksni` on its own thread.

use std::sync::mpsc::Sender;

use ksni::blocking::{Handle, TrayMethods};
use ksni::menu::{MenuItem, StandardItem};

use super::{Event, Info};

pub struct Tray {
    handle: Handle<Item>,
    info: Info,
}

impl Tray {
    pub fn new(events: Sender<Event>) -> Result<Self, String> {
        let item = Item {
            info: Info::default(),
            icons: icons(),
            events,
        };
        // Fails without a tray host (no StatusNotifierWatcher on the bus).
        let handle = item
            .spawn()
            .map_err(|e| format!("no tray on this desktop: {e}"))?;
        Ok(Self {
            handle,
            info: Info::default(),
        })
    }

    pub fn set_info(&mut self, info: Info) {
        if info == self.info {
            return;
        }
        self.info = info.clone();
        self.handle.update(|item| item.info = info);
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        let _ = self.handle.shutdown();
    }
}

/// The tray item as served over D-Bus.
struct Item {
    info: Info,
    icons: Vec<ksni::Icon>,
    events: Sender<Event>,
}

impl ksni::Tray for Item {
    fn id(&self) -> String {
        "sisyphus".to_string()
    }

    fn title(&self) -> String {
        "Sisyphus".to_string()
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        self.icons.clone()
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: "Sisyphus".to_string(),
            // The description may hold a little HTML.
            description: self
                .info
                .lines
                .iter()
                .map(|line| escape_markup(line))
                .collect::<Vec<_>>()
                .join("\n"),
            ..Default::default()
        }
    }

    /// A left click.
    fn activate(&mut self, _x: i32, _y: i32) {
        let _ = self.events.send(Event::Open);
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let mut items: Vec<MenuItem<Self>> = self
            .info
            .lines
            .iter()
            .map(|line| {
                StandardItem {
                    label: escape_label(line),
                    enabled: false,
                    ..Default::default()
                }
                .into()
            })
            .collect();
        if !items.is_empty() {
            items.push(MenuItem::Separator);
        }
        items.push(
            StandardItem {
                label: "Open Sisyphus".to_string(),
                activate: Box::new(|item: &mut Self| {
                    let _ = item.events.send(Event::Open);
                }),
                ..Default::default()
            }
            .into(),
        );
        items.push(
            StandardItem {
                label: "Quit".to_string(),
                activate: Box::new(|item: &mut Self| {
                    let _ = item.events.send(Event::Quit);
                }),
                ..Default::default()
            }
            .into(),
        );
        items
    }
}

/// The logo in the sizes trays commonly use, as ARGB32 in network byte
/// order.
fn icons() -> Vec<ksni::Icon> {
    [22u32, 32, 48, 64]
        .into_iter()
        .filter_map(|size| {
            let rgba = crate::logo::rgba(size)?;
            let data = rgba
                .chunks_exact(4)
                .flat_map(|p| [p[3], p[0], p[1], p[2]])
                .collect();
            Some(ksni::Icon {
                width: size as i32,
                height: size as i32,
                data,
            })
        })
        .collect()
}

/// Menu labels treat `_` as an access key marker; `__` shows one `_`.
fn escape_label(text: &str) -> String {
    text.replace('_', "__")
}

/// Tooltip descriptions may contain a subset of HTML.
fn escape_markup(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_labels_and_markup() {
        assert_eq!(escape_label("fix_login"), "fix__login");
        assert_eq!(escape_markup("a <b> & c"), "a &lt;b&gt; &amp; c");
    }

    #[test]
    fn icons_are_argb() {
        let icons = icons();
        assert_eq!(icons.len(), 4);
        for icon in icons {
            assert_eq!(icon.data.len(), (icon.width * icon.height * 4) as usize);
        }
    }
}
