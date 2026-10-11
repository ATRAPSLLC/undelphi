//! Virtual-method-table function-pointer array decoder.
//!
//! A class's VMT header is followed by an array of `pointer_size` function
//! pointers - one per user-declared virtual method slot. The slot count is
//! not stored anywhere explicitly; the canonical way to bound it is "up
//! to the next class's VMT base". ESET's `DelphiHelper` and `pythia` both
//! derive the count this way.
//!
//! ## Approach
//!
//! For each class `C`:
//!
//! 1. Take the starting VA as the end of `C`'s fixed header
//!    ([`Vmt::header_extent`]). On Delphi that is the `vmtSelfPtr` target,
//!    which *is* the first slot of the virtual method pointer array (as
//!    pythia also reads it); `TObject`'s own virtuals (`Destroy`,
//!    `NewInstance`, ...) sit at negative offsets inside the header. On FPC
//!    it follows the twelve header fields, and the array begins with
//!    `TObject`'s virtuals (`vDestroy`, `vNewInstance`, ...,
//!    FPC `rtl/inc/objpash.inc:90-103`).
//! 2. Take the ending VA as the smallest VMT-base VA strictly greater than
//!    that start across all discovered classes (when one exists). If no
//!    class follows, clamp to a conservative 2 KiB budget.
//! 3. Walk pointer-sized slots until the end, reading each as a code
//!    address. Stop early if a slot is zero or doesn't point into a
//!    readable segment - that typically marks a RTTI/data fragment we
//!    shouldn't cross into.
//!
//! ## Allocation note
//!
//! The returned `VirtualMethodEntry` records are `Copy`; no heap allocation
//! happens on the hot path beyond the result `Vec`.
//!
//! ## Offsets
//!
//! Virtual dispatch addresses a slot by its byte offset from the class
//! pointer ([`Vmt::class_pointer`]), and that is what a virtual property
//! accessor or an interface thunk stores. [`VirtualMethodEntry::vmt_offset`]
//! carries it, so both resolve the same way on Delphi (offset = slot ×
//! pointer size) and FPC (offset = header + slot × pointer size).

use crate::{
    classes::ClassSet,
    formats::BinaryContext,
    util::read_ptr,
    vmt::{Vmt, VmtFlavor},
};

/// One entry in the virtual-function-pointer array.
#[derive(Debug, Clone, Copy)]
pub struct VirtualMethodEntry {
    /// Zero-based slot index within the array (matches the Delphi `vtXxx`
    /// convention where `vt0` is the first user-declared slot).
    pub slot: usize,
    /// Virtual address of the method pointer's storage location.
    pub slot_va: u64,
    /// Byte offset of the slot from the class pointer
    /// ([`Vmt::class_pointer`]): the value a virtual accessor or interface
    /// thunk dispatches through.
    pub vmt_offset: u64,
    /// The code address the slot points at (the actual function entry).
    pub code_va: u64,
}

impl VirtualMethodEntry {
    /// Walks the virtual-method array following `vmt`.
    ///
    /// `upper_bound_va` is the first VA at which another class's VMT
    /// begins; slots at or beyond that boundary aren't read. Pass
    /// `None` to fall back to a 2 KiB scan window.
    pub fn iter(ctx: &BinaryContext<'_>, vmt: &Vmt<'_>, upper_bound_va: Option<u64>) -> Vec<Self> {
        let Some(header) = vmt.header_extent() else {
            return Vec::new();
        };
        let psize = vmt.pointer_size as usize;
        let start_va = header.end;
        let class_pointer = vmt.class_pointer();
        // 2 KiB default scan window when no tighter upper bound is supplied.
        // `saturating_add` guards against `start_va` near `u64::MAX` so we
        // never wrap into a smaller bound than we started with.
        let default_end = start_va.saturating_add(2048);
        let end_va = upper_bound_va.unwrap_or(default_end).max(start_va);
        if end_va <= start_va {
            return Vec::new();
        }
        let span = match end_va
            .checked_sub(start_va)
            .and_then(|v| usize::try_from(v).ok())
        {
            Some(s) => s,
            None => return Vec::new(),
        };
        if psize == 0 {
            return Vec::new();
        }
        let Some(max_slots) = span.checked_div(psize) else {
            return Vec::new();
        };
        if max_slots == 0 {
            return Vec::new();
        }

        let Some(start_file) = ctx.va_to_file(start_va) else {
            return Vec::new();
        };
        let data = ctx.data();
        let mut out = Vec::with_capacity(max_slots);
        for slot in 0..max_slots {
            let Some(off) = slot
                .checked_mul(psize)
                .and_then(|n| start_file.checked_add(n))
            else {
                break;
            };
            let Some(code_va) = read_ptr(data, off, psize) else {
                break;
            };
            if code_va == 0 {
                // A null slot usually means we've walked off the end of the
                // array into padding or an unrelated record.
                break;
            }
            // Validate: the code pointer should resolve into the image.
            if ctx.va_to_file(code_va).is_none() {
                break;
            }
            let slot_va = start_va.saturating_add((slot.saturating_mul(psize)) as u64);
            out.push(VirtualMethodEntry {
                slot,
                slot_va,
                vmt_offset: slot_va.saturating_sub(class_pointer),
                code_va,
            });
        }
        out
    }

