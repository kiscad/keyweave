//! Opaque attribute list: stable, aligned storage and stable borrowed Job value.
use super::*;

#[repr(C, align(16))]
#[derive(Clone)]
struct Block([u8; 16]);
pub(super) struct Attributes {
    storage: Box<[Block]>,
    // Numeric value borrows the parent's live Job; not a duplicated handle.
    jobs: Box<[Handle; 1]>,
    initialized: bool,
}
impl Attributes {
    pub(super) fn new(job: Handle) -> Result<Self, JobError> {
        let mut bytes = 0;
        // SAFETY: documented sizing query with NULL list and initialized SIZE_T;
        // count=1, reserved flags=0. No list may be deleted after this query.
        let result = unsafe { InitializeProcThreadAttributeList(null_mut(), 1, 0, &mut bytes) };
        if result != 0 {
            return Err(JobError::AttributeSize);
        }
        let cause = error("InitializeProcThreadAttributeList(size)");
        if !matches!(cause, JobError::Api { code: 122, .. }) {
            return Err(cause);
        }
        if bytes == 0 || bytes > 4096 {
            return Err(JobError::AttributeSize);
        }
        let mut owner = Self {
            storage: vec![Block([0; 16]); bytes.div_ceil(16)].into_boxed_slice(),
            jobs: Box::new([job]),
            initialized: false,
        };
        let capacity = owner.storage.len() * 16;
        // SAFETY: stable heap storage, at least requested bytes, 16-byte aligned
        // on both targets. API receives original size, not fabricated capacity.
        if unsafe { InitializeProcThreadAttributeList(owner.pointer(), 1, 0, &mut bytes) } == 0 {
            return Err(error("InitializeProcThreadAttributeList"));
        }
        owner.initialized = true;
        if bytes == 0 || bytes > capacity {
            return Err(JobError::AttributeSize);
        }
        // SAFETY: initialized list; stable one-HANDLE array remains live until
        // DeleteProcThreadAttributeList, including after moving owner. Borrowed
        // Job remains owned by spawn's resource owner. Reserved outputs NULL.
        if unsafe {
            UpdateProcThreadAttribute(
                owner.pointer(),
                0,
                0x2000d,
                owner.jobs.as_mut_ptr().cast(),
                size_of::<Handle>(),
                null_mut(),
                null_mut(),
            )
        } == 0
        {
            return Err(error("UpdateProcThreadAttribute(JOB_LIST)"));
        }
        Ok(owner)
    }
    pub(super) fn pointer(&mut self) -> *mut c_void {
        self.storage.as_mut_ptr().cast()
    }
}
impl Drop for Attributes {
    fn drop(&mut self) {
        if self.initialized {
            // SAFETY: delete exactly once, only after successful initialization;
            // storage and borrowed Job array are still alive during this call.
            unsafe { DeleteProcThreadAttributeList(self.pointer()) };
            self.initialized = false;
        }
    }
}
