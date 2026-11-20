//! Recent sessions of all connected agents, newest first.

use std::time::{SystemTime, UNIX_EPOCH};

use iced::widget::{button, center, column, container, row, scrollable, text};
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
    }
}

/// `None` while loading.
pub fn view(sessions: Option<&[SessionSummary]>) -> Element<'_, Message> {
    let header = row![
        text("Recent sessions").size(24).width(Fill),
        button(text("Agents"))
            .style(style::rounded(button::secondary))
            .on_press(Message::ShowAgents),
    ]
    .align_y(Center);

    let body: Element<'_, Message> = match sessions {
        None => center(text("Loading…").style(text::secondary)).into(),
        Some([]) => center(text("No sessions yet").style(text::secondary)).into(),
        Some(sessions) => scrollable(
            column(sessions.iter().enumerate().map(|(i, s)| session_row(i, s)))
                .spacing(8)
                .padding(iced::Padding::ZERO.right(12)),
        )
        .height(Fill)
        .into(),
    };

    container(container(column![header, body].spacing(20)).max_width(720))
        .padding(24)
        .center_x(Fill)
        .into()
}

fn session_row(index: usize, session: &SessionSummary) -> Element<'_, Message> {
    let mut details = vec![];
    if !session.project.is_empty() {
        details.push(session.project.clone());
    }
    if let Some(ms) = session.last_activity_ms {
        details.push(ago(ms));
    }

    let content = row![
        style::logo(&Integration::ALL[session.agent], 32.0),
        column![
            text(&session.title).size(16),
            text(details.join(" · ")).size(13).style(text::secondary),
        ]
        .spacing(2),
    ]
    .spacing(14)
    .align_y(Center);

    button(content)
        .width(Fill)
        .padding([12, 16])
        .style(style::row)
        .on_press(Message::Open(index))
        .into()
}

/// "5 min ago", "3 h ago", "yesterday", ...
fn ago(time_ms: u64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(time_ms);
    let minutes = now.saturating_sub(time_ms) / 60_000;
    match minutes {
        0 => "just now".to_string(),
        1..60 => format!("{minutes} min ago"),
        60..1440 => format!("{} h ago", minutes / 60),
        1440..2880 => "yesterday".to_string(),
        _ => format!("{} days ago", minutes / 1440),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_times() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        assert_eq!(ago(now), "just now");
        assert_eq!(ago(now - 5 * 60_000), "5 min ago");
        assert_eq!(ago(now - 3 * 3_600_000), "3 h ago");
        assert_eq!(ago(now - 30 * 3_600_000), "yesterday");
        assert_eq!(ago(now - 4 * 86_400_000), "4 days ago");
    }
}
