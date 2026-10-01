//! Terminal screen cell and row types.
//!
//! These types represent the contents of a terminal screen.
//! A [`Cell`] is a single grid cell and a [`Row`] is a single row.
//! Both are opaque values whose fields are accessed via their methods.
use std::{marker::PhantomData, mem::MaybeUninit, ptr::NonNull, sync::OnceLock};

use crate::{
    error::{Error, Result, from_optional_result_uninit, from_result, from_result_with_len},
    ffi,
    manifest::{self, Value},
    style::{self, PaletteIndex, RgbColor, Style},
    terminal::{Point, PointCoordinate, PointSpace, Terminal},
};

/// Terminal screen identifier.
///
/// Identifies which screen buffer is active in the terminal.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, int_enum::IntEnum)]
#[repr(i32)]
pub enum Screen {
    /// The primary (normal) screen.
    #[default]
    Primary = ffi::TerminalScreen::PRIMARY,
    /// The alternate screen.
    Alternate = ffi::TerminalScreen::ALTERNATE,
}

/// Resolved reference to a terminal cell position.
///
/// A grid reference is a resolved reference to a specific cell position in
/// the terminal's internal page structure. Obtain a grid reference from
/// [`Terminal::grid_ref`][crate::Terminal::grid_ref], then extract the cell
/// or row via [`GridRef::cell`] and [`GridRef::row`].
///
/// A grid reference is only valid until the next update to the terminal
/// instance. There is no guarantee that a grid reference will remain valid
/// after ANY operation, even if a seemingly unrelated part of the grid is
/// changed, so any information related to the grid reference should be read
/// and cached immediately after obtaining the grid reference.
///
/// This API is not meant to be used as the core of render loop.
/// It isn't built to sustain the framerates needed for rendering large screens.
/// Use the render state API for that.
#[derive(Clone, Debug)]
pub struct GridRef<'t> {
    pub(crate) inner: ffi::GridRef,
    pub(crate) _phan: PhantomData<&'t ffi::Terminal>,
}

impl GridRef<'_> {
    pub(crate) unsafe fn from_raw(inner: ffi::GridRef) -> Self {
        Self {
            inner,
            _phan: PhantomData,
        }
    }

    /// Get the row from a grid reference.
    pub fn row(&self) -> Result<Row> {
        let mut v = ffi::Row::default();
        let result =
            unsafe { ffi::ghostty_grid_ref_row(std::ptr::from_ref(&self.inner), &raw mut v) };
        from_result(result)?;
        Ok(Row(v))
    }
    /// Get the cell from a grid reference.
    pub fn cell(&self) -> Result<Cell> {
        let mut v = ffi::Cell::default();
        let result =
            unsafe { ffi::ghostty_grid_ref_cell(std::ptr::from_ref(&self.inner), &raw mut v) };
        from_result(result)?;
        Ok(Cell(v))
    }
    /// Get the style of the cell at the grid reference's position.
    pub fn style(&self) -> Result<Style> {
        let mut v = ffi::Style::default();
        let result =
            unsafe { ffi::ghostty_grid_ref_style(std::ptr::from_ref(&self.inner), &raw mut v) };
        from_result(result)?;
        Style::try_from(v)
    }

    /// Get the grapheme cluster codepoints for the cell at the grid
    /// reference's position.
    ///
    /// Writes the full grapheme cluster (the cell's primary codepoint
    /// followed by any combining codepoints) into the provided buffer.
    /// If the cell has no text, `Ok(0)` is returned.
    ///
    /// If the buffer is too small, the function returns
    /// `Err(Error::OutOfSpace { required })` where `required` is the
    /// required number of codepoints. The caller can then retry with
    /// a sufficiently sized buffer.
    pub fn graphemes(&self, buf: &mut [char]) -> Result<usize> {
        let mut len = 0;
        let result = unsafe {
            ffi::ghostty_grid_ref_graphemes(
                std::ptr::from_ref(&self.inner),
                std::ptr::from_mut(buf).cast(),
                buf.len(),
                &raw mut len,
            )
        };
        from_result_with_len(result, len)
    }

    /// Get the hyperlink URI for the cell at the grid reference's position.
    ///
    /// Writes the URI bytes into the provided buffer.
    /// If the cell has no hyperlink, `Ok(0)` is returned.
    ///
    /// If the buffer is too small, the function returns
    /// `Err(Error::OutOfSpace { required })` where `required` is the
    /// required number of codepoints. The caller can then retry with
    /// a sufficiently sized buffer.
    pub fn hyperlink_uri(&self, buf: &mut [u8]) -> Result<usize> {
        let mut len = 0;
        let result = unsafe {
            ffi::ghostty_grid_ref_hyperlink_uri(
                std::ptr::from_ref(&self.inner),
                std::ptr::from_mut(buf).cast(),
                buf.len(),
                &raw mut len,
            )
        };
        from_result_with_len(result, len)
    }
}

