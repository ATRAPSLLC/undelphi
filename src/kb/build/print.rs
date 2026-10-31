//! Type expressions as DCU32INT prints them (`array[$0..-$1] of
//! System.TVarRec`, `procedure(val Sender:System.TObject) of object`) and
//! the ordinal formats they share with values.

use crate::{
    dcu::{self, CallConv, DeclId, ImportKind, RangeKind, TypeKind as DcuType, TypeTarget},
    kb::build::{
        UnitBuilder,
        names::{dotted, is_aux_name},
    },
};

impl UnitBuilder<'_> {
    /// A type reference as DCU32INT shows it: a named type by name
    /// (qualified with its unit when imported), an anonymous one by its
    /// definition.
    pub fn type_ref(&self, index: u32) -> String {
        let Some(slot) = self.unit.type_slot(index) else {
            return String::new();
        };
        match slot.target {
            // A local copy of an imported type (`TCaption = type string`)
            // shows under the local name.
            TypeTarget::Import { unit, symbol }
                if self
                    .unit
                    .uses
                    .get(unit)
                    .and_then(|u| u.symbols.get(symbol))
                    .is_some_and(|s| matches!(s.kind, ImportKind::TypeCopy { .. }))
                    && slot.name.as_deref().is_some_and(|n| !is_aux_name(n)) =>
            {
                slot.name.clone().unwrap_or_default()
            }
            TypeTarget::Import { unit, symbol } => self.import_ref(unit, symbol),
            TypeTarget::Def(id) => match &slot.name {
                Some(name) if !is_aux_name(name) => name.clone(),
                // A type a declaration names `.92` is referred to by name.
                Some(name) if name.starts_with('.') => dotted(name, 'T'),
                _ => self
                    .unit
                    .type_defs
                    .get(id)
                    .map(|d| self.type_def(d))
                    .unwrap_or_default(),
            },
            TypeTarget::None => String::new(),
        }
    }

    /// A type definition as DCU32INT shows it.
    pub fn type_def(&self, def: &dcu::TypeDef) -> String {
        match &def.kind {
            DcuType::Range {
                kind, low, high, ..
            } => {
                let size = usize::try_from(def.size).unwrap_or(4);
                match kind {
                    RangeKind::Boolean => format!(
                        "{}..{}",
                        bool_bytes_str(*low, size),
                        bool_bytes_str(*high, size)
                    ),
                    RangeKind::Char => format!("{}..{}", char_value(*low), char_value(*high)),
                    RangeKind::WideChar => format!("{}..{}", wchar_str(*low), wchar_str(*high)),
                    // An unsigned range beyond 31 bits shows both bounds
                    // at its full width.
                    _ if *low >= 0 && *high > i64::from(i32::MAX) => {
                        format!("{}..{}", hex_value(*low, size), hex_value(*high, size))
                    }
                    _ => format!("{}..{}", ndx_str(*low), ndx_str(*high)),
                }
            }
            DcuType::Enum {
                low, high, members, ..
            } => {
                if members.is_empty() {
                    // A subrange of an enumeration, in numbers.
                    format!("{}..{}", ndx_str(*low), ndx_str(*high))
                } else {
                    let names: Vec<&str> = self
                        .unit
                        .decls_of(members)
                        .map(|d| d.name.as_str())
                        .collect();
                    format!("({})", names.join(","))
                }
            }
            DcuType::Float(k) => format!("float(fk{k:?})"),
            DcuType::ShortString { .. } if def.size == -1 => "ShortString".to_string(),
            DcuType::ShortString { .. } => format!("String[{}]", def.size.saturating_sub(1)),
            DcuType::String { .. } => "String".to_string(),
            DcuType::Variant { .. } => "variant".to_string(),
            DcuType::Pointer { target } => format!("^{}", self.type_ref(*target)),
            DcuType::Text => "text".to_string(),
            DcuType::File { element } if *element == 0 => "file".to_string(),
            DcuType::File { element } => format!("file of {}", self.type_ref(*element)),
            DcuType::Set { element, .. } => format!("set of {}", self.type_ref(*element)),
            DcuType::Array {
                index_type,
                element_type,
                ..
            } => format!(
                "array[{}] of {}",
                self.type_ref(*index_type),
                self.type_ref(*element_type)
            ),
            DcuType::ClassRef { class_type, .. } => {
                format!("class of {}", self.type_ref(*class_type))
            }
            DcuType::Void => "void ".to_string(),
            DcuType::ProcType {
                flags,
                result,
                call,
                params,
            } => self.proc_type(*flags, *result, *call, params),
            DcuType::Class(c) => {
                let mut bases: Vec<String> = Vec::new();
                if c.parent != 0 {
                    bases.push(self.type_ref(c.parent));
                }
                bases.extend(c.interfaces.iter().map(|&(i, _)| self.type_ref(i)));
                if bases.is_empty() {
                    "class".to_string()
                } else {
                    format!("class({})", bases.join(","))
                }
            }
            DcuType::Interface {
                parent, guid, b, ..
            } => {
                let mut out = String::from(if b & 4 != 0 {
                    "dispinterface"
                } else {
                    "interface"
                });
                if *parent != 0 {
                    out.push_str(&format!(" ({})", self.type_ref(*parent)));
                } else {
                    out.push(' ');
                }
                if guid.iter().any(|&b| b != 0) {
                    out.push_str(&format!("['{}']", guid_str(guid)));
                }
                if out.ends_with(' ') {
                    out.pop();
                }
                out
            }
            _ => String::new(),
        }
    }

    /// A procedural type: `procedure(val A:T;var B:U):R of object stdcall`.
    pub fn proc_type(&self, flags: u32, result: u32, call: CallConv, params: &[DeclId]) -> String {
        let result = self.result_type(result);
        let mut out = String::from(if result.is_some() {
            "function"
        } else {
            "procedure"
        });
        out.push_str(&self.proc_type_tail(flags, result, call, params));
        out
    }

    /// What follows `procedure` / `function` (and an interface method's
    /// name) in a procedural type: `(args):R of object stdcall`.
    pub fn proc_type_tail(
        &self,
        flags: u32,
        result: Option<u32>,
        call: CallConv,
        params: &[DeclId],
    ) -> String {
        let mut out = String::new();
        let args = self.arg_list(params);
        if !args.is_empty() {
            out.push('(');
            out.push_str(&args.join(";"));
            out.push(')');
        }
        if let Some(result) = result {
            out.push(':');
            out.push_str(&self.type_ref(result));
        }
        if flags & 0x10 != 0 {
            out.push_str(" of object");
        }
        match call {
            CallConv::Register => {}
            CallConv::Cdecl => out.push_str(" cdecl"),
            CallConv::Pascal => out.push_str(" pascal"),
            CallConv::Stdcall => out.push_str(" stdcall"),
            CallConv::Safecall => out.push_str(" safecall"),
        }
        out
    }
}

