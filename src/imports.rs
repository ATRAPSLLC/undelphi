//! Runtime-package imports: how a binary linked against Borland runtime
//! packages (`rtl60.bpl`, `vcl60.bpl`, a third-party `.bpl`, ...) refers to
//! RTTI, classes and routines that live in another module.
//!
//! Such a binary does not contain the referenced type or class. Every
//! cross-module reference - a property's `PPTypeInfo`, a class's
//! `vmtParent`, a published field's class cell - points at an import
//! address table slot instead, and the slot's import name says what it is.
//! Borland exports package symbols under C++Builder-style mangled names:
//!
//! ```text
//!   @$xp$14System@Integer          typeinfo of System.Integer
//!                                  (`$xp$`, then the length of the
//!                                  qualified name that follows)
//!   @Classes@TPersistent@          VMT of Classes.TPersistent
//!                                  (qualified name, trailing `@`)
//!   @System@TObject@Free$qqrv      routine System.TObject.Free
//!                                  (`$` starts the argument encoding)
//! ```
//!
//! Namespaced units (Delphi XE2+) add segments: `@Vcl@Controls@TControl@`.
//! The last segment is the symbol, the others form the unit name.
//!
//! Generic instances (Delphi 2009+) are one segment,
//! `%Name__N$<N type arguments>%`. Each argument is a Borland C++ type code:
//! a length-prefixed qualified name (`20System@UnicodeString`, which may
//! itself hold a nested generic - the length bounds it exactly), `p` / `r` /
//! `x` (pointer, reference, const) before another code - a Delphi class
//! type is a pointer in this view (`p25System@Classes@TComponent`) - or a
//! built-in (`i` Integer, `j` Int64, `o` Boolean, `ui` Cardinal, `d`
//! Double, ...). They render the way RTTI names the instance:
//! `@$xp$..System@Generics@Collections@%TList__1$p17System@TMetaClass%`
//! is `TList<System.TMetaClass>` in unit `System.Generics.Collections`
//! (empirically, the packages of RAD Studio 10 Seattle).
//!
//! Delphi's **Win64** compiler uses the Itanium C++ ABI scheme instead:
//!
//! ```text
//!   _ZTVN6System7Classes11TPersistentE   VMT of System.Classes.TPersistent
//!   _ZTRN6System7Classes10TAlignmentE    typeinfo (TR: Delphi's own marker)
//!   _ZN6System7Classes11TPersistent6AssignEPS1_   routine (+ parameters)
//! ```
//!
//! Names are `N <len><id>... E` with back-references (`S_` = the first
//! prefix seen, `S<n>_` = candidate n + 1); a generic is `Name__N` followed
//! by `I <type args> E` (`TSensor__1IPNS0_18TCustomLightSensorEE` is
//! `TSensor<System.Sensors.TCustomLightSensor>`), an interface argument is
//! wrapped in `System::DelphiInterface<I>`, and `P` marks the pointer a
//! class reference is in the C++ view. Both schemes render alike
//! (Seattle's 120 Win64 packages).
//!
//! The executable's own `PACKAGEINFO` carries no `requires` list (the
//! count is zero, verified on Delphi 6 output); the imported `.bpl`
//! modules are the only record of which packages it needs.
//!
//! Source: empirically decoded from `tests/samples/bcb6/known_rtl.exe`,
//! `usepkg.exe` and `gtpkg.bpl` (Delphi 6, built from
//! `tests/fixtures/bcb6/`).

use std::str;

use goblin::pe::PE;

use crate::{
    formats::BinaryContext,
    limits::{MAX_DELAY_DESCRIPTORS, MAX_DELAY_IMPORTS},
    util::{read_ptr, read_u32},
};

/// What a Borland-mangled import name refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolKind {
    /// A `PTypeInfo` record (`@$xp$...`).
    TypeInfo,
    /// A class VMT (`@Unit@Class@`).
    ClassVmt,
    /// A routine or variable (`@Unit@Name`, optionally `$`-suffixed).
    Routine,
}

impl SymbolKind {
    /// Stable label for persistence and display.
    pub const fn as_str(self) -> &'static str {
        match self {
            SymbolKind::TypeInfo => "typeinfo",
            SymbolKind::ClassVmt => "class",
            SymbolKind::Routine => "routine",
        }
    }
}

/// One imported symbol: the module it comes from, its raw import name and
/// the address-table slot the binary reaches it through.
#[derive(Debug, Clone)]
pub struct Import {
    /// VA of the import address table slot.
    pub iat_va: u64,
    module: String,
    name: String,
    delayed: bool,
}