/// Owned grid references that move with the terminal.
///
/// A tracked grid reference follows its cell across normal screen operations.
/// For example scrolling, scrollback pruning, resize/reflow, and other
/// terminal mutations update the tracked reference automatically.
///
/// A tracked reference can still lose its original semantic location.
/// This can happen when the underlying grid is reset, pruned, or otherwise
/// discarded in a way that cannot be mapped to a meaningful new cell.
/// In that state, [`TrackedGridRef::has_value`] returns `false` and
/// [`TrackedGridRef::snapshot`] / [`TrackedGridRef::point`] return `Ok(None)`.
/// The handle remains valid, and callers may move it to a new point with
/// [`TrackedGridRef::set`].
///
/// To read cell data from a tracked reference, first snapshot it with
/// [`TrackedGridRef::snapshot`]. The returned [`GridRef`] is again an
/// untracked reference and follows the same short lifetime rules as any
/// other untracked grid reference.
///
/// A tracked reference belongs to the terminal screen/page-list that was
/// active when it was created or last set. Converting it to a point uses that
/// owning screen/page-list, even if the terminal has since switched between
/// primary and alternate screens. Calling [`TrackedGridRef::set`] resolves
/// the new point against the terminal's currently active screen/page-list
/// and may move the tracked reference between screens.
///
/// If the tracked grid reference outlives the terminal it is created from,
/// it remains valid, but all APIs return either `false` or `Ok(None)`.
///
/// Each tracked reference adds bookkeeping to terminal mutations. Use them
/// sparingly for long-lived anchors such as selections, search state, marks,
/// or application-side bookmarks.
#[derive(Debug)]
pub struct TrackedGridRef {
    inner: NonNull<ffi::TrackedGridRefImpl>,
    terminal: NonNull<ffi::TerminalImpl>,
}

impl TrackedGridRef {
    pub(crate) fn new(
        inner: NonNull<ffi::TrackedGridRefImpl>,
        terminal: NonNull<ffi::TerminalImpl>,
    ) -> Self {
        Self { inner, terminal }
    }

    /// Whether a tracked grid reference currently has a meaningful value.
    ///
    /// If the terminal that created the tracked reference has been dropped,
    /// this returns false.
    #[must_use]
    pub fn has_value(&self) -> bool {
        unsafe { ffi::ghostty_tracked_grid_ref_has_value(self.inner.as_ptr()) }
    }

