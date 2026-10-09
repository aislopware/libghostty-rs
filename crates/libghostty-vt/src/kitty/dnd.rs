//! Drops onto the terminal through the
//! [Kitty drag and drop protocol](https://sw.kovidgoyal.net/kitty/dnd-protocol/)
//! (OSC 72).
//!
//! A program registers to accept drops. Native drags over the terminal are
//! then forwarded to it instead of being handled the traditional way
//! (pasting the dropped paths or text), and it requests the dropped data it
//! wants. libghostty-vt implements the protocol; the embedder connects it to
//! the platform's drag and drop:
//!
//!   1. Install [`Terminal::on_kitty_dnd`]: until then OSC 72 is ignored and
//!      programs fall back to their behavior without the protocol. The
//!      program registering yields [`Event::Registration`]; while
//!      [`Terminal::dnd_drop_registered`] is true, forward native drags over
//!      the terminal.
//!   2. As a native drag moves over the terminal, call
//!      [`Terminal::dnd_drop_move`]; when it leaves,
//!      [`Terminal::dnd_drop_leave`].
//!   3. The program answers with the operation it accepts
//!      ([`Event::Acceptance`], [`Terminal::dnd_drop_accepted`]), for the
//!      platform's drag feedback.
//!   4. On drop, call [`Terminal::dnd_drop`] and keep the native drop open:
//!      its data is read on demand.
//!   5. The program requests data ([`Event::DataRequest`]). Read the request
//!      with [`Terminal::dnd_drop_request`] and answer it, at once or later,
//!      with [`Terminal::dnd_drop_respond_data`] and
//!      [`Terminal::dnd_drop_respond_end`], or
//!      [`Terminal::dnd_drop_respond_error`]. Requests are served one at a
//!      time: after each answer, check for the next.
//!   6. The program concludes the drop ([`Event::Concluded`]): finish the
//!      native drop with that operation.
//!
//! Every program is treated as running on the same machine as the drop:
//! files reach it as `file://` URLs it opens itself, never through the
//! protocol's remote file transfer.
//!
//! The functions here may not be called from within the callback, which
//! only gets a shared reference to the terminal; record the event and act
//! on it once [`Terminal::vt_write`] returns. Answering a request writes to
//! the pty through the [pty write callback](Terminal::on_pty_write).
//!
//! Drags offered by the program (the protocol's other direction) are not
//! bound yet: their events arrive as [`Event::Offers`] and the like, and are
//! best ignored.

use crate::{
    error::{Result, from_optional_result, from_result},
    ffi,
    terminal::Terminal,
};

/// A drag and drop state change, delivered to [`Terminal::on_kitty_dnd`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Event {
    /// The program registered or unregistered to accept drops.
    Registration,
    /// The program answered the drag over the terminal.
    Acceptance,
    /// A drop data request needs serving.
    DataRequest,
    /// The program concluded the drop with this operation.
    Concluded(Operation),
    /// The program enabled or disabled offering drags.
    Offers,
    /// The program asked to start its offered drag.
    DragStart,
    /// The program changed the image of the started drag.
    DragImage,
    /// Requested drag data arrived or failed.
    DragData,
    /// The native drag in progress must be canceled.
    DragCancel,
}

impl Event {
    pub(crate) fn from_raw(raw: ffi::KittyDndEvent::Type) -> Option<Self> {
        Some(match raw {
            ffi::KittyDndEvent::REGISTRATION => Self::Registration,
            ffi::KittyDndEvent::ACCEPTANCE => Self::Acceptance,
            ffi::KittyDndEvent::DATA_REQUEST => Self::DataRequest,
            ffi::KittyDndEvent::CONCLUDED_NONE => Self::Concluded(Operation::None),
            ffi::KittyDndEvent::CONCLUDED_COPY => Self::Concluded(Operation::Copy),
            ffi::KittyDndEvent::CONCLUDED_MOVE => Self::Concluded(Operation::Move),
            ffi::KittyDndEvent::OFFERS => Self::Offers,
            ffi::KittyDndEvent::DRAG_START => Self::DragStart,
            ffi::KittyDndEvent::DRAG_IMAGE => Self::DragImage,
            ffi::KittyDndEvent::DRAG_DATA => Self::DragData,
            ffi::KittyDndEvent::DRAG_CANCEL => Self::DragCancel,
            _ => return None,
        })
    }
}

/// A drag and drop operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, int_enum::IntEnum)]
#[repr(i32)]
pub enum Operation {
    /// Nothing: not accepted, or canceled.
    None = ffi::KittyDndOperation::NONE,
    /// Copy the data.
    Copy = ffi::KittyDndOperation::COPY,
    /// Move the data.
    Move = ffi::KittyDndOperation::MOVE,
}

/// The operations a native drag allows, as a bitmask.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Operations(u32);

