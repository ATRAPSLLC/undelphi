//! Method-signature decoding.
//!
//! Recovers a method's calling convention, ordered parameters (name, type,
//! and passing mode), and return type. Three distinct on-disk sources carry
//! this information across the Delphi / FPC era matrix, and they are *not*
//! interchangeable:
//!
//! 1. **Delphi published-method trailer** - extra bytes appended to each
//!    [`crate::methods::MethodEntry`] in the classic `vmtMethodTable`. Only
//!    emitted under `{$METHODINFO ON}` (Web Services / RemObjects), so it is
//!    absent from most binaries. Layout reverse-engineered from
//!    IDR `IDCGen.cpp::OutputVmtMethodEntryTail` (32-bit) and
//!    IDR64 `IDCGen.cpp` (64-bit). Decoded by
//!    [`MethodSignature::from_published_trailer`].
//! 2. **FPC `TVmtMethodExTable`** - a separate, richer table sited
//!    immediately after the basic FPC method-name table. Documented in
//!    FPC `rtl/objpas/typinfo.pp`. Not decoded.
//! 3. **Delphi 2010+ extended-RTTI method table** - part of the extended
//!    `tkClass` type data. Layout is not publicly documented. Decoded by
//!    [`MethodSignature::from_delphi_extended`].
//!
//! Because availability varies by compiler era, the public accessor returns
//! a [`SignatureReport`] that distinguishes "no signature RTTI was emitted"
//! ([`SignatureReport::Absent`]) from "RTTI is present but this parser does
//! not decode its layout yet" ([`SignatureReport::Unsupported`]).

use std::str;

use crate::{
    extrtti::{AttributeEntry, Visibility, read_attr_data},
    formats::BinaryContext,
    rtti::TypeHeader,
    util::{read_ptr, read_short_string_at_file},
    vmt::VmtFlavor,
};

/// The on-disk structure a [`MethodSignature`] was decoded from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignatureSource {
    /// Classic `vmtMethodTable` entry trailer (Delphi, `{$METHODINFO ON}`).
    PublishedTrailer,
    /// FPC `TVmtMethodExTable`.
    FpcExMethodTable,
    /// Delphi 2010+ extended-RTTI method table.
    DelphiExtendedRtti,
}

/// Method kind. Ordinals mirror FPC's `TMethodKind`
/// (FPC `rtl/objpas/typinfo.pp:91-93`), which Delphi shares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MethodKind {
    /// `procedure`.
    Procedure,
    /// `function`.
    Function,
    /// `constructor`.
    Constructor,
    /// `destructor`.
    Destructor,
    /// `class procedure`.
    ClassProcedure,
    /// `class function`.
    ClassFunction,
    /// `class constructor`.
    ClassConstructor,
    /// `class destructor`.
    ClassDestructor,
    /// `operator` overload.
    OperatorOverload,
    /// An ordinal this parser doesn't recognise (carries the raw byte).
    Unknown(u8),
}

impl MethodKind {
    /// Maps a raw `TMethodKind` ordinal.
    pub fn from_u8(b: u8) -> Self {
        match b {
            0 => MethodKind::Procedure,
            1 => MethodKind::Function,
            2 => MethodKind::Constructor,
            3 => MethodKind::Destructor,
            4 => MethodKind::ClassProcedure,
            5 => MethodKind::ClassFunction,
            6 => MethodKind::ClassConstructor,
            7 => MethodKind::ClassDestructor,
            8 => MethodKind::OperatorOverload,
            other => MethodKind::Unknown(other),
        }
    }

    /// Stable lowercase label for display / persistence.
    pub fn as_str(self) -> &'static str {
        match self {
            MethodKind::Procedure => "procedure",
            MethodKind::Function => "function",
            MethodKind::Constructor => "constructor",
            MethodKind::Destructor => "destructor",
            MethodKind::ClassProcedure => "class procedure",
            MethodKind::ClassFunction => "class function",
            MethodKind::ClassConstructor => "class constructor",
            MethodKind::ClassDestructor => "class destructor",
            MethodKind::OperatorOverload => "operator",
            MethodKind::Unknown(_) => "unknown",
        }
    }
}

