//! DCU and DCP decoding.
//!
//! The golden tests read units compiled from our own sources
//! (`tests/fixtures/bcb6/src/dcutypes.pas`, the `gtpkg` package) by the
//! Delphi 6 compiler; the samples are committed and required. Every
//! expectation is the compiler's: field offsets and sizes come from the
//! source's layout rules and the unit's own `SizeOf` constants, which the
//! compiler evaluated into the same DCU.
//!
//! The Delphi 13 golden tests read `tests/fixtures/d13/src/modern.pas` and
//! the same `dcutypes` sources compiled for Win32 and Win64 by Delphi 13
//! (`tests/samples/d13`, committed and required): the records DCU32INT
//! predates (documentation comments, type spellings, the PKX1 package).
//!
//! The library tests decode every DCU and every unit of every DCP shipped
//! with C++Builder 6 (Delphi 6), RAD Studio 10 Seattle and RAD Studio 13
//! when the environment names their installs (`UNDELPHI_BCB6`,
//! `UNDELPHI_BCB6_PACKAGES`, `UNDELPHI_SEATTLE`, `UNDELPHI_D13`; see
//! [`lib_dir`]), and skip otherwise: the toolchains are proprietary and
//! never committed.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::{
    collections::{HashMap, HashSet},
    env, fs,
    path::{Path, PathBuf},
};

use undelphi::{
    DelphiBinary,
    dcu::{
        AddrTarget, CallConv, ConstData, Decl, DeclKind, FloatKind, ImportKind, MethodKind,
        Modifier, Platform, RangeKind, TypeKind, TypeSpelling, TypeTarget, Unit, UsesSection,
        Version, library::Library, mangle::Mangler, package::Package,
    },
};

const SAMPLES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/samples/bcb6");

fn read_sample(name: &str) -> Vec<u8> {
    let path = Path::new(SAMPLES).join(name);
    fs::read(&path).unwrap_or_else(|e| panic!("{}: {e} (committed sample)", path.display()))
}

fn dcutypes() -> Unit {
    Unit::parse(&read_sample("dcutypes.dcu")).expect("dcutypes.dcu decodes")
}

/// Returns the top-level declaration called `name`.
fn decl<'u>(unit: &'u Unit, name: &str) -> &'u Decl {
    unit.decls_of(&unit.main)
        .find(|d| d.name == name)
        .unwrap_or_else(|| panic!("no declaration {name}"))
}

/// Returns the type a type declaration names.
fn named_type(unit: &Unit, name: &str) -> u32 {
    match decl(unit, name).kind {
        DeclKind::Type { type_index } => type_index,
        ref k => panic!("{name} is {k:?}"),
    }
}

/// Returns `(name, type name, offset)` for every field of a record or class.
fn fields(unit: &Unit, type_index: u32) -> Vec<(String, String, i32)> {
    let def = unit.type_def_at(type_index).expect("local type");
    let members = match &def.kind {
        TypeKind::Record { members, .. } => members,
        TypeKind::Class(c) => &c.members,
        k => panic!("not a structured type: {k:?}"),
    };
    unit.decls_of(members)
        .filter(|d| d.tag == 0x2C)
        .map(|d| match d.kind {
            DeclKind::Local(l) => (
                d.name.clone(),
                unit.type_name(l.type_index as u32)
                    .unwrap_or("")
                    .to_string(),
                l.location,
            ),
            ref k => panic!("field {} is {k:?}", d.name),
        })
        .collect()
}

/// Returns the compiler-evaluated integer constant `name`.
fn int_const(unit: &Unit, name: &str) -> i64 {
    match &decl(unit, name).kind {
        DeclKind::Const(v) => match v.data {
            ConstData::Int(i) => i,
            ConstData::Bytes(_) => panic!("{name} is not an integer"),
        },
        k => panic!("{name} is {k:?}"),
    }
}

fn s(v: &str) -> String {
    v.to_string()
}

#[test]
fn unit_header_and_uses() {
    let u = dcutypes();
    assert_eq!(u.version, Version::DELPHI_6);
    assert_eq!(u.platform, Platform::Win32);
    assert_eq!(u.name, "dcutypes");
    assert_eq!(
        u.sources.first().map(|f| f.name.as_str()),
        Some("dcutypes.pas")
    );
    let uses: Vec<(&str, UsesSection)> = u
        .uses
        .iter()
        .map(|x| (x.name.as_str(), x.section))
        .collect();
    assert_eq!(
        uses,
        [
            ("Classes", UsesSection::Interface),
            ("SysInit", UsesSection::Interface),
            ("System", UsesSection::Interface),
            ("kernel32.dll", UsesSection::Dll),
        ]
    );
    // The external routine's DLL name.
    let dll = u
        .uses
        .iter()
        .find(|x| x.section == UsesSection::Dll)
        .unwrap();
    assert_eq!(dll.symbols.len(), 1);
    assert_eq!(dll.symbols[0].name, "Beep");
    assert_eq!(dll.symbols[0].kind, ImportKind::DllRoutine);
    // Imported types occupy the first type slots, named by their unit.
    assert_eq!(u.type_name(1), Some("Byte"));
    assert_eq!(u.type_unit(1), Some("System"));
    assert!(matches!(
        u.type_slot(1).unwrap().target,
        TypeTarget::Import { .. }
    ));
}