impl Import {
    /// Whether the import is delay-loaded (Delphi's `external ... delayed`
    /// and the RTL's own optional APIs): bound on first call through the
    /// PE delay-load directory rather than by the loader.
    #[inline]
    pub fn is_delayed(&self) -> bool {
        self.delayed
    }

    /// Name of the module that exports the symbol, e.g. `rtl60.bpl`.
    #[inline]
    pub fn module(&self) -> &str {
        &self.module
    }

    /// Raw import name as stored in the import table, e.g.
    /// `@$xp$14System@Integer`.
    #[inline]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns `true` when the exporting module is a Borland runtime
    /// package (`.bpl`).
    pub fn is_package(&self) -> bool {
        is_package_module(&self.module)
    }

    /// Demangles a Borland package symbol name. `None` for an ordinary
    /// (unmangled) import such as `kernel32.dll!GetProcAddress`.
    pub fn symbol(&self) -> Option<BorlandSymbol> {
        BorlandSymbol::demangle(&self.name)
    }
}

/// A demangled Borland package symbol.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BorlandSymbol {
    /// What the symbol refers to.
    pub kind: SymbolKind,
    /// Declaring unit, `.`-joined for namespaced units (`System`,
    /// `Vcl.Controls`). Empty for a typeinfo symbol in the C++ global
    /// namespace, whose name does not record its unit.
    pub unit: String,
    /// Symbol name within the unit; `.`-joined for a member
    /// (`TObject.Free`).
    pub name: String,
}

impl BorlandSymbol {
    /// Demangles a Borland package symbol name: `@$xp$<len><qualified>`
    /// typeinfo, `@<qualified>@` VMT, or `@<qualified>[$args]` routine.
    ///
    /// Generic instances are rendered as RTTI names them
    /// (`TDictionary<System.UnicodeString,Vcl.Themes.TStyleHook>`), and so
    /// are anonymous types (`:N`); see the module docs for the encodings.
    /// The unit is everything before the first generic segment or, without
    /// one, before the last segment - so a non-generic nested type keeps
    /// its outer type in the unit.
    pub fn demangle(mangled: &str) -> Option<Self> {
        if mangled.starts_with("_Z") {
            return demangle_itanium(mangled);
        }
        if let Some(rest) = mangled.strip_prefix("@$xp$") {
            let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
            let len: usize = rest.get(..digits)?.parse().ok()?;
            let qualified = rest.get(digits..)?;
            if qualified.len() != len {
                return None;
            }
            let (segments, used) = parse_qualified(qualified)?;
            if used != qualified.len() {
                return None;
            }
            return Self::split(SymbolKind::TypeInfo, render_anonymous(segments), false);
        }
        let body = mangled.strip_prefix('@')?;
        if body.starts_with('$') {
            return None;
        }
        let (segments, used) = parse_qualified(body)?;
        let rest = body.get(used..)?;
        if rest == "@" {
            return Self::split(SymbolKind::ClassVmt, render_anonymous(segments), false);
        }
        if !rest.is_empty() && !rest.starts_with('$') {
            return None;
        }
        // A routine's unit is its first segment; a member routine keeps
        // its class: `System@TObject@Free` is `TObject.Free` in `System`.
        Self::split(SymbolKind::Routine, segments, true)
    }

    /// Splits rendered segments into unit and name. `routine` puts every
    /// segment after the first into the name.
    fn split(kind: SymbolKind, segments: Vec<String>, routine: bool) -> Option<Self> {
        if segments.iter().any(String::is_empty) {
            return None;
        }
        // A type in the C++ global namespace (`$EXTERNALSYM`, and the
        // intrinsics: `@$xp$7Boolean` in Delphi 13's rtl370) has one
        // segment: the symbol does not record its unit.
        if let (SymbolKind::TypeInfo, [name]) = (kind, segments.as_slice()) {
            return Some(Self {
                kind,
                unit: String::new(),
                name: name.clone(),
            });
        }
        if segments.len() < 2 {
            return None;
        }
        let unit_len = if routine {
            1
        } else {
            segments
                .iter()
                .position(|s| s.contains('<'))
                .unwrap_or(segments.len().saturating_sub(1))
                .max(1)
        };
        let (unit, name) = segments.split_at(unit_len);
        Some(Self {
            kind,
            unit: unit.join("."),
            name: name.join("."),
        })
    }
}

