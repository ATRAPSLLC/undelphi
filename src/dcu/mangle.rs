//! Export names of DCU routines: the mangled names a runtime package
//! exports its routines under, generated from their declarations.
//!
//! A runtime package exports every routine of its units under a name that
//! encodes the unit, the routine and - for overload resolution - its
//! parameter types; the result type is not part of it. Generating that name
//! from a unit's declaration binds each export to its declaration exactly,
//! overloads included. [`Mangler::routine`] writes the dialect of the
//! unit's compiler and platform:
//!
//! - 32-bit Delphi 2009 and later: Borland mangling, below;
//! - 32-bit Delphi before 2009: the same with the differences listed after
//!   it;
//! - 64-bit Windows: Itanium mangling ([`super::itanium`]).
//!
//! The rules were established against the export tables of RAD Studio 10
//! Seattle's and C++Builder 6's runtime packages, which they now match for
//! 98 - 99 % of every export a DCU accounts for, and
//! against our own `dcutypespkg.bpl` exactly.
//!
//! ```text
//! name     @Unit@Segments@[Class@]Routine$<conv><params>
//!          unit segments capitalised then lower case (Urlclient); member
//!          names as declared, '&' escapes written '_'; constructors $bctr,
//!          destructors $bdtr, class constructors / destructors (the
//!          compiler's `Create@` / `Destroy@`) $bcctr / $bcdtr; the unit's
//!          initialization routine `initialization`; a generic instance's
//!          routine is qualified by the instance (%TList__1$<args>%)
//! conv     qqr register, qqs stdcall and safecall, q cdecl and pascal (a
//!          pascal name is upper-cased whole: @UNIT@NAME$QII)
//! params   'v' for none; else one encoding per parameter, Self and a
//!          constructor's hidden flag excluded, an open array's High
//!          included, a safecall result appended as a reference
//! param    ['r' var / out] ['x' const] type; const = flag bit 0x02 clear
//!          a register-convention structured value passed by reference
//!          (record, set, static array wider than a register; variant) is
//!          rx; other conventions write it x
//!          a const pointer to a builtin or static array is px<target>
//! type     builtins: v void, c AnsiChar, b WideChar, zc ShortInt, uc Byte,
//!          s SmallInt, us Word, i Integer, ui Cardinal, l long, j Int64,
//!          uj UInt64, f Single, d Double, g Extended, o Boolean
//!          a subrange: the builtin of its storage; an enum subrange: its
//!          base enumeration
//!          named: <length><Unit@Name> (nested types '@'-joined);
//!          $EXTERNALSYM types drop the unit; a C++ override in the unit
//!          ($OBJTYPENAME: B<raw>, N<name>, Win64 G<raw>) replaces it
//!          anonymous enumeration: <Unit_Segments>__<n>
//!          class: p<named>; class reference: p17System@TMetaClass
//!          interface: System@%DelphiInterface$<named>%
//!          dynamic array: System@%DynamicArray$<element>%
//!          static array: System@%StaticArray$<element>i$i<count>$%
//!          set: System@%Set$<element><t>$i<low>$<t>$i<high>$% (t: t1, or
//!          the builtin)
//!          AnsiString: System@%AnsiStringT$us$i<code page>$%
//!          ShortString: System@%SmallString$uc$i<length>$%
//!          generic instance: <generic unit>@%<Name>__<arity>$<args>%
//!          pointer: p<type>; untyped pointer pv
//!          procedural: pqqr<params>$<result>; method: ynpqqr<params>$<result>
//! repeat   a parameter whose encoding equals an earlier one's (and is not a
//!          bare builtin) is written t<n>, n its 1-based position
//! ```
//!
//! Before Delphi 2009: `AnsiString` is `17System@AnsiString` and `Char` is
//! `c`; a const class or pointer is `px<name>`; template type arguments are
//! prefixed `t` (`%DelphiInterface$t16Xmlintf@IXMLNode%`) and a set reads
//! `%Set$t<element>$i<storage>$<low>$i<storage>$<high>%`; a class method
//! takes its class reference as a first parameter (`p17System@TMetaClass`);
//! a `$NODEFINE` subrange keeps its name; System's `TGUID` is `5_GUID`.

use std::{
    collections::HashMap,
    mem,
    sync::{Arc, Mutex},
};

use crate::dcu::{
    CallConv, Decl, DeclKind, FloatKind, MethodKind, Modifier, Platform, Proc, RangeKind,
    StringKind, TypeKind, TypeTarget, Unit, Version,
    digest::{base64, md5},
    itanium::{self, It, ItArg},
    library::{Library, ResolvedType},
};

/// Record tags the mangling distinguishes.
mod tag {
    /// A value parameter.
    pub const VAL: u8 = 0x21;
    /// A var parameter.
    pub const VAR: u8 = 0x22;
}

/// Writes a unit segment as Borland mangling does: first character upper
/// case, the rest lower case (`URLClient` -> `Urlclient`).
fn segment(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for (i, c) in s.chars().enumerate() {
        if i == 0 {
            out.extend(c.to_uppercase());
        } else {
            out.extend(c.to_lowercase());
        }
    }
    out
}

/// The `@`-joined, case-normalised form of a unit name.
fn unit_path(unit: &str) -> String {
    unit.split('.').map(segment).collect::<Vec<_>>().join("@")
}

/// A length-prefixed name.
fn lp(name: &str) -> String {
    format!("{}{name}", name.len())
}

/// Returns whether an encoding is a bare builtin (never repeated by
/// reference): an optional `x`, an optional `u` / `z`, one letter.
fn is_builtin(enc: &str) -> bool {
    let rest = enc.strip_prefix('x').unwrap_or(enc);
    let rest = rest
        .strip_prefix('u')
        .or_else(|| rest.strip_prefix('z'))
        .unwrap_or(rest);
    rest.len() == 1 && rest.chars().all(|c| c.is_ascii_lowercase())
}

/// Replaces repeated non-builtin encodings by `t<n>` references.
fn with_repeats(encs: Vec<String>) -> String {
    let mut out = String::new();
    let mut seen: Vec<&str> = Vec::new();
    for (i, e) in encs.iter().enumerate() {
        match seen.iter().position(|s| *s == e) {
            Some(first) if !is_builtin(e) => {
                out.push('t');
                out.push_str(&repeat_index(first.saturating_add(1)));
            }
            _ => out.push_str(e),
        }
        let _ = i;
        seen.push(e);
    }
    out
}

/// The digit of a `t<n>` reference: 1 - 9, then letters.
fn repeat_index(n: usize) -> String {
    match n {
        0..=9 => n.to_string(),
        _ => char::from_u32(
            u32::try_from(n.saturating_sub(10))
                .unwrap_or(0)
                .saturating_add(u32::from(b'a')),
        )
        .map(String::from)
        .unwrap_or_default(),
    }
}

/// The special member names constructors and destructors export under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Special {
    /// `$bctr`: a constructor.
    Constructor,
    /// A record's constructor (`$bctr`; Itanium `C1`).
    RecordConstructor,
    /// `$bdtr`: a destructor.
    Destructor,
    /// `$bcctr`: a class constructor.
    ClassConstructor,
    /// `$bcdtr`: a class destructor.
    ClassDestructor,
}

impl Special {
    /// The exported member name.
    fn name(self) -> &'static str {
        match self {
            Self::Constructor | Self::RecordConstructor => "$bctr",
            Self::Destructor => "$bdtr",
            Self::ClassConstructor => "$bcctr",
            Self::ClassDestructor => "$bcdtr",
        }
    }
}

/// Generates Borland export names for the routines of units in a
/// [`Library`].
#[derive(Debug)]
pub struct Mangler<'l> {
    lib: &'l Library,
    /// Per unit: the address of each constructor / destructor routine.
    specials: Mutex<HashMap<String, Arc<HashMap<u32, Special>>>>,
}

impl<'l> Mangler<'l> {
    /// Creates a mangler resolving types through `lib`.
    #[must_use]
    pub fn new(lib: &'l Library) -> Self {
        Self {
            lib,
            specials: Mutex::new(HashMap::new()),
        }
    }

