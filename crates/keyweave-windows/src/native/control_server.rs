//! Actual server-side message-token authorization adapter. No host auto-start.
use super::{
    control_identity::ControlIdentity,
    control_message_pipe::{ControlMessagePipe, ControlPipeCleanup, ControlPipeCreateError},
};
use crate::session_control::{MESSAGE_CAPACITY, ReadResult, ServerTransport};
use std::{io, task::Poll};
/// Owns a cold caller baseline and one native message instance. Pair with the
/// bounded Server machine; poll owned worker supervision before public IPC.
pub struct ControlServerTransport {
    pipe: ControlMessagePipe,
    identity: ControlIdentity,
    authorized: bool,
    attempted: bool,
}
impl std::fmt::Debug for ControlServerTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ControlServerTransport")
            .finish_non_exhaustive()
    }
}
impl ControlServerTransport {
    pub fn listen(name: &str) -> Result<Self, ControlPipeCreateError> {
        let identity = ControlIdentity::capture().map_err(|failure| ControlPipeCreateError {
            operation: io::Error::other(failure),
            cleanup: ControlPipeCleanup {
                resources_released: true,
                ..ControlPipeCleanup::default()
            },
            security_cleanup: None,
        })?;
        let pipe = ControlMessagePipe::listen(name, &identity)?;
        Ok(Self {
            pipe,
            identity,
            authorized: false,
            attempted: false,
        })
    }
    /// Repeated nonblocking calls only within the machine's absolute deadline;
    /// a pending/cancel request alone never proves close or input recovery.
    pub fn close(&mut self) -> ControlPipeCleanup {
        self.authorized = false;
        self.pipe.close()
    }
}
impl ServerTransport for ControlServerTransport {
    fn accept(&mut self) -> Poll<io::Result<()>> {
        let result = self.pipe.poll_accept();
        if matches!(result, Poll::Ready(Ok(()))) {
            self.authorized = false;
            self.attempted = false;
        }
        result
    }
    fn authorize(&mut self) -> Poll<io::Result<bool>> {
        self.authorized = false;
        if std::mem::replace(&mut self.attempted, true) {
            return Poll::Ready(Err(io::Error::other(
                "control connection authorization already attempted",
            )));
        }
        let result = self
            .pipe
            .authorize_message(&self.identity)
            .map_err(io::Error::other);
        if let Ok(true) = result {
            self.authorized = true;
        }
        Poll::Ready(result)
    }
    fn read(&mut self, out: &mut [u8; MESSAGE_CAPACITY]) -> Poll<io::Result<ReadResult>> {
        self.authorized = false;
        self.pipe.poll_read(out)
    }
    fn write(&mut self, bytes: &[u8; 64]) -> Poll<io::Result<()>> {
        if !self.authorized {
            return Poll::Ready(Err(io::Error::other(
                "control reply before message authorization",
            )));
        }
        self.pipe.poll_write(bytes)
    }
    fn disconnect(&mut self) -> Poll<io::Result<()>> {
        self.authorized = false;
        self.pipe.poll_disconnect()
    }
}
#[cfg(test)]
mod tests;
