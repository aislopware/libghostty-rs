use std::cell::RefCell;

use super::*;

/// A terminal recording what it writes to the pty and the drag and drop
/// events it reports.
fn terminal<'a>(
    output: &'a RefCell<Vec<u8>>,
    events: &'a RefCell<Vec<Event>>,
) -> Terminal<'static, 'a> {
    let mut terminal = Terminal::new(80, 24).unwrap();
    terminal
        .on_pty_write(|_term, bytes| output.borrow_mut().extend_from_slice(bytes))
        .unwrap();
    terminal
        .on_kitty_dnd(|_term, event| events.borrow_mut().push(event))
        .unwrap();
    terminal
}

fn written(output: &RefCell<Vec<u8>>) -> String {
    String::from_utf8(std::mem::take(&mut *output.borrow_mut())).unwrap()
}

const AT: Position = Position {
    cell_x: 4,
    cell_y: 2,
    pixel_x: 40,
    pixel_y: 20,
    operations: Operations::COPY,
};

#[test]
fn nothing_goes_to_a_program_that_did_not_register() {
    let output = RefCell::new(Vec::new());
    let events = RefCell::new(Vec::new());
    let mut terminal = terminal(&output, &events);
    assert!(!terminal.dnd_drop_registered().unwrap());
    assert_eq!(terminal.dnd_drop_move(AT, &["text/plain"]).unwrap(), None);
    assert_eq!(terminal.dnd_drop(AT, &["text/plain"]).unwrap(), None);
    assert!(!terminal.dnd_drop_leave().unwrap());
    assert!(output.borrow().is_empty());
}

/// A program registers, hears the drag, accepts it, gets the drop, asks for
/// a MIME type, is answered, and concludes.
#[test]
fn a_drop_goes_to_the_program_that_asked_for_it() {
    let output = RefCell::new(Vec::new());
    let events = RefCell::new(Vec::new());
    let mut terminal = terminal(&output, &events);

    terminal.vt_write(b"\x1b]72;t=a;text/plain text/uri-list\x1b\\");
    assert_eq!(*events.borrow(), [Event::Registration]);
    assert!(terminal.dnd_drop_registered().unwrap());
    assert_eq!(
        terminal.dnd_drop_registered_mimes().unwrap(),
        Some(&b"text/plain text/uri-list"[..])
    );

    let mimes = ["text/uri-list", "text/plain"];
    assert_eq!(terminal.dnd_drop_move(AT, &mimes).unwrap(), Some(false));
    assert_eq!(
        written(&output),
        "\x1b]72;t=m:x=4:y=2:X=40:Y=20:o=1:m=0;text/uri-list text/plain \x1b\\"
    );
    assert_eq!(terminal.dnd_drop_accepted().unwrap(), None);
    terminal.vt_write(b"\x1b]72;t=m:o=1;text/uri-list\x1b\\");
    assert_eq!(events.borrow().last(), Some(&Event::Acceptance));
    assert_eq!(terminal.dnd_drop_accepted().unwrap(), Some(Operation::Copy));
    assert_eq!(terminal.dnd_drop_accepted_mimes().unwrap(), Some(&b"text/uri-list\0"[..]));

    assert_eq!(terminal.dnd_drop(AT, &mimes).unwrap(), Some(false));
    assert!(written(&output).starts_with("\x1b]72;t=M:x=4:y=2"));
    assert_eq!(terminal.dnd_drop_request().unwrap(), None);
    terminal.vt_write(b"\x1b]72;t=r:x=1\x1b\\");
    assert_eq!(events.borrow().last(), Some(&Event::DataRequest));
    let request = terminal.dnd_drop_request().unwrap().unwrap();
    assert_eq!((request.mime_index, request.mime), (0, &b"text/uri-list"[..]));
    let id = request.id;
    terminal.dnd_drop_respond_data(id, b"file:///tmp/a\r\n").unwrap();
    terminal.dnd_drop_respond_end(id).unwrap();
    // "file:///tmp/a\r\n"
    assert_eq!(
        written(&output),
        "\x1b]72;t=r:x=1:m=0;ZmlsZTovLy90bXAvYQ0K\x1b\\\x1b]72;t=r:x=1\x1b\\"
    );
    assert!(terminal.dnd_drop_respond_end(id).is_err(), "a request is answered once");

    terminal.vt_write(b"\x1b]72;t=r:x=2\x1b\\");
    let request = terminal.dnd_drop_request().unwrap().unwrap();
    terminal.dnd_drop_respond_error(request.id, Errno::Eio).unwrap();
    let failed = written(&output);
    assert!(failed.contains("t=R:x=2") && failed.contains("EIO"), "{failed:?}");

    terminal.vt_write(b"\x1b]72;t=r:o=1\x1b\\");
    assert_eq!(events.borrow().last(), Some(&Event::Concluded(Operation::Copy)));
    terminal.vt_write(b"\x1b]72;t=A\x1b\\");
    assert!(!terminal.dnd_drop_registered().unwrap());
}

/// A drag that leaves tells the program so.
#[test]
fn a_drag_leaving_is_told() {
    let output = RefCell::new(Vec::new());
    let events = RefCell::new(Vec::new());
    let mut terminal = terminal(&output, &events);
    terminal.vt_write(b"\x1b]72;t=a\x1b\\");
    terminal.dnd_drop_move(AT, &["text/plain"]).unwrap();
    let _ = written(&output);
    assert!(terminal.dnd_drop_leave().unwrap());
    assert_eq!(written(&output), "\x1b]72;t=m:x=-1:y=-1\x1b\\");
}
