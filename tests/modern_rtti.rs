//! Golden tests for the Delphi 2010+ metadata of a compiled program.
//!
//! `tests/samples/d13/{win32,win64}/modernbin.exe` is compiled by Delphi 13
//! from `tests/fixtures/d13/src/modernbin.dpr` and `modernrt.pas` (built by
//! `tests/fixtures/d13/build.sh`): extended RTTI of every visibility,
//! attributes with arguments on a class, a field, a method, a property, a
//! record and a record field, an indexed property, records with methods
//! and operators, a managed record, generic instances, an invokable
//! interface, an anonymous-method type, a scoped enumeration, a class
//! constructor, string literals, a resourcestring and a delay-loaded
//! import. Every expectation comes from that source; code addresses come
//! from each build's linker map. The samples are committed and required.

#![allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::unwrap_used,
    clippy::arithmetic_side_effects
)]

use std::{collections::HashMap, fs, path::PathBuf};

use undelphi::{
    DelphiBinary,
    classes::Class,
    entrypoints::EntrypointKind,
    extents::StructureKind,
    extrtti::{AttributeArg, Visibility},
    rtti::{Pointee, RecordMethodKind, Storage, TypeDetail, TypeKind},
    signatures::{DispatchKind, MethodKind, SignatureReport},
};

/// The two builds: sample directory and pointer size.
const BUILDS: [(&str, u64); 2] = [("win32", 4), ("win64", 8)];

fn sample(platform: &str, name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/samples/d13")
        .join(platform)
        .join(name);
    fs::read(&path).unwrap_or_else(|e| panic!("{}: {e} (committed sample)", path.display()))
}

/// The map's publics by name (code and data), unit-qualified as Delphi 13
/// writes them (`modernrt.TWidget.Read`).
fn map(platform: &str) -> HashMap<String, u64> {
    let text = String::from_utf8_lossy(&sample(platform, "modernbin.map")).replace('\r', "");
    let mut bases: HashMap<String, u64> = HashMap::new();
    for line in text.lines() {
        if line.contains("Detailed map of segments") {
            break;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 4 && parts[0].len() > 5 && parts[0].as_bytes()[4] == b':' {
            bases.insert(
                parts[0][..4].to_string(),
                u64::from_str_radix(&parts[0][5..], 16).unwrap(),
            );
        }
    }
    let mut out = HashMap::new();
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
        let (addr, symbol) = line.split_once(char::is_whitespace).unwrap();
        if let Some(base) = bases.get(&addr[..4]) {
            out.insert(
                symbol.trim().to_string(),
                base + u64::from_str_radix(&addr[5..], 16).unwrap(),
            );
        }
    }
    out
}

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

/// `(attribute type, arguments)` for display-free comparison.
fn attr<'a>(
    bin: &DelphiBinary<'a>,
    entry: &undelphi::extrtti::AttributeEntry<'a>,
) -> (String, Vec<AttributeArg<'a>>) {
    let decoded = bin.decode_attribute(entry);
    (decoded.type_name.unwrap_or("?").to_string(), decoded.args)
}

/// `(name, kind, code VA, result type, parameter names)` of a record method.
type RecordMethodRow = (String, RecordMethodKind, u64, Option<String>, Vec<String>);

fn tag(name: &str, value: i64) -> (String, Vec<AttributeArg<'static>>) {
    (
        "TagAttribute".to_string(),
        vec![
            AttributeArg::String(name.to_string()),
            AttributeArg::Ordinal(value),
        ],
    )
}

fn mark() -> (String, Vec<AttributeArg<'static>>) {
    ("MarkAttribute".to_string(), Vec::new())
}

#[test]
fn compiler_and_platform() {
    for (platform, ptr) in BUILDS {
        let data = sample(platform, "modernbin.exe");
        let bin = DelphiBinary::parse(&data).expect("parse");
        let info = bin.compiler().expect("compiler");
        assert_eq!(info.version, Some("37.0"));
        assert_eq!(info.product_name().as_deref(), Some("Delphi 13 Florence"));
        assert_eq!(class(&bin, "TWidget").vmt.pointer_size as u64, ptr);
    }
}