    /// Computes the tightest VA upper-bound for a class's
    /// virtual-method array against `set` - the next class's VMT base
    /// (or the `self_ptr` that follows this one), or the class's own name
    /// or tables when the compiler placed them right after the array (a
    /// class with no virtuals of its own), whichever is closest.
    pub fn upper_bound_for(set: &ClassSet<'_>, vmt: &Vmt<'_>) -> Option<u64> {
        let start = vmt.header_extent()?.end;
        let mut best: Option<u64> = [
            vmt.class_name_ptr,
            vmt.field_table,
            vmt.method_table,
            vmt.dynamic_table,
            vmt.init_table,
            vmt.intf_table,
            vmt.auto_table,
            vmt.type_info,
        ]
        .into_iter()
        // A table at the class pointer itself: no virtuals at all.
        .filter(|&va| va >= start)
        .min();
        for c in set.iter() {
            // The next VMT header we could bump into is some class's
            // `va`. Also the `self_ptr` target of earlier classes can
            // sit between our `self_ptr` and the next VMT header - use
            // whichever is closer.
            let candidates = [c.vmt.va, c.vmt.self_ptr];
            for &va in &candidates {
                if va > start {
                    match best {
                        Some(cur) if va >= cur => {}
                        _ => best = Some(va),
                    }
                }
            }
        }
        best
    }
}

/// One of `TObject`'s own virtual methods as a class's VMT records it: the
/// destructor, the allocation hooks, message dispatch and so on. A class
/// that overrides one (most override `Destroy`) has its own code there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StandardVirtual {
    /// The `TObject` method the slot holds, when the layout names it.
    pub name: Option<&'static str>,
    /// Byte offset of the slot from the class pointer: negative inside a
    /// Delphi header, positive in FPC's method array.
    pub vmt_offset: i64,
    /// Code VA the slot points at.
    pub code_va: u64,
}

/// Delphi's `TObject` virtuals at the end of the header, nearest the class
/// pointer first. Delphi 6 `System.pas`: `vmtDestroy = -4`,
/// `vmtFreeInstance = -8`, `vmtNewInstance = -12`, `vmtDefaultHandler =
/// -16`, `vmtDispatch = -20`, `vmtBeforeDestruction = -24`,
/// `vmtAfterConstruction = -28`, `vmtSafeCallException = -32` (32-bit;
/// pointer-sized slots). The last four have kept their places since Delphi
/// 1 (Eadom's Compiler-Internals notes, `typeinfo/ti_incl.idc`). Delphi 2009 adds
/// `vmtToString`, `vmtGetHashCode` and `vmtEquals` beyond them.
const DELPHI_HEADER_VIRTUALS: [&str; 11] = [
    "Destroy",
    "FreeInstance",
    "NewInstance",
    "DefaultHandler",
    "Dispatch",
    "BeforeDestruction",
    "AfterConstruction",
    "SafeCallException",
    "ToString",
    "GetHashCode",
    "Equals",
];

