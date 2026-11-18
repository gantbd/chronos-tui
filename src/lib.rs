#![forbid(unsafe_code)]
use chronos_model::{View, display_id};
use chronos_protocol::{RetrySafety, TaskSpec};
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
mod presence;
#[cfg(unix)]
pub mod profile;
mod watch;
pub use component::{
    Component, Input, Lifecycle, SchedulingMutationDisposition, TaskMutationDisposition,
    translate_key,
};
pub use hub::{Hub, SelectedTaskProjection, WorkbenchIntent, Workstream, validate_workstreams};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Style},
    widgets::{Block, Clear, List, ListItem, ListState, Paragraph, Wrap},
};

/// Presentation capabilities owned by this peer. They describe host-facing
/// composition support and never grant task mutation or scheduling authority.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CapabilityManifest {
    pub schema: u8,
    pub peer: &'static str,
    pub interactive: &'static [&'static str],
    pub headless: &'static [&'static str],
}

pub const CAPABILITIES: CapabilityManifest = CapabilityManifest {
    schema: 1,
    peer: "chronos",
    interactive: &["tasks.observe.v1"],
    headless: &["tasks.observe.v1"],
};

fn task_badges(view: &View, row: &chronos_model::Row) -> String {
    let marked = view.marked_cancel.iter().any(|target| {
        target.task == row.reference.task && target.incarnation == row.reference.incarnation
    });
    let cancel_receipt = view
        .cancel_receipts
        .iter()
        .find(|receipt| {
            receipt.task == row.reference.task && receipt.incarnation == row.reference.incarnation
        })
        .map(|receipt| match receipt.disposition {
            chronos_model::CancelDisposition::Applied => "A",
            chronos_model::CancelDisposition::Refused => "R",
            chronos_model::CancelDisposition::OutcomeUnknown => "?",
            chronos_model::CancelDisposition::NotSent => "N",
        });
    let eligibility_receipt = view
        .scheduling
        .eligibility_receipts
        .iter()
        .find(|receipt| {
            receipt.reference.task == row.reference.task
                && receipt.reference.incarnation == row.reference.incarnation
        })
        .map(|receipt| match receipt.disposition {
            chronos_model::EligibilityDisposition::Applied => "A",
            chronos_model::EligibilityDisposition::Refused => "R",
            chronos_model::EligibilityDisposition::OutcomeUnknown => "?",
            chronos_model::EligibilityDisposition::NotSent => "N",
        });
    let mut badges = if marked { "[x]" } else { "[ ]" }.to_owned();
    if let Some(receipt) = cancel_receipt {
        badges.push_str(&format!("[c{receipt}]"));
    }
    if let Some(receipt) = eligibility_receipt {
        badges.push_str(&format!("[s{receipt}]"));
    }
    badges
}

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
        None,
        notice,
        "Task batch (Tab adds row, Enter reviews exact graph, Esc discards)",
    );
}
pub(crate) fn draw_form_in(
    frame: &mut Frame<'_>,
    area: Rect,
    view: &View,
    entry: Option<&str>,
    task_review: Option<(&[TaskSpec], usize)>,
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
        "Chronos · revision {} · {} visible tasks · {} marked",
        view.revision,
        view.rows.len(),
        view.marked_cancel.len()
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
    let composing_tasks = entry.filter(|_| label.starts_with("Task batch"));
    let rows: Vec<_> = if let Some((tasks, _)) = task_review {
        tasks
            .iter()
            .enumerate()
            .map(|(index, task)| {
                let retry = match task.retry_safety {
                    RetrySafety::FenceUnknown => "fence",
                    RetrySafety::ConnectorIdempotent => "idempotent",
                };
                ListItem::new(format!(
                    "{:>3}  p={:<4} attempts={:<3} retry={:<10} deps={:<2} caps={:<2} {}",
                    index + 1,
                    task.priority,
                    task.max_attempts,
                    retry,
                    task.dependencies.len(),
                    task.capabilities.len(),
                    display_id(&task.id)
                ))
            })
            .collect()
    } else if let Some(batch) = composing_tasks {
        batch
            .split('\n')
            .enumerate()
            .map(|(index, id)| ListItem::new(format!("{:>3}  {}", index + 1, display_id(id))))
            .collect()
    } else {
        view.rows
            .iter()
            .map(|row| {
                ListItem::new(format!(
                    "{} {:<12} p={:<4} {}",
                    task_badges(view, row),
                    row.phase,
                    row.priority,
                    display_id(&row.reference.task)
                ))
            })
            .collect()
    };
    let list = List::new(rows)
        .block(Block::bordered().title(if task_review.is_some() {
            "Reviewed atomic task graph · selected row"
        } else if composing_tasks.is_some() {
            "Atomic task batch · current row selected"
        } else {
            "Tasks · current bounded page"
        }))
        .highlight_style(Style::default().fg(Color::Cyan))
        .highlight_symbol("▶ ");
    let selected = if let Some((_, selected)) = task_review {
        Some(selected)
    } else if let Some(batch) = composing_tasks {
        Some(batch.split('\n').count().saturating_sub(1))
    } else if view.rows.is_empty() {
        None
    } else {
        Some(view.selected)
    };
    let mut state = ListState::default().with_selected(selected);
    frame.render_stateful_widget(list, areas[1], &mut state);
    let status = status_text(view, entry, task_review, notice, label);
    frame.render_widget(Paragraph::new(status).wrap(Wrap { trim: false }), areas[2]);
    frame.render_widget(
        Paragraph::new(if view.scheduling.supported {
            "↑/↓ select · Space mark · r tasks · ] next · n new · c cancel marked/current · q quit\nreceipts c=cancel s=schedule · A applied · R refused · ? unknown · N not sent"
        } else { "↑/↓ select · Space mark · r refresh · ] next · n new · c cancel marked/current · q quit\nreceipts c=cancel s=schedule · A applied · R refused · ? unknown · N not sent" }),
        areas[3],
    );
}
fn status_text(
    view: &View,
    entry: Option<&str>,
    task_review: Option<(&[TaskSpec], usize)>,
    notice: &str,
    label: &str,
) -> String {
    if let Some(id) = entry {
        if label.starts_with("Task batch") {
            let count = id.split('\n').count();
            let current = id.rsplit('\n').next().unwrap_or_default();
            format!(
                "{}\n{} / {} rows · current {} / 256 bytes: {}\n{}",
                label,
                count,
                chronos_model::MAX_SUBMIT_BATCH,
                current.len(),
                display_id(current),
                notice
            )
        } else {
            format!("{}: {}\n{}", label, display_id(id), notice)
        }
    } else if let Some((tasks, selected)) = task_review {
        let task = &tasks[selected];
        let retry = match task.retry_safety {
            RetrySafety::FenceUnknown => "fence",
            RetrySafety::ConnectorIdempotent => "idempotent",
        };
        format!(
            "Review {} exact task(s) · selected {} · p={} · attempts={} · retry={} · deps={} [{}] · caps={} [{}]\nEnter submits once · ↑/↓ row · p priority · d dependencies · c capabilities · a attempts · r retry · Esc edits IDs\n{}",
            tasks.len(),
            display_id(&task.id),
            task.priority,
            task.max_attempts,
            retry,
            task.dependencies.len(),
            task.dependencies.join(", "),
            task.capabilities.len(),
            task.capabilities.join(", "),
            notice
        )
    } else if let Some(plan) = &view.scheduling.pending {
        format!("{plan}? y confirms; Esc discards. Exact observed revision retained.\n{notice}")
    } else if let Some(task) = &view.pending_cancel {
        format!(
            "Cancel {task}? y confirms; Esc discards. Exact incarnations retained; every child is reauthorized and receipted independently.\n{notice}"
        )
    } else {
        let receipts = if view.cancel_receipts.is_empty() {
            String::new()
        } else {
            let applied = view
                .cancel_receipts
                .iter()
                .filter(|receipt| {
                    matches!(
                        receipt.disposition,
                        chronos_model::CancelDisposition::Applied
                    )
                })
                .count();
            let refused = view
                .cancel_receipts
                .iter()
                .filter(|receipt| {
                    matches!(
                        receipt.disposition,
                        chronos_model::CancelDisposition::Refused
                    )
                })
                .count();
            let unknown = view
                .cancel_receipts
                .iter()
                .filter(|receipt| {
                    matches!(
                        receipt.disposition,
                        chronos_model::CancelDisposition::OutcomeUnknown
                    )
                })
                .count();
            let not_sent = view.cancel_receipts.len() - applied - refused - unknown;
            format!(
                "\nCancel receipts: applied={applied} refused={refused} unknown={unknown} not-sent={not_sent}"
            )
        };
        let scheduling_receipts = if view.scheduling.eligibility_receipts.is_empty() {
            String::new()
        } else {
            let applied = view
                .scheduling
                .eligibility_receipts
                .iter()
                .filter(|receipt| {
                    receipt.disposition == chronos_model::EligibilityDisposition::Applied
                })
                .count();
            let refused = view
                .scheduling
                .eligibility_receipts
                .iter()
                .filter(|receipt| {
                    receipt.disposition == chronos_model::EligibilityDisposition::Refused
                })
                .count();
            let unknown = view
                .scheduling
                .eligibility_receipts
                .iter()
                .filter(|receipt| {
                    receipt.disposition == chronos_model::EligibilityDisposition::OutcomeUnknown
                })
                .count();
            let not_sent = view.scheduling.eligibility_receipts.len() - applied - refused - unknown;
            format!(
                "\nSchedule receipts: applied={applied} refused={refused} unknown={unknown} not-sent={not_sent}"
            )
        };
        if view.scheduling.supported {
            format!(
                "{}{}{}\n{}\n{}",
                view.status, receipts, scheduling_receipts, view.scheduling.status, notice
            )
        } else {
            format!("{}{}\n{}", view.status, receipts, notice)
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
