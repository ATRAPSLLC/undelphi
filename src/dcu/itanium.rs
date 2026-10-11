//! Itanium-mangled export names of DCU routines, as 64-bit Windows Delphi
//! writes them (`_ZN6System8Sysutils8IntToStrEi`).
//!
//! The Delphi view is the same as for the 32-bit Borland names (see
//! [`super::mangle`]); only the spelling differs:
//!
//! ```text
//! name     _Z N <unit segments> [<class>...] <routine> E <params>
//!          constructor C3, destructor D0
//! params   v for none; top-level const dropped
//! var      R<type>; const by reference RK<type>
//! class    P N...E; class reference P N6System10TMetaClassE
//! strings  N6System13UnicodeStringE, N6System10WideStringE,
//!          N6System11AnsiStringTILt<code page>EEE
//! templates DelphiInterface<I>, DynamicArray<T>, Set<T, lo, hi> (typed
//!          literals L<type><value>E), StaticArray<T, n>
//! procedure PF<result><params>E; method U9__closurePF...E
//! builtins v void, b Boolean, c AnsiChar, w WideChar, a ShortInt,
//!          h Byte, s SmallInt, t Word, i Integer, j Cardinal, x Int64,
//!          y UInt64, f Single, d Double
//! ```
//!
//! Names repeat through the standard Itanium substitutions: every prefix of
//! a nested name, every class and template instance, every pointer,
//! reference and qualified type becomes `S_`, `S0_`, `S1_`, ... in order of
//! first appearance (the routine's own final name excepted).

use std::fmt::Write as _;

/// A type in the Itanium view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum It {
    /// A builtin code.
    Builtin(&'static str),
    /// A name: namespace segments then the type name; one segment is a
    /// global name.
    Name(Vec<String>),
    /// A template instance: the template's qualified name and arguments.
    Template(Vec<String>, Vec<ItArg>),
    /// A name nested in a template instance (`THashSet__1<T>::TVoid`):
    /// the template's qualified name, its arguments, then the nested path.
    Member(Vec<String>, Vec<ItArg>, Vec<String>),
    /// A pointer.
    Ptr(Box<It>),
    /// An lvalue reference.
    Ref(Box<It>),
    /// A const-qualified type.
    Const(Box<It>),
    /// A raw encoding from a C++ override (`P5HDC__`): written verbatim and
    /// counted as a single substitution, as Embarcadero's mangler does.
    Raw(String),
    /// A function type: result, parameters, and whether it is a method
    /// (`__closure`).
    Func {
        /// The result type.
        ret: Box<It>,
        /// The parameter types.
        params: Vec<It>,
        /// A method pointer rather than a plain one.
        closure: bool,
    },
}

/// A template argument.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ItArg {
    /// A type argument.
    Type(It),
    /// A typed integer literal.
    Lit(It, i64),
}

/// Writes Itanium names with a substitution table.
#[derive(Debug, Default)]
pub struct Emitter {
    subs: Vec<String>,
    /// The name written so far.
    pub out: String,
    /// Whether a method pointer's `U9__closure`-qualified type is a
    /// substitution candidate of its own, after the function type and the
    /// pointer (Delphi 10.4 and later; Seattle counts only those two).
    pub qualified_closure: bool,
}

/// The reference to substitution `i`: `S_`, `S0_`, `S1_`, ... (base 36).
fn sub_ref(i: usize) -> String {
    if i == 0 {
        return "S_".to_string();
    }
    let mut n = i.saturating_sub(1);
    let mut digits = Vec::new();
    loop {
        let d = n % 36;
        digits.push(
            char::from_digit(u32::try_from(d).unwrap_or(0), 36)
                .unwrap_or('0')
                .to_ascii_uppercase(),
        );
        n /= 36;
        if n == 0 {
            break;
        }
    }
    digits.reverse();
    format!("S{}_", digits.into_iter().collect::<String>())
}