    /// The constructors and destructors of `unit`'s classes and records, by
    /// the address of their implementing routine.
    fn specials(&self, unit: &Unit) -> Arc<HashMap<u32, Special>> {
        if let Ok(cache) = self.specials.lock()
            && let Some(hit) = cache.get(&unit.name)
        {
            return Arc::clone(hit);
        }
        let mut map = HashMap::new();
        for t in &unit.type_defs {
            let (members, record) = match &t.kind {
                TypeKind::Class(c) => (&c.members, false),
                TypeKind::Record { members, .. } | TypeKind::Object { members, .. } => {
                    (members, true)
                }
                _ => continue,
            };
            for d in unit.decls_of(members) {
                let DeclKind::Method(m) = d.kind else {
                    continue;
                };
                let class = m.local.flags_x & 0x01 != 0;
                let special = match (m.kind, class) {
                    (MethodKind::Constructor, false) if record => Special::RecordConstructor,
                    (MethodKind::Constructor, false) => Special::Constructor,
                    (MethodKind::Constructor, true) => Special::ClassConstructor,
                    (MethodKind::Destructor, false) => Special::Destructor,
                    (MethodKind::Destructor, true) => Special::ClassDestructor,
                    (MethodKind::Method, _) => continue,
                };
                map.insert(m.local.location.unsigned_abs(), special);
            }
        }
        let map = Arc::new(map);
        if let Ok(mut cache) = self.specials.lock() {
            cache.insert(unit.name.clone(), Arc::clone(&map));
        }
        map
    }

    /// Returns the export name of the routine `decl` of `unit`, or `None`
    /// when it is not a routine with a signature or a type it uses cannot be
    /// encoded.
    #[must_use]
    pub fn routine(&self, unit: &Arc<Unit>, decl: &Decl) -> Option<String> {
        let DeclKind::Proc(proc) = &decl.kind else {
            return None;
        };
        if proc.unnamed {
            return None;
        }
        if unit.platform == Platform::Win64 {
            return self.itanium_routine(unit, decl, proc);
        }
        self.borland_routine(unit, decl, proc).map(shorten)
    }

    /// The Borland name of `decl` (32-bit Windows), before
    /// [shortening](shorten).
    fn borland_routine(&self, unit: &Arc<Unit>, decl: &Decl, proc: &Proc) -> Option<String> {
        let mut name: Vec<String> = decl.name.split('.').map(|s| s.replace('&', "_")).collect();
        let class_special = match decl.name.rsplit('.').next() {
            Some("Create@") => Some(Special::ClassConstructor),
            Some("Destroy@") => Some(Special::ClassDestructor),
            _ => None,
        };
        if let Some(special) =
            class_special.or_else(|| self.specials(unit).get(&decl.addr).copied())
            && let Some(last) = name.last_mut()
        {
            *last = special.name().to_string();
        } else if decl.name.eq_ignore_ascii_case(&unit.name) {
            // The unit's initialization routine.
            name = vec!["initialization".to_string()];
        }
        let params = self.params(unit, proc)?;
        // A routine of a generic instance: the instance's template form
        // (in the generic's unit) qualifies it.
        if decl.name.starts_with('{') {
            let (instance, rest) = split_instance(&decl.name)?;
            let index = unit.decls_of(&unit.main).find_map(|d| match d.kind {
                DeclKind::Type { type_index } if d.name == instance => Some(type_index),
                _ => None,
            })?;
            let template = self.instance(unit, index)?;
            let raw = template.trim_start_matches(|c: char| c.is_ascii_digit());
            let mut member: Vec<String> = rest.split('.').map(|s| s.replace('&', "_")).collect();
            if let Some(special) =
                class_special.or_else(|| self.specials(unit).get(&decl.addr).copied())
                && let Some(last) = member.last_mut()
            {
                *last = special.name().to_string();
            }
            return Some(format!(
                "@{raw}@{}${}{params}",
                member.join("@"),
                conv(proc.call)
            ));
        }
        let out = format!(
            "@{}@{}${}{params}",
            unit_path(&unit.name),
            name.join("@"),
            conv(proc.call)
        );
        // A `pascal` routine has a C `pascal` name: all upper case.
        Some(if proc.call == CallConv::Pascal {
            out.to_ascii_uppercase()
        } else {
            out
        })
    }

    /// The parameter list of `proc`: `v` when it has none.
    fn params(&self, unit: &Arc<Unit>, proc: &Proc) -> Option<String> {
        let mut encs = Vec::new();
        for (i, d) in unit.decls_of(&proc.params).enumerate() {
            let DeclKind::Local(l) = d.kind else { continue };
            if d.name == "Self" && i == 0 {
                // Before 2009 a class method's class reference is a
                // parameter of its own.
                let class_ref = matches!(
                    self.lib
                        .resolve(unit, l.type_index.unsigned_abs())
                        .and_then(|r| r.unit.type_def_at(r.index).map(|t| t.kind.clone())),
                    Some(TypeKind::ClassRef { .. })
                );
                if classic(unit) && class_ref {
                    encs.push("p17System@TMetaClass".to_string());
                }
                continue;
            }
            if d.name == "." && self.is_boolean(unit, l.type_index.unsigned_abs()) {
                // A constructor's or destructor's hidden outer-call flag.
                continue;
            }
            encs.push(self.param(unit, d.tag, l.flags, l.type_index.unsigned_abs(), proc.call)?);
        }
        // A safecall function returns its result through a trailing
        // reference parameter.
        if proc.call == CallConv::Safecall
            && proc.result_type != 0
            && !matches!(
                unit.type_def_at(proc.result_type).map(|t| &t.kind),
                Some(TypeKind::Void)
            )
        {
            encs.push(format!("r{}", self.type_enc(unit, proc.result_type)?));
        }
        if encs.is_empty() {
            return Some("v".to_string());
        }
        Some(with_repeats(encs))
    }

    /// Returns whether type `index` of `unit` is a Boolean.
    fn is_boolean(&self, unit: &Arc<Unit>, index: u32) -> bool {
        let r = self.lib.resolve(unit, index);
        let (u, i) = match &r {
            Some(r) => (&r.unit, r.index),
            None => (unit, index),
        };
        matches!(
            u.type_def_at(i).map(|t| &t.kind),
            Some(TypeKind::Range {
                kind: RangeKind::Boolean,
                ..
            })
        )
    }

