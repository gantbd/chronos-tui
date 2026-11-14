//! Explicit, bounded reads from the configured Daimon host.
use daimon_protocol::{OutcomeV1, PresenceEntryV1, RefusalV1, ReplyV1};
use std::{path::PathBuf, sync::mpsc, time::Instant};

const LIMIT: u8 = 16;

pub(crate) enum Presence {
    Reading(mpsc::Receiver<(Instant, Result<Page, &'static str>)>),
    Ready(Instant, Result<Page, &'static str>),
}

pub(crate) struct Page {
    entries: Vec<PresenceEntryV1>,
    truncated: bool,
}

impl Presence {
    pub(crate) fn start(socket: PathBuf) -> Self {
        let (send, receive) = mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let started = Instant::now();
            let _ = send.send((started, read(socket)));
        });
        Self::Reading(receive)
    }

    pub(crate) fn poll(&mut self) -> bool {
        let Self::Reading(receiver) = self else {
            return false;
        };
        let (started, result) = match receiver.try_recv() {
            Ok(value) => value,
            Err(mpsc::TryRecvError::Empty) => return false,
            Err(mpsc::TryRecvError::Disconnected) => {
                (Instant::now(), Err("Daimon roster unavailable"))
            }
        };
        *self = Self::Ready(started, result);
        true
    }

    pub(crate) fn lines(&self) -> String {
        use std::fmt::Write;
        match self {
            Self::Reading(_) => "Reading Daimon roster…".into(),
            Self::Ready(_, Err(message)) => (*message).into(),
            Self::Ready(started, Ok(page)) => {
                let mut text = format!("Snapshot requested {}s ago", started.elapsed().as_secs());
                if page.entries.is_empty() {
                    text.push_str("\nNo visible supervised agents");
                }
                for entry in &page.entries {
                    text.push('\n');
                    for byte in entry.agent {
                        let _ = write!(text, "{byte:02x}");
                    }
                    let status = if entry.stale { "stale" } else { "observed" };
                    let _ = write!(text, "  {status} at read  {}ms old", entry.age_millis);
                }
                if page.truncated {
                    text.push_str("\nMore visible agents");
                }
                text
            }
        }
    }
}

#[cfg(unix)]
fn read(socket: PathBuf) -> Result<Page, &'static str> {
    let unavailable = "Daimon roster unavailable";
    let (session, port) = daimon_client::unix::connect(
        socket,
        daimon_wire::LimitsV1::default(),
        std::time::Duration::from_millis(750),
    )
    .map_err(|_| unavailable)?;
    let reply = daimon_client::ReadOnlyClient::new(session, port)
        .read_presence(LIMIT)
        .map_err(|_| unavailable)?;
    page(reply)
}

#[cfg(not(unix))]
fn read(_: PathBuf) -> Result<Page, &'static str> {
    Err("Daimon roster unavailable")
}

fn page(reply: ReplyV1) -> Result<Page, &'static str> {
    match reply {
        ReplyV1::Accepted(OutcomeV1::Presence { entries, truncated })
            if entries.len() <= usize::from(LIMIT)
                && (!truncated || entries.len() == usize::from(LIMIT))
                && entries.windows(2).all(|pair| pair[0].agent < pair[1].agent) =>
        {
            Ok(Page { entries, truncated })
        }
        ReplyV1::Refused(
            RefusalV1::AuthorityDenied
            | RefusalV1::AuthorityExpired
            | RefusalV1::UnknownSession
            | RefusalV1::PrincipalMismatch,
        ) => Err("Daimon roster access denied"),
        _ => Err("Daimon roster unavailable"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_page_and_distinguishes_denied_empty_and_stale() {
        let empty = page(ReplyV1::Accepted(OutcomeV1::Presence {
            entries: vec![],
            truncated: false,
        }))
        .unwrap();
        assert!(
            Presence::Ready(Instant::now(), Ok(empty))
                .lines()
                .contains("No visible")
        );
        assert!(
            page(ReplyV1::Accepted(OutcomeV1::Presence {
                entries: vec![],
                truncated: true,
            }))
            .is_err()
        );
        assert_eq!(
            page(ReplyV1::Refused(RefusalV1::AuthorityDenied)).err(),
            Some("Daimon roster access denied")
        );
        let observed = page(ReplyV1::Accepted(OutcomeV1::Presence {
            entries: vec![PresenceEntryV1 {
                agent: [7; 16],
                age_millis: 2050,
                stale: true,
            }],
            truncated: false,
        }))
        .unwrap();
        assert!(
            Presence::Ready(Instant::now(), Ok(observed))
                .lines()
                .contains("stale at read  2050ms old")
        );
    }
}
