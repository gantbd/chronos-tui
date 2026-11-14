#![forbid(unsafe_code)]
#[cfg(unix)]
use chronos_tui::profile;
#[cfg(unix)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use chronos_model::{Action, Model};
    use chronos_tui::{Component, Hub, Workstream, translate_key};
    use std::path::PathBuf;
    let mut cancel_receipts = false;
    let mut moira = Vec::new();
    let mut daimon = Vec::new();
    let mut args = std::env::args_os().skip(1);
    let Some(directory) = args.next() else {
        return Err(
            "usage: chronos-tui PRIVATE_SERVICE_DIRECTORY [--allow-writes] [--cancel-receipts] [--scheduling] [--watch] [--planning PATH] [--moira WORKSTREAM SOCKET SCOPE] [--daimon WORKSTREAM SOCKET] [--snapshot|--classic] [--name NAME] [--workstream NAME DIRECTORY]…".into(),
        );
    };
    if directory == "--help" {
        println!(
            "chronos-tui PRIVATE_SERVICE_DIRECTORY [--allow-writes] [--cancel-receipts] [--scheduling] [--watch] [--planning PATH] [--moira WORKSTREAM SOCKET SCOPE] [--daimon WORKSTREAM SOCKET] [--snapshot|--classic] [--name NAME] [--workstream NAME DIRECTORY]…\nUse --profile PATH [--allow-writes] [--cancel-receipts] [--scheduling] [--watch] [--planning PATH] [--moira WORKSTREAM SOCKET SCOPE] [--daimon WORKSTREAM SOCKET] to reopen a workspace. Append --save-profile PATH to a directory-based invocation to save and exit.\nRead-only by default. --scheduling opts into guarded pause, eligibility and capacity controls on an atomic local service. UI waits run on a bounded worker. --snapshot prints one page without entering terminal mode."
        );
        return Ok(());
    }
    if directory == "--profile" {
        let path = args.next().ok_or("--profile requires a path")?;
        let (mut writes, mut scheduling, mut watch) = (false, false, false);
        let mut planning = None;
        while let Some(flag) = args.next() {
            if flag == "--allow-writes" && !writes {
                writes = true;
            } else if flag == "--scheduling" && !scheduling {
                scheduling = true;
            } else if flag == "--watch" && !watch {
                watch = true;
            } else if flag == "--cancel-receipts" && !cancel_receipts {
                cancel_receipts = true;
            } else if flag == "--daimon" {
                daimon.push(peer_socket(
                    &mut args,
                    "--daimon requires WORKSTREAM SOCKET",
                )?);
            } else if flag == "--moira" {
                moira.push(moira_binding(&mut args)?);
            } else if flag == "--planning" && planning.is_none() {
                planning = Some(PathBuf::from(
                    args.next().ok_or("--planning requires a path")?,
                ));
            } else {
                return Err("unknown or duplicate workspace profile option".into());
            }
        }
        let mut hub = Hub::with_cancel_receipts(
            profile::load(std::path::Path::new(&path))?,
            writes,
            scheduling,
            watch,
            cancel_receipts,
        )?;
        if let Some(path) = planning {
            hub.load_planning(&path)?;
        }
        for (name, socket, scope) in moira {
            hub.bind_moira(&name, socket, scope)?;
        }
        for (name, socket) in daimon {
            hub.bind_daimon(&name, socket)?;
        }
        return run_terminal(&mut hub, Hub::poll, Hub::draw, Hub::update);
    }
    let mut writes = false;
    let mut scheduling = false;
    let mut watch = false;
    let mut snapshot = false;
    let directory = PathBuf::from(directory);
    let mut primary = Workstream {
        name: directory
            .file_name()
            .unwrap_or(directory.as_os_str())
            .to_string_lossy()
            .into_owned(),
        directory,
    };
    let mut additional = Vec::new();
    let mut classic = false;
    let mut save_profile = None;
    let mut planning = None;
    while let Some(flag) = args.next() {
        if flag == "--allow-writes" {
            writes = true;
        } else if flag == "--snapshot" {
            snapshot = true;
        } else if flag == "--scheduling" {
            scheduling = true;
        } else if flag == "--watch" {
            watch = true;
        } else if flag == "--classic" {
            classic = true;
        } else if flag == "--cancel-receipts" && !cancel_receipts {
            cancel_receipts = true;
        } else if flag == "--daimon" {
            daimon.push(peer_socket(
                &mut args,
                "--daimon requires WORKSTREAM SOCKET",
            )?);
        } else if flag == "--moira" {
            moira.push(moira_binding(&mut args)?);
        } else if flag == "--planning" && planning.is_none() {
            planning = Some(PathBuf::from(
                args.next().ok_or("--planning requires a path")?,
            ));
        } else if flag == "--save-profile" && save_profile.is_none() {
            save_profile = Some(PathBuf::from(
                args.next().ok_or("--save-profile requires a path")?,
            ));
        } else if flag == "--name" {
            primary.name = args
                .next()
                .and_then(|value| value.into_string().ok())
                .ok_or("--name requires a UTF-8 workstream name")?;
        } else if flag == "--workstream" {
            let name = args
                .next()
                .and_then(|value| value.into_string().ok())
                .ok_or("--workstream requires a UTF-8 name and service directory")?;
            let directory = args
                .next()
                .filter(|value| !value.is_empty() && !value.as_encoded_bytes().starts_with(b"--"))
                .ok_or("--workstream requires a service directory")?;
            additional.push(Workstream {
                name,
                directory: directory.into(),
            });
        } else {
            return Err("unknown option".into());
        }
    }
    if (!moira.is_empty() || !daimon.is_empty()) && (snapshot || classic || save_profile.is_some())
    {
        return Err("peer lookups require an interactive hub session".into());
    }
    if planning.is_some() && (snapshot || classic || save_profile.is_some()) {
        return Err("--planning requires an interactive hub session".into());
    }
    if cancel_receipts && (snapshot || classic || save_profile.is_some()) {
        return Err("--cancel-receipts requires an interactive hub".into());
    }
    if watch && (snapshot || classic) {
        return Err("--watch requires hub mode".into());
    }
    if let Some(path) = save_profile {
        if classic || snapshot {
            return Err("--save-profile requires hub configuration".into());
        }
        let streams = std::iter::once(primary)
            .chain(additional)
            .collect::<Vec<_>>();
        profile::save(&path, &streams)?;
        println!("Saved workspace profile {}", path.display());
        return Ok(());
    }
    if !classic && !snapshot {
        let mut hub = Hub::with_cancel_receipts(
            std::iter::once(primary).chain(additional).collect(),
            writes,
            scheduling,
            watch,
            cancel_receipts,
        )?;
        if let Some(path) = planning {
            hub.load_planning(&path)?;
        }
        for (name, socket, scope) in moira {
            hub.bind_moira(&name, socket, scope)?;
        }
        for (name, socket) in daimon {
            hub.bind_daimon(&name, socket)?;
        }
        return run_terminal(&mut hub, Hub::poll, Hub::draw, Hub::update);
    }
    if !additional.is_empty() {
        return Err("multiple workstreams require hub mode".into());
    }
    let client = chronos_local::LocalClient::new(primary.directory);
    if snapshot {
        let mut model = Model::new(client, false);
        if scheduling {
            model = model.with_scheduling();
        }
        model.apply(Action::Refresh);
        if scheduling {
            model.apply(Action::RefreshScheduling(std::time::SystemTime::now()));
            println!(
                "Scheduling revision {}: {}",
                model.view.scheduling.revision, model.view.scheduling.status
            );
            let s = &model.view.scheduling;
            println!(
                "enabled={} paused={} capacity={:?} held={:?} available={:?}",
                s.enabled, s.paused, s.capacity, s.held, s.available
            );
            if let Some(selected) = &s.selected {
                println!(
                    "Selected pause={} not-before={}",
                    selected.paused,
                    chronos_tui::format_not_before(selected.not_before)
                );
            }
        }
        for row in &model.view.rows {
            println!(
                "{} {}",
                row.phase,
                chronos_model::display_id(&row.reference.task)
            );
        }
        println!("{}", model.view.status);
        if model.view.mutation_blocked || scheduling && model.view.scheduling.mutation_blocked {
            return Err("snapshot not current".into());
        }
        return Ok(());
    }
    let mut component = if scheduling {
        Component::new_with_scheduling(client, writes)
    } else {
        Component::new(client, writes)
    };
    run_terminal(
        &mut component,
        Component::poll,
        |component, frame| component.draw_in(frame, frame.area()),
        |component, key| component.update(translate_key(key), true),
    )
}

