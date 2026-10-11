//! Extended RTTI blocks introduced with Delphi 2010 (`{$RTTI EXPLICIT ...}`).
//!
//! Classic RTTI (this crate's [`crate::properties`], [`crate::methods`])
//! only covers members declared `published`. Extended RTTI extends
//! coverage to `private` / `protected` / `public` declarations by
//! emitting auxiliary blocks that sit after the classic tables. Each
//! extended entry carries:
//!
//! - A `Visibility` flag - private / protected / public / published.
//! - A pointer to a `TPropInfo` / `TVmtMethodExEntry` / `TVmtFieldExEntry`
//!   record (the underlying classic record is reused; the entry here
//!   adds visibility + attribute metadata).
//! - A packed block of attribute-table bytes that follow.
//!
//! ## Property-table extended block
//!
//! Source: DelphiHelper `DelphiHelper/core/DelphiClass_TypeInfo_tkClass.py:66-88`.
//!
//! ```text
//!   PropCountEx: u16            (Delphi 2010 and later)
//!   entries[PropCountEx]:
//!     Flags:        u8          (low 2 bits = Visibility, upper bits reserved)
//!     Info:         PPropInfo   (pointer to a TPropInfo record elsewhere)
//!     ExtraLen:     u16         (byte-size of the trailing attribute payload)
//!     Extra:        [u8; ExtraLen - 2]
//! ```
//!
//! ## Attribute-table entries
//!
//! Delphi's attribute block is a **sequence of variable-size entries
//! packed back-to-back** (no leading count - the block size is
//! `ExtraLen − 2` and entries are parsed sequentially until exhausted).
//! Each entry is:
//!
//! ```text
//!   AttrType:  PPTypeInfo   (ptr-sized)
//!   AttrCtor:  CodePointer  (ptr-sized)
//!   ArgLen:    u16
//!   ArgData:   [u8; ArgLen]
//! ```
//!
//! `ArgData` packs the attribute-constructor arguments in the order they
//! appear in the attribute declaration. Primitive types use their natural
//! encoding; strings are `u16 length` + body; an enumeration is its ordinal
//! and a set its bitmap (see [`AttributeArg`]).
//!
//! Note: this differs from FPC's `TAttributeEntry`
//! (FPC `rtl/objpas/typinfo.pp:273-289`) which adds a
//! third `AttrProc` pointer. We use the Delphi layout here because it
//! matches our empirical corpus; an FPC-flavor variant can be added
//! when needed.

use std::ops::Deref;

use crate::{
    classes::{Class, ClassSet},
    detection::TargetOs,
    formats::BinaryContext,
    limits::{
        MAX_ANCESTOR_DEPTH, MAX_ATTR_DATA_BYTES, MAX_EXTENDED_PROPERTIES_PER_CLASS,
        MAX_IDENTIFIER_BYTES,
    },
    methods::DelphiExtMethod,
    properties::Property,
    rtti::{FloatType, OrdinalType, StringLayout, TkClassInfo, TypeDetail, TypeHeader},
    signatures::MethodSignature,
    util::{extended_to_f64, read_ptr, read_short_string_at_file, read_u16},
    vmt::VmtFlavor,
};

/// Member visibility class.
///
/// Matches Delphi's `TMemberVisibility` (`System.TypInfo`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Visibility {
    /// `private`.
    Private = 0,
    /// `protected`.
    Protected = 1,
    /// `public`.
    Public = 2,
    /// `published`.
    Published = 3,
    /// Out-of-range byte.
    Unknown = 0xff,
}

impl Visibility {
    /// Decode the visibility from the low two bits of an extended-RTTI
    /// flags byte (`0` private, `1` protected, `2` public, `3` published).
    pub fn from_flags(b: u8) -> Self {
        match b & 0x03 {
            0 => Visibility::Private,
            1 => Visibility::Protected,
            2 => Visibility::Public,
            3 => Visibility::Published,
            _ => Visibility::Unknown,
        }
    }
}

/// One entry in the extended-RTTI property table.
///
/// Implements [`Deref<Target = Property>`](std::ops::Deref), so
/// `ep.name()`, `ep.get`, `ep.prop_type_ref`, and every other [`Property`]
/// accessor work directly without going through `ep.info.…`.
#[derive(Debug, Clone)]
pub struct ExtendedProperty<'a> {
    /// Visibility of this property.
    pub visibility: Visibility,
    /// Raw flags byte (low 2 bits = visibility; upper bits reserved).
    pub flags: u8,
    /// Resolved property record (the same layout as `TPropInfo` from
    /// classic RTTI, but reached through an indirection). Most callers can
    /// ignore this field and access `Property` members directly via the
    /// [`Deref`] impl.
    pub info: Property<'a>,
    /// Raw attribute-table bytes trailing the header. Empty when no
    /// attributes are declared.
    pub attributes_raw: &'a [u8],
}