#[test]
fn records_without_rtti_have_their_layout() {
    let u = dcutypes();
    let plain = named_type(&u, "TPlainRec");
    assert_eq!(
        fields(&u, plain),
        [
            (s("A"), s("Byte"), 0),
            (s("B"), s("Integer"), 4),
            (s("C"), s("Word"), 8),
            (s("D"), s("Double"), 16),
        ]
    );
    let def = u.type_def_at(plain).unwrap();
    assert_eq!(i64::from(def.size), int_const(&u, "SizePlain"));
    assert_eq!(def.rtti_size, 0, "no managed fields, no RTTI");

    let packed = named_type(&u, "TPackedRec");
    assert_eq!(
        fields(&u, packed),
        [
            (s("A"), s("Byte"), 0),
            (s("B"), s("Integer"), 1),
            (s("C"), s("Word"), 5)
        ]
    );
    assert_eq!(
        i64::from(u.type_def_at(packed).unwrap().size),
        int_const(&u, "SizePacked")
    );

    // Variant parts overlay at the same offset.
    let variant = named_type(&u, "TVariantRec");
    assert_eq!(
        fields(&u, variant),
        [
            (s("Tag"), s("Integer"), 0),
            (s("Kind"), s("Byte"), 4),
            (s("I"), s("Integer"), 8),
            (s("D"), s("Double"), 8),
            (s("P"), s("Pointer"), 8),
            (s("W"), s("Word"), 12),
        ]
    );
    assert_eq!(
        i64::from(u.type_def_at(variant).unwrap().size),
        int_const(&u, "SizeVariant")
    );

    let nested = named_type(&u, "TNested");
    let f = fields(&u, nested);
    assert_eq!((f[0].1.as_str(), f[0].2), ("TPlainRec", 0));
    assert_eq!((f[1].0.as_str(), f[1].2), ("Count", 24));
    assert_eq!((f[2].0.as_str(), f[2].2), ("Name", 28));
    assert_eq!(
        i64::from(u.type_def_at(nested).unwrap().size),
        int_const(&u, "SizeNested")
    );

    let floats = named_type(&u, "TFloats");
    let offsets: Vec<(String, i32)> = fields(&u, floats).into_iter().map(|f| (f.1, f.2)).collect();
    assert_eq!(
        offsets,
        [
            (s("Single"), 0),
            (s("Double"), 8),
            (s("Extended"), 16),
            (s("Currency"), 32),
            (s("Comp"), 40),
            (s("Real48"), 48),
        ]
    );
    assert_eq!(
        i64::from(u.type_def_at(floats).unwrap().size),
        int_const(&u, "SizeFloats")
    );
}

#[test]
fn simple_types() {
    let u = dcutypes();
    let kind = |name| &u.type_def_at(named_type(&u, name)).unwrap().kind;

    let TypeKind::Enum {
        low, high, members, ..
    } = kind("TColorKind")
    else {
        panic!("TColorKind")
    };
    assert_eq!((*low, *high), (0, 2));
    let names: Vec<&str> = u.decls_of(members).map(|d| d.name.as_str()).collect();
    assert_eq!(names, ["ckRed", "ckGreen", "ckBlue"]);

    let TypeKind::Set { element, .. } = kind("TColorSet") else {
        panic!("TColorSet")
    };
    assert_eq!(*element, named_type(&u, "TColorKind"));
    assert_eq!(
        i64::from(u.type_def_at(named_type(&u, "TColorSet")).unwrap().size),
        int_const(&u, "SizeColorSet")
    );

    assert!(matches!(
        kind("TSmall"),
        TypeKind::Range {
            kind: RangeKind::Integer,
            low: 10,
            high: 20,
            ..
        }
    ));
    assert!(matches!(
        kind("TLetters"),
        TypeKind::Range {
            kind: RangeKind::Char,
            low: 97,
            high: 122,
            ..
        }
    ));
    assert!(matches!(kind("TShort"), TypeKind::ShortString { .. }));
    assert_eq!(
        i64::from(u.type_def_at(named_type(&u, "TShort")).unwrap().size),
        int_const(&u, "SizeShort")
    );

    // array[0..2, 1..3] of Single: an array of arrays.
    let TypeKind::Array {
        index_type,
        element_type,
        ..
    } = kind("TMatrix")
    else {
        panic!("TMatrix")
    };
    assert!(matches!(
        u.type_def_at(*index_type).unwrap().kind,
        TypeKind::Range {
            low: 0,
            high: 2,
            ..
        }
    ));
    let TypeKind::Array {
        element_type: inner,
        ..
    } = u.type_def_at(*element_type).unwrap().kind
    else {
        panic!("inner array")
    };
    assert_eq!(u.type_name(inner), Some("Single"));
    assert_eq!(
        i64::from(u.type_def_at(named_type(&u, "TMatrix")).unwrap().size),
        int_const(&u, "SizeMatrix")
    );

    let TypeKind::Pointer { target } = kind("PPlainRec") else {
        panic!("PPlainRec")
    };
    assert_eq!(*target, named_type(&u, "TPlainRec"));

    // Delphi 6 has no dynamic-array record: `array of Integer` is a pointer
    // to an open array.
    let TypeKind::Pointer { target } = kind("TIntArray") else {
        panic!("TIntArray")
    };
    assert!(matches!(
        u.type_def_at(*target).unwrap().kind,
        TypeKind::Array { .. }
    ));
}

#[test]
fn aliases_name_the_aliased_slot() {
    let u = dcutypes();
    // `type TMyInt = Integer` names the import's slot, which keeps the
    // imported name; the same for a local type.
    let my_int = named_type(&u, "TMyInt");
    assert_eq!(u.type_name(my_int), Some("Integer"));
    assert!(matches!(
        u.type_slot(my_int).unwrap().target,
        TypeTarget::Import { .. }
    ));
    assert_eq!(named_type(&u, "TPlainAlias"), named_type(&u, "TPlainRec"));
    assert_eq!(
        u.type_name(named_type(&u, "TPlainAlias")),
        Some("TPlainRec")
    );
    // `type Integer` is a type copy: a second import of Integer that
    // carries its own RTTI, named by the declaration.
    let strong = named_type(&u, "TStrongInt");
    assert_eq!(u.type_name(strong), Some("TStrongInt"));
    let TypeTarget::Import { unit, symbol } = u.type_slot(strong).unwrap().target else {
        panic!("a type copy is an import")
    };
    let copied = &u.uses[unit].symbols[symbol];
    assert_eq!(
        (u.uses[unit].name.as_str(), copied.name.as_str()),
        ("System", "Integer")
    );
    assert!(matches!(copied.kind, ImportKind::TypeCopy { rtti_size } if rtti_size > 0));
}

