//! Cold, read-only primary-token identity and explicit-user DACL ownership.
use crate::control_identity::{
    ControlIdentityError as Error, ControlIdentityFailure as Failure, Principal, Sid,
};
use std::{cell::Cell, ffi::c_void, marker::PhantomData, mem::size_of, ptr::null_mut, rc::Rc};
type Handle = *mut c_void;
#[repr(C)]
struct SidAttributes {
    sid: usize,
    attributes: u32,
}
#[cfg(not(test))]
mod api;
#[cfg(test)]
mod api_mock;
#[cfg(test)]
use api_mock as api;
thread_local! {static POISONED:Cell<bool>=const{Cell::new(false)};}
fn context() -> Result<(), Error> {
    if !super::outside_callback() {
        Err(Error::InCallback)
    } else if POISONED.get() {
        Err(Error::Poisoned)
    } else {
        Ok(())
    }
}
fn error(call: &'static str) -> Error {
    // SAFETY: immediate same-thread scalar GetLastError after the failed SDK call.
    Error::Api {
        call,
        code: unsafe { api::GetLastError() },
    }
}
#[derive(Default)]
struct Resources {
    process: Handle,
    token: Handle,
    text: *mut u16,
}
impl Resources {
    fn close(&mut self) -> (Option<u32>, Option<u32>, Option<u32>) {
        let mut results = [None; 3];
        for (index, slot) in [(0, &mut self.token), (1, &mut self.process)] {
            let handle = std::mem::replace(slot, null_mut());
            if handle.is_null() {
                continue;
            }
            // SAFETY: unique successful Open* result, never a pseudo handle. One
            // attempt; a failed close is reported/poisoned, not guessed complete.
            if unsafe { api::CloseHandle(handle) } == 0 {
                // SAFETY: immediate scalar failure code.
                results[index] = Some(unsafe { api::GetLastError() });
            }
        }
        let text = std::mem::replace(&mut self.text, null_mut());
        if !text.is_null() {
            // SAFETY: unique allocation returned successfully by ConvertSidToStringSidW.
            if !unsafe { api::LocalFree(text.cast()) }.is_null() {
                // SAFETY: immediate scalar failure code.
                results[2] = Some(unsafe { api::GetLastError() });
            }
        }
        if results.iter().any(Option::is_some) {
            POISONED.set(true);
        }
        (results[0], results[1], results[2])
    }
    fn finish<T>(mut self, result: Result<T, Error>) -> Result<T, Failure> {
        let (token_close, process_close, sid_text_free) = self.close();
        match result {
            Ok(value)
                if token_close.is_none() && process_close.is_none() && sid_text_free.is_none() =>
            {
                Ok(value)
            }
            result => Err(Failure {
                operation: result.err(),
                token_close,
                process_close,
                sid_text_free,
            }),
        }
    }
}
impl Drop for Resources {
    fn drop(&mut self) {
        let _ = self.close();
    }
}
struct Information {
    buffer: Box<[u64; 128]>,
    used: usize,
}
impl Information {
    fn bytes(&self) -> &[u8] {
        // SAFETY: initialized fixed Box; used was checked <=1024 on API success.
        unsafe { std::slice::from_raw_parts(self.buffer.as_ptr().cast(), self.used) }
    }
}
fn query(token: Handle, class: i32) -> Result<Information, Error> {
    let mut result = Information {
        buffer: Box::new([0; 128]),
        used: 0,
    };
    let mut needed = 0;
    // SAFETY: live token, stable 8-byte-aligned 1024-byte initialized allocation,
    // DWORD length output. No async operation retains this pointer. Embedded SID
    // addresses remain valid because the Box allocation is not moved on return.
    if unsafe {
        api::GetTokenInformation(
            token,
            class,
            result.buffer.as_mut_ptr().cast(),
            1024,
            &mut needed,
        )
    } == 0
    {
        return Err(error("GetTokenInformation(control)"));
    }
    if needed == 0 || needed > 1024 {
        return Err(Error::Malformed);
    }
    result.used = needed as usize;
    Ok(result)
}
#[derive(PartialEq, Eq)]
struct Stamp {
    token: [u8; 8],
    logon: [u8; 8],
    modified: [u8; 8],
}
fn stamp(info: &Information) -> Result<Stamp, Error> {
    let bytes = info.bytes();
    if bytes.len() != 56 || bytes[24..28] != 1u32.to_le_bytes() {
        return Err(Error::Malformed);
    }
    let mut result = Stamp {
        token: [0; 8],
        logon: [0; 8],
        modified: [0; 8],
    };
    result.token.copy_from_slice(&bytes[..8]);
    result.logon.copy_from_slice(&bytes[8..16]);
    result.modified.copy_from_slice(&bytes[48..56]);
    Ok(result)
}
fn user_sid(info: &Information) -> Result<(Sid, usize), Error> {
    if info.used < size_of::<SidAttributes>() {
        return Err(Error::Malformed);
    }
    // SAFETY: aligned stable Box prefix is at least SID_AND_ATTRIBUTES size.
    let header = unsafe { std::ptr::read(info.buffer.as_ptr().cast::<SidAttributes>()) };
    let offset = header
        .sid
        .checked_sub(info.buffer.as_ptr() as usize)
        .ok_or(Error::Malformed)?;
    if offset < size_of::<SidAttributes>()
        || !offset.is_multiple_of(4)
        || offset.checked_add(8).is_none_or(|end| end > info.used)
    {
        return Err(Error::Malformed);
    }
    let bytes = info.bytes();
    let len = 8 + usize::from(bytes[offset + 1]) * 4;
    if len > 68 || offset.checked_add(len).is_none_or(|end| end > info.used) {
        return Err(Error::Malformed);
    }
    Ok((Sid::binary(&bytes[offset..offset + len])?, offset))
}
fn no_impersonation(resources: &mut Resources) -> Result<(), Error> {
    let mut token = null_mut();
    // SAFETY: borrowed current-thread pseudo handle, TOKEN_QUERY=8, OpenAsSelf,
    // initialized output. Only a successful result transfers a real token owner.
    if unsafe { api::OpenThreadToken(api::GetCurrentThread(), 8, 1, &mut token) } != 0 {
        if token.is_null() {
            return Err(Error::Malformed);
        }
        resources.token = token;
        return Err(Error::Impersonating);
    }
    // SAFETY: immediate error scalar; only ERROR_NO_TOKEN establishes absence.
    let code = unsafe { api::GetLastError() };
    if code != 1008 {
        return Err(Error::Api {
            call: "OpenThreadToken(control)",
            code,
        });
    }
    Ok(())
}
fn read_principal(
    resources: &mut Resources,
    process: Handle,
) -> Result<(Principal, String), Error> {
    let mut token = null_mut();
    // SAFETY: borrowed live process/pseudo handle, TOKEN_QUERY only, no inheritance.
    if unsafe { api::OpenProcessToken(process, 8, &mut token) } == 0 {
        return Err(error("OpenProcessToken(control)"));
    }
    if token.is_null() {
        return Err(Error::Malformed);
    }
    resources.token = token;
    let before = stamp(&query(token, 10)?)?;
    let user = query(token, 1)?;
    let (sid, offset) = user_sid(&user)?;
    let session = query(token, 12)?;
    let bytes = session.bytes();
    if bytes.len() != 4 {
        return Err(Error::Malformed);
    }
    let session = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    if session == 0 {
        return Err(Error::SessionZero);
    }
    let mut text = null_mut();
    // SAFETY: verified in-allocation complete SID in the live user Box, SDK
    // returns one independent LocalAlloc WCHAR string; never dereference a raw
    // TOKEN_USER pointer before proving its range/alignment/length above.
    if unsafe {
        api::ConvertSidToStringSidW(
            user.buffer.as_ptr().cast::<u8>().add(offset).cast(),
            &mut text,
        )
    } == 0
    {
        return Err(error("ConvertSidToStringSidW(control)"));
    }
    if text.is_null() {
        return Err(Error::Malformed);
    }
    resources.text = text;
    let mut words = Vec::new();
    let mut terminated = false;
    for i in 0..=256 {
        // SAFETY: SDK guarantees a readable nul-terminated WCHAR string. Stop at
        // the first nul; bounded scan never reads past the live string's nul.
        let word = unsafe { *text.add(i) };
        if word == 0 {
            terminated = true;
            break;
        }
        if i == 256 {
            return Err(Error::Malformed);
        }
        words.push(word);
    }
    if !terminated {
        return Err(Error::Malformed);
    }
    let text = String::from_utf16(&words).map_err(|_| Error::Malformed)?;
    if Sid::text(&text)? != sid {
        return Err(Error::Malformed);
    }
    let after = stamp(&query(token, 10)?)?;
    if before != after {
        return Err(Error::Changed);
    }
    Ok((
        Principal {
            sid,
            logon: before.logon,
            session,
        },
        text,
    ))
}
fn collect(pid: Option<u32>) -> Result<(Principal, String), Failure> {
    context().map_err(Failure::operation)?;
    let mut resources = Resources::default();
    let result = (|| {
        no_impersonation(&mut resources)?;
        let process = if let Some(pid) = pid {
            if pid == 0 {
                return Err(Error::Malformed);
            }
            // SAFETY: limited read-only process query for the explicitly selected
            // PID. This does not kill/control/input; PID alone is not pipe proof.
            let handle = unsafe { api::OpenProcess(0x1000, 0, pid) };
            if handle.is_null() {
                return Err(error("OpenProcess(control)"));
            }
            resources.process = handle;
            // SAFETY: live owned process handle, scalar identity read.
            if unsafe { api::GetProcessId(handle) } != pid {
                return Err(Error::Changed);
            }
            handle
        } else {
            // SAFETY: borrowed pseudo handle; it is never adopted/closed.
            unsafe { api::GetCurrentProcess() }
        };
        let value = read_principal(&mut resources, process)?;
        if let Some(pid) = pid {
            // SAFETY: same live owned process handle after token snapshot.
            if unsafe { api::GetProcessId(process) } != pid {
                return Err(Error::Changed);
            }
        }
        Ok(value)
    })();
    resources.finish(result)
}
/// Immutable cold primary-token snapshot. Not an authenticated pipe connection.
pub struct ControlIdentity {
    principal: Principal,
    sid_text: String,
    _thread: PhantomData<Rc<()>>,
}
impl std::fmt::Debug for ControlIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ControlIdentity").finish_non_exhaustive()
    }
}
impl ControlIdentity {
    pub fn capture() -> Result<Self, Failure> {
        let (principal, sid_text) = collect(None)?;
        Ok(Self {
            principal,
            sid_text,
            _thread: PhantomData,
        })
    }
    /// Read-only primary-token scope comparison. The caller must separately bind
    /// a kernel-reported peer to a live pipe; arbitrary supplied PIDs are not auth.
    pub fn same_primary_token_scope_of(&self, pid: u32) -> Result<bool, Failure> {
        let before = Self::capture()?;
        if before.principal != self.principal {
            return Err(Failure::operation(Error::CallerChanged));
        }
        let (peer, _) = collect(Some(pid))?;
        let after = Self::capture()?;
        if after.principal != self.principal {
            return Err(Failure::operation(Error::CallerChanged));
        }
        Ok(peer == self.principal)
    }
    pub fn security_descriptor(&self) -> Result<ControlSecurity, Failure> {
        let current = Self::capture()?;
        if current.principal != self.principal {
            return Err(Failure::operation(Error::CallerChanged));
        }
        let sddl: Vec<u16> = format!("D:P(A;;GA;;;{})(A;;GA;;;SY)\0", self.sid_text)
            .encode_utf16()
            .collect();
        let mut pointer = null_mut();
        // SAFETY: bounded SDK-verified SID text in a fixed protected DACL template,
        // terminated WCHAR string, revision1. Only success transfers allocation.
        if unsafe {
            api::ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                1,
                &mut pointer,
                null_mut(),
            )
        } == 0
        {
            return Err(Failure::operation(error(
                "ConvertStringSecurityDescriptorToSecurityDescriptorW(control)",
            )));
        }
        if pointer.is_null() {
            return Err(Failure::operation(Error::Malformed));
        }
        Ok(ControlSecurity {
            pointer,
            _thread: PhantomData,
        })
    }
}
/// SDK-allocated explicit-user protected DACL. Native factories will borrow it
/// only synchronously; no raw handle/pointer, inheritance or Clone is exported.
pub struct ControlSecurity {
    pointer: Handle,
    _thread: PhantomData<Rc<()>>,
}
impl std::fmt::Debug for ControlSecurity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ControlSecurity").finish_non_exhaustive()
    }
}
impl ControlSecurity {
    pub fn close(&mut self) -> Result<(), Error> {
        if self.pointer.is_null() {
            return Ok(());
        }
        if !super::outside_callback() {
            return Err(Error::InCallback);
        }
        // SAFETY: sole SDK-owned LocalAlloc descriptor, no async operation holds
        // it. Failure retains the allocation for explicit bounded retry.
        if !unsafe { api::LocalFree(self.pointer) }.is_null() {
            POISONED.set(true);
            return Err(error("LocalFree(control security)"));
        }
        self.pointer = null_mut();
        Ok(())
    }
}
impl Drop for ControlSecurity {
    fn drop(&mut self) {
        if self.close().is_err() {
            POISONED.set(true);
        }
    }
}
#[cfg(test)]
mod tests;
