//! Format terminal content as plain text, VT sequences, or HTML.
//!
//! A formatter captures a reference to a terminal and formatting options.
//! It can be used repeatedly to produce output that reflects the current
//! terminal state at the time of each format call.
//!
//! # Copying a selection
//!
//! To get the text Ghostty would copy to the clipboard, format the
//! selection as plain text with soft wraps joined and trailing whitespace
//! trimmed, which [`FormatterOptions::copy`] sets up:
//!
//! ```
//! use libghostty_vt::{Terminal, fmt::{Formatter, FormatterOptions}};
//!
//! # fn main() -> Result<(), libghostty_vt::Error> {
//! let mut terminal = Terminal::new(10, 2)?;
//! terminal.vt_write(b"hello   ");
//! let all = terminal.select_all()?.expect("the screen has text");
//! terminal.set_selection(Some(&all))?;
//!
//! // The terminal's active selection, as a user would copy it.
//! let selection = terminal.selection()?.expect("a selection is active");
//! let options = FormatterOptions::copy().with_selection(&selection);
//! let mut formatter = Formatter::new(&terminal, options)?;
//! assert_eq!(formatter.format_alloc(None)?.to_vec(), b"hello");
//! # Ok(())
//! # }
//! ```
use std::marker::PhantomData;

use crate::{
    alloc::{Allocator, Bytes, Object},
    error::{Error, Result, from_result, from_result_with_len},
    ffi,
    selection::Selection,
    terminal::Terminal,
};

/// Formatter that formats terminal content.
#[derive(Debug)]
pub struct Formatter<'t, 'alloc: 'cb, 'cb: 't> {
    inner: Object<'alloc, ffi::FormatterImpl>,
    _terminal: PhantomData<&'t Terminal<'alloc, 'cb>>,
}

bitflags::bitflags! {
    /// Terminal state that [`Format::Vt`] output restores besides the text
    /// and its styles. Plain text and HTML output ignore extras.
    ///
    /// The values are libghostty's `GHOSTTY_FORMATTER_EXTRA_*` bits, frozen
    /// ABI, spelled out here so they do not depend on what bindgen makes of
    /// the macros.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub struct Extras: u32 {
        /// The color palette, with OSC 4.
        const PALETTE = 1 << 0;
        /// Terminal modes that differ from their defaults, with CSI h and l.
        const MODES = 1 << 1;
        /// The scrolling region, with DECSTBM and DECSLRM.
        const SCROLLING_REGION = 1 << 2;
        /// Tab stops, by clearing them all and setting each one again.
        const TABSTOPS = 1 << 3;
        /// The working directory, with OSC 7.
        const PWD = 1 << 4;
        /// Keyboard modes such as modifyOtherKeys.
        const KEYBOARD = 1 << 5;
        /// The cursor position, with CUP.
        const CURSOR = 1 << 6;
        /// The style the cursor writes new text with, with SGR.
        const STYLE = 1 << 7;
        /// The hyperlink the cursor writes new text with, with OSC 8.
        const HYPERLINK = 1 << 8;
        /// Character protection, with DECSCA.
        const PROTECTION = 1 << 9;
        /// The Kitty keyboard protocol flags.
        const KITTY_KEYBOARD = 1 << 10;
        /// The character sets, such as the line drawing set.
        const CHARSETS = 1 << 11;
    }
}

impl Extras {
    /// What a new formatter starts with: the extras that affect how output
    /// looks (the palette, and the cursor's style and hyperlink).
    pub const DEFAULT: Self = Self::PALETTE.union(Self::STYLE).union(Self::HYPERLINK);
}

impl Default for Extras {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Options for [creating a terminal formatter](Formatter::new).
///
/// The defaults are libghostty's: plain text, soft wraps kept, trailing
/// whitespace trimmed, the whole screen, and [`Extras::DEFAULT`].
#[derive(Clone, Copy, Debug)]
pub struct FormatterOptions<'t> {
    format: Format,
    unwrap: bool,
    trim: bool,
    // libghostty copies the selection when it is set, so the options keep
    // a copy too rather than a borrow. `'t` ties it to the terminal.
    selection: Option<ffi::Selection>,
    extras: Extras,
    _terminal: PhantomData<&'t ()>,
}

