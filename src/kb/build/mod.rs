//! Building knowledge-base records from DCUs, the way IDR's knowledge bases
//! were made: one module per unit, its declarations as DCU32INT shows them.
//!
//! IDR's maker is not published; its output is reproduced from its files.
//! Built from the Delphi 6 library, every record of kb6 whose unit decodes
//! to the same DCU comes out identical (`tests/kb.rs`), which pins these
//! conventions:
//!
//! - Units are numbered in the order of their uppercased names and named
//!   after their main source file (`classes` for `classes.pas`); a unit's
//!   uses are sorted as Delphi's `AnsiCompareText` sorts.
//! - Compiler-generated names are spelled out ([`names`]): `.1` as
//!   `_DV_1`, an unnamed routine as `_NF__2EA` (its address index).
//! - Type expressions are DCU32INT's ([`print`]); constant values follow
//!   the maker's own format ([`values`]).
//! - Some fields carry the maker's leftover state from earlier records: a
//!   record field's variant case, a variable's `absolute` target and its
//!   kind (`V` before the first thread variable, `T` after). The state runs
//!   across units in module order, so a knowledge base is built in one
//!   [`build`] call.
//! - Constants are written in name order across units; the other sections
//!   unit by unit in declaration order. Both lookup orders are sorted as
//!   Delphi's `TList.Sort` sorts (see [`Contents::to_bytes`]).
//!
//! The rules come from Delphi 6 units; the records of later releases
//! (Unicode names, generics, 64-bit fixup kinds) are built the same way
//! but have no published knowledge base to check against yet.

use std::{
    borrow::Cow,
    cell::{Cell, RefCell},
    cmp::Ordering,
    collections::{HashMap, HashSet},
    ops::Range,
    sync::Arc,
};

use crate::{
    dcu::{self, DeclId, DeclKind, MethodKind, SourceKind, TypeTarget, Unit},
    kb::{Contents, Module, Text, Use, cmp_ignore_case},
};

mod dump;
mod names;
mod print;
mod procs;
mod types;
mod values;
mod vars;

/// A unit to describe, with the name of the file it was read from (which
/// the module record keeps, in the file system's spelling).
#[derive(Debug, Clone)]
pub struct UnitFile {
    /// The DCU's file name (`SysUtils.dcu`).
    pub file_name: String,
    /// The decoded unit.
    pub unit: Arc<Unit>,
}

/// The name IDR's knowledge bases give a unit: its main source file's name
/// without the extension (`classes` for Delphi 6's `classes.pas`), else the
/// declared name.
fn module_name(unit: &Unit) -> &str {
    let stems = || {
        unit.sources
            .iter()
            .filter(|s| s.kind == SourceKind::Source)
            .filter_map(|s| s.name.rsplit_once('.').map(|(stem, _)| stem))
    };
    // The unit's own source, spelled as its file is: a unit compiled with
    // include files lists those as sources too, and from 10 Seattle
    // `System.dcu` lists `LocaleData.inc` first.
    stems()
        .find(|stem| stem.eq_ignore_ascii_case(&unit.name))
        .or_else(|| stems().next())
        .unwrap_or(&unit.name)
}

/// Builds the records of a knowledge base describing `units`.
///
/// Modules are numbered in the order of their uppercased names, as the
/// published files number them.
#[must_use]
pub fn build(units: &[UnitFile]) -> Contents<'static> {
    let mut order: Vec<&UnitFile> = units.iter().collect();
    order.sort_by(|a, b| {
        cmp_ignore_case(
            module_name(&a.unit).as_bytes(),
            module_name(&b.unit).as_bytes(),
        )
    });
    // A used unit or package is the module of that name, if there is one.
    let id_of = |name: &str| -> u16 {
        order
            .iter()
            .position(|u| module_name(&u.unit).eq_ignore_ascii_case(name))
            .and_then(|i| u16::try_from(i).ok())
            .unwrap_or(u16::MAX)
    };
    let mut contents = Contents::default();
    for (id, file) in order.iter().enumerate() {
        let unit = &file.unit;
        let id = u16::try_from(id).unwrap_or(u16::MAX);
        let mut uses: Vec<Use<'static>> = unit
            .uses
            .iter()
            .map(|u| Use {
                id: id_of(&u.name),
                name: text(&u.name),
            })
            .collect();
        uses.sort_by(|a, b| ansi_compare_text(a.name.as_bytes(), b.name.as_bytes()));
        contents.modules.push(Module {
            id,
            name: text(module_name(unit)),
            file_name: text(&file.file_name),
            uses,
        });
    }
    let universe = Universe {
        units: order
            .iter()
            .map(|u| (u.unit.name.to_ascii_lowercase(), &*u.unit))
            .collect(),
        case_state: Cell::new(-1),
        thread_seen: Cell::new(false),
    };
    for (id, file) in order.iter().enumerate() {
        let id = u16::try_from(id).unwrap_or(u16::MAX);
        let builder = UnitBuilder::new(&file.unit, id, &universe);
        builder.consts(&mut contents.consts);
        builder.types(&mut contents.types);
        builder.vars(&mut contents.vars);
        builder.res_strings(&mut contents.res_strings);
        builder.procs(&mut contents.procs);
    }
    // The files keep constants in name order across units (ties in module
    // order); every other section unit by unit, in declaration order.
    contents.consts.sort_by(|a, b| {
        cmp_ignore_case(a.name.as_bytes(), b.name.as_bytes()).then(a.module.cmp(&b.module))
    });
    contents
}