/// Demangles a Delphi Win64 symbol in the Itanium C++ ABI scheme: `_ZTV`
/// and a name (class VMT), `_ZTR` and a name (typeinfo; Delphi's own
/// marker), or `_ZN...E` and parameters (routine or variable).
fn demangle_itanium(mangled: &str) -> Option<BorlandSymbol> {
    let (kind, body) = if let Some(b) = mangled.strip_prefix("_ZTV") {
        (SymbolKind::ClassVmt, b)
    } else if let Some(b) = mangled.strip_prefix("_ZTR") {
        (SymbolKind::TypeInfo, b)
    } else {
        (SymbolKind::Routine, mangled.strip_prefix("_Z")?)
    };
    let mut parser = Itanium {
        s: body.as_bytes(),
        pos: 0,
        subs: Vec::new(),
        depth: 0,
    };
    let segments = parser.name(false)?;
    if kind != SymbolKind::Routine && parser.pos != body.len() {
        return None;
    }
    BorlandSymbol::split(
        kind,
        render_anonymous(segments),
        kind == SymbolKind::Routine,
    )
}

/// Renders anonymous-type components the way RTTI names them (`:N`):
///
/// - `Unit_Path__N` (unit level) becomes `:N`;
/// - `_Outer__N` (nested, Win64) becomes `:Outer`, `:N`;
/// - `_Outer` followed by `_N` (nested, 32-bit) becomes `:Outer`, `:N`;
/// - `_N` alone (inside a generic) becomes `:N`.
///
/// Other leading underscores are real identifiers (`_Recordset` in a COM
/// type library) and generic instances are left alone.
fn render_anonymous(segments: Vec<String>) -> Vec<String> {
    let index = |s: &str| -> Option<String> {
        let n = s.strip_prefix('_')?;
        (!n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())).then(|| n.to_string())
    };
    let suffixed = |s: &str| -> Option<(String, String)> {
        let (prefix, n) = s.rsplit_once("__")?;
        (!prefix.is_empty() && !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
            .then(|| (prefix.to_string(), n.to_string()))
    };
    let mut out = Vec::with_capacity(segments.len());
    for (i, segment) in segments.iter().enumerate() {
        if segment.contains('<') {
            out.push(segment.clone());
        } else if let Some(n) = index(segment) {
            out.push(format!(":{n}"));
        } else if let Some((prefix, n)) = suffixed(segment) {
            match prefix.strip_prefix('_') {
                Some(outer) => {
                    out.push(format!(":{outer}"));
                    out.push(format!(":{n}"));
                }
                None => out.push(format!(":{n}")),
            }
        } else if let (Some(outer), Some(next)) =
            (segment.strip_prefix('_'), segments.get(i.saturating_add(1)))
            && index(next).is_some()
        {
            out.push(format!(":{outer}"));
        } else {
            out.push(segment.clone());
        }
    }
    out
}

/// A parser for the subset of the Itanium C++ ABI mangling Delphi's Win64
/// compiler emits.
struct Itanium<'s> {
    s: &'s [u8],
    pos: usize,
    /// Substitution candidates, `S_` first: every name prefix, template
    /// instance and pointer type, in the order the ABI adds them.
    subs: Vec<Vec<String>>,
    depth: usize,
}

