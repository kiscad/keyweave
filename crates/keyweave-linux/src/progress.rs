//! Linux runtime/FD adapter for the shared progress protocol.
use crate::{capture::CaptureSource, pump::FrameBackend, runtime::Runtime};
pub use keyweave_progress::{
    CHALLENGE_BYTES, Challenge, ClientError, Policy, ProgressError, ProgressWatchdog,
    RECEIPT_BYTES, Receipt, RequestEvent, StopRequest,
};
use std::io::{Read, Write};
pub struct ProgressClient<R: Read, W: Write> {
    inner: keyweave_progress::ProgressClient<R, W>,
}
impl<R: Read, W: Write> ProgressClient<R, W> {
    /// Cold startup only: retain a pending challenge at epoch zero, never
    /// publish a metadata/standby heartbeat. Same client is handed to Runtime.
    pub(crate) fn receive_startup_command(&mut self) -> Result<RequestEvent, ClientError> {
        self.inner.receive_command(0)
    }
    pub(crate) fn receive_control_epoch(
        &mut self,
        epoch: u64,
    ) -> Result<RequestEvent, ClientError> {
        self.inner.receive_command(epoch)
    }
    pub(crate) fn last_request_id(&self) -> u64 {
        self.inner.last_request_id()
    }
    pub fn new(reader: R, writer: W, run_id: u64) -> Result<Self, ClientError> {
        Ok(Self {
            inner: keyweave_progress::ProgressClient::new(reader, writer, run_id)?,
        })
    }
    pub fn receive<S: CaptureSource, B: FrameBackend>(
        &mut self,
        runtime: &Runtime<S, B>,
    ) -> Result<bool, ClientError> {
        self.inner.receive(runtime.progress_epoch())
    }
    /// Explicit opt-in: Stop has priority over a pending challenge/reply.
    pub fn receive_command<S: CaptureSource, B: FrameBackend>(
        &mut self,
        runtime: &Runtime<S, B>,
    ) -> Result<RequestEvent, ClientError> {
        self.inner.receive_command(runtime.progress_epoch())
    }
    pub fn publish<S: CaptureSource, B: FrameBackend>(
        &mut self,
        runtime: &Runtime<S, B>,
    ) -> Result<bool, ClientError> {
        self.inner
            .publish(runtime.progress_epoch(), runtime.phase())
    }
    pub fn flush(&mut self) -> Result<bool, ClientError> {
        self.inner.flush()
    }
    #[cfg(target_os = "linux")]
    pub(crate) fn wait_requests(&self) -> [crate::reactor::WaitRequest<'_>; 2]
    where
        R: std::os::fd::AsFd,
        W: std::os::fd::AsFd,
    {
        self.wait_requests_with_control(false)
    }
    #[cfg(target_os = "linux")]
    pub(crate) fn wait_requests_with_control(
        &self,
        control: bool,
    ) -> [crate::reactor::WaitRequest<'_>; 2]
    where
        R: std::os::fd::AsFd,
        W: std::os::fd::AsFd,
    {
        use crate::reactor::{Interest, WaitRequest};
        [
            WaitRequest {
                fd: self.inner.reader().as_fd(),
                interest: if control || self.inner.can_receive() {
                    Interest::Read
                } else {
                    Interest::Errors
                },
            },
            WaitRequest {
                fd: self.inner.writer().as_fd(),
                interest: if self.inner.response_pending() {
                    Interest::Write
                } else {
                    Interest::Errors
                },
            },
        ]
    }
}
