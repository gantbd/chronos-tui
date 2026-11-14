//! Host entry point for text handoff over the current task observation.
use chronos_protocol::{Reference, Revision};
use std::{
    sync::{Arc, mpsc},
    thread,
};

#[derive(Clone, Copy)]
pub enum HandoffAction {
    Export,
    Review,
}

/// The host uses the workstream to resolve its configured service and source
/// association. The selector carries no source or destination authority.
pub struct HandoffRequest {
    pub action: HandoffAction,
    pub workstream: String,
    pub task: Reference,
    pub revision: Revision,
}

#[derive(Clone, Copy)]
pub enum HandoffResult {
    Exported,
    ReviewReady,
    Denied,
    Stale,
    Unavailable,
    PublicationUnknown,
}

pub(crate) type Host = Arc<dyn Fn(HandoffRequest) -> HandoffResult + Send + Sync>;

#[derive(Default)]
pub(crate) struct Handoff {
    pub host: Option<Host>,
    pub pending: Option<crate::explain::Selection>,
    pub worker: Option<mpsc::Receiver<HandoffResult>>,
    pub notice: &'static str,
    publication_unknown: bool,
}

impl Handoff {
    pub fn can_export(&self) -> bool {
        !self.publication_unknown
    }

    pub fn start(&mut self, request: HandoffRequest) {
        if matches!(request.action, HandoffAction::Export) && !self.can_export() {
            self.notice = "Export outcome unknown. Inspect the destination with the host.";
            return;
        }
        if self.worker.is_some() {
            return;
        }
        let Some(host) = self.host.clone() else {
            return;
        };
        let (send, receive) = mpsc::sync_channel(1);
        thread::spawn(move || {
            let _ = send.send(host(request));
        });
        self.worker = Some(receive);
        self.notice = "Waiting for the handoff host…";
    }

    pub fn poll(&mut self) -> bool {
        let Some(worker) = &self.worker else {
            return false;
        };
        self.notice = match worker.try_recv() {
            Ok(HandoffResult::Exported) => "Handoff exported.",
            Ok(HandoffResult::ReviewReady) => "Review ready in the handoff host.",
            Ok(HandoffResult::Denied) => "Handoff denied by the host.",
            Ok(HandoffResult::Stale) => "Handoff target changed. Refresh before trying again.",
            Ok(HandoffResult::Unavailable) => "Handoff unavailable.",
            Ok(HandoffResult::PublicationUnknown) | Err(mpsc::TryRecvError::Disconnected) => {
                self.publication_unknown = true;
                "Handoff outcome unknown. Inspect the destination with the host."
            }
            Err(mpsc::TryRecvError::Empty) => return false,
        };
        self.worker = None;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn unknown_publication_blocks_another_export() {
        let (sent, received) = mpsc::channel();
        let mut handoff = Handoff {
            host: Some(Arc::new(move |request| {
                sent.send(request).unwrap();
                HandoffResult::PublicationUnknown
            })),
            ..Handoff::default()
        };
        let request = || HandoffRequest {
            action: HandoffAction::Export,
            workstream: "Atlas".into(),
            task: Reference {
                task: "review".into(),
                incarnation: "3".into(),
            },
            revision: Revision::Finite("9".into()),
        };
        handoff.start(request());
        let observed = received.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(observed.workstream, "Atlas");
        assert_eq!(observed.task.incarnation, "3");
        let deadline = Instant::now() + Duration::from_secs(1);
        while !handoff.poll() {
            assert!(Instant::now() < deadline);
            thread::yield_now();
        }
        handoff.start(request());
        assert!(handoff.worker.is_none());
        assert!(matches!(
            received.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
    }
}
