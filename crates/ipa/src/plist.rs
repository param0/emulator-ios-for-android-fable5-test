//! Property-list parser.
//!
//! Handles the two encodings found in real IPAs:
//!   * Apple binary plist v0 (`bplist00`) — the common case for `Info.plist`,
//!     parsed here from scratch (no external crate), and
//!   * a pragmatic subset of the XML `<plist>` DTD, enough to read the string /
//!     integer / bool / dict / array keys the app manager cares about.
//!
//! The parser is defensive: it validates the trailer, object count, and every
//! reference against the buffer length, since `Info.plist` is untrusted input.

use std::collections::BTreeMap;
use std::fmt;

/// A decoded plist node. Dates/UIDs are preserved as raw values; the app
/// manager only needs strings/ints/bools/containers.
#[derive(Clone, Debug, PartialEq)]
pub enum PlistValue {
    Bool(bool),
    Integer(i64),
    Real(f64),
    String(String),
    Data(Vec<u8>),
    Array(Vec<PlistValue>),
    Dict(BTreeMap<String, PlistValue>),
}

impl PlistValue {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            PlistValue::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            PlistValue::Integer(i) => Some(*i),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[PlistValue]> {
        match self {
            PlistValue::Array(a) => Some(a),
            _ => None,
        }
    }

    /// Look up a key in a dict node.
    pub fn get(&self, key: &str) -> Option<&PlistValue> {
        match self {
            PlistValue::Dict(m) => m.get(key),
            _ => None,
        }
    }
}

#[derive(Debug)]
pub enum PlistError {
    Truncated,
    BadTrailer,
    BadReference,
    UnsupportedType(u8),
    Xml(String),
    NotAPlist,
}

impl fmt::Display for PlistError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PlistError::Truncated => f.write_str("truncated plist"),
            PlistError::BadTrailer => f.write_str("bad binary plist trailer"),
            PlistError::BadReference => f.write_str("object reference out of range"),
            PlistError::UnsupportedType(t) => write!(f, "unsupported object marker {t:#04x}"),
            PlistError::Xml(m) => write!(f, "xml plist: {m}"),
            PlistError::NotAPlist => f.write_str("not a recognised plist"),
        }
    }
}
impl std::error::Error for PlistError {}

/// Parse either a binary or XML plist, auto-detecting by magic.
pub fn parse(bytes: &[u8]) -> Result<PlistValue, PlistError> {
    if bytes.starts_with(b"bplist00") {
        BinaryPlist::parse(bytes)
    } else if looks_like_xml(bytes) {
        xml::parse(bytes)
    } else {
        Err(PlistError::NotAPlist)
    }
}

fn looks_like_xml(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(256)];
    let s = String::from_utf8_lossy(head);
    s.contains("<plist") || s.contains("<?xml")
}

// ---------------------------------------------------------------------------
// Binary plist (bplist00)
// ---------------------------------------------------------------------------

struct BinaryPlist<'a> {
    buf: &'a [u8],
    ref_size: usize,
    offsets: Vec<usize>,
}

impl<'a> BinaryPlist<'a> {
    fn parse(buf: &'a [u8]) -> Result<PlistValue, PlistError> {
        if buf.len() < 8 + 32 {
            return Err(PlistError::Truncated);
        }
        // Trailer is the final 32 bytes.
        let trailer = &buf[buf.len() - 32..];
        let offset_size = trailer[6] as usize;
        let ref_size = trailer[7] as usize;
        let num_objects = be_u64(&trailer[8..16]) as usize;
        let top_object = be_u64(&trailer[16..24]) as usize;
        let table_offset = be_u64(&trailer[24..32]) as usize;

        if offset_size == 0 || offset_size > 8 || ref_size == 0 || ref_size > 8 {
            return Err(PlistError::BadTrailer);
        }
        let table_end = table_offset
            .checked_add(num_objects.checked_mul(offset_size).ok_or(PlistError::BadTrailer)?)
            .ok_or(PlistError::BadTrailer)?;
        if table_end > buf.len() || top_object >= num_objects {
            return Err(PlistError::BadTrailer);
        }

        // Decode the offset table (each entry is `offset_size` big-endian bytes).
        let mut offsets = Vec::with_capacity(num_objects);
        for i in 0..num_objects {
            let start = table_offset + i * offset_size;
            offsets.push(be_uint(&buf[start..start + offset_size]) as usize);
        }

        let this = BinaryPlist { buf, ref_size, offsets };
        this.object_at(top_object, 0)
    }

