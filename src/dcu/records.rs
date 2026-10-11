//! Declaration and type-definition record readers: the reading halves of
//! DCU32INT's `TNameDecl` and `TTypeDef` descendants (`DCURecs.pas`).
//!
//! Each reader starts right after the record's tag and leaves the cursor on
//! the byte after the record; records that contain a declaration list leave
//! [`Parser::tag`] on the list's terminator.

use crate::dcu::{
    AddrTarget, CallConv, ClassDef, Decl, DeclFlags, DeclId, DeclKind, Error, ErrorKind, FloatKind,
    Local, Method, MethodKind, Modifier, Proc, Property, RangeKind, StringKind, TypeDef, TypeDefId,
    TypeKind, VarInfo, decode_name,
    parse::{ListKind, Owner, Parser, VarFlavor, tag},
    version::Version,
};

/// The parts of a named declaration read before its kind-specific fields.
struct Named {
    id: DeclId,
    offset: usize,
    tag: u8,
    addr: u32,
    name: String,
}

/// A flagged declaration's flags plus the Delphi 8 type-index override.
struct Flagged {
    flags: DeclFlags,
    b2: u32,
}

impl Parser<'_> {
    /// Starts a named declaration (DCU32INT `TNameDecl.Create`): claims its
    /// address and reads its name.
    fn begin_named(&mut self, t: u8) -> Result<Named, Error> {
        let offset = self.def_start;
        let id = self.reserve_decl();
        let addr = self.add_addr(AddrTarget::Decl(id))?;
        let name = decode_name(self.r.name()?);
        Ok(Named {
            id,
            offset,
            tag: t,
            addr,
            name,
        })
    }

    /// Stores a finished declaration and returns its id.
    fn finish_named(&mut self, n: Named, flags: Option<DeclFlags>, kind: DeclKind) -> DeclId {
        let decl = Decl {
            offset: n.offset,
            tag: n.tag,
            name: n.name,
            addr: n.addr,
            flags,
            add_info_flags: 0,
            kind,
            modifiers: Vec::new(),
        };
        self.store_decl(n.id, decl);
        n.id
    }

    /// Reads the flag fields of a flagged declaration (DCU32INT
    /// `TNameFDecl.Create`). `no_inf` suppresses the stamp, as in member
    /// and parameter lists.
    fn read_flagged(&mut self, n: &Named, no_inf: bool) -> Result<Flagged, Error> {
        let mut f = DeclFlags {
            flags: self.r.uindex()?,
            ..DeclFlags::default()
        };
        if self.version == Version::DELPHI_6 {
            // Delphi 6 keeps deprecated / platform / library here.
            self.decl_add_flags.push((n.id, (f.flags >> 9) & 7));
        }
        if self.d(Version::DELPHI_8) {
            f.flags1 = self.r.uindex()?;
        }
        if self.d(Version::DELPHI_2009) {
            f.flags2 = self.r.uindex()?;
        }
        if !no_inf && f.flags & 0x40 != 0 {
            f.stamp = self.r.u32()?;
        }
        let mut b2 = 0;
        if self.d(Version::DELPHI_8) && f.flags1 & 0x80 != 0 {
            b2 = self.r.uindex()?;
            if self.version == Version::DELPHI_8 && f.flags & 0x08 != 0 {
                self.r.uindex()?;
            }
        }
        Ok(Flagged { flags: f, b2 })
    }

    /// Reads the package index of a declaration in a package unit
    /// (DCU32INT `TNameFDecl.ReadPkgNdx`, Delphi 3 - 8 and Kylix).
    fn read_pkg_ndx(&mut self, f: &mut DeclFlags) -> Result<(), Error> {
        let in_range = (self.version >= Version::DELPHI_3 && self.version <= Version::DELPHI_8)
            || self.version.is_kylix();
        if self.from_package && in_range {
            let mask = if self.version < Version::DELPHI_6 {
                0x80
            } else {
                0x100
            };
            if f.flags & mask != 0 {
                f.package_index = Some(self.r.uindex()?);
            }
        }
        Ok(())
    }

    /// Reads a type declaration (`*`, DCU32INT `TTypeDecl`): a name for a
    /// type-table slot.
    pub fn read_type_decl(&mut self, lk: ListKind) -> Result<DeclId, Error> {
        let no_inf = lk.is_args() || lk.is_member_list();
        let n = self.begin_named(tag::TYPE)?;
        let mut fl = self.read_flagged(&n, no_inf)?;
        let mut type_index = self.r.uindex()?;
        if self.d(Version::DELPHI_8) && fl.b2 != 0 {
            type_index = fl.b2;
        }
        self.read_pkg_ndx(&mut fl.flags)?;
        let name = n.name.clone();
        self.add_type_name(type_index, n.addr, &name)?;
        Ok(self.finish_named(n, Some(fl.flags), DeclKind::Type { type_index }))
    }

    /// Reads a variable-like declaration (DCU32INT `TVarDecl` and
    /// descendants without a memory image).
    pub fn read_var(&mut self, t: u8, flavor: VarFlavor) -> Result<DeclId, Error> {
        let n = self.begin_named(t)?;
        let mut fl = self.read_flagged(&n, false)?;
        let info = VarInfo {
            type_index: self.r.uindex()?,
            offset: self.r.uindex()?,
        };
        self.read_pkg_ndx(&mut fl.flags)?;
        if flavor == VarFlavor::Absolute {
            self.ref_addr(info.offset)?;
        }
        let kind = match flavor {
            VarFlavor::ThreadVar => DeclKind::ThreadVar(info),
            VarFlavor::Absolute => DeclKind::AbsoluteVar(info),
            VarFlavor::Special => DeclKind::SpecialVar(info),
            _ => DeclKind::Var(info),
        };
        Ok(self.finish_named(n, Some(fl.flags), kind))
    }

    /// Reads a declaration with a memory image (DCU32INT `TVarCDecl`
    /// descendants): typed constants, resource strings, VMT symbols.
    pub fn read_var_c(&mut self, t: u8, flavor: VarFlavor) -> Result<DeclId, Error> {
        let n = self.begin_named(t)?;
        let mut fl = self.read_flagged(&n, false)?;
        let info = VarInfo {
            type_index: self.r.uindex()?,
            offset: self.r.uindex()?,
        };
        self.read_pkg_ndx(&mut fl.flags)?;
        let kind = match flavor {
            VarFlavor::ResourceString => DeclKind::ResourceString(info),
            VarFlavor::Vmt => DeclKind::Vmt(info),
            _ => DeclKind::TypedConst(info),
        };
        Ok(self.finish_named(n, Some(fl.flags), kind))
    }

    /// Reads a true constant (`%`, DCU32INT `TConstDecl`).
    pub fn read_const_decl(&mut self) -> Result<DeclId, Error> {
        let n = self.begin_named(tag::CONST)?;
        let fl = self.read_flagged(&n, false)?;
        let type_index = self.r.uindex()?;
        let value = self.read_const_value(type_index)?;
        Ok(self.finish_named(n, Some(fl.flags), DeclKind::Const(value)))
    }

    /// Reads a pre-Delphi 8 compiler intrinsic (DCU32INT `TSysProcDecl`).
    pub fn read_sys_proc(&mut self) -> Result<DeclId, Error> {
        let n = self.begin_named(tag::SYS_PROC)?;
        let flags = self.r.uindex()?;
        let index = self.r.index()?;
        Ok(self.finish_named(n, None, DeclKind::SysProc { flags, index }))
    }

    /// Reads an `exports` entry (DCU32INT `TExportDecl`).
    pub fn read_export(&mut self) -> Result<DeclId, Error> {
        let n = self.begin_named(tag::EXPORT)?;
        let symbol = self.r.uindex()?;
        let index = self.r.uindex()?;
        if self.d(Version::DELPHI_10_4_SYDNEY) {
            self.r.uindex()?;
        }
        Ok(self.finish_named(n, None, DeclKind::Export { symbol, index }))
    }

    /// Reads a label (DCU32INT `TLabelDecl`).
    pub fn read_label(&mut self) -> Result<DeclId, Error> {
        let n = self.begin_named(tag::LABEL)?;
        let offset = self.r.uindex()?;
        if self.d(Version::DELPHI_8) {
            self.r.uindex()?;
        }
        if self.d(Version::DELPHI_2009) {
            self.r.uindex()?;
        }
        Ok(self.finish_named(n, None, DeclKind::Label { offset }))
    }

    /// Reads the fields shared by locals, parameters, fields and methods
    /// (DCU32INT `TLocalDecl.Create`).
    fn read_local_fields(&mut self, n: &Named, lk: ListKind) -> Result<Local, Error> {
        let is_method = matches!(n.tag, tag::METHOD | tag::CONSTR | tag::DESTR);
        let d2_method = self.version == Version::DELPHI_2 && is_method;
        let flags = self.r.uindex()?;
        let mut flags_x = if self.d(Version::DELPHI_8) {
            // Delphi 8 moved the class-member bit from 0x01 to 0x10.
            let x = self.r.uindex()?;
            ((x & !0x10) << 1) | ((x & 0x10) >> 4)
        } else {
            flags
        };
        flags_x &= !0x8000_0000;
        if self.d(Version::DELPHI_2009) {
            self.r.uindex()?;
        }
        if self.d(Version::DELPHI_XE4) && lk.is_args() && flags & 0x40 != 0 {
            // A [ref] parameter.
            self.r.u32()?;
        }
        let mut type_index = 0i32;
        let mut location = 0i32;
        if d2_method {
            location = self.r.uindex()? as i32;
        } else if is_method {
            type_index = self.r.index()?;
        } else {
            type_index = self.r.uindex()? as i32;
        }
        let interface_index = if matches!(lk, ListKind::Interface | ListKind::DispInterface) {
            Some(self.r.uindex()?)
        } else {
            None
        };
        if d2_method {
            type_index = self.r.index()?;
        } else if n.tag == tag::ABS_LOC_VAR {
            // An address, so unsigned: read signed, the 2-byte form wraps
            // from 8192 on.
            let addr = self.r.uindex()?;
            self.ref_addr(addr)?;
            location = addr as i32;
        } else if is_method {
            location = self.r.uindex()? as i32;
        } else {
            location = self.r.index()?;
        }
        Ok(Local {
            flags,
            flags_x,
            type_index,
            location,
            interface_index,
        })
    }

    /// Reads a local, parameter, result, field, class variable or
    /// dispinterface property (DCU32INT `TLocalDecl` and the descendants
    /// that add no fields).
    pub fn read_local(&mut self, t: u8, lk: ListKind) -> Result<DeclId, Error> {
        let n = self.begin_named(t)?;
        let local = self.read_local_fields(&n, lk)?;
        let kind = match t {
            tag::CLASS_VAR => DeclKind::ClassVar(local),
            tag::PROPERTY => DeclKind::DispProperty(local),
            _ => DeclKind::Local(local),
        };
        Ok(self.finish_named(n, None, kind))
    }

    /// Reads a method (DCU32INT `TMethodDecl`).
    ///
    /// From Delphi 2009 a method record ends in a run of flag bytes whose
    /// layout DCU32INT never decoded; it skips bytes from a per-version set
    /// of values that were observed there, and so does this reader.
    pub fn read_method(&mut self, t: u8, lk: ListKind) -> Result<DeclId, Error> {
        let n = self.begin_named(t)?;
        let local = self.read_local_fields(&n, lk)?;
        let in_interface = matches!(lk, ListKind::Interface | ListKind::DispInterface);
        let mut import = 0;
        if !in_interface {
            if self.msil && local.location != 0 {
                self.r.u8_if(1)?;
            }
            if self.d(Version::DELPHI_2009) && !n.name.is_empty() {
                self.r.u8()?;
            }
            if self.d(Version::DELPHI_7) || n.name.is_empty() {
                import = self.r.uindex()?;
            }
            if self.d(Version::DELPHI_2009) && t == tag::METHOD {
                let set = method_skip_set(self.version);
                while set.contains(&self.r.peek()?) {
                    self.r.u8()?;
                }
            }
        }
        let kind = match t {
            tag::CONSTR => MethodKind::Constructor,
            tag::DESTR => MethodKind::Destructor,
            _ => MethodKind::Method,
        };
        Ok(self.finish_named(
            n,
            None,
            DeclKind::Method(Method {
                kind,
                local,
                in_interface,
                import,
            }),
        ))
    }

    /// Reads a property (DCU32INT `TPropDecl`).
    pub fn read_property(&mut self) -> Result<DeclId, Error> {
        let n = self.begin_named(tag::PROPERTY)?;
        let flags = self.r.index()?;
        let flags_x = if self.d(Version::DELPHI_8) {
            let x = self.r.uindex()?;
            ((x & !0x10) << 1) | ((x & 0x10) >> 4)
        } else {
            flags as u32
        };
        if self.d(Version::DELPHI_2009) {
            self.r.uindex()?;
        }
        let type_index = self.r.uindex()?;
        let ndx = self.r.index()?;
        let index = self.r.index()?;
        let read = self.r.uindex()?;
        let write = self.r.uindex()?;
        let stored = self.r.uindex()?;
        for a in [read, write, stored] {
            if a != 0 {
                self.ref_addr(a)?;
            }
        }
        let (mut read_orig, mut write_orig) = (0, 0);
        if self.d(Version::DELPHI_8) {
            read_orig = self.r.uindex()?;
            if read_orig != 0 {
                self.ref_addr(read_orig)?;
            }
            write_orig = self.r.uindex()?;
            if write_orig != 0 {
                self.ref_addr(write_orig)?;
            }
            if self.msil {
                self.r.uindex()?;
                self.r.uindex()?;
            }
        }
        let default = self.r.index()?;
        Ok(self.finish_named(
            n,
            None,
            DeclKind::Property(Property {
                flags,
                flags_x,
                type_index,
                ndx,
                index,
                read,
                write,
                stored,
                read_orig,
                write_orig,
                default,
            }),
        ))
    }

    /// Reads a parameter default binding (DCU32INT `TSetDeftInfo`): it
    /// takes no address and has no name.
    pub fn read_set_default(&mut self) -> Result<DeclId, Error> {
        let offset = self.def_start;
        let id = self.reserve_decl();
        let constant = self.r.uindex()?;
        let param = self.r.uindex()?;
        self.store_decl(
            id,
            Decl {
                offset,
                tag: tag::SET_DEFT,
                name: String::new(),
                addr: 0,
                flags: None,
                add_info_flags: 0,
                kind: DeclKind::SetDefault { constant, param },
                modifiers: Vec::new(),
            },
        );
        Ok(id)
    }

    /// Reads a copied declaration (DCU32INT `TCopyDecl`): it takes the name
    /// of the declaration it copies and, before Delphi 10.4, appends an
    /// address of its own (address 0 from 10.4).
    pub fn read_copy_decl(&mut self, t: u8) -> Result<DeclId, Error> {
        let offset = self.def_start;
        let id = self.reserve_decl();
        // From 10.4 a copy no longer takes an address of its own.
        let addr = if self.d(Version::DELPHI_10_4_SYDNEY) {
            0
        } else {
            self.append_addr(AddrTarget::Decl(id))?
        };
        let base = self.r.uindex()?;
        let name = match self.addr_decl(base) {
            Some(b) => self
                .decls
                .get(b)
                .and_then(Option::as_ref)
                .map(|d| d.name.clone())
                .unwrap_or_default(),
            None => return Err(self.error(ErrorKind::Malformed("copy of an unknown declaration"))),
        };
        self.store_decl(
            id,
            Decl {
                offset,
                tag: t,
                name,
                addr,
                flags: None,
                add_info_flags: 0,
                kind: DeclKind::Copy { base },
                modifiers: Vec::new(),
            },
        );
        Ok(id)
    }

    /// Reads a string-constant block record (DCU32INT `TStrConstDecl`, a
    /// flagged declaration with a block offset and size).
    pub fn read_str_const(&mut self) -> Result<DeclId, Error> {
        let n = self.begin_named(tag::STR_CONST_REC)?;
        let fl = self.read_flagged(&n, false)?;
        let (offset, size);
        if self.d(Version::DELPHI_XE) {
            let o = self.r.uindex()?;
            offset = if o == 0 { u32::MAX } else { o };
            size = self.r.uindex()?;
        } else {
            self.r.uindex()?;
            offset = u32::MAX;
            let x = self.r.uindex()?;
            size = if self.msil { u32::MAX } else { x };
        }
        if self.d(Version::DELPHI_XE) {
            self.r.u8()?;
        }
        Ok(self.finish_named(n, Some(fl.flags), DeclKind::StrConst { offset, size }))
    }

    /// Reads a routine (DCU32INT `TProcDecl`); `no_inf` is set for the
    /// Delphi 8+ form of a compiler intrinsic.
    pub fn read_proc(
        &mut self,
        t: u8,
        embedded: Vec<DeclId>,
        no_inf: bool,
    ) -> Result<DeclId, Error> {
        let n = self.begin_named(t)?;
        let fl = self.read_flagged(&n, no_inf)?;
        let unnamed = self.is_unnamed(&n.name);
        let data_flag = match self.version {
            Version::DELPHI_6 => 0x8000,
            Version::DELPHI_7 => 0x800,
            _ => 0,
        };
        let mut proc = Proc {
            proc_flags: self.r.uindex()?,
            size: self.r.uindex()?,
            vproc: 0,
            result_type: 0,
            class_type: 0,
            call: CallConv::Register,
            unnamed,
            just_data: fl.flags.flags & data_flag != 0,
            params: Vec::new(),
            locals: Vec::new(),
            embedded,
            template_params: Vec::new(),
        };
        if self.d(Version::DELPHI_XE) {
            self.r.u8()?;
        }
        if !unnamed {
            if self.after(Version::DELPHI_2) {
                proc.vproc = self.r.uindex()?;
            }
            proc.result_type = self.r.uindex()?;
            if self.d(Version::DELPHI_8) {
                proc.class_type = self.r.uindex()?;
            }
            self.read_tag()?;
            proc.call = self.read_call_conv()?;
            if self.d(Version::DELPHI_2009) {
                if self.tag == tag::A5_INFO {
                    self.read_tag()?;
                }
                if self.tag == tag::A6_INFO {
                    proc.template_params = self.read_a6_list()?;
                    self.read_tag()?;
                }
            }
            let all = self.read_decl_list(ListKind::Args, Owner::Decl(n.id))?;
            self.expect_stop1("routine header")?;
            let split = all
                .iter()
                .position(|&id| {
                    !self
                        .decls
                        .get(id)
                        .and_then(Option::as_ref)
                        .is_some_and(|d| matches!(d.tag, tag::VAL | tag::AR_VAR))
                })
                .unwrap_or(all.len());
            let mut all = all;
            proc.locals = all.split_off(split);
            proc.params = all;
        }
        Ok(self.finish_named(n, Some(fl.flags), DeclKind::Proc(proc)))
    }

    /// Returns whether a routine record has no signature (DCU32INT
    /// `TProcDecl.IsUnnamed`): empty, `.`, `..` (Delphi 6 on), `.X`
    /// (Delphi 8 and Kylix) or `$X` (XE on).
    fn is_unnamed(&self, name: &str) -> bool {
        if name.is_empty() || name == "." {
            return true;
        }
        if self.d(Version::DELPHI_6) && name == ".." {
            return true;
        }
        if self.version.is_kylix() || self.version >= Version::DELPHI_8 {
            if name.starts_with('.') {
                return true;
            }
            if name.starts_with('$') && self.d(Version::DELPHI_XE) {
                return true;
            }
        }
        false
    }

    /// Reads an optional calling-convention tag (DCU32INT `ReadCallKind`):
    /// when the current tag is one, consumes it and reads the next tag.
    fn read_call_conv(&mut self) -> Result<CallConv, Error> {
        let call = match self.tag {
            tag::CDECL => CallConv::Cdecl,
            tag::PASCAL => CallConv::Pascal,
            tag::STDCALL => CallConv::Stdcall,
            tag::SAFECALL => CallConv::Safecall,
            _ => return Ok(CallConv::Register),
        };
        self.read_tag()?;
        Ok(call)
    }

    /// Reads a generic parameter list (DCU32INT `TA6Def`): a declaration
    /// list ended by `c`.
    fn read_a6_list(&mut self) -> Result<Vec<DeclId>, Error> {
        self.read_tag()?;
        let items = self.read_decl_list(ListKind::A6, Owner::None)?;
        self.expect_stop1("generic parameter list")?;
        Ok(items)
    }

    /// Reads a generic parameter list standing in a declaration list.
    pub fn read_a6_decl(&mut self, t: u8) -> Result<DeclId, Error> {
        let offset = self.def_start;
        let id = self.reserve_decl();
        let items = self.read_a6_list()?;
        self.store_decl(
            id,
            Decl {
                offset,
                tag: t,
                name: String::new(),
                addr: 0,
                flags: None,
                add_info_flags: 0,
                kind: DeclKind::TemplateParams(items),
                modifiers: Vec::new(),
            },
        );
        Ok(id)
    }

    /// Reads a unit extra-information record (DCU32INT `TUnitAddInfo`):
    /// flags, an index and a declaration list whose terminator the caller
    /// skips.
    pub fn read_unit_add_info(&mut self, t: u8) -> Result<DeclId, Error> {
        let n = self.begin_named(t)?;
        let fl = self.read_flagged(&n, false)?;
        let b = self.r.uindex()?;
        self.read_tag()?;
        let items = self.read_decl_list(ListKind::UnitAddInfo, Owner::Decl(n.id))?;
        Ok(self.finish_named(n, Some(fl.flags), DeclKind::UnitAddInfo { b, items }))
    }

    /// Reads a record with a declaration list (DCU32INT `TORecDecl`).
    pub fn read_orec(&mut self, t: u8) -> Result<DeclId, Error> {
        let n = self.begin_named(t)?;
        let dw = self.r.u32()?;
        let bytes = [self.r.u8()?, self.r.u8()?];
        self.read_tag()?;
        let items = self.read_decl_list(ListKind::A6, Owner::Decl(n.id))?;
        self.expect_stop1("record list")?;
        Ok(self.finish_named(n, None, DeclKind::ORec { dw, bytes, items }))
    }

    /// Reads a delay-loaded import record (DCU32INT `TDelayedImpRec`).
    pub fn read_delayed_import(&mut self) -> Result<DeclId, Error> {
        let n = self.begin_named(tag::DELAYED_IMP_INFO)?;
        let inf = self.r.u32()?;
        let target = self.r.uindex()?;
        self.ref_addr(target)?;
        Ok(self.finish_named(n, None, DeclKind::DelayedImport { inf, target }))
    }

    /// Reads generic parameters (`\xA7`, DCU32INT
    /// `TTemplateParmsDeclModifier.Read`) and attaches them to the last
    /// type defined (for actual parameters) or to the list's owner. From
    /// Delphi 10.4 each parameter is followed by its
    /// [`TypeSpelling`](crate::dcu::TypeSpelling).
    pub fn read_template_params(&mut self, owner: Owner) -> Result<(), Error> {
        let generic = self.r.uindex()?;
        let count = self.r.uindex()?;
        let mut params = Vec::new();
        let mut spellings = Vec::new();
        for _ in 0..count {
            params.push(self.r.uindex()?);
            if self.d(Version::DELPHI_10_4_SYDNEY) {
                spellings.push(self.read_type_spelling()?);
            }
        }
        let m = Modifier::TemplateParams {
            owner: generic,
            params,
            spellings,
        };
        let target = if generic != 0 {
            self.last_type_def().map(Owner::Type).unwrap_or(Owner::None)
        } else {
            owner
        };
        match target {
            Owner::Type(id) => self.type_mods.push((id, m)),
            Owner::Decl(id) => self.decl_mods.push((id, m)),
            Owner::None => {}
        }
        Ok(())
    }

    // ----- type definitions -----

    /// Reads a type definition (DCU32INT `TTypeDef` descendants): the
    /// common header, then the kind-specific fields.
    pub fn read_type_def(&mut self, t: u8) -> Result<TypeDefId, Error> {
        let offset = self.def_start;
        let rtti_size = self.r.uindex()?;
        let size = self.r.index()?;
        let name_decl = self.r.uindex()?;
        let mut extra = 0;
        if self.msil {
            self.r.uindex()?;
            self.r.uindex()?;
        } else if self.d(Version::DELPHI_2005) {
            extra = self.r.uindex()?;
        }
        let (id, type_index) = self.add_type_def()?;
        let kind = self.read_type_kind(t, id)?;
        self.store_type_def(
            id,
            TypeDef {
                offset,
                tag: t,
                type_index,
                rtti_size,
                size,
                name_decl,
                extra,
                kind,
                modifiers: Vec::new(),
            },
        );
        Ok(id)
    }

    /// Reads a member list ended by `c` (DCU32INT `TRecBaseDef.ReadFields`).
    fn read_fields(&mut self, lk: ListKind, owner: TypeDefId) -> Result<Vec<DeclId>, Error> {
        self.read_tag()?;
        let fields = self.read_decl_list(lk, Owner::Type(owner))?;
        self.expect_stop1("member list")?;
        Ok(fields)
    }

    /// Reads the kind-specific fields of a type definition.
    fn read_type_kind(&mut self, t: u8, id: TypeDefId) -> Result<TypeKind, Error> {
        Ok(match t {
            tag::RANGE_DEF
            | tag::CH_RANGE_DEF
            | tag::BOOL_RANGE_DEF
            | tag::WCHAR_RANGE_DEF
            | tag::WIDE_RANGE_DEF => {
                let base = self.r.uindex()?;
                let low = self.r.index64()?;
                let high = self.r.index64()?;
                let b = self.read_range_flag()?;
                let kind = match t {
                    tag::CH_RANGE_DEF => RangeKind::Char,
                    tag::BOOL_RANGE_DEF => RangeKind::Boolean,
                    tag::WCHAR_RANGE_DEF => RangeKind::WideChar,
                    tag::WIDE_RANGE_DEF => RangeKind::Wide,
                    _ => RangeKind::Integer,
                };
                TypeKind::Range {
                    kind,
                    base,
                    low,
                    high,
                    b,
                }
            }
            tag::ENUM_DEF => {
                let storage = self.r.uindex()?;
                if self.d(Version::DELPHI_2009) {
                    self.r.uindex()?;
                }
                let base = self.r.index()?.unsigned_abs();
                let low = self.r.index64()?;
                let high = self.r.index64()?;
                let b = self.read_range_flag()?;
                TypeKind::Enum {
                    base,
                    storage,
                    low,
                    high,
                    b,
                    members: Vec::new(),
                }
            }
            tag::FLOAT_DEF => {
                let mut b = self.r.u8()?;
                if self.version >= Version::DELPHI_XE3 && b & 0x80 != 0 {
                    b &= !0x80;
                    self.r.u8()?;
                }
                TypeKind::Float(match b {
                    0 => FloatKind::Real48,
                    1 => FloatKind::Single,
                    2 => FloatKind::Double,
                    3 => FloatKind::Extended,
                    4 => FloatKind::Comp,
                    5 => FloatKind::Currency,
                    _ => return Err(self.error(ErrorKind::Malformed("unknown float kind"))),
                })
            }
            tag::PTR_DEF | tag::DYN_ARRAY_DEF => {
                let target = self.r.uindex()?;
                if self.d(Version::DELPHI_2009) {
                    self.r.uindex()?;
                }
                if t == tag::DYN_ARRAY_DEF {
                    TypeKind::DynArray { element: target }
                } else {
                    TypeKind::Pointer { target }
                }
            }
            tag::TEXT_DEF => TypeKind::Text,
            tag::FILE_DEF => TypeKind::File {
                element: self.r.uindex()?,
            },
            tag::SET_DEF => TypeKind::Set {
                first_byte: self.r.u8()?,
                element: self.r.uindex()?,
            },
            tag::SHORT_STR_DEF | tag::STRING_DEF | tag::WIDE_STR_DEF | tag::UNICODE_STRING_DEF => {
                let b1 = self.r.u8()?;
                let index_type = self.r.uindex()?;
                let element_type = self.r.uindex()?;
                let code_page = if self.d(Version::DELPHI_2009) {
                    self.r.uindex()?
                } else {
                    0
                };
                match t {
                    tag::SHORT_STR_DEF => TypeKind::ShortString {
                        b1,
                        index_type,
                        element_type,
                        code_page,
                    },
                    _ => TypeKind::String {
                        kind: match t {
                            tag::WIDE_STR_DEF => StringKind::Wide,
                            tag::UNICODE_STRING_DEF => StringKind::Unicode,
                            _ => StringKind::Ansi,
                        },
                        b1,
                        index_type,
                        element_type,
                        code_page,
                    },
                }
            }
            tag::ARRAY_DEF => {
                let b1 = self.r.u8()?;
                let index_type = self.r.uindex()?;
                let element_type = self.r.uindex()?;
                if self.msil {
                    self.r.uindex()?;
                }
                TypeKind::Array {
                    b1,
                    index_type,
                    element_type,
                }
            }
            tag::VARIANT_DEF => TypeKind::Variant {
                b: if self.after(Version::DELPHI_2) {
                    self.r.u8()?
                } else {
                    0
                },
            },
            tag::OBJ_VMT_DEF => {
                let class_type = self.r.uindex()?;
                let vmt_size = self.r.uindex()?;
                if self.msil {
                    self.r.uindex()?;
                }
                TypeKind::ClassRef {
                    class_type,
                    vmt_size,
                }
            }
            tag::REC_DEF => {
                let b2 = self.r.u8()?;
                if self.msil {
                    if self.d(Version::DELPHI_2006) {
                        self.r.u8()?;
                    }
                    self.r.uindex()?;
                    self.read_class_interfaces()?;
                } else if self.d(Version::DELPHI_2005) {
                    if self.d(Version::DELPHI_2006) {
                        self.r.u8()?;
                        self.r.u8()?;
                    }
                    self.r.uindex()?;
                    if self.version >= Version::DELPHI_2009 {
                        self.r.uindex()?;
                        self.r.uindex()?;
                        if self.version >= Version::DELPHI_2010 {
                            self.r.uindex()?;
                        }
                    }
                }
                let members = self.read_fields(ListKind::Fields, id)?;
                TypeKind::Record { b2, members }
            }
            tag::PROC_TYPE_DEF => self.read_proc_type(id)?,
            tag::OBJ_DEF => {
                let b03 = self.r.u8()?;
                if self.d(Version::DELPHI_2006) {
                    self.r.uindex()?;
                }
                if self.d(Version::DELPHI_XE) {
                    self.r.u8()?;
                }
                let parent = self.r.uindex()?;
                let vmt_offset = self.r.uindex()?;
                let vmt_decl = self.r.index()?;
                let virtual_count = self.r.index()?;
                if self.d(Version::DELPHI_XE2) {
                    self.r.uindex()?;
                }
                let members = self.read_fields(ListKind::Fields, id)?;
                TypeKind::Object {
                    b03,
                    parent,
                    vmt_offset,
                    vmt_decl,
                    virtual_count,
                    members,
                }
            }
            tag::CLASS_DEF | tag::META_CLASS_DEF => self.read_class(t, id)?,
            tag::INTERFACE_DEF => {
                if self.d(Version::DELPHI_2009) {
                    self.r.u8()?;
                }
                let parent = self.r.uindex()?;
                let virtual_count = self.r.index()?;
                let mut guid = [0u8; 16];
                guid.copy_from_slice(self.r.bytes(16)?);
                let b = self.r.u8()?;
                let lk = if b & 0x04 == 0 {
                    ListKind::Interface
                } else {
                    ListKind::DispInterface
                };
                if self.d(Version::DELPHI_8) {
                    if self.d(Version::DELPHI_2010) {
                        self.r.uindex()?;
                    }
                    let count = self.r.uindex()?;
                    for _ in 0..count {
                        self.r.uindex()?;
                        self.r.uindex()?;
                        if self.msil && self.d(Version::DELPHI_2006) {
                            self.r.uindex()?;
                            self.r.uindex()?;
                        }
                    }
                }
                let members = self.read_fields(lk, id)?;
                TypeKind::Interface {
                    parent,
                    virtual_count,
                    guid,
                    b,
                    members,
                }
            }
            tag::VOID => {
                if self.d(Version::DELPHI_XE) {
                    self.r.u8()?;
                }
                TypeKind::Void
            }
            tag::TEMPLATE_ARG_DEF => {
                let count = self.r.uindex()?;
                let mut constraints = Vec::new();
                for _ in 0..count {
                    constraints.push(self.r.uindex()?);
                }
                TypeKind::TemplateArg {
                    constraints,
                    v5: self.r.uindex()?,
                }
            }
            tag::TEMPLATE_CALL => {
                if self.d(Version::DELPHI_XE) {
                    self.r.u8()?;
                }
                let generic = self.r.uindex()?;
                let count = self.r.uindex()?;
                let mut args = Vec::new();
                let mut spellings = Vec::new();
                for _ in 0..count {
                    args.push(self.r.uindex()?);
                    if self.d(Version::DELPHI_10_4_SYDNEY) {
                        spellings.push(self.read_type_spelling()?);
                    }
                }
                TypeKind::TemplateCall {
                    generic,
                    args,
                    spellings,
                    full: self.r.uindex()?,
                }
            }
            _ => return Err(self.tag_error("type definition")),
        })
    }

    /// Reads the trailing flag of a range or enumeration: an index from
    /// Delphi 8, a byte before.
    fn read_range_flag(&mut self) -> Result<u32, Error> {
        if self.d(Version::DELPHI_8) {
            self.r.uindex()
        } else {
            self.r.u8().map(u32::from)
        }
    }

    /// Reads a procedural type (DCU32INT `TProcTypeDef`): flags, result,
    /// then modifier tags up to the parameter block `j`.
    fn read_proc_type(&mut self, id: TypeDefId) -> Result<TypeKind, Error> {
        let flags = if self.after(Version::DELPHI_2) {
            self.r.uindex()?
        } else {
            0
        };
        let result = self.r.uindex()?;
        let mut call = CallConv::Register;
        self.read_tag()?;
        while self.tag != tag::EMBEDDED_PROC_START {
            if self.tag == tag::STOP1 {
                return Ok(TypeKind::ProcType {
                    flags,
                    result,
                    call,
                    params: Vec::new(),
                });
            }
            let c = self.read_call_conv()?;
            if c == CallConv::Register {
                if self.d(Version::DELPHI_2009) {
                    match self.tag {
                        tag::A7_INFO => self.read_template_params(Owner::Type(id))?,
                        tag::A8_INFO => {
                            self.r.uindex()?;
                        }
                        _ => {}
                    }
                }
                self.read_tag()?;
            } else {
                call = c;
            }
        }
        let params = self.read_fields(ListKind::ArgsT, id)?;
        Ok(TypeKind::ProcType {
            flags,
            result,
            call,
            params,
        })
    }

    /// Reads a class (DCU32INT `TClassDef`, and `TMetaClassDef` for `W`).
    fn read_class(&mut self, t: u8, id: TypeDefId) -> Result<TypeKind, Error> {
        if self.d(Version::DELPHI_2006) {
            self.r.u8()?;
        }
        if self.d(Version::DELPHI_2009) {
            if self.d(Version::DELPHI_XE2) {
                self.r.u8()?;
            } else {
                self.r.uindex()?;
            }
            self.r.u8()?;
        }
        let parent = self.r.uindex()?;
        let instance_rtti_size = self.r.uindex()?;
        let instance_size = self.r.index()?;
        let vmt_addr = self.r.uindex()?;
        let virtual_count = self.r.uindex()?;
        let ndx_fe = self.r.uindex()?;
        let property_count = self.r.uindex()?;
        let (b04, mask) = if self.d(Version::DELPHI_8) {
            (self.r.uindex()?, 0x08)
        } else {
            (u32::from(self.r.u8()?), 0x10)
        };
        if self.d(Version::DELPHI_2010) {
            self.r.uindex()?;
        }
        if self.from_package && b04 & mask != 0 && !self.msil {
            self.r.uindex()?;
            let n = self.r.uindex()?;
            for _ in 0..n {
                self.r.uindex()?;
            }
        }
        let mut meta = None;
        let mut interfaces = Vec::new();
        if self.after(Version::DELPHI_2) {
            if t == tag::META_CLASS_DEF {
                meta = Some(self.r.uindex()?);
                self.r.uindex()?;
            }
            interfaces = self.read_class_interfaces()?;
        }
        let members = self.read_fields(ListKind::Class, id)?;
        Ok(TypeKind::Class(ClassDef {
            parent,
            instance_rtti_size,
            instance_size,
            vmt_addr,
            virtual_count,
            ndx_fe,
            property_count,
            b04,
            interfaces,
            meta,
            members,
        }))
    }

    /// Reads a class's implemented-interface table (DCU32INT
    /// `ReadClassInterfaces`).
    fn read_class_interfaces(&mut self) -> Result<Vec<(u32, u32)>, Error> {
        let count = self.r.index()?;
        let mut out = Vec::new();
        for _ in 0..count.max(0) {
            let interface = self.r.uindex()?;
            if self.msil && self.d(Version::DELPHI_2006) {
                self.r.uindex()?;
                self.r.uindex()?;
            }
            let methods = self.r.uindex()?;
            out.push((interface, methods));
            if self.msil {
                for _ in 0..methods {
                    self.r.uindex()?;
                    self.r.uindex()?;
                }
            } else if self.d(Version::DELPHI_2006) {
                self.r.uindex()?;
                let matches = self.r.uindex()?;
                if self.d(Version::DELPHI_2010) {
                    self.r.uindex()?;
                    self.r.uindex()?;
                    for _ in 0..matches {
                        self.r.u8()?;
                        self.r.name()?;
                        self.r.uindex()?;
                        self.r.uindex()?;
                    }
                }
                if self.d(Version::DELPHI_10_4_SYDNEY) {
                    // The interface as the declaration spells it when that
                    // differs from its type (`IUnknown` for `IInterface`).
                    self.read_spelled_name()?;
                }
            }
        }
        Ok(out)
    }
}

