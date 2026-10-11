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
}

impl Visibility {
    /// Decodes the visibility from the low two bits of an extended-RTTI
    /// flags byte (`0` private, `1` protected, `2` public, `3` published).
    pub fn from_flags(b: u8) -> Self {
        match b & 0x03 {
            0 => Visibility::Private,
            1 => Visibility::Protected,
            2 => Visibility::Public,
            _ => Visibility::Published,
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
    /// The property's attributes (`[Name(...)]` before the declaration).
    pub attributes: Vec<AttributeEntry<'a>>,
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
    /// Interprets `arg_data` as a single `u16`-prefixed string argument,
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
    /// Decodes every entry of an attribute block at `bytes`. Entries
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
        let mut class = type_info.and_then(|ti| classes.find_by_type_info(ti));
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
/// empty block is the bare length `2`. `None` when the length is below 2
/// or above [`MAX_ATTR_DATA_BYTES`], runs past `data`, or does not end on
/// an entry boundary. Every reader of an attribute block goes through this
/// one.
pub fn read_attr_data(
    data: &[u8],
    off: usize,
    ptr_size: usize,
) -> Option<(Vec<AttributeEntry<'_>>, usize)> {
    // The length counts itself, so less than 2 is malformed.
    let len = usize::from(read_u16(data, off)?);
    if !(2..=MAX_ATTR_DATA_BYTES).contains(&len) {
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

/// Decodes a single attribute entry at `cursor`. Returns the entry and the
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
    /// Walks the extended-RTTI property block for `class`. Returns an
    /// empty vector when no extended block is present (Delphi before 2010
    /// and FPC never emit one; later classes can disable it with the
    /// `{$RTTI}` directive). Entries whose `TPropInfo` lies outside the
    /// image (an ancestor's, in another module) are left out.
    pub fn iter(ctx: &BinaryContext<'a>, class: &Class<'a>) -> Vec<Self> {
        let psize = usize::from(class.vmt.pointer_size);
        let Some(table) = PropertyTables::read(ctx, class) else {
            return Vec::new();
        };
        table
            .extended
            .into_iter()
            .filter_map(|entry| {
                Some(ExtendedProperty {
                    visibility: Visibility::from_flags(entry.flags),
                    flags: entry.flags,
                    info: Property::from_delphi_record(ctx, entry.info, psize)?,
                    attributes: entry.attributes,
                })
            })
            .collect()
    }
}

/// One entry of a class's extended-property table (`PropDataEx`), before
/// its `TPropInfo` is dereferenced.
struct ExtendedEntry<'a> {
    flags: u8,
    info: u64,
    attributes: Vec<AttributeEntry<'a>>,
}

/// A class's property tables as one walk reads them: the classic
/// `TPropData` (skipped), the extended `PropDataEx` and where the
/// class-level `AttrData` that follows them starts.
struct PropertyTables<'a> {
    extended: Vec<ExtendedEntry<'a>>,
    attr_data: usize,
}

impl<'a> PropertyTables<'a> {
    /// Walks `class`'s `tkClass` property data:
    ///
    /// ```text
    ///   PropCount: u16, TPropInfo[PropCount]   classic (published)
    ///   PropCountEx: u16                         Delphi 2010+, always written
    ///   entries[PropCountEx]: Flags u8, PPropInfo ptr, AttrData
    ///   AttrData                                 the class's attributes
    /// ```
    ///
    /// `None` when the class predates extended RTTI or any part does not
    /// parse (a layout this decoder does not know).
    fn read(ctx: &BinaryContext<'a>, class: &Class<'a>) -> Option<Self> {
        if predates_extended_rtti(class) {
            return None;
        }
        let tk = TkClassInfo::from_vmt(ctx, &class.vmt)?;
        let data = ctx.data();
        let psize = usize::from(class.vmt.pointer_size);

        // The classic entries: a fixed part (four pointers, `Index`,
        // `Default`, `NameIndex`) and the name.
        let classic_count = usize::from(read_u16(data, tk.prop_data_file_offset)?);
        if classic_count > MAX_EXTENDED_PROPERTIES_PER_CLASS.checked_mul(4)? {
            return None;
        }
        let fixed = psize.checked_mul(4)?.checked_add(10)?;
        let mut cursor = tk.prop_data_file_offset.checked_add(2)?;
        for _ in 0..classic_count {
            let name = read_short_string_at_file(data, cursor.checked_add(fixed)?)?;
            if name.is_empty() || name.len() > MAX_IDENTIFIER_BYTES {
                return None;
            }
            cursor = cursor
                .checked_add(fixed)?
                .checked_add(1)?
                .checked_add(name.len())?;
        }

        let count = usize::from(read_u16(data, cursor)?);
        if count > MAX_EXTENDED_PROPERTIES_PER_CLASS {
            return None;
        }
        cursor = cursor.checked_add(2)?;
        let mut extended = Vec::with_capacity(count);
        for _ in 0..count {
            let flags = *data.get(cursor)?;
            let info_off = cursor.checked_add(1)?;
            let info = read_ptr(data, info_off, psize)?;
            let (attributes, next) = read_attr_data(data, info_off.checked_add(psize)?, psize)?;
            extended.push(ExtendedEntry {
                flags,
                info,
                attributes,
            });
            cursor = next;
        }
        Some(Self {
            extended,
            attr_data: cursor,
        })
    }
}

/// Finds the file offset of the class-level attribute block (`AttrData`) by
/// walking past the classic `TPropData` and the extended-property block
/// (Delphi XE3+ writes it right after them).
///
/// Returns `None` for binaries that don't emit extended RTTI (older
/// Delphi, or `{$RTTI EXPLICIT}` directives that disable it).
pub fn class_attribute_block_offset<'a>(
    ctx: &BinaryContext<'a>,
    class: &Class<'a>,
) -> Option<usize> {
    PropertyTables::read(ctx, class).map(|t| t.attr_data)
}

/// An indexed (array) property from a class's extended RTTI
/// (`TArrayPropInfo`): `property Items[Index: Integer]: T read ... write
/// ...`.
#[derive(Debug, Clone)]
pub struct ArrayProperty<'a> {
    /// Flags byte: bit 0 `default`, the visibility in bits 1-2.
    pub flags: u8,
    /// Position of the getter in the class's extended-method table
    /// ([`DelphiExtMethod::index`]; [`ArrayProperty::getter`] looks it up);
    /// `None` when the property is not readable.
    pub read_index: Option<u16>,
    /// Position of the setter, likewise; `None` when not writable.
    pub write_index: Option<u16>,
    name: &'a [u8],
    /// The property's attributes.
    pub attributes: Vec<AttributeEntry<'a>>,
}