    /// Snapshot a tracked grid reference into a regular [`GridRef`].
    ///
    /// The returned [`GridRef`] is an untracked snapshot and has the same lifetime
    /// rules as [`Terminal::grid_ref`]: it is only valid until the next terminal update.
    /// Snapshot immediately before calling [`GridRef::cell`], [`GridRef::row`],
    /// [`GridRef::graphemes`], [`GridRef::hyperlink_uri`], or [`GridRef::style`],
    ///
    /// If the tracked reference no longer has a meaningful value, this returns
    /// `Ok(None)`. This includes references whose owning terminal has been dropped.
    pub fn snapshot<'t>(&self, terminal: &'t Terminal<'_, '_>) -> Result<Option<GridRef<'t>>> {
        // The C ghostty_tracked_grid_ref_snapshot does not take a terminal, so
        // we validate the pairing here to keep the returned GridRef's lifetime
        // soundly tied to a terminal that actually owns the underlying pin.
        if self.terminal != terminal.inner.ptr {
            return Err(Error::InvalidValue);
        }
        let mut grid_ref = MaybeUninit::new(ffi::sized!(ffi::GridRef));
        let result = unsafe {
            ffi::ghostty_tracked_grid_ref_snapshot(self.inner.as_ptr(), grid_ref.as_mut_ptr())
        };

        from_optional_result_uninit(result, grid_ref).map(|value| {
            value.map(|raw| unsafe {
                // SAFETY: A successful libghostty snapshot initializes a
                // short-lived untracked grid reference for the provided
                // terminal. The returned Rust lifetime is tied to that
                // terminal borrow.
                GridRef::from_raw(raw)
            })
        })
    }

    /// Convert a tracked grid reference to a point in the requested coordinate space.
    ///
    /// This is the tracked equivalent of [`Terminal::point_from_grid_ref`].
    /// Unlike snapshotting, this does not expose an intermediate untracked
    /// [`GridRef`].
    ///
    /// A tracked reference is resolved against the terminal screen/page-list
    /// that currently owns the reference. If the terminal has switched between
    /// primary and alternate screens since the reference was created or last
    /// set, this may be different from the terminal's currently active screen.
    ///
    /// If the tracked reference no longer has a meaningful value, this returns
    /// `Ok(None)`. `Ok(None` is also returned when the reference cannot be represented
    /// in the requested coordinate space, including after the terminal that
    /// created the tracked reference has been dropped.
    pub fn point(&self, space: PointSpace) -> Result<Option<PointCoordinate>> {
        let mut point = MaybeUninit::<ffi::PointCoordinate>::zeroed();
        let result = unsafe {
            ffi::ghostty_tracked_grid_ref_point(
                self.inner.as_ptr(),
                space.into_raw(),
                point.as_mut_ptr(),
            )
        };

        from_optional_result_uninit(result, point).map(|value| value.map(Into::into))
    }

    /// Move an existing tracked grid reference to a new terminal point.
    ///
    /// On success, the tracked reference begins tracking the new point and any
    /// prior "no value" state is cleared. On `Err(Error::OutOfMemory)`, the original
    /// tracked reference is left unchanged.
    ///
    /// The terminal must be the same terminal that created the tracked reference.
    /// The point is resolved against the terminal screen/page-list that is active
    /// at the time this function is called. If the terminal has switched between
    /// primary and alternate screens, this may move the tracked reference from
    /// one screen/page-list to the other.
    pub fn set(&mut self, terminal: &mut Terminal<'_, '_>, point: Point) -> Result<&mut Self> {
        // The C layer validates the terminal/tracked-ref pairing and returns
        // GHOSTTY_INVALID_VALUE on mismatch, so we don't duplicate the check
        // on the Rust side.
        let result = unsafe {
            ffi::ghostty_tracked_grid_ref_set(
                self.inner.as_ptr(),
                terminal.inner.as_raw(),
                point.into(),
            )
        };
        from_result(result)?;
        Ok(self)
    }
}

impl Drop for TrackedGridRef {
    fn drop(&mut self) {
        unsafe { ffi::ghostty_tracked_grid_ref_free(self.inner.as_ptr()) }
    }
}

/// Represents a single terminal row.
///
/// The internal layout is opaque and must be queried via its methods.
/// Obtain cell values from terminal query APIs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Row(pub(crate) ffi::Row);

impl Row {
    fn get<T>(&self, tag: ffi::RowData::Type) -> Result<T> {
        let mut value = MaybeUninit::<T>::zeroed();
        let result = unsafe { ffi::ghostty_row_get(self.0, tag, value.as_mut_ptr().cast()) };
        // Since we manually model every possible query, this should never fail.
        from_result(result)?;
        // SAFETY: Value should be initialized after successful call.
        Ok(unsafe { value.assume_init() })
    }

    /// Whether this row is soft-wrapped.
    pub fn is_wrapped(self) -> Result<bool> {
        self.get(ffi::RowData::WRAP)
    }
    /// Whether this row is a continuation of a soft-wrapped row.
    pub fn is_wrap_continuation(self) -> Result<bool> {
        self.get(ffi::RowData::WRAP_CONTINUATION)
    }
    /// Whether any cells in this row have grapheme clusters.
    pub fn has_grapheme_cluster(self) -> Result<bool> {
        self.get(ffi::RowData::GRAPHEME)
    }
    /// Whether any cells in this row have styling (may have false positives).
    pub fn is_styled(self) -> Result<bool> {
        self.get(ffi::RowData::STYLED)
    }
    /// Whether any cells in this row have hyperlinks (may have false positives).
    pub fn has_hyperlink(self) -> Result<bool> {
        self.get(ffi::RowData::HYPERLINK)
    }
    /// The semantic prompt state of this row.
    pub fn semantic_prompt(self) -> Result<RowSemanticPrompt> {
        self.get::<ffi::RowSemanticPrompt::Type>(ffi::RowData::SEMANTIC_PROMPT)
            .and_then(|v| v.try_into().map_err(|_| Error::InvalidValue))
    }
    /// Whether this row contains a Kitty virtual placeholder.
    pub fn has_kitty_virtual_placeholder(self) -> Result<bool> {
        self.get(ffi::RowData::KITTY_VIRTUAL_PLACEHOLDER)
    }
    /// Whether this row is dirty and requires a redraw.
    pub fn is_dirty(self) -> Result<bool> {
        self.get(ffi::RowData::DIRTY)
    }
    /// Whether any cells in this row hold only a background colour (may have
    /// false positives). Such a cell has no style of its own, so
    /// [`Row::is_styled`] does not say so.
    pub fn has_background(self) -> Result<bool> {
        self.get(ffi::RowData::BACKGROUND)
    }
}

