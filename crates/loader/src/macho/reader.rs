//! A bounds-checked, cursored byte reader for parsing on-disk structures.
//!
//! Every accessor validates against the slice length and returns an
//! [`EmuError::MachO`] on truncation, so a malformed or maliciously-crafted
//! binary can never cause an out-of-bounds read — important given the input is
//! attacker-controlled app data.

use ios_emu_common::{EmuError, EmuResult};

pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Reader { data, pos: 0 }
    }

    /// Create a reader positioned at absolute offset `off`.
    pub fn at(data: &'a [u8], off: usize) -> EmuResult<Self> {
        if off > data.len() {
            return Err(EmuError::MachO(format!("offset {off:#x} past end")));
        }
        Ok(Reader { data, pos: off })
    }

    pub fn pos(&self) -> usize {
        self.pos
    }

    pub fn seek(&mut self, off: usize) -> EmuResult<()> {
        if off > self.data.len() {
            return Err(EmuError::MachO(format!("seek {off:#x} past end")));
        }
        self.pos = off;
        Ok(())
    }

    pub fn skip(&mut self, n: usize) -> EmuResult<()> {
        self.seek(self.pos.checked_add(n).ok_or_else(|| EmuError::MachO("skip overflow".into()))?)
    }

    pub fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }

    fn take(&mut self, n: usize) -> EmuResult<&'a [u8]> {
        let end = self
            .pos
            .checked_add(n)
            .ok_or_else(|| EmuError::MachO("read overflow".into()))?;
        if end > self.data.len() {
            return Err(EmuError::MachO(format!(
                "truncated: need {n} bytes at {:#x}, have {}",
                self.pos,
                self.data.len() - self.pos
            )));
        }
        let s = &self.data[self.pos..end];
        self.pos = end;
        Ok(s)
    }

    pub fn u8(&mut self) -> EmuResult<u8> {
        Ok(self.take(1)?[0])
    }

    pub fn u16_le(&mut self) -> EmuResult<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }

    pub fn u32_le(&mut self) -> EmuResult<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    pub fn u64_le(&mut self) -> EmuResult<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }

    pub fn u32_be(&mut self) -> EmuResult<u32> {
        Ok(u32::from_be_bytes(self.take(4)?.try_into().unwrap()))
    }

    pub fn u64_be(&mut self) -> EmuResult<u64> {
        Ok(u64::from_be_bytes(self.take(8)?.try_into().unwrap()))
    }

    /// Read a fixed-width, NUL-padded field as a `String` (e.g. segment names,
    /// 16-byte `segname`/`sectname`).
    pub fn fixed_str(&mut self, n: usize) -> EmuResult<String> {
        let raw = self.take(n)?;
        let end = raw.iter().position(|&b| b == 0).unwrap_or(n);
        Ok(String::from_utf8_lossy(&raw[..end]).into_owned())
    }

    /// Borrow `n` raw bytes without interpreting them.
    pub fn bytes(&mut self, n: usize) -> EmuResult<&'a [u8]> {
        self.take(n)
    }

    /// Peek a little-endian u32 without advancing.
    pub fn peek_u32_le(&self) -> EmuResult<u32> {
        if self.pos + 4 > self.data.len() {
            return Err(EmuError::MachO("peek past end".into()));
        }
        Ok(u32::from_le_bytes(self.data[self.pos..self.pos + 4].try_into().unwrap()))
    }
}
