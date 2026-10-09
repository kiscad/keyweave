//! Guarded capture loop; owns native progress channels, never logs to stdio.
use super::{
    progress_pipe::{PipeReader, PipeWriter},
    session::{CaptureSession, SessionError, SessionStep, ShutdownReport},
    worker_startup::{WorkerParentGuard, WorkerStartup},
};
use crate::{
    keys::{IdentityPolicy, InjectionTag},
    progress::{ClientError, ProgressClient},
    pump::SendBackend,
    runtime::ActorPhase,
};
use keyweave_engine::ConfigSnapshot;
use std::{
    io::{Read, Write},
    sync::Arc,
};
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum WorkerAccessError {
    #[error("worker execution forbidden in keyboard callback")]
    InCallback,
    #[error("worker terminal report missing")]
    State,
}
#[derive(Debug, thiserror::Error)]
pub enum WorkerInstallError {
    #[error(transparent)]
    Access(#[from] WorkerAccessError),
    #[error(transparent)]
    Progress(#[from] ClientError),
    #[error(transparent)]
    Session(#[from] SessionError),
}
#[derive(Debug, thiserror::Error)]
pub enum WorkerFault {
    #[error(transparent)]
    Progress(ClientError),
    #[error(transparent)]
    Session(SessionError),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkerStopReason {
    Quit,
    Emergency,
    Requested,
    Fault,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkerStep {
    Session(SessionStep),
    Stopped,
}
/// A completed frame can be an earlier frozen Active receipt, not a terminal
/// acknowledgement. Pending/absence never manufacture a newer Session epoch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkerReply {
    NoFrame,
    FrameSent,
    FramePending,
    Failed,
}
#[derive(Debug)]
pub struct WorkerReport {
    pub reason: WorkerStopReason,
    pub primary: Option<WorkerFault>,
    pub progress: Option<ClientError>,
    pub reply: WorkerReply,
}
fn context() -> Result<(), WorkerAccessError> {
    if super::outside_callback() {
        Ok(())
    } else {
        Err(WorkerAccessError::InCallback)
    }
}
struct Driver<B: SendBackend + 'static, R: Read, W: Write> {
    // Drop order is intentional: capture disables before progress endpoints.
    session: CaptureSession<B>,
    client: ProgressClient<R, W>,
    report: Option<WorkerReport>,
}
impl<B: SendBackend + 'static, R: Read, W: Write> Driver<B, R, W> {
    fn finish(
        &mut self,
        reason: WorkerStopReason,
        primary: Option<WorkerFault>,
        progress: Option<ClientError>,
        reply: WorkerReply,
    ) -> WorkerStep {
        // Explicit Hook-close-first recovery; retain this owner/ledger and its
        // independent shutdown report even if recovery remains Unresolved.
        self.session.shutdown();
        self.report = Some(WorkerReport {
            reason,
            primary,
            progress,
            reply,
        });
        WorkerStep::Stopped
    }
    fn reply(&self, sent: bool) -> WorkerReply {
        if sent {
            WorkerReply::FrameSent
        } else if self.client.response_pending() {
            WorkerReply::FramePending
        } else {
            WorkerReply::NoFrame
        }
    }
    fn step(&mut self) -> Result<WorkerStep, WorkerAccessError> {
        context()?;
        if self.report.is_some() {
            return Ok(WorkerStep::Stopped);
        }
        if let Err(error) = self.client.receive(&self.session) {
            let reply = if self.client.response_pending() {
                WorkerReply::FramePending
            } else {
                WorkerReply::NoFrame
            };
            return Ok(self.finish(
                WorkerStopReason::Fault,
                Some(WorkerFault::Progress(error)),
                None,
                reply,
            ));
        }
        let cycle = self.session.poll();
        let published = self.client.publish(&self.session);
        let (reply, progress) = match published {
            Ok(sent) => (self.reply(sent), None),
            Err(e) => (WorkerReply::Failed, Some(e)),
        };
        match cycle {
            Err(e) => Ok(self.finish(
                WorkerStopReason::Fault,
                Some(WorkerFault::Session(e)),
                progress,
                reply,
            )),
            Ok(SessionStep::Quit) => Ok(self.finish(WorkerStopReason::Quit, None, progress, reply)),
            Ok(SessionStep::Emergency) => {
                Ok(self.finish(WorkerStopReason::Emergency, None, progress, reply))
            }
            Ok(step) => {
                if let Some(error) = progress {
                    Ok(self.finish(
                        WorkerStopReason::Fault,
                        Some(WorkerFault::Progress(error)),
                        None,
                        reply,
                    ))
                } else {
                    Ok(WorkerStep::Session(step))
                }
            }
        }
    }
    fn request_stop(&mut self) -> Result<WorkerStep, WorkerAccessError> {
        context()?;
        if self.report.is_some() {
            return Ok(WorkerStep::Stopped);
        }
        let reply = if self.client.response_pending() {
            WorkerReply::FramePending
        } else {
            WorkerReply::NoFrame
        };
        Ok(self.finish(WorkerStopReason::Requested, None, None, reply))
    }
}
/// Real main must claim WorkerStartup before backend/input/threads/stdio.
/// No Clone/raw endpoints; preserve this same-thread owner through termination.
pub struct Worker<B: SendBackend + 'static> {
    _guard: WorkerParentGuard,
    driver: Driver<B, PipeReader, PipeWriter>,
}
impl<B: SendBackend + 'static> Worker<B> {
    /// Backend creation must not have captured/injected before admission.
    /// Installation failure requires process exit, not another startup attempt.
    pub fn install(
        startup: WorkerStartup,
        backend: B,
        tag: InjectionTag,
        policy: IdentityPolicy,
        config: Arc<ConfigSnapshot>,
    ) -> Result<Self, WorkerInstallError> {
        context()?;
        let (guard, run_id, reader, writer) = startup.into_parts();
        let client = ProgressClient::new(reader, writer, run_id)?;
        let session = CaptureSession::install(backend, tag, policy, config)?;
        Ok(Self {
            _guard: guard,
            driver: Driver {
                session,
                client,
                report: None,
            },
        })
    }
    pub fn step(&mut self) -> Result<WorkerStep, WorkerAccessError> {
        self.driver.step()
    }
    pub fn request_stop(&mut self) -> Result<WorkerStep, WorkerAccessError> {
        self.driver.request_stop()
    }
    /// Runs until the loop stops. The report is not output recovery/exit proof;
    /// caller must also inspect shutdown_report and leave original stdio binary.
    pub fn run(&mut self) -> Result<&WorkerReport, WorkerAccessError> {
        while self.step()? != WorkerStep::Stopped {}
        self.report().ok_or(WorkerAccessError::State)
    }
    pub fn report(&self) -> Option<&WorkerReport> {
        self.driver.report.as_ref()
    }
    pub fn shutdown_report(&self) -> Option<&ShutdownReport> {
        self.driver.session.shutdown_report()
    }
    pub fn phase(&self) -> ActorPhase {
        self.driver.session.phase()
    }
    pub fn progress_epoch(&self) -> u64 {
        self.driver.session.progress_epoch()
    }
}
#[cfg(test)]
mod tests;
