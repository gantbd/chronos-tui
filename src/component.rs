use chronos_client::WireClientV1;
use chronos_model::{Action, EligibilityEdit, MAX_SUBMIT_BATCH, SchedulingChange, View, Worker};
use chronos_protocol::{RetrySafety, TaskSpec};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::{Frame, layout::Rect};

/// Presentation input only. No terminal ownership or engine key bindings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Input {
    Close,
    Text(char),
    Backspace,
    NextItem,
    Enter,
    Escape,
    Up,
    Down,
    Ignore,
}

pub fn translate_key(key: KeyEvent) -> Input {
    if key.kind == KeyEventKind::Release {
        return Input::Ignore;
    }
    if key.modifiers == KeyModifiers::CONTROL && key.code == KeyCode::Char('c') {
        return if key.kind == KeyEventKind::Press {
            Input::Close
        } else {
            Input::Ignore
        };
    }
    if key.modifiers.intersects(
        KeyModifiers::CONTROL
            | KeyModifiers::ALT
            | KeyModifiers::SUPER
            | KeyModifiers::HYPER
            | KeyModifiers::META,
    ) {
        return Input::Ignore;
    }
    // Repeats are navigation/editing only, never submit, confirm or command.
    match key.code {
        KeyCode::Up => Input::Up,
        KeyCode::Down => Input::Down,
        KeyCode::Backspace => Input::Backspace,
        KeyCode::Tab if key.kind == KeyEventKind::Press => Input::NextItem,
        KeyCode::Char(ch) if !ch.is_control() && key.kind == KeyEventKind::Press => Input::Text(ch),
        KeyCode::Enter if key.kind == KeyEventKind::Press => Input::Enter,
        KeyCode::Esc if key.kind == KeyEventKind::Press => Input::Escape,
        _ => Input::Ignore,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lifecycle {
    Continue,
    CloseRequested,
}

/// Content-free terminal disposition for a dispatched task graph. Hosts use
/// this to settle end-to-end instrumentation without parsing task identities.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskMutationDisposition {
    Applied,
    Refused,
    OutcomeUnknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SchedulingMutationDisposition {
    Applied,
    Refused,
    OutcomeUnknown,
}

/// Host-owned pane. The caller selects its client, region, focus and lifecycle.
/// Service work uses the same bounded worker as the standalone application.
pub struct Component {
    worker: Worker,
    state: State,
}

#[derive(Default)]
struct State {
    entry_kind: EntryKind,
    view: View,
    entry: Option<String>,
    task_batch: Option<Vec<TaskSpec>>,
    task_batch_selected: usize,
    notice: String,
    writes: bool,
}
#[derive(Clone, Copy, Default)]
enum EntryKind {
    #[default]
    Task,
    TaskPriority,
    TaskMaxAttempts,
    TaskDependencies,
    TaskCapabilities,
    Capacity,
    NotBefore,
}
impl EntryKind {
    fn label(self) -> &'static str {
        match self {
            Self::Task => "Task batch (Tab adds row, Enter reviews exact graph, Esc discards)",
            Self::TaskPriority => "Priority: signed integer (Enter applies, Esc returns to review)",
            Self::TaskMaxAttempts => {
                "Maximum attempts: positive integer (Enter applies, Esc returns to review)"
            }
            Self::TaskDependencies => {
                "Dependencies: comma-separated exact task IDs (Enter applies, Esc returns)"
            }
            Self::TaskCapabilities => {
                "Required capabilities: comma-separated names (Enter applies, Esc returns)"
            }
            Self::Capacity => {
                "Concurrency capacity: positive integer (Enter proposes, Esc discards)"
            }
            Self::NotBefore => "Not-before: Unix seconds or clear (Enter proposes, Esc discards)",
        }
    }
}
fn parse_not_before(value: &str) -> Result<Option<std::time::SystemTime>, ()> {
    if value == "clear" {
        return Ok(None);
    }
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return Err(());
    }
    std::time::SystemTime::UNIX_EPOCH
        .checked_add(std::time::Duration::from_secs(
            value.parse().map_err(|_| ())?,
        ))
        .map(Some)
        .ok_or(())
}