/// Returns the flag-byte values DCU32INT skips at the end of a Delphi 2009+
/// method record for `version` (`TMethodDecl.Create`'s `sSkip` sets: each
/// release added values that appeared there).
fn method_skip_set(version: Version) -> &'static [u8] {
    const S12: &[u8] = &[0, 2, 4, 8, 0x10, 0x18, 0x20, 0x80, 0x84, 0x20, 0x21, 0x61];
    const S12A: &[u8] = &[0, 2, 4, 8, 0x10, 0x18, 0x20, 0x80, 0x84, 0x21, 0x61, 1];
    const S12B: &[u8] = &[
        0, 2, 4, 8, 0x10, 0x18, 0x20, 0x80, 0x84, 0x21, 0x61, 1, 0x28, 0x38,
    ];
    const S12C: &[u8] = &[
        0, 2, 4, 8, 0x10, 0x18, 0x20, 0x80, 0x84, 0x21, 0x61, 1, 0x28, 0x38, 0x42, 0x22, 0x09,
    ];
    const S17: &[u8] = &[
        0, 2, 4, 8, 0x10, 0x18, 0x20, 0x80, 0x84, 0x21, 0x61, 1, 0x28, 0x38, 0x42, 0x22, 0x09,
        0x47, 0x4F,
    ];
    const S20: &[u8] = &[
        0, 2, 4, 8, 0x10, 0x18, 0x20, 0x80, 0x84, 0x21, 0x61, 1, 0x28, 0x38, 0x42, 0x22, 0x09,
        0x47, 0x4F, 0x60,
    ];
    const S21: &[u8] = &[
        0, 2, 4, 8, 0x10, 0x18, 0x20, 0x80, 0x84, 0x21, 0x61, 1, 0x28, 0x38, 0x42, 0x22, 0x09,
        0x47, 0x4F, 0x60, 0xA1,
    ];
    const S24: &[u8] = &[
        0, 2, 4, 8, 0x10, 0x18, 0x20, 0x80, 0x84, 0x21, 0x61, 1, 0x28, 0x38, 0x42, 0x22, 0x09,
        0x47, 0x4F, 0x60, 0xA1, 0x07, 0x41,
    ];
    if version >= Version::DELPHI_10_2_TOKYO {
        S24
    } else if version >= Version::DELPHI_XE8 {
        S21
    } else if version >= Version::DELPHI_XE7 {
        S20
    } else if version >= Version::DELPHI_XE4 {
        S17
    } else if version >= Version::DELPHI_XE3 {
        S12C
    } else if version >= Version::DELPHI_XE2 {
        S12B
    } else if version >= Version::DELPHI_2010 {
        S12A
    } else {
        S12
    }
}
