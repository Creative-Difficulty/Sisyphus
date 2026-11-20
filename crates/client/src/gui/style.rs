//! Shared look: cards, list rows, buttons and agent logos.

use iced::widget::{button, container, svg};
use iced::{Border, Theme};

use crate::agents::Integration;

/// Rounded card. A green border marks something active (a connected agent).
pub fn card(theme: &Theme, active: bool) -> container::Style {
    let palette = theme.extended_palette();
    container::Style {
        background: Some(palette.background.weak.color.into()),
        border: Border {
            width: if active { 2.0 } else { 1.0 },
            radius: 12.0.into(),
            color: if active {
                palette.success.base.color
            } else {
                palette.background.strong.color
            },
        },
        ..container::Style::default()
    }
}

/// A clickable list row that lights up on hover.
pub fn row(theme: &Theme, status: button::Status) -> button::Style {
    let palette = theme.extended_palette();
    let background = match status {
        button::Status::Hovered | button::Status::Pressed => palette.background.strong.color,
        _ => palette.background.weak.color,
    };
    button::Style {
        background: Some(background.into()),
        text_color: palette.background.weak.text,
        border: Border {
            radius: 10.0.into(),
            ..Border::default()
        },
        ..button::Style::default()
    }
}

/// A standard button style with rounded corners.
pub fn rounded(
    base: fn(&Theme, button::Status) -> button::Style,
) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |theme, status| {
        let mut style = base(theme, status);
        style.border.radius = 6.0.into();
        style
    }
}

pub fn logo(integration: &Integration, size: f32) -> svg::Svg<'static> {
    let bytes: &'static [u8] = match integration {
        Integration::ClaudeCode(_) => include_bytes!("../../assets/claude-code.svg"),
        Integration::OpenCode(_) => include_bytes!("../../assets/opencode.svg"),
    };
    svg(svg::Handle::from_memory(bytes))
        .width(size)
        .height(size)
}