impl Operations {
    /// No operation.
    pub const NONE: Self = Self(0);
    /// Copying.
    pub const COPY: Self = Self(1);
    /// Moving.
    pub const MOVE: Self = Self(2);
    /// Either.
    pub const ANY: Self = Self(3);

    /// The raw bitmask.
    #[must_use]
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// Both sets together.
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

/// A POSIX error name the protocol answers a failed request with.
#[derive(Clone, Copy, Debug, PartialEq, Eq, int_enum::IntEnum)]
#[repr(i32)]
#[non_exhaustive]
pub enum Errno {
    /// Not permitted.
    Eperm = ffi::KittyDndErrno::EPERM,
    /// No such file or entry.
    Enoent = ffi::KittyDndErrno::ENOENT,
    /// An I/O error.
    Eio = ffi::KittyDndErrno::EIO,
    /// An invalid request.
    Einval = ffi::KittyDndErrno::EINVAL,
    /// Too many requests.
    Emfile = ffi::KittyDndErrno::EMFILE,
    /// Out of memory.
    Enomem = ffi::KittyDndErrno::ENOMEM,
    /// Too large.
    Efbig = ffi::KittyDndErrno::EFBIG,
    /// A directory.
    Eisdir = ffi::KittyDndErrno::EISDIR,
    /// Out of space.
    Enospc = ffi::KittyDndErrno::ENOSPC,
    /// Any other error.
    Eunknown = ffi::KittyDndErrno::EUNKNOWN,
}

/// Where a native drag is over the terminal, and what it allows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Position {
    /// Grid cell column, zero-based from the left.
    pub cell_x: u32,
    /// Grid cell row, zero-based from the top.
    pub cell_y: u32,
    /// Pixels from the left of the terminal's content area.
    pub pixel_x: i32,
    /// Pixels from the top of the terminal's content area.
    pub pixel_y: i32,
    /// The operations the native drag allows.
    pub operations: Operations,
}

impl Position {
    fn to_raw(self) -> ffi::KittyDndPosition {
        ffi::KittyDndPosition {
            cell_x: self.cell_x,
            cell_y: self.cell_y,
            pixel_x: self.pixel_x,
            pixel_y: self.pixel_y,
            operations: self.operations.0,
            ..ffi::sized!(ffi::KittyDndPosition)
        }
    }
}

/// A drop data request to serve.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DropRequest<'t> {
    /// Identifies the request when answering it. Requests are never reused,
    /// so an answer to a request the program abandoned is rejected rather
    /// than answering another.
    pub id: u32,
    /// Zero-based index into the MIME types given to [`Terminal::dnd_drop`].
    pub mime_index: u32,
    /// The MIME type to read from the native drop, as given there.
    pub mime: &'t [u8],
}

/// Drops onto the terminal.
impl Terminal<'_, '_> {
    fn dnd_get<T: Default>(&self, data: ffi::KittyDndData::Type) -> Result<Option<T>> {
        let mut value = T::default();
        // SAFETY: `value` has the output type the C API documents for `data`.
        let result = unsafe {
            ffi::ghostty_kitty_dnd_get(self.inner.as_raw(), data, (&raw mut value).cast())
        };
        from_optional_result(result, value)
    }

    fn dnd_string(&self, data: ffi::KittyDndData::Type) -> Result<Option<&[u8]>> {
        let mut value = ffi::String {
            ptr: std::ptr::null(),
            len: 0,
        };
        // SAFETY: a string output, as documented for `data`.
        let result = unsafe {
            ffi::ghostty_kitty_dnd_get(self.inner.as_raw(), data, (&raw mut value).cast())
        };
        Ok(from_optional_result(result, value)?
            // SAFETY: borrowed from the terminal until the next call that
            // takes it mutably.
            .map(|s| unsafe { s.to_bytes() }))
    }

    /// Whether the program is registered to accept drops: while it is,
    /// native drags over the terminal go to [`Self::dnd_drop_move`].
    pub fn dnd_drop_registered(&self) -> Result<bool> {
        Ok(self
            .dnd_get::<bool>(ffi::KittyDndData::DROP_REGISTERED)?
            .unwrap_or(false))
    }

    /// The MIME types the program registered with, space-separated and
    /// usually empty; `None` when not registered.
    pub fn dnd_drop_registered_mimes(&self) -> Result<Option<&[u8]>> {
        self.dnd_string(ffi::KittyDndData::DROP_REGISTERED_MIMES)
    }

    /// The operation the program accepts for the drag over the terminal;
    /// `None` until it answered.
    pub fn dnd_drop_accepted(&self) -> Result<Option<Operation>> {
        let raw = self.dnd_get::<ffi::KittyDndOperation::Type>(ffi::KittyDndData::DROP_ACCEPTED)?;
        Ok(raw.and_then(|raw| Operation::try_from(raw).ok()))
    }

