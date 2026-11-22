//! The window's look: pink, rounded and a little playful, set in Fredoka.
//! A light and a dark variant.

use iced::font::Weight;
use iced::theme::Palette;
use iced::widget::{button, container, progress_bar, scrollable, svg, text, text_input};
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

/// A session in the list: a pink pill, a deeper pink while running.
pub fn bubble(theme: &Theme, running: bool) -> container::Style {
    let pink = theme.palette().primary;
    container::Style {
        background: Some(pink.scale_alpha(if running { 0.3 } else { 0.14 }).into()),
        border: Border {
            radius: 999.0.into(),
            ..Border::default()
        },
        ..container::Style::default()
    }
}

/// A block of text on a solid bubble, a deeper pink than the background.
pub fn card(theme: &Theme) -> container::Style {
    let palette = theme.palette();
    container::Style {
        background: Some(mix(palette.background, palette.primary, 0.14).into()),
        border: Border {
            radius: 26.0.into(),
            ..Border::default()
        },
        ..container::Style::default()
    }
}

/// A rounded pink bar on a light pink track.
pub fn progress(theme: &Theme) -> progress_bar::Style {
    let palette = theme.palette();
    progress_bar::Style {
        background: mix(palette.background, palette.primary, 0.18).into(),
        bar: palette.primary.into(),
        border: Border {
            radius: 6.0.into(),
            ..Border::default()
        },
    }
}

/// `a` with `amount` of `b` mixed in, opaque.
fn mix(a: Color, b: Color, amount: f32) -> Color {
    let channel = |x: f32, y: f32| x + (y - x) * amount;
    Color::from_rgb(channel(a.r, b.r), channel(a.g, b.g), channel(a.b, b.b))
}

/// How much white lies over the background on raised surfaces.
fn veil(theme: &Theme) -> f32 {
    if theme.extended_palette().is_dark {
        0.06
    } else {
        0.45
    }
}

/// Scrollbar: a round pink blob on an invisible track, stronger while
/// hovered or dragged.
pub fn scrollbar(theme: &Theme, status: scrollable::Status) -> scrollable::Style {
    let pink = theme.palette().primary;
    let strong = matches!(
        status,
        scrollable::Status::Hovered {
            is_vertical_scrollbar_hovered: true,
            ..
        } | scrollable::Status::Dragged { .. }
    );
    let rail = scrollable::Rail {
        background: None,
        border: Border::default(),
        scroller: scrollable::Scroller {
            background: pink.scale_alpha(if strong { 0.75 } else { 0.4 }).into(),
            border: Border {
                radius: 999.0.into(),
                ..Border::default()
            },
        },
    };
    scrollable::Style {
        vertical_rail: rail,
        horizontal_rail: rail,
        ..scrollable::default(theme, status)
    }
}

/// A plain text field inside a bubble: no box of its own, pink selection.
pub fn input(theme: &Theme, _status: text_input::Status) -> text_input::Style {
    let palette = theme.palette();
    text_input::Style {
        background: Color::TRANSPARENT.into(),
        border: Border::default(),
        icon: palette.text.scale_alpha(0.55),
        placeholder: palette.text.scale_alpha(0.4),
        value: palette.text,
        selection: palette.primary.scale_alpha(0.35),
    }
}

/// An agent on the connect screen: a round-cornered tile, outlined in the
/// accent once connected.
pub fn tile(theme: &Theme, connected: bool) -> container::Style {
    let palette = theme.palette();
    container::Style {
        background: Some(Color::WHITE.scale_alpha(veil(theme)).into()),
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

#[derive(Debug, Clone, Copy)]
pub enum Icon {
    Sun,
    Moon,
    Gear,
    Back,
}

/// A line icon in the text color that turns pink on hover.
pub fn icon(icon: Icon) -> svg::Svg<'static> {
    let bytes: &'static [u8] = match icon {
        Icon::Sun => include_bytes!("../../assets/icons/sun.svg"),
        Icon::Moon => include_bytes!("../../assets/icons/moon.svg"),
        Icon::Gear => include_bytes!("../../assets/icons/gear.svg"),
        Icon::Back => include_bytes!("../../assets/icons/back.svg"),
    };
    svg(svg::Handle::from_memory(bytes))
        .width(24)
        .height(24)
        .style(|theme: &Theme, status| {
            let palette = theme.palette();
            svg::Style {
                color: Some(match status {
                    svg::Status::Hovered => palette.primary,
                    svg::Status::Idle => palette.text.scale_alpha(0.65),
                }),
            }
        })
}

pub fn logo(integration: &Integration, size: f32) -> svg::Svg<'static> {
    let bytes: &'static [u8] = match integration {
        Integration::ClaudeCode(_) => include_bytes!("../../assets/claude-code.svg"),
        Integration::OpenCode(_) => include_bytes!("../../assets/opencode.svg"),
        Integration::Codex(_) => include_bytes!("../../assets/codex.svg"),
    };
    svg(svg::Handle::from_memory(bytes))
        .width(size)
        .height(size)
}
