//! Parser for dyld's compressed bind opcode stream (`LC_DYLD_INFO_ONLY`).
//!
//! The output is a flat list of [`BindRecord`]s. Each record says "at
//! segment `seg_index` + `seg_offset`, store a pointer to `symbol` (from dylib
//! `lib_ordinal`) plus `addend`". The `core` crate resolves `symbol` to a
//! trampoline address and writes the pointer into guest memory once segments
//! are mapped and the slide is known.

use ios_emu_common::{EmuError, EmuResult};

// Bind opcodes (`<mach-o/loader.h>`).
const BIND_OPCODE_MASK: u8 = 0xF0;
const BIND_IMMEDIATE_MASK: u8 = 0x0F;
const BIND_OPCODE_DONE: u8 = 0x00;
const BIND_OPCODE_SET_DYLIB_ORDINAL_IMM: u8 = 0x10;
const BIND_OPCODE_SET_DYLIB_ORDINAL_ULEB: u8 = 0x20;
const BIND_OPCODE_SET_DYLIB_SPECIAL_IMM: u8 = 0x30;
const BIND_OPCODE_SET_SYMBOL_TRAILING_FLAGS_IMM: u8 = 0x40;
const BIND_OPCODE_SET_TYPE_IMM: u8 = 0x50;
const BIND_OPCODE_SET_ADDEND_SLEB: u8 = 0x60;
const BIND_OPCODE_SET_SEGMENT_AND_OFFSET_ULEB: u8 = 0x70;
const BIND_OPCODE_ADD_ADDR_ULEB: u8 = 0x80;
const BIND_OPCODE_DO_BIND: u8 = 0x90;
const BIND_OPCODE_DO_BIND_ADD_ADDR_ULEB: u8 = 0xA0;
const BIND_OPCODE_DO_BIND_ADD_ADDR_IMM_SCALED: u8 = 0xB0;
const BIND_OPCODE_DO_BIND_ULEB_TIMES_SKIPPING_ULEB: u8 = 0xC0;

/// The relocation type dyld should apply. Only pointer binds are meaningful on
/// arm64; text-absolute32 binds do not occur.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BindKind {
    Pointer,
    Other(u8),
}

impl From<u8> for BindKind {
    fn from(v: u8) -> Self {
        match v {
            1 => BindKind::Pointer, // BIND_TYPE_POINTER
            other => BindKind::Other(other),
        }
    }
}

/// One resolved location awaiting a pointer.
#[derive(Clone, Debug)]
pub struct BindRecord {
    pub seg_index: u8,
    pub seg_offset: u64,
    pub symbol: String,
    pub lib_ordinal: i64,
    pub addend: i64,
    pub kind: BindKind,
    pub weak: bool,
}

/// Decode the whole bind stream into records. `ptr_size` is 8 on arm64 and is
/// used by the "immediate scaled" and "times skipping" opcodes.
pub fn parse_bind_info(stream: &[u8]) -> EmuResult<Vec<BindRecord>> {
    let mut records = Vec::new();
    let mut p = 0usize;

    // Mutable "current" binding state, mutated by SET_* opcodes and snapshotted
    // by each DO_BIND.
    let mut seg_index: u8 = 0;
    let mut seg_offset: u64 = 0;
    let mut symbol = String::new();
    let mut lib_ordinal: i64 = 0;
    let mut addend: i64 = 0;
    let mut kind = BindKind::Pointer;
    let mut weak = false;
    const PTR: u64 = 8;

    while p < stream.len() {
        let byte = stream[p];
        p += 1;
        let opcode = byte & BIND_OPCODE_MASK;
        let imm = byte & BIND_IMMEDIATE_MASK;

        match opcode {
            BIND_OPCODE_DONE => { /* end of a sub-stream; keep going */ }
            BIND_OPCODE_SET_DYLIB_ORDINAL_IMM => {
                lib_ordinal = imm as i64;
            }
            BIND_OPCODE_SET_DYLIB_ORDINAL_ULEB => {
                lib_ordinal = read_uleb(stream, &mut p)? as i64;
            }
            BIND_OPCODE_SET_DYLIB_SPECIAL_IMM => {
                // Sign-extend the 4-bit immediate for self/main-exe/flat-lookup.
                lib_ordinal = if imm == 0 { 0 } else { (imm as i8 | 0xF0u8 as i8) as i64 };
            }
            BIND_OPCODE_SET_SYMBOL_TRAILING_FLAGS_IMM => {
                weak = imm & 0x1 != 0; // BIND_SYMBOL_FLAGS_WEAK_IMPORT
                symbol = read_cstr(stream, &mut p)?;
            }
            BIND_OPCODE_SET_TYPE_IMM => {
                kind = BindKind::from(imm);
            }
            BIND_OPCODE_SET_ADDEND_SLEB => {
                addend = read_sleb(stream, &mut p)?;
            }
            BIND_OPCODE_SET_SEGMENT_AND_OFFSET_ULEB => {
                seg_index = imm;
                seg_offset = read_uleb(stream, &mut p)?;
            }
            BIND_OPCODE_ADD_ADDR_ULEB => {
                seg_offset = seg_offset.wrapping_add(read_uleb(stream, &mut p)?);
            }
            BIND_OPCODE_DO_BIND => {
                emit(&mut records, seg_index, seg_offset, &symbol, lib_ordinal, addend, kind, weak);
                seg_offset = seg_offset.wrapping_add(PTR);
            }
            BIND_OPCODE_DO_BIND_ADD_ADDR_ULEB => {
                emit(&mut records, seg_index, seg_offset, &symbol, lib_ordinal, addend, kind, weak);
                seg_offset = seg_offset.wrapping_add(PTR).wrapping_add(read_uleb(stream, &mut p)?);
            }
            BIND_OPCODE_DO_BIND_ADD_ADDR_IMM_SCALED => {
                emit(&mut records, seg_index, seg_offset, &symbol, lib_ordinal, addend, kind, weak);
                seg_offset = seg_offset.wrapping_add(PTR).wrapping_add(imm as u64 * PTR);
            }
            BIND_OPCODE_DO_BIND_ULEB_TIMES_SKIPPING_ULEB => {
                let count = read_uleb(stream, &mut p)?;
                let skip = read_uleb(stream, &mut p)?;
                for _ in 0..count {
                    emit(&mut records, seg_index, seg_offset, &symbol, lib_ordinal, addend, kind, weak);
                    seg_offset = seg_offset.wrapping_add(PTR).wrapping_add(skip);
                }
            }
            other => {
                return Err(EmuError::Dyld(format!("unknown bind opcode {other:#x}")));
            }
        }
    }
    Ok(records)
}

