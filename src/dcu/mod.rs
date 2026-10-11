//! Delphi compiled units (`.dcu`) and compiled packages (`.dcp`).
//!
//! A DCU is the compiler's own record of a unit: every declaration of its
//! interface and implementation, the full layout of every type (records and
//! classes with every field and its offset, enums with their names, sets,
//! arrays, procedural types), every routine with its parameters, result,
//! calling convention and locals, the units it uses and the symbols it
//! imports from each, plus the machine code and RTTI of the unit with their
//! fixups. A runtime package's `.dcp` is a container of the DCUs of the
//! units it contains.
//!
//! This is what a binary's RTTI and export names cannot give: a record type
//! without managed fields has no RTTI, a plain routine's export name does
//! not carry its return type, and Delphi 6 RTTI omits private fields. The
//! DCUs shipped with a compiler fill those gaps for every runtime package
//! built from them.
//!
//! # Format
//!
//! The decoder is a port of the reading half of Alexei Hmelnov's DCU32INT
//! (version 1.26.1, zlib licence), which documents the
//! format for Delphi 2 - 10.3 Rio and Kylix 1 - 3, extended with the records
//! of the later releases (established on Delphi 13 and applied from 10.4,
//! see [`Version::DELPHI_10_4_SYDNEY`]: documentation comments
//! ([`Modifier::Documentation`]), type spellings ([`TypeSpelling`]), the
//! extra fields of generic instances, interface entries and exports - the
//! last two read and skipped - and the image base of a package unit). The
//! image is:
//!
//! 1. A [magic](Magic) naming the compiler and platform, the file
//!    size, a file time, a stamp and version-dependent header fields
//!    ([`Unit::flags`], [`Unit::priority`]).
//! 2. Source file records (`p` / `r` / `q` / `s` / `v` tags), the first
//!    numbered 0 naming the unit.
//! 3. Three uses lists: interface units (`d`), implementation units (`e`)
//!    and DLL imports (`h`), each unit followed by the types (`f` / `n`)
//!    and values (`g`) it imports and a `c` terminator.
//! 4. The declaration list: one tagged record per declaration, type
//!    definition, code block, fixup table or debug table, ended by `a`.
//!
//! Records cross-reference each other through two 1-based index spaces that
//! the reader rebuilds as it goes:
//!
//! - the **type table** ([`Unit::type_slot`]): imported types in import
//!   order, then every type definition in file order; a type declaration
//!   names a slot;
//! - the **address table** ([`Unit::addr`]): the unit itself, every imported
//!   unit, imported type and imported value, then every named declaration.
//!   Records can claim an address out of order (`drProcAddInfo`) or refer to
//!   one ahead of its declaration, so the table is filled as DCU32INT fills
//!   it, slot for slot.
//!
//! # Verification
//!
//! Every unit must parse to its final `a` tag at exactly the end of the
//! image; anything else is an [`Error`]. The tests run the decoder over the
//! Delphi 6 units committed with the C++Builder 6 fixtures and, when the
//! environment names the toolchains' installs, over every DCU of the
//! Delphi 6, 10 Seattle and 13 libraries, comparing types, fields and offsets with
//! DCU32INT's own output where it reaches. Units compiled by Delphi 13 from
//! our own sources pin the post-10.3 records.

mod addinfo;
mod digest;
pub mod itanium;
pub mod library;
pub mod locate;
pub mod mangle;
pub mod memory;
pub mod package;
mod parse;
mod reader;
mod records;
mod version;

use std::{error, fmt, ops::Range};

pub use self::version::{Magic, Platform, Version};

/// Index of a declaration in [`Unit::decls`].
pub type DeclId = usize;

/// Index of a type definition in [`Unit::type_defs`].
pub type TypeDefId = usize;

/// Why a DCU could not be decoded, and where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    /// Offset into the image where decoding stopped.
    pub offset: usize,
    /// What went wrong.
    pub kind: ErrorKind,
}

impl Error {
    /// Creates an error of `kind` at `offset`.
    #[must_use]
    pub fn new(offset: usize, kind: ErrorKind) -> Self {
        Self { offset, kind }
    }
}

/// The kinds of DCU decoding failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErrorKind {
    /// A record runs past the end of the image.
    Truncated,
    /// The magic is not a known DCU magic.
    UnknownMagic(u32),
    /// The size in the header differs from the image size.
    SizeMismatch {
        /// Size the header states.
        declared: u32,
        /// Size of the image.
        actual: usize,
    },
    /// A record ended with the wrong tag.
    UnexpectedTag {
        /// The structure being read.
        context: &'static str,
        /// The tag found.
        tag: u8,
    },
    /// A record violates a structural rule.
    Malformed(&'static str),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            ErrorKind::Truncated => write!(f, "truncated DCU at {:#x}", self.offset),
            ErrorKind::UnknownMagic(m) => write!(f, "unknown DCU magic {m:#010x}"),
            ErrorKind::SizeMismatch { declared, actual } => write!(
                f,
                "DCU header states {declared:#x} bytes, image has {actual:#x}"
            ),
            ErrorKind::UnexpectedTag { context, tag } => write!(
                f,
                "unexpected tag {tag:#04x} in {context} at {:#x}",
                self.offset
            ),
            ErrorKind::Malformed(what) => write!(f, "{what} at {:#x}", self.offset),
        }
    }
}

