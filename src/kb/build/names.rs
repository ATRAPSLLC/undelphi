//! Names: how the files spell declarations, compiler-generated names
//! (`.1` as `_DV_1`, unnamed routines as `_NF__2EA`) and imports.

use crate::{
    dcu::{self, AddrTarget, Decl, DeclKind},
    kb::build::UnitBuilder,
};

impl UnitBuilder<'_> {
    /// A declaration's name with `.` spelled out: `typed` for a typed
    /// constant or VMT, `true_const` for a true constant.
    pub fn decl_name_as(&self, decl: &Decl, typed: char, true_const: char) -> String {
        match decl.kind {
            DeclKind::Const(_) => dotted(&decl.name, true_const),
            _ if decl.name.is_empty() => self.decl_name(decl),
            _ => dotted(&decl.name, typed),
        }
    }

    /// The name a declaration has in a knowledge base: compiler-generated
    /// names are spelled out from the declaration's address.
    pub fn decl_name(&self, decl: &Decl) -> String {
        if decl.name.is_empty() {
            let prefix = match decl.kind {
                DeclKind::Proc(_) => "_NF_",
                DeclKind::Type { .. } => "_NT_",
                DeclKind::Method(_) | DeclKind::Local(_) => "_Nv_",
                _ => "_NV_",
            };
            format!("{prefix}_{:X}", decl.addr)
        } else {
            let kind = match decl.kind {
                DeclKind::Proc(_) => 'F',
                DeclKind::Type { .. } => 'T',
                _ => 'V',
            };
            dotted(&decl.name, kind)
        }
    }

    /// The name of what address-table slot `index` refers to.
    pub fn addr_name(&self, index: u32) -> String {
        match self.unit.addr(index) {
            Some(AddrTarget::Decl(id)) => self
                .unit
                .decls
                .get(id)
                .map(|d| self.decl_name(d))
                .unwrap_or_default(),
            _ => dotted(self.unit.addr_name(index).unwrap_or_default(), 'n'),
        }
    }

    /// An imported type's name, qualified with its unit's own spelling.
    pub fn import_ref(&self, unit: usize, symbol: usize) -> String {
        let u = self.unit.uses.get(unit);
        let name = u
            .and_then(|u| u.symbols.get(symbol))
            .map_or("", |s| s.name.as_str());
        let qualifier = u.map_or("", |u| {
            self.universe
                .unit(&u.name)
                .map_or(u.name.as_str(), |d| d.name.as_str())
        });
        format!("{qualifier}.{name}")
    }
}

/// Spells a compiler-generated name (`.1`, `.TList`) as the knowledge
/// bases do: `_D<kind>_` and the rest, where `kind` is `F` for a routine,
/// `T` a type, `V` a variable or constant, `n` an imported symbol and `v` a
/// parameter or local.
pub fn dotted(name: &str, kind: char) -> String {
    match name.strip_prefix('.') {
        Some(rest) => format!("_D{kind}_{rest}"),
        None => name.to_string(),
    }
}

/// Whether a declaration is the hidden `.` string constant that holds a
/// resource string's text (DCU32INT `TConstDecl.IsVisible`).
pub fn is_resource_text(decl: &Decl) -> bool {
    decl.name == "."
        && matches!(
            &decl.kind,
            DeclKind::Const(dcu::ConstValue {
                kind: None | Some(dcu::ConstKind::String),
                data: dcu::ConstData::Bytes(b),
                ..
            }) if b.len() > 8 && b.get(..4) == Some(&[0xFF; 4])
        )
}

/// Whether a name is made of dots only (`.`, `..`): the compiler's names
/// for anonymous data and default values.
pub fn is_dots(name: &str) -> bool {
    !name.is_empty() && name.bytes().all(|b| b == b'.')
}

/// Whether a type name is compiler-generated (`.1`, `:1`).
pub fn is_aux_name(name: &str) -> bool {
    name.starts_with('.') || name.starts_with(':') || name.contains('`')
}