/// Represents a single terminal cell.
///
/// The internal layout is opaque and must be queried via its methods.
/// Obtain cell values from terminal query APIs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell(pub(crate) ffi::Cell);

impl Cell {
    fn get<T>(&self, tag: ffi::CellData::Type) -> Result<T> {
        let mut value = MaybeUninit::<T>::zeroed();
        let result = unsafe { ffi::ghostty_cell_get(self.0, tag, value.as_mut_ptr().cast()) };
        // Since we manually model every possible query, this should never fail.
        from_result(result)?;
        // SAFETY: Value should be initialized after successful call.
        Ok(unsafe { value.assume_init() })
    }

    /// The codepoint of the cell (0 if empty or bg-color-only).
    pub fn codepoint(self) -> Result<u32> {
        self.get(ffi::CellData::CODEPOINT)
    }
    /// The content tag describing what kind of content is in the cell.
    pub fn content_tag(self) -> Result<CellContentTag> {
        self.get::<ffi::CellContentTag::Type>(ffi::CellData::CONTENT_TAG)
            .and_then(|v| v.try_into().map_err(|_| Error::InvalidValue))
    }
    /// The wide property of the cell.
    pub fn wide(self) -> Result<CellWide> {
        self.get::<ffi::CellWide::Type>(ffi::CellData::WIDE)
            .and_then(|v| v.try_into().map_err(|_| Error::InvalidValue))
    }
    /// Whether the cell has text to render.
    pub fn has_text(self) -> Result<bool> {
        self.get(ffi::CellData::HAS_TEXT)
    }
    /// Whether the cell has non-default styling.
    pub fn has_styling(self) -> Result<bool> {
        self.get(ffi::CellData::HAS_STYLING)
    }
    /// The style ID for the cell (for use with style lookups).
    pub fn style_id(self) -> Result<style::Id> {
        self.get(ffi::CellData::STYLE_ID).map(style::Id)
    }
    /// Whether the cell has a hyperlink.
    pub fn has_hyperlink(self) -> Result<bool> {
        self.get(ffi::CellData::HAS_HYPERLINK)
    }
    /// Whether the cell is protected.
    pub fn is_protected(self) -> Result<bool> {
        self.get(ffi::CellData::PROTECTED)
    }
    /// The semantic content type of the cell (from OSC 133).
    pub fn semantic_content(self) -> Result<CellSemanticContent> {
        self.get::<ffi::CellSemanticContent::Type>(ffi::CellData::SEMANTIC_CONTENT)
            .and_then(|v| v.try_into().map_err(|_| Error::InvalidValue))
    }

    /// Every field at once, decoded from the packed value where the linked
    /// build's [`CellLayout`] is known, and read field by field otherwise.
    /// A caller reading many cells can look the layout up once and call
    /// [`CellLayout::decode`] itself.
    /// The same values as the individual getters, for a fraction of their
    /// cost: each getter is a call into libghostty.
    #[inline]
    pub fn fields(self) -> Result<CellFields> {
        match CellLayout::linked() {
            Some(layout) => layout.decode(self),
            None => self.fields_one_by_one(),
        }
    }

    fn fields_one_by_one(self) -> Result<CellFields> {
        Ok(CellFields {
            content_tag: self.content_tag()?,
            codepoint: self.codepoint()?,
            style_id: self.style_id()?,
            wide: self.wide()?,
            protected: self.is_protected()?,
            hyperlink: self.has_hyperlink()?,
            semantic_content: self.semantic_content()?,
        })
    }

