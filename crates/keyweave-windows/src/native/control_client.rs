//! Expected-instance client for one bounded public message transaction.
use super::{
    control_identity::{ControlIdentity, ServerPeer},
    control_message_pipe::{self, ControlMessagePipe, ControlPipeCleanup},
};
use crate::{
    control_identity::{ControlIdentityError, ControlServerInstance},
    session_control::{ClientTransport, MESSAGE_CAPACITY, ReadResult},
};
use std::{io, task::Poll};
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error(
    "control client cleanup unresolved: pipe={pipe:?}, peer_close={peer_close:?}, peer_released={peer_released}"
)]
pub struct ControlClientCleanup {
    pub pipe: Option<ControlPipeCleanup>,
    pub peer_close: Option<ControlIdentityError>,
    pub peer_released: bool,
}
impl ControlClientCleanup {
    pub fn complete(&self) -> bool {
        self.peer_released
            && self.peer_close.is_none()
            && self.pipe.as_ref().is_none_or(ControlPipeCleanup::complete)
    }
    fn failed(&self) -> bool {
        self.peer_close.is_some()
            || self.pipe.is_some_and(|p| {
                p.in_callback
                    || p.cancel_error.is_some()
                    || p.wait_error.is_some()
                    || p.pipe_close_error.is_some()
                    || p.event_close_error.is_some()
            })
    }
}
pub struct ControlClientTransport {
    name: String,
    identity: ControlIdentity,
    pipe: Option<ControlMessagePipe>,
    peer: Option<ServerPeer>,
    authorized: bool,
    attempted: bool,
    failed: bool,
    closing: bool,
    peer_close_error: Option<ControlIdentityError>,
}
impl std::fmt::Debug for ControlClientTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ControlClientTransport")
            .finish_non_exhaustive()
    }
}
impl ControlClientTransport {
    /// Expected PID/creation metadata must come from the selected host instance;
    /// it is a stale-instance fence, not image authentication or a secret.
    /// Process pin is acquired before any pipe open and retained through close.
    pub fn new(name: &str, instance: ControlServerInstance) -> io::Result<Self> {
        if !super::outside_callback() {
            return Err(io::Error::other(
                "control client preparation in input callback",
            ));
        }
        control_message_pipe::name(name)?;
        let identity = ControlIdentity::capture().map_err(io::Error::other)?;
        let peer = ServerPeer::open(&identity, instance).map_err(io::Error::other)?;
        Ok(Self {
            name: name.to_owned(),
            identity,
            pipe: None,
            peer: Some(peer),
            authorized: false,
            attempted: false,
            failed: false,
            closing: false,
            peer_close_error: None,
        })
    }
    fn check(&self) -> io::Result<()> {
        if self.failed || self.closing {
            return Err(io::Error::other("control client is terminal"));
        }
        let peer = self
            .peer
            .as_ref()
            .ok_or_else(|| io::Error::other("missing control server pin"))?;
        peer.check(&self.identity).map_err(io::Error::other)?;
        if let Some(pipe) = &self.pipe
            && pipe.server_process_id()? != peer.process_id()
        {
            return Err(io::Error::other(
                "control pipe server does not match expected instance",
            ));
        }
        Ok(())
    }
    /// Nonblocking independent cleanup facts. Keep pin until pipe I/O and owned
    /// close are confirmed; retry only within the outer absolute close deadline.
    pub fn close_resources(&mut self) -> ControlClientCleanup {
        self.closing = true;
        self.authorized = false;
        let pipe = self.pipe.as_mut().map(ControlMessagePipe::close);
        if pipe.is_none_or(|p| p.resources_released)
            && let Some(peer) = &mut self.peer
            && let Err(error) = peer.close()
        {
            self.peer_close_error.get_or_insert(error);
        }
        ControlClientCleanup {
            pipe,
            peer_close: self.peer_close_error.clone(),
            peer_released: self.peer.as_ref().is_none_or(ServerPeer::released),
        }
    }
}
impl ClientTransport for ControlClientTransport {
    fn connect(&mut self) -> Poll<io::Result<()>> {
        let result = (|| {
            self.check()?;
            if self.pipe.is_some() {
                return Err(io::Error::other("control client already connected"));
            }
            match ControlMessagePipe::open(&self.name) {
                Ok(pipe) => {
                    self.pipe = Some(pipe);
                    self.check()?;
                    Ok(true)
                }
                Err(failure)
                    if matches!(failure.operation.raw_os_error(), Some(2 | 231))
                        && failure.cleanup.complete()
                        && failure.security_cleanup.is_none() =>
                {
                    Ok(false)
                }
                Err(failure) => Err(io::Error::other(failure)),
            }
        })();
        match result {
            Ok(true) => Poll::Ready(Ok(())),
            Ok(false) => Poll::Pending,
            Err(error) => {
                self.failed = true;
                Poll::Ready(Err(error))
            }
        }
    }
    fn authorize(&mut self) -> Poll<io::Result<bool>> {
        self.authorized = false;
        if std::mem::replace(&mut self.attempted, true) || self.pipe.is_none() {
            self.failed = true;
            return Poll::Ready(Err(io::Error::other(
                "invalid control client authorization",
            )));
        }
        let result = self.check();
        self.authorized = result.is_ok();
        self.failed = result.is_err();
        Poll::Ready(result.map(|()| true))
    }
    fn write(&mut self, message: &[u8; 32]) -> Poll<io::Result<()>> {
        if !self.authorized {
            return Poll::Ready(Err(io::Error::other(
                "control request before server authorization",
            )));
        }
        if let Err(error) = self.check() {
            self.failed = true;
            return Poll::Ready(Err(error));
        }
        let result = match &mut self.pipe {
            Some(pipe) => pipe.poll_write(message),
            None => Poll::Ready(Err(io::Error::other("missing control client pipe"))),
        };
        match result {
            Poll::Ready(Ok(())) => match self.check() {
                Ok(()) => Poll::Ready(Ok(())),
                Err(error) => {
                    self.failed = true;
                    Poll::Ready(Err(error))
                }
            },
            Poll::Ready(Err(error)) => {
                self.failed = true;
                Poll::Ready(Err(error))
            }
            Poll::Pending => Poll::Pending,
        }
    }
    fn read(&mut self, out: &mut [u8; MESSAGE_CAPACITY]) -> Poll<io::Result<ReadResult>> {
        if !self.authorized {
            return Poll::Ready(Err(io::Error::other(
                "control reply before server authorization",
            )));
        }
        if let Err(error) = self.check() {
            self.failed = true;
            return Poll::Ready(Err(error));
        }
        let result = match &mut self.pipe {
            Some(pipe) => pipe.poll_read(out),
            None => Poll::Ready(Err(io::Error::other("missing control client pipe"))),
        };
        match result {
            Poll::Ready(Ok(value)) => match self.check() {
                Ok(()) => Poll::Ready(Ok(value)),
                Err(error) => {
                    self.failed = true;
                    Poll::Ready(Err(error))
                }
            },
            Poll::Ready(Err(error)) => {
                self.failed = true;
                Poll::Ready(Err(error))
            }
            Poll::Pending => Poll::Pending,
        }
    }
    fn close(&mut self) -> Poll<io::Result<()>> {
        let report = self.close_resources();
        if report.failed() {
            Poll::Ready(Err(io::Error::other(report)))
        } else if report.complete() {
            Poll::Ready(Ok(()))
        } else {
            Poll::Pending
        }
    }
}
impl Drop for ControlClientTransport {
    fn drop(&mut self) {
        let report = self.close_resources();
        if report.pipe.is_some_and(|p| p.completion_pending)
            && let Some(peer) = self.peer.take()
        {
            // Pending OS storage/pipe is intentionally retained by its owner.
            // Keep this process pin as well; no subsequent authorization occurs.
            std::mem::forget(peer);
        }
    }
}
#[cfg(test)]
mod tests;