impl Default for FormatterOptions<'_> {
    fn default() -> Self {
        Self::new()
    }
}

impl<'t> FormatterOptions<'t> {
    /// Create a new set of options for [creating a terminal formatter](Formatter::new),
    /// with libghostty's defaults.
    #[must_use]
    pub fn new() -> Self {
        Self {
            format: Format::Plain,
            unwrap: false,
            trim: true,
            selection: None,
            extras: Extras::DEFAULT,
            _terminal: PhantomData,
        }
    }

    /// Options for the text Ghostty copies to the clipboard: plain text with
    /// soft-wrapped lines joined and trailing whitespace trimmed. Add the
    /// selection to copy with [`Self::with_selection`].
    #[must_use]
    pub fn copy() -> Self {
        Self::new().with_unwrap(true).with_trim(true)
    }

    /// Specify the output format to emit.
    #[must_use]
    pub fn with_format(mut self, value: Format) -> Self {
        self.format = value;
        self
    }
    /// Specify whether to unwrap soft-wrapped lines. VT output then leaves
    /// a soft-wrapped row for a replaying terminal of the same width to
    /// wrap itself.
    #[must_use]
    pub fn with_unwrap(mut self, value: bool) -> Self {
        self.unwrap = value;
        self
    }
    /// Specify whether to trim trailing whitespace on non-blank lines.
    ///
    /// Trailing blank lines are always trimmed, except in [`Format::Vt`]
    /// output without trimming, where they are kept as part of the layout.
    /// Defaults to `true`.
    #[must_use]
    pub fn with_trim(mut self, value: bool) -> Self {
        self.trim = value;
        self
    }
    /// Restrict output to a selection, such as the terminal's active one
    /// from [`Terminal::selection`].
    ///
    /// The selection must be a [valid snapshot selection](Selection#preconditions)
    /// for the terminal the formatter is created for. The formatter borrows
    /// the terminal, so the selection stays valid for the formatter's life.
    /// Without a selection, the whole screen is formatted.
    #[must_use]
    pub fn with_selection(mut self, value: &Selection<'t>) -> Self {
        self.selection = Some(value.inner);
        self
    }
    /// Specify the terminal state that [`Format::Vt`] output restores.
    /// Defaults to [`Extras::DEFAULT`].
    #[must_use]
    pub fn with_extras(mut self, value: Extras) -> Self {
        self.extras = value;
        self
    }
}

impl<'t, 'alloc: 'cb, 'cb: 't> Formatter<'t, 'alloc, 'cb> {
    /// Create a formatter for a terminal's active screen.
    pub fn new(terminal: &'t Terminal<'alloc, 'cb>, opts: FormatterOptions<'t>) -> Result<Self> {
        // SAFETY: A NULL allocator is always valid
        unsafe { Self::new_inner(std::ptr::null(), terminal, opts) }
    }

    /// Create a formatter for a terminal's active screen.
    ///
    /// See the [crate-level documentation](crate#memory-management-and-lifetimes)
    /// regarding custom memory management and lifetimes.
    pub fn new_with_alloc<'ctx: 'alloc>(
        alloc: &'alloc Allocator<'ctx>,
        terminal: &'t Terminal<'alloc, 'cb>,
        opts: FormatterOptions<'t>,
    ) -> Result<Self> {
        // SAFETY: Borrow checking should forbid invalid allocators
        unsafe { Self::new_inner(alloc.to_raw(), terminal, opts) }
    }

