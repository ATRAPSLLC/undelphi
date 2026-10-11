//! Primitive DCU readers: fixed-width integers, the packed index encodings,
//! and the three string forms.
//!
//! Every multi-byte value is little-endian. Indices use a prefix-length
//! encoding keyed by the low bits of the first byte (DCU32INT `ReadUIndex` /
//! `ReadIndex`):
//!
//! | First byte (binary) | Length | Value bits |
//! |---------------------|--------|------------|
//! | `xxxxxxx0`          | 1      | 7          |
//! | `xxxxxx01`          | 2      | 14         |
//! | `xxxxx011`          | 3      | 21         |
//! | `xxxx0111`          | 4      | 28         |
//! | `xxxx1111`          | 5      | 32 (bytes 1 - 4) |
//! | `yyyy1111`, `yyyy != 0` | 9  | 64 (bytes 1 - 8), Delphi 4 and later |
//!
//! The unsigned form shifts the prefix out; the signed form sign-extends from
//! the top value bit (an arithmetic shift of the little-endian word).

use crate::dcu::{Error, ErrorKind, version::Version};

/// A bounds-checked cursor over a DCU image.
///
/// The cursor knows the unit's [`Version`], because two encodings depend on
/// it: the 9-byte index form (Delphi 4 and later) and the long-name escape
/// (Delphi 2009 and later).
#[derive(Debug, Clone)]
pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
    version: Version,
}

impl<'a> Reader<'a> {
    /// Creates a cursor over `data`, starting at offset 0.
    #[must_use]
    pub fn new(data: &'a [u8], version: Version) -> Self {
        Self {
            data,
            pos: 0,
            version,
        }
    }

    /// Returns the current offset into the image.
    #[must_use]
    pub fn pos(&self) -> usize {
        self.pos
    }

