//! Placing knowledge-base routines in binaries (`kb::locate`): every
//! placement at a linker-map public carries the map's name, and the
//! release a binary was built with ranks first among the `syskb` files.
//!
//! The published knowledge bases are read from `tests/samples/idr` (see
//! `SAMPLES.md`); the Delphi 13 ones are built from the compiler's units
//! when `UNDELPHI_D13` names an install. A test whose inputs are missing
//! says so and passes.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use undelphi::{
    DelphiBinary,
    dcu::{Version, library::Library},
    kb::{
        self, CallKind, KnowledgeBase, ProcKind, UnitFile,
        locate::{KbIndex, KbPrototype, KbStorage, KbType, OwnerBasis, Reached, identify},
    },
};

/// A path under `tests/samples`.
fn sample(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/samples")
        .join(relative)
}

/// The names of a linker map's code publics, by address.
///
/// A map numbers its segments by the image's sections, in order, and
/// places each public at `segment:offset` from the segment's start;
/// Delphi 13 lists the publics by name only, Delphi 6 by name and by value.
fn map_publics(bin: &DelphiBinary<'_>, map: &Path) -> BTreeMap<u64, Vec<String>> {
    let pe = bin.ctx().pe().unwrap();
    let text = String::from_utf8_lossy(&fs::read(map).unwrap()).replace('\r', "");
    let mut lines = text.lines();
    let mut segments = BTreeMap::new();
    for line in lines.by_ref() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.first() == Some(&"Address") {
            break;
        }
        if let [address, _, _, "CODE" | "ICODE"] = fields[..] {
            let segment = u16::from_str_radix(&address[..4], 16).unwrap();
            let section = &pe.sections[usize::from(segment) - 1];
            segments.insert(segment, pe.image_base + u64::from(section.virtual_address));
        }
    }
    let mut publics = BTreeMap::new();
    for line in lines {
        if line.contains("Line numbers") || line.contains("Bound resource") {
            break;
        }
        let fields: Vec<&str> = line.split_whitespace().collect();
        let [address, name] = fields[..] else {
            continue;
        };
        let Some((segment, offset)) = address.split_once(':') else {
            continue;
        };
        let (Ok(segment), Ok(offset)) = (
            u16::from_str_radix(segment, 16),
            u64::from_str_radix(offset, 16),
        ) else {
            continue;
        };
        if let Some(base) = segments.get(&segment) {
            let names: &mut Vec<String> = publics.entry(base + offset).or_default();
            if !names.iter().any(|n| n == name) {
                names.push(name.to_string());
            }
        }
    }
    publics
}

/// Whether a knowledge-base name is the map's: the map qualifies a name by
/// its unit (`Gtmain.TForm1.Button1Click`, `System.CloseHandle`).
fn same_name(kb: &str, map: &str) -> bool {
    map.eq_ignore_ascii_case(kb)
        || map
            .to_ascii_lowercase()
            .ends_with(&format!(".{}", kb.to_ascii_lowercase()))
}

/// Whether a name is one the compiler made up for a routine the source
/// does not name, which the knowledge base and the map spell differently.
fn anonymous(name: &str) -> bool {
    name.starts_with("_DF_") || name.starts_with("_NF__")
}

/// The pattern of a routine: its bytes with the relocated ones blanked.
fn pattern(kb: &KnowledgeBase<'_>, index: usize) -> Vec<Option<u8>> {
    let proc = kb.procs().get(index).unwrap();
    proc.dump
        .code
        .iter()
        .zip(proc.dump.relocs.iter())
        .map(|(&b, &m)| (m != 0xFF).then_some(b))
        .collect()
}

/// Whether the knowledge base holds a routine the map's name names (the
/// map's name or one of its unit-stripped tails) with the pattern of the
/// routine placed: the two are byte for byte the same routine, which no
/// placement by bytes can tell apart (on Win64 `TDoubleHelper` and
/// `TExtendedHelper` compile alike).
fn indistinguishable(kb: &KnowledgeBase<'_>, placed: usize, map: &str) -> bool {
    let placed = pattern(kb, placed);
    let parts: Vec<&str> = map.split('.').collect();
    (0..parts.len()).any(|i| {
        kb.procs()
            .named(parts[i..].join(".").as_bytes())
            .into_iter()
            .any(|other| pattern(kb, other) == placed)
    })
}

