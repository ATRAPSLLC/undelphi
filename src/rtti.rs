//! Minimal RTTI (`PTypeInfo`) decoder, focused on the `tkClass` branch.
//!
//! The `tkClass` decode extracts the class's **unit name** from the
//! `vmtTypeInfo → tkClass → UnitName` record. Properties (the `TPropData`
//! tail of `tkClass`) are walked by [`crate::properties`] and
//! [`crate::extrtti`] from the offset recorded here.
//!
//! ## Layout (Delphi and FPC share the tkClass shape)
//!
//! `vmtTypeInfo` points at a `PTypeInfo` record, laid out as:
//!
//! ```text
//!   +0         Kind: u8                       (must equal tkClass = 7)
//!   +1         NameLen: u8
//!   +2..N      Name: [u8; NameLen]            (class name, matches vmtClassName)
//!   <TypeData starts at +2+NameLen>
//!   +0         ClassType: ptr                 (points back to VMT base)
//!   +ptr       ParentInfo: PPTypeInfo         (pointer-to-pointer-to-parent-TypeInfo)
//!   +2*ptr     PropCount: i16                 (classic tkClass property count)
//!   +2*ptr+2   UnitName: ShortString          (unit that declared this class)
//!   +...       TPropData: PropCount: u16      (a second, potentially-different count)
//!   +...       array[TPropData.PropCount] of TPropInfo   (deferred)
//! ```
//!
//! Source: DelphiHelper `DelphiHelper/core/DelphiClass_TypeInfo_tkClass.py:28-54`
//! for the Delphi-side pointer arithmetic; FPC `rtl/objpas/typinfo.pp:799-806`
//! for the FPC-side record layout. They agree.
//!
//! ## TTypeKind values
//!
//! **Delphi and FPC use incompatible `TTypeKind` enums.** Delphi orders
//! them as `tkUnknown, tkInteger, tkChar, tkEnumeration, tkFloat, tkString,
//! tkSet, tkClass, tkMethod, …` (so `tkClass == 7`). FPC reorders to
//! `tkUnknown, tkInteger, tkChar, tkEnumeration, tkFloat, tkSet, tkMethod,
//! tkSString, tkLString, tkAString, tkWString, tkVariant, tkArray, tkRecord,
//! tkInterface, tkClass, …` (so `tkClass == 15`). Sources:
//!
//! - Delphi: pythia `pythia/core/structures.py:11-34`
//! - FPC: FPC `rtl/inc/rttih.inc:29-34`
//!
//! This module honors the `VmtFlavor` tag and dispatches to the right
//! expected byte when validating `tkClass`.

use std::{collections::HashMap, ops::Range, str};

use crate::{
    extrtti::{AttributeEntry, Visibility, read_attr_data},
    formats::BinaryContext,
    interfaces::Guid,
    limits::{
        MAX_ENUM_RANGE, MAX_IDENTIFIER_BYTES, MAX_METHOD_PARAMS, MAX_METHODS_PER_RECORD,
        MAX_RECORD_FIELDS, MAX_RECORD_MANAGED_FIELDS,
    },
    util::{
        GENERIC_NAME_CHARS, as_identifier, deref_va, read_ptr, read_short_string_at_file,
        read_short_string_at_va, read_u16, read_u32,
    },
    vmt::{Vmt, VmtFlavor},
};

/// Raw byte value of `tkClass` per-flavor.
pub const fn tkclass_byte(flavor: VmtFlavor) -> u8 {
    match flavor {
        VmtFlavor::Delphi => 7,
        VmtFlavor::Fpc => 15,
    }
}

// Compile-time check the per-flavor `tkClass` byte values against the
// authoritative upstream sources (pythia, FPC's `rttih.inc`). Drift here
// would silently misclassify VMTs.
const _: () = {
    assert!(tkclass_byte(VmtFlavor::Delphi) == 7); // pythia/structures.py:19
    assert!(tkclass_byte(VmtFlavor::Fpc) == 15); // fpc-source/rtl/inc/rttih.inc:32
};

/// Maps a raw TTypeKind byte to the unified [`TypeKind`] enum, given the
/// flavor that emitted the binary. Because FPC reorders the enum, the
/// same byte value means different things in each flavor.
pub fn classify_kind_byte(byte: u8, flavor: VmtFlavor) -> TypeKind {
    match flavor {
        VmtFlavor::Delphi => TypeKind::from_u8(byte),
        VmtFlavor::Fpc => fpc_kind_from_byte(byte),
    }
}

/// FPC ordering (from FPC `rtl/inc/rttih.inc:29-34`):
/// `tkUnknown, tkInteger, tkChar, tkEnumeration, tkFloat, tkSet, tkMethod,
/// tkSString, tkLString, tkAString, tkWString, tkVariant, tkArray,
/// tkRecord, tkInterface, tkClass, tkObject, tkWChar, tkBool, tkInt64,
/// tkQWord, tkDynArray, tkInterfaceRaw, tkProcVar, tkUString, tkUChar,
/// tkHelper, tkFile, tkClassRef, tkPointer`.
fn fpc_kind_from_byte(byte: u8) -> TypeKind {
    match byte {
        0 => TypeKind::Unknown,
        1 => TypeKind::Integer,
        2 => TypeKind::Char,
        3 => TypeKind::Enumeration,
        4 => TypeKind::Float,
        5 => TypeKind::Set,
        6 => TypeKind::Method,
        7 => TypeKind::String,  // FPC tkSString
        8 => TypeKind::LString, // FPC tkLString
        9 => TypeKind::LString, // FPC tkAString - treat as AnsiString-like
        10 => TypeKind::WString,
        11 => TypeKind::Variant,
        12 => TypeKind::Array,
        13 => TypeKind::Record,
        14 => TypeKind::Interface,
        15 => TypeKind::Class,
        // 16 is tkObject - no Delphi equivalent; closest is Class.
        16 => TypeKind::Class,
        17 => TypeKind::WChar,
        // 18 is tkBool - Delphi doesn't have a dedicated kind (booleans
        // are enumerations in Delphi), so map to Enumeration.
        18 => TypeKind::Enumeration,
        19 => TypeKind::Int64,
        20 => TypeKind::Int64, // tkQWord
        21 => TypeKind::DynArray,
        22 => TypeKind::Interface, // tkInterfaceRaw
        23 => TypeKind::Procedure, // tkProcVar
        24 => TypeKind::UString,
        25 => TypeKind::WChar, // tkUChar
        // tkHelper / tkFile have no Delphi equivalent.
        26 | 27 => TypeKind::Unknown,
        28 => TypeKind::ClassRef,
        29 => TypeKind::Pointer,
        _ => TypeKind::Unknown,
    }
}

/// The Delphi `TTypeKind` enumeration. FPC's enum is **different** - see
/// module docs - and we do not expose it as a single shared type because
/// the semantic overlap is too small for a unified `TypeKind` to be honest.
/// When you need FPC values, dispatch on `VmtFlavor` and compare the raw
/// byte directly (or use [`tkclass_byte`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum TypeKind {
    /// Unknown / not a valid type info.
    Unknown = 0,
    /// Signed / unsigned integer types (`Integer`, `Byte`, …).
    Integer = 1,
    /// Single-byte character types.
    Char = 2,
    /// User-declared enumerations.
    Enumeration = 3,
    /// Single / double / extended float types.
    Float = 4,
    /// `ShortString`.
    String = 5,
    /// Pascal set types.
    Set = 6,
    /// Classes / `TObject` hierarchy.
    Class = 7,
    /// Method-of-object pointers.
    Method = 8,
    /// Wide character.
    WChar = 9,
    /// `AnsiString` / `RawByteString`.
    LString = 10,
    /// `WideString` (COM-style BSTR).
    WString = 11,
    /// `Variant` / `OleVariant`.
    Variant = 12,
    /// Static array types.
    Array = 13,
    /// `record` types.
    Record = 14,
    /// COM-style interfaces.
    Interface = 15,
    /// 64-bit integer types.
    Int64 = 16,
    /// Dynamic arrays.
    DynArray = 17,
    /// `UnicodeString` (Delphi 2009+).
    UString = 18,
    /// Class references (metaclasses).
    ClassRef = 19,
    /// Typed pointers.
    Pointer = 20,
    /// Procedure types.
    Procedure = 21,
    /// Managed records (Delphi 10.4+).
    MRecord = 22,
}

impl TypeKind {
    /// Decodes a raw byte. Values outside the documented range map to
    /// [`TypeKind::Unknown`] rather than causing a parse failure.
    pub fn from_u8(b: u8) -> Self {
        match b {
            1 => TypeKind::Integer,
            2 => TypeKind::Char,
            3 => TypeKind::Enumeration,
            4 => TypeKind::Float,
            5 => TypeKind::String,
            6 => TypeKind::Set,
            7 => TypeKind::Class,
            8 => TypeKind::Method,
            9 => TypeKind::WChar,
            10 => TypeKind::LString,
            11 => TypeKind::WString,
            12 => TypeKind::Variant,
            13 => TypeKind::Array,
            14 => TypeKind::Record,
            15 => TypeKind::Interface,
            16 => TypeKind::Int64,
            17 => TypeKind::DynArray,
            18 => TypeKind::UString,
            19 => TypeKind::ClassRef,
            20 => TypeKind::Pointer,
            21 => TypeKind::Procedure,
            22 => TypeKind::MRecord,
            _ => TypeKind::Unknown,
        }
    }
}

/// Partially-parsed `tkClass` TypeInfo record.
///
/// Only the `tkClass` header is decoded here. `prop_count` is the count
/// advertised by the TypeData header; the property array itself is walked
/// by [`crate::properties`] from [`TkClassInfo::prop_data_file_offset`].
#[derive(Debug, Clone, Copy)]
pub struct TkClassInfo<'a> {
    /// VA of the PTypeInfo record this was parsed from.
    pub type_info_va: u64,
    /// The Kind byte. Always [`TypeKind::Class`] in this struct, but kept
    /// so callers can distinguish error paths.
    pub kind: TypeKind,
    /// Class name as it appears in the PTypeInfo header (should match the
    /// VMT's own `class_name`).
    class_name: &'a [u8],
    /// VA the TypeData's `ClassType` slot points at (should equal the VMT
    /// base address).
    pub class_type_va: u64,
    /// VA the TypeData's `ParentInfo` slot holds. This is a `PPTypeInfo` -
    /// a pointer to a pointer to the parent's PTypeInfo record.
    pub parent_info_va: u64,
    /// Published-property count advertised by the TypeData header.
    /// **Note:** the RTTI stream then carries a `TPropData` block whose own
    /// `PropCount` may differ (ancestry-aware vs self-only semantics; which
    /// one each compiler writes is not settled).
    pub prop_count: i16,
    /// Unit name as a short-string body.
    unit_name: &'a [u8],
    /// File offset where the `TPropData` block starts, where the property
    /// walk begins.
    pub prop_data_file_offset: usize,
    /// The virtual address just past the bytes this decode read: the end of
    /// what [`TypeDetail::extent`] states. Kind-specific data the decoder
    /// does not read lies past it.
    end: u64,
}

impl<'a> TkClassInfo<'a> {
    /// Class name as `&str`: UTF-8, as Delphi 2009 and later store names, or `"<non-ascii>"`
    /// when the bytes are not valid UTF-8.
    #[inline]
    pub fn class_name(&self) -> &'a str {
        str::from_utf8(self.class_name).unwrap_or("<non-ascii>")
    }
    /// Raw class-name bytes.
    #[inline]
    pub fn class_name_bytes(&self) -> &'a [u8] {
        self.class_name
    }
    /// Unit name as `&str`: UTF-8, as Delphi 2009 and later store names, or `"<non-ascii>"`
    /// when the bytes are not valid UTF-8.
    #[inline]
    pub fn unit_name(&self) -> &'a str {
        str::from_utf8(self.unit_name).unwrap_or("<non-ascii>")
    }
    /// Raw unit-name bytes (short-string body).
    #[inline]
    pub fn unit_name_bytes(&self) -> &'a [u8] {
        self.unit_name
    }
}

impl<'a> TkClassInfo<'a> {
    /// Decodes the `tkClass` TypeInfo record referenced by a class's VMT.
    ///
    /// Returns `None` when `vmtTypeInfo` is null, the VA cannot be
    /// translated, the Kind byte is not the `tkClass` value for this
    /// flavor, or any length read fails.
    pub fn from_vmt(ctx: &BinaryContext<'a>, vmt: &Vmt<'a>) -> Option<Self> {
        if vmt.type_info == 0 {
            return None;
        }
        Self::from_va(ctx, vmt.type_info, vmt.pointer_size as usize, vmt.flavor)
    }

    /// Resolves a `ParentInfo` (`PPTypeInfo`) value into the parent
    /// class's `tkClass` record.
    pub fn from_parent_info(
        ctx: &BinaryContext<'a>,
        parent_info_va: u64,
        ptr_size: usize,
        flavor: VmtFlavor,
    ) -> Option<Self> {
        let type_info_va = deref_va(ctx, parent_info_va, ptr_size)?;
        Self::from_va(ctx, type_info_va, ptr_size, flavor)
    }