#[test]
fn class_fields_of_every_visibility() {
    for (platform, ptr) in BUILDS {
        let data = sample(platform, "modernbin.exe");
        let bin = DelphiBinary::parse(&data).expect("parse");
        let widget = class(&bin, "TWidget");
        // TInterfacedObject's own fields (12 / 24 bytes), then FCount ..
        // FRatio in declaration order; TPoint3 is 40 bytes, a set of three
        // values one byte, Double 8-aligned. The class var FInstances is no
        // instance field.
        let base = 3 * ptr;
        let fields: Vec<(String, Visibility, u32, Option<String>)> = bin
            .extended_fields(widget)
            .iter()
            .map(|f| {
                (
                    f.name().to_string(),
                    f.visibility,
                    f.offset,
                    bin.extended_field_type(widget, f)
                        .map(|h| h.name().to_string()),
                )
            })
            .collect();
        let o = |n: u64| u32::try_from(n).unwrap();
        let expected: Vec<(String, Visibility, u32, Option<String>)> = [
            ("FCount", Visibility::Private, base, "Integer"),
            ("FName", Visibility::Private, base + ptr, "string"),
            (
                "FItems",
                Visibility::Private,
                base + 2 * ptr,
                "TList<System.Integer>",
            ),
            ("FPoint", Visibility::Private, base + 3 * ptr, "TPoint3"),
            ("FLevel", Visibility::Private, base + 3 * ptr + 40, "TLevel"),
            (
                "FLevels",
                Visibility::Private,
                base + 3 * ptr + 41,
                "TLevels",
            ),
            (
                "FRatio",
                Visibility::Protected,
                base + 3 * ptr + 48,
                "Double",
            ),
        ]
        .iter()
        .map(|(n, v, off, t)| ((*n).to_string(), *v, o(*off), Some((*t).to_string())))
        .collect();
        assert_eq!(fields, expected, "{platform}");

        // [Mark] FCount.
        let fcount = &bin.extended_fields(widget)[0];
        let attrs: Vec<_> = fcount.attributes.iter().map(|a| attr(&bin, a)).collect();
        assert_eq!(attrs, vec![mark()], "{platform}");
    }
}

#[test]
fn class_attributes_with_arguments() {
    for (platform, _) in BUILDS {
        let data = sample(platform, "modernbin.exe");
        let bin = DelphiBinary::parse(&data).expect("parse");
        let map = map(platform);
        let widget = class(&bin, "TWidget");
        let attrs = bin.class_attributes(widget);
        assert_eq!(
            attrs.iter().map(|a| attr(&bin, a)).collect::<Vec<_>>(),
            vec![tag("class", 42)],
            "{platform}"
        );
        // The constructor is TagAttribute.Create.
        let decoded = bin.decode_attribute(&attrs[0]);
        assert_eq!(decoded.constructor, map["modernrt.TagAttribute.Create"]);
        assert_eq!(decoded.constructor_name, Some("Create"));

        // [Tag('prop', 9)] Name; the other properties carry none.
        let props: Vec<(String, Visibility, Vec<_>)> = bin
            .extended_properties(widget)
            .iter()
            .map(|p| {
                (
                    p.name().to_string(),
                    p.visibility,
                    p.attributes.iter().map(|a| attr(&bin, a)).collect(),
                )
            })
            .collect();
        assert_eq!(
            props,
            vec![
                ("Point".to_string(), Visibility::Public, vec![]),
                (
                    "Name".to_string(),
                    Visibility::Published,
                    vec![tag("prop", 9)]
                ),
                ("Count".to_string(), Visibility::Published, vec![]),
                ("Level".to_string(), Visibility::Published, vec![]),
                ("Levels".to_string(), Visibility::Published, vec![]),
            ],
            "{platform}"
        );
    }
}

