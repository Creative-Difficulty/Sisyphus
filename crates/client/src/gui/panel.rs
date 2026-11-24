//! The panel that drops down from the tray icon on Windows and macOS (a
//! left click on the icon): today's time, agent time, what an agent is
//! working on, and a way into the window. A small borderless window placed
//! at the icon, closed again when it loses focus.

use iced::widget::{button, column, container, row, text};
use iced::{Center, Element, Fill, Point, Size};
use sisyphus_common::api::Today;

use super::{Message, sessions::short_duration, style};
use crate::tray::IconRect;

/// The panel's size, in logical pixels.
pub const SIZE: Size = Size::new(300.0, 236.0);

/// Space between the icon and the panel, and the panel and the screen's
/// edge.
const GAP: f32 = 6.0;

/// What the panel shows.
pub struct Status<'a> {
    pub logged_in: bool,
    pub today: Option<Today>,
    /// "Working: …", "No agent working" or "Not tracking".
    pub working: &'a str,
}

pub fn view<'a>(status: Status<'a>) -> Element<'a, Message> {
    let heading = row![
        style::app_logo(26.0),
        text("Sisyphus").size(20).font(style::BOLD)
    ]
    .spacing(10)
    .align_y(Center);

    let time: Element<'a, Message> = match (status.logged_in, status.today) {
        (false, _) => text("Not logged in").size(16).style(style::muted).into(),
        (true, None) => text("Loading").size(16).style(style::muted).into(),
        (true, Some(today)) => column![
            text("Today").size(13).style(style::muted),
            text(short_duration(today.tracked_ms))
                .size(40)
                .font(style::BOLD)
                .style(style::accent),
            text(format!("{} agent time", short_duration(today.agent_ms)))
                .size(14)
                .style(style::muted),
        ]
        .spacing(2)
        .into(),
    };

    let open = button(text("Open Sisyphus").size(15).width(Fill).center())
        .padding([8, 16])
        .width(Fill)
        .style(style::rounded(style::pink))
        .on_press(Message::OpenFromPanel);

    container(
        column![
            heading,
            time,
            text(status.working).size(14),
            iced::widget::space().height(Fill),
            open,
        ]
        .spacing(12),
    )
    .padding(18)
    .width(Fill)
    .height(Fill)
    .into()
}

/// Where the panel goes, in logical pixels: centred on the icon, below it
/// when the icon is at the top of the screen (the macOS menu bar, a
/// taskbar at the top), otherwise above it (a taskbar at the bottom).
/// `monitor` is the size of the screen the panel is on, if known; it keeps
/// the panel on screen when the icon is near the right edge.
pub fn position(icon: IconRect, scale: f32, monitor: Option<Size>) -> Point {
    let scale = f64::from(scale.max(0.5));
    let icon_x = (icon.x / scale) as f32;
    let icon_y = (icon.y / scale) as f32;
    let icon_width = (icon.width / scale) as f32;
    let icon_height = (icon.height / scale) as f32;

    let mut x = icon_x + icon_width / 2.0 - SIZE.width / 2.0;
    // Only clamp on a screen that starts at the left edge: other screens'
    // offsets aren't known here.
    if let Some(monitor) = monitor
        && icon_x < monitor.width
    {
        x = x.min(monitor.width - SIZE.width - GAP);
    }
    let x = x.max(GAP);

    let top_half = match monitor {
        Some(monitor) => icon_y + icon_height / 2.0 < monitor.height / 2.0,
        None => icon_y < SIZE.height,
    };
    let y = if top_half {
        icon_y + icon_height + GAP
    } else {
        icon_y - SIZE.height - GAP
    };
    Point::new(x, y.max(0.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn icon(x: f64, y: f64, size: f64) -> IconRect {
        IconRect {
            x,
            y,
            width: size,
            height: size,
        }
    }

    #[test]
    fn below_a_menu_bar_icon() {
        // A Retina Mac: the icon at logical (1000, 0), 24 wide.
        let p = position(icon(2000.0, 0.0, 48.0), 2.0, Some(Size::new(1512.0, 982.0)));
        assert_eq!(p, Point::new(1012.0 - SIZE.width / 2.0, 30.0));
    }

    #[test]
    fn above_a_taskbar_icon() {
        let p = position(
            icon(1700.0, 1050.0, 24.0),
            1.0,
            Some(Size::new(1920.0, 1080.0)),
        );
        assert_eq!(
            p,
            Point::new(1712.0 - SIZE.width / 2.0, 1050.0 - SIZE.height - GAP)
        );
    }

    #[test]
    fn stays_on_screen_at_the_right_edge() {
        let p = position(
            icon(1900.0, 1050.0, 24.0),
            1.0,
            Some(Size::new(1920.0, 1080.0)),
        );
        assert_eq!(p.x, 1920.0 - SIZE.width - GAP);
    }

    #[test]
    fn without_the_screen_size() {
        let p = position(icon(10.0, 0.0, 24.0), 1.0, None);
        assert_eq!(p, Point::new(GAP, 24.0 + GAP));
    }
}
