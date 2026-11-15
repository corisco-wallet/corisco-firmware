//! Background thread for slow crypto, so the UI loop keeps rendering while it runs.

use esp_idf_svc::hal::cpu::Core;
use esp_idf_svc::hal::task::thread::ThreadSpawnConfiguration;
use std::thread::JoinHandle;

/// Left free after a worker stack so the allocations made while it runs can't fail.
const HEAP_HEADROOM: usize = 16 * 1024;

/// Whether internal RAM has a contiguous block for a `stack_size` worker stack plus headroom.
pub fn has_room_for(stack_size: usize) -> bool {
    let largest_block =
        unsafe { esp_idf_svc::sys::heap_caps_get_largest_free_block(esp_idf_svc::sys::MALLOC_CAP_INTERNAL) };
    let fits = largest_block >= stack_size + HEAP_HEADROOM;
    if !fits {
        log::warn!("worker: largest internal block {largest_block} too small for a {stack_size} stack");
    }
    fits
}

/// Stack bytes the calling task has never touched; logged so stack sizes can be tuned on hardware.
pub fn unused_stack_bytes() -> u32 {
    // ESP-IDF's StackType_t is a byte, so this is already in bytes.
    unsafe { esp_idf_svc::sys::uxTaskGetStackHighWaterMark(std::ptr::null_mut()) }
}

/// Runs `work` on a thread pinned to core 1.
pub fn spawn<T: Send + 'static>(stack_size: usize, work: impl FnOnce() -> T + Send + 'static) -> JoinHandle<T> {
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
    let handle = std::thread::Builder::new()
        .stack_size(stack_size)
        .spawn(move || {
            let output = work();
            log::info!("worker: done, {} of {stack_size} stack bytes never used", unused_stack_bytes());
            output
        })
        .expect("failed to spawn crypto worker");
    if let Err(e) = previous.set() {
        log::warn!("worker: thread spawn config not restored: {e:?}");
    }
    handle
}