    /// Decodes a `tkClass` TypeInfo at an arbitrary VA, given the flavor
    /// that produced the binary (determines which byte value to expect
    /// for `tkClass`).
    pub fn from_va(
        ctx: &BinaryContext<'a>,
        type_info_va: u64,
        ptr_size: usize,
        flavor: VmtFlavor,
    ) -> Option<Self> {
        let file_off = ctx.va_to_file(type_info_va)?;
        let data = ctx.data();

        let kind_byte = *data.get(file_off)?;
        if kind_byte != tkclass_byte(flavor) {
            return None;
        }
        // Always tag as Delphi's TypeKind::Class - callers who need the
        // raw byte use `kind_byte` directly.
        let kind = TypeKind::Class;

        let class_name_off = file_off.checked_add(1)?;
        let class_name = read_short_string_at_file(data, class_name_off)?;
        let mut type_data_off = file_off.checked_add(2)?.checked_add(class_name.len())?;

        // On `FPC_REQUIRES_PROPER_ALIGNMENT` targets (ARM / AArch64) the
        // first TypeData field is pointer-aligned after the preceding
        // `Name: ShortString`. x86 / x86-64 stay packed on *every* container
        // - including ELF and Mach-O - so the decision is made from the
        // architecture, not from "is this PE?".
        // Source: FPC `rtl/objpas/typinfo.pp:867-871`.
        if ptr_size > 1 && ctx.target_arch().fpc_requires_proper_alignment() {
            let rem = type_data_off.checked_rem(ptr_size)?;
            if rem != 0 {
                let pad = ptr_size.checked_sub(rem)?;
                type_data_off = type_data_off.checked_add(pad)?;
            }
        }

        let class_type_va = read_ptr(data, type_data_off, ptr_size)?;
        let parent_info_va_off = type_data_off.checked_add(ptr_size)?;
        let parent_info_va = read_ptr(data, parent_info_va_off, ptr_size)?;
        let prop_count_off = parent_info_va_off.checked_add(ptr_size)?;
        let prop_count = read_u16(data, prop_count_off)? as i16;
        let unit_name_off = prop_count_off.checked_add(2)?;
        let unit_name = read_short_string_at_file(data, unit_name_off)?;

        if !is_plausible_identifier(class_name) || !is_plausible_identifier(unit_name) {
            return None;
        }

        let prop_data_file_offset = unit_name_off.checked_add(1)?.checked_add(unit_name.len())?;

        Some(Self {
            type_info_va,
            kind,
            class_name,
            class_type_va,
            parent_info_va,
            prop_count,
            unit_name,
            prop_data_file_offset,
            end: va_at(type_info_va, file_off, prop_data_file_offset)?,
        })
    }
}

/// The virtual address of the file offset `end`, in the record that starts
/// at `start` (file offset) and `va`.
fn va_at(va: u64, start: usize, end: usize) -> Option<u64> {
    va.checked_add(u64::try_from(end.checked_sub(start)?).ok()?)
}

/// Minimal identifier sanity check - same rules as [`crate::vmt`] class
/// names but allows lowercase (unit names conventionally begin with a
/// capital but `SysInit`/`system`/etc. can appear lower).
fn is_plausible_identifier(name: &[u8]) -> bool {
    name.len() <= MAX_IDENTIFIER_BYTES && as_identifier(name, &GENERIC_NAME_CHARS).is_some()
}

/// A minimally-decoded RTTI type record: its Kind byte, classified Kind,
/// and name. Used to render declared types for properties / fields /
/// method return values without having to know the full per-Kind layout.
#[derive(Debug, Clone, Copy)]
pub struct TypeHeader<'a> {
    /// Virtual address of the PTypeInfo record.
    pub va: u64,
    /// Raw Kind byte as it appears on disk.
    pub kind_byte: u8,
    /// Kind mapped through the flavor into the unified [`TypeKind`] enum.
    pub kind: TypeKind,
    name: &'a [u8],
}

impl<'a> TypeHeader<'a> {
    /// Builds a header by hand: the `PTypeInfo` VA, the raw Kind byte, its
    /// classified [`TypeKind`] and the name bytes. [`Self::from_va`] decodes
    /// one from a binary.
    pub fn new(va: u64, kind_byte: u8, kind: TypeKind, name: &'a [u8]) -> Self {
        Self {
            va,
            kind_byte,
            kind,
            name,
        }
    }

    /// Type name as `&str`: UTF-8, as Delphi 2009 and later store names, or `"<non-ascii>"`
    /// when the bytes are not valid UTF-8.
    #[inline]
    pub fn name(&self) -> &'a str {
        str::from_utf8(self.name).unwrap_or("<non-ascii>")
    }

    /// Raw type-name bytes, borrowed from the input.
    #[inline]
    pub fn name_bytes(&self) -> &'a [u8] {
        self.name
    }

    /// Returns the virtual addresses of the header: the Kind byte and the
    /// name short string. The kind-specific data after it is not measured.
    pub fn extent(&self) -> Option<Range<u64>> {
        let len = u64::try_from(self.name.len()).ok()?.checked_add(2)?;
        Some(self.va..self.va.checked_add(len)?)
    }

    /// Decodes the type header (Kind + Name) at a `PTypeInfo` VA.
    pub fn from_va(ctx: &BinaryContext<'a>, type_info_va: u64, flavor: VmtFlavor) -> Option<Self> {
        if type_info_va == 0 {
            return None;
        }
        let file_off = ctx.va_to_file(type_info_va)?;
        let data = ctx.data();
        let kind_byte = *data.get(file_off)?;
        let kind = classify_kind_byte(kind_byte, flavor);
        let name_off = file_off.checked_add(1)?;
        let name = read_short_string_at_file(data, name_off)?;
        // C++Builder emits `__closure` typedefs as nameless tkMethod records
        // (`08 00` then the TypeData); every other kind must be named.
        let nameless_closure = name.is_empty() && kind == TypeKind::Method;
        if !nameless_closure && !is_plausible_identifier(name) {
            return None;
        }
        Some(Self {
            va: type_info_va,
            kind_byte,
            kind,
            name,
        })
    }

    /// Follows a `PPTypeInfo` (pointer-to-pointer-to-PTypeInfo)
    /// indirection and returns the target type header. Delphi's
    /// `PropType` field in `TPropInfo` is stored as a `PPTypeInfo`.
    pub fn from_pptr(
        ctx: &BinaryContext<'a>,
        pptr_va: u64,
        ptr_size: usize,
        flavor: VmtFlavor,
    ) -> Option<Self> {
        let type_info_va = Self::deref_pptypeinfo(ctx, pptr_va, ptr_size)?;
        Self::from_va(ctx, type_info_va, flavor)
    }

    /// Dereferences a `PPTypeInfo` VA once, returning the `PTypeInfo` VA
    /// it points at. Returns `None` when either VA is null or unmapped.
    #[inline]
    pub fn deref_pptypeinfo(ctx: &BinaryContext<'_>, pp_va: u64, ptr_size: usize) -> Option<u64> {
        deref_va(ctx, pp_va, ptr_size)
    }
}

/// Storage width of an ordinal / enumeration type.
///
/// Source: pythia `pythia/core/structures.py:49-56`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrdinalType {
    /// Signed 8-bit.
    SByte,
    /// Unsigned 8-bit.
    UByte,
    /// Signed 16-bit.
    SWord,
    /// Unsigned 16-bit.
    UWord,
    /// Signed 32-bit.
    SLong,
    /// Unsigned 32-bit.
    ULong,
    /// Unknown byte value - falls through gracefully.
    Unknown(u8),
}

impl OrdinalType {
    fn from_u8(b: u8) -> Self {
        match b {
            0 => OrdinalType::SByte,
            1 => OrdinalType::UByte,
            2 => OrdinalType::SWord,
            3 => OrdinalType::UWord,
            4 => OrdinalType::SLong,
            5 => OrdinalType::ULong,
            o => OrdinalType::Unknown(o),
        }
    }

    /// Storage width in bytes of this ordinal.
    pub fn size(self) -> usize {
        match self {
            OrdinalType::SByte | OrdinalType::UByte => 1,
            OrdinalType::SWord | OrdinalType::UWord => 2,
            OrdinalType::SLong | OrdinalType::ULong => 4,
            OrdinalType::Unknown(_) => 0,
        }
    }
}

/// Partially-decoded `tkEnumeration` type info.
///
/// Source layout: pythia `pythia/core/structures.py:104-112`;
/// Delphi 6 `TypInfo.pas` (`TTypeData` and `GetEnumName`).
///
/// ```text
///   OrdType:  u8
///   MinValue: i32
///   MaxValue: i32
///   BaseType: PPTypeInfo
///   NameList: ShortString[]   only on a root enumeration (BaseType = itself)
///   UnitName: ShortString     absent on C++Builder's `bool` and on Delphi
///                             6's compiler-intrinsic `Boolean` / `ByteBool`
///                             / `WordBool` / `LongBool` (the names are
///                             followed by alignment filler such as
///                             `8B C0`); Delphi XE+ writes `System` there
/// ```
///
/// Three shapes share the kind:
///
/// - A **root enumeration** (`BaseType` refers to the record itself) lists
///   `MaxValue + 1` names.
/// - A **subrange** of an enumeration (`TLeftRight = taLeftJustify ..
///   taRightJustify`) has no `NameList`; its names are the base type's,
///   indexed by ordinal, and the unit name follows `BaseType` directly.
///   When the base lives in another module (a runtime package: `BaseType`
///   is an import slot), the names are not in this binary and `values` is
///   empty (`TCustomLinkLabel.TLinkAlignment` in Seattle's `vcl230.bpl`).
/// - A **boolean-like** type (`ByteBool`, `WordBool`, `LongBool`) spans the
///   whole signed range of its storage (`MinValue < 0`) yet lists just
///   `False` and `True`; `GetEnumName` maps any non-zero value to `True`.
#[derive(Debug, Clone)]
pub struct EnumInfo<'a> {
    /// Header of the enumeration type.
    pub header: TypeHeader<'a>,
    /// Storage width (determines how enum literals are encoded on disk).
    pub ord: OrdinalType,
    /// Minimum ordinal value (usually `0`).
    pub min: i32,
    /// Maximum ordinal value (i.e. the highest element's index).
    pub max: i32,
    /// VA of the enumeration's base-type `PPTypeInfo` (for sub-ranged
    /// enum types; `0` for top-level enum declarations).
    pub base_type_ref: u64,
    /// Element names in ordinal order, `MinValue` first: the type's own
    /// names, or for a subrange the slice of its base type's names. A
    /// boolean-like type lists `False`, `True`.
    pub values: Vec<&'a [u8]>,
    unit_name: Option<&'a [u8]>,
    /// The virtual address just past the bytes this decode read: the end of
    /// what [`TypeDetail::extent`] states. Kind-specific data the decoder
    /// does not read lies past it.
    end: u64,
}

impl<'a> EnumInfo<'a> {
    /// Builds an enumeration by hand, e.g. to render the values of a type
    /// known from another source. [`Self::from_va`] decodes one from a
    /// binary; a hand-built one's [`TypeDetail::extent`] covers only its
    /// header.
    pub fn new(
        header: TypeHeader<'a>,
        ord: OrdinalType,
        min: i32,
        max: i32,
        base_type_ref: u64,
        values: Vec<&'a [u8]>,
        unit_name: Option<&'a [u8]>,
    ) -> Self {
        let end = header.extent().map_or(header.va, |extent| extent.end);
        Self {
            header,
            ord,
            min,
            max,
            base_type_ref,
            values,
            unit_name,
            end,
        }
    }

    /// Unit name the enumeration was declared in: UTF-8, as Delphi 2009 and
    /// later store names, or `"<non-ascii>"` when the bytes are not valid
    /// UTF-8.
    #[inline]
    pub fn unit_name(&self) -> Option<&'a str> {
        self.unit_name
            .map(|b| str::from_utf8(b).unwrap_or("<non-ascii>"))
    }
    /// Raw unit-name bytes.
    #[inline]
    pub fn unit_name_bytes(&self) -> Option<&'a [u8]> {
        self.unit_name
    }
}

impl<'a> EnumInfo<'a> {
    /// Decodes a `tkEnumeration` record at `type_info_va`.
    pub fn from_va(ctx: &BinaryContext<'a>, type_info_va: u64, flavor: VmtFlavor) -> Option<Self> {
        Self::decode(ctx, type_info_va, flavor, true)
    }

    /// Decodes the record; a subrange is followed to its base only when
    /// `follow_base`, and a base must be a root enumeration - so a crafted
    /// cycle of subranges cannot recurse.
    fn decode(
        ctx: &BinaryContext<'a>,
        type_info_va: u64,
        flavor: VmtFlavor,
        follow_base: bool,
    ) -> Option<Self> {
        let header = TypeHeader::from_va(ctx, type_info_va, flavor)?;
        if header.kind != TypeKind::Enumeration {
            return None;
        }
        let data = ctx.data();
        let file_off = ctx.va_to_file(type_info_va)?;
        let mut cursor = file_off.checked_add(2)?.checked_add(header.name.len())?;

        // `OrdType: u8` lives at the start of the tkEnumeration TypeData.
        let ord_byte = *data.get(cursor)?;
        let ord = OrdinalType::from_u8(ord_byte);
        cursor = cursor.checked_add(1)?;

        let min_end = cursor.checked_add(4)?;
        let min = i32::from_le_bytes(data.get(cursor..min_end)?.try_into().ok()?);
        cursor = min_end;
        let max_end = cursor.checked_add(4)?;
        let max = i32::from_le_bytes(data.get(cursor..max_end)?.try_into().ok()?);
        cursor = max_end;
        // BaseTypePtr is a PPTypeInfo. We require a known pointer size
        // from the parsed container; bail rather than guessing.
        let ptr_size = ctx.pointer_size()?.min(8);
        let bt_end = cursor.checked_add(ptr_size)?;
        let base_type_ref = match ptr_size {
            8 => u64::from_le_bytes(data.get(cursor..bt_end)?.try_into().ok()?),
            _ => {
                let four_end = cursor.checked_add(4)?;
                u32::from_le_bytes(data.get(cursor..four_end)?.try_into().ok()?) as u64
            }
        };
        cursor = bt_end;

        let base_va = TypeHeader::deref_pptypeinfo(ctx, base_type_ref, ptr_size);
        let base = base_va
            .filter(|&base| base != type_info_va)
            .and_then(|base| follow_base.then(|| Self::decode(ctx, base, flavor, false)))
            .flatten();
        let values = if let Some(base) = base {
            // Subrange: names live in the base type, indexed by ordinal.
            let first = usize::try_from(min.checked_sub(base.min)?).ok()?;
            let last = usize::try_from(max.checked_sub(base.min)?).ok()?;
            base.values.get(first..=last)?.to_vec()
        } else if let Some((names, after)) = read_enum_names(data, cursor, min, max) {
            // A root enumeration lists its own names. (FPC's root `BaseType`
            // need not dereference to the record itself.)
            cursor = after;
            names
        } else if base_va != Some(type_info_va) && follow_base && min <= max {
            // A subrange whose base lives in another module: the names are
            // not in this binary.
            Vec::new()
        } else {
            return None;
        };
        // Trailing UnitName ShortString. C++Builder omits it (its `bool` is
        // followed directly by alignment padding), so only a plausible
        // identifier counts as one.
        let unit_name =
            read_short_string_at_file(data, cursor).filter(|u| is_plausible_identifier(u));
        let end_off = match unit_name {
            Some(unit) => cursor.checked_add(1)?.checked_add(unit.len())?,
            None => cursor,
        };

        Some(Self {
            header,
            ord,
            min,
            max,
            base_type_ref,
            values,
            unit_name,
            end: va_at(type_info_va, file_off, end_off)?,
        })
    }
}