#[cfg(unix)]
fn run_terminal<T>(
    state: &mut T,
    poll: impl Fn(&mut T) -> bool,
    draw: impl Fn(&T, &mut ratatui::Frame<'_>),
    update: impl Fn(&mut T, crossterm::event::KeyEvent) -> chronos_tui::Lifecycle,
) -> Result<(), Box<dyn std::error::Error>> {
    use chronos_tui::Lifecycle;
    use crossterm::event::{self, Event};
    ratatui::run(|terminal| -> Result<(), Box<dyn std::error::Error>> {
        let mut redraw = true;
        loop {
            redraw |= poll(state);
            if redraw {
                terminal.draw(|frame| draw(state, frame))?;
                redraw = false;
            }
            if !event::poll(std::time::Duration::from_millis(25))? {
                continue;
            }
            let event = event::read()?;
            if matches!(event, Event::Resize(..)) {
                redraw = true;
                continue;
            }
            let Event::Key(key) = event else {
                continue;
            };
            if update(state, key) == Lifecycle::CloseRequested {
                break;
            }
            redraw = true;
        }
        Ok(())
    })
}

#[cfg(not(unix))]
fn main() {
    eprintln!("this TUI composition requires the Unix local service profile");
    std::process::exit(1);
}

#[cfg(unix)]
fn moira_binding(
    args: &mut impl Iterator<Item = std::ffi::OsString>,
) -> Result<(String, std::path::PathBuf, String), &'static str> {
    let error = "--moira requires WORKSTREAM SOCKET SCOPE";
    let (workstream, socket) = peer_socket(args, error)?;
    let scope = args
        .next()
        .and_then(|value| value.into_string().ok())
        .ok_or(error)?;
    Ok((workstream, socket, scope))
}

#[cfg(unix)]
fn peer_socket(
    args: &mut impl Iterator<Item = std::ffi::OsString>,
    error: &'static str,
) -> Result<(String, std::path::PathBuf), &'static str> {
    let workstream = args
        .next()
        .and_then(|value| value.into_string().ok())
        .ok_or(error)?;
    let socket = args.next().ok_or(error)?.into();
    Ok((workstream, socket))
}
