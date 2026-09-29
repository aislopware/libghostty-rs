//! The linked library's type manifest ([`ffi::ghostty_type_json`]).
//!
//! The manifest describes every public C type of the linked build: sizes,
//! enum values and the bit positions of packed values. It is JSON, and only a
//! few of its entries are read here, so this is a small parser for the JSON
//! the library writes rather than a general one.

use std::ffi::CStr;

use crate::ffi;

/// The manifest schema this binding reads.
const SCHEMA: u64 = 1;

/// A parsed JSON value.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Value {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<Value>),
    Object(Vec<(String, Value)>),
}

impl Value {
    /// The member `key` of an object.
    pub(crate) fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Self::Object(members) => members.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// The value at `path`, one object key per step.
    pub(crate) fn at(&self, path: &[&str]) -> Option<&Value> {
        path.iter().try_fold(self, |v, key| v.get(key))
    }

    /// A number that is a whole, non-negative integer.
    pub(crate) fn as_u64(&self) -> Option<u64> {
        match *self {
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "checked to be a whole number within range just before"
            )]
            Self::Number(n) if n >= 0.0 && n.fract() == 0.0 && n < 2f64.powi(53) => Some(n as u64),
            _ => None,
        }
    }

    /// A string.
    pub(crate) fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(s) => Some(s),
            _ => None,
        }
    }
}

/// The linked build's manifest, if it is one this binding can read: valid
/// JSON of the schema it knows, for a little-endian target.
pub(crate) fn linked() -> Option<Value> {
    // SAFETY: The manifest is a NUL-terminated string that lives for the
    // lifetime of the process.
    let raw = unsafe { CStr::from_ptr(ffi::ghostty_type_json()) };
    let manifest = parse(raw.to_str().ok()?)?;
    let known = manifest.get("schema").and_then(Value::as_u64) == Some(SCHEMA)
        && manifest.at(&["abi", "endian"]).and_then(Value::as_str) == Some("little");
    known.then_some(manifest)
}

/// Parse a JSON document, `None` if it is not valid JSON.
pub(crate) fn parse(text: &str) -> Option<Value> {
    let mut parser = Parser {
        bytes: text.as_bytes(),
        at: 0,
    };
    let value = parser.value(0)?;
    parser.whitespace();
    (parser.at == parser.bytes.len()).then_some(value)
}

/// Deeper nesting than any manifest has is refused rather than recursed into.
const MAX_DEPTH: usize = 32;

