//! The type and prototype notation of knowledge-base records, read back
//! into structure.
//!
//! A knowledge base stores type expressions, field and parameter types and
//! method prototypes as DCU32INT prints them (see [`build`](super::build)):
//! `array[$0..-$1] of System.TVarRec`, `procedure(val Sender:System.TObject)
//! of object`, `class function Create(val Self:class of TFoo):TFoo;
//! virtual`. [`TypeExpr::parse`] and [`Prototype::parse`] read that text
//! back into the expression it prints, so a consumer can resolve the names
//! in it against its own types rather than parse strings itself.
//!
//! Both are total: text outside the notation reads as
//! [`TypeExpr::Unknown`] (a prototype as `None`), never as an error, since
//! the published files carry forms IDR's maker produced from units no
//! longer available to check against.

use crate::kb::CallKind;

/// A type expression of the notation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeExpr {
    /// A type referred to by name, qualified with its unit when the record
    /// imports it (`System.Integer`, `TStrings`).
    Named(String),
    /// A distinct copy of another type (`type System.Cardinal`).
    Copy(Box<TypeExpr>),
    /// A pointer (`^TFoo`).
    Pointer(Box<TypeExpr>),
    /// An array: its index type (a range for a static array, `$0..-$1` for
    /// an open or dynamic one) and its element type.
    Array {
        /// The index type.
        index: Box<TypeExpr>,
        /// The element type.
        element: Box<TypeExpr>,
    },
    /// An ordinal subrange, its bounds as printed (`$0..$FF`, `#0..#$7F`,
    /// `false..true`).
    Range {
        /// The low bound.
        low: String,
        /// The high bound.
        high: String,
    },
    /// An enumeration's member names.
    Enum(Vec<String>),
    /// A set of an ordinal type.
    Set(Box<TypeExpr>),
    /// A typed file, or an untyped one.
    File(Option<Box<TypeExpr>>),
    /// A text file.
    Text,
    /// A short string of the given maximum length (`String[20]`), or of
    /// 255 (`ShortString`).
    ShortString(Option<u32>),
    /// A variant (`variant`).
    Variant,
    /// A floating-point type by its DCU kind (`float(fkExtended)` is
    /// `Extended`).
    Float(String),
    /// A class reference (`class of TFoo`).
    ClassRef(Box<TypeExpr>),
    /// A class, with its parent and the interfaces it implements in order.
    Class(Vec<TypeExpr>),
    /// An interface or dispinterface.
    Interface {
        /// Whether it is a dispinterface.
        dispatch: bool,
        /// Its parent interface.
        parent: Option<Box<TypeExpr>>,
        /// Its GUID, `{XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX}`.
        guid: Option<String>,
    },
    /// A procedural type.
    Procedure(Box<Signature>),
    /// No type: an untyped parameter (`const P:void `).
    Void,
    /// Text the notation does not cover, kept as stored.
    Unknown(String),
}

/// How a parameter is passed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Pass {
    /// By value (`val`).
    Value,
    /// As a constant (`const`).
    Const,
    /// By reference (`var`, or `out`).
    Var,
}

/// A parameter of a [`Signature`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Param {
    /// How it is passed.
    pub pass: Pass,
    /// Its name (`Self` and the hidden `_Dv_` included).
    pub name: String,
    /// Its type; [`TypeExpr::Void`] for an untyped parameter.
    pub ty: TypeExpr,
}

/// The parameters, result and calling convention of a routine or
/// procedural type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signature {
    /// The parameters, in order.
    pub params: Vec<Param>,
    /// A function's result type.
    pub result: Option<TypeExpr>,
    /// Whether a procedural type is a method pointer (`of object`).
    pub of_object: bool,
    /// The calling convention; `register` when the notation names none.
    pub call: CallKind,
}

/// What kind of routine a [`Prototype`] declares.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RoutineKind {
    /// `procedure`.
    Procedure,
    /// `function`.
    Function,
    /// `class procedure`.
    ClassProcedure,
    /// `class function`.
    ClassFunction,
    /// `constructor`.
    Constructor,
    /// `destructor`.
    Destructor,
    /// An interface's method (`method procedure`, `method function`).
    InterfaceMethod,
}

