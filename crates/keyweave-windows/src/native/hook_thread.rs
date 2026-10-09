use super::{KeyboardHook, Message};
use crate::{
    hook::{HookDispatcher, HookFault, HookHandler},
    keys::{HookRecord, IdentityPolicy, InjectionTag, Origin},
    reactor::WaitPlan,
};
use keyweave_types::CaptureDecision;
use std::{
    cell::{Cell, RefCell},
    ffi::c_void,
    marker::PhantomData,
    ptr::null_mut,
    rc::Rc,
};
type Handle = *mut c_void;
type Callback = unsafe extern "system" fn(i32, usize, isize) -> isize;
thread_local! {
    static DISPATCHER: RefCell<Option<HookDispatcher<Box<dyn HookHandler>>>> = const { RefCell::new(None) };
    static TAG: Cell<usize> = const { Cell::new(0) };
    static FAULT: Cell<Option<HookFault>> = const { Cell::new(None) };
    static POISONED: Cell<bool> = const { Cell::new(false) };
}
#[cfg(not(test))]
mod api {
    use super::*;
    #[link(name = "user32")]
    unsafe extern "system" {
        pub(super) fn SetWindowsHookExW(
            kind: i32,
            callback: Callback,
            module: Handle,
            thread: u32,
        ) -> Handle;
        pub(super) fn UnhookWindowsHookEx(hook: Handle) -> i32;
        pub(super) fn CallNextHookEx(hook: Handle, code: i32, message: usize, data: isize)
        -> isize;
        pub(super) fn GetMessageW(message: *mut Message, window: Handle, min: u32, max: u32)
        -> i32;
        pub(super) fn TranslateMessage(message: *const Message) -> i32;
        pub(super) fn DispatchMessageW(message: *const Message) -> isize;
        pub(super) fn PostQuitMessage(code: i32);
        pub(super) fn PeekMessageW(
            message: *mut Message,
            window: Handle,
            min: u32,
            max: u32,
            remove: u32,
        ) -> i32;
        pub(super) fn MsgWaitForMultipleObjectsEx(
            count: u32,
            handles: *const Handle,
            timeout: u32,
            mask: u32,
            flags: u32,
        ) -> u32;
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        pub(super) fn GetModuleHandleW(name: *const u16) -> Handle;
        pub(super) fn GetLastError() -> u32;
    }
}
use api::*;
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum HookSessionError {
    #[error("thread already owns a hook or cannot be reused")]
    Busy,
    #[error("Windows hook installation failed: {0}")]
    Install(u32),
    #[error("Windows message loop failed: {0}")]
    Message(u32),
    #[error("unexpected message wait result: {0}")]
    InvalidWait(u32),
    #[error("hook callback stopped: {0:?}")]
    Callback(HookFault),
    #[error("Windows hook removal failed: {0}")]
    Unhook(u32),
    #[error("hook session is already closed")]
    Closed,
}
/// One registration on this dedicated thread, never transferable. Installing
/// requires the caller's real session/neutral/readiness gates to have passed.
pub struct HookSession {
    handle: Handle,
    _thread: PhantomData<Rc<()>>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoopStep {
    Continue,
    Quit,
}
impl HookSession {
    /// One bounded cycle; control advances deadlines with a fresh clock before
    /// sent callbacks and again before waiting. Host handles Quit by unhooking
    /// and performing explicit actor recovery. No borrow is held during Win32.
    pub fn poll_with_control<F: FnMut() -> Result<WaitPlan, HookFault>>(
        &mut self,
        control: &mut F,
    ) -> Result<LoopStep, HookSessionError> {
        if self.handle.is_null() {
            return Err(HookSessionError::Closed);
        }
        if let Some(fault) = self.fault() {
            return Err(HookSessionError::Callback(fault));
        }
        if TAG.with(Cell::get) == 0 {
            return Err(HookSessionError::Closed);
        }
        checked_control(control)?;
        for _ in 0..64 {
            let mut message = Message::default();
            // SAFETY: initialized writable repr(C) MSG, current thread/null HWND,
            // zero filters, PM_REMOVE=1. Sent callbacks run synchronously here;
            // control returned and holds no actor or dispatcher mutable borrow.
            let present = unsafe { PeekMessageW(&mut message, null_mut(), 0, 0, 1) };
            if let Some(fault) = self.fault() {
                return Err(HookSessionError::Callback(fault));
            }
            if present == 0 {
                let plan = checked_control(control)?;
                // SAFETY: zero handles means message-only wait; null handles is
                // never read. timeout is 0..=20ms, QS_ALLINPUT=0x04ff and
                // MWMO_INPUTAVAILABLE=4 include seen-but-unremoved input. No
                // alertable APCs or owned handle copies/lifetime races are used.
                let result = unsafe {
                    MsgWaitForMultipleObjectsEx(0, std::ptr::null(), plan.timeout_ms(), 0x04ff, 4)
                };
                if result == u32::MAX {
                    // SAFETY: immediate thread-local error retrieval.
                    let error = unsafe { GetLastError() };
                    stop(HookFault::Handler);
                    return Err(HookSessionError::Message(error));
                }
                if let Some(fault) = self.fault() {
                    return Err(HookSessionError::Callback(fault));
                }
                if !matches!(result, 0 | 258) {
                    stop(HookFault::Handler);
                    return Err(HookSessionError::InvalidWait(result));
                }
                return Ok(LoopStep::Continue);
            }
            if message.message == 0x12 {
                TAG.with(|tag| tag.set(0));
                return Ok(LoopStep::Quit);
            }
            // SAFETY: PeekMessage initialized this thread's MSG; synchronous
            // borrowed pointer, no callback/user state borrow spans dispatch.
            unsafe {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
            if let Some(fault) = self.fault() {
                return Err(HookSessionError::Callback(fault));
            }
        }
        // A busy queue cannot suppress deadlines indefinitely or wait 20ms while
        // messages remain. Outer loop/control gets another bounded turn now.
        Ok(LoopStep::Continue)
    }
    pub fn fault(&self) -> Option<HookFault> {
        FAULT.with(Cell::get)
    }
    /// On failure, return ownership of the handler without losing its state.
    pub fn install(
        handler: Box<dyn HookHandler>,
        tag: InjectionTag,
        policy: IdentityPolicy,
    ) -> Result<Self, (HookSessionError, Box<dyn HookHandler>)> {
        if POISONED.with(Cell::get) || DISPATCHER.with(|slot| slot.borrow().is_some()) {
            return Err((HookSessionError::Busy, handler));
        }
        // SAFETY: null requests the current executable's borrowed module handle;
        // callback code is static in that module and remains loaded throughout.
        let module = unsafe { GetModuleHandleW(std::ptr::null()) };
        if module.is_null() {
            // SAFETY: GetLastError has no pointers and is thread-local.
            return Err((
                HookSessionError::Install(unsafe { GetLastError() }),
                handler,
            ));
        }
        FAULT.with(|fault| fault.set(None));
        DISPATCHER
            .with(|slot| *slot.borrow_mut() = Some(HookDispatcher::new(handler, tag, policy)));
        TAG.with(|value| value.set(tag.value()));
        // SAFETY: WH_KEYBOARD_LL=13 uses the installing thread, no injected DLL.
        // callback has the exact system ABI and is static. TLS state was installed
        // first; no stack/raw handler pointer escapes. Thread=0 selects desktop.
        let handle = unsafe { SetWindowsHookExW(13, callback, module, 0) };
        if handle.is_null() {
            // SAFETY: obtain the thread-local error immediately after failure.
            let error = unsafe { GetLastError() };
            TAG.with(|value| value.set(0));
            // This slot was just installed and no user callback can remove it.
            let dispatcher = DISPATCHER.with(|slot| slot.borrow_mut().take());
            if let Some(dispatcher) = dispatcher {
                return Err((HookSessionError::Install(error), dispatcher.into_handler()));
            }
            unreachable!("hook installation owns its TLS slot");
        }
        Ok(Self {
            handle,
            _thread: PhantomData,
        })
    }
    /// Explicit blocking message loop. No deadlines/worker watchdog are supplied
    /// by this primitive; the runtime coordinator must add them before deployment.
    pub fn run_until_quit(&mut self) -> Result<(), HookSessionError> {
        if self.handle.is_null() {
            return Err(HookSessionError::Closed);
        }
        loop {
            let mut message = Message::default();
            // SAFETY: unique initialized MSG output with correct repr(C) layout;
            // null HWND and zero filters consume this thread's complete queue.
            let result = unsafe { GetMessageW(&mut message, null_mut(), 0, 0) };
            if result == -1 {
                // SAFETY: immediate thread-local error retrieval, no pointers.
                let error = unsafe { GetLastError() };
                TAG.with(|tag| tag.set(0));
                POISONED.with(|poisoned| poisoned.set(true));
                return Err(HookSessionError::Message(error));
            }
            if let Some(fault) = FAULT.with(Cell::get) {
                return Err(HookSessionError::Callback(fault));
            }
            if result == 0 {
                TAG.with(|tag| tag.set(0));
                return Ok(());
            }
            // SAFETY: GetMessage returned a live initialized MSG on this thread;
            // the functions only borrow it synchronously. Handler is not borrowed
            // here, so sent-message callbacks can access TLS without aliasing.
            unsafe {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
    }
    /// Disable first, attempt unhook, then transfer handler for explicit output
    /// recovery. Removal failure does not lose the handler or pretend key release.
    pub fn close(&mut self) -> (Option<Box<dyn HookHandler>>, Result<(), HookSessionError>) {
        if self.handle.is_null() {
            return (None, Ok(()));
        }
        TAG.with(|tag| tag.set(0));
        let handle = std::mem::replace(&mut self.handle, null_mut());
        // SAFETY: unique HHOOK returned by this registration, same !Send thread;
        // callback was disabled first and owns no pointer into session/handler.
        let removed = unsafe { UnhookWindowsHookEx(handle) };
        let result = if removed == 0 {
            // SAFETY: immediate thread-local error retrieval.
            let error = unsafe { GetLastError() };
            POISONED.with(|poisoned| poisoned.set(true));
            Err(HookSessionError::Unhook(error))
        } else {
            Ok(())
        };
        let handler = DISPATCHER
            .with(|slot| slot.borrow_mut().take())
            .map(HookDispatcher::into_handler);
        (handler, result)
    }
}
fn checked_control<F: FnMut() -> Result<WaitPlan, HookFault>>(
    control: &mut F,
) -> Result<WaitPlan, HookSessionError> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(control)) {
        Ok(Ok(plan)) => Ok(plan),
        Ok(Err(fault)) => {
            stop(fault);
            Err(HookSessionError::Callback(fault))
        }
        Err(payload) => {
            std::mem::forget(payload);
            stop(HookFault::Panic);
            Err(HookSessionError::Callback(HookFault::Panic))
        }
    }
}
impl Drop for HookSession {
    fn drop(&mut self) {
        let _ = self.close();
    }
}
fn stop(fault: HookFault) {
    let _ = TAG.try_with(|tag| tag.set(0));
    let _ = POISONED.try_with(|poisoned| poisoned.set(true));
    let _ = FAULT.try_with(|value| {
        if value.get().is_none() {
            value.set(Some(fault));
        }
    });
    // SAFETY: posts WM_QUIT to the current installing thread, with no pointers.
    unsafe {
        PostQuitMessage(0);
    }
}
fn forward(code: i32, message: usize, data: isize) -> isize {
    // SAFETY: forward the OS callback arguments unchanged; HHOOK is ignored by
    // CallNextHookEx. No dereference of lParam, including for nCode<0.
    unsafe { CallNextHookEx(null_mut(), code, message, data) }
}
unsafe extern "system" fn callback(code: i32, message: usize, data: isize) -> isize {
    if code != 0 {
        return forward(code, message, data);
    }
    let tag = TAG
        .try_with(Cell::get)
        .ok()
        .and_then(|tag| InjectionTag::new(tag).ok());
    let Some(tag) = tag else {
        return forward(code, message, data);
    };
    if data == 0 || u32::try_from(message).is_err() {
        stop(HookFault::Record);
        return forward(code, message, data);
    }
    // SAFETY: HC_ACTION's non-null lParam is an OS-owned KBDLLHOOKSTRUCT valid
    // for this call. Copy its fields only; no reference/pointer is retained.
    let keyboard = unsafe { *(data as *const KeyboardHook) };
    let record = HookRecord {
        message: message as u32,
        vk_code: keyboard.vk,
        scan_code: keyboard.scan,
        flags: keyboard.flags,
        extra_info: keyboard.extra_info,
    };
    // Must precede mutable borrow: SendInput can synchronously reenter this Hook.
    if record.origin(tag) != Origin::Physical {
        return forward(code, message, data);
    }
    let result = DISPATCHER.try_with(|slot| {
        let Ok(mut slot) = slot.try_borrow_mut() else {
            return Err(HookFault::Reentrant);
        };
        let Some(dispatcher) = slot.as_mut() else {
            return Ok(CaptureDecision::Pass);
        };
        let decision = dispatcher.dispatch(code, Some(record));
        dispatcher.fault().map_or(Ok(decision), Err)
    });
    match result {
        Ok(Ok(CaptureDecision::Capture)) if TAG.try_with(Cell::get).unwrap_or(0) != 0 => 1,
        Ok(Ok(_)) => forward(code, message, data),
        Ok(Err(fault)) => {
            stop(fault);
            forward(code, message, data)
        }
        Err(_) => forward(code, message, data),
    }
}

#[cfg(test)]
#[allow(non_snake_case)]
mod api {
    use super::*;
    thread_local! { pub(super) static UNHOOK_FAIL: Cell<bool> = const { Cell::new(false) }; pub(super) static INSTALL_FAIL: Cell<bool> = const { Cell::new(false) }; pub(super) static LOOP_RESULT: Cell<i32> = const { Cell::new(0) }; }
    thread_local! {
        pub(super) static MESSAGES: RefCell<std::collections::VecDeque<u32>> = const { RefCell::new(std::collections::VecDeque::new()) };
        pub(super) static WAIT_RESULT: Cell<u32> = const { Cell::new(258) };
        pub(super) static WAIT_CALLS: RefCell<Vec<(u32,u32,u32,u32)>> = const { RefCell::new(Vec::new()) };
    }
    pub(super) unsafe fn PeekMessageW(
        message: *mut Message,
        _: Handle,
        _: u32,
        _: u32,
        _: u32,
    ) -> i32 {
        if let Some(code) = MESSAGES.with(|queue| queue.borrow_mut().pop_front()) {
            // SAFETY: the tested call supplied its live initialized MSG output.
            unsafe {
                (*message).message = code;
            }
            1
        } else {
            0
        }
    }
    pub(super) unsafe fn MsgWaitForMultipleObjectsEx(
        count: u32,
        handles: *const Handle,
        timeout: u32,
        mask: u32,
        flags: u32,
    ) -> u32 {
        assert!(handles.is_null());
        WAIT_CALLS.with(|calls| calls.borrow_mut().push((count, timeout, mask, flags)));
        WAIT_RESULT.with(Cell::get)
    }
    pub(super) unsafe fn GetModuleHandleW(_: *const u16) -> Handle {
        1usize as Handle
    }
    pub(super) unsafe fn SetWindowsHookExW(_: i32, _: Callback, _: Handle, _: u32) -> Handle {
        if INSTALL_FAIL.with(Cell::get) {
            null_mut()
        } else {
            2usize as Handle
        }
    }
    pub(super) unsafe fn GetLastError() -> u32 {
        5
    }
    pub(super) unsafe fn UnhookWindowsHookEx(_: Handle) -> i32 {
        i32::from(!UNHOOK_FAIL.with(Cell::get))
    }
    pub(super) unsafe fn CallNextHookEx(_: Handle, _: i32, _: usize, _: isize) -> isize {
        9
    }
    pub(super) unsafe fn GetMessageW(_: *mut Message, _: Handle, _: u32, _: u32) -> i32 {
        LOOP_RESULT.with(Cell::get)
    }
    pub(super) unsafe fn TranslateMessage(_: *const Message) -> i32 {
        0
    }
    pub(super) unsafe fn DispatchMessageW(_: *const Message) -> isize {
        0
    }
    pub(super) unsafe fn PostQuitMessage(_: i32) {}
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use crate::hook::HookEvent;
    struct Handler {
        nested: Option<bool>,
        seen: Rc<Cell<usize>>,
    }
    impl HookHandler for Handler {
        fn handle(&mut self, _: HookEvent) -> Result<CaptureDecision, HookFault> {
            self.seen.set(self.seen.get() + 1);
            if let Some(injected) = self.nested.take() {
                let keyboard = KeyboardHook {
                    vk: 0x41,
                    scan: 0x30,
                    flags: if injected { 0x10 } else { 0 },
                    time: 0,
                    extra_info: 1,
                };
                // SAFETY: test-owned live KBDLLHOOKSTRUCT supplied for HC_ACTION.
                assert_eq!(
                    unsafe { callback(0, 0x100, &keyboard as *const _ as isize) },
                    9
                );
            }
            Ok(CaptureDecision::Capture)
        }
    }
    fn install(nested: Option<bool>) -> (HookSession, Rc<Cell<usize>>) {
        let seen = Rc::new(Cell::new(0));
        let session = HookSession::install(
            Box::new(Handler {
                nested,
                seen: seen.clone(),
            }),
            InjectionTag::new(1).unwrap(),
            IdentityPolicy::CanonicalScan,
        )
        .unwrap_or_else(|(error, _)| panic!("{error}"));
        (session, seen)
    }
    fn send(scan: u32, up: bool) -> isize {
        let keyboard = KeyboardHook {
            vk: 0x41,
            scan,
            flags: if up { 0x80 } else { 0 },
            time: 0,
            extra_info: 0,
        };
        // SAFETY: test-owned initialized record remains live through callback.
        unsafe {
            callback(
                0,
                if up { 0x101 } else { 0x100 },
                &keyboard as *const _ as isize,
            )
        }
    }
    #[test]
    fn native_callback_chains_nonaction_and_owns_one_registration() {
        let (mut session, seen) = install(None);
        // SAFETY: non-action callback must never dereference arbitrary data.
        assert_eq!(unsafe { callback(-1, 0, 1) }, 9);
        assert_eq!(send(0x1e, false), 1);
        assert_eq!(send(0x1e, true), 1);
        assert_eq!(send(0x1e, true), 9);
        assert_eq!(seen.get(), 2);
        assert!(session.fault().is_none());
        let rejected = HookSession::install(
            Box::new(Handler { nested: None, seen }),
            InjectionTag::new(1).unwrap(),
            IdentityPolicy::CanonicalScan,
        );
        assert!(matches!(rejected, Err((HookSessionError::Busy, _))));
        session.run_until_quit().unwrap();
        let (handler, result) = session.close();
        result.unwrap();
        assert!(handler.is_some());
        assert!(session.close().0.is_none());
        assert_eq!(session.run_until_quit(), Err(HookSessionError::Closed));
        assert_eq!(send(0x1e, false), 9);
    }
    #[test]
    fn own_injection_reentrancy_bypasses_before_mutable_dispatcher_borrow() {
        let (mut session, seen) = install(Some(true));
        assert_eq!(send(0x1e, false), 1);
        assert_eq!(seen.get(), 1);
        assert_eq!(FAULT.with(Cell::get), None);
        session.close().1.unwrap();
    }
    #[test]
    fn physical_reentrancy_disables_outer_capture_and_poisoned_thread_reuse() {
        let (mut session, seen) = install(Some(false));
        assert_eq!(send(0x1e, false), 9);
        assert_eq!(seen.get(), 1);
        assert_eq!(
            session.run_until_quit(),
            Err(HookSessionError::Callback(HookFault::Reentrant))
        );
        let (handler, result) = session.close();
        result.unwrap();
        assert!(handler.is_some());
        let (_, seen) = (handler, seen);
        assert!(matches!(
            HookSession::install(
                Box::new(Handler { nested: None, seen }),
                InjectionTag::new(1).unwrap(),
                IdentityPolicy::CanonicalScan
            ),
            Err((HookSessionError::Busy, _))
        ));
    }
    #[test]
    fn api_failures_return_handler_and_do_not_allow_reuse_after_unhook_failure() {
        let seen = Rc::new(Cell::new(0));
        api::INSTALL_FAIL.with(|fail| fail.set(true));
        let failure = HookSession::install(
            Box::new(Handler {
                nested: None,
                seen: seen.clone(),
            }),
            InjectionTag::new(1).unwrap(),
            IdentityPolicy::CanonicalScan,
        );
        assert!(matches!(failure, Err((HookSessionError::Install(5), _))));
        api::INSTALL_FAIL.with(|fail| fail.set(false));
        let (mut session, _) = install(None);
        api::LOOP_RESULT.with(|result| result.set(-1));
        assert_eq!(session.run_until_quit(), Err(HookSessionError::Message(5)));
        api::UNHOOK_FAIL.with(|fail| fail.set(true));
        let (handler, result) = session.close();
        assert_eq!(result, Err(HookSessionError::Unhook(5)));
        assert!(handler.is_some());
        assert_eq!(send(0x1e, false), 9);
        assert!(matches!(
            HookSession::install(
                Box::new(Handler { nested: None, seen }),
                InjectionTag::new(1).unwrap(),
                IdentityPolicy::CanonicalScan
            ),
            Err((HookSessionError::Busy, _))
        ));
    }
    #[test]
    fn bounded_message_batch_yields_and_fresh_control_sets_next_wait() {
        let (mut session, _) = install(None);
        api::MESSAGES.with(|messages| messages.borrow_mut().extend(std::iter::repeat_n(0x400, 65)));
        let mut calls = 0;
        let mut control = || {
            calls += 1;
            Ok(WaitPlan::for_deadline(
                keyweave_types::MonoTime(0),
                Some(keyweave_types::MonoTime(if calls < 3 {
                    5_000_000
                } else {
                    1
                })),
            ))
        };
        assert_eq!(
            session.poll_with_control(&mut control).unwrap(),
            LoopStep::Continue
        );
        assert!(api::WAIT_CALLS.with(|calls| calls.borrow().is_empty()));
        assert_eq!(
            session.poll_with_control(&mut control).unwrap(),
            LoopStep::Continue
        );
        assert_eq!(calls, 3);
        assert_eq!(
            api::WAIT_CALLS.with(|calls| calls.borrow().clone()),
            vec![(0, 1, 0x04ff, 4)]
        );
        session.close().1.unwrap();
    }
    #[test]
    fn quit_disables_capture_before_returning_to_host() {
        let (mut session, _) = install(None);
        api::MESSAGES.with(|messages| messages.borrow_mut().push_back(0x12));
        assert_eq!(
            session
                .poll_with_control(&mut || Ok(WaitPlan::for_deadline(
                    keyweave_types::MonoTime(0),
                    None
                )))
                .unwrap(),
            LoopStep::Quit
        );
        assert_eq!(send(0x1e, false), 9);
        assert_eq!(
            session.poll_with_control(&mut || panic!("control must not run after Quit")),
            Err(HookSessionError::Closed)
        );
        assert!(api::WAIT_CALLS.with(|calls| calls.borrow().is_empty()));
        session.close().1.unwrap();
    }
    #[test]
    fn wait_failures_stop_capture_and_keep_error_diagnostics() {
        for (result, expected) in [
            (u32::MAX, HookSessionError::Message(5)),
            (0xc0, HookSessionError::InvalidWait(0xc0)),
        ] {
            std::thread::spawn(move || {
                let (mut session, _) = install(None);
                api::WAIT_RESULT.with(|value| value.set(result));
                assert_eq!(
                    session.poll_with_control(&mut || Ok(WaitPlan::for_deadline(
                        keyweave_types::MonoTime(0),
                        None
                    ))),
                    Err(expected)
                );
                assert_eq!(send(0x1e, false), 9);
                session.close().1.unwrap();
                assert!(POISONED.with(Cell::get));
            })
            .join()
            .unwrap();
        }
    }
    #[test]
    fn control_panic_and_error_stop_before_any_message_wait() {
        for panic in [false, true] {
            std::thread::spawn(move || {
                let (mut session, _) = install(None);
                let result = session.poll_with_control(&mut || {
                    assert!(!panic, "control failure");
                    Err(HookFault::Handler)
                });
                assert_eq!(
                    result,
                    Err(HookSessionError::Callback(if panic {
                        HookFault::Panic
                    } else {
                        HookFault::Handler
                    }))
                );
                assert_eq!(send(0x1e, false), 9);
                assert!(api::WAIT_CALLS.with(|calls| calls.borrow().is_empty()));
                session.close().1.unwrap();
            })
            .join()
            .unwrap();
        }
    }
}
