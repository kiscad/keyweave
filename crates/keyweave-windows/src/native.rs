//! Private Windows unsafe boundary; native calls require explicit session actions.
#![allow(unsafe_code)]
use crate::keys::{KeyboardPacket, MAX_BATCH};
mod config_artifact;
#[cfg(any(target_os = "windows", test))]
mod hook_thread;
#[cfg(any(target_os = "windows", test))]
mod input_probe;
mod job_process;
#[cfg(target_os = "windows")]
pub use config_artifact::{
    ArtifactCleanup, ArtifactCreateError, ArtifactError, ArtifactReaderError, ConfigArtifact,
    ConfigReader, MAX_ARTIFACT_BYTES,
};
mod supervisor;
#[cfg(target_os = "windows")]
pub use supervisor::{
    ChannelDirection, ChannelFailure, Supervisor, SupervisorChannelCleanup, SupervisorCleanup,
    SupervisorConfigCleanup, SupervisorConfigResources, SupervisorError, SupervisorSpawnError,
    SupervisorStatus, TerminationReason, WORKER_ARGUMENT,
};
mod progress_pipe;
mod session;
mod session_info;
mod session_notifications;
mod worker;
mod worker_startup;
#[cfg(target_os = "windows")]
pub use hook_thread::{HookSession, HookSessionError, LoopStep};
#[cfg(target_os = "windows")]
pub use input_probe::{check_input_access, sample_input_state};
#[cfg(target_os = "windows")]
pub use job_process::{
    ChannelCleanupReport, JobError, JobProcessState, JobSpawnError, OwnedJobProcess,
};
#[cfg(target_os = "windows")]
pub use progress_pipe::{PipeCloseReport, PipeReader, PipeWriter};
#[cfg(target_os = "windows")]
pub use session::{CaptureSession, SessionError, SessionStep, ShutdownReport};
#[cfg(target_os = "windows")]
pub use session_notifications::{NotificationCleanup, NotificationError, SessionNotifications};
#[cfg(target_os = "windows")]
pub use worker::{
    Worker, WorkerAccessError, WorkerFault, WorkerInstallError, WorkerReply, WorkerReport,
    WorkerStep, WorkerStopReason,
};
#[cfg(target_os = "windows")]
pub use worker_startup::{
    WorkerParentGuard, WorkerStartup, WorkerStartupError, check_synchronous_progress_pipe_rejected,
};

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct KeyboardHook {
    vk: u32,
    scan: u32,
    flags: u32,
    time: u32,
    extra_info: usize,
}
#[repr(C)]
#[derive(Default)]
struct Message {
    window: usize,
    message: u32,
    wparam: usize,
    lparam: isize,
    time: u32,
    x: i32,
    y: i32,
}
#[cfg(target_os = "windows")]
use crate::{
    keys::InjectionTag,
    pump::{Insertion, SendBackend, SubmitError},
};

#[cfg(all(
    target_os = "windows",
    not(any(target_arch = "x86", target_arch = "x86_64"))
))]
compile_error!("Windows native ABI currently supports only x86/x64");

