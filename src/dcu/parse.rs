//! The DCU reading state machine: header, source files, uses lists, the
//! declaration list and the tables it contains.
//!
//! [`Parser`] mirrors DCU32INT's `TUnit` reading state. The record readers
//! live in `records` (declarations and type definitions) and `addinfo`
//! (`drConstAddInfo` and inline bodies); all of them advance the shared
//! cursor and keep [`Parser::tag`] on the tag that ended the last record,
//! the way DCU32INT's global `Tag` works.

use std::{mem, ops::Range};

use crate::dcu::{
    AddrTarget, ConstData, Decl, DeclId, DeclKind, Error, ErrorKind, Fixup, ImportKind,
    ImportedSymbol, Modifier, SourceFile, SourceKind, TypeDef, TypeDefId, TypeKind, TypeSlot,
    TypeTarget, Unit, UnitImport, UsesSection, decode_name,
    reader::Reader,
    version::{Magic, Platform, Version},
};

/// Record tags (DCU32INT `dr*` / `ar*` constants), in the Delphi 6
/// numbering that [`Parser::fix_tag`] normalises later versions to.
pub mod tag {
    /// Final tag of every unit.
    pub const STOP_A: u8 = 0x61;
    /// .NET assembly data in a package unit.
    pub const ASSEMBLY_DATA: u8 = 0x62;
    /// End of an import list, a routine header or a member list.
    pub const STOP1: u8 = 0x63;
    /// An interface `uses` entry.
    pub const UNIT: u8 = 0x64;
    /// An implementation `uses` entry.
    pub const UNIT1: u8 = 0x65;
    /// An imported type.
    pub const IMP_TYPE: u8 = 0x66;
    /// An imported value.
    pub const IMP_VAL: u8 = 0x67;
    /// A DLL entry.
    pub const DLL: u8 = 0x68;
    /// An `exports` entry.
    pub const EXPORT: u8 = 0x69;
    /// Start of a nested declaration block.
    pub const EMBEDDED_PROC_START: u8 = 0x6A;
    /// End of a nested declaration block.
    pub const EMBEDDED_PROC_END: u8 = 0x6B;
    /// The code and data block.
    pub const CBLOCK: u8 = 0x6C;
    /// The fixup table.
    pub const FIXUP: u8 = 0x6D;
    /// An imported type redeclared `type B = type A`.
    pub const IMP_TYPE_DEF: u8 = 0x6E;
    /// A .NET record.
    pub const OREC: u8 = 0x6F;
    /// A source file.
    pub const SRC: u8 = 0x70;
    /// An object file.
    pub const OBJ: u8 = 0x71;
    /// A resource file.
    pub const RES: u8 = 0x72;
    /// An assembler file.
    pub const ASM: u8 = 0x73;
    /// A .NET assembly source.
    pub const ASSEMBLY_SRC: u8 = 0x74;
    /// The inline source (Delphi 2010 and later).
    pub const UNIT_INLINE_SRC: u8 = 0x76;
    /// An import separator.
    pub const STOP2: u8 = 0x9F;
    /// A constant.
    pub const CONST: u8 = 0x25;
    /// A resource string.
    pub const RES_STR: u8 = 0x32;
    /// A type name.
    pub const TYPE: u8 = 0x2A;
    /// A VMT symbol.
    pub const TYPE_P: u8 = 0x26;
    /// A routine.
    pub const PROC: u8 = 0x28;
    /// A compiler intrinsic routine.
    pub const SYS_PROC: u8 = 0x29;
    /// An untyped block type.
    pub const VOID: u8 = 0x40;
    /// A variable or local.
    pub const VAR: u8 = 0x20;
    /// A thread variable.
    pub const THREAD_VAR: u8 = 0x31;
    /// A typed constant.
    pub const VAR_C: u8 = 0x27;
    /// A Boolean range.
    pub const BOOL_RANGE_DEF: u8 = 0x41;
    /// A Char range.
    pub const CH_RANGE_DEF: u8 = 0x42;
    /// An enumeration.
    pub const ENUM_DEF: u8 = 0x43;
    /// An integer range.
    pub const RANGE_DEF: u8 = 0x44;
    /// A pointer.
    pub const PTR_DEF: u8 = 0x45;
    /// A class.
    pub const CLASS_DEF: u8 = 0x46;
    /// `class of`.
    pub const OBJ_VMT_DEF: u8 = 0x47;
    /// A procedural type.
    pub const PROC_TYPE_DEF: u8 = 0x48;
    /// A float.
    pub const FLOAT_DEF: u8 = 0x49;
    /// A set.
    pub const SET_DEF: u8 = 0x4A;
    /// A ShortString.
    pub const SHORT_STR_DEF: u8 = 0x4B;
    /// An array.
    pub const ARRAY_DEF: u8 = 0x4C;
    /// A record.
    pub const REC_DEF: u8 = 0x4D;
    /// A TP object.
    pub const OBJ_DEF: u8 = 0x4E;
    /// A file.
    pub const FILE_DEF: u8 = 0x4F;
    /// `Text`.
    pub const TEXT_DEF: u8 = 0x50;
    /// A WideChar range.
    pub const WCHAR_RANGE_DEF: u8 = 0x51;
    /// An AnsiString.
    pub const STRING_DEF: u8 = 0x52;
    /// A Variant.
    pub const VARIANT_DEF: u8 = 0x53;
    /// An interface.
    pub const INTERFACE_DEF: u8 = 0x54;
    /// A WideString.
    pub const WIDE_STR_DEF: u8 = 0x55;
    /// A wide range.
    pub const WIDE_RANGE_DEF: u8 = 0x56;
    /// A .NET metaclass.
    pub const META_CLASS_DEF: u8 = 0x57;
    /// A dynamic array (Delphi 2009 and later).
    pub const DYN_ARRAY_DEF: u8 = b'X';
    /// A generic parameter.
    pub const TEMPLATE_ARG_DEF: u8 = b'Y';
    /// A generic instantiation.
    pub const TEMPLATE_CALL: u8 = b'Z';
    /// A UnicodeString.
    pub const UNICODE_STRING_DEF: u8 = b'[';
    /// Code line table.
    pub const CODE_LINES: u8 = 0x90;
    /// Line range table.
    pub const LIN_NUM: u8 = 0x91;
    /// Structure scope table.
    pub const STRUC_SCOPE: u8 = 0x92;
    /// Symbol reference table.
    pub const SYMBOL_REF: u8 = 0x93;
    /// Local variable table.
    pub const LOC_VAR_TBL: u8 = 0x94;
    /// Unit flags.
    pub const UNIT_FLAGS: u8 = 0x96;
    /// A package unit's preferred image base (read from 10.4; seen in
    /// Delphi 13 Win64 packages).
    pub const IMAGE_BASE: u8 = 0xB7;
    /// Unit extra information.
    pub const UNIT_ADD_INFO: u8 = 0x34;
    /// C++ header flags.
    pub const CPP_FLAGS: u8 = 0x98;
    /// Declaration extra information.
    pub const CONST_ADD_INFO: u8 = 0x9C;
    /// Address claim.
    pub const PROC_ADD_INFO: u8 = 0x9E;
    /// .NET assembly information.
    pub const ASSEMBLY_INFO: u8 = 0x9D;
    /// A string-constant block.
    pub const STR_CONST_REC: u8 = 0x35;
    /// Kylix header record.
    pub const UNIT4: u8 = 0x0F;
    /// A compiler-allocated variable.
    pub const SPEC_VAR: u8 = 0x37;
    /// A class variable.
    pub const CLASS_VAR: u8 = 0x36;
    /// A C line.
    pub const CLINE: u8 = 0xA0;
    /// Unknown record of index lists.
    pub const A1_INFO: u8 = 0xA1;
    /// Empty marker.
    pub const A2_INFO: u8 = 0xA2;
    /// A copied declaration.
    pub const COPY_DECL: u8 = 0xA3;
    /// Generic marker.
    pub const A5_INFO: u8 = 0xA5;
    /// Generic parameter list.
    pub const A6_INFO: u8 = 0xA6;
    /// Generic parameters of the last type.
    pub const A7_INFO: u8 = 0xA7;
    /// Generic parameter address.
    pub const A8_INFO: u8 = 0xA8;
    /// Delay-loaded import.
    pub const DELAYED_IMP_INFO: u8 = 0xB0;
    /// An anonymous method block.
    pub const ANONYMOUS_BLOCK: u8 = 0x01;
    /// A value parameter.
    pub const VAL: u8 = 0x21;
    /// A var parameter.
    pub const AR_VAR: u8 = 0x22;
    /// A function result.
    pub const RESULT: u8 = 0x23;
    /// An absolute local.
    pub const ABS_LOC_VAR: u8 = 0x24;
    /// A label.
    pub const LABEL: u8 = 0x2B;
    /// Segment table (XE2 and later).
    pub const SEG_INFO: u8 = 0xB1;
    /// Address-to-segment table.
    pub const ADDR_TO_SEG_INFO: u8 = 0xB2;
    /// `final` marker (XE3 and later).
    pub const FINAL_FLAG: u8 = 0xC2;
    /// Unknown index (XE4 and later).
    pub const A9_INFO: u8 = 0xA9;
    /// Next overload link (XE7 and later).
    pub const NEXT_OVERLOAD: u8 = 0xB6;
    /// Dependency table (10 Seattle and later).
    pub const DEPENDENCY_INFO: u8 = 0xB5;
    /// A field.
    pub const FLD: u8 = 0x2C;
    /// A method.
    pub const METHOD: u8 = 0x2D;
    /// A constructor.
    pub const CONSTR: u8 = 0x2E;
    /// A destructor.
    pub const DESTR: u8 = 0x2F;
    /// A property.
    pub const PROPERTY: u8 = 0x30;
    /// A parameter default binding.
    pub const SET_DEFT: u8 = 0x9A;
    /// `cdecl`.
    pub const CDECL: u8 = 0x81;
    /// `pascal`.
    pub const PASCAL: u8 = 0x82;
    /// `stdcall`.
    pub const STDCALL: u8 = 0x83;
    /// `safecall`.
    pub const SAFECALL: u8 = 0x84;
}