#[test]
fn procedural_types() {
    let u = dcutypes();
    let TypeKind::ProcType {
        flags,
        result,
        call,
        params,
    } = &u.type_def_at(named_type(&u, "TCallback")).unwrap().kind
    else {
        panic!("TCallback")
    };
    assert_eq!(flags & 0x10, 0, "not of object");
    assert_eq!(u.type_name(*result), Some("Boolean"));
    assert_eq!(*call, CallConv::Register);
    let p: Vec<(&str, u8, u32)> = u
        .decls_of(params)
        .map(|d| match d.kind {
            DeclKind::Local(l) => (d.name.as_str(), d.tag, l.flags & 7),
            _ => panic!(),
        })
        .collect();
    // val A, const S (flag 0x02 clear), var R.
    assert_eq!(p, [("A", 0x21, 3), ("S", 0x21, 1), ("R", 0x22, 3)]);

    let TypeKind::ProcType { call, .. } =
        &u.type_def_at(named_type(&u, "TStdCallback")).unwrap().kind
    else {
        panic!()
    };
    assert_eq!(*call, CallConv::Stdcall);
    let TypeKind::ProcType { flags, .. } = &u
        .type_def_at(named_type(&u, "TMethodCallback"))
        .unwrap()
        .kind
    else {
        panic!()
    };
    assert_ne!(flags & 0x10, 0, "of object");
}

/// Returns the routine `name` with its parameters as `(name, tag, flags)`.
fn routine<'u>(
    u: &'u Unit,
    name: &str,
) -> (&'u undelphi::dcu::Proc, Vec<(String, u8, u32, String)>) {
    let DeclKind::Proc(p) = &decl(u, name).kind else {
        panic!("{name} is not a routine")
    };
    let params = u
        .decls_of(&p.params)
        .map(|d| match d.kind {
            DeclKind::Local(l) => (
                d.name.clone(),
                d.tag,
                l.flags,
                u.type_name(l.type_index as u32).unwrap_or("").to_string(),
            ),
            _ => panic!(),
        })
        .collect();
    (p, params)
}

#[test]
fn routine_signatures() {
    let u = dcutypes();

    let (p, params) = routine(&u, "Add");
    assert_eq!(u.type_name(p.result_type), Some("Integer"));
    assert_eq!(p.call, CallConv::Register);
    let regs: Vec<(String, u32)> = params.iter().map(|x| (x.0.clone(), x.2 & 0x08)).collect();
    assert_eq!(regs, [(s("A"), 8), (s("B"), 8)], "both in registers");

    let (p, params) = routine(&u, "StdCallFunc");
    assert_eq!(p.call, CallConv::Stdcall);
    assert_eq!(u.type_name(p.result_type), Some("LongBool"));
    assert_eq!(
        params.iter().map(|x| x.3.as_str()).collect::<Vec<_>>(),
        ["Integer", "Word"]
    );
    assert_eq!(routine(&u, "CdeclFunc").0.call, CallConv::Cdecl);
    assert_eq!(routine(&u, "PascalProc").0.call, CallConv::Pascal);
    assert_eq!(routine(&u, "SafeCallFunc").0.call, CallConv::Safecall);

    // var (0x22), out (var with flag 0x80), const (flag 0x02 clear).
    let (_, params) = routine(&u, "VarOut");
    let modes: Vec<(String, u8, u32, u32)> = params
        .iter()
        .map(|x| (x.0.clone(), x.1, x.2 & 0x80, x.2 & 7))
        .collect();
    assert_eq!(
        modes,
        [
            (s("A"), 0x22, 0, 3),
            (s("B"), 0x22, 0x80, 3),
            (s("C"), 0x22, 0, 1)
        ]
    );

    // An open array is a const array parameter plus its hidden High.
    let (_, params) = routine(&u, "OpenArray");
    assert_eq!(params.len(), 2);
    assert_eq!(params[1].0, ".");

    // A record result travels as a hidden result parameter.
    let (p, _) = routine(&u, "RecResult");
    assert_eq!(u.type_name(p.result_type), Some("TPlainRec"));

    // Overloads are separate routines with the same name.
    let overloads: Vec<String> = u
        .decls_of(&u.main)
        .filter(|d| d.name == "AddOverload")
        .map(|d| match &d.kind {
            DeclKind::Proc(p) => u.type_name(p.result_type).unwrap_or("").to_string(),
            _ => panic!(),
        })
        .collect();
    assert_eq!(overloads, ["Integer", "Double"]);

    // A nested routine is claimed by its host.
    let DeclKind::Proc(host) = &decl(&u, "NestedHost").kind else {
        panic!()
    };
    let inner: Vec<&str> = u
        .decls_of(&host.embedded)
        .map(|d| d.name.as_str())
        .collect();
    assert_eq!(inner, ["Inner"]);
}

#[test]
fn default_parameter_values() {
    let u = dcutypes();
    let (p, params) = routine(&u, "DefaultParams");
    assert_eq!(params.len(), 3);
    let defaults: Vec<(String, ConstData)> = u
        .decls_of(&p.locals)
        .filter_map(|d| match d.kind {
            DeclKind::SetDefault { constant, param } => {
                let value = match &u.decl_at(constant).unwrap().kind {
                    DeclKind::Const(v) => v.data.clone(),
                    _ => panic!(),
                };
                Some((u.decl_at(param).unwrap().name.clone(), value))
            }
            _ => None,
        })
        .collect();
    assert_eq!(defaults.len(), 2);
    assert_eq!(defaults[0], (s("B"), ConstData::Int(5)));
    assert_eq!(defaults[1].0, "S");
    // 'x' as an AnsiString literal: FF FF FF FF, length 1, 'x', NUL.
    assert_eq!(
        defaults[1].1,
        ConstData::Bytes(vec![0xFF, 0xFF, 0xFF, 0xFF, 1, 0, 0, 0, b'x', 0])
    );
}

