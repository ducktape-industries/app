//! What the pass's replay of wasmtime's translation needs in memory on the
//! shape that makes it largest (the review of #547, M2): one function whose
//! every local is read after a long chain of single-predecessor blocks, so
//! Cranelift's SSA builder, and the replay after it, define every local in
//! every block of the chain. Cranelift keeps those definitions dense, 4
//! bytes each; a map of them took the meter 1.29 GB on an 88 KB module
//! where wasmtime's own compile took 182 MB. Its own binary: the counting
//! allocator sees this one test only.
#![allow(
    clippy::unwrap_used,
    unsafe_code,
    reason = "a test fails loudly; a counting allocator is unsafe to implement"
)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

/// The system allocator, counting the bytes live and their peak.
struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

fn grew(bytes: usize) {
    let live = LIVE.fetch_add(bytes, Ordering::Relaxed) + bytes;
    PEAK.fetch_max(live, Ordering::Relaxed);
}

// SAFETY: every call goes to `System` with the caller's arguments; the
// counters only read the sizes.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller's contract, passed on
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            grew(layout.size());
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: the caller's contract, passed on
        unsafe { System.dealloc(ptr, layout) };
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: the caller's contract, passed on
        let moved = unsafe { System.realloc(ptr, layout, new_size) };
        if !moved.is_null() {
            LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
            grew(new_size);
        }
        moved
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// The most heap `meter` holds at once for `code`, past what was live before.
fn peak_of(code: &[u8]) -> usize {
    let before = LIVE.load(Ordering::Relaxed);
    PEAK.store(before, Ordering::Relaxed);
    view_meter::meter(code, 64 << 20).unwrap();
    PEAK.load(Ordering::Relaxed) - before
}

/// `branches` conditional branches out of a block, each starting a new
/// block, then a read of each of `locals` locals.
fn chain(locals: usize, branches: usize) -> Vec<u8> {
    let reads: String = (0..locals)
        .map(|i| format!("local.get {i} drop "))
        .collect();
    wat::parse_str(format!(
        "(module (func (export \"f\") (local {}) (block {} {reads})))",
        "i32 ".repeat(locals),
        "i32.const 0 br_if 0 ".repeat(branches),
    ))
    .unwrap()
}

#[test]
fn the_replay_keeps_definitions_as_densely_as_cranelift() {
    let (locals, branches) = (500, 4_000);
    let walrus_and_the_rest = peak_of(&chain(1, branches));
    let peak = peak_of(&chain(locals, branches));
    // every local defined in every block of the chain: 4 bytes each, twice
    // that at most with a vector's spare capacity (a map took about 30)
    let definitions = locals * branches;
    assert!(
        peak.saturating_sub(walrus_and_the_rest) <= 8 * definitions,
        "{locals} locals over {branches} blocks: {peak} bytes at the peak, {walrus_and_the_rest} \
         with one local"
    );
}
