//! Settings (gear icon): a compact list of the account, the agents, the
//! focused-app permission (Wayland only) and the server, in the dashboard's
//! bubble style.

use iced::widget::{button, column, container, row, scrollable, text, text_input};
use iced::{Center, Element, Fill};

use super::{Message, style};
use crate::agents::Integration;
use crate::config::{Config, DEFAULT_SERVER_URL};

/// Connect or disconnect `Integration::ALL[index]`. Returns an error to
/// show with it.
pub fn toggle(config: &mut Config, index: usize) -> Result<(), String> {
    let integration = &Integration::ALL[index];
    let id = integration.id();
    let connect = !config.is_connected(id);

    // Integrations that need something set up in the agent do it here.
    if connect && !integration.is_installed() {
        integration.install()?;
    }

    let mut updated = config.clone();
    updated.set_connected(id, connect);
    updated.save().map_err(|e| format!("Could not save: {e}"))?;
    *config = updated;
    Ok(())
}

/// The server address being edited.
pub struct ServerState<'a> {
    pub input: &'a str,
    pub error: Option<&'a str>,
}

pub fn view<'a>(
    config: &'a Config,
    errors: &'a [Option<String>],
    server: ServerState<'a>,
) -> Element<'a, Message> {
    let account_name = config
        .account()
        .and_then(|a| a.name.as_deref())
        .unwrap_or("Logged in");
    let account = bubble(
        row![
            column![
                text(account_name).size(16),
                text("Hack Club").size(13).style(style::muted),
            ]
            .spacing(2)
            .width(Fill),
            quiet_button("Log out", Message::Logout),
        ]
        .align_y(Center),
    );

    let mut agents = column![].spacing(10);
    for (index, integration) in Integration::ALL.iter().enumerate() {
        let connected = config.is_connected(integration.id());
        let action = if connected {
            quiet_button("Disconnect", Message::Toggle(index))
        } else {
            button(text("Connect").size(14))
                .padding([6, 18])
                .style(style::rounded(style::pink))
                .on_press(Message::Toggle(index))
                .into()
        };
        agents = agents.push(bubble(
            row![
                style::logo(integration, 26.0),
                text(integration.name()).size(16).width(Fill),
                action,
            ]
            .spacing(14)
            .align_y(Center),
        ));
        if let Some(error) = &errors[index] {
            agents =
                agents.push(container(text(error).size(13).style(text::danger)).padding([0, 22]));
        }
    }

    let current = config.server_url();
    let changed = server.input.trim().trim_end_matches('/') != current;
    let mut server_row = row![
        text_input(DEFAULT_SERVER_URL, server.input)
            .on_input(Message::ServerInput)
            .on_submit(Message::SaveServer)
            .padding([6, 0])
            .size(15)
            .style(style::input)
            .width(Fill),
    ]
    .spacing(14)
    .align_y(Center);
    if current != DEFAULT_SERVER_URL {
        server_row = server_row.push(quiet_button("Reset", Message::ResetServer));
    }
    server_row = server_row.push(
        button(text("Save").size(14))
            .padding([6, 18])
            .style(style::rounded(style::pink))
            .on_press_maybe(changed.then_some(Message::SaveServer)),
    );
    let mut server_section = column![bubble(server_row)].spacing(8);
    if let Some(error) = server.error {
        server_section = server_section
            .push(container(text(error).size(13).style(text::danger)).padding([0, 22]));
    }

    let mut content = column![
        text("Settings").size(26).font(style::BOLD),
        section("Account", account.into()),
        section("Agents", agents.into()),
    ]
    .spacing(24);
    // Only where the OS needs the user's OK to see the focused app.
    if crate::activity::focus_needs_consent() {
        let allowed = config.focus_allowed == Some(true);
        let action = if allowed {
            quiet_button("Stop", Message::AllowFocus(false))
        } else {
            button(text("Allow").size(14))
                .padding([6, 18])
                .style(style::rounded(style::pink))
                .on_press(Message::AllowFocus(true))
                .into()
        };
        let focus = bubble(
            row![
                column![
                    text("See the app in front").size(16),
                    text("Only its kind, never titles")
                        .size(13)
                        .style(style::muted),
                ]
                .spacing(2)
                .width(Fill),
                action,
            ]
            .align_y(Center),
        );
        content = content.push(section("Privacy", focus.into()));
    }
    content = content.push(section("Server", server_section.into()));

    scrollable(
        container(container(content).max_width(560))
            .padding(iced::Padding::from([0, 40]).bottom(32))
            .center_x(Fill),
    )
    .into()
}

fn section<'a>(label: &'a str, body: Element<'a, Message>) -> Element<'a, Message> {
    column![text(label).size(14).style(style::muted), body]
        .spacing(10)
        .into()
}

/// A row in the same pink bubble as the dashboard's sessions.
fn bubble<'a>(content: impl Into<Element<'a, Message>>) -> container::Container<'a, Message> {
    container(content)
        .width(Fill)
        .padding([10, 22])
        .style(|theme| style::bubble(theme, false))
}

/// A plain text button for secondary actions.
fn quiet_button(label: &str, message: Message) -> Element<'_, Message> {
    button(text(label).size(14))
        .padding([6, 4])
        .style(button::text)
        .on_press(message)
        .into()
}