#[test]
fn indexed_property_and_its_accessors() {
    for (platform, _) in BUILDS {
        let data = sample(platform, "modernbin.exe");
        let bin = DelphiBinary::parse(&data).expect("parse");
        let map = map(platform);
        let widget = class(&bin, "TWidget");
        // property Items[Index: Integer]: Integer read GetItem write
        // SetItem; default.
        let indexed = bin.indexed_properties(widget);
        assert_eq!(indexed.len(), 1, "{platform}");
        let items = &indexed[0];
        assert_eq!(items.name(), "Items");
        assert!(items.is_default());
        assert_eq!(items.visibility(), Visibility::Public);
        let methods = bin.extended_methods(widget);
        let getter = items.getter(&methods).unwrap();
        let setter = items.setter(&methods).unwrap();
        assert_eq!(
            (getter.name(), getter.code_va),
            ("GetItem", map["modernrt.TWidget.GetItem"])
        );
        assert_eq!(
            (setter.name(), setter.code_va),
            ("SetItem", map["modernrt.TWidget.SetItem"])
        );
    }
}

#[test]
fn methods_of_every_visibility_and_kind() {
    for (platform, ptr) in BUILDS {
        let data = sample(platform, "modernbin.exe");
        let bin = DelphiBinary::parse(&data).expect("parse");
        let map = map(platform);
        let widget = class(&bin, "TWidget");
        let rows: Vec<(String, u64, Visibility, DispatchKind, bool)> = bin
            .extended_methods(widget)
            .iter()
            .map(|m| {
                let i = m.info();
                (
                    m.name().to_string(),
                    m.code_va,
                    i.visibility(),
                    i.dispatch(),
                    i.is_static(),
                )
            })
            .collect();
        let at = |n: &str| map[&format!("modernrt.TWidget.{n}")];
        let expected: Vec<(String, u64, Visibility, DispatchKind, bool)> = [
            ("GetItem", Visibility::Private, DispatchKind::Static, false),
            ("SetItem", Visibility::Private, DispatchKind::Static, false),
            (
                "DoChange",
                Visibility::Protected,
                DispatchKind::Virtual,
                false,
            ),
            ("Create", Visibility::Public, DispatchKind::Static, false),
            ("Destroy", Visibility::Public, DispatchKind::Virtual, false),
            ("Read", Visibility::Public, DispatchKind::Static, false),
            ("Reset", Visibility::Public, DispatchKind::Static, false),
            ("Each", Visibility::Public, DispatchKind::Static, false),
            ("Instances", Visibility::Public, DispatchKind::Static, true),
            (
                "Changed",
                Visibility::Published,
                DispatchKind::Static,
                false,
            ),
        ]
        .iter()
        .map(|(n, v, d, s)| ((*n).to_string(), at(n), *v, *d, *s))
        .collect();
        assert_eq!(rows, expected, "{platform}");

        // DoChange is the class's first new virtual; Destroy overrides the
        // header slot the VMT names (Win64: behind the three C++ ABI slots).
        let ext = bin.extended_methods(widget);
        assert_eq!(ext[2].vmt_index, 0);
        let destroy_slot = if ptr == 8 { -4 } else { -1 };
        assert_eq!(ext[4].vmt_index, destroy_slot, "{platform}");

        let SignatureReport::Decoded(sigs) = bin.method_signatures(widget) else {
            panic!("signatures");
        };
        let kinds: Vec<(Option<&str>, MethodKind)> =
            sigs.iter().map(|s| (s.name, s.kind)).collect();
        assert_eq!(
            kinds,
            vec![
                (Some("GetItem"), MethodKind::Function),
                (Some("SetItem"), MethodKind::Procedure),
                (Some("DoChange"), MethodKind::Procedure),
                (Some("Create"), MethodKind::Constructor),
                (Some("Destroy"), MethodKind::Destructor),
                (Some("Read"), MethodKind::Function),
                (Some("Reset"), MethodKind::Procedure),
                (Some("Each"), MethodKind::Procedure),
                (Some("Instances"), MethodKind::ClassFunction),
                (Some("Changed"), MethodKind::Procedure),
            ],
            "{platform}"
        );
        // [Tag('method', 3)] Read(Count: Integer; out Data: TBytes).
        let read = &sigs[5];
        assert_eq!(
            read.attributes
                .iter()
                .map(|a| attr(&bin, a))
                .collect::<Vec<_>>(),
            vec![tag("method", 3)]
        );
        assert_eq!(
            bin.decode_attribute(&read.attributes[0]).to_string(),
            "[TagAttribute('method', 3)]"
        );
        let params: Vec<(Option<&str>, Option<&str>)> =
            read.params.iter().map(|p| (p.name, p.type_name)).collect();
        assert_eq!(
            params,
            vec![
                (Some("Self"), Some("TWidget")),
                (Some("Count"), Some("Integer")),
                (Some("Data"), Some("TArray<System.Byte>")),
            ],
            "{platform}"
        );
        // A static class method has no Self.
        assert!(sigs[8].params.is_empty());
        // The declared parameters leave the implicit Self out, and only it.
        let declared: Vec<Option<&str>> = read.declared_params().map(|p| p.name).collect();
        assert_eq!(declared, vec![Some("Count"), Some("Data")], "{platform}");
        assert!(read.has_self() && !sigs[8].has_self(), "{platform}");
        assert_eq!(sigs[8].declared_params().count(), 0, "{platform}");
    }
}

