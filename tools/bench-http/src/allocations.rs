use std::{
    alloc::{GlobalAlloc, Layout, System},
    sync::atomic::{
        AtomicBool, AtomicU64,
        Ordering::{Acquire, Relaxed, Release},
    },
};

use serde_json::{Value, json};

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);
static ALLOCATED_BYTES: AtomicU64 = AtomicU64::new(0);
static REALLOCATIONS: AtomicU64 = AtomicU64::new(0);
static REALLOCATED_BYTES: AtomicU64 = AtomicU64::new(0);
static UPDATING: AtomicBool = AtomicBool::new(false);

struct CountingAllocator;

// SAFETY: layouts, pointers and results are forwarded to System unchanged.
// The counters are allocation-free and never unwind.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller provides System's required layout.
        let ptr = unsafe { System.alloc(layout) };
        record_allocation(ptr, layout.size());
        ptr
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller provides System's required layout.
        let ptr = unsafe { System.alloc_zeroed(layout) };
        record_allocation(ptr, layout.size());
        ptr
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        // SAFETY: the caller supplies the System allocation and matching layout.
        let ptr = unsafe { System.realloc(ptr, layout, size) };
        if !ptr.is_null() {
            with_counters(|| {
                REALLOCATIONS.fetch_add(1, Relaxed);
                REALLOCATED_BYTES.fetch_add(size as u64, Relaxed);
            });
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: the caller supplies the System allocation and matching layout.
        unsafe { System.dealloc(ptr, layout) };
    }
}

fn record_allocation(ptr: *mut u8, size: usize) {
    if !ptr.is_null() {
        with_counters(|| {
            ALLOCATIONS.fetch_add(1, Relaxed);
            ALLOCATED_BYTES.fetch_add(size as u64, Relaxed);
        });
    }
}

// Only allocation-free, non-panicking atomic operations run inside this lock.
// Do not call System or build diagnostic output while holding it.
fn with_counters<T>(operation: impl FnOnce() -> T) -> T {
    while UPDATING.swap(true, Acquire) {
        std::hint::spin_loop();
    }
    let value = operation();
    UPDATING.store(false, Release);
    value
}

#[derive(Debug, Clone, Copy)]
pub(super) struct Snapshot([u64; 4]);

impl Snapshot {
    pub(super) fn now() -> Self {
        with_counters(|| {
            Self([
                ALLOCATIONS.load(Relaxed),
                ALLOCATED_BYTES.load(Relaxed),
                REALLOCATIONS.load(Relaxed),
                REALLOCATED_BYTES.load(Relaxed),
            ])
        })
    }

    pub(super) fn since(self, before: Self) -> Value {
        json!({
            "allocations": self.0[0] - before.0[0],
            "allocated_bytes": self.0[1] - before.0[1],
            "reallocations": self.0[2] - before.0[2],
            "reallocated_new_size_bytes": self.0[3] - before.0[3],
        })
    }
}