/// A name component: a length-prefixed source name, or one of the special
/// member names written raw (constructors `C1` - `C3`, destructors `D0` -
/// `D2`, Delphi's class constructor / destructor `v04cctr` / `v04cdtr`).
fn source(name: &str) -> String {
    if matches!(
        name,
        "C1" | "C2" | "C3" | "D0" | "D1" | "D2" | "v04cctr" | "v04cdtr"
    ) {
        return name.to_string();
    }
    format!("{}{name}", name.len())
}

/// The substitution key of a type.
fn key(t: &It) -> String {
    match t {
        It::Builtin(c) => format!("b:{c}"),
        It::Raw(r) => format!("raw:{r}"),
        It::Name(p) => p.join("::"),
        It::Template(p, a) => format!(
            "{}<{}>",
            p.join("::"),
            a.iter().map(arg_key).collect::<Vec<_>>().join(",")
        ),
        It::Member(p, a, m) => format!(
            "{}::{}",
            key(&It::Template(p.clone(), a.clone())),
            m.join("::")
        ),
        It::Ptr(i) => format!("P({})", key(i)),
        It::Ref(i) => format!("R({})", key(i)),
        It::Const(i) => format!("K({})", key(i)),
        It::Func {
            ret,
            params,
            closure,
        } => format!(
            "F{}({};{})",
            if *closure { "c" } else { "" },
            key(ret),
            params.iter().map(key).collect::<Vec<_>>().join(",")
        ),
    }
}

/// The substitution key of a template argument.
fn arg_key(a: &ItArg) -> String {
    match a {
        ItArg::Type(t) => key(t),
        ItArg::Lit(t, v) => format!("L{}={v}", key(t)),
    }
}

impl Emitter {
    /// Returns the index of `k` in the substitution table.
    fn find(&self, k: &str) -> Option<usize> {
        self.subs.iter().position(|s| s == k)
    }

    /// Adds `k` to the substitution table unless present.
    fn add(&mut self, k: String) {
        if self.find(&k).is_none() {
            self.subs.push(k);
        }
    }

    /// Writes a nested name - namespace prefixes, then the final component,
    /// with optional template arguments - adding every prefix (and the
    /// final name unless `is_function`) as a substitution.
    pub fn nested(&mut self, path: &[String], args: Option<&[ItArg]>, is_function: bool) {
        let full_key = |p: &[String]| p.join("::");
        // Whole type already seen.
        if !is_function {
            let k = match args {
                Some(a) => key(&It::Template(path.to_vec(), a.to_vec())),
                None => full_key(path),
            };
            if let Some(i) = self.find(&k) {
                self.out.push_str(&sub_ref(i));
                return;
            }
        }
        if path.len() == 1 && args.is_none() {
            if let Some(name) = path.first() {
                self.out.push_str(&source(name));
            }
            if !is_function {
                self.add(full_key(path));
            }
            return;
        }
        self.out.push('N');
        // Longest prefix already seen (the template name counts as one).
        let mut start = 0;
        for n in (1..=path.len()).rev() {
            if n == path.len() && (args.is_none() || is_function) {
                continue;
            }
            if let Some(i) = self.find(&full_key(path.get(..n).unwrap_or(&[]))) {
                self.out.push_str(&sub_ref(i));
                start = n;
                break;
            }
        }
        for n in start..path.len() {
            if let Some(seg) = path.get(n) {
                self.out.push_str(&source(seg));
            }
            let last = n.saturating_add(1) == path.len();
            if !last || args.is_some() || !is_function {
                self.add(full_key(path.get(..=n).unwrap_or(&[])));
            }
        }
        if let Some(a) = args {
            self.out.push('I');
            for arg in a {
                self.arg(arg);
            }
            self.out.push('E');
            self.add(key(&It::Template(path.to_vec(), a.to_vec())));
        }
        self.out.push('E');
    }

