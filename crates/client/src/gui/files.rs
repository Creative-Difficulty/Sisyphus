//! First start, after logging in: "Additional required files". Downloads
//! the local prompt model ([`crate::prompt_model`]) from the server, with a
//! progress bar. The screen only ever says "additional required files",
//! never "model" (decided 2026-11-22). Required: the window only goes on once the file is there
//! and checked, and comes back here if it goes missing.

use iced::widget::{button, column, container, progress_bar, row, text};
use iced::{Center, Element, Fill};

use super::{Message, style};
use crate::prompt_model::MODEL_SIZE;

/// Where the download stands.
#[derive(Debug, Clone, Default)]
pub struct Download {
    pub running: bool,
    /// Bytes so far.
    pub done: u64,
    pub finished: bool,
    pub error: Option<String>,
}

pub fn view(download: &Download) -> Element<'static, Message> {
    let fraction = if download.finished {
        1.0
    } else {
        download.done as f32 / MODEL_SIZE as f32
    };
    let status = if download.finished {
        "Ready".to_string()
    } else if download.error.is_some() {
        "Stopped".to_string()
    } else {
        format!(
            "{:.1} of {:.0} MB",
            download.done as f64 / 1e6,
            MODEL_SIZE as f64 / 1e6
        )
    };
    let file = container(
        column![
            row![
                text(if download.finished {
                    "Additional required files downloaded"
                } else {
                    "Downloading additional required files"
                })
                .size(17)
                .font(style::BOLD)
                .width(Fill),
                text(status).size(14).style(style::muted),
            ]
            .align_y(Center),
            progress_bar(0.0..=1.0, fraction)
                .girth(12)
                .style(style::progress),
        ]
        .spacing(12),
    )
    .width(Fill)
    .padding([20, 26])
    .style(style::card);

    let mut content = column![
        text("Additional required files")
            .size(30)
            .font(style::BOLD)
            .center(),
        file,
    ]
    .spacing(28)
    .align_x(Center);

    if let Some(error) = &download.error {
        content = content.push(text(error.clone()).size(14).style(text::danger).center());
        content = content.push(
            button(text("Try again").size(17))
                .padding([12, 40])
                .style(style::rounded(style::pink))
                .on_press(Message::DownloadFiles),
        );
    } else {
        content = content.push(
            button(text("Continue").size(17))
                .padding([12, 40])
                .style(style::rounded(style::pink))
                .on_press_maybe(download.finished.then_some(Message::Continue)),
        );
    }

    container(container(content).max_width(520))
        .padding(24)
        .center(Fill)
        .into()
}