impl<'a> ExtendedProperty<'a> {
    /// Decodes the property's attributes (`[Name(...)]` before the
    /// declaration). `ptr_size` is the binary's pointer size.
    pub fn attributes(&self, ptr_size: usize) -> Vec<AttributeEntry<'a>> {
        AttributeEntry::decode_block(self.attributes_raw, ptr_size).0
    }
}

impl<'a> Deref for ExtendedProperty<'a> {
    type Target = Property<'a>;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.info
    }
}

/// One entry from an RTTI attribute table.
#[derive(Debug, Clone, Copy)]
pub struct AttributeEntry<'a> {
    /// VA of the attribute class's `PPTypeInfo`. This is a *data* pointer,
    /// not a code pointer.
    pub attr_type_ref: u64,
    /// Absolute VA of the attribute class's constructor - a *code*
    /// pointer. Use this as a disassembler-naming hint (e.g. label as
    /// `<AttrClass>.Create`). Subtract the image base for an RVA.
    pub attr_ctor: u64,
    /// Raw bytes of the constructor arguments as packed by the compiler.
    /// See module docs for the encoding; a common case is a `u16`-prefixed
    /// Pascal string when the attribute takes one string arg.
    pub arg_data: &'a [u8],
}

impl<'a> AttributeEntry<'a> {
    /// Interpret `arg_data` as a single `u16`-prefixed string argument,
    /// when it matches that shape. Returns `None` otherwise.
    pub fn arg_as_string(&self) -> Option<&'a [u8]> {
        let len_bytes = self.arg_data.get(..2)?;
        let len = u16::from_le_bytes(len_bytes.try_into().ok()?) as usize;
        let total = len.checked_add(2)?;
        if self.arg_data.len() != total {
            return None;
        }
        let body = self.arg_data.get(2..)?;
        if body.iter().all(|&b| (0x20..=0x7e).contains(&b)) {
            Some(body)
        } else {
            None
        }
    }
}

impl<'a> AttributeEntry<'a> {
    /// Decode every entry of an attribute block at `bytes`. Entries
    /// are variable-size; iteration stops when the cursor would
    /// overrun the provided slice. Returns the list plus the byte
    /// length consumed.
    pub fn decode_block(bytes: &'a [u8], ptr_size: usize) -> (Vec<Self>, usize) {
        decode_attribute_block(bytes, ptr_size)
    }
}

/// One decoded attribute-constructor argument.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum AttributeArg<'a> {
    /// An ordinal: integer, character, enumeration or `Boolean` value.
    Ordinal(i64),
    /// A floating-point value (`Comp` and `Currency` as their scaled
    /// `Int64` read as a float: `Currency` already divided by 10 000).
    Float(f64),
    /// A long string (`string`, `AnsiString`, `WideString`): stored as
    /// UTF-8 with a `u16` length.
    String(String),
    /// A `ShortString`: its bytes, not re-encoded (stored with a `u16`
    /// length).
    ShortString(&'a [u8]),
    /// A set: the ordinals of its members, ascending.
    Set(Vec<i64>),
    /// A pointer, class reference or class (`TypeInfo(...)`, `TFoo`): the
    /// stored pointer and, when it is a `PPTypeInfo` (as for class
    /// references and type-info arguments), the type's name.
    Reference {
        /// The stored pointer.
        value: u64,
        /// The name of the type it refers to, when it resolves to one.
        type_name: Option<&'a str>,
    },
    /// Bytes of an argument whose type this decoder does not size, and
    /// everything after it.
    Raw(&'a [u8]),
}

/// An attribute with its type and constructor resolved and its arguments
/// decoded (as `System.Rtti.ConstructAttributes` reads them).
#[derive(Debug, Clone)]
pub struct DecodedAttribute<'a> {
    /// The attribute class's name (`TagAttribute`), when its type info
    /// resolves.
    pub type_name: Option<&'a str>,
    /// The constructor's code address.
    pub constructor: u64,
    /// The constructor's name (`Create`), when the attribute class (or an
    /// ancestor) lists it in its extended RTTI.
    pub constructor_name: Option<&'a str>,
    /// The arguments in declaration order. Bytes that cannot be decoded -
    /// all of them when the constructor or its signature is unknown, the
    /// rest from an argument of a type this decoder does not size, or any
    /// left over after the last parameter - end the list as one
    /// [`AttributeArg::Raw`].
    pub args: Vec<AttributeArg<'a>>,
}