/// A method prototype of a knowledge-base type
/// ([`Method::prototype`](super::Method::prototype)).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prototype {
    /// What kind of routine it is.
    pub kind: RoutineKind,
    /// The method's name.
    pub name: String,
    /// Its parameters, result and calling convention.
    pub signature: Signature,
    /// The directives after it, in order (`override`, `virtual`,
    /// `overload`, `dynamic`, `abstract`).
    pub directives: Vec<String>,
}

/// Splits `text` at each top-level `sep`: one not inside brackets,
/// parentheses, angle brackets or a quoted string.
fn split_top(text: &str, sep: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut quoted = false;
    let mut start = 0;
    for (i, c) in text.char_indices() {
        match c {
            '\'' => quoted = !quoted,
            '(' | '[' | '<' if !quoted => depth = depth.saturating_add(1),
            ')' | ']' | '>' if !quoted => depth = depth.saturating_sub(1),
            c if c == sep && depth == 0 && !quoted => {
                parts.push(text.get(start..i).unwrap_or_default());
                start = i.saturating_add(c.len_utf8());
            }
            _ => {}
        }
    }
    parts.push(text.get(start..).unwrap_or_default());
    parts
}

/// Returns the position just past the bracket that closes the one `text`
/// opens with, or `None` when it is never closed.
fn closing(text: &str) -> Option<usize> {
    let mut depth = 0usize;
    let mut quoted = false;
    for (i, c) in text.char_indices() {
        match c {
            '\'' => quoted = !quoted,
            '(' | '[' | '<' if !quoted => depth = depth.saturating_add(1),
            ')' | ']' | '>' if !quoted => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(i.saturating_add(1));
                }
            }
            _ => {}
        }
    }
    None
}

/// Returns the position of the first top-level `..` in `text`.
fn top_range(text: &str) -> Option<usize> {
    let mut depth = 0usize;
    let mut quoted = false;
    let bytes = text.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'\'' => quoted = !quoted,
            b'(' | b'[' => depth = depth.saturating_add(1),
            b')' | b']' => depth = depth.saturating_sub(1),
            b'.' if !quoted && depth == 0 && bytes.get(i.saturating_add(1)) == Some(&b'.') => {
                return Some(i);
            }
            _ => {}
        }
    }
    None
}

impl TypeExpr {
    /// Reads a type expression (see the module docs).
    #[must_use]
    pub fn parse(text: &str) -> Self {
        let t = text.trim();
        if t.is_empty() || t == "void" {
            return if text.is_empty() {
                Self::Unknown(String::new())
            } else {
                Self::Void
            };
        }
        if let Some(rest) = t.strip_prefix('^') {
            return Self::Pointer(Box::new(Self::parse(rest)));
        }
        if let Some(rest) = t.strip_prefix("type ") {
            return Self::Copy(Box::new(Self::parse(rest)));
        }
        if let Some(rest) = t.strip_prefix("set of ") {
            return Self::Set(Box::new(Self::parse(rest)));
        }
        if let Some(rest) = t.strip_prefix("file of ") {
            return Self::File(Some(Box::new(Self::parse(rest))));
        }
        if let Some(rest) = t.strip_prefix("class of ") {
            return Self::ClassRef(Box::new(Self::parse(rest)));
        }
        match t {
            "file" => return Self::File(None),
            "text" => return Self::Text,
            "variant" => return Self::Variant,
            "ShortString" => return Self::ShortString(None),
            "class" => return Self::Class(Vec::new()),
            _ => {}
        }
        // IDR's maker prints a dynamic array without bounds; read it as the
        // open bounds `build` prints.
        if let Some(element) = t.strip_prefix("array of ") {
            return Self::Array {
                index: Box::new(Self::Range {
                    low: "$0".to_string(),
                    high: "-$1".to_string(),
                }),
                element: Box::new(Self::parse(element)),
            };
        }
        if let Some(rest) = t.strip_prefix("array") {
            return Self::array(rest).unwrap_or_else(|| Self::Unknown(t.to_string()));
        }
        if let Some(rest) = t.strip_prefix("class(") {
            return match rest.strip_suffix(')') {
                Some(inner) => {
                    Self::Class(split_top(inner, ',').into_iter().map(Self::parse).collect())
                }
                None => Self::Unknown(t.to_string()),
            };
        }
        if let Some(rest) = t.strip_prefix("dispinterface") {
            return Self::interface(rest, true);
        }
        if let Some(rest) = t.strip_prefix("interface") {
            return Self::interface(rest, false);
        }
        if let Some(rest) = t.strip_prefix("function") {
            return Self::Procedure(Box::new(Signature::parse_tail(rest)));
        }
        if let Some(rest) = t.strip_prefix("procedure") {
            return Self::Procedure(Box::new(Signature::parse_tail(rest)));
        }
        if let Some(kind) = t.strip_prefix("float(").and_then(|r| r.strip_suffix(')')) {
            return Self::Float(kind.strip_prefix("fk").unwrap_or(kind).to_string());
        }
        if let Some(len) = t
            .strip_prefix("String[")
            .and_then(|r| r.strip_suffix(']'))
            .and_then(|n| n.parse().ok())
        {
            return Self::ShortString(Some(len));
        }
        if t.starts_with('(') && closing(t) == Some(t.len()) {
            let inner = t.get(1..t.len().saturating_sub(1)).unwrap_or_default();
            return Self::Enum(
                split_top(inner, ',')
                    .into_iter()
                    .map(|m| m.trim().to_string())
                    .collect(),
            );
        }
        if let Some(at) = top_range(t) {
            return Self::Range {
                low: t.get(..at).unwrap_or_default().trim().to_string(),
                high: t
                    .get(at.saturating_add(2)..)
                    .unwrap_or_default()
                    .trim()
                    .to_string(),
            };
        }
        if let Some(name) = Self::generic(t) {
            return Self::Named(name);
        }
        if t.chars().all(|c| {
            c.is_alphanumeric() || matches!(c, '.' | '_' | '<' | '>' | ',' | '@' | '$' | '`')
        }) {
            return Self::Named(t.to_string());
        }
        Self::Unknown(t.to_string())
    }