/// Calling convention. Ordinals mirror FPC's `TCallConv`
/// (FPC `rtl/objpas/typinfo.pp:104-106`), which Delphi
/// shares for the conventions it supports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallConv {
    /// `register` (Borland fastcall) - the Delphi default.
    Register,
    /// `cdecl`.
    Cdecl,
    /// `pascal`.
    Pascal,
    /// `stdcall`.
    StdCall,
    /// `safecall`.
    SafeCall,
    /// C++ `cppdecl`.
    CppDecl,
    /// 16-bit far call.
    Far16,
    /// Legacy FPC convention.
    OldFpcCall,
    /// Internal-proc convention.
    InternProc,
    /// `syscall`.
    SysCall,
    /// Software-float helper convention.
    SoftFloat,
    /// MetroWerks Pascal convention.
    MwPascal,
    /// An ordinal this parser doesn't recognise (carries the raw byte).
    Unknown(u8),
}

impl CallConv {
    /// Maps a raw `TCallConv` ordinal.
    pub fn from_u8(b: u8) -> Self {
        match b {
            0 => CallConv::Register,
            1 => CallConv::Cdecl,
            2 => CallConv::Pascal,
            3 => CallConv::StdCall,
            4 => CallConv::SafeCall,
            5 => CallConv::CppDecl,
            6 => CallConv::Far16,
            7 => CallConv::OldFpcCall,
            8 => CallConv::InternProc,
            9 => CallConv::SysCall,
            10 => CallConv::SoftFloat,
            11 => CallConv::MwPascal,
            other => CallConv::Unknown(other),
        }
    }

    /// Stable lowercase label for display / persistence.
    pub fn as_str(self) -> &'static str {
        match self {
            CallConv::Register => "register",
            CallConv::Cdecl => "cdecl",
            CallConv::Pascal => "pascal",
            CallConv::StdCall => "stdcall",
            CallConv::SafeCall => "safecall",
            CallConv::CppDecl => "cppdecl",
            CallConv::Far16 => "far16",
            CallConv::OldFpcCall => "oldfpccall",
            CallConv::InternProc => "internproc",
            CallConv::SysCall => "syscall",
            CallConv::SoftFloat => "softfloat",
            CallConv::MwPascal => "mwpascal",
            CallConv::Unknown(_) => "unknown",
        }
    }
}

/// How a parameter is passed, normalised across the Delphi and FPC flag
/// sets (`TParamFlag`), which differ in bit assignment and width.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamMode {
    /// Plain by-value parameter.
    Value,
    /// `var` (by-reference, read-write).
    Var,
    /// `const` (by-value or by-reference at the compiler's discretion).
    Const,
    /// `constref` (FPC: explicitly by-reference const).
    ConstRef,
    /// `out` (by-reference, write-only).
    Out,
}

impl ParamMode {
    /// Stable label for persistence and display: the Pascal keyword, and
    /// `value` for a by-value parameter.
    pub const fn as_str(self) -> &'static str {
        match self {
            ParamMode::Value => "value",
            ParamMode::Var => "var",
            ParamMode::Const => "const",
            ParamMode::ConstRef => "constref",
            ParamMode::Out => "out",
        }
    }

    /// Whether the argument is passed by reference: `var`, `out` and
    /// `constref`. A `const` parameter is by reference at the compiler's
    /// discretion, which the RTTI does not record.
    pub const fn by_ref(self) -> bool {
        matches!(self, ParamMode::Var | ParamMode::Out | ParamMode::ConstRef)
    }
}

/// Decoded `TParamFlag` set for one parameter.
///
/// The Delphi flag set is one byte; FPC's is a two-byte set with extra
/// flags (`pfConstRef`, `pfHidden`, `pfHigh`, `pfSelf`, `pfVmt`). The bit
/// meanings beyond the first six differ, so the raw value is interpreted
/// against the source flavor it was read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParamFlags {
    raw: u16,
    fpc: bool,
}