#[test]
fn generic_instances() {
    for (platform, ptr) in BUILDS {
        let data = sample(platform, "modernbin.exe");
        let bin = DelphiBinary::parse(&data).expect("parse");
        let map = map(platform);
        for (name, field_type) in [
            ("TBox<System.Integer>", "Integer"),
            ("TBox<System.string>", "string"),
        ] {
            let c = class(&bin, name);
            assert_eq!(bin.parent_class(c).map(Class::name), Some("TObject"));
            let fields: Vec<(String, u32, Option<String>)> = bin
                .extended_fields(c)
                .iter()
                .map(|f| {
                    (
                        f.name().to_string(),
                        f.offset,
                        bin.extended_field_type(c, f).map(|h| h.name().to_string()),
                    )
                })
                .collect();
            assert_eq!(
                fields,
                vec![(
                    "FValue".to_string(),
                    u32::try_from(ptr).unwrap(),
                    Some(field_type.to_string())
                )],
                "{platform} {name}"
            );
        }
        // A string function returns through a hidden parameter: pfResult,
        // which the compiler names #1.
        let SignatureReport::Decoded(sigs) =
            bin.method_signatures(class(&bin, "TBox<System.string>"))
        else {
            panic!("signatures");
        };
        let get = sigs.iter().find(|s| s.name == Some("Get")).unwrap();
        assert_eq!(
            get.code_va,
            Some(map["modernrt.{modernrt}TBox<System.string>.Get"])
        );
        let hidden: Vec<bool> = get.params.iter().map(|p| p.flags.is_hidden()).collect();
        assert_eq!(hidden, vec![false, true], "{platform}");
        assert_eq!(get.params[1].name, Some("\u{1}"));
    }
}