/// Places `index` (of `kb`) in each program and checks every placement at
/// a map public against it; returns the number of publics named right.
fn check_against_maps(kb: &KnowledgeBase<'_>, index: &KbIndex, programs: &[(&str, &str)]) -> usize {
    let mut right = 0;
    for (exe, map) in programs {
        let data = fs::read(sample(exe)).unwrap();
        let bin = DelphiBinary::parse(&data).unwrap();
        let publics = map_publics(&bin, &sample(map));
        let location = index.locate(bin.ctx());
        assert!(location.scanned() > 0, "{exe}: the scan placed nothing");
        for placement in &location.placements {
            let Some(truth) = publics.get(&placement.va) else {
                continue;
            };
            if truth.iter().any(|t| same_name(&placement.name, t)) {
                right += 1;
                continue;
            }
            assert!(
                placement.shared
                    || anonymous(&placement.name)
                    || truth
                        .iter()
                        .any(|t| indistinguishable(kb, placement.index, t)),
                "{exe}: {:#x} placed as {} ({:?}), the map names {truth:?}",
                placement.va,
                placement.name,
                placement.reached
            );
        }
        let init = publics
            .iter()
            .find(|(_, names)| names.iter().any(|n| n.ends_with("@InitExe")))
            .map(|(&va, _)| va);
        if let Some(init) = init {
            assert!(
                location.at(init).any(|p| p.name == "@InitExe"),
                "{exe}: @InitExe is not placed at {init:#x}"
            );
        }
    }
    right
}

/// The published Delphi 6 knowledge base places routines in the Delphi 6
/// fixture programs where their linker maps put them, and nowhere a map
/// names another routine.
#[test]
fn kb6_places_delphi6_routines_where_the_maps_do() {
    let path = sample("idr/win32/kb6.bin");
    let Ok(data) = fs::read(&path) else {
        eprintln!("skipping: {} missing", path.display());
        return;
    };
    let kb = KnowledgeBase::parse(&data).unwrap();
    let index = KbIndex::new(&kb, Version::DELPHI_6);
    let right = check_against_maps(
        &kb,
        &index,
        &[
            ("bcb6/gtforms.exe", "bcb6/gtforms.map"),
            ("bcb6/known.exe", "bcb6/known.map"),
            ("bcb6/tables.exe", "bcb6/tables.map"),
            ("bcb6/deep.exe", "bcb6/deep.map"),
        ],
    );
    assert!(right > 0);
}

/// A block the compiler emits without a name is data, not a routine: the
/// pointer tables and string literals the knowledge bases of Delphi 6 and 7
/// file as code are in the image, byte for byte, and are placed nowhere.
#[test]
fn unnamed_data_blocks_are_not_placed_as_routines() {
    // Each program with its knowledge base and the blocks of it: address,
    // unit, record name.
    type Blocks = &'static [(u64, &'static str, &'static str)];
    let cases: [(&str, &str, Version, Blocks); 2] = [
        (
            "bcb6/gtforms.exe",
            "idr/win32/kb6.bin",
            Version::DELPHI_6,
            &[
                (0x40c314, "SysUtils", "_NF__AD4"),
                (0x40c400, "SysUtils", "_NF__AD5"),
                (0x421d30, "Classes", "_NF__E01"),
                (0x4333a8, "Dialogs", "_NF__4C2"),
                (0x458650, "Forms", "_NF__A31"),
            ],
        ),
        (
            "lightalloy/LA.exe",
            "idr/win32/kb7.bin",
            Version::DELPHI_7,
            &[
                (0x411f74, "SysUtils", "_NF__C58"),
                (0x412060, "SysUtils", "_NF__C59"),
                (0x42d7e8, "Classes", "_NF__E7C"),
                (0x43e8b0, "Dialogs", "_NF__533"),
                (0x445cd0, "Buttons", "_NF__1F3"),
                (0x4487ec, "Buttons", "_NF__383"),
                (0x4699a4, "Themes", "_NF__337"),
                (0x49c8b0, "Forms", "_NF__A5A"),
            ],
        ),
    ];
    let mut checked = 0;
    for (exe, kb_path, version, blocks) in cases {
        let (Ok(kb_data), Ok(program)) = (fs::read(sample(kb_path)), fs::read(sample(exe))) else {
            eprintln!("skipping {exe}: {kb_path} or the program missing");
            continue;
        };
        let kb = KnowledgeBase::parse(&kb_data).unwrap();
        let bin = DelphiBinary::parse(&program).unwrap();
        let location = KbIndex::new(&kb, version).locate(bin.ctx());
        for &(va, unit, name) in blocks {
            // The record the image holds at `va`.
            let held: Vec<_> = kb
                .procs()
                .named(name.as_bytes())
                .into_iter()
                .filter_map(|i| kb.procs().get(i).ok())
                .filter(|p| {
                    kb.module(p.module)
                        .ok()
                        .flatten()
                        .is_some_and(|m| m.name.to_string().eq_ignore_ascii_case(unit))
                })
                .filter(|p| {
                    bin.ctx()
                        .va_to_file(va)
                        .and_then(|at| bin.ctx().data().get(at..))
                        .is_some_and(|bytes| p.dump.matches(bytes))
                })
                .collect();
            let [block] = held.as_slice() else {
                panic!("{exe}: {unit}.{name} is not in the image at {va:#x}");
            };
            assert!(!block.holds_code(version), "{unit}.{name}");
            assert!(
                location.at(va).next().is_none(),
                "{exe}: the data block {unit}.{name} is placed as a routine at {va:#x}"
            );
            checked += 1;
        }
    }
    if checked == 0 {
        eprintln!("skipping: no program or knowledge base present");
    }
}

