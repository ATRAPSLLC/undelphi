//! Types: kind, size, declaration, RTTI, and the fields, properties and
//! methods of classes, records and interfaces.

use std::{collections::HashSet, ops::Range};

use crate::{
    dcu::{
        self, AddrTarget, CallConv, Decl, DeclId, DeclKind, ImportKind, MethodKind,
        TypeKind as DcuType, TypeTarget, Version,
    },
    kb::{
        Dump, Field as KbField, Method as KbMethod, ProcKind, Property as KbProperty, Scope, Text,
        Type, TypeKind as KbTypeKind,
    },
};

use super::{
    UnitBuilder,
    names::{dotted, is_aux_name},
    text,
};

impl UnitBuilder<'_> {
    /// Appends a record for every type declaration of the unit, in
    /// declaration order.
    pub fn types(&self, out: &mut Vec<Type<'static>>) {
        let mut seen = HashSet::new();
        // Types declared inside routines are left out.
        for &id in &self.unit.main {
            let Some(decl) = self.unit.decls.get(id) else {
                continue;
            };
            let DeclKind::Type { type_index } = decl.kind else {
                continue;
            };
            let Some(slot) = self.unit.type_slot(type_index) else {
                continue;
            };
            let name = self.decl_name(decl);
            // The first name of a type carries its definition, RTTI
            // (wherever the unit keeps it) and members; a later name repeats
            // the kind and size and refers to the first. An imported type
            // is an alias.
            let first = seen.insert(type_index);
            let record = match slot.target {
                TypeTarget::Def(def_id) => {
                    let Some(def) = self.unit.type_defs.get(def_id) else {
                        continue;
                    };
                    let mut record = Type {
                        size: u32::from_ne_bytes(def.size.to_ne_bytes()),
                        module: self.module,
                        name: text(&name),
                        kind: type_kind(&def.kind),
                        vmt_count: vmt_count(&def.kind),
                        decl: text(&self.type_ref(type_index)),
                        dump: Dump::default(),
                        fields: Vec::new(),
                        properties: Vec::new(),
                        methods: Vec::new(),
                    };
                    if first {
                        let rtti = match self.unit.addr(def.name_decl) {
                            Some(AddrTarget::Decl(named)) if self.memory.contains_key(&named) => {
                                named
                            }
                            _ => id,
                        };
                        record.decl = text(&self.type_decl(def));
                        record.dump = self.dump(rtti);
                        if let Some(members) = members_of(&def.kind) {
                            self.members(members, type_index, &mut record);
                            match def.kind {
                                dcu::TypeKind::Record { .. } => {
                                    self.number_cases(members, &mut record);
                                }
                                // Showing a class or object leaves the
                                // branch number at -1.
                                dcu::TypeKind::Class(_) | dcu::TypeKind::Object { .. } => {
                                    self.universe.case_state.set(-1);
                                }
                                _ => {}
                            }
                        }
                    }
                    record
                }
                TypeTarget::Import { unit, symbol } => {
                    let copy = self
                        .unit
                        .uses
                        .get(unit)
                        .and_then(|u| u.symbols.get(symbol))
                        .is_some_and(|s| matches!(s.kind, ImportKind::TypeCopy { .. }));
                    let target = self.import_ref(unit, symbol);
                    Type {
                        size: self.type_size(type_index),
                        module: self.module,
                        name: text(&name),
                        kind: KbTypeKind::Alias,
                        vmt_count: 0,
                        decl: text(&if !first {
                            self.type_ref(type_index)
                        } else if copy {
                            format!("type {target}")
                        } else {
                            target
                        }),
                        dump: if first {
                            self.dump(id)
                        } else {
                            Dump::default()
                        },
                        fields: Vec::new(),
                        properties: Vec::new(),
                        methods: Vec::new(),
                    }
                }
                TypeTarget::None => continue,
            };
            out.push(record);
        }
    }

    /// Fills a type's fields, properties and methods from its member
    /// declarations, in declaration order.
    pub fn members(&self, members: &[DeclId], owner: u32, record: &mut Type<'static>) {
        for d in self.unit.decls_of(members) {
            match &d.kind {
                DeclKind::Local(l) if d.tag == 0x2C => record.fields.push(KbField {
                    scope: scope(l.flags_x),
                    offset: l.location,
                    case: -1,
                    name: text(&if d.name.is_empty() {
                        self.universe
                            .field_at(self.unit, owner, l.location, 0)
                            .unwrap_or_default()
                    } else {
                        dotted(&d.name, 'v')
                    }),
                    type_def: text(&self.type_ref(u32::from_ne_bytes(l.type_index.to_ne_bytes()))),
                }),
                DeclKind::DispProperty(l) => record.properties.push(KbProperty {
                    scope: scope(l.flags_x),
                    // The access the dispinterface allows: 2 read-only, 6
                    // read-write.
                    index: l
                        .interface_index
                        .map_or(0, |i| i32::from_ne_bytes(i.to_ne_bytes())),
                    disp_id: l.location,
                    name: text(&d.name),
                    type_def: text(&self.type_ref(u32::from_ne_bytes(l.type_index.to_ne_bytes()))),
                    read: Text::default(),
                    write: Text::default(),
                    stored: Text::default(),
                }),
                DeclKind::Property(p) => record.properties.push(KbProperty {
                    scope: scope(p.flags_x),
                    index: p.index,
                    disp_id: 0,
                    name: text(&d.name),
                    // An array property's type is the accessor's anonymous
                    // signature, which the files leave out.
                    type_def: text(&if p.type_index == 0
                        || self.is_anonymous_proc_type(p.type_index)
                    {
                        String::new()
                    } else {
                        self.type_ref(p.type_index)
                    }),
                    read: text(&self.accessor(p.read, owner)),
                    write: text(&self.accessor(p.write, owner)),
                    stored: text(&self.accessor(p.stored, owner)),
                }),
                DeclKind::Method(m) => {
                    let (kind, prototype) = self.method(d, m);
                    record.methods.push(KbMethod {
                        scope: scope(m.local.flags_x),
                        kind,
                        prototype: text(&prototype),
                    });
                }
                _ => {}
            }
        }
    }

    /// Numbers the variant-part branches a record's fields belong to, as
    /// DCU32INT's `TRecDef.Show` reconstructs them from the offsets (the
    /// DCU does not record them): a field gets the 1-based number of the
    /// innermost branch it lies in. The IDR files keep the number of the
    /// last branch entered for fields outside any branch, also across
    /// records of a unit, so a record without a variant part repeats the
    /// last number used, in this unit or the units before it (`-1` before
    /// any).
    pub fn number_cases(&self, members: &[DeclId], record: &mut Type<'static>) {
        let fields: Vec<(i32, i32)> = self
            .unit
            .decls_of(members)
            .filter_map(|d| match &d.kind {
                DeclKind::Local(l) if d.tag == 0x2C => Some((
                    l.location,
                    self.type_size_signed(u32::from_ne_bytes(l.type_index.to_ne_bytes())),
                )),
                _ => None,
            })
            .collect();
        let mut cases = vec![-1; fields.len()];
        let mut current = self.universe.case_state.get();
        show_case(&fields, 0..fields.len(), &mut cases, &mut current);
        self.universe.case_state.set(current);
        for (field, case) in record.fields.iter_mut().zip(cases) {
            field.case = case;
        }
    }

    /// The size of a value of type `index` following imports, `-1` when
    /// unknown.
    pub fn type_size_signed(&self, index: u32) -> i32 {
        self.universe
            .resolve(self.unit, index)
            .and_then(|(u, i)| u.type_def_at(i))
            .map_or(-1, |d| d.size)
    }

    /// Whether type `index` is an anonymous procedural type.
    pub fn is_anonymous_proc_type(&self, index: u32) -> bool {
        let named = self
            .unit
            .type_slot(index)
            .and_then(|s| s.name.as_deref())
            .is_some_and(|n| !is_aux_name(n));
        !named
            && matches!(
                self.unit.type_def_at(index).map(|d| &d.kind),
                Some(dcu::TypeKind::ProcType { .. })
            )
    }

    /// The name of a property accessor (a field or routine), or empty.
    pub fn accessor(&self, index: u32, owner: u32) -> String {
        match self.unit.addr(index) {
            None | Some(AddrTarget::Empty) => String::new(),
            // A compiler-added field for part of a record field reads as
            // the field covering it, which may be inherited.
            Some(AddrTarget::Decl(id))
                if self
                    .unit
                    .decls
                    .get(id)
                    .is_some_and(|d| d.name.is_empty() && d.tag == 0x2C) =>
            {
                let offset = match self.unit.decls.get(id).map(|d| &d.kind) {
                    Some(DeclKind::Local(l)) => l.location,
                    _ => 0,
                };
                self.universe
                    .field_at(self.unit, owner, offset, 0)
                    .unwrap_or_default()
            }
            // An unnamed symbol of another unit.
            Some(AddrTarget::Import { .. })
                if self.unit.addr_name(index).is_some_and(str::is_empty) =>
            {
                format!("_Nv__{index:X}")
            }
            // A constant of another unit (`stored False`) is qualified.
            Some(AddrTarget::Import { unit, .. }) => format!(
                "{}.{}",
                self.unit.uses.get(unit).map_or("", |u| {
                    self.universe
                        .unit(&u.name)
                        .map_or(u.name.as_str(), |d| d.name.as_str())
                }),
                self.addr_name(index)
            ),
            Some(_) => self.addr_name(index),
        }
    }

    /// A method's kind and prototype, as DCU32INT's `TMethodDecl.Show`
    /// prints them without its comments: `procedure Name(val Self:T;...);
    /// override; virtual` for a class, `method function Name(...):R
    /// stdcall` for an interface.
    pub fn method(&self, decl: &Decl, m: &dcu::Method) -> (ProcKind, String) {
        let flags = m.local.flags;
        let mut out = String::new();
        if m.local.flags_x & 1 != 0 {
            out.push_str("class ");
        }
        if m.in_interface {
            // An interface method's signature is the procedural type its
            // location names (DCU32INT `GetTypeDef(NDX)`).
            let def = self
                .unit
                .type_def_at(u32::from_ne_bytes(m.local.location.to_ne_bytes()));
            if let Some(dcu::TypeKind::ProcType {
                flags: pflags,
                result,
                call,
                params,
            }) = def.map(|d| &d.kind)
            {
                let result = self.result_type(*result);
                // DCU32INT also looks the location up as an address and,
                // when a routine sits there, names that routine's kind.
                let at = u32::from_ne_bytes(m.local.location.to_ne_bytes());
                out.push_str(match self.unit.decl_at(at).map(|d| &d.kind) {
                    Some(DeclKind::Proc(p)) if self.result_type(p.result_type).is_some() => {
                        "function "
                    }
                    Some(DeclKind::Proc(_)) => "procedure ",
                    _ => "method ",
                });
                out.push_str(if result.is_some() {
                    "function "
                } else {
                    "procedure "
                });
                out.push_str(&decl.name);
                out.push_str(&self.proc_type_tail(*pflags, result, *call, params));
                let kind = if result.is_some() {
                    ProcKind::Function
                } else {
                    ProcKind::Procedure
                };
                return (kind, out);
            }
            out.push_str(&decl.name);
            return (ProcKind::Method, out);
        }
        let routine = match self
            .unit
            .decl_at(u32::from_ne_bytes(m.local.location.to_ne_bytes()))
        {
            Some(Decl {
                kind: DeclKind::Proc(p),
                ..
            }) => Some(p),
            _ => None,
        };
        let result = routine.and_then(|p| self.result_type(p.result_type));
        let kind = match (m.kind, routine) {
            (MethodKind::Constructor, _) => ProcKind::Constructor,
            (MethodKind::Destructor, _) => ProcKind::Destructor,
            (MethodKind::Method, None) => ProcKind::Method,
            (MethodKind::Method, Some(_)) if result.is_some() => ProcKind::Function,
            (MethodKind::Method, Some(_)) => ProcKind::Procedure,
        };
        out.push_str(match kind {
            ProcKind::Constructor => "constructor ",
            ProcKind::Destructor => "destructor ",
            ProcKind::Function => "function ",
            ProcKind::Procedure => "procedure ",
            _ => "method ",
        });
        out.push_str(&self.decl_name(decl));
        if routine.is_none() {
            // DCU32INT's `: ` and the unresolved address.
            out.push_str(":?");
        }
        if let Some(p) = routine {
            let args = self.arg_list(&p.params);
            if !args.is_empty() {
                out.push('(');
                out.push_str(&args.join(";"));
                out.push(')');
            }
            if let Some(r) = result {
                out.push(':');
                out.push_str(&self.type_ref(r));
            }
            match p.call {
                CallConv::Register => {}
                CallConv::Cdecl => out.push_str(";cdecl"),
                CallConv::Pascal => out.push_str(";pascal"),
                CallConv::Stdcall => out.push_str(";stdcall"),
                CallConv::Safecall => out.push_str(";safecall"),
            }
            if self.unit.version.delphi_at_least(Version::DELPHI_4)
                && !self.unit.version.delphi_at_least(Version::DELPHI_2005)
                && p.vproc & 0x1000 != 0
            {
                out.push_str(";overload");
            }
        }
        if flags & 0x20 != 0 {
            out.push_str("; override");
        }
        match flags & 0xC0 {
            0x40 => out.push_str("; virtual"),
            0x80 => out.push_str("; dynamic"),
            0xC0 => out.push_str("; message"),
            _ => {}
        }
        (kind, out)
    }

    /// Parameters as `mode Name:Type` (`val`, `var` or `const`), default
    /// values as `_DC_=value`.
    pub fn arg_list(&self, params: &[DeclId]) -> Vec<String> {
        self.unit
            .decls_of(params)
            .filter_map(|d| {
                // A parameter's default value: `_DC_=Nil`.
                if let DeclKind::Const(cv) = &d.kind {
                    return Some(format!("{}={}", dotted(&d.name, 'C'), self.const_value(cv)));
                }
                let DeclKind::Local(l) = &d.kind else {
                    return None;
                };
                let mode = match d.tag {
                    0x21 if l.flags & 7 == 1 => "const",
                    0x21 => "val",
                    0x22 if l.flags & 7 == 1 => "const",
                    0x22 => "var",
                    _ => return None,
                };
                Some(format!(
                    "{mode} {}:{}",
                    dotted(&d.name, 'v'),
                    self.type_ref(u32::from_ne_bytes(l.type_index.to_ne_bytes()))
                ))
            })
            .collect()
    }

    /// The size of a value of type `index`, following imports;
    /// `0xFFFFFFFF` when the defining unit is not described.
    pub fn type_size(&self, index: u32) -> u32 {
        self.universe
            .resolve(self.unit, index)
            .and_then(|(u, i)| u.type_def_at(i))
            .map_or(u32::MAX, |d| u32::from_ne_bytes(d.size.to_ne_bytes()))
    }

    /// A type declaration's right-hand side, as the knowledge bases keep
    /// it: a record's is empty.
    pub fn type_decl(&self, def: &dcu::TypeDef) -> String {
        match &def.kind {
            dcu::TypeKind::Record { .. } => String::new(),
            _ => self.type_def(def),
        }
    }
}

