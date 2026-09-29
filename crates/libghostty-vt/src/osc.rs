//! Handling OSC (Operating System Command) escape sequences.

use std::{marker::PhantomData, mem::MaybeUninit};

use crate::{
    alloc::{Allocator, Object},
    error::{Result, from_result},
    ffi,
};

/// OSC (Operating System Command) sequence parser and command handling.
///
/// The parser operates in a streaming fashion, processing input byte-by-byte
/// to handle OSC sequences that may arrive in fragments across multiple reads.
/// This interface makes it easy to integrate into most environments and avoids
/// over-allocating buffers.
#[derive(Debug)]
pub struct Parser<'alloc>(Object<'alloc, ffi::OscParserImpl>);

impl<'alloc> Parser<'alloc> {
    /// Create a new OSC parser.
    pub fn new() -> Result<Self> {
        // SAFETY: A NULL allocator is always valid
        unsafe { Self::new_inner(std::ptr::null()) }
    }

    /// Create a new OSC parser with a custom allocator.
    ///
    /// See the [crate-level documentation](crate#memory-management-and-lifetimes)
    /// regarding custom memory management and lifetimes.
    pub fn new_with_alloc<'ctx: 'alloc>(alloc: &'alloc Allocator<'ctx>) -> Result<Self> {
        // SAFETY: Borrow checking should forbid invalid allocators
        unsafe { Self::new_inner(alloc.to_raw()) }
    }

    unsafe fn new_inner(alloc: *const ffi::Allocator) -> Result<Self> {
        let mut raw: ffi::OscParser = std::ptr::null_mut();
        let result = unsafe { ffi::ghostty_osc_new(alloc, &raw mut raw) };
        from_result(result)?;
        Ok(Self(Object::new(raw)?))
    }

    /// Reset an OSC parser instance to its initial state.
    ///
    /// Resets the parser state, clearing any partially parsed OSC sequences
    /// and returning the parser to its initial state. This is useful for
    /// reusing a parser instance or recovering from parse errors.
    pub fn reset(&mut self) {
        unsafe { ffi::ghostty_osc_reset(self.0.as_raw()) }
    }

    /// Parse the next byte in an OSC sequence.
    ///
    /// Processes a single byte as part of an OSC sequence. The parser maintains
    /// internal state to track the progress through the sequence. Call this
    /// function for each byte in the sequence data.
    ///
    /// When finished pumping the parser with bytes, call [`Parser::end`] to
    /// get the final result.
    pub fn next_byte(&mut self, byte: u8) {
        unsafe { ffi::ghostty_osc_next(self.0.as_raw(), byte) }
    }

    /// Finalize OSC parsing and retrieve the parsed command.
    ///
    /// Call this function after feeding all bytes of an OSC sequence to the parser
    /// using [`Parser::next_byte`], except the byte that ended it. Pass that
    /// byte as `terminator`. Invalid commands will return a command with type
    /// [`CommandType::Invalid`].
    ///
    /// Commands that reply to the program end their reply the same way the
    /// request ended: a terminator of 0x07 (BEL) gets a BEL reply, and any
    /// other byte an ST reply. If the program cancelled the sequence with CAN
    /// (0x18) or SUB (0x1A), pass that byte: the sequence is discarded and the
    /// command is [`CommandType::Invalid`], whatever it contained.
    pub fn end<'p>(&'p mut self, terminator: u8) -> Command<'p, 'alloc> {
        // NULL is an invalid or cancelled sequence, not a failure.
        let raw = unsafe { ffi::ghostty_osc_end(self.0.as_raw(), terminator) };
        Command {
            inner: Object::new(raw).ok(),
            _parser: PhantomData,
        }
    }
}

impl Drop for Parser<'_> {
    fn drop(&mut self) {
        unsafe { ffi::ghostty_osc_free(self.0.as_raw()) }
    }
}

/// A parsed OSC (Operating System Command) command.
///
/// The command can be queried for its type and associated data.
#[derive(Debug)]
pub struct Command<'p, 'alloc> {
    inner: Option<Object<'alloc, ffi::OscCommandImpl>>,
    _parser: PhantomData<&'p Parser<'alloc>>,
}