#[test]
fn classes_with_private_members() {
    let u = dcutypes();
    let base = named_type(&u, "TBase");
    assert_eq!(
        fields(&u, base),
        [
            (s("FPriv"), s("Integer"), 4),
            (s("FName"), s("AnsiString"), 8),
            (s("FProt"), s("Word"), 12),
        ]
    );
    let TypeKind::Class(c) = &u.type_def_at(base).unwrap().kind else {
        panic!()
    };
    assert_eq!(c.instance_size, 16);
    assert_eq!(u.type_name(c.parent), Some("TObject"));
    assert_eq!(c.virtual_count, 2, "VirtProc and GetName");

    let methods: Vec<(&str, MethodKind, u32, i32)> = u
        .decls_of(&c.members)
        .filter_map(|d| match d.kind {
            DeclKind::Method(m) => {
                Some((d.name.as_str(), m.kind, m.local.flags, m.local.type_index))
            }
            _ => None,
        })
        .collect();
    // flags: 0x40 virtual, 0x80 dynamic, 0x20 override, 0x01 class,
    // visibility in 0x0E. A virtual method's type_index is its VMT slot
    // (Destroy overrides slot -1, VMT offset -4); a dynamic one's is its
    // dynamic index.
    assert_eq!(
        methods,
        [
            ("VirtProc", MethodKind::Method, 0x44, 0),
            ("DynProc", MethodKind::Method, 0x84, -1),
            ("GetName", MethodKind::Method, 0x44, 1),
            ("Create", MethodKind::Constructor, 0x02, -9),
            ("Destroy", MethodKind::Destructor, 0x62, -1),
            ("ClassFunc", MethodKind::Method, 0x03, -9),
        ]
    );
    // A method's location is the address of its implementation.
    let create = u
        .decls_of(&c.members)
        .find(|d| d.name == "Create")
        .and_then(|d| match d.kind {
            DeclKind::Method(m) => Some(m.local.location),
            _ => None,
        })
        .unwrap();
    assert_eq!(u.decl_at(create as u32).unwrap().name, "TBase.Create");

    let prop = u
        .decls_of(&c.members)
        .find_map(|d| match d.kind {
            DeclKind::Property(p) if d.name == "Name" => Some(p),
            _ => None,
        })
        .unwrap();
    assert_eq!(u.addr_name(prop.read), Some("FName"));
    assert_eq!(u.addr_name(prop.write), Some("FName"));

    let shape = named_type(&u, "TShape");
    let TypeKind::Class(c) = &u.type_def_at(shape).unwrap().kind else {
        panic!()
    };
    assert_eq!(c.instance_size, 48);
    assert_eq!(c.interfaces.len(), 1);
    assert_eq!(c.interfaces[0].0, named_type(&u, "IShape"));
    let f = fields(&u, shape);
    assert_eq!(f[2], (s("FOnChange"), s("TMethodCallback"), 32));
}

#[test]
fn interfaces() {
    let u = dcutypes();
    let TypeKind::Interface {
        guid,
        members,
        virtual_count,
        ..
    } = &u.type_def_at(named_type(&u, "IShape")).unwrap().kind
    else {
        panic!()
    };
    // {8E5B6D4A-1C2F-4E3A-9B7D-0A1B2C3D4E5F} in TGUID layout.
    assert_eq!(
        guid,
        &[
            0x4A, 0x6D, 0x5B, 0x8E, 0x2F, 0x1C, 0x3A, 0x4E, 0x9B, 0x7D, 0x0A, 0x1B, 0x2C, 0x3D,
            0x4E, 0x5F
        ]
    );
    assert_eq!(*virtual_count, 5, "IUnknown's three plus Area and Scale");
    // In an interface a method's type_index is its slot and its location
    // the type index of its procedural type.
    let m: Vec<(&str, i32, String)> = u
        .decls_of(members)
        .map(|d| match d.kind {
            DeclKind::Method(m) => {
                let TypeKind::ProcType { result, .. } =
                    u.type_def_at(m.local.location as u32).unwrap().kind
                else {
                    panic!()
                };
                (
                    d.name.as_str(),
                    m.local.type_index,
                    u.type_name(result).unwrap_or("").to_string(),
                )
            }
            _ => panic!(),
        })
        .collect();
    assert_eq!(m[0], ("Area", 3, s("Double")));
    assert_eq!(m[1].0, "Scale");
    assert_eq!(m[1].1, 4);
}

#[test]
fn constants_variables_and_resourcestrings() {
    let u = dcutypes();
    assert_eq!(int_const(&u, "CInt"), 42);
    assert_eq!(int_const(&u, "CEnum"), 1);
    // Delphi 6 types an exact decimal literal as Currency and stores it as
    // the Extended image of the value times 10 000: 3.5 is 35000.0.
    let DeclKind::Const(v) = &decl(&u, "CFloat").kind else {
        panic!()
    };
    assert!(matches!(
        u.type_def_at(v.type_index).unwrap().kind,
        TypeKind::Float(FloatKind::Currency)
    ));
    assert_eq!(
        v.data,
        ConstData::Bytes(vec![0, 0, 0, 0, 0, 0, 0xB8, 0x88, 0x0E, 0x40])
    );
    assert!(matches!(decl(&u, "CSet").kind, DeclKind::TypedConst(_)));
    assert!(matches!(
        decl(&u, "SMessage").kind,
        DeclKind::ResourceString(_)
    ));
    let DeclKind::Var(g) = decl(&u, "GVar").kind else {
        panic!()
    };
    assert_eq!(g.type_index, named_type(&u, "TPlainRec"));
    assert!(matches!(decl(&u, "TVar").kind, DeclKind::ThreadVar(_)));
    // Interface declarations carry the interface flag; the
    // implementation-only record does not.
    assert!(decl(&u, "TPlainRec").is_interface());
    assert!(!decl(&u, "TImplOnly").is_interface());
}

#[test]
fn deprecated_is_recorded() {
    let u = dcutypes();
    // Delphi 6 keeps deprecated / platform / library in the declaration
    // flags; bit 0 is deprecated.
    assert_eq!(decl(&u, "OldProc").add_info_flags & 1, 1);
    assert_eq!(decl(&u, "Add").add_info_flags, 0);
}

#[test]
fn address_table_resolves() {
    let u = dcutypes();
    // The used units come first, then their imported symbols.
    assert_eq!(u.addr(1), Some(AddrTarget::Unit(0)));
    assert_eq!(u.addr_name(1), Some("Classes"));
    // Every declaration's address points back at it.
    for (id, d) in u.decls.iter().enumerate() {
        if d.addr != 0 {
            assert_eq!(u.addr(d.addr), Some(AddrTarget::Decl(id)), "{}", d.name);
        }
    }
}

#[test]
fn package_container() {
    let data = read_sample("gtpkg.dcp");
    let pkg = Package::parse(&data, false).expect("gtpkg.dcp");
    assert_eq!(pkg.version, '7');
    let names: Vec<&str> = pkg.units.iter().map(|u| u.name.as_str()).collect();
    assert_eq!(names, ["gtshared", "gtpkg"]);
    let shared = Unit::parse_from_package(pkg.unit_data(&data, "gtshared").unwrap()).unwrap();
    assert_eq!(shared.name, "gtshared");
    assert_eq!(shared.version, Version::DELPHI_6);
    assert!(shared.from_package);
    // The package's own unit is also emitted on its own.
    let own = Unit::parse(&read_sample("gtpkg.dcu")).unwrap();
    assert_eq!(own.name, "gtpkg");
}