    fn read_ref(&self, at: usize) -> Result<usize, PlistError> {
        if at + self.ref_size > self.buf.len() {
            return Err(PlistError::Truncated);
        }
        Ok(be_uint(&self.buf[at..at + self.ref_size]) as usize)
    }

    /// Decode object number `index`. `depth` guards against maliciously cyclic
    /// container references blowing the stack.
    fn object_at(&self, index: usize, depth: usize) -> Result<PlistValue, PlistError> {
        if depth > 128 {
            return Err(PlistError::BadReference);
        }
        let off = *self.offsets.get(index).ok_or(PlistError::BadReference)?;
        let marker = *self.buf.get(off).ok_or(PlistError::Truncated)?;
        let ty = marker >> 4;
        let n = (marker & 0x0f) as usize;

        match ty {
            0x0 => match n {
                0x0 => Err(PlistError::UnsupportedType(marker)), // null
                0x8 => Ok(PlistValue::Bool(false)),
                0x9 => Ok(PlistValue::Bool(true)),
                _ => Err(PlistError::UnsupportedType(marker)),
            },
            0x1 => {
                // Integer: 2^n bytes, big-endian.
                let len = 1usize << n;
                let bytes = self.slice(off + 1, len)?;
                Ok(PlistValue::Integer(be_int(bytes)))
            }
            0x2 => {
                // Real: 2^n bytes IEEE-754 big-endian.
                let len = 1usize << n;
                let bytes = self.slice(off + 1, len)?;
                let v = match len {
                    4 => f32::from_be_bytes(bytes.try_into().unwrap()) as f64,
                    8 => f64::from_be_bytes(bytes.try_into().unwrap()),
                    _ => return Err(PlistError::UnsupportedType(marker)),
                };
                Ok(PlistValue::Real(v))
            }
            0x3 => {
                // Date: 8-byte big-endian f64 seconds since 2001. Surface as Real.
                let bytes = self.slice(off + 1, 8)?;
                Ok(PlistValue::Real(f64::from_be_bytes(bytes.try_into().unwrap())))
            }
            0x4 => {
                let (count, data_off) = self.count_and_offset(off, n)?;
                Ok(PlistValue::Data(self.slice(data_off, count)?.to_vec()))
            }
            0x5 => {
                // ASCII string.
                let (count, data_off) = self.count_and_offset(off, n)?;
                let bytes = self.slice(data_off, count)?;
                Ok(PlistValue::String(String::from_utf8_lossy(bytes).into_owned()))
            }
            0x6 => {
                // UTF-16BE string.
                let (count, data_off) = self.count_and_offset(off, n)?;
                let bytes = self.slice(data_off, count * 2)?;
                let units: Vec<u16> =
                    bytes.chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
                Ok(PlistValue::String(String::from_utf16_lossy(&units)))
            }
            0x8 => {
                // UID: treat as integer.
                let len = n + 1;
                Ok(PlistValue::Integer(be_int(self.slice(off + 1, len)?)))
            }
            0xA => {
                // Array of object refs.
                let (count, refs_off) = self.count_and_offset(off, n)?;
                let mut out = Vec::with_capacity(count);
                for i in 0..count {
                    let r = self.read_ref(refs_off + i * self.ref_size)?;
                    out.push(self.object_at(r, depth + 1)?);
                }
                Ok(PlistValue::Array(out))
            }
            0xD => {
                // Dict: `count` key refs followed by `count` value refs.
                let (count, keys_off) = self.count_and_offset(off, n)?;
                let vals_off = keys_off + count * self.ref_size;
                let mut map = BTreeMap::new();
                for i in 0..count {
                    let kref = self.read_ref(keys_off + i * self.ref_size)?;
                    let vref = self.read_ref(vals_off + i * self.ref_size)?;
                    let key = match self.object_at(kref, depth + 1)? {
                        PlistValue::String(s) => s,
                        _ => return Err(PlistError::BadReference),
                    };
                    map.insert(key, self.object_at(vref, depth + 1)?);
                }
                Ok(PlistValue::Dict(map))
            }
            _ => Err(PlistError::UnsupportedType(marker)),
        }
    }

