//! Decoders for three of the VMT's auxiliary tables that carry runtime
//! metadata beyond the method / field / interface / type-info tables
//! already covered by dedicated modules: the init (managed-fields) table,
//! the dynamic-dispatch table and the automation table.
//!
//! ## Init table (`vmtInitTable`)
//!
//! This slot points at a `PTypeInfo` whose `Kind` is `tkRecord` and
//! whose managed-fields list names every instance offset that carries a
//! ref-counted reference (strings, interfaces, dynamic arrays, other
//! managed records). The runtime consumes this table during
//! `AfterConstruction` and `Destroy` to initialise and finalise those
//! fields. Decoding it is a straight reuse of
//! [`crate::rtti::RecordInfo::from_va`].
//!
//! Source: FPC `rtl/inc/rtti.inc` - `InitializeArray` / `FinalizeArray`.
//!
//! ## Dynamic-dispatch table (`vmtDynamicTable`)
//!
//! Dynamic methods are declared with `dynamic;` or `message <ID>;` and
//! their dispatch goes through a separate lookup table rather than the
//! virtual method pointer array - the point being that derived classes
//! don't grow the VMT for every dynamic method.
//!
//! On-disk layout (both Delphi and FPC):
//!
//! ```text
//!   Count:     u16
//!   Indexes:   [i16; Count]    (message ID per slot)
//!   Handlers:  [ptr; Count]    (code pointer per slot, same order)
//! ```
//!
//! The indexes come first so the runtime can do a tight binary search on
//! a packed i16 array without touching the handler pointers.
//!
//! ## Automation table (`vmtAutoTable`)
//!
//! A class's `automated` section (the Delphi 2 OLE-automation model, still
//! accepted by Delphi 13 on Win32 and Win64) is described by this table,
//! which `OleAuto.pas` dispatches `IDispatch.Invoke` calls through. Delphi
//! only. The record is packed: on Win64 the pointers widen to eight bytes
//! but the count and the `i32` fields stay unpadded (32-byte entries).
//!
//! Source: Delphi 6 `OleAuto.pas` (`TAutoTable`, `TAutoEntry`,
//! `TParamList`); empirically validated against `tables.exe` as Delphi 6
//! and Delphi 13 Win32 / Win64 build it (`tests/samples/bcb6`,
//! `tests/samples/d13`).
//!
//! ```text
//!   EntryCount: i32
//!   entries[EntryCount]:
//!     DispID:   i32
//!     Name:     ptr -> ShortString
//!     Flags:    u32           (afMethod 1, afPropGet 2, afPropSet 4,
//!                              afVirtual 8)
//!     Params:   ptr -> { ResType: u8; ParamCount: u8;
//!                        ParamTypes: [u8; ParamCount] }
//!     Address:  ptr           (code VA, or a VMT byte offset when
//!                              afVirtual is set)
//! ```
//!
//! Dcc32 emits the entries in reverse declaration order, a property's
//! setter before its getter. The parameter bytes are `Variant` type codes
//! (`varInteger` = 3, ...) with `0x80` marking a by-reference parameter.
//!
//! The returned dynamic-slot vectors contain plain values (no borrowed
//! slices), so they don't carry the input lifetime.

use std::{ops::Range, str};

use crate::{
    formats::BinaryContext,
    limits::{MAX_AUTO_ENTRIES_PER_CLASS, MAX_AUTO_PARAMS, MAX_DYNAMIC_SLOTS_PER_CLASS},
    rtti::RecordInfo,
    util::{read_ptr, read_short_string_at_file, read_u16, read_u32},
    vmt::{Vmt, VmtFlavor},
};

impl<'a> RecordInfo<'a> {
    /// Decode the class's init (managed-fields) table - the
    /// `vmtInitTable` synthetic record that enumerates instance
    /// offsets the runtime needs to refcount-manage. Returns `None`
    /// when `vmtInitTable` is null or the record can't be decoded.
    pub fn from_init_table(ctx: &BinaryContext<'a>, vmt: &Vmt<'a>) -> Option<Self> {
        if vmt.init_table == 0 {
            return None;
        }
        Self::from_va(ctx, vmt.init_table, vmt.flavor)
    }
}