/// Which declaration list is being read (DCU32INT `TDeclListKind`); a few
/// records read differently depending on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListKind {
    /// The unit's top level.
    Main,
    /// A routine's parameters and locals.
    Args,
    /// A procedural type's parameters.
    ArgsT,
    /// A nested declaration block.
    Embedded,
    /// Record and object members.
    Fields,
    /// Class members.
    Class,
    /// Interface members.
    Interface,
    /// Dispinterface members.
    DispInterface,
    /// A unit extra-information list.
    UnitAddInfo,
    /// A generic parameter or .NET record list.
    A6,
}

impl ListKind {
    /// Returns whether the list holds the members of a structured type.
    #[must_use]
    pub fn is_member_list(self) -> bool {
        matches!(
            self,
            Self::Class | Self::Interface | Self::DispInterface | Self::Fields
        )
    }

    /// Returns whether the list holds parameters.
    #[must_use]
    pub fn is_args(self) -> bool {
        matches!(self, Self::Args | Self::ArgsT)
    }
}

/// What owns the list being read, for records that attach to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Owner {
    /// The unit itself.
    None,
    /// A declaration (a routine, a unit extra-information record).
    Decl(DeclId),
    /// A type definition (a record, class, interface or procedural type).
    Type(TypeDefId),
}

/// DCU32INT's `TUnit` reading state.
#[derive(Debug)]
pub struct Parser<'a> {
    /// The cursor.
    pub r: Reader<'a>,
    /// The compiler release.
    pub version: Version,
    /// The target platform.
    pub platform: Platform,
    /// Whether the unit is .NET IL.
    pub msil: bool,
    /// Whether the unit comes out of a package.
    pub from_package: bool,
    /// The last tag read.
    pub tag: u8,
    /// Offset of the last tag read.
    pub def_start: usize,
    /// Declarations; `None` while a record is still being read.
    pub decls: Vec<Option<Decl>>,
    /// Type definitions; `None` while a record is still being read.
    pub type_defs: Vec<Option<TypeDef>>,
    /// The type table.
    pub types: Vec<TypeSlot>,
    /// The address table.
    pub addrs: Vec<AddrTarget>,
    /// The next type-table slot a definition takes, 0-based (`FTypeDefCnt`).
    pub type_def_count: usize,
    /// The next address a declaration claims, or 0 to append
    /// (`FhNextAddr`).
    pub next_addr: u32,
    /// The used units.
    pub uses: Vec<UnitImport>,
    /// The source files.
    pub sources: Vec<SourceFile>,
    /// Current nesting depth of declaration blocks.
    pub embed_depth: usize,
    /// Number of depths with a pending block.
    pub max_embed_depth: usize,
    /// Pending nested declaration blocks by depth.
    pub embedded: Vec<Vec<DeclId>>,
    /// Modifiers to attach to declarations once read.
    pub decl_mods: Vec<(DeclId, Modifier)>,
    /// `drConstAddInfo` flags to attach to declarations once read.
    pub decl_add_flags: Vec<(DeclId, u32)>,
    /// Modifiers to attach to type definitions once read.
    pub type_mods: Vec<(TypeDefId, Modifier)>,
    /// Modifiers to attach to whatever declaration holds an address once
    /// the unit is read (the address may be claimed after the record).
    pub decl_mods_at: Vec<(u32, Modifier)>,
    /// The code and data block.
    pub data_block: Option<Range<usize>>,
    /// The fixups.
    pub fixups: Option<Vec<Fixup>>,
    /// Whether each single-instance table has been read: code lines, line
    /// ranges, local variables, segments.
    pub seen: [bool; 4],
    /// The number of segments in the segment table.
    pub seg_count: u32,
}

