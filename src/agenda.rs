//! Read-only eligibility windows from the public V5 service.
use chronos_client::SchedulingWireClientV5;
use chronos_local::LocalClient;
use chronos_model::display_id;
use chronos_protocol::{Reference, Revision, scheduling_v5 as v5};
use std::{cmp::Ordering, path::PathBuf, sync::mpsc};

pub(crate) const WINDOW: usize = 8;

#[derive(PartialEq, Eq)]
pub(crate) struct Scope {
    pub stream: usize,
    pub observation: u64,
    pub revision: String,
    pub first: usize,
    pub last: usize,
    pub first_incarnation: String,
    pub first_task: String,
}

pub(crate) enum Agenda {
    Reading(mpsc::Receiver<Result<Vec<String>, &'static str>>),
    Ready(Result<Vec<String>, &'static str>),
}

impl Agenda {
    pub(crate) fn start(directory: PathBuf, revision: String, tasks: Vec<Reference>) -> Self {
        let (send, receive) = mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let mut client = LocalClient::new(directory);
            let result = (|| {
                let mut rows = Vec::new();
                for task in tasks {
                    let response = client
                        .request_scheduling_v5(v5::Request {
                            version: 5,
                            command: v5::Command::Eligibility { task: task.clone() },
                        })
                        .map_err(|_| "Eligibility read unavailable")?;
                    let v5::Outcome::Eligibility {
                        revision: observed,
                        eligibility,
                        ..
                    } = response.outcome
                    else {
                        return Err("Eligibility read refused");
                    };
                    let exact = match observed {
                        Revision::Finite(value) => value == revision,
                        Revision::Exhausted => revision == "exhausted",
                    };
                    if !exact {
                        return Err("Task revision changed. Refresh and load the agenda again.");
                    }
                    let Some(eligibility) = eligibility else {
                        return Err(
                            "Eligibility unavailable for this window. Check scheduling and refresh.",
                        );
                    };
                    rows.push((task.task, eligibility));
                }
                rows.sort_by(|(left_id, left), (right_id, right)| {
                    match (&left.not_before, &right.not_before) {
                        (Some(left), Some(right)) => compare_time(left, right),
                        (Some(_), None) => Ordering::Less,
                        (None, Some(_)) => Ordering::Greater,
                        (None, None) => Ordering::Equal,
                    }
                    .then_with(|| left_id.cmp(right_id))
                });
                Ok(rows
                    .into_iter()
                    .map(|(task, eligibility)| {
                        let time = eligibility.not_before.map_or_else(
                            || "unscheduled".into(),
                            |time| {
                                format!(
                                    "{}{}.{:09}",
                                    if time.before_epoch { "-" } else { "" },
                                    time.seconds,
                                    time.nanos
                                )
                            },
                        );
                        format!(
                            "{time}  {}{}",
                            if eligibility.paused { "[paused] " } else { "" },
                            display_id(&task)
                        )
                    })
                    .collect())
            })();
            let _ = send.send(result);
        });
        Self::Reading(receive)
    }

    pub(crate) fn poll(&mut self) -> bool {
        let Self::Reading(receiver) = self else {
            return false;
        };
        let result = match receiver.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return false,
            Err(mpsc::TryRecvError::Disconnected) => Err("Eligibility read unavailable"),
        };
        *self = Self::Ready(result);
        true
    }
}

fn compare_time(left: &v5::Timestamp, right: &v5::Timestamp) -> Ordering {
    let magnitude = || {
        left.seconds
            .len()
            .cmp(&right.seconds.len())
            .then_with(|| left.seconds.cmp(&right.seconds))
            .then_with(|| left.nanos.cmp(&right.nanos))
    };
    match (left.before_epoch, right.before_epoch) {
        (true, true) => magnitude().reverse(),
        (false, false) => magnitude(),
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
    }
}
