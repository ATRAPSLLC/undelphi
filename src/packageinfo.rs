//! `PACKAGEINFO` resource parser.
//!
//! Every Delphi / C++Builder executable that uses the VCL or FMX libraries
//! embeds a `PACKAGEINFO` resource of type `RT_RCDATA` that lists the units
//! compiled into the binary and the packages it requires at load time.
//!
//! Layout (all integers little-endian, all strings NUL-terminated ASCII),
//! verbatim from pythia `pythia/core/structures.py:66-83`:
//!
//! ```text
//! packageinfo {
//!     Flags          u32
//!     RequiresCount  u32
//!     Requires[RequiresCount] {
//!         HashCode   u8
//!         Name       CString
//!     }
//!     ContainsCount  u32
//!     Contains[ContainsCount] {
//!         Flags      u8
//!         HashCode   u8
//!         Name       CString
//!     }
//! }
//! ```
//!
//! Notes:
//!
//! - The outer `Flags` is the `pfXxx` bitfield of Delphi 6 `SysUtils.pas`,
//!   decoded by [`PackageInfo::module_type`], [`PackageInfo::producer`] and
//!   the option accessors:
//!
//!   ```text
//!     $00000001 pfNeverBuild        $C0000000 pfModuleTypeMask
//!     $00000002 pfDesignOnly          $40000000 pfPackageModule
//!     $00000004 pfRunOnly             $80000000 pfLibraryModule
//!     $00000008 pfIgnoreDupUnits      $00000000 pfExeModule
//!     $0C000000 pfProducerMask
//!       $00000000 pfV3Produced, $04000000 pfProducerUndefined,
//!       $08000000 pfBCB4Produced, $0C000000 pfDelphi4Produced
//!   ```
//!
//!   Empirically every program from Delphi 6 to 12 carries module type
//!   `$C0000000` rather than the declared `pfExeModule = 0` (our Delphi 6
//!   fixtures, Delphi 7 `LA.exe`, HeidiSQL XE5 / 12); C++Builder-built
//!   packages carry `pfBCB4Produced` and Delphi-built ones
//!   `pfDelphi4Produced` (the 104 packages shipped with C++Builder 6).
//!   Later compilers also set bits Delphi 6 does not define (`$00100000`
//!   on XE5 / 12 programs, `$00300000` on a Delphi 12 package); those are
//!   kept in [`PackageInfo::undocumented_flags`].
//! - The per-unit `Flags` byte is the `ufXxx` set (`ufMainUnit` `$01`,
//!   `ufPackageUnit` `$02`, `ufWeakUnit` `$04`, `ufOrgWeakUnit` `$08`,
//!   `ufImplicitUnit` `$10`), decoded by the [`ContainedUnit`] accessors.
//! - `HashCode` is a compiler-emitted check digit. We expose it but do not
//!   verify it.

use std::str;

/// Parsed PACKAGEINFO resource. All strings borrow from the resource
/// body provided to [`parse`].
#[derive(Debug, Clone)]
pub struct PackageInfo<'a> {
    /// Package-level flags (`pfXxx`); see [`Self::module_type`],
    /// [`Self::producer`] and the option accessors.
    pub flags: u32,
    /// Names of packages this binary requires (BPL dependencies).
    pub requires: Vec<RequiredPackage<'a>>,
    /// Units linked into this binary.
    pub contains: Vec<ContainedUnit<'a>>,
}

/// Module kind recorded in a PACKAGEINFO's `pfModuleTypeMask` bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModuleType {
    /// A program: `$C0000000` as every compiler writes it, or the declared
    /// `pfExeModule = 0`.
    Program,
    /// A runtime or design-time package (`pfPackageModule`).
    Package,
    /// A library (`pfLibraryModule`).
    Library,
}

/// Which compiler family produced a module, from `pfProducerMask`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Producer {
    /// `pfV3Produced`: a Delphi 3-era producer.
    DelphiV3,
    /// `pfProducerUndefined`.
    Undefined,
    /// `pfBCB4Produced`: C++Builder.
    CppBuilder,
    /// `pfDelphi4Produced`: Delphi 4 or later.
    Delphi,
}