impl<'a> DecodedAttribute<'a> {
    /// Resolves `entry`: the attribute type from its `PPTypeInfo`, the
    /// constructor among the extended methods of the attribute class and its
    /// ancestors (by code address), then each argument by the constructor's
    /// parameter types - pointer-sized for pointers, class references and
    /// classes, a `u16`-length UTF-8 long string or `ShortString`, a set by
    /// its size, otherwise the type's size (Delphi 13 `System.Rtti`,
    /// `ParseArgs`; the `ShortString` length is what the compiler writes).
    ///
    /// The compiler writes no signature for a constructor with a parameter
    /// type that has no RTTI (a set of an anonymous range, `set of 0..100`);
    /// such an attribute's arguments stay undecoded ([`AttributeArg::Raw`]),
    /// as the RTL itself cannot construct it.
    pub fn decode(
        ctx: &BinaryContext<'a>,
        classes: &ClassSet<'a>,
        entry: &AttributeEntry<'a>,
        layout: StringLayout,
    ) -> Self {
        let ptr_size = ctx.pointer_size().unwrap_or(4);
        let type_info = read_ptr(
            ctx.data(),
            ctx.va_to_file(entry.attr_type_ref).unwrap_or(usize::MAX),
            ptr_size,
        );
        let type_name = type_info
            .and_then(|ti| TypeHeader::from_va(ctx, ti, VmtFlavor::Delphi))
            .map(|h| h.name());
        let mut out = Self {
            type_name,
            constructor: entry.attr_ctor,
            constructor_name: None,
            args: Vec::new(),
        };
        let mut class = type_info.and_then(|ti| classes.iter().find(|c| c.vmt.type_info == ti));
        let mut ctor = None;
        // The constructor may be inherited (`TCustomAttribute.Create`).
        for _ in 0..MAX_ANCESTOR_DEPTH {
            let Some(c) = class else { break };
            ctor = DelphiExtMethod::iter(ctx, &c.vmt)
                .into_iter()
                .find(|m| m.code_va == entry.attr_ctor);
            if ctor.is_some() {
                break;
            }
            class = c.parent_index.and_then(|i| classes.get(i));
        }
        let Some(ctor) = ctor else {
            if entry.arg_data.is_empty() {
                return out;
            }
            out.args.push(AttributeArg::Raw(entry.arg_data));
            return out;
        };
        out.constructor_name = Some(ctor.name());
        let Some(sig) = MethodSignature::from_delphi_extended(
            ctx,
            ctor.name(),
            ctor.code_va,
            ctor.trailer,
            ptr_size,
            VmtFlavor::Delphi,
            ctor.info(),
        ) else {
            // No signature: the arguments cannot be sized.
            if !entry.arg_data.is_empty() {
                out.args.push(AttributeArg::Raw(entry.arg_data));
            }
            return out;
        };
        let mut rest = entry.arg_data;
        // The signature lists the implicit `Self` first, unflagged.
        let has_self = !ctor.info().is_static();
        for p in sig
            .params
            .iter()
            .skip(usize::from(has_self))
            .filter(|p| !p.flags.is_hidden())
        {
            let detail = ctx
                .va_to_file(p.type_va)
                .and_then(|off| read_ptr(ctx.data(), off, ptr_size))
                .and_then(|ti| TypeDetail::from_va(ctx, ti, VmtFlavor::Delphi, layout));
            match detail.and_then(|d| decode_arg(ctx, &d, rest, ptr_size)) {
                Some((arg, used)) => {
                    out.args.push(arg);
                    rest = rest.get(used..).unwrap_or_default();
                }
                None => {
                    out.args.push(AttributeArg::Raw(rest));
                    return out;
                }
            }
        }
        if !rest.is_empty() {
            out.args.push(AttributeArg::Raw(rest));
        }
        out
    }
}