impl ParamFlags {
    // Bits shared between Delphi and FPC `TParamFlag`.
    const PF_VAR: u16 = 1 << 0;
    const PF_CONST: u16 = 1 << 1;
    const PF_OUT: u16 = 1 << 5;
    // FPC-only bits (Delphi reuses bit 6 for `pfResult`).
    const FPC_PF_CONSTREF: u16 = 1 << 6;
    const FPC_PF_HIDDEN: u16 = 1 << 7;
    const FPC_PF_HIGH: u16 = 1 << 8;
    const FPC_PF_SELF: u16 = 1 << 9;
    const FPC_PF_VMT: u16 = 1 << 10;
    const FPC_PF_RESULT: u16 = 1 << 11;
    // Delphi `pfResult` is bit 6.
    const DELPHI_PF_RESULT: u16 = 1 << 6;

    /// Constructs from a Delphi one-byte `TParamFlags`.
    pub fn from_delphi(byte: u8) -> Self {
        Self {
            raw: u16::from(byte),
            fpc: false,
        }
    }

    /// Constructs from an FPC two-byte `TParamFlags` set.
    pub fn from_fpc(set: u16) -> Self {
        Self {
            raw: set,
            fpc: true,
        }
    }

    /// The passing mode (`out` > `var` > `constref` > `const` > value).
    pub fn mode(self) -> ParamMode {
        if self.raw & Self::PF_OUT != 0 {
            ParamMode::Out
        } else if self.raw & Self::PF_VAR != 0 {
            ParamMode::Var
        } else if self.fpc && self.raw & Self::FPC_PF_CONSTREF != 0 {
            ParamMode::ConstRef
        } else if self.raw & Self::PF_CONST != 0 {
            ParamMode::Const
        } else {
            ParamMode::Value
        }
    }

    /// Whether the compiler flags this as a hidden parameter rather than one
    /// written in the source signature: on FPC `Self`, the VMT pointer, an
    /// open-array `High` bound, the function `Result` and other hidden
    /// parameters; on Delphi only `Result` (`pfResult`) - Delphi's implicit
    /// `Self` is the unflagged first parameter of a method that is not
    /// static ([`ExtendedMethodInfo::is_static`]). Consumers rendering a
    /// human-facing prototype usually skip these.
    pub fn is_hidden(self) -> bool {
        if self.fpc {
            self.raw
                & (Self::FPC_PF_HIDDEN
                    | Self::FPC_PF_HIGH
                    | Self::FPC_PF_SELF
                    | Self::FPC_PF_VMT
                    | Self::FPC_PF_RESULT)
                != 0
        } else {
            self.raw & Self::DELPHI_PF_RESULT != 0
        }
    }

    /// The raw flag bits, for consumers that need the exact set.
    pub fn raw(self) -> u16 {
        self.raw
    }
}

/// One parameter of a [`MethodSignature`].
#[derive(Debug, Clone)]
pub struct MethodParam<'a> {
    /// Parameter name, when the RTTI carried one.
    pub name: Option<&'a str>,
    /// Passing mode and raw flags.
    pub flags: ParamFlags,
    /// Resolved type name, when the `PPTypeInfo` pointer decoded.
    pub type_name: Option<&'a str>,
    /// The parameter's `PPTypeInfo` VA (`0` when absent).
    pub type_va: u64,
    /// The parameter's attributes (Delphi extended RTTI; empty otherwise).
    pub attributes: Vec<AttributeEntry<'a>>,
}

/// Extra metadata carried by Delphi extended-method-section entries
/// (absent from published-trailer entries).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExtendedMethodInfo {
    /// Raw entry flags word (`TVmtMethodExEntry.Flags`), decoded by the
    /// methods below.
    pub flags: u16,
    /// The VMT slot (counted from the class pointer; negative inside the
    /// header) for a [`DispatchKind::Virtual`] method, the dynamic index for
    /// a dynamic one; meaningless for a static method.
    pub vmt_index: i16,
}

/// How a method is called (`System.TypInfo.TDispatchKind`, the two bits an
/// extended method entry stores).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchKind {
    /// A direct call.
    Static,
    /// Through the VMT (`virtual`, `override`).
    Virtual,
    /// Through the dynamic table (`dynamic`).
    Dynamic,
    /// Through the dynamic table by message number (`message`).
    Message,
}

