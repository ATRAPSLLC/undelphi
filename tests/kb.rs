//! IDR knowledge bases: every published file (when present under
//! `tests/samples/idr`, see `SAMPLES.md`) decodes record by record, its
//! lookups find what a scan finds, and writing its contents back
//! reproduces the file; a knowledge base built from the Delphi 6 library
//! (when `UNDELPHI_BCB6` names a C++Builder 6 install) reproduces kb6's
//! records.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::{
    collections::HashMap,
    env, fs,
    path::{Path, PathBuf},
};

use undelphi::{
    dcu::library::Library,
    kb::{
        self, CallKind, Contents, Dump, Fixup, FixupKind, Format, KnowledgeBase, Module, Proc,
        ProcKind, Record, Section, Text, UnitFile, cmp_ignore_case,
    },
};

/// Every published knowledge base present locally, smallest first.
fn published() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/samples/idr");
    let mut files: Vec<PathBuf> = ["win32", "win64"]
        .iter()
        .filter_map(|d| fs::read_dir(root.join(d)).ok())
        .flatten()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "bin"))
        .collect();
    files.sort_by_key(|p| fs::metadata(p).map(|m| m.len()).unwrap_or(0));
    files
}

/// The section tables as stored: `(max_size, [(offset, size)])` each.
fn raw_tables(data: &[u8]) -> Vec<(u32, Vec<(u32, u32)>)> {
    let u32_at = |at: usize| u32::from_le_bytes(data[at..at + 4].try_into().unwrap());
    let mut at = u32_at(data.len() - 4) as usize;
    (0..6)
        .map(|_| {
            let count = u32_at(at) as usize;
            let max = u32_at(at + 4);
            let entries = (0..count)
                .map(|i| (u32_at(at + 8 + 16 * i), u32_at(at + 12 + 16 * i)))
                .collect();
            at += 8 + 16 * count;
            (max, entries)
        })
        .collect()
}

/// The keys of a section's two orders: module IDs and uppercased names
/// (the Delphi 2 files order a module's routines by file position, so the
/// module order is compared by module alone).
fn order_keys<'a, R: Record<'a>>(s: &Section<'_, 'a, R>, format: Format) -> [Vec<Vec<u8>>; 2] {
    let module = |i: usize| {
        let bytes = s.bytes(i).unwrap();
        R::module_of(bytes, format).unwrap().to_be_bytes().to_vec()
    };
    let name = |i: usize| {
        let bytes = s.bytes(i).unwrap();
        R::name_of(bytes, format).unwrap().to_ascii_uppercase()
    };
    [
        s.module_order().map(module).collect(),
        s.name_order().map(name).collect(),
    ]
}

/// Asserts both orders of each section of `a` and `b` have the same keys
/// position by position (records with equal keys may be swapped).
fn assert_same_orders(a: &KnowledgeBase<'_>, b: &KnowledgeBase<'_>, what: &str) {
    let f = a.header().format;
    assert_eq!(
        order_keys(&a.modules(), f),
        order_keys(&b.modules(), f),
        "{what} modules"
    );
    assert_eq!(
        order_keys(&a.consts(), f),
        order_keys(&b.consts(), f),
        "{what} consts"
    );
    assert_eq!(
        order_keys(&a.types(), f),
        order_keys(&b.types(), f),
        "{what} types"
    );
    assert_eq!(
        order_keys(&a.vars(), f),
        order_keys(&b.vars(), f),
        "{what} vars"
    );
    assert_eq!(
        order_keys(&a.res_strings(), f),
        order_keys(&b.res_strings(), f),
        "{what} resource strings"
    );
    assert_eq!(
        order_keys(&a.procs(), f),
        order_keys(&b.procs(), f),
        "{what} procs"
    );
}