    unsafe fn new_inner(
        alloc: *const ffi::Allocator,
        terminal: &'t Terminal<'alloc, 'cb>,
        opts: FormatterOptions<'t>,
    ) -> Result<Self> {
        let mut raw: ffi::Formatter = std::ptr::null_mut();

        let result = unsafe {
            ffi::ghostty_formatter_terminal_new(alloc, &raw mut raw, terminal.inner.as_raw())
        };
        from_result(result)?;

        // Wrap the handle first, so a failing option below still frees it.
        let formatter = Self {
            inner: Object::new(raw)?,
            _terminal: PhantomData,
        };
        let emit: ffi::FormatterFormat::Type = opts.format.into();
        formatter.set(ffi::FormatterOption::EMIT, &raw const emit)?;
        formatter.set(ffi::FormatterOption::UNWRAP, &raw const opts.unwrap)?;
        formatter.set(ffi::FormatterOption::TRIM, &raw const opts.trim)?;
        let extras: ffi::FormatterExtras = opts.extras.bits();
        formatter.set(ffi::FormatterOption::EXTRAS, &raw const extras)?;
        if let Some(selection) = &opts.selection {
            formatter.set(
                ffi::FormatterOption::SELECTION,
                std::ptr::from_ref(selection),
            )?;
        }
        Ok(formatter)
    }

    /// Set one option. libghostty copies the value during the call.
    fn set<T>(&self, option: ffi::FormatterOption::Type, value: *const T) -> Result<()> {
        // SAFETY: The formatter is live and `value` points to the type the
        // option documents, valid for this call.
        from_result(unsafe {
            ffi::ghostty_formatter_set(self.inner.as_raw(), option, value.cast())
        })
    }

    /// Run the formatter and stream output to a writer.
    ///
    /// Each call formats the current terminal state and invokes the writer
    /// synchronously as output becomes available. The writer may be called
    /// more than once.
    ///
    /// If an error occurs, the writer may already contain a partial formatted
    /// output. The operation cannot be resumed from that partial output. This
    /// function does not flush or make the writer's destination durable.
    ///
    /// The writer runs inside a call into libghostty, so a panic in it aborts
    /// the process.
    ///
    /// # Safety
    ///
    /// The writer must not call formatter or terminal APIs using this
    /// formatter or its terminal. The borrow checker can't rule that out,
    /// since the formatter only borrows its terminal immutably, and some
    /// terminal methods that take `&self` still change libghostty state (e.g.
    /// [`Terminal::track_grid_ref`](crate::Terminal::track_grid_ref)).
    ///
    /// # Errors
    ///
    /// Returns [`Error::IoError`] if the writer rejects output, and
    /// [`Error::LimitExceeded`] if output accounting overflows.
    pub unsafe fn format_write<W: std::io::Write>(&mut self, writer: &mut W) -> Result<()> {
        let writer = crate::io::to_writer(writer);
        // SAFETY: The writer outlives this synchronous call, and the caller
        // promises it doesn't reach this formatter or its terminal.
        from_result(unsafe { ffi::ghostty_formatter_format(self.inner.as_raw(), writer) })
    }

    /// Run the formatter and append its output to `out`.
    ///
    /// The safe form of [`Self::format_write`] for the common case: a `Vec`
    /// only grows its own buffer, so it cannot reach the formatter or its
    /// terminal. The output streams straight into `out`, with no buffer
    /// allocated in libghostty and copied out.
    ///
    /// # Errors
    ///
    /// As [`Self::format_write`]; `out` may hold a partial output then.
    pub fn format_vec(&mut self, out: &mut Vec<u8>) -> Result<()> {
        // SAFETY: `Vec<u8>`'s `Write` touches nothing but the vector.
        unsafe { self.format_write(out) }
    }

