//! Terminal host for independent Chronos service projections.
use crate::agenda::{Agenda, Scope, WINDOW};
use crate::attempt::{Binding, Lookup};
use crate::explain::{Detail, Selection};
use crate::graph::Graph;
use crate::handoff::{Handoff, HandoffAction, HandoffRequest, HandoffResult};
use crate::planning::{Plan, day};
use crate::presence::Presence;
use crate::watch::Watch;
use crate::{Component, Input, Lifecycle, format_not_before, translate_key};
use chronos_model::{Row, View, display_id};
use chronos_protocol::dependencies_v6::Direction;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
};
use std::path::PathBuf;

pub struct Workstream {
    pub name: String,
    pub directory: PathBuf,
}

/// Bounded owner-published task fact used by composition hosts to build a
/// read-only operator agenda. This is not an approval, assignment, claim, or
/// scheduling decision.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperatorAgendaTask {
    pub task: String,
    pub incarnation: String,
    pub phase: &'static str,
    pub priority: i32,
    pub dependency_count: usize,
}

/// One Chronos workstream contribution to a host-owned operator-agenda
/// projection. `current == false` means the host must not present cached rows
/// as actionable current facts.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperatorAgendaObservation {
    pub workstream: String,
    pub revision: String,
    pub current: bool,
    pub truncated: bool,
    pub tasks: Vec<OperatorAgendaTask>,
}

/// Exact currently selected owner projection for cross-peer relationship
/// composition. Absence means there is no current stable selection; the host
/// must not invent a task, incarnation, revision, or attempt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SelectedTaskProjection {
    pub workstream: String,
    pub revision: String,
    pub task: String,
    pub incarnation: String,
    pub attempt: Option<u32>,
}

/// Host-invoked presentation intents. These enter the same bounded forms and
/// exact-confirmation paths as keyboard input; they do not bypass Chronos
/// authority or dispatch a mutation directly.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkbenchIntent {
    NewTask,
    CancelSelectedTask,
    OpenSchedule,
    SetTaskPause,
    SetTaskNotBefore,
}

pub fn validate_workstreams(workstreams: &[Workstream]) -> Result<(), &'static str> {
    if !(1..=32).contains(&workstreams.len()) {
        return Err("provide between one and 32 workstreams");
    }
    for (index, stream) in workstreams.iter().enumerate() {
        if stream.name.trim().is_empty()
            || stream.name.len() > 96
            || stream.name.chars().any(char::is_control)
            || workstreams[..index]
                .iter()
                .any(|previous| previous.name == stream.name)
        {
            return Err("workstream names must be distinct, nonempty and at most 96 bytes");
        }
        if stream.directory.as_os_str().is_empty() {
            return Err("workstream service directory is empty");
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Surface {
    Board,
    Tasks,
    Dependencies,
    Schedule,
    Execution,
    Overview,
    Plan,
}

impl Surface {
    const ALL: [Self; 7] = [
        Self::Board,
        Self::Tasks,
        Self::Dependencies,
        Self::Schedule,
        Self::Execution,
        Self::Overview,
        Self::Plan,
    ];

    fn title(self) -> &'static str {
        match self {
            Self::Board => "Board",
            Self::Tasks => "Tasks",
            Self::Dependencies => "Dependencies",
            Self::Schedule => "Schedule",
            Self::Execution => "Execution",
            Self::Overview => "Overview",
            Self::Plan => "Plan",
        }
    }
}

struct Stream {
    name: String,
    directory: PathBuf,
    component: Component,
    watch: Option<Watch>,
    moira: Option<Binding>,
    refreshing: bool,
    observation: u64,
}

pub struct Hub {
    streams: Vec<Stream>,
    active: usize,
    navigating: bool,
    surface: Surface,
    writes: bool,
    scheduling: bool,
    search: Option<Search>,
    explanation: Option<(Selection, Detail)>,
    presence: Option<Presence>,
    presence_open: bool,
    presence_scroll: u16,
    attempt_lookup: Option<(Option<(Selection, u32)>, Lookup)>,
    agenda: Option<(Scope, Agenda)>,
    agenda_visible: bool,
    graph: Option<(Selection, Graph)>,
    graph_visible: bool,
    plan: Option<Plan>,
    handoff: Handoff,
}

const ACTIVE: Style = Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD);
const DIM: Style = Style::new().fg(Color::Gray);