struct Parser<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.at).copied()
    }

    fn bump(&mut self) -> Option<u8> {
        let b = self.peek()?;
        self.at += 1;
        Some(b)
    }

    fn whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.at += 1;
        }
    }

    fn literal(&mut self, word: &[u8], value: Value) -> Option<Value> {
        let end = self.at.checked_add(word.len())?;
        (self.bytes.get(self.at..end)? == word).then(|| {
            self.at = end;
            value
        })
    }

    fn value(&mut self, depth: usize) -> Option<Value> {
        if depth > MAX_DEPTH {
            return None;
        }
        self.whitespace();
        match self.peek()? {
            b'{' => self.object(depth),
            b'[' => self.array(depth),
            b'"' => self.string().map(Value::String),
            b't' => self.literal(b"true", Value::Bool(true)),
            b'f' => self.literal(b"false", Value::Bool(false)),
            b'n' => self.literal(b"null", Value::Null),
            _ => self.number(),
        }
    }

    fn object(&mut self, depth: usize) -> Option<Value> {
        self.bump();
        let mut members = Vec::new();
        self.whitespace();
        if self.peek()? == b'}' {
            self.bump();
            return Some(Value::Object(members));
        }
        loop {
            self.whitespace();
            let key = self.string()?;
            self.whitespace();
            (self.bump()? == b':').then_some(())?;
            members.push((key, self.value(depth + 1)?));
            self.whitespace();
            match self.bump()? {
                b',' => {}
                b'}' => return Some(Value::Object(members)),
                _ => return None,
            }
        }
    }

    fn array(&mut self, depth: usize) -> Option<Value> {
        self.bump();
        let mut items = Vec::new();
        self.whitespace();
        if self.peek()? == b']' {
            self.bump();
            return Some(Value::Array(items));
        }
        loop {
            items.push(self.value(depth + 1)?);
            self.whitespace();
            match self.bump()? {
                b',' => {}
                b']' => return Some(Value::Array(items)),
                _ => return None,
            }
        }
    }

    fn string(&mut self) -> Option<String> {
        (self.bump()? == b'"').then_some(())?;
        let mut out = String::new();
        loop {
            let start = self.at;
            while !matches!(self.peek()?, b'"' | b'\\') {
                self.at += 1;
            }
            out.push_str(std::str::from_utf8(self.bytes.get(start..self.at)?).ok()?);
            if self.bump()? == b'"' {
                return Some(out);
            }
            let escaped = match self.bump()? {
                b'"' => '"',
                b'\\' => '\\',
                b'/' => '/',
                b'b' => '\u{8}',
                b'f' => '\u{c}',
                b'n' => '\n',
                b'r' => '\r',
                b't' => '\t',
                b'u' => self.unicode_escape()?,
                _ => return None,
            };
            out.push(escaped);
        }
    }

    fn hex4(&mut self) -> Option<u32> {
        let end = self.at.checked_add(4)?;
        let digits = std::str::from_utf8(self.bytes.get(self.at..end)?).ok()?;
        self.at = end;
        u32::from_str_radix(digits, 16).ok()
    }

    fn unicode_escape(&mut self) -> Option<char> {
        let high = self.hex4()?;
        if !(0xd800..0xdc00).contains(&high) {
            return char::from_u32(high);
        }
        (self.bump()? == b'\\' && self.bump()? == b'u').then_some(())?;
        let low = self.hex4()?;
        (0xdc00..0xe000).contains(&low).then_some(())?;
        char::from_u32(0x10000 + ((high - 0xd800) << 10) + (low - 0xdc00))
    }

    fn number(&mut self) -> Option<Value> {
        let start = self.at;
        while matches!(
            self.peek(),
            Some(b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9')
        ) {
            self.at += 1;
        }
        let text = std::str::from_utf8(self.bytes.get(start..self.at)?).ok()?;
        text.parse().ok().map(Value::Number)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_json_the_manifest_is_made_of() {
        let v = parse(r#" {"a": [1, 2.5, -3e2, true, false, null], "b": {"c": "d\"\\\u00e9\ud83d\ude00"}, "e": {}} "#)
            .expect("valid JSON");
        assert_eq!(
            v.get("a"),
            Some(&Value::Array(vec![
                Value::Number(1.0),
                Value::Number(2.5),
                Value::Number(-300.0),
                Value::Bool(true),
                Value::Bool(false),
                Value::Null,
            ]))
        );
        assert_eq!(v.at(&["b", "c"]).and_then(Value::as_str), Some("d\"\\é😀"));
        assert_eq!(v.get("e"), Some(&Value::Object(Vec::new())));
        assert_eq!(v.at(&["a", "x"]), None);
    }

    #[test]
    fn refuses_what_is_not_json() {
        for text in [
            "",
            "{",
            "[1,]",
            "{\"a\" 1}",
            "{\"a\":1} x",
            "tru",
            "\"\\q\"",
            "\"\\ud800\"",
            "{\"a\":1,}",
        ] {
            assert_eq!(parse(text), None, "{text:?}");
        }
        let deep = "[".repeat(MAX_DEPTH + 2) + &"]".repeat(MAX_DEPTH + 2);
        assert_eq!(parse(&deep), None, "nesting past the limit");
    }

    #[test]
    fn whole_numbers_only_read_as_integers() {
        assert_eq!(Value::Number(42.0).as_u64(), Some(42));
        assert_eq!(Value::Number(4.5).as_u64(), None);
        assert_eq!(Value::Number(-1.0).as_u64(), None);
        assert_eq!(Value::String("1".into()).as_u64(), None);
    }

    #[test]
    fn the_linked_manifest_is_readable() {
        let manifest = linked().expect("the linked manifest has a schema this binding reads");
        assert!(manifest.at(&["types", "GhosttyCell"]).is_some());
    }
}