impl error::Error for Error {}

/// A source file the unit was compiled from (`$I` includes, `$L` objects
/// and `$R` resources included).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceFile {
    /// What the record names.
    pub kind: SourceKind,
    /// The file name as the compiler recorded it.
    pub name: String,
    /// The file's DOS date-time stamp (`0` or `-1` when not recorded).
    pub time: u32,
    /// The file number line tables refer to; `0` is the unit's own source.
    pub index: u32,
}

/// The kinds of [`SourceFile`] record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    /// A Pascal source or include file (`p`).
    Source,
    /// A linked object file, `{$L}` (`q`).
    Object,
    /// A linked resource file, `{$R}` (`r`).
    Resource,
    /// An assembler file (`s`, Delphi 5 debug units).
    Assembler,
    /// The source of inlined routines (`v`, Delphi 2010 and later).
    InlineSource,
    /// A .NET assembly (`t` / `\x9D`).
    Assembly,
}

/// Which uses list a unit import comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsesSection {
    /// The interface `uses` clause.
    Interface,
    /// The implementation `uses` clause.
    Implementation,
    /// A DLL the unit imports routines from (`external`).
    Dll,
}

/// A unit (or DLL) the unit refers to, with the symbols it imports from it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitImport {
    /// The unit or DLL name.
    pub name: String,
    /// Which list it is in.
    pub section: UsesSection,
    /// The used unit's stamp (or, for a DLL, an unused field).
    pub stamp: u32,
    /// The address-table index of the unit reference.
    pub addr: u32,
    /// The imported symbols, in file order.
    pub symbols: Vec<ImportedSymbol>,
}

/// A symbol imported from a used unit or DLL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedSymbol {
    /// The symbol's name as the exporting unit declares it.
    pub name: String,
    /// What kind of symbol it is.
    pub kind: ImportKind,
    /// The exporting declaration's stamp (for a DLL routine, its index).
    pub stamp: u32,
    /// The address-table index the import occupies.
    pub addr: u32,
    /// The type-table index the import occupies, for a type.
    pub type_index: Option<u32>,
}

/// The kinds of [`ImportedSymbol`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportKind {
    /// A type (`f`).
    Type,
    /// A type the unit redeclares as `type B = type A` (`n`), carrying
    /// RTTI of `rtti_size` bytes in this unit.
    TypeCopy {
        /// Size of the local RTTI.
        rtti_size: u32,
    },
    /// A value: routine, variable or constant (`g`).
    Value,
    /// A DLL routine (`g` in a DLL list).
    DllRoutine,
}

/// What a type-table slot holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeTarget {
    /// Nothing yet: a name without a definition (never in a fully read
    /// unit unless the slot is an unused gap).
    None,
    /// A type defined in this unit.
    Def(TypeDefId),
    /// A type imported from another unit: `uses[unit].symbols[symbol]`.
    Import {
        /// Index into [`Unit::uses`].
        unit: usize,
        /// Index into that import's [`UnitImport::symbols`].
        symbol: usize,
    },
}

/// One slot of the type table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeSlot {
    /// What the slot refers to.
    pub target: TypeTarget,
    /// The name a type declaration gave the slot (a type can be
    /// anonymous). Compiler-generated names start with `.` or, from Delphi
    /// 2009, `:`, or contain `` ` ``.
    pub name: Option<String>,
    /// The address-table index of the declaration that named the slot, or
    /// 0.
    pub decl: u32,
}

/// What an address-table slot holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddrTarget {
    /// A reserved slot nothing claimed.
    Empty,
    /// The unit's own reference (Delphi 7 and later), until the unit's
    /// initialization routine claims the slot.
    SelfUnit,
    /// A used unit: index into [`Unit::uses`].
    Unit(usize),
    /// An imported symbol: `uses[unit].symbols[symbol]`.
    Import {
        /// Index into [`Unit::uses`].
        unit: usize,
        /// Index into that import's [`UnitImport::symbols`].
        symbol: usize,
    },
    /// A declaration of this unit.
    Decl(DeclId),
}

/// The fields every "named, flagged" declaration carries (DCU32INT
/// `TNameFDecl`): types, variables, constants, routines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DeclFlags {
    /// The declaration flags. Bit `0x40` marks an interface (exported)
    /// declaration; Delphi 6 keeps `deprecated` / `platform` / `library`
    /// in bits 9 - 11.
    pub flags: u32,
    /// Second flag word (Delphi 8 and later).
    pub flags1: u32,
    /// Third flag word (Delphi 2009 and later).
    pub flags2: u32,
    /// The declaration's stamp, present on interface declarations: the
    /// value importing units record to detect a changed interface.
    pub stamp: u32,
    /// Index of the declaration's owner package, in a unit read from a
    /// package (Delphi 3 - 8).
    pub package_index: Option<u32>,
}