/// Asserts each order is sorted by its key.
fn assert_sorted<'a, R: Record<'a>>(s: &Section<'_, 'a, R>, format: Format, what: &str) {
    let [by_module, by_name] = order_keys(s, format);
    assert!(by_module.is_sorted(), "{what}: module order");
    assert!(
        by_name
            .windows(2)
            .all(|w| cmp_ignore_case(&w[0], &w[1]).is_le()),
        "{what}: name order"
    );
}

#[test]
fn published_knowledge_bases_round_trip() {
    let files = published();
    if files.is_empty() {
        eprintln!("skipping: no knowledge bases under tests/samples/idr");
        return;
    }
    for path in files {
        let what = path.file_name().unwrap().to_string_lossy().into_owned();
        let data = fs::read(&path).unwrap();
        let kb = KnowledgeBase::parse(&data).unwrap_or_else(|e| panic!("{what}: {e}"));
        assert_eq!(kb.header().format, Format::Idr, "{what}");
        let f = kb.header().format;
        assert_sorted(&kb.modules(), f, &what);
        assert_sorted(&kb.consts(), f, &what);
        assert_sorted(&kb.types(), f, &what);
        assert_sorted(&kb.vars(), f, &what);
        assert_sorted(&kb.res_strings(), f, &what);
        assert_sorted(&kb.procs(), f, &what);

        let contents = kb.contents().unwrap_or_else(|e| panic!("{what}: {e}"));
        let written = contents.to_bytes().unwrap();
        // Header and records byte for byte, then the tables: the same
        // spans, and orders that agree up to records with equal keys.
        let tables = u32::from_le_bytes(data[data.len() - 4..].try_into().unwrap()) as usize;
        assert_eq!(written.len(), data.len(), "{what}: size");
        let first_diff = written[..tables]
            .iter()
            .zip(&data[..tables])
            .position(|(a, b)| a != b);
        assert_eq!(first_diff, None, "{what}: header and records differ");
        assert_eq!(raw_tables(&written), raw_tables(&data), "{what}: tables");
        let reread = KnowledgeBase::parse(&written).unwrap();
        assert_same_orders(&kb, &reread, &what);
        assert_eq!(reread.contents().unwrap(), contents, "{what}: contents");
        // The lookup orders come out as IDR's maker sorted them, so the
        // file is reproduced byte for byte - except the Delphi 2 files,
        // whose maker kept a unit's routines in file order.
        if !what.ends_with("kb2.bin") {
            assert!(written == data, "{what}: not byte-identical");
        }
    }
}

#[test]
fn kb6_lookups() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/samples/idr/win32/kb6.bin");
    let Ok(data) = fs::read(&path) else {
        eprintln!("skipping: kb6.bin missing");
        return;
    };
    let kb = KnowledgeBase::parse(&data).unwrap();
    assert_eq!(kb.modules().len(), 531);
    assert_eq!(kb.procs().len(), 37_804);

    let sysutils = kb.module_named("SYSUTILS").unwrap().expect("SysUtils");
    assert_eq!(sysutils.file_name.as_str(), Some("SysUtils.dcu"));
    assert!(
        sysutils
            .uses
            .iter()
            .any(|u| u.name.as_str() == Some("SysConst"))
    );
    assert_eq!(kb.module(sysutils.id).unwrap().unwrap(), sysutils);

    // The two IntToStr overloads (Integer, Int64), both in SysUtils.
    let procs = kb.procs();
    let found = procs.named(b"inttostr");
    assert_eq!(found.len(), 2);
    let int_to_str: Vec<Proc<'_>> = found.iter().map(|&i| procs.get(i).unwrap()).collect();
    for p in &int_to_str {
        assert_eq!(p.module, sysutils.id);
        assert_eq!(p.kind, ProcKind::Function);
        assert_eq!(p.call, CallKind::Register);
        assert_eq!(p.type_def.as_str(), Some("System.AnsiString"));
        assert_eq!(p.vproc & 0x1000, 0x1000, "overload");
        assert_eq!(p.args.len(), 1);
        // The tail of the dump is the "%d" format constant, referred to
        // from the code by an address fixup into the routine itself.
        assert_eq!(&p.dump.code[p.dump.code.len() - 4..], b"%d\0\0");
        assert!(p.dump.matches(&p.dump.code));
    }
    let types: Vec<&str> = int_to_str
        .iter()
        .map(|p| p.args[0].type_def.as_str().unwrap())
        .collect();
    assert!(types.contains(&"System.Integer") && types.contains(&"System.Int64"));

    // Lookups by module agree with a scan.
    let by_module = procs.of_module(sysutils.id);
    let scanned = (0..procs.len())
        .filter(|&i| procs.get(i).unwrap().module == sysutils.id)
        .count();
    assert_eq!(by_module.len(), scanned);

    let types = kb.types();
    let list = types.get(types.named(b"TList")[0]).unwrap();
    assert_eq!(list.decl.as_str(), Some("class(System.TObject)"));
    assert_eq!(list.vmt_count, 4);
    assert!(
        list.methods
            .iter()
            .any(|m| m.prototype.to_string().starts_with("function Add("))
    );
}

