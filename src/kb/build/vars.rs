//! Variables, thread variables and resource strings.

use crate::{
    dcu::{self, DeclKind},
    kb::{ResString, Var, VarKind},
};

use super::{UnitBuilder, names::is_dots, text, values::ansi_string_text};

impl UnitBuilder<'_> {
    /// Appends a record for every variable and thread variable of the
    /// unit.
    ///
    /// The files fill a variable's `absolute` target from a buffer that
    /// holds the last declaration shown when the variable has none: a
    /// type as `Name=Decl`, a variable or typed constant by name.
    pub fn vars(&self, out: &mut Vec<Var<'static>>) {
        let mut last = String::new();
        for &id in &self.unit.main {
            let Some(decl) = self.unit.decls.get(id) else {
                continue;
            };
            let name = self.decl_name(decl);
            let (kind, info) = match &decl.kind {
                // A variable is `V` until the first thread variable, `T`
                // from there on, across units.
                DeclKind::Var(v) if self.universe.thread_seen.get() => (VarKind::Thread, v),
                DeclKind::Var(v) => (VarKind::Var, v),
                DeclKind::ThreadVar(v) => {
                    self.universe.thread_seen.set(true);
                    (VarKind::Thread, v)
                }
                DeclKind::AbsoluteVar(v) if self.universe.thread_seen.get() => (VarKind::Thread, v),
                DeclKind::AbsoluteVar(v) => (VarKind::Var, v),
                DeclKind::SpecialVar(v) => (VarKind::Special, v),
                DeclKind::Type { type_index } => {
                    last = match self.unit.type_def_at(*type_index) {
                        Some(def) if def.name_decl == decl.addr || def.name_decl == 0 => {
                            format!("{name}={}", self.type_decl(def))
                        }
                        Some(_) => format!("{name}={}", self.type_ref(*type_index)),
                        None => format!("{name}={}", self.type_ref(*type_index)),
                    };
                    continue;
                }
                DeclKind::TypedConst(_) => {
                    last = name;
                    continue;
                }
                DeclKind::Const(cv) if is_dots(&decl.name) => {
                    last = self.const_value(cv);
                    continue;
                }
                DeclKind::Const(cv) => {
                    last = format!("{}={}", decl.name, self.const_value(cv));
                    continue;
                }
                _ => continue,
            };
            let absolute = matches!(decl.kind, DeclKind::AbsoluteVar(_));
            let abs_name = if absolute {
                self.addr_name(info.offset)
            } else if is_dots(&decl.name) {
                name.clone()
            } else if decl.name.starts_with('.') {
                // A compiler-generated `.N` variable shows none.
                String::new()
            } else {
                last.clone()
            };
            out.push(Var {
                module: self.module,
                name: text(&name),
                kind,
                type_def: text(&self.type_ref(info.type_index)),
                abs_name: text(&abs_name),
            });
            if !is_dots(&decl.name) {
                last = name;
            }
        }
    }

    /// Appends a record for every resource string of the unit: its name,
    /// type and text.
    pub fn res_strings(&self, out: &mut Vec<ResString<'static>>) {
        // Those declared inside routines too.
        for decl in &self.unit.decls {
            let DeclKind::ResourceString(v) = &decl.kind else {
                continue;
            };
            // The text is the `.` string constant declared right after.
            let context = match self
                .unit
                .decl_at(decl.addr.saturating_add(1))
                .map(|d| &d.kind)
            {
                Some(DeclKind::Const(dcu::ConstValue {
                    data: dcu::ConstData::Bytes(b),
                    ..
                })) => match ansi_string_text(b) {
                    Some(t) if t.is_empty() => "''".to_string(),
                    t => t.unwrap_or_default(),
                },
                _ => String::new(),
            };
            out.push(ResString {
                module: self.module,
                name: text(&self.decl_name(decl)),
                type_def: text(&self.type_ref(v.type_index)),
                context: text(&context),
            });
        }
    }
}