impl DeclFlags {
    /// Returns whether the declaration is in the unit's interface.
    #[must_use]
    pub fn is_interface(&self) -> bool {
        self.flags & 0x40 != 0
    }
}

/// The value of a constant declaration or attribute argument.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConstValue {
    /// The constant's type-table index.
    pub type_index: u32,
    /// What kind of value it is (Delphi 5 and later).
    pub kind: Option<ConstKind>,
    /// The value.
    pub data: ConstData,
}

/// The kinds of constant (DCU32INT `cv*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConstKind {
    /// An ordinal or pointer.
    Scalar,
    /// An AnsiString literal (`FF FF FF FF`, length, characters).
    String,
    /// A resource string.
    ResourceString,
    /// A floating-point value, as an `Extended` image. A constant of type
    /// `Currency` (Delphi types exact decimal literals so) stores the value
    /// times 10 000.
    Float,
    /// A set, or (XE2 and later) a nil pointer with no value.
    Set,
    /// A UnicodeString literal (Delphi 2009 and later).
    UnicodeString,
}

/// A constant's payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConstData {
    /// An integer (ordinals, enumeration members, pointers).
    Int(i64),
    /// The value's memory image (strings, floats, sets, records).
    Bytes(Vec<u8>),
}

/// How a routine passes its parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CallConv {
    /// Borland register (`EAX`, `EDX`, `ECX`), the default.
    #[default]
    Register,
    /// `cdecl`.
    Cdecl,
    /// `pascal`.
    Pascal,
    /// `stdcall`.
    Stdcall,
    /// `safecall`.
    Safecall,
}

/// A declaration of the unit, in any declaration list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decl {
    /// Offset of the record's tag in the image.
    pub offset: usize,
    /// The record tag, normalised to the Delphi 6 numbering (Delphi 2006
    /// moved `0x2D` - `0x36` up by one).
    pub tag: u8,
    /// The declared name (empty for some compiler-generated records).
    pub name: String,
    /// The address-table index of the declaration, or 0 for records that
    /// take none.
    pub addr: u32,
    /// The flags of a flagged declaration.
    pub flags: Option<DeclFlags>,
    /// The flags of the declaration's `drConstAddInfo` record (deprecation,
    /// inline, attributes, ...).
    pub add_info_flags: u32,
    /// What the declaration is.
    pub kind: DeclKind,
    /// Extra information attached by later records.
    pub modifiers: Vec<Modifier>,
}

impl Decl {
    /// Returns whether the declaration is in the unit's interface.
    #[must_use]
    pub fn is_interface(&self) -> bool {
        self.flags.is_some_and(|f| f.is_interface())
    }
}

/// A variable-like declaration's type and storage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VarInfo {
    /// The type-table index.
    pub type_index: u32,
    /// The offset in the data block (or, for `absolute`, the target's
    /// address index).
    pub offset: u32,
}

/// A local, parameter, field or result (DCU32INT `TLocalDecl`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Local {
    /// The raw flags. For a parameter, bit `0x02` clear means `const`
    /// (low bits `1` / `0`; `3` / `2` are by value), `0x08` in a register
    /// (`location` numbers it), `0x10` on the stack (`location` is the
    /// frame offset), `0x20` copied to a frame-local, `0x80` (with a `var`
    /// record) `out`; bit `0x40` (XE4 and later) marks a `[ref]` parameter.
    pub flags: u32,
    /// The visibility flags normalised to the Delphi 6 layout:
    /// `flags_x & 0x0E` is 0 private, 2 public, 4 protected, 0x0A
    /// published; bit 1 class member.
    pub flags_x: u32,
    /// The type-table index (for a method: signed, see [`Method`]).
    pub type_index: i32,
    /// For a field the byte offset; for a parameter or local its frame
    /// offset or register number; for a method the address index of its
    /// routine.
    pub location: i32,
    /// The member's dispatch index in an interface or dispinterface.
    pub interface_index: Option<u32>,
}

/// A method in a class, object, record or interface member list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Method {
    /// What kind of method.
    pub kind: MethodKind,
    /// The local-declaration fields: `local.location` is the address index
    /// of the implementing routine, `local.type_index` the VMT slot of a
    /// virtual method or the dynamic index of a dynamic one.
    pub local: Local,
    /// Whether the method belongs to an interface or dispinterface.
    pub in_interface: bool,
    /// An address index the method refers to: for a method implemented in
    /// a parent class, that parent's routine.
    pub import: u32,
}

/// The kinds of method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MethodKind {
    /// A procedure or function.
    Method,
    /// A constructor.
    Constructor,
    /// A destructor.
    Destructor,
}