    /// Returns the number of bytes left after the cursor.
    #[must_use]
    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    /// Returns the whole image the cursor reads.
    #[must_use]
    pub fn data(&self) -> &'a [u8] {
        self.data
    }

    /// Changes the version the version-dependent encodings follow; used
    /// once the magic has been decoded.
    pub fn set_version(&mut self, version: Version) {
        self.version = version;
    }

    /// Builds an error of `kind` at the current offset.
    #[must_use]
    pub fn error(&self, kind: ErrorKind) -> Error {
        Error::new(self.pos, kind)
    }

    /// Returns the next byte without consuming it.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Truncated`] at the end of the image.
    pub fn peek(&self) -> Result<u8, Error> {
        self.data
            .get(self.pos)
            .copied()
            .ok_or_else(|| self.error(ErrorKind::Truncated))
    }

    /// Reads `len` bytes.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Truncated`] when fewer than `len` bytes remain.
    pub fn bytes(&mut self, len: usize) -> Result<&'a [u8], Error> {
        let end = self
            .pos
            .checked_add(len)
            .ok_or_else(|| self.error(ErrorKind::Truncated))?;
        let out = self
            .data
            .get(self.pos..end)
            .ok_or_else(|| self.error(ErrorKind::Truncated))?;
        self.pos = end;
        Ok(out)
    }

    /// Skips `len` bytes.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Truncated`] when fewer than `len` bytes remain.
    pub fn skip(&mut self, len: usize) -> Result<(), Error> {
        self.bytes(len).map(|_| ())
    }

    /// Reads one byte.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Truncated`] at the end of the image.
    pub fn u8(&mut self) -> Result<u8, Error> {
        let b = self.peek()?;
        self.pos = self.pos.saturating_add(1);
        Ok(b)
    }

    /// Reads one byte when it equals `value` (DCU32INT `ReadByteIfEQ`),
    /// returning whether it did.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Truncated`] at the end of the image.
    pub fn u8_if(&mut self, value: u8) -> Result<bool, Error> {
        if self.peek()? == value {
            self.pos = self.pos.saturating_add(1);
            return Ok(true);
        }
        Ok(false)
    }

    /// Reads a little-endian `u16`.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Truncated`] when fewer than 2 bytes remain.
    pub fn u16(&mut self) -> Result<u16, Error> {
        let b = self.bytes(2)?;
        Ok(u16::from_le_bytes([
            b.first().copied().unwrap_or(0),
            b.get(1).copied().unwrap_or(0),
        ]))
    }

    /// Reads a little-endian `u32`.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Truncated`] when fewer than 4 bytes remain.
    pub fn u32(&mut self) -> Result<u32, Error> {
        let b = self.bytes(4)?;
        let mut word = [0u8; 4];
        word.copy_from_slice(b);
        Ok(u32::from_le_bytes(word))
    }

    /// Reads the raw bytes of a packed index: the prefix-tagged 1 - 5 byte
    /// body and, for the 9-byte form, the high word.
    ///
    /// Returns the low 32 bits as read (still carrying the prefix for the
    /// short forms), the body length, and the high word when present.
    fn index_body(&mut self) -> Result<(u32, usize, Option<u32>), Error> {
        let b0 = self.u8()?;
        if b0 & 0x01 == 0 {
            return Ok((u32::from(b0), 1, None));
        }
        let b1 = self.u8()?;
        if b0 & 0x02 == 0 {
            return Ok((u32::from(u16::from_le_bytes([b0, b1])), 2, None));
        }
        let b2 = self.u8()?;
        if b0 & 0x04 == 0 {
            return Ok((u32::from_le_bytes([b0, b1, b2, 0]), 3, None));
        }
        let b3 = self.u8()?;
        if b0 & 0x08 == 0 {
            return Ok((u32::from_le_bytes([b0, b1, b2, b3]), 4, None));
        }
        let b4 = self.u8()?;
        let low = u32::from_le_bytes([b1, b2, b3, b4]);
        let high = if self.version.after(Version::DELPHI_3) && b0 & 0xF0 != 0 {
            Some(self.u32()?)
        } else {
            None
        };
        Ok((low, 5, high))
    }

    /// Reads an unsigned packed index (DCU32INT `ReadUIndex`), keeping the
    /// low 32 bits the way every structural field uses it.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Truncated`] when the index runs past the end.
    pub fn uindex(&mut self) -> Result<u32, Error> {
        self.uindex64().map(|v| v as u32)
    }

    /// Reads an unsigned packed index with its full 64-bit value.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Truncated`] when the index runs past the end.
    pub fn uindex64(&mut self) -> Result<u64, Error> {
        let (raw, len, high) = self.index_body()?;
        let low = match len {
            1 => raw >> 1,
            2 => raw >> 2,
            3 => raw >> 3,
            4 => raw >> 4,
            _ => raw,
        };
        Ok(u64::from(high.unwrap_or(0)) << 32 | u64::from(low))
    }

    /// Reads a signed packed index (DCU32INT `ReadIndex`), keeping the low
    /// 32 bits.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Truncated`] when the index runs past the end.
    pub fn index(&mut self) -> Result<i32, Error> {
        self.index64().map(|v| v as i32)
    }

    /// Reads a signed packed index with its full 64-bit value. A 5-byte
    /// body without a high word is sign-extended from bit 31.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Truncated`] when the index runs past the end.
    pub fn index64(&mut self) -> Result<i64, Error> {
        let (raw, len, high) = self.index_body()?;
        // Shift the body to the top of a 32-bit word, then arithmetic-shift
        // it back down: that sign-extends from the top value bit.
        let low = match len {
            1 => ((raw << 24) as i32) >> 25,
            2 => ((raw << 16) as i32) >> 18,
            3 => ((raw << 8) as i32) >> 11,
            4 => (raw as i32) >> 4,
            _ => raw as i32,
        };
        Ok(match high {
            Some(high) => (i64::from(high) << 32) | i64::from(low as u32),
            None => i64::from(low),
        })
    }

    /// Reads a length-prefixed identifier (DCU32INT `ReadName`).
    ///
    /// The length is one byte; from Delphi 2009 on a length of `0xFF` is an
    /// escape followed by a 32-bit length, so a name can exceed 254 bytes.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Truncated`] when the name runs past the end.
    pub fn name(&mut self) -> Result<&'a [u8], Error> {
        let mut len = u32::from(self.u8()?);
        if len == 0xFF && self.version.delphi_at_least(Version::DELPHI_2009) {
            len = self.u32()?;
        }
        self.bytes(len as usize)
    }

    /// Reads a ShortString: a one-byte length and that many bytes
    /// (DCU32INT `ReadStr` / `ReadShortName`).
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Truncated`] when the string runs past the end.
    pub fn short_string(&mut self) -> Result<&'a [u8], Error> {
        let len = self.u8()?;
        self.bytes(usize::from(len))
    }

    /// Reads a string with a packed-index length (DCU32INT `ReadNDXStr`).
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Truncated`] when the string runs past the end,
    /// or [`ErrorKind::Malformed`] when the length exceeds 1 MiB (the bound
    /// DCU32INT applies).
    pub fn index_string(&mut self) -> Result<&'a [u8], Error> {
        let len = self.uindex()?;
        if len > 0x10_0000 {
            return Err(self.error(ErrorKind::Malformed("index string longer than 1 MiB")));
        }
        self.bytes(len as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reader(bytes: &[u8]) -> Reader<'_> {
        Reader::new(bytes, Version::DELPHI_10_SEATTLE)
    }

    #[test]
    fn unsigned_index_forms() {
        assert_eq!(reader(&[0xFE]).uindex().unwrap(), 0x7F);
        assert_eq!(reader(&[0x13, 0xFF, 0x73]).uindex().unwrap(), 0xE7FE2);
        assert_eq!(reader(&[0x05, 0x01]).uindex().unwrap(), 0x41);
        assert_eq!(
            reader(&[0x0F, 0x78, 0x56, 0x34, 0x12]).uindex().unwrap(),
            0x1234_5678
        );
        assert_eq!(
            reader(&[0x1F, 1, 0, 0, 0, 2, 0, 0, 0]).uindex64().unwrap(),
            0x2_0000_0001
        );
    }

    #[test]
    fn signed_index_forms() {
        assert_eq!(reader(&[0xFE]).index().unwrap(), -1);
        assert_eq!(reader(&[0x02]).index().unwrap(), 1);
        assert_eq!(reader(&[0xFD, 0xFF]).index().unwrap(), -1);
        assert_eq!(reader(&[0xFB, 0xFF, 0xFF]).index().unwrap(), -1);
        assert_eq!(reader(&[0xF7, 0xFF, 0xFF, 0xFF]).index().unwrap(), -1);
        assert_eq!(
            reader(&[0x0F, 0xFF, 0xFF, 0xFF, 0xFF]).index64().unwrap(),
            -1
        );
    }

    #[test]
    fn long_names_from_delphi_2009() {
        let mut bytes = vec![0xFF, 3, 0, 0, 0];
        bytes.extend_from_slice(b"abc");
        assert_eq!(reader(&bytes).name().unwrap(), b"abc");
        let mut d6 = Reader::new(&[2, b'h', b'i'], Version::DELPHI_6);
        assert_eq!(d6.name().unwrap(), b"hi");
    }
}