#[test]
fn records_with_methods_operators_and_attributes() {
    for (platform, ptr) in BUILDS {
        let data = sample(platform, "modernbin.exe");
        let bin = DelphiBinary::parse(&data).expect("parse");
        let map = map(platform);

        let TypeDetail::Record(point) = find_type(&bin, "TPoint3") else {
            panic!("TPoint3 kind");
        };
        assert_eq!(point.record_size, 40);
        // A record's RTTI names no unit; the 2010+ type table does.
        assert_eq!(
            bin.declaring_units().get(&point.header.va),
            Some(&"modernrt"),
            "{platform}"
        );
        let fields: Vec<(String, u32, Visibility)> = point
            .fields
            .iter()
            .map(|f| (f.name().to_string(), f.offset, f.visibility()))
            .collect();
        assert_eq!(
            fields,
            vec![
                ("FTag".to_string(), 0, Visibility::Private),
                ("X".to_string(), 8, Visibility::Public),
                ("Y".to_string(), 16, Visibility::Public),
                ("Z".to_string(), 24, Visibility::Public),
                ("Name".to_string(), 32, Visibility::Public),
            ],
            "{platform}"
        );
        let methods: Vec<RecordMethodRow> = point
            .methods
            .iter()
            .map(|m| {
                let sig = m.signature.as_ref().unwrap();
                (
                    m.name().to_string(),
                    m.kind(),
                    m.code_va,
                    sig.result_type.map(|h| h.name().to_string()),
                    sig.params.iter().map(|p| p.name().to_string()).collect(),
                )
            })
            .collect();
        let at = |n: &str| map[&format!("modernrt.TPoint3.{n}")];
        let names = |list: &[&str]| list.iter().map(|s| (*s).to_string()).collect::<Vec<_>>();
        // Each record method's kind as a signature names it, and its Self.
        let kinds: Vec<(&str, MethodKind, bool)> = point
            .methods
            .iter()
            .map(|m| (m.name(), m.method_kind(), m.has_self()))
            .collect();
        assert!(
            kinds.contains(&("Create", MethodKind::Constructor, true)),
            "{kinds:?}"
        );
        assert!(
            kinds.contains(&("Length", MethodKind::Function, true)),
            "{kinds:?}"
        );
        assert!(
            kinds.contains(&("&op_Addition", MethodKind::OperatorOverload, false)),
            "{kinds:?}"
        );
        assert_eq!(
            methods,
            vec![
                (
                    "Create".to_string(),
                    RecordMethodKind::Constructor,
                    at("Create"),
                    None,
                    names(&["AX", "AY", "AZ"])
                ),
                (
                    "Length".to_string(),
                    RecordMethodKind::Method,
                    at("Length"),
                    Some("Double".to_string()),
                    vec![]
                ),
                (
                    "&op_Addition".to_string(),
                    RecordMethodKind::Operator,
                    at("&op_Addition"),
                    Some("TPoint3".to_string()),
                    names(&["A", "B"])
                ),
            ],
            "{platform}"
        );
        assert!(point.operators.is_none());

        // [Tag('rec', 7)] TTaggedRec = ([Mark] A: Integer; B: string).
        let TypeDetail::Record(tagged) = find_type(&bin, "TTaggedRec") else {
            panic!("TTaggedRec kind");
        };
        assert_eq!(tagged.record_size, 2 * u32::try_from(ptr).unwrap());
        assert_eq!(
            tagged
                .attributes
                .iter()
                .map(|a| attr(&bin, a))
                .collect::<Vec<_>>(),
            vec![tag("rec", 7)],
            "{platform}"
        );
        let field_attrs: Vec<Vec<_>> = tagged
            .fields
            .iter()
            .map(|f| f.attributes.iter().map(|a| attr(&bin, a)).collect())
            .collect();
        assert_eq!(field_attrs, vec![vec![mark()], vec![]]);

        // A managed record: tkMRecord with Initialize / Finalize in its
        // operator table.
        let TypeDetail::Record(managed) = find_type(&bin, "TManaged") else {
            panic!("TManaged kind");
        };
        assert_eq!(managed.header.kind, TypeKind::MRecord);
        let ops = managed.operators.expect("operator table");
        assert_eq!(
            (ops.flags, ops.initialize, ops.finalize, ops.assign),
            (
                0,
                Some(map["modernrt.TManaged.&op_Initialize"]),
                Some(map["modernrt.TManaged.&op_Finalize"]),
                None
            ),
            "{platform}"
        );
    }
}

#[test]
fn enums_sets_interfaces_and_method_references() {
    for (platform, _) in BUILDS {
        let data = sample(platform, "modernbin.exe");
        let bin = DelphiBinary::parse(&data).expect("parse");
        // A scoped enumeration keeps its plain names.
        let TypeDetail::Enumeration(level) = find_type(&bin, "TLevel") else {
            panic!("TLevel kind");
        };
        assert_eq!(level.values, vec![&b"Low"[..], b"Mid", b"High"]);
        assert_eq!(level.unit_name(), Some("modernrt"));
        let TypeDetail::Set(levels) = find_type(&bin, "TLevels") else {
            panic!("TLevels kind");
        };
        assert_eq!(levels.element_type.map(|h| h.name()), Some("TLevel"));

        let TypeDetail::Interface(source) = find_type(&bin, "ISource") else {
            panic!("ISource kind");
        };
        assert_eq!(
            source.guid.to_string(),
            "{3C4D5E6F-7081-4293-A4B5-C6D7E8F90A1B}"
        );
        let rtti = bin.interface_rtti(&source).expect("method info");
        let names: Vec<&str> = rtti.methods.iter().map(|m| m.name()).collect();
        assert_eq!(names, vec!["Read", "Reset"], "{platform}");

        // `reference to procedure` is an interface type.
        assert_eq!(
            find_type(&bin, "TNotify").header().kind,
            TypeKind::Interface
        );
    }
}

