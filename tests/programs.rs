//! Golden tests against our own compiled fixture programs.
//!
//! The same Pascal sources (`tests/fixtures/bcb6/src/`) are compiled by
//! Delphi 6 (`tests/samples/bcb6/`, built by `tests/fixtures/bcb6/build.sh`)
//! and by Delphi 13 for Win32 and Win64 (`tests/samples/d13/`, built by
//! `tests/fixtures/d13/build.sh`), so every type, member and form value is
//! known exactly, and each assertion runs over every build that has the
//! program. Where the compilers legitimately differ (the RTL's own type
//! names and sizes, the VMT header, what the linker pulls in) the expected
//! value is stated per build. Code addresses come from each build's linker
//! map, never from a previous run. The C++Builder programs exist only as
//! C++Builder 6 builds.
//!
//! The samples are committed: a missing sample fails the test rather than
//! skipping it. The tests never compile anything; if a fixture source
//! changes, rebuild locally and update these assertions in lockstep.

#![allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::unwrap_used,
    clippy::arithmetic_side_effects
)]

use std::{
    collections::{HashMap, HashSet},
    fs,
    path::PathBuf,
};

use undelphi::{
    DelphiBinary,
    classes::Class,
    detection::{Compiler, DetectionSource, TargetArch},
    dfm::{DfmObject, DfmValue},
    dvclal::Edition,
    entrypoints::EntrypointKind,
    imports::{BorlandSymbol, SymbolKind},
    interfaces::ThunkTarget,
    packageinfo::{ModuleType, Producer},
    properties::{AccessKind, AccessTarget},
    rtti::{TypeDetail, TypeKind},
};

/// A compiler and target the fixture programs were built with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Build {
    /// C++Builder 6's Delphi 6 compiler (`dcc32` 14.0).
    Delphi6,
    /// Delphi 13 Florence, `dcc32` 37.0.
    Delphi13Win32,
    /// Delphi 13 Florence, `dcc64` 37.0.
    Delphi13Win64,
}

/// Every build of the Pascal fixture programs.
const BUILDS: [Build; 3] = [Build::Delphi6, Build::Delphi13Win32, Build::Delphi13Win64];

impl Build {
    /// The sample directory, relative to the crate root.
    fn dir(self) -> &'static str {
        match self {
            Self::Delphi6 => "tests/samples/bcb6",
            Self::Delphi13Win32 => "tests/samples/d13/win32",
            Self::Delphi13Win64 => "tests/samples/d13/win64",
        }
    }

    fn path(self, name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join(self.dir())
            .join(name)
    }

    fn sample(self, name: &str) -> Vec<u8> {
        fs::read(self.path(name))
            .unwrap_or_else(|e| panic!("{}/{name}: {e} (committed sample)", self.dir()))
    }

    /// Whether the build comes from a Delphi 2009+ compiler (Unicode RTL,
    /// extended VMT header, extended RTTI).
    fn modern(self) -> bool {
        self != Self::Delphi6
    }

    /// The pointer size in bytes.
    fn ptr(self) -> u64 {
        if self == Self::Delphi13Win64 { 8 } else { 4 }
    }

    /// How the RTL names the `string` type in RTTI.
    fn string(self) -> &'static str {
        if self.modern() { "string" } else { "String" }
    }

    /// Code publics of a map: symbol -> VA (the last of overloads).
    fn map(self, name: &str, image_base: u64) -> HashMap<String, u64> {
        self.map_entries(name, image_base).into_iter().collect()
    }

    /// Every `(symbol, VA)` of a map's code publics, overloads included.
    ///
    /// The segment table gives each code segment's base (`.text`, and from
    /// Delphi 2010 the unit-initialization `.itext`); Delphi 6 writes a zero
    /// base for CODE, which starts at image base + 0x1000. Delphi 13 names
    /// symbols with their unit (`known.TButton.Click`); each is also listed
    /// without it, as Delphi 6 names them.
    fn map_entries(self, name: &str, image_base: u64) -> Vec<(String, u64)> {
        let text = String::from_utf8_lossy(&self.sample(name)).replace('\r', "");
        let mut bases: HashMap<String, u64> = HashMap::new();
        for line in text.lines().take(12) {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 4
                && matches!(parts[3], "CODE" | "ICODE")
                && parts[0].len() > 5
                && parts[0].as_bytes()[4] == b':'
            {
                let mut base = u64::from_str_radix(&parts[0][5..], 16).unwrap();
                if base == 0 {
                    base = image_base + 0x1000;
                }
                bases.insert(parts[0][..4].to_string(), base);
            }
        }
        let mut out = Vec::new();
        let mut in_publics = false;
        for line in text.lines() {
            if line.contains("Publics by Name") {
                in_publics = true;
                continue;
            }
            if line.contains("Publics by Value") {
                break;
            }
            let line = line.trim();
            if !in_publics || line.len() < 6 {
                continue;
            }
            let Some(base) = bases.get(&line[..4]) else {
                continue;
            };
            let (addr, symbol) = line.split_once(char::is_whitespace).unwrap();
            let va = base + u64::from_str_radix(&addr[5..], 16).unwrap();
            let symbol = symbol.trim().to_string();
            if self.modern()
                && let Some((_, rest)) = symbol.split_once('.')
                && !rest.is_empty()
                && !rest.starts_with('.')
            {
                out.push((rest.to_string(), va));
            }
            out.push((symbol, va));
        }
        out
    }
}

/// `(dispid, name, flags, result type, parameter types, address)` of an
/// automation-table entry.
type AutoRow<'a> = (i32, &'a str, u32, u8, Vec<u8>, u64);

fn class<'b, 'a>(bin: &'b DelphiBinary<'a>, name: &str) -> &'b Class<'a> {
    bin.classes()
        .find_by_name(name)
        .unwrap_or_else(|| panic!("class {name}"))
}

fn find_type<'a>(bin: &DelphiBinary<'a>, name: &str) -> TypeDetail<'a> {
    bin.types()
        .into_iter()
        .find(|t| t.header().name() == name)
        .unwrap_or_else(|| panic!("type {name}"))
}

/// `(property name, resolved type name)` for every published property.
fn prop_types(bin: &DelphiBinary<'_>, class_name: &str) -> Vec<(String, Option<String>)> {
    let c = class(bin, class_name);
    bin.properties_with_types(c)
        .iter()
        .map(|p| {
            (
                p.property.name().to_string(),
                p.ty.map(|h| h.name().to_string()),
            )
        })
        .collect()
}

fn pairs(list: &[(&str, &str)]) -> Vec<(String, Option<String>)> {
    list.iter()
        .map(|(n, t)| ((*n).to_string(), Some((*t).to_string())))
        .collect()
}

/// `(name, type name, flags)` for a tkMethod type's parameters.
fn method_params(detail: &TypeDetail<'_>) -> Vec<(String, String, u8)> {
    let TypeDetail::Method(m) = detail else {
        panic!("not a tkMethod: {detail:?}");
    };
    m.params
        .iter()
        .map(|p| (p.name().to_string(), p.type_name().to_string(), p.flags))
        .collect()
}

fn param(name: &str, ty: &str, flags: u8) -> (String, String, u8) {
    (name.to_string(), ty.to_string(), flags)
}