/// A property (DCU32INT `TPropDecl`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Property {
    /// The raw flags.
    pub flags: i32,
    /// The visibility flags (see [`Local::flags_x`]).
    pub flags_x: u32,
    /// The type-table index; 0 for a redeclaration that only changes
    /// visibility or specifiers.
    pub type_index: u32,
    /// The property's dispatch index (or `MinInt` style sentinel).
    pub ndx: i32,
    /// The `index` specifier value.
    pub index: i32,
    /// Address index of the `read` accessor, or 0.
    pub read: u32,
    /// Address index of the `write` accessor, or 0.
    pub write: u32,
    /// Address index of the `stored` accessor, or 0.
    pub stored: u32,
    /// The accessor named in the source when the compiler substituted a
    /// thunk for `read` (Delphi 8 and later).
    pub read_orig: u32,
    /// The same for `write`.
    pub write_orig: u32,
    /// The `default` value.
    pub default: i32,
}

/// A routine (DCU32INT `TProcDecl`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proc {
    /// The routine flags (`B0`).
    pub proc_flags: u32,
    /// The size of the routine's code.
    pub size: u32,
    /// The virtual-method link (`VProc`).
    pub vproc: u32,
    /// The result type-table index (0 for a procedure).
    pub result_type: u32,
    /// The owning class's type index (Delphi 8 and later; 0 otherwise).
    pub class_type: u32,
    /// The calling convention.
    pub call: CallConv,
    /// Whether the record has no signature (compiler-generated blocks
    /// named `.`, `..`, `.N` or `$thunk`).
    pub unnamed: bool,
    /// Whether the routine is really a data block (Delphi 6 - 7 string
    /// literals and typed-constant images).
    pub just_data: bool,
    /// The parameters (`val` and `var` records), in order.
    pub params: Vec<DeclId>,
    /// Everything after the parameters: result, locals, nested types.
    pub locals: Vec<DeclId>,
    /// Nested routines and declarations read before the routine.
    pub embedded: Vec<DeclId>,
    /// Generic type parameters (Delphi 2009 and later).
    pub template_params: Vec<DeclId>,
}

/// What a [`Decl`] declares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeclKind {
    /// A type name (`*`): names type-table slot `type_index`.
    Type {
        /// The named type-table index.
        type_index: u32,
    },
    /// The VMT symbol of a class (`&`).
    Vmt(VarInfo),
    /// A true constant (`%`).
    Const(ConstValue),
    /// A resource string (`2`).
    ResourceString(VarInfo),
    /// A global variable (` `).
    Var(VarInfo),
    /// A thread variable (`1`).
    ThreadVar(VarInfo),
    /// A typed constant (`'`).
    TypedConst(VarInfo),
    /// A variable declared `absolute`; `offset` is the target's address
    /// index.
    AbsoluteVar(VarInfo),
    /// A compiler-allocated variable (`7`: class variable storage and the
    /// like).
    SpecialVar(VarInfo),
    /// A string-constant or data block (`5`, Delphi 8 and later).
    StrConst {
        /// Offset in the data block (`u32::MAX` when unset).
        offset: u32,
        /// Size of the block.
        size: u32,
    },
    /// A routine (`(`, and `)` from Delphi 8).
    Proc(Proc),
    /// A compiler-intrinsic routine before Delphi 8 (`)`).
    SysProc {
        /// The intrinsic's flags.
        flags: u32,
        /// The intrinsic's number.
        index: i32,
    },
    /// An `exports` entry (`i`).
    Export {
        /// The exported routine's address index.
        symbol: u32,
        /// The export ordinal.
        index: u32,
    },
    /// A label (`+`).
    Label {
        /// The label's code offset.
        offset: u32,
    },
    /// A parameter (`!` value, `"` var), result (`#`), field (`,`), local
    /// variable (` `) or absolute local (`$`); `tag` tells which.
    Local(Local),
    /// A method (`-` / `.` / `/`).
    Method(Method),
    /// A class variable (`6`, Delphi 2006 and later).
    ClassVar(Local),
    /// A property (`0`).
    Property(Property),
    /// A dispinterface property.
    DispProperty(Local),
    /// A default-value binding of a parameter (`\x9A`).
    SetDefault {
        /// The constant's address index.
        constant: u32,
        /// The parameter's address index.
        param: u32,
    },
    /// A declaration that repeats another one (`\xA3`).
    Copy {
        /// The copied declaration's address index.
        base: u32,
    },
    /// Unit-level extra information with its own declaration list (`4`).
    UnitAddInfo {
        /// A leading index.
        b: u32,
        /// The contained declarations.
        items: Vec<DeclId>,
    },
    /// A record with a declaration list (`o`, Delphi 2009 and later).
    ORec {
        /// A leading word.
        dw: u32,
        /// Two flag bytes.
        bytes: [u8; 2],
        /// The contained declarations.
        items: Vec<DeclId>,
    },
    /// A delay-loaded import (`\xB0`).
    DelayedImport {
        /// A leading word.
        inf: u32,
        /// The imported routine's address index.
        target: u32,
    },
    /// A generic parameter list that stands in a declaration list (`\xA6`).
    TemplateParams(Vec<DeclId>),
}