/// The storage of the RTL's scalar and string types, on both platforms:
/// `Extended` is 10 bytes on Win32 and a `Double` on Win64, a class is a
/// pointer to its instance, and a record has none (it is described by its
/// declaration).
#[test]
fn types_report_their_storage() {
    for (platform, ptr) in BUILDS {
        let data = sample(platform, "modernbin.exe");
        let bin = DelphiBinary::parse(&data).expect("parse");
        let storage = |name: &str| find_type(&bin, name).storage(ptr);
        assert_eq!(
            storage("Integer"),
            Some(Storage::Int {
                bytes: 4,
                signed: true
            }),
            "{platform}"
        );
        assert_eq!(
            storage("Cardinal"),
            Some(Storage::Int {
                bytes: 4,
                signed: false
            }),
            "{platform}"
        );
        assert_eq!(
            storage("Int64"),
            Some(Storage::Int {
                bytes: 8,
                signed: true
            }),
            "{platform}"
        );
        assert_eq!(storage("Boolean"), Some(Storage::Bool), "{platform}");
        assert_eq!(
            storage("Char"),
            Some(Storage::Char { bytes: 2 }),
            "{platform}"
        );
        assert_eq!(
            storage("string"),
            Some(Storage::Pointer(Pointee::Chars { bytes: 2 })),
            "{platform}"
        );
        assert_eq!(
            storage("Double"),
            Some(Storage::Float { bytes: 8 }),
            "{platform}"
        );
        assert_eq!(
            storage("TObject"),
            Some(Storage::Pointer(Pointee::Instance)),
            "{platform}"
        );
        assert_eq!(
            storage("ISource"),
            Some(Storage::Pointer(Pointee::Interface)),
            "{platform}"
        );
        assert!(matches!(
            find_type(&bin, "TLevel"),
            TypeDetail::Enumeration(_)
        ));
        assert_eq!(
            storage("TLevels"),
            None,
            "{platform}: a set is described by its declaration"
        );
    }
}

/// The code entry points name a class's non-published methods from its
/// extended RTTI and a record's methods from the record's, at the addresses
/// the linker map gives them; an abstract method is not labelled.
#[test]
fn entrypoints_name_extended_and_record_methods() {
    for (platform, _) in BUILDS {
        let data = sample(platform, "modernbin.exe");
        let bin = DelphiBinary::parse(&data).expect("parse");
        let map = map(platform);
        let entries = bin.code_entrypoints();
        let labelled = |kind: EntrypointKind, label: &str| {
            entries
                .iter()
                .find(|e| e.kind == kind && e.name_hint == label)
                .map(|e| e.va)
        };
        for name in ["GetItem", "SetItem", "DoChange", "Read", "Each"] {
            assert_eq!(
                labelled(EntrypointKind::ExtendedMethod, &format!("TWidget.{name}")),
                Some(map[&format!("modernrt.TWidget.{name}")]),
                "{platform} {name}"
            );
        }
        for name in ["Create", "Length", "&op_Addition"] {
            assert_eq!(
                labelled(EntrypointKind::RecordMethod, &format!("TPoint3.{name}")),
                Some(map[&format!("modernrt.TPoint3.{name}")]),
                "{platform} {name}"
            );
        }
    }
}

#[test]
fn class_constructor_runs_from_the_init_table() {
    for (platform, _) in BUILDS {
        let data = sample(platform, "modernbin.exe");
        let bin = DelphiBinary::parse(&data).expect("parse");
        let map = map(platform);
        let table = bin.unit_init_table().expect("init table");
        let pair = (
            Some(map["modernrt.TWidget.CreateClass@"]),
            Some(map["modernrt.TWidget.DestroyClass@"]),
        );
        assert!(
            table
                .units
                .iter()
                .any(|u| (u.init_va, u.finalize_va) == pair),
            "{platform}"
        );
    }
}