    /// Writes a template instance as the prefix of an enclosing nested
    /// name (no `N`...`E` of its own): its namespace prefixes, the template
    /// name and `I`...`E`, each prefix and the instance becoming
    /// substitutions.
    pub fn nested_prefix(&mut self, path: &[String], args: &[ItArg]) {
        let full_key = |p: &[String]| p.join("::");
        let id = key(&It::Template(path.to_vec(), args.to_vec()));
        if let Some(i) = self.find(&id) {
            self.out.push_str(&sub_ref(i));
            return;
        }
        let mut start = 0;
        for n in (1..=path.len()).rev() {
            if let Some(i) = self.find(&full_key(path.get(..n).unwrap_or(&[]))) {
                self.out.push_str(&sub_ref(i));
                start = n;
                break;
            }
        }
        for n in start..path.len() {
            if let Some(seg) = path.get(n) {
                self.out.push_str(&source(seg));
            }
            self.add(full_key(path.get(..=n).unwrap_or(&[])));
        }
        self.out.push('I');
        for arg in args {
            self.arg(arg);
        }
        self.out.push('E');
        self.add(id);
    }

    /// Writes one name component: a length-prefixed source name, or one of
    /// the special member names written raw (`C1` - `C3`, `D0` - `D2`,
    /// `v04cctr`, `v04cdtr`).
    pub fn component(&mut self, name: &str) {
        self.out.push_str(&source(name));
    }

    /// Writes a template argument.
    fn arg(&mut self, a: &ItArg) {
        match a {
            ItArg::Type(t) => self.ty(t),
            ItArg::Lit(t, v) => {
                self.out.push('L');
                self.ty(t);
                if *v < 0 {
                    let _ = write!(self.out, "n{}", v.unsigned_abs());
                } else {
                    let _ = write!(self.out, "{v}");
                }
                self.out.push('E');
            }
        }
    }

    /// Writes a type.
    pub fn ty(&mut self, t: &It) {
        match t {
            It::Builtin(c) => self.out.push_str(c),
            It::Raw(r) => {
                let k = key(t);
                if let Some(n) = self.find(&k) {
                    self.out.push_str(&sub_ref(n));
                    return;
                }
                self.out.push_str(r);
                // A bare builtin is never a substitution.
                if r.len() > 1 {
                    self.add(k);
                }
            }
            It::Name(p) => self.nested(p, None, false),
            It::Template(p, a) => self.nested(p, Some(a), false),
            It::Member(p, a, m) => {
                let k = key(t);
                if let Some(n) = self.find(&k) {
                    self.out.push_str(&sub_ref(n));
                    return;
                }
                let outer = key(&It::Template(p.clone(), a.clone()));
                self.out.push('N');
                self.nested_prefix(p, a);
                for (i, seg) in m.iter().enumerate() {
                    self.out.push_str(&source(seg));
                    let path = m.get(..=i).unwrap_or(&[]).join("::");
                    self.add(format!("{outer}::{path}"));
                }
                self.out.push('E');
            }
            It::Ptr(i) | It::Ref(i) | It::Const(i) => {
                let k = key(t);
                // A method pointer: the vendor qualifier applies to the
                // pointer.
                let closure =
                    matches!(t, It::Ptr(_)) && matches!(**i, It::Func { closure: true, .. });
                let qualified = format!("U{k}");
                let found = if closure && self.qualified_closure {
                    self.find(&qualified)
                } else {
                    self.find(&k)
                };
                if let Some(n) = found {
                    self.out.push_str(&sub_ref(n));
                    return;
                }
                if closure {
                    self.out.push_str("U9__closure");
                }
                self.out.push(match t {
                    It::Ptr(_) => 'P',
                    It::Ref(_) => 'R',
                    _ => 'K',
                });
                self.ty(i);
                self.add(k);
                if closure && self.qualified_closure {
                    self.add(qualified);
                }
            }
            It::Func {
                ret,
                params,
                closure: _,
            } => {
                let k = key(t);
                if let Some(n) = self.find(&k) {
                    self.out.push_str(&sub_ref(n));
                    return;
                }
                self.out.push('F');
                self.ty(ret);
                if params.is_empty() {
                    self.out.push('v');
                }
                for p in params {
                    self.ty(p);
                }
                self.out.push('E');
                self.add(k);
            }
        }
    }
}

