//! Full pipeline test: assemble a Mach-O, map it into a `Machine`, and drive the
//! run loop with a scripted executor. Exercises loader -> memory -> stack setup
//! -> syscall dispatch without a real CPU backend.

use ios_emu_core::cpu::{ExecEvent, ScriptedExecutor};
use ios_emu_core::{Machine, MachineConfig};
use ios_emu_vfs::Sandbox;

const MH_MAGIC_64: u32 = 0xfeed_facf;
const CPU_TYPE_ARM64: u32 = 0x0100_000c;
const MH_EXECUTE: u32 = 0x2;
const MH_PIE: u32 = 0x0020_0000;
const LC_SEGMENT_64: u32 = 0x19;
const LC_MAIN: u32 = 0x28 | 0x8000_0000;

fn build_macho() -> Vec<u8> {
    let mut cmds = Vec::new();

    // __PAGEZERO so the image mirrors a real layout (skipped when mapping).
    cmds.extend_from_slice(&LC_SEGMENT_64.to_le_bytes());
    cmds.extend_from_slice(&72u32.to_le_bytes());
    let mut pz = [0u8; 16];
    pz[.."__PAGEZERO".len()].copy_from_slice(b"__PAGEZERO");
    cmds.extend_from_slice(&pz);
    cmds.extend_from_slice(&0u64.to_le_bytes()); // vmaddr
    cmds.extend_from_slice(&0x1_0000_0000u64.to_le_bytes()); // vmsize
    cmds.extend_from_slice(&0u64.to_le_bytes()); // fileoff
    cmds.extend_from_slice(&0u64.to_le_bytes()); // filesize
    cmds.extend_from_slice(&0u32.to_le_bytes()); // maxprot none
    cmds.extend_from_slice(&0u32.to_le_bytes()); // initprot none
    cmds.extend_from_slice(&0u32.to_le_bytes()); // nsects
    cmds.extend_from_slice(&0u32.to_le_bytes()); // flags

    // __TEXT
    cmds.extend_from_slice(&LC_SEGMENT_64.to_le_bytes());
    cmds.extend_from_slice(&72u32.to_le_bytes());
    let mut tx = [0u8; 16];
    tx[.."__TEXT".len()].copy_from_slice(b"__TEXT");
    cmds.extend_from_slice(&tx);
    cmds.extend_from_slice(&0x1_0000_0000u64.to_le_bytes()); // vmaddr
    cmds.extend_from_slice(&0x4000u64.to_le_bytes()); // vmsize
    cmds.extend_from_slice(&0u64.to_le_bytes()); // fileoff
    cmds.extend_from_slice(&0x80u64.to_le_bytes()); // filesize (fits our buffer)
    cmds.extend_from_slice(&5u32.to_le_bytes()); // maxprot r-x
    cmds.extend_from_slice(&5u32.to_le_bytes()); // initprot r-x
    cmds.extend_from_slice(&0u32.to_le_bytes()); // nsects
    cmds.extend_from_slice(&0u32.to_le_bytes()); // flags

    // LC_MAIN, entry at file offset 0x40.
    cmds.extend_from_slice(&LC_MAIN.to_le_bytes());
    cmds.extend_from_slice(&24u32.to_le_bytes());
    cmds.extend_from_slice(&0x40u64.to_le_bytes()); // entryoff
    cmds.extend_from_slice(&0u64.to_le_bytes()); // stacksize

    let mut out = Vec::new();
    out.extend_from_slice(&MH_MAGIC_64.to_le_bytes());
    out.extend_from_slice(&CPU_TYPE_ARM64.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&MH_EXECUTE.to_le_bytes());
    out.extend_from_slice(&3u32.to_le_bytes()); // ncmds
    out.extend_from_slice(&(cmds.len() as u32).to_le_bytes());
    out.extend_from_slice(&MH_PIE.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&cmds);
    // Pad up to the __TEXT filesize so segment bytes are readable (never shrink:
    // the load commands already extend past 0x80).
    if out.len() < 0x80 {
        out.resize(0x80, 0);
    }
    out
}

fn temp_sandbox() -> Sandbox {
    let root = std::env::temp_dir().join(format!("ios-emu-e2e-{}", std::process::id()));
    Sandbox::prepare(&root, &root.join("System"), "E2E", "E2E-UUID").unwrap()
}

#[test]
fn loads_and_reaches_entry() {
    let bytes = build_macho();
    let cfg = MachineConfig::default();
    let machine = Machine::load_bare(&cfg, &bytes, temp_sandbox()).expect("load");

    // Entry = __TEXT vmaddr (0x1_0000_0000) + slide(0) + entryoff(0x40).
    assert_eq!(machine.entry.raw(), 0x1_0000_0040);
}

#[test]
fn run_loop_services_exit() {
    let bytes = build_macho();
    let cfg = MachineConfig::default();
    let mut machine = Machine::load_bare(&cfg, &bytes, temp_sandbox()).expect("load");

    // Simulate the guest issuing exit(7): x16 = SYS_exit, x0 = 7.
    machine.cpu.x[16] = 1;
    machine.cpu.x[0] = 7;
    let mut exec = ScriptedExecutor::new([ExecEvent::Syscall { imm: 0x80 }]);
    let exit = machine.run(&mut exec).expect("run");
    assert_eq!(exit.code, 7);
}