/// Reads a root enumeration's name list at file offset `at`: two names for
/// a boolean-like type (`min < 0`), otherwise `max - min + 1`. Answers the
/// names and the offset just past them; `None` when any name is
/// implausible or the range is out of bounds.
fn read_enum_names(data: &[u8], at: usize, min: i32, max: i32) -> Option<(Vec<&[u8]>, usize)> {
    // i64 arithmetic avoids overflow on extreme min/max from misaligned
    // RTTI.
    let count = if min < 0 {
        2
    } else {
        let range = (max as i64).checked_sub(min as i64)?;
        if !(0..=MAX_ENUM_RANGE).contains(&range) {
            return None;
        }
        usize::try_from(range.checked_add(1)?).ok()?
    };
    let mut cursor = at;
    let mut names = Vec::with_capacity(count.min(256));
    for _ in 0..count {
        let name = read_short_string_at_file(data, cursor)?;
        if !is_plausible_identifier(name) {
            return None;
        }
        cursor = cursor.checked_add(1)?.checked_add(name.len())?;
        names.push(name);
    }
    Some((names, cursor))
}

/// Width of the `Single` / `Double` / `Extended` / `Comp` / `Currency`
/// float types. Stored as a single byte at `+0` of `tkFloat` TypeData.
///
/// Source: pythia `pythia/core/structures.py:58-64`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FloatType {
    /// 4-byte IEEE 754.
    Single,
    /// 8-byte IEEE 754.
    Double,
    /// 10-byte Intel 80-bit extended precision.
    Extended,
    /// 8-byte Int64 scaled - historical Borland "Comp" type.
    Comp,
    /// 8-byte Int64 scaled by 10 000 - Delphi `Currency`.
    Currency,
    /// Unknown discriminator byte.
    Unknown(u8),
}

impl FloatType {
    fn from_u8(b: u8) -> Self {
        match b {
            0 => FloatType::Single,
            1 => FloatType::Double,
            2 => FloatType::Extended,
            3 => FloatType::Comp,
            4 => FloatType::Currency,
            other => FloatType::Unknown(other),
        }
    }
}

/// `tkInteger` / `tkChar` / `tkWChar` - bounded ordinal types.
#[derive(Debug, Clone, Copy)]
pub struct OrdinalInfo<'a> {
    /// Type header (Kind + Name).
    pub header: TypeHeader<'a>,
    /// Storage width discriminator.
    pub ord: OrdinalType,
    /// Minimum value (interpreted per `ord.size()` and signedness).
    pub min: i32,
    /// Maximum value.
    pub max: i32,
    /// The virtual address just past the bytes this decode read: the end of
    /// what [`TypeDetail::extent`] states. Kind-specific data the decoder
    /// does not read lies past it.
    end: u64,
}

/// `tkFloat` - floating-point types.
#[derive(Debug, Clone, Copy)]
pub struct FloatInfo<'a> {
    /// Type header.
    pub header: TypeHeader<'a>,
    /// Which float width this is.
    pub float_type: FloatType,
    /// The virtual address just past the bytes this decode read: the end of
    /// what [`TypeDetail::extent`] states. Kind-specific data the decoder
    /// does not read lies past it.
    end: u64,
}

/// `tkSet` - set-of-enumeration types.
#[derive(Debug, Clone, Copy)]
pub struct SetInfo<'a> {
    /// Header (name + Kind).
    pub header: TypeHeader<'a>,
    /// VA of the base-type pointer. Dereference once to reach the element
    /// enumeration's `PTypeInfo`.
    pub comp_type_ref: u64,
    /// Convenience: the resolved element type header if the pointer was
    /// walkable. `None` when the indirection fails.
    pub element_type: Option<TypeHeader<'a>>,
    /// The element type's lowest and highest ordinal, when it resolves to
    /// an enumeration, integer or character type.
    pub bounds: Option<(i32, i32)>,
    /// The set's size in bytes: with bit 7 of `SetTypeOrSize` set, its low
    /// bits (`System.TypInfo.SizeOfSet`); otherwise from [`Self::bounds`]
    /// (one byte per eight ordinals from the lowest multiple of 8, three
    /// widened to four), since before bit 7 the byte is the elements'
    /// ordinal type even for a set wider than 32 bits. `None` when the
    /// element type does not resolve.
    pub size: Option<u32>,
    /// The virtual address just past the bytes this decode read: the end of
    /// what [`TypeDetail::extent`] states. Kind-specific data the decoder
    /// does not read lies past it.
    end: u64,
}

/// `tkClassRef` - `class of TSomething` metaclass references.
#[derive(Debug, Clone, Copy)]
pub struct ClassRefInfo<'a> {
    /// Header.
    pub header: TypeHeader<'a>,
    /// VA of the referenced class's `PPTypeInfo`.
    pub instance_type_ref: u64,
    /// Resolved instance type header, if the indirection succeeded.
    pub instance_type: Option<TypeHeader<'a>>,
    /// The virtual address just past the bytes this decode read: the end of
    /// what [`TypeDetail::extent`] states. Kind-specific data the decoder
    /// does not read lies past it.
    end: u64,
}

/// `tkDynArray` - dynamic array types.
#[derive(Debug, Clone, Copy)]
pub struct DynArrayInfo<'a> {
    /// Header.
    pub header: TypeHeader<'a>,
    /// Size of one element in bytes.
    pub elem_size: u32,
    /// Element type VA (non-zero only for managed element types - strings,
    /// interfaces, other dynamic arrays, etc.). Null for plain scalar
    /// element types. See `DynArrayInfo::elem_type_any` for the unified
    /// element-type view.
    pub elem_type_ref_managed: u64,
    /// Element type VA emitted for *all* element types on modern Delphi.
    pub elem_type_ref_any: u64,
    /// Resolved element type header if either VA points at a usable record.
    pub element_type: Option<TypeHeader<'a>>,
    unit_name: Option<&'a [u8]>,
    /// The virtual address just past the bytes this decode read: the end of
    /// what [`TypeDetail::extent`] states. Kind-specific data the decoder
    /// does not read lies past it.
    end: u64,
}

impl<'a> DynArrayInfo<'a> {
    /// Unit name as `&str`: UTF-8, as Delphi 2009 and later store names, or `"<non-ascii>"`
    /// when the bytes are not valid UTF-8.
    #[inline]
    pub fn unit_name(&self) -> Option<&'a str> {
        self.unit_name
            .map(|b| str::from_utf8(b).unwrap_or("<non-ascii>"))
    }
    /// Raw unit-name bytes.
    #[inline]
    pub fn unit_name_bytes(&self) -> Option<&'a [u8]> {
        self.unit_name
    }
}

/// `tkInterface` - COM-style interface types.
#[derive(Debug, Clone, Copy)]
pub struct InterfaceTypeInfo<'a> {
    /// Header.
    pub header: TypeHeader<'a>,
    /// Parent interface's `PPTypeInfo` VA.
    pub parent_ref: u64,
    /// Parent interface header if the indirection succeeded.
    pub parent_type: Option<TypeHeader<'a>>,
    /// Interface flags byte (bit 0 = HasGuid, etc.).
    pub flags: u8,
    /// Interface GUID.
    pub guid: Guid,
    unit_name: Option<&'a [u8]>,
    /// The virtual address just past the bytes this decode read: the end of
    /// what [`TypeDetail::extent`] states. Kind-specific data the decoder
    /// does not read lies past it.
    end: u64,
}

impl<'a> InterfaceTypeInfo<'a> {
    /// Unit name as `&str`: UTF-8, as Delphi 2009 and later store names, or `"<non-ascii>"`
    /// when the bytes are not valid UTF-8.
    #[inline]
    pub fn unit_name(&self) -> Option<&'a str> {
        self.unit_name
            .map(|b| str::from_utf8(b).unwrap_or("<non-ascii>"))
    }
    /// Raw unit-name bytes.
    #[inline]
    pub fn unit_name_bytes(&self) -> Option<&'a [u8]> {
        self.unit_name
    }
}

/// One entry in a `tkRecord` managed-fields list.
#[derive(Debug, Clone, Copy)]
pub struct RecordManagedField<'a> {
    /// Field-type `PPTypeInfo` VA.
    pub type_ref: u64,
    /// Byte offset within the record.
    pub offset: u64,
    /// Resolved field-type header if the indirection succeeded.
    pub field_type: Option<TypeHeader<'a>>,
}

/// One entry in a `tkRecord`'s full (extended) field table - *every* field,
/// not just the managed ones. Emitted by Delphi 2010+.
#[derive(Debug, Clone)]
pub struct RecordField<'a> {
    /// Field-type `PPTypeInfo` VA.
    pub type_ref: u64,
    /// Resolved field-type header, if the indirection succeeded.
    pub field_type: Option<TypeHeader<'a>>,
    /// Byte offset within the record.
    pub offset: u32,
    /// Visibility / flags byte.
    pub flags: u8,
    /// Field name (short-string body; may be empty for anonymous fields).
    name: &'a [u8],
    /// The field's attributes.
    pub attributes: Vec<AttributeEntry<'a>>,
}

impl<'a> RecordField<'a> {
    /// The declared visibility (the low two bits of [`Self::flags`]).
    pub fn visibility(&self) -> Visibility {
        Visibility::from_flags(self.flags)
    }

    /// Field name as `&str`: UTF-8, as Delphi 2009 and later store names, or `"<non-ascii>"`
    /// when the bytes are not valid UTF-8.
    #[inline]
    pub fn name(&self) -> &'a str {
        str::from_utf8(self.name).unwrap_or("<non-ascii>")
    }
    /// Raw field-name bytes.
    #[inline]
    pub fn name_bytes(&self) -> &'a [u8] {
        self.name
    }
}

/// `tkRecord` - value-type record definition.
#[derive(Debug, Clone)]
pub struct RecordInfo<'a> {
    /// Header.
    pub header: TypeHeader<'a>,
    /// Total record size in bytes.
    pub record_size: u32,
    /// Managed-field entries (references that need refcount management -
    /// strings, dynamic arrays, interfaces, other records with managed
    /// members).
    pub managed_fields: Vec<RecordManagedField<'a>>,
    /// Full field table - every field with its name, type, offset, and
    /// visibility. Populated only for Delphi 2010+ records that emit the
    /// extended layout; empty otherwise (pre-2010 Delphi, FPC, or the
    /// synthetic `vmtInitTable` record).
    pub fields: Vec<RecordField<'a>>,
    /// The record's operator table (`RecOps`, Delphi 10.4+ managed
    /// records); `None` when the record has none.
    pub operators: Option<RecordOperators>,
    /// The record's attributes (Delphi 2010+).
    pub attributes: Vec<AttributeEntry<'a>>,
    /// The record's methods, constructors and operators with their
    /// signatures (Delphi 2010+, as `{$RTTI}` selects them).
    pub methods: Vec<RecordMethod<'a>>,
    /// The virtual address just past the bytes this decode read: the end of
    /// what [`TypeDetail::extent`] states. Kind-specific data the decoder
    /// does not read lies past it.
    end: u64,
}

/// A managed record's operator table (`System.TRecordOperatorTable`): up
/// to four pointer-sized slots, `RecFlags` then the code addresses of the
/// `Initialize`, `Finalize` and `Assign` class operators (indices
/// `roiRecFlags` .. `roiAssignOperator`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecordOperators {
    /// `RecFlags` (`rofHasWeakField` = 2, ...); `0` when absent.
    pub flags: u64,
    /// The `class operator Initialize`.
    pub initialize: Option<u64>,
    /// The `class operator Finalize`.
    pub finalize: Option<u64>,
    /// The `class operator Assign`.
    pub assign: Option<u64>,
}

/// What a record method is (`System.Rtti.TRecMethodKind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordMethodKind {
    /// An instance method: it has `Self`, which its signature omits.
    Method,
    /// A `class ... static` method.
    Static,
    /// A constructor.
    Constructor,
    /// A class operator.
    Operator,
}

/// One method of a record's extended RTTI (`TRecordTypeMethod`).
#[derive(Debug, Clone)]
pub struct RecordMethod<'a> {
    /// Flags byte: the kind in bits 0-1, the visibility in bits 2-3.
    pub flags: u8,
    /// Code address of the method.
    pub code_va: u64,
    name: &'a [u8],
    /// The signature, when the compiler wrote one.
    pub signature: Option<ProcedureSignature<'a>>,
    /// The method's attributes.
    pub attributes: Vec<AttributeEntry<'a>>,
}

impl<'a> RecordMethod<'a> {
    /// The method name: UTF-8, as Delphi 2009 and later store names, or `"<non-ascii>"` when
    /// the bytes are not valid UTF-8.
    pub fn name(&self) -> &'a str {
        str::from_utf8(self.name).unwrap_or("<non-ascii>")
    }

    /// What the method is.
    pub fn kind(&self) -> RecordMethodKind {
        match self.flags & 0x03 {
            0 => RecordMethodKind::Method,
            1 => RecordMethodKind::Static,
            2 => RecordMethodKind::Constructor,
            _ => RecordMethodKind::Operator,
        }
    }

    /// The declared visibility.
    pub fn visibility(&self) -> Visibility {
        Visibility::from_flags(self.flags >> 2)
    }
}

/// `tkPointer` - `^T` typed pointer.
#[derive(Debug, Clone, Copy)]
pub struct PointerInfo<'a> {
    /// Header.
    pub header: TypeHeader<'a>,
    /// `PPTypeInfo` VA of the pointed-to type (`0` for untyped `Pointer`).
    pub ref_type_ref: u64,
    /// Resolved pointed-to type header, if the indirection succeeded.
    pub ref_type: Option<TypeHeader<'a>>,
    /// The virtual address just past the bytes this decode read: the end of
    /// what [`TypeDetail::extent`] states. Kind-specific data the decoder
    /// does not read lies past it.
    end: u64,
}

