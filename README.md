# libghostty-rs

> [!NOTE]
> **aislopware/libghostty-rs** is the binding [onicode](https://github.com/aislopware/onicode)'s terminals run on. It
> targets the frozen libghostty-vt ABI of [aislopware/ghostty](https://github.com/aislopware/ghostty), and links a
> prebuilt library by sha256, so Zig is only needed with `GHOSTTY_SOURCE_DIR`. It adds a `Send` terminal, one-call
> dirty-row reads, packed-cell decoding from the build's type manifest, VT-format replay, and memory-usage and
> render-state queries. Onicode pins a rev on `main`.
>
> Since 2026-11-07 (owner ruling) it builds for macOS and iOS 27.2 and later on Apple silicon only:
> `aarch64-apple-darwin`, `aarch64-apple-ios` and `aarch64-apple-ios-sim`, on an Apple silicon Mac. Linux, Android,
> Wasm, Windows, Intel Macs, shared linking and pkg-config are gone, and nothing is kept for compatibility.

Rust bindings and safe API for [libghostty-vt](https://ghostty.org), the virtual terminal emulator library extracted from [Ghostty](https://ghostty.org).

## Workspace Layout

- `crates/libghostty-vt-sys` — raw FFI bindings generated from `ghostty/vt.h`
- `crates/libghostty-vt` — safe Rust wrappers (Terminal, RenderState, KeyEncoder, MouseEncoder, etc.)
- `example/grid_ref_tracked_rs` — focused example of tracked grid references following cells through scrollback and reset
- `example/ghostling_rs` — Rust port of [ghostling](https://github.com/ghostty-org/ghostling), a minimal terminal emulator using [macroquad](https://macroquad.rs)

## Quick Start

```rust
use libghostty_vt::{Terminal, RenderState};
use libghostty_vt::render::{RowIterator, CellIterator};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create a terminal with 80 columns and 24 rows.
    let mut terminal = Terminal::new(80, 24)?;

    // Register an effect handler for PTY write-back (e.g. query responses).
    terminal.on_pty_write(|_term, data| {
        println!("PTY response: {} bytes", data.len());
    })?;

    // Feed VT-encoded data into the terminal.
    terminal.vt_write(b"Hello, \x1b[1;32mworld\x1b[0m!\r\n");
    terminal.vt_write(b"\x1b[38;2;255;128;0morange text\x1b[0m\r\n");

    // Capture a render snapshot and iterate rows/cells.
    let mut render_state = RenderState::new()?;
    let mut rows = RowIterator::new()?;
    let mut cells = CellIterator::new()?;

    let snapshot = render_state.update(&terminal)?;
    let mut row_iter = rows.update(&snapshot)?;

    while let Some(row) = row_iter.next() {
        let mut cell_iter = cells.update(row)?;
        while let Some(cell) = cell_iter.next() {
            let graphemes: Vec<char> = cell.graphemes()?;
            print!("{graphemes:?}");
        }
        println!();
    }

    Ok(())
}
```

## Building

No Zig is needed by default. `libghostty-vt-sys` links a prebuilt static
`libghostty-vt` for the Ghostty commit pinned in `build.rs`: `prebuilt.toml`
maps each Rust target to a release asset and its sha256, and the build script
downloads it with `curl`, checks the hash and that it was built for macOS or
iOS 27.2, and caches it under `~/Library/Caches/libghostty-vt` (or
`$LIBGHOSTTY_VT_SYS_CACHE_DIR`), one directory per hash. A target with no
entry, or an archive whose hash does not match, fails the build with the fix
in the message. Targets: `aarch64-apple-darwin`, `aarch64-apple-ios` and
`aarch64-apple-ios-sim`, built on an Apple silicon Mac; any other target or
host fails the build.

The other ways in, in the order the build script tries them:

- `GHOSTTY_SOURCE_DIR`: build that Ghostty checkout with Zig 0.17 (on PATH, or
  named by `ZIG`). This is the only path that runs Zig. Source builds derive the
  optimize mode from Cargo's profile (override with `LIBGHOSTTY_VT_SYS_OPTIMIZE`)
  and target the `baseline` CPU (override with `LIBGHOSTTY_VT_SYS_CPU`). macOS
  builds host-native; iOS builds through Ghostty's xcframework emit, which needs
  Xcode with the iOS SDK.
- `LIBGHOSTTY_VT_SYS_LIB_DIR`: link a directory holding `lib/libghostty-vt.a`
  and `include/`. The Nix checks use it with Ghostty's own package of the
  pinned commit, since the Nix sandbox cannot download the prebuilt library.

The library is always linked statically.

```sh
nix develop
cargo check
cargo test -p libghostty-vt-sys
cargo build --manifest-path example/Cargo.toml --workspace
```

### Regenerating bindings

`bindings.rs` and `layouts.rs` in `crates/libghostty-vt-sys/src/` are checked
in and must match the Ghostty commit pinned in `crates/libghostty-vt-sys/build.rs` and
`flake.nix` (run `nix flake update ghostty` after changing the latter); the
`bindings-fresh` Nix check fails otherwise. After bumping the pin, regenerate
the bindings from the dev shell:

```sh
cargo run -p libghostty-vt-sys --features bindgen-tool --bin gen-bindings && cargo fmt -p libghostty-vt-sys
```

The generator reads the headers from `GHOSTTY_INCLUDE_DIR`, or from
`GHOSTTY_SOURCE_DIR/include`, and otherwise from the headers the build script
installed for that same build. It parses them for each vendored target using
explicit Clang targets and verifies that the API declarations are identical.
`layouts.rs` holds compile-time size, alignment and field-offset assertions,
gated by the target ABI reported by `rustc`, for the three Apple targets in the
generator's `TARGETS` list. Targets share checks only when Clang generated
identical assertions. Regeneration needs no target SDKs or installed
cross-target Rust standard libraries, and ordinary builds need neither Clang
nor the generator.

### Miri Verification

Run the Rust-owned soundness checks with:

```sh
nix run .#miri
```

The command supplies its own nightly Miri toolchain and checks both binding and
wrapper crates. These checks intentionally stay on Rust-owned unsafe seams such
as string, slice, and allocator plumbing. They do not execute the native
Ghostty FFI backend.

Use `nix develop .#miri` when working interactively with the same toolchain.

### Running the example

The examples live in their own workspace under `example/`. To run one, enter
its folder and run:

```sh
cargo run
```

The examples have their own target directory, `example/target/`.