#[repr(C)]
#[derive(Clone, Copy)]
struct KeyboardInput {
    vk: u16,
    scan: u16,
    flags: u32,
    time: u32,
    extra_info: usize,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct MouseInput {
    dx: i32,
    dy: i32,
    data: u32,
    flags: u32,
    time: u32,
    extra_info: usize,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct HardwareInput {
    message: u32,
    low: u16,
    high: u16,
}
#[repr(C)]
#[derive(Clone, Copy)]
union Payload {
    keyboard: KeyboardInput,
    mouse: MouseInput,
    hardware: HardwareInput,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct Input {
    kind: u32,
    payload: Payload,
}
impl Input {
    const fn empty() -> Self {
        Self {
            kind: 1,
            payload: Payload {
                mouse: MouseInput {
                    dx: 0,
                    dy: 0,
                    data: 0,
                    flags: 0,
                    time: 0,
                    extra_info: 0,
                },
            },
        }
    }
    fn set(&mut self, packet: KeyboardPacket) {
        self.kind = 1;
        self.payload.keyboard = KeyboardInput {
            vk: packet.vk,
            scan: packet.scan,
            flags: packet.flags,
            time: packet.time,
            extra_info: packet.extra_info,
        };
    }
}
fn valid(packets: &[KeyboardPacket], tag: usize) -> bool {
    tag != 0
        && !packets.is_empty()
        && packets.len() <= MAX_BATCH
        && packets.iter().all(|packet| {
            packet.vk == 0
                && (1..=255).contains(&packet.scan)
                && packet.flags & 8 != 0
                && packet.flags & !11 == 0
                && packet.time == 0
                && packet.extra_info == tag
        })
}

#[cfg(target_os = "windows")]
#[link(name = "user32")]
unsafe extern "system" {
    fn SendInput(count: u32, inputs: *const Input, size: i32) -> u32;
}
#[cfg(target_os = "windows")]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetLastError() -> u32;
    fn SetLastError(error: u32);
}

/// Constructing this sender does not inject. Keep it on the Hook message thread.
#[cfg(target_os = "windows")]
pub struct SendInputBackend {
    buffer: Box<[Input; MAX_BATCH]>,
    tag: InjectionTag,
    _thread: std::marker::PhantomData<std::rc::Rc<()>>,
}
#[cfg(target_os = "windows")]
impl SendInputBackend {
    pub fn new(tag: InjectionTag) -> Self {
        Self {
            buffer: Box::new([Input::empty(); MAX_BATCH]),
            tag,
            _thread: std::marker::PhantomData,
        }
    }
}
#[cfg(target_os = "windows")]
impl SendBackend for SendInputBackend {
    fn submit(&mut self, packets: &[KeyboardPacket]) -> Result<Insertion, SubmitError> {
        if !valid(packets, self.tag.value()) {
            return Err(SubmitError::Rejected);
        }
        for (input, packet) in self.buffer.iter_mut().zip(packets) {
            input.set(*packet);
        }
        // SAFETY: valid() limits count to 1..=256; the uniquely borrowed buffer
        // contains that many fully initialized repr(C) INPUTs, discriminant 1
        // with KEYBDINPUT payload. Win32 uses DWORD/u16/ULONG_PTR sizes and system
        // calling convention, x86/x64 layouts are tested below. SendInput borrows
        // the array only for this synchronous call; no pointer escapes. cbSize is
        // the complete INPUT size (including its MOUSEINPUT-sized union), not KI.
        // Set/GetLastError are thread-local with no parameters/pointers retained.
        let (accepted, error) = unsafe {
            SetLastError(0);
            let accepted = SendInput(
                packets.len() as u32,
                self.buffer.as_ptr(),
                std::mem::size_of::<Input>() as i32,
            );
            let error = GetLastError();
            (accepted, error)
        };
        Ok(Insertion {
            accepted: accepted as usize,
            last_error: (accepted as usize != packets.len()).then_some(error),
        })
    }
}

#[cfg(all(test, not(target_os = "windows")))]
pub(super) use session::CaptureSession;

pub(super) fn outside_callback() -> bool {
    hook_thread::outside_callback()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::{align_of, offset_of, size_of};
    #[test]
    fn input_layout_includes_larger_mouse_union() {
        let wide = size_of::<usize>() == 8;
        assert_eq!(size_of::<HardwareInput>(), 8);
        assert_eq!(offset_of!(HardwareInput, message), 0);
        assert_eq!(offset_of!(HardwareInput, low), 4);
        assert_eq!(offset_of!(HardwareInput, high), 6);
        assert_eq!(size_of::<KeyboardInput>(), if wide { 24 } else { 16 });
        assert_eq!(size_of::<MouseInput>(), if wide { 32 } else { 24 });
        assert_eq!(size_of::<Input>(), if wide { 40 } else { 28 });
        assert_eq!(align_of::<Input>(), align_of::<usize>());
        assert_eq!(offset_of!(Input, kind), 0);
        assert_eq!(offset_of!(Input, payload), if wide { 8 } else { 4 });
        assert_eq!(offset_of!(KeyboardInput, vk), 0);
        assert_eq!(offset_of!(KeyboardInput, scan), 2);
        assert_eq!(offset_of!(KeyboardInput, flags), 4);
        assert_eq!(offset_of!(KeyboardInput, time), 8);
        assert_eq!(
            offset_of!(KeyboardInput, extra_info),
            if wide { 16 } else { 12 }
        );
        assert_eq!(
            offset_of!(MouseInput, extra_info),
            if wide { 24 } else { 20 }
        );
        assert_eq!(offset_of!(MouseInput, dx), 0);
        assert_eq!(offset_of!(MouseInput, dy), 4);
        assert_eq!(offset_of!(MouseInput, data), 8);
        assert_eq!(offset_of!(MouseInput, flags), 12);
        assert_eq!(offset_of!(MouseInput, time), 16);
        let packet = KeyboardPacket {
            vk: 0,
            scan: 0x1d,
            flags: 11,
            time: 0,
            extra_info: usize::MAX,
        };
        let mut native = Input::empty();
        native.set(packet);
        assert_eq!(native.kind, 1);
        // SAFETY: set() just selected and initialized the keyboard union member.
        let keyboard = unsafe { native.payload.keyboard };
        assert_eq!(
            (
                keyboard.vk,
                keyboard.scan,
                keyboard.flags,
                keyboard.time,
                keyboard.extra_info
            ),
            (0, 0x1d, 11, 0, usize::MAX)
        );
    }
    #[test]
    fn rejects_invalid_packets_before_native_invocation() {
        let packet = KeyboardPacket {
            vk: 0,
            scan: 0x1e,
            flags: 8,
            time: 0,
            extra_info: 1,
        };
        assert!(valid(&[packet; MAX_BATCH], 1));
        assert!(!valid(&[packet; MAX_BATCH + 1], 1));
        assert!(!valid(&[], 1));
        assert!(!valid(&[packet], 0));
        for invalid in [
            KeyboardPacket { vk: 1, ..packet },
            KeyboardPacket { scan: 0, ..packet },
            KeyboardPacket {
                scan: 256,
                ..packet
            },
            KeyboardPacket { flags: 4, ..packet },
            KeyboardPacket {
                flags: 12,
                ..packet
            },
            KeyboardPacket {
                extra_info: 2,
                ..packet
            },
            KeyboardPacket { time: 1, ..packet },
        ] {
            assert!(!valid(&[packet, invalid], 1));
        }
    }
    #[test]
    fn hook_and_message_layouts_match_pointer_width() {
        let wide = size_of::<usize>() == 8;
        assert_eq!(size_of::<KeyboardHook>(), if wide { 24 } else { 20 });
        assert_eq!(offset_of!(KeyboardHook, vk), 0);
        assert_eq!(offset_of!(KeyboardHook, scan), 4);
        assert_eq!(offset_of!(KeyboardHook, flags), 8);
        assert_eq!(offset_of!(KeyboardHook, time), 12);
        assert_eq!(offset_of!(KeyboardHook, extra_info), 16);
        assert_eq!(size_of::<Message>(), if wide { 48 } else { 28 });
        assert_eq!(offset_of!(Message, window), 0);
        assert_eq!(offset_of!(Message, message), if wide { 8 } else { 4 });
        assert_eq!(offset_of!(Message, wparam), if wide { 16 } else { 8 });
        assert_eq!(offset_of!(Message, lparam), if wide { 24 } else { 12 });
        assert_eq!(offset_of!(Message, time), if wide { 32 } else { 16 });
        assert_eq!(offset_of!(Message, x), if wide { 36 } else { 20 });
        assert_eq!(offset_of!(Message, y), if wide { 40 } else { 24 });
    }
}