/// The three `TObject` virtuals `System.pas` declares after `Destroy` where
/// it defines `CPP_ABI_SUPPORT` (Win64 and the LLVM-linked targets; seen in
/// Seattle's `System.dcu` and Delphi 13 binaries, XE2 - XE8 unverified),
/// nearest the class pointer first: `CPP_ABI_ADJUST = 3 *
/// SizeOf(Pointer)` moves every other header virtual three slots out
/// (`vmtDestroy = -8 - CPP_ABI_ADJUST`).
const DELPHI_CPP_ABI_VIRTUALS: [&str; 3] = ["CPP_ABI_3", "CPP_ABI_2", "CPP_ABI_1"];

/// Data slots in front of the virtuals in a Delphi 4+ header: `vmtSelfPtr`
/// through `vmtParent`.
const DELPHI_HEADER_DATA_SLOTS: u8 = 11;

/// FPC's `TObject` virtuals, the first slots of the method array
/// (FPC `rtl/inc/objpash.inc:90-103`).
const FPC_OBJECT_VIRTUALS: [&str; 13] = [
    "Destroy",
    "NewInstance",
    "FreeInstance",
    "SafeCallException",
    "DefaultHandler",
    "AfterConstruction",
    "BeforeDestruction",
    "DefaultHandlerStr",
    "Dispatch",
    "DispatchStr",
    "Equals",
    "GetHashCode",
    "ToString",
];

impl StandardVirtual {
    /// Reads the `TObject` virtual slots of `vmt`.
    ///
    /// Delphi: every pointer-sized code slot between `vmtParent` and the
    /// class pointer, nearest first: the C++ ABI slots where the header has
    /// them (Win64), then the eight Delphi 4+ slots and Delphi 2009's three.
    /// Headers shorter than Delphi 4's 19 slots are not read. FPC: the first 13
    /// method-array slots of [`VirtualMethodEntry::iter`].
    pub fn iter(ctx: &BinaryContext<'_>, vmt: &Vmt<'_>) -> Vec<Self> {
        match vmt.flavor {
            VmtFlavor::Delphi => delphi_header_virtuals(ctx, vmt).unwrap_or_default(),
            VmtFlavor::Fpc => VirtualMethodEntry::iter(ctx, vmt, None)
                .into_iter()
                .zip(FPC_OBJECT_VIRTUALS)
                .filter_map(|(slot, name)| {
                    Some(Self {
                        name: Some(name),
                        vmt_offset: i64::try_from(slot.vmt_offset).ok()?,
                        code_va: slot.code_va,
                    })
                })
                .collect(),
        }
    }
}

/// Reads a Delphi 4+ header's code slots, nearest the class pointer first.
fn delphi_header_virtuals(ctx: &BinaryContext<'_>, vmt: &Vmt<'_>) -> Option<Vec<StandardVirtual>> {
    // Eight virtuals up to Delphi 2007, eleven from 2009, three more with
    // the C++ ABI slots.
    let count = vmt
        .header_slot_count
        .checked_sub(DELPHI_HEADER_DATA_SLOTS)
        .filter(|&n| n >= 8)?;
    let cpp_abi = usize::from(count)
        == DELPHI_HEADER_VIRTUALS
            .len()
            .saturating_add(DELPHI_CPP_ABI_VIRTUALS.len());
    let names: Vec<&'static str> = if cpp_abi {
        DELPHI_CPP_ABI_VIRTUALS
            .iter()
            .chain(DELPHI_HEADER_VIRTUALS.iter())
            .copied()
            .collect()
    } else {
        DELPHI_HEADER_VIRTUALS.to_vec()
    };
    let psize = i64::from(vmt.pointer_size);
    let data = ctx.data();
    let mut out = Vec::with_capacity(usize::from(count));
    for back in 1..=i64::from(count) {
        let vmt_offset = back.checked_mul(psize)?.checked_neg()?;
        let slot_va = vmt.class_pointer().checked_add_signed(vmt_offset)?;
        let code_va = read_ptr(
            data,
            ctx.va_to_file(slot_va)?,
            usize::from(vmt.pointer_size),
        )?;
        if code_va == 0 || !ctx.is_code_va(code_va) {
            continue;
        }
        let name = usize::try_from(back.checked_sub(1)?)
            .ok()
            .and_then(|i| names.get(i))
            .copied();
        out.push(StandardVirtual {
            name,
            vmt_offset,
            code_va,
        });
    }
    Some(out)
}
