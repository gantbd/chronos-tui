//! Nonblocking read-only product-alignment observation for one workstream.

use chronos_client::AlignmentWireClientV1;
use chronos_protocol::alignment_v1::{
    Command, Currentness, Limits, Observation, Outcome, Recommendation, RefusalCode, Request,
};
use std::{path::PathBuf, sync::mpsc, thread};

const LIMITS: Limits = Limits {
    max_field_bytes: 256,
    max_evidence: 128,
    max_next_tasks: 64,
};

pub struct Alignment {
    directory: PathBuf,
    scope: String,
    observation: Option<Box<Observation>>,
    worker: Option<
        mpsc::Receiver<Result<chronos_protocol::alignment_v1::Response, chronos_local::CallError>>,
    >,
    notice: &'static str,
}

impl Alignment {
    pub fn new(directory: PathBuf, scope: String) -> Self {
        Self {
            directory,
            scope,
            observation: None,
            worker: None,
            notice: "Press r to read the product alignment proposal.",
        }
    }

    pub fn start(&mut self) {
        if self.worker.is_some() {
            return;
        }
        let directory = self.directory.clone();
        let scope = self.scope.clone();
        let (send, receive) = mpsc::sync_channel(1);
        thread::spawn(move || {
            let mut client = chronos_local::alignment::AlignmentLocalClient::new(directory, LIMITS);
            let _ = send.send(client.request_alignment_v1(Request {
                version: 1,
                command: Command::Observe {
                    chronos_scope: scope,
                },
            }));
        });
        self.worker = Some(receive);
        self.notice = "Reading the current alignment frontier…";
    }

    pub fn poll(&mut self) -> bool {
        let Some(worker) = &self.worker else {
            return false;
        };
        match worker.try_recv() {
            Ok(Ok(response)) => {
                match response.outcome {
                    Outcome::Observed { observation } => {
                        self.observation = Some(observation);
                        self.notice = "Current authorized alignment observation.";
                    }
                    Outcome::Refused { code } => {
                        self.observation = None;
                        self.notice = refusal_notice(code);
                    }
                }
                self.worker = None;
                true
            }
            Ok(Err(error)) => {
                self.observation = None;
                self.notice = match error {
                    chronos_local::CallError::NotSent => {
                        "Alignment endpoint unavailable; no request was sent."
                    }
                    chronos_local::CallError::OutcomeUnknown => {
                        "Alignment reply unavailable after the read request. Refresh safely."
                    }
                    chronos_local::CallError::InvalidResponse => {
                        "Alignment endpoint returned an invalid response."
                    }
                };
                self.worker = None;
                true
            }
            Err(mpsc::TryRecvError::Empty) => false,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.observation = None;
                self.notice = "Alignment reader stopped before returning a result.";
                self.worker = None;
                true
            }
        }
    }

    pub fn lines(&self) -> String {
        let Some(value) = &self.observation else {
            return format!(
                "Chronos scope  {}\n\n{}\n\nThis surface is observation-only. It cannot apply, accept, claim, reprioritize, or sign off.",
                self.scope, self.notice
            );
        };
        let task = &value.control_task;
        let mut next = value
            .recommended_next
            .iter()
            .map(|reference| format!("  • {} #{}", reference.task, reference.incarnation))
            .collect::<Vec<_>>();
        if next.is_empty() {
            next.push("  — no next work; recommendation requires human attention".into());
        }
        format!(
            "Chronos scope  {}\nCurrentness    {}\nRecommendation {}\n\nGoal           {} / {} @ {}\nProposal       {} / {} @ {}\nAuthor         {}\n\nControl task   {} #{}\nTask revision  {}\nPhase          {}\nPriority       {}\nPrerequisites  {}\n\nEnsemble\n  Advocate     {}\n  Product mgr  {}\n  Supervisor   {}\n\nEvidence       {} frontier · {} dissent · {} pending human gates · {} acceptance\n\nRecommended next\n{}\n\n{}\nObservation only · no apply, accept, claim, reprioritize, or sign-off authority.",
            value.chronos_scope,
            currentness(value.currentness),
            recommendation(value.recommendation),
            value.goal.namespace,
            value.goal.document,
            value.goal.evidence_revision,
            value.proposal.namespace,
            value.proposal.document,
            value.proposal.evidence_revision,
            value.author,
            task.reference.task,
            task.reference.incarnation,
            revision(&task.revision),
            phase(&task.snapshot.phase),
            task.snapshot.priority,
            task.snapshot.dependency_count,
            value.cohort.external_advocate,
            value.cohort.product_manager,
            value.cohort.frontier_supervisor,
            value.evidence_frontier.len(),
            value.unresolved_dissent.len(),
            value.pending_human_gates.len(),
            value.acceptance_evidence.len(),
            next.join("\n"),
            self.notice,
        )
    }

    pub const fn notice(&self) -> &'static str {
        self.notice
    }

    pub(crate) fn observation(&self) -> Option<&Observation> {
        self.observation.as_deref()
    }
}

