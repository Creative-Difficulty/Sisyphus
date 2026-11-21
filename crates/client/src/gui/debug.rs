//! The debug screen, opened and closed with Ctrl+D pressed twice: the live
//! attention of each followed session (computed with the server's formula),
//! what the tracker reads right now, uploads, and the last heartbeat.

use iced::widget::{column, container, row, scrollable, text};
use iced::{Center, Element, Fill};

use super::{Message, style};
use crate::tracker::{DebugInfo, SessionDebug};

/// `info` is `None` when this window doesn't track (`error` says why).
pub fn view<'a>(
    info: Option<DebugInfo>,
    error: Option<&'a str>,
    now_ms: u64,
) -> Element<'a, Message> {
    let mut content = column![text("Debug").size(26).font(style::BOLD)].spacing(14);

    match info {
        None => {
            content = content.push(card(
                text(format!(
                    "Not tracking in this window: {}",
                    error.unwrap_or("unknown reason")
                ))
                .size(14),
            ));
        }
        Some(info) => {
            content = content.push(heading("Attention"));
            if info.sessions.is_empty() {
                content = content.push(muted("No sessions followed right now"));
            }
            for session in &info.sessions {
                content = content.push(session_bubble(session));
            }

            content = content.push(heading("Now"));
            content = content.push(card(now_rows(&info, now_ms)));

            content = content.push(heading("Last heartbeat"));
            let heartbeat = match &info.last_heartbeat {
                Some(hb) => serde_json::to_string_pretty(hb).unwrap_or_default(),
                None => "None yet. They appear a few seconds after each minute.".to_string(),
            };
            content = content.push(card(text(heartbeat).size(13)));
        }
    }
    content = content.push(muted("Press Ctrl+D twice to close"));

    let page = container(content.padding(iced::Padding::ZERO.right(16)))
        .max_width(760)
        .padding(iced::Padding::from([0, 40]).bottom(32));
    scrollable(container(page).center_x(Fill))
        .direction(scrollable::Direction::Vertical(
            scrollable::Scrollbar::new().width(8).scroller_width(8),
        ))
        .style(style::scrollbar)
        .height(Fill)
        .into()
}

fn session_bubble<'a>(session: &SessionDebug) -> Element<'a, Message> {
    let mut about = column![
        text(session.title.clone())
            .size(16)
            .wrapping(text::Wrapping::None)
    ];
    if !session.project.is_empty() {
        about = about.push(text(session.project.clone()).size(13).style(style::muted));
    }
    let mut facts = row![
        small(format!("this minute {:.2}", session.minute_attention)),
        small(format!("credit {:.2}", session.multiplier)),
        small(
            if session.running {
                "working"
            } else {
                "waiting"
            }
            .to_string()
        ),
    ]
    .spacing(18);
    if session.in_front {
        facts = facts.push(small("in front".to_string()));
    }
    let value = column![
        text(format!("{:.2}", session.attention))
            .size(30)
            .font(style::BOLD)
            .style(style::accent),
        text("attention").size(12).style(style::muted),
    ]
    .align_x(Center);
    bubble(
        row![column![about, facts].spacing(6).width(Fill), value]
            .spacing(14)
            .align_y(Center),
    )
    .into()
}

fn now_rows<'a>(info: &DebugInfo, now_ms: u64) -> Element<'a, Message> {
    let input = match info.since_input_ms {
        Some(ms) => ago(ms),
        None => "unknown".to_string(),
    };
    let focus = info
        .focus
        .map(|f| {
            serde_json::to_value(f)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default()
        })
        .unwrap_or_else(|| "unknown".to_string());
    let upload = match &info.last_upload {
        Some((at, what)) => format!("{}, {what}", ago(now_ms.saturating_sub(*at))),
        None => "not yet (only while logged in, every 30 s)".to_string(),
    };
    let rows = [
        ("Last input", input),
        ("App in front", focus),
        (
            "Activity",
            if info.activity { "allowed" } else { "off" }.to_string(),
        ),
        ("Backends", info.backends.clone()),
        ("Watched folders", info.watched_folders.to_string()),
        ("Heartbeats this run", info.heartbeats.to_string()),
        ("Waiting for upload", info.queued.to_string()),
        ("Last upload", upload),
    ];
    column(rows.into_iter().map(|(label, value)| {
        row![
            text(label).size(14).style(style::muted).width(170),
            text(value).size(14).width(Fill),
        ]
        .into()
    }))
    .spacing(6)
    .into()
}

/// "3 s ago", "2 min ago".
fn ago(ms: u64) -> String {
    let secs = ms / 1000;
    if secs < 120 {
        format!("{secs} s ago")
    } else {
        format!("{} min ago", secs / 60)
    }
}

fn heading<'a>(label: &'a str) -> Element<'a, Message> {
    text(label).size(18).font(style::BOLD).into()
}

fn muted<'a>(label: &'a str) -> Element<'a, Message> {
    text(label).size(13).style(style::muted).into()
}

fn small<'a>(label: String) -> Element<'a, Message> {
    text(label).size(13).style(style::muted).into()
}

/// A block of several lines.
fn card<'a>(content: impl Into<Element<'a, Message>>) -> container::Container<'a, Message> {
    container(content)
        .width(Fill)
        .padding([16, 22])
        .style(style::card)
}

/// One row, pill-shaped like the session list.
fn bubble<'a>(content: impl Into<Element<'a, Message>>) -> container::Container<'a, Message> {
    container(content)
        .width(Fill)
        .padding([12, 22])
        .style(|theme| style::bubble(theme, false))
}