fn task_ids(value: &str) -> Result<Vec<String>, &'static str> {
    let ids: Vec<_> = value.split('\n').map(str::to_owned).collect();
    if ids.is_empty() || ids.len() > MAX_SUBMIT_BATCH {
        return Err("Task batch must contain 1–128 rows; nothing was sent.");
    }
    if ids.iter().any(|id| id.is_empty()) {
        return Err("Every task row needs an ID; nothing was sent.");
    }
    if ids.iter().any(|id| id.len() > 256) {
        return Err("Each task ID is limited to 256 UTF-8 bytes; nothing was sent.");
    }
    let unique = ids
        .iter()
        .map(String::as_str)
        .collect::<std::collections::HashSet<_>>();
    if unique.len() != ids.len() {
        return Err("Task IDs must be unique within the batch; nothing was sent.");
    }
    Ok(ids)
}

fn bounded_list(value: &str, max_entries: usize, max_bytes: usize) -> Result<Vec<String>, ()> {
    if value.trim().is_empty() {
        return Ok(Vec::new());
    }
    let values = value
        .split(',')
        .map(str::trim)
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if values.len() > max_entries
        || values
            .iter()
            .any(|value| value.is_empty() || value.len() > max_bytes)
        || values
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len()
            != values.len()
    {
        return Err(());
    }
    Ok(values)
}

impl Component {
    pub fn new<C: WireClientV1 + Send + 'static>(client: C, writes: bool) -> Self {
        Self::from_worker(Worker::spawn(client, writes), writes)
    }
    pub fn new_with_scheduling<
        C: WireClientV1 + chronos_client::SchedulingWireClientV5 + Send + 'static,
    >(
        client: C,
        writes: bool,
    ) -> Self {
        Self::from_worker(Worker::spawn_with_scheduling(client, writes), writes)
    }
    fn from_worker(worker: Worker, writes: bool) -> Self {
        Self {
            worker,
            state: State {
                writes,
                ..State::default()
            },
        }
    }

    /// Nonblocking delivery; receiving acknowledges the worker's last projection.
    pub fn poll(&mut self) -> bool {
        if let Ok(view) = self.worker.updates.try_recv() {
            self.state.view = view;
            self.state.notice.clear();
            true
        } else {
            false
        }
    }

    pub fn update(&mut self, input: Input, focused: bool) -> Lifecycle {
        let (lifecycle, action) = self.state.update(input, focused, self.worker.is_busy());
        if let Some(action) = action {
            let recovery = match &action {
                Action::SubmitGraph(tasks) => Some(tasks.clone()),
                _ => None,
            };
            if self.worker.try_dispatch(action).is_err() {
                self.state.task_batch = recovery;
                self.state.notice =
                    "Service action pending; reviewed task graph preserved for retry.".into();
            }
        }
        lifecycle
    }

    pub(crate) fn refresh_from_feed(&self) -> bool {
        !self.editing()
            && !self.state.view.manual_refresh_required
            && !self.has_pending()
            && !self.worker.is_busy()
            && self
                .worker
                .try_dispatch(Action::RefreshKeepingSelection)
                .is_ok()
    }

    pub(crate) fn has_pending(&self) -> bool {
        self.state.view.pending_cancel.is_some() || self.state.view.scheduling.pending.is_some()
    }

    pub(crate) fn is_busy(&self) -> bool {
        self.worker.is_busy()
    }

    pub(crate) fn feed_notice(&mut self) {
        self.state.notice =
            "Feed changed or is unavailable. Discard the old intent and refresh.".into();
    }

    pub(crate) fn select(&mut self, index: usize) -> bool {
        if self.editing() || self.state.view.rows.get(index).is_none() {
            return false;
        }
        if self.state.view.selected == index && !self.worker.is_busy() {
            return true;
        }
        self.state.view.pending_cancel.is_none()
            && self.state.view.scheduling.pending.is_none()
            && !self.worker.is_busy()
            && self.worker.try_dispatch(Action::Select(index)).is_ok()
    }

    pub(crate) fn view(&self) -> &View {
        &self.state.view
    }

    pub(crate) fn editing(&self) -> bool {
        self.state.entry.is_some() || self.state.task_batch.is_some()
    }

    pub(crate) fn composing_task_batch(&self) -> bool {
        self.state.task_batch.is_some()
            || self.state.entry.is_some() && matches!(self.state.entry_kind, EntryKind::Task)
    }

    pub(crate) fn confirming_cancel_batch(&self) -> bool {
        self.state.view.pending_cancel.is_some()
    }

    pub(crate) fn confirming_scheduling(&self) -> bool {
        self.state.view.scheduling.pending.is_some()
    }

    pub(crate) fn confirming_eligibility_batch(&self) -> bool {
        self.state
            .view
            .scheduling
            .pending
            .as_deref()
            .is_some_and(|pending| pending.contains("exact task"))
    }

    pub(crate) fn task_mutation_disposition(&self) -> TaskMutationDisposition {
        match self.state.view.last_task_mutation {
            Some(chronos_model::TaskMutationDisposition::Applied) => {
                TaskMutationDisposition::Applied
            }
            Some(chronos_model::TaskMutationDisposition::OutcomeUnknown) => {
                TaskMutationDisposition::OutcomeUnknown
            }
            Some(chronos_model::TaskMutationDisposition::Refused) | None => {
                TaskMutationDisposition::Refused
            }
        }
    }

    pub(crate) fn scheduling_mutation_disposition(&self) -> SchedulingMutationDisposition {
        let status = &self.state.view.scheduling.status;
        if status.starts_with("Applied") || status.contains("control applied") {
            SchedulingMutationDisposition::Applied
        } else if status.contains("unknown") || status.contains("unavailable") {
            SchedulingMutationDisposition::OutcomeUnknown
        } else {
            SchedulingMutationDisposition::Refused
        }
    }

    pub(crate) fn mutation_recovery_required(&self) -> bool {
        self.state.view.manual_refresh_required
    }

    pub(crate) fn status(&self) -> String {
        crate::status_text(
            &self.state.view,
            self.state.entry.as_deref(),
            self.state
                .task_batch
                .as_deref()
                .map(|tasks| (tasks, self.state.task_batch_selected)),
            &self.state.notice,
            self.state.entry_kind.label(),
        )
    }

    pub fn draw_in(&self, frame: &mut Frame<'_>, area: Rect) {
        crate::draw_form_in(
            frame,
            area,
            &self.state.view,
            self.state.entry.as_deref(),
            self.state
                .task_batch
                .as_deref()
                .map(|tasks| (tasks, self.state.task_batch_selected)),
            &self.state.notice,
            self.state.entry_kind.label(),
        );
    }
}