/// Decodes every `.dcu` and every unit of every `.dcp` under `dir`,
/// returning the number of units and the failures.
fn decode_library(dir: &Path) -> (usize, Vec<String>) {
    let mut count = 0;
    let mut failures = Vec::new();
    let mut entries: Vec<_> = fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .collect();
    entries.sort();
    for path in entries {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase);
        match ext.as_deref() {
            Some("dcu") => {
                count += 1;
                if let Err(e) = Unit::parse(&fs::read(&path).unwrap()) {
                    failures.push(format!("{}: {e}", path.display()));
                }
            }
            Some("dcp") => {
                let data = fs::read(&path).unwrap();
                let pkg = match Package::parse(&data, false) {
                    Ok(p) => p,
                    Err(e) => {
                        failures.push(format!("{}: {e}", path.display()));
                        continue;
                    }
                };
                for u in &pkg.units {
                    count += 1;
                    let bytes = pkg.unit_data(&data, &u.name).unwrap();
                    if let Err(e) = Unit::parse_from_package(bytes) {
                        failures.push(format!("{} [{}]: {e}", path.display(), u.name));
                    }
                }
            }
            _ => {}
        }
    }
    (count, failures)
}

/// The library directories to check (see [`lib_dir`]).
const LIBRARIES: &[&str] = &[
    "UNDELPHI_BCB6/Lib/Obj",
    "UNDELPHI_SEATTLE/lib/win32/release",
    "UNDELPHI_SEATTLE/lib/win32/debug",
    "UNDELPHI_SEATTLE/lib/win64/release",
    "UNDELPHI_SEATTLE/lib/win64/debug",
    "UNDELPHI_SEATTLE/lib/osx32/release",
    "UNDELPHI_SEATTLE/lib/iossimulator/release",
    "UNDELPHI_SEATTLE/lib/iosDevice32/release",
    "UNDELPHI_SEATTLE/lib/iosDevice64/release",
    "UNDELPHI_SEATTLE/lib/android/release",
    "UNDELPHI_D13/lib/win32/release",
    "UNDELPHI_D13/lib/win32/debug",
    "UNDELPHI_D13/lib/win64/release",
    "UNDELPHI_D13/lib/win64/debug",
    "UNDELPHI_D13/lib/win64x/release",
    "UNDELPHI_D13/lib/win64x/debug",
];

#[test]
fn every_shipped_unit_decodes() {
    for lib in LIBRARIES {
        let Some(dir) = lib_dir(lib) else {
            continue;
        };
        let (count, failures) = decode_library(&dir);
        assert!(count > 0, "{lib}: no units");
        assert!(
            failures.is_empty(),
            "{lib}: {} of {count} failed:\n{}",
            failures.len(),
            failures.join("\n")
        );
    }
}

#[test]
fn seattle_units_report_their_platform() {
    let Some(root) = lib_dir("UNDELPHI_SEATTLE/lib") else {
        return;
    };
    for (dir, platform) in [("win32", Platform::Win32), ("win64", Platform::Win64)] {
        let path = root.join(dir).join("release/System.Types.dcu");
        let Ok(data) = fs::read(&path) else {
            eprintln!("skipping {}: not present", path.display());
            continue;
        };
        let u = Unit::parse(&data).unwrap();
        assert_eq!(u.version, Version::DELPHI_10_SEATTLE);
        assert_eq!(u.platform, platform);
        assert_eq!(u.name, "System.Types");
        // TPoint has no RTTI-visible layout before Delphi 2010 binaries;
        // the DCU always has it.
        let point = u
            .decls_of(&u.main)
            .find_map(|d| match d.kind {
                DeclKind::Type { type_index } if d.name == "TPoint" => Some(type_index),
                _ => None,
            })
            .unwrap();
        let def = u.type_def_at(point).unwrap();
        assert_eq!(def.size, 8);
        let TypeKind::Record { members, .. } = &def.kind else {
            panic!()
        };
        let f: Vec<(&str, i32)> = u
            .decls_of(members)
            .filter_map(|d| match d.kind {
                DeclKind::Local(l) if d.tag == 0x2C => Some((d.name.as_str(), l.location)),
                _ => None,
            })
            .collect();
        assert_eq!(f, [("X", 0), ("Y", 4)]);

        // TStringList: 92 / 152 bytes in the VMT, the declared fields plus
        // the hidden monitor slot.
        let classes =
            Unit::parse(&fs::read(root.join(dir).join("release/System.Classes.dcu")).unwrap())
                .unwrap();
        let list = classes
            .decls_of(&classes.main)
            .find_map(|d| match d.kind {
                DeclKind::Type { type_index } if d.name == "TStringList" => Some(type_index),
                _ => None,
            })
            .unwrap();
        let TypeKind::Class(c) = &classes.type_def_at(list).unwrap().kind else {
            panic!()
        };
        let expected = if platform == Platform::Win64 { 152 } else { 92 };
        assert_eq!(classes.instance_size(c), expected);
        assert_eq!(
            c.instance_size + i32::from(classes.pointer_size()),
            expected
        );
    }
}

#[test]
fn unit_names_are_the_declared_ones() {
    // Delphi 6 ships lower-case source names (`classes.pas`); the unit's
    // initialization routine carries the declared spelling.
    let Some(lib) = lib_dir("UNDELPHI_BCB6/Lib/Obj") else {
        return;
    };
    for (file, declared) in [
        ("classes.dcu", "Classes"),
        ("sysutils.dcu", "SysUtils"),
        ("types.dcu", "Types"),
    ] {
        let Ok(data) = fs::read(lib.join(file)) else {
            eprintln!("skipping {file}: not present");
            continue;
        };
        let u = Unit::parse(&data).unwrap();
        assert_eq!(u.name, declared);
        assert_eq!(
            u.sources[0].name.to_ascii_lowercase(),
            format!("{}.pas", declared.to_ascii_lowercase())
        );
    }
}

