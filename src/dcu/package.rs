//! Compiled packages (`.dcp`): the container of a runtime package's unit
//! DCUs (DCU32INT `DCP.pas`).
//!
//! ```text
//! magic        'PKG' + version char ('4', '5', '7', '9'), 'PKX0', or 'PKX1'
//! [u32]        .NET packages (.dcpil), and for 'PKX1' (Delphi 13 Win64)
//!              the DCU magic of the package's target
//! nRequires    i32
//! nContains    i32
//! szContains   u32   size of the contains table
//! pContains    u32   offset of the contains table
//! ...          rest of the header
//! ```
//!
//! The contains table lists, per unit, the offset of its embedded DCU, two
//! flag words, the unit's code and data sizes in the package, version
//! dependent extra words, and its NUL-terminated name (plus a second name
//! from version `9`). Each embedded DCU starts with its magic and its own
//! size, so it can be sliced out and read with
//! [`Unit::parse_from_package`](crate::dcu::Unit::parse_from_package).
//!
//! For `PKX0` packages the number of extra words is not fixed; as DCU32INT
//! does, the reader tries 8, then 5 extra words and keeps the layout whose
//! table ends exactly at `szContains`.

use std::{error, fmt};

/// A unit contained in a package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageUnit {
    /// The unit name.
    pub name: String,
    /// Offset of the unit's DCU in the package image.
    pub offset: usize,
    /// Size of the unit's DCU.
    pub size: usize,
}

/// A decoded package directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Package {
    /// The version character of the magic (`'4'` - `'9'`, or `'X'`).
    pub version: char,
    /// The number of required packages.
    pub requires: u32,
    /// The contained units.
    pub units: Vec<PackageUnit>,
}

/// Why a package directory could not be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageError {
    /// The image is not a DCP (bad magic or version).
    NotAPackage,
    /// A header field or table entry points outside the image.
    OutOfBounds,
    /// The contains table does not decode to its stated size.
    BadContainsTable,
}

impl fmt::Display for PackageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NotAPackage => "not a compiled package (.dcp)",
            Self::OutOfBounds => "package table points outside the image",
            Self::BadContainsTable => "package contains table is malformed",
        })
    }
}

impl error::Error for PackageError {}

/// Reads a little-endian `u32` at `at`.
fn u32_at(data: &[u8], at: usize) -> Option<u32> {
    let b = data.get(at..at.checked_add(4)?)?;
    let mut w = [0u8; 4];
    w.copy_from_slice(b);
    Some(u32::from_le_bytes(w))
}

impl Package {
    /// Reads the directory of a package image. `msil` selects the .NET
    /// layout (`.dcpil`), which has an extra header word.
    ///
    /// # Errors
    ///
    /// Returns a [`PackageError`] for a non-package image or a malformed
    /// directory.
    pub fn parse(data: &[u8], msil: bool) -> Result<Self, PackageError> {
        if data.len() < 0x40 {
            return Err(PackageError::NotAPackage);
        }
        let magic = data.get(..4).ok_or(PackageError::NotAPackage)?;
        // `PKX1` (Delphi 13 Win64) is `PKX0` with the target's DCU magic
        // after it.
        let pkx1 = magic == b"PKX1";
        let (version, mut extra_words) = if magic == b"PKX0" || pkx1 {
            ('X', 3usize)
        } else if magic.get(..3) == Some(b"PKG") {
            let v = char::from(*magic.get(3).ok_or(PackageError::NotAPackage)?);
            if !matches!(v, '4' | '5' | '7' | '9') {
                return Err(PackageError::NotAPackage);
            }
            (v, 0usize)
        } else {
            return Err(PackageError::NotAPackage);
        };
        let header = if msil || pkx1 { 8 } else { 4 };
        let requires = u32_at(data, header).ok_or(PackageError::OutOfBounds)?;
        if requires > 0x100 {
            return Err(PackageError::NotAPackage);
        }
        let contains = u32_at(data, header.saturating_add(4)).ok_or(PackageError::OutOfBounds)?;
        let size =
            u32_at(data, header.saturating_add(8)).ok_or(PackageError::OutOfBounds)? as usize;
        let start =
            u32_at(data, header.saturating_add(12)).ok_or(PackageError::OutOfBounds)? as usize;
        let end = start.checked_add(size).ok_or(PackageError::OutOfBounds)?;
        if end > data.len() || end == 0 || data.get(end.saturating_sub(1)) != Some(&0) {
            return Err(PackageError::OutOfBounds);
        }
        loop {
            match Self::read_table(data, start, end, contains, version, msil, extra_words) {
                Ok(units) => {
                    return Ok(Self {
                        version,
                        requires,
                        units,
                    });
                }
                Err(e) if extra_words == 0 => return Err(e),
                Err(_) => extra_words = extra_words.saturating_sub(3),
            }
        }
    }

    /// Decodes the contains table with `extra_words` additional `PKX0`
    /// words per entry.
    fn read_table(
        data: &[u8],
        start: usize,
        end: usize,
        count: u32,
        version: char,
        msil: bool,
        extra_words: usize,
    ) -> Result<Vec<PackageUnit>, PackageError> {
        // pData, F, FT, bplCode, bplData.
        const FIXED: usize = 20;
        let mut units = Vec::new();
        let mut at = start;
        for _ in 0..count {
            if at.saturating_add(FIXED).saturating_sub(start) >= end.saturating_sub(start) {
                break;
            }
            let offset = u32_at(data, at).ok_or(PackageError::OutOfBounds)? as usize;
            let size =
                u32_at(data, offset.saturating_add(4)).ok_or(PackageError::OutOfBounds)? as usize;
            if offset.checked_add(size).is_none_or(|e| e > data.len()) {
                return Err(PackageError::OutOfBounds);
            }
            let mut name_at = at.saturating_add(FIXED);
            if version > '4' {
                name_at = name_at.saturating_add(4);
            }
            if version >= '9' || msil {
                name_at = name_at.saturating_add(4);
            }
            if version == 'X' {
                name_at =
                    name_at.saturating_add(5usize.saturating_add(extra_words).saturating_mul(4));
            }
            if name_at >= end {
                break;
            }
            let rest = data.get(name_at..end).ok_or(PackageError::OutOfBounds)?;
            let len = rest
                .iter()
                .position(|&b| b == 0)
                .ok_or(PackageError::BadContainsTable)?;
            if len > 255 {
                return Err(PackageError::BadContainsTable);
            }
            let name = String::from_utf8_lossy(rest.get(..len).unwrap_or(&[])).into_owned();
            units.push(PackageUnit { name, offset, size });
            at = name_at.saturating_add(len).saturating_add(1);
            if version >= '9' {
                if at >= end {
                    at = at.saturating_add(1);
                    break;
                }
                let second = data.get(at..end).ok_or(PackageError::OutOfBounds)?;
                let len2 = second
                    .iter()
                    .position(|&b| b == 0)
                    .ok_or(PackageError::BadContainsTable)?;
                at = at.saturating_add(len2).saturating_add(1);
            }
        }
        if at != end {
            return Err(PackageError::BadContainsTable);
        }
        Ok(units)
    }

    /// Returns the DCU image of the contained unit `name` (compared without
    /// regard to ASCII case).
    #[must_use]
    pub fn unit_data<'d>(&self, data: &'d [u8], name: &str) -> Option<&'d [u8]> {
        let u = self
            .units
            .iter()
            .find(|u| u.name.eq_ignore_ascii_case(name))?;
        data.get(u.offset..u.offset.checked_add(u.size)?)
    }
}