impl State {
    fn update(&mut self, input: Input, focused: bool, busy: bool) -> (Lifecycle, Option<Action>) {
        if !focused {
            return (Lifecycle::Continue, None);
        }
        if input == Input::Close
            || input == Input::Text('q') && self.entry.is_none() && self.task_batch.is_none()
        {
            return (Lifecycle::CloseRequested, None);
        }
        let action = if let Some(id) = &mut self.entry {
            match input {
                Input::Escape => {
                    self.entry = None;
                    if !matches!(self.entry_kind, EntryKind::Task) {
                        self.entry_kind = EntryKind::Task;
                        self.notice = "Returned to the reviewed task graph; nothing sent.".into();
                    }
                    None
                }
                Input::Enter if !busy => {
                    let value = self.entry.take().unwrap();
                    let task_field = matches!(
                        self.entry_kind,
                        EntryKind::TaskPriority
                            | EntryKind::TaskMaxAttempts
                            | EntryKind::TaskDependencies
                            | EntryKind::TaskCapabilities
                    );
                    if task_field {
                        let applied = match self.entry_kind {
                            EntryKind::TaskPriority => value
                                .parse::<i32>()
                                .ok()
                                .and_then(|priority| {
                                    self.task_batch.as_mut().and_then(|tasks| {
                                        tasks.get_mut(self.task_batch_selected).map(|task| {
                                            task.priority = priority;
                                            self.notice =
                                                format!("Priority set to {priority}; graph not sent.");
                                        })
                                    })
                                })
                                .is_some(),
                            EntryKind::TaskMaxAttempts => value
                                .parse::<u32>()
                                .ok()
                                .filter(|attempts| *attempts > 0)
                                .and_then(|attempts| {
                                    self.task_batch.as_mut().and_then(|tasks| {
                                        tasks.get_mut(self.task_batch_selected).map(|task| {
                                            task.max_attempts = attempts;
                                            self.notice = format!(
                                                "Maximum attempts set to {attempts}; graph not sent."
                                            );
                                        })
                                    })
                                })
                                .is_some(),
                            EntryKind::TaskDependencies => {
                                let limits = chronos_local::wire_limits();
                                bounded_list(
                                    &value,
                                    limits.max_dependencies_per_task,
                                    limits.max_id_bytes,
                                )
                                .ok()
                                .and_then(|dependencies| {
                                    self.task_batch.as_mut().and_then(|tasks| {
                                        tasks.get_mut(self.task_batch_selected).and_then(|task| {
                                            (!dependencies.iter().any(|value| value == &task.id))
                                                .then(|| {
                                                    task.dependencies = dependencies;
                                                    self.notice =
                                                        "Dependencies updated; graph not sent."
                                                            .into();
                                                })
                                        })
                                    })
                                })
                                .is_some()
                            }
                            EntryKind::TaskCapabilities => {
                                let limits = chronos_local::wire_limits();
                                bounded_list(
                                    &value,
                                    limits.max_capabilities_per_task,
                                    limits.max_capability_bytes,
                                )
                                .ok()
                                .and_then(|capabilities| {
                                    self.task_batch.as_mut().and_then(|tasks| {
                                        tasks.get_mut(self.task_batch_selected).map(|task| {
                                            task.capabilities = capabilities;
                                            self.notice =
                                                "Required capabilities updated; graph not sent."
                                                    .into();
                                        })
                                    })
                                })
                                .is_some()
                            }
                            _ => unreachable!("task field was checked above"),
                        };
                        if applied {
                            self.entry_kind = EntryKind::Task;
                        } else {
                            self.entry = Some(value);
                            self.notice =
                                "Invalid field value; reviewed graph preserved and nothing sent."
                                    .into();
                        }
                        return (Lifecycle::Continue, None);
                    }
                    let action = match self.entry_kind {
                        EntryKind::Task => match task_ids(&value) {
                            Ok(ids) => {
                                self.task_batch = Some(
                                    ids.into_iter()
                                        .map(|id| TaskSpec {
                                            id,
                                            priority: 0,
                                            dependencies: Vec::new(),
                                            capabilities: Vec::new(),
                                            max_attempts: 3,
                                            retry_safety: RetrySafety::FenceUnknown,
                                        })
                                        .collect(),
                                );
                                self.task_batch_selected = 0;
                                self.notice = "Review exact owner fields; Enter submits, ↑/↓ selects, p priority, d dependencies, c capabilities, a attempts, r retry safety, Esc edits IDs.".into();
                                None
                            }
                            Err(notice) => {
                                self.notice = notice.into();
                                None
                            }
                        },
                        EntryKind::TaskPriority
                        | EntryKind::TaskMaxAttempts
                        | EntryKind::TaskDependencies
                        | EntryKind::TaskCapabilities => {
                            unreachable!("task fields return before action dispatch")
                        }
                        EntryKind::Capacity => {
                            value.parse::<usize>().ok().filter(|n| *n > 0).map(|n| {
                                Action::PlanScheduling(SchedulingChange::SetConcurrencyLimit(n))
                            })
                        }
                        EntryKind::NotBefore => {
                            parse_not_before(&value).ok().and_then(|not_before| {
                                self.has_eligibility_targets().then_some({
                                    Action::PlanScheduling(SchedulingChange::SetEligibility(
                                        EligibilityEdit::SetNotBefore(not_before),
                                    ))
                                })
                            })
                        }
                    };
                    if action.is_none() && self.task_batch.is_none() {
                        self.entry = Some(value);
                        if self.notice.is_empty() {
                            self.notice = "Invalid value; nothing was sent.".into();
                        }
                    }
                    action
                }
                Input::NextItem if matches!(self.entry_kind, EntryKind::Task) => {
                    let rows = id.split('\n').count();
                    let current = id.rsplit('\n').next().unwrap_or_default();
                    if current.is_empty() {
                        self.notice = "Finish the current task ID before adding a row.".into();
                    } else if rows == MAX_SUBMIT_BATCH {
                        self.notice =
                            format!("Atomic task batches are limited to {MAX_SUBMIT_BATCH} rows.");
                    } else {
                        id.push('\n');
                        self.notice = format!("{} task rows in this atomic batch.", rows + 1);
                    }
                    None
                }
                Input::Backspace => {
                    id.pop();
                    None
                }
                Input::Text(ch)
                    if !ch.is_control()
                        && id.rsplit('\n').next().unwrap_or_default().len() + ch.len_utf8()
                            <= 256 =>
                {
                    id.push(ch);
                    None
                }
                _ => None,
            }
        } else if self.task_batch.is_some() {
            match input {
                Input::Enter if !busy => {
                    let tasks = self.task_batch.take().unwrap();
                    self.notice = format!(
                        "Submitting {} exact task(s) as one atomic graph.",
                        tasks.len()
                    );
                    Some(Action::SubmitGraph(tasks))
                }
                Input::Escape => {
                    let ids = self
                        .task_batch
                        .take()
                        .unwrap()
                        .into_iter()
                        .map(|task| task.id)
                        .collect::<Vec<_>>()
                        .join("\n");
                    self.entry_kind = EntryKind::Task;
                    self.entry = Some(ids);
                    self.notice =
                        "Returned to task IDs; structured changes discarded, nothing sent.".into();
                    None
                }
                Input::Up => {
                    self.task_batch_selected = self.task_batch_selected.saturating_sub(1);
                    None
                }
                Input::Down => {
                    self.task_batch_selected = (self.task_batch_selected + 1)
                        .min(self.task_batch.as_ref().unwrap().len().saturating_sub(1));
                    None
                }
                Input::Text('p' | 'a' | 'd' | 'c') => {
                    let task = &self.task_batch.as_ref().unwrap()[self.task_batch_selected];
                    let (kind, value, notice) = match input {
                        Input::Text('p') => (
                            EntryKind::TaskPriority,
                            String::new(),
                            format!("Current priority {}; type a replacement.", task.priority),
                        ),
                        Input::Text('a') => (
                            EntryKind::TaskMaxAttempts,
                            String::new(),
                            format!(
                                "Current maximum attempts {}; type a replacement.",
                                task.max_attempts
                            ),
                        ),
                        Input::Text('d') => (
                            EntryKind::TaskDependencies,
                            task.dependencies.join(", "),
                            "Edit exact dependency IDs; empty clears them.".into(),
                        ),
                        Input::Text('c') => (
                            EntryKind::TaskCapabilities,
                            task.capabilities.join(", "),
                            "Edit required capability names; empty clears them.".into(),
                        ),
                        _ => unreachable!(),
                    };
                    self.entry_kind = kind;
                    self.entry = Some(value);
                    self.notice = notice;
                    None
                }
                Input::Text('r') => {
                    let task = &mut self.task_batch.as_mut().unwrap()[self.task_batch_selected];
                    task.retry_safety = match task.retry_safety {
                        RetrySafety::FenceUnknown => RetrySafety::ConnectorIdempotent,
                        RetrySafety::ConnectorIdempotent => RetrySafety::FenceUnknown,
                    };
                    self.notice = "Retry safety toggled; graph not sent.".into();
                    None
                }
                _ => None,
            }
        } else if self.view.scheduling.pending.is_some() {
            match input {
                Input::Text('y') => Some(Action::ConfirmScheduling),
                Input::Escape => Some(Action::DiscardPlan),
                _ => None,
            }
        } else if self.view.pending_cancel.is_some() {
            match input {
                Input::Text('y') => Some(Action::ConfirmCancel),
                Input::Escape => Some(Action::DiscardPlan),
                _ => None,
            }
        } else {
            match input {
                Input::Up => Some(Action::Up),
                Input::Down => Some(Action::Down),
                Input::Text('r') => Some(Action::Refresh),
                Input::Text(']') => Some(Action::NextPage),
                Input::Text(' ') if self.writes && !self.view.mutation_blocked && !busy => {
                    Some(Action::ToggleCancelSelection)
                }
                Input::Text('c') => Some(Action::PlanCancel),
                Input::Text('n') if self.writes && !self.view.mutation_blocked && !busy => {
                    self.entry_kind = EntryKind::Task;
                    self.entry = Some(String::new());
                    None
                }
                Input::Text('s') if self.view.scheduling.supported => {
                    Some(Action::RefreshScheduling(std::time::SystemTime::now()))
                }
                Input::Text('e') if self.can_schedule(busy) && !self.view.scheduling.enabled => {
                    Some(Action::PlanScheduling(SchedulingChange::Enable))
                }
                Input::Text('p') if self.can_schedule(busy) && self.view.scheduling.enabled => {
                    Some(Action::PlanScheduling(SchedulingChange::SetPaused(
                        !self.view.scheduling.paused,
                    )))
                }
                Input::Text('t') if self.can_schedule(busy) => {
                    let targets = if self.view.marked_cancel.is_empty() {
                        self.view.scheduling.selected.iter().collect::<Vec<_>>()
                    } else {
                        self.view.scheduling.marked.iter().collect::<Vec<_>>()
                    };
                    (!targets.is_empty()).then_some({
                        let paused = !targets.iter().all(|target| target.paused);
                        Action::PlanScheduling(SchedulingChange::SetEligibility(
                            EligibilityEdit::SetPaused(paused),
                        ))
                    })
                }
                Input::Text('b') if self.can_schedule(busy) && self.view.scheduling.enabled => {
                    self.entry_kind = EntryKind::Capacity;
                    self.entry = Some(String::new());
                    None
                }
                Input::Text('d') if self.can_schedule(busy) && self.has_eligibility_targets() => {
                    self.entry_kind = EntryKind::NotBefore;
                    self.entry = Some(String::new());
                    None
                }
                _ => None,
            }
        };
        (Lifecycle::Continue, action)
    }
    fn can_schedule(&self, busy: bool) -> bool {
        self.writes
            && self.view.scheduling.supported
            && !self.view.scheduling.mutation_blocked
            && !busy
    }

