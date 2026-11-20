//! Sisyphus GUI: pick which AI agents to connect.

use iced::widget::{button, center, column, container, row, svg, text};
use iced::{Border, Center, Element, Fill, Theme};
use sisyphus_client::agents::Integration;
use sisyphus_client::config::Config;

fn main() -> iced::Result {
    iced::application(App::new, App::update, App::view)
        .title("Sisyphus")
        .window_size((520.0, 320.0))
        .centered()
        .run()
}

struct App {
    config: Config,
    /// Last error per integration, shown under its card.
    errors: Vec<Option<String>>,
}

#[derive(Debug, Clone)]
enum Message {
    /// Connect or disconnect the integration at this index of
    /// [`Integration::ALL`].
    Toggle(usize),
}

impl App {
    fn new() -> Self {
        Self {
            config: Config::load(),
            errors: vec![None; Integration::ALL.len()],
        }
    }

    fn update(&mut self, message: Message) {
        let Message::Toggle(index) = message;
        let integration = &Integration::ALL[index];
        let id = integration.id();
        let connect = !self.config.is_connected(id);

        // Integrations that need something set up in the agent do it here.
        if connect
            && !integration.is_installed()
            && let Err(e) = integration.install()
        {
            self.errors[index] = Some(e);
            return;
        }

        let mut config = self.config.clone();
        config.set_connected(id, connect);
        match config.save() {
            Ok(()) => {
                self.config = config;
                self.errors[index] = None;
            }
            Err(e) => self.errors[index] = Some(format!("Could not save: {e}")),
        }
    }

    fn view(&self) -> Element<'_, Message> {
        let cards = Integration::ALL
            .iter()
            .enumerate()
            .map(|(index, integration)| self.card(index, integration));
        center(row(cards).spacing(24)).padding(24).into()
    }

    fn card(&self, index: usize, integration: &Integration) -> Element<'_, Message> {
        let connected = self.config.is_connected(integration.id());

        let (label, style): (_, fn(&Theme, button::Status) -> button::Style) = if connected {
            ("Disconnect", button::secondary)
        } else {
            ("Connect", button::primary)
        };
        let action = button(text(label).width(Fill).center()).style(move |theme, status| {
            let mut s = style(theme, status);
            s.border.radius = 6.0.into();
            s
        });

        let mut content = column![
            svg(svg::Handle::from_memory(logo(integration)))
                .width(64)
                .height(64),
            text(integration.name()).size(18),
            action.width(Fill).on_press(Message::Toggle(index)),
        ]
        .spacing(16)
        .align_x(Center);

        if let Some(error) = &self.errors[index] {
            content = content.push(text(error).size(12).style(text::danger));
        }

        container(content)
            .width(200)
            .padding(24)
            .style(move |theme: &Theme| card_style(theme, connected))
            .into()
    }
}

/// Rounded card; a green border marks a connected agent.
fn card_style(theme: &Theme, connected: bool) -> container::Style {
    let palette = theme.extended_palette();
    container::Style {
        background: Some(palette.background.weak.color.into()),
        border: Border {
            width: if connected { 2.0 } else { 1.0 },
            radius: 12.0.into(),
            color: if connected {
                palette.success.base.color
            } else {
                palette.background.strong.color
            },
        },
        ..container::Style::default()
    }
}

fn logo(integration: &Integration) -> &'static [u8] {
    match integration {
        Integration::ClaudeCode(_) => include_bytes!("../assets/claude-code.svg"),
        Integration::OpenCode(_) => include_bytes!("../assets/opencode.svg"),
    }
}