/// A directory of a compiler install, given as `VARIABLE/path`: the install
/// directory the environment variable names (`UNDELPHI_BCB6` for
/// C++Builder 6, `UNDELPHI_BCB6_PACKAGES` for the folder holding its runtime
/// packages, `UNDELPHI_SEATTLE` for RAD Studio 10 Seattle, `UNDELPHI_D13` for
/// RAD Studio 13) joined with the path in the install's standard layout.
/// `None`, and the test skips, when the variable is unset or the directory
/// is missing.
fn lib_dir(spec: &str) -> Option<PathBuf> {
    let (var, rel) = spec.split_once('/').unwrap_or((spec, ""));
    let dir = env::var_os(var)
        .map(|root| Path::new(&root).join(rel))
        .filter(|d| d.is_dir());
    if dir.is_none() {
        eprintln!("skipping: {spec} not present (set {var})");
    }
    dir
}

#[test]
fn every_fixture_routine_gets_its_exported_name() {
    // dcutypespkg.bpl exports every routine of dcutypes under its mangled
    // name; resolving System's types needs the Delphi 6 library.
    let Some(d6) = lib_dir("UNDELPHI_BCB6/Lib/Obj") else {
        return;
    };
    let samples = Path::new(SAMPLES);
    let lib = Library::open_dirs(&[samples, &d6]).unwrap();
    let mangler = Mangler::new(&lib);
    let data = read_sample("dcutypespkg.bpl");
    let bin = DelphiBinary::parse(&data).unwrap();
    let exports: HashSet<String> = bin
        .exports()
        .into_iter()
        .filter(|(n, _)| {
            (n.contains("$q") || n.contains("$Q"))
                && (n.starts_with("@Dcutypes@") || n.starts_with("@DCUTYPES@"))
        })
        .map(|(n, _)| n.to_string())
        .collect();
    let unit = lib.unit("dcutypes").unwrap();
    let mut generated = HashSet::new();
    for d in unit.decls_of(&unit.main) {
        if let Some(name) = mangler.routine(&unit, d) {
            generated.insert(name);
        }
    }
    // Every exported routine is generated exactly (the external Beep import
    // is not exported, so nothing extra is required of it).
    let missing: Vec<&String> = exports.iter().filter(|e| !generated.contains(*e)).collect();
    assert!(
        missing.is_empty(),
        "not generated: {missing:?}\ngenerated: {generated:?}"
    );
    for name in [
        "@Dcutypes@AddOverload$qqri",
        "@Dcutypes@AddOverload$qqrd",
        "@Dcutypes@VarOut$qqrrit1rx18Dcutypes@TPlainRec",
        "@Dcutypes@OpenArray$qqrpxixi",
        "@Dcutypes@SafeCallFunc$qqsiri",
        "@DCUTYPES@PASCALPROC$QII",
        "@Dcutypes@TBase@$bctr$qqrx17System@AnsiString",
        "@Dcutypes@TBase@ClassFunc$qqrp17System@TMetaClassi",
        "@Dcutypes@initialization$qqrv",
    ] {
        assert!(generated.contains(name), "{name}");
    }
}

/// Generates the export name of every routine of every unit each package
/// contains and returns `(matched, exported functions)`.
fn mangle_coverage(bpls: &Path, lib: &Path) -> (usize, usize) {
    let lib = Library::open(lib).unwrap();
    let mangler = Mangler::new(&lib);
    let (mut matched, mut total) = (0, 0);
    let mut paths: Vec<_> = fs::read_dir(bpls)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .collect();
    paths.retain(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("bpl")));
    for path in paths {
        let data = fs::read(&path).unwrap();
        let Ok(bin) = DelphiBinary::parse(&data) else {
            continue;
        };
        let exports: HashSet<String> = bin
            .exports()
            .into_iter()
            .filter(|(n, va)| {
                bin.import_at(*va).is_none()
                    && (n.contains("$q") || (n.starts_with("_Z") && !n.ends_with('E')))
            })
            .map(|(n, _)| n.to_string())
            .collect();
        total += exports.len();
        let mut hit = HashSet::new();
        for u in bin
            .package_info()
            .map(|i| i.contains.clone())
            .unwrap_or_default()
        {
            let Some(unit) = lib.unit(u.name) else {
                continue;
            };
            for d in unit.decls_of(&unit.main) {
                if let Some(m) = mangler.routine(&unit, d)
                    && exports.contains(&m)
                {
                    hit.insert(m);
                }
            }
        }
        matched += hit.len();
    }
    (matched, total)
}

#[test]
fn export_names_match_the_runtime_packages() {
    // Floors from the measured coverage: the exports
    // not matched belong mostly to units without DCUs (C++Builder's units
    // compiled from C++, the linker's package entry points) and to a few
    // generic instances.
    for (bpls, lib, floor) in [
        ("UNDELPHI_BCB6_PACKAGES", "UNDELPHI_BCB6/Lib/Obj", 28_000),
        (
            "UNDELPHI_SEATTLE/Redist/win32",
            "UNDELPHI_SEATTLE/lib/win32/release",
            155_000,
        ),
        (
            "UNDELPHI_SEATTLE/Redist/win64",
            "UNDELPHI_SEATTLE/lib/win64/release",
            108_000,
        ),
        (
            "UNDELPHI_D13/Redist/win32",
            "UNDELPHI_D13/lib/win32/release",
            161_000,
        ),
        (
            "UNDELPHI_D13/Redist/win64",
            "UNDELPHI_D13/lib/win64/release",
            117_700,
        ),
    ] {
        let (Some(b), Some(l)) = (lib_dir(bpls), lib_dir(lib)) else {
            continue;
        };
        let (matched, total) = mangle_coverage(&b, &l);
        assert!(
            matched >= floor,
            "{bpls}: {matched} of {total} matched, expected at least {floor}"
        );
    }
}

// ----- Delphi 13 -----

const D13: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/samples/d13");

fn read_d13(platform: &str, name: &str) -> Vec<u8> {
    let path = Path::new(D13).join(platform).join(name);
    fs::read(&path).unwrap_or_else(|e| panic!("{}: {e} (committed sample)", path.display()))
}

fn modern(platform: &str) -> Unit {
    Unit::parse(&read_d13(platform, "modern.dcu")).expect("modern.dcu decodes")
}