/// DCU32INT's `ShowCase` over `fields[range]` (offset, size), assigning
/// each field the branch number `current` holds when it is shown.
pub fn show_case(fields: &[(i32, i32)], range: Range<usize>, cases: &mut [i32], current: &mut i32) {
    let span = |size: i32| if size < 0 { 1 } else { size };
    let at = |i: usize| fields.get(i).copied().unwrap_or((0, 0));
    // The smallest offset among fields starting before the previous one
    // ends: where a variant part starts.
    let mut case_ofs = i32::MAX;
    let mut prev_end = -1;
    for i in range.clone() {
        let (ofs, size) = at(i);
        if ofs < prev_end {
            case_ofs = case_ofs.min(ofs);
        }
        prev_end = ofs.saturating_add(span(size));
    }
    // The first field of the variant part.
    let start = (case_ofs < i32::MAX)
        .then(|| {
            range.clone().find(|&i| {
                let (ofs, size) = at(i);
                ofs > case_ofs || ofs == case_ofs && size != 0
            })
        })
        .flatten();
    let prefix_end = start.unwrap_or(range.end);
    for i in range.start..prefix_end {
        if let Some(c) = cases.get_mut(i) {
            *c = *current;
        }
    }
    let Some(mut branch) = start else {
        return;
    };
    let mut number: i32 = 0;
    // Bounded by the number of fields.
    for _ in 0..fields.len() {
        number = number.saturating_add(1);
        *current = number;
        let (ofs, size) = at(branch);
        let ofs_max = ofs.saturating_add(span(size));
        let next = (branch.saturating_add(1)..range.end).find(|&j| {
            let (o, _) = at(j);
            o >= case_ofs && o < ofs_max
        });
        show_case(fields, branch..next.unwrap_or(range.end), cases, current);
        match next {
            Some(n) => branch = n,
            None => break,
        }
    }
}

