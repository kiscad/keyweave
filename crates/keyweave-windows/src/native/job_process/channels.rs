//! Owned worker endpoints, retained until all parent-side cleanup is confirmed.
use super::super::progress_pipe::{PipeCloseReport, PipeReader, PipeWriter};
use super::*;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ChannelCleanupReport {
    /// stdin, stdout, stderr respectively; all attempted even after a failure.
    pub inheritance_errors: [Option<u32>; 3],
    pub reader: PipeCloseReport,
    pub writer: PipeCloseReport,
    pub stdout_close_error: Option<u32>,
}
impl ChannelCleanupReport {
    pub fn complete(&self) -> bool {
        self.inheritance_errors.iter().all(Option::is_none)
            && self.reader.complete()
            && self.writer.complete()
            && self.stdout_close_error.is_none()
    }
}
pub(super) struct ChildProgress {
    reader: PipeReader,
    writer: PipeWriter,
    stdout: Handle,
}
impl ChildProgress {
    pub(super) fn new(reader: PipeReader, writer: PipeWriter) -> Self {
        Self {
            reader,
            writer,
            stdout: null_mut(),
        }
    }
    fn handles(&self) -> [Handle; 3] {
        [
            self.reader.transfer_state().0,
            self.stdout,
            self.writer.transfer_state().0,
        ]
    }
    pub(super) fn prepare(&mut self) -> Result<[Handle; 3], JobError> {
        let (reader, reader_id, reader_idle) = self.reader.transfer_state();
        let (writer, writer_id, writer_idle) = self.writer.transfer_state();
        if !reader_idle || !writer_idle || reader == writer || reader_id == writer_id {
            return Err(JobError::PipeState);
        }
        const NUL: [u16; 4] = [78, 85, 76, 0];
        // SAFETY: fixed NUL device, write-only, share read/write; no inheritance
        // or security pointers. Synchronous output is never used for protocol.
        self.stdout = unsafe {
            CreateFileW(
                NUL.as_ptr(),
                0x40000000,
                3,
                std::ptr::null(),
                3,
                0,
                null_mut(),
            )
        };
        if self.stdout as usize == usize::MAX {
            self.stdout = null_mut();
            return Err(error("CreateFileW(NUL)"));
        }
        let handles = self.handles();
        for handle in handles {
            // SAFETY: actual uniquely owned idle pipe/NUL handle, mask=INHERIT.
            // No Job, parent endpoint or event handle is marked inheritable.
            if unsafe { SetHandleInformation(handle, 1, 1) } == 0 {
                return Err(error("SetHandleInformation(inherit)"));
            }
        }
        Ok(handles)
    }
    pub(super) fn close(&mut self) -> ChannelCleanupReport {
        let mut report = ChannelCleanupReport::default();
        for (index, handle) in self.handles().into_iter().enumerate() {
            if !handle.is_null() {
                // SAFETY: still owned handle, mask only clears inheritance;
                // permitted even if a pipe IO awaits cancellation completion.
                if unsafe { SetHandleInformation(handle, 1, 0) } == 0 {
                    report.inheritance_errors[index] = Some(last_error_code());
                }
            }
        }
        report.reader = self.reader.close_for_transfer();
        report.writer = self.writer.close_for_transfer();
        if !self.stdout.is_null() {
            // SAFETY: unique NUL handle with no submitted IO; retain on failure.
            if unsafe { CloseHandle(self.stdout) } == 0 {
                report.stdout_close_error = Some(last_error_code());
            } else {
                self.stdout = null_mut();
            }
        }
        report
    }
}
impl Drop for ChildProgress {
    fn drop(&mut self) {
        if super::super::hook_thread::outside_callback() {
            let _ = self.close();
        } else if !self.stdout.is_null() {
            // SAFETY: NUL has no flight; closing our handle doesn't wait on IO.
            unsafe { CloseHandle(self.stdout) };
        }
        // Endpoint Drop preserves pending OS buffers and poisons on failure.
    }
}