    /// The encoding of one parameter.
    fn param(
        &self,
        unit: &Arc<Unit>,
        tag: u8,
        flags: u32,
        index: u32,
        call: CallConv,
    ) -> Option<String> {
        // Bit 0x02 clear: const (low bits 1 / 0; 3 / 2 are by value).
        let is_const = flags & 0x02 == 0;
        let resolved = self.lib.resolve(unit, index);
        let def = resolved
            .as_ref()
            .and_then(|r| r.unit.type_def_at(r.index).map(|t| t.kind.clone()));
        // Untyped `var X` / `const X`: a pointer to void.
        if matches!(def, Some(TypeKind::Void)) {
            return Some(if is_const { "pxv" } else { "pv" }.to_string());
        }
        // An open array: a pointer to its element, then its High.
        if let Some(TypeKind::Array {
            index_type,
            element_type,
            ..
        }) = &def
            && let Some(r) = &resolved
            && is_open(&r.unit, *index_type)
        {
            let element = self.type_enc(&r.unit, *element_type)?;
            return Some(format!("p{}{element}", if is_const { "x" } else { "" }));
        }
        let base = self.type_enc(unit, index)?;
        let mut out = String::new();
        // A structured value the callee receives by reference (a record,
        // variant, static array or set wider than a register) is a const
        // reference in the C++ view; before 10.4 (verified on Seattle) only
        // when not declared `const`, from 10.4 (verified on 13) either way.
        let by_reference = tag == tag::VAL
            && (!is_const || unit.version.delphi_at_least(Version::DELPHI_10_4_SYDNEY))
            && call == CallConv::Register
            && resolved.as_ref().is_some_and(|r| {
                let size = r.unit.type_def_at(r.index).map_or(0, |t| t.size);
                match def {
                    Some(TypeKind::Variant { .. }) => true,
                    Some(
                        TypeKind::Record { .. }
                        | TypeKind::Object { .. }
                        | TypeKind::Array { .. }
                        | TypeKind::Set { .. }
                        | TypeKind::ShortString { .. },
                    ) => size > i32::from(r.unit.pointer_size()),
                    _ => false,
                }
            });
        if by_reference {
            return Some(format!("rx{base}"));
        }
        if tag == tag::VAL
            && call != CallConv::Register
            && matches!(
                def,
                Some(TypeKind::Variant { .. } | TypeKind::Record { .. } | TypeKind::Object { .. })
            )
        {
            if !unit.version.delphi_at_least(Version::DELPHI_10_4_SYDNEY) {
                // Up to Seattle the other conventions pass a structured
                // value as a const value.
                return Some(format!("x{base}"));
            }
            // From 10.4 a `const` one wider than a register is a const
            // reference, as with `register`, and a plain one a plain value.
            let wide = matches!(def, Some(TypeKind::Variant { .. }))
                || resolved.as_ref().is_some_and(|r| {
                    r.unit.type_def_at(r.index).map_or(0, |t| t.size)
                        > i32::from(r.unit.pointer_size())
                });
            return Some(match (is_const, wide) {
                (true, true) => format!("rx{base}"),
                (true, false) => format!("x{base}"),
                (false, _) => base,
            });
        }
        if tag == tag::VAR {
            out.push('r');
        }
        if is_const && classic(unit) && base.starts_with('p') && tag == tag::VAL {
            // Before 2009 a const class or pointer is a pointer to const.
            return Some(format!("px{}", base.get(1..).unwrap_or("")));
        }
        if is_const {
            // A const pointer-typed value points to const data.
            if tag == tag::VAL
                && matches!(def, Some(TypeKind::Pointer { .. }))
                && let Some(target) = base.strip_prefix('p')
                && (is_builtin(target)
                    || target == "v"
                    || target
                        .trim_start_matches(|c: char| c.is_ascii_digit())
                        .starts_with("System@%StaticArray"))
            {
                return Some(format!("px{target}"));
            }
            out.push('x');
        }
        out.push_str(&base);
        Some(out)
    }

    /// The encoding of type `index` of `unit`. Resolution stops at the
    /// first declaration on the way that carries a C++ type-name override
    /// (`HDC`, declared `type UINT_PTR`, keeps its `HDC__*`).
    fn type_enc(&self, unit: &Arc<Unit>, index: u32) -> Option<String> {
        let mut unit = Arc::clone(unit);
        let mut index = index;
        for _ in 0..32 {
            let slot = unit.type_slot(index)?;
            if let Some(decl) = unit.decl_at(slot.decl)
                && let Some(enc) = override_enc(&unit, decl)
            {
                return Some(enc);
            }
            let TypeTarget::Import { unit: u, symbol } = slot.target else {
                return self.resolved_enc(&ResolvedType { unit, index });
            };
            let import = unit.uses.get(u)?;
            let name = import.symbols.get(symbol)?.name.clone();
            let next = self.lib.unit(&import.name)?;
            let decl = next.decls_of(&next.main).find(|d| {
                matches!(d.kind, DeclKind::Type { .. }) && d.name.eq_ignore_ascii_case(&name)
            })?;
            if let Some(enc) = override_enc(&next, decl) {
                return Some(enc);
            }
            let DeclKind::Type { type_index } = decl.kind else {
                return None;
            };
            unit = next;
            index = type_index;
        }
        None
    }

