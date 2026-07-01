//! End-to-end parse of a hand-assembled 64-bit Mach-O, exercising the real
//! byte-level parsing path (header + LC_SEGMENT_64 + LC_MAIN) rather than the
//! isolated unit tests.

use ios_emu_loader::macho::image::EntryPoint;
use ios_emu_loader::MachOImage;

const MH_MAGIC_64: u32 = 0xfeed_facf;
const CPU_TYPE_ARM64: u32 = 0x0100_000c;
const MH_EXECUTE: u32 = 0x2;
const MH_PIE: u32 = 0x0020_0000;
const LC_SEGMENT_64: u32 = 0x19;
const LC_MAIN: u32 = 0x28 | 0x8000_0000;

/// Assemble a minimal but structurally-valid arm64 Mach-O executable with one
/// `__TEXT` segment and an `LC_MAIN` entry point.
fn build() -> Vec<u8> {
    let mut cmds = Vec::new();

    // --- LC_SEGMENT_64 __TEXT (no sections) ---
    let seg_start = cmds.len();
    cmds.extend_from_slice(&LC_SEGMENT_64.to_le_bytes());
    cmds.extend_from_slice(&72u32.to_le_bytes()); // cmdsize
    let mut segname = [0u8; 16];
    segname[.."__TEXT".len()].copy_from_slice(b"__TEXT");
    cmds.extend_from_slice(&segname);
    cmds.extend_from_slice(&0x1_0000_0000u64.to_le_bytes()); // vmaddr
    cmds.extend_from_slice(&0x4000u64.to_le_bytes()); // vmsize
    cmds.extend_from_slice(&0u64.to_le_bytes()); // fileoff
    cmds.extend_from_slice(&0x4000u64.to_le_bytes()); // filesize
    cmds.extend_from_slice(&5u32.to_le_bytes()); // maxprot r-x
    cmds.extend_from_slice(&5u32.to_le_bytes()); // initprot r-x
    cmds.extend_from_slice(&0u32.to_le_bytes()); // nsects
    cmds.extend_from_slice(&0u32.to_le_bytes()); // flags
    assert_eq!(cmds.len() - seg_start, 72);

    // --- LC_MAIN ---
    cmds.extend_from_slice(&LC_MAIN.to_le_bytes());
    cmds.extend_from_slice(&24u32.to_le_bytes()); // cmdsize
    cmds.extend_from_slice(&0x100u64.to_le_bytes()); // entryoff
    cmds.extend_from_slice(&0u64.to_le_bytes()); // stacksize

    // --- mach_header_64 ---
    let mut out = Vec::new();
    out.extend_from_slice(&MH_MAGIC_64.to_le_bytes());
    out.extend_from_slice(&CPU_TYPE_ARM64.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // cpusubtype
    out.extend_from_slice(&MH_EXECUTE.to_le_bytes());
    out.extend_from_slice(&2u32.to_le_bytes()); // ncmds
    out.extend_from_slice(&(cmds.len() as u32).to_le_bytes()); // sizeofcmds
    out.extend_from_slice(&MH_PIE.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // reserved
    out.extend_from_slice(&cmds);
    out
}

#[test]
fn parses_header_segment_and_entry() {
    let bytes = build();
    let image = MachOImage::parse(&bytes).expect("parse");

    assert!(image.is_pie());
    assert_eq!(image.filetype, MH_EXECUTE);
    assert_eq!(image.segments.len(), 1);

    let text = &image.segments[0];
    assert_eq!(text.name, "__TEXT");
    assert_eq!(text.vmaddr, 0x1_0000_0000);
    assert_eq!(image.text_vmaddr(), 0x1_0000_0000);

    match image.entry.expect("entry") {
        EntryPoint::Main { file_offset, .. } => assert_eq!(file_offset, 0x100),
        other => panic!("expected LC_MAIN, got {other:?}"),
    }
    assert!(!image.is_encrypted());
}

#[test]
fn rejects_truncated_input() {
    let bytes = build();
    // Chop off mid-load-command; the bounds-checked reader must error, not panic.
    assert!(MachOImage::parse(&bytes[..40]).is_err());
}
