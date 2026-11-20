//! First screen: connect the agents to track.

use iced::widget::{button, center, column, container, row, text};
use iced::{Center, Element, Fill};

use super::{Message, style};
use crate::agents::Integration;
use crate::config::Config;

/// Connect or disconnect `Integration::ALL[index]`. Returns an error to
/// show under its card.
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

pub fn view<'a>(config: &'a Config, errors: &'a [Option<String>]) -> Element<'a, Message> {
    let cards = Integration::ALL
        .iter()
        .enumerate()
        .map(|(index, integration)| card(config, index, integration, errors[index].as_deref()));

    let mut content = column![row(cards).spacing(24)].spacing(32).align_x(Center);

    if !config.connected_agents.is_empty() {
        content = content.push(
            button(text("Continue").center())
                .padding([10, 32])
                .style(style::rounded(style::pink))
                .on_press(Message::ShowSessions),
        );
    }

    center(content).padding(24).into()
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