    /// The encoding of a resolved type.
    fn resolved_enc(&self, r: &ResolvedType) -> Option<String> {
        let unit = &r.unit;
        let slot = unit.type_slot(r.index)?;
        // The declaration that defines the type (`[TN]`), not an alias that
        // happened to name the slot first.
        let defining = match slot.target {
            TypeTarget::Def(id) => unit
                .type_defs
                .get(id)
                .and_then(|t| unit.decl_at(t.name_decl))
                .filter(|d| matches!(d.kind, DeclKind::Type { .. })),
            _ => None,
        };
        let slot_name = defining
            .map(|d| d.name.clone())
            .or_else(|| slot.name.clone());
        let naming = defining.or_else(|| unit.decl_at(slot.decl));
        // An anonymous type the C++ header names after its unit:
        // `:1` in System.Classes is `System_Classes__1`.
        let name = match slot_name.as_deref() {
            Some(n)
                if n.starts_with(':')
                    && n.len() > 1
                    && n[1..].bytes().all(|b| b.is_ascii_digit()) =>
            {
                let base: Vec<String> = unit.name.split('.').map(segment).collect();
                Some(format!("{}__{}", base.join("_"), &n[1..]))
            }
            Some(n) if n.starts_with('.') || n.starts_with(':') => None,
            other => other.map(str::to_string),
        };
        let cpp_flags = naming
            .and_then(|d| {
                d.modifiers.iter().find_map(|m| match m {
                    Modifier::CppFlags(f) => Some(*f),
                    _ => None,
                })
            })
            .unwrap_or(0);
        let external = cpp_flags & 0x04 != 0;
        let nodefine = cpp_flags & 0x08 != 0;
        // C++Builder 6 maps System's TGUID to its own `_GUID`.
        if classic(unit) && unit.name == "System" && slot_name.as_deref() == Some("TGUID") {
            return Some("5_GUID".to_string());
        }
        if let Some(decl) = naming
            && let Some(enc) = override_enc(unit, decl)
        {
            return Some(enc);
        }
        let named = |n: &str| {
            let n = n.replace('.', "@");
            if external {
                lp(&n)
            } else {
                lp(&format!("{}@{n}", unit_path(&unit.name)))
            }
        };
        let def = match slot.target {
            TypeTarget::Def(id) => unit.type_defs.get(id)?,
            _ => return name.map(|n| named(&n)),
        };
        if unit.name == "System"
            && let Some(n) = &name
            && let Some(code) = system_builtin(n)
        {
            // `Char` is AnsiChar before Delphi 2009.
            return Some(
                if n == "Char" && classic(unit) {
                    "c"
                } else {
                    code
                }
                .to_string(),
            );
        }
        // A generic instance: the template form of its generic (a dynamic
        // array instance, `TArray<T>`, stays `DynamicArray<T>`), from its
        // TemplateCall or, for one instantiated implicitly, from its name.
        let instance = || {
            self.instance(unit, r.index).or_else(|| {
                slot_name
                    .as_deref()
                    .filter(|n| n.starts_with('{'))
                    .and_then(|n| self.instance_by_name(unit, n))
            })
        };
        // A procedural instance has no C++ template; it is spelled out
        // below like any procedural type.
        if !matches!(
            def.kind,
            TypeKind::DynArray { .. } | TypeKind::ProcType { .. }
        ) && let Some(generic) = instance()
        {
            return Some(match def.kind {
                TypeKind::Class(_) => format!("p{generic}"),
                TypeKind::Interface { .. } => template(unit, "System@%DelphiInterface", &[generic]),
                _ => generic,
            });
        }
        Some(match &def.kind {
            TypeKind::Enum { base, members, .. }
                if members.is_empty() && *base != r.index && *base != 0 =>
            {
                // A subrange of an enumeration is its base type.
                let base = *base;
                return self.type_enc(unit, base);
            }
            TypeKind::TemplateCall { full, .. } => {
                let full = *full;
                return self.type_enc(unit, full);
            }
            TypeKind::Array {
                index_type,
                element_type,
                ..
            } if !is_open(unit, *index_type) => {
                let (lo, hi) = self.ordinal_range(unit, *index_type)?;
                let count = hi.checked_sub(lo)?.checked_add(1)?;
                let e = self.type_enc(unit, *element_type)?;
                lp(&format!("System@%StaticArray${e}i$i{count}$%"))
            }
            TypeKind::ShortString { .. } => lp(&format!(
                "System@%SmallString$uc$i{}$%",
                def.size.saturating_sub(1)
            )),
            TypeKind::Class(_) => format!("p{}", named(name.as_deref()?)),
            TypeKind::ClassRef { .. } => "p17System@TMetaClass".to_string(),
            TypeKind::Interface { .. } => {
                template(unit, "System@%DelphiInterface", &[named(name.as_deref()?)])
            }
            TypeKind::DynArray { element } => {
                // The element slot holds an open array of the element type.
                let element = match unit.type_def_at(*element).map(|t| &t.kind) {
                    Some(TypeKind::Array {
                        index_type,
                        element_type,
                        ..
                    }) if is_open(unit, *index_type) => *element_type,
                    _ => *element,
                };
                let e = self.type_enc(unit, element)?;
                template(unit, "System@%DynamicArray", &[e])
            }
            TypeKind::String {
                kind: StringKind::Ansi,
                ..
            } if classic(unit) => "17System@AnsiString".to_string(),
            TypeKind::String {
                kind: StringKind::Ansi,
                code_page,
                ..
            } => lp(&format!("System@%AnsiStringT$us$i{code_page}$%")),
            TypeKind::String {
                kind: StringKind::Unicode,
                ..
            } => "20System@UnicodeString".to_string(),
            TypeKind::String {
                kind: StringKind::Wide,
                ..
            } => "17System@WideString".to_string(),
            TypeKind::Pointer { target } => match self.lib.resolve(unit, *target) {
                Some(t)
                    if matches!(
                        t.unit.type_def_at(t.index).map(|d| &d.kind),
                        Some(TypeKind::Void)
                    ) =>
                {
                    "pv".to_string()
                }
                Some(t) => format!("p{}", self.resolved_enc(&t)?),
                None => return None,
            },
            TypeKind::ProcType {
                flags,
                result,
                call,
                params,
            } => {
                let mut encs = Vec::new();
                for d in unit.decls_of(params) {
                    let DeclKind::Local(l) = d.kind else { continue };
                    if d.tag != tag::VAL && d.tag != tag::VAR {
                        continue;
                    }
                    if d.name == "Self" {
                        continue;
                    }
                    encs.push(self.param(
                        unit,
                        d.tag,
                        l.flags,
                        l.type_index.unsigned_abs(),
                        *call,
                    )?);
                }
                let list = if encs.is_empty() {
                    "v".to_string()
                } else {
                    with_repeats(encs)
                };
                let ret = if *result == 0
                    || matches!(
                        unit.type_def_at(*result).map(|t| &t.kind),
                        Some(TypeKind::Void)
                    ) {
                    "v".to_string()
                } else {
                    self.type_enc(unit, *result)?
                };
                let closure = if flags & 0x10 != 0 { "yn" } else { "" };
                format!("{closure}p{}{list}${ret}", conv(*call))
            }
            // Before 2009 a type the C++ header defines itself ($NODEFINE)
            // keeps its name (later ones carry an explicit override); any
            // other subrange is stored as - and mangled as - the smallest
            // integer that holds it.
            TypeKind::Range { .. } if nodefine && classic(unit) && name.is_some() => {
                named(name.as_deref()?)
            }
            TypeKind::Range { kind, high, .. } => range_code(*kind, def.size, *high)?.to_string(),
            TypeKind::Float(k) => match name {
                Some(n) => named(&n),
                None => match k {
                    FloatKind::Single => "f".to_string(),
                    FloatKind::Double => "d".to_string(),
                    FloatKind::Extended => "g".to_string(),
                    _ => return None,
                },
            },
            TypeKind::Set { element, .. } => {
                // `Set<T, lo, hi>`: the bounds are typed by T, written as a
                // reference to the first argument unless T is a builtin,
                // and as T's C value (255 is `-1` in a signed char).
                let e = self.type_enc(unit, *element)?;
                let (lo, hi) = self.ordinal_range(unit, *element)?;
                if classic(unit) {
                    // `Set<T, lo, hi>` with the bounds typed by T's storage.
                    let storage = self.storage_code(unit, *element).unwrap_or("uc");
                    return Some(lp(&format!(
                        "System@%Set$t{e}$i{storage}${lo}$i{storage}${hi}%"
                    )));
                }
                let t = if is_builtin(&e) {
                    e.clone()
                } else {
                    "t1".to_string()
                };
                let value = |v: i64| match e.as_str() {
                    "c" | "zc" => i64::from(v as u8 as i8),
                    _ => v,
                };
                lp(&format!(
                    "System@%Set${e}{t}$i{}${t}$i{}$%",
                    value(lo),
                    value(hi)
                ))
            }
            _ => named(name.as_deref()?),
        })
    }

    /// The template form of type `index` of `unit` when a `TemplateCall`
    /// of the unit instantiates it: `<unit>@%<Name>__<arity>$<args>%`, the
    /// arguments with their own repeat numbering.
    fn instance(&self, unit: &Arc<Unit>, index: u32) -> Option<String> {
        let (generic, args) = unit.type_defs.iter().find_map(|t| match &t.kind {
            TypeKind::TemplateCall {
                generic,
                args,
                full,
                ..
            } if *full == index => Some((*generic, args.clone())),
            _ => None,
        })?;
        let g = self.lib.resolve(unit, generic)?;
        let gname = g.name()?.to_string();
        let (base, arity) = gname.split_once('`')?;
        let mut encs = Vec::new();
        for a in args {
            encs.push(self.type_enc(unit, a)?);
        }
        Some(lp(&format!(
            "{}@%{}__{arity}${}%",
            unit_path(&g.unit.name),
            base.replace('.', "@"),
            with_repeats(encs)
        )))
    }

    /// The template form of a generic instance from its name,
    /// `{Unit}Base<Arg,...>`, each argument resolved by name from `ctx`.
    fn instance_by_name(&self, ctx: &Arc<Unit>, name: &str) -> Option<String> {
        // A type nested in an instance (`{U}TList<X>.TEmptyFunc`) is the
        // instance's template form with the nested path appended.
        if let Some((instance, nested)) = split_instance(name) {
            let outer = self.instance_by_name(ctx, instance)?;
            let bare = outer.trim_start_matches(|c: char| c.is_ascii_digit());
            return Some(lp(&format!("{bare}@{}", nested.replace('.', "@"))));
        }
        let rest = name.strip_prefix('{')?;
        let (gunit, rest) = rest.split_once('}')?;
        let open = rest.find('<')?;
        let base = rest.get(..open)?;
        let inner = rest.get(open.checked_add(1)?..rest.len().checked_sub(1)?)?;
        let args = split_args(inner);
        let mut encs = Vec::new();
        for a in &args {
            encs.push(self.enc_by_name(ctx, a)?);
        }
        Some(template(
            ctx,
            &format!(
                "{}@%{}__{}",
                unit_path(gunit),
                base.replace('.', "@"),
                args.len()
            ),
            &encs,
        ))
    }

    /// The encoding of the type named `q` (`Unit.Name`, or an instance
    /// name) as seen from `ctx`: a declaration of `ctx` of that name first,
    /// else the longest leading run of segments naming a library unit.
    fn enc_by_name(&self, ctx: &Arc<Unit>, q: &str) -> Option<String> {
        let local = |u: &Arc<Unit>, n: &str| {
            u.decls_of(&u.main).find_map(|d| match d.kind {
                DeclKind::Type { type_index } if d.name.eq_ignore_ascii_case(n) => Some(type_index),
                _ => None,
            })
        };
        if let Some(index) = local(ctx, q) {
            return self.type_enc(ctx, index);
        }
        if q.starts_with('{') {
            return self.instance_by_name(ctx, q);
        }
        let segs: Vec<&str> = q.split('.').collect();
        (1..segs.len()).rev().find_map(|n| {
            let unit = self.lib.unit(&segs.get(..n)?.join("."))?;
            let index = local(&unit, &segs.get(n..)?.join("."))?;
            self.type_enc(&unit, index)
        })
    }