/// The `pfXxx` bits Delphi 6 `SysUtils.pas` defines.
const DOCUMENTED_FLAGS: u32 = 0xCC00_000F;

impl PackageInfo<'_> {
    /// The module kind (`pfModuleTypeMask`).
    pub fn module_type(&self) -> ModuleType {
        match self.flags >> 30 {
            1 => ModuleType::Package,
            2 => ModuleType::Library,
            _ => ModuleType::Program,
        }
    }

    /// The producing compiler family (`pfProducerMask`).
    pub fn producer(&self) -> Producer {
        match (self.flags >> 26) & 0x3 {
            0 => Producer::DelphiV3,
            1 => Producer::Undefined,
            2 => Producer::CppBuilder,
            _ => Producer::Delphi,
        }
    }

    /// `pfNeverBuild`: built with implicit building off.
    #[inline]
    pub fn never_build(&self) -> bool {
        self.flags & 0x1 != 0
    }

    /// `pfDesignOnly`: a design-time package.
    #[inline]
    pub fn design_only(&self) -> bool {
        self.flags & 0x2 != 0
    }

    /// `pfRunOnly`: a runtime-only package.
    #[inline]
    pub fn run_only(&self) -> bool {
        self.flags & 0x4 != 0
    }

    /// `pfIgnoreDupUnits`.
    #[inline]
    pub fn ignore_dup_units(&self) -> bool {
        self.flags & 0x8 != 0
    }

    /// Flag bits Delphi 6 does not define, set by later compilers.
    #[inline]
    pub fn undocumented_flags(&self) -> u32 {
        self.flags & !DOCUMENTED_FLAGS
    }
}

impl ContainedUnit<'_> {
    /// `ufMainUnit`: the program, library or package unit itself.
    #[inline]
    pub fn is_main(&self) -> bool {
        self.flags & 0x01 != 0
    }

    /// `ufPackageUnit`: the unit is the package's own source unit.
    #[inline]
    pub fn is_package(&self) -> bool {
        self.flags & 0x02 != 0
    }

    /// `ufWeakUnit`: weakly packaged.
    #[inline]
    pub fn is_weak(&self) -> bool {
        self.flags & 0x04 != 0
    }

    /// `ufOrgWeakUnit`: declared weakly packaged.
    #[inline]
    pub fn is_original_weak(&self) -> bool {
        self.flags & 0x08 != 0
    }

    /// `ufImplicitUnit`: pulled in implicitly, not listed in the source.
    #[inline]
    pub fn is_implicit(&self) -> bool {
        self.flags & 0x10 != 0
    }
}

/// One entry of the PACKAGEINFO `Requires` list.
#[derive(Debug, Clone, Copy)]
pub struct RequiredPackage<'a> {
    /// Compiler-emitted check digit.
    pub hash: u8,
    /// Package name (e.g. `rtl110.bpl` without the extension, or `rtl`).
    pub name: &'a str,
}

/// One entry of the PACKAGEINFO `Contains` list.
#[derive(Debug, Clone, Copy)]
pub struct ContainedUnit<'a> {
    /// Per-unit `ufXxx` flags; see [`Self::is_main`] and friends.
    pub flags: u8,
    /// Compiler-emitted check digit.
    pub hash: u8,
    /// Fully qualified unit name (e.g. `System.SysUtils`).
    pub name: &'a str,
}

/// Parses a PACKAGEINFO resource body.
///
/// Returns `None` on any structural error: truncation, non-ASCII name, or
/// impossible count. The parser deliberately does not allocate for strings -
/// every `name` is a slice of the input buffer.
pub fn parse<'a>(raw: &'a [u8]) -> Option<PackageInfo<'a>> {
    let mut cur = Cursor::new(raw);
    let flags = cur.read_u32()?;
    let requires_count = cur.read_u32()? as usize;
    // Plausibility guard - a resource with millions of entries is garbage.
    if requires_count > 65_536 {
        return None;
    }
    let mut requires = Vec::with_capacity(requires_count);
    for _ in 0..requires_count {
        let hash = cur.read_u8()?;
        let name = cur.read_cstr_ascii()?;
        requires.push(RequiredPackage { hash, name });
    }

    let contains_count = cur.read_u32()? as usize;
    if contains_count > 65_536 {
        return None;
    }
    let mut contains = Vec::with_capacity(contains_count);
    for _ in 0..contains_count {
        let flags_b = cur.read_u8()?;
        let hash = cur.read_u8()?;
        let name = cur.read_cstr_ascii()?;
        contains.push(ContainedUnit {
            flags: flags_b,
            hash,
            name,
        });
    }

    Some(PackageInfo {
        flags,
        requires,
        contains,
    })
}

