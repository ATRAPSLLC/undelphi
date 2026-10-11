//! Delphi unit initialization table (`PackageInfoTable`).
//!
//! Every Delphi module carries a compiler-generated table that the RTL walks
//! at startup to run each unit's `initialization` section, and backwards at
//! shutdown to run the `finalization` sections. Delphi 6 `System.pas`:
//!
//! ```text
//!   PackageInfoTable = packed record
//!     UnitCount: Integer;           number of UnitInfo entries, > 0
//!     UnitInfo:  PUnitEntryTable;   -> UnitCount x { Init, FInit: Pointer }
//!   end;
//! ```
//!
//! Delphi 2010 and later append a `TypeInfo` part that lists every RTTI type
//! of the module, grouped by declaring unit:
//!
//! ```text
//!     TypeCount: Integer;
//!     TypeTable: ^array[TypeCount] of Pointer;
//!     UnitCount: Integer;           units in the type table (not UnitInfo's)
//!     UnitNames: PShortString;      UnitCount packed ShortStrings
//! ```
//!
//! On 64-bit each `Integer` is padded to pointer alignment, so the record is
//! 8 / 24 / 48 bytes (Delphi 6 / 2010+ 32-bit / 2010+ 64-bit) and the
//! `UnitInfo` array follows it directly - which is how the decoder tells the
//! layouts apart.
//!
//! ## Init entries
//!
//! The entries are in initialization order (`SysInit`, `System`, ...) and
//! end with the module's own unit, whose `Init` is nil (a program's main
//! block is not an initialization section). A nil `Init` / `FInit` means the
//! unit has no such section; so does a pointer to a data cell holding nil,
//! which Delphi XE+ uses as a placeholder for every item the linker removed
//! (HeidiSQL XE5: 29 such units, all pointing at the type table's
//! placeholder cell). The table carries no unit names; its order
//! differs from `PACKAGEINFO`'s `contains` list, and the 2010+ `UnitNames`
//! list covers a different set of units, so entries stay unnamed.
//! Empirically validated against the `dcc32` map of
//! `tests/samples/bcb6/known.exe`: each `Init` is the routine the map names
//! after its unit and each `FInit` that unit's `Finalization`.
//!
//! From Delphi 2010 not every entry is a unit: each class constructor /
//! class destructor pair is an entry of its own (`Init` the class's
//! `Create@` routine, `FInit` its `Destroy@`), placed before its unit's
//! entry, and the module's own unit can have an `FInit` (a program's
//! `Finalization`). Delphi 13's `known.exe` (`tests/samples/d13`): 45
//! entries for the 29 units it links, 11 of them class constructor pairs
//! (`TEncoding`, `TThread`, `TComponent`, ...), the rest of the extra entries
//! nil; Win32 and Win64 alike, checked against both maps.
//!
//! ## Type table
//!
//! `TypeTable` holds, unit by unit in `UnitNames` order, `PPTypeInfo` cells
//! (each the self-referencing cell in front of a type's RTTI), and ends each
//! unit's run with the value `1` - so there are exactly `UnitCount` markers.
//! A type the linker removed keeps its slot but points at a shared cell
//! holding nil. Empirically: HeidiSQL XE5 (8 340 slots, 212 units, 2 754
//! live) and HeidiSQL 12 x64 (14 578 slots, 474 units).
//!
//! ## Location
//!
//! - A package exports `@GetPackageInfoTable`, whose body is
//!   `B8 <table> C3` (`mov eax, table; ret`) or, on 64-bit,
//!   `48 8D 05 <rel32> C3` (`lea rax, [rip+table]; ret`).
//! - A program's entry point passes the table to `@InitExe` within its first
//!   instructions: `B8 <table> E8 <rel32>` (32-bit, after the frame setup and
//!   a console program's `IsConsole := True`) or
//!   `48 8D 0D <rel32> E8 <rel32>` (64-bit).
//!
//! Every candidate is validated before use: a plausible count, a `UnitInfo`
//! pointer just past the record, and every non-nil `Init` / `FInit` in code.

use std::{ops::Range, str};

use crate::{
    formats::BinaryContext,
    util::{read_ptr, read_short_string_at_va, read_u32},
};

/// Upper bound on the units one table may list; guards adversarial counts.
const MAX_UNITS: usize = 16_384;

/// Upper bound on the type-table slots one module may list.
const MAX_TYPE_SLOTS: usize = 1 << 20;

/// How many bytes of a program's entry point are searched for the table.
const ENTRY_SCAN_BYTES: usize = 96;

/// The decoded unit initialization table of a Delphi module.
#[derive(Debug, Clone)]
pub struct UnitInitTable<'a> {
    /// VA of the `PackageInfoTable` record.
    pub va: u64,
    /// The `UnitInfo` entries, in initialization order.
    pub units: Vec<UnitEntry>,
    /// The Delphi 2010+ type table, grouped by unit. `None` on earlier
    /// compilers.
    pub types: Option<Vec<UnitTypes<'a>>>,
}