    /// Reads a generic instance's name, qualified with its unit, from the
    /// two spellings knowledge bases use: IDR's maker prints the name as
    /// the source spells it followed by the qualified, arity-marked one
    /// (`TArray<System.TClass>System.TArray`1<System.TClass>`), `build` the
    /// unit in braces before it (`{System}TArray<System.TObject>`). Both
    /// read as `System.TArray<System.TClass>`.
    fn generic(t: &str) -> Option<String> {
        // `build` can also put the referring unit before the braces
        // (`System.Types.{System}TArray<System.Integer>`); the braces name
        // the unit that declares the generic.
        if let Some(open) = t
            .find('{')
            .filter(|&at| t.get(..at).is_none_or(|p| !p.contains('<')))
        {
            let (unit, name) = t.get(open.saturating_add(1)..)?.split_once('}')?;
            return Some(format!("{unit}.{name}"));
        }
        let open = t.find('<')?;
        let end = open.saturating_add(closing(t.get(open..)?)?);
        let qualified = t.get(end..).filter(|q| !q.is_empty())?;
        let tick = qualified.find('`')?;
        let digits = qualified
            .get(tick.saturating_add(1)..)?
            .find(|c: char| !c.is_ascii_digit())
            .map_or(qualified.len(), |n| {
                tick.saturating_add(1).saturating_add(n)
            });
        Some(format!(
            "{}{}",
            qualified.get(..tick)?,
            qualified.get(digits..)?
        ))
    }

    /// Reads what follows `array`: `[index] of element`.
    fn array(rest: &str) -> Option<Self> {
        let end = closing(rest).filter(|_| rest.starts_with('['))?;
        let index = rest.get(1..end.saturating_sub(1))?;
        let element = rest.get(end..)?.trim_start().strip_prefix("of ")?;
        Some(Self::Array {
            index: Box::new(Self::parse(index)),
            element: Box::new(Self::parse(element)),
        })
    }

    /// Reads what follows `interface` / `dispinterface`: ` (Parent)['{guid}']`.
    fn interface(rest: &str, dispatch: bool) -> Self {
        let mut rest = rest.trim_start();
        let mut parent = None;
        if rest.starts_with('(')
            && let Some(end) = closing(rest)
        {
            parent = rest
                .get(1..end.saturating_sub(1))
                .map(|p| Box::new(Self::parse(p)));
            rest = rest.get(end..).unwrap_or_default();
        }
        let guid = rest
            .trim()
            .strip_prefix("['")
            .and_then(|r| r.strip_suffix("']"))
            .map(str::to_string);
        Self::Interface {
            dispatch,
            parent,
            guid,
        }
    }