/// Builds the records of one unit.
struct UnitBuilder<'u> {
    unit: &'u Unit,
    module: u16,
    /// Every unit being described.
    universe: &'u Universe<'u>,
    /// Each declaration's part of the unit's block.
    memory: HashMap<DeclId, Range<usize>>,
    /// The kind of each method, by the address of its routine.
    method_kinds: HashMap<u32, MethodKind>,
    /// The addresses a fixup in a typed constant's data refers to.
    data_refs: HashSet<u32>,
    /// The class references the value printer showed (see
    /// [`UnitBuilder::consts`]).
    shown_class_refs: RefCell<Vec<u32>>,
}

impl<'u> UnitBuilder<'u> {
    fn new(unit: &'u Unit, module: u16, universe: &'u Universe<'u>) -> Self {
        let memory = unit
            .memory()
            .into_iter()
            .map(|m| (m.decl, m.range))
            .collect();
        let method_kinds = unit
            .decls
            .iter()
            .filter_map(|d| match &d.kind {
                DeclKind::Method(m) => {
                    Some((u32::from_ne_bytes(m.local.location.to_ne_bytes()), m.kind))
                }
                _ => None,
            })
            .collect();
        let data_refs = unit
            .memory()
            .into_iter()
            .filter(|m| {
                unit.decls.get(m.decl).is_some_and(|d| match &d.kind {
                    DeclKind::TypedConst(v) => universe.contains_string(unit, v.type_index, 0),
                    _ => false,
                })
            })
            .flat_map(|m| {
                unit.fixups
                    .iter()
                    .filter(move |f| {
                        usize::try_from(f.offset).is_ok_and(|o| m.range.contains(&o))
                            && !unit.is_marker(f)
                    })
                    .map(|f| f.target)
            })
            .collect();
        Self {
            unit,
            module,
            universe,
            memory,
            method_kinds,
            data_refs,
            shown_class_refs: RefCell::new(Vec::new()),
        }
    }
}

/// Every unit a knowledge base describes, by lowercased name: what a type
/// imported from one of them resolves to.
struct Universe<'u> {
    units: HashMap<String, &'u Unit>,
    /// The variant branch number last entered (see
    /// [`UnitBuilder::number_cases`]).
    case_state: Cell<i32>,
    /// Whether a thread variable has been shown (see
    /// [`UnitBuilder::vars`]).
    thread_seen: Cell<bool>,
}