#[test]
fn dump_matching_ignores_relocated_bytes() {
    let dump = Dump::new(
        vec![0xE8, 0, 0, 0, 0, 0xC3].into(),
        vec![0, 0xFF, 0xFF, 0xFF, 0xFF, 0].into(),
        vec![Fixup {
            kind: FixupKind::Jump,
            offset: 1,
            name: Text::from("Target"),
        }],
    )
    .unwrap();
    assert!(dump.matches(&[0xE8, 0x12, 0x34, 0x56, 0x78, 0xC3, 0x90]));
    assert!(!dump.matches(&[0xE9, 0x12, 0x34, 0x56, 0x78, 0xC3]));
    assert!(!dump.matches(&[0xE8, 0x12]));
}

#[test]
fn built_contents_write_and_read_back() {
    let module = |id: u16, name: &'static str| Module {
        id,
        name: Text::from(name),
        file_name: Text::from(format!("{name}.dcu")),
        uses: Vec::new(),
    };
    let proc = |module: u16, name: &'static str| Proc {
        module,
        name: Text::from(name),
        embedded: false,
        dump_type: undelphi::kb::DumpType::Code,
        kind: ProcKind::Procedure,
        call: CallKind::Stdcall,
        vproc: 0,
        type_def: Text::default(),
        dump: Dump::default(),
        args: Vec::new(),
        locals: None,
    };
    let contents = Contents {
        modules: vec![module(0, "Zeta"), module(1, "alpha")],
        procs: vec![proc(1, "b"), proc(0, "aB"), proc(1, "A_")],
        ..Contents::default()
    };
    let bytes = contents.to_bytes().unwrap();
    let kb = KnowledgeBase::parse(&bytes).unwrap();
    assert_eq!(kb.contents().unwrap(), contents);
    assert_eq!(kb.module_named("ALPHA").unwrap().unwrap().id, 1);
    // `_` sorts after the letters once uppercased.
    let names: Vec<String> = kb
        .procs()
        .name_order()
        .map(|i| kb.procs().get(i).unwrap().name.to_string())
        .collect();
    assert_eq!(names, ["aB", "A_", "b"]);
    assert_eq!(kb.procs().of_module(1), [2, 0]);
    assert!(kb.procs().named(b"missing").is_empty());
}