impl DispatchKind {
    /// Stable label for persistence and display.
    pub const fn as_str(self) -> &'static str {
        match self {
            DispatchKind::Static => "static",
            DispatchKind::Virtual => "virtual",
            DispatchKind::Dynamic => "dynamic",
            DispatchKind::Message => "message",
        }
    }
}

/// The bits of `TVmtMethodExEntry.Flags` (`System.Rtti`: `mfClassMethod`,
/// `mfHasSelf`, `mfSpecial`, the dispatch kind at bit 3, the visibility at
/// bit 5, `mfAbstract`).
const MF_CLASS_METHOD: u16 = 0x01;
const MF_HAS_SELF: u16 = 0x02;
const MF_SPECIAL: u16 = 0x04;
const MF_ABSTRACT: u16 = 0x80;

impl ExtendedMethodInfo {
    /// The declared visibility.
    pub fn visibility(&self) -> Visibility {
        Visibility::from_flags((self.flags >> 5) as u8)
    }

    /// How the method is called.
    pub fn dispatch(&self) -> DispatchKind {
        match (self.flags >> 3) & 0x03 {
            0 => DispatchKind::Static,
            1 => DispatchKind::Virtual,
            2 => DispatchKind::Dynamic,
            _ => DispatchKind::Message,
        }
    }

    /// Whether the method is `abstract`.
    pub fn is_abstract(&self) -> bool {
        self.flags & MF_ABSTRACT != 0
    }

    /// Whether the method has no `Self` (a `class ... static` method or a
    /// class operator).
    pub fn is_static(&self) -> bool {
        if self.flags & MF_SPECIAL != 0 {
            self.flags & 0x03 == 2
        } else {
            self.flags & MF_HAS_SELF == 0
        }
    }

    /// The method kind, given whether it returns a value (as
    /// `TRttiInstanceMethodEx.GetMethodKind` derives it).
    pub fn kind(&self, has_result: bool) -> MethodKind {
        if self.flags & MF_SPECIAL != 0 {
            return match self.flags & 0x03 {
                0 => MethodKind::Constructor,
                1 => MethodKind::Destructor,
                2 => MethodKind::OperatorOverload,
                _ if has_result => MethodKind::Function,
                _ => MethodKind::Procedure,
            };
        }
        let class = self.flags & MF_HAS_SELF == 0 || self.flags & MF_CLASS_METHOD != 0;
        match (class, has_result) {
            (true, true) => MethodKind::ClassFunction,
            (true, false) => MethodKind::ClassProcedure,
            (false, true) => MethodKind::Function,
            (false, false) => MethodKind::Procedure,
        }
    }
}

/// A decoded method signature: calling convention, ordered parameters, and
/// return type.
#[derive(Debug, Clone)]
pub struct MethodSignature<'a> {
    /// Method name, when available from the source table.
    pub name: Option<&'a str>,
    /// Procedure / function / constructor / …
    pub kind: MethodKind,
    /// Calling convention.
    pub call_conv: CallConv,
    /// Ordered formal parameters (including any hidden ones - filter with
    /// [`ParamFlags::is_hidden`]). For Delphi instance methods the first
    /// parameter is the implicit `Self` (passed `pfAddress`).
    pub params: Vec<MethodParam<'a>>,
    /// Resolved return-type name, when present (`None` for procedures).
    pub result_type_name: Option<&'a str>,
    /// Return type's `PPTypeInfo` VA (`0` for procedures).
    pub result_type_va: u64,
    /// Method code entry point VA, when the source table carries it.
    pub code_va: Option<u64>,
    /// Which on-disk structure this signature came from.
    pub source: SignatureSource,
    /// Extended-section metadata (flags, VMT index), when the signature came
    /// from a Delphi extended-method entry.
    pub extended: Option<ExtendedMethodInfo>,
    /// The method's attributes (Delphi extended RTTI; empty otherwise).
    pub attributes: Vec<AttributeEntry<'a>>,
}

