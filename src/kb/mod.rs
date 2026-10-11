//! IDR knowledge bases (`kb*.bin`, `syskb*.bin`).
//!
//! The Interactive Delphi Reconstructor identifies library code and names
//! its types from a knowledge base per Delphi release: for every unit of the
//! release's library, its constants, types, variables, resource strings and
//! routines as the compiler's DCU records them. A routine carries its code
//! with the bytes its relocations patch masked out and each relocation's
//! target named, so the routine can be recognised in any binary that links
//! it; types carry their declaration, size, fields, properties and methods.
//! The published files cover Delphi 2 - 7 and 2005 - XE6 (`kb2` ... `kb2014`
//! with their `syskb` subsets for `System` / `SysInit`) on Win32 and XE2 -
//! XE6 (`syskb2012` ... `syskb2014`) on Win64.
//!
//! [`KnowledgeBase`] reads a file without copying; [`Contents`] holds every
//! record and writes the format back ([`Contents::to_bytes`]), byte for byte
//! as IDR wrote it. [`build`] makes the records from a release's DCUs as
//! IDR's maker made them, so knowledge bases can be produced for releases
//! IDR never covered.
//!
//! # Format
//!
//! Every integer is little-endian. A string is a `u16` length, that many
//! bytes in the compiler's ANSI code page, and a NUL.
//!
//! ```text
//!   header (309 bytes)
//!     signature    [24]  "IDR Knowledge Base File" (version 2) or
//!                        "IDD Knowledge Base File" (version 1), NUL-padded
//!     is_msil      u8
//!     delphi_ver   i32
//!     crc          u32
//!     description  [256] NUL-padded
//!     kb_version   u32   1 or 2
//!     reserved     [16]
//!   records: modules, constants, types, variables, resource strings,
//!            routines, each section's records back to back
//!   section tables, in the same order, each:
//!     count        i32
//!     max_size     i32   the largest record of the section
//!     entries[count] { offset u32, size u32, by_module i32, by_name i32 }
//!   tables_offset  u32   file offset of the first section table
//! ```
//!
//! `by_name` lists the section's records ordered by their uppercased name
//! (IDR searches it with `stricmp`) and `by_module` grouped by module ID,
//! which IDR searches for a module's range; within a module the files order
//! by uppercased name too, except the Delphi 2 ones' routines, which keep
//! file order. [`Contents::to_bytes`] writes (module ID, uppercased name). Each kind of record is
//! described with its type ([`Module`], [`Const`], [`Type`], [`Var`],
//! [`ResString`], [`Proc`]).
//!
//! The records follow DCU32INT's view of a unit: type expressions and
//! constant values are its printed forms (`array[$0..-$1] of
//! System.TVarRec`), argument flags and locations are the DCU's, and a
//! routine's or type's [`Dump`] is its block of the unit's code or data.
//!
//! Source: IDR's `KnowledgeBase.cpp` (`MKnowledgeBase::Open`,
//! `CheckKBFile`, the `Get*Info` readers) and `TypeInfo.cpp` (fields,
//! properties, methods); every published file parses to its last byte.

use std::{borrow::Cow, cmp::Ordering, error, fmt, iter, marker::PhantomData, ops::Range, str};

mod build;
mod float;
mod write;

pub use self::{
    build::{UnitFile, build},
    write::WriteError,
};

/// Size of the fixed header, reserved bytes included.
const HEADER_SIZE: usize = 309;
/// Offset of the reserved bytes that end the header.
const RESERVED_OFFSET: usize = 293;
/// Length of the signature field.
const SIGNATURE_LEN: usize = 24;
/// Length of the description field.
const DESCRIPTION_LEN: usize = 256;
/// Size of a section table entry.
const ENTRY_SIZE: usize = 16;
/// Signature of a version 2 file.
const SIGNATURE_IDR: &[u8] = b"IDR Knowledge Base File";
/// Signature of a version 1 file.
const SIGNATURE_IDD: &[u8] = b"IDD Knowledge Base File";

/// Why a knowledge base could not be read, and where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    /// File offset where reading stopped.
    pub offset: usize,
    /// What went wrong.
    pub kind: ErrorKind,
}

/// The kinds of knowledge-base reading failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErrorKind {
    /// A structure runs past the end of the file or of its record.
    Truncated,
    /// The signature and version are not a known pair.
    UnknownFormat,
    /// A structure violates a rule of the format.
    Malformed(&'static str),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            ErrorKind::Truncated => write!(f, "truncated knowledge base at {:#x}", self.offset),
            ErrorKind::UnknownFormat => f.write_str("not an IDR knowledge base"),
            ErrorKind::Malformed(what) => write!(f, "{what} at {:#x}", self.offset),
        }
    }
}

impl error::Error for Error {}

/// A string of a knowledge base: bytes in the ANSI code page of the
/// compiler the records come from.
///
/// [`Display`](fmt::Display) maps each byte to the Unicode code point of
/// the same value (Latin-1), which is exact for the ASCII identifiers that
/// make up nearly every string.
#[derive(Clone, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Text<'a>(pub Cow<'a, [u8]>);

impl<'a> Text<'a> {
    /// Returns the raw bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Returns the string when it is valid UTF-8 (always so for ASCII).
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        str::from_utf8(&self.0).ok()
    }

    /// Returns whether the string is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Compares with `other` ignoring ASCII case, the order of the name
    /// indexes.
    #[must_use]
    pub fn cmp_ignore_case(&self, other: &[u8]) -> Ordering {
        cmp_ignore_case(&self.0, other)
    }

    /// Returns a copy that owns its bytes.
    #[must_use]
    pub fn into_owned(self) -> Text<'static> {
        Text(Cow::Owned(self.0.into_owned()))
    }
}

impl fmt::Display for Text<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0
            .iter()
            .try_for_each(|&b| fmt::Write::write_char(f, char::from(b)))
    }
}

impl fmt::Debug for Text<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.to_string())
    }
}

impl<'a> From<&'a str> for Text<'a> {
    fn from(s: &'a str) -> Self {
        Self(Cow::Borrowed(s.as_bytes()))
    }
}

impl From<String> for Text<'_> {
    fn from(s: String) -> Self {
        Self(Cow::Owned(s.into_bytes()))
    }
}

impl<'a> From<&'a [u8]> for Text<'a> {
    fn from(b: &'a [u8]) -> Self {
        Self(Cow::Borrowed(b))
    }
}

impl From<Vec<u8>> for Text<'_> {
    fn from(b: Vec<u8>) -> Self {
        Self(Cow::Owned(b))
    }
}

