//! Published-field table decoder.
//!
//! Reads the table referenced by `vmtFieldTable`. Delphi emits either the
//! legacy layout (a single count + entries that index into a separate
//! field-classes table) or the modern layout (post-2010 extended RTTI,
//! one self-contained entry per field with a direct `PPTypeInfo` pointer).
//! FPC emits yet another shape.
//!
//! ## Delphi layout
//!
//! Source: Delphi 6 `Classes.pas` (`TFieldClassTable`), RAD Studio 10
//! Seattle's `System.TypInfo` (`TVmtFieldTable`, `TVmtFieldExEntry`),
//! IDR64 `IDCGen.cpp` (`OutputFieldTable`); empirically validated
//! on Delphi 6, 7, XE5, 10 Seattle (Win32 + Win64) and 12.
//!
//! ```text
//!   Count:     u16           published fields
//!   ClassTab:  ptr           class-types table (see Field::class_table)
//!   entries[Count]:
//!     Offset:    u32         instance-relative byte offset
//!     TypeIndex: u16         index into ClassTab
//!     Name:      ShortString
//!   -- Delphi 2010+ only --
//!   ExCount:   u16           every field the class declares
//!   ex-entries[ExCount]:     see ExtendedField
//! ```
//!
//! [`Field::iter`] reads the published entries; [`ExtendedField::iter`]
//! reads the extension. (An earlier reading took a zero `Count` as a
//! different "modern" layout with four unknown bytes: those were the
//! `ClassTab` pointer, and the extension after a non-zero `Count` was
//! never read.)
//!
//! ## FPC layout
//!
//! Source: FPC `rtl/objpas/typinfo.pp:214-250`.
//!
//! ```text
//!   TVmtFieldTable:
//!     Count:     u16
//!     ClassTab:  ptr         (PVmtFieldClassTab - array of PClass)
//!     Fields:    TVmtFieldEntry[Count]
//!
//!   TVmtFieldEntry (variable size):
//!     FieldOffset: SizeUInt  (ptr-sized)
//!     TypeIndex:   u16
//!     Name:        ShortString
//! ```
//!
//! ## Allocation
//!
//! The field-name slice borrows from the caller's byte buffer.

use std::{ops::Range, str};

use crate::{
    extrtti::{AttributeEntry, Visibility, read_attr_data},
    formats::BinaryContext,
    limits::{MAX_FIELDS_PER_CLASS, MAX_IDENTIFIER_BYTES},
    util::{as_identifier, read_ptr, read_short_string_at_file, read_u16, read_u32},
    vmt::{Vmt, VmtFlavor},
};

/// One published field of a class.
#[derive(Debug, Clone, Copy)]
pub struct Field<'a> {
    /// Byte offset of the field within the instance (relative to the
    /// object's start).
    pub offset: u32,
    /// A table index into the class-types array (Delphi and FPC published
    /// fields).
    pub type_ref: FieldTypeRef,
    /// Field name; borrows from the input buffer.
    name: &'a [u8],
}

impl<'a> Field<'a> {
    /// Field name as `&str`: UTF-8, as Delphi 2009 and later store names, or `"<non-ascii>"`
    /// when the bytes are not valid UTF-8.
    #[inline]
    pub fn name(&self) -> &'a str {
        str::from_utf8(self.name).unwrap_or("<non-ascii>")
    }

    /// Raw field name bytes (borrows from the input buffer).
    #[inline]
    pub fn name_bytes(&self) -> &'a [u8] {
        self.name
    }
}

/// How a field's type is referenced.
#[derive(Debug, Clone, Copy)]
pub enum FieldTypeRef {
    /// Direct `PPTypeInfo` VA. No current decoder produces it: Delphi
    /// 2010+ fields with a `PPTypeInfo` are [`ExtendedField`]s.
    TypeInfoPtr(u64),
    /// Index into the class-types table (published fields).
    TypeIndex(u16),
}

impl<'a> Field<'a> {
    /// Decodes every published field declared on `vmt`. Returns an
    /// empty vector if the class has no field table or the table is
    /// malformed.
    pub fn iter(ctx: &BinaryContext<'a>, vmt: &Vmt<'a>) -> Vec<Self> {
        if vmt.field_table == 0 {
            return Vec::new();
        }
        let result = match vmt.flavor {
            VmtFlavor::Delphi => iter_delphi(ctx, vmt),
            VmtFlavor::Fpc => iter_fpc(ctx, vmt),
        };
        result.map_or_else(
            || {
                crate::__undelphi_trace_warn!(
                    vmt_va = vmt.va,
                    field_table = vmt.field_table,
                    "Field::iter: field-table walk bailed out"
                );
                Vec::new()
            },
            |(fields, _)| fields,
        )
    }