impl<'a> PointerInfo<'a> {
    /// Decodes a `tkPointer` record. The `TypeData` is a single `PPTypeInfo`
    /// at offset 0 (pythia `pythia/core/structures.py:146-149`).
    pub fn from_va(ctx: &BinaryContext<'a>, type_info_va: u64, flavor: VmtFlavor) -> Option<Self> {
        let header = TypeHeader::from_va(ctx, type_info_va, flavor)?;
        if header.kind != TypeKind::Pointer {
            return None;
        }
        let data = ctx.data();
        let ptr_size = ctx.pointer_size()?;
        let off = ctx
            .va_to_file(type_info_va)?
            .checked_add(2)?
            .checked_add(header.name.len())?;
        let ref_type_ref = read_ptr(data, off, ptr_size)?;
        let ref_type = TypeHeader::from_pptr(ctx, ref_type_ref, ptr_size, flavor);
        Some(Self {
            header,
            ref_type_ref,
            ref_type,
            end: va_at(
                type_info_va,
                ctx.va_to_file(type_info_va)?,
                off.checked_add(ptr_size)?,
            )?,
        })
    }
}

/// `tkArray` - fixed-length `array[...] of T`.
#[derive(Debug, Clone, Copy)]
pub struct ArrayInfo<'a> {
    /// Header.
    pub header: TypeHeader<'a>,
    /// Total array size in bytes.
    pub array_size: u32,
    /// Element count.
    pub element_count: u32,
    /// `PPTypeInfo` VA of the element type.
    pub element_type_ref: u64,
    /// Resolved element type header, if the indirection succeeded.
    pub element_type: Option<TypeHeader<'a>>,
    /// The virtual address just past the bytes this decode read: the end of
    /// what [`TypeDetail::extent`] states. Kind-specific data the decoder
    /// does not read lies past it.
    end: u64,
}

impl<'a> ArrayInfo<'a> {
    /// Decodes a `tkArray` record. `TypeData` is `Size: u32`, `ElCount: u32`,
    /// `ElType: PPTypeInfo` (pythia `pythia/core/structures.py:165-172`,
    /// matching Delphi's `TArrayTypeData`). The trailing dimension table is
    /// not decoded here.
    pub fn from_va(ctx: &BinaryContext<'a>, type_info_va: u64, flavor: VmtFlavor) -> Option<Self> {
        let header = TypeHeader::from_va(ctx, type_info_va, flavor)?;
        if header.kind != TypeKind::Array {
            return None;
        }
        let data = ctx.data();
        let ptr_size = ctx.pointer_size()?;
        let base = ctx
            .va_to_file(type_info_va)?
            .checked_add(2)?
            .checked_add(header.name.len())?;
        let array_size = read_u32(data, base)?;
        let element_count = read_u32(data, base.checked_add(4)?)?;
        let element_type_ref = read_ptr(data, base.checked_add(8)?, ptr_size)?;
        let element_type = TypeHeader::from_pptr(ctx, element_type_ref, ptr_size, flavor);
        Some(Self {
            header,
            array_size,
            element_count,
            element_type_ref,
            element_type,
            end: va_at(
                type_info_va,
                ctx.va_to_file(type_info_va)?,
                base.checked_add(8)?.checked_add(ptr_size)?,
            )?,
        })
    }
}

