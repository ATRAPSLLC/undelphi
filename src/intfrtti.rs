//! Delphi interface method RTTI.
//!
//! A Delphi `tkInterface` record continues past its unit name with a method
//! table. Interfaces compiled with method info (`{$M+}`, which `IInvokable`
//! carries - the SOAP / invokable-interface machinery) list every method
//! they declare; others record only the count.
//!
//! Source: Delphi 6 `IntfInfo.pas` (`GetIntfMetaData`, `FillMethodArray`),
//! the reader the SOAP runtime itself uses; validated against
//! `tests/samples/bcb6/deep.exe` (`ISvc`). Delphi 2010+ adds an `AttrData`
//! block per parameter and per method (`TIntfMethodEntry` /
//! `TIntfMethodParam` in `System.TypInfo`), validated against Delphi 13's
//! Win32 and Win64 builds of `deep.exe` and `modernbin.exe` (`ISource`,
//! `tests/samples/d13`).
//!
//! ```text
//!   ... IntfParent, IntfFlags, Guid, IntfUnit (see InterfaceTypeInfo)
//!   MethodCount: u16              methods this interface declares
//!   RttiCount:   u16              $FFFF when no method info was emitted
//!   methods[RttiCount]:
//!     Name:       ShortString
//!     Kind:       u8              0 procedure, 1 function
//!     CC:         u8              TCallConv: register, cdecl, pascal,
//!                                 stdcall, safecall
//!     ParamCount: u8              including the implicit Self
//!     params[ParamCount]:
//!       Flags:    u8              TParamFlags
//!       Name:     ShortString
//!       TypeName: ShortString
//!       TypeInfo: PPTypeInfo
//!       AttrData: u16 + bytes     Delphi 2010+ only
//!     ResultTypeName: ShortString functions only
//!     ResultType:     PPTypeInfo  functions only (2010+: when the name is
//!                                 not empty)
//!     AttrData:   u16 + bytes     Delphi 2010+ only
//! ```
//!
//! The methods are the interface's own, in vtable order after every
//! ancestor's methods; [`InterfaceRtti::ancestor_method_count`] follows the
//! `IntfParent` chain (each ancestor records its `MethodCount` even without
//! method info) to place them.
//!
//! Empirically, Delphi 6 and 13 also set bit 3 (`$08`) of `IntfFlags` -
//! outside the three `TIntfFlag` members - on an interface that carries
//! method info (`deep.exe`: `ISvc` has flags `$09`).

use std::str;

use crate::{
    formats::BinaryContext,
    rtti::{InterfaceTypeInfo, MethodKind, TypeHeader},
    signatures::CallConv,
    util::{read_ptr, read_short_string_at_file, read_u16},
    vmt::VmtFlavor,
};

/// Upper bound on the methods one interface may declare.
const MAX_METHODS: usize = 4096;

/// Upper bound on how deep an interface ancestry is followed.
const MAX_ANCESTRY: usize = 64;

/// The method table of a Delphi interface's RTTI.
#[derive(Debug, Clone)]
pub struct InterfaceRtti<'a> {
    /// Number of methods the interface itself declares.
    pub method_count: u16,
    /// Method-info records, in declaration (vtable) order. Empty when the
    /// interface was compiled without method info.
    pub methods: Vec<InterfaceMethodRtti<'a>>,
}

/// One method of an interface, from its method info.
#[derive(Debug, Clone)]
pub struct InterfaceMethodRtti<'a> {
    name: &'a [u8],
    /// Procedure or function.
    pub kind: MethodKind,
    /// Calling convention.
    pub calling_convention: CallConv,
    /// `PPTypeInfo` of the implicit `Self` parameter (the interface).
    pub self_type_ref: u64,
    /// Declared parameters, excluding `Self`.
    pub params: Vec<InterfaceParamRtti<'a>>,
    result_type_name: Option<&'a [u8]>,
    /// `PPTypeInfo` of the result, `0` for a procedure.
    pub result_type_ref: u64,
}

/// One declared parameter of an interface method.
#[derive(Debug, Clone)]
pub struct InterfaceParamRtti<'a> {
    /// `TParamFlags`: `$01` var, `$02` const, `$04` array, `$08` address,
    /// `$10` reference, `$20` out.
    pub flags: u8,
    name: &'a [u8],
    type_name: &'a [u8],
    /// `PPTypeInfo` of the parameter's type.
    pub type_ref: u64,
}

impl<'a> InterfaceMethodRtti<'a> {
    /// Method name, lossily decoded.
    #[inline]
    pub fn name(&self) -> &'a str {
        str::from_utf8(self.name).unwrap_or("<non-ascii>")
    }

    /// Result type name for a function, lossily decoded.
    #[inline]
    pub fn result_type_name(&self) -> Option<&'a str> {
        self.result_type_name
            .map(|n| str::from_utf8(n).unwrap_or("<non-ascii>"))
    }
}

impl<'a> InterfaceParamRtti<'a> {
    /// Parameter name, lossily decoded.
    #[inline]
    pub fn name(&self) -> &'a str {
        str::from_utf8(self.name).unwrap_or("<non-ascii>")
    }

    /// Declared type name, lossily decoded.
    #[inline]
    pub fn type_name(&self) -> &'a str {
        str::from_utf8(self.type_name).unwrap_or("<non-ascii>")
    }
}

