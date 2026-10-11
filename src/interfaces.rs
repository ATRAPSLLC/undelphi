//! Interface-table iterator.
//!
//! Delphi and FPC use **different** interface table layouts.
//!
//! ## Delphi layout
//!
//! Source: pythia `pythia/core/structures.py:273-283`. The table
//! itself is length-prefixed; each entry inlines a 16-byte GUID followed by
//! three pointer-sized slots.
//!
//! ```text
//!   NumEntries: u32
//!   entries[NumEntries]:
//!     Guid:      [u8; 16]        (inline, as `{D1 : D2 : D3 : D4[]}`)
//!     VtablePtr: ptr
//!     IOffset:   u32             (classic Delphi kept this 32-bit)
//!     GetterPtr: ptr
//! ```
//!
//! On 64-bit Delphi the 3 pointer slots grow to 8 bytes each and `IOffset`
//! becomes a 4-byte field (still 32-bit), often padded to 8 bytes. The
//! implementation below auto-computes entry size from the slot layout.
//!
//! ## FPC layout
//!
//! Source: FPC `rtl/inc/objpash.inc:183-208` - FPC's
//! `tinterfaceentry` and `tinterfacetable`.
//!
//! ```text
//!   EntryCount: SizeUInt         (ptr-sized, NOT u32)
//!   entries[EntryCount]:
//!     IIDRef:           ptr      (pointer to PGuid - two-level indirection)
//!     VTable:           ptr
//!     IOffset:          sizeuint (ptr-sized, union with IOffsetAsCodePtr)
//!     IIDStrRef:        ptr      (pointer to PShortString)
//!     IType:            u8       (one-byte enum, then padding)
//! ```
//!
//! FPC's entry is variant-laid-out: the first three fields are always
//! present; `IIDStrRef` and `IType` are part of the Corba-interface
//! overlay. Classic COM-interface entries only include the first three
//! fields, with `IIDRef` pointing at a usable GUID. We decode the
//! first-three-fields prefix and fetch the GUID via `IIDRef`.
//!
//! The two reference fields changed shape between releases. FPC 3.2 (and
//! the trunk source above) store `^PGuid` / `^PShortString` - two levels of
//! indirection, for dynamic packages. FPC 3.0 stores `IID: PGuid` /
//! `IIDStr: PShortString`, pointing straight at the 16 GUID bytes and at
//! the text (empirically, `cheatengine-x86_64.exe`, FPC 3.0.4). A COM
//! entry's text is its upper-cased `GuidToString`, so the one-level reading
//! is taken exactly when the GUID read at `IIDRef` formats to the string
//! read at `IIDStrRef` - a pointer pair cannot satisfy that by accident.
//! A Corba entry has no GUID (a null `IID`, or a `^PGuid` cell holding
//! null); it is reported with a zero GUID and its name in
//! [`InterfaceEntry::iid_str`]. This path follows the record layout but is
//! not yet exercised by a sample: the zero-GUID entries in the corpus are
//! COM interfaces declared without a GUID, whose text is empty.
//!
//! ## Interface vtables and adjustor thunks
//!
//! An interface reference points at the interface's slot *inside* the
//! object (`IOffset` bytes in), not at the object. Each entry's `VTable`
//! therefore holds compiler-generated **adjustor thunks**, one per method:
//! each moves `Self` back by `IOffset` and transfers to the implementing
//! method. [`InterfaceThunk::decode`] recognises every encoding seen across
//! Delphi 6 / 7 / XE5 / 12, C++Builder 6 and FPC 3.0 / 3.2 (32- and 64-bit):
//!
//! ```text
//!   Self adjustment (add a negative value, or subtract IOffset)
//!     32-bit, Self on the stack   83 44 24 04 ib | 81 44 24 04 id   add [esp+4]
//!                                 83 6C 24 04 ib | 81 6C 24 04 id   sub [esp+4]   (FPC)
//!     32-bit, Self in eax         83 C0 ib | 05 id                  add eax
//!                                 83 E8 ib | 2D id                  sub eax       (FPC)
//!     64-bit, Self in rcx         48 83 C1 ib | 48 81 C1 id         add rcx       (Win64)
//!                                 48 83 E9 ib | 48 81 E9 id         sub rcx
//!     64-bit, Self in rdi         48 83 C7 ib | 48 81 C7 id         add rdi       (SysV)
//!                                 48 83 EF ib | 48 81 EF id         sub rdi       (FPC)
//!   Transfer
//!     direct                      E9 rel32 | EB rel8
//!     virtual (VMT byte offset)   8B 44 24 04 8B 00 FF 60 ib|FF A0 id       Delphi, stack Self
//!                                 50 8B 00 8B 40 ib|8B 80 id 87 04 24 C3    Delphi, eax Self
//!                                 8B 44 24 04 8B 08 FF 61 ib|FF A1 id       FPC, stack Self
//!                                 8B 08 FF 61 ib|FF A1 id                   FPC, eax Self
//!                                 53 50 8B 00 8B 40 ib|8B 80 id
//!                                       89 44 24 04 58 C3                   FPC, eax Self
//!                                 48 8B 01 FF 60 ib|FF A0 id                Win64
//!                                 48 8B 07 FF 60 ib|FF A0 id                SysV
//! ```
//!
//! A displacement of zero is encoded without one (`FF 20`, `FF 21`, `8B 00`
//! in place of `FF 60 ib` / `FF 61 ib` / `8B 40 ib`) and decodes as offset 0.
//!
//! The adjustment always equals `-IOffset`, which also bounds the vtable:
//! the walk stops at the first slot whose thunk adjusts by anything else.
//! A virtual transfer means the interface method is implemented by a
//! virtual method; its target depends on the runtime class and resolves
//! against that class's VMT ([`crate::DelphiBinary::interface_method_target`]).

