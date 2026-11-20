//! The client's window.
//!
//! Screens: the dashboard ([`sessions`]), refreshed every few seconds, and
//! settings, where agents are connected ([`agents`]). The window opens on
//! settings until an agent is connected.
//! The window opens on the sessions list if an agent is already connected.

use std::time::Duration;

use iced::futures::{SinkExt, Stream, StreamExt};
use iced::theme::Mode;
use iced::widget::{button, column, row, space};
use iced::{Element, Fill, Subscription, Task, Theme};

use crate::agents::Integration;
use crate::config::Config;

mod agents;
mod sessions;
mod style;

use sessions::SessionSummary;

/// Open the window. Blocks until it is closed.
pub fn run() -> iced::Result {
    iced::application(App::new, App::update, App::view)
        .subscription(App::subscription)
        .theme(App::theme)
        .font(style::FONTS[0])
        .font(style::FONTS[1])
        .default_font(style::FONT)
        .title("Sisyphus")
        .window_size((720.0, 560.0))
        .centered()
        .run()
}

struct App {
    config: Config,
    /// The OS is in dark mode. Used unless the user picked a mode.
    system_dark: bool,
    screen: Screen,
    /// Last error per integration, shown under its card.
    errors: Vec<Option<String>>,
}

enum Screen {
    /// Connect and disconnect agents.
    Settings,
    /// `None` while loading.
    Sessions(Option<Vec<SessionSummary>>),
}

#[derive(Debug, Clone)]
pub enum Message {
    /// Connect or disconnect `Integration::ALL[index]`.
    Toggle(usize),
    ShowSettings,
    ShowSessions,
    SessionsLoaded(Vec<SessionSummary>),
    /// Time to reload the session list.
    Refresh,
    /// The OS switched between light and dark mode.
    SystemTheme(Mode),
    /// The user switched between light and dark mode.
    ToggleDark,
}

impl App {
    fn new() -> (Self, Task<Message>) {
        let mut app = Self {
            config: Config::load(),
            system_dark: false,
            screen: Screen::Settings,
            errors: vec![None; Integration::ALL.len()],
        };
        let task = if app.config.connected_agents.is_empty() {
            Task::none()
        } else {
            app.show_sessions()
        };
        let theme = iced::system::theme().map(Message::SystemTheme);
        (app, Task::batch([task, theme]))
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Toggle(index) => {
                self.errors[index] = agents::toggle(&mut self.config, index).err();
                Task::none()
            }
            Message::ShowSettings => {
                self.screen = Screen::Settings;
                Task::none()
            }
            Message::ShowSessions => self.show_sessions(),
            Message::SessionsLoaded(list) => {
                if let Screen::Sessions(sessions) = &mut self.screen {
                    *sessions = Some(list);
                }
                Task::none()
            }
            Message::Refresh => match self.screen {
                Screen::Sessions(_) => self.load_sessions(),
                _ => Task::none(),
            },
            Message::SystemTheme(mode) => {
                // `None` means no preference; keep what we have.
                if mode != Mode::None {
                    self.system_dark = mode == Mode::Dark;
                }
                Task::none()
            }
            Message::ToggleDark => {
                let mut config = self.config.clone();
                config.dark_mode = Some(!self.is_dark());
                // If saving fails the switch still applies until restart.
                let _ = config.save();
                self.config = config;
                Task::none()
            }
        }
    }

    fn view(&self) -> Element<'_, Message> {
        let screen = match &self.screen {
            Screen::Settings => agents::view(&self.config, &self.errors),
            Screen::Sessions(list) => sessions::view(list.as_deref()),
        };
        // Top left: settings from the dashboard, back from settings (once
        // there is a dashboard to go back to). Top right: light/dark.
        let nav: Element<'_, Message> = match self.screen {
            Screen::Sessions(_) => icon_button(style::Icon::Gear, Message::ShowSettings),
            Screen::Settings if !self.config.connected_agents.is_empty() => {
                icon_button(style::Icon::Back, Message::ShowSessions)
            }
            Screen::Settings => space().into(),
        };
        let theme_icon = if self.is_dark() {
            style::Icon::Sun
        } else {
            style::Icon::Moon
        };
        column![
            row![
                nav,
                space().width(Fill),
                icon_button(theme_icon, Message::ToggleDark)
            ]
            .padding([14, 20]),
            screen,
        ]
        .into()
    }

    fn subscription(&self) -> Subscription<Message> {
        let system = iced::system::theme_changes().map(Message::SystemTheme);
        match self.screen {
            Screen::Sessions(_) => Subscription::batch([system, Subscription::run(refresh_ticks)]),
            _ => system,
        }
    }

    fn theme(&self) -> Theme {
        style::theme(self.is_dark())
    }

    /// The user's choice if they made one, otherwise the OS setting.
    fn is_dark(&self) -> bool {
        self.config.dark_mode.unwrap_or(self.system_dark)
    }

    /// Switch to the session list and load it in the background.
    fn show_sessions(&mut self) -> Task<Message> {
        self.screen = Screen::Sessions(None);
        self.load_sessions()
    }

    /// Load the session list in the background. The shown list stays until
    /// the new one arrives.
    fn load_sessions(&self) -> Task<Message> {
        let connected: Vec<usize> = Integration::ALL
            .iter()
            .enumerate()
            .filter(|(_, i)| self.config.is_connected(i.id()))
            .map(|(index, _)| index)
            .collect();
        Task::perform(
            on_thread(move || sessions::load(&connected)),
            Message::SessionsLoaded,
        )
    }
}

fn icon_button(icon: style::Icon, message: Message) -> Element<'static, Message> {
    button(style::icon(icon))
        .padding(4)
        .style(|_, _| button::Style::default())
        .on_press(message)
        .into()
}

/// How often the session list reloads, to keep "Running" current.
const REFRESH_INTERVAL: Duration = Duration::from_secs(10);

/// A `Refresh` message every [`REFRESH_INTERVAL`]. The timer thread stops
/// once the subscription is dropped.
fn refresh_ticks() -> impl Stream<Item = Message> {
    iced::stream::channel(1, async move |mut output| {
        let (tx, mut rx) = iced::futures::channel::mpsc::unbounded();
        std::thread::spawn(move || {
            while !tx.is_closed() {
                std::thread::sleep(REFRESH_INTERVAL);
                if tx.unbounded_send(()).is_err() {
                    break;
                }
            }
        });
        while rx.next().await.is_some() {
            if output.send(Message::Refresh).await.is_err() {
                break;
            }
        }
    })
}

/// Run blocking work on its own thread, so the window stays responsive.
fn on_thread<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> impl Future<Output = T> {
    let (tx, rx) = iced::futures::channel::oneshot::channel();
    std::thread::spawn(move || {
        let _ = tx.send(work());
    });
    async move { rx.await.expect("worker thread panicked") }
}