impl<'a> ArrayProperty<'a> {
    /// The property name: UTF-8, as Delphi 2009 and later store names, or `"<non-ascii>"` when
    /// the bytes are not valid UTF-8.
    pub fn name(&self) -> &'a str {
        str::from_utf8(self.name).unwrap_or("<non-ascii>")
    }

    /// Returns the getter among `methods` (the class's
    /// [`DelphiExtMethod::iter`]), by its table position.
    pub fn getter<'m>(
        &self,
        methods: &'m [DelphiExtMethod<'a>],
    ) -> Option<&'m DelphiExtMethod<'a>> {
        let index = self.read_index?;
        methods.iter().find(|m| m.index == index)
    }

    /// Returns the setter among `methods`, likewise.
    pub fn setter<'m>(
        &self,
        methods: &'m [DelphiExtMethod<'a>],
    ) -> Option<&'m DelphiExtMethod<'a>> {
        let index = self.write_index?;
        methods.iter().find(|m| m.index == index)
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
///   structural date the VMT carries ([`crate::DelphiBinary`] also checks
///   the compiler version). Without this gate the walkers read
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
    /// Decodes the class-level attribute block found by
    /// [`class_attribute_block_offset`]. Returns an empty vector when the
    /// binary doesn't carry an `AttrData` trailer for this class.
    pub fn iter_class(ctx: &BinaryContext<'a>, class: &Class<'a>) -> Vec<Self> {
        let psize = usize::from(class.vmt.pointer_size);
        let Some((entries, _)) = class_attribute_block_offset(ctx, class)
            .and_then(|off| read_attr_data(ctx.data(), off, psize))
        else {
            return Vec::new();
        };
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