    /// Run the formatter and return an allocated buffer with the output.
    ///
    /// Each call formats the current terminal state. The buffer is allocated
    /// using the provided allocator (or the default allocator if `None`).
    /// Empty output returns an empty buffer.
    pub fn format_alloc<'a, 'ctx: 'a>(
        &mut self,
        alloc: Option<&'a Allocator<'ctx>>,
    ) -> Result<Bytes<'a>> {
        let alloc = if let Some(alloc) = alloc {
            alloc.to_raw()
        } else {
            std::ptr::null()
        };

        let mut bytes = std::ptr::null_mut();
        let mut len = 0usize;
        let result = unsafe {
            ffi::ghostty_formatter_format_alloc(
                self.inner.as_raw(),
                alloc,
                std::ptr::from_mut(&mut bytes),
                std::ptr::from_mut(&mut len),
            )
        };
        from_result(result)?;

        // SAFETY: On success, libghostty hands over `len` bytes allocated
        // with `alloc`, or NULL for empty output.
        Ok(unsafe { Bytes::from_raw_parts(bytes, len, alloc) })
    }

    /// Run the formatter and produce output into the caller-provided buffer.
    ///
    /// Each call formats the current terminal state. If the buffer is too small,
    /// returns `Err(Error::OutOfSpace { required })` where `required` is the
    /// required size. The caller can then retry with a larger buffer.
    pub fn format_buf(&mut self, buf: &mut [u8]) -> Result<usize> {
        let mut len = 0usize;
        let result = unsafe {
            ffi::ghostty_formatter_format_buf(
                self.inner.as_raw(),
                std::ptr::from_mut(buf).cast(),
                buf.len(),
                std::ptr::from_mut(&mut len),
            )
        };
        // On OUT_OF_SPACE upstream stores the required size in `len`, which
        // the docs above promise callers; plain `from_result` would drop it.
        from_result_with_len(result, len)
    }

    /// Query the required buffer size for the formatted output.
    ///
    /// The result can be used to create a sufficiently large buffer
    /// for [`Formatter::format_buf`].
    pub fn format_len(&mut self) -> Result<usize> {
        let mut len = 0usize;
        let result = unsafe {
            ffi::ghostty_formatter_format_buf(
                self.inner.as_raw(),
                std::ptr::null_mut(),
                0,
                std::ptr::from_mut(&mut len),
            )
        };
        // This should always fail with OutOfSpace.
        match from_result(result) {
            Err(Error::OutOfSpace { .. }) => Ok(len),
            Err(e) => Err(e),
            Ok(()) => Err(Error::InvalidValue),
        }
    }
}

impl Drop for Formatter<'_, '_, '_> {
    fn drop(&mut self) {
        unsafe { ffi::ghostty_formatter_free(self.inner.as_raw()) }
    }
}

/// Output format.
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, int_enum::IntEnum)]
pub enum Format {
    /// Plain text (no escape sequences).
    Plain = ffi::FormatterFormat::PLAIN,
    /// VT sequences preserving colors, styles, URLs, etc.
    Vt = ffi::FormatterFormat::VT,
    /// HTML with inline styles.
    Html = ffi::FormatterFormat::HTML,
}

#[cfg(all(test, not(miri)))]
mod tests {
    use super::*;

    /// A writer that accepts at most one byte per write, counting calls.
    struct ByteAtATime {
        bytes: Vec<u8>,
        writes: usize,
    }

    impl std::io::Write for ByteAtATime {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.writes += 1;
            self.bytes.extend_from_slice(&buf[..1]);
            Ok(1)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// A writer that rejects everything.
    struct Reject;

    impl std::io::Write for Reject {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("rejected"))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn format_write<W: std::io::Write>(
        formatter: &mut Formatter<'_, '_, '_>,
        writer: &mut W,
    ) -> Result<()> {
        // SAFETY: None of the test writers touch the terminal.
        unsafe { formatter.format_write(writer) }
    }

    fn format_buf(formatter: &mut Formatter<'_, '_, '_>) -> Vec<u8> {
        let mut buf = vec![0; formatter.format_len().unwrap()];
        let len = formatter.format_buf(&mut buf).unwrap();
        buf.truncate(len);
        buf
    }

    #[test]
    fn vt_output_without_trim_ends_with_the_trailing_rows() {
        let mut terminal = Terminal::new(10, 4).unwrap();
        terminal.vt_write(b"top\r\n\r\n");
        let vt = |trim| {
            let options = FormatterOptions::new()
                .with_format(Format::Vt)
                .with_trim(trim)
                .with_extras(Extras::empty());
            format_buf(&mut Formatter::new(&terminal, options).unwrap())
        };
        assert!(
            vt(false).ends_with(b"top\r\n\r\n\r\n"),
            "every row but the last ends"
        );
        assert_eq!(vt(true), b"top", "trimmed output leaves them out");
    }

