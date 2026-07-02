//! Parser for dyld's compressed rebase opcode stream (`LC_DYLD_INFO[_ONLY]`).
//!
//! Rebasing relocates the absolute pointers the linker baked into `__DATA` /
//! `__DATA_CONST` (Obj-C metadata, vtables, `__mod_init_func`, ...) by the image
//! slide. Every such pointer stores its *preferred* target address; at load time
//! the runtime adds `dynamic_base - preferred_base` to it. Skipping this is
//! exactly what makes a slid PIE dereference an un-slid `0x1_00xx_xxxx` address
//! and fault.
//!
//! The output is a flat list of [`RebaseLocation`]s; the `core` crate reads the
//! stored pointer at each and writes it back plus the slide (it owns the slide
//! and the mapped segments).

use ios_emu_common::{EmuError, EmuResult};

// Rebase opcodes (`<mach-o/loader.h>`).
const REBASE_OPCODE_MASK: u8 = 0xF0;
const REBASE_IMMEDIATE_MASK: u8 = 0x0F;
const REBASE_OPCODE_DONE: u8 = 0x00;
const REBASE_OPCODE_SET_TYPE_IMM: u8 = 0x10;
const REBASE_OPCODE_SET_SEGMENT_AND_OFFSET_ULEB: u8 = 0x20;
const REBASE_OPCODE_ADD_ADDR_ULEB: u8 = 0x30;
const REBASE_OPCODE_ADD_ADDR_IMM_SCALED: u8 = 0x40;
const REBASE_OPCODE_DO_REBASE_IMM_TIMES: u8 = 0x50;
const REBASE_OPCODE_DO_REBASE_ULEB_TIMES: u8 = 0x60;
const REBASE_OPCODE_DO_REBASE_ADD_ADDR_ULEB: u8 = 0x70;
const REBASE_OPCODE_DO_REBASE_ULEB_TIMES_SKIPPING_ULEB: u8 = 0x80;

/// `REBASE_TYPE_POINTER` — the only type that occurs in arm64 PIE images (the
/// `TEXT_ABSOLUTE32`/`TEXT_PCREL32` types are 32-bit and i386-only).
pub const REBASE_TYPE_POINTER: u8 = 1;

/// One pointer slot to relocate: add the slide to the value stored at segment
/// `seg_index` + `seg_offset`.
#[derive(Clone, Debug)]
pub struct RebaseLocation {
    pub seg_index: u8,
    pub seg_offset: u64,
    pub rtype: u8,
}

/// Decode the whole rebase stream into locations. Pointer width is 8 (arm64).
pub fn parse_rebase_info(stream: &[u8]) -> EmuResult<Vec<RebaseLocation>> {
    let mut out = Vec::new();
    let mut p = 0usize;

    let mut seg_index: u8 = 0;
    let mut seg_offset: u64 = 0;
    let mut rtype: u8 = REBASE_TYPE_POINTER;
    const PTR: u64 = 8;

    while p < stream.len() {
        let byte = stream[p];
        p += 1;
        let opcode = byte & REBASE_OPCODE_MASK;
        let imm = byte & REBASE_IMMEDIATE_MASK;

        match opcode {
            // Terminator / trailing padding — treated as no-ops so the loop
            // naturally ends at the stream boundary.
            REBASE_OPCODE_DONE => {}
            REBASE_OPCODE_SET_TYPE_IMM => rtype = imm,
            REBASE_OPCODE_SET_SEGMENT_AND_OFFSET_ULEB => {
                seg_index = imm;
                seg_offset = read_uleb(stream, &mut p)?;
            }
            REBASE_OPCODE_ADD_ADDR_ULEB => {
                seg_offset = seg_offset.wrapping_add(read_uleb(stream, &mut p)?);
            }
            REBASE_OPCODE_ADD_ADDR_IMM_SCALED => {
                seg_offset = seg_offset.wrapping_add(imm as u64 * PTR);
            }
            REBASE_OPCODE_DO_REBASE_IMM_TIMES => {
                for _ in 0..imm {
                    out.push(RebaseLocation { seg_index, seg_offset, rtype });
                    seg_offset = seg_offset.wrapping_add(PTR);
                }
            }
            REBASE_OPCODE_DO_REBASE_ULEB_TIMES => {
                let count = read_uleb(stream, &mut p)?;
                for _ in 0..count {
                    out.push(RebaseLocation { seg_index, seg_offset, rtype });
                    seg_offset = seg_offset.wrapping_add(PTR);
                }
            }
            REBASE_OPCODE_DO_REBASE_ADD_ADDR_ULEB => {
                out.push(RebaseLocation { seg_index, seg_offset, rtype });
                seg_offset =
                    seg_offset.wrapping_add(PTR).wrapping_add(read_uleb(stream, &mut p)?);
            }
            REBASE_OPCODE_DO_REBASE_ULEB_TIMES_SKIPPING_ULEB => {
                let count = read_uleb(stream, &mut p)?;
                let skip = read_uleb(stream, &mut p)?;
                for _ in 0..count {
                    out.push(RebaseLocation { seg_index, seg_offset, rtype });
                    seg_offset = seg_offset.wrapping_add(PTR).wrapping_add(skip);
                }
            }
            other => return Err(EmuError::Dyld(format!("unknown rebase opcode {other:#x}"))),
        }
    }
    Ok(out)
}

fn read_uleb(buf: &[u8], p: &mut usize) -> EmuResult<u64> {
    let mut result: u64 = 0;
    let mut shift = 0u32;
    loop {
        let b = *buf.get(*p).ok_or_else(|| EmuError::Dyld("rebase uleb truncated".into()))?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rebase_imm_times() {
        // SET_TYPE_IMM(POINTER); SET_SEGMENT_AND_OFFSET(seg=2, off=0x10);
        // DO_REBASE_IMM_TIMES(3); DONE
        let mut s = vec![REBASE_OPCODE_SET_TYPE_IMM | REBASE_TYPE_POINTER];
        s.push(REBASE_OPCODE_SET_SEGMENT_AND_OFFSET_ULEB | 2);
        s.push(0x10);
        s.push(REBASE_OPCODE_DO_REBASE_IMM_TIMES | 3);
        s.push(REBASE_OPCODE_DONE);

        let locs = parse_rebase_info(&s).unwrap();
        assert_eq!(locs.len(), 3);
        assert_eq!(locs[0].seg_index, 2);
        assert_eq!(locs[0].seg_offset, 0x10);
        assert_eq!(locs[1].seg_offset, 0x18); // +8
        assert_eq!(locs[2].seg_offset, 0x20); // +8
        assert!(locs.iter().all(|l| l.rtype == REBASE_TYPE_POINTER));
    }

    #[test]
    fn rebase_times_skipping() {
        // SET_SEGMENT_AND_OFFSET(seg=1, off=0); TIMES_SKIPPING(count=2, skip=8)
        let mut s = vec![REBASE_OPCODE_SET_SEGMENT_AND_OFFSET_ULEB | 1];
        s.push(0x00);
        s.push(REBASE_OPCODE_DO_REBASE_ULEB_TIMES_SKIPPING_ULEB);
        s.push(2); // count
        s.push(8); // skip
        let locs = parse_rebase_info(&s).unwrap();
        assert_eq!(locs.len(), 2);
        assert_eq!(locs[0].seg_offset, 0);
        assert_eq!(locs[1].seg_offset, 16); // +PTR(8) +skip(8)
    }
}
