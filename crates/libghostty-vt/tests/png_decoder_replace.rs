//! `replace_png_decoder` needs no `unsafe`: libghostty's hook points at this
//! crate's decoder from the first terminal on, so a decoder set or cleared on
//! another thread, while terminals exist, is the one they decode with. The
//! decoder hook is process-global, so this lives in its own test binary.
// These call into libghostty, which Miri cannot execute.
#![cfg(all(feature = "kitty-graphics", not(miri)))]

use std::{cell::RefCell, rc::Rc};

use libghostty_vt::{
    Terminal,
    alloc::{Allocator, Bytes},
    kitty::graphics::{self, DecodePng, DecodedImage},
};

/// Returns a 1x1 red pixel, whatever it is sent.
struct OnePixel;

impl DecodePng for OnePixel {
    fn decode_png<'alloc>(
        &mut self,
        alloc: &'alloc Allocator<'_>,
        _data: &[u8],
    ) -> Option<DecodedImage<'alloc>> {
        let mut data = Bytes::new_with_alloc(alloc, 4).ok()?;
        data.copy_from_slice(&[255, 0, 0, 255]);
        Some(DecodedImage {
            width: 1,
            height: 1,
            data,
        })
    }
}

/// A terminal that takes Kitty images, and what it writes back.
fn kitty_terminal() -> (Terminal<'static, 'static>, Rc<RefCell<Vec<u8>>>) {
    let replies = Rc::new(RefCell::new(Vec::new()));
    let mut terminal = Terminal::new(80, 24).unwrap();
    terminal.resize(80, 24, 8, 16).unwrap();
    terminal
        .set_kitty_image_storage_limit(64 * 1024 * 1024)
        .unwrap();
    let sink = Rc::clone(&replies);
    terminal
        .on_pty_write(move |_, data| sink.borrow_mut().extend_from_slice(data))
        .unwrap();
    (terminal, replies)
}

/// Transmit a 1x1 PNG as image `id` and return the terminal's reply.
fn transmit(
    terminal: &mut Terminal<'static, 'static>,
    replies: &RefCell<Vec<u8>>,
    id: u32,
) -> String {
    replies.borrow_mut().clear();
    terminal.vt_write(format!("\x1b_Ga=t,f=100,i={id};").as_bytes());
    terminal.vt_write(
        b"iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAA\
          DUlEQVR4nGP4z8DwHwAFAAH/iZk9HQAAAABJRU5ErkJggg==\
          \x1b\\",
    );
    String::from_utf8_lossy(&replies.borrow()).into_owned()
}

#[test]
fn a_decoder_replaced_on_another_thread_decodes_for_terminals_made_before() {
    let (mut terminal, replies) = kitty_terminal();

    // No decoder yet. The hook is this crate's already, so PNG is on and
    // the image fails to decode, rather than PNG being off.
    let reply = transmit(&mut terminal, &replies, 1);
    assert!(reply.contains("i=1;EINVAL: invalid data"), "{reply:?}");

    std::thread::spawn(|| graphics::replace_png_decoder(Some(Box::new(OnePixel))))
        .join()
        .unwrap();
    let reply = transmit(&mut terminal, &replies, 2);
    assert!(reply.contains("i=2;OK"), "{reply:?}");

    // A terminal made after the decoder decodes with it too.
    let (mut later, later_replies) = kitty_terminal();
    let reply = transmit(&mut later, &later_replies, 3);
    assert!(reply.contains("i=3;OK"), "{reply:?}");

    // Cleared again, an image fails to decode once more.
    graphics::replace_png_decoder(None);
    let reply = transmit(&mut terminal, &replies, 4);
    assert!(reply.contains("i=4;EINVAL: invalid data"), "{reply:?}");
}