/// Sum type that wraps whichever per-Kind decoder matched.
///
/// `decode_type_detail` returns this, so callers can match on Kind without
/// pre-dispatching. `#[non_exhaustive]`: new per-Kind variants may be added
/// in future releases, so external matches need a wildcard arm.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum TypeDetail<'a> {
    /// tkPointer - `^T` typed pointer.
    Pointer(PointerInfo<'a>),
    /// tkArray - fixed-length array.
    Array(ArrayInfo<'a>),
    /// tkClass - full class record including unit + published-property count.
    Class(TkClassInfo<'a>),
    /// tkEnumeration - element names + bounds.
    Enumeration(EnumInfo<'a>),
    /// tkInteger / tkChar / tkWChar.
    Ordinal(OrdinalInfo<'a>),
    /// tkFloat.
    Float(FloatInfo<'a>),
    /// tkSet.
    Set(SetInfo<'a>),
    /// tkClassRef.
    ClassRef(ClassRefInfo<'a>),
    /// tkDynArray.
    DynArray(DynArrayInfo<'a>),
    /// tkInterface.
    Interface(InterfaceTypeInfo<'a>),
    /// tkRecord.
    Record(RecordInfo<'a>),
    /// tkMethod - method-of-object pointer (event handlers).
    Method(MethodInfo<'a>),
    /// tkProcedure - first-class procedure reference.
    Procedure(ProcedureInfo<'a>),
    /// tkLString / tkUString / tkWString - string RTTI with code page.
    String(StringInfo<'a>),
    /// tkString - a `ShortString` type with its declared capacity.
    ShortString(ShortStringInfo<'a>),
    /// tkInt64 (FPC also tkQWord) - a 64-bit ordinal with its bounds.
    Int64(Int64Info<'a>),
    /// Known Kind but not yet a dedicated decoder - just the header.
    Other(TypeHeader<'a>),
}

impl<'a> TypeDetail<'a> {
    /// Copy of the contained header regardless of variant.
    pub fn header(&self) -> TypeHeader<'a> {
        match self {
            TypeDetail::Class(x) => TypeHeader {
                va: x.type_info_va,
                kind_byte: tkclass_byte(VmtFlavor::Delphi),
                kind: TypeKind::Class,
                name: x.class_name,
            },
            TypeDetail::Enumeration(x) => x.header,
            TypeDetail::Ordinal(x) => x.header,
            TypeDetail::Float(x) => x.header,
            TypeDetail::Set(x) => x.header,
            TypeDetail::ClassRef(x) => x.header,
            TypeDetail::DynArray(x) => x.header,
            TypeDetail::Pointer(x) => x.header,
            TypeDetail::Array(x) => x.header,
            TypeDetail::Interface(x) => x.header,
            TypeDetail::Record(x) => x.header,
            TypeDetail::Method(x) => x.header,
            TypeDetail::Procedure(x) => x.header,
            TypeDetail::String(x) => x.header,
            TypeDetail::ShortString(x) => x.header,
            TypeDetail::Int64(x) => x.header,
            TypeDetail::Other(x) => *x,
        }
    }

    /// Returns the virtual addresses of the record: its header and as much
    /// of its kind-specific data as the decode read. Data the decoder does
    /// not read - a record's extended field table, a class's property data,
    /// a procedure's signature, which lies apart - is not included.
    pub fn extent(&self) -> Option<Range<u64>> {
        let end = match self {
            TypeDetail::Class(x) => x.end,
            TypeDetail::Enumeration(x) => x.end,
            TypeDetail::Ordinal(x) => x.end,
            TypeDetail::Float(x) => x.end,
            TypeDetail::Set(x) => x.end,
            TypeDetail::ClassRef(x) => x.end,
            TypeDetail::DynArray(x) => x.end,
            TypeDetail::Pointer(x) => x.end,
            TypeDetail::Array(x) => x.end,
            TypeDetail::Interface(x) => x.end,
            TypeDetail::Record(x) => x.end,
            TypeDetail::Method(x) => x.end,
            TypeDetail::Procedure(x) => x.end,
            TypeDetail::String(x) => x.end,
            TypeDetail::ShortString(x) => x.end,
            TypeDetail::Int64(x) => x.end,
            TypeDetail::Other(x) => return x.extent(),
        };
        let start = self.header().va;
        (start < end).then_some(start..end)
    }
}

impl<'a> TypeDetail<'a> {
    /// Decodes whichever kind is stored at `type_info_va`, returning the rich
    /// per-kind variant. `layout` is the binary's string layout (see
    /// [`crate::DelphiBinary::string_layout`]).
    pub fn from_va(
        ctx: &BinaryContext<'a>,
        type_info_va: u64,
        flavor: VmtFlavor,
        layout: StringLayout,
    ) -> Option<Self> {
        let header = TypeHeader::from_va(ctx, type_info_va, flavor)?;
        let ptr_size = ctx.pointer_size().unwrap_or(8);
        let detail = match header.kind {
            TypeKind::Class => {
                Self::Class(TkClassInfo::from_va(ctx, type_info_va, ptr_size, flavor)?)
            }
            TypeKind::Enumeration => {
                Self::Enumeration(EnumInfo::from_va(ctx, type_info_va, flavor)?)
            }
            TypeKind::Integer | TypeKind::Char | TypeKind::WChar => {
                Self::Ordinal(OrdinalInfo::from_va(ctx, type_info_va, flavor)?)
            }
            TypeKind::Float => Self::Float(FloatInfo::from_va(ctx, type_info_va, flavor)?),
            TypeKind::Set => Self::Set(SetInfo::from_va(ctx, type_info_va, flavor)?),
            TypeKind::ClassRef => Self::ClassRef(ClassRefInfo::from_va(ctx, type_info_va, flavor)?),
            TypeKind::DynArray => Self::DynArray(DynArrayInfo::from_va(ctx, type_info_va, flavor)?),
            TypeKind::Pointer => Self::Pointer(PointerInfo::from_va(ctx, type_info_va, flavor)?),
            TypeKind::Array => Self::Array(ArrayInfo::from_va(ctx, type_info_va, flavor)?),
            TypeKind::Interface => {
                Self::Interface(InterfaceTypeInfo::from_va(ctx, type_info_va, flavor)?)
            }
            TypeKind::Record | TypeKind::MRecord => {
                Self::Record(RecordInfo::from_va(ctx, type_info_va, flavor)?)
            }
            TypeKind::Method => Self::Method(MethodInfo::from_va(ctx, type_info_va, flavor)?),
            TypeKind::Procedure => {
                Self::Procedure(ProcedureInfo::from_va(ctx, type_info_va, flavor)?)
            }
            TypeKind::LString | TypeKind::UString | TypeKind::WString => {
                Self::String(StringInfo::from_va(ctx, type_info_va, flavor, layout)?)
            }
            TypeKind::String => {
                Self::ShortString(ShortStringInfo::from_va(ctx, type_info_va, flavor)?)
            }
            TypeKind::Int64 => Self::Int64(Int64Info::from_va(ctx, type_info_va, flavor)?),
            _ => Self::Other(header),
        };
        Some(detail)
    }

    /// The `PPTypeInfo` references this record points at - parent types,
    /// enumeration base types, set/array element types, record managed-field
    /// types, etc. Each value is a `PPTypeInfo` VA (dereference once to reach
    /// the target `PTypeInfo`). Drives the transitive-closure type walk in
    /// [`crate::DelphiBinary::types`]; `0` entries are filtered by the
    /// caller. Property/field type references are *not* included here (they
    /// live in the separate `TPropData` / field-table structures, seeded by
    /// the caller).
    pub fn referenced_pptrs(&self) -> Vec<u64> {
        match self {
            TypeDetail::Class(x) => vec![x.parent_info_va],
            TypeDetail::Enumeration(x) => vec![x.base_type_ref],
            TypeDetail::Set(x) => vec![x.comp_type_ref],
            TypeDetail::ClassRef(x) => vec![x.instance_type_ref],
            TypeDetail::DynArray(x) => vec![x.elem_type_ref_any, x.elem_type_ref_managed],
            TypeDetail::Pointer(x) => vec![x.ref_type_ref],
            TypeDetail::Array(x) => vec![x.element_type_ref],
            TypeDetail::Interface(x) => vec![x.parent_ref],
            TypeDetail::Record(x) => x
                .managed_fields
                .iter()
                .map(|m| m.type_ref)
                .chain(x.fields.iter().map(|f| f.type_ref))
                .collect(),
            TypeDetail::Procedure(x) => {
                let mut refs: Vec<u64> = x.params.iter().map(|p| p.type_ref).collect();
                refs.push(x.result_type_ref);
                refs
            }
            TypeDetail::Method(x) => {
                let mut refs = x.param_type_refs.clone();
                refs.push(x.result_type_ref);
                refs
            }
            TypeDetail::Ordinal(_)
            | TypeDetail::Float(_)
            | TypeDetail::String(_)
            | TypeDetail::ShortString(_)
            | TypeDetail::Int64(_)
            | TypeDetail::Other(_) => Vec::new(),
        }
    }
}

impl<'a> OrdinalInfo<'a> {
    /// Decodes a `tkInteger` / `tkChar` / `tkWChar` record.
    pub fn from_va(ctx: &BinaryContext<'a>, type_info_va: u64, flavor: VmtFlavor) -> Option<Self> {
        let header = TypeHeader::from_va(ctx, type_info_va, flavor)?;
        if !matches!(
            header.kind,
            TypeKind::Integer | TypeKind::Char | TypeKind::WChar
        ) {
            return None;
        }
        let data = ctx.data();
        let off = ctx
            .va_to_file(type_info_va)?
            .checked_add(2)?
            .checked_add(header.name.len())?;
        let ord = OrdinalType::from_u8(*data.get(off)?);
        let min_start = off.checked_add(1)?;
        let min_end = off.checked_add(5)?;
        let max_end = off.checked_add(9)?;
        let min = i32::from_le_bytes(data.get(min_start..min_end)?.try_into().ok()?);
        let max = i32::from_le_bytes(data.get(min_end..max_end)?.try_into().ok()?);
        Some(Self {
            header,
            ord,
            min,
            max,
            end: va_at(type_info_va, ctx.va_to_file(type_info_va)?, max_end)?,
        })
    }
}

/// `tkString` - a `ShortString` type (`string[N]`).
///
/// Delphi 6 `TypInfo.pas`: the TypeData is `MaxLength: Byte`, the declared
/// capacity (255 for plain `ShortString`).
#[derive(Debug, Clone, Copy)]
pub struct ShortStringInfo<'a> {
    /// Header of the string type.
    pub header: TypeHeader<'a>,
    /// Declared maximum length in characters.
    pub max_length: u8,
    end: u64,
}

impl<'a> ShortStringInfo<'a> {
    /// Decodes a `tkString` record.
    pub fn from_va(ctx: &BinaryContext<'a>, type_info_va: u64, flavor: VmtFlavor) -> Option<Self> {
        let header = TypeHeader::from_va(ctx, type_info_va, flavor)?;
        if header.kind != TypeKind::String {
            return None;
        }
        let start = ctx.va_to_file(type_info_va)?;
        let off = start.checked_add(2)?.checked_add(header.name.len())?;
        let max_length = *ctx.data().get(off)?;
        Some(Self {
            header,
            max_length,
            end: va_at(type_info_va, start, off.checked_add(1)?)?,
        })
    }
}

/// `tkInt64` (and FPC `tkQWord`) - a 64-bit ordinal type with its bounds.
///
/// Delphi 6 `TypInfo.pas`: the TypeData is `MinInt64Value, MaxInt64Value:
/// Int64`. An unsigned 64-bit type keeps that layout with unsigned bit
/// patterns: Delphi's `UInt64` is `tkInt64` with bounds `0` and `-1`
/// (HeidiSQL XE5), and FPC's `tkQWord` stores `MinQWordValue,
/// MaxQWordValue: QWord`. [`Self::unsigned`] says how to read them.
#[derive(Debug, Clone, Copy)]
pub struct Int64Info<'a> {
    /// Header of the ordinal type.
    pub header: TypeHeader<'a>,
    /// Lower bound, as stored (reinterpret as `u64` when [`Self::unsigned`]).
    pub min: i64,
    /// Upper bound, as stored (reinterpret as `u64` when [`Self::unsigned`]).
    pub max: i64,
    /// `true` when the bounds are unsigned: FPC `tkQWord`, or a Delphi
    /// `tkInt64` whose bounds are ordered only as unsigned values
    /// (`UInt64`: `0 .. $FFFFFFFFFFFFFFFF`).
    pub unsigned: bool,
    end: u64,
}

impl<'a> Int64Info<'a> {
    /// Decodes a `tkInt64` / `tkQWord` record.
    pub fn from_va(ctx: &BinaryContext<'a>, type_info_va: u64, flavor: VmtFlavor) -> Option<Self> {
        let header = TypeHeader::from_va(ctx, type_info_va, flavor)?;
        if header.kind != TypeKind::Int64 {
            return None;
        }
        let data = ctx.data();
        let start = ctx.va_to_file(type_info_va)?;
        let off = start.checked_add(2)?.checked_add(header.name.len())?;
        let read = |at: usize| -> Option<i64> {
            Some(i64::from_le_bytes(
                data.get(at..at.checked_add(8)?)?.try_into().ok()?,
            ))
        };
        let min = read(off)?;
        let max = read(off.checked_add(8)?)?;
        let unsigned = (flavor == VmtFlavor::Fpc && header.kind_byte == 20) || min > max;
        if unsigned && (min as u64) > (max as u64) {
            return None;
        }
        Some(Self {
            header,
            min,
            max,
            unsigned,
            end: va_at(type_info_va, start, off.checked_add(16)?)?,
        })
    }
}

impl<'a> FloatInfo<'a> {
    /// Decodes a `tkFloat` record.
    pub fn from_va(ctx: &BinaryContext<'a>, type_info_va: u64, flavor: VmtFlavor) -> Option<Self> {
        let header = TypeHeader::from_va(ctx, type_info_va, flavor)?;
        if header.kind != TypeKind::Float {
            return None;
        }
        let data = ctx.data();
        let off = ctx
            .va_to_file(type_info_va)?
            .checked_add(2)?
            .checked_add(header.name.len())?;
        let float_type = FloatType::from_u8(*data.get(off)?);
        Some(Self {
            header,
            float_type,
            end: va_at(
                type_info_va,
                ctx.va_to_file(type_info_va)?,
                off.checked_add(1)?,
            )?,
        })
    }
}

impl<'a> SetInfo<'a> {
    /// Decodes a `tkSet` record and resolves the element-enumeration
    /// type, if reachable.
    pub fn from_va(ctx: &BinaryContext<'a>, type_info_va: u64, flavor: VmtFlavor) -> Option<Self> {
        let header = TypeHeader::from_va(ctx, type_info_va, flavor)?;
        if header.kind != TypeKind::Set {
            return None;
        }
        let data = ctx.data();
        let ptr_size = ctx.pointer_size()?;
        // tkSet TypeData = { SetTypeOrSize:u8, CompType:PPTypeInfo, ... }
        // (Delphi 6 names the byte OrdType; a large set's size needs bit 7,
        // which arrived with sets wider than 32 bits).
        let type_off = ctx
            .va_to_file(type_info_va)?
            .checked_add(2)?
            .checked_add(header.name.len())?;
        let type_or_size = *data.get(type_off)?;
        let off = type_off.checked_add(1)?;
        let comp_type_ref = read_ptr(data, off, ptr_size)?;
        let element_type = TypeHeader::from_pptr(ctx, comp_type_ref, ptr_size, flavor);
        let bounds = element_type.and_then(|h| match h.kind {
            TypeKind::Enumeration => EnumInfo::from_va(ctx, h.va, flavor).map(|e| (e.min, e.max)),
            TypeKind::Integer | TypeKind::Char | TypeKind::WChar => {
                OrdinalInfo::from_va(ctx, h.va, flavor).map(|o| (o.min, o.max))
            }
            _ => None,
        });
        // With bit 7 the compiler states the size. Without it the byte is
        // the elements' ordinal type, which says nothing about a set wider
        // than 32 bits on compilers before the bit (XE5 writes a 32-byte
        // `set of AnsiChar` as `otUByte`), so the size comes from the
        // element bounds as the compiler lays sets out: one byte per eight
        // ordinals from the lowest multiple of 8, three bytes widened to
        // four.
        let size = if type_or_size & 0x80 != 0 {
            Some(u32::from(type_or_size & 0x7F))
        } else {
            bounds.and_then(|(min, max)| {
                let bytes = max
                    .div_euclid(8)
                    .checked_sub(min.div_euclid(8))?
                    .checked_add(1)?;
                let bytes = u32::try_from(bytes).ok().filter(|b| (1..=32).contains(b))?;
                Some(if bytes == 3 { 4 } else { bytes })
            })
        };
        Some(Self {
            header,
            comp_type_ref,
            element_type,
            bounds,
            size,
            end: va_at(
                type_info_va,
                ctx.va_to_file(type_info_va)?,
                off.checked_add(ptr_size)?,
            )?,
        })
    }
}

impl<'a> ClassRefInfo<'a> {
    /// Decodes a `tkClassRef` record.
    pub fn from_va(ctx: &BinaryContext<'a>, type_info_va: u64, flavor: VmtFlavor) -> Option<Self> {
        let header = TypeHeader::from_va(ctx, type_info_va, flavor)?;
        if header.kind != TypeKind::ClassRef {
            return None;
        }
        let data = ctx.data();
        let ptr_size = ctx.pointer_size()?;
        let off = ctx
            .va_to_file(type_info_va)?
            .checked_add(2)?
            .checked_add(header.name.len())?;
        let instance_type_ref = read_ptr(data, off, ptr_size)?;
        let instance_type = TypeHeader::from_pptr(ctx, instance_type_ref, ptr_size, flavor);
        Some(Self {
            header,
            instance_type_ref,
            instance_type,
            end: va_at(
                type_info_va,
                ctx.va_to_file(type_info_va)?,
                off.checked_add(ptr_size)?,
            )?,
        })
    }
}

impl<'a> DynArrayInfo<'a> {
    /// Decodes a `tkDynArray` record.
    pub fn from_va(ctx: &BinaryContext<'a>, type_info_va: u64, flavor: VmtFlavor) -> Option<Self> {
        let header = TypeHeader::from_va(ctx, type_info_va, flavor)?;
        if header.kind != TypeKind::DynArray {
            return None;
        }
        let data = ctx.data();
        let ptr_size = ctx.pointer_size()?;
        let mut off = ctx
            .va_to_file(type_info_va)?
            .checked_add(2)?
            .checked_add(header.name.len())?;
        let elem_size_end = off.checked_add(4)?;
        let elem_size = i32::from_le_bytes(data.get(off..elem_size_end)?.try_into().ok()?) as u32;
        off = elem_size_end;
        let elem_type_ref_managed = read_ptr(data, off, ptr_size)?;
        off = off.checked_add(ptr_size)?;
        // VarType index skipped.
        off = off.checked_add(4)?;
        let elem_type_ref_any = read_ptr(data, off, ptr_size)?;
        off = off.checked_add(ptr_size)?;
        let unit_name = read_short_string_at_file(data, off);
        let end_off = match unit_name {
            Some(unit) => off.checked_add(1)?.checked_add(unit.len())?,
            None => off,
        };
        // Pick whichever pointer resolves - `elem_type_ref_any` is
        // emitted on modern Delphi/FPC even for non-managed elements;
        // the managed variant can be null.
        let element_type = if elem_type_ref_any != 0 {
            TypeHeader::from_pptr(ctx, elem_type_ref_any, ptr_size, flavor)
        } else {
            TypeHeader::from_pptr(ctx, elem_type_ref_managed, ptr_size, flavor)
        };
        Some(Self {
            header,
            elem_size,
            elem_type_ref_managed,
            elem_type_ref_any,
            element_type,
            unit_name,
            end: va_at(type_info_va, ctx.va_to_file(type_info_va)?, end_off)?,
        })
    }
}

impl<'a> InterfaceTypeInfo<'a> {
    /// VA just past the record's unit name, where a Delphi interface's
    /// method table begins (see [`crate::intfrtti`]).
    #[inline]
    pub fn method_table_va(&self) -> u64 {
        self.end
    }

    /// Decodes a `tkInterface` record.
    pub fn from_va(ctx: &BinaryContext<'a>, type_info_va: u64, flavor: VmtFlavor) -> Option<Self> {
        let header = TypeHeader::from_va(ctx, type_info_va, flavor)?;
        if header.kind != TypeKind::Interface {
            return None;
        }
        let data = ctx.data();
        let ptr_size = ctx.pointer_size()?;
        let mut off = ctx
            .va_to_file(type_info_va)?
            .checked_add(2)?
            .checked_add(header.name.len())?;
        let parent_ref = read_ptr(data, off, ptr_size)?;
        off = off.checked_add(ptr_size)?;
        let flags = *data.get(off)?;
        off = off.checked_add(1)?;
        let guid_end = off.checked_add(16)?;
        let guid_bytes: [u8; 16] = data.get(off..guid_end)?.try_into().ok()?;
        let guid = Guid::from(guid_bytes);
        off = guid_end;
        let unit_name = read_short_string_at_file(data, off);
        let end_off = match unit_name {
            Some(unit) => off.checked_add(1)?.checked_add(unit.len())?,
            None => off,
        };
        let parent_type = TypeHeader::from_pptr(ctx, parent_ref, ptr_size, flavor);
        Some(Self {
            header,
            parent_ref,
            parent_type,
            flags,
            guid,
            unit_name,
            end: va_at(type_info_va, ctx.va_to_file(type_info_va)?, end_off)?,
        })
    }
}

/// Method kind - matches Delphi's `TMethodKind` and FPC's mkXxx constants
/// in `typinfo.pp`. Source: pythia `pythia/core/structures.py:36-47`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MethodKind {
    /// `procedure` - no result.
    Procedure = 0,
    /// `function` - has a result value.
    Function = 1,
    /// `constructor`.
    Constructor = 2,
    /// `destructor`.
    Destructor = 3,
    /// `class procedure`.
    ClassProcedure = 4,
    /// `class function`.
    ClassFunction = 5,
    /// `class constructor` (Delphi 2010+).
    ClassConstructor = 6,
    /// Operator overload.
    OperatorOverload = 7,
    /// `safecall procedure`.
    SafeProcedure = 8,
    /// `safecall function`.
    SafeFunction = 9,
    /// Unknown / out-of-range byte.
    Unknown = 0xff,
}

impl MethodKind {
    /// Maps a raw `TMethodKind` ordinal; out-of-range values map to
    /// [`MethodKind::Unknown`].
    pub fn from_u8(b: u8) -> Self {
        match b {
            0 => MethodKind::Procedure,
            1 => MethodKind::Function,
            2 => MethodKind::Constructor,
            3 => MethodKind::Destructor,
            4 => MethodKind::ClassProcedure,
            5 => MethodKind::ClassFunction,
            6 => MethodKind::ClassConstructor,
            7 => MethodKind::OperatorOverload,
            8 => MethodKind::SafeProcedure,
            9 => MethodKind::SafeFunction,
            _ => MethodKind::Unknown,
        }
    }
}

/// One formal parameter of a method-of-object or procedure type.
#[derive(Debug, Clone, Copy)]
pub struct MethodParam<'a> {
    /// `Flags` byte - `pfVar`, `pfConst`, `pfArray`, `pfOut`, `pfResult`, etc.
    /// We expose the raw byte; see Embarcadero DocWiki "TParamFlag".
    pub flags: u8,
    name: &'a [u8],
    type_name: &'a [u8],
}

impl<'a> MethodParam<'a> {
    /// Parameter name as `&str`: UTF-8, as Delphi 2009 and later store names, or
    /// `"<non-ascii>"` when the bytes are not valid UTF-8.
    #[inline]
    pub fn name(&self) -> &'a str {
        str::from_utf8(self.name).unwrap_or("<non-ascii>")
    }
    /// Raw parameter name bytes.
    #[inline]
    pub fn name_bytes(&self) -> &'a [u8] {
        self.name
    }
    /// Parameter type name as `&str`: UTF-8, as Delphi 2009 and later store names, or
    /// `"<non-ascii>"` when the bytes are not valid UTF-8. Unlike class RTTI,
    /// method RTTI stores the parameter's type as a textual identifier
    /// rather than a `PPTypeInfo`.
    #[inline]
    pub fn type_name(&self) -> &'a str {
        str::from_utf8(self.type_name).unwrap_or("<non-ascii>")
    }
    /// Raw parameter type-name bytes.
    #[inline]
    pub fn type_name_bytes(&self) -> &'a [u8] {
        self.type_name
    }
}

/// `tkMethod` - method-of-object pointer signature.
///
/// Example: `TNotifyEvent = procedure(Sender: TObject) of object`.
///
/// Layout (from pythia `pythia/core/structures.py:204-214`):
///
/// ```text
///   MethodType: u8            // Procedure / Function / etc.
///   NumParams:  u8
///   Params:     array[NumParams]
///     ParamFlags: u8
///     ParamName:  ShortString
///     TypeName:   ShortString
/// ```
#[derive(Debug, Clone)]
pub struct MethodInfo<'a> {
    /// Type header.
    pub header: TypeHeader<'a>,
    /// Method kind (procedure / function / constructor / …).
    pub kind: MethodKind,
    /// Formal parameter list in declaration order.
    pub params: Vec<MethodParam<'a>>,
    /// Return-type name when [`MethodKind::Function`] / related; empty otherwise.
    pub result_type: Option<&'a [u8]>,
    /// Modern-RTTI (Delphi 2010+) per-parameter `PPTypeInfo` references, in
    /// declaration order. Empty on legacy binaries that carry only the
    /// name-based parameter list.
    pub param_type_refs: Vec<u64>,
    /// Modern-RTTI return-type `PPTypeInfo` reference (`0` when absent).
    pub result_type_ref: u64,
    /// The virtual address just past the bytes this decode read: the end of
    /// what [`TypeDetail::extent`] states. Kind-specific data the decoder
    /// does not read lies past it.
    end: u64,
}

/// One parameter of a `tkProcedure` signature (`TProcedureParam`).
#[derive(Debug, Clone)]
pub struct SignatureParam<'a> {
    /// `TParamFlags` byte (var / const / out / …).
    pub flags: u8,
    /// Parameter-type `PPTypeInfo` VA.
    pub type_ref: u64,
    /// Resolved parameter-type header, if the indirection succeeded.
    pub param_type: Option<TypeHeader<'a>>,
    /// Parameter name (short-string body; may be empty).
    name: &'a [u8],
    /// The parameter's attributes (Delphi 2010+; empty otherwise).
    pub attributes: Vec<AttributeEntry<'a>>,
}

impl<'a> SignatureParam<'a> {
    /// Parameter name as `&str`: UTF-8, as Delphi 2009 and later store names, or
    /// `"<non-ascii>"` when the bytes are not valid UTF-8.
    #[inline]
    pub fn name(&self) -> &'a str {
        str::from_utf8(self.name).unwrap_or("<non-ascii>")
    }
    /// Raw parameter-name bytes.
    #[inline]
    pub fn name_bytes(&self) -> &'a [u8] {
        self.name
    }
}

/// `tkProcedure` - first-class reference-to-procedure type. Decodes the
/// inline `TProcedureSignature` (Delphi 2010+ / FPC); on older binaries that
/// emit only the header the signature fields are empty.
#[derive(Debug, Clone)]
pub struct ProcedureInfo<'a> {
    /// Type header.
    pub header: TypeHeader<'a>,
    /// Calling-convention byte (`TCallConv`); `0xFF` when no signature.
    pub call_conv: u8,
    /// Ordered parameters.
    pub params: Vec<SignatureParam<'a>>,
    /// Result-type `PPTypeInfo` VA (`0` for procedures / void).
    pub result_type_ref: u64,
    /// Resolved result-type header, if present and resolvable.
    pub result_type: Option<TypeHeader<'a>>,
    /// The virtual address just past the bytes this decode read: the end of
    /// what [`TypeDetail::extent`] states. Kind-specific data the decoder
    /// does not read lies past it.
    end: u64,
}

/// The string RTTI layout a binary's runtime emits.
///
/// Code-page-aware runtimes (Delphi 2009 and later, Free Pascal 3.0 and
/// later) stamp a `u16` code page right after the name of every ANSI string
/// record (`tkLString`, FPC `tkAString`). Older runtimes emit no type data
/// for any string kind, and `tkUString` / `tkWString` records never carry
/// any.
///
/// Source: FPC `rtl/objpas/typinfo.pp` (`tkAString:
/// (CodePage: Word)`), and the records of the Delphi 7 and XE5 samples.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StringLayout {
    /// String records end at their name.
    Plain,
    /// ANSI string records carry a code page after their name.
    CodePaged,
}

/// `tkLString` / `tkUString` / `tkWString` - string RTTI record.
#[derive(Debug, Clone, Copy)]
pub struct StringInfo<'a> {
    /// Type header.
    pub header: TypeHeader<'a>,
    /// Code page of an ANSI string on a [`StringLayout::CodePaged`] runtime:
    /// `0` is the default ANSI code page, `65001` is UTF-8 and `65535` marks
    /// `RawByteString`. `None` for wide strings and for runtimes that predate
    /// code pages.
    pub code_page: Option<u16>,
    /// The virtual address just past the bytes this decode read: the end of
    /// what [`TypeDetail::extent`] states. Kind-specific data the decoder
    /// does not read lies past it.
    end: u64,
}

