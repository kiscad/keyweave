//! Windows public pipe service; no worker creation, input or active-loop stdio.
use crate::public_control::{PublicSession, WindowsEndpoint};
use keyweave_progress::session_control::{Reply, Request, Snapshot};
use keyweave_windows::session_control::{Client, ClientStep, EndReason, Server};
use keyweave_windows::{
    ControlClientTransport, ControlServerInstance, ControlServerTransport, RunIdentity,
};
use std::time::{Duration, Instant};
pub struct Control {
    endpoint: WindowsEndpoint,
    state: PublicSession,
    machine: Server,
    transport: ControlServerTransport,
    close_deadline: Option<Instant>,
    close_expired: bool,
}
impl Control {
    pub fn listen(name: &str) -> Result<Self, String> {
        let instance = ControlServerInstance::current().map_err(|e| e.to_string())?;
        let endpoint = WindowsEndpoint::new(name, instance.process_id(), instance.creation_time())
            .map_err(str::to_owned)?;
        let session = RunIdentity::generate().map_err(|e| e.to_string())?.run_id();
        let state = PublicSession::new(Snapshot {
            session,
            phase: keyweave_progress::session_control::Phase::Starting,
            progress_epoch: 0,
            generation: None,
            exit: None,
            recovery_confirmed: false,
            fault: false,
        })
        .map_err(|e| e.to_string())?;
        let transport = ControlServerTransport::listen(name).map_err(|e| e.to_string())?;
        Ok(Self {
            endpoint,
            state,
            machine: Server::new(),
            transport,
            close_deadline: None,
            close_expired: false,
        })
    }
    pub fn endpoint(&self) -> &WindowsEndpoint {
        &self.endpoint
    }
    pub fn session(&self) -> u64 {
        self.state.session()
    }
    pub fn stop_requested(&self) -> bool {
        self.state.stop_requested()
    }
    pub fn poll(&mut self, now: Duration, snapshot: Snapshot) -> Result<(), String> {
        self.state.observe(snapshot).map_err(|e| e.to_string())?;
        let state = &mut self.state;
        self.machine
            .poll(&mut self.transport, now, |request| state.handle(request))
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    pub fn close(&mut self) -> Result<(), String> {
        let until = *self
            .close_deadline
            .get_or_insert_with(|| Instant::now() + Duration::from_millis(100));
        loop {
            let report = self.transport.close();
            if report.resources_released {
                return if report.complete() && !self.close_expired {
                    Ok(())
                } else {
                    Err(format!("control cleanup: {report:?}"))
                };
            }
            if Instant::now() >= until {
                self.close_expired = true;
                return Err(format!("control cleanup timeout: {report:?}"));
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}
/// One public request. No worker spawn; error after write does not undo Stop.
pub fn request(endpoint: &WindowsEndpoint, request: Request) -> Result<Reply, String> {
    let start = Instant::now();
    let instance =
        ControlServerInstance::from_parts(endpoint.process_id(), endpoint.creation_time())
            .map_err(|e| e.to_string())?;
    let mut transport =
        ControlClientTransport::new(endpoint.pipe(), instance).map_err(|e| e.to_string())?;
    let mut machine = Client::new(request, Duration::ZERO).map_err(|e| e.to_string())?;
    let outcome = loop {
        match machine.poll(&mut transport, start.elapsed()) {
            Ok(ClientStep::Finished {
                reason: EndReason::Completed,
                reply: Some(reply),
            }) => break Ok(reply),
            Ok(ClientStep::Finished { reason, .. }) => {
                break Err(format!("control request ended: {reason:?}"));
            }
            Ok(ClientStep::Pending) => std::thread::sleep(Duration::from_millis(1)),
            Err(error) => break Err(error.to_string()),
        }
    };
    let until =
        (start + Duration::from_millis(1100)).min(Instant::now() + Duration::from_millis(100));
    let cleanup = loop {
        let report = transport.close_resources();
        if report.complete() {
            break Ok(());
        }
        if report.pipe.is_none_or(|pipe| pipe.resources_released) && report.peer_released
            || Instant::now() >= until
        {
            break Err(format!("client cleanup: {report:?}"));
        }
        std::thread::sleep(Duration::from_millis(1));
    };
    match (outcome, cleanup) {
        (Ok(reply), Ok(())) => Ok(reply),
        (Err(error), Ok(())) => Err(error),
        (result, Err(cleanup)) => Err(format!("request={result:?}; {cleanup}")),
    }
}
