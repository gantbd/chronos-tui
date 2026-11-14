//! Bounded feed pulls off the terminal thread. Deltas invalidate projections.
use chronos_client::WireClientV3;
use chronos_local::LocalClient;
use chronos_protocol::{Revision, coordination_v3 as wire};
use std::{path::PathBuf, sync::mpsc, time::Duration};

#[derive(PartialEq, Eq)]
pub(crate) enum Observation {
    Waiting,
    Unavailable,
    Refused,
    Head {
        generation: String,
        revision: u64,
        gap: bool,
    },
}

pub(crate) struct Watch {
    updates: mpsc::Receiver<Observation>,
    _stop: mpsc::Sender<()>,
    pub(crate) observation: Observation,
    generation: Option<String>,
    pub(crate) resync: bool,
}

impl Watch {
    pub(crate) fn start(directory: PathBuf) -> Self {
        let (send, updates) = mpsc::sync_channel(1);
        let (stop, stopped) = mpsc::channel();
        std::thread::spawn(move || {
            let mut client = LocalClient::new(directory);
            let mut after = None;
            loop {
                let continuing = after.is_some();
                let request = wire::Request {
                    version: 3,
                    command: wire::Command::Observe {
                        after: after.take(),
                        limit: 16,
                    },
                };
                let observation = match client.request_v3(request) {
                    Ok(response) => {
                        let gap = continuing
                            && matches!(&response.outcome, wire::Outcome::ResyncRequired { .. });
                        match response.outcome {
                            wire::Outcome::ResyncRequired { head }
                            | wire::Outcome::Changes { cursor: head, .. } => {
                                let Revision::Finite(revision) = &head.revision else {
                                    break;
                                };
                                let Ok(revision) = revision.parse() else {
                                    break;
                                };
                                let generation = head.generation.clone();
                                after = Some(head);
                                Observation::Head {
                                    generation,
                                    revision,
                                    gap,
                                }
                            }
                            _ => Observation::Refused,
                        }
                    }
                    Err(_) => Observation::Unavailable,
                };
                if send.send(observation).is_err()
                    || stopped.recv_timeout(Duration::from_secs(1))
                        != Err(mpsc::RecvTimeoutError::Timeout)
                {
                    break;
                }
            }
        });
        Self {
            updates,
            _stop: stop,
            observation: Observation::Waiting,
            generation: None,
            resync: true,
        }
    }

    pub(crate) fn poll(&mut self) -> (bool, bool) {
        let incoming = match self.updates.try_recv() {
            Ok(incoming) => incoming,
            Err(mpsc::TryRecvError::Empty) => return (false, false),
            Err(mpsc::TryRecvError::Disconnected) => Observation::Unavailable,
        };
        if let Observation::Head {
            generation, gap, ..
        } = &incoming
            && (*gap || self.generation.as_ref() != Some(generation))
        {
            self.resync = true;
            self.generation = Some(generation.clone());
        }
        let changed = self.observation != incoming;
        self.observation = incoming;
        (true, changed)
    }

    pub(crate) fn revision(&self) -> Option<u64> {
        match self.observation {
            Observation::Head { revision, .. } => Some(revision),
            _ => None,
        }
    }

    pub(crate) fn status(&self, observed: Option<u64>) -> &'static str {
        match self.observation {
            Observation::Waiting => "feed connecting",
            Observation::Unavailable => "feed offline or unavailable",
            Observation::Refused => "feed refused",
            Observation::Head { .. } if self.resync => "feed resync pending",
            Observation::Head { revision, .. } if observed.is_none_or(|view| view < revision) => {
                "feed refresh pending"
            }
            Observation::Head { .. } => "feed observed",
        }
    }
}
