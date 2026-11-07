# libghostty-vt-sys

Raw FFI bindings for libghostty-vt.

- Builds for `aarch64-apple-darwin`, `aarch64-apple-ios` and
  `aarch64-apple-ios-sim` only, on an Apple silicon Mac; the libraries target
  macOS and iOS 27.2 and later. Any other target or host fails the build.
- Exposes checked-in generated bindings in `src/bindings.rs`.
- By default links the prebuilt static library for the build's target that
  `prebuilt.toml` lists: a release asset built from the pinned Ghostty commit
  by `.github/workflows/prebuilt.yml`, named by its sha256. The build script
  downloads it with `curl`, checks its sha256 and its `meta.toml` (Ghostty
  commit, target and the 27.2 deployment target), unpacks it with `tar` and
  caches it outside `target/` under its hash, so a clean or a new target dir
  costs no download. No Zig is needed. A target `prebuilt.toml` does not list,
  a manifest for another pin or a hash mismatch fails the build with the fix
  in the message; it never falls back to building from source.
- The cache is `LIBGHOSTTY_VT_SYS_CACHE_DIR` when set, else
  `~/Library/Caches/libghostty-vt`. Each entry is `<sha256>/` with `lib/`,
  `include/` and `meta.toml`; delete any of them at will, and the next build
  fetches it again.
- Set `LIBGHOSTTY_VT_SYS_LIB_DIR` to a directory with `lib/libghostty-vt.a`
  and `include/` (an unpacked prebuilt archive, or Ghostty's Nix package's
  `dev` output, as the flake checks use) to link it as it is, with no network.
- Set `GHOSTTY_SOURCE_DIR` to a Ghostty checkout to build libghostty-vt from
  it with Zig 0.17. This is the only path that runs Zig, and it warns on every
  build so it is never on by accident. It is how a pin bump is tested, and
  how bindings are regenerated. Zig's cache is shared under the cache
  directory above, so a second target dir rebuilds in seconds. The deployment
  target comes from the checkout (the pinned fork commit sets 27.2).
- Source builds only:
  - Set `ZIG` to an executable path or command name, without arguments, to
    override the default `zig` executable resolved through PATH.
  - Builds target Zig's portable `baseline` CPU by default. Set
    `LIBGHOSTTY_VT_SYS_CPU` to `native`, a named CPU model such as
    `apple_m1`, or another Zig CPU expression to optimize for known hardware.
  - Set `LIBGHOSTTY_VT_SYS_OPTIMIZE` to `Debug`, `ReleaseSafe`, `ReleaseFast`,
    or `ReleaseSmall` to override the Zig optimize mode, which otherwise
    follows the Cargo profile.
  - Setting `LIBGHOSTTY_VT_SYS_CPU` or `LIBGHOSTTY_VT_SYS_OPTIMIZE` without
    `GHOSTTY_SOURCE_DIR` is an error: the prebuilt libraries are always
    ReleaseFast for the baseline CPU.
  - macOS builds host-native. iOS targets build through ghostty's
    xcframework emit instead of a flat cross build, which needs Xcode with
    the iOS SDK.
- The library is always linked statically.

## Moving the pin

1. Change `GHOSTTY_COMMIT` (and `ZIG_VERSION` if the commit's
   `minimum_zig_version` moved) in `build.rs`.
2. Regenerate the bindings against a checkout of that commit:
   `GHOSTTY_SOURCE_DIR=<checkout> cargo run -p libghostty-vt-sys --features bindgen-tool --bin gen-bindings`.
3. Push. The push runs the prebuilt workflow, which builds every target,
   releases the archives under `vt-<commit12>` and commits the new
   `prebuilt.toml`. Until it has, builds without `GHOSTTY_SOURCE_DIR` fail
   with a message naming the workflow.