impl Hub {
    pub fn new(
        workstreams: Vec<Workstream>,
        writes: bool,
        scheduling: bool,
    ) -> Result<Self, &'static str> {
        Self::with_watch(workstreams, writes, scheduling, false)
    }

    pub fn with_watch(
        workstreams: Vec<Workstream>,
        writes: bool,
        scheduling: bool,
        watch: bool,
    ) -> Result<Self, &'static str> {
        Self::with_cancel_receipts(workstreams, writes, scheduling, watch, false)
    }

    /// Select V3 cancellation for a local atomic service profile.
    pub fn with_cancel_receipts(
        workstreams: Vec<Workstream>,
        writes: bool,
        scheduling: bool,
        watch: bool,
        cancel_receipts: bool,
    ) -> Result<Self, &'static str> {
        validate_workstreams(&workstreams)?;
        if cancel_receipts && (!writes || !cfg!(unix)) {
            return Err("--cancel-receipts requires a writable Unix hub");
        }
        let mut streams = Vec::new();
        for Workstream { name, directory } in workstreams {
            let observer = watch.then(|| Watch::start(directory.clone()));
            let client = chronos_local::LocalClient::new(directory.clone());
            #[cfg(unix)]
            let receipt_component = || {
                let client = crate::cancel_receipt::ReceiptClient(chronos_local::LocalClient::new(
                    directory.clone(),
                ));
                if scheduling {
                    Component::new_with_scheduling(client, writes)
                } else {
                    Component::new(client, writes)
                }
            };
            #[cfg(not(unix))]
            let receipt_component = || unreachable!();
            let component = if cancel_receipts {
                receipt_component()
            } else if scheduling {
                Component::new_with_scheduling(client, writes)
            } else {
                Component::new(client, writes)
            };
            streams.push(Stream {
                name,
                directory,
                component,
                watch: observer,
                moira: None,
                refreshing: false,
                observation: 0,
            });
        }
        Ok(Self {
            streams,
            active: 0,
            navigating: false,
            surface: Surface::Board,
            writes,
            scheduling,
            search: None,
            explanation: None,
            presence: None,
            presence_open: false,
            presence_scroll: 0,
            attempt_lookup: None,
            agenda: None,
            agenda_visible: false,
            graph: None,
            graph_visible: false,
            plan: None,
            handoff: Handoff::default(),
        })
    }

    pub fn load_planning(
        &mut self,
        path: &std::path::Path,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.plan = Some(Plan::load(path)?);
        Ok(())
    }

    pub fn bind_moira(
        &mut self,
        workstream: &str,
        socket: PathBuf,
        scope: String,
    ) -> Result<(), &'static str> {
        let stream = self
            .streams
            .iter_mut()
            .find(|stream| stream.name == workstream)
            .ok_or("unknown workstream for --moira")?;
        if stream.moira.is_some()
            || socket.as_os_str().is_empty()
            || !(1..=moira_protocol::attempt_lookup::MAX_SCOPE_BYTES).contains(&scope.len())
        {
            return Err("invalid or duplicate --moira binding");
        }
        stream.moira = Some(Binding {
            socket,
            scope,
            daimon: None,
        });
        Ok(())
    }

    pub fn bind_daimon(&mut self, workstream: &str, socket: PathBuf) -> Result<(), &'static str> {
        let binding = self
            .streams
            .iter_mut()
            .find(|stream| stream.name == workstream)
            .and_then(|stream| stream.moira.as_mut())
            .ok_or("--daimon requires a matching --moira binding")?;
        if binding.daimon.is_some() || socket.as_os_str().is_empty() {
            return Err("invalid or duplicate --daimon binding");
        }
        binding.daimon = Some(socket);
        Ok(())
    }

    /// The callback runs off the terminal thread and must reauthorize the live
    /// source and destination and revalidate the exact Chronos observation.
    pub fn bind_handoff(
        &mut self,
        host: impl Fn(HandoffRequest) -> HandoffResult + Send + Sync + 'static,
    ) {
        self.handoff.host = Some(std::sync::Arc::new(host));
    }

    pub const fn writes_enabled(&self) -> bool {
        self.writes
    }

    pub const fn scheduling_enabled(&self) -> bool {
        self.scheduling
    }

    /// Whether the active workstream is composing an owner-atomic task graph.
    /// A composition host may keep Tab inside this form instead of treating it
    /// as a global focus gesture.
    pub fn composing_task_batch(&self) -> bool {
        self.streams[self.active].component.composing_task_batch()
    }

    /// Whether the active peer is holding an exact bounded cancellation plan.
    pub fn confirming_cancel_batch(&self) -> bool {
        self.streams[self.active]
            .component
            .confirming_cancel_batch()
    }

    pub fn confirming_scheduling(&self) -> bool {
        self.streams[self.active].component.confirming_scheduling()
    }

    pub fn confirming_eligibility_batch(&self) -> bool {
        self.streams[self.active]
            .component
            .confirming_eligibility_batch()
    }

    /// Classifies the last delivered task mutation without exposing task IDs.
    pub fn task_mutation_disposition(&self) -> crate::TaskMutationDisposition {
        self.streams[self.active]
            .component
            .task_mutation_disposition()
    }

    pub fn scheduling_mutation_disposition(&self) -> crate::SchedulingMutationDisposition {
        self.streams[self.active]
            .component
            .scheduling_mutation_disposition()
    }

    pub fn mutation_recovery_required(&self) -> bool {
        self.streams
            .iter()
            .any(|stream| stream.component.mutation_recovery_required())
    }

    /// Projects blocked and uncertain tasks from each bounded current owner
    /// page. A composition host may route the operator back to Chronos, but
    /// must not infer authority or mutate from this observation.
    pub fn operator_agenda_observations(&self) -> Vec<OperatorAgendaObservation> {
        self.streams
            .iter()
            .enumerate()
            .map(|(index, stream)| {
                let current = self.stream_view(index);
                let view = stream.component.view();
                OperatorAgendaObservation {
                    workstream: stream.name.clone(),
                    revision: view.revision.clone(),
                    current: current.is_some(),
                    truncated: view.has_next,
                    tasks: current
                        .into_iter()
                        .flat_map(|view| view.rows.iter())
                        .filter(|row| row.phase == "blocked" || row.phase == "uncertain")
                        .map(|row| OperatorAgendaTask {
                            task: row.reference.task.clone(),
                            incarnation: row.reference.incarnation.clone(),
                            phase: row.phase,
                            priority: row.priority,
                            dependency_count: row.dependency_count,
                        })
                        .collect(),
                }
            })
            .collect()
    }

    /// Enters one peer-owned interactive workflow from a composition host.
    /// Current revision, freshness, pending work and selection are checked
    /// before the ordinary component input is admitted.
    pub fn invoke_workbench(
        &mut self,
        intent: WorkbenchIntent,
    ) -> Result<&'static str, &'static str> {
        self.navigating = false;
        match intent {
            WorkbenchIntent::OpenSchedule => {
                if !self.scheduling {
                    return Err("Chronos scheduling was not selected for this session");
                }
                self.surface = Surface::Schedule;
                Ok("Opened Chronos scheduling")
            }
            WorkbenchIntent::SetTaskPause | WorkbenchIntent::SetTaskNotBefore => {
                if !self.writes || !self.scheduling {
                    return Err("Chronos scheduling writes are disabled for this session");
                }
                let stream = &self.streams[self.active];
                let view = stream.component.view();
                let observed_targets = if view.marked_cancel.is_empty() {
                    usize::from(view.scheduling.selected.is_some())
                } else {
                    view.scheduling.marked.len()
                };
                let expected_targets = view.marked_cancel.len().max(1);
                if !view.scheduling.enabled
                    || view.scheduling.mutation_blocked
                    || observed_targets != expected_targets
                {
                    return Err(
                        "Refresh Chronos scheduling and every marked/current eligibility before writing",
                    );
                }
                if stream.component.editing()
                    || stream.component.has_pending()
                    || stream.component.is_busy()
                {
                    return Err("Chronos already has an interactive action in flight");
                }
                self.surface = Surface::Schedule;
                let (key, message) = match intent {
                    WorkbenchIntent::SetTaskPause => (
                        't',
                        "Opened Chronos's marked/current task-pause confirmation",
                    ),
                    WorkbenchIntent::SetTaskNotBefore => {
                        ('d', "Opened Chronos's marked/current not-before form")
                    }
                    _ => unreachable!(),
                };
                self.update(KeyEvent::new(KeyCode::Char(key), KeyModifiers::NONE));
                Ok(message)
            }
            WorkbenchIntent::NewTask => {
                if !self.writes {
                    return Err("Chronos writes are disabled for this session");
                }
                let stream = &self.streams[self.active];
                if self.current_view().is_none() {
                    return Err("Chronos task state is stale; refresh before writing");
                }
                if stream.component.editing()
                    || stream.component.has_pending()
                    || stream.component.is_busy()
                {
                    return Err("Chronos already has an interactive action in flight");
                }
                self.dismiss_presentation_overlays();
                self.surface = Surface::Board;
                self.update(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));
                Ok("Opened the bounded Chronos new-task form")
            }
            WorkbenchIntent::CancelSelectedTask => {
                if !self.writes {
                    return Err("Chronos writes are disabled for this session");
                }
                let stream = &self.streams[self.active];
                let Some(view) = self.current_view() else {
                    return Err("Chronos task state is stale; refresh before writing");
                };
                if view.rows.get(view.selected).is_none() {
                    return Err("Select a current Chronos task before cancelling");
                }
                if stream.component.editing()
                    || stream.component.has_pending()
                    || stream.component.is_busy()
                {
                    return Err("Chronos already has an interactive action in flight");
                }
                self.dismiss_presentation_overlays();
                self.surface = Surface::Board;
                self.update(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE));
                Ok("Opened Chronos's exact marked/current cancellation confirmation")
            }
        }
    }

    fn dismiss_presentation_overlays(&mut self) {
        self.presence_open = false;
        self.presence_scroll = 0;
        self.graph_visible = false;
        self.search = None;
        self.agenda_visible = false;
        self.handoff.pending = None;
    }

    fn start_handoff(&mut self, action: HandoffAction, target: Selection) {
        if self.explanation_selection().as_ref() != Some(&target) || target.revision == "exhausted"
        {
            self.handoff.notice = "Handoff target changed. Refresh before trying again.";
            return;
        }
        self.handoff.start(HandoffRequest {
            action,
            workstream: self.streams[target.stream].name.clone(),
            task: chronos_protocol::Reference {
                task: target.task,
                incarnation: target.incarnation,
            },
            revision: chronos_protocol::Revision::Finite(target.revision),
        });
    }

    pub fn poll(&mut self) -> bool {
        let mut changed = self.handoff.poll();
        if self.presence.as_mut().is_some_and(Presence::poll) {
            changed = true;
            if !self.presence_open {
                self.presence = None;
            }
        }
        for stream in &mut self.streams {
            let delivered = stream.component.poll();
            changed |= delivered;
            let mut invalidate_details = delivered;
            if let Some(watch) = &mut stream.watch {
                // Reconcile a delivered refresh before consuming a new
                // generation observation that may require another refresh.
                if delivered && stream.refreshing {
                    stream.refreshing = false;
                    if stream.component.view().revision.parse::<u64>().is_ok()
                        && !stream.component.view().mutation_blocked
                    {
                        watch.resync = false;
                    }
                }
                let was_resync = watch.resync;
                let (received, feed_changed) = watch.poll();
                changed |= feed_changed;
                invalidate_details |= feed_changed && (watch.resync || watch.revision().is_none());
                if !was_resync && watch.resync {
                    stream.refreshing = false;
                }
                let view = stream.component.view();
                let behind = watch.revision().is_some_and(|head| {
                    view.revision
                        .parse::<u64>()
                        .is_ok_and(|current| current < head)
                });
                if received
                    && watch.revision().is_some()
                    && (watch.resync || behind || view.mutation_blocked)
                {
                    stream.refreshing |= stream.component.refresh_from_feed();
                }
            }
            if invalidate_details {
                stream.observation = stream.observation.wrapping_add(1);
            }
        }
        if let Some((_, detail)) = &mut self.explanation {
            changed |= detail.poll();
        }
        if self.attempt_lookup.is_some() {
            let selected = if self.surface == Surface::Execution
                && !self.navigating
                && self.search.is_none()
            {
                self.attempt_selection()
            } else {
                None
            };
            if let Some((target, lookup)) = &mut self.attempt_lookup {
                if *target != selected {
                    changed |= target.take().is_some();
                }
                changed |= lookup.poll();
                // Keep at most one worker until the abandoned request finishes.
                if target.is_none() && !matches!(lookup, Lookup::Reading(_)) {
                    self.attempt_lookup = None;
                }
            }
        }
        if let Some((_, agenda)) = &mut self.agenda {
            changed |= agenda.poll();
        }
        if let Some((_, graph)) = &mut self.graph {
            changed |= graph.poll();
        }
        changed
    }

    pub fn update(&mut self, key: KeyEvent) -> Lifecycle {
        let input = translate_key(key);
        if input == Input::Close {
            return Lifecycle::CloseRequested;
        }
        if self.presence_open {
            if input == Input::Escape {
                self.presence_open = false;
                if matches!(self.presence, Some(Presence::Ready(..))) {
                    self.presence = None;
                }
            } else if input == Input::Text('l') {
                self.open_presence();
            } else if input == Input::Up {
                self.presence_scroll = self.presence_scroll.saturating_sub(1);
            } else if input == Input::Down {
                self.presence_scroll = self.presence_scroll.saturating_add(1);
            }
            return Lifecycle::Continue;
        }
        if self.graph_visible {
            if input == Input::Escape {
                self.graph_visible = false;
            } else if let Some(selection) = self.explanation_selection()
                && let Some((scope, graph)) = &mut self.graph
                && *scope == selection
            {
                match input {
                    Input::Up => graph.selected = graph.selected.saturating_sub(1),
                    Input::Down => graph.down(),
                    Input::Enter => graph.follow(),
                    _ if key_event_direction(key) => {
                        graph.direction(if key.code == KeyCode::Left {
                            Direction::Prerequisites
                        } else {
                            Direction::Dependents
                        });
                    }
                    _ => {}
                }
            }
            return Lifecycle::Continue;
        }
        if self.search.is_some() {
            self.update_search(input);
            return Lifecycle::Continue;
        }
        if let Some(target) = self.handoff.pending.take() {
            if input == Input::Enter {
                self.start_handoff(HandoffAction::Export, target);
                return Lifecycle::Continue;
            }
            if input == Input::Ignore {
                self.handoff.pending = Some(target);
                return Lifecycle::Continue;
            }
            self.handoff.notice = "Handoff confirmation discarded.";
            if input == Input::Escape {
                return Lifecycle::Continue;
            }
        }
        if !self.streams[self.active].component.editing()
            && key.kind == KeyEventKind::Press
            && key.modifiers == KeyModifiers::NONE
        {
            match key.code {
                KeyCode::Char('h' | 'v')
                    if self.surface == Surface::Tasks
                        && !self.navigating
                        && self.handoff.host.is_some() =>
                {
                    if self.handoff.worker.is_none()
                        && let Some(target) = self.explanation_selection()
                    {
                        if key.code == KeyCode::Char('h') {
                            if self.handoff.can_export() {
                                self.handoff.pending = Some(target);
                            } else {
                                self.handoff.notice = "Export outcome unknown. Inspect the destination with the host.";
                            }
                        } else {
                            self.start_handoff(HandoffAction::Review, target);
                        }
                    }
                    return Lifecycle::Continue;
                }
                KeyCode::Char('a') if self.surface == Surface::Schedule && !self.navigating => {
                    self.agenda_visible = true;
                    if !matches!(self.agenda, Some((_, Agenda::Reading(_))))
                        && let Some(scope) = self.agenda_scope()
                    {
                        let stream = &self.streams[self.active];
                        let tasks = stream.component.view().rows[scope.first..scope.last]
                            .iter()
                            .map(|row| row.reference.clone())
                            .collect();
                        let agenda =
                            Agenda::start(stream.directory.clone(), scope.revision.clone(), tasks);
                        self.agenda = Some((scope, agenda));
                    }
                    return Lifecycle::Continue;
                }
                KeyCode::Char('g') if self.surface == Surface::Dependencies && !self.navigating => {
                    if !self.graph.as_ref().is_some_and(|(_, graph)| graph.busy())
                        && let Some(selection) = self.explanation_selection()
                        && selection.revision != "exhausted"
                    {
                        let stream = &self.streams[self.active];
                        let view = stream.component.view();
                        let graph = Graph::open(
                            stream.directory.clone(),
                            selection.revision.clone(),
                            view.rows[view.selected].reference.clone(),
                        );
                        self.graph = Some((selection, graph));
                        self.graph_visible = true;
                    }
                    return Lifecycle::Continue;
                }
                KeyCode::Char('x') if self.surface == Surface::Dependencies && !self.navigating => {
                    if !matches!(self.explanation, Some((_, Detail::Reading(_))))
                        && let Some(selection) = self.explanation_selection()
                    {
                        let detail =
                            Detail::start(self.streams[self.active].directory.clone(), &selection);
                        self.explanation = Some((selection, detail));
                    }
                    return Lifecycle::Continue;
                }
                KeyCode::Char('l') if self.surface == Surface::Execution && !self.navigating => {
                    self.open_presence();
                    return Lifecycle::Continue;
                }
                KeyCode::Char('x') if self.surface == Surface::Execution && !self.navigating => {
                    if !matches!(self.attempt_lookup, Some((_, Lookup::Reading(_))))
                        && let Some((selection, attempt)) = self.attempt_selection()
                        && let Some(binding) = self.streams[self.active].moira.clone()
                        && let Ok(incarnation) = selection.incarnation.parse()
                    {
                        let lookup =
                            Lookup::start(binding, selection.task.clone(), incarnation, attempt);
                        self.attempt_lookup = Some((Some((selection, attempt)), lookup));
                    }
                    return Lifecycle::Continue;
                }
                KeyCode::Char('/' | 'w') => {
                    self.search = Some(Search {
                        tasks: key.code == KeyCode::Char('/'),
                        query: String::new(),
                        selected: 0,
                        notice: "",
                    });
                    return Lifecycle::Continue;
                }
                KeyCode::Tab if self.streams.len() > 1 => {
                    self.navigating = !self.navigating;
                    return Lifecycle::Continue;
                }
                KeyCode::Char('1'..='7') => {
                    let KeyCode::Char(digit) = key.code else {
                        unreachable!()
                    };
                    self.surface = Surface::ALL[digit as usize - '1' as usize];
                    self.agenda_visible = false;
                    self.navigating = false;
                    return Lifecycle::Continue;
                }
                _ => {}
            }
        }
        if self.navigating {
            match input {
                Input::Up => self.active = self.active.saturating_sub(1),
                Input::Down => self.active = (self.active + 1).min(self.streams.len() - 1),
                Input::Enter | Input::Escape => self.navigating = false,
                Input::Text('q') => return Lifecycle::CloseRequested,
                _ => {}
            }
            return Lifecycle::Continue;
        }
        if self.surface == Surface::Plan {
            if input == Input::Text('q') {
                return Lifecycle::CloseRequested;
            }
            if let Some(plan) = &mut self.plan {
                match input {
                    Input::Up => plan.selected = plan.selected.saturating_sub(1),
                    Input::Down => {
                        plan.selected = (plan.selected + 1).min(plan.items.len().saturating_sub(1))
                    }
                    _ => {}
                }
            }
            return Lifecycle::Continue;
        }
        if self.surface == Surface::Overview {
            match input {
                Input::Up => self.active = self.active.saturating_sub(1),
                Input::Down => self.active = (self.active + 1).min(self.streams.len() - 1),
                Input::Enter => self.surface = Surface::Board,
                Input::Text('q') => return Lifecycle::CloseRequested,
                _ => {}
            }
            return Lifecycle::Continue;
        }
        let stream = &mut self.streams[self.active];
        if let Some(watch) = &stream.watch
            && (watch.resync || watch.revision().is_none())
            && (matches!(
                input,
                Input::Text('n' | 'c' | 'e' | 'p' | 't' | 'b' | 'd' | 'y') | Input::Enter
            ))
            && (!stream.component.editing() || input == Input::Enter)
        {
            stream.component.feed_notice();
            return Lifecycle::Continue;
        }
        let component = &mut stream.component;
        if self.surface == Surface::Board
            && !component.editing()
            && key.kind != KeyEventKind::Release
            && key.modifiers == KeyModifiers::NONE
            && matches!(key.code, KeyCode::Left | KeyCode::Right)
        {
            if let Some(index) = adjacent_lane(component.view(), key.code == KeyCode::Right) {
                component.select(index);
            }
            return Lifecycle::Continue;
        }
        component.update(input, true)
    }

    fn stream_view(&self, index: usize) -> Option<&View> {
        let stream = self.streams.get(index)?;
        let view = stream.component.view();
        if view.mutation_blocked && view.revision != "exhausted"
            || stream.watch.as_ref().is_some_and(|watch| {
                watch.resync
                    || watch.revision().is_none_or(|head| {
                        view.revision
                            .parse::<u64>()
                            .is_ok_and(|revision| revision < head)
                    })
            })
        {
            return None;
        }
        Some(view)
    }

    fn current_view(&self) -> Option<&View> {
        self.stream_view(self.active)
    }

    fn explanation_selection(&self) -> Option<Selection> {
        let view = self.current_view()?;
        let row = view.rows.get(view.selected)?;
        Some(Selection::new(
            self.active,
            self.streams[self.active].observation,
            &row.reference,
            &view.revision,
        ))
    }

    fn attempt_selection(&self) -> Option<(Selection, u32)> {
        let view = self.current_view()?;
        let attempt = view.rows.get(view.selected)?.attempt?;
        Some((self.explanation_selection()?, attempt))
    }

    fn open_presence(&mut self) {
        if !self.presence_open && matches!(self.presence, Some(Presence::Reading(_))) {
            return;
        }
        self.presence_open = true;
        self.presence_scroll = 0;
        if !matches!(self.presence, Some(Presence::Reading(_))) {
            self.presence = self.streams[self.active]
                .moira
                .as_ref()
                .and_then(|binding| binding.daimon.clone())
                .map(Presence::start);
        }
    }

    fn attempt_text(&self) -> String {
        if self.streams[self.active].moira.is_none() {
            return "Moira lookup is not configured.".into();
        }
        let Some((selection, attempt)) = self.attempt_selection() else {
            return "Select a current Chronos attempt.".into();
        };
        self.attempt_lookup
            .as_ref()
            .filter(|(target, _)| {
                target
                    .as_ref()
                    .is_some_and(|(key, number)| *key == selection && *number == attempt)
            })
            .map_or_else(
                || "x inspect Moira receipt".into(),
                |(_, lookup)| lookup.lines(),
            )
    }

    fn agenda_scope(&self) -> Option<Scope> {
        let view = self.current_view()?;
        if !view.scheduling.supported {
            return None;
        }
        let first = (view.selected / WINDOW) * WINDOW;
        let row = view.rows.get(first)?;
        Some(Scope {
            stream: self.active,
            observation: self.streams[self.active].observation,
            revision: view.revision.clone(),
            first,
            last: (first + WINDOW).min(view.rows.len()),
            first_incarnation: row.reference.incarnation.clone(),
            first_task: row.reference.task.clone(),
        })
    }

    /// Whether the selected service has delivered a current task page.
    pub fn has_current_tasks(&self) -> bool {
        self.current_view()
            .is_some_and(|view| view.revision != "unknown")
    }

    pub fn selected_task(&self) -> Option<SelectedTaskProjection> {
        let stream = self.streams.get(self.active)?;
        let view = self.current_view()?;
        if view.revision == "unknown" {
            return None;
        }
        let row = view.rows.get(view.selected)?;
        Some(SelectedTaskProjection {
            workstream: stream.name.clone(),
            revision: view.revision.clone(),
            task: row.reference.task.clone(),
            incarnation: row.reference.incarnation.clone(),
            attempt: row.attempt,
        })
    }

    pub fn draw(&self, frame: &mut Frame<'_>) {
        self.draw_in(frame, frame.area());
    }

    /// Render within the composition host's pane.
    pub fn draw_in(&self, frame: &mut Frame<'_>, area: Rect) {
        let area = area.intersection(frame.area());
        if area.is_empty() {
            return;
        }
        let stream = &self.streams[self.active];
        let view = stream.component.view();
        let [header, body, status, keys] = Layout::vertical([
            Constraint::Length(3),
            Constraint::Min(1),
            Constraint::Length(5),
            Constraint::Length(2),
        ])
        .areas(area);
        let mut tabs = vec![Span::styled(" CHRONOS   ", ACTIVE)];
        for (index, surface) in Surface::ALL.into_iter().enumerate() {
            let style = if self.surface == surface { ACTIVE } else { DIM };
            tabs.push(Span::styled(
                format!(" {} {} ", index + 1, surface.title()),
                style,
            ));
        }
        let feed = stream
            .watch
            .as_ref()
            .map(|watch| feed_status(watch, view))
            .unwrap_or("");
        let mode = if self.writes {
            "write-enabled"
        } else {
            "read-only"
        };
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(tabs),
                Line::from(format!(
                    " {}  ·  revision {}  ·  {mode}",
                    stream.name, view.revision
                )),
                Line::styled(feed, DIM),
            ]),
            header,
        );

        if self.presence_open {
            let text = self
                .presence
                .as_ref()
                .map_or_else(|| "Daimon roster is not configured".into(), Presence::lines);
            let block = panel("Daimon-supervised agents", true);
            let inner = block.inner(body);
            let lines = text.lines().count();
            let paragraph = Paragraph::new(text).block(block);
            let max_scroll = lines.saturating_sub(usize::from(inner.height)) as u16;
            frame.render_widget(
                paragraph.scroll((self.presence_scroll.min(max_scroll), 0)),
                body,
            );
            frame.render_widget(
                Paragraph::new(" ↑/↓ scroll   l refresh roster   Esc return").style(DIM),
                keys,
            );
            return;
        }
        if self.graph_visible {
            self.draw_graph(frame, body);
            frame.render_widget(
                Paragraph::new(
                    " ← prerequisites   → dependents   ↑/↓ select   Enter follow   Esc return",
                )
                .style(DIM),
                keys,
            );
            return;
        }
        if let Some(search) = &self.search {
            self.draw_search(frame, body, search);
            frame.render_widget(Paragraph::new(search.notice).style(DIM), status);
            frame.render_widget(
                Paragraph::new(" ↑/↓ select   Enter open   Esc return").style(DIM),
                keys,
            );
            return;
        }
        if self.surface == Surface::Plan && !self.navigating {
            self.draw_plan(frame, body, status);
            frame.render_widget(
                Paragraph::new(" ↑/↓ plan item   1 return to board   q quit").style(DIM),
                keys,
            );
            return;
        }
        if body.width >= 99 {
            let [left, middle, right] = Layout::horizontal([
                Constraint::Length(23),
                Constraint::Min(43),
                Constraint::Length(29),
            ])
            .spacing(1)
            .areas(body);
            self.sidebar(frame, left);
            self.center(frame, middle, view);
            self.inspector(frame, right, view);
        } else if self.navigating {
            self.sidebar(frame, body);
        } else if body.width >= 60 {
            let [middle, right] =
                Layout::horizontal([Constraint::Min(36), Constraint::Length(24)]).areas(body);
            self.center(frame, middle, view);
            self.inspector(frame, right, view);
        } else {
            self.center(frame, body, view);
        }

        let handoff_status = if self.surface == Surface::Tasks && self.handoff.host.is_some() {
            if let Some(target) = &self.handoff.pending {
                format!(
                    "Export handoff for {}? Enter confirms · Esc discards",
                    display_id(&target.task)
                )
            } else {
                self.handoff.notice.to_owned()
            }
        } else {
            String::new()
        };
        let status_text = if handoff_status.is_empty() {
            stream.component.status()
        } else {
            format!("{}\n{handoff_status}", stream.component.status())
        };
        frame.render_widget(
            Paragraph::new(status_text)
                .block(panel("Activity", false))
                .wrap(Wrap { trim: true }),
            status,
        );
        let mut footer = vec![Line::from(if self.navigating {
            " ↑/↓ workstream   Enter return to tasks   Tab switch focus   q quit"
        } else if self.surface == Surface::Schedule {
            " w switch   ↑/↓ task   Space mark   a agenda   s refresh schedule   r tasks   q quit"
        } else if self.surface == Surface::Dependencies {
            " w switch   / search   ↑/↓ task   x explain   g graph   r refresh   q quit"
        } else if self.surface == Surface::Overview {
            " ↑/↓ workstream   Enter open board   q quit"
        } else if self.surface == Surface::Board {
            " w switch   / search   ←/→ lane   ↑/↓ task   Space mark   r refresh   n new   c cancel set   q quit"
        } else {
            " w switch   / search   ↑/↓ task   Space mark   r refresh   ] next   n new   c cancel set   q quit"
        })];
        if self.surface == Surface::Tasks && self.handoff.host.is_some() {
            footer.push(Line::from(" h export handoff   v review handoff"));
        } else if view.scheduling.supported && self.surface != Surface::Overview {
            footer.push(Line::from(" e enable   p global pause   t set marked/current pause   b capacity   d marked/current not-before"));
        }
        frame.render_widget(Paragraph::new(footer).style(DIM), keys);
    }

    fn draw_plan(&self, frame: &mut Frame<'_>, area: Rect, details: Rect) {
        let block = panel("Gantt · supplied planning dates", true);
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let Some(plan) = &self.plan else {
            frame.render_widget(
                Paragraph::new("Launch with --planning PATH to open a planning source."),
                inner,
            );
            return;
        };
        let [header, body] =
            Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(inner);
        let label_width = usize::from((body.width / 3).min(32));
        let width = usize::from(body.width).saturating_sub(label_width + 4);
        let range = plan.range();
        let header_text = match range {
            Some((start, end)) => format!("{start} to {end} · inclusive civil dates"),
            None => "No complete planned ranges".into(),
        };
        frame.render_widget(Paragraph::new(header_text).style(DIM), header);
        let rows = plan.items.iter().map(|item| {
            let label = format!("{} / {}", item.workstream, item.label);
            let label: String = label.chars().take(label_width).collect();
            let bar = match (range, item.start.as_deref(), item.end.as_deref()) {
                (Some((minimum, maximum)), Some(start), Some(end)) if width > 0 => {
                    let min = day(minimum).unwrap();
                    let span = day(maximum).unwrap() - min;
                    let scale = |date| (day(date).unwrap() - min) * (width - 1) / span.max(1);
                    let first = scale(start);
                    let last = scale(end);
                    format!("{}{}", " ".repeat(first), "━".repeat(last - first + 1))
                }
                (_, None, _) | (_, _, None) => "No planned range".into(),
                _ => String::new(),
            };
            ListItem::new(format!("{label:<label_width$}  {bar}"))
        });
        let mut state = ListState::default()
            .with_selected(plan.items.get(plan.selected).map(|_| plan.selected));
        frame.render_stateful_widget(
            List::new(rows)
                .highlight_symbol("› ")
                .highlight_style(ACTIVE),
            body,
            &mut state,
        );
        if let Some(item) = plan.items.get(plan.selected) {
            let info = format!(
                "{} / {}\nStart {}   End {}   Due {}",
                item.workstream,
                item.label,
                item.start.as_deref().unwrap_or("—"),
                item.end.as_deref().unwrap_or("—"),
                item.due.as_deref().unwrap_or("—")
            );
            frame.render_widget(
                Paragraph::new(info)
                    .block(panel("Plan item", false))
                    .wrap(Wrap { trim: true }),
                details,
            );
        }
    }

    fn draw_graph(&self, frame: &mut Frame<'_>, area: Rect) {
        let block = panel("Dependency graph · visible edges only", true);
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let Some(selection) = self.explanation_selection() else {
            frame.render_widget(
                Paragraph::new("Graph is stale. Return to the task view and refresh."),
                inner,
            );
            return;
        };
        let Some((_, graph)) = self.graph.as_ref().filter(|(scope, _)| *scope == selection) else {
            frame.render_widget(
                Paragraph::new("Task observation changed. Return and reopen the graph."),
                inner,
            );
            return;
        };
        let [heading, content] =
            Layout::vertical([Constraint::Length(2), Constraint::Min(0)]).areas(inner);
        let (neighbors, status) = graph.rows();
        frame.render_widget(
            Paragraph::new(vec![Line::from(graph.title()), Line::styled(status, DIM)]),
            heading,
        );
        let mut state = ListState::default()
            .with_selected(neighbors.get(graph.selected).map(|_| graph.selected));
        frame.render_stateful_widget(
            List::new(neighbors)
                .highlight_symbol("› ")
                .highlight_style(ACTIVE),
            content,
            &mut state,
        );
    }

    fn update_search(&mut self, input: Input) {
        let Some(mut search) = self.search.take() else {
            return;
        };
        search.notice = "";
        match input {
            Input::Escape => return,
            Input::Backspace => {
                search.query.pop();
                search.selected = 0;
            }
            Input::Text(ch) if search.query.len() + ch.len_utf8() <= 256 => {
                search.query.push(ch);
                search.selected = 0;
            }
            Input::Up => search.selected = search.selected.saturating_sub(1),
            Input::Down => {
                search.selected = (search.selected + 1)
                    .min(search.matches(&self.streams).len().saturating_sub(1));
            }
            Input::Enter => {
                if let Some(&(stream, task)) = search.matches(&self.streams).get(search.selected) {
                    if task.is_none_or(|index| self.streams[stream].component.select(index)) {
                        self.active = stream;
                        self.navigating = false;
                        self.surface = Surface::Board;
                        return;
                    }
                    search.notice = "Selection unavailable while the workstream is busy or confirming another task.";
                }
            }
            _ => {}
        }
        self.search = Some(search);
    }

    fn draw_search(&self, frame: &mut Frame<'_>, area: Rect, search: &Search) {
        let block = panel(
            if search.tasks {
                "Cross-workstream tasks"
            } else {
                "Switch workstream"
            },
            true,
        );
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let [heading, results] =
            Layout::vertical([Constraint::Length(2), Constraint::Min(0)]).areas(inner);
        let unseen = self
            .streams
            .iter()
            .filter(|stream| {
                let view = stream.component.view();
                view.mutation_blocked && view.revision != "exhausted"
            })
            .count();
        let partial = self
            .streams
            .iter()
            .filter(|stream| stream.component.view().has_next)
            .count();
        let matches = search.matches(&self.streams);
        let detail = if search.tasks {
            format!(
                "{} matches on observed pages · {partial} with more pages · {unseen} unverified",
                matches.len()
            )
        } else {
            format!("{} workstreams · {unseen} unverified", matches.len())
        };
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(format!("Search: {}", search.query)),
                Line::styled(detail, DIM),
            ]),
            heading,
        );
        let items = matches.iter().map(|&(index, task)| {
            let stream = &self.streams[index];
            let view = stream.component.view();
            let uncertain = view.mutation_blocked && view.revision != "exhausted";
            let status = if uncertain { " [unverified]" } else { "" };
            ListItem::new(if let Some(task) = task {
                let row = &view.rows[task];
                format!(
                    "{} · {} · {}{status}",
                    display_id(&row.reference.task),
                    row.phase,
                    stream.name
                )
            } else {
                format!("{}{status}", stream.name)
            })
        });
        let mut state = ListState::default()
            .with_selected(matches.get(search.selected).map(|_| search.selected));
        frame.render_stateful_widget(
            List::new(items)
                .highlight_symbol("› ")
                .highlight_style(ACTIVE),
            results,
            &mut state,
        );
    }

    fn sidebar(&self, frame: &mut Frame<'_>, area: Rect) {
        let mut rows = Vec::new();
        let mut project = None;
        let mut selected = 0;
        for (index, stream) in self.streams.iter().enumerate() {
            let (group, label) = stream
                .name
                .split_once(" / ")
                .map_or((None, stream.name.as_str()), |(group, label)| {
                    (Some(group), label)
                });
            if group != project {
                if let Some(group) = group {
                    rows.push(ListItem::new(group).style(DIM.add_modifier(Modifier::BOLD)));
                }
                project = group;
            }
            if index == self.active {
                selected = rows.len();
            }
            let view = stream.component.view();
            let pending = view.pending_cancel.is_some() || view.scheduling.pending.is_some();
            rows.push(ListItem::new(format!(
                "{}{}{}",
                if group.is_some() { "  " } else { "" },
                if pending { "* " } else { "" },
                label,
            )));
        }
        let mut state = ListState::default().with_selected(Some(selected));
        frame.render_stateful_widget(
            List::new(rows)
                .block(panel("Workstreams", self.navigating))
                .highlight_symbol("› ")
                .highlight_style(ACTIVE),
            area,
            &mut state,
        );
    }

    fn overview(&self, frame: &mut Frame<'_>, area: Rect) {
        let rows = self.streams.iter().map(|stream| {
            let view = stream.component.view();
            let mut counts = [0; 4];
            for row in &view.rows {
                counts[lane(row)] += 1;
            }
            let feed = stream
                .watch
                .as_ref()
                .map(|watch| format!(" · {}", feed_status(watch, view)))
                .unwrap_or_default();
            let condition = if view.pending_cancel.is_some() || view.scheduling.pending.is_some() {
                " · confirmation pending"
            } else if view.mutation_blocked {
                " · writes blocked"
            } else {
                ""
            };
            let [blocked, ready, active, closed] = counts;
            ListItem::new(vec![
                Line::from(format!("{}{condition}{feed}", stream.name)),
                Line::from(format!(
                    "Blocked {blocked}  Ready {ready}  Active {active}  Closed {closed}{}",
                    if view.has_next { "  + pages" } else { "" },
                ))
                .style(DIM),
            ])
        });
        let mut state = ListState::default().with_selected(Some(self.active));
        frame.render_stateful_widget(
            List::new(rows)
                .block(panel(
                    "Workstream overview · observed pages",
                    !self.navigating,
                ))
                .highlight_symbol("› ")
                .highlight_style(ACTIVE),
            area,
            &mut state,
        );
    }

    fn inspector(&self, frame: &mut Frame<'_>, area: Rect, view: &View) {
        let explanation = if self.surface == Surface::Dependencies {
            self.explanation_text()
        } else if self.surface == Surface::Execution {
            self.attempt_text()
        } else {
            String::new()
        };
        inspector(frame, area, view, &explanation);
    }

    fn explanation_text(&self) -> String {
        let Some(selection) = self.explanation_selection() else {
            return "Select a task on a current page to explain readiness.".into();
        };
        self.explanation
            .as_ref()
            .filter(|(key, _)| *key == selection)
            .map_or_else(
                || "x explain readiness".into(),
                |(_, detail)| detail.lines(),
            )
    }

    fn center(&self, frame: &mut Frame<'_>, area: Rect, view: &View) {
        if self.surface == Surface::Board && area.width >= 43 {
            board(frame, area, view, !self.navigating);
            return;
        }
        if self.surface == Surface::Overview {
            self.overview(frame, area);
            return;
        }
        let title = match self.surface {
            Surface::Overview => unreachable!("overview is rendered separately"),
            Surface::Plan => "Tasks",
            Surface::Dependencies => "Dependency readiness",
            Surface::Schedule => "Scheduling",
            Surface::Execution => "Execution attempts",
            Surface::Tasks | Surface::Board => "Tasks",
        };
        let block = panel(title, !self.navigating);
        let inner = block.inner(area);
        frame.render_widget(block, area);
        if self.surface == Surface::Schedule {
            let s = &view.scheduling;
            let mut text = if s.supported {
                format!(
                    "Scheduling revision {}\nEnabled {}   Paused {}\nCapacity {:?}   Available {:?}\nMarked eligibility set {}\n\nSelected task not-before\n{}\n\nPress s to refresh.\nTask date ranges are unavailable.",
                    s.revision,
                    s.enabled,
                    s.paused,
                    s.capacity,
                    s.available,
                    s.marked.len(),
                    format_not_before(s.selected.as_ref().and_then(|entry| entry.not_before)),
                )
            } else {
                "Launch with --scheduling to inspect task eligibility and capacity.".into()
            };
            if let Some(scope) = self.agenda_scope() {
                if self.agenda_visible
                    && let Some((_, agenda)) = self.agenda.as_ref().filter(|(key, _)| *key == scope)
                {
                    let rows = match agenda {
                        Agenda::Reading(_) => "Reading eligibility…".into(),
                        Agenda::Ready(Ok(rows)) => rows.join("\n"),
                        Agenda::Ready(Err(message)) => (*message).into(),
                    };
                    text = format!(
                        "Eligibility agenda · observed tasks {}–{} of {}\nNot-before (Unix seconds)   Task\n{}",
                        scope.first + 1,
                        scope.last,
                        view.rows.len(),
                        rows,
                    );
                } else {
                    text.push_str("\n\nPress a to load the eligibility agenda.");
                }
            }
            frame.render_widget(Paragraph::new(text).wrap(Wrap { trim: true }), inner);
            return;
        }
        let note = match self.surface {
            Surface::Dependencies => "Prerequisites   Phase        Task · g opens visible edges",
            Surface::Execution => "Chronos task attempts · l roster",
            _ if view.rows.is_empty() => "No tasks on this page",
            _ => "Phase          Task",
        };
        let narrow_detail =
            matches!(self.surface, Surface::Dependencies | Surface::Execution) && area.width < 60;
        let [heading, rows, detail] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(if narrow_detail { 7 } else { 0 }),
        ])
        .areas(inner);
        frame.render_widget(Paragraph::new(note).style(DIM), heading);
        if narrow_detail {
            frame.render_widget(
                Paragraph::new(if self.surface == Surface::Execution {
                    self.attempt_text()
                } else {
                    self.explanation_text()
                })
                .wrap(Wrap { trim: true }),
                detail,
            );
        }
        let items = view.rows.iter().map(|row| {
            let detail = match self.surface {
                Surface::Dependencies => format!("{:<15} {}", row.dependency_count, row.phase),
                Surface::Execution => row
                    .attempt
                    .map_or("—".into(), |number| format!("#{number}")),
                _ => row.phase.into(),
            };
            ListItem::new(format!("{detail:<13} {}", display_id(&row.reference.task)))
        });
        let mut state =
            ListState::default().with_selected(view.rows.get(view.selected).map(|_| view.selected));
        frame.render_stateful_widget(
            List::new(items)
                .highlight_style(ACTIVE)
                .highlight_symbol("› "),
            rows,
            &mut state,
        );
    }
}