    /// For container/string markers, the low nibble is the element count unless
    /// it is `0xf`, in which case an integer object follows giving the count.
    /// Returns `(count, offset_of_first_element)`.
    fn count_and_offset(&self, off: usize, n: usize) -> Result<(usize, usize), PlistError> {
        if n != 0x0f {
            return Ok((n, off + 1));
        }
        let size_marker = *self.buf.get(off + 1).ok_or(PlistError::Truncated)?;
        if size_marker >> 4 != 0x1 {
            return Err(PlistError::BadTrailer);
        }
        let int_len = 1usize << (size_marker & 0x0f);
        let count = be_uint(self.slice(off + 2, int_len)?) as usize;
        Ok((count, off + 2 + int_len))
    }

    fn slice(&self, off: usize, len: usize) -> Result<&'a [u8], PlistError> {
        let end = off.checked_add(len).ok_or(PlistError::Truncated)?;
        self.buf.get(off..end).ok_or(PlistError::Truncated)
    }
}

fn be_u64(b: &[u8]) -> u64 {
    let mut arr = [0u8; 8];
    arr.copy_from_slice(&b[..8]);
    u64::from_be_bytes(arr)
}

/// Big-endian unsigned of arbitrary 1..=8 byte width.
fn be_uint(b: &[u8]) -> u64 {
    b.iter().fold(0u64, |acc, &x| (acc << 8) | x as u64)
}

/// Big-endian signed integer; 8-byte values are treated as two's complement,
/// matching CFBinaryPlist which stores negatives as 8-byte.
fn be_int(b: &[u8]) -> i64 {
    if b.len() == 8 {
        be_u64(b) as i64
    } else {
        be_uint(b) as i64
    }
}

// ---------------------------------------------------------------------------
// Minimal XML plist
// ---------------------------------------------------------------------------

mod xml {
    use super::{PlistError, PlistValue};
    use std::collections::BTreeMap;

    /// A tiny recursive-descent reader over the `<plist>` element subset used by
    /// `Info.plist`. Not a general XML parser: it understands the fixed tag
    /// vocabulary (`dict`,`array`,`key`,`string`,`integer`,`real`,`true`,
    /// `false`,`data`) and ignores attributes, comments, and the DOCTYPE.
    pub fn parse(bytes: &[u8]) -> Result<PlistValue, PlistError> {
        let text = std::str::from_utf8(bytes).map_err(|_| PlistError::Xml("not utf-8".into()))?;
        let mut p = Parser { s: text.as_bytes(), i: 0 };
        p.skip_prologue();
        p.expect_open("plist")?;
        let v = p.value()?;
        Ok(v)
    }

