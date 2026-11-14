use super::*;
use chronos_sdk_embedded::{
    engine::{Engine, Task},
    host::{native::RequestV1, scheduling::*},
    native_client::EmbeddedClient,
    service::{Authorizer, Service, scheduling::SchedulingAuthorizerV1},
};
struct Owner;
impl Authorizer for Owner {
    fn allows(&mut self, _: &RequestV1) -> bool {
        true
    }
}
impl SchedulingAuthorizerV1 for Owner {
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
fn pane() -> Component {
    let mut engine = Engine::new();
    engine.submit(Task::new("fixture", "")).unwrap();
    let service = Service::new(
        engine,
        Owner,
        chronos_sdk_embedded::service::Limits {
            max_graph_tasks: 4,
            max_page_entries: 64,
        },
    );
    Component::new_with_scheduling(
        EmbeddedClient::new(service, chronos_local::wire_limits()),
        true,
    )
}
fn deliver(pane: &mut Component) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while !pane.poll() {
        assert!(
            std::time::Instant::now() < deadline,
            "projection not delivered"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}
fn key(pane: &mut Component, ch: char) {
    pane.update(Input::Text(ch), true);
    deliver(pane);
}
fn entry(pane: &mut Component, key: char, value: &str) {
    pane.update(Input::Text(key), true);
    for ch in value.chars() {
        pane.update(Input::Text(ch), true);
    }
    pane.update(Input::Enter, true);
    deliver(pane);
    assert!(pane.state.view.scheduling.pending.is_some());
    key_confirm(pane);
}
fn key_confirm(pane: &mut Component) {
    key(pane, 'y');
    assert!(pane.state.view.scheduling.status.contains("applied"));
}
#[test]
fn terminal_gestures_drive_guarded_controls_and_preserve_existing_time_on_pause() {
    let mut pane = pane();
    deliver(&mut pane);
    key(&mut pane, 's');
    key(&mut pane, 'e');
    assert!(pane.state.view.scheduling.pending.is_some());
    assert_eq!(pane.state.view.scheduling.signals.requests, 1);
    key_confirm(&mut pane);
    key(&mut pane, 's');
    entry(&mut pane, 'b', "3");
    key(&mut pane, 's');
    assert_eq!(pane.state.view.scheduling.capacity, Some(3));
    entry(&mut pane, 'd', "100000");
    key(&mut pane, 's');
    let original = pane
        .state
        .view
        .scheduling
        .selected
        .as_ref()
        .unwrap()
        .not_before;
    key(&mut pane, 't');
    key_confirm(&mut pane);
    key(&mut pane, 's');
    let selected = pane.state.view.scheduling.selected.as_ref().unwrap();
    assert!(selected.paused);
    assert_eq!(selected.not_before, original);
    entry(&mut pane, 'd', "clear");
    key(&mut pane, 's');
    assert!(
        pane.state
            .view
            .scheduling
            .selected
            .as_ref()
            .unwrap()
            .not_before
            .is_none()
    );
    key(&mut pane, 'p');
    key_confirm(&mut pane);
    key(&mut pane, 's');
    assert!(pane.state.view.scheduling.paused);
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 24)).unwrap();
    terminal
        .draw(|frame| pane.draw_in(frame, frame.area()))
        .unwrap();
    let rendered: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(rendered.contains("Scheduling"));
    assert!(rendered.contains("paused=true"));
    assert!(pane.state.view.scheduling.signals.backend_errors == 0);
}
#[test]
fn readonly_busy_and_invalid_forms_do_not_dispatch_or_lose_input() {
    let mut state = State::default();
    state.view.scheduling.supported = true;
    state.view.scheduling.mutation_blocked = false;
    for ch in ['e', 'p', 'b', 'd', 't'] {
        assert!(state.update(Input::Text(ch), true, false).1.is_none());
    }
    assert!(state.entry.is_none());
    state.writes = true;
    assert!(state.update(Input::Text('p'), true, false).1.is_none());
    assert!(state.update(Input::Text('b'), true, false).1.is_none());
    assert!(state.entry.is_none());
    assert!(matches!(
        state.update(Input::Text('e'), true, false).1,
        Some(Action::PlanScheduling(SchedulingChange::Enable))
    ));
    state.view.scheduling.enabled = true;
    assert!(state.update(Input::Text('e'), true, false).1.is_none());
    state.update(Input::Text('b'), true, false);
    state.update(Input::Text('0'), true, false);
    assert!(state.update(Input::Enter, true, false).1.is_none());
    assert_eq!(state.entry.as_deref(), Some("0"));
    state.update(Input::Backspace, true, false);
    state.update(Input::Text('2'), true, false);
    assert!(state.update(Input::Enter, true, true).1.is_none());
    assert_eq!(state.entry.as_deref(), Some("2"));
    assert!(matches!(
        state.update(Input::Enter, true, false).1,
        Some(Action::PlanScheduling(
            SchedulingChange::SetConcurrencyLimit(2)
        ))
    ));
    for invalid in ["", "-1", "1.5", "2026-11-10", "18446744073709551616"] {
        assert!(parse_not_before(invalid).is_err());
    }
    assert_eq!(parse_not_before("clear"), Ok(None));
}