    /// The palette index for the cell's background color.
    ///
    /// Only valid when [`Cell::content_tag`] is [`CellContentTag::BgColorPalette`].
    pub fn bg_color_palette(self) -> Result<PaletteIndex> {
        self.get(ffi::CellData::COLOR_PALETTE).map(PaletteIndex)
    }
    /// The RGB color value for the cell's background color.
    ///
    /// Only valid when [`Cell::content_tag`] is [`CellContentTag::BgColorRgb`].
    pub fn bg_color_rgb(self) -> Result<RgbColor> {
        Ok(self.get::<ffi::ColorRgb>(ffi::CellData::COLOR_RGB)?.into())
    }
}

/// Every field of a [`Cell`], read at once: what [`Cell::fields`] returns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellFields {
    /// See [`Cell::content_tag`].
    pub content_tag: CellContentTag,
    /// See [`Cell::codepoint`]: zero unless the content is a codepoint.
    pub codepoint: u32,
    /// See [`Cell::style_id`].
    pub style_id: style::Id,
    /// See [`Cell::wide`].
    pub wide: CellWide,
    /// See [`Cell::is_protected`].
    pub protected: bool,
    /// See [`Cell::has_hyperlink`].
    pub hyperlink: bool,
    /// See [`Cell::semantic_content`].
    pub semantic_content: CellSemanticContent,
}

impl CellFields {
    /// See [`Cell::has_text`].
    #[must_use]
    pub const fn has_text(&self) -> bool {
        self.codepoint != 0
    }

    /// See [`Cell::has_styling`]: the cell's style is not the default one.
    #[must_use]
    pub const fn has_styling(&self) -> bool {
        self.style_id.0 != DEFAULT_STYLE_ID
    }
}

/// The style id of the default style (`style.default_id` in libghostty),
/// which a cell without styling carries.
const DEFAULT_STYLE_ID: ffi::StyleId = 0;

/// Where the fields of a packed [`Cell`] sit in the linked libghostty.
///
/// libghostty documents the cell as a packed `u64` whose layout its type
/// manifest ([`ffi::ghostty_type_json`]) describes for the linked build, and
/// supports decoding it from that manifest instead of calling
/// [`ffi::ghostty_cell_get`] once per field. This is that decoding: the
/// positions are read from the manifest once, never assumed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellLayout {
    content_tag: Bits,
    codepoint: Bits,
    style_id: Bits,
    wide: Bits,
    protected: Bits,
    hyperlink: Bits,
    semantic_content: Bits,
}

/// One field of a packed value: its lowest bit and a mask of its width.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Bits {
    lsb: u32,
    mask: u64,
}

impl Bits {
    /// The field `name` of a packed descriptor, with `base` added to its
    /// lowest bit, if it fits in `width` bits of a `u64`.
    fn read(packed: &Value, name: &str, base: u64, max_width: u64) -> Option<Self> {
        let field = packed.at(&["bits", name])?;
        let lsb = base.checked_add(field.get("lsb")?.as_u64()?)?;
        let width = field.get("width")?.as_u64()?;
        if width == 0 || width > max_width || lsb.checked_add(width)? > 64 {
            return None;
        }
        Some(Self {
            lsb: u32::try_from(lsb).ok()?,
            mask: u64::MAX >> (64 - width),
        })
    }

    /// Whether the field `name` of a packed descriptor holds the C enum
    /// `type_name`, and the manifest gives that enum the `values` this
    /// binding was generated with: the decoder turns the bits into enum
    /// values as they are.
    fn holds_enum(
        manifest: &Value,
        packed: &Value,
        name: &str,
        type_name: &str,
        values: &[(&str, std::ffi::c_int)],
    ) -> bool {
        let typed = packed.at(&["bits", name, "type"]).and_then(Value::as_str) == Some(type_name);
        let known = manifest.at(&["types", type_name, "values"]);
        typed
            && values.iter().all(|&(key, value)| {
                known.and_then(|v| v.get(key)).and_then(Value::as_u64) == u64::try_from(value).ok()
            })
    }

    #[inline]
    const fn of(self, raw: u64) -> u64 {
        // `lsb` is below 64 (checked when the layout was read), so the
        // wrapping shift is the plain one without a check per field.
        raw.wrapping_shr(self.lsb) & self.mask
    }
}