/// The `known.dpr` RTTI shared by the static and runtime-package builds.
fn assert_known_types(b: Build, bin: &DelphiBinary<'_>) {
    let TypeDetail::Enumeration(color) = find_type(bin, "TColor") else {
        panic!("TColor kind");
    };
    let values: Vec<&[u8]> = color.values.clone();
    assert_eq!(
        values,
        [&b"clRed"[..], b"clGreen", b"clBlue", b"clAlpha"],
        "{b:?} TColor values"
    );
    assert_eq!(color.unit_name(), Some("known"));

    // Delphi emits no hidden Self (unlike FPC); a class parameter carries
    // pfAddress (0x08).
    assert_eq!(
        method_params(&find_type(bin, "TProgressEvent")),
        vec![
            param("Sender", "TObject", 0x08),
            param("Percent", "Integer", 0)
        ],
        "{b:?}"
    );
}

#[test]
fn known_static() {
    for b in BUILDS {
        let data = b.sample("known.exe");
        let bin = DelphiBinary::parse(&data).expect("parse");
        let map = b.map("known.map", bin.image_base().unwrap());

        // A statically linked Delphi 6 image carries only the era marker;
        // Delphi 13 writes its full build string.
        let info = bin.compiler().expect("compiler");
        assert_eq!(info.compiler, Compiler::Delphi);
        if b.modern() {
            assert_eq!(info.source, DetectionSource::BuildString, "{b:?}");
            assert_eq!(info.version, Some("37.0"));
            assert_eq!(info.product_name().as_deref(), Some("Delphi 13 Florence"));
        } else {
            assert_eq!(info.source, DetectionSource::BorlandRegistry);
            assert_eq!(info.version, None);
        }
        assert_eq!(bin.edition(), Some(Edition::Enterprise));
        let arch = if b == Build::Delphi13Win64 {
            TargetArch::X86_64
        } else {
            TargetArch::X86
        };
        assert_eq!(bin.target_arch(), arch);
        assert!(bin.required_packages().is_empty());

        // The header: vmtSelfPtr = -76 in Delphi 6 (19 slots); -88 from
        // Delphi 2009 (22); Win64 -176 - CPP_ABI_ADJUST (25).
        let tshape = class(&bin, "TShape");
        let slots = match b {
            Build::Delphi6 => 19,
            Build::Delphi13Win32 => 22,
            Build::Delphi13Win64 => 25,
        };
        assert_eq!(tshape.header_slots(), slots, "{b:?}");
        assert_eq!(
            bin.parent_class(tshape).map(Class::name),
            Some("TPersistent")
        );

        assert_eq!(
            prop_types(&bin, "TShape"),
            pairs(&[
                ("Name", b.string()),
                ("Color", "TColor"),
                ("Visible", "Boolean")
            ]),
            "{b:?}"
        );
        assert_eq!(
            prop_types(&bin, "TButton"),
            pairs(&[("Width", "Integer"), ("OnProgress", "TProgressEvent")])
        );
        let tbutton = class(&bin, "TButton");
        let methods: Vec<(&str, u64)> = bin
            .methods(tbutton)
            .iter()
            .map(|m| (m.name(), m.code_va))
            .collect();
        assert_eq!(methods, vec![("Click", map["TButton.Click"])], "{b:?}");

        assert_known_types(b, &bin);

        let TypeDetail::Set(colors) = find_type(&bin, "TColors") else {
            panic!("TColors kind");
        };
        assert_eq!(colors.element_type.map(|h| h.name()), Some("TColor"));

        // TVertex = (X, Y: Integer; Name: string; Weight: Integer): the
        // string sits after two Integers, a pointer later the Weight.
        let TypeDetail::Record(vertex) = find_type(&bin, "TVertex") else {
            panic!("TVertex kind");
        };
        let size = if b == Build::Delphi13Win64 { 24 } else { 16 };
        assert_eq!(vertex.record_size, size, "{b:?}");
        let managed: Vec<(u64, &str)> = vertex
            .managed_fields
            .iter()
            .map(|f| (f.offset, f.field_type.map_or("?", |h| h.name())))
            .collect();
        assert_eq!(managed, vec![(8, b.string())], "{b:?}");

        let TypeDetail::DynArray(array) = find_type(&bin, "TStringArray") else {
            panic!("TStringArray kind");
        };
        assert_eq!(array.element_type.map(|h| h.name()), Some(b.string()));

        let TypeDetail::Interface(widget) = find_type(&bin, "IWidget") else {
            panic!("IWidget kind");
        };
        assert_eq!(
            widget.guid.to_string(),
            "{1A2B3C4D-5E6F-7081-9223-A1B2C3D4E5F6}"
        );
        assert_eq!(widget.parent_type.map(|h| h.name()), Some("IInterface"));
    }
}

