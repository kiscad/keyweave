//! Windows CaptureSession adapter; transport must be nonblocking and private.
use crate::{
    mixed::SendMixedBackend,
    native::{CaptureSession, MixedCaptureSession},
    pump::SendBackend,
};
pub use keyweave_progress::{
    CHALLENGE_BYTES, Challenge, Policy, ProgressError, ProgressPhase, ProgressWatchdog,
    RECEIPT_BYTES, Receipt, RequestEvent, StopRequest,
};
use std::io::{Read, Write};
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("progress I/O is forbidden in a keyboard callback")]
    InCallback,
    #[error(transparent)]
    Stream(#[from] keyweave_progress::ClientError),
}
fn outside_callback() -> Result<(), ClientError> {
    if crate::native::outside_callback() {
        Ok(())
    } else {
        Err(ClientError::InCallback)
    }
}
mod sealed {
    pub trait Sealed {}
    impl<B: crate::pump::SendBackend + 'static> Sealed for crate::native::CaptureSession<B> {}
    impl<B: crate::mixed::SendMixedBackend + 'static> Sealed for crate::native::MixedCaptureSession<B> {}
}
/// Only owned native session types supply real poll epochs and lifecycle phases.
pub trait SessionProgress: sealed::Sealed {
    fn progress_epoch(&self) -> u64;
    fn progress_phase(&self) -> Option<ProgressPhase>;
}
impl<B: SendBackend + 'static> SessionProgress for CaptureSession<B> {
    fn progress_epoch(&self) -> u64 {
        self.progress_epoch()
    }
    fn progress_phase(&self) -> Option<ProgressPhase> {
        self.progress_phase()
    }
}
impl<B: SendMixedBackend + 'static> SessionProgress for MixedCaptureSession<B> {
    fn progress_epoch(&self) -> u64 {
        self.progress_epoch()
    }
    fn progress_phase(&self) -> Option<ProgressPhase> {
        self.progress_phase()
    }
}
pub struct ProgressClient<R: Read, W: Write> {
    inner: keyweave_progress::ProgressClient<R, W>,
}
impl<R: Read, W: Write> ProgressClient<R, W> {
    pub fn response_pending(&self) -> bool {
        self.inner.response_pending()
    }
    pub fn new(reader: R, writer: W, run_id: u64) -> Result<Self, ClientError> {
        outside_callback()?;
        Ok(Self {
            inner: keyweave_progress::ProgressClient::new(reader, writer, run_id)?,
        })
    }
    pub fn receive<S: SessionProgress>(&mut self, session: &S) -> Result<bool, ClientError> {
        outside_callback()?;
        self.inner
            .receive(session.progress_epoch())
            .map_err(Into::into)
    }
    pub fn publish<S: SessionProgress>(&mut self, session: &S) -> Result<bool, ClientError> {
        outside_callback()?;
        match session.progress_phase() {
            Some(phase) => self
                .inner
                .publish(session.progress_epoch(), phase)
                .map_err(Into::into),
            None => Ok(false),
        }
    }
    pub fn receive_command<S: SessionProgress>(
        &mut self,
        session: &S,
    ) -> Result<RequestEvent, ClientError> {
        outside_callback()?;
        self.inner
            .receive_command(session.progress_epoch())
            .map_err(Into::into)
    }
    pub fn flush(&mut self) -> Result<bool, ClientError> {
        outside_callback()?;
        self.inner.flush().map_err(Into::into)
    }
}
