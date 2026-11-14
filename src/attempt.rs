//! On-demand correlation via Moira's authenticated read-only socket profile.
use daimon_client::ExecutionStatusV1;
use moira_protocol::attempt_lookup::{AttemptLookup, AttemptLookupRequestV1, ExecutionEvidence};
use std::{path::PathBuf, sync::mpsc};

#[derive(Clone)]
pub(crate) struct Binding {
    pub socket: PathBuf,
    pub scope: String,
    pub daimon: Option<PathBuf>,
}

type Observation = (Option<AttemptLookup>, Option<ExecutionStatusV1>);

pub(crate) enum Lookup {
    Reading(mpsc::Receiver<Observation>),
    Ready(Observation),
}

impl Lookup {
    pub(crate) fn start(binding: Binding, task: String, incarnation: u64, attempt: u32) -> Self {
        let (send, receive) = mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let result = request(
                binding.socket,
                AttemptLookupRequestV1 {
                    service_scope: binding.scope,
                    task,
                    incarnation,
                    attempt,
                },
            );
            let status = match (binding.daimon, result) {
                (
                    Some(socket),
                    Some(AttemptLookup::Found {
                        execution: ExecutionEvidence::Recorded { execution_id, .. },
                        ..
                    }),
                ) => Some(inspect(socket, execution_id)),
                _ => None,
            };
            let _ = send.send((result, status));
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
            Err(mpsc::TryRecvError::Disconnected) => (None, None),
        };
        *self = Self::Ready(result);
        true
    }

    pub(crate) fn lines(&self) -> String {
        fn hex(bytes: [u8; 16]) -> String {
            use std::fmt::Write;
            let mut value = String::with_capacity(32);
            for byte in bytes {
                let _ = write!(value, "{byte:02x}");
            }
            value
        }

        match self {
            Self::Reading(_) => "Reading Moira receipt…".into(),
            Self::Ready((None | Some(AttemptLookup::Unavailable), _)) => {
                "Moira lookup unavailable".into()
            }
            Self::Ready((Some(AttemptLookup::Denied), _)) => "Moira lookup denied".into(),
            Self::Ready((Some(AttemptLookup::Unresolved), _)) => {
                "No exact Moira correlation resolved".into()
            }
            Self::Ready((
                Some(AttemptLookup::Found {
                    workflow,
                    execution,
                }),
                status,
            )) => {
                let workflow = hex(workflow.0);
                match execution {
                    ExecutionEvidence::ReceiptPending => {
                        format!("Moira workflow {workflow}\nDaimon receipt pending")
                    }
                    ExecutionEvidence::Recorded {
                        execution_id,
                        disposition,
                    } => format!(
                        "Moira workflow {workflow}\nDaimon execution {}\nMoira receipt {disposition:?}\n{}",
                        hex(*execution_id),
                        status_text(*status)
                    ),
                }
            }
        }
    }
}

#[cfg(unix)]
fn request(socket: PathBuf, request: AttemptLookupRequestV1) -> Option<AttemptLookup> {
    use moira_wire::{LimitsV1, decode_attempt_reply, decode_ready, encode_attempt_request};
    use std::{
        io::{Read, Write},
        os::unix::net::UnixStream,
        time::Duration,
    };

    fn frame(socket: &mut UnixStream, limit: usize) -> Option<Vec<u8>> {
        let mut header = [0; 4];
        socket.read_exact(&mut header).ok()?;
        let size = u32::from_be_bytes(header) as usize;
        if !(1..=limit).contains(&size) {
            return None;
        }
        let mut bytes = vec![0; size];
        socket.read_exact(&mut bytes).ok()?;
        Some(bytes)
    }

    let mut stream = UnixStream::connect(socket).ok()?;
    let timeout = Some(Duration::from_millis(750));
    stream.set_read_timeout(timeout).ok()?;
    stream.set_write_timeout(timeout).ok()?;
    let mut limits = LimitsV1::default();
    let ready = decode_ready(&frame(&mut stream, 32)?, limits).ok()?;
    limits.max_frame_bytes = limits.max_frame_bytes.min(ready.max_frame_bytes as usize);
    let bytes = encode_attempt_request(&request, limits).ok()?;
    stream
        .write_all(&u32::try_from(bytes.len()).ok()?.to_be_bytes())
        .ok()?;
    stream.write_all(&bytes).ok()?;
    decode_attempt_reply(&frame(&mut stream, limits.max_frame_bytes)?, limits).ok()
}

#[cfg(not(unix))]
fn request(_: PathBuf, _: AttemptLookupRequestV1) -> Option<AttemptLookup> {
    None
}

fn status_text(status: Option<ExecutionStatusV1>) -> &'static str {
    use daimon_protocol::ExecutionPhaseV1;
    match status {
        Some(ExecutionStatusV1::Observed(ExecutionPhaseV1::OutcomeUnknown)) => {
            "Daimon status: outcome unknown"
        }
        Some(ExecutionStatusV1::Observed(ExecutionPhaseV1::Completed)) => {
            "Daimon status: completed"
        }
        Some(ExecutionStatusV1::Missing) => "Daimon execution missing",
        Some(ExecutionStatusV1::Denied) => "Daimon inspection denied",
        Some(ExecutionStatusV1::Unavailable) => "Daimon inspection unavailable",
        None => "Daimon inspection is not configured",
    }
}