    #[test]
    fn writer_output_matches_buffer_output_in_every_format() {
        let mut terminal = Terminal::new(20, 3).unwrap();
        terminal.vt_write(b"plain \x1b[1;31mbold red\x1b[0m\r\nsecond line");
        for format in [Format::Plain, Format::Vt, Format::Html] {
            let options = FormatterOptions::new().with_format(format);
            let mut formatter = Formatter::new(&terminal, options).unwrap();
            let expected = format_buf(&mut formatter);
            assert!(!expected.is_empty(), "{format:?}");

            let mut actual = Vec::new();
            format_write(&mut formatter, &mut actual).unwrap();
            assert_eq!(actual, expected, "{format:?}");
        }
    }

    /// A too-small buffer reports the size the output needs, so the caller
    /// can retry with exactly that much, as the docs promise.
    #[test]
    fn format_buf_reports_the_required_size() {
        let mut terminal = Terminal::new(10, 3).unwrap();
        terminal.vt_write(b"one\r\ntwo");
        let options = FormatterOptions::new().with_format(Format::Plain);
        let mut formatter = Formatter::new(&terminal, options).unwrap();

        // "one\ntwo" is 7 bytes.
        let mut buf = [0; 3];
        let err = formatter.format_buf(&mut buf).unwrap_err();
        assert!(matches!(err, Error::OutOfSpace { required: 7 }), "{err:?}");

        let mut buf = [0; 7];
        let len = formatter.format_buf(&mut buf).unwrap();
        assert_eq!(&buf[..len], b"one\ntwo");
    }

    /// Mirrors upstream's `PageList VT without trim keeps trailing blank
    /// rows` test in `src/terminal/formatter.zig`.
    #[test]
    fn only_untrimmed_vt_keeps_trailing_blank_rows() {
        let mut terminal = Terminal::new(10, 6).unwrap();
        terminal.vt_write(b"one\r\ntwo");
        let format = |format, trim| {
            let options = FormatterOptions::new()
                .with_format(format)
                .with_trim(trim)
                .with_extras(Extras::empty());
            format_buf(&mut Formatter::new(&terminal, options).unwrap())
        };

        // Six rows, two with text, so five newlines.
        assert_eq!(format(Format::Vt, false), b"one\r\ntwo\r\n\r\n\r\n\r\n");
        // Trimmed output and other formats still drop them.
        assert_eq!(format(Format::Vt, true), b"one\r\ntwo");
        assert_eq!(format(Format::Plain, false), b"one\ntwo");
    }

    /// A selection goes through the same upstream `PageListFormatter`, so
    /// its trailing blank rows are kept too, and only up to the selection's
    /// last row.
    #[test]
    fn only_untrimmed_vt_keeps_trailing_blank_rows_in_selection() {
        use crate::terminal::{Point, PointCoordinate};

        let mut terminal = Terminal::new(10, 6).unwrap();
        terminal.vt_write(b"one\r\ntwo");
        let at = |x, y| {
            terminal
                .grid_ref(Point::Active(PointCoordinate { x, y }))
                .unwrap()
        };
        // Rows 0 to 3: two with text, then two blank ones.
        let selection = Selection::new(at(0, 0), at(9, 3), false);
        let format = |format, trim| {
            let options = FormatterOptions::new()
                .with_format(format)
                .with_trim(trim)
                .with_extras(Extras::empty())
                .with_selection(&selection);
            format_buf(&mut Formatter::new(&terminal, options).unwrap())
        };

        // Four rows, so three newlines: one between the text rows and one
        // for each blank row. The blank rows below the selection don't count.
        assert_eq!(format(Format::Vt, false), b"one\r\ntwo\r\n\r\n");
        assert_eq!(format(Format::Vt, true), b"one\r\ntwo");
        assert_eq!(format(Format::Plain, false), b"one\ntwo");
    }

