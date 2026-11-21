//! The client's window.
//!
//! Screens:
//! - first start ([`onboarding`]): log in with Hack Club, connect agents.
//!   The window resumes at whichever step isn't done yet.
//! - on Linux Wayland without the input helper, a popup to set it up
//!   ([`input_helper`]); it can't be skipped
//! - the dashboard ([`sessions`]), refreshed every few seconds
//! - settings ([`settings`], gear icon): account and agents as a compact list

use std::time::Duration;

use iced::futures::{SinkExt, Stream, StreamExt};
use iced::theme::Mode;
use iced::widget::{button, column, row, space};
use iced::{Element, Fill, Subscription, Task, Theme};

use crate::agents::Integration;
use crate::config::Config;

mod debug;
mod input_helper;
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
    /// Today's credited time and agent time from the server; `None` until
    /// it answered.
    today: Option<sisyphus_common::api::Today>,
    /// Logging out on the server is under way.
    logout_pending: bool,
    /// Logging out on the server failed.
    logout_error: Option<String>,
    /// Records and uploads heartbeats while the window is open. `None` if
    /// another Sisyphus already does (`tracker_error` says why).
    tracker: Option<crate::tracker::Tracker>,
    tracker_error: Option<String>,
    /// When Ctrl+D was last pressed; a second press soon after toggles the
    /// debug screen.
    debug_key_at: Option<std::time::Instant>,
    input_helper_pending: bool,
    input_helper_error: Option<String>,
    /// For the settings row: whether the input helper answers and whether
    /// it's installed. `None` where it doesn't apply (not Linux, or X11
    /// without it installed).
    input_helper_running: Option<(bool, bool)>,
}

/// What the input helper can be asked to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HelperAction {
    Install,
    Restart,
    Uninstall,
}