/// A relocation decodes to the routine it names: in a Delphi 6 program,
/// `@InitExe` is reached from the scan and the routines it calls by
/// reference.
#[test]
fn relocations_place_the_routines_they_name() {
    let (Ok(data), Ok(program)) = (
        fs::read(sample("idr/win32/syskb6.bin")),
        fs::read(sample("bcb6/gtforms.exe")),
    ) else {
        eprintln!("skipping: syskb6.bin or gtforms.exe missing");
        return;
    };
    let kb = KnowledgeBase::parse(&data).unwrap();
    let bin = DelphiBinary::parse(&program).unwrap();
    let location = KbIndex::new(&kb, Version::DELPHI_6).locate(bin.ctx());
    let reached = |name: &str| {
        location
            .placements
            .iter()
            .find(|p| p.name == name)
            .map(|p| p.reached)
    };
    assert_eq!(reached("@InitExe"), Some(Reached::Scan));
    assert_eq!(reached("@StartExe"), Some(Reached::Reference));
    assert_eq!(reached("InitializeModule"), Some(Reached::Reference));
}

/// Every unit initialization slot the knowledge base names in a Delphi 6
/// program is the unit the linker map names its routine after, and the
/// knowledge base declares `TObject` in `System`.
#[test]
fn kb6_names_unit_initialization_slots_as_the_maps_do() {
    let Ok(data) = fs::read(sample("idr/win32/kb6.bin")) else {
        eprintln!("skipping: kb6.bin missing");
        return;
    };
    let kb = KnowledgeBase::parse(&data).unwrap();
    let index = KbIndex::new(&kb, Version::DELPHI_6);
    assert!(
        index
            .class_units("TObject")
            .any(|u| u.eq_ignore_ascii_case("System"))
    );
    for exe in ["gtforms", "known", "tables"] {
        let program = fs::read(sample(&format!("bcb6/{exe}.exe"))).unwrap();
        let bin = DelphiBinary::parse(&program).unwrap();
        let publics = map_publics(&bin, &sample(&format!("bcb6/{exe}.map")));
        let table = bin.unit_init_table().unwrap();
        let slots: Vec<(Option<u64>, Option<u64>)> = table
            .units
            .iter()
            .map(|e| (e.init_va, e.finalize_va))
            .collect();
        let location = index.locate(bin.ctx());
        let owners = index.init_owners(&location, bin.ctx(), &slots);
        let mut named = 0;
        for (slot, owner) in slots.iter().zip(&owners) {
            let Some(owner) = owner else { continue };
            named += 1;
            let finalization = owner.routine.eq_ignore_ascii_case("Finalization");
            let va = if finalization { slot.1 } else { slot.0 }.unwrap();
            let names = publics.get(&va).cloned().unwrap_or_default();
            let expected = if finalization {
                "Finalization"
            } else {
                owner.unit.as_str()
            };
            assert!(
                names.iter().any(|n| n.eq_ignore_ascii_case(expected)),
                "{exe}: slot at {va:#x} named {} ({:?}), the map names {names:?}",
                owner.unit,
                owner.basis
            );
        }
        assert!(named > 0, "{exe}: no slot named");
        assert!(
            owners
                .iter()
                .flatten()
                .any(|o| o.basis == OwnerBasis::Adjacent)
        );
    }
}