    /// Options reach libghostty by key: the defaults are libghostty's, and
    /// each one set changes the output it should.
    #[test]
    fn options_are_set_by_key() {
        let mut terminal = Terminal::new(6, 3).unwrap();
        // "abcdefgh" soft-wraps after six columns; "x  " has trailing blanks.
        terminal.vt_write(b"abcdefgh\r\nx  ");
        let plain = |options: FormatterOptions<'_>| {
            format_buf(&mut Formatter::new(&terminal, options).unwrap())
        };
        assert_eq!(plain(FormatterOptions::new()), b"abcdef\ngh\nx");
        assert_eq!(plain(FormatterOptions::copy()), b"abcdefgh\nx");

        // Default VT extras include the palette (OSC 4); none leaves it out.
        let vt = |extras| {
            plain(
                FormatterOptions::new()
                    .with_format(Format::Vt)
                    .with_extras(extras),
            )
        };
        let osc4 = |bytes: &[u8]| bytes.windows(4).any(|w| w == b"\x1b]4;");
        assert!(osc4(&vt(Extras::DEFAULT)));
        assert!(!osc4(&vt(Extras::empty())));
    }

    #[test]
    fn partial_writes_are_retried_until_complete() {
        let mut terminal = Terminal::new(20, 3).unwrap();
        terminal.vt_write(b"hello\r\nworld");
        let mut formatter = Formatter::new(&terminal, FormatterOptions::new()).unwrap();
        let expected = format_buf(&mut formatter);

        let mut writer = ByteAtATime {
            bytes: Vec::new(),
            writes: 0,
        };
        format_write(&mut formatter, &mut writer).unwrap();
        assert_eq!(writer.bytes, expected);
        assert_eq!(writer.writes, expected.len());
    }

    #[test]
    fn writer_failure_is_an_io_error() {
        let mut terminal = Terminal::new(8, 2).unwrap();
        terminal.vt_write(b"hello");
        let mut formatter = Formatter::new(&terminal, FormatterOptions::new()).unwrap();
        assert!(matches!(
            format_write(&mut formatter, &mut Reject),
            Err(Error::IoError)
        ));
        // The formatter stays usable afterwards.
        let mut output = Vec::new();
        format_write(&mut formatter, &mut output).unwrap();
        assert_eq!(output, b"hello");
    }

    /// A writer that accepts `chunks` writes, then rejects everything.
    struct FailAfter {
        chunks: usize,
        bytes: Vec<u8>,
    }

    impl std::io::Write for FailAfter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            if self.chunks == 0 {
                return Err(std::io::Error::other("rejected"));
            }
            self.chunks -= 1;
            self.bytes.extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn large_output_streams_in_several_writes() {
        // Far more output than libghostty buffers before handing it over.
        let mut terminal = Terminal::new(200, 100).unwrap();
        for i in 0..100 {
            terminal.vt_write(format!("{i:0>199}\r\n").as_bytes());
        }
        let mut formatter = Formatter::new(&terminal, FormatterOptions::new()).unwrap();
        let expected = format_buf(&mut formatter);
        assert!(expected.len() > 16 * 1024);

        let mut writer = FailAfter {
            chunks: usize::MAX,
            bytes: Vec::new(),
        };
        format_write(&mut formatter, &mut writer).unwrap();
        assert_eq!(writer.bytes, expected);
        assert!(usize::MAX - writer.chunks > 1, "output came in one write");

        // A writer that fails after the first chunk fails the whole call,
        // having received only part of the output.
        let mut writer = FailAfter {
            chunks: 1,
            bytes: Vec::new(),
        };
        assert!(matches!(
            format_write(&mut formatter, &mut writer),
            Err(Error::IoError)
        ));
        assert!(!writer.bytes.is_empty() && writer.bytes.len() < expected.len());
    }

    #[test]
    fn format_vec_appends_what_format_buf_writes() {
        let mut terminal = Terminal::new(200, 100).unwrap();
        for i in 0..100 {
            terminal.vt_write(format!("{i:0>199}\r\n").as_bytes());
        }
        let mut formatter = Formatter::new(&terminal, FormatterOptions::new()).unwrap();
        let expected = format_buf(&mut formatter);

        let mut out = b"kept".to_vec();
        formatter.format_vec(&mut out).unwrap();
        assert_eq!(out[..4], *b"kept");
        assert_eq!(out[4..], expected);
    }
}