/// Builds `_Z` + the nested function name + parameters.
#[must_use]
pub fn function(path: &[String], params: &[It]) -> String {
    function_with(Emitter::default(), path, params)
}

/// Builds a function name as [`function`] does, with `e`'s settings (see
/// [`Emitter::qualified_closure`]).
#[must_use]
pub fn function_with(mut e: Emitter, path: &[String], params: &[It]) -> String {
    e.out.push_str("_Z");
    e.nested(path, None, true);
    if params.is_empty() {
        e.out.push('v');
    }
    for p in params {
        e.ty(p);
    }
    e.out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(p: &[&str]) -> It {
        It::Name(p.iter().map(|s| s.to_string()).collect())
    }

    fn path(p: &[&str]) -> Vec<String> {
        p.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn substitutions_follow_the_exports() {
        // _ZN6System7Classes7TStream18WriteDescendentResENS_13UnicodeStringEPNS0_10TComponentES4_
        let comp = It::Ptr(Box::new(n(&["System", "Classes", "TComponent"])));
        let got = function(
            &path(&["System", "Classes", "TStream", "WriteDescendentRes"]),
            &[n(&["System", "UnicodeString"]), comp.clone(), comp],
        );
        assert_eq!(
            got,
            "_ZN6System7Classes7TStream18WriteDescendentResENS_13UnicodeStringEPNS0_10TComponentES4_"
        );
        // _ZN6System8Sysutils6FormatENS_13UnicodeStringEPKNS_7TVarRecEiRKNS0_15TFormatSettingsE
        let got = function(
            &path(&["System", "Sysutils", "Format"]),
            &[
                n(&["System", "UnicodeString"]),
                It::Ptr(Box::new(It::Const(Box::new(n(&["System", "TVarRec"]))))),
                It::Builtin("i"),
                It::Ref(Box::new(It::Const(Box::new(n(&[
                    "System",
                    "Sysutils",
                    "TFormatSettings",
                ]))))),
            ],
        );
        assert_eq!(
            got,
            "_ZN6System8Sysutils6FormatENS_13UnicodeStringEPKNS_7TVarRecEiRKNS0_15TFormatSettingsE"
        );
    }

    #[test]
    fn templates_and_closures() {
        // _ZN6System8Sysutils13StringReplaceENS_13UnicodeStringES1_S1_NS_3SetINS0_19System_Sysutils__75ELS3_0ELS3_1EEE
        let e = n(&["System", "Sysutils", "System_Sysutils__75"]);
        let set = It::Template(
            path(&["System", "Set"]),
            vec![
                ItArg::Type(e.clone()),
                ItArg::Lit(e.clone(), 0),
                ItArg::Lit(e, 1),
            ],
        );
        let s = n(&["System", "UnicodeString"]);
        let got = function(
            &path(&["System", "Sysutils", "StringReplace"]),
            &[s.clone(), s.clone(), s, set],
        );
        assert_eq!(
            got,
            "_ZN6System8Sysutils13StringReplaceENS_13UnicodeStringES1_S1_NS_3SetINS0_19System_Sysutils__75ELS3_0ELS3_1EEE"
        );
        // _ZN6System7Classes12TBasicAction12SetOnExecuteEU9__closurePFvPNS_7TObjectEE
        let ev = It::Ptr(Box::new(It::Func {
            ret: Box::new(It::Builtin("v")),
            params: vec![It::Ptr(Box::new(n(&["System", "TObject"])))],
            closure: true,
        }));
        let got = function(
            &path(&["System", "Classes", "TBasicAction", "SetOnExecute"]),
            &[ev],
        );
        assert_eq!(
            got,
            "_ZN6System7Classes12TBasicAction12SetOnExecuteEU9__closurePFvPNS_7TObjectEE"
        );
    }
}
