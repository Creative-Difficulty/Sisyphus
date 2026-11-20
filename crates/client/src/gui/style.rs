//! The window's look: pink, rounded and a little playful, set in Fredoka.
//! A light and a dark variant.

use iced::font::Weight;
use iced::theme::Palette;
use iced::widget::{button, container, rule, svg, text};
use iced::{Border, Color, Font, Theme};

use crate::agents::Integration;

/// Fredoka, regular and semibold (SIL Open Font License, see
/// `assets/fonts/Fredoka-OFL.txt`).
pub const FONTS: [&[u8]; 2] = [
    include_bytes!("../../assets/fonts/Fredoka-Regular.ttf"),
    include_bytes!("../../assets/fonts/Fredoka-SemiBold.ttf"),
];
/// The family name these font files register under.
pub const FONT: Font = Font::with_name("Fredoka Light");
pub const BOLD: Font = Font {
    weight: Weight::Semibold,
    ..FONT
};

pub fn theme(dark: bool) -> Theme {
    let rgb = Color::from_rgb8;
    if dark {
        Theme::custom(
            "Sisyphus Dark",
            Palette {
                background: rgb(0x2A, 0x10, 0x22),
                text: rgb(0xFF, 0xE0, 0xEE),
                primary: rgb(0xFF, 0x6F, 0xAE),
                success: rgb(0x5F, 0xD3, 0xA6),
                warning: rgb(0xFF, 0xC0, 0x6B),
                danger: rgb(0xFF, 0x6B, 0x7F),
            },
        )
    } else {
        Theme::custom(
            "Sisyphus",
            Palette {
                background: rgb(0xFF, 0xD6, 0xE7),
                text: rgb(0x4A, 0x19, 0x42),
                primary: rgb(0xE0, 0x2A, 0x78),
                success: rgb(0x23, 0x8F, 0x6B),
                warning: rgb(0xB9, 0x6E, 0x10),
                danger: rgb(0xC0, 0x24, 0x48),
            },
        )
    }
}

/// Secondary text: the text color, faded.
pub fn muted(theme: &Theme) -> text::Style {
    text::Style {
        color: Some(theme.palette().text.scale_alpha(0.55)),
    }
}

/// Text in the accent pink.
pub fn accent(theme: &Theme) -> text::Style {
    text::Style {
        color: Some(theme.palette().primary),
    }
}

/// The line between list entries.
pub fn rule(theme: &Theme) -> rule::Style {
    rule::Style {
        color: theme.palette().primary.scale_alpha(0.25),
        ..rule::default(theme)
    }
}

/// The mark at the edge of an entry; invisible unless `shown`.
pub fn mark(theme: &Theme, shown: bool) -> container::Style {
    container::Style {
        background: shown.then(|| theme.palette().primary.into()),
        border: Border {
            radius: 2.0.into(),
            ..Border::default()
        },
        ..container::Style::default()
    }
}

/// An agent on the connect screen: a round-cornered tile, outlined in the
/// accent once connected.
pub fn tile(theme: &Theme, connected: bool) -> container::Style {
    let palette = theme.palette();
    let veil = if theme.extended_palette().is_dark {
        0.06
    } else {
        0.45
    };
    container::Style {
        background: Some(Color::WHITE.scale_alpha(veil).into()),
        border: Border {
            width: 2.0,
            radius: 22.0.into(),
            color: if connected {
                palette.primary
            } else {
                palette.primary.scale_alpha(0.2)
            },
        },
        ..container::Style::default()
    }
}

/// A plain text link that turns pink on hover.
pub fn link(theme: &Theme, status: button::Status) -> button::Style {
    let palette = theme.palette();
    button::Style {
        text_color: match status {
            button::Status::Hovered | button::Status::Pressed => palette.primary,
            _ => palette.text.scale_alpha(0.6),
        },
        ..button::Style::default()
    }
}

/// The main button: pink with a white label.
pub fn pink(theme: &Theme, status: button::Status) -> button::Style {
    button::Style {
        text_color: Color::WHITE,
        ..button::primary(theme, status)
    }
}

/// A standard button style, fully rounded.
pub fn rounded(
    base: fn(&Theme, button::Status) -> button::Style,
) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |theme, status| {
        let mut style = base(theme, status);
        style.border.radius = 999.0.into();
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
