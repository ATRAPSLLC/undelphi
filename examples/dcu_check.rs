//! Decodes every `.dcu` (and every unit inside every `.dcp`) under the given
//! paths and reports the ones that fail, with the error.
//!
//! ```sh
//! cargo run --release --example dcu_check -- path/to/lib/win32/release
//! ```
//!
//! A unit decodes only when it parses to its final tag at exactly the end of
//! the image, so a clean run over a compiler's library is a strong check of
//! the record layouts for that version.

use std::{env, fs, path::Path, process};

use undelphi::dcu::{Unit, package::Package};

fn main() {
    let paths: Vec<String> = env::args().skip(1).collect();
    if paths.is_empty() {
        eprintln!("usage: dcu_check <dir-or-file>...");
        process::exit(2);
    }
    let mut stats = Stats::default();
    for p in &paths {
        walk(Path::new(p), &mut stats);
    }
    println!(
        "units: {} ok, {} failed; packages: {} ({} units, {} failed)",
        stats.ok, stats.failed, stats.packages, stats.package_units, stats.package_failed
    );
    if stats.failed > 0 || stats.package_failed > 0 {
        process::exit(1);
    }
}

#[derive(Default)]
struct Stats {
    ok: usize,
    failed: usize,
    packages: usize,
    package_units: usize,
    package_failed: usize,
}

fn walk(path: &Path, stats: &mut Stats) {
    if path.is_dir() {
        let Ok(entries) = fs::read_dir(path) else {
            return;
        };
        let mut entries: Vec<_> = entries.flatten().map(|e| e.path()).collect();
        entries.sort();
        for e in entries {
            walk(&e, stats);
        }
        return;
    }
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    let Ok(data) = fs::read(path) else {
        return;
    };
    match ext.as_deref() {
        Some("dcu") => match Unit::parse(&data) {
            Ok(_) => stats.ok = stats.ok.saturating_add(1),
            Err(e) => {
                stats.failed = stats.failed.saturating_add(1);
                println!("{}: {e}", path.display());
            }
        },
        Some("dcp") => {
            stats.packages = stats.packages.saturating_add(1);
            match Package::parse(&data, false) {
                Ok(pkg) => {
                    for u in &pkg.units {
                        stats.package_units = stats.package_units.saturating_add(1);
                        let Some(bytes) = pkg.unit_data(&data, &u.name) else {
                            continue;
                        };
                        if let Err(e) = Unit::parse_from_package(bytes) {
                            stats.package_failed = stats.package_failed.saturating_add(1);
                            println!("{} [{}]: {e}", path.display(), u.name);
                        }
                    }
                }
                Err(e) => {
                    stats.package_failed = stats.package_failed.saturating_add(1);
                    println!("{}: {e}", path.display());
                }
            }
        }
        _ => {}
    }
}