/// One `UnitInfo` entry: a unit's `initialization` and `finalization`
/// procedures, or from Delphi 2010 a class constructor / destructor pair
/// (see the module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnitEntry {
    /// Code VA of the unit's `initialization` section, if it has one.
    pub init_va: Option<u64>,
    /// Code VA of the unit's `finalization` section, if it has one.
    pub finalize_va: Option<u64>,
}

/// The RTTI types one unit declares, from the Delphi 2010+ type table.
#[derive(Debug, Clone)]
pub struct UnitTypes<'a> {
    name: &'a [u8],
    /// VA of each surviving type's `PPTypeInfo` cell, in declaration order.
    /// Dereference once for the `PTypeInfo`.
    pub type_refs: Vec<u64>,
    /// Number of slots whose type the linker removed.
    pub removed: usize,
}

impl<'a> UnitTypes<'a> {
    /// Unit name, lossily decoded.
    #[inline]
    pub fn name(&self) -> &'a str {
        str::from_utf8(self.name).unwrap_or("<non-ascii>")
    }

    /// Raw unit-name bytes.
    #[inline]
    pub fn name_bytes(&self) -> &'a [u8] {
        self.name
    }
}

impl<'a> UnitInitTable<'a> {
    /// Locate and decode the module's table, from the
    /// `@GetPackageInfoTable` export or the program entry point. `None` for
    /// non-PE input or when no candidate validates.
    pub fn locate(ctx: &BinaryContext<'a>) -> Option<Self> {
        let ptr_size = ctx.pointer_size()?;
        candidates(ctx, ptr_size)
            .into_iter()
            .find_map(|va| Self::decode(ctx, va, ptr_size))
    }

    /// Decode the table at `va`. `None` unless it validates.
    pub fn decode(ctx: &BinaryContext<'a>, va: u64, ptr_size: usize) -> Option<Self> {
        let data = ctx.data();
        let off = ctx.va_to_file(va)?;
        let count = usize::try_from(read_u32(data, off)?).ok()?;
        if !(1..=MAX_UNITS).contains(&count) {
            return None;
        }
        let field = ptr_size; // each field occupies one pointer-aligned slot
        let info = read_ptr(data, off.checked_add(field)?, ptr_size)?;
        let ptr_bytes = u64::try_from(ptr_size).ok()?;
        let classic_end = va.checked_add(ptr_bytes.checked_mul(2)?)?;
        let extended_end = va.checked_add(ptr_bytes.checked_mul(6)?)?;
        let (extended, header_end) = if info == classic_end {
            (false, classic_end)
        } else if info == extended_end {
            (true, extended_end)
        } else {
            return None;
        };
        let units = read_units(ctx, header_end, count, ptr_size)?;
        let types = if extended {
            read_types(ctx, off, field, ptr_size)
        } else {
            None
        };
        Some(Self { va, units, types })
    }

    /// Returns the virtual addresses the record and its `UnitInfo` array
    /// occupy. The type table and unit names it points at lie elsewhere.
    pub fn extent(&self, ptr_size: usize) -> Option<Range<u64>> {
        let ptr_bytes = u64::try_from(ptr_size).ok()?;
        let header = ptr_bytes.checked_mul(if self.types.is_some() { 6 } else { 2 })?;
        let entries = ptr_bytes
            .checked_mul(2)?
            .checked_mul(u64::try_from(self.units.len()).ok()?)?;
        Some(self.va..self.va.checked_add(header)?.checked_add(entries)?)
    }
}

/// Collects candidate table VAs: the `@GetPackageInfoTable` export's
/// target, then the operand of the entry point's call to `@InitExe`.
fn candidates(ctx: &BinaryContext<'_>, ptr_size: usize) -> Vec<u64> {
    let mut out = Vec::new();
    let (Some(pe), Some(base)) = (ctx.pe(), ctx.image_base()) else {
        return out;
    };
    for export in &pe.exports {
        if export.name != Some("@GetPackageInfoTable") {
            continue;
        }
        if let Some(va) = base.checked_add(export.rva as u64)
            && let Some(code) = code_at(ctx, va, 8)
            && let Some(table) = getter_target(code, va, ptr_size)
        {
            out.push(table);
        }
    }
    if let Some(va) = base.checked_add(pe.entry as u64)
        && let Some(code) = code_at(ctx, va, ENTRY_SCAN_BYTES)
    {
        out.extend(entry_targets(code, va, ptr_size));
    }
    out
}

/// Decodes `@GetPackageInfoTable`'s `mov eax, table; ret` or
/// `lea rax, [rip+table]; ret`.
fn getter_target(code: &[u8], va: u64, ptr_size: usize) -> Option<u64> {
    match (ptr_size, code) {
        (4, [0xB8, a, b, c, d, 0xC3, ..]) => Some(u64::from(u32::from_le_bytes([*a, *b, *c, *d]))),
        (8, [0x48, 0x8D, 0x05, a, b, c, d, 0xC3, ..]) => {
            let rel = i64::from(i32::from_le_bytes([*a, *b, *c, *d]));
            va.checked_add(7)?.checked_add_signed(rel)
        }
        _ => None,
    }
}

