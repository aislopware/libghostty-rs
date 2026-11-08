//! Query compile-time build configuration of libghostty-vt.
//!
//! These values reflect the options the library was built with and are constant for the lifetime of the process.
//!
//! # Example
//! ```rust
//! use libghostty_vt::{Error, build_info::*};
//!
//! fn print_build_info() -> Result<(), Error> {
//!     println!(
//!         "SIMD: {}",
//!         if supports_simd().unwrap_or(false) { "enabled" } else { "disabled" }
//!     );
//!     println!(
//!         "Kitty graphics: {}",
//!         if supports_kitty_graphics().unwrap_or(false) { "enabled" } else { "disabled" }
//!     );
//!     println!(
//!         "Tmux control mode: {}",
//!         if supports_tmux_control_mode().unwrap_or(false) { "enabled" } else { "disabled" }
//!     );
//!     Ok(())
//! }
//! ```

use std::mem::MaybeUninit;

use crate::{
    error::{Error, Result, from_result},
    ffi::{self, BuildInfo as Info},
};

/// Whether SIMD-accelerated code paths are enabled.
pub fn supports_simd() -> Result<bool> {
    build_info(Info::SIMD)
}

/// Whether Kitty graphics protocol support is available.
pub fn supports_kitty_graphics() -> Result<bool> {
    build_info(Info::KITTY_GRAPHICS)
}

/// Whether tmux control mode support is available.
pub fn supports_tmux_control_mode() -> Result<bool> {
    build_info(Info::TMUX_CONTROL_MODE)
}

/// Whether the library was built with a feature. A build without one
/// leaves out its functions, so the wrappers over them fail.
///
/// A library older than the feature queries answers
/// [`Error::Unsupported`].
pub fn supports(feature: Feature) -> Result<bool> {
    build_info(feature.into())
}

/// A feature libghostty-vt can be built with or without. See [`supports`].
#[repr(i32)]
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, int_enum::IntEnum)]
#[non_exhaustive]
pub enum Feature {
    /// [Snapshots](crate::snapshot): `ghostty_snapshot_*`.
    Snapshot = Info::FEATURE_SNAPSHOT,
    /// The [formatter](crate::fmt): `ghostty_formatter_*`.
    Formatter = Info::FEATURE_FORMATTER,
    /// [Selections](crate::selection), with selection gestures.
    Selection = Info::FEATURE_SELECTION,
    /// [Search](crate::search): `ghostty_search_*`.
    Search = Info::FEATURE_SEARCH,
    /// The [render state](crate::render): `ghostty_render_state_*`.
    RenderState = Info::FEATURE_RENDER_STATE,
    /// Key, mouse, focus and paste encoding.
    InputEncode = Info::FEATURE_INPUT_ENCODE,
    /// Color parsing and palettes: `ghostty_color_*`.
    Color = Info::FEATURE_COLOR,
    /// Grid references, cells and rows: `ghostty_grid_ref_*`,
    /// `ghostty_tracked_grid_ref_*`, `ghostty_cell_get*`, `ghostty_row_get*`.
    GridIntrospection = Info::FEATURE_GRID_INTROSPECTION,
    /// Ghostty's glyph protocol, which lets applications register custom
    /// glyphs. It has no functions of its own.
    GlyphProtocol = Info::FEATURE_GLYPH_PROTOCOL,
}

/// The optimization mode the library was built with.
pub fn optimize_mode() -> Result<OptimizeMode> {
    build_info::<ffi::OptimizeMode::Type>(Info::OPTIMIZE)
        .and_then(|v| v.try_into().map_err(|_| Error::Unsupported))
}

/// The full version string (e.g. "1.2.3" or "1.2.3-dev+abcdef").
///
/// The bytes are not guaranteed to be valid UTF-8.
///
/// # Example
/// ```rust
/// use libghostty_vt::build_info::*;
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let version = version_string()?;
/// // Validate it yourself if you need a `&str`.
/// println!("libghostty-vt {}", std::str::from_utf8(version)?);
/// # Ok(())
/// # }
/// ```
pub fn version_string() -> Result<&'static [u8]> {
    build_string(Info::VERSION_STRING)
}
/// The major version number.
pub fn major_version() -> Result<usize> {
    build_info(Info::VERSION_MAJOR)
}
/// The minor version number.
pub fn minor_version() -> Result<usize> {
    build_info(Info::VERSION_MINOR)
}
/// The patch version number.
pub fn patch_version() -> Result<usize> {
    build_info(Info::VERSION_PATCH)
}
/// The pre metadata string (e.g. "alpha", "beta", "dev").
///
/// Has zero length if no pre metadata is present.
///
/// The bytes are not guaranteed to be valid UTF-8.
pub fn pre_version() -> Result<&'static [u8]> {
    build_string(Info::VERSION_PRE)
}
/// The build metadata string (e.g. commit hash).
///
/// Has zero length if no build metadata is present.
///
/// The bytes are not guaranteed to be valid UTF-8.
pub fn build_version() -> Result<&'static [u8]> {
    build_string(Info::VERSION_BUILD)
}

/// Query one of the `GhosttyString` build info fields as raw bytes.
fn build_string(tag: ffi::BuildInfo::Type) -> Result<&'static [u8]> {
    build_info::<ffi::String>(tag)
        // SAFETY: libghostty fills these strings from its comptime build
        // options (`terminal_options`), or from a `""` literal when the
        // pre/build metadata is absent. Both live in the library's static
        // data, so they're valid for `'static`. Upstream makes no UTF-8 or
        // NUL-termination promise, so we hand out the bytes as-is.
        .map(|s| unsafe { s.to_bytes() })
}

fn build_info<T>(tag: ffi::BuildInfo::Type) -> Result<T> {
    let mut value = MaybeUninit::zeroed();
    let result = unsafe { ffi::ghostty_build_info(tag, std::ptr::from_mut(&mut value).cast()) };
    from_result(result)?;
    // SAFETY: Value should be initialized after successful call.
    Ok(unsafe { value.assume_init() })
}

/// The optimization mode libghostty is compiled with.
#[repr(i32)]
#[derive(Copy, Clone, Debug, PartialEq, Eq, int_enum::IntEnum)]
pub enum OptimizeMode {
    /// Debug mode.
    ///
    /// Very slow with all safety checks enabled.
    Debug = ffi::OptimizeMode::DEBUG,
    /// Release mode optimized for safety.
    ///
    /// Faster than debug due to better code generation,
    /// but still very slow due to active safety checks.
    ReleaseSafe = ffi::OptimizeMode::RELEASE_SAFE,
    /// Release mode optimized for size.
    ReleaseSmall = ffi::OptimizeMode::RELEASE_SMALL,
    /// Release mode optimized for speed.
    ReleaseFast = ffi::OptimizeMode::RELEASE_FAST,
}