/// Compares two names the way IDR's `stricmp` orders them: byte by byte
/// after ASCII uppercasing.
#[must_use]
pub fn cmp_ignore_case(a: &[u8], b: &[u8]) -> Ordering {
    a.iter()
        .map(u8::to_ascii_uppercase)
        .cmp(b.iter().map(u8::to_ascii_uppercase))
}

/// Defines a byte-coded field: named values plus an `Other` arm, so every
/// byte converts both ways unchanged.
macro_rules! byte_enum {
    (
        $(#[$meta:meta])*
        $name:ident { $($(#[$vmeta:meta])* $variant:ident = $value:expr),+ $(,)? }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum $name {
            $($(#[$vmeta])* $variant,)+
            /// A value outside the documented set, kept as stored.
            Other(u8),
        }

        impl From<u8> for $name {
            fn from(b: u8) -> Self {
                match b {
                    $(v if v == $value => Self::$variant,)+
                    other => Self::Other(other),
                }
            }
        }

        impl From<$name> for u8 {
            fn from(v: $name) -> u8 {
                match v {
                    $($name::$variant => $value,)+
                    $name::Other(other) => other,
                }
            }
        }
    };
}

byte_enum! {
    /// How a [`Fixup`] patches its bytes.
    FixupKind {
        /// The absolute address of the target (`'A'`).
        Address = b'A',
        /// A relative jump or call to the target (`'J'`).
        Jump = b'J',
        /// The address of a data target (`'D'`).
        Data = b'D',
    }
}

byte_enum! {
    /// What a [`Const`] declares.
    ConstKind {
        /// A true constant (`'C'`).
        Const = b'C',
        /// A VMT or other compiler-generated data declaration (`'P'`).
        Vmt = b'P',
        /// A typed constant (`'V'`).
        TypedConst = b'V',
    }
}

byte_enum! {
    /// The kind of a [`Type`], DCU32INT's type-definition tag.
    TypeKind {
        /// An array (`drArrayDef`, `'L'`).
        Array = b'L',
        /// A class (`drClassDef`, `'F'`).
        Class = b'F',
        /// A typed or untyped file (`drFileDef`, `'O'`).
        File = b'O',
        /// A floating-point type (`drFloatDef`, `'I'`).
        Float = b'I',
        /// An interface (`drInterfaceDef`, `'T'`).
        Interface = b'T',
        /// An old-style object (`drObjVMTDef`, `'G'`).
        Object = b'G',
        /// A procedural type (`drProcTypeDef`, `'H'`).
        ProcType = b'H',
        /// A pointer (`drPtrDef`, `'E'`).
        Pointer = b'E',
        /// An ordinal subrange or enumeration (`drRangeDef`, `'D'`).
        Range = b'D',
        /// A record (`drRecDef`, `'M'`).
        Record = b'M',
        /// A set (`drSetDef`, `'J'`).
        Set = b'J',
        /// A short string (`drShortStrDef`, `'K'`).
        ShortString = b'K',
        /// A long string (`drStringDef`, `'R'`).
        String = b'R',
        /// A text file (`drTextDef`, `'P'`).
        Text = b'P',
        /// A variant (`drVariantDef`, `'S'`).
        Variant = b'S',
        /// A declaration that renames another type (`'Z'`).
        Alias = b'Z',
    }
}

byte_enum! {
    /// What a [`Var`] declares.
    VarKind {
        /// A global variable (`'V'`).
        Var = b'V',
        /// An `absolute` variable (`'A'`).
        Absolute = b'A',
        /// A compiler-defined variable (`'S'`).
        Special = b'S',
        /// A thread variable (`'T'`).
        Thread = b'T',
    }
}

byte_enum! {
    /// The kind of a routine or method.
    ProcKind {
        /// A method without a more specific kind (`'M'`).
        Method = b'M',
        /// A procedure (`'P'`).
        Procedure = b'P',
        /// A function (`'F'`).
        Function = b'F',
        /// A constructor (`'C'`).
        Constructor = b'C',
        /// A destructor (`'D'`).
        Destructor = b'D',
    }
}

byte_enum! {
    /// The calling convention of a [`Proc`].
    CallKind {
        /// `register`, the default.
        Register = 0,
        /// `cdecl`.
        Cdecl = 1,
        /// `pascal`.
        Pascal = 2,
        /// `stdcall`.
        Stdcall = 3,
        /// `safecall`.
        Safecall = 4,
    }
}

byte_enum! {
    /// Whether a [`Proc`]'s dump is code or data.
    DumpType {
        /// Machine code (`'C'`).
        Code = b'C',
        /// Data (`'D'`).
        Data = b'D',
    }
}

byte_enum! {
    /// The visibility of a field, property or method.
    Scope {
        /// `private`.
        Private = 9,
        /// `protected`.
        Protected = 10,
        /// `public`.
        Public = 11,
        /// `published`.
        Published = 12,
    }
}

byte_enum! {
    /// How an [`Arg`] is passed.
    ArgTag {
        /// By value or `const` (DCU `arg`, `0x21`).
        Val = 0x21,
        /// By reference: `var` or `out` (DCU `var`, `0x22`).
        Var = 0x22,
    }
}

/// Which signature and record layout a file uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Format {
    /// Version 1, `"IDD Knowledge Base File"`: type records lack their
    /// size.
    Idd,
    /// Version 2, `"IDR Knowledge Base File"`: every published file.
    Idr,
}

impl Format {
    /// Returns the version number the header stores.
    #[must_use]
    pub fn version(self) -> u32 {
        match self {
            Self::Idd => 1,
            Self::Idr => 2,
        }
    }

    /// Returns the signature the header starts with.
    #[must_use]
    pub fn signature(self) -> &'static [u8] {
        match self {
            Self::Idd => SIGNATURE_IDD,
            Self::Idr => SIGNATURE_IDR,
        }
    }
}

/// The fixed header of a knowledge base.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header<'a> {
    /// The signature and layout.
    pub format: Format,
    /// Whether the records describe .NET (Delphi for .NET) units.
    pub is_msil: bool,
    /// The Delphi version the file declares; `0` in nearly every published
    /// file.
    pub delphi_version: i32,
    /// A checksum field; `0xFFFFFFFF` in every published file.
    pub crc: u32,
    /// The 256-byte description field without its trailing NULs: empty in
    /// every published file except the Delphi 2 ones, whose field holds
    /// uninitialized memory.
    pub description: Text<'a>,
}

impl Default for Header<'_> {
    fn default() -> Self {
        Self {
            format: Format::Idr,
            is_msil: false,
            delphi_version: 0,
            crc: u32::MAX,
            description: Text::default(),
        }
    }
}

/// A unit and the units it uses.
///
/// ```text
///   id         u16
///   name       string
///   file_name  string
///   uses_count u16
///   use_ids    u16[uses_count]
///   use_names  string[uses_count]
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Module<'a> {
    /// The module ID records refer to; the module's position in the
    /// section in every published file.
    pub id: u16,
    /// The unit name.
    pub name: Text<'a>,
    /// The file the unit was read from (`SysUtils.dcu`).
    pub file_name: Text<'a>,
    /// The units and DLLs it uses.
    pub uses: Vec<Use<'a>>,
}