use std::{fmt, ops::Range, str};

use crate::{
    formats::BinaryContext,
    limits::{MAX_INTERFACES_PER_CLASS, MAX_INTERFACES_PER_CLASS_FPC, MAX_METHODS_PER_CLASS},
    properties::{Access, AccessKind},
    util::{read_ptr, read_short_string_at_va, read_u32},
    vmt::{Vmt, VmtFlavor},
};

/// A 128-bit GUID, stored in the Microsoft canonical record form.
///
/// Formatting matches Delphi's `GuidToString` / Microsoft's
/// `StringFromGUID2`: `{D1-D2-D3-D4[0..2]-D4[2..8]}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Guid {
    /// First 32-bit component (little-endian).
    pub data1: u32,
    /// Next 16-bit component (little-endian).
    pub data2: u16,
    /// Next 16-bit component (little-endian).
    pub data3: u16,
    /// Final 8 bytes (big-endian when rendered as a string).
    pub data4: [u8; 8],
}

impl Guid {
    /// Construct from 16 little-endian bytes - equivalent to
    /// [`From<[u8; 16]>`](#impl-From%3C%5Bu8%3B+16%5D%3E-for-Guid),
    /// kept for backwards compatibility with code that holds a `&[u8; 16]`.
    #[inline]
    pub fn from_bytes(bytes: &[u8; 16]) -> Self {
        Self::from(*bytes)
    }

    /// Render as `{XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX}` (Microsoft /
    /// Delphi format), allocating a `String`. Equivalent to
    /// `format!("{guid}")`. Prefer the `Display` impl in hot paths to
    /// avoid the allocation.
    pub fn to_string_delphi(&self) -> String {
        self.to_string()
    }
}

impl From<[u8; 16]> for Guid {
    /// Construct a `Guid` from 16 little-endian bytes - the on-disk
    /// layout used by both Delphi (`TGuid` literal) and FPC.
    fn from(bytes: [u8; 16]) -> Self {
        Self {
            data1: u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
            data2: u16::from_le_bytes([bytes[4], bytes[5]]),
            data3: u16::from_le_bytes([bytes[6], bytes[7]]),
            data4: [
                bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14],
                bytes[15],
            ],
        }
    }
}

impl fmt::Display for Guid {
    /// Writes `{XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX}` directly to the
    /// formatter - no intermediate `String` allocation.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let d = &self.data4;
        write!(
            f,
            "{{{:08X}-{:04X}-{:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}}}",
            self.data1, self.data2, self.data3, d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7]
        )
    }
}