struct Search {
    tasks: bool,
    query: String,
    selected: usize,
    notice: &'static str,
}

impl Search {
    fn matches(&self, streams: &[Stream]) -> Vec<(usize, Option<usize>)> {
        let query = self.query.to_lowercase();
        let terms: Vec<_> = query.split_whitespace().collect();
        let mut results = Vec::new();
        for (index, stream) in streams.iter().enumerate() {
            let name = stream.name.to_lowercase();
            if !self.tasks {
                if terms.iter().all(|term| name.contains(term)) {
                    results.push((index, None));
                }
                continue;
            }
            for (task, row) in stream.component.view().rows.iter().enumerate() {
                let id = row.reference.task.to_lowercase();
                if terms.iter().all(|term| {
                    if let Some(phase) = term.strip_prefix("phase:") {
                        row.phase == phase
                    } else if let Some(scope) = term.strip_prefix("in:") {
                        name.contains(scope)
                    } else {
                        id.contains(term)
                    }
                }) {
                    results.push((index, Some(task)));
                }
            }
        }
        results
    }
}

fn feed_status(watch: &Watch, view: &View) -> &'static str {
    if view.manual_refresh_required {
        "manual refresh required"
    } else {
        watch.status(view.revision.parse().ok())
    }
}

fn key_event_direction(key: KeyEvent) -> bool {
    key.kind == KeyEventKind::Press
        && key.modifiers == KeyModifiers::NONE
        && matches!(key.code, KeyCode::Left | KeyCode::Right)
}