/// An entry of [`Module::uses`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Use<'a> {
    /// The used unit's module ID, `0xFFFF` for a DLL or a unit the file
    /// does not contain.
    pub id: u16,
    /// The unit or DLL name.
    pub name: Text<'a>,
}

/// A relocation in a [`Dump`]: the bytes at `offset` hold the address of
/// the declaration `name`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fixup<'a> {
    /// How the bytes encode the target.
    pub kind: FixupKind,
    /// Offset of the patched bytes in the dump.
    pub offset: u32,
    /// The target declaration's name; the record's own name for a
    /// reference into its own dump.
    pub name: Text<'a>,
}

/// A declaration's bytes and relocations.
///
/// ```text
///   total       u32   size of what follows
///   size        u32
///   fixup_count u32
///   code        [size]
///   relocs      [size]  0xFF where a relocation patches the byte
///   fixups[fixup_count] { kind u8, offset u32, name string }
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Dump<'a> {
    /// The bytes as the unit holds them, relocations unpatched.
    pub code: Cow<'a, [u8]>,
    /// One byte per code byte: `0xFF` where a relocation patches it, `0`
    /// elsewhere.
    pub relocs: Cow<'a, [u8]>,
    /// The relocations.
    pub fixups: Vec<Fixup<'a>>,
    /// The relocation count as stored: `fixups.len()`, except that the
    /// Delphi 2 and 3 files record counts for some empty dumps without any
    /// relocation data.
    pub fixup_count: u32,
}

impl<'a> Dump<'a> {
    /// Creates a dump from its bytes, relocation mask and relocations.
    ///
    /// # Errors
    ///
    /// Returns [`WriteError::TooLarge`] when there are more than
    /// `u32::MAX` relocations.
    pub fn new(
        code: Cow<'a, [u8]>,
        relocs: Cow<'a, [u8]>,
        fixups: Vec<Fixup<'a>>,
    ) -> Result<Self, WriteError> {
        let fixup_count =
            u32::try_from(fixups.len()).map_err(|_| WriteError::TooLarge("fixup count"))?;
        Ok(Self {
            code,
            relocs,
            fixups,
            fixup_count,
        })
    }

    /// Returns whether the dump holds no bytes.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.code.is_empty()
    }

    /// Returns whether `bytes` starts with this dump, ignoring the bytes a
    /// relocation patches: the test IDR's `ScanCode` applies at each
    /// candidate address.
    #[must_use]
    pub fn matches(&self, bytes: &[u8]) -> bool {
        let Some(candidate) = bytes.get(..self.code.len()) else {
            return false;
        };
        self.code
            .iter()
            .zip(candidate)
            .zip(self.relocs.iter().chain(iter::repeat(&0)))
            .all(|((&want, &got), &mask)| mask == 0xFF || want == got)
    }
}

/// A constant (`ConstDataTable`).
///
/// ```text
///   module    u16
///   name      string
///   kind      u8      ConstKind
///   type_def  string
///   value     string
///   dump      Dump
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Const<'a> {
    /// The declaring module's ID.
    pub module: u16,
    /// The constant's name.
    pub name: Text<'a>,
    /// What it declares.
    pub kind: ConstKind,
    /// Its type expression; empty for an untyped constant.
    pub type_def: Text<'a>,
    /// Its value, as DCU32INT prints it.
    pub value: Text<'a>,
    /// Its bytes in the unit's data (typed constants, VMTs).
    pub dump: Dump<'a>,
}

/// A type (`TypeDataTable`).
///
/// ```text
///   size       u32      version 2 only
///   module     u16
///   name       string
///   kind       u8       TypeKind
///   vmt_count  u16
///   decl       string
///   dump       Dump
///   fields     { total u32, count u16, Field[count] }
///   properties { total u32, count u16, Property[count] }
///   methods    { total u32, count u16, Method[count] }
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Type<'a> {
    /// The size of a value of the type (an instance pointer for a class);
    /// a version 1 file stores none and this is the record's size.
    pub size: u32,
    /// The declaring module's ID.
    pub module: u16,
    /// The type's name.
    pub name: Text<'a>,
    /// The kind of definition.
    pub kind: TypeKind,
    /// The number of virtual methods a class or object declares.
    pub vmt_count: u16,
    /// The declaration's right-hand side, as DCU32INT prints it
    /// (`class(TPersistent)`, `(alNone,alTop)`).
    pub decl: Text<'a>,
    /// The type's RTTI and VMT bytes in the unit's data.
    pub dump: Dump<'a>,
    /// The fields of a class, object or record.
    pub fields: Vec<Field<'a>>,
    /// The properties of a class or interface.
    pub properties: Vec<Property<'a>>,
    /// The methods of a class, object or interface.
    pub methods: Vec<Method<'a>>,
}

/// A field of a [`Type`].
///
/// ```text
///   scope    u8
///   offset   i32
///   case     i32   variant-part case, -1 outside a variant part
///   name     string
///   type_def string
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field<'a> {
    /// Its visibility.
    pub scope: Scope,
    /// Its byte offset in an instance.
    pub offset: i32,
    /// The case of the record's variant part it belongs to, `-1` outside
    /// one.
    pub case: i32,
    /// Its name.
    pub name: Text<'a>,
    /// Its type expression.
    pub type_def: Text<'a>,
}

/// A property of a [`Type`].
///
/// ```text
///   scope    u8
///   index    i32
///   disp_id  i32
///   name, type_def, read, write, stored   string
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Property<'a> {
    /// Its visibility.
    pub scope: Scope,
    /// Its `index` specifier, `-1` without one; IDR reads bits `2` / `4`
    /// as read-only / write-only.
    pub index: i32,
    /// Its dispatch ID.
    pub disp_id: i32,
    /// Its name.
    pub name: Text<'a>,
    /// Its type expression.
    pub type_def: Text<'a>,
    /// The field or method it reads.
    pub read: Text<'a>,
    /// The field or method it writes.
    pub write: Text<'a>,
    /// Its `stored` specifier.
    pub stored: Text<'a>,
}

