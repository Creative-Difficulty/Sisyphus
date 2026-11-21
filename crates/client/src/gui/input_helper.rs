//! Linux Wayland only: asks to set up the input helper
//! ([`crate::input_helper`]), saying exactly what it asks for. Shown at
//! start while the helper isn't running. It can't be skipped (decided
//! 2026-11-21): the key and mouse counts are needed.

use iced::widget::{button, column, container, scrollable, text};
use iced::{Center, Element, Fill};

use super::{Message, style};

pub fn view(pending: bool, error: Option<&str>) -> Element<'static, Message> {
    let card = |title: &'static str, body: &'static str| {
        container(
            column![
                text(title).size(17).font(style::BOLD),
                text(body).size(15).style(style::muted),
            ]
            .spacing(6),
        )
        .width(Fill)
        .padding([16, 26])
        .style(style::card)
    };

    let mut content = column![
        text("One more step on your desktop")
            .size(30)
            .font(style::BOLD)
            .center(),
        text(
            "Your desktop (Wayland) doesn't tell apps whether you typed or used the \
             mouse. Sisyphus needs that to count your time, so it asks for your \
             password once to install a small helper."
        )
        .size(16)
        .style(style::accent)
        .center(),
        column![
            card(
                "What it does",
                "It counts how many keys you press and how often you move, click or \
                 scroll, and passes only those numbers to Sisyphus. It never records \
                 which keys you press or what you type.",
            ),
            card(
                "What your password allows",
                "Installing a copy of this app at /usr/local/libexec/sisyphus-input-helper \
                 and the system service sisyphus-input-helper. The service reads your \
                 keyboard and mouse devices (/dev/input). It doesn't run as root: it runs \
                 as its own locked-down user without network access, and only your \
                 account can read its numbers.",
            ),
            card(
                "Removing it",
                "Run: sudo /usr/local/libexec/sisyphus-input-helper input-helper uninstall",
            ),
        ]
        .spacing(12),
        text("Questions or concerns? Contact @simon on Slack.")
            .size(14)
            .style(style::muted)
            .center(),
    ]
    .spacing(22)
    .align_x(Center);

    if let Some(error) = error {
        content = content.push(
            text(error.to_string())
                .size(14)
                .style(text::danger)
                .center(),
        );
    }

    let set_up = button(
        text(if pending {
            "Waiting for your password"
        } else {
            "Set up"
        })
        .size(17),
    )
    .padding([12, 48])
    .style(style::rounded(style::pink))
    .on_press_maybe((!pending).then_some(Message::SetUpInputHelper));
    content = content.push(set_up);

    scrollable(
        container(container(content).max_width(560))
            .padding(iced::Padding::from([24, 40]))
            .center_x(Fill),
    )
    .direction(scrollable::Direction::Vertical(
        scrollable::Scrollbar::new().width(8).scroller_width(8),
    ))
    .style(style::scrollbar)
    .height(Fill)
    .into()
}