const fn refusal_notice(code: RefusalCode) -> &'static str {
    match code {
        RefusalCode::InvalidRequest => "Alignment request was refused as invalid.",
        RefusalCode::UnsupportedVersion => "Alignment endpoint does not support this profile.",
        RefusalCode::Unauthorized => "Alignment disclosure is not authorized for this scope.",
        RefusalCode::Unavailable => "Current alignment evidence is unavailable.",
        RefusalCode::Limit => "Alignment observation exceeded a configured bound.",
        RefusalCode::Capacity => "Alignment endpoint could not allocate bounded response state.",
    }
}

const fn currentness(value: Currentness) -> &'static str {
    match value {
        Currentness::Current => "current",
        Currentness::GoalChanged => "goal changed",
        Currentness::WorkChanged => "work changed",
        Currentness::CohortChanged => "ensemble changed",
        Currentness::EvidenceChanged => "evidence changed",
    }
}

const fn recommendation(value: Recommendation) -> &'static str {
    match value {
        Recommendation::Continue => "continue",
        Recommendation::Reorder => "reorder",
        Recommendation::ReviseGoal => "revise goal · human review",
        Recommendation::EscalateHuman => "escalate to human",
        Recommendation::Stop => "stop · human review",
    }
}

fn revision(value: &chronos_protocol::Revision) -> &str {
    match value {
        chronos_protocol::Revision::Finite(value) => value,
        chronos_protocol::Revision::Exhausted => "exhausted",
    }
}

const fn phase(value: &chronos_protocol::Phase) -> &'static str {
    match value {
        chronos_protocol::Phase::Blocked => "blocked",
        chronos_protocol::Phase::Ready => "ready",
        chronos_protocol::Phase::Running => "running",
        chronos_protocol::Phase::Uncertain => "uncertain",
        chronos_protocol::Phase::Succeeded => "succeeded",
        chronos_protocol::Phase::Failed => "failed",
        chronos_protocol::Phase::Cancelled => "cancelled",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chronos_protocol::alignment_v1::{
        Cohort, ControlTaskObservation, OwnerEvidenceReference, SourceReference, SourceTarget,
    };
    use chronos_protocol::{Phase, Reference, Revision, Snapshot};

    fn source(document: &str) -> SourceReference {
        SourceReference {
            namespace: "vellum-workspace".into(),
            document: document.into(),
            evidence_domain: "vellum-source-version".into(),
            evidence_revision: "rev-7".into(),
            byte_length: "128".into(),
            target: SourceTarget::Document,
        }
    }

    fn evidence(resource: &str) -> OwnerEvidenceReference {
        OwnerEvidenceReference {
            owner: "tellus".into(),
            resource: resource.into(),
            revision: "1".into(),
        }
    }

    #[test]
    fn successful_observation_orients_without_claiming_decision_authority() {
        let mut alignment = Alignment::new("/unused".into(), "product-delivery".into());
        alignment.observation = Some(Box::new(Observation {
            version: 1,
            record_revision: "0".into(),
            chronos_scope: "product-delivery".into(),
            goal: source("delivery-goal"),
            control_task: ControlTaskObservation {
                reference: Reference {
                    task: "pm-control".into(),
                    incarnation: "91".into(),
                },
                revision: Revision::Finite("66".into()),
                snapshot: Snapshot {
                    phase: Phase::Blocked,
                    attempt_number: None,
                    dependency_count: 6,
                    required_capability_count: 0,
                    priority: 10,
                    max_attempts: 1,
                },
            },
            cohort: Cohort {
                external_advocate: "human-advocate".into(),
                product_manager: "product-manager".into(),
                frontier_supervisor: "pantheon-insider".into(),
            },
            budget_policy: evidence("budget"),
            evidence_frontier: vec![evidence("qualification")],
            unresolved_dissent: vec![evidence("dissent")],
            pending_human_gates: vec![evidence("go-no-go")],
            proposal: source("alignment-proposal"),
            author: "product-manager".into(),
            recommendation: Recommendation::Continue,
            recommended_next: vec![Reference {
                task: "next-bounded-slice".into(),
                incarnation: "2".into(),
            }],
            acceptance_evidence: Vec::new(),
            currentness: Currentness::Current,
        }));
        alignment.notice = "Current authorized alignment observation.";

        let rendered = alignment.lines();
        assert!(rendered.contains("Recommendation continue"));
        assert!(rendered.contains("Product mgr  product-manager"));
        assert!(rendered.contains("pm-control #91"));
        assert!(rendered.contains("next-bounded-slice #2"));
        assert!(rendered.contains("1 pending human gates"));
        assert!(rendered.contains("no apply, accept, claim, reprioritize, or sign-off authority"));
    }
}
