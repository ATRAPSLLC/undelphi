//! Locating a program's DCU declarations in its binary
//! ([`undelphi::dcu::locate`], [`undelphi::DelphiBinary::locate`]), checked
//! against the linker maps the fixture builds keep.
//!
//! The committed samples suffice for the first test (`modernbin.exe` with
//! its own unit `modernrt.dcu`, among the units in its sample directory)
//! and for the sweep over every program and package without the RTL. The
//! rest needs the compilers' unit libraries (proprietary, never committed;
//! named by the environment, see [`lib_dir`]) and is skipped without them:
//! the fixture programs and packages through the whole RTL.

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
    DelphiBinary,
    dcu::{
        library::Library,
        locate::{Address, InitOwner, Location},
    },
};

const SAMPLES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/samples");

/// Where a linker map puts a public symbol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MapAddress {
    Va(u64),
    Thread(u64),
}

/// Every public of a linker map by name ("Publics by Name"). Segments with
/// a base of 0 (Delphi 6) take their PE section's address; offsets are
/// signed 32-bit (the map writes `SysInit.__ImageBase` below its segment
/// that way); a `TLS` segment gives thread-local offsets.
fn map_publics(map: &Path, image: &[u8]) -> HashMap<String, Vec<MapAddress>> {
    let text = String::from_utf8_lossy(&fs::read(map).unwrap()).replace('\r', "");
    let pe = undelphi::goblin::pe::PE::parse(image).unwrap();
    let mut segments: HashMap<String, (u64, bool)> = HashMap::new();
    for line in text.lines() {
        if line.contains("Detailed map of segments") {
            break;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 4 || parts[0].len() < 6 || parts[0].as_bytes()[4] != b':' {
            continue;
        }
        let class = parts[3];
        let mut base = u64::from_str_radix(&parts[0][5..], 16).unwrap();
        if base == 0
            && class != "TLS"
            && let Some(section) = pe.sections.iter().find(|s| s.name().ok() == Some(class))
        {
            base = pe.image_base + u64::from(section.virtual_address);
        }
        segments
            .entry(parts[0][..4].to_string())
            .or_insert((base, class == "TLS"));
    }
    let mut out: HashMap<String, Vec<MapAddress>> = HashMap::new();
    let mut publics = false;
    for line in text.lines() {
        if line.contains("Publics by Name") {
            publics = true;
            continue;
        }
        if line.contains("Publics by Value") {
            break;
        }
        let Some((at, name)) = line.trim().split_once(char::is_whitespace) else {
            continue;
        };
        if !publics || at.len() < 6 {
            continue;
        }
        let Some(&(base, tls)) = segments.get(&at[..4]) else {
            continue;
        };
        let offset = i64::from(u32::from_str_radix(&at[5..], 16).unwrap() as i32);
        let address = if tls {
            MapAddress::Thread(offset as u64)
        } else {
            MapAddress::Va(base.wrapping_add_signed(offset))
        };
        out.entry(name.trim().to_string())
            .or_default()
            .push(address);
    }
    out
}

/// What checking a [`Location`] against a map found.
#[derive(Debug, Default)]
struct Checked {
    /// Placements the map confirms.
    placements: usize,
    /// Thread variables among them.
    threads: usize,
    /// Placements the map contradicts.
    wrong: Vec<String>,
    /// Slots with routines named after the map's owner of their routine.
    slots: usize,
    /// Slots named after another owner than the map's.
    wrong_slots: Vec<String>,
    /// Slots with routines left unnamed (the program's own among them).
    unnamed: usize,
}

/// Checks every placement and named slot of `bin`'s location against
/// `map`. A Delphi 6 map names symbols without their unit (`qualified`
/// false); names it lists more than once are not checked.
fn check(
    bin: &DelphiBinary<'_>,
    loc: &Location,
    map: &Path,
    image: &[u8],
    qualified: bool,
) -> Checked {
    let publics = map_publics(map, image);
    let key = |unit: &str, name: &str| {
        if qualified {
            format!("{unit}.{name}")
        } else {
            name.to_string()
        }
    };
    let mut out = Checked::default();
    for p in &loc.placements {
        let Some([truth]) = publics.get(&key(&p.unit, &p.name)).map(Vec::as_slice) else {
            continue;
        };
        // Delphi 6's map has no TLS segment to check a thread variable by.
        let got = match p.address {
            Address::Va(va) => MapAddress::Va(va),
            Address::Thread(_) if !qualified => continue,
            Address::Thread(offset) => MapAddress::Thread(offset),
        };
        if got == *truth {
            out.placements += 1;
            out.threads += usize::from(matches!(got, MapAddress::Thread(_)));
        } else {
            out.wrong.push(format!(
                "{}.{} at {:?}, map {truth:?}",
                p.unit, p.name, p.address
            ));
        }
    }
    let mut by_va: HashMap<u64, Vec<&str>> = HashMap::new();
    for (name, addresses) in &publics {
        for a in addresses {
            if let MapAddress::Va(va) = a {
                by_va.entry(*va).or_default().push(name);
            }
        }
    }
    let table = bin.unit_init_table().unwrap();
    assert_eq!(loc.init_table.len(), table.units.len());
    for (slot, (entry, owner)) in table.units.iter().zip(&loc.init_table).enumerate() {
        let Some(va) = entry.init_va.or(entry.finalize_va) else {
            continue;
        };
        let names = by_va.get(&va).cloned().unwrap_or_default();
        let matches = match owner {
            None => {
                out.unnamed += 1;
                continue;
            }
            // A unit's initialization routine bears the unit's name; when
            // a slot has only a finalization, the map's `Finalization` is
            // unqualified on Delphi 6.
            Some(InitOwner::Unit(unit)) => {
                names.contains(&key(unit, unit).as_str())
                    || entry.init_va.is_none()
                        && names.contains(&key(unit, "Finalization").as_str())
            }
            Some(InitOwner::Class { unit, class }) => {
                let prefix = key(unit, &format!("{class}."));
                names
                    .iter()
                    .any(|n| n.starts_with(&prefix) && n.ends_with('@'))
            }
        };
        if matches {
            out.slots += 1;
        } else {
            out.wrong_slots
                .push(format!("slot {slot}: {owner:?}, map {names:?}"));
        }
    }
    out
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
fn own_unit_is_located_without_the_toolchain() {
    // modernrt.dcu alone: its routines that the binary's RTTI or the init
    // table reach are placed (relocations into the absent RTL are not
    // followed), TWidget's class constructor slot is named by its wrapper,
    // which calls TWidget.CreateClass in the same unit, and modernrt's own
    // slot - routines dropped on Win32 - is the next: a class's slot
    // precedes its unit's.
    for platform in ["win32", "win64"] {
        let dir = Path::new(SAMPLES).join("d13").join(platform);
        let library = Library::open(&dir).unwrap();
        let image = fs::read(dir.join("modernbin.exe")).unwrap();
        let bin = DelphiBinary::parse(&image).unwrap();
        let loc = bin.locate(&library);
        let checked = check(&bin, &loc, &dir.join("modernbin.map"), &image, true);
        assert!(checked.wrong.is_empty(), "{platform}: {:?}", checked.wrong);
        assert!(loc.conflicts.is_empty(), "{platform}: {:?}", loc.conflicts);
        assert!(
            checked.wrong_slots.is_empty(),
            "{platform}: {:?}",
            checked.wrong_slots
        );
        assert!(checked.placements >= 40, "{platform}: {checked:?}");
        let widget = loc
            .init_table
            .iter()
            .position(|o| {
                *o == Some(InitOwner::Class {
                    unit: "modernrt".into(),
                    class: "TWidget".into(),
                })
            })
            .unwrap_or_else(|| panic!("{platform}: {:?}", loc.init_table));
        assert_eq!(widget, 44, "{platform}");
        assert_eq!(
            loc.init_table[45],
            Some(InitOwner::Unit("modernrt".into())),
            "{platform}"
        );
        let changed = loc.find("modernrt", "TWidget.Changed").unwrap();
        assert!(matches!(changed.address, Address::Va(_)) && changed.verified);
    }
}

#[test]
fn programs_are_located_through_the_rtl() {
    // Each fixture program with the RTL of the compiler that built it and
    // its own units: every placement and named slot the map can check
    // agrees with it, and the floors pin how much is found. With every
    // linked unit at hand (PACKAGEINFO lists them), Delphi 13 Win32 names
    // every slot that has routines but the program's own; Win64 and
    // Delphi 6 keep the counter routines of units without initialization
    // code, which only the variables around their counters or the uses
    // order tell apart.
    struct Case {
        exe: &'static str,
        lib: &'static str,
        qualified: bool,
        placements: usize,
        threads: usize,
        slots: usize,
        unnamed: usize,
    }
    let cases = [
        Case {
            exe: "bcb6/known.exe",
            lib: "UNDELPHI_BCB6/Lib/Obj",
            qualified: false,
            placements: 1_150,
            threads: 0,
            slots: 7,
            unnamed: 7,
        },
        Case {
            exe: "bcb6/gtforms.exe",
            lib: "UNDELPHI_BCB6/Lib/Obj",
            qualified: false,
            placements: 3_300,
            threads: 0,
            slots: 24,
            unnamed: 34,
        },
        Case {
            exe: "d13/win32/known.exe",
            lib: "UNDELPHI_D13/lib/win32/release",
            qualified: true,
            placements: 4_200,
            threads: 8,
            slots: 26,
            unnamed: 1,
        },
        Case {
            exe: "d13/win32/modernbin.exe",
            lib: "UNDELPHI_D13/lib/win32/release",
            qualified: true,
            placements: 4_250,
            threads: 8,
            slots: 27,
            unnamed: 1,
        },
        Case {
            exe: "d13/win32/gtforms.exe",
            lib: "UNDELPHI_D13/lib/win32/release",
            qualified: true,
            placements: 10_450,
            threads: 20,
            slots: 67,
            unnamed: 1,
        },
        Case {
            exe: "d13/win64/known.exe",
            lib: "UNDELPHI_D13/lib/win64/release",
            qualified: true,
            placements: 4_300,
            threads: 10,
            slots: 28,
            unnamed: 12,
        },
        Case {
            exe: "d13/win64/modernbin.exe",
            lib: "UNDELPHI_D13/lib/win64/release",
            qualified: true,
            placements: 4_350,
            threads: 10,
            slots: 30,
            unnamed: 12,
        },
        Case {
            exe: "d13/win64/gtforms.exe",
            lib: "UNDELPHI_D13/lib/win64/release",
            qualified: true,
            placements: 10_700,
            threads: 22,
            slots: 74,
            unnamed: 64,
        },
    ];
    for case in cases {
        let Some(rtl) = lib_dir(case.lib) else {
            continue;
        };
        let exe = Path::new(SAMPLES).join(case.exe);
        let own = exe.parent().unwrap();
        let library = Library::open_dirs(&[own, &rtl]).unwrap();
        let image = fs::read(&exe).unwrap();
        let bin = DelphiBinary::parse(&image).unwrap();
        let loc = bin.locate(&library);
        let map = exe.with_extension("map");
        let checked = check(&bin, &loc, &map, &image, case.qualified);
        let name = case.exe;
        assert!(checked.wrong.is_empty(), "{name}: {:?}", checked.wrong);
        assert!(loc.conflicts.is_empty(), "{name}: {:?}", loc.conflicts);
        assert!(
            checked.wrong_slots.is_empty(),
            "{name}: {:?}",
            checked.wrong_slots
        );
        assert!(checked.placements >= case.placements, "{name}: {checked:?}");
        assert!(checked.threads >= case.threads, "{name}: {checked:?}");
        assert!(checked.slots >= case.slots, "{name}: {checked:?}");
        assert!(checked.unnamed <= case.unnamed, "{name}: {checked:?}");
    }
}

#[test]
fn every_sample_locates_without_contradiction() {
    // Every committed program and package (runtime-package programs
    // included, whose tables also hold the required packages' units),
    // with only the committed units and, when present, with the RTL: no
    // placement or named slot the map contradicts, and no variable the
    // relocations disagree on.
    for (dir, lib, qualified) in [
        ("bcb6", "UNDELPHI_BCB6/Lib/Obj", false),
        ("d13/win32", "UNDELPHI_D13/lib/win32/release", true),
        ("d13/win64", "UNDELPHI_D13/lib/win64/release", true),
    ] {
        let own = Path::new(SAMPLES).join(dir);
        let rtl = lib_dir(lib);
        let mut modules: Vec<PathBuf> = fs::read_dir(&own)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| matches!(p.extension().and_then(|e| e.to_str()), Some("exe" | "bpl")))
            .filter(|p| p.with_extension("map").exists())
            .collect();
        modules.sort();
        let mut libraries = vec![Library::open(&own).unwrap()];
        if let Some(rtl) = &rtl {
            libraries.push(Library::open_dirs(&[&own, rtl]).unwrap());
        }
        for path in &modules {
            let image = fs::read(path).unwrap();
            let bin = DelphiBinary::parse(&image).unwrap();
            if bin.unit_init_table().is_none() {
                continue;
            }
            for library in &libraries {
                let loc = bin.locate(library);
                let checked = check(&bin, &loc, &path.with_extension("map"), &image, qualified);
                let name = path.strip_prefix(SAMPLES).unwrap().display();
                assert!(checked.wrong.is_empty(), "{name}: {:?}", checked.wrong);
                assert!(
                    checked.wrong_slots.is_empty(),
                    "{name}: {:?}",
                    checked.wrong_slots
                );
                assert!(loc.conflicts.is_empty(), "{name}: {:?}", loc.conflicts);
            }
        }
    }
}
