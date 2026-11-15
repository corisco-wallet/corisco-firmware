//! Background thread for slow crypto, so the UI loop keeps rendering while it runs.
//!
//! Worker stacks and handoff cells live on the general heap, so everything holding seed or PIN
//! bytes is zeroed before it is freed.

use esp_idf_svc::hal::cpu::Core;
use esp_idf_svc::hal::task::thread::ThreadSpawnConfiguration;
use std::mem::MaybeUninit;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use zeroize::Zeroize;

/// Left free after a worker stack so the allocations made while it runs can't fail.
const HEAP_HEADROOM: usize = 16 * 1024;
/// Kept clear below the scrubbing frame's locals: the Xtensa window-spill area sits just under SP.
const SCRUB_SP_MARGIN: usize = 256;

/// Whether internal RAM has a contiguous block for a `stack_size` worker stack plus headroom.
pub fn has_room_for(stack_size: usize) -> bool {
    let largest_block = largest_internal_block();
    let fits = largest_block >= stack_size + HEAP_HEADROOM;
    if !fits {
        log::warn!("worker: largest internal block {largest_block} too small for a {stack_size} stack");
    }
    fits
}

pub fn largest_internal_block() -> usize {
    unsafe { esp_idf_svc::sys::heap_caps_get_largest_free_block(esp_idf_svc::sys::MALLOC_CAP_INTERNAL) }
}

/// Stack bytes the calling task has never touched; logged so stack sizes can be tuned on hardware.
pub fn unused_stack_bytes() -> u32 {
    // ESP-IDF's StackType_t is a byte, so this is already in bytes.
    unsafe { esp_idf_svc::sys::uxTaskGetStackHighWaterMark(std::ptr::null_mut()) }
}

/// Heap cell for a value crossing the thread boundary; wiped as soon as the value is moved out.
struct SecretSlot<T>(Mutex<MaybeUninit<T>>);

impl<T> SecretSlot<T> {
    fn new(value: T) -> Arc<Self> {
        Arc::new(Self(Mutex::new(MaybeUninit::new(value))))
    }

    fn empty() -> Arc<Self> {
        Arc::new(Self(Mutex::new(MaybeUninit::uninit())))
    }

    fn put(&self, value: T) {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).write(value);
    }

    /// # Safety
    /// A value must have been stored and not taken since.
    unsafe fn take(&self) -> T {
        let mut cell = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let value = cell.assume_init_read();
        cell.zeroize();
        value
    }
}

pub struct Worker<T> {
    handle: JoinHandle<()>,
    output: Arc<SecretSlot<T>>,
}

impl<T> Worker<T> {
    pub fn is_finished(&self) -> bool {
        self.handle.is_finished()
    }

    pub fn join(self) -> T {
        self.handle.join().expect("crypto worker panicked");
        // SAFETY: the thread returned normally, and storing the output is the last thing it does.
        unsafe { self.output.take() }
    }
}

/// Runs `work` on a thread pinned to core 1, or hands `work` back if the thread can't be created.
pub fn spawn<T, F>(stack_size: usize, work: F) -> Result<Worker<T>, F>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    let input = SecretSlot::new(work);
    let output = SecretSlot::empty();
    // pthreads default to priority 5 with no core affinity, which could starve the priority-1
    // UI loop on core 0; pin to core 1 at the UI's priority instead.
    let previous = ThreadSpawnConfiguration::get().unwrap_or_default();
    let worker_conf = ThreadSpawnConfiguration {
        name: Some(c"crypto"),
        priority: 1,
        pin_to_core: Some(Core::Core1),
        ..Default::default()
    };
    if let Err(e) = worker_conf.set() {
        log::warn!("worker: spawn config not applied: {e:?}");
    }
    let spawned = {
        let (input, output) = (input.clone(), output.clone());
        std::thread::Builder::new().stack_size(stack_size).spawn(move || {
            // SAFETY: stored above and taken only here, since the thread started.
            let work = unsafe { input.take() };
            output.put(work());
            let never_used = unused_stack_bytes() as usize;
            log::info!("worker: done, {never_used} of {stack_size} stack bytes never used");
            scrub_used_stack_below_caller(never_used);
        })
    };
    if let Err(e) = previous.set() {
        log::warn!("worker: thread spawn config not restored: {e:?}");
    }
    match spawned {
        Ok(handle) => Ok(Worker { handle, output }),
        Err(e) => {
            log::warn!("worker: thread spawn failed: {e}");
            // SAFETY: the thread never ran, so the work is still in the slot.
            Err(unsafe { input.take() })
        }
    }
}

/// Zeroes the part of the current task's stack that finished work touched below this frame,
/// where it left PIN, seed and key bytes.
///
/// The never-touched bottom `never_used` bytes are skipped: they hold no secrets, and they include
/// the 0xA5 canary FreeRTOS checks on every context switch to detect a stack overflow.
#[inline(never)]
fn scrub_used_stack_below_caller(never_used: usize) {
    let marker = 0u8;
    let below_frame = std::ptr::addr_of!(marker) as usize - SCRUB_SP_MARGIN;
    let stack_start = unsafe { esp_idf_svc::sys::pxTaskGetStackStart(std::ptr::null_mut()) } as usize;
    let used_start = stack_start + never_used;
    if stack_start == 0 || below_frame <= used_start {
        log::warn!("worker: stack not scrubbed");
        return;
    }
    // SAFETY: [used_start, below_frame) is this task's own stack below every live frame; nothing
    // else references it, and interrupts/context switches only use it while this task is paused.
    let used = unsafe { std::slice::from_raw_parts_mut(used_start as *mut MaybeUninit<u8>, below_frame - used_start) };
    used.zeroize();
}