    /// Returns the name a [`TypeExpr::Named`] refers to.
    #[must_use]
    pub fn name(&self) -> Option<&str> {
        match self {
            Self::Named(name) => Some(name),
            _ => None,
        }
    }
}

impl Param {
    /// Reads a parameter: `val Sender:System.TObject`, `const P:void `.
    fn parse(text: &str) -> Option<Self> {
        let t = text.trim_start();
        let (word, rest) = t.split_once(' ')?;
        let pass = match word {
            "val" => Pass::Value,
            "const" => Pass::Const,
            "var" | "out" => Pass::Var,
            _ => return None,
        };
        let (name, ty) = rest.split_once(':')?;
        Some(Self {
            pass,
            name: name.trim().to_string(),
            ty: TypeExpr::parse(ty),
        })
    }
}

impl Signature {
    /// Reads what follows a routine's name or a procedural type's keyword:
    /// `(params):Result of object stdcall`, any part absent.
    fn parse_tail(text: &str) -> Self {
        let mut rest = text.trim();
        let mut call = CallKind::Register;
        for (word, kind) in [
            ("cdecl", CallKind::Cdecl),
            ("pascal", CallKind::Pascal),
            ("stdcall", CallKind::Stdcall),
            ("safecall", CallKind::Safecall),
        ] {
            if let Some(r) = rest.strip_suffix(word)
                && (r.is_empty() || r.ends_with(' ') || r.ends_with(')'))
            {
                call = kind;
                rest = r.trim_end();
                break;
            }
        }
        let of_object = match rest.strip_suffix(" of object") {
            Some(r) => {
                rest = r;
                true
            }
            None => false,
        };
        let mut params = Vec::new();
        if rest.starts_with('(')
            && let Some(end) = closing(rest)
        {
            let inner = rest.get(1..end.saturating_sub(1)).unwrap_or_default();
            params = split_top(inner, ';')
                .into_iter()
                .filter_map(Param::parse)
                .collect();
            rest = rest.get(end..).unwrap_or_default();
        }
        let result = rest
            .trim()
            .strip_prefix(':')
            .map(TypeExpr::parse)
            .filter(|r| *r != TypeExpr::Unknown("?".to_string()));
        Self {
            params,
            result,
            of_object,
            call,
        }
    }
}

