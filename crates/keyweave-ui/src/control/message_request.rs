//! One Windows transaction step per frame, then independent bounded cleanup.
use keyweave_progress::session_control::{Reply, Request};
use keyweave_windows::session_control::{Client, ClientStep, ClientTransport, EndReason};
use std::{
    task::Poll,
    time::{Duration, Instant},
};
pub(super) struct MessageRequest<T: ClientTransport> {
    transport: T,
    machine: Client,
    started: Instant,
    closing: Option<(Instant, String)>,
    cleanup_error: Option<String>,
    done: bool,
}
impl<T: ClientTransport> MessageRequest<T> {
    pub(super) fn new_at(transport: T, request: Request, started: Instant) -> Result<Self, String> {
        let machine = Client::new(request, Duration::ZERO).map_err(|e| e.to_string())?;
        Ok(Self {
            transport,
            machine,
            started,
            closing: None,
            cleanup_error: None,
            done: false,
        })
    }
    pub(super) fn poll_at(&mut self, now: Instant) -> Result<Option<Reply>, String> {
        if self.done {
            return Ok(None);
        }
        if let Some((until, primary)) = &self.closing {
            if now >= *until {
                self.done = true;
                return Err(format!(
                    "{primary}; cleanup unconfirmed at absolute deadline; first_cleanup_error={:?}",
                    self.cleanup_error
                ));
            }
            match self.transport.close() {
                Poll::Pending => Ok(None),
                Poll::Ready(Ok(())) => {
                    self.done = true;
                    Err(format!(
                        "{primary}; first_cleanup_error={:?}",
                        self.cleanup_error
                    ))
                }
                Poll::Ready(Err(error)) => {
                    self.cleanup_error.get_or_insert_with(|| error.to_string());
                    Ok(None)
                }
            }
        } else {
            let result = now
                .checked_duration_since(self.started)
                .ok_or_else(|| "GUI control clock moved before transaction start".to_owned())
                .and_then(|elapsed| {
                    self.machine
                        .poll(&mut self.transport, elapsed)
                        .map_err(|e| e.to_string())
                });
            match result {
                Ok(ClientStep::Pending) => Ok(None),
                Ok(ClientStep::Finished {
                    reason: EndReason::Completed,
                    reply: Some(reply),
                }) => {
                    self.done = true;
                    Ok(Some(reply))
                }
                Ok(ClientStep::Finished { reason, .. }) => {
                    self.done = true;
                    Err(format!("Host request ended: {reason:?}"))
                }
                Err(error) => {
                    self.closing = Some((
                        (self.started + Duration::from_millis(1100))
                            .min(now + Duration::from_millis(100)),
                        error,
                    ));
                    Ok(None)
                }
            }
        }
    }
}
#[cfg(test)]
mod tests;