/// A method of a [`Type`].
///
/// ```text
///   scope     u8
///   kind      u8   ProcKind
///   prototype string
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Method<'a> {
    /// Its visibility.
    pub scope: Scope,
    /// Its kind.
    pub kind: ProcKind,
    /// Its prototype (`function Get(val Self:TList;val
    /// Index:System.Integer):System.Pointer`).
    pub prototype: Text<'a>,
}

/// A variable (`VarDataTable`).
///
/// ```text
///   module    u16
///   name      string
///   kind      u8   VarKind
///   type_def  string
///   abs_name  string
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Var<'a> {
    /// The declaring module's ID.
    pub module: u16,
    /// The variable's name.
    pub name: Text<'a>,
    /// What it declares.
    pub kind: VarKind,
    /// Its type expression.
    pub type_def: Text<'a>,
    /// The variable an `absolute` declaration overlays.
    pub abs_name: Text<'a>,
}

/// A resource string (`ResStrDataTable`).
///
/// ```text
///   module    u16
///   name      string
///   type_def  string
///   context   string
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResString<'a> {
    /// The declaring module's ID.
    pub module: u16,
    /// The resource string's name.
    pub name: Text<'a>,
    /// Its type expression.
    pub type_def: Text<'a>,
    /// Its text.
    pub context: Text<'a>,
}

/// A routine (`ProcDataTable`).
///
/// ```text
///   module     u16
///   name       string   Class.Method for a method
///   embedded   u8       has nested routines
///   dump_type  u8       DumpType
///   kind       u8       ProcKind
///   call       u8       CallKind
///   vproc      i32      DCU routine flags
///   type_def   string   a function's result type
///   dump       Dump
///   args       { total u32, count u16, Arg[count] }
///   locals     { total u32, ... }   Delphi 2 files only
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proc<'a> {
    /// The declaring module's ID.
    pub module: u16,
    /// The routine's name; `Class.Method` for a method.
    pub name: Text<'a>,
    /// Whether it contains nested routines.
    pub embedded: bool,
    /// Whether the dump is code or data.
    pub dump_type: DumpType,
    /// Its kind.
    pub kind: ProcKind,
    /// Its calling convention.
    pub call: CallKind,
    /// The DCU routine flags (DCU32INT `VProc`); bit `0x1000` marks an
    /// `overload`.
    pub vproc: i32,
    /// A function's result type.
    pub type_def: Text<'a>,
    /// Its code.
    pub dump: Dump<'a>,
    /// Its parameters, `Self` and hidden ones included.
    pub args: Vec<Arg<'a>>,
    /// The body of the locals section only the Delphi 2 files have (a
    /// `u16` count, `0` in every one); `None` without the section.
    pub locals: Option<Cow<'a, [u8]>>,
}

/// A parameter of a [`Proc`].
///
/// ```text
///   tag      u8    ArgTag
///   flags    i32   DCU parameter flags
///   location i32
///   name     string
///   type_def string
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Arg<'a> {
    /// How it is passed.
    pub tag: ArgTag,
    /// The DCU parameter flags (see [`crate::dcu::Local::flags`]): `0x08`
    /// in a register, `0x10` on the stack.
    pub flags: i32,
    /// The register number (`0` EAX, `1` EDX, `2` ECX) or stack offset.
    pub location: i32,
    /// Its name.
    pub name: Text<'a>,
    /// Its type expression (`void ` for an untyped parameter).
    pub type_def: Text<'a>,
}

/// A record kind of a knowledge base: decoded from and encoded to its
/// section's bytes.
pub trait Record<'a>: Sized {
    /// Decodes one record from exactly its bytes.
    ///
    /// # Errors
    ///
    /// Returns an error when the bytes do not form one record; offsets are
    /// relative to the record.
    fn decode(bytes: &'a [u8], format: Format) -> Result<Self, Error>;

    /// Appends the record's bytes to `out`.
    ///
    /// # Errors
    ///
    /// Returns an error when a string or list exceeds its length field.
    fn encode(&self, out: &mut Vec<u8>, format: Format) -> Result<(), WriteError>;

    /// Returns the record's name.
    fn name(&self) -> &Text<'a>;

    /// Returns the ID of the module the record belongs to (a module's own
    /// ID for a [`Module`]).
    fn module(&self) -> u16;

    /// Reads the name from the record's bytes without decoding the rest.
    fn name_of(bytes: &'a [u8], format: Format) -> Option<&'a [u8]>;

    /// Reads the module ID from the record's bytes without decoding the
    /// rest.
    fn module_of(bytes: &'a [u8], format: Format) -> Option<u16>;
}

/// A bounds-checked cursor over one record or structure.
struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
    /// File offset of `data`, for error offsets.
    base: usize,
}

impl<'a> Cursor<'a> {
    fn new(data: &'a [u8], base: usize) -> Self {
        Self { data, pos: 0, base }
    }

    fn error(&self, kind: ErrorKind) -> Error {
        Error {
            offset: self.base.saturating_add(self.pos),
            kind,
        }
    }

    fn bytes(&mut self, n: usize) -> Result<&'a [u8], Error> {
        let end = self
            .pos
            .checked_add(n)
            .ok_or_else(|| self.error(ErrorKind::Truncated))?;
        let out = self
            .data
            .get(self.pos..end)
            .ok_or_else(|| self.error(ErrorKind::Truncated))?;
        self.pos = end;
        Ok(out)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], Error> {
        let bytes = self.bytes(N)?;
        bytes
            .try_into()
            .map_err(|_| self.error(ErrorKind::Truncated))
    }

    fn u8(&mut self) -> Result<u8, Error> {
        self.array::<1>().map(|[b]| b)
    }

    fn u16(&mut self) -> Result<u16, Error> {
        self.array().map(u16::from_le_bytes)
    }

    fn u32(&mut self) -> Result<u32, Error> {
        self.array().map(u32::from_le_bytes)
    }

    fn i32(&mut self) -> Result<i32, Error> {
        self.array().map(i32::from_le_bytes)
    }

    fn text(&mut self) -> Result<Text<'a>, Error> {
        let len = usize::from(self.u16()?);
        let body = self.bytes(len)?;
        if self.u8()? != 0 {
            return Err(self.error(ErrorKind::Malformed("string not NUL-terminated")));
        }
        Ok(Text(Cow::Borrowed(body)))
    }

    /// Splits off a block of `len` bytes as its own cursor.
    fn block(&mut self, len: usize) -> Result<Self, Error> {
        let base = self.base.saturating_add(self.pos);
        let data = self.bytes(len)?;
        Ok(Self::new(data, base))
    }

    /// Splits off a block whose `u32` length comes first.
    fn sized_block(&mut self) -> Result<Self, Error> {
        let len = usize::try_from(self.u32()?)
            .map_err(|_| self.error(ErrorKind::Malformed("block size")))?;
        self.block(len)
    }

    fn rest(&mut self) -> &'a [u8] {
        let out = self.data.get(self.pos..).unwrap_or_default();
        self.pos = self.data.len();
        out
    }

    fn at_end(&self) -> bool {
        self.pos >= self.data.len()
    }

    fn expect_end(&self, what: &'static str) -> Result<(), Error> {
        if self.at_end() {
            Ok(())
        } else {
            Err(self.error(ErrorKind::Malformed(what)))
        }
    }

    /// Reads a counted list block: `u32` total, `u16` count, the items.
    fn list<T>(
        &mut self,
        what: &'static str,
        mut item: impl FnMut(&mut Self) -> Result<T, Error>,
    ) -> Result<Vec<T>, Error> {
        let mut block = self.sized_block()?;
        let count = block.u16()?;
        let mut out = Vec::with_capacity(usize::from(count).min(block.data.len()));
        for _ in 0..count {
            out.push(item(&mut block)?);
        }
        block.expect_end(what)?;
        Ok(out)
    }

    fn dump(&mut self) -> Result<Dump<'a>, Error> {
        let mut block = self.sized_block()?;
        let size = usize::try_from(block.u32()?)
            .map_err(|_| block.error(ErrorKind::Malformed("dump size")))?;
        let fixup_count = block.u32()?;
        let code = block.bytes(size)?;
        let relocs = block.bytes(size)?;
        let mut fixups = Vec::new();
        // The Delphi 2 and 3 files give some empty dumps a count but no
        // relocation data; only the block size is reliable.
        while !block.at_end() && u32::try_from(fixups.len()).is_ok_and(|n| n < fixup_count) {
            let kind = FixupKind::from(block.u8()?);
            let offset = block.u32()?;
            let name = block.text()?;
            fixups.push(Fixup { kind, offset, name });
        }
        block.expect_end("dump size")?;
        Ok(Dump {
            code: Cow::Borrowed(code),
            relocs: Cow::Borrowed(relocs),
            fixups,
            fixup_count,
        })
    }
}