impl CellLayout {
    /// The layout of the linked build, read from its manifest on first use.
    ///
    /// `None` when the manifest does not describe a cell this binding can
    /// decode (an unknown schema, a missing field, a field wider than its
    /// value type); [`Cell::fields`] then reads each field through the C API.
    #[must_use]
    #[inline]
    pub fn linked() -> Option<&'static Self> {
        static LAYOUT: OnceLock<Option<CellLayout>> = OnceLock::new();
        LAYOUT
            .get_or_init(|| Self::from_manifest(&manifest::linked()?))
            .as_ref()
    }

    fn from_manifest(manifest: &Value) -> Option<Self> {
        let cell = manifest.at(&["types", "GhosttyCell"])?;
        let packed =
            cell.get("kind")?.as_str()? == "packed" && cell.get("underlying")?.as_str()? == "u64";
        if !packed {
            return None;
        }
        let content = cell.at(&["bits", "content"])?;
        let content_lsb = content.get("lsb")?.as_u64()?;
        // Both codepoint arms must hold the codepoint in the same bits.
        let codepoint = Bits::read(
            content.at(&["arms", "CODEPOINT"])?,
            "codepoint",
            content_lsb,
            32,
        )?;
        let grapheme = Bits::read(
            content.at(&["arms", "CODEPOINT_GRAPHEME"])?,
            "codepoint",
            content_lsb,
            32,
        )?;
        if codepoint != grapheme {
            return None;
        }
        let enums = [
            (
                "content_tag",
                "GhosttyCellContentTag",
                &[
                    ("CODEPOINT", ffi::CellContentTag::CODEPOINT),
                    (
                        "CODEPOINT_GRAPHEME",
                        ffi::CellContentTag::CODEPOINT_GRAPHEME,
                    ),
                    ("BG_COLOR_PALETTE", ffi::CellContentTag::BG_COLOR_PALETTE),
                    ("BG_COLOR_RGB", ffi::CellContentTag::BG_COLOR_RGB),
                ][..],
            ),
            (
                "wide",
                "GhosttyCellWide",
                &[
                    ("NARROW", ffi::CellWide::NARROW),
                    ("WIDE", ffi::CellWide::WIDE),
                    ("SPACER_TAIL", ffi::CellWide::SPACER_TAIL),
                    ("SPACER_HEAD", ffi::CellWide::SPACER_HEAD),
                ][..],
            ),
            (
                "semantic_content",
                "GhosttyCellSemanticContent",
                &[
                    ("OUTPUT", ffi::CellSemanticContent::OUTPUT),
                    ("INPUT", ffi::CellSemanticContent::INPUT),
                    ("PROMPT", ffi::CellSemanticContent::PROMPT),
                ][..],
            ),
        ];
        let enums_known = enums.iter().all(|&(name, type_name, values)| {
            Bits::holds_enum(manifest, cell, name, type_name, values)
        });
        if !enums_known {
            return None;
        }
        Some(Self {
            content_tag: Bits::read(cell, "content_tag", 0, 31)?,
            codepoint,
            style_id: Bits::read(cell, "style_id", 0, u64::from(ffi::StyleId::BITS))?,
            wide: Bits::read(cell, "wide", 0, 31)?,
            protected: Bits::read(cell, "protected", 0, 1)?,
            hyperlink: Bits::read(cell, "hyperlink", 0, 1)?,
            semantic_content: Bits::read(cell, "semantic_content", 0, 31)?,
        })
    }

    /// Every field of `cell`. Debug builds check the result against the
    /// getters.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidValue`] for an enum value this binding does not know.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "each field was checked to fit its type when the layout was read"
    )]
    #[inline]
    pub fn decode(&self, cell: Cell) -> Result<CellFields> {
        let raw = cell.0;
        let content_tag = CellContentTag::try_from(self.content_tag.of(raw) as i32)
            .map_err(|_| Error::InvalidValue)?;
        let codepoint = match content_tag {
            CellContentTag::Codepoint | CellContentTag::CodepointGrapheme => {
                self.codepoint.of(raw) as u32
            }
            CellContentTag::BgColorPalette | CellContentTag::BgColorRgb => 0,
        };
        let fields = CellFields {
            content_tag,
            codepoint,
            style_id: style::Id(self.style_id.of(raw) as ffi::StyleId),
            wide: CellWide::try_from(self.wide.of(raw) as i32).map_err(|_| Error::InvalidValue)?,
            protected: self.protected.of(raw) != 0,
            hyperlink: self.hyperlink.of(raw) != 0,
            semantic_content: CellSemanticContent::try_from(self.semantic_content.of(raw) as i32)
                .map_err(|_| Error::InvalidValue)?,
        };
        debug_assert_eq!(
            cell.fields_one_by_one().ok(),
            Some(fields),
            "a decoded cell differs from libghostty's own reading of it"
        );
        Ok(fields)
    }
}

