//! The client's window.
//!
//! Screens:
//! - first start ([`onboarding`]): log in with Hack Club, then connect
//!   agents. The window resumes at whichever step isn't done yet.
//! - the dashboard ([`sessions`]), refreshed every few seconds
//! - settings ([`settings`], gear icon): account and agents as a compact list
//! - on Wayland, a one-time popup ([`consent`]) asking to read which app is
//!   in front

use std::time::Duration;

use iced::futures::{SinkExt, Stream, StreamExt};
use iced::theme::Mode;
use iced::widget::{button, column, opaque, row, space, stack};
use iced::{Element, Fill, Subscription, Task, Theme};

use crate::activity;
use crate::agents::Integration;
use crate::config::Config;

mod consent;
mod onboarding;
mod sessions;
mod settings;
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
    /// A login in the browser is under way.
    login_pending: bool,
    login_error: Option<String>,
    /// The server address as typed in settings, not yet saved.
    server_input: String,
    server_error: Option<String>,
}

enum Screen {
    /// First start, step 1: log in with Hack Club.
    Login,
    /// First start, step 2: connect agents.
    ConnectAgents,
    /// Account and agents, from the gear icon.
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
    /// Log in with Hack Club in the browser.
    Login,
    /// The browser login ended: the account name, or what went wrong.
    LoginFinished(Result<Option<String>, String>),
    /// Forget the account on this device.
    Logout,
    /// Next first-start step.
    Continue,
    /// The server address field in settings changed.
    ServerInput(String),
    /// Save the typed server address.
    SaveServer,
    /// Go back to the default server.
    ResetServer,
    /// The user answered whether Sisyphus may read the focused app.
    AllowFocus(bool),
}

impl App {
    fn new() -> (Self, Task<Message>) {
        let mut app = Self {
            config: Config::load(),
            system_dark: false,
            screen: Screen::Login,
            errors: vec![None; Integration::ALL.len()],
            login_pending: false,
            login_error: None,
            server_input: String::new(),
            server_error: None,
        };
        app.server_input = app.config.server_url();
        let task = app.resume();
        let theme = iced::system::theme().map(Message::SystemTheme);
        (app, Task::batch([task, theme]))
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Toggle(index) => {
                self.errors[index] = settings::toggle(&mut self.config, index).err();
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
            Message::Login => {
                self.login_pending = true;
                self.login_error = None;
                let server_url = self.config.server_url();
                Task::perform(
                    on_thread(move || crate::login::login(&server_url)),
                    Message::LoginFinished,
                )
            }
            Message::LoginFinished(result) => {
                self.login_pending = false;
                match result {
                    Ok(name) => {
                        let mut config = self.config.clone();
                        config.account = Some(crate::config::Account {
                            name,
                            server: Some(config.server_url()),
                        });
                        if let Err(e) = config.save() {
                            self.login_error = Some(format!("Could not save: {e}"));
                        }
                        self.config = config;
                    }
                    Err(e) => self.login_error = Some(e),
                }
                Task::none()
            }
            Message::Logout => {
                let mut config = self.config.clone();
                config.account = None;
                if let Err(e) = config.save() {
                    self.login_error = Some(format!("Could not save: {e}"));
                }
                self.config = config;
                self.screen = Screen::Login;
                Task::none()
            }
            Message::Continue => self.resume(),
            Message::ServerInput(value) => {
                self.server_input = value;
                self.server_error = None;
                Task::none()
            }
            Message::SaveServer => {
                let url = self.server_input.trim().trim_end_matches('/').to_string();
                if !crate::login::is_safe_server(&url) {
                    self.server_error = Some(
                        "Use an https:// address (http only works for localhost).".to_string(),
                    );
                    return Task::none();
                }
                let server_url = (url != crate::config::DEFAULT_SERVER_URL).then_some(url);
                self.set_server(server_url)
            }
            Message::ResetServer => self.set_server(None),
            Message::AllowFocus(allowed) => {
                let mut config = self.config.clone();
                config.focus_allowed = Some(allowed);
                // If saving fails the answer still applies until restart.
                let _ = config.save();
                self.config = config;
                Task::none()
            }
        }
    }

    fn view(&self) -> Element<'_, Message> {
        let screen = match &self.screen {
            Screen::Login => onboarding::login(
                &self.config,
                onboarding::LoginState {
                    pending: self.login_pending,
                    error: self.login_error.as_deref(),
                },
            ),
            Screen::ConnectAgents => onboarding::agents(&self.config, &self.errors),
            Screen::Settings => settings::view(
                &self.config,
                &self.errors,
                settings::ServerState {
                    input: &self.server_input,
                    error: self.server_error.as_deref(),
                },
            ),
            Screen::Sessions(list) => sessions::view(list.as_deref()),
        };
        // Top left: settings from the dashboard, back from settings.
        // Nothing during first start. Top right: light/dark.
        let nav: Element<'_, Message> = match self.screen {
            Screen::Sessions(_) => icon_button(style::Icon::Gear, Message::ShowSettings),
            Screen::Settings => icon_button(style::Icon::Back, Message::ShowSessions),
            Screen::Login | Screen::ConnectAgents => space().into(),
        };
        let theme_icon = if self.is_dark() {
            style::Icon::Sun
        } else {
            style::Icon::Moon
        };
        let window = column![
            row![
                nav,
                space().width(Fill),
                icon_button(theme_icon, Message::ToggleDark)
            ]
            .padding([14, 20]),
            screen,
        ];
        if self.asks_focus_consent() {
            stack![window, opaque(consent::view())].into()
        } else {
            window.into()
        }
    }

    /// Ask once, on the dashboard, where the OS needs the user's OK to see
    /// the focused app.
    fn asks_focus_consent(&self) -> bool {
        matches!(self.screen, Screen::Sessions(_))
            && self.config.focus_allowed.is_none()
            && activity::focus_needs_consent()
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

    /// Switch servers (`None` = the default). The login belongs to the old
    /// server, so without an account on the new one this goes back to the
    /// login step.
    fn set_server(&mut self, server_url: Option<String>) -> Task<Message> {
        let mut config = self.config.clone();
        config.server_url = server_url;
        if let Err(e) = config.save() {
            self.server_error = Some(format!("Could not save: {e}"));
            return Task::none();
        }
        self.config = config;
        self.server_input = self.config.server_url();
        self.server_error = None;
        if self.config.account().is_none() {
            self.screen = Screen::Login;
        }
        Task::none()
    }

    /// Go to the first unfinished first-start step, or the dashboard.
    fn resume(&mut self) -> Task<Message> {
        if self.config.account().is_none() {
            self.screen = Screen::Login;
            Task::none()
        } else if self.config.connected_agents.is_empty() {
            self.screen = Screen::ConnectAgents;
            Task::none()
        } else {
            self.show_sessions()
        }
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