/// Reads a record's leading `u16` module ID and the string after it.
fn leading_name(bytes: &[u8], skip: usize) -> Option<&[u8]> {
    let at = skip.checked_add(2)?;
    let len = usize::from(u16::from_le_bytes(
        bytes.get(at..at.checked_add(2)?)?.try_into().ok()?,
    ));
    let start = at.checked_add(2)?;
    bytes.get(start..start.checked_add(len)?)
}

/// Reads the `u16` at `at`.
fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        bytes.get(at..at.checked_add(2)?)?.try_into().ok()?,
    ))
}

impl<'a> Record<'a> for Module<'a> {
    fn decode(bytes: &'a [u8], _format: Format) -> Result<Self, Error> {
        let mut c = Cursor::new(bytes, 0);
        let id = c.u16()?;
        let name = c.text()?;
        let file_name = c.text()?;
        let count = usize::from(c.u16()?);
        let mut ids = Vec::with_capacity(count.min(bytes.len()));
        for _ in 0..count {
            ids.push(c.u16()?);
        }
        let mut uses = Vec::with_capacity(ids.len());
        for id in ids {
            uses.push(Use {
                id,
                name: c.text()?,
            });
        }
        c.expect_end("module record size")?;
        Ok(Self {
            id,
            name,
            file_name,
            uses,
        })
    }

    fn encode(&self, out: &mut Vec<u8>, _format: Format) -> Result<(), WriteError> {
        write::put_u16(out, self.id);
        write::put_text(out, &self.name)?;
        write::put_text(out, &self.file_name)?;
        write::put_count(out, self.uses.len(), "uses list")?;
        for u in &self.uses {
            write::put_u16(out, u.id);
        }
        for u in &self.uses {
            write::put_text(out, &u.name)?;
        }
        Ok(())
    }

    fn name(&self) -> &Text<'a> {
        &self.name
    }

    fn module(&self) -> u16 {
        self.id
    }

    fn name_of(bytes: &'a [u8], _format: Format) -> Option<&'a [u8]> {
        leading_name(bytes, 0)
    }

    fn module_of(bytes: &'a [u8], _format: Format) -> Option<u16> {
        u16_at(bytes, 0)
    }
}

impl<'a> Record<'a> for Const<'a> {
    fn decode(bytes: &'a [u8], _format: Format) -> Result<Self, Error> {
        let mut c = Cursor::new(bytes, 0);
        let out = Self {
            module: c.u16()?,
            name: c.text()?,
            kind: ConstKind::from(c.u8()?),
            type_def: c.text()?,
            value: c.text()?,
            dump: c.dump()?,
        };
        c.expect_end("constant record size")?;
        Ok(out)
    }

    fn encode(&self, out: &mut Vec<u8>, _format: Format) -> Result<(), WriteError> {
        write::put_u16(out, self.module);
        write::put_text(out, &self.name)?;
        out.push(self.kind.into());
        write::put_text(out, &self.type_def)?;
        write::put_text(out, &self.value)?;
        write::put_dump(out, &self.dump)
    }

    fn name(&self) -> &Text<'a> {
        &self.name
    }

    fn module(&self) -> u16 {
        self.module
    }

    fn name_of(bytes: &'a [u8], _format: Format) -> Option<&'a [u8]> {
        leading_name(bytes, 0)
    }

    fn module_of(bytes: &'a [u8], _format: Format) -> Option<u16> {
        u16_at(bytes, 0)
    }
}

/// Size of the leading `size` field of a version 2 type record.
fn type_prefix(format: Format) -> usize {
    match format {
        Format::Idd => 0,
        Format::Idr => 4,
    }
}