impl<'p> Command<'p, '_> {
    /// Get the type of an OSC command.
    ///
    /// This can be used to determine what kind of command was parsed and
    /// what data might be available from it.
    #[must_use]
    pub fn command_type(self) -> CommandType<'p> {
        self.command_type_inner().unwrap_or(CommandType::Invalid)
    }

    fn command_type_inner(&self) -> Option<CommandType<'p>> {
        use ffi::OscCommandData as Data;
        use ffi::OscCommandType as Type;

        let raw_type = unsafe { ffi::ghostty_osc_command_type(self.inner.as_ref()?.as_raw()) };
        Some(match raw_type {
            Type::CHANGE_WINDOW_TITLE => CommandType::ChangeWindowTitle {
                title: self.c_str(Data::CHANGE_WINDOW_TITLE_STR)?,
            },
            Type::CHANGE_WINDOW_ICON => CommandType::ChangeWindowIcon,
            Type::SEMANTIC_PROMPT => CommandType::SemanticPrompt,
            Type::CLIPBOARD_CONTENTS => CommandType::ClipboardContents,
            Type::REPORT_PWD => CommandType::ReportPwd,
            Type::MOUSE_SHAPE => CommandType::MouseShape,
            Type::COLOR_OPERATION => CommandType::ColorOperation,
            Type::KITTY_COLOR_PROTOCOL => CommandType::KittyColorProtocol,
            Type::SHOW_DESKTOP_NOTIFICATION => CommandType::ShowDesktopNotification,
            Type::HYPERLINK_START => CommandType::HyperlinkStart,
            Type::HYPERLINK_END => CommandType::HyperlinkEnd,
            Type::CONEMU_SLEEP => CommandType::ConemuSleep,
            Type::CONEMU_SHOW_MESSAGE_BOX => CommandType::ConemuShowMessageBox,
            Type::CONEMU_CHANGE_TAB_TITLE => CommandType::ConemuChangeTabTitle,
            Type::CONEMU_PROGRESS_REPORT => CommandType::ConemuProgressReport,
            Type::CONEMU_WAIT_INPUT => CommandType::ConemuWaitInput,
            Type::CONEMU_GUIMACRO => CommandType::ConemuGuiMacro,
            Type::CONEMU_RUN_PROCESS => CommandType::ConemuRunProcess,
            Type::CONEMU_OUTPUT_ENVIRONMENT_VARIABLE => {
                CommandType::ConemuOutputEnvironmentVariable
            }
            Type::CONEMU_XTERM_EMULATION => CommandType::ConemuXtermEmulation,
            Type::CONEMU_COMMENT => CommandType::ConemuComment,
            Type::KITTY_TEXT_SIZING => CommandType::KittyTextSizing,

            _ => return None,
        })
    }

    fn get<T>(&self, tag: ffi::OscCommandData::Type) -> Option<T> {
        let inner = self.inner.as_ref()?;
        let mut value = MaybeUninit::<T>::zeroed();
        let result = unsafe {
            ffi::ghostty_osc_command_data(inner.as_raw(), tag, value.as_mut_ptr().cast())
        };

        if result {
            // SAFETY: Value should be initialized after successful call.
            Some(unsafe { value.assume_init() })
        } else {
            None
        }
    }

    /// A string the command owns, which C hands out as `const char *`.
    fn c_str(&self, tag: ffi::OscCommandData::Type) -> Option<&'p str> {
        let ptr: *const std::ffi::c_char = self.get(tag)?;
        if ptr.is_null() {
            return None;
        }
        // SAFETY: the parser owns the null-terminated string until its next
        // call, and `'p` borrows the parser for as long as the command lives.
        unsafe { std::ffi::CStr::from_ptr(ptr) }.to_str().ok()
    }
}

/// Type of an OSC command.
#[repr(i32)]
#[derive(Debug, Clone, Default)]
#[expect(missing_docs, reason = "missing upstream docs")]
pub enum CommandType<'p> {
    #[default]
    Invalid,
    ChangeWindowTitle {
        /// Window title string data.
        title: &'p str,
    },
    ChangeWindowIcon,
    SemanticPrompt,
    ClipboardContents,
    ReportPwd,
    MouseShape,
    ColorOperation,
    KittyColorProtocol,
    ShowDesktopNotification,
    HyperlinkStart,
    HyperlinkEnd,
    ConemuSleep,
    ConemuShowMessageBox,
    ConemuChangeTabTitle,
    ConemuProgressReport,
    ConemuWaitInput,
    ConemuGuiMacro,
    ConemuRunProcess,
    ConemuOutputEnvironmentVariable,
    ConemuXtermEmulation,
    ConemuComment,
    KittyTextSizing,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(bytes: &[u8], terminator: u8, check: impl FnOnce(CommandType<'_>)) {
        let mut parser = Parser::new().expect("parser");
        for &byte in bytes {
            parser.next_byte(byte);
        }
        check(parser.end(terminator).command_type());
    }

    #[test]
    fn a_title_ended_by_bel_is_read() {
        parse(b"0;title", 0x07, |c| {
            assert!(
                matches!(c, CommandType::ChangeWindowTitle { title: "title" }),
                "{c:?}"
            );
        });
    }

    #[test]
    fn invalid_and_cancelled_sequences_are_invalid_commands() {
        for (bytes, terminator) in [(&b""[..], 0x07), (b"0;title", 0x18), (b"0;title", 0x1a)] {
            parse(bytes, terminator, |c| {
                assert!(matches!(c, CommandType::Invalid), "{c:?}")
            });
        }
    }
}