impl Itanium<'_> {
    fn peek(&self) -> Option<u8> {
        self.s.get(self.pos).copied()
    }

    fn eat(&mut self, b: u8) -> bool {
        let hit = self.peek() == Some(b);
        if hit {
            self.pos = self.pos.saturating_add(1);
        }
        hit
    }

    /// `<name>`: `N <component>+ E`, a substitution, or a source name.
    ///
    /// Substitution candidates follow the ABI: every prefix built from a
    /// source name and every template instance is added; a component that
    /// is itself a substitution is not added again. An unscoped source name
    /// is a candidate when it names a type (`as_type`).
    fn name(&mut self, as_type: bool) -> Option<Vec<String>> {
        if self.eat(b'N') {
            let mut parts: Vec<String> = Vec::new();
            while !self.eat(b'E') {
                if self.peek() == Some(b'S') {
                    parts = self.substitution()?;
                } else if self.eat(b'I') {
                    let args = self.template_args()?;
                    let last = parts.last_mut()?;
                    *last = render_generic(last, &args);
                    self.subs.push(parts.clone());
                } else {
                    parts.push(self.source_name()?);
                    self.subs.push(parts.clone());
                }
            }
            return (!parts.is_empty()).then_some(parts);
        }
        if self.peek() == Some(b'S') {
            return self.substitution();
        }
        let parts = vec![self.source_name()?];
        if as_type {
            self.subs.push(parts.clone());
        }
        Some(parts)
    }

    /// `<len><identifier>`.
    fn source_name(&mut self) -> Option<String> {
        let rest = self.s.get(self.pos..)?;
        let digits = rest.iter().take_while(|b| b.is_ascii_digit()).count();
        let len: usize = str::from_utf8(rest.get(..digits)?).ok()?.parse().ok()?;
        let start = self.pos.checked_add(digits)?;
        let end = start.checked_add(len)?;
        let id = str::from_utf8(self.s.get(start..end)?).ok()?;
        self.pos = end;
        (!id.is_empty()).then(|| id.to_string())
    }

    /// `S_` (first candidate) or `S<base-36 seq>_` (candidate seq + 1).
    fn substitution(&mut self) -> Option<Vec<String>> {
        if !self.eat(b'S') {
            return None;
        }
        let mut index = 0usize;
        let mut seen_digit = false;
        while let Some(b) = self.peek() {
            self.pos = self.pos.saturating_add(1);
            match b {
                b'_' => {
                    let slot = if seen_digit { index.checked_add(1)? } else { 0 };
                    return self.subs.get(slot).cloned();
                }
                b'0'..=b'9' | b'A'..=b'Z' => {
                    let digit = char::from(b).to_digit(36)?;
                    index = index
                        .checked_mul(36)?
                        .checked_add(usize::try_from(digit).ok()?)?;
                }
                _ => return None,
            }
            seen_digit = true;
        }
        None
    }

    /// `I <type>+ E`, rendered.
    fn template_args(&mut self) -> Option<Vec<String>> {
        let mut args = Vec::new();
        while !self.eat(b'E') {
            args.push(self.type_name()?);
        }
        Some(args)
    }

    /// One `<type>`, rendered the way Delphi RTTI names it.
    fn type_name(&mut self) -> Option<String> {
        self.depth = self.depth.checked_add(1)?;
        if self.depth > 64 {
            return None;
        }
        let code = self.peek()?;
        let rendered = match code {
            b'P' | b'R' | b'K' => {
                // A class type is a pointer in the C++ view; RTTI drops it.
                self.pos = self.pos.saturating_add(1);
                let inner = self.type_name()?;
                if code == b'P' {
                    self.subs.push(vec![inner.clone()]);
                }
                inner
            }
            b'N' | b'S' | b'0'..=b'9' => {
                let parts = self.name(true)?;
                let parts = if self.eat(b'I') {
                    let args = self.template_args()?;
                    let mut parts = parts;
                    let last = parts.last_mut()?;
                    *last = render_generic(last, &args);
                    self.subs.push(parts.clone());
                    parts
                } else {
                    parts
                };
                let joined = parts.join(".");
                // `DelphiInterface<I>` wraps an interface reference.
                joined
                    .strip_prefix("System.DelphiInterface<")
                    .and_then(|r| r.strip_suffix('>'))
                    .map_or(joined.clone(), str::to_string)
            }
            _ => {
                self.pos = self.pos.saturating_add(1);
                builtin(code)?.to_string()
            }
        };
        self.depth = self.depth.saturating_sub(1);
        Some(rendered)
    }
}

/// Renders a Win64 generic component `Name__N` with its arguments as
/// `Name<A,B>`.
fn render_generic(component: &str, args: &[String]) -> String {
    let base = component.rsplit_once("__").map_or(component, |(b, _)| b);
    format!("{base}<{}>", args.join(","))
}

/// Maps an Itanium built-in type code to its Delphi name.
fn builtin(code: u8) -> Option<&'static str> {
    Some(match code {
        b'v' => "void",
        b'b' => "Boolean",
        b'c' => "AnsiChar",
        b'a' => "ShortInt",
        b'h' => "Byte",
        b's' => "SmallInt",
        b't' => "Word",
        b'i' | b'l' => "Integer",
        b'j' | b'm' => "Cardinal",
        b'x' => "Int64",
        b'y' => "UInt64",
        b'f' => "Single",
        b'd' => "Double",
        b'e' => "Extended",
        b'w' => "WideChar",
        _ => return None,
    })
}

/// Parses an `@`-separated qualified name from the start of `s`, stopping
/// at its end, at a top-level `$` (a routine's argument list) or at a
/// trailing `@`. Answers the rendered segments and the bytes consumed.
fn parse_qualified(s: &str) -> Option<(Vec<String>, usize)> {
    let bytes = s.as_bytes();
    let mut segments = Vec::new();
    let mut pos = 0usize;
    loop {
        let (segment, next) = if bytes.get(pos) == Some(&b'%') {
            parse_generic(s, pos)?
        } else {
            let len = bytes
                .get(pos..)?
                .iter()
                .position(|&b| b == b'@' || b == b'$')
                .unwrap_or(bytes.len().saturating_sub(pos));
            (
                s.get(pos..pos.checked_add(len)?)?.to_string(),
                pos.checked_add(len)?,
            )
        };
        segments.push(segment);
        pos = next;
        match (bytes.get(pos), bytes.get(pos.saturating_add(1))) {
            (Some(b'@'), Some(b'$') | None) | (Some(b'$'), _) | (None, _) => {
                return Some((segments, pos));
            }
            (Some(b'@'), Some(_)) => pos = pos.checked_add(1)?,
            _ => return None,
        }
    }
}