impl<'a> MethodInfo<'a> {
    /// Decodes a `tkMethod` record.
    pub fn from_va(ctx: &BinaryContext<'a>, type_info_va: u64, flavor: VmtFlavor) -> Option<Self> {
        let header = TypeHeader::from_va(ctx, type_info_va, flavor)?;
        if header.kind != TypeKind::Method {
            return None;
        }
        let data = ctx.data();
        let mut cursor = ctx
            .va_to_file(type_info_va)?
            .checked_add(2)?
            .checked_add(header.name.len())?;
        let kind = MethodKind::from_u8(*data.get(cursor)?);
        cursor = cursor.checked_add(1)?;
        let num_params = *data.get(cursor)? as usize;
        cursor = cursor.checked_add(1)?;
        if num_params > MAX_METHOD_PARAMS {
            return None;
        }
        // Per-parameter `Flags` is `set of TParamFlag`. FPC's `TParamFlag`
        // has 12 elements → a 2-byte set; Delphi's has 7 → a 1-byte set.
        // Reading the wrong width shifts every subsequent field and yields
        // garbage names, so the width is flavor-dependent.
        // Source: FPC `rtl/objpas/typinfo.pp:94-96, 981-997`.
        let flag_width = match flavor {
            VmtFlavor::Fpc => 2usize,
            VmtFlavor::Delphi => 1,
        };
        let mut params = Vec::with_capacity(num_params);
        for _ in 0..num_params {
            let flags = *data.get(cursor)?;
            cursor = cursor.checked_add(flag_width)?;
            let name = read_short_string_at_file(data, cursor)?;
            cursor = cursor.checked_add(1)?.checked_add(name.len())?;
            let type_name = read_short_string_at_file(data, cursor)?;
            cursor = cursor.checked_add(1)?.checked_add(type_name.len())?;
            params.push(MethodParam {
                flags,
                name,
                type_name,
            });
        }
        let is_function = matches!(
            kind,
            MethodKind::Function | MethodKind::ClassFunction | MethodKind::SafeFunction
        );
        let result_type = if is_function {
            let rt = read_short_string_at_file(data, cursor)?;
            cursor = cursor.checked_add(1)?.checked_add(rt.len())?;
            Some(rt)
        } else {
            None
        };

        // Modern-RTTI trailer (Delphi 2010+): `[ResultTypeRef: ptr (if
        // function)]`, `CC: u8`, `ParamTypeRefs[num_params]: ptr`,
        // `MethSig: ptr`. Source: IDR64 `IDCGen.cpp::OutputRTTIMethod`.
        // Strictly validated so legacy binaries (which stop after the
        // name-based params) don't yield garbage references.
        let ptr_size = ctx.pointer_size().unwrap_or(0);
        let (param_type_refs, result_type_ref) = parse_method_modern_refs(
            ctx,
            cursor,
            &params,
            result_type,
            is_function,
            ptr_size,
            flavor,
        )
        .unwrap_or_default();
        // The trailer, where one was read: the result's reference, the
        // calling convention, each parameter's reference and the signature's.
        let end_off = if param_type_refs.is_empty() && result_type_ref == 0 {
            cursor
        } else {
            let references = params
                .len()
                .checked_add(1)?
                .checked_add(usize::from(is_function))?;
            cursor
                .checked_add(1)?
                .checked_add(references.checked_mul(ptr_size)?)?
        };

        Some(Self {
            header,
            kind,
            params,
            result_type,
            param_type_refs,
            result_type_ref,
            end: va_at(type_info_va, ctx.va_to_file(type_info_va)?, end_off)?,
        })
    }
}

/// Parses the modern `tkMethod` reference trailer at `cursor` (after the
/// name-based parameter list and result-type string). Returns
/// `(param_type_refs, result_type_ref)` or `None` if the layout isn't
/// present / doesn't validate.
fn parse_method_modern_refs<'a>(
    ctx: &BinaryContext<'a>,
    cursor: usize,
    params: &[MethodParam<'a>],
    result_type: Option<&'a [u8]>,
    is_function: bool,
    ptr_size: usize,
    flavor: VmtFlavor,
) -> Option<(Vec<u64>, u64)> {
    if ptr_size == 0 {
        return None;
    }
    // The trailer carries no information when there are no parameters and no
    // result type, and there's nothing to cross-check it against - so skip it
    // rather than risk misreading legacy bytes.
    if params.is_empty() && !is_function {
        return None;
    }
    // Cross-check: each modern `PPTypeInfo` ref must resolve to a type whose
    // name equals the already-decoded name-based type. This conclusively
    // distinguishes the real Delphi-2010+ trailer from legacy bytes that
    // merely *look* like one. A non-empty name that matches is "confirmed";
    // an empty name can't be checked. We require at least one confirmation so
    // the trailer is never accepted on unverifiable (empty-name) refs alone.
    let mut confirmed = 0usize;
    let mut check = |r: u64, expected: &[u8]| -> bool {
        if expected.is_empty() {
            return true; // unverifiable - must be carried by another match
        }
        if TypeHeader::from_pptr(ctx, r, ptr_size, flavor)
            .map(|h| h.name_bytes() == expected)
            .unwrap_or(false)
        {
            confirmed = confirmed.saturating_add(1);
            true
        } else {
            false
        }
    };

    let data = ctx.data();
    let mut off = cursor;
    let result_type_ref = if is_function {
        let r = read_ptr(data, off, ptr_size)?;
        off = off.checked_add(ptr_size)?;
        if !check(r, result_type.unwrap_or(b"")) {
            return None;
        }
        r
    } else {
        0
    };
    let cc = *data.get(off)?;
    if cc > 11 {
        return None;
    }
    off = off.checked_add(1)?;
    let mut refs = Vec::with_capacity(params.len());
    for p in params {
        let r = read_ptr(data, off, ptr_size)?;
        off = off.checked_add(ptr_size)?;
        if !check(r, p.type_name) {
            return None;
        }
        refs.push(r);
    }
    if confirmed == 0 {
        return None; // nothing could be verified - don't trust it
    }
    Some((refs, result_type_ref))
}

impl<'a> ProcedureInfo<'a> {
    /// Decodes a `tkProcedure` record, including its inline
    /// `TProcedureSignature` (`Flags: u8`, `CC: u8`, `ResultTypeRef: ptr`,
    /// `ParamCount: u8`, then `{ParamFlags: u8, ParamTypeRef: ptr, Name:
    /// ShortString}` per parameter).
    ///
    /// Source: FPC `rtl/objpas/typinfo.pp:357-372`
    /// (`TProcedureSignature` / `TProcedureParam`). The signature is strictly
    /// validated - a `0xFF` flags byte, an implausible parameter count, or a
    /// parameter type pointer that doesn't resolve yields a header-only
    /// result rather than garbage (older binaries emit no signature).
    pub fn from_va(ctx: &BinaryContext<'a>, type_info_va: u64, flavor: VmtFlavor) -> Option<Self> {
        let header = TypeHeader::from_va(ctx, type_info_va, flavor)?;
        if header.kind != TypeKind::Procedure {
            return None;
        }
        // The TypeData is one pointer, to the signature, which lies apart.
        let end = header.extent().map_or(header.va, |extent| extent.end);
        let bare = Self {
            header,
            call_conv: 0xFF,
            params: Vec::new(),
            result_type_ref: 0,
            result_type: None,
            end,
        };
        let Some(ptr_size) = ctx.pointer_size() else {
            return Some(bare);
        };
        let Some(td) = ctx
            .va_to_file(type_info_va)
            .and_then(|o| o.checked_add(2))
            .and_then(|o| o.checked_add(header.name.len()))
        else {
            return Some(bare);
        };
        let bare = Self {
            end: end.saturating_add(u64::try_from(ptr_size).unwrap_or_default()),
            ..bare
        };
        // Delphi's `tkProcedure` TypeData is a pointer to the
        // `TProcedureSignature` (FPC embeds it inline; we read the Delphi
        // pointer form). Source: pythia `pythia/core/structures.py:174-177`.
        let sig = ctx.data().get(td..).and_then(|_| {
            let sig_va = read_ptr(ctx.data(), td, ptr_size)?;
            let sig_off = ctx.va_to_file(sig_va)?;
            decode_procedure_signature(ctx, sig_off, ptr_size, flavor, header)
        });
        Some(sig.map_or(bare.clone(), |sig| ProcedureInfo {
            end: bare.end,
            ..sig
        }))
    }
}

/// Decodes an inline `TProcedureSignature` at `td` (file offset), returning a
/// fully-populated [`ProcedureInfo`] or `None` if it fails validation.
fn decode_procedure_signature<'a>(
    ctx: &BinaryContext<'a>,
    td: usize,
    ptr_size: usize,
    flavor: VmtFlavor,
    header: TypeHeader<'a>,
) -> Option<ProcedureInfo<'a>> {
    let (sig, _) = parse_procedure_signature(ctx, td, ptr_size, flavor)?;
    let sig = sig?;
    Some(ProcedureInfo {
        header,
        call_conv: sig.call_conv,
        params: sig.params,
        result_type_ref: sig.result_type_ref,
        result_type: sig.result_type,
        end: header.va,
    })
}

/// A decoded `TProcedureSignature`: what a `tkProcedure` type and a record
/// method carry.
#[derive(Debug, Clone)]
pub struct ProcedureSignature<'a> {
    /// Calling-convention byte (`TCallConv`).
    pub call_conv: u8,
    /// Result-type `PPTypeInfo` VA (`0` for a procedure).
    pub result_type_ref: u64,
    /// Resolved result-type header, if present and resolvable.
    pub result_type: Option<TypeHeader<'a>>,
    /// Ordered parameters.
    pub params: Vec<SignatureParam<'a>>,
}