fn panel(title: &str, focused: bool) -> Block<'_> {
    Block::default()
        .borders(Borders::ALL)
        .title(format!(" {title} "))
        .border_style(if focused { ACTIVE } else { DIM })
}

fn lane(row: &Row) -> usize {
    match row.phase {
        "blocked" => 0,
        "ready" => 1,
        "running" | "uncertain" => 2,
        _ => 3,
    }
}

fn adjacent_lane(view: &View, right: bool) -> Option<usize> {
    let current = lane(view.rows.get(view.selected)?);
    let mut target = None;
    for (index, row) in view.rows.iter().enumerate() {
        let candidate = lane(row);
        let distance = if right {
            candidate.checked_sub(current)
        } else {
            current.checked_sub(candidate)
        };
        if let Some(distance) = distance.filter(|&distance| distance != 0)
            && target.is_none_or(|(best, _)| distance < best)
        {
            target = Some((distance, index));
        }
    }
    target.map(|(_, index)| index)
}

fn board(frame: &mut Frame<'_>, area: Rect, view: &View, focused: bool) {
    let block = panel("Board", focused);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let areas = Layout::horizontal([Constraint::Ratio(1, 4); 4]).areas::<4>(inner);
    for (index, title) in ["Blocked", "Ready", "Active", "Closed"]
        .into_iter()
        .enumerate()
    {
        let mut selected = None;
        let mut items = Vec::new();
        for (row_index, row) in view
            .rows
            .iter()
            .enumerate()
            .filter(|(_, row)| lane(row) == index)
        {
            if row_index == view.selected {
                selected = Some(items.len());
            }
            let detail = if index >= 2 {
                format!("{} ", row.phase)
            } else {
                String::new()
            };
            items.push(ListItem::new(format!(
                "{} {detail}{}",
                crate::task_badges(view, row),
                display_id(&row.reference.task)
            )));
        }
        let heading = format!("{title} {}", items.len());
        let mut state = ListState::default().with_selected(selected);
        frame.render_stateful_widget(
            List::new(items)
                .block(Block::default().borders(Borders::LEFT).title(heading))
                .highlight_style(ACTIVE)
                .highlight_symbol("› "),
            areas[index],
            &mut state,
        );
    }
}