/// Row semantic prompt state.
///
/// Indicates whether any cells in a row are part of a shell prompt, as reported by OSC 133 sequences.
#[repr(i32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, int_enum::IntEnum)]
pub enum RowSemanticPrompt {
    /// No prompt cells in this row.
    None = ffi::RowSemanticPrompt::NONE,
    /// Prompt cells exist and this is a primary prompt line.
    Prompt = ffi::RowSemanticPrompt::PROMPT,
    /// Prompt cells exist and this is a continuation line.
    Continuation = ffi::RowSemanticPrompt::PROMPT_CONTINUATION,
}

/// Cell content tag.
///
/// Describes what kind of content a cell holds.
#[repr(i32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, int_enum::IntEnum)]
pub enum CellContentTag {
    /// A single codepoint (may be zero for empty).
    Codepoint = ffi::CellContentTag::CODEPOINT,
    /// A codepoint that is part of a multi-codepoint grapheme cluster.
    CodepointGrapheme = ffi::CellContentTag::CODEPOINT_GRAPHEME,
    /// No text; background color from palette.
    BgColorPalette = ffi::CellContentTag::BG_COLOR_PALETTE,
    /// No text; background color as RGB.
    BgColorRgb = ffi::CellContentTag::BG_COLOR_RGB,
}

/// Cell wide property.
///
/// Describes the width behavior of a cell.
#[repr(i32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, int_enum::IntEnum)]
pub enum CellWide {
    /// Not a wide character, cell width 1.
    Narrow = ffi::CellWide::NARROW,
    /// Wide character, cell width 2.  
    Wide = ffi::CellWide::WIDE,
    /// Spacer after wide character. Do not render.
    SpacerTail = ffi::CellWide::SPACER_TAIL,
    /// Spacer at end of soft-wrapped line for a wide character.
    SpacerHead = ffi::CellWide::SPACER_HEAD,
}

/// Semantic content type of a cell.
///
/// Set by semantic prompt sequences (OSC 133) to distinguish between
/// command output, user input, and shell prompt text.
#[repr(i32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, int_enum::IntEnum)]
pub enum CellSemanticContent {
    /// Regular output content, such as command output.
    Output = ffi::CellSemanticContent::OUTPUT,
    /// Content that is part of user input.
    Input = ffi::CellSemanticContent::INPUT,
    /// Content that is part of a shell prompt.
    Prompt = ffi::CellSemanticContent::PROMPT,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest;

    /// A row an erase under a coloured pen left only a background on says so;
    /// a row of plain text does not.
    #[test]
    fn a_row_holding_only_a_background_says_so() {
        let mut terminal = Terminal::new(6, 3).expect("terminal");
        terminal.vt_write(b"plain\r\n\x1b[41m\x1b[K\x1b[0m");
        let row = |y| {
            terminal
                .grid_ref(Point::Active(PointCoordinate { x: 0, y }))
                .and_then(|r| r.row())
                .expect("row")
        };
        assert!(!row(0).has_background().expect("flag"));
        assert!(row(1).has_background().expect("flag"));
        assert!(!row(1).is_styled().expect("flag"), "the colour is no style");
    }