/// The `syskb` files of the published releases, with the release each was
/// built for.
fn published_syskbs() -> Vec<(Version, Vec<u8>)> {
    [
        (Version::DELPHI_2, "2"),
        (Version::DELPHI_3, "3"),
        (Version::DELPHI_4, "4"),
        (Version::DELPHI_5, "5"),
        (Version::DELPHI_6, "6"),
        (Version::DELPHI_7, "7"),
        (Version::DELPHI_2005, "2005"),
        (Version::DELPHI_2006, "2006"),
        (Version::DELPHI_2009, "2009"),
        (Version::DELPHI_2010, "2010"),
        (Version::DELPHI_XE, "2011"),
        (Version::DELPHI_XE2, "2012"),
        (Version::DELPHI_XE3, "2013"),
        (Version::DELPHI_XE4, "2014"),
    ]
    .into_iter()
    .filter_map(|(version, name)| {
        fs::read(sample(&format!("idr/win32/syskb{name}.bin")))
            .ok()
            .map(|data| (version, data))
    })
    .collect()
}

/// Delphi 6 and C++Builder 6 programs rank the Delphi 6 `syskb` first, and
/// a Free Pascal program ranks none: it places no `SysInit` routine.
#[test]
fn identify_ranks_the_building_release_first() {
    let files = published_syskbs();
    if files.len() < 14 {
        eprintln!("skipping: the published syskb files are missing");
        return;
    }
    let kbs: Vec<KnowledgeBase<'_>> = files
        .iter()
        .map(|(_, data)| KnowledgeBase::parse(data).unwrap())
        .collect();
    let indexes: Vec<KbIndex> = files
        .iter()
        .zip(&kbs)
        .map(|((version, _), kb)| KbIndex::new(kb, *version))
        .collect();
    let candidates: Vec<&KbIndex> = indexes.iter().collect();
    let first = |exe: &str| {
        let data = fs::read(sample(exe)).unwrap();
        let bin = DelphiBinary::parse(&data).unwrap();
        identify(bin.ctx(), &candidates)
            .first()
            .map(|&(i, _)| candidates[i].version())
    };
    assert_eq!(first("bcb6/gtforms.exe"), Some(Version::DELPHI_6));
    assert_eq!(first("bcb6/known.exe"), Some(Version::DELPHI_6));
    assert_eq!(first("bcb6/cppknown.exe"), Some(Version::DELPHI_6));
    if fs::metadata(sample("known-rtti/known.win32.exe")).is_ok() {
        assert_eq!(first("known-rtti/known.win32.exe"), None);
    }
}

/// The knowledge base of the units under `dir`, built as `kb_build` does.
fn build_kb(dir: &Path, units: Option<&[&str]>) -> Vec<u8> {
    let lib = Library::open(dir).unwrap();
    let mut names: Vec<String> = lib.unit_names().map(str::to_string).collect();
    names.sort();
    let files: Vec<UnitFile> = names
        .iter()
        .filter(|n| units.is_none_or(|u| u.iter().any(|w| w.eq_ignore_ascii_case(n))))
        .filter_map(|n| lib.unit(n))
        .filter(|u| !u.from_package)
        .map(|unit: Arc<_>| UnitFile {
            file_name: format!("{}.dcu", unit.name),
            unit,
        })
        .collect();
    kb::build(&files).to_bytes().unwrap()
}

/// A knowledge base built from the Delphi 13 units places routines in the
/// Delphi 13 fixtures (Win32 and Win64) where their maps put them, and
/// its `syskb` identifies them.
#[test]
fn built_delphi13_kb_places_delphi13_routines() {
    let Some(root) = env::var_os("UNDELPHI_D13") else {
        eprintln!("skipping: UNDELPHI_D13 not set");
        return;
    };
    for platform in ["win32", "win64"] {
        let dir = Path::new(&root).join("lib").join(platform).join("release");
        let data = build_kb(&dir, None);
        let kb = KnowledgeBase::parse(&data).unwrap();
        let index = KbIndex::new(&kb, Version::DELPHI_13_FLORENCE);
        let right = check_against_maps(
            &kb,
            &index,
            &[(
                &format!("d13/{platform}/gtforms.exe"),
                &format!("d13/{platform}/gtforms.map"),
            )],
        );
        assert!(right > 0, "{platform}: nothing named right");

        let sys = build_kb(&dir, Some(&["System", "SysInit"]));
        let sys = KnowledgeBase::parse(&sys).unwrap();
        let sys = KbIndex::new(&sys, Version::DELPHI_13_FLORENCE);
        let program = fs::read(sample(&format!("d13/{platform}/gtforms.exe"))).unwrap();
        let bin = DelphiBinary::parse(&program).unwrap();
        assert_eq!(
            identify(bin.ctx(), &[&sys]).first().map(|&(i, _)| i),
            Some(0),
            "{platform}: the Delphi 13 syskb does not identify its program"
        );
    }
}