impl<'u> Universe<'u> {
    /// Returns the unit named `name` (ignoring ASCII case).
    pub fn unit(&self, name: &str) -> Option<&'u Unit> {
        self.units.get(&name.to_ascii_lowercase()).copied()
    }

    /// Follows type `index` of `unit` through imports to the unit that
    /// defines it and the type's index there.
    pub fn resolve<'a>(&self, mut unit: &'a Unit, mut index: u32) -> Option<(&'a Unit, u32)>
    where
        'u: 'a,
    {
        // Bounded: a chain of re-exported imports is short.
        for _ in 0..16 {
            let TypeTarget::Import { unit: u, symbol } = unit.type_slot(index)?.target else {
                return Some((unit, index));
            };
            let import = unit.uses.get(u)?;
            let name = &import.symbols.get(symbol)?.name;
            let target = self.unit(&import.name)?;
            index = target.decls_of(&target.main).find_map(|d| match d.kind {
                DeclKind::Type { type_index } if d.name.eq_ignore_ascii_case(name) => {
                    Some(type_index)
                }
                _ => None,
            })?;
            unit = target;
        }
        None
    }

    /// The name of the named field of record or class `index` (of `unit`)
    /// whose bytes include `offset`, searching a class's ancestors too: an
    /// unnamed field the compiler adds for a property that reads part of a
    /// record field is shown under that field's name.
    pub fn field_at(&self, unit: &Unit, index: u32, offset: i32, depth: u8) -> Option<String> {
        if depth > 32 {
            return None;
        }
        let (unit, index) = self.resolve(unit, index)?;
        let def = unit.type_def_at(index)?;
        let (members, parent) = match &def.kind {
            dcu::TypeKind::Class(c) => (c.members.as_slice(), c.parent),
            dcu::TypeKind::Record { members, .. } => (members.as_slice(), 0),
            dcu::TypeKind::Object {
                members, parent, ..
            } => (members.as_slice(), *parent),
            _ => return None,
        };
        let own = unit.decls_of(members).find(|d| match &d.kind {
            DeclKind::Local(l) if d.tag == 0x2C && !d.name.is_empty() => {
                let size = self
                    .resolve(unit, u32::from_ne_bytes(l.type_index.to_ne_bytes()))
                    .and_then(|(u, i)| u.type_def_at(i))
                    .map_or(1, |d| d.size.max(1));
                l.location <= offset && offset < l.location.saturating_add(size)
            }
            _ => false,
        });
        match own {
            Some(d) => Some(d.name.clone()),
            None if parent != 0 => self.field_at(unit, parent, offset, depth.saturating_add(1)),
            None => None,
        }
    }

    /// Whether a value of type `index` of `unit` holds a long string: a
    /// string, or an array or record with one inside. DCU32INT marks an
    /// anonymous block as data when it shows a typed constant's string
    /// that points at it.
    pub fn contains_string(&self, unit: &Unit, index: u32, depth: u8) -> bool {
        if depth > 16 {
            return false;
        }
        let depth = depth.saturating_add(1);
        let Some((unit, index)) = self.resolve(unit, index) else {
            // An import from a unit not described: judge by name.
            return matches!(
                unit.type_name(index),
                Some("AnsiString" | "String" | "WideString")
            );
        };
        match unit.type_def_at(index).map(|d| &d.kind) {
            Some(dcu::TypeKind::String { .. }) => true,
            Some(dcu::TypeKind::Array { element_type, .. }) => {
                self.contains_string(unit, *element_type, depth)
            }
            Some(dcu::TypeKind::Record { members, .. }) => unit.decls_of(members).any(|d| {
                matches!(&d.kind, DeclKind::Local(l) if d.tag == 0x2C
                    && self.contains_string(unit, u32::from_ne_bytes(l.type_index.to_ne_bytes()), depth))
            }),
            _ => false,
        }
    }
}

/// Compares two names as Delphi's `AnsiCompareText` does on an English
/// system for the characters names use (Windows `CompareString`, word
/// sort, case-insensitive): hyphens and apostrophes count only when
/// everything else ties, punctuation sorts before digits and digits before
/// letters.
fn ansi_compare_text(a: &[u8], b: &[u8]) -> Ordering {
    /// Primary weight of a character; `None` for those word sort skips.
    fn weight(c: u8) -> Option<(u8, u8)> {
        const SYMBOLS: &[u8] = b" !\"#$%&()*,./:;?@[\\]^_`{|}~+<=>";
        match c {
            b'-' | b'\'' => None,
            b'0'..=b'9' => Some((2, c)),
            b'a'..=b'z' | b'A'..=b'Z' => Some((3, c.to_ascii_uppercase())),
            _ => Some(
                SYMBOLS
                    .iter()
                    .position(|&s| s == c)
                    .and_then(|i| u8::try_from(i).ok())
                    .map_or((4, c), |i| (1, i)),
            ),
        }
    }
    let key = |s: &[u8]| s.iter().filter_map(|&c| weight(c)).collect::<Vec<_>>();
    key(a).cmp(&key(b)).then_with(|| cmp_ignore_case(a, b))
}

/// An owned knowledge-base string: the text's characters as the ANSI bytes
/// they were read from (the DCU's names and strings come in as Latin-1);
/// a character beyond Latin-1 (a Unicode name) as its UTF-8 bytes.
pub fn text(s: &str) -> Text<'static> {
    let mut out = Vec::with_capacity(s.len());
    for c in s.chars() {
        match u8::try_from(u32::from(c)) {
            Ok(b) => out.push(b),
            Err(_) => {
                let mut buf = [0u8; 4];
                out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            }
        }
    }
    Text(Cow::Owned(out))
}