/// Decodes one argument of type `detail` from the front of `data`, returning
/// it and the bytes it used.
fn decode_arg<'a>(
    ctx: &BinaryContext<'a>,
    detail: &TypeDetail<'a>,
    data: &'a [u8],
    ptr_size: usize,
) -> Option<(AttributeArg<'a>, usize)> {
    let int = |size: usize, signed: bool| -> Option<(AttributeArg<'a>, usize)> {
        // An ordinal type this crate does not size has size 0.
        if size == 0 {
            return None;
        }
        let bytes = data.get(..size)?;
        let mut buf = [0u8; 8];
        buf.get_mut(..size)?.copy_from_slice(bytes);
        let raw = u64::from_le_bytes(buf);
        let shift = 64u32.checked_sub(u32::try_from(size).ok()?.checked_mul(8)?)?;
        let value = if signed {
            ((raw << shift) as i64) >> shift
        } else {
            raw as i64
        };
        Some((AttributeArg::Ordinal(value), size))
    };
    let signed = |ord: OrdinalType| {
        matches!(
            ord,
            OrdinalType::SByte | OrdinalType::SWord | OrdinalType::SLong
        )
    };
    match detail {
        TypeDetail::Ordinal(o) => int(o.ord.size(), signed(o.ord)),
        TypeDetail::Enumeration(e) => int(e.ord.size(), signed(e.ord)),
        TypeDetail::Int64(i) => int(8, !i.unsigned),
        TypeDetail::Float(f) => {
            let (size, value) = match f.float_type {
                FloatType::Single => {
                    let b = data.get(..4)?;
                    (4, f64::from(f32::from_le_bytes(b.try_into().ok()?)))
                }
                FloatType::Double => {
                    let b = data.get(..8)?;
                    (8, f64::from_le_bytes(b.try_into().ok()?))
                }
                // Win32 Extended is the x87 80-bit form, Win64's a Double;
                // other 64-bit targets pad the 80-bit form to 16 bytes or
                // use a Double, which the argument bytes do not tell apart.
                FloatType::Extended if ptr_size == 4 => {
                    (10, extended_to_f64(data.get(..10)?.try_into().ok()?))
                }
                FloatType::Extended if ctx.target_os() == TargetOs::Windows => {
                    let b = data.get(..8)?;
                    (8, f64::from_le_bytes(b.try_into().ok()?))
                }
                FloatType::Comp => {
                    let b = data.get(..8)?;
                    (8, i64::from_le_bytes(b.try_into().ok()?) as f64)
                }
                FloatType::Currency => {
                    let b = data.get(..8)?;
                    (8, i64::from_le_bytes(b.try_into().ok()?) as f64 / 10_000.0)
                }
                _ => return None,
            };
            Some((AttributeArg::Float(value), size))
        }
        TypeDetail::String(_) => {
            let len = usize::from(read_u16(data, 0)?);
            let end = len.checked_add(2)?;
            let body = data.get(2..end)?;
            Some((
                AttributeArg::String(String::from_utf8_lossy(body).into_owned()),
                end,
            ))
        }
        // The compiler writes a ShortString argument with a `u16` length,
        // like a long string (Delphi 13; `ParseArgs.ReadShortString` reads
        // a `u8`, which would misread it).
        TypeDetail::ShortString(_) => {
            let len = usize::from(read_u16(data, 0)?);
            let end = len.checked_add(2)?;
            Some((AttributeArg::ShortString(data.get(2..end)?), end))
        }
        TypeDetail::Set(set) => {
            let size = usize::try_from(set.size?).ok()?;
            let bytes = data.get(..size)?;
            // Bit 0 of the first byte is the element type's lowest ordinal
            // rounded down to a multiple of 8 (`SetLoByte`).
            let low = i64::from(set.bounds?.0).div_euclid(8).checked_mul(8)?;
            let mut members = Vec::new();
            for (i, byte) in bytes.iter().enumerate() {
                for bit in 0..8u32 {
                    if byte & (1 << bit) != 0 {
                        let index = i64::try_from(i)
                            .ok()?
                            .checked_mul(8)?
                            .checked_add(i64::from(bit))?;
                        members.push(low.checked_add(index)?);
                    }
                }
            }
            Some((AttributeArg::Set(members), size))
        }
        TypeDetail::Class(_) | TypeDetail::ClassRef(_) | TypeDetail::Pointer(_) => {
            let value = read_ptr(data, 0, ptr_size)?;
            let type_name = ctx
                .va_to_file(value)
                .and_then(|off| read_ptr(ctx.data(), off, ptr_size))
                .and_then(|ti| TypeHeader::from_va(ctx, ti, VmtFlavor::Delphi))
                .map(|h| h.name());
            Some((AttributeArg::Reference { value, type_name }, ptr_size))
        }
        _ => None,
    }
}

