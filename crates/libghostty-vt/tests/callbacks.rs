//! Effect callbacks: what they cost and where they may go. The allocation
//! count is process-wide, so this is its own test binary with a single test.
// These call into libghostty, which Miri cannot execute.
#![cfg(not(miri))]

use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
    sync::atomic::{AtomicUsize, Ordering},
};

use libghostty_vt::Terminal;

struct Counting;

static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

// SAFETY: every call goes straight to `System`, which upholds the contract.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

/// A terminal moved after its callbacks are registered still reaches them,
/// and a callback allocates nothing on the Rust side.
#[test]
fn callbacks_survive_a_move_and_allocate_nothing() {
    let bells = Cell::new(0_u32);
    let holds = Cell::new(0_u32);
    let mut terminal = Terminal::new(8, 2).unwrap();
    terminal.on_bell(|_| bells.set(bells.get() + 1)).unwrap();
    terminal
        .on_render_hold(|_, _| holds.set(holds.get() + 1))
        .unwrap();

    // Moved into a box and out again: the callbacks' table stays put.
    let mut terminal = *Box::new(terminal);
    terminal.vt_write(b"\x07");
    assert_eq!(bells.get(), 1);

    let before = ALLOCATIONS.load(Ordering::Relaxed);
    for _ in 0..100 {
        terminal.vt_write(b"\x07\x1b[?2026h\x1b[?2026l");
    }
    let allocated = ALLOCATIONS.load(Ordering::Relaxed) - before;
    assert_eq!((bells.get(), holds.get()), (101, 200));
    assert_eq!(allocated, 0, "Rust allocations in 300 callbacks");
}