/// One interface entry on a class.
///
/// `iid_str` is only populated for FPC-flavor entries; Delphi uses COM
/// interfaces exclusively and doesn't emit the Corba-name slot.
#[derive(Debug, Clone)]
pub struct InterfaceEntry<'a> {
    /// Interface GUID (COM IID). All-zero when this is a Corba interface
    /// identified by [`Self::iid_str`] instead.
    pub guid: Guid,
    /// VA of the interface-dispatch vtable.
    pub vtable_va: u64,
    /// Offset relative to the instance (Delphi) or the VMT (FPC), depending
    /// on flavor. Exposed as read, for downstream analysis to interpret.
    pub offset: u64,
    /// Code VA of a static `implements` getter: [`Self::getter`] when it is
    /// [`AccessKind::Static`], `0` otherwise.
    pub getter_va: u64,
    /// How the interface is obtained when the class delegates it with
    /// `implements` (then [`Self::vtable_va`] and [`Self::offset`] are 0):
    /// the Delphi `ImplGetter`, which uses the property-accessor encoding
    /// (Delphi 6 `System.pas` `InvokeImplGetter`) - a field holding the
    /// interface ([`AccessKind::Field`], instance offset), a virtual getter
    /// ([`AccessKind::Virtual`], VMT byte offset; see [`Access::resolve`]),
    /// or a static getter ([`AccessKind::Static`]). [`AccessKind::None`]
    /// for a directly implemented interface and on FPC.
    pub getter: Access,
    /// For FPC Corba interfaces: the textual IID (e.g. `"ISomething"`).
    /// Resolved via `IIDStrRef: ^PShortString`. `None` on Delphi or when
    /// the indirection couldn't be followed.
    iid_str: Option<&'a [u8]>,
}

/// One method on an interface - produced by
/// [`crate::DelphiBinary::interface_methods`].
#[derive(Debug, Clone)]
pub struct InterfaceMethod<'a> {
    /// Slot index within the interface's vtable (0 = first method).
    pub slot_index: u16,
    /// Absolute VA of the slot's code: the adjustor thunk, read off the
    /// interface's `vtable_va` array. Subtract the image base for an RVA.
    pub code_va: u64,
    /// The decoded adjustor thunk at [`Self::code_va`], naming the
    /// implementing method. `None` when the slot is not a recognised thunk
    /// (e.g. a slot pointing straight at a method).
    pub thunk: Option<InterfaceThunk>,
    /// Method name when the interface RTTI carries one (FPC `tkInterface`
    /// records do; classic Delphi COM interfaces do not). Lossily decoded.
    pub name: Option<&'a str>,
}

/// Where an interface adjustor thunk transfers control.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThunkTarget {
    /// A direct jump to the implementing method's code VA.
    Code(u64),
    /// A jump through the object's VMT: the implementing method is virtual,
    /// in the slot at this byte offset from the class pointer.
    VmtOffset(u32),
}

/// A decoded interface adjustor thunk: `Self += self_adjust`, then
/// transfer to [`Self::target`]. See the module docs for the encodings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InterfaceThunk {
    /// Signed amount added to `Self`; `-IOffset` of the interface entry.
    pub self_adjust: i64,
    /// Where control goes after the adjustment.
    pub target: ThunkTarget,
}

impl InterfaceThunk {
    /// Decode the adjustor thunk at `va`. `pointer_size` selects the 32- or
    /// 64-bit encodings. `None` when the bytes match no known thunk.
    pub fn decode(ctx: &BinaryContext<'_>, va: u64, pointer_size: usize) -> Option<Self> {
        let off = ctx.va_to_file(va)?;
        let code = ctx.data().get(off..)?;
        let (self_adjust, len) = match pointer_size {
            4 => adjust_32(code)?,
            8 => adjust_64(code)?,
            _ => return None,
        };
        let tail = code.get(len..)?;
        let tail_va = va.checked_add(u64::try_from(len).ok()?)?;
        let target = transfer(tail, tail_va, pointer_size)?;
        Some(Self {
            self_adjust,
            target,
        })
    }
}

