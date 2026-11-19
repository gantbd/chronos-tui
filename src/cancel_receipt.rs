//! Route guarded cancellation through the local service's V3 receipt contract.
use chronos_client::{SchedulingWireClientV5, WireClientV1, WireClientV3, WorkflowWireClientV9};
use chronos_local::{CallError, LocalClient};
use chronos_protocol::{
    Command, Failure, FailureCode, Outcome, Request, Response, coordination_v3 as v3,
    scheduling_v5 as v5,
};

pub(crate) struct ReceiptClient(pub LocalClient);

impl WireClientV1 for ReceiptClient {
    type Error = CallError;

    fn request(&mut self, request: Request) -> Result<Response, CallError> {
        let Command::Cancel { expected, task } = request.command else {
            return self.0.request(request);
        };
        let outcome = self.cancel(v3::Mutation::Cancel { expected, task })?;
        Ok(Response {
            version: 1,
            outcome,
        })
    }
}

impl SchedulingWireClientV5 for ReceiptClient {
    type Error = CallError;

    fn request_scheduling_v5(&mut self, request: v5::Request) -> Result<v5::Response, CallError> {
        self.0.request_scheduling_v5(request)
    }
}

impl WorkflowWireClientV9 for ReceiptClient {
    type Error = CallError;

    fn request_workflow_v9(
        &mut self,
        request: chronos_protocol::workflow_v9::Request,
    ) -> Result<chronos_protocol::workflow_v9::Response, CallError> {
        self.0.request_workflow_v9(request)
    }
}

impl ReceiptClient {
    fn cancel(&mut self, command: v3::Mutation) -> Result<Outcome, CallError> {
        let next = self
            .0
            .request_v3(v3::Request {
                version: 3,
                command: v3::Command::NextInvocation {},
            })
            .map_err(|_| CallError::NotSent)?;
        let serial = match next.outcome {
            v3::Outcome::NextInvocation {
                serial: Some(serial),
            } => serial,
            v3::Outcome::Refused { code } => return Ok(refused(refusal_code(code))),
            _ => return Ok(refused(FailureCode::UnsupportedVersion)),
        };
        let key = invocation_key()?;
        let result = self.0.request_v3(v3::Request {
            version: 3,
            command: v3::Command::ExecuteInvocation {
                invocation: v3::Invocation {
                    serial: serial.clone(),
                    key: key.clone(),
                },
                command,
            },
        });
        let result = match result {
            Ok(v3::Response {
                outcome:
                    v3::Outcome::Refused {
                        code: v3::RefusalCode::Unavailable,
                    },
                ..
            })
            | Err(CallError::OutcomeUnknown | CallError::InvalidResponse) => {
                // An uncertain dispatch permits exactly a lookup of the original receipt.
                let reply = self
                    .0
                    .request_v3(v3::Request {
                        version: 3,
                        command: v3::Command::LookupInvocation {
                            invocation: v3::Invocation { serial, key },
                        },
                    })
                    .map_err(|_| CallError::OutcomeUnknown)?;
                if !matches!(reply.outcome, v3::Outcome::InvocationResult { .. }) {
                    return Err(CallError::OutcomeUnknown);
                }
                reply
            }
            result => result?,
        };
        match result.outcome {
            v3::Outcome::InvocationResult { result, .. } => match result {
                v3::MutationResult::Applied {} => Ok(Outcome::Applied {}),
                v3::MutationResult::RevisionMismatch { expected, actual } => Ok(Outcome::Refused {
                    failure: Failure::RevisionMismatch { expected, actual },
                }),
                v3::MutationResult::RevisionExhausted {} => {
                    Ok(refused(FailureCode::RevisionExhausted))
                }
                v3::MutationResult::Refused { code } => Ok(refused(match code {
                    v3::MutationCode::TaskNotFound => FailureCode::TaskNotFound,
                    v3::MutationCode::StaleReference => FailureCode::StaleReference,
                    v3::MutationCode::AlreadyTerminal => FailureCode::AlreadyTerminal,
                    _ => return Err(CallError::InvalidResponse),
                })),
            },
            v3::Outcome::Refused { code } => Ok(refused(refusal_code(code))),
            _ => Err(CallError::OutcomeUnknown),
        }
    }
}

fn invocation_key() -> Result<String, CallError> {
    use std::{fmt::Write as _, io::Read as _};
    let mut bytes = [0; 16];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut bytes))
        .map_err(|_| CallError::NotSent)?;
    let mut key = String::with_capacity(32);
    for byte in bytes {
        write!(key, "{byte:02x}").expect("writing to String cannot fail");
    }
    Ok(key)
}

fn refused(code: FailureCode) -> Outcome {
    Outcome::Refused {
        failure: Failure::Refused { code },
    }
}

fn refusal_code(code: v3::RefusalCode) -> FailureCode {
    match code {
        v3::RefusalCode::Unauthorized => FailureCode::Unauthorized,
        v3::RefusalCode::UnsupportedVersion => FailureCode::UnsupportedVersion,
        v3::RefusalCode::Limit => FailureCode::Limit,
        v3::RefusalCode::Capacity => FailureCode::Capacity,
        v3::RefusalCode::Exhausted => FailureCode::RevisionExhausted,
        _ => FailureCode::InvalidRequest,
    }
}
