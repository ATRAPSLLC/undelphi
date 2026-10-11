//! Checks generated export names against runtime packages: for every
//! routine of every unit a package contains, generates its mangled name
//! (Borland on Win32, Itanium on Win64) with `dcu::mangle::Mangler` and
//! looks it up in the package's exports.
//!
//! ```sh
//! cargo run --release --example mangle_check -- path/to/Redist/win32 path/to/lib/win32/release [-v]
//! ```
//!
//! Reports how many exported functions a generated name matches. With
//! `-v` it also lists, per unmatched Borland export, whether it belongs to
//! a generic instance, to a routine whose parameters were encoded
//! differently (with our candidates), or to no routine at all (unmatched
//! Itanium exports are listed as `itanium`, unclassified), plus units
//! the library lacks (`MISSING`) and routines no name was generated for
//! (`NONAME`). With `MANGLE_DUMP` set it prints every generated name
//! (`GEN`) and export (`EXP`) for offline comparison.

use std::{
    collections::HashSet,
    env, fs,
    path::{Path, PathBuf},
    process,
    sync::Arc,
};

use undelphi::{
    DelphiBinary,
    dcu::{DeclKind, library::Library, mangle::Mangler},
};

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let (Some(bpls), Some(lib)) = (args.first(), args.get(1)) else {
        eprintln!("usage: mangle_check <bpl dir> <dcu dir> [-v]");
        process::exit(2);
    };
    let verbose = args.iter().any(|a| a == "-v");
    let lib = Library::open(Path::new(lib)).unwrap_or_else(|e| {
        eprintln!("{e}");
        process::exit(1);
    });
    let mangler = Mangler::new(&lib);
    let mut paths: Vec<_> = fs::read_dir(bpls)
        .map(|r| r.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    paths.retain(|p: &PathBuf| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("bpl")));
    paths.sort();
    let (mut functions, mut matched, mut generated) = (0usize, 0usize, 0usize);
    let mut shown = 0usize;
    let mut generated_names: Vec<String> = Vec::new();
    for path in &paths {
        let Ok(data) = fs::read(path) else { continue };
        let Ok(bin) = DelphiBinary::parse(&data) else {
            continue;
        };
        let exports: HashSet<String> = bin
            .exports()
            .into_iter()
            .filter(|(name, va)| {
                bin.import_at(*va).is_none()
                    && (name.contains("$q") || (name.starts_with("_Z") && !name.ends_with('E')))
            })
            .map(|(name, _)| name.to_string())
            .collect();
        functions = functions.saturating_add(exports.len());
        if env::var_os("MANGLE_DUMP").is_some() {
            for e in &exports {
                println!("EXP {e}");
            }
        }
        let mut hit: HashSet<&str> = HashSet::new();
        let units: Vec<String> = bin
            .package_info()
            .map(|i| i.contains.iter().map(|u| u.name.to_string()).collect())
            .unwrap_or_default();
        for name in &units {
            let Some(unit) = lib.unit(name) else {
                if verbose {
                    println!("MISSING {name}");
                }
                continue;
            };
            let unit: Arc<_> = unit;
            for d in unit.decls_of(&unit.main) {
                if !matches!(d.kind, DeclKind::Proc(_)) {
                    continue;
                }
                let Some(m) = mangler.routine(&unit, d) else {
                    if verbose {
                        println!("NONAME {} {}", unit.name, d.name);
                    }
                    continue;
                };
                generated = generated.saturating_add(1);
                if env::var_os("MANGLE_DUMP").is_some() {
                    println!("GEN {m}");
                }
                generated_names.push(m.clone());
                if let Some(e) = exports.get(&m) {
                    hit.insert(e.as_str());
                } else if verbose && shown < 400 {
                    // Known export: the only one starting with the name part.
                    let prefix = m.split('$').next().unwrap_or("");
                    let cands: Vec<&String> = exports
                        .iter()
                        .filter(|e| e.split('$').next() == Some(prefix))
                        .collect();
                    if let [only] = cands.as_slice() {
                        println!("{m}\n{only}\n");
                        shown = shown.saturating_add(1);
                    }
                }
            }
        }
        matched = matched.saturating_add(hit.len());
        if verbose {
            let prefixes: HashSet<String> = generated_names
                .iter()
                .map(|g| g.split('$').next().unwrap_or("").to_string())
                .collect();
            for e in exports.iter().filter(|e| !hit.contains(e.as_str())) {
                // The classes read Borland names; an Itanium name is only
                // reported as unmatched.
                let class = if e.starts_with("_Z") {
                    "itanium"
                } else if e.split('$').next().unwrap_or("").contains('%') {
                    "generic"
                } else if prefixes.contains(e.split('$').next().unwrap_or("")) {
                    "params"
                } else {
                    "undeclared"
                };
                println!("UNMATCHED {class} {e}");
                if class != "undeclared" && class != "itanium" {
                    let prefix = e.split('$').next().unwrap_or("");
                    for g in generated_names
                        .iter()
                        .filter(|g| g.split('$').next() == Some(prefix))
                        .take(3)
                    {
                        println!("  ours {g}");
                    }
                }
            }
        }
        generated_names.clear();
    }
    println!("generated {generated} names; matched {matched} of {functions} exported functions");
}