    /// The builtin code of the storage of the ordinal type `index`.
    fn storage_code(&self, unit: &Arc<Unit>, index: u32) -> Option<&'static str> {
        let r = self.lib.resolve(unit, index)?;
        let def = r.unit.type_def_at(r.index)?;
        let high = match def.kind {
            TypeKind::Range { high, .. } | TypeKind::Enum { high, .. } => high,
            _ => return None,
        };
        // An enumeration's storage is unsigned.
        Some(
            match (def.size, matches!(def.kind, TypeKind::Enum { .. })) {
                (1, true) => "uc",
                (2, true) => "us",
                (4, true) => "ui",
                _ => range_code(RangeKind::Integer, def.size, high)?,
            },
        )
    }

    /// The bounds of the ordinal type `index` of `unit`.
    fn ordinal_range(&self, unit: &Arc<Unit>, index: u32) -> Option<(i64, i64)> {
        let r = self.lib.resolve(unit, index)?;
        match r.unit.type_def_at(r.index)?.kind {
            TypeKind::Range { low, high, .. } | TypeKind::Enum { low, high, .. } => {
                Some((low, high))
            }
            _ => None,
        }
    }
}

/// Returns the `$OBJTYPENAME` spelling of `kind` (`B` Borland, `G`
/// Itanium, `N` a name) from a declaration's C++ names, as `dialect` (`B`
/// or `G`) reads it.
///
/// Up to Seattle a unit holds one spelling, for its own platform. Delphi 13
/// holds several, separated by `\x01` (`Bo\x01Gb` for `Boolean`), and a
/// spelling can mix dialects: text after `:B`, `:G` or `:V` (MSVC) belongs
/// to that dialect alone, text after `:N` and before any marker to all.
/// `N:B%:V?$:NSmallString:GILh255EE:B$uc$i255$%:V$0PP@@` is the name
/// `%SmallString$uc$i255$%` for Borland and `SmallStringILh255EE` for
/// Itanium.
fn obj_type_name(names: &[String; 3], kind: char, dialect: char) -> Option<String> {
    let raw = names[2].split('\u{1}').find_map(|s| s.strip_prefix(kind))?;
    let mut out = String::new();
    let mut owner = 'N';
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        if c == ':'
            && let Some(&m) = chars.peek()
            && matches!(m, 'B' | 'G' | 'N' | 'V')
        {
            owner = m;
            chars.next();
            continue;
        }
        if owner == 'N' || owner == dialect {
            out.push(c);
        }
    }
    Some(out)
}

/// The encoding a C++ header override on `decl` gives its type: `B<raw>`
/// is the encoding itself, `N<name>` a name in the unit's namespace (the
/// global one for an `$EXTERNALSYM` type).
fn override_enc(unit: &Unit, decl: &Decl) -> Option<String> {
    let external = decl
        .modifiers
        .iter()
        .any(|m| matches!(m, Modifier::CppFlags(f) if f & 0x04 != 0));
    decl.modifiers.iter().find_map(|m| {
        let Modifier::CppNames { names, .. } = m else {
            return None;
        };
        if let Some(raw) = obj_type_name(names, 'B', 'B') {
            return Some(raw);
        }
        let n = obj_type_name(names, 'N', 'B')?;
        Some(if external {
            lp(&n)
        } else {
            lp(&format!("{}@{n}", unit_path(&unit.name)))
        })
    })
}