    /// Returns the virtual addresses `vmt`'s published-field table occupies:
    /// its header and its entries.
    ///
    /// `None` when the class has no field table or its walk bails out. The
    /// class-types table a legacy header points at lies elsewhere and is not
    /// included.
    pub fn table_extent(ctx: &BinaryContext<'a>, vmt: &Vmt<'a>) -> Option<Range<u64>> {
        if vmt.field_table == 0 {
            return None;
        }
        let base_off = ctx.va_to_file(vmt.field_table)?;
        let end_off = match vmt.flavor {
            VmtFlavor::Delphi => match read_extended_delphi(ctx, vmt) {
                Some((_, end)) => end,
                None => iter_delphi(ctx, vmt)?.1,
            },
            VmtFlavor::Fpc => iter_fpc(ctx, vmt)?.1,
        };
        let len = u64::try_from(end_off.checked_sub(base_off)?).ok()?;
        Some(vmt.field_table..vmt.field_table.checked_add(len)?)
    }

    /// Reads the class-types table a legacy Delphi or FPC field table points
    /// at: the VA stored in each of its `^TClass` cells, in
    /// [`FieldTypeRef::TypeIndex`] order.
    ///
    /// Delphi points each cell at the target VMT's `vmtSelfPtr` slot, which
    /// is the VMT base [`Vmt::va`] names; FPC stores a `PClass` that must be
    /// dereferenced once. In a binary linked against runtime packages a cell
    /// is an import slot. Empty when the class has no field table or no
    /// published fields, or the table is malformed.
    pub fn class_table(ctx: &BinaryContext<'a>, vmt: &Vmt<'a>) -> Vec<u64> {
        read_class_table(ctx, vmt).map_or_else(Vec::new, |(_, cells)| cells)
    }

    /// Returns the virtual addresses `vmt`'s field class-types table
    /// occupies: its `u16` count and its pointer-sized cells.
    ///
    /// `None` when [`Field::class_table`] finds no table.
    pub fn class_table_extent(ctx: &BinaryContext<'a>, vmt: &Vmt<'a>) -> Option<Range<u64>> {
        read_class_table(ctx, vmt).map(|(extent, _)| extent)
    }
}

/// Walks the class-types table behind a legacy Delphi or FPC field table,
/// answering its extent and the class reference in each cell.
///
/// Delphi 6 declares it as `TFieldClassTable = packed record Count:
/// Smallint; Classes: array of ^TPersistentClass end` (`Classes.pas`); FPC's
/// `TVmtFieldClassTab` is the same, but pointer-aligned on
/// `FPC_REQUIRES_PROPER_ALIGNMENT` targets (`typinfo.pp`).
fn read_class_table(ctx: &BinaryContext<'_>, vmt: &Vmt<'_>) -> Option<(Range<u64>, Vec<u64>)> {
    if vmt.field_table == 0 {
        return None;
    }
    let data = ctx.data();
    let psize = vmt.pointer_size as usize;
    let base_off = ctx.va_to_file(vmt.field_table)?;
    // No published fields means nothing indexes a class table.
    if vmt.flavor == VmtFlavor::Delphi && read_u16(data, base_off)? == 0 {
        return None;
    }
    let aligned = vmt.flavor == VmtFlavor::Fpc && ptr_aligned_on_non_x86(ctx);
    let after_count = base_off.checked_add(2)?;
    let classtab_off = if aligned {
        align_up(after_count, psize)?
    } else {
        after_count
    };
    let table_va = read_ptr(data, classtab_off, psize)?;
    if table_va == 0 {
        return None;
    }
    let table_off = ctx.va_to_file(table_va)?;
    let count = usize::from(read_u16(data, table_off)?);
    if count > MAX_FIELDS_PER_CLASS {
        return None;
    }
    let mut cursor = table_off.checked_add(2)?;
    if aligned {
        cursor = align_up(cursor, psize)?;
    }
    let mut cells = Vec::with_capacity(count);
    for _ in 0..count {
        cells.push(read_ptr(data, cursor, psize)?);
        cursor = cursor.checked_add(psize)?;
    }
    let len = u64::try_from(cursor.checked_sub(table_off)?).ok()?;
    Some((table_va..table_va.checked_add(len)?, cells))
}