enum Screen {
    /// First start, step 1: log in with Hack Club.
    Login,
    /// First start, step 2: connect agents.
    ConnectAgents,
    /// Linux Wayland without the input helper: ask to set it up.
    InputHelper,
    /// Account and agents, from the gear icon.
    Settings,
    /// Live attention and tracker state, from Ctrl+D pressed twice.
    Debug,
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
    /// Log this device out on the server, then forget the account.
    Logout,
    /// The server answered the logout.
    LogoutFinished(Result<(), String>),
    /// Today's credited time and agent time from the server.
    TodayLoaded(Result<sisyphus_common::api::Today, String>),
    /// Next first-start step.
    Continue,
    /// The server address field in settings changed.
    ServerInput(String),
    /// Save the typed server address.
    SaveServer,
    /// Go back to the default server.
    ResetServer,
    /// Install, restart or uninstall the input helper (polkit asks for the
    /// password).
    InputHelper(HelperAction),
    InputHelperFinished(HelperAction, Result<(), String>),
    /// Ctrl+D was pressed.
    DebugKey,
    /// Redraw the debug screen.
    DebugTick,
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
            today: None,
            logout_pending: false,
            logout_error: None,
            tracker: None,
            tracker_error: None,
            debug_key_at: None,
            input_helper_pending: false,
            input_helper_error: None,
            input_helper_running: None,
        };
        match crate::tracker::Tracker::start(false) {
            Ok(tracker) => app.tracker = Some(tracker),
            Err(e) => {
                eprintln!("not tracking: {e}");
                app.tracker_error = Some(e);
            }
        }
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
                self.input_helper_error = None;
                self.input_helper_running = input_helper_status();
                Task::none()
            }
            Message::ShowSessions => self.show_sessions(),
            Message::DebugKey => {
                let now = std::time::Instant::now();
                let second = self
                    .debug_key_at
                    .is_some_and(|t| now.duration_since(t) < DEBUG_KEY_WINDOW);
                if !second {
                    self.debug_key_at = Some(now);
                    return Task::none();
                }
                self.debug_key_at = None;
                if matches!(self.screen, Screen::Debug) {
                    self.resume()
                } else {
                    self.screen = Screen::Debug;
                    Task::none()
                }
            }
            Message::DebugTick => Task::none(),
            Message::SessionsLoaded(list) => {
                if let Screen::Sessions(sessions) = &mut self.screen {
                    *sessions = Some(list);
                }
                Task::none()
            }
            Message::Refresh => match self.screen {
                Screen::Sessions(_) => Task::batch([self.load_sessions(), self.load_today()]),
                _ => Task::none(),
            },
            Message::TodayLoaded(result) => {
                // On failure keep the last value; the next refresh retries.
                if let Ok(today) = result {
                    self.today = Some(today);
                }
                Task::none()
            }
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
                // The device stays able to upload until the server unlinks
                // it, so only forget the account once the server agreed.
                self.logout_pending = true;
                self.logout_error = None;
                let server_url = self.config.server_url();
                Task::perform(
                    on_thread(move || crate::server::logout(&server_url)),
                    Message::LogoutFinished,
                )
            }
            Message::LogoutFinished(result) => {
                self.logout_pending = false;
                if let Err(e) = result {
                    self.logout_error = Some(e);
                    return Task::none();
                }
                let mut config = self.config.clone();
                config.account = None;
                if let Err(e) = config.save() {
                    self.login_error = Some(format!("Could not save: {e}"));
                }
                self.config = config;
                self.today = None;
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
            Message::InputHelper(action) => {
                self.input_helper_pending = true;
                self.input_helper_error = None;
                input_helper_task(action)
            }
            Message::InputHelperFinished(action, result) => {
                self.input_helper_pending = false;
                self.input_helper_running = input_helper_status();
                if let Err(e) = result {
                    self.input_helper_error = Some(e);
                    return Task::none();
                }
                // Installing continues from the popup; after uninstalling on
                // Wayland the popup is due again.
                if action == HelperAction::Install
                    || !matches!(self.screen, Screen::Settings)
                    || crate::activity::input_helper_missing()
                {
                    self.resume()
                } else {
                    Task::none()
                }
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
            Screen::InputHelper => input_helper::view(
                self.input_helper_pending,
                self.input_helper_error.as_deref(),
            ),
            Screen::Settings => settings::view(
                &self.config,
                &self.errors,
                settings::ServerState {
                    input: &self.server_input,
                    error: self.server_error.as_deref(),
                },
                settings::LogoutState {
                    pending: self.logout_pending,
                    error: self.logout_error.as_deref(),
                },
                self.input_helper_running
                    .map(|(running, installed)| settings::HelperState {
                        running,
                        installed,
                        pending: self.input_helper_pending,
                        error: self.input_helper_error.as_deref(),
                    }),
            ),
            Screen::Sessions(list) => sessions::view(list.as_deref(), self.today),
            Screen::Debug => debug::view(
                self.tracker.as_ref().map(crate::tracker::Tracker::debug),
                self.tracker_error.as_deref(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_millis() as u64),
            ),
        };
        // Top left: settings from the dashboard, back from settings.
        // Nothing during first start. Top right: light/dark.
        let nav: Element<'_, Message> = match self.screen {
            Screen::Sessions(_) => icon_button(style::Icon::Gear, Message::ShowSettings),
            Screen::Settings => icon_button(style::Icon::Back, Message::ShowSessions),
            Screen::Debug => icon_button(style::Icon::Back, Message::Continue),
            Screen::Login | Screen::ConnectAgents | Screen::InputHelper => space().into(),
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
        let keys = iced::keyboard::listen().filter_map(debug_key);
        match self.screen {
            Screen::Sessions(_) => {
                Subscription::batch([system, keys, Subscription::run(refresh_ticks)])
            }
            Screen::Debug => Subscription::batch([system, keys, Subscription::run(debug_ticks)]),
            _ => Subscription::batch([system, keys]),
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
        } else if crate::activity::input_helper_missing() {
            self.screen = Screen::InputHelper;
            Task::none()
        } else {
            self.show_sessions()
        }
    }

    /// Switch to the session list and load it in the background.
    fn show_sessions(&mut self) -> Task<Message> {
        self.screen = Screen::Sessions(None);
        Task::batch([self.load_sessions(), self.load_today()])
    }

    /// Ask the server for today's credited time, when logged in.
    fn load_today(&self) -> Task<Message> {
        if self.config.account().is_none() {
            return Task::none();
        }
        let server_url = self.config.server_url();
        Task::perform(
            on_thread(move || crate::server::today(&server_url)),
            Message::TodayLoaded,
        )
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

/// The second Ctrl+D must come this soon after the first.
const DEBUG_KEY_WINDOW: Duration = Duration::from_millis(800);

/// Ctrl+D (not a held-down repeat).
fn debug_key(event: iced::keyboard::Event) -> Option<Message> {
    use iced::keyboard::{Event, Key};
    match event {
        Event::KeyPressed {
            key: Key::Character(c),
            modifiers,
            repeat: false,
            ..
        } if modifiers.control() && c.eq_ignore_ascii_case("d") => Some(Message::DebugKey),
        _ => None,
    }
}

/// A `DebugTick` every second while the debug screen is open.
fn debug_ticks() -> impl Stream<Item = Message> {
    iced::stream::channel(1, async move |mut output| {
        let (tx, mut rx) = iced::futures::channel::mpsc::unbounded();
        std::thread::spawn(move || {
            while !tx.is_closed() {
                std::thread::sleep(Duration::from_secs(1));
                if tx.unbounded_send(()).is_err() {
                    break;
                }
            }
        });
        while rx.next().await.is_some() {
            if output.send(Message::DebugTick).await.is_err() {
                break;
            }
        }
    })
}

/// Run an input helper action on a worker thread (pkexec waits for the
/// password).
fn input_helper_task(action: HelperAction) -> Task<Message> {
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        use crate::input_helper::{Action, run_as_admin};
        let admin = match action {
            HelperAction::Install => Action::Install,
            HelperAction::Restart => Action::Restart,
            HelperAction::Uninstall => Action::Uninstall,
        };
        Task::perform(on_thread(move || run_as_admin(admin)), move |result| {
            Message::InputHelperFinished(action, result)
        })
    }
    #[cfg(not(all(unix, not(target_os = "macos"))))]
    {
        Task::done(Message::InputHelperFinished(action, Ok(())))
    }
}

/// Whether the input helper answers, where it applies: on Linux Wayland,
/// or wherever it's installed.
fn input_helper_status() -> Option<(bool, bool)> {
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        use crate::input_helper::{installed, running};
        (crate::activity::linux::is_wayland() || installed()).then(|| (running(), installed()))
    }
    #[cfg(not(all(unix, not(target_os = "macos"))))]
    {
        None
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use iced::keyboard::{Event, Key, Location, Modifiers, key};

    fn press(c: &str, modifiers: Modifiers, repeat: bool) -> Event {
        Event::KeyPressed {
            key: Key::Character(c.into()),
            modified_key: Key::Character(c.into()),
            physical_key: key::Physical::Code(key::Code::KeyD),
            location: Location::Standard,
            modifiers,
            text: None,
            repeat,
        }
    }

    #[test]
    fn ctrl_d_is_the_debug_key() {
        assert!(matches!(
            debug_key(press("d", Modifiers::CTRL, false)),
            Some(Message::DebugKey)
        ));
        assert!(debug_key(press("d", Modifiers::empty(), false)).is_none());
        assert!(debug_key(press("d", Modifiers::CTRL, true)).is_none());
        assert!(debug_key(press("x", Modifiers::CTRL, false)).is_none());
    }
}
