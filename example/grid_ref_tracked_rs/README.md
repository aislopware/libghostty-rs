# grid_ref_tracked_rs

Rust recreation of Ghostty's `c-vt-grid-ref-tracked` example.

This demonstrates the safe `libghostty-vt` tracked grid reference API: create a
tracked reference to a cell, let terminal mutations move that cell into
scrollback, detect when a reset discards the location, and reuse the same handle
for a new point.

## Usage

```sh
cargo run --manifest-path example/Cargo.toml -p grid_ref_tracked_rs
```