/// Pairs the records of a section by (unit, name, occurrence) and counts
/// the generated ones identical to the reference, module IDs compared by
/// the units they stand for.
fn identical<'a, R: Record<'a> + Clone + PartialEq>(
    reference: &Section<'_, 'a, R>,
    generated: &Section<'_, 'a, R>,
    units: (&HashMap<u16, String>, &HashMap<u16, String>),
    neutral: impl Fn(&mut R),
) -> (usize, usize) {
    let keyed = |s: &Section<'_, 'a, R>, names: &HashMap<u16, String>| {
        let mut seen: HashMap<(String, Vec<u8>), usize> = HashMap::new();
        let mut out: HashMap<(String, Vec<u8>, usize), R> = HashMap::new();
        for r in s.iter() {
            let mut r = r.unwrap();
            let unit = names.get(&r.module()).cloned().unwrap_or_default();
            let name = r.name().as_bytes().to_vec();
            let n = seen.entry((unit.clone(), name.clone())).or_default();
            neutral(&mut r);
            out.insert((unit, name, *n), r);
            *n += 1;
        }
        out
    };
    let a = keyed(reference, units.0);
    let b = keyed(generated, units.1);
    let common: Vec<_> = a.keys().filter(|k| b.contains_key(*k)).collect();
    let same = common.iter().filter(|k| a.get(**k) == b.get(**k)).count();
    (same, common.len())
}

#[test]
fn generated_from_delphi6_matches_kb6() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let lib_dir = env::var_os("UNDELPHI_BCB6").map(|d| Path::new(&d).join("Lib/Obj"));
    let (Ok(kb6), Some(Ok(lib))) = (
        fs::read(root.join("tests/samples/idr/win32/kb6.bin")),
        lib_dir.as_deref().map(Library::open),
    ) else {
        eprintln!("skipping: kb6.bin or the Delphi 6 library (UNDELPHI_BCB6) missing");
        return;
    };
    let reference = KnowledgeBase::parse(&kb6).unwrap();
    // The units kb6 describes that the library holds as standalone DCUs.
    let units: Vec<UnitFile> = reference
        .modules()
        .iter()
        .filter_map(|m| {
            let m = m.unwrap();
            let stem = m.file_name.to_string();
            let stem = stem.strip_suffix(".dcu").unwrap_or(&stem).to_string();
            lib.unit(&stem)
                .filter(|u| !u.from_package)
                .map(|unit| UnitFile {
                    file_name: format!("{}.dcu", unit.name),
                    unit,
                })
        })
        .collect();
    assert_eq!(units.len(), 508);
    let bytes = kb::build(&units).to_bytes().unwrap();
    let generated = KnowledgeBase::parse(&bytes).unwrap();
    let names = |kb: &KnowledgeBase<'_>| -> HashMap<u16, String> {
        kb.modules()
            .iter()
            .map(|m| {
                let m = m.unwrap();
                (m.id, m.name.to_string().to_ascii_uppercase())
            })
            .collect()
    };
    let (na, nb) = (names(&reference), names(&generated));
    let units = (&na, &nb);

    // Floors at the measured agreement. All but two of the remaining
    // differences lie in the input: BCB6 rebuilt some units (other code,
    // offsets, name casing) and lacks 23 of kb6's units, whose types other
    // records name. The two: kb6 registers no offset-0 class references
    // for IBCustomDataSet's `DefaultFieldClasses`, and CRTL's `__mbctype`
    // shows a different last declaration.
    let (same, total) = identical(&reference.procs(), &generated.procs(), units, |p| {
        p.module = 0
    });
    assert!(same >= 36_336, "procs {same} of {total}");
    let (same, total) = identical(&reference.types(), &generated.types(), units, |t| {
        t.module = 0
    });
    assert!(same >= 13_999, "types {same} of {total}");
    let (same, total) = identical(&reference.consts(), &generated.consts(), units, |c| {
        c.module = 0
    });
    assert!(same >= 36_189, "consts {same} of {total}");
    let (same, total) = identical(&reference.vars(), &generated.vars(), units, |v| {
        v.module = 0
    });
    assert!(same >= 1_190, "vars {same} of {total}");
    let (same, total) = identical(
        &reference.res_strings(),
        &generated.res_strings(),
        units,
        |r| r.module = 0,
    );
    assert!(same >= 2_133, "resource strings {same} of {total}");
}
