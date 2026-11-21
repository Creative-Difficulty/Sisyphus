//! The one-time question on Wayland: may Sisyphus see which app is in
//! front? Wayland keeps windows private, so we ask and say exactly what
//! happens. The answer is saved as `Config::focus_allowed` and can be
//! changed in settings.

use iced::widget::{button, center, column, container, row, text};
use iced::{Center, Element};

use super::{Message, style};

pub fn view() -> Element<'static, Message> {
    let line = |s: &'static str| text(s).size(15).style(style::muted).center();
    let card = column![
        text("See which app you're in?").size(24).font(style::BOLD),
        column![
            line("Wayland keeps windows private, so Sisyphus asks first."),
            line(
                "Every few seconds it checks which app is in front and keeps \
                 only its kind: your agent, a terminal, an editor, a browser \
                 or something else."
            ),
            line(
                "It never reads window titles, your screen or what you type, \
                 and the app's name never leaves this computer. No admin \
                 rights needed."
            ),
        ]
        .spacing(10)
        .align_x(Center),
        row![
            button(text("Not now").size(16))
                .padding([10, 20])
                .style(button::text)
                .on_press(Message::AllowFocus(false)),
            button(text("Allow").size(16))
                .padding([10, 36])
                .style(style::rounded(style::pink))
                .on_press(Message::AllowFocus(true)),
        ]
        .spacing(12)
        .align_y(Center),
    ]
    .spacing(22)
    .align_x(Center);

    center(
        container(card)
            .max_width(480)
            .padding([32, 36])
            .style(style::popup),
    )
    .padding(24)
    .style(style::backdrop)
    .into()
}