struct Cursor<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    fn read_u8(&mut self) -> Option<u8> {
        let b = *self.buf.get(self.pos)?;
        self.pos = self.pos.checked_add(1)?;
        Some(b)
    }

    fn read_u32(&mut self) -> Option<u32> {
        let end = self.pos.checked_add(4)?;
        let bytes = self.buf.get(self.pos..end)?;
        self.pos = end;
        Some(u32::from_le_bytes(bytes.try_into().ok()?))
    }

    /// Reads a zero-terminated ASCII string and advances past the terminator.
    fn read_cstr_ascii(&mut self) -> Option<&'a str> {
        let start = self.pos;
        let tail = self.buf.get(start..)?;
        let rel = tail.iter().position(|&b| b == 0)?;
        let end = start.checked_add(rel)?;
        let bytes = self.buf.get(start..end)?;
        self.pos = end.checked_add(1)?;
        str::from_utf8(bytes).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_package_flags() {
        let pkg = PackageInfo {
            flags: 0x4C00_0005,
            requires: Vec::new(),
            contains: Vec::new(),
        };
        assert_eq!(pkg.module_type(), ModuleType::Package);
        assert_eq!(pkg.producer(), Producer::Delphi);
        assert!(pkg.never_build() && pkg.run_only() && !pkg.design_only());
        assert_eq!(pkg.undocumented_flags(), 0);
        let program = PackageInfo {
            flags: 0xCC10_0000,
            requires: Vec::new(),
            contains: Vec::new(),
        };
        assert_eq!(program.module_type(), ModuleType::Program);
        assert_eq!(program.undocumented_flags(), 0x0010_0000);
    }

    #[test]
    fn parses_synthetic_packageinfo() {
        // Minimal synthetic resource: 1 require, 2 contains.
        let mut raw = Vec::new();
        raw.extend_from_slice(&0x0000_0001u32.to_le_bytes()); // Flags
        raw.extend_from_slice(&1u32.to_le_bytes()); // RequiresCount
        raw.push(0x11); // hash
        raw.extend_from_slice(b"rtl\0");
        raw.extend_from_slice(&2u32.to_le_bytes()); // ContainsCount
        raw.push(0x01); // flags
        raw.push(0x22); // hash
        raw.extend_from_slice(b"System.SysUtils\0");
        raw.push(0x00); // flags
        raw.push(0x33); // hash
        raw.extend_from_slice(b"Main\0");

        let pkg = parse(&raw).expect("should parse");
        assert_eq!(pkg.flags, 1);
        assert_eq!(pkg.requires.len(), 1);
        assert_eq!(pkg.requires[0].name, "rtl");
        assert_eq!(pkg.requires[0].hash, 0x11);
        assert_eq!(pkg.contains.len(), 2);
        assert_eq!(pkg.contains[0].name, "System.SysUtils");
        assert_eq!(pkg.contains[0].flags, 0x01);
        assert_eq!(pkg.contains[1].name, "Main");
    }

    #[test]
    fn rejects_implausible_counts() {
        // Claim 1 billion requires in a 12-byte buffer.
        let mut raw = Vec::new();
        raw.extend_from_slice(&0u32.to_le_bytes());
        raw.extend_from_slice(&1_000_000_000u32.to_le_bytes());
        raw.extend_from_slice(&0u32.to_le_bytes());
        assert!(parse(&raw).is_none());
    }

    #[test]
    fn rejects_truncated_buffer() {
        assert!(parse(&[]).is_none());
        assert!(parse(&[0u8; 3]).is_none());
    }
}