/// Reads a `TAttrData` block at file offset `off` of `data`: a `u16`
/// length that counts itself, then that many bytes of [`AttributeEntry`]
/// records. Returns the entries and the offset just past the block.
///
/// Delphi 2010+ writes one after every extended member, signature
/// parameter and type (`System.TypInfo`: `TAttrData`, `TAttrEntry`); an
/// empty block is the bare length `2`. `None` when the length is below 2,
/// runs past `data`, or does not end on an entry boundary.
pub fn read_attr_data(
    data: &[u8],
    off: usize,
    ptr_size: usize,
) -> Option<(Vec<AttributeEntry<'_>>, usize)> {
    // The length counts itself, so less than 2 is malformed.
    let len = usize::from(read_u16(data, off)?);
    if len < 2 {
        return None;
    }
    let end = off.checked_add(len)?;
    let body = data.get(off.checked_add(2)?..end)?;
    let (entries, used) = decode_attribute_block(body, ptr_size);
    (used == body.len()).then_some((entries, end))
}

fn decode_attribute_block<'a>(
    bytes: &'a [u8],
    ptr_size: usize,
) -> (Vec<AttributeEntry<'a>>, usize) {
    let mut cursor = 0usize;
    let mut out = Vec::new();
    while let Some((entry, advance)) = decode_one_attribute(bytes, cursor, ptr_size) {
        out.push(entry);
        cursor = advance;
    }
    (out, cursor)
}

/// Decode a single attribute entry at `cursor`. Returns the entry and the
/// cursor position one byte past it, or `None` when any read overruns.
fn decode_one_attribute<'a>(
    bytes: &'a [u8],
    cursor: usize,
    ptr_size: usize,
) -> Option<(AttributeEntry<'a>, usize)> {
    let attr_type_ref = read_ptr(bytes, cursor, ptr_size)?;
    let attr_ctor_off = cursor.checked_add(ptr_size)?;
    let arg_len_off = attr_ctor_off.checked_add(ptr_size)?;
    let arg_start = arg_len_off.checked_add(2)?;
    let attr_ctor = read_ptr(bytes, attr_ctor_off, ptr_size)?;
    let arg_len = read_u16(bytes, arg_len_off).map(usize::from)?;
    let arg_end = arg_start.checked_add(arg_len)?;
    let arg_data = bytes.get(arg_start..arg_end)?;
    Some((
        AttributeEntry {
            attr_type_ref,
            attr_ctor,
            arg_data,
        },
        arg_end,
    ))
}

impl<'a> ExtendedProperty<'a> {
    /// Walk the extended-RTTI property block for `class`. Returns an
    /// empty vector when no extended block is present (Delphi before 2010
    /// and FPC never emit one; later classes can disable it with the
    /// `{$RTTI}` directive).
    pub fn iter(ctx: &BinaryContext<'a>, class: &Class<'a>) -> Vec<Self> {
        if predates_extended_rtti(class) {
            return Vec::new();
        }
        let tk = match TkClassInfo::from_vmt(ctx, &class.vmt) {
            Some(v) => v,
            None => return Vec::new(),
        };
        // Re-walk classic properties to advance past them, then try the
        // extended block.
        let data = ctx.data();
        let psize = class.vmt.pointer_size as usize;
        let Some(classic_count) = read_u16(data, tk.prop_data_file_offset).map(usize::from) else {
            return Vec::new();
        };
        let Some(fixed) = psize.checked_mul(4).and_then(|n| n.checked_add(10)) else {
            return Vec::new();
        };
        let Some(mut cursor) = tk.prop_data_file_offset.checked_add(2) else {
            return Vec::new();
        };
        for _ in 0..classic_count {
            let Some(name_off) = cursor.checked_add(fixed) else {
                return Vec::new();
            };
            let Some(name) = read_short_string_at_file(data, name_off) else {
                return Vec::new();
            };
            if name.is_empty() || name.len() > MAX_IDENTIFIER_BYTES {
                return Vec::new();
            }
            let Some(advance) = fixed.checked_add(1).and_then(|n| n.checked_add(name.len())) else {
                return Vec::new();
            };
            let Some(next) = cursor.checked_add(advance) else {
                return Vec::new();
            };
            cursor = next;
        }

        // Extended block starts here. Read u16 count.
        let Some(ext_count) = read_u16(data, cursor).map(usize::from) else {
            return Vec::new();
        };
        if ext_count == 0 || ext_count > MAX_EXTENDED_PROPERTIES_PER_CLASS {
            return Vec::new();
        }
        let Some(after_ext_count) = cursor.checked_add(2) else {
            return Vec::new();
        };
        cursor = after_ext_count;

        let mut out = Vec::with_capacity(ext_count);
        for _ in 0..ext_count {
            // Entry header: Flags:u8, PPropInfo:ptr, ExtraLen:u16, Extra.
            let Some(&flags) = data.get(cursor) else {
                break;
            };
            let Some(info_ptr_off) = cursor.checked_add(1) else {
                break;
            };
            let info_ptr = match read_ptr(data, info_ptr_off, psize) {
                Some(v) => v,
                None => break,
            };
            let Some(extra_len_off) = info_ptr_off.checked_add(psize) else {
                break;
            };
            let Some(extra_len) = read_u16(data, extra_len_off).map(usize::from) else {
                break;
            };
            let Some(attr_start) = extra_len_off.checked_add(2) else {
                break;
            };
            if extra_len < 2 {
                break;
            }
            let Some(attrs_len) = extra_len.checked_sub(2) else {
                break;
            };
            let Some(attr_end) = attr_start.checked_add(attrs_len) else {
                break;
            };
            let Some(attributes_raw) = data.get(attr_start..attr_end) else {
                break;
            };
            let Some(advance) = 1usize
                .checked_add(psize)
                .and_then(|n| n.checked_add(extra_len))
            else {
                break;
            };
            let Some(next_cursor) = cursor.checked_add(advance) else {
                break;
            };

            // Dereference info_ptr to a TPropInfo and decode it.
            let Some(prop) = Property::from_delphi_record(ctx, info_ptr, psize) else {
                // Skip entries we can't dereference - some records point at
                // Ancestor classes that live outside our scanned region.
                cursor = next_cursor;
                continue;
            };

            out.push(ExtendedProperty {
                visibility: Visibility::from_flags(flags),
                flags,
                info: prop,
                attributes_raw,
            });
            cursor = next_cursor;
        }
        out
    }
}