/// Decodes a 32-bit `Self` adjustment, answering the signed amount and the
/// instruction length.
fn adjust_32(code: &[u8]) -> Option<(i64, usize)> {
    let imm8 = |at: usize| code.get(at).map(|&b| i64::from(b as i8));
    let imm32 = |at: usize| read_i32(code, at).map(i64::from);
    match code {
        [0x83, 0x44, 0x24, 0x04, ..] => Some((imm8(4)?, 5)),
        [0x81, 0x44, 0x24, 0x04, ..] => Some((imm32(4)?, 8)),
        [0x83, 0x6C, 0x24, 0x04, ..] => Some((imm8(4)?.checked_neg()?, 5)),
        [0x81, 0x6C, 0x24, 0x04, ..] => Some((imm32(4)?.checked_neg()?, 8)),
        [0x83, 0xC0, ..] => Some((imm8(2)?, 3)),
        [0x05, ..] => Some((imm32(1)?, 5)),
        [0x83, 0xE8, ..] => Some((imm8(2)?.checked_neg()?, 3)),
        [0x2D, ..] => Some((imm32(1)?.checked_neg()?, 5)),
        _ => None,
    }
}

/// Decodes a 64-bit `Self` adjustment (`rcx` on Win64, `rdi` on SysV),
/// answering the signed amount and the instruction length.
fn adjust_64(code: &[u8]) -> Option<(i64, usize)> {
    let imm8 = || code.get(3).map(|&b| i64::from(b as i8));
    let imm32 = || read_i32(code, 3).map(i64::from);
    match code {
        [0x48, 0x83, 0xC1 | 0xC7, ..] => Some((imm8()?, 4)),
        [0x48, 0x81, 0xC1 | 0xC7, ..] => Some((imm32()?, 7)),
        [0x48, 0x83, 0xE9 | 0xEF, ..] => Some((imm8()?.checked_neg()?, 4)),
        [0x48, 0x81, 0xE9 | 0xEF, ..] => Some((imm32()?.checked_neg()?, 7)),
        _ => None,
    }
}

/// Decodes the transfer that follows the adjustment at `va`.
fn transfer(code: &[u8], va: u64, pointer_size: usize) -> Option<ThunkTarget> {
    match code {
        [0xE9, ..] => {
            let rel = read_i32(code, 1)?;
            Some(ThunkTarget::Code(
                va.checked_add(5)?.checked_add_signed(i64::from(rel))?,
            ))
        }
        [0xEB, rel, ..] => Some(ThunkTarget::Code(
            va.checked_add(2)?
                .checked_add_signed(i64::from(*rel as i8))?,
        )),
        _ => vmt_dispatch(code, pointer_size).map(ThunkTarget::VmtOffset),
    }
}

/// Decodes a virtual transfer, answering the VMT byte offset it jumps
/// through.
fn vmt_dispatch(code: &[u8], pointer_size: usize) -> Option<u32> {
    // `jmp [reg+disp8]` / `jmp [reg+disp32]`, `mov reg,[reg+disp8|32]`.
    let disp = |at: usize, wide: bool| -> Option<u32> {
        if wide {
            u32::try_from(read_i32(code, at)?).ok()
        } else {
            code.get(at).map(|&b| u32::from(b))
        }
    };
    if pointer_size == 8 {
        return match code {
            [0x48, 0x8B, 0x01 | 0x07, 0xFF, 0x20, ..] => Some(0),
            [0x48, 0x8B, 0x01 | 0x07, 0xFF, 0x60, ..] => disp(5, false),
            [0x48, 0x8B, 0x01 | 0x07, 0xFF, 0xA0, ..] => disp(5, true),
            _ => None,
        };
    }
    match code {
        // Delphi, Self on the stack: mov eax,[esp+4]; mov eax,[eax]; jmp [eax+d]
        [0x8B, 0x44, 0x24, 0x04, 0x8B, 0x00, 0xFF, 0x20, ..] => Some(0),
        [0x8B, 0x44, 0x24, 0x04, 0x8B, 0x00, 0xFF, 0x60, ..] => disp(8, false),
        [0x8B, 0x44, 0x24, 0x04, 0x8B, 0x00, 0xFF, 0xA0, ..] => disp(8, true),
        // FPC, Self on the stack: mov eax,[esp+4]; mov ecx,[eax]; jmp [ecx+d]
        [0x8B, 0x44, 0x24, 0x04, 0x8B, 0x08, 0xFF, 0x21, ..] => Some(0),
        [0x8B, 0x44, 0x24, 0x04, 0x8B, 0x08, 0xFF, 0x61, ..] => disp(8, false),
        [0x8B, 0x44, 0x24, 0x04, 0x8B, 0x08, 0xFF, 0xA1, ..] => disp(8, true),
        // Delphi, Self in eax: push eax; mov eax,[eax]; mov eax,[eax+d]; xchg; ret
        [0x50, 0x8B, 0x00, 0x8B, 0x00, ..] => Some(0),
        [0x50, 0x8B, 0x00, 0x8B, 0x40, ..] => disp(5, false),
        [0x50, 0x8B, 0x00, 0x8B, 0x80, ..] => disp(5, true),
        // FPC, Self in eax: mov ecx,[eax]; jmp [ecx+d]
        [0x8B, 0x08, 0xFF, 0x21, ..] => Some(0),
        [0x8B, 0x08, 0xFF, 0x61, ..] => disp(4, false),
        [0x8B, 0x08, 0xFF, 0xA1, ..] => disp(4, true),
        // FPC, Self in eax, registers preserved: push ebx; push eax; ...
        [0x53, 0x50, 0x8B, 0x00, 0x8B, 0x00, ..] => Some(0),
        [0x53, 0x50, 0x8B, 0x00, 0x8B, 0x40, ..] => disp(6, false),
        [0x53, 0x50, 0x8B, 0x00, 0x8B, 0x80, ..] => disp(6, true),
        _ => None,
    }
}

