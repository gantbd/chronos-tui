//! Two independent read-only panes; the host owns layout, focus and exit.
#![forbid(unsafe_code)]

#[cfg(unix)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use chronos_tui::{Component, Lifecycle, translate_key};
    use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
    use ratatui::{
        layout::{Constraint, Layout},
        widgets::Paragraph,
    };

    let directory = std::env::args_os()
        .nth(1)
        .ok_or("usage: composite PRIVATE_SERVICE_DIRECTORY")?;
    let mut panes = [
        Some(Component::new(
            chronos_local::LocalClient::new(directory.clone()),
            false,
        )),
        Some(Component::new(
            chronos_local::LocalClient::new(directory),
            false,
        )),
    ];
    let mut focus = 0;
    ratatui::run(|terminal| -> Result<(), Box<dyn std::error::Error>> {
        let mut redraw = true;
        loop {
            for pane in panes.iter_mut().flatten() {
                redraw |= pane.poll();
            }
            if redraw {
                terminal.draw(|frame| {
                    let bands = Layout::vertical([Constraint::Length(2), Constraint::Min(1)])
                        .split(frame.area());
                    frame.render_widget(
                        Paragraph::new(format!(
                            "Focused pane {} · Tab switches focus · q closes only this pane",
                            focus + 1
                        )),
                        bands[0],
                    );
                    let regions =
                        Layout::horizontal([Constraint::Percentage(50); 2]).split(bands[1]);
                    for (pane, area) in panes.iter().zip(regions.iter()) {
                        if let Some(pane) = pane {
                            pane.draw_in(frame, *area);
                        }
                    }
                })?;
                redraw = false;
            }
            if !event::poll(std::time::Duration::from_millis(25))? {
                continue;
            }
            match event::read()? {
                Event::Resize(..) => redraw = true,
                Event::Key(key) => {
                    if key.code == KeyCode::Tab
                        && key.kind == KeyEventKind::Press
                        && key.modifiers == KeyModifiers::NONE
                    {
                        if panes[1 - focus].is_some() {
                            focus = 1 - focus;
                        }
                    } else if let Some(pane) = &mut panes[focus]
                        && pane.update(translate_key(key), true) == Lifecycle::CloseRequested
                    {
                        // Closing one pane cannot terminate its live sibling.
                        panes[focus] = None;
                        focus = 1 - focus;
                        if panes[focus].is_none() {
                            break;
                        }
                    }
                    redraw = true;
                }
                _ => {}
            }
        }
        Ok(())
    })
}

#[cfg(not(unix))]
fn main() {
    eprintln!("this example requires the Unix local-service adapter");
}