impl<'a> Parser<'a> {
    /// Decodes a whole unit.
    ///
    /// # Errors
    ///
    /// Returns an [`Error`] for an unknown magic, a size mismatch, a
    /// truncated or malformed record, or a unit that does not end with its
    /// final tag exactly at the end of the image.
    pub fn run(data: &'a [u8], from_package: bool) -> Result<Unit, Error> {
        let mut r = Reader::new(data, Version::DELPHI_2);
        let word = r.u32()?;
        let magic =
            Magic::decode(word).ok_or_else(|| Error::new(0, ErrorKind::UnknownMagic(word)))?;
        r.set_version(magic.version);
        let mut p = Parser {
            r,
            version: magic.version,
            platform: magic.platform,
            msil: magic.msil,
            from_package,
            tag: 0,
            def_start: 0,
            decls: Vec::new(),
            type_defs: Vec::new(),
            types: Vec::new(),
            addrs: Vec::new(),
            type_def_count: 0,
            next_addr: 0,
            uses: Vec::new(),
            sources: Vec::new(),
            embed_depth: 0,
            max_embed_depth: 0,
            embedded: Vec::new(),
            decl_mods: Vec::new(),
            decl_add_flags: Vec::new(),
            type_mods: Vec::new(),
            decl_mods_at: Vec::new(),
            data_block: None,
            fixups: None,
            seen: [false; 4],
            seg_count: 0,
        };
        let header = p.read_header()?;
        let name = p.read_sources()?;
        p.read_uses(tag::UNIT)?;
        p.read_uses(tag::UNIT1)?;
        p.read_uses(tag::DLL)?;
        let main = p.read_decl_list(ListKind::Main, Owner::None)?;
        if p.tag != tag::STOP_A {
            return Err(Error::new(
                p.def_start,
                ErrorKind::UnexpectedTag {
                    context: "declaration list",
                    tag: p.tag,
                },
            ));
        }
        if p.r.remaining() != 0 {
            return Err(p.r.error(ErrorKind::Malformed("data after the final tag")));
        }
        if !p.platform.is_llvm() && (p.data_block.is_none() || p.fixups.is_none()) {
            return Err(p.r.error(ErrorKind::Malformed("unit without a code block")));
        }
        p.finish(header, name, main)
    }

    /// Returns whether the unit is a Delphi (not Kylix) release at or after
    /// `v`.
    #[must_use]
    pub fn d(&self, v: Version) -> bool {
        self.version.delphi_at_least(v)
    }

    /// Returns whether the release is numerically after `v`.
    #[must_use]
    pub fn after(&self, v: Version) -> bool {
        self.version.after(v)
    }

    /// Reads the next tag, remembering where it starts.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Truncated`] at the end of the image.
    pub fn read_tag(&mut self) -> Result<u8, Error> {
        self.def_start = self.r.pos();
        self.tag = self.r.u8()?;
        Ok(self.tag)
    }

    /// Normalises a tag to the Delphi 6 numbering: from Delphi 2006 the
    /// tags `0x2D` - `0x36` moved up by one, and `0x2D` became the class
    /// variable tag.
    #[must_use]
    pub fn fix_tag(&self, t: u8) -> u8 {
        if self.d(Version::DELPHI_2006) && (0x2D..=0x36).contains(&t) {
            let t = t.saturating_sub(1);
            return if t < 0x2D { tag::CLASS_VAR } else { t };
        }
        t
    }

    /// Builds an error of `kind` at the current offset.
    #[must_use]
    pub fn error(&self, kind: ErrorKind) -> Error {
        self.r.error(kind)
    }

    /// Builds an "unexpected tag" error for the last tag read.
    #[must_use]
    pub fn tag_error(&self, context: &'static str) -> Error {
        Error::new(
            self.def_start,
            ErrorKind::UnexpectedTag {
                context,
                tag: self.tag,
            },
        )
    }

