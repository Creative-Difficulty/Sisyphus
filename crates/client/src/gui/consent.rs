//! First start, step 3: allow Sisyphus to read activity. Says plainly that
//! it is needed to count time, and exactly what is and isn't shared. The
//! answer is saved as `Config::activity_allowed` and can be changed in
//! settings.

use iced::widget::{button, center, column, container, text};
use iced::{Center, Element, Fill};

use super::{Message, style};

pub fn view() -> Element<'static, Message> {
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

    center(
        container(
            column![
                text("Let Sisyphus see you working")
                    .size(30)
                    .font(style::BOLD),
                text("Required to count your time. Without it, Sisyphus can lose time.")
                    .size(16)
                    .style(style::accent)
                    .center(),
                column![
                    card(
                        "The only activity shared",
                        "The kind of app in front (your agent, a terminal, an \
                         editor, a browser or something else) and whether you \
                         were active, for every 10 seconds. Nothing more.",
                    ),
                    card(
                        "Never recorded",
                        "What you type, which keys you press, your mouse \
                         movements, app names, window titles or your screen.",
                    ),
                ]
                .spacing(12),
                column![
                    button(text("Allow").size(17))
                        .padding([12, 48])
                        .style(style::rounded(style::pink))
                        .on_press(Message::AllowActivity(true)),
                    button(text("Not now").size(14))
                        .padding([6, 12])
                        .style(button::text)
                        .on_press(Message::AllowActivity(false)),
                ]
                .spacing(6)
                .align_x(Center),
            ]
            .spacing(22)
            .align_x(Center),
        )
        .max_width(520),
    )
    .padding(24)
    .into()
}