    fn has_eligibility_targets(&self) -> bool {
        if self.view.marked_cancel.is_empty() {
            self.view.scheduling.selected.is_some()
        } else {
            self.view.scheduling.marked.len() == self.view.marked_cancel.len()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_component_submits_and_confirms_through_real_embedded_sdk() {
        use chronos_sdk_embedded::{
            engine::Engine,
            host::native::RequestV1,
            native_client::EmbeddedClient,
            service::{Authorizer, Service},
        };
        struct SyntheticOwner;
        impl Authorizer for SyntheticOwner {
            fn allows(&mut self, _: &RequestV1) -> bool {
                true
            }
        }
        let host = Service::new(
            Engine::new(),
            SyntheticOwner,
            chronos_sdk_embedded::service::Limits {
                max_graph_tasks: 4,
                max_page_entries: 64,
            },
        );
        let client = EmbeddedClient::new(host, chronos_local::wire_limits());
        let mut pane = Component::new(client, true);
        // Only a dead-test ceiling, not a latency objective. Production uses
        // nonblocking poll; waiting here stays on this headless test thread.
        fn deliver(pane: &mut Component) {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
            while !pane.poll() {
                assert!(
                    std::time::Instant::now() < deadline,
                    "embedded projection not delivered"
                );
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        }
        deliver(&mut pane);
        pane.update(Input::Text('n'), true);
        for (index, id) in ["embedded-one", "embedded-two", "embedded-three"]
            .into_iter()
            .enumerate()
        {
            if index > 0 {
                pane.update(Input::NextItem, true);
            }
            for ch in id.chars() {
                pane.update(Input::Text(ch), true);
            }
        }
        pane.update(Input::Enter, true);
        assert!(pane.composing_task_batch());
        pane.update(Input::Down, true);
        pane.update(Input::Text('p'), true);
        pane.update(Input::Text('7'), true);
        pane.update(Input::Enter, true);
        pane.update(Input::Text('d'), true);
        for ch in "embedded-one".chars() {
            pane.update(Input::Text(ch), true);
        }
        pane.update(Input::Enter, true);
        pane.update(Input::Text('c'), true);
        for ch in "local-agent".chars() {
            pane.update(Input::Text(ch), true);
        }
        pane.update(Input::Enter, true);
        pane.update(Input::Text('a'), true);
        pane.update(Input::Text('5'), true);
        pane.update(Input::Enter, true);
        pane.update(Input::Text('r'), true);
        pane.update(Input::Enter, true);
        deliver(&mut pane);
        assert_eq!(pane.state.view.rows.len(), 3);
        assert_eq!(
            pane.state
                .view
                .rows
                .iter()
                .filter(|row| row.phase == "ready")
                .count(),
            2
        );
        assert_eq!(
            pane.state
                .view
                .rows
                .iter()
                .filter(|row| row.phase == "blocked")
                .count(),
            1
        );
        let configured = pane
            .state
            .view
            .rows
            .iter()
            .find(|row| row.reference.task == "embedded-two")
            .unwrap();
        assert_eq!(configured.priority, 7);
        assert_eq!(configured.dependency_count, 1);
        assert_eq!(configured.required_capability_count, 1);
        for index in 0..3 {
            if index > 0 {
                pane.update(Input::Down, true);
                deliver(&mut pane);
            }
            pane.update(Input::Text(' '), true);
            deliver(&mut pane);
        }
        assert_eq!(pane.state.view.marked_cancel.len(), 3);
        pane.update(Input::Text('c'), true);
        deliver(&mut pane);
        assert_eq!(
            pane.state.view.pending_cancel.as_deref(),
            Some("3 marked tasks")
        );
        pane.update(Input::Text('y'), true);
        deliver(&mut pane);
        assert!(
            pane.state
                .view
                .rows
                .iter()
                .all(|row| row.phase == "cancelled")
        );
        assert_eq!(pane.state.view.cancel_receipts.len(), 3);
        assert_eq!(pane.state.view.signals.requests, 8);
        assert_eq!(pane.state.view.signals.backend_errors, 0);
        assert_eq!(pane.update(Input::Close, true), Lifecycle::CloseRequested);
    }

    #[test]
    fn unfocused_pane_ignores_input_and_close_is_only_output() {
        let mut state = State {
            writes: true,
            ..State::default()
        };
        state.view.mutation_blocked = false;
        assert_eq!(
            state.update(Input::Close, false, false).0,
            Lifecycle::Continue
        );
        state.update(Input::Text('n'), false, false);
        assert!(state.entry.is_none());
        assert_eq!(
            state.update(Input::Close, true, false).0,
            Lifecycle::CloseRequested
        );
        state.update(Input::Text('n'), true, false);
        assert_eq!(state.entry.as_deref(), Some(""));
    }

    #[test]
    fn form_is_byte_bounded_and_busy_submit_preserves_input() {
        let mut state = State {
            entry: Some(String::new()),
            ..State::default()
        };
        for _ in 0..130 {
            state.update(Input::Text('é'), true, false);
        }
        assert_eq!(state.entry.as_ref().unwrap().len(), 256);
        assert!(state.update(Input::Enter, true, true).1.is_none());
        assert!(state.entry.is_some());
        assert!(state.update(Input::Enter, true, false).1.is_none());
        assert!(state.entry.is_none());
        assert_eq!(state.task_batch.as_ref().unwrap().len(), 1);
        assert!(matches!(
            state.update(Input::Enter, true, false).1,
            Some(Action::SubmitGraph(tasks)) if tasks.len() == 1 && tasks[0].id.len() == 256
        ));
        assert!(state.task_batch.is_none());
    }

    #[test]
    fn batch_composer_is_bounded_and_refuses_empty_or_duplicate_rows() {
        let mut state = State {
            entry: Some("same".into()),
            ..State::default()
        };
        state.update(Input::NextItem, true, false);
        assert_eq!(state.entry.as_deref(), Some("same\n"));
        state.update(Input::NextItem, true, false);
        assert!(state.notice.contains("Finish"));
        for ch in "same".chars() {
            state.update(Input::Text(ch), true, false);
        }
        assert!(state.update(Input::Enter, true, false).1.is_none());
        assert!(state.notice.contains("unique"));
        assert_eq!(state.entry.as_deref(), Some("same\nsame"));

        state.entry = Some(
            (0..MAX_SUBMIT_BATCH)
                .map(|index| format!("task-{index}"))
                .collect::<Vec<_>>()
                .join("\n"),
        );
        state.update(Input::NextItem, true, false);
        assert!(state.notice.contains("limited"));
        assert_eq!(
            state.entry.as_ref().unwrap().split('\n').count(),
            MAX_SUBMIT_BATCH
        );
    }

    #[test]
    fn releases_modifiers_and_repeated_confirmation_are_ignored() {
        let key = KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE);
        assert_eq!(
            translate_key(KeyEvent {
                kind: KeyEventKind::Repeat,
                ..key
            }),
            Input::Ignore
        );
        assert_eq!(
            translate_key(KeyEvent {
                kind: KeyEventKind::Release,
                ..key
            }),
            Input::Ignore
        );
        assert_eq!(
            translate_key(KeyEvent {
                modifiers: KeyModifiers::ALT,
                ..key
            }),
            Input::Ignore
        );
        assert_eq!(translate_key(key), Input::Text('y'));
        assert_eq!(
            translate_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
            Input::NextItem
        );
    }
}
#[cfg(test)]
mod scheduling_tests;