impl<'a> Record<'a> for Type<'a> {
    fn decode(bytes: &'a [u8], format: Format) -> Result<Self, Error> {
        let mut c = Cursor::new(bytes, 0);
        let size = match format {
            Format::Idr => c.u32()?,
            // IDR's reader takes the record size for version 1.
            Format::Idd => u32::try_from(bytes.len())
                .map_err(|_| c.error(ErrorKind::Malformed("type record size")))?,
        };
        let out = Self {
            size,
            module: c.u16()?,
            name: c.text()?,
            kind: TypeKind::from(c.u8()?),
            vmt_count: c.u16()?,
            decl: c.text()?,
            dump: c.dump()?,
            fields: c.list("field list size", |c| {
                Ok(Field {
                    scope: Scope::from(c.u8()?),
                    offset: c.i32()?,
                    case: c.i32()?,
                    name: c.text()?,
                    type_def: c.text()?,
                })
            })?,
            properties: c.list("property list size", |c| {
                Ok(Property {
                    scope: Scope::from(c.u8()?),
                    index: c.i32()?,
                    disp_id: c.i32()?,
                    name: c.text()?,
                    type_def: c.text()?,
                    read: c.text()?,
                    write: c.text()?,
                    stored: c.text()?,
                })
            })?,
            methods: c.list("method list size", |c| {
                Ok(Method {
                    scope: Scope::from(c.u8()?),
                    kind: ProcKind::from(c.u8()?),
                    prototype: c.text()?,
                })
            })?,
        };
        c.expect_end("type record size")?;
        Ok(out)
    }

    fn encode(&self, out: &mut Vec<u8>, format: Format) -> Result<(), WriteError> {
        if format == Format::Idr {
            write::put_u32(out, self.size);
        }
        write::put_u16(out, self.module);
        write::put_text(out, &self.name)?;
        out.push(self.kind.into());
        write::put_u16(out, self.vmt_count);
        write::put_text(out, &self.decl)?;
        write::put_dump(out, &self.dump)?;
        write::put_list(out, &self.fields, "field list", |out, f| {
            out.push(f.scope.into());
            write::put_i32(out, f.offset);
            write::put_i32(out, f.case);
            write::put_text(out, &f.name)?;
            write::put_text(out, &f.type_def)
        })?;
        write::put_list(out, &self.properties, "property list", |out, p| {
            out.push(p.scope.into());
            write::put_i32(out, p.index);
            write::put_i32(out, p.disp_id);
            write::put_text(out, &p.name)?;
            write::put_text(out, &p.type_def)?;
            write::put_text(out, &p.read)?;
            write::put_text(out, &p.write)?;
            write::put_text(out, &p.stored)
        })?;
        write::put_list(out, &self.methods, "method list", |out, m| {
            out.push(m.scope.into());
            out.push(m.kind.into());
            write::put_text(out, &m.prototype)
        })
    }

    fn name(&self) -> &Text<'a> {
        &self.name
    }

    fn module(&self) -> u16 {
        self.module
    }

    fn name_of(bytes: &'a [u8], format: Format) -> Option<&'a [u8]> {
        leading_name(bytes, type_prefix(format))
    }

    fn module_of(bytes: &'a [u8], format: Format) -> Option<u16> {
        u16_at(bytes, type_prefix(format))
    }
}

impl<'a> Record<'a> for Var<'a> {
    fn decode(bytes: &'a [u8], _format: Format) -> Result<Self, Error> {
        let mut c = Cursor::new(bytes, 0);
        let out = Self {
            module: c.u16()?,
            name: c.text()?,
            kind: VarKind::from(c.u8()?),
            type_def: c.text()?,
            abs_name: c.text()?,
        };
        c.expect_end("variable record size")?;
        Ok(out)
    }

    fn encode(&self, out: &mut Vec<u8>, _format: Format) -> Result<(), WriteError> {
        write::put_u16(out, self.module);
        write::put_text(out, &self.name)?;
        out.push(self.kind.into());
        write::put_text(out, &self.type_def)?;
        write::put_text(out, &self.abs_name)
    }

    fn name(&self) -> &Text<'a> {
        &self.name
    }

    fn module(&self) -> u16 {
        self.module
    }

    fn name_of(bytes: &'a [u8], _format: Format) -> Option<&'a [u8]> {
        leading_name(bytes, 0)
    }

    fn module_of(bytes: &'a [u8], _format: Format) -> Option<u16> {
        u16_at(bytes, 0)
    }
}

impl<'a> Record<'a> for ResString<'a> {
    fn decode(bytes: &'a [u8], _format: Format) -> Result<Self, Error> {
        let mut c = Cursor::new(bytes, 0);
        let out = Self {
            module: c.u16()?,
            name: c.text()?,
            type_def: c.text()?,
            context: c.text()?,
        };
        c.expect_end("resource string record size")?;
        Ok(out)
    }

    fn encode(&self, out: &mut Vec<u8>, _format: Format) -> Result<(), WriteError> {
        write::put_u16(out, self.module);
        write::put_text(out, &self.name)?;
        write::put_text(out, &self.type_def)?;
        write::put_text(out, &self.context)
    }

    fn name(&self) -> &Text<'a> {
        &self.name
    }

    fn module(&self) -> u16 {
        self.module
    }

    fn name_of(bytes: &'a [u8], _format: Format) -> Option<&'a [u8]> {
        leading_name(bytes, 0)
    }

    fn module_of(bytes: &'a [u8], _format: Format) -> Option<u16> {
        u16_at(bytes, 0)
    }
}

impl<'a> Record<'a> for Proc<'a> {
    fn decode(bytes: &'a [u8], _format: Format) -> Result<Self, Error> {
        let mut c = Cursor::new(bytes, 0);
        let module = c.u16()?;
        let name = c.text()?;
        let embedded = c.u8()? != 0;
        let dump_type = DumpType::from(c.u8()?);
        let kind = ProcKind::from(c.u8()?);
        let call = CallKind::from(c.u8()?);
        let vproc = c.i32()?;
        let type_def = c.text()?;
        let dump = c.dump()?;
        let args = c.list("argument list size", |c| {
            Ok(Arg {
                tag: ArgTag::from(c.u8()?),
                flags: c.i32()?,
                location: c.i32()?,
                name: c.text()?,
                type_def: c.text()?,
            })
        })?;
        let locals = if c.at_end() {
            None
        } else {
            let mut block = c.sized_block()?;
            Some(Cow::Borrowed(block.rest()))
        };
        c.expect_end("routine record size")?;
        Ok(Self {
            module,
            name,
            embedded,
            dump_type,
            kind,
            call,
            vproc,
            type_def,
            dump,
            args,
            locals,
        })
    }

    fn encode(&self, out: &mut Vec<u8>, _format: Format) -> Result<(), WriteError> {
        write::put_u16(out, self.module);
        write::put_text(out, &self.name)?;
        out.push(u8::from(self.embedded));
        out.push(self.dump_type.into());
        out.push(self.kind.into());
        out.push(self.call.into());
        write::put_i32(out, self.vproc);
        write::put_text(out, &self.type_def)?;
        write::put_dump(out, &self.dump)?;
        write::put_list(out, &self.args, "argument list", |out, a| {
            out.push(a.tag.into());
            write::put_i32(out, a.flags);
            write::put_i32(out, a.location);
            write::put_text(out, &a.name)?;
            write::put_text(out, &a.type_def)
        })?;
        if let Some(locals) = &self.locals {
            write::put_block(out, locals)?;
        }
        Ok(())
    }

    fn name(&self) -> &Text<'a> {
        &self.name
    }

    fn module(&self) -> u16 {
        self.module
    }

    fn name_of(bytes: &'a [u8], _format: Format) -> Option<&'a [u8]> {
        leading_name(bytes, 0)
    }

    fn module_of(bytes: &'a [u8], _format: Format) -> Option<u16> {
        u16_at(bytes, 0)
    }
}