/// One entry in the dynamic-dispatch table.
#[derive(Debug, Clone, Copy)]
pub struct DynamicSlot {
    /// Dispatch index (message ID for `message` declarations, or a
    /// compiler-assigned negative index for plain `dynamic` methods).
    pub index: i16,
    /// Code VA of the handler.
    pub handler_va: u64,
}

impl DynamicSlot {
    /// Returns the virtual addresses `vmt`'s dynamic-dispatch table
    /// occupies: a `u16` count, the count's `u16` indexes, then as many
    /// pointer-sized handlers.
    ///
    /// `None` when the class has none, or its count is implausible.
    pub fn table_extent(ctx: &BinaryContext<'_>, vmt: &Vmt<'_>) -> Option<Range<u64>> {
        if vmt.dynamic_table == 0 {
            return None;
        }
        let count = usize::from(read_u16(ctx.data(), ctx.va_to_file(vmt.dynamic_table)?)?);
        if count > MAX_DYNAMIC_SLOTS_PER_CLASS {
            return None;
        }
        let entry = usize::from(vmt.pointer_size).checked_add(2)?;
        let len = u64::try_from(count.checked_mul(entry)?.checked_add(2)?).ok()?;
        Some(vmt.dynamic_table..vmt.dynamic_table.checked_add(len)?)
    }

    /// Decode the dynamic-dispatch table for `vmt`. Returns an empty
    /// vector when `vmtDynamicTable` is null or the layout walk
    /// bails.
    pub fn iter(ctx: &BinaryContext<'_>, vmt: &Vmt<'_>) -> Vec<Self> {
        if vmt.dynamic_table == 0 {
            return Vec::new();
        }
        let Some(file_off) = ctx.va_to_file(vmt.dynamic_table) else {
            return Vec::new();
        };
        let data = ctx.data();
        let Some(count) = read_u16(data, file_off).map(usize::from) else {
            return Vec::new();
        };
        if count == 0 || count > MAX_DYNAMIC_SLOTS_PER_CLASS {
            return Vec::new();
        }
        let psize = vmt.pointer_size as usize;
        let mut out = Vec::with_capacity(count);
        let Some(indexes_off) = file_off.checked_add(2) else {
            return Vec::new();
        };
        let Some(handlers_off) = count
            .checked_mul(2)
            .and_then(|n| indexes_off.checked_add(n))
        else {
            return Vec::new();
        };
        for i in 0..count {
            let Some(index_off) = i.checked_mul(2).and_then(|n| indexes_off.checked_add(n)) else {
                break;
            };
            let Some(index) = read_u16(data, index_off).map(|u| u as i16) else {
                break;
            };
            let Some(handler_off) = i
                .checked_mul(psize)
                .and_then(|n| handlers_off.checked_add(n))
            else {
                break;
            };
            let Some(handler_va) = read_ptr(data, handler_off, psize) else {
                break;
            };
            out.push(Self { index, handler_va });
        }
        out
    }
}

/// Size of one packed `TAutoEntry`: the `i32` `DispID` and `Flags` and the
/// pointer-sized `Name`, `Params` and `Address` (20 bytes on 32-bit, 32 on
/// Win64, where the count is still an unpadded `i32`).
fn auto_entry_size(pointer_size: usize) -> Option<usize> {
    pointer_size.checked_mul(3)?.checked_add(8)
}

/// One entry in a class's automation table (`vmtAutoTable`): a method or
/// property accessor declared in an `automated` section.
#[derive(Debug, Clone)]
pub struct AutoEntry<'a> {
    /// The `dispid` the member was declared with.
    pub dispid: i32,
    /// Member name; borrows from the input buffer.
    name: &'a [u8],
    /// Raw `af*` flags; see [`Self::is_method`] and friends.
    pub flags: u32,
    /// `Variant` type code of the result (`0` for none).
    pub result_type: u8,
    /// `Variant` type code of each parameter, in declaration order; `0x80`
    /// marks a by-reference parameter.
    pub param_types: Vec<u8>,
    /// Code VA of the implementation, or a VMT byte offset when
    /// [`Self::is_virtual`].
    pub address: u64,
}

