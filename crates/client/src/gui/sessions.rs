//! Recent sessions of all connected agents, newest first, and whether
//! each is running.

use std::time::{SystemTime, UNIX_EPOCH};

use iced::widget::{center, column, container, row, scrollable, text};
use iced::{Center, Element, Fill, FillPortion};

use super::{Message, style};
use crate::agents::{AgentInstance, Integration};
use sisyphus_common::api::Today;

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
/// `today`: today's credited time and agent time from the server, if it
/// answered.
pub fn view(sessions: Option<&[SessionSummary]>, today: Option<Today>) -> Element<'_, Message> {
    let quiet =
        |label| -> Element<'_, Message> { center(text(label).size(13).style(style::muted)).into() };
    let body = match sessions {
        None => quiet("loading"),
        Some([]) => quiet("no sessions yet"),
        Some(sessions) => {
            let bubbles = column(sessions.iter().map(entry)).spacing(10);
            scrollable(bubbles.padding(iced::Padding::ZERO.right(16)))
                .direction(scrollable::Direction::Vertical(
                    scrollable::Scrollbar::new().width(8).scroller_width(8),
                ))
                .style(style::scrollbar)
                .height(Fill)
                .into()
        }
    };

    // Top half: the list. Bottom half: today's figures.
    let list = column![text("Sessions").size(26).font(style::BOLD), body]
        .spacing(14)
        .height(FillPortion(1));

    container(container(column![list, stats(today.unwrap_or_default())].spacing(24)).max_width(760))
        .padding(iced::Padding::from([0, 40]).bottom(32))
        .center_x(Fill)
        .into()
}

/// Today's recorded time and agent time, from the server, and a few
/// figures. Prompts and sessions are placeholders (zero) for now.
fn stats<'a>(today: Today) -> Element<'a, Message> {
    let agent = short_duration(today.agent_ms);
    let minutes = today.tracked_ms / 60_000;
    let today = format!("{}h {:02}m", minutes / 60, minutes % 60);
    let figure = |value: String, label: &'a str| {
        column![
            text(value).size(30).font(style::BOLD),
            text(label).size(14).style(style::muted),
        ]
        .align_x(Center)
    };
    center(
        column![
            text("Recorded today").size(16).style(style::muted),
            text(today).size(84).font(style::BOLD).style(style::accent),
            row![
                figure("0".to_string(), "prompts"),
                figure("0".to_string(), "sessions"),
                figure(agent, "agent time"),
            ]
            .spacing(56),
        ]
        .spacing(6)
        .align_x(Center),
    )
    .height(FillPortion(1))
    .into()
}

/// "42m", or "1h 05m" from an hour on.
pub fn short_duration(ms: u64) -> String {
    let minutes = ms / 60_000;
    if minutes < 60 {
        format!("{minutes}m")
    } else {
        format!("{}h {:02}m", minutes / 60, minutes % 60)
    }
}

/// One session as a bubble: agent, title, project, age. A running
/// session's age reads "working" and its bubble is a deeper pink.
fn entry(session: &SessionSummary) -> Element<'_, Message> {
    let running = session.running;
    let age = if running {
        text("working").size(14).style(style::accent)
    } else {
        let age = session.last_activity_ms.map(age).unwrap_or_default();
        text(age).size(14).style(style::muted)
    };

    let mut about = column![text(&session.title).size(16).wrapping(text::Wrapping::None)];
    if !session.project.is_empty() {
        about = about.push(text(&session.project).size(13).style(style::muted));
    }

    container(
        row![
            style::logo(&Integration::ALL[session.agent], 26.0),
            about.width(Fill),
            age,
        ]
        .spacing(14)
        .align_y(Center),
    )
    .padding([10, 22])
    .style(move |theme| style::bubble(theme, running))
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
