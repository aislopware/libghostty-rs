# libghostty-vt-sys

Raw FFI bindings for libghostty-vt.

- Exposes checked-in generated bindings in `src/bindings.rs`.
- By default links the prebuilt static library for the build's target that
  `prebuilt.toml` lists: a release asset built from the pinned Ghostty commit
  by `.github/workflows/prebuilt.yml`, named by its sha256. The build script
  downloads it with `curl`, checks its sha256 and its `meta.toml` (Ghostty
  commit and target), unpacks it with `tar` and caches it outside `target/`
  under its hash, so a clean or a new target dir costs no download. No Zig is
  needed. A target `prebuilt.toml` does not list, a manifest for another pin
  or a hash mismatch fails the build with the fix in the message; it never
  falls back to building from source.
- The cache is `LIBGHOSTTY_VT_SYS_CACHE_DIR` when set, else
  `~/Library/Caches/libghostty-vt` on macOS and `$XDG_CACHE_HOME/libghostty-vt`
  or `~/.cache/libghostty-vt` on Linux. Each entry is `<sha256>/` with `lib/`, `include/` and `meta.toml`;
  delete any of them at will, and the next build fetches it again.
- Set `LIBGHOSTTY_VT_SYS_LIB_DIR` to a directory with `lib/` and `include/`
  (an unpacked prebuilt archive, for example) to link it as it is, with no
  network: for offline machines and CI caches.
- Set `GHOSTTY_SOURCE_DIR` to a Ghostty checkout to build libghostty-vt from
  it with Zig 0.17. This is the only path that runs Zig, and it warns on every
  build so it is never on by accident. It is how a pin bump is tested, and
  how bindings are regenerated. Zig's cache is shared under the cache
  directory above, so a second target dir rebuilds in seconds.
- Source builds only:
  - Set `ZIG` to an executable path or command name, without arguments, to
    override the default `zig` executable resolved through PATH.
  - Set `GHOSTTY_ZIG_SYSTEM_DIR` to force Zig package resolution through a
    pre-fetched `zig build --system` directory. This is intended for Nix and
    other sandboxed package managers that cannot fetch during build scripts.
  - Builds target Zig's portable `baseline` CPU by default. Set
    `LIBGHOSTTY_VT_SYS_CPU` to `native`, a named CPU model such as
    `x86_64_v3`, or another Zig CPU expression to optimize for known hardware.
  - Set `LIBGHOSTTY_VT_SYS_OPTIMIZE` to `Debug`, `ReleaseSafe`, `ReleaseFast`,
    or `ReleaseSmall` to override the Zig optimize mode, which otherwise
    follows the Cargo profile.
  - Setting `LIBGHOSTTY_VT_SYS_CPU` or `LIBGHOSTTY_VT_SYS_OPTIMIZE` without
    `GHOSTTY_SOURCE_DIR` is an error: the prebuilt libraries are always
    ReleaseFast for the baseline CPU.
  - iOS targets (`aarch64-apple-ios`, `aarch64-apple-ios-sim`) build through
    ghostty's xcframework emit instead of a flat cross build. This requires a
    macOS host with Xcode and the iOS SDK installed, and supports static
    linking only. The simulator library is arm64-only (what simulators run on
    Apple silicon), so `x86_64-apple-ios` is not supported.
- Static linking is the baseline rather than a Cargo feature. Enable the
  additive `link-dynamic` feature to link the shared library instead; the
  prebuilt libraries are static, so it needs `GHOSTTY_SOURCE_DIR` or
  `LIBGHOSTTY_VT_SYS_LIB_DIR`.
- If the `pkg-config` feature is enabled, the build uses an installed
  `libghostty-vt` found through `pkg-config` when neither `GHOSTTY_SOURCE_DIR`
  nor `LIBGHOSTTY_VT_SYS_LIB_DIR` is set, before the prebuilt library. With the
  default static link mode, it probes Ghostty's `libghostty-vt-static`
  pkg-config module instead.
- The bindings are generated from the libghostty-vt 1.0.0 headers, whose C
  ABI holds for every 1.x release, so an installed 1.x library links; only the
  pinned commit carries the fork's terminal fixes.

## Moving the pin

1. Change `GHOSTTY_COMMIT` (and `ZIG_VERSION` if the commit's
   `minimum_zig_version` moved) in `build.rs`.
2. Regenerate the bindings against a checkout of that commit:
   `GHOSTTY_SOURCE_DIR=<checkout> cargo run -p libghostty-vt-sys --features bindgen-tool --bin gen-bindings`.
3. Push. The push runs the prebuilt workflow, which builds every target,
   releases the archives under `vt-<commit12>` and commits the new
   `prebuilt.toml`. Until it has, builds without `GHOSTTY_SOURCE_DIR` fail
   with a message naming the workflow.
