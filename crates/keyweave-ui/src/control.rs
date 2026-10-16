//! Explicit status/stop clients. GUI never owns, starts or captures a worker.
use eframe::egui;
#[cfg(any(target_os = "linux", windows))]
use keyweave_progress::session_control::Request;
use keyweave_progress::session_control::{Command, Outcome, Phase, Reply};
use std::time::{Duration, Instant};
#[cfg(any(windows, test))]
mod message_request;
struct Observation {
    endpoint: String,
    reply: Reply,
    at: Instant,
}
#[cfg(target_os = "linux")]
type PlatformClient = keyweave_linux::session_control::Client;
#[cfg(windows)]
type PlatformClient = message_request::MessageRequest<keyweave_windows::ControlClientTransport>;
#[cfg(any(target_os = "linux", windows))]
struct Pending {
    endpoint: String,
    display: bool,
    client: PlatformClient,
}
#[cfg(any(target_os = "linux", windows))]
impl Pending {
    fn poll(&mut self) -> Result<Option<Reply>, String> {
        #[cfg(target_os = "linux")]
        {
            self.client.poll().map_err(|e| e.to_string())
        }
        #[cfg(windows)]
        {
            self.client.poll_at(Instant::now())
        }
    }
}
#[derive(Default)]
pub struct ControlPane {
    pub path: String,
    notice: String,
    last: Option<Observation>,
    confirm: Option<(String, u64)>,
    follow: Option<String>,
    available: bool,
    #[cfg(any(target_os = "linux", windows, test))]
    next: Option<Instant>,
    #[cfg(any(target_os = "linux", windows))]
    id: u64,
    #[cfg(any(target_os = "linux", windows))]
    pending: Option<Pending>,
}
impl ControlPane {
    pub fn snapshot(&self) -> Option<keyweave_progress::session_control::Snapshot> {
        self.last.as_ref().map(|o| o.reply.snapshot)
    }
    pub fn can_stop(&self) -> bool {
        self.available
            && self.last.as_ref().is_some_and(|o| {
                o.endpoint == self.path
                    && o.at.elapsed() < Duration::from_secs(2)
                    && !matches!(o.reply.snapshot.phase, Phase::Exited | Phase::Unobserved)
            })
    }
    #[cfg(any(target_os = "linux", windows, test))]
    fn observe(&mut self, endpoint: String, reply: Reply) {
        self.available = true;
        self.notice=match reply.outcome {
            Outcome::Status=>"Last host observation; capture readiness is not inferred".into(),
            Outcome::StopAccepted=>"Stop request accepted; wait for observed process exit and independent recovery facts".into(),
            Outcome::StaleSession=>"Session changed; previous Stop was not applied".into(),
        };
        if matches!(reply.snapshot.phase, Phase::Exited | Phase::Unobserved) {
            self.follow = None;
        }
        self.last = Some(Observation {
            endpoint,
            reply,
            at: Instant::now(),
        });
        self.next = Some(Instant::now() + Duration::from_millis(200));
    }
    /// Observation disconnect never promises cancellation of a submitted Stop.
    pub fn disconnect(&mut self) {
        self.available = false;
        self.follow = None;
        self.confirm = None;
        #[cfg(any(target_os = "linux", windows))]
        if let Some(pending) = &mut self.pending {
            pending.display = false;
        }
    }
    #[cfg(any(target_os = "linux", windows, test))]
    fn deliver(&mut self, endpoint: String, display: bool, result: Result<Reply, String>) {
        if !display || endpoint != self.path {
            return;
        }
        match result {
            Ok(reply) => self.observe(endpoint, reply),
            Err(error) => {
                self.disconnect();
                self.notice = format!(
                    "Host unavailable; submitted requests may have taken effect; exit/recovery unconfirmed: {error}"
                );
            }
        }
    }
    #[cfg(any(target_os = "linux", windows))]
    fn begin(&mut self, command: Command) -> Result<(), String> {
        if self.pending.is_some() {
            return Err("A host request is already pending".into());
        }
        self.id = self
            .id
            .checked_add(1)
            .ok_or("Host request ID exhausted; reopen settings")?;
        let request = Request {
            id: self.id,
            command,
        };
        #[cfg(target_os = "linux")]
        let client = keyweave_linux::session_control::Client::connect(
            std::path::Path::new(&self.path),
            request,
            Duration::from_secs(2),
        )
        .map_err(|e| e.to_string())?;
        #[cfg(windows)]
        let client = {
            let started = Instant::now();
            let endpoint = self
                .path
                .parse::<keyweave_progress::session_control::WindowsEndpoint>()
                .map_err(str::to_owned)?;
            let instance = keyweave_windows::ControlServerInstance::from_parts(
                endpoint.process_id(),
                endpoint.creation_time(),
            )
            .map_err(|e| e.to_string())?;
            let transport =
                keyweave_windows::ControlClientTransport::new(endpoint.pipe(), instance)
                    .map_err(|e| e.to_string())?;
            message_request::MessageRequest::new_at(transport, request, started)?
        };
        self.pending = Some(Pending {
            endpoint: self.path.clone(),
            display: true,
            client,
        });
        Ok(())
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    fn begin(&mut self, _: Command) -> Result<(), String> {
        Err("Local session control is available on Windows and Linux".into())
    }
    pub fn query(&mut self) -> Result<(), String> {
        if let Err(error) = self.begin(Command::Status) {
            self.disconnect();
            return Err(error);
        }
        self.follow = Some(self.path.clone());
        Ok(())
    }
    pub fn confirm_stop(&mut self) -> Result<(), String> {
        let (endpoint, session) = self.confirm.as_ref().ok_or("Select Request stop first")?;
        if !self.can_stop()
            || endpoint != &self.path
            || self.snapshot().map(|s| s.session) != Some(*session)
        {
            return Err(
                "Endpoint/session changed or observation expired; refresh status before stopping"
                    .into(),
            );
        }
        if let Err(error) = self.begin(Command::Stop { session: *session }) {
            self.disconnect();
            return Err(error);
        }
        self.confirm = None;
        Ok(())
    }
    pub fn select_stop(&mut self) -> Result<(), String> {
        if !self.can_stop() {
            return Err("Query a current same-user host before requesting Stop".into());
        }
        if let Some(snapshot) = self.snapshot() {
            self.confirm = Some((self.path.clone(), snapshot.session));
        }
        Ok(())
    }
    pub fn poll(&mut self) {
        if self
            .follow
            .as_ref()
            .is_some_and(|endpoint| endpoint != &self.path)
        {
            self.disconnect();
        }
        #[cfg(any(target_os = "linux", windows))]
        {
            if self
                .pending
                .as_ref()
                .is_some_and(|p| p.endpoint != self.path)
            {
                self.disconnect();
            }
            let result = self
                .pending
                .as_mut()
                .and_then(|pending| match pending.poll() {
                    Ok(None) => None,
                    Ok(Some(reply)) => Some((pending.endpoint.clone(), pending.display, Ok(reply))),
                    Err(error) => Some((pending.endpoint.clone(), pending.display, Err(error))),
                });
            if let Some((endpoint, display, result)) = result {
                self.pending = None;
                self.deliver(endpoint, display, result);
            }
            if self.pending.is_none()
                && self.follow.as_ref() == Some(&self.path)
                && self.next.is_some_and(|at| Instant::now() >= at)
                && let Err(error) = self.begin(Command::Status)
            {
                self.disconnect();
                self.notice = format!("Host unavailable; exit/recovery unconfirmed: {error}");
            }
        }
    }
    pub fn contents(&mut self, ui: &mut egui::Ui) {
        self.poll();
        ui.collapsing("Local session status and stop",|ui| {
            ui.label("Connect to an explicitly started host. Closing Settings keeps that host independent.");
            ui.horizontal(|ui| {
                #[cfg(target_os="linux")]
                ui.label("Linux control socket");
                #[cfg(windows)]
                ui.label("Windows host endpoint");
                #[cfg(not(any(target_os="linux",windows)))]
                ui.label("Host endpoint");
                if ui.text_edit_singleline(&mut self.path).changed() {self.disconnect();}
                if ui.button("Query host").clicked() && let Err(error)=self.query() {self.disconnect();self.notice=error;}
                if ui.button("Disconnect observer").clicked() {self.disconnect();self.notice="Observer disconnected; submitted requests may still take effect".into();}
            });
            if let Some(o)=&self.last {let s=o.reply.snapshot;
                ui.label(format!("Last observation from {} ({:.1}s ago)",o.endpoint,o.at.elapsed().as_secs_f32()));
                ui.label(format!("Public session {} · {:?} · progress {} · last observed generation {:?}",s.session,s.phase,s.progress_epoch,s.generation));
                ui.label(format!("Observed worker exit: {:?} · worker recovery confirmed: {} · host fault: {}",s.exit,s.recovery_confirmed,s.fault));
            }
            if ui.add_enabled(self.can_stop(),egui::Button::new("Request stop")).clicked() && let Err(error)=self.select_stop() {self.notice=error;}
            if let Some((_,session))=&self.confirm {let session=*session;ui.horizontal(|ui| {
                ui.label(format!("Request Stop for public session {session}?"));
                if ui.button("Confirm stop request").clicked() && let Err(error)=self.confirm_stop() {self.notice=error;}
                if ui.button("Keep session running").clicked() {self.confirm=None;}
            });}
            if !self.notice.is_empty() {ui.label(&self.notice);}
        });
        #[cfg(any(target_os = "linux", windows))]
        if self.pending.is_some() || self.follow.is_some() {
            ui.ctx().request_repaint_after(Duration::from_millis(20));
        }
    }
}
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use keyweave_progress::session_control::{Exit, Snapshot};
    fn reply(session: u64) -> Reply {
        Reply {
            id: 1,
            outcome: Outcome::Status,
            snapshot: Snapshot {
                session,
                phase: Phase::Starting,
                progress_epoch: 0,
                generation: None,
                exit: None,
                recovery_confirmed: false,
                fault: false,
            },
        }
    }
    #[test]
    fn stop_requires_fresh_exact_endpoint_and_confirmation_binds_observed_session() {
        let mut pane = ControlPane {
            path: "/private/control".into(),
            ..Default::default()
        };
        assert!(!pane.can_stop());
        assert!(pane.select_stop().is_err());
        pane.observe(pane.path.clone(), reply(1));
        assert!(pane.can_stop());
        pane.select_stop().unwrap();
        pane.observe(pane.path.clone(), reply(2));
        assert!(pane.confirm_stop().is_err());
        pane.select_stop().unwrap();
        pane.path = "/different/control".into();
        assert!(!pane.can_stop());
        assert!(pane.confirm_stop().is_err());
        pane.path = "/private/control".into();
        pane.last.as_mut().unwrap().at = Instant::now() - Duration::from_secs(3);
        assert!(!pane.can_stop());
    }
    #[test]
    fn stop_ack_disconnect_and_actual_exit_are_distinct_observations() {
        let mut pane = ControlPane {
            path: "/private/control".into(),
            ..Default::default()
        };
        let mut ack = reply(1);
        ack.outcome = Outcome::StopAccepted;
        ack.snapshot.phase = Phase::StopRequested;
        pane.observe(pane.path.clone(), ack);
        assert_eq!(pane.snapshot().unwrap().exit, None);
        assert!(!pane.snapshot().unwrap().recovery_confirmed);
        pane.disconnect();
        assert_eq!(pane.snapshot().unwrap().phase, Phase::StopRequested);
        let mut exited = reply(1);
        exited.snapshot.phase = Phase::Exited;
        exited.snapshot.exit = Some(Exit::Code(0));
        pane.observe(pane.path.clone(), exited);
        assert!(!pane.can_stop());
        assert!(!pane.snapshot().unwrap().recovery_confirmed);
    }
    #[test]
    fn disconnect_failure_or_endpoint_switch_does_not_reenable_stop_from_a_late_result() {
        let mut pane = ControlPane {
            path: "old".into(),
            ..Default::default()
        };
        pane.observe("old".into(), reply(1));
        pane.select_stop().unwrap();
        pane.disconnect();
        assert!(!pane.can_stop() && pane.confirm.is_none());
        pane.deliver("old".into(), false, Ok(reply(1)));
        assert!(!pane.can_stop());
        pane.path = "new".into();
        pane.deliver("old".into(), true, Ok(reply(2)));
        assert!(!pane.can_stop());
        pane.path = "old".into();
        assert!(!pane.can_stop());
        pane.deliver("old".into(), true, Ok(reply(2)));
        assert!(pane.can_stop());
        pane.deliver(
            "old".into(),
            true,
            Err("synthetic transport failure".into()),
        );
        assert!(!pane.can_stop());
        assert_eq!(pane.snapshot().unwrap().session, 2);
        assert!(pane.notice.contains("unconfirmed"));
    }
    #[test]
    #[cfg(target_os = "linux")]
    #[ignore = "requires an IPC-enabled environment; explicit native transport gate"]
    fn actual_gui_client_waits_for_exit_after_stop_ack_instead_of_inventing_recovery() {
        use std::os::unix::fs::DirBuilderExt;
        struct Directory(std::path::PathBuf);
        impl Drop for Directory {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let directory = std::env::temp_dir().join(format!("kw-ui-control-{}", std::process::id()));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&directory)
            .unwrap();
        // Arm cleanup only after this test created the directory successfully.
        let dir = Directory(directory);
        let path = dir.0.join("control");
        let mut server = keyweave_linux::session_control::Server::bind(&path).unwrap();
        let mut pane = ControlPane {
            path: path.to_str().unwrap().into(),
            ..Default::default()
        };
        let mut source = reply(server.session()).snapshot;
        pane.query().unwrap();
        let until = Instant::now() + Duration::from_secs(3);
        while pane.snapshot().is_none() {
            server.poll(source).unwrap();
            pane.poll();
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(1));
        }
        pane.select_stop().unwrap();
        pane.confirm_stop().unwrap();
        while pane.snapshot().unwrap().phase != Phase::StopRequested {
            server.poll(source).unwrap();
            pane.poll();
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(server.stop_requested());
        assert_eq!(pane.snapshot().unwrap().exit, None);
        assert!(!pane.snapshot().unwrap().recovery_confirmed);
        // Synthetic server DTO, not a real worker or global recovery assertion.
        source.phase = Phase::Exited;
        source.exit = Some(Exit::Code(0));
        while pane.snapshot().unwrap().phase != Phase::Exited {
            server.poll(source).unwrap();
            pane.poll();
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(!pane.can_stop());
        assert!(!pane.snapshot().unwrap().recovery_confirmed);
        server.close().unwrap();
        assert!(!path.exists());
    }
}