/// Result of asking a class for its method signatures, with explicit era
/// signalling so consumers can tell "no signature RTTI present" from
/// "present but not yet decoded".
#[derive(Debug, Clone)]
pub enum SignatureReport<'a> {
    /// One or more signatures decoded.
    Decoded(Vec<MethodSignature<'a>>),
    /// The class carries no method-signature RTTI - classic / pre-2010
    /// Delphi, or FPC built without extended method RTTI. Not an error.
    Absent,
    /// Signature RTTI appears to be present but this parser does not decode
    /// its layout / compiler-version variant yet.
    Unsupported,
}

impl<'a> SignatureReport<'a> {
    /// The decoded signatures, or an empty slice for
    /// [`Absent`](Self::Absent) / [`Unsupported`](Self::Unsupported).
    pub fn decoded(&self) -> &[MethodSignature<'a>] {
        match self {
            SignatureReport::Decoded(v) => v,
            _ => &[],
        }
    }
}

impl<'a> MethodSignature<'a> {
    /// Whether the method has a `Self`: every method but a `class static`
    /// one or a class operator, which an extended entry flags
    /// ([`ExtendedMethodInfo::is_static`]). A published-table signature,
    /// which carries no such entry, is an instance method's.
    pub fn has_self(&self) -> bool {
        self.extended.as_ref().is_none_or(|info| !info.is_static())
    }

    /// The parameters the source declares: without the ones the compiler
    /// flags hidden ([`ParamFlags::is_hidden`]) and without Delphi's
    /// implicit `Self`, the unflagged first parameter of a method that has
    /// one ([`Self::has_self`]; FPC flags its `Self` hidden).
    pub fn declared_params(&self) -> impl Iterator<Item = &MethodParam<'a>> + '_ {
        let leading_self = self.has_self()
            && self
                .params
                .first()
                .is_some_and(|param| !param.flags.fpc && !param.flags.is_hidden());
        self.params
            .iter()
            .skip(usize::from(leading_self))
            .filter(|param| !param.flags.is_hidden())
    }

    /// Decodes a Delphi published-method-table entry trailer.
    ///
    /// `trailer` is the byte range an entry's `Size` field reserves beyond
    /// the bare `Size + CodeAddr + Name` record (see
    /// [`crate::methods::MethodEntry::trailer`]). `name` and `code_va` come
    /// from the surrounding [`crate::methods::MethodEntry`]. Returns `None`
    /// when the trailer is empty or too short to hold the fixed header.
    ///
    /// Layout (per IDR `IDCGen.cpp::OutputVmtMethodEntryTail`):
    ///
    /// ```text
    ///   Version:    u8
    ///   CC:         u8                 (TCallConv)
    ///   ResultType: ptr                (PPTypeInfo; 0 for procedures)
    ///   ParOff:     u16
    ///   ParamCount: u8
    ///   params[ParamCount]:
    ///     Flags:     u8                (Delphi TParamFlags)
    ///     ParamType: ptr               (PPTypeInfo)
    ///     ParOff:    u16
    ///     Name:      ShortString
    ///     AttrData:  u16-length-prefixed block (Len includes itself)
    ///   AttrData (method-level, same shape)
    /// ```
    pub fn from_published_trailer(
        ctx: &BinaryContext<'a>,
        name: &'a str,
        code_va: u64,
        trailer: &'a [u8],
        ptr_size: usize,
        flavor: VmtFlavor,
    ) -> Option<Self> {
        Self::decode_trailer(
            ctx,
            Some(name),
            Some(code_va),
            trailer,
            ptr_size,
            flavor,
            SignatureSource::PublishedTrailer,
            None,
        )
    }

