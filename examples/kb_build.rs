//! Builds an IDR knowledge base from a directory of DCUs.
//!
//! ```sh
//! cargo run --release --example kb_build -- path/to/lib out.bin [unit-list]
//! ```
//!
//! `unit-list` is a file with one unit name per line, limiting the build to
//! those units (for comparing with a published knowledge base whose unit
//! set is known). Units the directory holds only inside packages (`.dcp`)
//! are left out: a packaged unit carries import thunks instead of its code.
//! Each module's file name is the unit's declared name plus `.dcu`.

use std::{collections::HashSet, env, fs, path::Path, process, sync::Arc};

use undelphi::{
    dcu::library::Library,
    kb::{self, UnitFile},
};

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let (Some(dir), Some(out)) = (args.first(), args.get(1)) else {
        eprintln!("usage: kb_build <dcu dir> <out.bin> [unit-list]");
        process::exit(2);
    };
    let wanted: Option<HashSet<String>> = args.get(2).map(|list| {
        fs::read_to_string(list)
            .unwrap_or_else(|e| {
                eprintln!("{list}: {e}");
                process::exit(1);
            })
            .lines()
            .map(|l| l.trim().to_ascii_lowercase())
            .filter(|l| !l.is_empty())
            .collect()
    });
    let lib = Library::open(Path::new(dir)).unwrap_or_else(|e| {
        eprintln!("{dir}: {e}");
        process::exit(1);
    });
    let mut names: Vec<String> = lib.unit_names().map(str::to_string).collect();
    names.sort();
    let units: Vec<UnitFile> = names
        .iter()
        .filter(|n| wanted.as_ref().is_none_or(|w| w.contains(n.as_str())))
        .filter_map(|n| {
            let unit = lib.unit(n);
            if unit.is_none() {
                eprintln!("{n}: does not decode");
            }
            unit
        })
        .filter(|u| !u.from_package)
        .map(|unit: Arc<_>| UnitFile {
            file_name: format!("{}.dcu", unit.name),
            unit,
        })
        .collect();
    let contents = kb::build(&units);
    let bytes = contents.to_bytes().unwrap_or_else(|e| {
        eprintln!("{out}: {e}");
        process::exit(1);
    });
    fs::write(out, bytes).unwrap_or_else(|e| {
        eprintln!("{out}: {e}");
        process::exit(1);
    });
    println!("{} units -> {out}", units.len());
}