    /// The MIME types the program accepts for the drag over the terminal,
    /// most preferred first, each followed by a NUL byte; empty when it
    /// didn't say, `None` until it answered.
    pub fn dnd_drop_accepted_mimes(&self) -> Result<Option<&[u8]>> {
        self.dnd_string(ffi::KittyDndData::DROP_ACCEPTED_MIMES)
    }

    /// The drop data request to serve, if any.
    pub fn dnd_drop_request(&self) -> Result<Option<DropRequest<'_>>> {
        let mut value = ffi::sized!(ffi::KittyDndDataRequest);
        // SAFETY: a sized request struct, as documented for DROP_REQUEST.
        let result = unsafe {
            ffi::ghostty_kitty_dnd_get(
                self.inner.as_raw(),
                ffi::KittyDndData::DROP_REQUEST,
                (&raw mut value).cast(),
            )
        };
        Ok(from_optional_result(result, value)?.map(|raw| DropRequest {
            id: raw.id,
            mime_index: raw.mime_index,
            // SAFETY: borrowed from the terminal until the next call that
            // takes it mutably.
            mime: unsafe { raw.mime.to_bytes() },
        }))
    }

    /// A native drag of `mimes` moves over the terminal at `position`.
    /// `None` when the program isn't registered to accept drops; otherwise
    /// whether the drag entering discarded an unconcluded previous drop,
    /// which must then be finished natively with no operation.
    pub fn dnd_drop_move(&mut self, position: Position, mimes: &[&str]) -> Result<Option<bool>> {
        let raw = position.to_raw();
        let mimes: Vec<ffi::String> = mimes.iter().map(|m| ffi::String::from(*m)).collect();
        let mut discarded = false;
        // SAFETY: the position, the MIME types and the flag outlive this
        // synchronous call.
        let result = unsafe {
            ffi::ghostty_kitty_dnd_drop_move(
                self.inner.as_raw(),
                &raw const raw,
                mimes.as_ptr(),
                mimes.len(),
                &raw mut discarded,
            )
        };
        from_optional_result(result, ())?;
        Ok((result == ffi::Result::SUCCESS).then_some(discarded))
    }

    /// The native drag left the terminal without dropping. `false` when the
    /// program isn't registered to accept drops.
    pub fn dnd_drop_leave(&mut self) -> Result<bool> {
        // SAFETY: a live terminal.
        let result = unsafe { ffi::ghostty_kitty_dnd_drop_leave(self.inner.as_raw()) };
        Ok(from_optional_result(result, ())?.is_some())
    }

    /// The native drag of `mimes` dropped onto the terminal at `position`.
    /// Its data is requested on demand, so keep it until the program
    /// concludes the drop. As for [`Self::dnd_drop_move`].
    pub fn dnd_drop(&mut self, position: Position, mimes: &[&str]) -> Result<Option<bool>> {
        let raw = position.to_raw();
        let mimes: Vec<ffi::String> = mimes.iter().map(|m| ffi::String::from(*m)).collect();
        let mut discarded = false;
        // SAFETY: as for `dnd_drop_move`.
        let result = unsafe {
            ffi::ghostty_kitty_dnd_drop(
                self.inner.as_raw(),
                &raw const raw,
                mimes.as_ptr(),
                mimes.len(),
                &raw mut discarded,
            )
        };
        from_optional_result(result, ())?;
        Ok((result == ffi::Result::SUCCESS).then_some(discarded))
    }

    /// Send some of the data of request `id`, as the native drop delivers
    /// it. [`Error::Rejected`](crate::Error::Rejected) when `id` is not the
    /// request being served.
    pub fn dnd_drop_respond_data(&mut self, id: u32, data: &[u8]) -> Result<()> {
        // SAFETY: the data outlives this synchronous call.
        let result = unsafe {
            ffi::ghostty_kitty_dnd_drop_respond_data(
                self.inner.as_raw(),
                id,
                data.as_ptr(),
                data.len(),
            )
        };
        from_result(result)
    }

    /// Finish request `id`; the next request, if any, is then
    /// [`Self::dnd_drop_request`].
    pub fn dnd_drop_respond_end(&mut self, id: u32) -> Result<()> {
        // SAFETY: a live terminal.
        let result = unsafe { ffi::ghostty_kitty_dnd_drop_respond_end(self.inner.as_raw(), id) };
        from_result(result)
    }

    /// Fail request `id` with `error`; the next request, if any, is then
    /// [`Self::dnd_drop_request`].
    pub fn dnd_drop_respond_error(&mut self, id: u32, error: Errno) -> Result<()> {
        // SAFETY: a live terminal.
        let result = unsafe {
            ffi::ghostty_kitty_dnd_drop_respond_error(self.inner.as_raw(), id, error.into())
        };
        from_result(result)
    }
}

#[cfg(all(test, not(miri)))]
mod tests;