impl<'a> AttributeEntry<'a> {
    /// Walk the class attribute table at `va` (typically the
    /// `TkClassInfo` `AttributeTable` pointer, for newer Delphi
    /// versions that emit one).
    pub fn iter_at(ctx: &BinaryContext<'a>, va: u64, ptr_size: usize) -> Vec<Self> {
        iter_class_attributes(ctx, va, ptr_size)
    }
}

fn iter_class_attributes<'a>(
    ctx: &BinaryContext<'a>,
    va: u64,
    ptr_size: usize,
) -> Vec<AttributeEntry<'a>> {
    if va == 0 {
        return Vec::new();
    }
    let Some(file_off) = ctx.va_to_file(va) else {
        return Vec::new();
    };
    let data = ctx.data();
    // The block has no count: decode entries greedily within a bound.
    let Some(remaining) = data.len().checked_sub(file_off) else {
        return Vec::new();
    };
    let cap = MAX_ATTR_DATA_BYTES.min(remaining);
    let Some(end) = file_off.checked_add(cap) else {
        return Vec::new();
    };
    let Some(slice) = data.get(file_off..end) else {
        return Vec::new();
    };
    decode_attribute_block(slice, ptr_size).0
}

/// Find the file offset of the class-level attribute block by walking
/// past the classic `TPropData` and the extended-property block.
///
/// Modern Delphi RTTI (XE3+) emits the class-level `AttrData` immediately
/// after the extended-property block. The block is a `u16` length
/// followed by `length - 2` bytes of packed attribute entries (same
/// shape as the per-property attribute payloads).
///
/// Returns `None` for binaries that don't emit extended RTTI (older
/// Delphi, or `{$RTTI EXPLICIT}` directives that disable it).
pub fn class_attribute_block_offset<'a>(
    ctx: &BinaryContext<'a>,
    class: &Class<'a>,
) -> Option<usize> {
    if predates_extended_rtti(class) {
        return None;
    }
    let tk = TkClassInfo::from_vmt(ctx, &class.vmt)?;
    let data = ctx.data();
    let psize = class.vmt.pointer_size as usize;

    // Walk past the classic TPropData entries.
    let classic_count = read_u16(data, tk.prop_data_file_offset)? as usize;
    let max_classic = MAX_EXTENDED_PROPERTIES_PER_CLASS.checked_mul(4)?;
    if classic_count > max_classic {
        return None;
    }
    let fixed = psize.checked_mul(4)?.checked_add(10)?;
    let mut cursor = tk.prop_data_file_offset.checked_add(2)?;
    for _ in 0..classic_count {
        let name_off = cursor.checked_add(fixed)?;
        let name = read_short_string_at_file(data, name_off)?;
        if name.is_empty() || name.len() > MAX_IDENTIFIER_BYTES {
            return None;
        }
        let advance = fixed.checked_add(1)?.checked_add(name.len())?;
        cursor = cursor.checked_add(advance)?;
    }

    // Walk past the extended-property block, if any. Some classes have
    // no extended RTTI; the u16 we read here will be either a real
    // ext_count (followed by entries) or the leading u16 of the
    // class-level AttrData block. We can't tell without trying both.
    //
    // Heuristic: try to walk as if it's the extended-property block.
    // If every entry's `extra_len` stays within plausible bounds and we
    // land on a position where the next u16 is a plausible AttrData
    // length, we accept it. Otherwise treat the original u16 as the
    // AttrData length directly.
    let ext_marker = read_u16(data, cursor)? as usize;
    let mut after_ext = cursor.checked_add(2)?;
    let mut ext_walk_ok = true;
    if ext_marker > 0 && ext_marker <= MAX_EXTENDED_PROPERTIES_PER_CLASS {
        // Tentatively walk as ext-property block.
        let mut probe = after_ext;
        for _ in 0..ext_marker {
            // Each entry: u8 flags + ptr info + u16 extra_len + extra.
            if data.get(probe).is_none() {
                ext_walk_ok = false;
                break;
            }
            let Some(extra_len_off) = probe.checked_add(1).and_then(|n| n.checked_add(psize))
            else {
                ext_walk_ok = false;
                break;
            };
            let extra_len = match read_u16(data, extra_len_off) {
                Some(v) => v as usize,
                None => {
                    ext_walk_ok = false;
                    break;
                }
            };
            if !(2..=MAX_ATTR_DATA_BYTES).contains(&extra_len) {
                ext_walk_ok = false;
                break;
            }
            let Some(advance) = 1usize
                .checked_add(psize)
                .and_then(|n| n.checked_add(extra_len))
            else {
                ext_walk_ok = false;
                break;
            };
            let Some(next) = probe.checked_add(advance) else {
                ext_walk_ok = false;
                break;
            };
            probe = next;
        }
        if ext_walk_ok {
            after_ext = probe;
        }
    }

    Some(
        if ext_walk_ok && ext_marker > 0 && ext_marker <= MAX_EXTENDED_PROPERTIES_PER_CLASS {
            after_ext
        } else if ext_marker == 0 && read_u16(data, after_ext).is_some_and(|len| len >= 2) {
            // An empty `PropDataEx` (no extended properties): its zero
            // count, then the AttrData (`Len` is at least 2).
            after_ext
        } else {
            // Delphi 2010+ always writes the `PropDataEx` count; a walk
            // that fails means a layout this decoder does not know.
            return None;
        },
    )
}