/// Parses a generic instance `%Name__N$<N type args>%` at `pos`, answering
/// `Name<Arg,...>` and the position after the closing `%`.
fn parse_generic(s: &str, pos: usize) -> Option<(String, usize)> {
    let body = s.get(pos.checked_add(1)?..)?;
    let marker = body.find("__")?;
    let name = body.get(..marker)?;
    let after = body.get(marker.checked_add(2)?..)?;
    let digits = after.bytes().take_while(u8::is_ascii_digit).count();
    let count: usize = after.get(..digits)?.parse().ok()?;
    let mut cursor = pos
        .checked_add(1)?
        .checked_add(marker)?
        .checked_add(2)?
        .checked_add(digits)?;
    if s.as_bytes().get(cursor) != Some(&b'$') {
        return None;
    }
    cursor = cursor.checked_add(1)?;
    let mut args = Vec::with_capacity(count);
    for _ in 0..count {
        let (arg, next) = parse_type(s, cursor, 0)?;
        args.push(arg);
        cursor = next;
    }
    if s.as_bytes().get(cursor) != Some(&b'%') {
        return None;
    }
    Some((
        format!("{name}<{}>", args.join(",")),
        cursor.checked_add(1)?,
    ))
}

/// Parses one Borland C++ type code at `pos`: a length-prefixed qualified
/// name, a pointer / reference / const prefix (class types are pointers in
/// the C++ view, so `p` is dropped as RTTI names do), or a built-in type.
fn parse_type(s: &str, pos: usize, depth: usize) -> Option<(String, usize)> {
    if depth > 32 {
        return None;
    }
    let bytes = s.as_bytes();
    let code = *bytes.get(pos)?;
    if code.is_ascii_digit() {
        let digits = bytes
            .get(pos..)?
            .iter()
            .take_while(|b| b.is_ascii_digit())
            .count();
        let len: usize = s.get(pos..pos.checked_add(digits)?)?.parse().ok()?;
        let start = pos.checked_add(digits)?;
        let end = start.checked_add(len)?;
        let (segments, used) = parse_qualified(s.get(start..end)?)?;
        return (used == len).then(|| (segments.join("."), end));
    }
    let next = pos.checked_add(1)?;
    match code {
        b'p' | b'r' | b'x' => parse_type(s, next, depth.saturating_add(1)),
        b'u' => {
            let (name, end) = match bytes.get(next)? {
                b'c' => ("Byte", next.checked_add(1)?),
                b's' => ("Word", next.checked_add(1)?),
                b'i' | b'l' => ("Cardinal", next.checked_add(1)?),
                b'j' => ("UInt64", next.checked_add(1)?),
                _ => return None,
            };
            Some((name.to_string(), end))
        }
        _ => {
            let name = match code {
                b'v' => "void",
                b'o' => "Boolean",
                b'c' => "AnsiChar",
                b'b' => "WideChar",
                b'z' => "ShortInt",
                b's' => "SmallInt",
                b'i' | b'l' => "Integer",
                b'j' => "Int64",
                b'f' => "Single",
                b'd' => "Double",
                b'g' => "Extended",
                _ => return None,
            };
            Some((name.to_string(), next))
        }
    }
}

/// Collects every import of a PE binary: the import table's in its order
/// (an ordinal import named `ORDINAL n`, as goblin names it), then the
/// delay-load table's. Empty for non-PE containers.
pub fn collect(ctx: &BinaryContext<'_>) -> Vec<Import> {
    let (Some(pe), Some(base)) = (ctx.pe(), ctx.image_base()) else {
        return Vec::new();
    };
    let mut out: Vec<Import> = pe
        .imports
        .iter()
        .filter_map(|import| {
            Some(Import {
                iat_va: base.checked_add(u64::try_from(import.offset).ok()?)?,
                module: import.dll.to_owned(),
                name: import.name.to_string(),
                delayed: false,
            })
        })
        .collect();
    collect_delayed(ctx, pe, base, &mut out);
    out
}

/// Size of one `ImgDelayDescr` (eight `u32`s).
const DELAY_DESCRIPTOR_SIZE: usize = 32;