fn inspector(frame: &mut Frame<'_>, area: Rect, view: &View, explanation: &str) {
    let block = panel("Inspector", false);
    let mut details = if let Some(row) = view.rows.get(view.selected) {
        format!(
            "{}\n\nPhase        {}\nPriority     {}\nIncarnation  {}\nAttempt      {}\nPrerequisites {}\nCapabilities  {}\n\n{}{}",
            display_id(&row.reference.task),
            row.phase,
            row.priority,
            row.reference.incarnation,
            row.attempt
                .map_or("—".into(), |attempt| attempt.to_string()),
            row.dependency_count,
            row.required_capability_count,
            if view.mutation_blocked {
                "Refresh to verify this view.\n"
            } else {
                ""
            },
            if view.has_next {
                "More tasks on next page."
            } else {
                ""
            },
        )
    } else {
        "Select a task to inspect its state.".into()
    };
    if !explanation.is_empty() {
        details.push('\n');
        details.push_str(explanation);
    }
    frame.render_widget(
        Paragraph::new(details)
            .block(block)
            .wrap(Wrap { trim: true }),
        area,
    );
}

#[cfg(test)]
mod workbench_tests {
    use super::*;
    use chronos_host::scheduling::{SchedulingRequestV1, SchedulingResponseV1};
    use chronos_sdk_embedded::service::scheduling::SchedulingAuthorizerV1;
    use chronos_sdk_embedded::{
        engine::{Engine, StateRevision, TaskSpecV2},
        host::native::RequestV1,
        native_client::EmbeddedClient,
        service::{Authorizer, Limits, Service},
    };

