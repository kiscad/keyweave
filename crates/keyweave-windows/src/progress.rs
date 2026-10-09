//! Windows CaptureSession adapter; transport must be nonblocking and private.
use crate::{native::CaptureSession, pump::SendBackend};
pub use keyweave_progress::{
    CHALLENGE_BYTES, Challenge, Policy, ProgressError, ProgressPhase, ProgressWatchdog,
    RECEIPT_BYTES, Receipt,
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
    pub fn receive<B: SendBackend + 'static>(
        &mut self,
        session: &CaptureSession<B>,
    ) -> Result<bool, ClientError> {
        outside_callback()?;
        self.inner
            .receive(session.progress_epoch())
            .map_err(Into::into)
    }
    pub fn publish<B: SendBackend + 'static>(
        &mut self,
        session: &CaptureSession<B>,
    ) -> Result<bool, ClientError> {
        outside_callback()?;
        match session.progress_phase() {
            Some(phase) => self
                .inner
                .publish(session.progress_epoch(), phase)
                .map_err(Into::into),
            None => Ok(false),
        }
    }
    pub fn flush(&mut self) -> Result<bool, ClientError> {
        outside_callback()?;
        self.inner.flush().map_err(Into::into)
    }
}