    /// Fails unless the last tag read is [`tag::STOP1`].
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::UnexpectedTag`] naming `context`.
    pub fn expect_stop1(&self, context: &'static str) -> Result<(), Error> {
        if self.tag == tag::STOP1 {
            Ok(())
        } else {
            Err(self.tag_error(context))
        }
    }

    // ----- address and type tables -----

    /// Claims an address-table slot for `target` (DCU32INT `AddAddrDef`):
    /// the slot a preceding `drProcAddInfo` named, or the next free one.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Malformed`] when the named slot is out of range
    /// or already taken.
    pub fn add_addr(&mut self, target: AddrTarget) -> Result<u32, Error> {
        if self.next_addr > 0 {
            let index = self.next_addr;
            let slot = usize::try_from(index)
                .ok()
                .and_then(|i| i.checked_sub(1))
                .and_then(|i| self.addrs.get_mut(i))
                .ok_or_else(|| {
                    Error::new(
                        self.def_start,
                        ErrorKind::Malformed("address claim beyond the address table"),
                    )
                })?;
            // The self reference is a nil slot in DCU32INT: the unit's
            // initialization routine, named after the unit, claims it.
            if !matches!(*slot, AddrTarget::Empty | AddrTarget::SelfUnit) {
                return Err(Error::new(
                    self.def_start,
                    ErrorKind::Malformed("address claim on a used slot"),
                ));
            }
            *slot = target;
            self.next_addr = self.next_addr.saturating_add(1);
            return Ok(index);
        }
        self.append_addr(target)
    }

    /// Appends an address-table slot regardless of a pending claim
    /// (DCU32INT `AppendAddrDef`).
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Malformed`] when the table outgrows `u32`.
    pub fn append_addr(&mut self, target: AddrTarget) -> Result<u32, Error> {
        self.addrs.push(target);
        u32::try_from(self.addrs.len())
            .map_err(|_| self.error(ErrorKind::Malformed("address table too large")))
    }

    /// Reserves address `index` when it is the next one to be created
    /// (DCU32INT `RefAddrDef`): records may refer one slot ahead of the
    /// declaration that will claim it.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Malformed`] for a reference further ahead.
    pub fn ref_addr(&mut self, index: u32) -> Result<(), Error> {
        let count = self.addrs.len();
        let index = usize::try_from(index).unwrap_or(usize::MAX);
        if index > count {
            if index != count.saturating_add(1) {
                return Err(self.error(ErrorKind::Malformed("forward address reference")));
            }
            self.addrs.push(AddrTarget::Empty);
        }
        Ok(())
    }

    /// Returns the address-table slot at 1-based `index`.
    #[must_use]
    pub fn addr(&self, index: u32) -> Option<AddrTarget> {
        let i = usize::try_from(index).ok()?.checked_sub(1)?;
        self.addrs.get(i).copied()
    }

    /// Grows the type table to hold slot `index` (1-based).
    fn ensure_type_slot(&mut self, index: usize) {
        if index > self.types.len() {
            self.types.resize(
                index,
                TypeSlot {
                    target: TypeTarget::None,
                    name: None,
                    decl: 0,
                },
            );
        }
    }

    /// Records the name a type declaration gives type slot `index`
    /// (DCU32INT `AddTypeName`): a real name replaces a compiler-generated
    /// one, never the other way round.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Malformed`] when `index` lies beyond any slot the
    /// rest of the image could define: every definition takes at least one
    /// byte, so a larger index is corrupt (and would make the table huge).
    pub fn add_type_name(&mut self, index: u32, decl: u32, name: &str) -> Result<(), Error> {
        let Ok(index) = usize::try_from(index) else {
            return Err(self
                .r
                .error(ErrorKind::Malformed("type index out of range")));
        };
        if index == 0 {
            return Ok(());
        }
        if index > self.types.len().saturating_add(self.r.remaining()) {
            return Err(self
                .r
                .error(ErrorKind::Malformed("type index out of range")));
        }
        self.ensure_type_slot(index);
        let aux = |n: &str, v: &Self| is_aux_name(n, v.version);
        let replace = |slot: &TypeSlot, v: &Self| match &slot.name {
            None => true,
            Some(old) => !aux(name, v) && aux(old, v),
        };
        let do_replace = self
            .types
            .get(index.saturating_sub(1))
            .is_some_and(|slot| replace(slot, self));
        if let Some(slot) = self.types.get_mut(index.saturating_sub(1)) {
            if do_replace {
                slot.name = Some(name.to_string());
            }
            if slot.decl == 0 {
                slot.decl = decl;
            }
        }
        Ok(())
    }

    /// Reserves the next type-table slot for a type definition being read
    /// (DCU32INT `AddTypeDef`) and returns its id and 1-based index.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Malformed`] when the slot already holds a
    /// definition or an import.
    pub fn add_type_def(&mut self) -> Result<(TypeDefId, u32), Error> {
        let index = self.type_def_count.saturating_add(1);
        self.ensure_type_slot(index);
        let id = self.type_defs.len();
        self.type_defs.push(None);
        let slot = self
            .types
            .get_mut(self.type_def_count)
            .ok_or_else(|| self.r.error(ErrorKind::Malformed("type table")))?;
        if slot.target != TypeTarget::None {
            return Err(Error::new(
                self.def_start,
                ErrorKind::Malformed("type definition overrides a type slot"),
            ));
        }
        slot.target = TypeTarget::Def(id);
        self.type_def_count = index;
        let index =
            u32::try_from(index).map_err(|_| self.error(ErrorKind::Malformed("type table")))?;
        Ok((id, index))
    }

    /// Returns the type definition in the last type slot taken
    /// (DCU32INT `GetLastAddedTypeDef`), if that slot holds one.
    #[must_use]
    pub fn last_type_def(&self) -> Option<TypeDefId> {
        match self.types.get(self.type_def_count.checked_sub(1)?)?.target {
            TypeTarget::Def(id) => Some(id),
            _ => None,
        }
    }

    /// Reserves a declaration id for a record being read.
    pub fn reserve_decl(&mut self) -> DeclId {
        self.decls.push(None);
        self.decls.len().saturating_sub(1)
    }

    /// Stores a finished declaration under its reserved id.
    pub fn store_decl(&mut self, id: DeclId, decl: Decl) {
        if let Some(slot) = self.decls.get_mut(id) {
            *slot = Some(decl);
        }
    }

    /// Stores a finished type definition under its reserved id.
    pub fn store_type_def(&mut self, id: TypeDefId, def: TypeDef) {
        if let Some(slot) = self.type_defs.get_mut(id) {
            *slot = Some(def);
        }
    }

    /// Returns whether address `index` is a routine declaration.
    #[must_use]
    pub fn addr_is_proc(&self, index: u32) -> bool {
        match self.addr(index) {
            Some(AddrTarget::Decl(id)) => self
                .decls
                .get(id)
                .and_then(Option::as_ref)
                .is_some_and(|d| matches!(d.kind, DeclKind::Proc(_))),
            _ => false,
        }
    }

    /// Returns the declaration id at address `index`.
    #[must_use]
    pub fn addr_decl(&self, index: u32) -> Option<DeclId> {
        match self.addr(index)? {
            AddrTarget::Decl(id) => Some(id),
            _ => None,
        }
    }

    // ----- header, sources, uses -----

    /// Reads the unit header (DCU32INT `ReadUnitHeader`) and the first
    /// tag after it.
    fn read_header(&mut self) -> Result<Header, Error> {
        let size = self.r.u32()?;
        if usize::try_from(size).ok() != Some(self.r.data().len()) {
            return Err(Error::new(
                4,
                ErrorKind::SizeMismatch {
                    declared: size,
                    actual: self.r.data().len(),
                },
            ));
        }
        let mut h = Header {
            file_time: self.r.u32()?,
            ..Header::default()
        };
        if self.version == Version::DELPHI_2 {
            self.r.u8()?;
            self.read_tag()?;
            return Ok(h);
        }
        h.stamp = self.r.u32()?;
        self.r.u8()?;
        if self.d(Version::DELPHI_7) {
            self.r.u8()?;
            self.add_addr(AddrTarget::SelfUnit)?;
        }
        if self.d(Version::DELPHI_2005) {
            h.header_name = Some(decode_name(self.r.short_string()?));
        }
        if self.d(Version::DELPHI_2009) {
            self.r.uindex()?;
            self.r.uindex()?;
        }
        self.read_tag()?;
        if self.version.is_kylix() {
            if self.tag == tag::UNIT4 {
                while self.tag == tag::UNIT4 {
                    self.r.u32()?;
                    self.read_tag()?;
                }
            } else if self.tag != tag::UNIT_FLAGS {
                self.r.skip(3)?;
                self.read_tag()?;
            }
        }
        if self.tag == tag::UNIT_FLAGS {
            h.flags = self.r.uindex()?;
            if self.d(Version::DELPHI_2006) {
                h.flags1 = self.r.uindex()?;
            }
            if self.after(Version::DELPHI_3) {
                h.priority = self.r.uindex()?;
            }
            self.read_tag()?;
        }
        if self.tag == tag::IMAGE_BASE && self.d(Version::DELPHI_10_4_SYDNEY) {
            h.image_base = Some(self.r.uindex64()?);
            self.read_tag()?;
        }
        Ok(h)
    }

    /// Reads the source-file records (DCU32INT `ReadSourceFiles`) and
    /// returns the unit name: the main source's file name without
    /// directory or extension.
    fn read_sources(&mut self) -> Result<String, Error> {
        let mut main: Option<usize> = None;
        loop {
            let kind = match self.tag {
                tag::SRC => SourceKind::Source,
                tag::RES => SourceKind::Resource,
                tag::OBJ => SourceKind::Object,
                tag::ASM => SourceKind::Assembler,
                tag::UNIT_INLINE_SRC if self.d(Version::DELPHI_2010) => SourceKind::InlineSource,
                tag::ASSEMBLY_INFO if self.d(Version::DELPHI_2005) && self.msil => {
                    SourceKind::Assembly
                }
                tag::ASSEMBLY_SRC if self.msil => SourceKind::Assembly,
                _ => break,
            };
            if self.tag == tag::ASSEMBLY_INFO {
                let name = decode_name(self.r.index_string()?);
                self.sources.push(SourceFile {
                    kind,
                    name,
                    time: 0,
                    index: 0,
                });
            } else {
                let name = decode_name(self.r.name()?);
                let time = self.r.u32()?;
                let index = self.r.uindex()?;
                if index == 0 {
                    main = Some(self.sources.len());
                }
                if self.msil && self.tag != tag::RES && self.tag != tag::ASSEMBLY_SRC {
                    self.r.short_string()?;
                }
                self.sources.push(SourceFile {
                    kind,
                    name,
                    time,
                    index,
                });
            }
            self.read_tag()?;
        }
        if self.sources.is_empty() && !self.from_package {
            return Err(self.error(ErrorKind::Malformed("no source files")));
        }
        let main = main.or(if self.sources.is_empty() {
            None
        } else {
            Some(0)
        });
        let Some(src) = main.and_then(|i| self.sources.get(i)) else {
            return Ok(String::new());
        };
        if src.kind == SourceKind::Assembly {
            return Ok(String::new());
        }
        let file = src
            .name
            .rsplit(['\\', '/'])
            .next()
            .unwrap_or(src.name.as_str());
        Ok(match file.rfind('.') {
            Some(dot) if dot > 0 => file.get(..dot).unwrap_or(file).to_string(),
            _ => file.to_string(),
        })
    }

    /// Reads one uses list (DCU32INT `ReadUses`): every unit with the
    /// symbols imported from it.
    fn read_uses(&mut self, list: u8) -> Result<(), Error> {
        let section = match list {
            tag::UNIT1 => UsesSection::Implementation,
            tag::DLL => UsesSection::Dll,
            _ => UsesSection::Interface,
        };
        while self.tag == list {
            let name = decode_name(self.r.name()?);
            let mut package = 0;
            if list != tag::DLL && self.d(Version::DELPHI_8) {
                package = self.r.uindex()?;
            }
            let stamp = if self.d(Version::DELPHI_2006) {
                self.r.uindex()?
            } else {
                self.r.u32()?
            };
            if self.version == Version::DELPHI_7 || (self.d(Version::DELPHI_8) && list == tag::DLL)
            {
                self.r.u32()?;
            }
            if self.d(Version::DELPHI_2009) {
                self.r.uindex()?;
            }
            let unit = self.uses.len();
            let addr = self.add_addr(AddrTarget::Unit(unit))?;
            self.uses.push(UnitImport {
                name,
                section,
                stamp,
                addr,
                symbols: Vec::new(),
            });
            if package > 0 && !self.d(Version::DELPHI_2009) {
                self.ref_addr(package)?;
            }
            loop {
                self.read_tag()?;
                match self.tag {
                    tag::IMP_TYPE | tag::IMP_TYPE_DEF if list != tag::DLL => {
                        let name = decode_name(self.r.name()?);
                        let kind = if self.tag == tag::IMP_TYPE_DEF {
                            ImportKind::TypeCopy {
                                rtti_size: self.r.uindex()?,
                            }
                        } else {
                            ImportKind::Type
                        };
                        let stamp = self.r.u32()?;
                        let symbol = self.uses.get(unit).map_or(0, |u| u.symbols.len());
                        // A plain import's slot carries the imported name from
                        // the start, so a later `type TFoo = Integer` keeps it
                        // `Integer`. A type copy (`type TFoo = type Integer`)
                        // is a new type: its declaration names the slot.
                        let slot_name = match kind {
                            ImportKind::Type => Some(name.clone()),
                            _ => None,
                        };
                        self.types.push(TypeSlot {
                            target: TypeTarget::Import { unit, symbol },
                            name: slot_name,
                            decl: 0,
                        });
                        let type_index = u32::try_from(self.types.len()).ok();
                        let addr = self.add_addr(AddrTarget::Import { unit, symbol })?;
                        self.type_def_count = self.types.len();
                        if let Some(u) = self.uses.get_mut(unit) {
                            u.symbols.push(ImportedSymbol {
                                name,
                                kind,
                                stamp,
                                addr,
                                type_index,
                            });
                        }
                    }
                    tag::IMP_VAL => {
                        let name = decode_name(self.r.name()?);
                        let stamp = self.r.u32()?;
                        let symbol = self.uses.get(unit).map_or(0, |u| u.symbols.len());
                        let addr = self.add_addr(AddrTarget::Import { unit, symbol })?;
                        let kind = if list == tag::DLL {
                            ImportKind::DllRoutine
                        } else {
                            ImportKind::Value
                        };
                        if let Some(u) = self.uses.get_mut(unit) {
                            u.symbols.push(ImportedSymbol {
                                name,
                                kind,
                                stamp,
                                addr,
                                type_index: None,
                            });
                        }
                    }
                    tag::STOP2 => {
                        if self.d(Version::DELPHI_8) {
                            self.r.u32()?;
                        }
                    }
                    // Delphi 10.4+ writes a unit's documentation comments
                    // where the uses list ends.
                    tag::CONST_ADD_INFO if self.msil || self.d(Version::DELPHI_10_4_SYDNEY) => {
                        self.read_const_add_info()?;
                    }
                    _ => break,
                }
            }
            if self.tag != tag::STOP1 {
                return Err(self.tag_error("import list"));
            }
            self.read_tag()?;
            if self.tag == tag::PROC_ADD_INFO {
                if !self.d(Version::DELPHI_7) {
                    break;
                }
                let v = self.r.index()?;
                self.set_proc_add_info(v);
                self.read_tag()?;
            }
        }
        Ok(())
    }

    /// Applies a `drProcAddInfo` value (DCU32INT `SetProcAddInfo`): `-1`
    /// resumes appending, a positive value makes the following
    /// declarations claim addresses from that slot on.
    pub fn set_proc_add_info(&mut self, v: i32) {
        if v == -1 {
            self.next_addr = 0;
        }
        if v >= 1 && self.d(Version::DELPHI_7) {
            self.next_addr = v.unsigned_abs();
        }
    }

    // ----- nested declaration blocks -----

    /// Enters a nested declaration block (DCU32INT `IncEmbedDepth`) and
    /// returns the depth index the block's declarations append to.
    fn inc_embed_depth(&mut self) -> usize {
        self.embed_depth = self.embed_depth.saturating_add(1);
        if self.embed_depth > self.max_embed_depth {
            self.max_embed_depth = self.embed_depth;
        }
        if self.embedded.len() < self.embed_depth {
            self.embedded.resize(self.embed_depth, Vec::new());
        }
        self.embed_depth.saturating_sub(1)
    }

    /// Takes the pending nested block for a routine being declared
    /// (DCU32INT `ConsumeEmbedded`): the innermost block deeper than the
    /// current depth.
    pub fn consume_embedded(&mut self) -> Vec<DeclId> {
        if self.max_embed_depth == 0 || self.embed_depth >= self.max_embed_depth {
            return Vec::new();
        }
        self.max_embed_depth = self.max_embed_depth.saturating_sub(1);
        self.embedded
            .get_mut(self.max_embed_depth)
            .map(mem::take)
            .unwrap_or_default()
    }

    // ----- the declaration list -----

    /// Reads a declaration list (DCU32INT `ReadDeclList`) starting at the
    /// current tag, and returns its declarations. The list ends at the
    /// first tag the list kind does not accept; that tag stays in
    /// [`Parser::tag`].
    ///
    /// # Errors
    ///
    /// Returns an [`Error`] when a record inside the list is malformed.
    pub fn read_decl_list(&mut self, lk: ListKind, owner: Owner) -> Result<Vec<DeclId>, Error> {
        let mut out = Vec::new();
        // Delphi 2009+ and .NET bracket constant blocks inside parameter
        // lists with end / start pairs; the starts after ends are skipped.
        let mut emb_end_count = 0usize;
        loop {
            let t = self.fix_tag(self.tag);
            let mut decl: Option<DeclId> = None;
            match t {
                tag::TYPE => decl = Some(self.read_type_decl(lk)?),
                tag::TYPE_P => decl = Some(self.read_var_c(t, VarFlavor::Vmt)?),
                tag::CONST => decl = Some(self.read_const_decl()?),
                tag::RES_STR => decl = Some(self.read_var_c(t, VarFlavor::ResourceString)?),
                tag::SYS_PROC => {
                    decl = Some(if self.d(Version::DELPHI_8) {
                        self.read_proc(t, Vec::new(), true)?
                    } else {
                        self.read_sys_proc()?
                    });
                }
                tag::PROC => {
                    let embedded = self.consume_embedded();
                    decl = Some(self.read_proc(t, embedded, false)?);
                }
                tag::EMBEDDED_PROC_START => {
                    if (self.msil || self.d(Version::DELPHI_2009)) && emb_end_count > 0 {
                        emb_end_count = emb_end_count.saturating_sub(1);
                    } else {
                        let slot = self.inc_embed_depth();
                        self.read_tag()?;
                        let block = self.read_decl_list(ListKind::Embedded, Owner::None)?;
                        self.embed_depth = self.embed_depth.saturating_sub(1);
                        if self.tag != tag::EMBEDDED_PROC_END {
                            return Err(self.tag_error("nested declaration block"));
                        }
                        if let Some(list) = self.embedded.get_mut(slot) {
                            list.extend(block);
                        }
                    }
                }
                tag::EMBEDDED_PROC_END => {
                    let in_args = (lk == ListKind::ArgsT || lk == ListKind::Args)
                        && self.after(Version::DELPHI_3);
                    if !in_args {
                        break;
                    }
                    if self.msil || self.d(Version::DELPHI_2009) {
                        emb_end_count = emb_end_count.saturating_add(1);
                    }
                }
                tag::VAR => {
                    decl = Some(if lk.is_args() {
                        self.read_local(t, lk)?
                    } else {
                        self.read_var(t, VarFlavor::Var)?
                    });
                }
                tag::THREAD_VAR => decl = Some(self.read_var(t, VarFlavor::ThreadVar)?),
                tag::EXPORT => decl = Some(self.read_export()?),
                tag::VAR_C => decl = Some(self.read_var_c(t, VarFlavor::TypedConst)?),
                tag::VAL | tag::AR_VAR | tag::RESULT | tag::FLD => {
                    decl = Some(self.read_local(t, lk)?);
                }
                tag::ABS_LOC_VAR => {
                    decl = Some(if lk == ListKind::Main {
                        self.read_var(t, VarFlavor::Absolute)?
                    } else {
                        self.read_local(t, lk)?
                    });
                }
                tag::LABEL => decl = Some(self.read_label()?),
                tag::METHOD | tag::CONSTR | tag::DESTR => decl = Some(self.read_method(t, lk)?),
                tag::CLASS_VAR => {
                    if !self.d(Version::DELPHI_2006) {
                        break;
                    }
                    decl = Some(self.read_local(t, lk)?);
                }
                tag::PROPERTY => {
                    decl = Some(if lk == ListKind::DispInterface {
                        self.read_local(t, lk)?
                    } else {
                        self.read_property()?
                    });
                }
                tag::CDECL | tag::PASCAL | tag::STDCALL | tag::SAFECALL => {}
                tag::SET_DEFT => decl = Some(self.read_set_default()?),
                tag::STOP2 => {
                    if self.d(Version::DELPHI_8) {
                        self.r.u32()?;
                    }
                }
                tag::STR_CONST_REC => {
                    if !self.d(Version::DELPHI_8) {
                        break;
                    }
                    decl = Some(self.read_str_const()?);
                }
                tag::SPEC_VAR => {
                    if !self.d(Version::DELPHI_2006) {
                        break;
                    }
                    decl = Some(self.read_var(t, VarFlavor::Special)?);
                }
                tag::RANGE_DEF
                | tag::CH_RANGE_DEF
                | tag::BOOL_RANGE_DEF
                | tag::WCHAR_RANGE_DEF
                | tag::WIDE_RANGE_DEF
                | tag::ENUM_DEF
                | tag::FLOAT_DEF
                | tag::PTR_DEF
                | tag::TEXT_DEF
                | tag::FILE_DEF
                | tag::SET_DEF
                | tag::SHORT_STR_DEF
                | tag::STRING_DEF
                | tag::WIDE_STR_DEF
                | tag::ARRAY_DEF
                | tag::VARIANT_DEF
                | tag::OBJ_VMT_DEF
                | tag::REC_DEF
                | tag::PROC_TYPE_DEF
                | tag::OBJ_DEF
                | tag::CLASS_DEF
                | tag::INTERFACE_DEF
                | tag::VOID => {
                    self.read_type_def(t)?;
                }
                tag::META_CLASS_DEF => {
                    if !self.d(Version::DELPHI_8) {
                        break;
                    }
                    self.read_type_def(t)?;
                }
                tag::DYN_ARRAY_DEF
                | tag::TEMPLATE_ARG_DEF
                | tag::TEMPLATE_CALL
                | tag::UNICODE_STRING_DEF => {
                    if !self.d(Version::DELPHI_2009) {
                        break;
                    }
                    self.read_type_def(t)?;
                }
                tag::CBLOCK => {
                    if lk != ListKind::Main {
                        break;
                    }
                    if self.data_block.is_some() {
                        return Err(self.error(ErrorKind::Malformed("second code block")));
                    }
                    let size = self.r.uindex()? as usize;
                    let start = self.r.pos();
                    self.r.skip(size)?;
                    self.data_block = Some(start..self.r.pos());
                }
                tag::FIXUP => self.read_fixups()?,
                tag::CODE_LINES => self.read_code_lines()?,
                tag::LIN_NUM => self.read_line_ranges()?,
                tag::STRUC_SCOPE => self.read_struc_scope()?,
                tag::LOC_VAR_TBL => self.read_loc_var_table()?,
                tag::SYMBOL_REF => self.read_symbol_info()?,
                tag::UNIT_ADD_INFO => {
                    if !self.d(Version::DELPHI_7) {
                        break;
                    }
                    decl = Some(self.read_unit_add_info(t)?);
                }
                tag::CONST_ADD_INFO => {
                    if !(self.d(Version::DELPHI_7) || self.version >= Version::KYLIX_3) {
                        break;
                    }
                    self.read_const_add_info()?;
                }
                tag::PROC_ADD_INFO => {
                    if !self.d(Version::DELPHI_7) {
                        break;
                    }
                    let v = self.r.index()?;
                    self.set_proc_add_info(v);
                }
                tag::NEXT_OVERLOAD => {
                    // DCU32INT tests `not(Ver>=XE7) and (Ver<K1)`, which
                    // lets Kylix through; Kylix never writes the tag.
                    if self.version < Version::DELPHI_XE7 && !self.version.is_kylix() {
                        break;
                    }
                    let v = self.r.uindex()?;
                    self.ref_addr(v)?;
                }
                tag::DEPENDENCY_INFO => {
                    if self.version < Version::DELPHI_10_SEATTLE && !self.version.is_kylix() {
                        break;
                    }
                    self.read_dependency_info()?;
                }
                tag::OREC => {
                    if !self.d(Version::DELPHI_8) {
                        break;
                    }
                    if self.version >= Version::DELPHI_2009 {
                        decl = Some(self.read_orec(t)?);
                    } else {
                        self.r.uindex()?;
                    }
                }
                tag::CPP_FLAGS => {
                    if !self.d(Version::DELPHI_3) {
                        break;
                    }
                    let flags = self.r.u8()?;
                    let addr = self.r.uindex()?;
                    self.decl_mods_at.push((addr, Modifier::CppFlags(flags)));
                }
                tag::CLINE => {
                    if !self.d(Version::DELPHI_2006) {
                        break;
                    }
                    if self.d(Version::DELPHI_XE) {
                        self.r.u8()?;
                    }
                    let len = self.r.uindex()? as usize;
                    self.r.skip(len)?;
                }
                tag::A1_INFO => {
                    if !self.d(Version::DELPHI_2006) {
                        break;
                    }
                    for _ in 0..4 {
                        self.r.uindex()?;
                    }
                    let n = self.r.uindex()?;
                    for _ in 0..n {
                        self.r.uindex()?;
                    }
                }
                tag::A2_INFO => {
                    if !self.d(Version::DELPHI_2006) {
                        break;
                    }
                }
                tag::COPY_DECL => {
                    if !self.d(Version::DELPHI_2006) {
                        break;
                    }
                    decl = Some(self.read_copy_decl(t)?);
                }
                tag::A5_INFO => {
                    if !self.d(Version::DELPHI_2009) {
                        break;
                    }
                }
                tag::A6_INFO => {
                    if !self.d(Version::DELPHI_2009) {
                        break;
                    }
                    decl = Some(self.read_a6_decl(t)?);
                }
                tag::A7_INFO => {
                    if !self.d(Version::DELPHI_2009) {
                        break;
                    }
                    self.read_template_params(owner)?;
                }
                tag::A8_INFO => {
                    if !self.d(Version::DELPHI_2009) {
                        break;
                    }
                    self.r.uindex()?;
                }
                tag::A9_INFO => {
                    if !self.d(Version::DELPHI_XE4) {
                        break;
                    }
                    self.r.uindex()?;
                }
                tag::ANONYMOUS_BLOCK => {
                    if !self.d(Version::DELPHI_2010) {
                        break;
                    }
                    self.r.uindex()?;
                    self.r.uindex()?;
                }
                tag::DELAYED_IMP_INFO => {
                    if !self.d(Version::DELPHI_2009) {
                        break;
                    }
                    decl = Some(self.read_delayed_import()?);
                }
                tag::SEG_INFO => {
                    if !self.d(Version::DELPHI_XE2) {
                        break;
                    }
                    self.read_seg_info()?;
                }
                tag::ADDR_TO_SEG_INFO => {
                    if !self.d(Version::DELPHI_XE2) {
                        break;
                    }
                    self.read_addr_to_seg_info()?;
                }
                tag::ASSEMBLY_DATA => {
                    if !(self.from_package && self.msil) {
                        break;
                    }
                    return Err(self.error(ErrorKind::Malformed(
                        ".NET package assembly data is not supported",
                    )));
                }
                tag::FINAL_FLAG => {
                    if !self.d(Version::DELPHI_XE3) {
                        break;
                    }
                    self.r.uindex()?;
                }
                _ => break,
            }
            if let Some(id) = decl {
                out.push(id);
            }
            self.read_tag()?;
        }
        Ok(out)
    }

    // ----- tables -----

    /// Reads the fixup table (DCU32INT `LoadFixups`): delta-coded offsets
    /// into the code block, each with a kind byte and a target address.
    fn read_fixups(&mut self) -> Result<(), Error> {
        if self.fixups.is_some() {
            return Err(self.error(ErrorKind::Malformed("second fixup table")));
        }
        let block = self.data_block.as_ref().map_or(0, Range::len);
        let count = self.r.uindex()?;
        let mut out = Vec::new();
        let mut offset: u64 = 0;
        for _ in 0..count {
            let delta = self.r.uindex64()?;
            offset = offset.saturating_add(delta);
            if delta > u64::from(u32::MAX) || offset > block as u64 {
                return Err(self.error(ErrorKind::Malformed("fixup beyond the code block")));
            }
            let kind = self.r.u8()?;
            let target = self.r.uindex()?;
            out.push(Fixup {
                offset: offset as u32,
                kind,
                target,
            });
        }
        self.fixups = Some(out);
        Ok(())
    }

    /// Marks the single-instance table `which` as read.
    fn once(&mut self, which: usize, what: &'static str) -> Result<(), Error> {
        match self.seen.get_mut(which) {
            Some(seen) if *seen => Err(self.r.error(ErrorKind::Malformed(what))),
            Some(seen) => {
                *seen = true;
                Ok(())
            }
            None => Ok(()),
        }
    }

    /// Reads the code-line table (DCU32INT `LoadCodeLines`): per entry a
    /// line delta and a code-offset delta.
    ///
    /// The compiler sometimes stores a backward code step as a 16-bit
    /// value (`0xFFFC` for -4; DCU32INT notes `0xFFFF` runs in Delphi
    /// 2009's `MidasLib`, and Delphi 13 debug units have such steps at
    /// inlined lines). A delta below `0x10000` whose forward reading would
    /// leave the code block is therefore taken as that backward step; the
    /// table is only validated, not kept.
    fn read_code_lines(&mut self) -> Result<(), Error> {
        self.once(0, "second code-line table")?;
        let block = self.data_block.as_ref().map_or(0, Range::len) as u64;
        let count = self.r.uindex()?;
        let mut offset: u64 = 0;
        for _ in 0..count {
            self.r.index()?;
            let delta = self.r.uindex64()?;
            let forward = offset.saturating_add(delta);
            offset = if forward > block && delta < 0x1_0000 {
                forward.checked_sub(0x1_0000).unwrap_or(forward)
            } else {
                forward
            };
            if !self.from_package && (delta > u64::from(u32::MAX) || offset > block) {
                return Err(self.error(ErrorKind::Malformed("code line beyond the code block")));
            }
        }
        Ok(())
    }

    /// Reads the line-range table (DCU32INT `LoadLineRanges`): each range
    /// maps a run of internal line numbers to a source file.
    fn read_line_ranges(&mut self) -> Result<(), Error> {
        self.once(1, "second line-range table")?;
        let count = self.r.uindex()?;
        for _ in 0..count {
            self.r.uindex()?;
            self.r.uindex()?;
            let file = self.r.uindex()?;
            if !self.sources.iter().any(|s| s.index == file) {
                return Err(self.error(ErrorKind::Malformed("line range names an unknown file")));
            }
        }
        Ok(())
    }

    /// Reads the structure-scope table (DCU32INT `LoadStrucScope`): five
    /// indices per entry, six from 10.3 Rio.
    fn read_struc_scope(&mut self) -> Result<(), Error> {
        let count = self.r.uindex()?;
        let per = if self.d(Version::DELPHI_10_3_RIO) {
            6
        } else {
            5
        };
        for _ in 0..u64::from(count).saturating_mul(per) {
            self.r.uindex()?;
        }
        Ok(())
    }

    /// Reads the symbol-reference table (DCU32INT `LoadSymbolInfo`).
    fn read_symbol_info(&mut self) -> Result<(), Error> {
        let count = self.r.uindex()?;
        self.r.uindex()?;
        for _ in 0..count {
            self.r.uindex()?;
            self.r.uindex()?;
            let refs = self.r.uindex()?;
            self.r.uindex()?;
            for _ in 0..refs {
                self.r.uindex()?;
            }
        }
        Ok(())
    }

    /// Reads the local-variable table (DCU32INT `LoadLocVarTbl`). Win64
    /// units write three extra records after each routine's first entry
    /// without counting them.
    fn read_loc_var_table(&mut self) -> Result<(), Error> {
        self.once(2, "second local-variable table")?;
        let count = self.r.uindex()?;
        if self.platform != Platform::Win64 {
            for _ in 0..count {
                self.r.uindex()?;
                self.r.uindex()?;
                self.r.index()?;
            }
            return Ok(());
        }
        let mut proc_rec = 0u32;
        let mut i = 0u32;
        while i < count {
            let sym = self.r.uindex()?;
            let mut extra = false;
            if proc_rec == 0 && sym != 0 && self.addr_is_proc(sym) {
                proc_rec = 3;
                extra = true;
            }
            self.r.uindex()?;
            if proc_rec > 1 {
                self.r.uindex()?;
            } else {
                self.r.index()?;
            }
            if !extra {
                i = i.saturating_add(1);
            }
            if proc_rec > 0 {
                proc_rec = proc_rec.saturating_sub(1);
            }
        }
        Ok(())
    }

    /// Reads the segment table (XE2 and later).
    fn read_seg_info(&mut self) -> Result<(), Error> {
        self.once(3, "second segment table")?;
        let count = self.r.uindex()?;
        self.seg_count = count;
        for _ in 0..count {
            self.r.short_string()?;
            self.r.u8()?;
            self.r.uindex()?;
        }
        Ok(())
    }

    /// Reads the address-to-segment table (XE2 and later).
    fn read_addr_to_seg_info(&mut self) -> Result<(), Error> {
        let count = self.r.uindex()?;
        for _ in 0..count {
            self.r.uindex()?;
            self.r.uindex()?;
            self.r.u8()?;
            for _ in 0..4 {
                self.r.uindex()?;
            }
        }
        Ok(())
    }

    /// Reads the dependency table (10 Seattle and later; DCU32INT
    /// `ReadDependencyInfo`).
    fn read_dependency_info(&mut self) -> Result<(), Error> {
        let count = self.r.uindex()?;
        for _ in 0..count {
            if self.r.u8()? != 0 {
                return Err(self.error(ErrorKind::Malformed("dependency entry flag")));
            }
            self.r.short_string()?;
            self.r.u16()?;
            self.r.uindex()?;
            let n = self.r.uindex()?;
            for _ in 0..n {
                self.r.uindex()?;
            }
        }
        Ok(())
    }

    // ----- finishing -----

    /// Assembles the [`Unit`]: attaches deferred modifiers and flags,
    /// links enumeration members, and collects unclaimed nested blocks.
    fn finish(mut self, header: Header, name: String, main: Vec<DeclId>) -> Result<Unit, Error> {
        for (id, flags) in mem::take(&mut self.decl_add_flags) {
            if let Some(Some(d)) = self.decls.get_mut(id) {
                d.add_info_flags = flags;
            }
        }
        for (addr, m) in mem::take(&mut self.decl_mods_at) {
            if let Some(id) = self.addr_decl(addr) {
                self.decl_mods.push((id, m));
            }
        }
        for (id, m) in mem::take(&mut self.decl_mods) {
            if let Some(Some(d)) = self.decls.get_mut(id) {
                d.modifiers.push(m);
            }
        }
        for (id, m) in mem::take(&mut self.type_mods) {
            if let Some(Some(t)) = self.type_defs.get_mut(id) {
                t.modifiers.push(m);
            }
        }
        let decls: Vec<Decl> = self
            .decls
            .into_iter()
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| Error::new(0, ErrorKind::Malformed("unfinished declaration")))?;
        let mut type_defs: Vec<TypeDef> = self
            .type_defs
            .into_iter()
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| Error::new(0, ErrorKind::Malformed("unfinished type definition")))?;
        link_enum_members(&decls, &main, &self.types, &mut type_defs);
        // The source file name may differ in case from the declaration
        // (Delphi 6 ships `classes.pas`); the unit's initialization routine
        // is named exactly as the unit is declared.
        let name = main
            .iter()
            .filter_map(|&id| decls.get(id))
            .find(|d| matches!(d.kind, DeclKind::Proc(_)) && d.name.eq_ignore_ascii_case(&name))
            .map_or(name, |d| d.name.clone());
        let unclaimed = self.embedded.into_iter().flatten().collect();
        let fixups = self.fixups.unwrap_or_default();
        Ok(Unit {
            version: self.version,
            platform: self.platform,
            msil: self.msil,
            from_package: self.from_package,
            name,
            header_name: header.header_name,
            file_time: header.file_time,
            stamp: header.stamp,
            flags: header.flags,
            flags1: header.flags1,
            priority: header.priority,
            image_base: header.image_base,
            sources: self.sources,
            uses: self.uses,
            decls,
            type_defs,
            main,
            unclaimed,
            block: self
                .data_block
                .as_ref()
                .and_then(|r| self.r.data().get(r.clone()))
                .map(<[u8]>::to_vec)
                .unwrap_or_default(),
            data_block: self.data_block,
            fixups,
            types: self.types,
            addrs: self.addrs,
        })
    }
}