    struct SyntheticOwner;

    impl Authorizer for SyntheticOwner {
        fn allows(&mut self, _: &RequestV1) -> bool {
            true
        }
    }

    impl SchedulingAuthorizerV1 for SyntheticOwner {
        fn allows_scheduling(&mut self, _: &SchedulingRequestV1) -> bool {
            true
        }

        fn allows_scheduling_result(
            &mut self,
            _: &SchedulingRequestV1,
            _: &SchedulingResponseV1,
        ) -> bool {
            true
        }
    }

    fn hub(writes: bool, scheduling: bool) -> Hub {
        hub_with_engine(Engine::new(), writes, scheduling)
    }

    fn hub_with_engine(engine: Engine, writes: bool, scheduling: bool) -> Hub {
        let service = Service::new(
            engine,
            SyntheticOwner,
            Limits {
                max_graph_tasks: 8,
                max_page_entries: 64,
            },
        );
        let client = EmbeddedClient::new(service, chronos_local::wire_limits());
        let component = if scheduling {
            Component::new_with_scheduling(client, writes)
        } else {
            Component::new(client, writes)
        };
        Hub {
            streams: vec![Stream {
                name: "Embedded".into(),
                directory: "embedded".into(),
                component,
                watch: None,
                moira: None,
                refreshing: false,
                observation: 0,
            }],
            active: 0,
            navigating: false,
            surface: Surface::Board,
            writes,
            scheduling,
            search: None,
            explanation: None,
            presence: None,
            presence_open: false,
            presence_scroll: 0,
            attempt_lookup: None,
            agenda: None,
            agenda_visible: false,
            graph: None,
            graph_visible: false,
            plan: None,
            handoff: Handoff::default(),
        }
    }