/// Parses a `TProcedureSignature` at file offset `off`: `Flags: u8` (`0xFF`
/// = no signature, nothing follows), `CC: u8`, `ResultType: ptr`,
/// `ParamCount: u8`, then per parameter `{Flags: u8, ParamType: ptr, Name:
/// ShortString}` and, in Delphi, the parameter's `TAttrData`.
///
/// Returns the signature (`None` for the explicit "no signature" form) and
/// the offset just past it; `None` overall when it fails validation.
///
/// Source: Delphi 13 `System.TypInfo` (`TProcedureSignature`,
/// `TProcedureParam`) and `System.Rtti` (`TProcSig.Create`), FPC
/// `typinfo.pp:357-372` (no attribute data).
fn parse_procedure_signature<'a>(
    ctx: &BinaryContext<'a>,
    off: usize,
    ptr_size: usize,
    flavor: VmtFlavor,
) -> Option<(Option<ProcedureSignature<'a>>, usize)> {
    let data = ctx.data();
    let flags = *data.get(off)?;
    if flags == 0xFF {
        return Some((None, off.checked_add(1)?));
    }
    let call_conv = *data.get(off.checked_add(1)?)?;
    if call_conv > 11 {
        return None; // not a valid TCallConv → not this layout
    }
    let result_type_ref = read_ptr(data, off.checked_add(2)?, ptr_size)?;
    let result_type = TypeHeader::from_pptr(ctx, result_type_ref, ptr_size, flavor);
    if result_type_ref != 0 && result_type.is_none() {
        return None;
    }
    let count_off = off.checked_add(2)?.checked_add(ptr_size)?;
    let param_count = *data.get(count_off)? as usize;
    if param_count > MAX_METHOD_PARAMS {
        return None;
    }
    let mut cursor = count_off.checked_add(1)?;
    let mut params = Vec::with_capacity(param_count);
    for _ in 0..param_count {
        let pflags = *data.get(cursor)?;
        let type_ref = read_ptr(data, cursor.checked_add(1)?, ptr_size)?;
        let param_type = TypeHeader::from_pptr(ctx, type_ref, ptr_size, flavor);
        if type_ref != 0 && param_type.is_none() {
            return None; // garbage → not a real signature
        }
        let name_off = cursor.checked_add(1)?.checked_add(ptr_size)?;
        let name = read_short_string_at_file(data, name_off)?;
        if !name.is_empty() && !is_plausible_identifier(name) {
            return None;
        }
        cursor = name_off.checked_add(1)?.checked_add(name.len())?;
        let mut attributes = Vec::new();
        if flavor == VmtFlavor::Delphi {
            let (attrs, next) = read_attr_data(data, cursor, ptr_size)?;
            attributes = attrs;
            cursor = next;
        }
        params.push(SignatureParam {
            flags: pflags,
            type_ref,
            param_type,
            name,
            attributes,
        });
    }
    Some((
        Some(ProcedureSignature {
            call_conv,
            result_type_ref,
            result_type,
            params,
        }),
        cursor,
    ))
}

impl<'a> StringInfo<'a> {
    /// Decodes a `tkLString` / `tkUString` / `tkWString` record, reading the
    /// code page where `layout` says the record carries one.
    pub fn from_va(
        ctx: &BinaryContext<'a>,
        type_info_va: u64,
        flavor: VmtFlavor,
        layout: StringLayout,
    ) -> Option<Self> {
        let header = TypeHeader::from_va(ctx, type_info_va, flavor)?;
        if !matches!(
            header.kind,
            TypeKind::LString | TypeKind::UString | TypeKind::WString
        ) {
            return None;
        }
        let start = ctx.va_to_file(type_info_va)?;
        let name_end = start.checked_add(2)?.checked_add(header.name.len())?;
        // The code page is the whole type data of a code-paged ANSI string
        // and sits right after the name. A record that walks off EOF is
        // truncated: surface `None` rather than misreport code page 0, which
        // is valid metadata ("the default ANSI code page").
        let (code_page, end) = match (layout, header.kind) {
            (StringLayout::CodePaged, TypeKind::LString) => {
                let end = name_end.checked_add(2)?;
                let bytes = ctx.data().get(name_end..end)?;
                (Some(u16::from_le_bytes(bytes.try_into().ok()?)), end)
            }
            _ => (None, name_end),
        };
        Some(Self {
            header,
            code_page,
            end: va_at(type_info_va, start, end)?,
        })
    }
}

impl<'a> RecordInfo<'a> {
    /// Decodes a `tkRecord` record including its managed-field entries.
    ///
    /// Accepts records with an empty on-disk name - the compiler
    /// emits `vmtInitTable` as a synthetic `tkRecord` with no name
    /// whose sole purpose is to enumerate the host class's managed
    /// fields. [`TypeHeader::from_va`] rejects empty names, so we
    /// read the header directly here.
    pub fn from_va(ctx: &BinaryContext<'a>, type_info_va: u64, flavor: VmtFlavor) -> Option<Self> {
        let data = ctx.data();
        let file_off = ctx.va_to_file(type_info_va)?;
        let kind_byte = *data.get(file_off)?;
        // Delphi tkRecord (14) or tkMRecord (22, a managed record, same
        // layout); FPC tkRecord (13).
        let kind = match (flavor, kind_byte) {
            (VmtFlavor::Delphi, 14) | (VmtFlavor::Fpc, 13) => TypeKind::Record,
            (VmtFlavor::Delphi, 22) => TypeKind::MRecord,
            _ => return None,
        };
        let name_off = file_off.checked_add(1)?;
        let name = read_short_string_at_file(data, name_off)?;
        let header = TypeHeader {
            va: type_info_va,
            kind_byte,
            kind,
            name,
        };
        let ptr_size = ctx.pointer_size()?;
        let mut off = file_off.checked_add(2)?.checked_add(name.len())?;
        let record_size_end = off.checked_add(4)?;
        let record_size =
            i32::from_le_bytes(data.get(off..record_size_end)?.try_into().ok()?) as u32;
        off = record_size_end;
        let num_managed_end = off.checked_add(4)?;
        let num_managed =
            i32::from_le_bytes(data.get(off..num_managed_end)?.try_into().ok()?) as usize;
        off = num_managed_end;
        if num_managed > MAX_RECORD_MANAGED_FIELDS {
            return None;
        }
        let mut managed_fields = Vec::with_capacity(num_managed);
        for _ in 0..num_managed {
            let type_ref = read_ptr(data, off, ptr_size)?;
            off = off.checked_add(ptr_size)?;
            let offset = read_ptr(data, off, ptr_size)?;
            off = off.checked_add(ptr_size)?;
            let field_type = TypeHeader::from_pptr(ctx, type_ref, ptr_size, flavor);
            managed_fields.push(RecordManagedField {
                type_ref,
                offset,
                field_type,
            });
        }
        // Full (extended) field table - Delphi 2010+ lists every field after
        // the managed-field / operator section. Strictly validated so that
        // pre-2010 Delphi, FPC, and the synthetic (empty-name) init-table
        // record never produce garbage: any implausible field discards the
        // whole list (returns empty rather than partial).
        let tail = if flavor == VmtFlavor::Delphi && !name.is_empty() {
            parse_record_tail(ctx, off, ptr_size, flavor)
        } else {
            None
        };
        let (fields, operators, attributes, methods, end) = match tail {
            Some(t) => (t.fields, t.operators, t.attributes, t.methods, t.end),
            None => (Vec::new(), None, Vec::new(), Vec::new(), off),
        };
        Some(Self {
            header,
            record_size,
            managed_fields,
            fields,
            operators,
            attributes,
            methods,
            end: va_at(type_info_va, file_off, end)?,
        })
    }
}

/// The Delphi 2010+ part of a record's type data after its managed fields.
struct RecordTail<'a> {
    operators: Option<RecordOperators>,
    fields: Vec<RecordField<'a>>,
    attributes: Vec<AttributeEntry<'a>>,
    methods: Vec<RecordMethod<'a>>,
    /// The file offset just past the tail.
    end: usize,
}

/// Parses the Delphi 2010+ record tail that follows the managed fields:
/// `NumOps: u8`, `RecOps[NumOps]: ptr` (a [`RecordOperators`] table),
/// `RecFldCnt: i32`, the fields
/// (`TypeRef: ptr; FldOffset: NativeInt; Flags: u8; Name: ShortString;
/// AttrData`), the record's `AttrData`, `RecMethCnt: u16` and the methods
/// (`Flags: u8; Code: ptr; Name: ShortString; Sig: TProcedureSignature;
/// AttrData`).
///
/// Source: Delphi 13 `System.TypInfo` (`tkRecord` variant of `TTypeData`,
/// `TRecordTypeField`, `TRecordTypeMethod`) and `System.Rtti`
/// (`TRttiRecordType.GetDeclaredFields` / `GetDeclaredMethods`). Returns
/// `None` if anything looks implausible, since the host record may predate
/// this layout.
fn parse_record_tail<'a>(
    ctx: &BinaryContext<'a>,
    start_off: usize,
    ptr_size: usize,
    flavor: VmtFlavor,
) -> Option<RecordTail<'a>> {
    let data = ctx.data();
    let num_ops = *data.get(start_off)? as usize;
    if num_ops > 4 {
        return None; // not the extended layout
    }
    let mut off = start_off.checked_add(1)?;
    let mut slots = [0u64; 4];
    for slot in slots.iter_mut().take(num_ops) {
        *slot = read_ptr(data, off, ptr_size)?;
        off = off.checked_add(ptr_size)?;
    }
    let [flags, initialize, finalize, assign] = slots;
    let code = |va: u64| -> Option<Option<u64>> {
        match va {
            0 => Some(None),
            va if ctx.is_code_va(va) => Some(Some(va)),
            _ => None,
        }
    };
    let operators = if num_ops == 0 {
        None
    } else {
        Some(RecordOperators {
            flags,
            initialize: code(initialize)?,
            finalize: code(finalize)?,
            assign: code(assign)?,
        })
    };
    let count = read_u32(data, off)? as usize;
    off = off.checked_add(4)?;
    if count > MAX_RECORD_FIELDS {
        return None;
    }
    let mut fields = Vec::with_capacity(count);
    for _ in 0..count {
        let type_ref = read_ptr(data, off, ptr_size)?;
        // `FldOffset` is a NativeInt (`TManagedField`), so on 64-bit the
        // flags and name follow at `2 * ptr_size`.
        let offset = read_ptr(data, off.checked_add(ptr_size)?, ptr_size)? as u32;
        let flags_off = off.checked_add(ptr_size)?.checked_add(ptr_size)?;
        let flags = *data.get(flags_off)?;
        let name_off = flags_off.checked_add(1)?;
        let fname = read_short_string_at_file(data, name_off)?;
        // Structural gate: a real field has a plausible (or empty) name and a
        // type pointer that is either null or resolves to a valid record.
        // Random bytes past a non-extended record fail this immediately.
        if !fname.is_empty() && !is_plausible_identifier(fname) {
            return None;
        }
        let field_type = TypeHeader::from_pptr(ctx, type_ref, ptr_size, flavor);
        if type_ref != 0 && field_type.is_none() {
            return None;
        }
        let attr_off = name_off.checked_add(1)?.checked_add(fname.len())?;
        let (attributes, next) = read_attr_data(data, attr_off, ptr_size)?;
        off = next;
        fields.push(RecordField {
            type_ref,
            field_type,
            offset,
            flags,
            name: fname,
            attributes,
        });
    }
    let (attributes, next) = read_attr_data(data, off, ptr_size)?;
    off = next;
    let method_count = usize::from(read_u16(data, off)?);
    off = off.checked_add(2)?;
    if method_count > MAX_METHODS_PER_RECORD {
        return None;
    }
    let mut methods = Vec::with_capacity(method_count);
    for _ in 0..method_count {
        let flags = *data.get(off)?;
        let code_va = read_ptr(data, off.checked_add(1)?, ptr_size)?;
        if !ctx.is_code_va(code_va) {
            return None;
        }
        let name_off = off.checked_add(1)?.checked_add(ptr_size)?;
        let name = read_short_string_at_file(data, name_off)?;
        if !is_plausible_identifier(name) {
            return None;
        }
        let sig_off = name_off.checked_add(1)?.checked_add(name.len())?;
        let (signature, after_sig) = parse_procedure_signature(ctx, sig_off, ptr_size, flavor)?;
        let (method_attributes, next) = read_attr_data(data, after_sig, ptr_size)?;
        off = next;
        methods.push(RecordMethod {
            flags,
            code_va,
            name,
            signature,
            attributes: method_attributes,
        });
    }
    Some(RecordTail {
        operators,
        fields,
        attributes,
        methods,
        end: off,
    })
}

/// Sweeps the binary's read-only data for every FPC `tkInterface` record
/// and indexes them by GUID. Used by
/// [`crate::DelphiBinary::interface_methods`] to recover per-method
/// names: the per-class `InterfaceEntry` carries the GUID but no
/// pointer to the tkInterface PTypeInfo, so we have to find it by
/// scanning.
///
/// On Delphi binaries this returns an empty map - Delphi's classic
/// `tkInterface` doesn't carry a method table. (Modern extended RTTI
/// does, on a different layout, not yet supported.)
///
/// False-positive guard: each candidate is fully decoded and the GUID
/// must be non-zero before it's accepted.
pub fn scan_fpc_tkinterface_index(ctx: &BinaryContext<'_>) -> HashMap<Guid, u64> {
    let mut idx = HashMap::new();
    let Some(_) = ctx.pointer_size() else {
        return idx;
    };
    // FPC `tkInterface` kind byte is 14 (per `fpc_kind_from_byte`).
    const FPC_TK_INTERFACE: u8 = 14;
    for range in ctx.scan_ranges() {
        let Some(slice) = ctx.section_data(range) else {
            continue;
        };
        let mut i = 0usize;
        while i < slice.len() {
            if slice.get(i).copied() == Some(FPC_TK_INTERFACE)
                && let Some(va) = range.va.checked_add(i as u64)
                && let Some(info) = InterfaceTypeInfo::from_va(ctx, va, VmtFlavor::Fpc)
                && (info.guid.data1 != 0
                    || info.guid.data2 != 0
                    || info.guid.data3 != 0
                    || info.guid.data4 != [0; 8])
            {
                idx.entry(info.guid).or_insert(va);
            }
            let Some(next) = i.checked_add(1) else {
                break;
            };
            i = next;
        }
    }
    idx
}

