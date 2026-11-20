//! The client's window.
//!
//! Screens: connect agents ([`agents`]) → recent sessions ([`sessions`]).
//! The window opens on the sessions list if an agent is already connected.

use iced::{Element, Task};

use crate::agents::Integration;
use crate::config::Config;

mod agents;
mod sessions;
mod style;

use sessions::SessionSummary;

/// Open the window. Blocks until it is closed.
pub fn run() -> iced::Result {
    iced::application(App::new, App::update, App::view)
        .title("Sisyphus")
        .window_size((720.0, 560.0))
        .centered()
        .run()
}

struct App {
    config: Config,
    screen: Screen,
    /// Last error per integration, shown under its card.
    errors: Vec<Option<String>>,
}

enum Screen {
    Agents,
    /// `None` while loading.
    Sessions(Option<Vec<SessionSummary>>),
}

#[derive(Debug, Clone)]
pub enum Message {
    /// Connect or disconnect `Integration::ALL[index]`.
    Toggle(usize),
    ShowAgents,
    ShowSessions,
    SessionsLoaded(Vec<SessionSummary>),
    /// Open the session at this index of the list.
    Open(usize),
}

impl App {
    fn new() -> (Self, Task<Message>) {
        let mut app = Self {
            config: Config::load(),
            screen: Screen::Agents,
            errors: vec![None; Integration::ALL.len()],
        };
        let task = if app.config.connected_agents.is_empty() {
            Task::none()
        } else {
            app.show_sessions()
        };
        (app, task)
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Toggle(index) => {
                self.errors[index] = agents::toggle(&mut self.config, index).err();
                Task::none()
            }
            Message::ShowAgents => {
                self.screen = Screen::Agents;
                Task::none()
            }
            Message::ShowSessions => self.show_sessions(),
            Message::SessionsLoaded(list) => {
                if let Screen::Sessions(sessions) = &mut self.screen {
                    *sessions = Some(list);
                }
                Task::none()
            }
            Message::Open(_) => Task::none(),
        }
    }

    fn view(&self) -> Element<'_, Message> {
        match &self.screen {
            Screen::Agents => agents::view(&self.config, &self.errors),
            Screen::Sessions(list) => sessions::view(list.as_deref()),
        }
    }

    /// Switch to the session list and load it in the background.
    fn show_sessions(&mut self) -> Task<Message> {
        self.screen = Screen::Sessions(None);
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