    #[test]
    fn operator_agenda_projects_blocked_work_without_ready_noise() {
        let mut engine = Engine::new();
        let root = TaskSpecV2::new("root");
        let mut blocked = TaskSpecV2::new("blocked");
        blocked.priority = 10;
        blocked.dependencies.insert(root.id.clone());
        engine
            .submit_coordination_graph_at_revision(StateRevision::ZERO, vec![root, blocked])
            .unwrap();
        let mut hub = hub_with_engine(engine, true, true);
        poll_until(&mut hub, |hub| hub.has_current_tasks());

        let agenda = hub.operator_agenda_observations();
        assert_eq!(agenda.len(), 1);
        assert_eq!(agenda[0].tasks.len(), 1);
        assert_eq!(agenda[0].tasks[0].task, "blocked");
        assert_eq!(agenda[0].tasks[0].phase, "blocked");
        assert_eq!(agenda[0].tasks[0].priority, 10);
        assert_eq!(agenda[0].tasks[0].dependency_count, 1);
    }

    fn poll_until(hub: &mut Hub, condition: impl Fn(&Hub) -> bool) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while !condition(hub) {
            hub.poll();
            assert!(
                std::time::Instant::now() < deadline,
                "embedded workbench intent did not settle"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    #[test]
    fn workbench_intents_enter_the_existing_bounded_and_confirmed_flows() {
        let mut hub = hub(true, true);
        poll_until(&mut hub, |hub| hub.has_current_tasks());
        let agenda = hub.operator_agenda_observations();
        assert_eq!(agenda.len(), 1);
        assert_eq!(agenda[0].workstream, "Embedded");
        assert!(agenda[0].current);
        assert!(agenda[0].tasks.is_empty());
        hub.presence_open = true;
        hub.presence_scroll = 4;
        hub.graph_visible = true;
        hub.agenda_visible = true;
        assert_eq!(
            hub.invoke_workbench(WorkbenchIntent::NewTask),
            Ok("Opened the bounded Chronos new-task form")
        );
        assert!(!hub.presence_open);
        assert_eq!(hub.presence_scroll, 0);
        assert!(!hub.graph_visible);
        assert!(!hub.agenda_visible);
        assert!(hub.streams[0].component.editing());
        for character in "palette-task".chars() {
            hub.update(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE));
        }
        hub.update(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(hub.streams[0].component.composing_task_batch());
        hub.update(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        poll_until(&mut hub, |hub| {
            !hub.streams[0].component.view().rows.is_empty()
        });
        assert_eq!(
            hub.selected_task(),
            Some(SelectedTaskProjection {
                workstream: "Embedded".into(),
                revision: "1".into(),
                task: "palette-task".into(),
                incarnation: "0".into(),
                attempt: None,
            })
        );
        assert_eq!(
            hub.invoke_workbench(WorkbenchIntent::CancelSelectedTask),
            Ok("Opened Chronos's exact marked/current cancellation confirmation")
        );
        poll_until(&mut hub, |hub| {
            hub.streams[0].component.view().pending_cancel.is_some()
        });
        assert_eq!(
            hub.invoke_workbench(WorkbenchIntent::OpenSchedule),
            Ok("Opened Chronos scheduling")
        );
        assert_eq!(hub.surface, Surface::Schedule);
    }

    #[test]
    fn workbench_intents_refuse_absent_launch_authority() {
        let mut hub = hub(false, false);
        poll_until(&mut hub, |hub| hub.has_current_tasks());
        assert_eq!(
            hub.invoke_workbench(WorkbenchIntent::NewTask),
            Err("Chronos writes are disabled for this session")
        );
        assert_eq!(
            hub.invoke_workbench(WorkbenchIntent::OpenSchedule),
            Err("Chronos scheduling was not selected for this session")
        );
    }
}