impl Mangler<'_> {
    /// The Itanium name of `decl` (64-bit Windows).
    fn itanium_routine(&self, unit: &Arc<Unit>, decl: &Decl, proc: &Proc) -> Option<String> {
        let class_special = match decl.name.rsplit('.').next() {
            Some("Create@") => Some(Special::ClassConstructor),
            Some("Destroy@") => Some(Special::ClassDestructor),
            _ => None,
        };
        let special = class_special.or_else(|| self.specials(unit).get(&decl.addr).copied());
        let mut path: Vec<String>;
        let mut member: Vec<String>;
        if decl.name.starts_with('{') {
            let (instance, rest) = split_instance(&decl.name)?;
            let index = unit.decls_of(&unit.main).find_map(|d| match d.kind {
                DeclKind::Type { type_index } if d.name == instance => Some(type_index),
                _ => None,
            })?;
            let It::Template(tpath, targs) = self.it_instance(unit, index)? else {
                return None;
            };
            // The instance is the class prefix: emitted as a template-id.
            let params = self.it_params(unit, proc)?;
            member = rest.split('.').map(|s| s.replace('&', "_")).collect();
            if let Some(sp) = special
                && let Some(last) = member.last_mut()
            {
                *last = it_special(sp).to_string();
            }
            return Some(itanium_instance_function(
                emitter(unit),
                &tpath,
                &targs,
                &member,
                &params,
            ));
        }
        path = unit.name.split('.').map(segment).collect();
        member = decl.name.split('.').map(|s| s.replace('&', "_")).collect();
        if let Some(sp) = special
            && let Some(last) = member.last_mut()
        {
            *last = it_special(sp).to_string();
        } else if decl.name.eq_ignore_ascii_case(&unit.name) {
            member = vec!["initialization".to_string()];
        }
        path.extend(member);
        let params = self.it_params(unit, proc)?;
        Some(itanium::function_with(emitter(unit), &path, &params))
    }

    /// The parameter types of `proc` in the Itanium view.
    fn it_params(&self, unit: &Arc<Unit>, proc: &Proc) -> Option<Vec<It>> {
        let mut out = Vec::new();
        for (i, d) in unit.decls_of(&proc.params).enumerate() {
            let DeclKind::Local(l) = d.kind else { continue };
            if d.name == "Self" && i == 0 {
                continue;
            }
            if d.name == "." && self.is_boolean(unit, l.type_index.unsigned_abs()) {
                continue;
            }
            // An open array's High, typed by the open index range (`0..-1`):
            // a signed integer of the range's size (`long long` on Delphi
            // 13 Win64, `int` on Seattle).
            let index = l.type_index.unsigned_abs();
            if d.name == "." && is_open(unit, index) {
                let size = unit.type_def_at(index).map_or(4, |t| t.size);
                out.push(It::Builtin(if size == 8 { "x" } else { "i" }));
                continue;
            }
            out.push(self.it_param(unit, d.tag, l.flags, index, proc.call)?);
        }
        if proc.call == CallConv::Safecall
            && proc.result_type != 0
            && !matches!(
                unit.type_def_at(proc.result_type).map(|t| &t.kind),
                Some(TypeKind::Void)
            )
        {
            out.push(It::Ref(Box::new(self.it_type(unit, proc.result_type)?)));
        }
        Some(out)
    }

    /// One parameter in the Itanium view: top-level const dropped, `var`
    /// and `out` a reference, a const structured value a const reference.
    fn it_param(
        &self,
        unit: &Arc<Unit>,
        tag: u8,
        flags: u32,
        index: u32,
        call: CallConv,
    ) -> Option<It> {
        let is_const = flags & 0x02 == 0;
        let resolved = self.lib.resolve(unit, index);
        let def = resolved
            .as_ref()
            .and_then(|r| r.unit.type_def_at(r.index).map(|t| t.kind.clone()));
        if matches!(def, Some(TypeKind::Void)) {
            let v = It::Builtin("v");
            return Some(It::Ptr(Box::new(if is_const {
                It::Const(Box::new(v))
            } else {
                v
            })));
        }
        if let Some(TypeKind::Array {
            index_type,
            element_type,
            ..
        }) = &def
            && let Some(r) = &resolved
            && is_open(&r.unit, *index_type)
        {
            let e = self.it_type(&r.unit, *element_type)?;
            return Some(It::Ptr(Box::new(if is_const {
                It::Const(Box::new(e))
            } else {
                e
            })));
        }
        let t = self.it_type(unit, index)?;
        if tag == tag::VAR {
            return Some(It::Ref(Box::new(if is_const {
                It::Const(Box::new(t))
            } else {
                t
            })));
        }
        // A variant, or a record wider than a register, travels as a const
        // reference. From 10.4 (verified on 13) a `const` record of exactly
        // a register's width does too, and a plain variant in a routine of
        // another convention (`stdcall`, `safecall`) travels by value.
        let size = resolved
            .as_ref()
            .and_then(|r| r.unit.type_def_at(r.index).map(|d| d.size))
            .unwrap_or(0);
        let modern = unit.version.delphi_at_least(Version::DELPHI_10_4_SYDNEY);
        let by_reference = match def {
            Some(TypeKind::Variant { .. }) => !modern || is_const || call == CallConv::Register,
            Some(TypeKind::Record { .. } | TypeKind::Object { .. }) => {
                size > 8 || (modern && is_const && size == 8)
            }
            _ => false,
        };
        if by_reference {
            return Some(It::Ref(Box::new(It::Const(Box::new(t)))));
        }
        // A const pointer to a builtin points to const data.
        if is_const
            && matches!(def, Some(TypeKind::Pointer { .. }))
            && let It::Ptr(inner) = &t
            && matches!(**inner, It::Builtin(_))
        {
            return Some(It::Ptr(Box::new(It::Const(inner.clone()))));
        }
        Some(t)
    }

    /// The Itanium type of type `index` of `unit`, stopping at C++
    /// overrides on the way.
    fn it_type(&self, unit: &Arc<Unit>, index: u32) -> Option<It> {
        let mut unit = Arc::clone(unit);
        let mut index = index;
        for _ in 0..32 {
            let slot = unit.type_slot(index)?;
            if let Some(decl) = unit.decl_at(slot.decl)
                && let Some(t) = it_override(&unit, decl)
            {
                return Some(t);
            }
            let TypeTarget::Import { unit: u, symbol } = slot.target else {
                return self.it_resolved(&ResolvedType { unit, index });
            };
            let import = unit.uses.get(u)?;
            let name = import.symbols.get(symbol)?.name.clone();
            let next = self.lib.unit(&import.name)?;
            let decl = next.decls_of(&next.main).find(|d| {
                matches!(d.kind, DeclKind::Type { .. }) && d.name.eq_ignore_ascii_case(&name)
            })?;
            if let Some(t) = it_override(&next, decl) {
                return Some(t);
            }
            let DeclKind::Type { type_index } = decl.kind else {
                return None;
            };
            unit = next;
            index = type_index;
        }
        None
    }

    /// The qualified Itanium name of a named type of `unit`.
    fn it_name(unit: &Unit, name: &str, external: bool) -> It {
        if external {
            return It::Name(vec![name.replace('.', "@")]);
        }
        let mut p: Vec<String> = unit.name.split('.').map(segment).collect();
        p.extend(name.split('.').map(str::to_string));
        It::Name(p)
    }

    /// The Itanium type of a resolved type.
    fn it_resolved(&self, r: &ResolvedType) -> Option<It> {
        let unit = &r.unit;
        let slot = unit.type_slot(r.index)?;
        let defining = match slot.target {
            TypeTarget::Def(id) => unit
                .type_defs
                .get(id)
                .and_then(|t| unit.decl_at(t.name_decl))
                .filter(|d| matches!(d.kind, DeclKind::Type { .. })),
            _ => None,
        };
        let slot_name = defining
            .map(|d| d.name.clone())
            .or_else(|| slot.name.clone());
        let naming = defining.or_else(|| unit.decl_at(slot.decl));
        if let Some(decl) = naming
            && let Some(t) = it_override(unit, decl)
        {
            return Some(t);
        }
        let cpp_flags = naming
            .and_then(|d| {
                d.modifiers.iter().find_map(|m| match m {
                    Modifier::CppFlags(f) => Some(*f),
                    _ => None,
                })
            })
            .unwrap_or(0);
        let external = cpp_flags & 0x04 != 0;
        let name = match slot_name.as_deref() {
            Some(n)
                if n.starts_with(':')
                    && n.len() > 1
                    && n[1..].bytes().all(|b| b.is_ascii_digit()) =>
            {
                let base: Vec<String> = unit.name.split('.').map(segment).collect();
                Some(format!("{}__{}", base.join("_"), &n[1..]))
            }
            Some(n) if n.starts_with('.') || n.starts_with(':') => None,
            other => other.map(str::to_string),
        };
        let def = match slot.target {
            TypeTarget::Def(id) => unit.type_defs.get(id)?,
            _ => return name.map(|n| Self::it_name(unit, &n, external)),
        };
        if unit.name == "System"
            && let Some(n) = &name
            && let Some(code) = it_system_builtin(n)
        {
            return Some(code);
        }
        // A type nested in an instance (`{U}THashSet<string>.TVoid`): the
        // instance, which the unit declares, then the nested path.
        let nested = || {
            let (instance, member) = split_instance(slot_name.as_deref()?)?;
            let index = unit.decls_of(&unit.main).find_map(|d| match d.kind {
                DeclKind::Type { type_index } if d.name == instance => Some(type_index),
                _ => None,
            })?;
            let It::Template(p, a) = self.it_instance(unit, index)? else {
                return None;
            };
            Some(It::Member(
                p,
                a,
                member.split('.').map(str::to_string).collect(),
            ))
        };
        if !matches!(def.kind, TypeKind::DynArray { .. })
            && let Some(t) = self.it_instance(unit, r.index).or_else(nested)
        {
            return Some(match def.kind {
                TypeKind::Class(_) => It::Ptr(Box::new(t)),
                TypeKind::Interface { .. } => {
                    it_template(&["System", "DelphiInterface"], vec![ItArg::Type(t)])
                }
                _ => t,
            });
        }
        let named = |n: &str| Self::it_name(unit, n, external);
        Some(match &def.kind {
            TypeKind::Enum { base, members, .. }
                if members.is_empty() && *base != r.index && *base != 0 =>
            {
                let base = *base;
                return self.it_type(unit, base);
            }
            TypeKind::TemplateCall { full, .. } => {
                let full = *full;
                return self.it_type(unit, full);
            }
            TypeKind::Array {
                index_type,
                element_type,
                ..
            } if !is_open(unit, *index_type) => {
                let (lo, hi) = self.ordinal_range(unit, *index_type)?;
                let count = hi.checked_sub(lo)?.checked_add(1)?;
                let e = self.it_type(unit, *element_type)?;
                it_template(
                    &["System", "StaticArray"],
                    vec![ItArg::Type(e), ItArg::Lit(It::Builtin("i"), count)],
                )
            }
            TypeKind::ShortString { .. } => it_template(
                &["System", "SmallString"],
                vec![ItArg::Lit(
                    It::Builtin("h"),
                    i64::from(def.size.saturating_sub(1)),
                )],
            ),
            TypeKind::Class(_) => It::Ptr(Box::new(named(name.as_deref()?))),
            // A named class reference keeps its typedef name.
            TypeKind::ClassRef { class_type, .. } => match name {
                // A class reference the C++ header defines itself
                // (`TClass`) is `TMetaClass*`; any other named one keeps
                // its typedef name.
                Some(_) if cpp_flags & 0x08 != 0 => It::Ptr(Box::new(It::Name(vec![
                    "System".into(),
                    "TMetaClass".into(),
                ]))),
                Some(n) => named(&n),
                // `class of TObject` is System::TClass.
                None if self
                    .lib
                    .resolve(unit, *class_type)
                    .is_some_and(|c| c.unit.name == "System" && c.name() == Some("TObject")) =>
                {
                    It::Name(vec!["System".into(), "TClass".into()])
                }
                None => It::Ptr(Box::new(It::Name(vec![
                    "System".into(),
                    "TMetaClass".into(),
                ]))),
            },
            TypeKind::Interface { .. } => it_template(
                &["System", "DelphiInterface"],
                vec![ItArg::Type(named(name.as_deref()?))],
            ),
            TypeKind::DynArray { element } => {
                let element = match unit.type_def_at(*element).map(|t| &t.kind) {
                    Some(TypeKind::Array {
                        index_type,
                        element_type,
                        ..
                    }) if is_open(unit, *index_type) => *element_type,
                    _ => *element,
                };
                it_template(
                    &["System", "DynamicArray"],
                    vec![ItArg::Type(self.it_type(unit, element)?)],
                )
            }
            TypeKind::String {
                kind: StringKind::Ansi,
                code_page,
                ..
            } => it_template(
                &["System", "AnsiStringT"],
                vec![ItArg::Lit(It::Builtin("t"), i64::from(*code_page))],
            ),
            TypeKind::String {
                kind: StringKind::Unicode,
                ..
            } => It::Name(vec!["System".into(), "UnicodeString".into()]),
            TypeKind::String {
                kind: StringKind::Wide,
                ..
            } => It::Name(vec!["System".into(), "WideString".into()]),
            TypeKind::Pointer { target } => match self.lib.resolve(unit, *target) {
                Some(t)
                    if matches!(
                        t.unit.type_def_at(t.index).map(|d| &d.kind),
                        Some(TypeKind::Void)
                    ) =>
                {
                    It::Ptr(Box::new(It::Builtin("v")))
                }
                Some(_) => It::Ptr(Box::new(self.it_type(unit, *target)?)),
                None => return None,
            },
            TypeKind::ProcType {
                flags,
                result,
                params,
                call,
            } => {
                let mut ps = Vec::new();
                for d in unit.decls_of(params) {
                    let DeclKind::Local(l) = d.kind else { continue };
                    if d.tag != tag::VAL && d.tag != tag::VAR {
                        continue;
                    }
                    if d.name == "Self" {
                        continue;
                    }
                    ps.push(self.it_param(
                        unit,
                        d.tag,
                        l.flags,
                        l.type_index.unsigned_abs(),
                        *call,
                    )?);
                }
                let ret = if *result == 0
                    || matches!(
                        unit.type_def_at(*result).map(|t| &t.kind),
                        Some(TypeKind::Void)
                    ) {
                    It::Builtin("v")
                } else {
                    self.it_type(unit, *result)?
                };
                It::Ptr(Box::new(It::Func {
                    ret: Box::new(ret),
                    params: ps,
                    closure: flags & 0x10 != 0,
                }))
            }
            TypeKind::Range { kind, high, .. } => it_range(*kind, def.size, *high)?,
            TypeKind::Float(k) => match name {
                Some(n) => named(&n),
                None => It::Builtin(match k {
                    FloatKind::Single => "f",
                    FloatKind::Double => "d",
                    FloatKind::Extended => "e",
                    _ => return None,
                }),
            },
            TypeKind::Set { element, .. } => {
                let e = self.it_type(unit, *element)?;
                let (lo, hi) = self.ordinal_range(unit, *element)?;
                it_template(
                    &["System", "Set"],
                    vec![
                        ItArg::Type(e.clone()),
                        ItArg::Lit(e.clone(), lo),
                        ItArg::Lit(e, hi),
                    ],
                )
            }
            _ => named(name.as_deref()?),
        })
    }

    /// The template form of a generic instance in the Itanium view.
    fn it_instance(&self, unit: &Arc<Unit>, index: u32) -> Option<It> {
        let (generic, args) = unit.type_defs.iter().find_map(|t| match &t.kind {
            TypeKind::TemplateCall {
                generic,
                args,
                full,
                ..
            } if *full == index => Some((*generic, args.clone())),
            _ => None,
        })?;
        let g = self.lib.resolve(unit, generic)?;
        let gname = g.name()?.to_string();
        let (base, arity) = gname.split_once('`')?;
        let mut p: Vec<String> = g.unit.name.split('.').map(segment).collect();
        p.extend(base.split('.').map(str::to_string));
        // The template is named with its arity: TList__1.
        if let Some(last) = p.last_mut() {
            *last = format!("{last}__{arity}");
        }
        let mut a = Vec::new();
        for x in args {
            a.push(ItArg::Type(self.it_type(unit, x)?));
        }
        Some(It::Template(p, a))
    }
}

