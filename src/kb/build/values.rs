//! Constants, typed constants and VMTs, and the values the files print
//! for them (`$03`, `#$2B`, `(Left:$B;Top:$2)`, `Button@`).

use std::{borrow::Cow, collections::HashSet, ops::Range};

use crate::{
    dcu::{self, AddrTarget, DeclId, DeclKind, RangeKind, Unit},
    kb::{
        Const, ConstKind, Dump, Fixup, FixupKind,
        build::{
            UnitBuilder,
            names::is_resource_text,
            print::{bool_str, char_value, hex_value, mask, ndx_str},
            text,
        },
        float,
    },
};

impl UnitBuilder<'_> {
    /// Appends a record for every constant, typed constant and VMT of
    /// the unit.
    pub fn consts(&self, out: &mut Vec<Const<'static>>) {
        // Enumeration members are constants of the DCU, not of the files.
        let members: HashSet<DeclId> = self
            .unit
            .type_defs
            .iter()
            .filter_map(|d| match &d.kind {
                dcu::TypeKind::Enum { members, .. } => Some(members.iter().copied()),
                _ => None,
            })
            .flatten()
            .collect();
        // One constant per name and unit, the first (the compiler's `.`
        // default-value constants share a name).
        let first = out.len();
        // The unit's own declarations, and the default values in the
        // parameter lists of procedural types (interface methods).
        let signature_defaults = self.unit.type_defs.iter().filter_map(|d| match &d.kind {
            dcu::TypeKind::ProcType { params, .. } => Some(params.iter().copied()),
            _ => None,
        });
        let ids: Vec<DeclId> = self
            .unit
            .main
            .iter()
            .copied()
            .chain(signature_defaults.flatten())
            .collect();
        for id in ids {
            let Some(decl) = self.unit.decls.get(id) else {
                continue;
            };
            if members.contains(&id) || is_resource_text(decl) {
                continue;
            }
            if !self.unit.main.contains(&id) && !matches!(decl.kind, DeclKind::Const(_)) {
                continue;
            }
            let (kind, type_index, value) = match &decl.kind {
                DeclKind::Const(cv) => (ConstKind::Const, None, self.const_value(cv)),
                DeclKind::ResourceString(v) => {
                    (ConstKind::TypedConst, Some(v.type_index), "@".to_string())
                }
                DeclKind::TypedConst(v) => {
                    self.shown_class_refs.borrow_mut().clear();
                    // Without data in the unit's block, `?`.
                    let value = self.memory.get(&id).map_or_else(
                        || "?".to_string(),
                        |r| self.memory_value(v.type_index, r.clone(), 0),
                    );
                    (ConstKind::TypedConst, Some(v.type_index), value)
                }
                DeclKind::Vmt(v) => (ConstKind::TypedConst, Some(v.type_index), String::new()),
                _ => continue,
            };
            let name = self.decl_name_as(decl, 'V', 'C');
            let record = Const {
                module: self.module,
                name: text(&name),
                kind,
                type_def: text(&type_index.map(|t| self.type_ref(t)).unwrap_or_default()),
                value: text(&value),
                dump: match &decl.kind {
                    // A string, set or float constant's image is its value.
                    DeclKind::Const(dcu::ConstValue {
                        data: dcu::ConstData::Bytes(b),
                        ..
                    }) => Dump {
                        code: Cow::Owned(b.clone()),
                        relocs: Cow::Owned(vec![0; b.len()]),
                        fixups: Vec::new(),
                        fixup_count: 0,
                    },
                    // Showing a class reference registers its target at
                    // offset 0 ahead of the relocations; the list keeps
                    // each (offset, name) once.
                    DeclKind::TypedConst(_) => {
                        let mut dump = self.dump(id);
                        let shown: Vec<Fixup<'static>> = self
                            .shown_class_refs
                            .borrow()
                            .iter()
                            .map(|&t| Fixup {
                                kind: FixupKind::Address,
                                offset: 0,
                                name: text(&self.addr_name(t)),
                            })
                            .collect();
                        if !shown.is_empty() {
                            let mut fixups: Vec<Fixup<'static>> = Vec::new();
                            for f in shown.into_iter().chain(dump.fixups.drain(..)) {
                                if !fixups
                                    .iter()
                                    .any(|g| g.offset == f.offset && g.name == f.name)
                                {
                                    fixups.push(f);
                                }
                            }
                            dump.fixup_count = u32::try_from(fixups.len()).unwrap_or(u32::MAX);
                            dump.fixups = fixups;
                        }
                        dump
                    }
                    _ => self.dump(id),
                },
            };
            let known = out.get(first..).is_some_and(|o| {
                o.iter()
                    .any(|c| c.name.as_bytes() == record.name.as_bytes())
            });
            if !known {
                out.push(record);
            }
        }
    }

    /// A true constant's value as the knowledge bases print it.
    pub fn const_value(&self, cv: &dcu::ConstValue) -> String {
        // A long string or class reference given other than as a string
        // literal (a default parameter value) shows nothing.
        let kind = self
            .universe
            .resolve(self.unit, cv.type_index)
            .and_then(|(u, i)| u.type_def_at(i))
            .map(|d| &d.kind);
        if cv.kind != Some(dcu::ConstKind::String)
            && matches!(
                kind,
                Some(dcu::TypeKind::String { .. } | dcu::TypeKind::ClassRef { .. })
            )
        {
            return String::new();
        }
        match &cv.data {
            dcu::ConstData::Int(v) => self.ordinal_value(cv.type_index, *v),
            dcu::ConstData::Bytes(b) => match cv.kind {
                Some(dcu::ConstKind::String) => match ansi_string_text(b) {
                    Some(t) if t.is_empty() => "''".to_string(),
                    t => t.unwrap_or_default(),
                },
                // Stored as an Extended image whatever the type; a
                // Currency constant (an exact decimal literal) holds the
                // value times 10 000.
                Some(dcu::ConstKind::Float) => b
                    .get(..10)
                    .and_then(|b| b.try_into().ok())
                    .map(|b| {
                        let scale = if self.is_currency(cv.type_index) {
                            -4
                        } else {
                            0
                        };
                        float::extended(b, scale)
                    })
                    .unwrap_or_default(),
                _ => self.bytes_value(cv.type_index, b),
            },
        }
    }

    /// An ordinal of type `index` as the knowledge bases print it.
    pub fn ordinal_value(&self, index: u32, v: i64) -> String {
        let resolved = self.universe.resolve(self.unit, index);
        let def = resolved.and_then(|(u, i)| u.type_def_at(i));
        match def.map(|d| (&d.kind, d.size)) {
            Some((
                dcu::TypeKind::Range {
                    kind: RangeKind::Char | RangeKind::WideChar,
                    ..
                },
                _,
            )) => char_value(v),
            Some((
                dcu::TypeKind::Range {
                    kind: RangeKind::Boolean,
                    ..
                },
                _,
            )) => bool_str(v),
            Some((dcu::TypeKind::Enum { members, low, .. }, size)) => resolved
                .and_then(|(u, _)| {
                    let at = usize::try_from(v.checked_sub(*low)?).ok()?;
                    u.decls_of(members).nth(at).map(|d| d.name.clone())
                })
                .unwrap_or_else(|| {
                    // A value no member names: `Type($04)`.
                    let name = self.unit.type_name(index).unwrap_or_default();
                    format!(
                        "{name}({})",
                        hex_value(v, usize::try_from(size).unwrap_or(1))
                    )
                }),
            Some((dcu::TypeKind::Float(dcu::FloatKind::Currency), _)) => float::currency(v),
            Some((dcu::TypeKind::Pointer { .. } | dcu::TypeKind::Class(_), size)) => {
                if v == 0 {
                    "Nil".to_string()
                } else {
                    hex_value(mask(v, 4), usize::try_from(size).unwrap_or(4))
                }
            }
            Some((dcu::TypeKind::Range { low, .. }, size)) if *low >= 0 => {
                let size = usize::try_from(size).unwrap_or(4);
                hex_value(mask(v, size), size)
            }
            Some(_) => hex_value(v, 0),
            None => hex_value(v, 4),
        }
    }

    /// A value given as bytes (floats, sets, records) of type `index`.
    pub fn bytes_value(&self, index: u32, bytes: &[u8]) -> String {
        self.value_of(self.unit, index, bytes, &[], 0)
    }

    /// A typed constant's value from its bytes in the unit's block.
    pub fn memory_value(&self, index: u32, range: Range<usize>, depth: u8) -> String {
        let bytes = self.unit.block.get(range.clone()).unwrap_or_default();
        let relocs: Vec<(usize, u32)> = self
            .unit
            .fixups
            .iter()
            .filter(|f| !self.unit.is_marker(f))
            .filter_map(|f| {
                let at = usize::try_from(f.offset).ok()?;
                range
                    .contains(&at)
                    .then(|| (at.saturating_sub(range.start), f.target))
            })
            .collect();
        self.value_of(self.unit, index, bytes, &relocs, depth)
    }

    /// A value of type `index` of unit `types` held in `bytes`, whose
    /// relocations are `relocs` (offset in `bytes`, target address in this
    /// unit, which holds the value).
    pub fn value_of(
        &self,
        types: &Unit,
        index: u32,
        bytes: &[u8],
        relocs: &[(usize, u32)],
        depth: u8,
    ) -> String {
        if depth > 16 {
            return String::new();
        }
        let depth = depth.saturating_add(1);
        let Some((unit, index)) = self.universe.resolve(types, index) else {
            return String::new();
        };
        let Some(def) = unit.type_def_at(index) else {
            return String::new();
        };
        let size = usize::try_from(def.size).unwrap_or(0);
        let reloc_at = |at: usize| relocs.iter().find(|(o, _)| *o == at).map(|(_, t)| *t);
        let int = |b: &[u8], signed: bool| -> i64 {
            let mut buf = [0u8; 8];
            let n = b.len().min(8);
            if let (Some(dst), Some(src)) = (buf.get_mut(..n), b.get(..n)) {
                dst.copy_from_slice(src);
            }
            let raw = i64::from_le_bytes(buf);
            if signed && n > 0 && n < 8 {
                let shift = u32::try_from(64usize.saturating_sub(n.saturating_mul(8))).unwrap_or(0);
                raw.wrapping_shl(shift).wrapping_shr(shift)
            } else {
                raw
            }
        };
        let here = bytes.get(..size.min(bytes.len())).unwrap_or_default();
        match &def.kind {
            dcu::TypeKind::Range { kind, low, .. } => {
                let v = int(here, *low < 0);
                match kind {
                    RangeKind::Char | RangeKind::WideChar => char_value(v),
                    RangeKind::Boolean => bool_str(v),
                    // Unsigned types pad to their size, signed ones not.
                    _ if *low >= 0 => hex_value(v, size),
                    _ => hex_value(v, 0),
                }
            }
            dcu::TypeKind::Enum { members, low, .. } => {
                let v = int(here, false);
                usize::try_from(v.saturating_sub(*low))
                    .ok()
                    .and_then(|at| unit.decls_of(members).nth(at))
                    .map_or_else(
                        || {
                            // A value no member names: `Type($04)`.
                            let name = unit.type_name(index).unwrap_or_default();
                            format!("{name}({})", hex_value(v, size))
                        },
                        |d| d.name.clone(),
                    )
            }
            dcu::TypeKind::ShortString { .. } => {
                let len = usize::from(here.first().copied().unwrap_or(0));
                let text: String = here
                    .get(1..len.saturating_add(1).min(here.len()))
                    .unwrap_or_default()
                    .iter()
                    .map(|&c| {
                        if c < 32 {
                            format!("#{c}")
                        } else {
                            char::from(c).to_string()
                        }
                    })
                    .collect();
                if text.is_empty() {
                    "''".to_string()
                } else {
                    text
                }
            }
            dcu::TypeKind::Float(k) => float_value(*k, here),
            dcu::TypeKind::Set {
                element,
                first_byte,
            } => self.set_value(unit, *element, *first_byte, here),
            dcu::TypeKind::Pointer { target }
                if reloc_at(0).is_some() && self.is_char(unit, *target) =>
            {
                let addend = usize::try_from(int(here, false)).unwrap_or(0);
                format!(
                    "{}@",
                    reloc_at(0)
                        .map(|t| self.pchar_at(t, addend))
                        .unwrap_or_default()
                )
            }
            dcu::TypeKind::Pointer { .. } | dcu::TypeKind::Class(_) => {
                if reloc_at(0).is_some() {
                    "@".to_string()
                } else if here.iter().all(|&b| b == 0) {
                    "Nil".to_string()
                } else {
                    hex_value(int(here, false), size)
                }
            }
            dcu::TypeKind::ClassRef { .. } => {
                if let Some(t) = reloc_at(0) {
                    self.shown_class_refs.borrow_mut().push(t);
                }
                String::new()
            }
            dcu::TypeKind::ProcType { flags, .. } if flags & 0x10 == 0 => {
                if reloc_at(0).is_some() {
                    "@".to_string()
                } else {
                    "Nil".to_string()
                }
            }
            dcu::TypeKind::String { .. } => match reloc_at(0) {
                // The bytes hold the offset into the target's block.
                Some(target) => format!(
                    "{}@",
                    self.string_at(target, usize::try_from(int(here, false)).unwrap_or(0))
                ),
                None => "''".to_string(),
            },
            dcu::TypeKind::Array { element_type, .. } if self.is_char(unit, *element_type) => here
                .iter()
                .map(|&c| {
                    if c < 32 {
                        format!("#{c}")
                    } else {
                        char::from(c).to_string()
                    }
                })
                .collect(),
            dcu::TypeKind::Array {
                index_type,
                element_type,
                ..
            } => {
                let count = self.ordinal_count(unit, *index_type);
                let elem = self.size_in(unit, *element_type);
                let items: Vec<String> = (0..count)
                    .map(|i| {
                        let start = i.saturating_mul(elem);
                        let end = start.saturating_add(elem);
                        let sub: Vec<(usize, u32)> = relocs
                            .iter()
                            .filter(|(o, _)| (start..end).contains(o))
                            .map(|(o, t)| (o.saturating_sub(start), *t))
                            .collect();
                        let b = bytes.get(start..end.min(bytes.len())).unwrap_or_default();
                        self.value_of(unit, *element_type, b, &sub, depth)
                    })
                    .collect();
                format!("({})", items.join(","))
            }
            dcu::TypeKind::Record { members, .. } => {
                let items: Vec<String> = unit
                    .decls_of(members)
                    .filter_map(|d| match &d.kind {
                        DeclKind::Local(l) if d.tag == 0x2C => {
                            let t = u32::from_ne_bytes(l.type_index.to_ne_bytes());
                            let start = usize::try_from(l.location).ok()?;
                            let len = self.size_in(unit, t);
                            let end = start.saturating_add(len);
                            let sub: Vec<(usize, u32)> = relocs
                                .iter()
                                .filter(|(o, _)| (start..end).contains(o))
                                .map(|(o, t)| (o.saturating_sub(start), *t))
                                .collect();
                            let b = bytes.get(start..end.min(bytes.len())).unwrap_or_default();
                            Some(format!(
                                "{}:{}",
                                d.name,
                                self.value_of(unit, t, b, &sub, depth)
                            ))
                        }
                        _ => None,
                    })
                    .collect();
                format!("({})", items.join(";"))
            }
            _ => String::new(),
        }
    }

    /// Whether type `index` is `Currency`.
    pub fn is_currency(&self, index: u32) -> bool {
        matches!(
            self.universe
                .resolve(self.unit, index)
                .and_then(|(u, i)| u.type_def_at(i))
                .map(|d| &d.kind),
            Some(dcu::TypeKind::Float(dcu::FloatKind::Currency))
        )
    }

    /// Whether type `index` of `unit` is an AnsiChar range.
    pub fn is_char(&self, unit: &Unit, index: u32) -> bool {
        let (unit, index) = self.universe.resolve(unit, index).unwrap_or((unit, index));
        matches!(
            unit.type_def_at(index).map(|d| &d.kind),
            Some(dcu::TypeKind::Range {
                kind: RangeKind::Char,
                ..
            })
        )
    }

    /// The NUL-terminated text `addend` bytes into the block of the
    /// declaration at address `target`.
    pub fn pchar_at(&self, target: u32, addend: usize) -> String {
        let Some(AddrTarget::Decl(id)) = self.unit.addr(target) else {
            return String::new();
        };
        let Some(r) = self.memory.get(&id) else {
            return String::new();
        };
        // The files show the text only when its NUL comes before the
        // block's last byte.
        let bytes = self
            .unit
            .block
            .get(r.start.saturating_add(addend)..r.end.saturating_sub(1))
            .unwrap_or_default();
        let Some(len) = bytes.iter().position(|&c| c == 0) else {
            return String::new();
        };
        bytes
            .get(..len)
            .unwrap_or_default()
            .iter()
            .map(|&c| {
                if c < 32 {
                    format!("#{c}")
                } else {
                    char::from(c).to_string()
                }
            })
            .collect()
    }

    /// The number of values of ordinal type `index` of `unit`.
    pub fn ordinal_count(&self, unit: &Unit, index: u32) -> usize {
        let (unit, index) = self.universe.resolve(unit, index).unwrap_or((unit, index));
        match unit.type_def_at(index).map(|d| &d.kind) {
            Some(
                dcu::TypeKind::Range { low, high, .. } | dcu::TypeKind::Enum { low, high, .. },
            ) => usize::try_from(high.saturating_sub(*low).saturating_add(1)).unwrap_or(0),
            _ => 0,
        }
    }

    /// The size of type `index` of `unit`, following imports.
    pub fn size_in(&self, unit: &Unit, index: u32) -> usize {
        self.universe
            .resolve(unit, index)
            .and_then(|(u, i)| u.type_def_at(i))
            .and_then(|d| usize::try_from(d.size).ok())
            .unwrap_or(0)
    }

    /// A set value: `[a,b..c]`, ranges of consecutive members.
    pub fn set_value(&self, unit: &Unit, element: u32, first_byte: u8, bytes: &[u8]) -> String {
        let base = usize::from(first_byte).saturating_mul(8);
        let members: Vec<usize> = bytes
            .iter()
            .enumerate()
            .flat_map(|(i, b)| {
                (0..8)
                    .filter(move |bit| b & (1 << bit) != 0)
                    .map(move |bit| i.saturating_mul(8).saturating_add(bit))
            })
            .map(|v| v.saturating_add(base))
            .collect();
        let (eu, ei) = self
            .universe
            .resolve(unit, element)
            .unwrap_or((unit, element));
        let element_size = eu
            .type_def_at(ei)
            .and_then(|d| usize::try_from(d.size).ok())
            .unwrap_or(1);
        let name = |v: usize| -> String {
            let v = i64::try_from(v).unwrap_or(0);
            match eu.type_def_at(ei).map(|d| &d.kind) {
                Some(dcu::TypeKind::Enum { members, low, .. }) => {
                    usize::try_from(v.saturating_sub(*low))
                        .ok()
                        .and_then(|at| eu.decls_of(members).nth(at))
                        .map_or_else(|| ndx_str(v), |d| d.name.clone())
                }
                Some(dcu::TypeKind::Range {
                    kind: RangeKind::Char,
                    ..
                }) => char_value(v),
                Some(_) => hex_value(v, element_size),
                None => ndx_str(v),
            }
        };
        let mut parts = Vec::new();
        let mut i = 0;
        while let Some(&start) = members.get(i) {
            let mut end = start;
            while members.get(i.saturating_add(1)) == Some(&end.saturating_add(1)) {
                end = end.saturating_add(1);
                i = i.saturating_add(1);
            }
            parts.push(if end == start {
                name(start)
            } else {
                format!("{}..{}", name(start), name(end))
            });
            i = i.saturating_add(1);
        }
        format!("[{}]", parts.join(","))
    }

    /// The text of the string literal `addend` bytes into the block of
    /// the declaration at address `target` (`addend` points at the
    /// characters, past the reference count and length).
    pub fn string_at(&self, target: u32, addend: usize) -> String {
        match self.unit.addr(target) {
            Some(AddrTarget::Decl(id)) => self
                .memory
                .get(&id)
                // As for PChars, the NUL must come before the block's last
                // byte.
                .and_then(|r| {
                    self.unit.block.get(
                        r.start.saturating_add(addend).saturating_sub(8)..r.end.saturating_sub(1),
                    )
                })
                .and_then(ansi_string_text)
                .unwrap_or_default(),
            _ => String::new(),
        }
    }
}