impl Prototype {
    /// Reads a method prototype (see the module docs); `None` for one whose
    /// kind or name the notation does not give (`method _Nv__1E0:?`).
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let mut parts = split_top(text, ';').into_iter();
        let head = parts.next()?.trim();
        let directives = parts
            .map(|d| d.trim().to_string())
            .filter(|d| !d.is_empty())
            .collect();
        let (kind, rest) = [
            ("class function ", RoutineKind::ClassFunction),
            ("class procedure ", RoutineKind::ClassProcedure),
            ("method function ", RoutineKind::InterfaceMethod),
            ("method procedure ", RoutineKind::InterfaceMethod),
            ("constructor ", RoutineKind::Constructor),
            ("destructor ", RoutineKind::Destructor),
            ("function ", RoutineKind::Function),
            ("procedure ", RoutineKind::Procedure),
        ]
        .into_iter()
        .find_map(|(prefix, kind)| head.strip_prefix(prefix).map(|r| (kind, r)))?;
        let end = rest.find(['(', ':', ' ']).unwrap_or(rest.len());
        let name = rest.get(..end)?.to_string();
        if name.is_empty() {
            return None;
        }
        Some(Self {
            kind,
            name,
            signature: Signature::parse_tail(rest.get(end..).unwrap_or_default()),
            directives,
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn named(name: &str) -> TypeExpr {
        TypeExpr::Named(name.to_string())
    }

    #[test]
    fn type_expressions_read_back_as_printed() {
        assert_eq!(
            TypeExpr::parse("array[$0..-$1] of System.TVarRec"),
            TypeExpr::Array {
                index: Box::new(TypeExpr::Range {
                    low: "$0".into(),
                    high: "-$1".into()
                }),
                element: Box::new(named("System.TVarRec")),
            }
        );
        assert_eq!(
            TypeExpr::parse("^_TRUSTEE_A"),
            TypeExpr::Pointer(Box::new(named("_TRUSTEE_A")))
        );
        assert_eq!(
            TypeExpr::parse("type System.Cardinal"),
            TypeExpr::Copy(Box::new(named("System.Cardinal")))
        );
        assert_eq!(
            TypeExpr::parse("(alNone,alTop)"),
            TypeExpr::Enum(vec!["alNone".into(), "alTop".into()])
        );
        assert_eq!(
            TypeExpr::parse("class(ActnMan.TCustomActionControl)"),
            TypeExpr::Class(vec![named("ActnMan.TCustomActionControl")])
        );
        assert_eq!(
            TypeExpr::parse("String[20]"),
            TypeExpr::ShortString(Some(20))
        );
        assert_eq!(
            TypeExpr::parse("TArray<System.TClass>System.TArray`1<System.TClass>"),
            named("System.TArray<System.TClass>")
        );
        assert_eq!(
            TypeExpr::parse("{System.Generics.Collections}TList<System.TObject>"),
            named("System.Generics.Collections.TList<System.TObject>")
        );
        assert_eq!(
            TypeExpr::parse("System.Types.{System}TArray<System.Integer>"),
            named("System.TArray<System.Integer>")
        );
        assert!(matches!(
            TypeExpr::parse("array of System.Classes.TCollectionItem"),
            TypeExpr::Array { .. }
        ));
        assert_eq!(
            TypeExpr::parse("float(fkExtended)"),
            TypeExpr::Float("Extended".into())
        );
        assert_eq!(
            TypeExpr::parse("file of void "),
            TypeExpr::File(Some(Box::new(TypeExpr::Void)))
        );
        assert_eq!(
            TypeExpr::parse("set of $1..$8"),
            TypeExpr::Set(Box::new(TypeExpr::Range {
                low: "$1".into(),
                high: "$8".into()
            }))
        );
        assert_eq!(
            TypeExpr::parse(
                "dispinterface (System.IDispatch)['{00000512-0000-0010-8000-00AA006D2EA4}']"
            ),
            TypeExpr::Interface {
                dispatch: true,
                parent: Some(Box::new(named("System.IDispatch"))),
                guid: Some("{00000512-0000-0010-8000-00AA006D2EA4}".into()),
            }
        );
    }

    #[test]
    fn procedural_types_read_their_signature() {
        let TypeExpr::Procedure(sig) = TypeExpr::parse(
            "procedure(val Action:Classes.TBasicAction;var Handled:System.Boolean) of object",
        ) else {
            panic!("not a procedural type");
        };
        assert!(sig.of_object);
        assert_eq!(sig.call, CallKind::Register);
        assert_eq!(sig.params.len(), 2);
        assert_eq!(sig.params[1].pass, Pass::Var);
        assert_eq!(sig.params[1].ty, named("System.Boolean"));

        let TypeExpr::Procedure(sig) = TypeExpr::parse("function:System.HRESULT stdcall") else {
            panic!("not a procedural type");
        };
        assert!(sig.params.is_empty());
        assert_eq!(sig.result, Some(named("System.HRESULT")));
        assert_eq!(sig.call, CallKind::Stdcall);
    }

    #[test]
    fn prototypes_read_kind_name_signature_and_directives() {
        let p = Prototype::parse(
            "class function Create(val Self:class of CoConnection):Connection; override; virtual",
        )
        .unwrap();
        assert_eq!(p.kind, RoutineKind::ClassFunction);
        assert_eq!(p.name, "Create");
        assert_eq!(
            p.signature.params[0].ty,
            TypeExpr::ClassRef(Box::new(named("CoConnection")))
        );
        assert_eq!(p.signature.result, Some(named("Connection")));
        assert_eq!(p.directives, vec!["override", "virtual"]);

        let p = Prototype::parse(
            "method function AddRefHfont(val Self:IFont;val font:Windows.HFONT):System.HRESULT stdcall",
        )
        .unwrap();
        assert_eq!(p.kind, RoutineKind::InterfaceMethod);
        assert_eq!(p.signature.call, CallKind::Stdcall);
        assert_eq!(p.signature.result, Some(named("System.HRESULT")));

        let p = Prototype::parse("procedure Error(val Self:class of TList;const Msg:System.AnsiString;val Data:System.Integer);overload").unwrap();
        assert_eq!(p.signature.params[1].pass, Pass::Const);
        assert_eq!(p.directives, vec!["overload"]);

        assert_eq!(
            Prototype::parse("method _Nv__1E0:?; override; virtual"),
            None
        );
    }
}