/// Header of an FPC `TIntfMethodTable`. The body is a sequence of
/// [`IntfMethodEntry`] records reachable via [`Self::entries`].
///
/// Source: FPC `rtl/objpas/typinfo.pp:429-443`.
#[derive(Debug, Clone)]
pub struct IntfMethodTable<'a> {
    method_count: u16,
    rtti_count: u16,
    entries: Vec<IntfMethodEntry<'a>>,
}

impl<'a> IntfMethodTable<'a> {
    /// Decodes the method table referenced by an FPC `tkInterface`
    /// `PTypeInfo` record at `tkintf_va`.
    ///
    /// Returns `None` when the layout doesn't parse cleanly. The
    /// header (`method_count`, `rtti_count`) is recovered even when
    /// per-method names are absent - `rtti_count == 0xFFFF` means
    /// the compiler emitted no per-method records, so
    /// [`Self::entries`] will be empty in that case.
    pub fn from_tkinterface(ctx: &BinaryContext<'a>, tkintf_va: u64) -> Option<Self> {
        let header = TypeHeader::from_va(ctx, tkintf_va, VmtFlavor::Fpc)?;
        if header.kind != TypeKind::Interface {
            return None;
        }
        let ptr_size = ctx.pointer_size()?;
        let data = ctx.data();

        // FPC's `aligntoptr` is a no-op on architectures without
        // `FPC_REQUIRES_PROPER_ALIGNMENT` (i.e. x86 / x86_64 in any
        // container). It only inserts padding on ARM / AArch64 etc.
        let needs_alignment = ctx.target_arch().fpc_requires_proper_alignment();

        // Walk past kind(1), namelen(1), name, parent_ref(ptr),
        // flags(1), guid(16), unit_name(shortstring) → reach
        // PropertyTable start.
        let file_off = ctx.va_to_file(tkintf_va)?;
        let after_name = file_off.checked_add(2)?.checked_add(header.name.len())?;
        let after_parent = after_name.checked_add(ptr_size)?;
        let after_flags = after_parent.checked_add(1)?;
        let after_guid = after_flags.checked_add(16)?;
        let unit_name_len = *data.get(after_guid)? as usize;
        let after_unit_name = after_guid.checked_add(1)?.checked_add(unit_name_len)?;

        // PropertyTable starts here.
        let prop_table_off = align_to_ptr(after_unit_name, ptr_size, needs_alignment)?;

        // TPropData: u16 count, then variable-length entries. We bail
        // on non-empty published-property tables - interfaces with
        // published properties are vanishingly rare, and walking past
        // them needs a flavor-specific TPropInfo decoder we don't
        // duplicate here.
        let prop_count = read_u16(data, prop_table_off)? as usize;
        if prop_count != 0 {
            return None;
        }
        let after_prop_data = prop_table_off.checked_add(2)?;

        // MethodTable starts at aligntoptr(PropertyTable.Tail).
        let method_table_off = align_to_ptr(after_prop_data, ptr_size, needs_alignment)?;

        // TIntfMethodTable: u16 count, u16 rtti_count.
        let method_count = read_u16(data, method_table_off)?;
        let rtti_count = read_u16(data, method_table_off.checked_add(2)?)?;

        if method_count as usize > MAX_INTF_METHODS {
            return None;
        }

        // `rtti_count == 0xFFFF` is the documented "no per-method RTTI"
        // sentinel - the compiler knows the slot count but emitted no
        // metadata for any individual method.
        if rtti_count == u16::MAX || rtti_count == 0 {
            return Some(Self {
                method_count,
                rtti_count,
                entries: Vec::new(),
            });
        }

        let entries_start = method_table_off.checked_add(4)?;
        let mut entry_off = align_to_ptr(entries_start, ptr_size, needs_alignment)?;

        let walk_count = rtti_count.min(method_count) as usize;
        let mut entries = Vec::with_capacity(walk_count);
        for slot in 0..walk_count {
            // TIntfMethodEntry layout (no HAVE_INVOKEHELPER on the
            // platforms we target):
            //   ResultType: PPTypeInfo  (ptr)
            //   CC: u8
            //   Kind: u8
            //   ParamCount: u16
            //   StackSize: SizeInt      (ptr-sized)
            //   NamePtr: PShortString   (ptr)
            let cc_off = entry_off.checked_add(ptr_size)?;
            let kind_off = cc_off.checked_add(1)?;
            let param_count_off = kind_off.checked_add(1)?;
            let stack_size_off = param_count_off.checked_add(2)?;
            let name_ptr_off = stack_size_off.checked_add(ptr_size)?;

            let result_type_ref = read_ptr(data, entry_off, ptr_size)?;
            let calling_convention = *data.get(cc_off)?;
            let kind_byte = *data.get(kind_off)?;
            let param_count = read_u16(data, param_count_off)?;
            let stack_size = read_ptr(data, stack_size_off, ptr_size)?;
            let name_ptr = read_ptr(data, name_ptr_off, ptr_size)?;
            let name = read_short_string_at_va(ctx, name_ptr)?;

            entries.push(IntfMethodEntry {
                slot: slot as u16,
                result_type_ref,
                calling_convention,
                kind_byte,
                kind: MethodKind::from_u8(kind_byte),
                param_count,
                stack_size,
                name,
            });

            // Bail on parametered methods (the inline
            // `TVmtMethodParam` walker would duplicate the method-RTTI
            // decoder); the partial list up to the first parametered
            // method is still useful for naming purposes.
            if param_count != 0 {
                break;
            }
            let after_entry_base = name_ptr_off.checked_add(ptr_size)?;
            // ResultLocs is present when ResultType is non-null.
            let after_entry = if result_type_ref != 0 {
                after_entry_base.checked_add(ptr_size)?
            } else {
                after_entry_base
            };
            entry_off = align_to_ptr(after_entry, ptr_size, needs_alignment)?;
        }

        Some(Self {
            method_count,
            rtti_count,
            entries,
        })
    }

    /// Total number of slots in the interface's vtable (matches the
    /// Delphi `Methods` count).
    #[inline]
    pub fn method_count(&self) -> u16 {
        self.method_count
    }

    /// Number of entries that carry per-method RTTI metadata. Equal
    /// to [`Self::method_count`] when full RTTI was emitted; `0xFFFF`
    /// when the compiler emitted no per-method records (interfaces
    /// declared without `{$M+}` mode in their source unit). When the
    /// sentinel is set, [`Self::entries`] returns an empty slice -
    /// the vtable still has `method_count` slots, but their names
    /// are not recoverable from this RTTI.
    #[inline]
    pub fn rtti_count(&self) -> u16 {
        self.rtti_count
    }

    /// Per-method RTTI records, one per slot. Empty when
    /// `rtti_count == 0xFFFF`.
    #[inline]
    pub fn entries(&self) -> &[IntfMethodEntry<'a>] {
        &self.entries
    }
}

/// One entry in an FPC `tkInterface` method table.
///
/// Source: FPC `rtl/objpas/typinfo.pp:398-427`
/// (`TIntfMethodEntry`). We don't decode parameters here - the entry
/// is variable-length, and we bail past entries that declare any
/// parameter so the simple shape stays clean.
#[derive(Debug, Clone, Copy)]
pub struct IntfMethodEntry<'a> {
    slot: u16,
    result_type_ref: u64,
    calling_convention: u8,
    kind_byte: u8,
    kind: MethodKind,
    param_count: u16,
    stack_size: u64,
    name: &'a [u8],
}

impl<'a> IntfMethodEntry<'a> {
    /// Slot index within the interface's vtable.
    #[inline]
    pub fn slot(&self) -> u16 {
        self.slot
    }

    /// Method name as `&str`: UTF-8, as Delphi 2009 and later store names, or `"<non-ascii>"`
    /// when the bytes are not valid UTF-8.
    #[inline]
    pub fn name(&self) -> &'a str {
        str::from_utf8(self.name).unwrap_or("<non-ascii>")
    }

    /// Raw method-name bytes (the on-disk shortstring body).
    #[inline]
    pub fn name_bytes(&self) -> &'a [u8] {
        self.name
    }

    /// `PPTypeInfo` VA of the result type, or `0` for procedures.
    #[inline]
    pub fn result_type_ref(&self) -> u64 {
        self.result_type_ref
    }

    /// Calling-convention byte (`TCallConv`).
    #[inline]
    pub fn calling_convention(&self) -> u8 {
        self.calling_convention
    }

    /// Raw method-kind byte (`TMethodKind`).
    #[inline]
    pub fn kind_byte(&self) -> u8 {
        self.kind_byte
    }

    /// Method kind mapped through [`MethodKind`].
    #[inline]
    pub fn kind(&self) -> MethodKind {
        self.kind
    }

    /// Number of formal parameters declared in source. When non-zero
    /// the parameter records aren't decoded; future iterations may
    /// expose them.
    #[inline]
    pub fn param_count(&self) -> u16 {
        self.param_count
    }

    /// Stack-bytes consumed by the call (FPC
    /// `TIntfMethodEntry.StackSize`).
    #[inline]
    pub fn stack_size(&self) -> u64 {
        self.stack_size
    }
}

/// Pointer-align `off` to the next multiple of `ptr_size`. When
/// `apply_alignment` is `false` (Windows PE, where FPC packs records),
/// returns `off` unchanged.
fn align_to_ptr(off: usize, ptr_size: usize, apply_alignment: bool) -> Option<usize> {
    if !apply_alignment || ptr_size == 0 {
        return Some(off);
    }
    let rem = off.checked_rem(ptr_size)?;
    if rem == 0 {
        Some(off)
    } else {
        let pad = ptr_size.checked_sub(rem)?;
        off.checked_add(pad)
    }
}

/// Hard cap on FPC interface method-table entries - guards against
/// adversarially-large `Count` fields.
const MAX_INTF_METHODS: usize = 1024;

#[cfg(test)]
mod tests {
    use super::*;

    /// Locks every documented Delphi TTypeKind byte against
    /// pythia `pythia/core/structures.py:11-34`.
    /// If a future refactor renumbers or drops a variant, this test
    /// flags it before silent misdecoding ships.
    #[test]
    fn delphi_tkkind_bytes_match_pythia() {
        let cases: &[(u8, TypeKind)] = &[
            (0, TypeKind::Unknown),
            (1, TypeKind::Integer),
            (2, TypeKind::Char),
            (3, TypeKind::Enumeration),
            (4, TypeKind::Float),
            (5, TypeKind::String),
            (6, TypeKind::Set),
            (7, TypeKind::Class),
            (8, TypeKind::Method),
            (9, TypeKind::WChar),
            (10, TypeKind::LString),
            (11, TypeKind::WString),
            (12, TypeKind::Variant),
            (13, TypeKind::Array),
            (14, TypeKind::Record),
            (15, TypeKind::Interface),
            (16, TypeKind::Int64),
            (17, TypeKind::DynArray),
            (18, TypeKind::UString),
            (19, TypeKind::ClassRef),
            (20, TypeKind::Pointer),
            (21, TypeKind::Procedure),
            (22, TypeKind::MRecord), // Delphi 10.4+; not in pythia table
        ];
        for &(byte, kind) in cases {
            assert_eq!(TypeKind::from_u8(byte), kind, "byte {byte}");
            assert_eq!(
                classify_kind_byte(byte, VmtFlavor::Delphi),
                kind,
                "byte {byte} via classify_kind_byte"
            );
        }
    }

    /// Locks the FPC TTypeKind ordering from
    /// FPC `rtl/inc/rttih.inc:29-34`. FPC reorders the
    /// enum vs Delphi (e.g. tkClass is 15 in FPC vs 7 in Delphi); the
    /// table below is the authoritative byte → kind translation we use
    /// when classifying FPC RTTI records.
    #[test]
    fn fpc_tkkind_bytes_match_rttih_inc() {
        let cases: &[(u8, TypeKind)] = &[
            (0, TypeKind::Unknown),
            (1, TypeKind::Integer),
            (2, TypeKind::Char),
            (3, TypeKind::Enumeration),
            (4, TypeKind::Float),
            (5, TypeKind::Set),
            (6, TypeKind::Method),
            (10, TypeKind::WString),
            (11, TypeKind::Variant),
            (13, TypeKind::Record),
            (14, TypeKind::Interface),
            (15, TypeKind::Class),
            (17, TypeKind::WChar),
            (19, TypeKind::Int64),
            (21, TypeKind::DynArray),
            (24, TypeKind::UString),
            (28, TypeKind::ClassRef),
            (29, TypeKind::Pointer),
        ];
        for &(byte, kind) in cases {
            assert_eq!(
                classify_kind_byte(byte, VmtFlavor::Fpc),
                kind,
                "FPC byte {byte}"
            );
        }
    }

    #[test]
    fn type_kind_decodes_all_documented_values() {
        assert_eq!(TypeKind::from_u8(0), TypeKind::Unknown);
        assert_eq!(TypeKind::from_u8(7), TypeKind::Class);
        assert_eq!(TypeKind::from_u8(15), TypeKind::Interface);
        assert_eq!(TypeKind::from_u8(17), TypeKind::DynArray);
        assert_eq!(TypeKind::from_u8(18), TypeKind::UString);
        assert_eq!(TypeKind::from_u8(22), TypeKind::MRecord);
        assert_eq!(TypeKind::from_u8(99), TypeKind::Unknown);
    }

    #[test]
    fn identifier_plausibility() {
        assert!(is_plausible_identifier(b"TComponent"));
        assert!(is_plausible_identifier(b"System.SysUtils"));
        assert!(is_plausible_identifier(b"Vcl.Forms"));
        assert!(is_plausible_identifier(b"TComparer<System.TPair>"));
        assert!(!is_plausible_identifier(b""));
        assert!(!is_plausible_identifier(b"hello\x00world"));
    }
}