#[cfg(unix)]
fn inspect(socket: PathBuf, id: [u8; 16]) -> ExecutionStatusV1 {
    let Ok((session, port)) = daimon_client::unix::connect(
        socket,
        daimon_wire::LimitsV1::default(),
        std::time::Duration::from_millis(750),
    ) else {
        return ExecutionStatusV1::Unavailable;
    };
    daimon_client::ReadOnlyClient::new(session, port)
        .inspect_status(daimon_protocol::ExecutionIdV1::new(id))
}

#[cfg(not(unix))]
fn inspect(_: PathBuf, _: [u8; 16]) -> ExecutionStatusV1 {
    ExecutionStatusV1::Unavailable
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        os::unix::net::{UnixListener, UnixStream},
        time::Duration,
    };

    fn send(stream: &mut UnixStream, bytes: Vec<u8>) {
        stream
            .write_all(&(bytes.len() as u32).to_be_bytes())
            .unwrap();
        stream.write_all(&bytes).unwrap();
    }

    fn receive(stream: &mut UnixStream) -> Vec<u8> {
        let mut header = [0; 4];
        stream.read_exact(&mut header).unwrap();
        let mut body = vec![0; u32::from_be_bytes(header) as usize];
        stream.read_exact(&mut body).unwrap();
        body
    }

    #[test]
    fn exact_attempt_uses_distinct_peer_sessions_and_preserves_outcome_unknown() {
        use daimon_protocol::{
            CommandV1, ExecutionIdV1, ExecutionPhaseV1, OutcomeV1, ReplyV1, SessionHandleV1,
        };
        use moira_protocol::{DispositionV1, WorkflowIdV1};
        let prefix = format!("chronos-lookup-{}", std::process::id());
        let moira = std::env::temp_dir().join(format!("{prefix}-m"));
        let daimon = std::env::temp_dir().join(format!("{prefix}-d"));
        let m = UnixListener::bind(&moira).unwrap();
        let d = UnixListener::bind(&daimon).unwrap();
        let execution_id = [9; 16];
        let server = std::thread::spawn(move || {
            let (mut stream, _) = m.accept().unwrap();
            let limits = moira_wire::LimitsV1::default();
            send(
                &mut stream,
                moira_wire::encode_ready(
                    moira_wire::ReadyV1 {
                        protocol_version: 1,
                        max_frame_bytes: 1024,
                        max_requests: 1,
                    },
                    limits,
                )
                .unwrap(),
            );
            let exact = moira_wire::decode_attempt_request(&receive(&mut stream), limits).unwrap();
            assert_eq!(
                (
                    exact.service_scope.as_str(),
                    exact.task.as_str(),
                    exact.incarnation,
                    exact.attempt
                ),
                ("atlas", "review", 4, 2)
            );
            send(
                &mut stream,
                moira_wire::encode_attempt_reply(
                    &AttemptLookup::Found {
                        workflow: WorkflowIdV1([3; 16]),
                        execution: ExecutionEvidence::Recorded {
                            execution_id,
                            disposition: DispositionV1::Uncertain,
                        },
                    },
                    limits,
                )
                .unwrap(),
            );

            let (mut stream, _) = d.accept().unwrap();
            let limits = daimon_wire::LimitsV1::default();
            let session = SessionHandleV1(73);
            send(
                &mut stream,
                daimon_wire::encode_session_opened(session, limits).unwrap(),
            );
            let frame = daimon_wire::decode_request(&receive(&mut stream), limits).unwrap();
            assert_eq!(frame.session, session);
            let execution_id = ExecutionIdV1::new(execution_id);
            assert_eq!(frame.command, CommandV1::Inspect { execution_id });
            send(
                &mut stream,
                daimon_wire::encode_reply(
                    &ReplyV1::Accepted(OutcomeV1::Status {
                        execution_id,
                        phase: ExecutionPhaseV1::OutcomeUnknown,
                    }),
                    limits,
                )
                .unwrap(),
            );
        });
        let Lookup::Reading(receiver) = Lookup::start(
            Binding {
                socket: moira.clone(),
                scope: "atlas".into(),
                daimon: Some(daimon.clone()),
            },
            "review".into(),
            4,
            2,
        ) else {
            unreachable!()
        };
        let result = receiver.recv_timeout(Duration::from_secs(3));
        let _ = std::fs::remove_file(moira);
        let _ = std::fs::remove_file(daimon);
        let text = Lookup::Ready(result.unwrap()).lines();
        server.join().unwrap();
        assert!(text.contains("Moira receipt Uncertain"));
        assert!(text.contains("Daimon status: outcome unknown"));
    }
}