/// The unit header fields.
#[derive(Debug, Default)]
struct Header {
    file_time: u32,
    stamp: u32,
    header_name: Option<String>,
    flags: u32,
    flags1: u32,
    priority: u32,
    image_base: Option<u64>,
}

/// The flavours of variable-like declaration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VarFlavor {
    /// A variable.
    Var,
    /// A thread variable.
    ThreadVar,
    /// A typed constant.
    TypedConst,
    /// A resource string.
    ResourceString,
    /// A VMT symbol.
    Vmt,
    /// An `absolute` variable.
    Absolute,
    /// A compiler-allocated variable.
    Special,
}

/// Returns whether `name` is compiler-generated (DCU32INT `IsAuxName`):
/// it starts with `.`, or from Delphi 2009 starts with `:` or contains
/// `` ` ``.
#[must_use]
pub fn is_aux_name(name: &str, version: Version) -> bool {
    if name.starts_with('.') {
        return true;
    }
    version.delphi_at_least(Version::DELPHI_2009) && (name.starts_with(':') || name.contains('`'))
}

/// Links each enumeration to its member constants (DCU32INT
/// `SetEnumConsts`): a run of consecutive integer constants of one local
/// enumeration type whose values span exactly the type's range.
fn link_enum_members(
    decls: &[Decl],
    main: &[DeclId],
    types: &[TypeSlot],
    type_defs: &mut [TypeDef],
) {
    let enum_of = |d: &Decl| match &d.kind {
        DeclKind::Const(v) => match v.data {
            ConstData::Int(value) => Some((v.type_index, value)),
            ConstData::Bytes(_) => None,
        },
        _ => None,
    };
    let mut i = 0usize;
    while i < main.len() {
        let Some((type_index, first)) = main.get(i).and_then(|&id| decls.get(id)).and_then(enum_of)
        else {
            i = i.saturating_add(1);
            continue;
        };
        let mut run = vec![(first, main.get(i).copied().unwrap_or(0))];
        let mut j = i.saturating_add(1);
        while let Some((t, v)) = main.get(j).and_then(|&id| decls.get(id)).and_then(enum_of) {
            if t != type_index {
                break;
            }
            run.push((v, main.get(j).copied().unwrap_or(0)));
            j = j.saturating_add(1);
        }
        let def = usize::try_from(type_index)
            .ok()
            .and_then(|t| t.checked_sub(1))
            .and_then(|t| types.get(t))
            .and_then(|slot| match slot.target {
                TypeTarget::Def(id) => Some(id),
                _ => None,
            });
        if let Some(def) = def.and_then(|id| type_defs.get_mut(id))
            && let TypeKind::Enum {
                low, high, members, ..
            } = &mut def.kind
            && members.is_empty()
        {
            let min = run.iter().map(|r| r.0).min().unwrap_or(0);
            let max = run.iter().map(|r| r.0).max().unwrap_or(0);
            if min == *low && max == *high {
                run.sort_by_key(|r| r.0);
                run.dedup_by_key(|r| r.0);
                *members = run.into_iter().map(|r| r.1).collect();
            }
        }
        i = j;
    }
}
