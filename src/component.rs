use chronos_client::WireClientV1;
use chronos_model::{Action, SchedulingChange, View, Worker};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::{Frame, layout::Rect};

/// Presentation input only. No terminal ownership or engine key bindings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Input {
    Close,
    Text(char),
    Backspace,
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
    notice: String,
    writes: bool,
}
#[derive(Clone, Copy, Default)]
enum EntryKind {
    #[default]
    Task,
    Capacity,
    NotBefore,
}
impl EntryKind {
    fn label(self) -> &'static str {
        match self {
            Self::Task => "New task ID (Enter submits, Esc discards)",
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
        if let Some(action) = action
            && self.worker.try_dispatch(action).is_err()
        {
            self.state.notice = "Service action pending; input was not queued or replayed.".into();
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
        self.state.entry.is_some()
    }

    pub(crate) fn status(&self) -> String {
        crate::status_text(
            &self.state.view,
            self.state.entry.as_deref(),
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
        if input == Input::Close || input == Input::Text('q') && self.entry.is_none() {
            return (Lifecycle::CloseRequested, None);
        }
        let action = if let Some(id) = &mut self.entry {
            match input {
                Input::Escape => {
                    self.entry = None;
                    None
                }
                Input::Enter if !busy => {
                    let value = self.entry.take().unwrap();
                    let action = match self.entry_kind {
                        EntryKind::Task => Some(Action::Submit(value.clone())),
                        EntryKind::Capacity => {
                            value.parse::<usize>().ok().filter(|n| *n > 0).map(|n| {
                                Action::PlanScheduling(SchedulingChange::SetConcurrencyLimit(n))
                            })
                        }
                        EntryKind::NotBefore => {
                            parse_not_before(&value).ok().and_then(|not_before| {
                                self.view.scheduling.selected.as_ref().map(|selected| {
                                    Action::PlanScheduling(
                                        SchedulingChange::SetSelectedEligibility {
                                            paused: selected.paused,
                                            not_before,
                                        },
                                    )
                                })
                            })
                        }
                    };
                    if action.is_none() {
                        self.entry = Some(value);
                        self.notice = "Invalid value; nothing was sent.".into();
                    }
                    action
                }
                Input::Backspace => {
                    id.pop();
                    None
                }
                Input::Text(ch) if !ch.is_control() && id.len() + ch.len_utf8() <= 256 => {
                    id.push(ch);
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
                    self.view.scheduling.selected.as_ref().map(|selected| {
                        Action::PlanScheduling(SchedulingChange::SetSelectedEligibility {
                            paused: !selected.paused,
                            not_before: selected.not_before,
                        })
                    })
                }
                Input::Text('b') if self.can_schedule(busy) && self.view.scheduling.enabled => {
                    self.entry_kind = EntryKind::Capacity;
                    self.entry = Some(String::new());
                    None
                }
                Input::Text('d')
                    if self.can_schedule(busy) && self.view.scheduling.selected.is_some() =>
                {
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
        for ch in "embedded-pane".chars() {
            pane.update(Input::Text(ch), true);
        }
        pane.update(Input::Enter, true);
        deliver(&mut pane);
        assert_eq!(pane.state.view.rows[0].phase, "ready");
        pane.update(Input::Text('c'), true);
        deliver(&mut pane);
        assert!(pane.state.view.pending_cancel.is_some());
        pane.update(Input::Text('y'), true);
        deliver(&mut pane);
        assert_eq!(pane.state.view.rows[0].phase, "cancelled");
        assert_eq!(pane.state.view.signals.requests, 5);
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
        assert!(
            matches!(state.update(Input::Enter, true, false).1, Some(Action::Submit(id)) if id.len() == 256)
        );
        assert!(state.entry.is_none());
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
    }
}
#[cfg(test)]
mod scheduling_tests;
