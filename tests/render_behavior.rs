use chronos_model::{Row, View};
use chronos_protocol::Reference;
use ratatui::{Terminal, backend::TestBackend, layout::Rect, widgets::Paragraph};

#[test]
fn embedded_projection_never_changes_peer_cells_outside_its_owned_rectangle() {
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    let owned = Rect::new(7, 4, 61, 20);
    terminal
        .draw(|frame| {
            for y in 0..30 {
                frame.render_widget(Paragraph::new("P".repeat(100)), Rect::new(0, y, 100, 1));
            }
            chronos_tui::draw_in(frame, owned, &View::default(), None, "");
        })
        .unwrap();
    let buffer = terminal.backend().buffer();
    for y in 0..30 {
        for x in 0..100 {
            if !owned.contains((x, y).into()) {
                assert_eq!(buffer[(x, y)].symbol(), "P", "peer cell changed at {x},{y}");
            }
        }
    }
    assert_eq!(buffer[(7, 4)].symbol(), "C");
}

#[test]
fn empty_offscreen_and_clipped_render_regions_are_safe() {
    for area in [
        Rect::new(2, 2, 0, 0),
        Rect::new(50, 50, 10, 10),
        Rect::new(19, 9, 20, 20),
        Rect::new(1, 1, 1, 1),
    ] {
        let mut terminal = Terminal::new(TestBackend::new(20, 10)).unwrap();
        terminal
            .draw(|frame| chronos_tui::draw_in(frame, area, &View::default(), None, ""))
            .unwrap();
    }
}

#[test]
fn bounded_terminal_view_renders_selection_state_and_escaped_ids() {
    let mut terminal = Terminal::new(TestBackend::new(100, 20)).unwrap();
    let view = View {
        rows: vec![Row {
            reference: Reference {
                task: "task\u{1b}[2J".into(),
                incarnation: "0".into(),
            },
            phase: "ready",
            priority: 0,
            attempt: None,
            dependency_count: 0,
            required_capability_count: 0,
        }],
        revision: "1".into(),
        mutation_blocked: false,
        ..View::default()
    };
    terminal
        .draw(|frame| chronos_tui::render(frame, &view, None, ""))
        .unwrap();
    let output: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(output.contains("Chronos"));
    assert!(output.contains("ready"));
    assert!(output.contains("task"));
    assert!(!output.contains('\u{1b}'));
}

#[test]
fn confirmation_and_entry_render_instead_of_ambiguous_mutation_gestures() {
    let mut terminal = Terminal::new(TestBackend::new(100, 20)).unwrap();
    let view = View {
        pending_cancel: Some("task".into()),
        ..View::default()
    };
    terminal
        .draw(|frame| chronos_tui::render(frame, &view, None, ""))
        .unwrap();
    let output: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(output.contains("y confirms"));
    terminal
        .draw(|frame| chronos_tui::render(frame, &view, Some("new-task"), ""))
        .unwrap();
    let output: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(output.contains("Enter submits"));
}