/// An indexed (array) property from a class's extended RTTI
/// (`TArrayPropInfo`): `property Items[Index: Integer]: T read ... write
/// ...`.
#[derive(Debug, Clone)]
pub struct ArrayProperty<'a> {
    /// Flags byte: bit 0 `default`, the visibility in bits 1-2.
    pub flags: u8,
    /// Index of the getter in the class's extended-method list
    /// ([`crate::methods::DelphiExtMethod::iter`] order); `None` when the
    /// property is not readable.
    pub read_index: Option<u16>,
    /// Index of the setter, likewise; `None` when not writable.
    pub write_index: Option<u16>,
    name: &'a [u8],
    /// The property's attributes.
    pub attributes: Vec<AttributeEntry<'a>>,
}

impl<'a> ArrayProperty<'a> {
    /// The property name, lossily decoded.
    pub fn name(&self) -> &'a str {
        str::from_utf8(self.name).unwrap_or("<non-ascii>")
    }

    /// Whether the property is the class's `default` array property.
    pub fn is_default(&self) -> bool {
        self.flags & 0x01 != 0
    }

    /// The declared visibility.
    pub fn visibility(&self) -> Visibility {
        Visibility::from_flags(self.flags >> 1)
    }

    /// Decodes `class`'s indexed properties: the `ArrayPropCount: u16` and
    /// `TArrayPropInfo` entries (`Flags: u8; ReadIndex, WriteIndex: u16
    /// ($FFFF = none); Name: ShortString; AttrData`) after the class-level
    /// `AttrData` (Delphi 13 `System.TypInfo`, `tkClass` variant; `System.Rtti`
    /// `TRttiIndexedProperty`). Empty when the class predates extended RTTI
    /// or declares none.
    pub fn iter(ctx: &BinaryContext<'a>, class: &Class<'a>) -> Vec<Self> {
        read_array_properties(ctx, class).unwrap_or_default()
    }
}

