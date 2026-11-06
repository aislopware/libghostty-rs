//! Idiomatic, safe Rust bindings for
//! [`libghostty-vt`](https://libghostty.tip.ghostty.org/),
//! a terminal emulation library extracted from the
//! [Ghostty](https://ghostty.org) terminal emulator.
//!
//! `libghostty-vt` contains the logic for handling the core parts of a
//! terminal emulator: parsing terminal escape sequences, maintaining terminal
//! state, encoding input events, etc. It can handle scrollback, line wrapping,
//! reflow on resize, and more.
//!
//! <div class="warning">
//!
//! This library is currently in development and the API is not yet stable.
//! Breaking changes are expected in future versions.
//! Use with caution in production code.
//!
//! </div>
//!
//! The core type of `libghostty-vt` is the [`Terminal`] — start there to get a
//! better sense of how everything is structured. You can also check out
//! a list of all top-level modules and their functions below.
//!
//! # Examples
//!
//! [`ghostling-rs`](https://github.com/Uzaaft/libghostty-rs/blob/master/example/ghostling_rs/src/main.rs)
//! is a minimal yet functional terminal emulator built with `libghostty-vt`,
//! written as a single file with only around 1000 lines of heavily commented,
//! easily digestible code. It is based on the original [Ghostling](https://github.com/ghostty-org/ghostling)
//! which is built on the same underlying C API.
//!
//! Other examples are currently work-in-progress — if you have any ideas for
//! examples, feel free to share them with us as an issue or pull request.
//!
//! # Memory management and lifetimes
//!
//! When creating the terminal and various other objects, you can control their
//! memory management via a **custom allocator**, usually specified with
//! methods like [`Terminal::new_with_alloc`]. Objects that accept allocators
//! are also bound by the `'alloc` lifetime, since they internally contain
//! a reference to the allocator. If you do not use a custom allocator,
//! feel free to always set the lifetime to `'static`.
//!
//! ## Using the unstable `Allocator` API
//!
//! You can adapt the existing, unstable `Allocator` API into a
//! [libghostty-friendly allocator](alloc::Allocator) via its `From`
//! implementation. Note that the `'alloc` lifetime must at least
//! live as long as the `Allocator` instance itself, and that the allocator
//! must be [`Sync`], since a terminal using it can move to another thread.
//!
//! # Thread safety
//!
//! libghostty-vt documents its threading rules (`vt.h`, "Threads"): the
//! library creates no threads, every callback runs on the thread whose call
//! triggered it, calls on one handle and on any handle borrowed from it must
//! not run at the same time, and handles that share nothing can be used on
//! different threads. The rules tie no handle to the thread that created
//! it, so a handle may move between threads as long as its calls never
//! overlap.
//!
//! The bindings map these rules onto Rust's auto traits:
//!
//! - [`Terminal`] is [`Send`] but not [`Sync`]: it can move to another
//!   thread, and `&mut self` already keeps its calls from overlapping.
//!   Everything that borrows the terminal, such as a
//!   [`GridRef`](screen::GridRef), stays with it through the borrow.
//! - Effect callbacks must be [`Send`], since they move with the terminal
//!   and run on whichever thread calls it. Share state with them through
//!   atomics or a [`Mutex`](std::sync::Mutex) rather than
//!   [`Cell`](std::cell::Cell) or [`RefCell`](std::cell::RefCell).
//! - Custom [allocators](alloc::Allocator) must be [`Sync`], since a terminal
//!   using one may run on another thread than other handles sharing it.
//! - A [`RenderState`] and its iterators are [`Send`]: they only touch the
//!   terminal during calls that borrow it.
//! - A [`Search`](search::Search) and a
//!   [`TrackedGridRef`](screen::TrackedGridRef) outlive any borrow of their
//!   terminal, yet freeing them unregisters state inside it. Every call that
//!   touches the terminal takes it as an argument, and dropping one hands it
//!   to its terminal, which frees it at its next write or when it is dropped
//!   itself, so either can be dropped on any thread.
//! - The process-wide settings in [`sys`] and the PNG decoder hook have their
//!   own rules, documented on each function.
//!
//! A terminal can therefore run on its own thread (or task in async
//! programming), talking to the rest of the program over
//! [channels](std::sync::mpsc::channel), or be created on one thread and
//! handed to another. Under sufficient load, it is generally more efficient
//! to offload terminal emulation to its own operating system-level thread,
//! in order to reduce competition with other business logic.
#![doc(
    html_logo_url = "https://raw.githubusercontent.com/ghostty-org/ghostty/2d0fb81751def478e2f8a5f7e2ee91fa9cbf9bff/images/icons/icon_128@2x.png"
)]
#![warn(clippy::pedantic)]
#![warn(missing_docs)]
#![warn(missing_debug_implementations)]
#![warn(missing_copy_implementations)]
#![warn(clippy::allow_attributes)]
#![warn(clippy::allow_attributes_without_reason)]
#![expect(
    clippy::needless_pass_by_value,
    reason = "handles such as grid refs and option structs are small, and taken
    by value the same way the C API takes them"
)]
#![allow(
    clippy::missing_errors_doc,
    reason = "underlying C API may return any error outside of expected and
    mitigated situations, and it is not feasible to document them all"
)]
#![cfg_attr(docsrs, feature(doc_cfg))]

pub use libghostty_vt_sys as ffi;

// Make sure that `Terminal`'s own impl blocks (i.e. core functions)
// are placed *before* any extra impl blocks from other modules,
// e.g. Kitty Graphics extensions, Selection APIs
pub mod terminal;

pub mod alloc;
pub mod build_info;
pub mod error;
pub mod fmt;
pub mod focus;
pub(crate) mod io;
pub mod key;
pub mod kitty;
pub mod log;
pub(crate) mod manifest;
pub mod mouse;
pub mod osc;
pub mod paste;
pub mod render;
pub mod screen;
pub mod search;
pub mod selection;
pub mod sgr;
pub mod snapshot;
pub mod style;
#[cfg(test)]
pub(crate) mod sync_cell;
pub mod sys;
pub mod unicode;

#[doc(inline)]
pub use crate::{
    error::Error,
    log::{Logger, set_logger},
    render::RenderState,
    terminal::Terminal,
};

pub(crate) fn sys_set<T>(opt: ffi::SysOption::Type, val: *const T) -> error::Result<()> {
    let result = unsafe { ffi::ghostty_sys_set(opt, val.cast()) };
    error::from_result(result)
}