/// The Itanium name of a constructor / destructor variant.
fn it_special(s: Special) -> &'static str {
    match s {
        Special::Constructor => "C3",
        Special::RecordConstructor => "C1",
        Special::Destructor => "D0",
        Special::ClassConstructor => "v04cctr",
        Special::ClassDestructor => "v04cdtr",
    }
}

/// An Itanium emitter configured for the compiler that wrote `unit`.
fn emitter(unit: &Unit) -> itanium::Emitter {
    let mut e = itanium::Emitter::default();
    e.qualified_closure = unit.version.delphi_at_least(Version::DELPHI_10_4_SYDNEY);
    e
}

/// A template instance of a `System` template (Itanium view).
fn it_template(path: &[&str], args: Vec<ItArg>) -> It {
    It::Template(path.iter().map(|s| (*s).to_string()).collect(), args)
}

/// A function whose prefix is a generic instance: `_ZN <instance> <member>
/// E <params>`.
fn itanium_instance_function(
    mut e: itanium::Emitter,
    tpath: &[String],
    targs: &[ItArg],
    member: &[String],
    params: &[It],
) -> String {
    e.out.push_str("_ZN");
    // The instance is the prefix; the members follow inside the same N...E.
    e.nested_prefix(tpath, targs);
    for m in member {
        e.component(m);
    }
    e.out.push('E');
    if params.is_empty() {
        e.out.push('v');
    }
    for p in params {
        e.ty(p);
    }
    e.out
}

/// The Itanium type of a fundamental `System` type.
fn it_system_builtin(name: &str) -> Option<It> {
    Some(It::Builtin(match name {
        "Integer" | "LongInt" | "FixedInt" => "i",
        "Cardinal" | "LongWord" | "FixedUInt" => "j",
        "NativeInt" => "x",
        "NativeUInt" => "y",
        "SmallInt" => "s",
        "Word" => "t",
        "ShortInt" => "a",
        "Byte" => "h",
        "Int64" => "x",
        "UInt64" => "y",
        "Boolean" => "b",
        "AnsiChar" => "c",
        "WideChar" | "Char" => "w",
        "Single" => "f",
        "Double" => "d",
        // Win64 stores Extended as a Double but mangles it as long double.
        "Extended" => "e",
        "Pointer" => return Some(It::Ptr(Box::new(It::Builtin("v")))),
        _ => return None,
    }))
}

/// The Itanium builtin of an ordinal range.
fn it_range(kind: RangeKind, size: i32, high: i64) -> Option<It> {
    Some(It::Builtin(match (kind, size) {
        (RangeKind::Char, _) => "c",
        (RangeKind::WideChar, _) => "w",
        (RangeKind::Boolean, 1) => "b",
        (_, 1) if high <= i64::from(i8::MAX) => "a",
        (_, 1) => "h",
        (_, 2) if high <= i64::from(i16::MAX) => "s",
        (_, 2) => "t",
        (_, 4) if high <= i64::from(i32::MAX) => "i",
        (_, 4) => "j",
        (_, 8) if high >= 0 => "x",
        (_, 8) => "y",
        _ => return None,
    }))
}

/// The Itanium type a C++ override on `decl` gives (the override is written
/// in Borland's encoding: `B<raw>` / `N<name>`).
fn it_override(unit: &Unit, decl: &Decl) -> Option<It> {
    let external = decl
        .modifiers
        .iter()
        .any(|m| matches!(m, Modifier::CppFlags(f) if f & 0x04 != 0));
    decl.modifiers.iter().find_map(|m| {
        let Modifier::CppNames { names, .. } = m else {
            return None;
        };
        // `G<raw>` is the Itanium encoding itself, `B<raw>` the Borland one.
        // Seattle counts a raw spelling as one substitution candidate
        // (`P6HWND__`); from 10.4 (verified on 13) each of its components
        // is a candidate (`6HWND__`, then `P6HWND__`).
        if let Some(raw) = obj_type_name(names, 'G', 'G') {
            return Some(match itanium_fragment(&raw)? {
                It::Builtin(b) => It::Builtin(b),
                parsed if unit.version.delphi_at_least(Version::DELPHI_10_4_SYDNEY) => parsed,
                _ => It::Raw(raw),
            });
        }
        if let Some(raw) = obj_type_name(names, 'B', 'B') {
            return borland_to_it(&raw);
        }
        let n = obj_type_name(names, 'N', 'G')?;
        Some(Mangler::it_name(unit, &n, external))
    })
}