/// A placed routine carries its declaration: `TObject.ClassName` takes the
/// class reference as `Self`, `TObject.Free` an instance, and the two
/// `IntToStr` overloads a Delphi 6 program links are told apart by their
/// parameter, `Integer` and `Int64`.
#[test]
fn placements_carry_their_declarations() {
    let (Ok(data), Ok(program)) = (
        fs::read(sample("idr/win32/kb6.bin")),
        fs::read(sample("bcb6/gtforms.exe")),
    ) else {
        eprintln!("skipping: kb6.bin or gtforms.exe missing");
        return;
    };
    let kb = KnowledgeBase::parse(&data).unwrap();
    let bin = DelphiBinary::parse(&program).unwrap();
    let index = KbIndex::new(&kb, Version::DELPHI_6);
    let location = index.locate(bin.ctx());
    let declared = |name: &str| -> Vec<KbPrototype> {
        location
            .placements
            .iter()
            .filter(|p| p.name == name)
            .map(|p| index.prototype(p).cloned().unwrap())
            .collect()
    };
    let class_name = &declared("TObject.ClassName")[0];
    assert_eq!(class_name.kind, ProcKind::Function);
    assert_eq!(class_name.call, CallKind::Register);
    assert_eq!(class_name.result, "ShortString");
    assert_eq!(class_name.params[0].name, "Self");
    assert_eq!(class_name.params[0].type_def, "class of TObject");

    let free = &declared("TObject.Free")[0];
    assert_eq!(free.kind, ProcKind::Procedure);
    assert_eq!(free.params[0].type_def, "TObject");

    let mut int_to_str: Vec<String> = declared("IntToStr")
        .into_iter()
        .inspect(|p| assert!(p.overload))
        .map(|p| p.params[0].type_def.clone())
        .collect();
    int_to_str.sort();
    assert_eq!(int_to_str, ["System.Int64", "System.Integer"]);
}

/// The import slot an indirect `jmp` at `va` reads (`FF 25`): an absolute
/// address on Win32, relative to the next instruction on Win64.
fn jump_slot(bin: &DelphiBinary<'_>, va: u64) -> Option<u64> {
    let at = bin.ctx().va_to_file(va)?;
    let bytes = bin.ctx().data().get(at..at.checked_add(6)?)?;
    if bytes[..2] != [0xFF, 0x25] {
        return None;
    }
    let disp = u32::from_le_bytes(bytes[2..6].try_into().ok()?);
    if bin.ctx().pointer_size() == Some(4) {
        Some(u64::from(disp))
    } else {
        va.checked_add(6)?
            .checked_add_signed(i64::from(disp.cast_signed()))
    }
}