/// An ordinal as the knowledge bases print a constant: hexadecimal padded
/// to the type's size, a negative value as `-$` and its magnitude.
pub fn hex_value(v: i64, size: usize) -> String {
    if v < 0 {
        format!("-${:X}", v.unsigned_abs())
    } else {
        format!("${:0width$X}", v, width = size.saturating_mul(2).max(1))
    }
}

/// `v` as an unsigned value of `size` bytes.
pub fn mask(v: i64, size: usize) -> i64 {
    match size {
        1 => v & 0xFF,
        2 => v & 0xFFFF,
        4 => v & 0xFFFF_FFFF,
        _ => v,
    }
}

/// A Boolean range bound as DCU32INT's `BoolStr` shows the value's
/// `size` bytes: `false` for 0, `true` for 1, `true{$FF}` otherwise.
pub fn bool_bytes_str(v: i64, size: usize) -> String {
    match mask(v, size) {
        0 => "false".to_string(),
        1 => "true".to_string(),
        other => format!("true{{{}}}", hex_value(other, size)),
    }
}

/// A WideChar bound as DCU32INT's `WCharStr` shows it: `#n` below 32,
/// else the character (`?` beyond Latin-1) and its code.
pub fn wchar_str(v: i64) -> String {
    if (0..32).contains(&v) {
        format!("#{v}")
    } else {
        let c = u8::try_from(v).map_or('?', char::from);
        format!("'{c}'{{#${v:X}}}")
    }
}

/// A character constant: `#8` below 32, `#$2B` from there.
pub fn char_value(v: i64) -> String {
    if (0..32).contains(&v) {
        format!("#{v}")
    } else {
        format!("#${v:X}")
    }
}

/// Formats a GUID stored in `TGUID` layout as `{XXXXXXXX-XXXX-...}`.
pub fn guid_str(g: &[u8; 16]) -> String {
    let [a0, a1, a2, a3, b0, b1, c0, c1, d @ ..] = *g;
    let d: Vec<String> = d.iter().map(|b| format!("{b:02X}")).collect();
    format!(
        "{{{:08X}-{:04X}-{:04X}-{}-{}}}",
        u32::from_le_bytes([a0, a1, a2, a3]),
        u16::from_le_bytes([b0, b1]),
        u16::from_le_bytes([c0, c1]),
        d.get(..2).unwrap_or_default().concat(),
        d.get(2..).unwrap_or_default().concat()
    )
}

/// An ordinal as DCU32INT's `NDXToStr` prints it: signed hexadecimal. A
/// value beyond 32 bits is printed as its high and low words, and a
/// negative one negated word by word without the carry between them (so
/// `-$8000000000000000` reads `-$7FFFFFFF00000000`).
pub fn ndx_str(v: i64) -> String {
    if let Ok(v) = i32::try_from(v) {
        return if v < 0 {
            format!("-${:X}", v.unsigned_abs())
        } else {
            format!("${v:X}")
        };
    }
    let [l0, l1, l2, l3, h0, h1, h2, h3] = v.to_le_bytes();
    let lo = u32::from_le_bytes([l0, l1, l2, l3]);
    let hi = i32::from_le_bytes([h0, h1, h2, h3]);
    if hi < 0 {
        format!("-${:X}{:08X}", !hi, lo.wrapping_neg())
    } else if hi == 0 {
        format!("${lo:X}")
    } else {
        format!("${hi:X}{lo:08X}")
    }
}

/// A Boolean range bound.
pub fn bool_str(v: i64) -> String {
    match v {
        0 => "false".to_string(),
        1 => "true".to_string(),
        other => ndx_str(other),
    }
}