/// Walks a Delphi published-field table, answering its published fields
/// and the file offset just past them - where the Delphi 2010+ extension
/// begins.
///
/// The table always opens with `Count: u16`, `ClassTab: Pointer` and
/// `Count` entries of `{Offset: u32; TypeIndex: u16; Name: ShortString}`,
/// whether or not `Count` is zero (Delphi 6 `Classes.pas`; Seattle's
/// `System.TypInfo` `TVmtFieldTable`).
fn iter_delphi<'a>(ctx: &BinaryContext<'a>, vmt: &Vmt<'a>) -> Option<(Vec<Field<'a>>, usize)> {
    let base_off = ctx.va_to_file(vmt.field_table)?;
    let data = ctx.data();
    let count = usize::from(read_u16(data, base_off)?);
    if count > MAX_FIELDS_PER_CLASS {
        return None;
    }
    let psize = vmt.pointer_size as usize;
    let mut cursor = base_off.checked_add(2)?.checked_add(psize)?;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let offset = read_u32(data, cursor)?;
        let type_index = read_u16(data, cursor.checked_add(4)?)?;
        let name = read_short_string_at_file(data, cursor.checked_add(6)?)?;
        if !is_plausible_name(name) {
            return None;
        }
        out.push(Field {
            offset,
            type_ref: FieldTypeRef::TypeIndex(type_index),
            name,
        });
        cursor = cursor.checked_add(7)?.checked_add(name.len())?;
    }
    Some((out, cursor))
}

/// One field of a class's Delphi 2010+ extended field table: every field
/// the class declares - private, protected and public as well as
/// published - with its type, offset and attributes.
///
/// After the published entries the table continues with `ExCount: u16` and
/// `ExCount` records (Seattle `System.TypInfo` `TVmtFieldExEntry`; IDR64
/// `IDCGen.cpp` `OutputFieldTable`):
///
/// ```text
///   Flags:    u8            low two bits: visibility
///   TypeRef:  PPTypeInfo
///   Offset:   u32           instance-relative
///   Name:     ShortString
///   AttrData: u16 + bytes   length counts itself; 2 = no attributes
/// ```
///
/// Validated against RAD Studio 10 Seattle's `TStringList` (Win32 and
/// Win64): `FList`, `FCount`, `FCapacity`, `FSorted`, `FDuplicates`,
/// `FCaseSensitive`, `FOnChange`, `FOnChanging`, `FOwnsObject`.
#[derive(Debug, Clone)]
pub struct ExtendedField<'a> {
    /// Declared visibility.
    pub visibility: Visibility,
    /// Raw flags byte (low two bits are the visibility).
    pub flags: u8,
    /// `PPTypeInfo` of the field's type; an import slot when the type lives
    /// in a runtime package.
    pub type_ref: u64,
    /// Byte offset of the field within the instance.
    pub offset: u32,
    name: &'a [u8],
    /// The field's attributes (`[Name(...)]` before the declaration).
    pub attributes: Vec<AttributeEntry<'a>>,
}

impl<'a> ExtendedField<'a> {
    /// Field name: UTF-8, as Delphi 2009 and later store names, or `"<non-ascii>"` when the
    /// bytes are not valid UTF-8.
    #[inline]
    pub fn name(&self) -> &'a str {
        str::from_utf8(self.name).unwrap_or("<non-ascii>")
    }

    /// Raw field-name bytes.
    #[inline]
    pub fn name_bytes(&self) -> &'a [u8] {
        self.name
    }

    /// Decodes the extended field table of `vmt`. Empty for VMT headers
    /// older than Delphi 2009 (the table ends with the published entries),
    /// on FPC, and when any entry fails validation;
    /// [`DelphiBinary::extended_fields`](crate::DelphiBinary::extended_fields)
    /// also skips binaries whose compiler predates Delphi 2010.
    pub fn iter(ctx: &BinaryContext<'a>, vmt: &Vmt<'a>) -> Vec<Self> {
        read_extended_delphi(ctx, vmt).map_or_else(Vec::new, |(fields, _)| fields)
    }
}