/// Information attached to a declaration or type by a later record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Modifier {
    /// `deprecated` with its message.
    Deprecated(String),
    /// A custom attribute (XE6 and later).
    Attribute(Attribute),
    /// The formal or actual generic parameters of a type or routine.
    TemplateParams {
        /// 0 for formal parameters, else the type index of the generic
        /// being instantiated.
        owner: u32,
        /// The parameters' type-table indices.
        params: Vec<u32>,
        /// Per parameter, how the source spells it (Delphi 10.4 and later;
        /// empty before).
        spellings: Vec<TypeSpelling>,
    },
    /// An `inline` routine's body, as DCU32INT's inline byte code.
    Inline(InlineBody),
    /// A .NET generated name.
    GeneratedName(String),
    /// .NET extra parameters.
    ExtraArgs(Vec<ExtraArg>),
    /// C++ header directives (`drCPPFlags`): `0x04` `$EXTERNALSYM`, `0x08`
    /// `$NODEFINE`, `0x10` `$NOINCLUDE`, `0x20` `$OBJTYPENAME`.
    CppFlags(u8),
    /// One line of the declaration's XML documentation comment (`///`),
    /// in source order (`drConstAddInfo` sub-record `0x08`, Delphi 10.4
    /// and later).
    Documentation(String),
    /// How the source spells a declaration's types (`drConstAddInfo`
    /// sub-record `0x17`, Delphi 10.4 and later).
    DeclaredType {
        /// The type of a variable, constant, field or parameter.
        value: TypeSpelling,
        /// The result type of a routine.
        result: TypeSpelling,
    },
    /// The names a declaration has in C++ (`drConstAddInfo` sub-record
    /// `0x13`, Delphi 2009 and later): `[$EXTERNALSYM, ?, $OBJTYPENAME]`,
    /// with two indices of unknown purpose.
    CppNames {
        /// First index.
        a: u32,
        /// Second index.
        b: u32,
        /// The three names.
        names: [String; 3],
    },
}

/// How the source spells a type, recorded where it differs from the type
/// the compiler resolved (Delphi 10.4 and later): `TMyInt` for a variable
/// of `type TMyInt = Integer`, `TBytes` for a parameter whose type resolves
/// to the instance `TArray<Byte>`. An empty spelling records nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TypeSpelling {
    /// The address of the type declaration or import the source names.
    pub address: Option<u32>,
    /// The type name the source uses, when no address identifies it (an
    /// alias of a generic instance), or its C++ spelling (see
    /// [`TypeSpelling::cpp_kind`]).
    pub name: Option<String>,
    /// The unit declaring [`TypeSpelling::name`].
    pub unit: Option<String>,
    /// Present when [`TypeSpelling::name`] is a C++ spelling: 4 is seen
    /// with dynamic-array typedefs (`System::TStringDynArray`), 8 with
    /// string and character types (`System::AnsiString`, `char *`).
    pub cpp_kind: Option<u32>,
}

/// A custom attribute applied to a declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attribute {
    /// The attribute constructor's address index.
    pub constructor: u32,
    /// The member the attribute applies to.
    pub member: u32,
    /// The attribute class's type-table index.
    pub attribute_type: u32,
    /// The constructor arguments.
    pub args: Vec<AttributeArg>,
}

/// An attribute constructor argument.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttributeArg {
    /// A constant.
    Const(ConstValue),
    /// `TypeInfo(T)`.
    TypeInfo {
        /// The type-table index.
        type_index: u32,
        /// The type's address index.
        type_addr: u32,
    },
}

/// The stored body of an `inline` routine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineBody {
    /// Where the inline byte code lies in the image.
    pub code: Range<usize>,
    /// The root node reference (`node count * 2`, or odd for a leaf).
    pub root: u32,
    /// The number of result variables.
    pub result_vars: u32,
    /// The addresses the body refers to.
    pub addrs: Vec<u32>,
}

/// A .NET extra parameter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtraArg {
    /// The parameter name.
    pub name: String,
    /// The parameter's type-table index.
    pub type_index: u32,
}

/// A type definition (DCU32INT `TTypeDef`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeDef {
    /// Offset of the record's tag in the image.
    pub offset: usize,
    /// The record tag.
    pub tag: u8,
    /// The type's own type-table index.
    pub type_index: u32,
    /// The size of the type's RTTI in the data block (0 when it has none).
    pub rtti_size: u32,
    /// The size of a variable of the type in bytes (`-1` when it has none,
    /// as for an open type).
    pub size: i32,
    /// The address index of the declaration that names the type - the
    /// `type T = ...` record itself, not a later alias (DCU32INT prints it
    /// as `[TN]`); 0 for an anonymous type.
    pub name_decl: u32,
    /// An extra index (Delphi 2005 and later).
    pub extra: u32,
    /// What the type is.
    pub kind: TypeKind,
    /// Extra information attached by later records.
    pub modifiers: Vec<Modifier>,
}