/// Returns the declared-type spellings a declaration carries.
fn declared_type(d: &Decl) -> (&TypeSpelling, &TypeSpelling) {
    d.modifiers
        .iter()
        .find_map(|m| match m {
            Modifier::DeclaredType { value, result } => Some((value, result)),
            _ => None,
        })
        .unwrap_or_else(|| panic!("{} has no declared type", d.name))
}

#[test]
fn delphi13_fixtures_decode_on_both_platforms() {
    for (platform, expected) in [("win32", Platform::Win32), ("win64", Platform::Win64)] {
        for name in ["modern.dcu", "dcutypes.dcu"] {
            let u = Unit::parse(&read_d13(platform, name)).unwrap();
            assert_eq!(u.version, Version::DELPHI_13_FLORENCE, "{platform}/{name}");
            assert_eq!(u.version.compiler_version(), Some(37));
            assert_eq!(u.version.product_name(), "Delphi 13 Florence");
            assert_eq!(u.platform, expected);
            assert_eq!(u.image_base, None, "a standalone unit has no image base");
        }
        // The cross-version control: the Delphi 6 fixture's layout is the
        // same under Delphi 13 (TPlainRec: Byte, Integer, Word, Double).
        let u = Unit::parse(&read_d13(platform, "dcutypes.dcu")).unwrap();
        assert_eq!(
            fields(&u, named_type(&u, "TPlainRec")),
            [
                (s("A"), s("Byte"), 0),
                (s("B"), s("Integer"), 4),
                (s("C"), s("Word"), 8),
                (s("D"), s("Double"), 16),
            ]
        );
    }
}

#[test]
fn delphi13_documentation_comments_attach_to_their_declarations() {
    let u = modern("win32");
    let docs = |name: &str| -> Vec<String> {
        decl(&u, name)
            .modifiers
            .iter()
            .filter_map(|m| match m {
                Modifier::Documentation(line) => Some(line.clone()),
                _ => None,
            })
            .collect()
    };
    assert_eq!(
        docs("TCount"),
        [" <summary>An alias the compiler resolves to Integer.</summary>"]
    );
    assert_eq!(
        docs("AddCounts"),
        [
            " <summary>Adds two counts.</summary>",
            " <param name=\"A\">The first count.</param>",
        ]
    );
    assert!(docs("GCount").is_empty());
}

#[test]
fn delphi13_records_how_the_source_spells_types() {
    for platform in ["win32", "win64"] {
        let u = modern(platform);
        let count = decl(&u, "TCount");
        // `GCount: TCount` resolves to Integer; the spelling is TCount.
        let g = decl(&u, "GCount");
        let DeclKind::Var(v) = &g.kind else { panic!() };
        assert_eq!(u.type_name(v.type_index), Some("Integer"));
        assert_eq!(declared_type(g).0.address, Some(count.addr));
        // An alias of a generic instance is spelled by name.
        let (value, _) = declared_type(decl(&u, "GBytes"));
        assert_eq!(value.name.as_deref(), Some("TBytes"));
        assert_eq!(value.unit.as_deref(), Some("System.SysUtils"));
        // A routine's result and parameters.
        let add = decl(&u, "AddCounts");
        assert_eq!(declared_type(add).1.address, Some(count.addr));
        let DeclKind::Proc(p) = &add.kind else {
            panic!()
        };
        for param in u.decls_of(&p.params) {
            assert_eq!(declared_type(param).0.address, Some(count.addr));
        }
        // A spelled-out instance argument: TCountBox = TBox<TCount>.
        let def = u.type_def_at(named_type(&u, "TCountBox")).unwrap();
        let TypeKind::TemplateCall {
            args, spellings, ..
        } = &def.kind
        else {
            panic!("{:?}", def.kind)
        };
        assert_eq!(u.type_name(args[0]), Some("Integer"));
        assert_eq!(spellings[0].address, Some(count.addr));
    }
}

#[test]
fn delphi13_win64_packages_carry_their_image_base() {
    for (platform, base) in [("win32", None), ("win64", Some(0x0040_0000_u64))] {
        let data = read_d13(platform, "modernpkg.dcp");
        let pkg = Package::parse(&data, false).unwrap();
        let unit = Unit::parse_from_package(pkg.unit_data(&data, "modern").unwrap()).unwrap();
        assert_eq!(unit.name, "modern");
        assert_eq!(unit.image_base, base, "{platform}");
    }
}

#[test]
fn delphi13_fixture_routines_get_their_exported_names() {
    // The packages export every routine of their unit under its mangled
    // name: Borland on Win32 (one shortened with its MD5 suffix), Itanium
    // on Win64. System's types come from the Delphi 13 library.
    for platform in ["win32", "win64"] {
        let Some(lib) = lib_dir(&format!("UNDELPHI_D13/lib/{platform}/release")) else {
            continue;
        };
        let samples = Path::new(D13).join(platform);
        let lib = Library::open_dirs(&[&samples, &lib]).unwrap();
        let mangler = Mangler::new(&lib);
        for (bpl, unit, prefixes) in [
            ("modernpkg.bpl", "modern", ["@Modern@", "_ZN6Modern"]),
            (
                "dcutypespkg.bpl",
                "dcutypes",
                ["@Dcutypes@", "_ZN8Dcutypes"],
            ),
        ] {
            let data = read_d13(platform, bpl);
            let bin = DelphiBinary::parse(&data).unwrap();
            let exports: HashSet<String> = bin
                .exports()
                .into_iter()
                .filter(|(n, _)| {
                    prefixes.iter().any(|p| n.starts_with(p))
                        && (n.contains("$q") || (n.starts_with("_Z") && !n.ends_with('E')))
                })
                .map(|(n, _)| n.to_string())
                .collect();
            assert!(!exports.is_empty(), "{platform}/{bpl}");
            if platform == "win32" && unit == "modern" {
                // The long routine's name is past the 252-character limit.
                assert!(exports.iter().any(|e| e.len() == 253), "no shortened name");
            }
            let u = lib.unit(unit).unwrap();
            let generated: HashSet<String> = u
                .decls_of(&u.main)
                .filter_map(|d| mangler.routine(&u, d))
                .collect();
            let mut missing: Vec<&String> =
                exports.iter().filter(|e| !generated.contains(*e)).collect();
            missing.sort();
            assert!(
                missing.is_empty(),
                "{platform}/{bpl}: not generated: {missing:#?}"
            );
        }
    }
}