impl<'a> AutoEntry<'a> {
    /// Member name as `&str`, lossily decoded.
    #[inline]
    pub fn name(&self) -> &'a str {
        str::from_utf8(self.name).unwrap_or("<non-ascii>")
    }

    /// Raw member name bytes.
    #[inline]
    pub fn name_bytes(&self) -> &'a [u8] {
        self.name
    }

    /// Returns `true` for an automated method (`afMethod`).
    #[inline]
    pub fn is_method(&self) -> bool {
        self.flags & 0x1 != 0
    }

    /// Returns `true` for a property getter (`afPropGet`).
    #[inline]
    pub fn is_getter(&self) -> bool {
        self.flags & 0x2 != 0
    }

    /// Returns `true` for a property setter (`afPropSet`).
    #[inline]
    pub fn is_setter(&self) -> bool {
        self.flags & 0x4 != 0
    }

    /// Returns `true` when [`Self::address`] is a VMT offset (`afVirtual`).
    #[inline]
    pub fn is_virtual(&self) -> bool {
        self.flags & 0x8 != 0
    }

    /// Decode the automation table of `vmt`. Returns an empty vector when
    /// `vmtAutoTable` is null, the binary is not a Delphi image, or the
    /// table is malformed.
    pub fn iter(ctx: &BinaryContext<'a>, vmt: &Vmt<'a>) -> Vec<Self> {
        read_auto_table(ctx, vmt).unwrap_or_default()
    }

    /// Returns the virtual addresses `vmt`'s automation table occupies: its
    /// `i32` count and its fixed-size entries. The names and parameter lists
    /// the entries point at lie elsewhere and are not included.
    ///
    /// `None` when [`Self::iter`] finds no table.
    pub fn table_extent(ctx: &BinaryContext<'a>, vmt: &Vmt<'a>) -> Option<Range<u64>> {
        let count = auto_entry_count(ctx, vmt)?;
        read_auto_table(ctx, vmt)?;
        let size = auto_entry_size(usize::from(vmt.pointer_size))?;
        let len = u64::try_from(count.checked_mul(size)?.checked_add(4)?).ok()?;
        Some(vmt.auto_table..vmt.auto_table.checked_add(len)?)
    }
}

/// Reads the entry count of `vmt`'s automation table, when it has one.
fn auto_entry_count(ctx: &BinaryContext<'_>, vmt: &Vmt<'_>) -> Option<usize> {
    if vmt.auto_table == 0 || vmt.flavor != VmtFlavor::Delphi {
        return None;
    }
    let count = usize::try_from(read_u32(ctx.data(), ctx.va_to_file(vmt.auto_table)?)?).ok()?;
    (1..=MAX_AUTO_ENTRIES_PER_CLASS)
        .contains(&count)
        .then_some(count)
}

/// Walks `vmt`'s automation table. `None` when any entry fails to decode.
fn read_auto_table<'a>(ctx: &BinaryContext<'a>, vmt: &Vmt<'a>) -> Option<Vec<AutoEntry<'a>>> {
    let count = auto_entry_count(ctx, vmt)?;
    let data = ctx.data();
    let p = usize::from(vmt.pointer_size);
    let size = auto_entry_size(p)?;
    let mut cursor = ctx.va_to_file(vmt.auto_table)?.checked_add(4)?;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let dispid = read_u32(data, cursor)? as i32;
        let name_va = read_ptr(data, cursor.checked_add(4)?, p)?;
        let flags = read_u32(data, cursor.checked_add(p.checked_add(4)?)?)?;
        let params_va = read_ptr(data, cursor.checked_add(p.checked_add(8)?)?, p)?;
        let address = read_ptr(
            data,
            cursor.checked_add(p.checked_mul(2)?.checked_add(8)?)?,
            p,
        )?;
        if flags & !0xF != 0 {
            return None;
        }
        let name = read_short_string_at_file(data, ctx.va_to_file(name_va)?)?;
        if name.is_empty() || !name.iter().all(|&b| b.is_ascii_alphanumeric() || b == b'_') {
            return None;
        }
        let params_off = ctx.va_to_file(params_va)?;
        let result_type = *data.get(params_off)?;
        let param_count = usize::from(*data.get(params_off.checked_add(1)?)?);
        if param_count > MAX_AUTO_PARAMS {
            return None;
        }
        let types_off = params_off.checked_add(2)?;
        let param_types = data
            .get(types_off..types_off.checked_add(param_count)?)?
            .to_vec();
        out.push(AutoEntry {
            dispid,
            name,
            flags,
            result_type,
            param_types,
            address,
        });
        cursor = cursor.checked_add(size)?;
    }
    Some(out)
}
