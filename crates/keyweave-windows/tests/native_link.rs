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
        HookSession::install as *const () as usize,
        HookSession::close as *const () as usize,
        HookSession::held_keys as *const () as usize,
        keyweave_windows::CaptureSession::<SendInputBackend>::install as *const () as usize,
        keyweave_windows::CaptureSession::<SendInputBackend>::poll as *const () as usize,
        keyweave_windows::CaptureSession::<SendInputBackend>::shutdown as *const () as usize,
        HookSession::run_until_quit as *const () as usize,
        HookSession::poll_with_control::<Control> as *const () as usize,
        <SendInputBackend as SendBackend>::submit as *const () as usize,
        keyweave_windows::sample_input_state as *const () as usize,
        keyweave_windows::check_input_access as *const () as usize,
    ];
    std::hint::black_box(addresses);
    // Construction allocates the fixed buffer only; Drop does not inject.
    if let Ok(tag) = InjectionTag::new(1) {
        std::hint::black_box(SendInputBackend::new(tag));
    }
}