/// Reads `class`'s indexed-property table; `None` when it does not parse.
fn read_array_properties<'a>(
    ctx: &BinaryContext<'a>,
    class: &Class<'a>,
) -> Option<Vec<ArrayProperty<'a>>> {
    let psize = usize::from(class.vmt.pointer_size);
    let data = ctx.data();
    let attr_off = class_attribute_block_offset(ctx, class)?;
    let (_, mut off) = read_attr_data(data, attr_off, psize)?;
    let count = usize::from(read_u16(data, off)?);
    if count > MAX_EXTENDED_PROPERTIES_PER_CLASS {
        return None;
    }
    off = off.checked_add(2)?;
    let index = |v: u16| (v != 0xFFFF).then_some(v);
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let flags = *data.get(off)?;
        let read_index = index(read_u16(data, off.checked_add(1)?)?);
        let write_index = index(read_u16(data, off.checked_add(3)?)?);
        let name_off = off.checked_add(5)?;
        let name = read_short_string_at_file(data, name_off)?;
        if name.is_empty() || name.len() > MAX_IDENTIFIER_BYTES {
            return None;
        }
        let (attributes, next) = read_attr_data(
            data,
            name_off.checked_add(1)?.checked_add(name.len())?,
            psize,
        )?;
        off = next;
        out.push(ArrayProperty {
            flags,
            read_index,
            write_index,
            name,
            attributes,
        });
    }
    Some(out)
}

/// Returns `true` when `class`'s `tkClass` record cannot carry the Delphi
/// extended-RTTI blocks these walkers read, so whatever follows its classic
/// property data belongs to the next record.
///
/// - A Delphi VMT older than the Delphi 2009 header (22 slots): extended
///   RTTI arrived with Delphi 2010, and the 2009 header is the closest
///   structural date the VMT carries. Without this gate the walkers read
///   Delphi 6 / C++Builder 6 neighbours as attribute blocks
///   (`tests/samples/bcb6/gtforms.exe`).
/// - Any FPC class: FPC 3.0 / 3.2 emit no extended blocks, and FPC trunk
///   stores attributes through an attribute-table pointer, not in this
///   layout. Read as Delphi blocks, the FPC corpus yields ASCII names as
///   constructor pointers.
fn predates_extended_rtti(class: &Class<'_>) -> bool {
    class.vmt.flavor == VmtFlavor::Fpc || !class.vmt.is_delphi_2009_or_later()
}

impl<'a> AttributeEntry<'a> {
    /// Decode the class-level attribute block - finds the offset via
    /// [`class_attribute_block_offset`] and decodes the `u16`-prefixed
    /// packed attribute entries that follow. Returns an empty vector
    /// when the binary doesn't carry an `AttrData` trailer for this
    /// class.
    pub fn iter_class(ctx: &BinaryContext<'a>, class: &Class<'a>) -> Vec<Self> {
        let psize = class.vmt.pointer_size as usize;
        let Some(off) = class_attribute_block_offset(ctx, class) else {
            return Vec::new();
        };
        let data = ctx.data();
        // AttrData layout: `u16 length` (counting itself), then
        // `length - 2` bytes of packed attribute entries.
        let Some(length) = read_u16(data, off).map(usize::from) else {
            return Vec::new();
        };
        if length < 2 {
            return Vec::new();
        }
        let Some(body_len) = length.checked_sub(2) else {
            return Vec::new();
        };
        if body_len > MAX_ATTR_DATA_BYTES {
            return Vec::new();
        }
        let Some(body_start) = off.checked_add(2) else {
            return Vec::new();
        };
        let Some(body_end) = body_start.checked_add(body_len) else {
            return Vec::new();
        };
        let Some(body) = data.get(body_start..body_end) else {
            return Vec::new();
        };
        let entries = decode_attribute_block(body, psize).0;
        // Every attribute is constructed by code: a constructor outside the
        // code sections means the bytes were not an attribute block.
        if entries.iter().any(|entry| !ctx.is_code_va(entry.attr_ctor)) {
            return Vec::new();
        }
        entries
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visibility_bits() {
        assert_eq!(Visibility::from_flags(0), Visibility::Private);
        assert_eq!(Visibility::from_flags(1), Visibility::Protected);
        assert_eq!(Visibility::from_flags(2), Visibility::Public);
        assert_eq!(Visibility::from_flags(3), Visibility::Published);
        assert_eq!(Visibility::from_flags(0x82), Visibility::Public);
    }

    #[test]
    fn empty_attribute_block_decodes_trivially() {
        // A zero-byte slice should yield no entries.
        let (entries, consumed) = decode_attribute_block(&[], 8);
        assert!(entries.is_empty());
        assert_eq!(consumed, 0);
    }
}