/// Parses a raw Itanium type fragment (`P6HWND__`, `l`, `Pv`).
fn itanium_fragment(raw: &str) -> Option<It> {
    fn one(s: &str) -> Option<(It, &str)> {
        let first = s.chars().next()?;
        match first {
            'P' => one(s.get(1..)?).map(|(t, r)| (It::Ptr(Box::new(t)), r)),
            'R' => one(s.get(1..)?).map(|(t, r)| (It::Ref(Box::new(t)), r)),
            'K' => one(s.get(1..)?).map(|(t, r)| (It::Const(Box::new(t)), r)),
            '0'..='9' => {
                let digits = s.bytes().take_while(u8::is_ascii_digit).count();
                let len: usize = s.get(..digits)?.parse().ok()?;
                let end = digits.checked_add(len)?;
                Some((
                    It::Name(vec![s.get(digits..end)?.to_string()]),
                    s.get(end..)?,
                ))
            }
            _ => {
                let code: &'static str = match first {
                    'v' => "v",
                    'b' => "b",
                    'c' => "c",
                    'a' => "a",
                    'h' => "h",
                    'w' => "w",
                    's' => "s",
                    't' => "t",
                    'i' => "i",
                    'j' => "j",
                    'l' => "l",
                    'm' => "m",
                    'x' => "x",
                    'y' => "y",
                    'f' => "f",
                    'd' => "d",
                    'e' => "e",
                    _ => return None,
                };
                Some((It::Builtin(code), s.get(1..)?))
            }
        }
    }
    let (t, rest) = one(raw)?;
    rest.is_empty().then_some(t)
}

/// Translates a Borland type encoding fragment to the Itanium view.
fn borland_to_it(raw: &str) -> Option<It> {
    let (t, rest) = borland_type(raw)?;
    rest.is_empty().then_some(t)
}

/// Parses one Borland type from the front of `s`.
fn borland_type(s: &str) -> Option<(It, &str)> {
    let first = s.chars().next()?;
    match first {
        'p' => borland_type(s.get(1..)?).map(|(t, r)| (It::Ptr(Box::new(t)), r)),
        'r' => borland_type(s.get(1..)?).map(|(t, r)| (It::Ref(Box::new(t)), r)),
        'x' => borland_type(s.get(1..)?).map(|(t, r)| (It::Const(Box::new(t)), r)),
        '0'..='9' => {
            let digits = s.bytes().take_while(u8::is_ascii_digit).count();
            let len: usize = s.get(..digits)?.parse().ok()?;
            let name = s.get(digits..digits.checked_add(len)?)?;
            let parts: Vec<String> = name.split('@').map(str::to_string).collect();
            Some((It::Name(parts), s.get(digits.checked_add(len)?..)?))
        }
        'u' | 'z' => {
            let code = s.get(..2)?;
            let it = match code {
                "uc" => "h",
                "zc" => "a",
                "us" => "t",
                "ui" => "j",
                "ul" => "m",
                "uj" => "y",
                _ => return None,
            };
            Some((It::Builtin(it), s.get(2..)?))
        }
        _ => {
            let it = match first {
                'v' => "v",
                'c' => "c",
                'b' => "w",
                's' => "s",
                'i' => "i",
                'l' => "l",
                'j' => "x",
                'f' => "f",
                'd' => "d",
                'g' => "d",
                'o' => "b",
                _ => return None,
            };
            Some((It::Builtin(it), s.get(1..)?))
        }
    }
}

/// Splits a generic argument list at its top-level commas.
fn split_args(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut cur = String::new();
    for c in s.chars() {
        match c {
            '<' => depth = depth.saturating_add(1),
            '>' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                out.push(mem::take(&mut cur));
                continue;
            }
            _ => {}
        }
        cur.push(c);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Splits a generic instance member name - `{Unit}Name<Args>.Member` -
/// into the instance's type name and the member path.
fn split_instance(name: &str) -> Option<(&str, &str)> {
    let mut depth = 0usize;
    for (i, c) in name.char_indices() {
        match c {
            '<' => depth = depth.saturating_add(1),
            '>' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    let end = i.checked_add(1)?;
                    let rest = name.get(end..)?.strip_prefix('.')?;
                    return Some((name.get(..end)?, rest));
                }
            }
            _ => {}
        }
    }
    None
}

/// The longest Borland export name the compiler writes in full.
const MAX_NAME: usize = 252;

/// Shortens a Borland name longer than [`MAX_NAME`] the way the compiler
/// does: its first 231 characters, then the first 22 characters of the
/// base64 MD5 digest of the whole name (253 characters in all).
fn shorten(name: String) -> String {
    if name.len() <= MAX_NAME {
        return name;
    }
    let digest = base64(&md5(name.as_bytes()));
    let mut out: String = name.chars().take(231).collect();
    out.extend(digest.chars().take(22));
    out
}

/// Returns whether `unit` uses the pre-2009 mangling dialect: AnsiString
/// by name, `Char` as `c`, `t`-prefixed template type arguments, class
/// methods with their class reference.
fn classic(unit: &Unit) -> bool {
    !unit.version.delphi_at_least(Version::DELPHI_2009)
}

/// The template form of `Name<args>` in a dialect: modern `$a1a2%`,
/// classic `$ta1$ta2%`.
fn template(unit: &Unit, path: &str, args: &[String]) -> String {
    if classic(unit) {
        let a: Vec<String> = args.iter().map(|a| format!("t{a}")).collect();
        lp(&format!("{path}${}%", a.join("$")))
    } else {
        lp(&format!("{path}${}%", with_repeats(args.to_vec())))
    }
}

/// Returns whether type `index` is an open-array index (`0..-1`).
fn is_open(unit: &Unit, index: u32) -> bool {
    matches!(
        unit.type_def_at(index).map(|t| &t.kind),
        Some(TypeKind::Range {
            low: 0,
            high: -1,
            ..
        })
    )
}

/// The suffix of a calling convention.
fn conv(call: CallConv) -> &'static str {
    match call {
        CallConv::Register => "qqr",
        CallConv::Stdcall | CallConv::Safecall => "qqs",
        CallConv::Cdecl => "q",
        CallConv::Pascal => "q",
    }
}

/// The builtin code of a fundamental `System` type.
fn system_builtin(name: &str) -> Option<&'static str> {
    Some(match name {
        "Integer" | "NativeInt" | "LongInt" | "FixedInt" => "i",
        "Cardinal" | "NativeUInt" | "LongWord" | "FixedUInt" => "ui",
        "SmallInt" => "s",
        "Word" => "us",
        "ShortInt" => "zc",
        "Byte" => "uc",
        "Int64" => "j",
        "UInt64" => "uj",
        "Boolean" => "o",
        "AnsiChar" => "c",
        "WideChar" | "Char" => "b",
        "Single" => "f",
        "Double" => "d",
        "Extended" => "g",
        "Pointer" => "pv",
        _ => return None,
    })
}

/// The builtin code of an ordinal range: the signed integer of its size
/// when its highest value fits, the unsigned one otherwise.
fn range_code(kind: RangeKind, size: i32, high: i64) -> Option<&'static str> {
    Some(match (kind, size) {
        (RangeKind::Char, _) => "c",
        (RangeKind::WideChar, _) => "b",
        (RangeKind::Boolean, 1) => "o",
        (_, 1) if high <= i64::from(i8::MAX) => "zc",
        (_, 1) => "uc",
        (_, 2) if high <= i64::from(i16::MAX) => "s",
        (_, 2) => "us",
        (_, 4) if high <= i64::from(i32::MAX) => "i",
        (_, 4) => "ui",
        (_, 8) if high >= 0 => "j",
        (_, 8) => "uj",
        _ => return None,
    })
}