/// Reads the PE delay-load directory (data directory 13) into `out`:
/// `ImgDelayDescr` records (`grAttrs`, `rvaDLLName`, `rvaHmod`, `rvaIAT`,
/// `rvaINT`, `rvaBoundIAT`, `rvaUnloadIAT`, `dwTimeStamp`) up to an
/// all-zero one or the directory's end, each with a name table parallel to
/// its IAT. Attribute bit 0 marks RVAs (what Delphi 13 writes); without it
/// the fields are VAs. A name table entry with the high bit set is an
/// ordinal (`ORDINAL n`), otherwise the address of a hint word and the
/// name. A descriptor or entry that does not read is skipped, keeping the
/// rest; [`MAX_DELAY_DESCRIPTORS`] and [`MAX_DELAY_IMPORTS`] bound the walk.
fn collect_delayed(ctx: &BinaryContext<'_>, pe: &PE<'_>, base: u64, out: &mut Vec<Import>) {
    let Some(dir) = pe
        .header
        .optional_header
        .and_then(|h| h.data_directories.get_delay_import_descriptor().copied())
    else {
        return;
    };
    if dir.virtual_address == 0 || dir.size == 0 {
        return;
    }
    let count = usize::try_from(dir.size)
        .unwrap_or(usize::MAX)
        .checked_div(DELAY_DESCRIPTOR_SIZE)
        .unwrap_or(0)
        .min(MAX_DELAY_DESCRIPTORS);
    let mut total = 0usize;
    for i in 0..count {
        let Some(desc_va) = i
            .checked_mul(DELAY_DESCRIPTOR_SIZE)
            .and_then(|step| u64::try_from(step).ok())
            .and_then(|step| {
                base.checked_add(u64::from(dir.virtual_address))?
                    .checked_add(step)
            })
        else {
            break;
        };
        match read_delay_descriptor(ctx, base, desc_va, &mut total, out) {
            Some(true) => {}
            // The all-zero terminator.
            Some(false) => break,
            None => continue,
        }
    }
}

/// Reads one delay-load descriptor at `desc_va` into `out`, counting the
/// entries in `total`: `Some(false)` at the terminator, `None` when the
/// descriptor does not read (its readable entries are kept).
fn read_delay_descriptor(
    ctx: &BinaryContext<'_>,
    base: u64,
    desc_va: u64,
    total: &mut usize,
    out: &mut Vec<Import>,
) -> Option<bool> {
    let data = ctx.data();
    let ptr_size = ctx.pointer_size()?;
    let ordinal_flag = 1u64 << (ptr_size.checked_mul(8)?.checked_sub(1)?);
    let off = ctx.va_to_file(desc_va)?;
    let attrs = read_u32(data, off)?;
    let name_field = read_u32(data, off.checked_add(4)?)?;
    if name_field == 0 {
        return Some(false);
    }
    let iat_field = read_u32(data, off.checked_add(12)?)?;
    let int_field = read_u32(data, off.checked_add(16)?)?;
    let rva = attrs & 1 != 0;
    let to_va = |field: u64| -> Option<u64> {
        if rva {
            base.checked_add(field)
        } else {
            Some(field)
        }
    };
    let module = read_c_string(data, ctx.va_to_file(to_va(u64::from(name_field))?)?)?;
    let iat = to_va(u64::from(iat_field))?;
    let names = ctx.va_to_file(to_va(u64::from(int_field))?)?;
    for i in 0..MAX_DELAY_IMPORTS {
        if *total >= MAX_DELAY_IMPORTS {
            break;
        }
        let step = i.checked_mul(ptr_size)?;
        let thunk = read_ptr(data, names.checked_add(step)?, ptr_size)?;
        if thunk == 0 {
            break;
        }
        let name = if thunk & ordinal_flag != 0 {
            Some(format!("ORDINAL {}", thunk & 0xFFFF))
        } else {
            to_va(thunk)
                .and_then(|hint| ctx.va_to_file(hint))
                .and_then(|at| read_c_string(data, at.checked_add(2)?))
        };
        let Some(name) = name else {
            continue;
        };
        out.push(Import {
            iat_va: iat.checked_add(u64::try_from(step).ok()?)?,
            module: module.clone(),
            name,
            delayed: true,
        });
        *total = total.saturating_add(1);
    }
    Some(true)
}

/// Reads a NUL-terminated ASCII string at `off`.
fn read_c_string(data: &[u8], off: usize) -> Option<String> {
    let rest = data.get(off..)?;
    let len = rest.iter().take(4096).position(|&b| b == 0)?;
    Some(String::from_utf8_lossy(rest.get(..len)?).into_owned())
}

/// Returns `true` for a Borland runtime package module name (`*.bpl`).
pub fn is_package_module(module: &str) -> bool {
    module.len() > 4
        && module
            .get(module.len().saturating_sub(4)..)
            .is_some_and(|ext| ext.eq_ignore_ascii_case(".bpl"))
}