/// One entry of the lazy binding info, keyed by the byte `offset` at which its
/// opcode sequence begins — exactly the value `__stub_helper` loads into `w16`
/// and passes to `dyld_stub_binder`. The pointer to update lives at segment
/// `seg_index` + `seg_offset`.
#[derive(Clone, Debug)]
pub struct LazyBind {
    pub offset: u64,
    pub seg_index: u8,
    pub seg_offset: u64,
    pub symbol: String,
}

/// Parse the *lazy* bind stream, recording the start offset of every symbol's
/// sequence so `dyld_stub_binder` can look a symbol up by the offset the stub
/// hands it. Lazy sequences are simple: set segment/offset, set ordinal, set
/// symbol, `DO_BIND`, `DONE`.
pub fn parse_lazy_bind_info(stream: &[u8]) -> EmuResult<Vec<LazyBind>> {
    let mut out = Vec::new();
    let mut p = 0usize;
    let mut seq_start = 0u64; // offset of the current symbol's first opcode
    let mut seg_index = 0u8;
    let mut seg_offset = 0u64;
    let mut symbol = String::new();

    while p < stream.len() {
        let byte = stream[p];
        p += 1;
        let opcode = byte & BIND_OPCODE_MASK;
        let imm = byte & BIND_IMMEDIATE_MASK;
        match opcode {
            // End of this symbol; the next sequence starts at the following byte.
            BIND_OPCODE_DONE => seq_start = p as u64,
            BIND_OPCODE_SET_DYLIB_ORDINAL_IMM | BIND_OPCODE_SET_DYLIB_SPECIAL_IMM => {}
            BIND_OPCODE_SET_DYLIB_ORDINAL_ULEB => {
                read_uleb(stream, &mut p)?;
            }
            BIND_OPCODE_SET_SYMBOL_TRAILING_FLAGS_IMM => {
                symbol = read_cstr(stream, &mut p)?;
            }
            BIND_OPCODE_SET_TYPE_IMM => {}
            BIND_OPCODE_SET_ADDEND_SLEB => {
                read_sleb(stream, &mut p)?;
            }
            BIND_OPCODE_SET_SEGMENT_AND_OFFSET_ULEB => {
                seg_index = imm;
                seg_offset = read_uleb(stream, &mut p)?;
            }
            BIND_OPCODE_ADD_ADDR_ULEB => {
                seg_offset = seg_offset.wrapping_add(read_uleb(stream, &mut p)?);
            }
            BIND_OPCODE_DO_BIND => {
                out.push(LazyBind { offset: seq_start, seg_index, seg_offset, symbol: symbol.clone() });
            }
            // Other DO_BIND_* forms do not appear in lazy streams; ignore.
            _ => {}
        }
    }
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
fn emit(
    out: &mut Vec<BindRecord>,
    seg_index: u8,
    seg_offset: u64,
    symbol: &str,
    lib_ordinal: i64,
    addend: i64,
    kind: BindKind,
    weak: bool,
) {
    out.push(BindRecord {
        seg_index,
        seg_offset,
        symbol: symbol.to_owned(),
        lib_ordinal,
        addend,
        kind,
        weak,
    });
}

fn read_uleb(buf: &[u8], p: &mut usize) -> EmuResult<u64> {
    let mut result: u64 = 0;
    let mut shift = 0u32;
    loop {
        let b = *buf.get(*p).ok_or_else(|| EmuError::Dyld("uleb truncated".into()))?;
        *p += 1;
        if shift < 64 {
            result |= ((b & 0x7f) as u64) << shift;
        }
        shift += 7;
        if b & 0x80 == 0 {
            break;
        }
    }
    Ok(result)
}

fn read_sleb(buf: &[u8], p: &mut usize) -> EmuResult<i64> {
    let mut result: i64 = 0;
    let mut shift = 0u32;
    let mut byte;
    loop {
        byte = *buf.get(*p).ok_or_else(|| EmuError::Dyld("sleb truncated".into()))?;
        *p += 1;
        result |= ((byte & 0x7f) as i64) << shift;
        shift += 7;
        if byte & 0x80 == 0 {
            break;
        }
    }
    if shift < 64 && (byte & 0x40) != 0 {
        result |= -1i64 << shift; // sign-extend
    }
    Ok(result)
}

fn read_cstr(buf: &[u8], p: &mut usize) -> EmuResult<String> {
    let start = *p;
    while *p < buf.len() && buf[*p] != 0 {
        *p += 1;
    }
    if *p >= buf.len() {
        return Err(EmuError::Dyld("bind symbol string not terminated".into()));
    }
    let s = String::from_utf8_lossy(&buf[start..*p]).into_owned();
    *p += 1; // consume NUL
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leb128() {
        let mut p = 0;
        assert_eq!(read_uleb(&[0xe5, 0x8e, 0x26], &mut p).unwrap(), 624485);
        let mut p = 0;
        assert_eq!(read_sleb(&[0x9b, 0xf1, 0x59], &mut p).unwrap(), -624485);
    }

    #[test]
    fn lazy_bind_records_sequence_offsets() {
        // Two lazy symbols, each: SET_SEGMENT_AND_OFFSET; SET_SYMBOL; DO_BIND; DONE.
        let mut s = Vec::new();
        s.push(BIND_OPCODE_SET_SEGMENT_AND_OFFSET_ULEB | 2);
        s.push(0x00);
        s.push(BIND_OPCODE_SET_SYMBOL_TRAILING_FLAGS_IMM);
        s.extend_from_slice(b"_objc_msgSend\0");
        s.push(BIND_OPCODE_DO_BIND);
        s.push(BIND_OPCODE_DONE);
        let off_b = s.len() as u64;
        s.push(BIND_OPCODE_SET_SEGMENT_AND_OFFSET_ULEB | 2);
        s.push(0x08);
        s.push(BIND_OPCODE_SET_SYMBOL_TRAILING_FLAGS_IMM);
        s.extend_from_slice(b"_malloc\0");
        s.push(BIND_OPCODE_DO_BIND);
        s.push(BIND_OPCODE_DONE);

        let lz = parse_lazy_bind_info(&s).unwrap();
        assert_eq!(lz.len(), 2);
        assert_eq!(lz[0].offset, 0);
        assert_eq!(lz[0].symbol, "_objc_msgSend");
        assert_eq!(lz[0].seg_offset, 0);
        assert_eq!(lz[1].offset, off_b);
        assert_eq!(lz[1].symbol, "_malloc");
        assert_eq!(lz[1].seg_offset, 8);
    }

    #[test]
    fn simple_bind_stream() {
        // SET_DYLIB_ORDINAL_IMM(1); SET_SYMBOL "_x"; SET_TYPE_IMM(1);
        // SET_SEGMENT_AND_OFFSET(seg=2, off=0x10); DO_BIND; DONE
        let mut s = vec![BIND_OPCODE_SET_DYLIB_ORDINAL_IMM | 1];
        s.push(BIND_OPCODE_SET_SYMBOL_TRAILING_FLAGS_IMM);
        s.extend_from_slice(b"_x\0");
        s.push(BIND_OPCODE_SET_TYPE_IMM | 1);
        s.push(BIND_OPCODE_SET_SEGMENT_AND_OFFSET_ULEB | 2);
        s.push(0x10);
        s.push(BIND_OPCODE_DO_BIND);
        s.push(BIND_OPCODE_DONE);

        let recs = parse_bind_info(&s).unwrap();
        assert_eq!(recs.len(), 1);
        let r = &recs[0];
        assert_eq!(r.symbol, "_x");
        assert_eq!(r.seg_index, 2);
        assert_eq!(r.seg_offset, 0x10);
        assert_eq!(r.lib_ordinal, 1);
        assert_eq!(r.kind, BindKind::Pointer);
    }
}