/// The flavours of subrange.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RangeKind {
    /// An integer subrange (`D`).
    Integer,
    /// A `Char` (AnsiChar) range (`B`).
    Char,
    /// A `Boolean`-like range (`A`).
    Boolean,
    /// A `WideChar` range (`Q`).
    WideChar,
    /// A wide integer range (`V`).
    Wide,
}

/// The floating-point kinds (`TFloatType` order).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FloatKind {
    /// 6-byte `Real48`.
    Real48,
    /// `Single`.
    Single,
    /// `Double`.
    Double,
    /// `Extended` (10 bytes on Win32, 8 on Win64).
    Extended,
    /// `Comp`.
    Comp,
    /// `Currency`.
    Currency,
}

/// The string type flavours.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StringKind {
    /// `AnsiString` (`R`).
    Ansi,
    /// `WideString` (`U`).
    Wide,
    /// `UnicodeString` (`[`, Delphi 2009 and later).
    Unicode,
}

/// What a [`TypeDef`] defines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeKind {
    /// A subrange or ordinal base type.
    Range {
        /// Which flavour.
        kind: RangeKind,
        /// The base type's index.
        base: u32,
        /// Lowest value.
        low: i64,
        /// Highest value.
        high: i64,
        /// A trailing flag.
        b: u32,
    },
    /// An enumeration.
    Enum {
        /// The enumeration this one is a subrange of: its own index for a
        /// root enumeration (`TBorderStyle = bsNone..bsSingle` names
        /// `TFormBorderStyle`).
        base: u32,
        /// The index of the ordinal range the values are stored as (0 for
        /// a subrange).
        storage: u32,
        /// Lowest value.
        low: i64,
        /// Highest value.
        high: i64,
        /// A trailing flag.
        b: u32,
        /// The member constants, ordered by value (filled after the unit
        /// is read; empty for a subrange of an enumeration).
        members: Vec<DeclId>,
    },
    /// A floating-point type.
    Float(FloatKind),
    /// A typed pointer.
    Pointer {
        /// The target type's index.
        target: u32,
    },
    /// `Text`.
    Text,
    /// `file of T`.
    File {
        /// The element type's index.
        element: u32,
    },
    /// `set of T`.
    Set {
        /// The 0-based number of the set's first byte.
        first_byte: u8,
        /// The element type's index.
        element: u32,
    },
    /// `string[N]`.
    ShortString {
        /// A leading flag.
        b1: u8,
        /// The index (length range) type.
        index_type: u32,
        /// The element type.
        element_type: u32,
        /// The code page (Delphi 2009 and later).
        code_page: u32,
    },
    /// A long string.
    String {
        /// Which flavour.
        kind: StringKind,
        /// A leading flag.
        b1: u8,
        /// The index type.
        index_type: u32,
        /// The element type.
        element_type: u32,
        /// The code page (Delphi 2009 and later; 0 for the default).
        code_page: u32,
    },
    /// `array[I] of E`.
    Array {
        /// A leading flag.
        b1: u8,
        /// The index type.
        index_type: u32,
        /// The element type.
        element_type: u32,
    },
    /// `Variant`.
    Variant {
        /// A flag byte.
        b: u8,
    },
    /// `class of T`.
    ClassRef {
        /// The class type's index.
        class_type: u32,
        /// The size of the class's VMT block.
        vmt_size: u32,
    },
    /// A record (or, with methods, an advanced record).
    Record {
        /// A flag byte.
        b2: u8,
        /// The members: fields, methods, properties, nested types.
        members: Vec<DeclId>,
    },
    /// A procedural type.
    ProcType {
        /// The flags; `flags & 0x10` is `of object`.
        flags: u32,
        /// The result type's index (0 for a procedure).
        result: u32,
        /// The calling convention.
        call: CallConv,
        /// The parameters, result and modifiers.
        params: Vec<DeclId>,
    },
    /// A Turbo Pascal `object`.
    Object {
        /// A flag byte.
        b03: u8,
        /// The parent's index.
        parent: u32,
        /// The VMT field's offset.
        vmt_offset: u32,
        /// The address index of the VMT declaration.
        vmt_decl: i32,
        /// The number of virtual methods.
        virtual_count: i32,
        /// The members.
        members: Vec<DeclId>,
    },
    /// A class (or, with `meta`, a .NET metaclass).
    Class(ClassDef),
    /// An interface or dispinterface.
    Interface {
        /// The parent interface's index.
        parent: u32,
        /// The number of methods including inherited ones.
        virtual_count: i32,
        /// The IID, as stored (`TGUID` layout).
        guid: [u8; 16],
        /// The flag byte; bit 2 marks a dispinterface.
        b: u8,
        /// The members.
        members: Vec<DeclId>,
    },
    /// An untyped block (`@`): typeless data, Win64 unwind data.
    Void,
    /// A dynamic array (Delphi 2009 and later, earlier a [`TypeKind::Pointer`]
    /// with an array target).
    DynArray {
        /// The element type's index.
        element: u32,
    },
    /// A generic type parameter.
    TemplateArg {
        /// Constraint type indices.
        constraints: Vec<u32>,
        /// A trailing index.
        v5: u32,
    },
    /// An instantiation of a generic.
    TemplateCall {
        /// The generic type's index.
        generic: u32,
        /// The type arguments.
        args: Vec<u32>,
        /// Per argument, how the source spells it (Delphi 10.4 and later;
        /// empty before).
        spellings: Vec<TypeSpelling>,
        /// The full instance type's index.
        full: u32,
    },
}