/// Maps a Delphi RTL/VCL runtime package (`rtl60.bpl`, `vcl50.bpl`, ...) to
/// the `dcc` compiler version that built against it, e.g. `"14.0"` for
/// Delphi 6. `None` for other modules and for `rtl100` (shared by Delphi
/// 2006 and 2007).
pub fn rtl_package_compiler_version(module: &str) -> Option<&'static str> {
    let lower = module.to_ascii_lowercase();
    let stem = lower.strip_suffix(".bpl")?;
    let suffix = stem
        .strip_prefix("rtl")
        .or_else(|| stem.strip_prefix("vcl"))?;
    Some(match suffix {
        "40" => "12.0",
        "50" => "13.0",
        "60" => "14.0",
        "70" => "15.0",
        "90" => "17.0",
        "120" => "20.0",
        "140" => "21.0",
        "150" => "22.0",
        "160" => "23.0",
        "170" => "24.0",
        "180" => "25.0",
        "190" => "26.0",
        "200" => "27.0",
        "210" => "28.0",
        "220" => "29.0",
        "230" => "30.0",
        "240" => "31.0",
        "250" => "32.0",
        "260" => "33.0",
        "270" => "34.0",
        "280" => "35.0",
        "290" => "36.0",
        // RAD Studio 13 numbers its packages after the compiler.
        "370" => "37.0",
        _ => return None,
    })
}