/// Reads a little-endian `i32` at `at`.
fn read_i32(code: &[u8], at: usize) -> Option<i32> {
    Some(i32::from_le_bytes(
        code.get(at..at.checked_add(4)?)?.try_into().ok()?,
    ))
}

impl<'a> InterfaceEntry<'a> {
    /// For FPC Corba interfaces: the textual IID (e.g. `"ISomething"`),
    /// lossily decoded. `None` on Delphi binaries or when the indirection
    /// couldn't be followed.
    #[inline]
    pub fn iid_str(&self) -> Option<&'a str> {
        self.iid_str
            .map(|b| str::from_utf8(b).unwrap_or("<non-ascii>"))
    }

    /// Raw IID-string bytes for FPC Corba interfaces. `None` on Delphi or
    /// when the indirection couldn't be followed.
    #[inline]
    pub fn iid_str_bytes(&self) -> Option<&'a [u8]> {
        self.iid_str
    }
}

impl<'a> InterfaceMethod<'a> {
    /// Convert this method's absolute code VA to an RVA by subtracting
    /// `image_base`.
    #[inline]
    pub fn method_rva(&self, image_base: u64) -> Option<u64> {
        self.code_va.checked_sub(image_base)
    }

    /// Walk an interface's method-pointer array and return one entry
    /// per slot. The count is recovered by reading pointer-sized
    /// slots from `entry.vtable_va` and stopping at the first slot
    /// that does not contain a plausible code VA (zero, unmapped, or
    /// outside the binary's primary code section). When the first slot
    /// is an adjustor thunk for this entry (adjustment `-IOffset`), the
    /// walk also stops at the first slot that is not: interface vtables
    /// are laid out back to back, and without this bound the walk ran on
    /// into the next interface's slots.
    ///
    /// `pointer_size` is the binary's pointer width (4 or 8). Names
    /// are always `None` here - names live in `tkInterface` RTTI which
    /// this walker doesn't consult; callers wanting names should go
    /// through [`crate::DelphiBinary::interface_methods`], which
    /// merges the RTTI index in.
    ///
    /// Hard-capped at [`MAX_METHODS_PER_CLASS`] to keep adversarial
    /// input from forcing pathological scans.
    pub fn iter(
        ctx: &BinaryContext<'a>,
        entry: &InterfaceEntry<'a>,
        pointer_size: usize,
    ) -> Vec<Self> {
        if entry.vtable_va == 0 || pointer_size == 0 {
            return Vec::new();
        }
        let Some(base_off) = ctx.va_to_file(entry.vtable_va) else {
            return Vec::new();
        };
        let data = ctx.data();
        let expected_adjust = i64::try_from(entry.offset).ok().and_then(i64::checked_neg);
        let mut thunked = false;
        let mut out = Vec::new();
        for slot in 0..MAX_METHODS_PER_CLASS {
            let Some(off) = slot
                .checked_mul(pointer_size)
                .and_then(|n| base_off.checked_add(n))
            else {
                break;
            };
            let Some(va) = read_ptr(data, off, pointer_size) else {
                break;
            };
            if va == 0 || !ctx.is_code_va(va) {
                break;
            }
            let thunk = InterfaceThunk::decode(ctx, va, pointer_size);
            let own = thunk.is_some_and(|t| Some(t.self_adjust) == expected_adjust);
            if slot == 0 {
                thunked = own;
            } else if thunked && !own {
                break;
            }
            out.push(InterfaceMethod {
                slot_index: slot as u16,
                code_va: va,
                thunk,
                name: None,
            });
        }
        out
    }
}

