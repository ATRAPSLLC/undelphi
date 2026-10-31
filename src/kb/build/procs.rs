//! Routines: kind, calling convention, flags, result, code and
//! parameters.

use crate::{
    dcu::{self, CallConv, Decl, DeclKind, MethodKind},
    kb::{
        Arg, ArgTag, CallKind, DumpType, Proc, ProcKind,
        build::{UnitBuilder, names::dotted, text},
    },
};

impl UnitBuilder<'_> {
    /// Appends a record for every routine of the unit, in declaration
    /// order.
    pub fn procs(&self, out: &mut Vec<Proc<'static>>) {
        for (id, decl) in self.unit.decls.iter().enumerate() {
            let DeclKind::Proc(p) = &decl.kind else {
                continue;
            };
            let name = self.decl_name(decl);
            let result = self.result_type(p.result_type);
            let kind = match self.method_kinds.get(&decl.addr) {
                Some(MethodKind::Constructor) => ProcKind::Constructor,
                Some(MethodKind::Destructor) => ProcKind::Destructor,
                _ if result.is_some() => ProcKind::Function,
                _ => ProcKind::Procedure,
            };
            let args = self
                .unit
                .decls_of(&p.params)
                .filter_map(|a| self.arg(a))
                .collect();
            out.push(Proc {
                module: self.module,
                name: text(&name),
                embedded: !p.embedded.is_empty(),
                dump_type: if p.unnamed && self.data_refs.contains(&decl.addr) {
                    DumpType::Data
                } else {
                    DumpType::Code
                },
                kind,
                call: call_kind(p.call),
                vproc: i32::from_ne_bytes(p.vproc.to_ne_bytes()),
                type_def: result.map(|r| text(&self.type_ref(r))).unwrap_or_default(),
                dump: self.dump(id),
                args,
                locals: None,
            });
        }
    }

    /// The argument record of a parameter declaration.
    pub fn arg(&self, decl: &Decl) -> Option<Arg<'static>> {
        let DeclKind::Local(l) = &decl.kind else {
            return None;
        };
        let tag = match decl.tag {
            0x21 => ArgTag::Val,
            0x22 => ArgTag::Var,
            _ => return None,
        };
        Some(Arg {
            tag,
            flags: i32::from_ne_bytes(l.flags.to_ne_bytes()),
            location: l.location,
            name: text(&dotted(&decl.name, 'v')),
            type_def: text(&self.type_ref(u32::from_ne_bytes(l.type_index.to_ne_bytes()))),
        })
    }

    /// A routine's result type: `None` for a procedure, whose result
    /// slot names an untyped (`void`) type.
    pub fn result_type(&self, index: u32) -> Option<u32> {
        if index == 0 {
            return None;
        }
        match self.unit.type_def_at(index).map(|d| &d.kind) {
            Some(dcu::TypeKind::Void) => None,
            _ => Some(index),
        }
    }
}

/// The knowledge-base calling convention of a DCU one.
pub fn call_kind(call: CallConv) -> CallKind {
    match call {
        CallConv::Register => CallKind::Register,
        CallConv::Cdecl => CallKind::Cdecl,
        CallConv::Pascal => CallKind::Pascal,
        CallConv::Stdcall => CallKind::Stdcall,
        CallConv::Safecall => CallKind::Safecall,
    }
}