#[test]
fn literals_resourcestrings_and_delayed_imports() {
    for (platform, _) in BUILDS {
        let data = sample(platform, "modernbin.exe");
        let bin = DelphiBinary::parse(&data).expect("parse");
        let strings: Vec<String> = bin
            .resource_strings()
            .into_iter()
            .map(|s| s.text)
            .filter(|t| t.contains("Widget"))
            .collect();
        assert_eq!(strings, vec!["Widget missing"], "{platform}");

        // `external 'kernel32.dll' name 'GetTickCount' delayed`.
        let delayed: Vec<(&str, &str)> = bin
            .imports()
            .iter()
            .filter(|i| i.is_delayed())
            .map(|i| (i.module(), i.name()))
            .collect();
        assert!(
            delayed.contains(&("kernel32.dll", "GetTickCount")),
            "{platform}: {delayed:?}"
        );
        assert!(
            bin.imports()
                .iter()
                .filter(|i| i.name() == "GetTickCount")
                .any(|i| !i.is_delayed()),
            "System's own GetTickCount import is an ordinary one"
        );

        // The UTF-16 literal 'unicode literal λ' as a string constant.
        let wanted: Vec<u8> = "unicode literal λ"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        let ctx = bin.ctx();
        let found = bin.structure_extents().into_iter().any(|e| {
            e.kind == StructureKind::StringLiteral
                && ctx.va_to_file(e.extent.start).is_some_and(|off| {
                    let len = usize::try_from(e.extent.end - e.extent.start).unwrap();
                    data[off..off + len]
                        .windows(wanted.len())
                        .any(|w| w == wanted.as_slice())
                })
        });
        assert!(found, "{platform}");
    }
}

#[test]
fn attribute_arguments_of_every_kind() {
    for (platform, _) in BUILDS {
        let data = sample(platform, "modernbin.exe");
        let bin = DelphiBinary::parse(&data).expect("parse");
        let holder = class(&bin, "TArgHolder");
        let attrs = bin.class_attributes(holder);
        assert_eq!(attrs.len(), 1);
        let decoded = bin.decode_attribute(&attrs[0]);
        assert_eq!(decoded.type_name, Some("ArgsAttribute"));
        // As written in modernrt.pas; Win32 stores Extended in 80 bits,
        // Win64 as a Double, both decode to -1.5.
        let args = decoded.args;
        let expected = vec![
            AttributeArg::Ordinal(200),
            AttributeArg::Ordinal(-100),
            AttributeArg::Ordinal(60_000),
            AttributeArg::Ordinal(-30_000),
            AttributeArg::Ordinal(4_000_000_000),
            AttributeArg::Ordinal(-2_000_000_000),
            AttributeArg::Ordinal(-5_000_000_000),
            AttributeArg::Ordinal(i64::from(b'Z')),
            AttributeArg::Ordinal(i64::from(b'q')),
            AttributeArg::Ordinal(1),
            AttributeArg::Ordinal(2),
            AttributeArg::Set(vec![0, 2]),
            AttributeArg::Set(vec![3, 64, 200]),
            AttributeArg::Float(0.75),
            AttributeArg::Float(2.5),
            AttributeArg::Float(-1.5),
            AttributeArg::Float(12.3456),
            AttributeArg::String("unicode \u{3bb}".to_string()),
            AttributeArg::String("ansi".to_string()),
            AttributeArg::String("utf8 \u{3bb}".to_string()),
            AttributeArg::String("wide".to_string()),
            AttributeArg::ShortString(b"short"),
        ];
        assert_eq!(args[..expected.len()], expected[..], "{platform}");
        let names: Vec<Option<&str>> = args[expected.len()..]
            .iter()
            .map(|a| match a {
                AttributeArg::Reference { type_name, .. } => *type_name,
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(names, vec![Some("TObject"), Some("TPoint3")], "{platform}");

        // `set of 0..100` has no RTTI, so its constructor has no signature:
        // the arguments stay raw (the 13-byte set).
        let holder = class(&bin, "TDigitsHolder");
        let decoded = bin.decode_attribute(&bin.class_attributes(holder)[0]);
        assert_eq!(decoded.type_name, Some("DigitsAttribute"));
        let [AttributeArg::Raw(raw)] = decoded.args.as_slice() else {
            panic!("{platform}: {:?}", decoded.args);
        };
        assert_eq!(raw.len(), 13, "{platform}");
    }
}