impl<'a> InterfaceEntry<'a> {
    /// Returns the virtual addresses `vmt`'s interface table occupies: its
    /// count, the padding that aligns the entries, and the entries.
    ///
    /// Delphi entries are a GUID and three pointer-sized slots after a
    /// 32-bit count padded to pointer alignment; FPC entries are five
    /// pointer-sized slots after a pointer-sized count. `None` when the
    /// class has none, or its count is implausible.
    pub fn table_extent(ctx: &BinaryContext<'a>, vmt: &Vmt<'a>) -> Option<Range<u64>> {
        if vmt.intf_table == 0 {
            return None;
        }
        let data = ctx.data();
        let base_off = ctx.va_to_file(vmt.intf_table)?;
        let psize = usize::from(vmt.pointer_size);
        let (count, header, entry) = match vmt.flavor {
            VmtFlavor::Delphi => (
                usize::try_from(read_u32(data, base_off)?).ok()?,
                psize.max(4),
                psize.checked_mul(3)?.checked_add(16)?,
            ),
            VmtFlavor::Fpc => (
                usize::try_from(read_ptr(data, base_off, psize)?).ok()?,
                psize,
                psize.checked_mul(5)?,
            ),
        };
        let limit = match vmt.flavor {
            VmtFlavor::Delphi => MAX_INTERFACES_PER_CLASS,
            VmtFlavor::Fpc => MAX_INTERFACES_PER_CLASS_FPC,
        };
        if count > limit {
            return None;
        }
        let len = u64::try_from(count.checked_mul(entry)?.checked_add(header)?).ok()?;
        Some(vmt.intf_table..vmt.intf_table.checked_add(len)?)
    }

    /// Decode every interface entry declared on `vmt`. Returns an
    /// empty vector when the class has no interface table or the
    /// table is malformed.
    pub fn iter(ctx: &BinaryContext<'a>, vmt: &Vmt<'a>) -> Vec<Self> {
        if vmt.intf_table == 0 {
            return Vec::new();
        }
        let result = match vmt.flavor {
            VmtFlavor::Delphi => iter_delphi(ctx, vmt),
            VmtFlavor::Fpc => iter_fpc(ctx, vmt),
        };
        result.unwrap_or_else(|| {
            crate::__undelphi_trace_warn!(
                vmt_va = vmt.va,
                intf_table = vmt.intf_table,
                "InterfaceEntry::iter: interface-table walk bailed out"
            );
            Vec::new()
        })
    }
}