    struct Parser<'a> {
        s: &'a [u8],
        i: usize,
    }

    impl<'a> Parser<'a> {
        fn skip_ws(&mut self) {
            while self.i < self.s.len() && self.s[self.i].is_ascii_whitespace() {
                self.i += 1;
            }
        }

        /// Skip `<?xml ...?>`, `<!DOCTYPE ...>` and comments before `<plist>`.
        fn skip_prologue(&mut self) {
            loop {
                self.skip_ws();
                if self.starts_with(b"<?") {
                    self.advance_past(b"?>");
                } else if self.starts_with(b"<!--") {
                    self.advance_past(b"-->");
                } else if self.starts_with(b"<!") {
                    self.advance_past(b">");
                } else {
                    break;
                }
            }
        }

        fn starts_with(&self, tag: &[u8]) -> bool {
            self.s[self.i..].starts_with(tag)
        }

        fn advance_past(&mut self, end: &[u8]) {
            if let Some(pos) = find(&self.s[self.i..], end) {
                self.i += pos + end.len();
            } else {
                self.i = self.s.len();
            }
        }

        fn expect_open(&mut self, name: &str) -> Result<(), PlistError> {
            self.skip_ws();
            let open = format!("<{name}");
            if !self.starts_with(open.as_bytes()) {
                return Err(PlistError::Xml(format!("expected <{name}>")));
            }
            self.advance_past(b">");
            Ok(())
        }

        fn value(&mut self) -> Result<PlistValue, PlistError> {
            self.skip_ws();
            if self.starts_with(b"<dict>") {
                self.parse_dict()
            } else if self.starts_with(b"<array>") {
                self.parse_array()
            } else if self.starts_with(b"<string>") {
                Ok(PlistValue::String(self.text_element("string")?))
            } else if self.starts_with(b"<integer>") {
                let t = self.text_element("integer")?;
                Ok(PlistValue::Integer(t.trim().parse().unwrap_or(0)))
            } else if self.starts_with(b"<real>") {
                let t = self.text_element("real")?;
                Ok(PlistValue::Real(t.trim().parse().unwrap_or(0.0)))
            } else if self.starts_with(b"<true/>") {
                self.i += "<true/>".len();
                Ok(PlistValue::Bool(true))
            } else if self.starts_with(b"<false/>") {
                self.i += "<false/>".len();
                Ok(PlistValue::Bool(false))
            } else if self.starts_with(b"<data>") {
                let t = self.text_element("data")?;
                Ok(PlistValue::Data(t.trim().as_bytes().to_vec())) // base64 left encoded
            } else {
                Err(PlistError::Xml("unexpected element".into()))
            }
        }

        fn parse_dict(&mut self) -> Result<PlistValue, PlistError> {
            self.i += "<dict>".len();
            let mut map = BTreeMap::new();
            loop {
                self.skip_ws();
                if self.starts_with(b"</dict>") {
                    self.i += "</dict>".len();
                    break;
                }
                if !self.starts_with(b"<key>") {
                    return Err(PlistError::Xml("expected <key> in dict".into()));
                }
                let key = self.text_element("key")?;
                let val = self.value()?;
                map.insert(key, val);
            }
            Ok(PlistValue::Dict(map))
        }

        fn parse_array(&mut self) -> Result<PlistValue, PlistError> {
            self.i += "<array>".len();
            let mut out = Vec::new();
            loop {
                self.skip_ws();
                if self.starts_with(b"</array>") {
                    self.i += "</array>".len();
                    break;
                }
                out.push(self.value()?);
            }
            Ok(PlistValue::Array(out))
        }

        fn text_element(&mut self, name: &str) -> Result<String, PlistError> {
            let open = format!("<{name}>");
            let close = format!("</{name}>");
            self.i += open.len();
            let rest = &self.s[self.i..];
            let end = find(rest, close.as_bytes())
                .ok_or_else(|| PlistError::Xml(format!("unterminated <{name}>")))?;
            let raw = &rest[..end];
            self.i += end + close.len();
            Ok(unescape(std::str::from_utf8(raw).unwrap_or("")))
        }
    }

    fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
        hay.windows(needle.len()).position(|w| w == needle)
    }

    fn unescape(s: &str) -> String {
        s.replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&quot;", "\"")
            .replace("&apos;", "'")
            .replace("&amp;", "&")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xml_roundtrip() {
        let doc = br#"<?xml version="1.0"?>
        <!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "x">
        <plist version="1.0">
        <dict>
            <key>CFBundleName</key><string>Demo</string>
            <key>MinimumOSVersion</key><string>7.0</string>
            <key>LSRequiresIPhoneOS</key><true/>
            <key>CFBundleIconFiles</key>
            <array><string>Icon.png</string><string>Icon@2x.png</string></array>
        </dict>
        </plist>"#;
        let v = parse(doc).unwrap();
        assert_eq!(v.get("CFBundleName").and_then(|x| x.as_str()), Some("Demo"));
        assert_eq!(v.get("LSRequiresIPhoneOS"), Some(&PlistValue::Bool(true)));
        assert_eq!(v.get("CFBundleIconFiles").and_then(|x| x.as_array()).map(|a| a.len()), Some(2));
    }

    #[test]
    fn binary_bool_and_int() {
        // Build a tiny bplist: top object is a dict {"k": 7}.
        // Objects: 0=dict, 1=key "k", 2=int 7.
        let mut buf = b"bplist00".to_vec();
        let mut offsets = Vec::new();

        offsets.push(buf.len());
        buf.push(0xD1); // dict, count 1
        buf.push(1); // key ref -> obj 1
        buf.push(2); // val ref -> obj 2

        offsets.push(buf.len());
        buf.push(0x51); // ASCII string len 1
        buf.push(b'k');

        offsets.push(buf.len());
        buf.push(0x10); // int, 1 byte
        buf.push(7);

        let table_offset = buf.len();
        for &o in &offsets {
            buf.push(o as u8); // offset_size = 1
        }

        // Trailer.
        buf.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
        buf.push(1); // offset_size
        buf.push(1); // ref_size
        buf.extend_from_slice(&(offsets.len() as u64).to_be_bytes());
        buf.extend_from_slice(&0u64.to_be_bytes()); // top object
        buf.extend_from_slice(&(table_offset as u64).to_be_bytes());

        let v = parse(&buf).unwrap();
        assert_eq!(v.get("k").and_then(|x| x.as_i64()), Some(7));
    }
}