/// Reads the Delphi 2010+ extension of `vmt`'s field table, answering its
/// entries and the file offset just past them. Every entry must have a
/// plausible name, an offset inside the instance and a well-formed
/// attribute block.
fn read_extended_delphi<'a>(
    ctx: &BinaryContext<'a>,
    vmt: &Vmt<'a>,
) -> Option<(Vec<ExtendedField<'a>>, usize)> {
    if vmt.field_table == 0 || !vmt.is_delphi_2009_or_later() {
        return None;
    }
    let (_, legacy_end) = iter_delphi(ctx, vmt)?;
    let data = ctx.data();
    let count = usize::from(read_u16(data, legacy_end)?);
    if count > MAX_FIELDS_PER_CLASS {
        return None;
    }
    let psize = vmt.pointer_size as usize;
    let mut cursor = legacy_end.checked_add(2)?;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let flags = *data.get(cursor)?;
        let type_ref = read_ptr(data, cursor.checked_add(1)?, psize)?;
        let offset_off = cursor.checked_add(1)?.checked_add(psize)?;
        let offset = read_u32(data, offset_off)?;
        let name_off = offset_off.checked_add(4)?;
        let name = read_short_string_at_file(data, name_off)?;
        if !is_plausible_name(name) || (vmt.instance_size != 0 && offset >= vmt.instance_size) {
            return None;
        }
        let attr_off = name_off.checked_add(1)?.checked_add(name.len())?;
        let (attributes, next) = read_attr_data(data, attr_off, psize)?;
        out.push(ExtendedField {
            visibility: Visibility::from_flags(flags),
            flags,
            type_ref,
            offset,
            name,
            attributes,
        });
        cursor = next;
    }
    Some((out, cursor))
}

/// Walks an FPC published-field table, answering its fields and the file
/// offset just past the last.
fn iter_fpc<'a>(ctx: &BinaryContext<'a>, vmt: &Vmt<'a>) -> Option<(Vec<Field<'a>>, usize)> {
    let base_off = ctx.va_to_file(vmt.field_table)?;
    let data = ctx.data();
    let psize = vmt.pointer_size as usize;
    let count = read_u16(data, base_off)? as usize;
    if count > MAX_FIELDS_PER_CLASS {
        return None;
    }
    // On Mach-O / ELF with FPC's `FPC_REQUIRES_PROPER_ALIGNMENT` (enabled
    // on non-x86 targets), the `ClassTab: pointer` field is pointer-aligned
    // after the `Count: u16`, which inserts `ptr_size - 2` bytes of padding.
    // On PE packs are kept.
    let needs_alignment = ptr_aligned_on_non_x86(ctx);
    let after_count = base_off.checked_add(2)?;
    let classtab_off = if needs_alignment {
        align_up(after_count, psize)?
    } else {
        after_count
    };
    let mut cursor = classtab_off.checked_add(psize)?;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        if needs_alignment {
            cursor = align_up(cursor, psize)?;
        }
        let offset = read_ptr(data, cursor, psize)? as u32;
        let ti_off = cursor.checked_add(psize)?;
        let name_off = ti_off.checked_add(2)?;
        let type_index = read_u16(data, ti_off)?;
        let name = read_short_string_at_file(data, name_off)?;
        if !is_plausible_name(name) {
            return None;
        }
        let advance = psize
            .checked_add(2)?
            .checked_add(1)?
            .checked_add(name.len())?;
        out.push(Field {
            offset,
            type_ref: FieldTypeRef::TypeIndex(type_index),
            name,
        });
        cursor = cursor.checked_add(advance)?;
    }
    Some((out, cursor))
}

fn ptr_aligned_on_non_x86(ctx: &BinaryContext<'_>) -> bool {
    // FPC pointer-aligns field-table records only on
    // `FPC_REQUIRES_PROPER_ALIGNMENT` architectures (ARM / AArch64). x86 /
    // x86-64 stay packed on every container - PE *and* ELF / Mach-O - so the
    // decision is made from the architecture, not from "is this PE?" (which
    // wrongly aligned x86-64 ELF / Mach-O).
    ctx.target_arch().fpc_requires_proper_alignment()
}

#[inline]
fn align_up(off: usize, to: usize) -> Option<usize> {
    if to == 0 {
        return Some(off);
    }
    let rem = off.checked_rem(to)?;
    if rem == 0 {
        Some(off)
    } else {
        let pad = to.checked_sub(rem)?;
        off.checked_add(pad)
    }
}

fn is_plausible_name(name: &[u8]) -> bool {
    name.len() <= MAX_IDENTIFIER_BYTES && as_identifier(name, &[]).is_some()
}
