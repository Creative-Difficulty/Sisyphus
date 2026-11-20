//! First start, in two steps: log in with Hack Club, then connect agents.
//! The window resumes at whichever step isn't done yet.

use iced::widget::{button, center, column, container, row, text};
use iced::{Center, Element, Fill};

use super::{Message, style};
use crate::agents::Integration;
use crate::config::Config;

/// Where the browser login stands.
pub struct LoginState<'a> {
    pub pending: bool,
    pub error: Option<&'a str>,
}

/// Step 1: log in. Once logged in, shows who and a way on.
pub fn login<'a>(config: &'a Config, login: LoginState<'a>) -> Element<'a, Message> {
    let mut content = column![
        text("Sisyphus").size(44).font(style::BOLD),
        text("Time you spend building with AI, counted honestly.").style(style::muted),
    ]
    .spacing(10)
    .align_x(Center);

    let action: Element<'_, Message> = match &config.account {
        Some(account) => column![
            text(account.name.as_deref().unwrap_or("Logged in")).size(20),
            continue_button(true),
        ]
        .spacing(16)
        .align_x(Center)
        .into(),
        None => {
            let label = if login.pending {
                "Waiting for your browser…"
            } else {
                "Log in with Hack Club"
            };
            button(text(label).size(17))
                .padding([12, 32])
                .style(style::rounded(style::pink))
                .on_press_maybe((!login.pending).then_some(Message::Login))
                .into()
        }
    };
    content = content.push(container(action).padding([28, 0]));

    if let Some(error) = login.error {
        content = content.push(text(error).size(13).style(text::danger));
    }
    center(content).padding(24).into()
}

/// Step 2: connect agents. Continue once at least one is connected.
pub fn agents<'a>(config: &'a Config, errors: &'a [Option<String>]) -> Element<'a, Message> {
    let cards = Integration::ALL
        .iter()
        .enumerate()
        .map(|(index, integration)| card(config, index, integration, errors[index].as_deref()));

    center(
        column![
            text("Connect your agents").size(30).font(style::BOLD),
            row(cards).spacing(24),
            continue_button(!config.connected_agents.is_empty()),
        ]
        .spacing(32)
        .align_x(Center),
    )
    .padding(24)
    .into()
}

fn continue_button(enabled: bool) -> Element<'static, Message> {
    button(text("Continue").size(17))
        .padding([12, 40])
        .style(style::rounded(style::pink))
        .on_press_maybe(enabled.then_some(Message::Continue))
        .into()
}

fn card<'a>(
    config: &Config,
    index: usize,
    integration: &Integration,
    error: Option<&'a str>,
) -> Element<'a, Message> {
    let connected = config.is_connected(integration.id());
    let (label, base): (_, fn(&_, _) -> _) = if connected {
        ("Disconnect", button::secondary)
    } else {
        ("Connect", style::pink)
    };

    let mut content = column![
        style::logo(integration, 64.0),
        text(integration.name()).size(22).font(style::BOLD),
        button(text(label).width(Fill).center())
            .width(Fill)
            .style(style::rounded(base))
            .on_press(Message::Toggle(index)),
    ]
    .spacing(16)
    .align_x(Center);

    if let Some(error) = error {
        content = content.push(text(error).size(12).style(text::danger));
    }

    container(content)
        .width(200)
        .padding(24)
        .style(move |theme| style::tile(theme, connected))
        .into()
}
