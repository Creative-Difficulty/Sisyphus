//! Recent sessions of all connected agents, newest first, and whether
//! each is running.

use std::time::{SystemTime, UNIX_EPOCH};

use iced::widget::{button, center, column, container, row, rule, scrollable, text};
use iced::{Center, Element, Fill};

use super::{Message, style};
use crate::agents::{AgentInstance, Integration};

/// How many sessions the list shows.
const LIMIT: usize = 100;

/// What the list needs to show and open a session. Plain data, so it can
/// be built on a worker thread.
#[derive(Debug, Clone)]
pub struct SessionSummary {
    /// Index into [`Integration::ALL`].
    pub agent: usize,
    pub id: String,
    pub title: String,
    /// Name of the project folder.
    pub project: String,
    pub last_activity_ms: Option<u64>,
    /// The agent is working on a turn right now.
    pub running: bool,
}

/// Sessions of the given agents, most recently active first. Blocking.
pub fn load(agents: &[usize]) -> Vec<SessionSummary> {
    let mut sessions: Vec<SessionSummary> = agents
        .iter()
        .flat_map(|&agent| {
            Integration::ALL[agent]
                .instances()
                .unwrap_or_default()
                .into_iter()
                .map(move |i| summary(agent, i.as_ref()))
                .collect::<Vec<_>>()
        })
        .collect();
    sessions.sort_by_key(|s| std::cmp::Reverse(s.last_activity_ms));
    sessions.truncate(LIMIT);
    sessions
}

fn summary(agent: usize, instance: &dyn AgentInstance) -> SessionSummary {
    let project = instance
        .get_project_path()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_default();
    SessionSummary {
        agent,
        id: instance.get_uuid(),
        title: instance.get_title(),
        project,
        last_activity_ms: instance.get_last_activity_ms(),
        running: instance.is_running(),
    }
}

/// `None` while loading.
pub fn view(sessions: Option<&[SessionSummary]>) -> Element<'_, Message> {
    let header = row![
        text("Sessions").size(34).font(style::BOLD).width(Fill),
        button(text("Agents").size(16))
            .padding(0)
            .style(style::link)
            .on_press(Message::ShowAgents),
    ]
    .align_y(Center);

    let quiet =
        |label| -> Element<'_, Message> { center(text(label).size(13).style(style::muted)).into() };
    let body = match sessions {
        None => quiet("loading"),
        Some([]) => quiet("no sessions yet"),
        Some(sessions) => {
            let mut ledger = column![rule::horizontal(1).style(style::rule)];
            for session in sessions {
                ledger = ledger
                    .push(entry(session))
                    .push(rule::horizontal(1).style(style::rule));
            }
            scrollable(ledger.padding(iced::Padding::ZERO.right(16)))
                .direction(scrollable::Direction::Vertical(
                    scrollable::Scrollbar::new().width(2).scroller_width(2),
                ))
                .height(Fill)
                .into()
        }
    };

    container(container(column![header, body].spacing(28)).max_width(760))
        .padding(iced::Padding::from([0, 40]).bottom(32))
        .center_x(Fill)
        .into()
}

/// One ledger entry: age, agent, title, project.
fn entry(session: &SessionSummary) -> Element<'_, Message> {
    // A running session's age reads "working", and an accent mark sits at
    // its edge.
    let running = session.running;
    let age = if running {
        text("working").size(14).style(style::accent)
    } else {
        let age = session.last_activity_ms.map(age).unwrap_or_default();
        text(age).size(14).style(style::muted)
    };

    row![
        container(text(""))
            .width(2)
            .height(Fill)
            .style(move |theme| style::mark(theme, running)),
        age.width(64),
        style::logo(&Integration::ALL[session.agent], 22.0),
        text(&session.title)
            .size(17)
            .width(Fill)
            .wrapping(text::Wrapping::None),
        text(&session.project)
            .size(14)
            .style(style::muted)
            .wrapping(text::Wrapping::None),
    ]
    .spacing(14)
    .height(54)
    .align_y(Center)
    .into()
}

/// Compact age, like a log timestamp: "now", "4m", "2h", "3d", "5w".
fn age(time_ms: u64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(time_ms);
    let minutes = now.saturating_sub(time_ms) / 60_000;
    match minutes {
        0 => "now".to_string(),
        1..60 => format!("{minutes}m"),
        60..1440 => format!("{}h", minutes / 60),
        1440..10080 => format!("{}d", minutes / 1440),
        _ => format!("{}w", minutes / 10080),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_ages() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        assert_eq!(age(now), "now");
        assert_eq!(age(now - 5 * 60_000), "5m");
        assert_eq!(age(now - 3 * 3_600_000), "3h");
        assert_eq!(age(now - 30 * 3_600_000), "1d");
        assert_eq!(age(now - 20 * 86_400_000), "2w");
    }
}