/// One entry of a section table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Entry {
    /// The record's file offset.
    offset: u32,
    /// The record's size.
    size: u32,
    /// The record at this position of the (module, name) order.
    by_module: u32,
    /// The record at this position of the name order.
    by_name: u32,
}

/// A section's table as read from the file.
#[derive(Debug, Clone, Default)]
struct Table {
    max_size: u32,
    entries: Vec<Entry>,
}

/// One section of a [`KnowledgeBase`]: its records in file order, plus the
/// two orders its table keeps.
#[derive(Debug, Clone, Copy)]
pub struct Section<'k, 'a, R> {
    kb: &'k KnowledgeBase<'a>,
    table: &'k Table,
    marker: PhantomData<fn() -> R>,
}

impl<'k, 'a, R: Record<'a>> Section<'k, 'a, R> {
    /// Returns the number of records.
    #[must_use]
    pub fn len(&self) -> usize {
        self.table.entries.len()
    }

    /// Returns whether the section has no records.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.table.entries.is_empty()
    }

    /// Returns the size of the largest record, as the table states it.
    #[must_use]
    pub fn max_size(&self) -> u32 {
        self.table.max_size
    }

    /// Returns the bytes of record `index`.
    #[must_use]
    pub fn bytes(&self, index: usize) -> Option<&'a [u8]> {
        let entry = self.table.entries.get(index)?;
        self.kb.record_bytes(*entry)
    }

    /// Decodes record `index`.
    ///
    /// # Errors
    ///
    /// Returns an error when the index is out of range or the record is
    /// malformed; the offset is the file offset.
    pub fn get(&self, index: usize) -> Result<R, Error> {
        let entry = self.table.entries.get(index).ok_or(Error {
            offset: 0,
            kind: ErrorKind::Malformed("record index out of range"),
        })?;
        let bytes = self.kb.record_bytes(*entry).ok_or(Error {
            offset: entry.offset as usize,
            kind: ErrorKind::Truncated,
        })?;
        R::decode(bytes, self.kb.header.format).map_err(|e| Error {
            offset: e.offset.saturating_add(entry.offset as usize),
            kind: e.kind,
        })
    }

    /// Decodes every record in file order.
    pub fn iter(&self) -> impl Iterator<Item = Result<R, Error>> + '_ {
        (0..self.len()).map(|i| self.get(i))
    }

    /// Returns the record indices in name order (uppercased name).
    pub fn name_order(&self) -> impl Iterator<Item = usize> + '_ {
        self.table.entries.iter().map(|e| e.by_name as usize)
    }

    /// Returns the record indices grouped by module ID (in ascending
    /// order; see the [module docs](self) for the order within a module).
    pub fn module_order(&self) -> impl Iterator<Item = usize> + '_ {
        self.table.entries.iter().map(|e| e.by_module as usize)
    }

    /// Returns the indices of the records named `name` (ignoring ASCII
    /// case), found by binary search of the name order.
    #[must_use]
    pub fn named(&self, name: &[u8]) -> Vec<usize> {
        let format = self.kb.header.format;
        let key = |pos: usize| -> Option<(usize, &'a [u8])> {
            let index = self.table.entries.get(pos)?.by_name as usize;
            Some((index, R::name_of(self.bytes(index)?, format)?))
        };
        let range = self.equal_range(|pos| key(pos).map(|(_, n)| cmp_ignore_case(n, name)));
        range.filter_map(|pos| key(pos).map(|(i, _)| i)).collect()
    }

    /// Returns the indices of the records of module `module`, in the
    /// file's order within a module, found by binary search of the module
    /// order.
    #[must_use]
    pub fn of_module(&self, module: u16) -> Vec<usize> {
        let format = self.kb.header.format;
        let key = |pos: usize| -> Option<(usize, u16)> {
            let index = self.table.entries.get(pos)?.by_module as usize;
            Some((index, R::module_of(self.bytes(index)?, format)?))
        };
        let range = self.equal_range(|pos| key(pos).map(|(_, m)| m.cmp(&module)));
        range.filter_map(|pos| key(pos).map(|(i, _)| i)).collect()
    }

    /// Returns the positions whose key compares equal, given the ordering
    /// of the key at each position against the target (`None` for an
    /// unreadable entry, which ends the search).
    fn equal_range(&self, cmp: impl Fn(usize) -> Option<Ordering>) -> Range<usize> {
        let partition = |pred: &dyn Fn(Ordering) -> bool| {
            let (mut lo, mut hi) = (0usize, self.len());
            while lo < hi {
                let mid = lo.midpoint(hi);
                match cmp(mid) {
                    Some(o) if pred(o) => lo = mid.saturating_add(1),
                    Some(_) => hi = mid,
                    None => return None,
                }
            }
            Some(lo)
        };
        let start = partition(&|o| o == Ordering::Less);
        let end = partition(&|o| o != Ordering::Greater);
        match (start, end) {
            (Some(start), Some(end)) if start <= end => start..end,
            _ => 0..0,
        }
    }
}

/// An IDR knowledge base read from its bytes; records are decoded on
/// demand and borrow from the input.
///
/// # Examples
///
/// ```no_run
/// use undelphi::kb::KnowledgeBase;
///
/// let data = std::fs::read("kb6.bin")?;
/// let kb = KnowledgeBase::parse(&data)?;
/// for index in kb.procs().named(b"IntToStr") {
///     let proc = kb.procs().get(index)?;
///     println!("{} ({} bytes)", proc.name, proc.dump.code.len());
/// }
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone)]
pub struct KnowledgeBase<'a> {
    data: &'a [u8],
    header: Header<'a>,
    modules: Table,
    consts: Table,
    types: Table,
    vars: Table,
    res_strings: Table,
    procs: Table,
}