/// Finds each `mov eax, imm32; call` (32-bit) or
/// `lea rcx, [rip+rel32]; call` (64-bit) operand in a program's first
/// instructions.
fn entry_targets(code: &[u8], va: u64, ptr_size: usize) -> Vec<u64> {
    let mut out = Vec::new();
    for at in 0..code.len() {
        let Some(window) = code.get(at..) else {
            break;
        };
        let target = match (ptr_size, window) {
            (4, [0xB8, a, b, c, d, 0xE8, ..]) => {
                Some(u64::from(u32::from_le_bytes([*a, *b, *c, *d])))
            }
            (8, [0x48, 0x8D, 0x0D, a, b, c, d, 0xE8, ..]) => {
                let rel = i64::from(i32::from_le_bytes([*a, *b, *c, *d]));
                u64::try_from(at)
                    .ok()
                    .and_then(|n| va.checked_add(n))
                    .and_then(|v| v.checked_add(7))
                    .and_then(|v| v.checked_add_signed(rel))
            }
            _ => None,
        };
        out.extend(target);
    }
    out
}

/// Borrows up to `len` bytes of code at `va`.
fn code_at<'a>(ctx: &BinaryContext<'a>, va: u64, len: usize) -> Option<&'a [u8]> {
    let off = ctx.va_to_file(va)?;
    let data = ctx.data();
    data.get(off..off.checked_add(len)?.min(data.len()))
}

/// Reads `count` `{Init, FInit}` pairs at `va`, requiring every pointer to
/// be nil, code, or the linker's removed-item placeholder.
fn read_units(
    ctx: &BinaryContext<'_>,
    va: u64,
    count: usize,
    ptr_size: usize,
) -> Option<Vec<UnitEntry>> {
    let data = ctx.data();
    let mut cursor = ctx.va_to_file(va)?;
    let mut out = Vec::with_capacity(count);
    let code = |p: u64| -> Option<Option<u64>> {
        match p {
            0 => Some(None),
            p if ctx.is_code_va(p) => Some(Some(p)),
            // Delphi XE+ points a unit the linker removed at a shared data
            // cell holding nil, the same placeholder the type table uses.
            p if ctx.va_to_file(p).and_then(|o| read_ptr(data, o, ptr_size)) == Some(0) => {
                Some(None)
            }
            _ => None,
        }
    };
    for _ in 0..count {
        let init = read_ptr(data, cursor, ptr_size)?;
        let finit = read_ptr(data, cursor.checked_add(ptr_size)?, ptr_size)?;
        out.push(UnitEntry {
            init_va: code(init)?,
            finalize_va: code(finit)?,
        });
        cursor = cursor.checked_add(ptr_size.checked_mul(2)?)?;
    }
    Some(out)
}

/// Reads the Delphi 2010+ `TypeInfo` part that starts in the third field of
/// the record at file offset `off`.
fn read_types<'a>(
    ctx: &BinaryContext<'a>,
    off: usize,
    field: usize,
    ptr_size: usize,
) -> Option<Vec<UnitTypes<'a>>> {
    let data = ctx.data();
    let at = |n: usize| off.checked_add(field.checked_mul(n)?);
    let type_count = usize::try_from(read_u32(data, at(2)?)?).ok()?;
    let table = read_ptr(data, at(3)?, ptr_size)?;
    let unit_count = usize::try_from(read_u32(data, at(4)?)?).ok()?;
    let mut name_va = read_ptr(data, at(5)?, ptr_size)?;
    if type_count > MAX_TYPE_SLOTS || unit_count > MAX_UNITS {
        return None;
    }
    let mut cursor = ctx.va_to_file(table)?;
    let mut units = Vec::with_capacity(unit_count);
    let mut slots = 0usize;
    for _ in 0..unit_count {
        let name = read_short_string_at_va(ctx, name_va)?;
        name_va = name_va.checked_add(u64::try_from(name.len()).ok()?.checked_add(1)?)?;
        let mut unit = UnitTypes {
            name,
            type_refs: Vec::new(),
            removed: 0,
        };
        loop {
            if slots >= type_count {
                return None;
            }
            let slot = read_ptr(data, cursor, ptr_size)?;
            cursor = cursor.checked_add(ptr_size)?;
            slots = slots.checked_add(1)?;
            if slot == 1 {
                break;
            }
            let type_info = ctx
                .va_to_file(slot)
                .and_then(|o| read_ptr(data, o, ptr_size))?;
            if type_info == 0 {
                unit.removed = unit.removed.checked_add(1)?;
            } else {
                unit.type_refs.push(slot);
            }
        }
        units.push(unit);
    }
    (slots == type_count).then_some(units)
}
