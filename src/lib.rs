#![forbid(unsafe_code)]
use chronos_model::{View, display_id};
mod agenda;
mod attempt;
#[cfg(unix)]
mod cancel_receipt;
mod component;
mod explain;
mod graph;
mod handoff;
pub use handoff::{HandoffAction, HandoffRequest, HandoffResult};
mod hub;
mod planning;
mod watch;
pub use component::{Component, Input, Lifecycle, translate_key};
pub use hub::{Hub, Workstream, validate_workstreams};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Style},
    widgets::{Block, Clear, List, ListItem, ListState, Paragraph, Wrap},
};

pub fn render(frame: &mut Frame<'_>, view: &View, entry: Option<&str>, notice: &str) {
    draw_in(frame, frame.area(), view, entry, notice);
}

/// Embeddable projection into host-owned layout. Does not read input, own a
/// terminal or change cells outside the intersection with the current frame.
/// The host owns pane geometry, focus, event routing and process lifecycle.
pub fn draw_in(frame: &mut Frame<'_>, area: Rect, view: &View, entry: Option<&str>, notice: &str) {
    draw_form_in(
        frame,
        area,
        view,
        entry,
        notice,
        "New task ID (Enter submits, Esc discards)",
    );
}
pub(crate) fn draw_form_in(
    frame: &mut Frame<'_>,
    area: Rect,
    view: &View,
    entry: Option<&str>,
    notice: &str,
    label: &str,
) {
    let area = area.intersection(frame.area());
    if area.is_empty() {
        return;
    }
    frame.render_widget(Clear, area);
    let areas = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(1),
        Constraint::Length(3),
        Constraint::Length(2),
    ])
    .split(area);
    let mut heading = format!(
        "Chronos · revision {} · {} visible tasks",
        view.revision,
        view.rows.len()
    );
    if view.scheduling.supported {
        let s = &view.scheduling;
        heading.push_str(&format!(
            "\nScheduling r={} enabled={} paused={} slots={:?} held={:?} available={:?}",
            s.revision, s.enabled, s.paused, s.capacity, s.held, s.available
        ));
        if let Some(selected) = &s.selected {
            heading.push_str(&format!(
                "\nSelected pause={} not-before={}",
                selected.paused,
                format_not_before(selected.not_before)
            ));
        }
    }
    frame.render_widget(Paragraph::new(heading), areas[0]);
    let rows: Vec<_> = view
        .rows
        .iter()
        .map(|row| {
            ListItem::new(format!(
                "{:<12} p={:<4} {}",
                row.phase,
                row.priority,
                display_id(&row.reference.task)
            ))
        })
        .collect();
    let list = List::new(rows)
        .block(Block::bordered().title("Tasks · current bounded page"))
        .highlight_style(Style::default().fg(Color::Cyan))
        .highlight_symbol("▶ ");
    let mut state = ListState::default().with_selected(if view.rows.is_empty() {
        None
    } else {
        Some(view.selected)
    });
    frame.render_stateful_widget(list, areas[1], &mut state);
    let status = status_text(view, entry, notice, label);
    frame.render_widget(Paragraph::new(status).wrap(Wrap { trim: false }), areas[2]);
    frame.render_widget(
        Paragraph::new(if view.scheduling.supported {
            "↑/↓ select · r tasks · ] next · n new · c cancel · q quit\ns scheduling · e enable · p global pause · t task pause · b capacity · d not-before"
        } else { "↑/↓ select · r refresh first page · ] next page · n new task · c cancel · q quit" }),
        areas[3],
    );
}
fn status_text(view: &View, entry: Option<&str>, notice: &str, label: &str) -> String {
    if let Some(id) = entry {
        format!("{}: {}\n{}", label, display_id(id), notice)
    } else if let Some(plan) = &view.scheduling.pending {
        format!("{plan}? y confirms; Esc discards. Exact observed revision retained.\n{notice}")
    } else if let Some(task) = &view.pending_cancel {
        format!(
            "Cancel {task}? y confirms; Esc discards. Original revision/incarnation retained.\n{notice}"
        )
    } else {
        if view.scheduling.supported {
            format!("{}\n{}\n{}", view.status, view.scheduling.status, notice)
        } else {
            format!("{}\n{}", view.status, notice)
        }
    }
}

pub fn format_not_before(value: Option<std::time::SystemTime>) -> String {
    match value {
        None => "none".into(),
        Some(value) => match value.duration_since(std::time::SystemTime::UNIX_EPOCH) {
            Ok(d) => format!("{}.{:09} Unix seconds", d.as_secs(), d.subsec_nanos()),
            Err(e) => format!(
                "-{}.{:09} Unix seconds",
                e.duration().as_secs(),
                e.duration().subsec_nanos()
            ),
        },
    }
}