impl<'a> InterfaceRtti<'a> {
    /// Decode the method table that follows `info`'s unit name.
    /// `attributes` selects the Delphi 2010+ layout with `AttrData` blocks.
    /// `None` when the table does not parse.
    pub fn decode(
        ctx: &BinaryContext<'a>,
        info: &InterfaceTypeInfo<'a>,
        attributes: bool,
    ) -> Option<Self> {
        let data = ctx.data();
        let ptr_size = ctx.pointer_size()?;
        let mut cursor = ctx.va_to_file(info.method_table_va())?;
        let method_count = read_u16(data, cursor)?;
        let rtti_count = read_u16(data, cursor.checked_add(2)?)?;
        cursor = cursor.checked_add(4)?;
        if usize::from(method_count) > MAX_METHODS {
            return None;
        }
        if rtti_count == u16::MAX {
            return Some(Self {
                method_count,
                methods: Vec::new(),
            });
        }
        if rtti_count != method_count {
            return None;
        }
        let mut methods = Vec::with_capacity(usize::from(rtti_count));
        for _ in 0..rtti_count {
            let (method, next) = read_method(data, cursor, ptr_size, attributes)?;
            methods.push(method);
            cursor = next;
        }
        Some(Self {
            method_count,
            methods,
        })
    }

    /// Sum of the `MethodCount`s of `info`'s ancestors: the vtable slot of
    /// its first own method. `None` when an ancestor's table cannot be read.
    pub fn ancestor_method_count(
        ctx: &BinaryContext<'a>,
        info: &InterfaceTypeInfo<'a>,
        flavor: VmtFlavor,
    ) -> Option<usize> {
        let ptr_size = ctx.pointer_size()?;
        let mut total = 0usize;
        let mut parent_ref = info.parent_ref;
        for _ in 0..MAX_ANCESTRY {
            let Some(parent_va) = TypeHeader::deref_pptypeinfo(ctx, parent_ref, ptr_size) else {
                return Some(total);
            };
            if parent_va == 0 {
                return Some(total);
            }
            let parent = InterfaceTypeInfo::from_va(ctx, parent_va, flavor)?;
            let count = read_u16(ctx.data(), ctx.va_to_file(parent.method_table_va())?)?;
            total = total.checked_add(usize::from(count))?;
            parent_ref = parent.parent_ref;
        }
        None
    }
}

/// Reads one method record at file offset `at`, answering it and the offset
/// just past it.
fn read_method(
    data: &[u8],
    at: usize,
    ptr_size: usize,
    attributes: bool,
) -> Option<(InterfaceMethodRtti<'_>, usize)> {
    let name = read_name(data, at)?;
    let mut cursor = at.checked_add(1)?.checked_add(name.len())?;
    let kind_byte = *data.get(cursor)?;
    let cc = *data.get(cursor.checked_add(1)?)?;
    let param_count = *data.get(cursor.checked_add(2)?)?;
    cursor = cursor.checked_add(3)?;
    if kind_byte > 1 || cc > 4 || param_count == 0 {
        return None;
    }
    let mut self_type_ref = 0;
    let mut params = Vec::with_capacity(usize::from(param_count));
    for index in 0..param_count {
        let flags = *data.get(cursor)?;
        let param_name = read_short_string_at_file(data, cursor.checked_add(1)?)?;
        cursor = cursor.checked_add(2)?.checked_add(param_name.len())?;
        let type_name = read_short_string_at_file(data, cursor)?;
        cursor = cursor.checked_add(1)?.checked_add(type_name.len())?;
        let type_ref = read_ptr(data, cursor, ptr_size)?;
        cursor = cursor.checked_add(ptr_size)?;
        if attributes {
            cursor = skip_attr_data(data, cursor)?;
        }
        if index == 0 {
            self_type_ref = type_ref;
        } else {
            params.push(InterfaceParamRtti {
                flags,
                name: param_name,
                type_name,
                type_ref,
            });
        }
    }
    let kind = MethodKind::from_u8(kind_byte);
    let (result_type_name, result_type_ref) = if kind == MethodKind::Function {
        let result_name = read_short_string_at_file(data, cursor)?;
        cursor = cursor.checked_add(1)?.checked_add(result_name.len())?;
        let reference = if !attributes || !result_name.is_empty() {
            let r = read_ptr(data, cursor, ptr_size)?;
            cursor = cursor.checked_add(ptr_size)?;
            r
        } else {
            0
        };
        (Some(result_name), reference)
    } else {
        (None, 0)
    };
    if attributes {
        cursor = skip_attr_data(data, cursor)?;
    }
    Some((
        InterfaceMethodRtti {
            name,
            kind,
            calling_convention: CallConv::from_u8(cc),
            self_type_ref,
            params,
            result_type_name,
            result_type_ref,
        },
        cursor,
    ))
}

/// Reads a method name, requiring a plausible identifier.
fn read_name(data: &[u8], at: usize) -> Option<&[u8]> {
    let name = read_short_string_at_file(data, at)?;
    (!name.is_empty() && name.iter().all(|&b| b.is_ascii_alphanumeric() || b == b'_'))
        .then_some(name)
}

/// Skips a Delphi 2010+ `AttrData` block: a `u16` length that counts itself,
/// then the attribute entries.
fn skip_attr_data(data: &[u8], at: usize) -> Option<usize> {
    let len = usize::from(read_u16(data, at)?);
    if len < 2 {
        return None;
    }
    at.checked_add(len)
}