#[test]
fn known_runtime_packages() {
    for b in BUILDS {
        let data = b.sample("known_rtl.exe");
        let bin = DelphiBinary::parse(&data).expect("parse");

        // The RTL - and its markers - live in the rtl package; the import
        // pins the version.
        let (rtl, version, product, classes_unit) = if b.modern() {
            ("rtl370.bpl", "37.0", "Delphi 13 Florence", "System.Classes")
        } else {
            ("rtl60.bpl", "14.0", "Delphi 6", "Classes")
        };
        let info = bin.compiler().expect("compiler");
        assert_eq!(info.compiler, Compiler::Delphi);
        assert_eq!(info.version, Some(version), "{b:?}");
        assert_eq!(info.product_name().as_deref(), Some(product));
        if !b.modern() {
            assert_eq!(info.source, DetectionSource::PackageImports);
        }
        assert_eq!(bin.required_packages(), vec![rtl], "{b:?}");

        // The executable's PACKAGEINFO has no requires list: it contains
        // only its own units.
        let package = bin.package_info().expect("PACKAGEINFO");
        assert!(package.requires.is_empty());
        let units: Vec<&str> = package.contains.iter().map(|u| u.name).collect();
        assert_eq!(units, vec!["known", "SysInit"], "{b:?}");

        let tshape = class(&bin, "TShape");
        assert!(bin.parent_class(tshape).is_none());
        let parent = bin.external_parent(tshape).expect("external parent");
        assert_eq!(parent.module(), rtl);
        assert_eq!(
            parent.symbol().map(|s| (s.kind, s.unit, s.name)),
            Some((
                SymbolKind::ClassVmt,
                classes_unit.to_string(),
                "TPersistent".to_string()
            )),
            "{b:?}"
        );

        // Types declared in the exe resolve locally; RTL types are imports.
        let typed: Vec<(String, String)> = ["TShape", "TButton"]
            .iter()
            .flat_map(|name| {
                let c = class(&bin, name);
                bin.properties(c)
                    .into_iter()
                    .map(|p| {
                        let ty = bin.property_type(c, &p).map_or_else(
                            || {
                                let symbol =
                                    bin.import_at(p.prop_type_ref).unwrap().symbol().unwrap();
                                assert_eq!(symbol.kind, SymbolKind::TypeInfo);
                                if symbol.unit.is_empty() {
                                    symbol.name
                                } else {
                                    format!("{}.{}", symbol.unit, symbol.name)
                                }
                            },
                            |h| h.name().to_string(),
                        );
                        (p.name().to_string(), ty)
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        // Delphi 13's rtl370 exports System's intrinsics in the C++ global
        // namespace on Win32 (`@$xp$7Boolean`: no unit in the symbol); its
        // Win64 symbols keep the namespace (`_ZTRN6System7BooleanE`).
        let (string, boolean, integer) = match b {
            Build::Delphi6 => ("System.String", "System.Boolean", "System.Integer"),
            Build::Delphi13Win32 => ("System.string", "Boolean", "Integer"),
            Build::Delphi13Win64 => ("System.string", "System.Boolean", "System.Integer"),
        };
        let expected: Vec<(String, String)> = [
            ("Name", string),
            ("Color", "TColor"),
            ("Visible", boolean),
            ("Width", integer),
            ("OnProgress", "TProgressEvent"),
        ]
        .iter()
        .map(|(n, t)| ((*n).to_string(), (*t).to_string()))
        .collect();
        assert_eq!(typed, expected, "{b:?}");

        assert_known_types(b, &bin);
    }
}

#[test]
fn tables_property_kinds_and_specifiers() {
    for b in BUILDS {
        let data = b.sample("tables.exe");
        let bin = DelphiBinary::parse(&data).expect("parse");
        let map = b.map("tables.map", bin.image_base().unwrap());

        // From Delphi 2009 Char is WideChar, and RTTI names it Char.
        let wide_char = if b.modern() { "Char" } else { "WideChar" };
        assert_eq!(
            prop_types(&bin, "TAllProps"),
            pairs(&[
                ("PInt", "Integer"),
                ("PByte", "Byte"),
                ("PSmall", "TSmall"),
                ("PInt64", "Int64"),
                ("PChar", "Char"),
                ("PWChar", wide_char),
                ("PShort", "ShortString"),
                ("PStr", b.string()),
                ("PWStr", "WideString"),
                ("PVar", "Variant"),
                ("PSingle", "Single"),
                ("PDouble", "Double"),
                ("PExt", "Extended"),
                ("PComp", "Comp"),
                ("PCurr", "Currency"),
                ("PKind", "TKind"),
                ("PKinds", "TKinds"),
                ("PList", "TStrings"),
                ("OnKind", "TKindEvent"),
                ("OnParse", "TParseFunc"),
                ("PSource", "ISource"),
                ("PStored", "Integer"),
                ("PNoDef", "Integer"),
                ("PReadOnly", "Integer"),
                ("PMethod", b.string()),
                ("PVirt", "Integer"),
                ("PSlot2", "Integer"),
                ("PSlot3", "Integer"),
            ]),
            "{b:?}"
        );

        let all = class(&bin, "TAllProps");
        let props: HashMap<String, _> = bin
            .properties(all)
            .into_iter()
            .map(|p| (p.name().to_string(), p))
            .collect();
        // `default` clauses; MinInt marks "no default" (and `nodefault`).
        assert_eq!(props["PInt"].default, 7);
        assert_eq!(props["PKind"].default, 1, "default kBeta");
        assert_eq!(props["PKinds"].default, 1, "default [kAlpha] as a bitmask");
        assert_eq!(props["PByte"].default, i32::MIN);
        assert_eq!(props["PNoDef"].default, i32::MIN);
        // `stored` constants and a missing setter.
        assert_eq!(
            (props["PStored"].stored.kind, props["PStored"].stored.value),
            (AccessKind::Const, 0)
        );
        assert_eq!(
            (props["PInt"].stored.kind, props["PInt"].stored.value),
            (AccessKind::Const, 1)
        );
        assert_eq!(props["PReadOnly"].set.kind, AccessKind::None);
        // Field, static and indexed accessors. FInt follows the VMT
        // pointer (Delphi 2009+ objects keep the monitor at the end).
        assert_eq!(
            (props["PInt"].get.kind, props["PInt"].get.value),
            (AccessKind::Field, b.ptr()),
            "{b:?}"
        );
        assert_eq!(
            (props["PMethod"].get.kind, props["PMethod"].get.value),
            (AccessKind::Static, map["TAllProps.GetStr"])
        );
        assert_eq!(props["PSlot2"].index, 2);
        assert_eq!(props["PSlot3"].index, 3);
        assert_eq!(props["PSlot3"].get.value, map["TAllProps.GetSlot"]);
        // A virtual getter stores the slot's VMT byte offset: GetVirt is
        // the fourth virtual (after TPersistent's three).
        let virt = props["PVirt"].get;
        assert_eq!(
            (virt.kind, virt.value),
            (AccessKind::Virtual, 3 * b.ptr()),
            "{b:?}"
        );
        assert_eq!(
            virt.resolve(&bin.virtual_methods(all)),
            AccessTarget::CodeVa(map["TAllProps.GetVirt"])
        );
        assert_eq!(bin.virtual_methods(all).len(), 4, "{b:?}");
    }
}

#[test]
fn tables_vmt_tables() {
    for b in BUILDS {
        let data = b.sample("tables.exe");
        let bin = DelphiBinary::parse(&data).expect("parse");
        let map = b.map("tables.map", bin.image_base().unwrap());
        let p = b.ptr();

        // Field table + class table, published methods. The fields follow
        // TComponent's own: 48 bytes in Delphi 6, 64 / 120 in Delphi 13
        // (its System.Classes.dcu states the instance size).
        let owner = class(&bin, "TOwnerThing");
        let fields: Vec<(String, u32, String)> = bin
            .fields(owner)
            .iter()
            .map(|f| {
                let target = bin.field_class(owner, f).expect("field class");
                (f.name().to_string(), f.offset, target.name().to_string())
            })
            .collect();
        let base: u32 = match b {
            Build::Delphi6 => 48,
            Build::Delphi13Win32 => 64,
            Build::Delphi13Win64 => 120,
        };
        let step = p as u32;
        let expected: Vec<(String, u32, String)> = [
            ("PartA", base, "TPart"),
            ("PartB", base + step, "TPart"),
            ("Names", base + 2 * step, "TStringList"),
        ]
        .iter()
        .map(|(n, o, c)| ((*n).to_string(), *o, (*c).to_string()))
        .collect();
        assert_eq!(fields, expected, "{b:?}");
        let typed: Vec<Option<&str>> = bin
            .fields_with_types(owner)
            .iter()
            .map(|f| f.ty.map(|h| h.name()))
            .collect();
        assert_eq!(
            typed,
            vec![Some("TPart"), Some("TPart"), Some("TStringList")]
        );
        let methods: Vec<(&str, u64)> = bin
            .methods(owner)
            .iter()
            .map(|m| (m.name(), m.code_va))
            .collect();
        assert_eq!(
            methods,
            vec![
                ("DoClick", map["TOwnerThing.DoClick"]),
                ("DoOther", map["TOwnerThing.DoOther"]),
            ],
            "{b:?}"
        );

        // Interface table: ISink descends from ISource, so both share one
        // vtable slot at the end of the instance.
        let dispatcher = class(&bin, "TDispatcher");
        let mut intfs: Vec<(String, u64)> = bin
            .interfaces(dispatcher)
            .iter()
            .map(|i| (i.guid.to_string(), i.offset))
            .collect();
        intfs.sort();
        // The slot follows the last field: TInterfacedObject's own (12
        // bytes, Win64 24 per System.dcu), then FStr .. FPlain, padded.
        let slot = match b {
            Build::Delphi6 | Build::Delphi13Win32 => 72,
            Build::Delphi13Win64 => 128,
        };
        assert_eq!(
            intfs,
            vec![
                ("{11111111-2222-3333-4444-555555555555}".to_string(), slot),
                ("{66666666-7777-8888-9999-AAAAAAAAAAAA}".to_string(), slot),
            ],
            "{b:?}"
        );

        // Dynamic table: negative indexes for `dynamic`, message IDs for
        // `message`.
        let slots: Vec<(i16, u64)> = bin
            .dynamic_slots(dispatcher)
            .iter()
            .map(|s| (s.index, s.handler_va))
            .collect();
        assert_eq!(
            slots,
            vec![
                (-1, map["TDispatcher.DynA"]),
                (-2, map["TDispatcher.DynB"]),
                (0x0400 + 1, map["TDispatcher.WMGtPing"]),
                (0x000F, map["TDispatcher.WMPaint"]),
            ],
            "{b:?}"
        );
        let overrides: Vec<(i16, u64)> = bin
            .dynamic_slots(class(&bin, "TDispatcher2"))
            .iter()
            .map(|s| (s.index, s.handler_va))
            .collect();
        assert_eq!(overrides, vec![(-1, map["TDispatcher2.DynA"])]);

        // Init table: every managed field, in offset order.
        let init = bin.init_table(dispatcher).expect("init table");
        let managed: Vec<(u64, TypeKind)> = init
            .managed_fields
            .iter()
            .map(|f| (f.offset, f.field_type.expect("managed type").kind))
            .collect();
        let expected = match b {
            Build::Delphi6 => vec![
                (12, TypeKind::LString),
                (16, TypeKind::WString),
                (24, TypeKind::Variant),
                (40, TypeKind::Interface),
                (44, TypeKind::DynArray),
                (48, TypeKind::Record),
                (56, TypeKind::Array),
            ],
            Build::Delphi13Win32 => vec![
                (12, TypeKind::UString),
                (16, TypeKind::WString),
                (24, TypeKind::Variant),
                (40, TypeKind::Interface),
                (44, TypeKind::DynArray),
                (48, TypeKind::Record),
                (56, TypeKind::Array),
            ],
            // A 24-byte Variant on Win64.
            Build::Delphi13Win64 => vec![
                (24, TypeKind::UString),
                (32, TypeKind::WString),
                (40, TypeKind::Variant),
                (64, TypeKind::Interface),
                (72, TypeKind::DynArray),
                (80, TypeKind::Record),
                (96, TypeKind::Array),
            ],
        };
        assert_eq!(managed, expected, "{b:?}");
        assert_eq!(bin.virtual_methods(dispatcher).len(), 2);

        // Automation table: reverse declaration order, setter before
        // getter. Delphi 2009+ dropped `automated` sections' table.
        let auto: Vec<AutoRow<'_>> = bin
            .auto_entries(class(&bin, "TAutoThing"))
            .iter()
            .map(|e| {
                (
                    e.dispid,
                    e.name(),
                    e.flags,
                    e.result_type,
                    e.param_types.clone(),
                    e.address,
                )
            })
            .collect();
        assert_eq!(
            auto,
            vec![
                (103, "Level", 0x4, 0, vec![3], map["TAutoThing.SetLevel"]),
                (103, "Level", 0x2, 3, vec![], map["TAutoThing.GetLevel"]),
                (102, "Reset", 0x1, 0, vec![], map["TAutoThing.Reset"]),
                (101, "Ping", 0x1, 3, vec![3], map["TAutoThing.Ping"]),
            ],
            "{b:?}"
        );

        // The Turbo Pascal `object` has a header-less VMT: not a class.
        assert!(bin.classes().find_by_name("TLegacy").is_none());
    }
}

#[test]
fn tables_standalone_types() {
    for b in BUILDS {
        let data = b.sample("tables.exe");
        let bin = DelphiBinary::parse(&data).expect("parse");

        let TypeDetail::Ordinal(small) = find_type(&bin, "TSmall") else {
            panic!("TSmall kind");
        };
        assert_eq!((small.min, small.max), (1, 100));

        // pfVar = 0x01, pfConst = 0x02, pfAddress = 0x08, pfOut = 0x20.
        assert_eq!(
            method_params(&find_type(&bin, "TKindEvent")),
            vec![
                param("Sender", "TObject", 0x08),
                param("Kind", "TKind", 0),
                param("Handled", "Boolean", 0x01),
            ],
            "{b:?}"
        );
        let parse_func = find_type(&bin, "TParseFunc");
        assert_eq!(
            method_params(&parse_func),
            vec![param("S", b.string(), 0x02), param("N", "Integer", 0x20)],
            "{b:?}"
        );
        let TypeDetail::Method(parse_func) = parse_func else {
            unreachable!();
        };
        assert_eq!(parse_func.result_type, Some(&b"Boolean"[..]));

        let TypeDetail::Interface(sink) = find_type(&bin, "ISink") else {
            panic!("ISink kind");
        };
        assert_eq!(sink.parent_type.map(|h| h.name()), Some("ISource"));

        // TTagged = (Tag: Integer; Name: string).
        let TypeDetail::Record(tagged) = find_type(&bin, "TTagged") else {
            panic!("TTagged kind");
        };
        assert_eq!(tagged.record_size, 2 * b.ptr() as u32, "{b:?}");
        assert_eq!(tagged.managed_fields.len(), 1);
        assert_eq!(tagged.managed_fields[0].offset, b.ptr());
    }
}

fn child<'o, 'a>(object: &'o DfmObject<'a>, name: &str) -> &'o DfmObject<'a> {
    object
        .children
        .iter()
        .find(|c| c.object_name() == name)
        .unwrap_or_else(|| panic!("child {name}"))
}

fn value<'o, 'a>(object: &'o DfmObject<'a>, name: &str) -> &'o DfmValue<'a> {
    &object
        .properties
        .iter()
        .find(|p| p.name() == name)
        .unwrap_or_else(|| panic!("property {name} on {}", object.object_name()))
        .value
}

fn text(object: &DfmObject<'_>, name: &str) -> String {
    value(object, name)
        .as_text()
        .expect("text value")
        .into_owned()
}

#[test]
fn gtforms_tpf0_values() {
    for b in BUILDS {
        let data = b.sample("gtforms.exe");
        let bin = DelphiBinary::parse(&data).expect("parse");
        let map = b.map("gtforms.map", bin.image_base().unwrap());

        let forms: HashMap<&str, &DfmObject<'_>> = bin
            .forms()
            .iter()
            .map(|(_, f)| (f.class_name(), f))
            .collect();
        let mut names: Vec<&str> = forms.keys().copied().collect();
        names.sort_unstable();
        assert_eq!(
            names,
            vec!["TGTChildForm", "TGTFrame", "TGTMainForm"],
            "{b:?}"
        );

        let main = forms["TGTMainForm"];
        assert_eq!(text(main, "Caption"), "undelphi ground-truth form");
        assert!(matches!(value(main, "Tag"), DfmValue::Int(-5)));
        assert!(matches!(value(main, "KeyPreview"), DfmValue::Bool(true)));
        assert_eq!(text(main, "Position"), "poScreenCenter");
        let DfmValue::Set(style) = value(main, "Font.Style") else {
            panic!("Font.Style");
        };
        assert_eq!(style, &vec![&b"fsBold"[..], b"fsItalic"]);
        assert_eq!(text(main, "OnCreate"), "FormCreate");
        let form_class = class(&bin, "TGTMainForm");
        assert_eq!(
            bin.resolve_event_handler(form_class, "FormCreate"),
            Some(map["TGTMainForm.FormCreate"]),
            "{b:?}"
        );

        // Non-ASCII text whose UTF-8 is shorter than UTF-16 is vaUTF8String.
        assert_eq!(text(child(main, "Label1"), "Caption"), "Größe λβγ");
        let DfmValue::List(lines) = value(child(main, "Memo1"), "Lines.Strings") else {
            panic!("Lines.Strings");
        };
        let lines: Vec<String> = lines
            .iter()
            .map(|v| v.as_text().unwrap().into_owned())
            .collect();
        assert_eq!(lines, vec!["line one", "line two", ""]);
        let DfmValue::Binary(picture) = value(child(main, "Image1"), "Picture.Data") else {
            panic!("Picture.Data");
        };
        assert_eq!(picture.len(), 82);
        assert!(picture.starts_with(b"\x07TBitmap"));
        let DfmValue::Collection(panels) = value(child(main, "StatusBar1"), "Panels") else {
            panic!("Panels");
        };
        let panel_text: Vec<String> = panels
            .iter()
            .map(|item| {
                item.iter()
                    .find(|p| p.name() == "Text")
                    .unwrap()
                    .value
                    .as_text()
                    .unwrap()
                    .into_owned()
            })
            .collect();
        assert_eq!(panel_text, vec!["first panel", "second panel"]);

        // Filer flags: inline frame, inherited component inside it.
        let frame = child(main, "Frame1");
        assert!(frame.flags.inline_);
        let edit = child(frame, "Edit1");
        assert!(edit.flags.inherited_);
        assert_eq!(text(edit, "Text"), "overridden in host");

        // One value of every remaining TValueType.
        let values = child(main, "Values1");
        assert!(matches!(
            value(values, "Big"),
            DfmValue::Int64(1_234_567_890_123)
        ));
        assert_eq!(value(values, "Ratio").as_f64(), Some(1.5));
        assert_eq!(value(values, "Precise").as_f64(), Some(3.25));
        assert!(matches!(
            value(values, "Money"),
            DfmValue::Currency(125_000)
        ));
        assert!(matches!(value(values, "When"), DfmValue::Double(w) if *w == 37000.5));
        assert!(matches!(value(values, "Wide"), DfmValue::Utf16(_)));
        assert_eq!(text(values, "Wide"), "Привет");
        assert_eq!(text(values, "Mode"), "gmBusy");
        assert!(matches!(value(values, "Small"), DfmValue::Int(100)));
        assert!(matches!(value(values, "Medium"), DfmValue::Int(30_000)));
        assert!(matches!(value(values, "Large"), DfmValue::Int(70_000)));
        assert!(matches!(value(values, "Negative"), DfmValue::Int(-200)));
        assert!(matches!(value(values, "Enabled2"), DfmValue::Bool(false)));
        assert!(matches!(value(values, "Link"), DfmValue::Nil));
        // Both compilers write an empty string as a zero-length vaWString.
        assert!(
            matches!(value(values, "Text"), DfmValue::Utf16(b) if b.is_empty()),
            "{b:?}"
        );
        assert_eq!(text(values, "LongText").len(), 260);
        assert!(text(values, "LongText").ends_with("tail"));

        // Visual form inheritance and a child-position modifier.
        let child_form = forms["TGTChildForm"];
        assert!(child(child_form, "Label1").flags.inherited_);
        assert_eq!(text(child(child_form, "Label1"), "Caption"), "child label");
        let check = child(child_form, "CheckBox1");
        assert!(check.flags.child_pos);
        assert_eq!(check.child_pos, Some(1));

        // Every component field resolves to its class.
        let fields: Vec<(String, String)> = bin
            .fields(form_class)
            .iter()
            .map(|f| {
                let target = bin.field_class(form_class, f).expect("field class");
                (f.name().to_string(), target.name().to_string())
            })
            .collect();
        let expected: Vec<(String, String)> = [
            ("Label1", "TLabel"),
            ("Memo1", "TMemo"),
            ("Image1", "TImage"),
            ("Button1", "TButton"),
            ("StatusBar1", "TStatusBar"),
            ("Frame1", "TGTFrame"),
            ("Values1", "TGTValues"),
        ]
        .iter()
        .map(|(n, c)| ((*n).to_string(), (*c).to_string()))
        .collect();
        assert_eq!(fields, expected, "{b:?}");
    }
}

#[test]
fn runtime_package_and_its_client() {
    for b in BUILDS {
        let (rtl, version) = if b.modern() {
            ("rtl370.bpl", "37.0")
        } else {
            ("rtl60.bpl", "14.0")
        };
        let data = b.sample("gtpkg.bpl");
        let pkg = DelphiBinary::parse(&data).expect("parse bpl");
        let info = pkg.compiler().expect("compiler");
        assert_eq!(info.version, Some(version), "{b:?}");
        if !b.modern() {
            assert_eq!(info.source, DetectionSource::PackageImports);
        }
        let package = pkg.package_info().expect("PACKAGEINFO");
        let requires: Vec<&str> = package.requires.iter().map(|r| r.name).collect();
        assert_eq!(requires, vec![rtl], "{b:?}");
        let units: Vec<&str> = package.contains.iter().map(|u| u.name).collect();
        assert_eq!(units, vec!["gtpkg", "gtshared", "SysInit"], "{b:?}");
        let names: Vec<&str> = pkg
            .properties(class(&pkg, "TGTShared"))
            .iter()
            .map(|p| p.name())
            .collect();
        assert_eq!(names, vec!["Count", "Caption"]);

        let data = b.sample("usepkg.exe");
        let client = DelphiBinary::parse(&data).expect("parse exe");
        assert_eq!(client.required_packages(), vec![rtl, "gtpkg.bpl"], "{b:?}");
        let local = class(&client, "TGTLocal");
        let parent = client.external_parent(local).expect("external parent");
        assert_eq!(parent.module(), "gtpkg.bpl");
        assert_eq!(
            parent.symbol(),
            Some(BorlandSymbol {
                kind: SymbolKind::ClassVmt,
                unit: "Gtshared".into(),
                name: "TGTShared".into(),
            }),
            "{b:?}"
        );
        // TGTLocal's own property is numbered after the two it inherits.
        let extra = client.properties(local);
        assert_eq!(extra.len(), 1);
        assert_eq!((extra[0].name(), extra[0].name_index), ("Extra", 2));
    }
}

#[test]
fn cpp_builder_vcl_classes() {
    let b = Build::Delphi6;
    let data = b.sample("cppknown.exe");
    let bin = DelphiBinary::parse(&data).expect("parse");
    let map = b.map("cppknown.map", bin.image_base().unwrap());

    assert_eq!(bin.compiler_kind(), Some(Compiler::CppBuilder));

    // C++Builder names RTTI after the C++ spelling, and writes `default`
    // as 0 rather than Delphi's MinInt.
    assert_eq!(
        prop_types(&bin, "TCppShape"),
        pairs(&[("Name", "AnsiString"), ("Visible", "bool")])
    );
    let shape = class(&bin, "TCppShape");
    assert!(bin.properties(shape).iter().all(|p| p.default == 0));

    let button = class(&bin, "TCppButton");
    let props = bin.properties_with_types(button);
    assert_eq!(props[0].property.name(), "Width");
    assert_eq!(props[0].ty.map(|h| h.name()), Some("int"));
    // A `__closure` typedef gets a nameless tkMethod record.
    assert_eq!(props[1].property.name(), "OnProgress");
    let event = bin
        .property_type_detail(button, &props[1].property)
        .expect("closure type");
    assert_eq!(event.header().name(), "");
    assert_eq!(
        method_params(&event),
        vec![param("Sender", "TObject", 0x08), param("Percent", "int", 0)]
    );

    let methods: Vec<(&str, u64)> = bin
        .methods(button)
        .iter()
        .map(|m| (m.name(), m.code_va))
        .collect();
    assert_eq!(
        methods,
        vec![(
            "Click",
            map["__fastcall TCppButton::Click(System::TObject *)"]
        )]
    );
    let virtuals = bin.virtual_methods(button);
    assert_eq!(virtuals.len(), 4);
    assert_eq!(
        virtuals[3].code_va, map["__fastcall TCppButton::Paint()"],
        "Paint follows TPersistent's three virtuals"
    );

    // C++ `bool` is a tkEnumeration with no unit name.
    let TypeDetail::Enumeration(boolean) = find_type(&bin, "bool") else {
        panic!("bool kind");
    };
    assert_eq!(boolean.values, vec![&b"false"[..], b"true"]);
    assert_eq!(boolean.unit_name(), None);

    // An ordinary C++ class has a C++ vtable, not a Delphi VMT.
    assert!(bin.classes().find_by_name("TPlainShape").is_none());
}

#[test]
fn cpp_builder_without_vcl() {
    // A plain Borland C++ program: no Delphi RTL and no classes, but the
    // C++Builder linker still adds a DVCLAL resource, and the C++ runtime's
    // debugger-hook marker identifies the toolchain.
    let data = Build::Delphi6.sample("cppplain.exe");
    let bin = DelphiBinary::parse(&data).expect("parse");
    let info = bin.compiler().expect("compiler");
    assert_eq!(info.compiler, Compiler::CppBuilder);
    assert_eq!(info.source, DetectionSource::CppRuntime);
    assert_eq!(bin.edition(), Some(Edition::Enterprise));
    assert!(bin.classes().is_empty());
}

/// Every code address undelphi labels must start a routine the linker map
/// names - across the whole RTL and VCL linked in, not only our own
/// classes. Interface slots are compiler-generated adjustor thunks, which
/// the map does not list; their decoded targets must be named instead.
#[test]
fn every_code_entrypoint_starts_a_mapped_routine() {
    for b in BUILDS {
        for name in ["known", "tables", "deep", "gtforms"] {
            let data = b.sample(&format!("{name}.exe"));
            let bin = DelphiBinary::parse(&data).expect("parse");
            let mapped: HashSet<u64> = b
                .map_entries(&format!("{name}.map"), bin.image_base().unwrap())
                .into_iter()
                .map(|(_, va)| va)
                .collect();
            let mut checked = 0usize;
            for entry in bin.code_entrypoints() {
                if entry.kind == EntrypointKind::InterfaceMethod {
                    continue;
                }
                assert!(
                    mapped.contains(&entry.va),
                    "{b:?} {name}: {} at {:#x} ({:?}) is not a mapped routine",
                    entry.name_hint,
                    entry.va,
                    entry.kind
                );
                checked += 1;
            }
            assert!(checked > 100, "{b:?} {name}: only {checked} entrypoints");
        }
    }
}

#[test]
fn unit_init_table_matches_the_map() {
    for b in BUILDS {
        let data = b.sample("known.exe");
        let bin = DelphiBinary::parse(&data).expect("parse");
        let map = b.map("known.map", bin.image_base().unwrap());
        let table = bin.unit_init_table().expect("unit init table");
        let entries: Vec<(Option<u64>, Option<u64>)> = table
            .units
            .iter()
            .map(|u| (u.init_va, u.finalize_va))
            .collect();
        // SysInit and System initialize first; the program unit is last and
        // has no initialization section.
        assert_eq!(entries[0].0, Some(map["SysInit"]), "{b:?}");
        assert_eq!(entries[1].0, Some(map["System"]));
        if b.modern() {
            // 29 units and their class constructor / destructor pairs, each
            // pair an entry of its own before its unit's; the program's
            // Finalization is its last entry's FInit.
            assert_eq!(entries.len(), 45, "{b:?}");
            assert_eq!(
                entries[13],
                (
                    Some(map["System.SysUtils.TLanguages.Create@"]),
                    Some(map["System.SysUtils.TLanguages.Destroy@"])
                ),
                "{b:?}"
            );
            assert_eq!(
                entries[40],
                (
                    Some(map["System.Classes.TComponent.Create@"]),
                    Some(map["System.Classes.TComponent.Destroy@"])
                ),
                "{b:?}"
            );
            assert_eq!(
                entries[43],
                (
                    Some(map["System.Classes.System.Classes"]),
                    Some(map["System.Classes.Finalization"])
                ),
                "{b:?}"
            );
            assert_eq!(
                entries[44],
                (None, Some(map["known.Finalization"])),
                "{b:?}"
            );
            // Delphi 2010+ adds the table of type-info references.
            assert!(table.types.is_some(), "{b:?}");
        } else {
            assert_eq!(entries.len(), 14);
            assert_eq!(entries[12].0, Some(map["Classes"]));
            assert_eq!(entries[13].0, None);
            assert!(table.units.iter().all(|u| u.finalize_va.is_some()));
            assert!(table.types.is_none());
        }
        assert_eq!(bin.unit_init_procs().len(), entries.len(), "{b:?}");

        // A package reaches its table through @GetPackageInfoTable. Like a
        // program's it covers the whole uses closure, the required
        // packages' units through small stubs the map names after them.
        let data = b.sample("gtpkg.bpl");
        let pkg = DelphiBinary::parse(&data).expect("parse bpl");
        let package_units = if b.modern() { 46 } else { 14 };
        assert_eq!(
            pkg.unit_init_table().expect("package table").units.len(),
            package_units,
            "{b:?}"
        );
    }
}

#[test]
fn interface_thunks_resolve_to_implementations() {
    for b in BUILDS {
        let data = b.sample("deep.exe");
        let bin = DelphiBinary::parse(&data).expect("parse");
        let map = b.map("deep.map", bin.image_base().unwrap());
        let imp = class(&bin, "TImpl");
        let svc = bin
            .interfaces(imp)
            .into_iter()
            .find(|e| e.guid.to_string() == "{0A1B2C3D-4E5F-6071-8293-A4B5C6D7E8F9}")
            .expect("ISvc entry");
        let methods = bin.interface_methods(&svc);
        // IInterface's three methods, then ISvc's four - named from ISvc's
        // method info.
        let names: Vec<Option<&str>> = methods.iter().map(|m| m.name).collect();
        assert_eq!(
            names,
            vec![
                None,
                None,
                None,
                Some("Add"),
                Some("Rename"),
                Some("Ratio"),
                Some("Reset")
            ],
            "{b:?}"
        );
        // Each thunk subtracts the interface slot's offset from Self.
        let adjust = -i64::try_from(svc.offset).unwrap();
        assert!(
            methods
                .iter()
                .all(|m| m.thunk.map(|t| t.self_adjust) == Some(adjust)),
            "{b:?}"
        );
        let targets: Vec<u64> = methods[3..]
            .iter()
            .map(|m| bin.interface_method_target(imp, m).expect("target"))
            .collect();
        assert_eq!(
            targets,
            vec![
                map["TImpl.Add"],
                map["TImpl.Rename"],
                map["TImpl.Ratio"],
                map["TImpl.Reset"],
            ],
            "{b:?}"
        );
        // Reset is virtual: its thunk dispatches through the VMT, so for a
        // TImpl2 instance it reaches the override.
        assert_eq!(
            methods[6].thunk.map(|t| t.target),
            Some(ThunkTarget::VmtOffset(0))
        );
        assert_eq!(
            bin.interface_method_target(class(&bin, "TImpl2"), &methods[6]),
            Some(map["TImpl2.Reset"])
        );

        // `implements` delegation: through a field, and through a getter.
        let entries = bin.interfaces(imp);
        let extra = entries
            .iter()
            .find(|e| e.guid.to_string() == "{11223344-5566-7788-99AA-BBCCDDEEFF00}")
            .expect("IExtra");
        assert_eq!((extra.vtable_va, extra.offset, extra.getter_va), (0, 0, 0));
        assert_eq!(
            (extra.getter.kind, extra.getter.value),
            (AccessKind::Field, 3 * b.ptr()),
            "{b:?}"
        );
        let more = entries
            .iter()
            .find(|e| e.guid.to_string() == "{22334455-6677-8899-AABB-CCDDEEFF0011}")
            .expect("IMore");
        assert_eq!(more.getter.kind, AccessKind::Static);
        assert_eq!(more.getter_va, map["TImpl.GetMore"]);
    }
}

#[test]
fn invokable_interface_method_info() {
    for b in BUILDS {
        let data = b.sample("deep.exe");
        let bin = DelphiBinary::parse(&data).expect("parse");
        let TypeDetail::Interface(svc) = find_type(&bin, "ISvc") else {
            panic!("ISvc kind");
        };
        // Bit 3 marks method info (beyond the three TIntfFlags).
        assert_eq!(svc.flags, 0x09, "{b:?}");
        assert_eq!(svc.parent_type.map(|h| h.name()), Some("IInvokable"));
        let rtti = bin.interface_rtti(&svc).expect("method info");
        assert_eq!(rtti.method_count, 4);
        let rendered: Vec<String> = rtti
            .methods
            .iter()
            .map(|m| {
                let params: Vec<String> = m
                    .params
                    .iter()
                    .map(|p| format!("{:#x} {}: {}", p.flags, p.name(), p.type_name()))
                    .collect();
                format!(
                    "{:?} {:?} {}({}) {:?}",
                    m.kind,
                    m.calling_convention,
                    m.name(),
                    params.join("; "),
                    m.result_type_name()
                )
            })
            .collect();
        let s = b.string();
        assert_eq!(
            rendered,
            vec![
                "Function Register Add(0x0 A: Integer; 0x0 B: Integer) Some(\"Integer\")"
                    .to_string(),
                format!(
                    "Procedure Register Rename(0x2 Old: {s}; 0x1 New: {s}; 0x20 Count: Integer) None"
                ),
                "Function StdCall Ratio(0x0 X: Double) Some(\"Double\")".to_string(),
                "Procedure Register Reset() None".to_string(),
            ],
            "{b:?}"
        );
        // An interface without method info records only its count.
        let TypeDetail::Interface(extra) = find_type(&bin, "IExtra") else {
            panic!("IExtra kind");
        };
        let rtti = bin.interface_rtti(&extra).expect("count");
        assert_eq!((rtti.method_count, rtti.methods.len()), (1, 0), "{b:?}");
    }
}

#[test]
fn deep_type_shapes() {
    for b in BUILDS {
        let data = b.sample("deep.exe");
        let bin = DelphiBinary::parse(&data).expect("parse");

        // An enumeration subrange takes its names from the base type.
        let TypeDetail::Enumeration(mid) = find_type(&bin, "TMid") else {
            panic!("TMid kind");
        };
        assert_eq!((mid.min, mid.max), (1, 3));
        assert_eq!(mid.values, vec![&b"lvLow"[..], b"lvMid", b"lvHigh"]);

        // Boolean-likes span their storage's signed range but list two
        // names; Delphi 6 writes no unit name for the compiler intrinsics,
        // Delphi 13 names System.
        let TypeDetail::Enumeration(byte_bool) = find_type(&bin, "ByteBool") else {
            panic!("ByteBool kind");
        };
        assert_eq!((byte_bool.min, byte_bool.max), (i32::MIN, i32::MAX));
        assert_eq!(byte_bool.values, vec![&b"False"[..], b"True"]);
        let unit = if b.modern() { Some("System") } else { None };
        assert_eq!(byte_bool.unit_name(), unit, "{b:?}");

        let TypeDetail::ShortString(name) = find_type(&bin, "TName") else {
            panic!("TName kind");
        };
        assert_eq!(name.max_length, 20);
        let TypeDetail::Int64(big) = find_type(&bin, "TBig") else {
            panic!("TBig kind");
        };
        assert_eq!(
            (big.min, big.max, big.unsigned),
            (-5_000_000_000, 5_000_000_000, false)
        );
        let TypeDetail::Ordinal(letter) = find_type(&bin, "TLetter") else {
            panic!("TLetter kind");
        };
        // `'a'..'z'`: an AnsiChar range in Delphi 6, a WideChar range from
        // 2009.
        let char_kind = if b.modern() {
            TypeKind::WChar
        } else {
            TypeKind::Char
        };
        assert_eq!(
            (letter.header.kind, letter.min, letter.max),
            (char_kind, 97, 122),
            "{b:?}"
        );
        let TypeDetail::Set(nibbles) = find_type(&bin, "TNibbles") else {
            panic!("TNibbles kind");
        };
        assert_eq!(nibbles.element_type.map(|h| h.name()), Some("TNibble"));

        // TArr = array[1..3] of string.
        let TypeDetail::Array(arr) = find_type(&bin, "TArr") else {
            panic!("TArr kind");
        };
        assert_eq!(
            (arr.array_size, arr.element_count),
            (3 * b.ptr() as u32, 3),
            "{b:?}"
        );
        assert_eq!(arr.element_type.map(|h| h.name()), Some(b.string()));
        let TypeDetail::Record(outer) = find_type(&bin, "TOuter") else {
            panic!("TOuter kind");
        };
        // Win64: TInner 16 bytes, a 24-byte Variant.
        let (size, managed_expected) = if b == Build::Delphi13Win64 {
            (
                72,
                vec![
                    (0, TypeKind::Record),
                    (16, TypeKind::DynArray),
                    (24, TypeKind::Variant),
                    (48, TypeKind::Array),
                ],
            )
        } else {
            (
                48,
                vec![
                    (0, TypeKind::Record),
                    (8, TypeKind::DynArray),
                    (16, TypeKind::Variant),
                    (32, TypeKind::Array),
                ],
            )
        };
        assert_eq!(outer.record_size, size, "{b:?}");
        let managed: Vec<(u64, TypeKind)> = outer
            .managed_fields
            .iter()
            .map(|f| (f.offset, f.field_type.expect("type").kind))
            .collect();
        assert_eq!(managed, managed_expected, "{b:?}");
        let TypeDetail::DynArray(recs) = find_type(&bin, "TRecArr") else {
            panic!("TRecArr kind");
        };
        assert_eq!(recs.elem_size, 2 * b.ptr() as u32, "{b:?}");
        assert_eq!(recs.element_type.map(|h| h.name()), Some("TInner"));

        assert_eq!(
            prop_types(&bin, "TOddProps"),
            pairs(&[
                ("ByteB", "ByteBool"),
                ("WordB", "WordBool"),
                ("LongB", "LongBool"),
                ("Name", "TName"),
                ("Big", "TBig"),
                ("Card", "Cardinal"),
                ("Letter", "TLetter"),
                ("Mid", "TMid"),
                ("Nibbles", "TNibbles"),
            ]),
            "{b:?}"
        );
    }
}

#[test]
fn extended_rtti_only_from_delphi_2010() {
    // Delphi 6 has no extended RTTI; Delphi 13 emits it for every class by
    // default (public and published properties, fields of every
    // visibility), so the form class shows its published properties
    // there too.
    for b in BUILDS {
        let data = b.sample("gtforms.exe");
        let bin = DelphiBinary::parse(&data).expect("parse");
        if !b.modern() {
            for c in bin.classes().iter() {
                assert!(bin.class_attributes(c).is_empty(), "{}", c.name());
                assert!(bin.extended_properties(c).is_empty(), "{}", c.name());
            }
        } else {
            let form = class(&bin, "TGTMainForm");
            assert!(!bin.extended_fields(form).is_empty(), "{b:?}");
        }
    }
}

#[test]
fn tobject_virtuals_in_the_header() {
    for b in BUILDS {
        let data = b.sample("tables.exe");
        let bin = DelphiBinary::parse(&data).expect("parse");
        let entries = b.map_entries("tables.map", bin.image_base().unwrap());
        let names_at = |va: u64| -> Vec<&str> {
            entries
                .iter()
                .filter(|(_, v)| *v == va)
                .map(|(n, _)| n.as_str())
                .collect()
        };
        // Delphi 6 keeps eight TObject virtuals at negative offsets,
        // Destroy nearest the class pointer; Delphi 2009 adds ToString,
        // GetHashCode and Equals beyond them, and Win64 puts the three
        // C++ ABI slots in front of Destroy.
        let mut expected: Vec<&str> = Vec::new();
        if b == Build::Delphi13Win64 {
            expected.extend(["CPP_ABI_3", "CPP_ABI_2", "CPP_ABI_1"]);
        }
        expected.extend([
            "Destroy",
            "FreeInstance",
            "NewInstance",
            "DefaultHandler",
            "Dispatch",
            "BeforeDestruction",
            "AfterConstruction",
            "SafeCallException",
        ]);
        if b.modern() {
            expected.extend(["ToString", "GetHashCode", "Equals"]);
        }
        let virtuals = bin.standard_virtuals(class(&bin, "TStringList"));
        let names: Vec<(Option<&str>, i64)> =
            virtuals.iter().map(|v| (v.name, v.vmt_offset)).collect();
        let want: Vec<(Option<&str>, i64)> = expected
            .iter()
            .zip(1i64..)
            .map(|(n, i)| (Some(*n), -i * b.ptr() as i64))
            .collect();
        assert_eq!(names, want, "{b:?}");
        // Every named slot of every class is the routine of that name.
        for c in bin.classes().iter() {
            for v in bin.standard_virtuals(c) {
                let name = v.name.expect("every header slot is named");
                assert!(
                    names_at(v.code_va)
                        .iter()
                        .any(|n| n.ends_with(&format!(".{name}"))),
                    "{b:?} {}.{name} at {:#x}",
                    c.name(),
                    v.code_va
                );
            }
        }
    }
}

#[test]
fn resource_strings_and_their_records() {
    for b in BUILDS {
        let data = b.sample("tables.exe");
        let bin = DelphiBinary::parse(&data).expect("parse");
        let strings = bin.resource_strings();
        // Every resourcestring the linked units declare.
        let count = match b {
            Build::Delphi6 => 117,
            Build::Delphi13Win32 => 211,
            Build::Delphi13Win64 => 212,
        };
        assert_eq!(strings.len(), count, "{b:?}");
        assert!(strings.iter().all(|s| s.record_va.is_some()));
        let ours: Vec<(&str, u64)> = strings
            .iter()
            .filter(|s| s.text.starts_with("undelphi"))
            .map(|s| (s.text.as_str(), s.record_va.unwrap()))
            .collect();
        assert_eq!(ours.len(), 2);
        assert_eq!(ours[0].0, "undelphi resourcestring alpha");
        assert_eq!(ours[1].0, "undelphi resourcestring beta");
        // Each TResStringRec is { &HInstance, Identifier }: the first
        // pointer is SysInit's HInstance, from the map's data publics.
        let hinstance = b.data_symbol("tables.map", "HInstance");
        let ctx = bin.ctx();
        for (_, va) in &ours {
            let off = ctx.va_to_file(*va).unwrap();
            let module = if b.ptr() == 8 {
                u64::from_le_bytes(data[off..off + 8].try_into().unwrap())
            } else {
                u64::from(u32::from_le_bytes(data[off..off + 4].try_into().unwrap()))
            };
            assert_eq!(module, hinstance, "{b:?} HInstance");
        }
    }
}

#[test]
fn package_info_flags() {
    for b in BUILDS {
        let data = b.sample("gtpkg.bpl");
        let pkg = DelphiBinary::parse(&data).expect("parse");
        let info = pkg.package_info().expect("PACKAGEINFO");
        // {$RUNONLY} and {$IMPLICITBUILD OFF}.
        assert_eq!(info.module_type(), ModuleType::Package);
        assert_eq!(info.producer(), Producer::Delphi);
        assert!(info.run_only() && info.never_build() && !info.design_only());
        let units: Vec<(&str, bool, bool)> = info
            .contains
            .iter()
            .map(|u| (u.name, u.is_main(), u.is_package()))
            .collect();
        assert_eq!(
            units,
            vec![
                ("gtpkg", true, true),
                ("gtshared", false, false),
                ("SysInit", false, false)
            ],
            "{b:?}"
        );

        let data = b.sample("known.exe");
        let exe = DelphiBinary::parse(&data).expect("parse");
        let info = exe.package_info().expect("PACKAGEINFO");
        let flags = if b.modern() { 0xCC10_0000 } else { 0xCC00_0000 };
        assert_eq!(info.flags, flags, "{b:?}");
        assert_eq!(info.module_type(), ModuleType::Program);
        let windows_name = if b.modern() {
            "Winapi.Windows"
        } else {
            "Windows"
        };
        let windows = info
            .contains
            .iter()
            .find(|u| u.name == windows_name)
            .unwrap();
        assert!(windows.is_weak() && windows.is_original_weak() && windows.is_implicit());
    }
}

impl Build {
    /// The VA of a public of any segment, for records that point at
    /// variables. A segment the map lists at base 0 (Delphi 6) starts at the
    /// PE section named after its class (`DATA`).
    fn data_symbol(self, name: &str, symbol: &str) -> u64 {
        let text = String::from_utf8_lossy(&self.sample(name)).replace('\r', "");
        let image = self.sample(name.replace(".map", ".exe").as_str());
        let pe = goblin::pe::PE::parse(&image).unwrap();
        let image_base = pe.image_base;
        let mut bases: HashMap<String, u64> = HashMap::new();
        for line in text.lines() {
            if line.contains("Detailed map of segments") {
                break;
            }
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 4 && parts[0].len() > 5 && parts[0].as_bytes()[4] == b':' {
                let mut base = u64::from_str_radix(&parts[0][5..], 16).unwrap();
                if base == 0
                    && let Some(section) =
                        pe.sections.iter().find(|s| s.name().ok() == Some(parts[3]))
                {
                    base = image_base + u64::from(section.virtual_address);
                }
                bases.entry(parts[0][..4].to_string()).or_insert(base);
            }
        }
        let mut in_publics = false;
        for line in text.lines() {
            if line.contains("Publics by Name") {
                in_publics = true;
                continue;
            }
            let line = line.trim();
            if !in_publics || line.len() < 6 {
                continue;
            }
            let (addr, sym) = line.split_once(char::is_whitespace).unwrap();
            let sym = sym.trim();
            if sym == symbol || sym.ends_with(&format!(".{symbol}")) {
                let base = bases[&addr[..4]];
                return base + u64::from_str_radix(&addr[5..], 16).unwrap();
            }
        }
        panic!("{symbol} not in {name}");
    }
}
