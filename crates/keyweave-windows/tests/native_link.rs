//! Link the production FFI module, rather than the unit-test Win32 replacements.
//! Merely retain function addresses; never call Hook installation or SendInput.
#[cfg(target_os = "windows")]
#[test]
fn production_native_symbols_link_without_installation_or_injection() {
    use keyweave_windows::{
        HookSession, SendInputBackend, hook::HookFault, keys::InjectionTag, pump::SendBackend,
        reactor::WaitPlan,
    };
    type Control = fn() -> Result<WaitPlan, HookFault>;
    let addresses = [
        keyweave_windows::ControlServerInstance::current as *const () as usize,
        keyweave_windows::ControlClientTransport::new as *const () as usize,
        keyweave_windows::ControlClientTransport::close_resources as *const () as usize,
        <keyweave_windows::ControlClientTransport as keyweave_windows::session_control::ClientTransport>::connect as *const () as usize,
        <keyweave_windows::ControlClientTransport as keyweave_windows::session_control::ClientTransport>::read as *const () as usize,
        keyweave_windows::ControlServerTransport::listen as *const () as usize,
        keyweave_windows::ControlServerTransport::close as *const () as usize,
        <keyweave_windows::ControlServerTransport as keyweave_windows::session_control::ServerTransport>::authorize as *const () as usize,
        keyweave_windows::ControlMessagePipe::listen as *const () as usize,
        keyweave_windows::ControlMessagePipe::open as *const () as usize,
        keyweave_windows::ControlMessagePipe::poll_accept as *const () as usize,
        keyweave_windows::ControlMessagePipe::poll_read as *const () as usize,
        keyweave_windows::ControlMessagePipe::poll_write as *const () as usize,
        keyweave_windows::ControlMessagePipe::poll_disconnect as *const () as usize,
        keyweave_windows::ControlMessagePipe::close as *const () as usize,
        keyweave_windows::ControlIdentity::capture as *const () as usize,
        keyweave_windows::ControlIdentity::same_primary_token_scope_of as *const () as usize,
        keyweave_windows::ControlIdentity::security_descriptor as *const () as usize,
        keyweave_windows::ControlSecurity::close as *const () as usize,
        keyweave_windows::ConsoleControl::install as *const () as usize,
        keyweave_windows::ConsoleControl::close as *const () as usize,
        keyweave_windows::RunIdentity::generate as *const () as usize,
        keyweave_windows::runtime::preflight_config as *const () as usize,
        keyweave_windows::startup::ensure_preparation_context as *const () as usize,
        keyweave_windows::ConfigArtifact::create as *const () as usize,
        keyweave_windows::ConfigArtifact::prepare_reader as *const () as usize,
        keyweave_windows::ConfigArtifact::close as *const () as usize,
        keyweave_windows::WorkerStartup::claim_config_reader as *const () as usize,
        keyweave_windows::WorkerParentGuard::claim_config_reader as *const () as usize,
        keyweave_windows::OwnedJobProcess::spawn_suspended_with_config as *const () as usize,
        keyweave_windows::OwnedJobProcess::close_config as *const () as usize,
        keyweave_windows::OwnedJobProcess::close_config_preparation_reader as *const () as usize,
        keyweave_windows::ConfigReader::close as *const () as usize,
        <keyweave_windows::ConfigReader as std::io::Read>::read as *const () as usize,
        keyweave_windows::Worker::<SendInputBackend>::install as *const () as usize,
        keyweave_windows::Worker::<SendInputBackend>::step as *const () as usize,
        keyweave_windows::Worker::<SendInputBackend>::run as *const () as usize,
        keyweave_windows::Worker::<SendInputBackend>::request_stop as *const () as usize,
        keyweave_windows::Supervisor::spawn_with_config as *const () as usize,
        keyweave_windows::Supervisor::close_resources as *const () as usize,
        keyweave_windows::SupervisorConfigResources::close as *const () as usize,
        keyweave_windows::Supervisor::spawn as *const () as usize,
        keyweave_windows::Supervisor::poll as *const () as usize,
        keyweave_windows::Supervisor::request_stop as *const () as usize,
        keyweave_windows::Supervisor::request_shutdown as *const () as usize,
        keyweave_windows::Supervisor::close_channels as *const () as usize,
        keyweave_windows::WorkerStartup::claim as *const () as usize,
        keyweave_windows::check_synchronous_progress_pipe_rejected as *const () as usize,
        keyweave_windows::PipeReader::pair as *const () as usize,
        keyweave_windows::PipeReader::close as *const () as usize,
        keyweave_windows::PipeWriter::close as *const () as usize,
        <keyweave_windows::PipeReader as std::io::Read>::read as *const () as usize,
        <keyweave_windows::PipeWriter as std::io::Write>::write as *const () as usize,
        keyweave_windows::OwnedJobProcess::spawn_suspended as *const () as usize,
        keyweave_windows::OwnedJobProcess::spawn_suspended_with_progress as *const () as usize,
        keyweave_windows::OwnedJobProcess::finish_channel_transfer as *const () as usize,
        keyweave_windows::OwnedJobProcess::resume as *const () as usize,
        keyweave_windows::OwnedJobProcess::request_stop as *const () as usize,
        keyweave_windows::OwnedJobProcess::poll_exit as *const () as usize,
        HookSession::install as *const () as usize,
        HookSession::close as *const () as usize,
        HookSession::held_keys as *const () as usize,
        keyweave_windows::CaptureSession::<SendInputBackend>::install as *const () as usize,
        keyweave_windows::CaptureSession::<SendInputBackend>::poll as *const () as usize,
        keyweave_windows::CaptureSession::<SendInputBackend>::shutdown as *const () as usize,
        keyweave_windows::progress::ProgressClient::<std::io::Empty, std::io::Sink>::receive::<
            keyweave_windows::CaptureSession<SendInputBackend>,
        > as *const () as usize,
        keyweave_windows::progress::ProgressClient::<std::io::Empty, std::io::Sink>::publish::<
            keyweave_windows::CaptureSession<SendInputBackend>,
        > as *const () as usize,
        keyweave_windows::progress::ProgressClient::<std::io::Empty, std::io::Sink>::receive_command::<
            keyweave_windows::CaptureSession<SendInputBackend>,
        > as *const () as usize,
        keyweave_windows::progress::ProgressClient::<std::io::Empty, std::io::Sink>::receive::<
            keyweave_windows::MixedCaptureSession<SendInputBackend>,
        > as *const () as usize,
        keyweave_windows::progress::ProgressClient::<std::io::Empty, std::io::Sink>::publish::<
            keyweave_windows::MixedCaptureSession<SendInputBackend>,
        > as *const () as usize,
        keyweave_windows::progress::ProgressClient::<std::io::Empty, std::io::Sink>::receive_command::<
            keyweave_windows::MixedCaptureSession<SendInputBackend>,
        > as *const () as usize,
        keyweave_windows::MixedWorker::<SendInputBackend>::install as *const () as usize,
        keyweave_windows::MixedWorker::<SendInputBackend>::step as *const () as usize,
        keyweave_windows::MixedWorker::<SendInputBackend>::run as *const () as usize,
        keyweave_windows::MixedWorker::<SendInputBackend>::request_stop as *const () as usize,
        keyweave_windows::MixedCaptureSession::<SendInputBackend>::install as *const () as usize,
        keyweave_windows::MixedCaptureSession::<SendInputBackend>::poll as *const () as usize,
        keyweave_windows::MixedCaptureSession::<SendInputBackend>::shutdown as *const () as usize,
        HookSession::install_mouse_handler as *const () as usize,
        HookSession::held_mouse_buttons as *const () as usize,
        HookSession::close_with_mouse as *const () as usize,
        <SendInputBackend as keyweave_windows::mixed::SendMixedBackend>::submit_mixed as *const ()
            as usize,
        <SendInputBackend as keyweave_windows::pointer::SendPointerBackend>::submit_pointer
            as *const () as usize,
        keyweave_windows::mixed_actor::preflight_mixed_config as *const () as usize,
        HookSession::run_until_quit as *const () as usize,
        HookSession::poll_with_control::<Control> as *const () as usize,
        <SendInputBackend as SendBackend>::submit as *const () as usize,
        keyweave_windows::sample_input_state as *const () as usize,
        keyweave_windows::check_input_access as *const () as usize,
        keyweave_windows::SessionNotifications::install as *const () as usize,
        keyweave_windows::SessionNotifications::close as *const () as usize,
    ];
    std::hint::black_box(addresses);
    // Construction allocates the fixed buffer only; Drop does not inject.
    if let Ok(tag) = InjectionTag::new(1) {
        std::hint::black_box(SendInputBackend::new(tag));
    }
}