    /// Decodes the signature carried by a Delphi extended-method-section
    /// entry ([`crate::methods::DelphiExtMethod`]). The entry's `trailer`
    /// has the same layout as the published trailer; this constructor tags
    /// the result [`SignatureSource::DelphiExtendedRtti`] and attaches the
    /// entry's flags / VMT index. Returns `None` for entries with no tail.
    pub fn from_delphi_extended(
        ctx: &BinaryContext<'a>,
        name: &'a str,
        code_va: u64,
        trailer: &'a [u8],
        ptr_size: usize,
        flavor: VmtFlavor,
        extended: ExtendedMethodInfo,
    ) -> Option<Self> {
        Self::decode_trailer(
            ctx,
            Some(name),
            Some(code_va),
            trailer,
            ptr_size,
            flavor,
            SignatureSource::DelphiExtendedRtti,
            Some(extended),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn decode_trailer(
        ctx: &BinaryContext<'a>,
        name: Option<&'a str>,
        code_va: Option<u64>,
        trailer: &'a [u8],
        ptr_size: usize,
        flavor: VmtFlavor,
        source: SignatureSource,
        extended: Option<ExtendedMethodInfo>,
    ) -> Option<Self> {
        // version(1) + cc(1) + result(ptr) + parOff(2) + paramCount(1).
        let header_len = 2usize.checked_add(ptr_size)?.checked_add(3)?;
        if trailer.len() < header_len {
            return None;
        }
        let _version = *trailer.first()?;
        let call_conv = CallConv::from_u8(*trailer.get(1)?);
        let result_type_va = read_ptr(trailer, 2, ptr_size)?;
        // parOff at 2 + ptr_size (skipped), param count just after it.
        let param_count = *trailer.get(2usize.checked_add(ptr_size)?.checked_add(2)?)?;

        let (result_type_name, result_type_va) =
            resolve_type(ctx, result_type_va, ptr_size, flavor);

        let mut params = Vec::with_capacity(usize::from(param_count));
        let mut cursor = header_len;
        for _ in 0..param_count {
            // flags(1) + type(ptr) + parOff(2) + name(shortstring) + attr.
            let flags = ParamFlags::from_delphi(*trailer.get(cursor)?);
            let type_off = cursor.checked_add(1)?;
            let param_type_va = read_ptr(trailer, type_off, ptr_size)?;
            let name_off = type_off.checked_add(ptr_size)?.checked_add(2)?;
            let pname = read_short_string_at_file(trailer, name_off)?;
            let (type_name, param_type_va) = resolve_type(ctx, param_type_va, ptr_size, flavor);
            // The name, then the parameter's AttrData block.
            let after_name = name_off.checked_add(1)?.checked_add(pname.len())?;
            let (attributes, next) = read_attr_data(trailer, after_name, ptr_size)?;
            params.push(MethodParam {
                name: bytes_to_name(pname),
                flags,
                type_name,
                type_va: param_type_va,
                attributes,
            });
            cursor = next;
        }
        // The method's own attributes follow its parameters.
        let attributes = read_attr_data(trailer, cursor, ptr_size)
            .map(|(a, _)| a)
            .unwrap_or_default();

        // The published trailer carries no kind: only a result type tells a
        // function from a procedure. An extended entry's flags give the
        // full kind.
        let kind = match extended {
            Some(info) => info.kind(result_type_va != 0),
            None if result_type_va != 0 => MethodKind::Function,
            None => MethodKind::Procedure,
        };

        Some(Self {
            name,
            kind,
            call_conv,
            params,
            result_type_name,
            result_type_va,
            code_va,
            source,
            extended,
            attributes,
        })
    }
}

/// Resolves a `PPTypeInfo` VA to a borrowed type name, returning the name (or
/// `None`) alongside the VA (normalised to `0` when null).
fn resolve_type<'a>(
    ctx: &BinaryContext<'a>,
    pptr_va: u64,
    ptr_size: usize,
    flavor: VmtFlavor,
) -> (Option<&'a str>, u64) {
    if pptr_va == 0 {
        return (None, 0);
    }
    let name = TypeHeader::from_pptr(ctx, pptr_va, ptr_size, flavor).map(|h| h.name());
    (name, pptr_va)
}

/// Converts a short-string body to a name, treating an empty body as `None`.
fn bytes_to_name(bytes: &[u8]) -> Option<&str> {
    if bytes.is_empty() {
        None
    } else {
        Some(str::from_utf8(bytes).unwrap_or("<non-ascii>"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats::BinaryContext;

    #[test]
    fn method_kind_and_callconv_roundtrip() {
        assert_eq!(MethodKind::from_u8(1), MethodKind::Function);
        assert_eq!(MethodKind::from_u8(2).as_str(), "constructor");
        assert_eq!(MethodKind::from_u8(200), MethodKind::Unknown(200));
        assert_eq!(CallConv::from_u8(0), CallConv::Register);
        assert_eq!(CallConv::from_u8(3).as_str(), "stdcall");
        assert_eq!(CallConv::from_u8(99), CallConv::Unknown(99));
    }

    #[test]
    fn delphi_param_flags_modes() {
        assert_eq!(ParamFlags::from_delphi(0).mode(), ParamMode::Value);
        assert_eq!(ParamFlags::from_delphi(0b1).mode(), ParamMode::Var);
        assert_eq!(ParamFlags::from_delphi(0b10).mode(), ParamMode::Const);
        assert_eq!(ParamFlags::from_delphi(0b10_0000).mode(), ParamMode::Out);
        // out beats var when both bits are set.
        assert_eq!(ParamFlags::from_delphi(0b10_0001).mode(), ParamMode::Out);
        // pfResult (bit 6) is hidden in Delphi.
        assert!(ParamFlags::from_delphi(0b100_0000).is_hidden());
        assert!(!ParamFlags::from_delphi(0b1).is_hidden());
    }

    #[test]
    fn fpc_param_flags_modes() {
        // constref is FPC bit 6; in Delphi that bit means pfResult.
        assert_eq!(ParamFlags::from_fpc(1 << 6).mode(), ParamMode::ConstRef);
        // hidden / self / vmt / high / result are all "hidden".
        assert!(ParamFlags::from_fpc(1 << 9).is_hidden()); // pfSelf
        assert!(ParamFlags::from_fpc(1 << 11).is_hidden()); // pfResult
        assert!(!ParamFlags::from_fpc(1 << 1).is_hidden()); // pfConst
    }

    #[test]
    fn report_decoded_accessor() {
        let empty: SignatureReport<'_> = SignatureReport::Absent;
        assert!(empty.decoded().is_empty());
        assert!(SignatureReport::Unsupported.decoded().is_empty());
    }

    /// Builds a 64-bit published-method trailer for two parameters with null
    /// (`0`) type pointers - so decoding walks the byte layout without
    /// needing a populated binary - and confirm the structure parses.
    #[test]
    fn published_trailer_decodes_byte_layout() {
        fn push_ptr0(v: &mut Vec<u8>) {
            v.extend_from_slice(&[0u8; 8]);
        }
        fn push_param(v: &mut Vec<u8>, flags: u8, name: &str) {
            v.push(flags);
            push_ptr0(v); // ParamType = 0
            v.extend_from_slice(&[0, 0]); // ParOff
            v.push(name.len() as u8);
            v.extend_from_slice(name.as_bytes());
            v.extend_from_slice(&[2, 0]); // AttrData: Len = 2 (no attributes)
        }

        let mut t = Vec::new();
        t.push(3); // Version
        t.push(3); // CC = stdcall
        push_ptr0(&mut t); // ResultType = 0 → procedure
        t.extend_from_slice(&[0, 0]); // ParOff
        t.push(2); // ParamCount
        push_param(&mut t, 0b1, "AValue"); // var
        push_param(&mut t, 0b10, "X"); // const

        // Empty binary: every type VA is 0, so resolution short-circuits and
        // never touches the (absent) image.
        let ctx = BinaryContext::new(&[]);
        let sig = MethodSignature::from_published_trailer(
            &ctx,
            "DoThing",
            0x401000,
            &t,
            8,
            VmtFlavor::Delphi,
        )
        .expect("trailer decodes");

        assert_eq!(sig.name, Some("DoThing"));
        assert_eq!(sig.call_conv, CallConv::StdCall);
        assert_eq!(sig.kind, MethodKind::Procedure);
        assert_eq!(sig.result_type_va, 0);
        assert_eq!(sig.code_va, Some(0x401000));
        assert_eq!(sig.params.len(), 2);
        assert_eq!(sig.params[0].name, Some("AValue"));
        assert_eq!(sig.params[0].flags.mode(), ParamMode::Var);
        assert_eq!(sig.params[1].name, Some("X"));
        assert_eq!(sig.params[1].flags.mode(), ParamMode::Const);
    }
}