fn iter_delphi<'a>(ctx: &BinaryContext<'a>, vmt: &Vmt<'a>) -> Option<Vec<InterfaceEntry<'a>>> {
    let base_off = ctx.va_to_file(vmt.intf_table)?;
    let data = ctx.data();
    let count = read_u32(data, base_off)? as usize;
    if count == 0 || count > MAX_INTERFACES_PER_CLASS {
        return Some(Vec::new());
    }

    let psize = vmt.pointer_size as usize;
    if psize < 4 {
        return None;
    }
    // Entry layout on 32-bit Delphi: GUID(16) + VTable(4) + IOffset(4) + Getter(4) = 28.
    // Entry layout on 64-bit Delphi: GUID(16) + VTable(8) + IOffset(8) + Getter(8) = 40.
    // The `IOffset` field is typed as `Integer`/`Longint` in Delphi source
    // but the record is pointer-aligned, so on 64-bit it occupies 8 bytes.
    // Verified empirically against HeidiSQL's `TInterfacedObject` entry.
    let offset_slot = psize;
    let entry_size = 16usize
        .checked_add(psize)?
        .checked_add(offset_slot)?
        .checked_add(psize)?;
    // After the 4-byte NumEntries header, pad to pointer alignment so
    // the first GUID's `Data1` lands on an aligned boundary.
    let entries_start = base_off
        .checked_add(4)?
        .checked_add(psize.checked_sub(4)?)?;

    let mut out = Vec::with_capacity(count);
    let mut cursor = entries_start;
    for _ in 0..count {
        let guid_end = cursor.checked_add(16)?;
        let guid_bytes = data.get(cursor..guid_end)?;
        let guid = Guid::from_bytes(guid_bytes.try_into().ok()?);
        let vtable_off = guid_end;
        let offset_off = vtable_off.checked_add(psize)?;
        let getter_off = offset_off.checked_add(offset_slot)?;
        let vtable_va = read_ptr(data, vtable_off, psize)?;
        let offset = read_ptr(data, offset_off, offset_slot)?;
        let getter = Access::accessor_from_ptr(read_ptr(data, getter_off, psize)?, psize);
        let getter_va = match getter.kind {
            AccessKind::Static => getter.value,
            _ => 0,
        };
        out.push(InterfaceEntry {
            guid,
            vtable_va,
            offset,
            getter_va,
            getter,
            iid_str: None,
        });
        cursor = cursor.checked_add(entry_size)?;
    }
    Some(out)
}

fn iter_fpc<'a>(ctx: &BinaryContext<'a>, vmt: &Vmt<'a>) -> Option<Vec<InterfaceEntry<'a>>> {
    let base_off = ctx.va_to_file(vmt.intf_table)?;
    let data = ctx.data();
    let psize = vmt.pointer_size as usize;
    // `EntryCount` is `SizeUInt` (pointer-sized) per objpash.inc:206.
    let count = read_ptr(data, base_off, psize)? as usize;
    if count == 0 || count > MAX_INTERFACES_PER_CLASS_FPC {
        return Some(Vec::new());
    }

    // FPC entry: IIDRef (ptr) + VTable (ptr) + IOffset (ptr) + IIDStrRef (ptr) + IType (byte + padding).
    // Size varies because of the trailing one-byte enum. In practice FPC
    // emits records of size `round_up(4*ptr + 1, ptr)` = 5*ptr on both 32
    // and 64 bit, giving 20/40-byte entries.
    let entry_size = psize.checked_mul(5)?;

    let mut out = Vec::with_capacity(count);
    let mut cursor = base_off.checked_add(psize)?;
    for _ in 0..count {
        let iid_ref = read_ptr(data, cursor, psize)?;
        let vtable_off = cursor.checked_add(psize)?;
        let ioffset_off = vtable_off.checked_add(psize)?;
        let iid_str_ref_off = ioffset_off.checked_add(psize)?;
        let vtable_va = read_ptr(data, vtable_off, psize)?;
        let ioffset = read_ptr(data, ioffset_off, psize)?;
        let iid_str_ref = read_ptr(data, iid_str_ref_off, psize)?;

        // FPC 3.0 one-level references, recognised when the GUID formats
        // to its own text (see the module docs). Otherwise FPC 3.2's
        // `^PGuid` / `^PShortString`, dereferenced twice. A Corba entry
        // carries only a name. If none of these reads succeeds the entry is
        // malformed - skip rather than emit a null GUID that would later be
        // mistaken for a real `IID_NULL`.
        let ids = read_fpc30_ids(ctx, iid_ref, iid_str_ref)
            .or_else(|| {
                read_fpc_iid(ctx, iid_ref, psize)
                    .map(|guid| (guid, read_fpc_iid_str(ctx, iid_str_ref, psize)))
            })
            .or_else(|| read_fpc_corba_name(ctx, iid_ref, iid_str_ref, psize));
        let Some((guid, iid_str)) = ids else {
            cursor = cursor.checked_add(entry_size)?;
            continue;
        };

        out.push(InterfaceEntry {
            guid,
            vtable_va,
            offset: ioffset,
            getter_va: 0,
            getter: Access {
                kind: AccessKind::None,
                value: 0,
            },
            iid_str,
        });
        cursor = cursor.checked_add(entry_size)?;
    }
    Some(out)
}