/// Every import thunk the knowledge base places - an `external` routine's
/// `jmp [slot]` stub - names by its relocation the import its slot holds:
/// the binary's import table states the DLL and entry the routine is
/// declared for, so a host binds the declaration to that import, never to
/// a name. The Delphi 6 program against the published knowledge base, and
/// the Delphi 13 ones against knowledge bases built from the units.
#[test]
fn placed_import_thunks_name_the_import_their_slot_holds() {
    let mut cases: Vec<(Vec<u8>, Version, String)> = Vec::new();
    match fs::read(sample("idr/win32/kb6.bin")) {
        Ok(data) => cases.push((data, Version::DELPHI_6, "bcb6/gtforms.exe".into())),
        Err(_) => eprintln!("skipping Delphi 6: kb6.bin missing"),
    }
    match env::var_os("UNDELPHI_D13") {
        Some(root) => {
            for platform in ["win32", "win64"] {
                let dir = Path::new(&root).join("lib").join(platform).join("release");
                cases.push((
                    build_kb(&dir, None),
                    Version::DELPHI_13_FLORENCE,
                    format!("d13/{platform}/gtforms.exe"),
                ));
            }
        }
        None => eprintln!("skipping Delphi 13: UNDELPHI_D13 not set"),
    }
    for (data, version, exe) in cases {
        let Ok(program) = fs::read(sample(&exe)) else {
            eprintln!("skipping: {exe} missing");
            continue;
        };
        let kb = KnowledgeBase::parse(&data).unwrap();
        let bin = DelphiBinary::parse(&program).unwrap();
        let location = KbIndex::new(&kb, version).locate(bin.ctx());
        let mut thunks = 0usize;
        let mut misnamed = Vec::new();
        for placement in &location.placements {
            let Some(import) = jump_slot(&bin, placement.va).and_then(|slot| bin.import_at(slot))
            else {
                continue;
            };
            thunks += 1;
            let proc = kb.procs().get(placement.index).unwrap();
            let named = proc
                .dump
                .fixups
                .iter()
                .filter(|fixup| fixup.offset == 2)
                .map(|fixup| fixup.name.to_string())
                .collect::<Vec<_>>();
            if named != [import.name()] {
                misnamed.push(format!(
                    "{:#x} {}.{}: {named:?} against {}!{}",
                    placement.va,
                    placement.unit,
                    placement.name,
                    import.module(),
                    import.name()
                ));
            }
        }
        assert!(thunks > 0, "{exe}: no import thunk placed");
        assert!(misnamed.is_empty(), "{exe}: {misnamed:#?}");
    }
}

/// A type the knowledge base declares states what it is, by the qualified
/// names its prototypes use, as Delphi 6's `Windows` unit declares it: a
/// handle, a `DWORD` and a `UINT` are four unsigned bytes or the intrinsic
/// `LongWord`/`Cardinal` their aliases lead to, `BOOL` a four-byte boolean
/// or `LongBool`, `PSecurityAttributes` a pointer. A record (`TRect`) is
/// held in no register and states nothing, and neither does a name no record
/// declares, nor an unqualified `DWORD`, which `System` records as a range
/// of `Integer` and `Windows` as `LongWord`.
#[test]
fn a_declared_type_states_what_it_is() {
    let Ok(data) = fs::read(sample("idr/win32/kb6.bin")) else {
        eprintln!("skipping: kb6.bin missing");
        return;
    };
    let kb = KnowledgeBase::parse(&data).unwrap();
    let index = KbIndex::new(&kb, Version::DELPHI_6);
    // What the knowledge base records under a name, for a failure's message.
    let records = |name: &str| -> Vec<String> {
        let bare = name.rsplit('.').next().unwrap_or(name);
        kb.types()
            .iter()
            .filter_map(Result::ok)
            .filter(|t| {
                t.name
                    .as_str()
                    .is_some_and(|n| n.eq_ignore_ascii_case(bare))
            })
            .map(|t| {
                format!(
                    "{:?} size {} decl {:?} unit {:?}",
                    t.kind,
                    t.size,
                    t.decl.as_str(),
                    index.unit_name(t.module)
                )
            })
            .collect()
    };
    let intrinsic = |ty: &Option<KbType>, names: &[&str]| {
        matches!(ty, Some(KbType::Intrinsic(n))
            if names.iter().any(|w| n.rsplit('.').next().is_some_and(|b| b.eq_ignore_ascii_case(w))))
    };
    for name in [
        "Windows.HWND",
        "Windows.DWORD",
        "Windows.UINT",
        "Windows.THandle",
    ] {
        let ty = index.type_of(name);
        assert!(
            ty == Some(KbType::Storage(KbStorage::Int {
                bytes: 4,
                signed: false
            })) || intrinsic(&ty, &["LongWord", "Cardinal"]),
            "{name}: {ty:?} from {:#?}",
            records(name)
        );
    }
    let bool_ty = index.type_of("Windows.BOOL");
    assert!(
        bool_ty == Some(KbType::Storage(KbStorage::Bool { bytes: 4 }))
            || intrinsic(&bool_ty, &["LongBool"]),
        "BOOL: {bool_ty:?} from {:#?} and {:#?}",
        records("Windows.BOOL"),
        records("LongBool")
    );
    assert_eq!(
        index.type_of("Windows.PSecurityAttributes"),
        Some(KbType::Storage(KbStorage::Pointer))
    );
    assert_eq!(index.type_of("Windows.TRect"), None);
    assert_eq!(index.type_of("DWORD"), None, "{:#?}", records("DWORD"));
    assert_eq!(index.type_of("NoSuchType"), None);
}
