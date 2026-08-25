//! x86/x64 disassembly + normalized signatures.
//!
//! Normalization goal: make "logically equivalent after recompilation" instructions produce the same
//! signature, so that when byte-level diff (chunked/sliding) fails, function/basic-block-level match
//! anchors can still be located.
//!
//! Normalization rules (conservative; only relocation-sensitive fields are masked for now):
//! - Relative branch targets (NearBranch rel32/16) -> masked
//! - Immediates (Immediate*) -> masked
//! - Memory operand displacements (displacement / RIP-relative addresses) -> masked
//! - Preserved: mnemonic, register numbers, memory base/index/scale

use iced_x86::{Decoder, DecoderOptions, Instruction, OpKind};

/// Normalized result of one disassembled instruction.
pub struct Insn {
    /// Byte offset in the file (global).
    pub offset: usize,
    /// Instruction byte length.
    pub len: usize,
    /// Normalized signature (xxh64).
    pub sig: u64,
}

/// Disassemble a slice of x86/x64 machine code.
/// `data` is the segment's byte slice, `base_ip` the segment's start virtual address, and
/// `file_base` the segment's start file offset.
pub fn disassemble(data: &[u8], base_ip: u64, file_base: usize, bitness: u32) -> Vec<Insn> {
    let mut decoder = Decoder::with_ip(bitness, data, base_ip, DecoderOptions::NONE);
    let mut insn = Instruction::default();
    let mut out = Vec::new();
    while decoder.can_decode() {
        decoder.decode_out(&mut insn);
        let len = insn.len();
        if len == 0 {
            break;
        }
        let off = (insn.ip() - base_ip) as usize;
        out.push(Insn {
            offset: file_base + off,
            len,
            sig: norm_sig(&insn),
        });
    }
    out
}

fn norm_sig(insn: &Instruction) -> u64 {
    let mut s = String::with_capacity(48);
    s.push_str(&format!("{:?}", insn.mnemonic()));
    for i in 0..insn.op_count() {
        s.push('|');
        s.push_str(&norm_operand(insn, i));
    }
    xxhash_rust::xxh64::xxh64(s.as_bytes(), 0x9E37_79B9_7F4A_7C15)
}

fn norm_operand(insn: &Instruction, i: u32) -> String {
    use OpKind::*;
    match insn.op_kind(i) {
        Register => "reg".to_string(),
        NearBranch16 | NearBranch32 | NearBranch64 => "rel".to_string(),
        FarBranch16 | FarBranch32 => "far".to_string(),
        Immediate8 | Immediate8_2nd | Immediate16 | Immediate32 | Immediate64
        | Immediate8to16 | Immediate8to32 | Immediate8to64 | Immediate32to64 => "imm".to_string(),
        Memory => {
            if insn.is_ip_rel_memory_operand() {
                "m[rip]".to_string()
            } else {
                format!(
                    "m[base+idx*{}]",
                    insn.memory_index_scale()
                )
            }
        }
        MemorySegSI | MemorySegESI | MemorySegRSI | MemorySegDI | MemorySegEDI | MemorySegRDI
        | MemoryESDI | MemoryESEDI | MemoryESRDI => "m[seg]".to_string(),
    }
}