impl<'a> KnowledgeBase<'a> {
    /// Reads the header and section tables of a knowledge base.
    ///
    /// # Errors
    ///
    /// Returns an error when the signature is unknown or a table does not
    /// fit the file. Records are checked only when decoded.
    pub fn parse(data: &'a [u8]) -> Result<Self, Error> {
        let mut c = Cursor::new(data, 0);
        let signature = c.bytes(SIGNATURE_LEN)?;
        let is_msil = c.u8()? != 0;
        let delphi_version = c.i32()?;
        let crc = c.u32()?;
        let description = c.bytes(DESCRIPTION_LEN)?;
        let version = c.u32()?;
        let format = [Format::Idd, Format::Idr]
            .into_iter()
            .find(|f| f.version() == version && nul_terminated(signature) == f.signature())
            .ok_or(Error {
                offset: 0,
                kind: ErrorKind::UnknownFormat,
            })?;
        let header = Header {
            format,
            is_msil,
            delphi_version,
            crc,
            description: Text(Cow::Borrowed(trim_trailing_nuls(description))),
        };

        let trailer = data.len().checked_sub(4).ok_or(Error {
            offset: 0,
            kind: ErrorKind::Truncated,
        })?;
        let mut t = Cursor::new(data, 0);
        t.pos = trailer;
        let tables = usize::try_from(t.u32()?).map_err(|_| Error {
            offset: trailer,
            kind: ErrorKind::Malformed("table offset"),
        })?;
        let mut c = Cursor::new(data.get(..trailer).unwrap_or_default(), 0);
        c.pos = tables;
        let mut table = || -> Result<Table, Error> {
            let count = usize::try_from(c.i32()?)
                .map_err(|_| c.error(ErrorKind::Malformed("negative record count")))?;
            let max_size = c.u32()?;
            let mut block = c.block(
                count
                    .checked_mul(ENTRY_SIZE)
                    .ok_or_else(|| c.error(ErrorKind::Malformed("record count")))?,
            )?;
            let mut entries = Vec::with_capacity(count);
            for _ in 0..count {
                entries.push(Entry {
                    offset: block.u32()?,
                    size: block.u32()?,
                    by_module: block.u32()?,
                    by_name: block.u32()?,
                });
            }
            Ok(Table { max_size, entries })
        };
        let kb = Self {
            data,
            header,
            modules: table()?,
            consts: table()?,
            types: table()?,
            vars: table()?,
            res_strings: table()?,
            procs: table()?,
        };
        if !c.at_end() {
            return Err(c.error(ErrorKind::Malformed("data after the section tables")));
        }
        for t in kb.tables() {
            let n = t.entries.len();
            for e in &t.entries {
                if e.by_module as usize >= n || e.by_name as usize >= n {
                    return Err(Error {
                        offset: tables,
                        kind: ErrorKind::Malformed("index entry out of range"),
                    });
                }
            }
        }
        Ok(kb)
    }

    /// Returns the header.
    #[must_use]
    pub fn header(&self) -> &Header<'a> {
        &self.header
    }

    /// Returns the modules (units).
    #[must_use]
    pub fn modules(&self) -> Section<'_, 'a, Module<'a>> {
        self.section(&self.modules)
    }

    /// Returns the constants.
    #[must_use]
    pub fn consts(&self) -> Section<'_, 'a, Const<'a>> {
        self.section(&self.consts)
    }

    /// Returns the types.
    #[must_use]
    pub fn types(&self) -> Section<'_, 'a, Type<'a>> {
        self.section(&self.types)
    }

    /// Returns the variables.
    #[must_use]
    pub fn vars(&self) -> Section<'_, 'a, Var<'a>> {
        self.section(&self.vars)
    }

    /// Returns the resource strings.
    #[must_use]
    pub fn res_strings(&self) -> Section<'_, 'a, ResString<'a>> {
        self.section(&self.res_strings)
    }

    /// Returns the routines.
    #[must_use]
    pub fn procs(&self) -> Section<'_, 'a, Proc<'a>> {
        self.section(&self.procs)
    }

    /// Returns the module with ID `id`.
    ///
    /// # Errors
    ///
    /// Returns an error when the module record is malformed.
    pub fn module(&self, id: u16) -> Result<Option<Module<'a>>, Error> {
        let modules = self.modules();
        modules
            .of_module(id)
            .first()
            .map(|&i| modules.get(i))
            .transpose()
    }

    /// Returns the module named `name` (ignoring ASCII case).
    ///
    /// # Errors
    ///
    /// Returns an error when the module record is malformed.
    pub fn module_named(&self, name: &str) -> Result<Option<Module<'a>>, Error> {
        let modules = self.modules();
        modules
            .named(name.as_bytes())
            .first()
            .map(|&i| modules.get(i))
            .transpose()
    }

    /// Decodes every record.
    ///
    /// # Errors
    ///
    /// Returns the first record that fails to decode.
    pub fn contents(&self) -> Result<Contents<'a>, Error> {
        Ok(Contents {
            header: self.header.clone(),
            modules: self.modules().iter().collect::<Result<_, _>>()?,
            consts: self.consts().iter().collect::<Result<_, _>>()?,
            types: self.types().iter().collect::<Result<_, _>>()?,
            vars: self.vars().iter().collect::<Result<_, _>>()?,
            res_strings: self.res_strings().iter().collect::<Result<_, _>>()?,
            procs: self.procs().iter().collect::<Result<_, _>>()?,
        })
    }

    fn section<'k, R>(&'k self, table: &'k Table) -> Section<'k, 'a, R> {
        Section {
            kb: self,
            table,
            marker: PhantomData,
        }
    }

    fn tables(&self) -> [&Table; 6] {
        [
            &self.modules,
            &self.consts,
            &self.types,
            &self.vars,
            &self.res_strings,
            &self.procs,
        ]
    }

    fn record_bytes(&self, entry: Entry) -> Option<&'a [u8]> {
        let start = entry.offset as usize;
        self.data
            .get(start..start.checked_add(entry.size as usize)?)
    }
}

/// Every record of a knowledge base, in file order; what
/// [`KnowledgeBase::contents`] decodes and [`Contents::to_bytes`] writes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Contents<'a> {
    /// The header.
    pub header: Header<'a>,
    /// The modules.
    pub modules: Vec<Module<'a>>,
    /// The constants.
    pub consts: Vec<Const<'a>>,
    /// The types.
    pub types: Vec<Type<'a>>,
    /// The variables.
    pub vars: Vec<Var<'a>>,
    /// The resource strings.
    pub res_strings: Vec<ResString<'a>>,
    /// The routines.
    pub procs: Vec<Proc<'a>>,
}

/// Returns `bytes` without its trailing NULs.
fn trim_trailing_nuls(bytes: &[u8]) -> &[u8] {
    let end = bytes
        .iter()
        .rposition(|&b| b != 0)
        .map_or(0, |i| i.saturating_add(1));
    bytes.get(..end).unwrap_or_default()
}

/// Returns `bytes` up to its first NUL.
fn nul_terminated(bytes: &[u8]) -> &[u8] {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    bytes.get(..end).unwrap_or_default()
}