/// Returns `true` when `module` is a Delphi RTL or VCL runtime package of
/// any version (`rtl<digits>.bpl`, `vcl<digits>.bpl`).
pub fn is_rtl_package(module: &str) -> bool {
    let lower = module.to_ascii_lowercase();
    lower
        .strip_suffix(".bpl")
        .and_then(|stem| {
            stem.strip_prefix("rtl")
                .or_else(|| stem.strip_prefix("vcl"))
        })
        .is_some_and(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sym(kind: SymbolKind, unit: &str, name: &str) -> Option<BorlandSymbol> {
        Some(BorlandSymbol {
            kind,
            unit: unit.into(),
            name: name.into(),
        })
    }

    #[test]
    fn demangles_typeinfo() {
        assert_eq!(
            BorlandSymbol::demangle("@$xp$14System@Integer"),
            sym(SymbolKind::TypeInfo, "System", "Integer")
        );
        assert_eq!(
            BorlandSymbol::demangle("@$xp$21Vcl@Controls@TControl"),
            sym(SymbolKind::TypeInfo, "Vcl.Controls", "TControl")
        );
        // Length prefix that disagrees with the name is rejected.
        assert_eq!(BorlandSymbol::demangle("@$xp$9System@Integer"), None);
    }

    #[test]
    fn demangles_class_vmt() {
        assert_eq!(
            BorlandSymbol::demangle("@Classes@TPersistent@"),
            sym(SymbolKind::ClassVmt, "Classes", "TPersistent")
        );
        assert_eq!(
            BorlandSymbol::demangle("@Vcl@Controls@TControl@"),
            sym(SymbolKind::ClassVmt, "Vcl.Controls", "TControl")
        );
    }

    #[test]
    fn demangles_generic_instances() {
        assert_eq!(
            BorlandSymbol::demangle(
                "@$xp$59System@Generics@Collections@%TList__1$p17System@TMetaClass%"
            ),
            sym(
                SymbolKind::TypeInfo,
                "System.Generics.Collections",
                "TList<System.TMetaClass>"
            )
        );
        // A nested generic argument is bounded by its length prefix, and a
        // member type after the instance stays in the name.
        assert_eq!(
            BorlandSymbol::demangle(
                "@$xp$107System@Generics@Collections@%TEnumerable__1$p59System@Generics@Collections@%TList__1$p17System@TMetaClass%%"
            ),
            sym(
                SymbolKind::TypeInfo,
                "System.Generics.Collections",
                "TEnumerable<System.Generics.Collections.TList<System.TMetaClass>>"
            )
        );
        assert_eq!(
            BorlandSymbol::demangle(
                "@System@Generics@Collections@%TDictionary__2$i20System@UnicodeString%@TKeyCollection@"
            ),
            sym(
                SymbolKind::ClassVmt,
                "System.Generics.Collections",
                "TDictionary<Integer,System.UnicodeString>.TKeyCollection"
            )
        );
    }

    #[test]
    fn demangles_win64_itanium_names() {
        assert_eq!(
            BorlandSymbol::demangle("_ZTVN6System7Classes11TPersistentE"),
            sym(SymbolKind::ClassVmt, "System.Classes", "TPersistent")
        );
        assert_eq!(
            BorlandSymbol::demangle("_ZTRN6System7Classes10TAlignmentE"),
            sym(SymbolKind::TypeInfo, "System.Classes", "TAlignment")
        );
        // A back-reference names an earlier prefix: S0_ = System::Sensors.
        assert_eq!(
            BorlandSymbol::demangle(
                "_ZTRN6System7Sensors10Components10TSensor__1IPNS0_18TCustomLightSensorEEE"
            ),
            sym(
                SymbolKind::TypeInfo,
                "System.Sensors.Components",
                "TSensor<System.Sensors.TCustomLightSensor>"
            )
        );
        // Repeated arguments come back through back-references (SC_, S9_).
        assert_eq!(
            BorlandSymbol::demangle(
                "_ZTRN6System8Sysutils8TProc__4INS_12DynamicArrayIPNS_8Generics11Collections8TList__1IPNS_8Bindings10Expression18TBindingExpressionEEEEESC_S9_PNS_7TObjectEEE"
            ),
            sym(
                SymbolKind::TypeInfo,
                "System.Sysutils",
                "TProc<System.DynamicArray<System.Generics.Collections.TList<System.Bindings.Expression.TBindingExpression>>,System.DynamicArray<System.Generics.Collections.TList<System.Bindings.Expression.TBindingExpression>>,System.Bindings.Expression.TBindingExpression,System.TObject>"
            )
        );
        assert_eq!(
            BorlandSymbol::demangle(
                "_ZTRN6System14IEnumerable__1INS_15DelphiInterfaceINS_10IInterfaceEEEEE"
            ),
            sym(
                SymbolKind::TypeInfo,
                "System",
                "IEnumerable<System.IInterface>"
            )
        );
        assert_eq!(
            BorlandSymbol::demangle("_ZN6System10AnsiToUtf8ENS_13UnicodeStringE"),
            sym(SymbolKind::Routine, "System", "AnsiToUtf8")
        );
    }

    #[test]
    fn renders_anonymous_types_like_rtti() {
        // A leading underscore alone is an ordinary identifier.
        assert_eq!(
            BorlandSymbol::demangle("@$xp$17Adoint@_Recordset"),
            sym(SymbolKind::TypeInfo, "Adoint", "_Recordset")
        );
        assert_eq!(
            BorlandSymbol::demangle("@$xp$20Vcl@Mask@Vcl_Mask__2"),
            sym(SymbolKind::TypeInfo, "Vcl.Mask", ":2")
        );
        assert_eq!(
            BorlandSymbol::demangle("@$xp$26Vcl@Forms@_TApplication@_1"),
            sym(SymbolKind::TypeInfo, "Vcl.Forms.:TApplication", ":1")
        );
        assert_eq!(
            BorlandSymbol::demangle("_ZTRN6System8Bindings10Expression22_TBindingExpression__1E"),
            sym(
                SymbolKind::TypeInfo,
                "System.Bindings.Expression.:TBindingExpression",
                ":1"
            )
        );
        assert_eq!(
            BorlandSymbol::demangle("_ZTRN17Emuvclusegenerics10TMyList__1IdE2_1E"),
            sym(
                SymbolKind::TypeInfo,
                "Emuvclusegenerics",
                "TMyList<Double>.:1"
            )
        );
    }

    #[test]
    fn demangles_routines() {
        assert_eq!(
            BorlandSymbol::demangle("@System@TObject@Free$qqrv"),
            sym(SymbolKind::Routine, "System", "TObject.Free")
        );
        assert_eq!(
            BorlandSymbol::demangle("@System@Output"),
            sym(SymbolKind::Routine, "System", "Output")
        );
        assert_eq!(BorlandSymbol::demangle("GetProcAddress"), None);
        assert_eq!(BorlandSymbol::demangle("@$xt$Foo"), None);
    }

    #[test]
    fn maps_rtl_packages_to_compiler_versions() {
        assert_eq!(rtl_package_compiler_version("rtl60.bpl"), Some("14.0"));
        assert_eq!(rtl_package_compiler_version("VCL50.BPL"), Some("13.0"));
        assert_eq!(rtl_package_compiler_version("rtl290.bpl"), Some("36.0"));
        assert_eq!(rtl_package_compiler_version("rtl370.bpl"), Some("37.0"));
        assert_eq!(rtl_package_compiler_version("rtl100.bpl"), None);
        assert_eq!(rtl_package_compiler_version("gtpkg.bpl"), None);
        assert!(is_rtl_package("rtl100.bpl"));
        assert!(!is_rtl_package("rtlfoo.bpl"));
        assert!(is_package_module("gtpkg.BPL"));
        assert!(!is_package_module("kernel32.dll"));
    }
}