/// The member declarations of a type with members.
pub fn members_of(kind: &dcu::TypeKind) -> Option<&[DeclId]> {
    match kind {
        dcu::TypeKind::Record { members, .. }
        | dcu::TypeKind::Object { members, .. }
        | dcu::TypeKind::Interface { members, .. } => Some(members),
        dcu::TypeKind::Class(c) => Some(&c.members),
        _ => None,
    }
}

/// The knowledge-base visibility of DCU visibility flags.
pub fn scope(flags_x: u32) -> Scope {
    match flags_x & 0x0E {
        0x00 => Scope::Private,
        0x04 => Scope::Protected,
        0x02 => Scope::Public,
        0x0A => Scope::Published,
        other => Scope::Other(u8::try_from(other).unwrap_or(0)),
    }
}

/// The knowledge-base kind of a DCU type definition (DCU32INT's record
/// tag letters).
pub fn type_kind(kind: &dcu::TypeKind) -> KbTypeKind {
    match kind {
        DcuType::Range { .. } | DcuType::Enum { .. } => KbTypeKind::Range,
        DcuType::Float(_) => KbTypeKind::Float,
        DcuType::Pointer { .. } => KbTypeKind::Pointer,
        DcuType::Text => KbTypeKind::Text,
        DcuType::File { .. } => KbTypeKind::File,
        DcuType::Set { .. } => KbTypeKind::Set,
        // DCU32INT reads a short string as an array.
        DcuType::ShortString { .. } => KbTypeKind::Array,
        DcuType::String { .. } => KbTypeKind::String,
        DcuType::Array { .. } | DcuType::DynArray { .. } => KbTypeKind::Array,
        DcuType::Variant { .. } => KbTypeKind::Variant,
        DcuType::ClassRef { .. } => KbTypeKind::Object,
        DcuType::Record { .. } => KbTypeKind::Record,
        DcuType::ProcType { .. } => KbTypeKind::ProcType,
        DcuType::Object { .. } => KbTypeKind::Other(b'N'),
        DcuType::Class(_) => KbTypeKind::Class,
        DcuType::Interface { .. } => KbTypeKind::Interface,
        DcuType::Void => KbTypeKind::Other(b'@'),
        DcuType::TemplateArg { .. } | DcuType::TemplateCall { .. } => KbTypeKind::Other(b'?'),
    }
}

/// The number of virtual methods a class, object or interface declares.
pub fn vmt_count(kind: &dcu::TypeKind) -> u16 {
    let n = match kind {
        dcu::TypeKind::Class(c) => i64::from(c.virtual_count),
        dcu::TypeKind::Object { virtual_count, .. } => i64::from(*virtual_count),
        dcu::TypeKind::Interface { virtual_count, .. } => i64::from(*virtual_count),
        _ => 0,
    };
    u16::try_from(n).unwrap_or(0)
}