/// Follow `IIDStrRef: ^PShortString` two levels of indirection to recover
/// the Corba interface name.
fn read_fpc_iid_str<'a>(
    ctx: &BinaryContext<'a>,
    iid_str_ref_va: u64,
    ptr_size: usize,
) -> Option<&'a [u8]> {
    if iid_str_ref_va == 0 {
        return None;
    }
    let outer_off = ctx.va_to_file(iid_str_ref_va)?;
    let data = ctx.data();
    let pstr_va = read_ptr(data, outer_off, ptr_size)?;
    read_short_string_at_va(ctx, pstr_va)
}

/// Reads an FPC 3.0 entry's one-level `IID: PGuid` / `IIDStr: PShortString`
/// pair, accepting it only when the GUID formats to the text.
fn read_fpc30_ids<'a>(
    ctx: &BinaryContext<'a>,
    iid_va: u64,
    iid_str_va: u64,
) -> Option<(Guid, Option<&'a [u8]>)> {
    let text = read_short_string_at_va(ctx, iid_str_va)?;
    let off = ctx.va_to_file(iid_va)?;
    let guid = Guid::from_bytes(ctx.data().get(off..off.checked_add(16)?)?.try_into().ok()?);
    text.eq_ignore_ascii_case(guid.to_string_delphi().as_bytes())
        .then_some((guid, Some(text)))
}

/// Reads a Corba entry, which has no GUID: a null FPC 3.0 `IID`, or an FPC
/// 3.2 `^PGuid` cell holding null. The name comes from `IIDStrRef`, through
/// two levels of indirection or (FPC 3.0) one.
fn read_fpc_corba_name<'a>(
    ctx: &BinaryContext<'a>,
    iid_ref_va: u64,
    iid_str_ref_va: u64,
    ptr_size: usize,
) -> Option<(Guid, Option<&'a [u8]>)> {
    let no_guid =
        iid_ref_va == 0 || read_ptr(ctx.data(), ctx.va_to_file(iid_ref_va)?, ptr_size)? == 0;
    if !no_guid {
        return None;
    }
    let name = read_fpc_iid_str(ctx, iid_str_ref_va, ptr_size)
        .filter(|n| is_interface_name(n))
        .or_else(|| {
            read_short_string_at_va(ctx, iid_str_ref_va).filter(|n| is_interface_name(n))
        })?;
    let zero = Guid::from_bytes(&[0; 16]);
    Some((zero, Some(name)))
}

/// Returns `true` for a plausible Pascal interface name.
fn is_interface_name(name: &[u8]) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && name
            .iter()
            .all(|&b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.')
}

fn read_fpc_iid(ctx: &BinaryContext<'_>, iid_ref_va: u64, ptr_size: usize) -> Option<Guid> {
    if iid_ref_va == 0 {
        return None;
    }
    let first_off = ctx.va_to_file(iid_ref_va)?;
    let data = ctx.data();
    let guid_va = read_ptr(data, first_off, ptr_size)?;
    let guid_off = ctx.va_to_file(guid_va)?;
    let guid_end = guid_off.checked_add(16)?;
    let guid_bytes = data.get(guid_off..guid_end)?;
    Some(Guid::from_bytes(guid_bytes.try_into().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guid_renders_canonical_form() {
        // The IUnknown IID: {00000000-0000-0000-C000-000000000046}
        let g = Guid {
            data1: 0,
            data2: 0,
            data3: 0,
            data4: [0xC0, 0, 0, 0, 0, 0, 0, 0x46],
        };
        assert_eq!(
            g.to_string_delphi(),
            "{00000000-0000-0000-C000-000000000046}"
        );
    }

    #[test]
    fn guid_from_bytes_matches_record() {
        let bytes = [
            0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF, 0x01, 0x23, 0x45, 0x67, 0x89, 0xAB,
            0xCD, 0xEF,
        ];
        let g = Guid::from_bytes(&bytes);
        assert_eq!(g.data1, 0x67452301);
        assert_eq!(g.data2, 0xAB89);
        assert_eq!(g.data3, 0xEFCD);
        assert_eq!(g.data4, [0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF]);
    }
}