/// A class definition (DCU32INT `TClassDef`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassDef {
    /// The parent class's index (0 for `TObject` itself).
    pub parent: u32,
    /// The size of the instance RTTI.
    pub instance_rtti_size: u32,
    /// The size of the declared fields, the VMT pointer included, as the
    /// compiler records it. From Delphi 2009 every instance also ends in a
    /// hidden monitor slot that this size excludes; see
    /// [`Unit::instance_size`] for the size the class's VMT states.
    pub instance_size: i32,
    /// The address index of the VMT.
    pub vmt_addr: u32,
    /// The number of virtual methods, inherited ones included.
    pub virtual_count: u32,
    /// An index of unknown purpose (`NdxFE`).
    pub ndx_fe: u32,
    /// The number of published properties.
    pub property_count: u32,
    /// A flag field (`B04`).
    pub b04: u32,
    /// Implemented interfaces: `(interface type index, method count)`.
    pub interfaces: Vec<(u32, u32)>,
    /// The metaclass's class type index (.NET `TMetaClassDef`).
    pub meta: Option<u32>,
    /// The members: fields, methods, properties, nested types, class
    /// variables.
    pub members: Vec<DeclId>,
}

/// A fixup in the unit's code and data block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fixup {
    /// The offset in the data block.
    pub offset: u32,
    /// The fixup kind (version dependent; one kind marks the start of each
    /// declaration's block).
    pub kind: u8,
    /// The target's address index.
    pub target: u32,
}

/// A decoded DCU.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unit {
    /// The compiler release.
    pub version: Version,
    /// The target platform.
    pub platform: Platform,
    /// Whether the unit is .NET IL.
    pub msil: bool,
    /// Whether the unit was read out of a package (`.dcp`).
    pub from_package: bool,
    /// The unit name as declared: the name of the unit's initialization
    /// routine, which the compiler names after the `unit` clause (the main
    /// source file's name can differ in case, as Delphi 6's `classes.pas`
    /// does).
    pub name: String,
    /// The unit's namespace prefix as the header records it (Delphi 2005
    /// and later; `System` for `System.Types`).
    pub header_name: Option<String>,
    /// The file time in the header.
    pub file_time: u32,
    /// The unit stamp (Delphi 3 and later).
    pub stamp: u32,
    /// The unit flags.
    pub flags: u32,
    /// The second flag word (Delphi 2006 and later).
    pub flags1: u32,
    /// The initialization priority (Delphi 4 and later).
    pub priority: u32,
    /// The preferred image base of the package the unit was compiled
    /// into, as a package records it in each unit (read from 10.4; seen in
    /// Delphi 13 Win64 packages).
    pub image_base: Option<u64>,
    /// The source files.
    pub sources: Vec<SourceFile>,
    /// The used units and DLLs with their imports.
    pub uses: Vec<UnitImport>,
    /// Every declaration, in reading order.
    pub decls: Vec<Decl>,
    /// Every type definition, in reading order.
    pub type_defs: Vec<TypeDef>,
    /// The unit's top-level declaration list.
    pub main: Vec<DeclId>,
    /// Declarations read inside a nested-declaration block that no routine
    /// claimed (DCU32INT keeps them aside as well).
    pub unclaimed: Vec<DeclId>,
    /// Where the code and data block lies in the image.
    pub data_block: Option<Range<usize>>,
    /// The code and data block's bytes: every routine's machine code, typed
    /// constant and RTTI record, unrelocated (see [`memory`]).
    pub block: Vec<u8>,
    /// The fixups of the code and data block.
    pub fixups: Vec<Fixup>,
    types: Vec<TypeSlot>,
    addrs: Vec<AddrTarget>,
}

impl Unit {
    /// Decodes a DCU image.
    ///
    /// # Errors
    ///
    /// Returns an [`Error`] when the image is not a DCU of a known version,
    /// is truncated, or does not parse to its final tag at exactly the end
    /// of the image.
    pub fn parse(data: &[u8]) -> Result<Self, Error> {
        parse::Parser::run(data, false)
    }