/// A floating-point constant of kind `k` from its bytes.
pub fn float_value(k: dcu::FloatKind, b: &[u8]) -> String {
    let v = match k {
        dcu::FloatKind::Single => b
            .get(..4)
            .and_then(|b| b.try_into().ok())
            .map(|b| f64::from(f32::from_le_bytes(b))),
        dcu::FloatKind::Double => b
            .get(..8)
            .and_then(|b| b.try_into().ok())
            .map(f64::from_le_bytes),
        dcu::FloatKind::Extended => {
            return b
                .get(..10)
                .and_then(|b| b.try_into().ok())
                .map(|b| float::extended(b, 0))
                .unwrap_or_default();
        }
        dcu::FloatKind::Currency => {
            return b
                .get(..8)
                .and_then(|b| b.try_into().ok())
                .map(|b| float::currency(i64::from_le_bytes(b)))
                .unwrap_or_default();
        }
        _ => None,
    };
    v.map(float::double).unwrap_or_default()
}

/// The text of an AnsiString literal image (`-1`, length, characters,
/// NUL): control characters as `#n`.
pub fn ansi_string_text(b: &[u8]) -> Option<String> {
    let len = usize::try_from(u32::from_le_bytes(b.get(4..8)?.try_into().ok()?)).ok()?;
    let chars = b.get(8..8usize.checked_add(len)?)?;
    Some(
        chars
            .iter()
            .map(|&c| {
                if c < 32 {
                    format!("#{c}")
                } else {
                    char::from(c).to_string()
                }
            })
            .collect(),
    )
}
