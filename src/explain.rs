//! Selected-task readiness through the existing authorized V2 read.
use chronos_client::WireClientV2;
use chronos_local::LocalClient;
use chronos_protocol::{Reference, Revision, coordination_v2 as v2};
use std::{path::PathBuf, sync::mpsc};

#[derive(PartialEq, Eq)]
pub(crate) struct Selection {
    pub stream: usize,
    pub observation: u64,
    pub task: String,
    pub incarnation: String,
    pub revision: String,
}

impl Selection {
    pub(crate) fn new(
        stream: usize,
        observation: u64,
        reference: &Reference,
        revision: &str,
    ) -> Self {
        Self {
            stream,
            observation,
            task: reference.task.clone(),
            incarnation: reference.incarnation.clone(),
            revision: revision.into(),
        }
    }
}

pub(crate) enum Detail {
    Reading(mpsc::Receiver<Result<v2::Explanation, &'static str>>),
    Ready(Result<v2::Explanation, &'static str>),
}

impl Detail {
    pub(crate) fn start(directory: PathBuf, selection: &Selection) -> Self {
        let (send, receive) = mpsc::sync_channel(1);
        let task = selection.task.clone();
        let incarnation = selection.incarnation.clone();
        let revision = selection.revision.clone();
        std::thread::spawn(move || {
            let result = (|| {
                let response = LocalClient::new(directory)
                    .request_v2(v2::Request {
                        version: 2,
                        command: v2::Command::Explain { task: task.clone() },
                    })
                    .map_err(|_| "Explanation unavailable")?;
                response
                    .validate(chronos_local::wire_limits().max_id_bytes)
                    .map_err(|_| "Invalid explanation response")?;
                let v2::Outcome::Explanation {
                    value: Some(explanation),
                } = response.outcome
                else {
                    return Err("Explanation refused or task no longer present");
                };
                let reference = &explanation.entry.reference;
                let exact_revision = matches!(
                    &explanation.revision,
                    Revision::Finite(value) if value == &revision
                ) || matches!(explanation.revision, Revision::Exhausted)
                    && revision == "exhausted";
                if !exact_revision || reference.task != task || reference.incarnation != incarnation
                {
                    return Err(
                        "Explanation is from a different observation. Refresh and inspect again.",
                    );
                }
                Ok(explanation)
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
            Err(mpsc::TryRecvError::Disconnected) => Err("Explanation unavailable"),
        };
        *self = Self::Ready(result);
        true
    }

    pub(crate) fn lines(&self) -> String {
        match self {
            Self::Reading(_) => "Reading explanation…".into(),
            Self::Ready(Err(message)) => (*message).into(),
            Self::Ready(Ok(explanation)) => {
                let reason = match explanation.readiness {
                    v2::Readiness::DependenciesPending { remaining } => {
                        format!("Awaiting {remaining} prerequisite(s)")
                    }
                    v2::Readiness::ReadyForAuthorizedClaim {} => {
                        "Ready for authorized claim".into()
                    }
                    v2::Readiness::RunningAttempt {} => "Attempt running".into(),
                    v2::Readiness::UncertainRequiresReconciliation {} => {
                        "Attempt needs reconciliation".into()
                    }
                    v2::Readiness::Terminal {} => "Terminal task".into(),
                };
                format!(
                    "{reason}\nReferencing tasks {}",
                    explanation.referencing_tasks
                )
            }
        }
    }
}