    /// Decodes a DCU image embedded in a package (`.dcp`): a few records
    /// carry package-only fields, and a source-file list may be absent.
    ///
    /// # Errors
    ///
    /// As [`Unit::parse`].
    pub fn parse_from_package(data: &[u8]) -> Result<Self, Error> {
        parse::Parser::run(data, true)
    }

    /// Returns the size of a pointer on the unit's platform.
    #[must_use]
    pub fn pointer_size(&self) -> u8 {
        self.platform.pointer_size()
    }

    /// Returns the instance size of `class` as its VMT states it
    /// (`vmtInstanceSize`): the declared size plus, from Delphi 2009, the
    /// hidden `TMonitor` slot every object ends in (one pointer).
    ///
    /// Checked against the binaries: every class of RAD Studio 10 Seattle's
    /// runtime packages whose VMT and DCU both state a size differs by
    /// exactly one pointer (10 647 Win32, 10 540 Win64 classes); Delphi 6
    /// sizes are equal.
    #[must_use]
    pub fn instance_size(&self, class: &ClassDef) -> i32 {
        if self.version.delphi_at_least(Version::DELPHI_2009) {
            class
                .instance_size
                .saturating_add(i32::from(self.pointer_size()))
        } else {
            class.instance_size
        }
    }

    /// Returns the type-table slot at 1-based `index`.
    #[must_use]
    pub fn type_slot(&self, index: u32) -> Option<&TypeSlot> {
        let i = usize::try_from(index).ok()?.checked_sub(1)?;
        self.types.get(i)
    }

    /// Returns the number of type-table slots.
    #[must_use]
    pub fn type_count(&self) -> usize {
        self.types.len()
    }

    /// Returns the type defined in this unit at type-table `index`.
    #[must_use]
    pub fn type_def_at(&self, index: u32) -> Option<&TypeDef> {
        match self.type_slot(index)?.target {
            TypeTarget::Def(id) => self.type_defs.get(id),
            _ => None,
        }
    }

    /// Returns the name of the type at type-table `index`: the declared
    /// name of a local type, the imported name of an imported one.
    #[must_use]
    pub fn type_name(&self, index: u32) -> Option<&str> {
        let slot = self.type_slot(index)?;
        if let Some(name) = &slot.name {
            return Some(name);
        }
        match slot.target {
            TypeTarget::Import { unit, symbol } => self
                .uses
                .get(unit)?
                .symbols
                .get(symbol)
                .map(|s| s.name.as_str()),
            _ => None,
        }
    }

    /// Returns the unit that defines the type at type-table `index`: this
    /// unit's name for a local type, the used unit's for an imported one.
    #[must_use]
    pub fn type_unit(&self, index: u32) -> Option<&str> {
        match self.type_slot(index)?.target {
            TypeTarget::Import { unit, .. } => self.uses.get(unit).map(|u| u.name.as_str()),
            _ => Some(&self.name),
        }
    }

    /// Returns the address-table slot at 1-based `index`.
    #[must_use]
    pub fn addr(&self, index: u32) -> Option<AddrTarget> {
        let i = usize::try_from(index).ok()?.checked_sub(1)?;
        self.addrs.get(i).copied()
    }

    /// Returns the number of address-table slots.
    #[must_use]
    pub fn addr_count(&self) -> usize {
        self.addrs.len()
    }

    /// Returns the declaration at address-table `index`.
    #[must_use]
    pub fn decl_at(&self, index: u32) -> Option<&Decl> {
        match self.addr(index)? {
            AddrTarget::Decl(id) => self.decls.get(id),
            _ => None,
        }
    }

    /// Returns the name of whatever address-table `index` refers to.
    #[must_use]
    pub fn addr_name(&self, index: u32) -> Option<&str> {
        match self.addr(index)? {
            AddrTarget::Decl(id) => self.decls.get(id).map(|d| d.name.as_str()),
            AddrTarget::Import { unit, symbol } => self
                .uses
                .get(unit)?
                .symbols
                .get(symbol)
                .map(|s| s.name.as_str()),
            AddrTarget::Unit(unit) => self.uses.get(unit).map(|u| u.name.as_str()),
            AddrTarget::SelfUnit => Some(&self.name),
            AddrTarget::Empty => None,
        }
    }

    /// Returns the declarations of `ids`, skipping unknown ids.
    pub fn decls_of<'u>(&'u self, ids: &'u [DeclId]) -> impl Iterator<Item = &'u Decl> + 'u {
        ids.iter().filter_map(|&id| self.decls.get(id))
    }

    /// Returns the top-level declarations in the unit's interface.
    pub fn interface(&self) -> impl Iterator<Item = &Decl> + '_ {
        self.decls_of(&self.main).filter(|d| d.is_interface())
    }
}

/// Decodes a DCU name: UTF-8 when it is valid UTF-8 (Delphi 2009 and later
/// write identifiers as UTF-8), otherwise Latin-1.
#[must_use]
pub fn decode_name(bytes: &[u8]) -> String {
    match str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => bytes.iter().map(|&b| char::from(b)).collect(),
    }
}