/// Reads the code publics of a dcc map: symbol -> addresses (an overloaded
/// routine is listed once per overload).
fn map_code_publics(path: &Path) -> HashMap<String, Vec<u64>> {
    let text = String::from_utf8_lossy(&fs::read(path).unwrap()).replace('\r', "");
    let mut bases = HashMap::new();
    for line in text.lines() {
        if line.contains("Detailed map of segments") {
            break;
        }
        let p: Vec<&str> = line.split_whitespace().collect();
        if p.len() >= 4 && matches!(p[3], "CODE" | "ICODE") && p[0].len() > 5 {
            let base = u64::from_str_radix(&p[0][5..], 16).unwrap();
            bases.insert(
                p[0][..4].to_string(),
                if base == 0 { 0x0040_1000 } else { base },
            );
        }
    }
    let mut out: HashMap<String, Vec<u64>> = HashMap::new();
    let mut publics = false;
    for line in text.lines() {
        if line.contains("Publics by Name") {
            publics = true;
            continue;
        }
        if line.contains("Publics by Value") {
            break;
        }
        let Some((addr, symbol)) = line.trim().split_once(char::is_whitespace) else {
            continue;
        };
        if !publics || addr.len() < 6 {
            continue;
        }
        if let Some(base) = bases.get(&addr[..4]) {
            out.entry(symbol.trim().to_string())
                .or_default()
                .push(base + u64::from_str_radix(&addr[5..], 16).unwrap());
        }
    }
    out
}

#[test]
fn routine_code_matches_the_linked_binary() {
    // Every routine of these units that the program links (and whose name
    // is unique, so the map's address is unambiguous) has exactly the bytes
    // its DCU part holds, relocations masked: the memory map and the fixup
    // sizes are right. Delphi 6's map names routines without their unit.
    let samples = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/samples");
    for (exe, lib, qualified, units, floor) in [
        (
            "bcb6/known.exe",
            "UNDELPHI_BCB6/Lib/Obj",
            false,
            &["System", "Classes", "TypInfo", "SysInit"][..],
            600,
        ),
        (
            "d13/win32/known.exe",
            "UNDELPHI_D13/lib/win32/release",
            true,
            &["System", "System.SysUtils", "System.Classes", "System.Rtti"][..],
            3_500,
        ),
        (
            "d13/win64/known.exe",
            "UNDELPHI_D13/lib/win64/release",
            true,
            &["System", "System.SysUtils", "System.Classes", "System.Rtti"][..],
            3_500,
        ),
    ] {
        let Some(lib_path) = lib_dir(lib) else {
            continue;
        };
        let library = Library::open(&lib_path).unwrap();
        let data = fs::read(samples.join(exe)).unwrap();
        let bin = DelphiBinary::parse(&data).unwrap();
        let ctx = bin.ctx();
        let map = map_code_publics(&samples.join(exe.replace(".exe", ".map")));
        let mut checked = 0;
        for name in units {
            let u = library.unit(name).unwrap();
            let mut counts: HashMap<&str, usize> = HashMap::new();
            for d in &u.decls {
                if matches!(d.kind, DeclKind::Proc(_)) {
                    *counts.entry(d.name.as_str()).or_default() += 1;
                }
            }
            for part in u.memory() {
                let d = &u.decls[part.decl];
                if !matches!(d.kind, DeclKind::Proc(_)) || counts[d.name.as_str()] > 1 {
                    continue;
                }
                let key = if qualified {
                    format!("{}.{}", u.name, d.name)
                } else {
                    d.name.clone()
                };
                let Some([va]) = map.get(&key).map(Vec::as_slice) else {
                    continue;
                };
                let off = ctx.va_to_file(*va).unwrap();
                let code = &u.block[part.range.clone()];
                assert!(data.len() >= off + code.len(), "{key} runs past the image");
                let mut masked = vec![false; code.len()];
                for f in &u.fixups {
                    let at = f.offset as usize;
                    if let Some(size) = u.fixup_size(f)
                        && part.range.contains(&at)
                    {
                        let rel = at - part.range.start;
                        for m in masked.iter_mut().skip(rel).take(size) {
                            *m = true;
                        }
                    }
                }
                let same = code
                    .iter()
                    .zip(&data[off..])
                    .zip(&masked)
                    .all(|((a, b), m)| *m || a == b);
                // Delphi 6's unqualified map can name a Windows API thunk
                // like a SysUtils routine (FindClose); skip such thunks.
                if !qualified && data[off] == 0xFF && data[off + 1] == 0x25 {
                    continue;
                }
                assert!(same, "{exe}: {key} at {va:#x} differs from its DCU code");
                checked += 1;
            }
        }
        assert!(checked >= floor, "{exe}: only {checked} routines checked");
    }
}

#[test]
fn a_type_index_beyond_the_image_is_rejected() {
    // A type declaration's index re-encoded as the 5-byte form: with its
    // own value the unit still decodes (so the edit hits the index), with
    // 0x10000000 it is refused instead of growing the type table to that
    // size.
    let data = read_sample("dcutypes.dcu");
    let unit = Unit::parse(&data).unwrap();
    let reencode = |at: usize, value: u32| {
        let mut out = data.clone();
        let mut bytes = vec![0x0F];
        bytes.extend_from_slice(&value.to_le_bytes());
        out.splice(at..at + 1, bytes);
        let size = u32::try_from(out.len()).unwrap();
        out[4..8].copy_from_slice(&size.to_le_bytes());
        out
    };
    let (decl, at) = unit
        .decls_of(&unit.main)
        .filter(|d| d.tag == 0x2A)
        .find_map(|d| {
            let DeclKind::Type { type_index } = d.kind else {
                return None;
            };
            let start = d.offset + 2 + d.name.len();
            let encoded = u8::try_from(type_index << 1).ok()?;
            (start..start + 12)
                .find(|&p| data[p] == encoded && Unit::parse(&reencode(p, type_index)).is_ok())
                .map(|p| (d.name.clone(), p))
        })
        .expect("a type declaration with a one-byte index");
    let err = Unit::parse(&reencode(at, 0x1000_0000)).unwrap_err();
    assert!(
        err.to_string().contains("type index out of range"),
        "{decl}: {err}"
    );
}