    /// Every cell of a screen holding each kind of content, width, style,
    /// link, protection and semantic content decodes to what the C getters
    /// read.
    #[test]
    fn decoded_cells_match_the_getters() {
        let layout = CellLayout::linked().expect("the linked build describes its cell");
        let mut terminal = Terminal::new(12, 8).expect("terminal");
        terminal.vt_write(
            concat!(
                "a\x1b[1;31mb\x1b[0m字e\u{301}\x1b]8;;http://x\x1b\\l\x1b]8;;\x1b\\",
                "\x1b[1\"qp\x1b[0\"q\r\n",
                "\x1b[41m\x1b[K\x1b[0m\r\n",
                "\x1b[48;2;1;2;3m\x1b[K\x1b[0m\r\n",
                "\x1b]133;A\x1b\\$ \x1b]133;B\x1b\\ls\x1b]133;C\x1b\\\r\n",
                "0123456789a字",
            )
            .as_bytes(),
        );
        let (mut tags, mut widths, mut semantics) = (Vec::new(), Vec::new(), Vec::new());
        let (mut styled, mut linked, mut protected) = (false, false, false);
        for y in 0..8 {
            for x in 0..12 {
                let cell = terminal
                    .grid_ref(Point::Active(PointCoordinate { x, y }))
                    .and_then(|r| r.cell())
                    .expect("cell");
                let decoded = layout.decode(cell).expect("decodes");
                assert_eq!(
                    Ok(decoded),
                    cell.fields_one_by_one().map_err(|_| ()),
                    "{x},{y}"
                );
                assert_eq!(decoded.has_text(), cell.has_text().expect("has_text"));
                assert_eq!(
                    decoded.has_styling(),
                    cell.has_styling().expect("has_styling")
                );
                tags.push(decoded.content_tag);
                widths.push(decoded.wide);
                semantics.push(decoded.semantic_content);
                styled |= decoded.has_styling();
                linked |= decoded.hyperlink;
                protected |= decoded.protected;
            }
        }
        for tag in [
            CellContentTag::Codepoint,
            CellContentTag::CodepointGrapheme,
            CellContentTag::BgColorPalette,
            CellContentTag::BgColorRgb,
        ] {
            assert!(tags.contains(&tag), "{tag:?} was seen");
        }
        for wide in [
            CellWide::Narrow,
            CellWide::Wide,
            CellWide::SpacerTail,
            CellWide::SpacerHead,
        ] {
            assert!(widths.contains(&wide), "{wide:?} was seen");
        }
        for content in [
            CellSemanticContent::Output,
            CellSemanticContent::Input,
            CellSemanticContent::Prompt,
        ] {
            assert!(semantics.contains(&content), "{content:?} was seen");
        }
        assert!(styled && linked && protected);
    }

    /// A manifest without a field, with one too wide for its type, with the
    /// two codepoint arms apart, or with an enum field of another type or
    /// numbering is refused, so the getters are used.
    #[test]
    fn a_manifest_that_does_not_describe_the_cell_is_refused() {
        let cell = |content_tag: &str, style_width: u32, grapheme_lsb: u32| {
            format!(
                r#"{{"types":{{"GhosttyCell":{{"kind":"packed","underlying":"u64","bits":{{
                {content_tag}
                "content":{{"lsb":2,"width":24,"arms":{{
                    "CODEPOINT":{{"bits":{{"codepoint":{{"lsb":0,"width":21}}}}}},
                    "CODEPOINT_GRAPHEME":{{"bits":{{"codepoint":{{"lsb":{grapheme_lsb},"width":21}}}}}}}}}},
                "style_id":{{"lsb":26,"width":{style_width}}},
                "wide":{{"lsb":42,"width":2,"type":"GhosttyCellWide"}},
                "protected":{{"lsb":44,"width":1}},
                "hyperlink":{{"lsb":45,"width":1}},
                "semantic_content":{{"lsb":46,"width":2,"type":"GhosttyCellSemanticContent"}}}}}},
                "GhosttyCellContentTag":{{"values":{{"CODEPOINT":0,"CODEPOINT_GRAPHEME":1,"BG_COLOR_PALETTE":2,"BG_COLOR_RGB":3}}}},
                "GhosttyCellWide":{{"values":{{"NARROW":0,"WIDE":1,"SPACER_TAIL":2,"SPACER_HEAD":3}}}},
                "GhosttyCellSemanticContent":{{"values":{{"OUTPUT":0,"INPUT":1,"PROMPT":2}}}}}}}}"#
            )
        };
        let read =
            |text: String| CellLayout::from_manifest(&manifest::parse(&text).expect("valid JSON"));
        let tag = r#""content_tag":{"lsb":0,"width":2,"type":"GhosttyCellContentTag"},"#;
        let good = read(cell(tag, 16, 0)).expect("the layout libghostty writes");
        assert_eq!(Some(good), CellLayout::linked().copied());
        assert_eq!(read(cell("", 16, 0)), None, "no content tag");
        assert_eq!(
            read(cell(tag, 17, 0)),
            None,
            "a style id wider than 16 bits"
        );
        assert_eq!(read(cell(tag, 16, 1)), None, "codepoint arms apart");
        let outside = cell(tag, 16, 0).replace(r#""lsb":46"#, r#""lsb":63"#);
        assert_eq!(read(outside), None, "a field past bit 63");
        let untyped = cell(tag, 16, 0).replace(r#","type":"GhosttyCellWide""#, "");
        assert_eq!(read(untyped), None, "a width without its enum");
        let renumbered =
            cell(tag, 16, 0).replace(r#""INPUT":1,"PROMPT":2"#, r#""INPUT":2,"PROMPT":1"#);
        assert_eq!(read(renumbered), None, "semantic content numbered apart");
    }
}
