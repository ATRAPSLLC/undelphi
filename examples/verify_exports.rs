//! Verifies `classes()` and `types()` against the exports of Borland runtime
//! packages.
//!
//! A runtime package (`.bpl`) exports every class VMT it defines as
//! `@Unit@Class@` and every typeinfo as `@$xp$<len>Unit@Type`, so its export
//! table is a compiler-produced answer key: each exported VMT must be a class
//! `classes()` finds under the same name, each exported typeinfo a type
//! `types()` finds under the same name.
//!
//! ```sh
//! cargo run --release --example verify_exports -- path/to/*.bpl
//! ```
//!
//! Three kinds of export are not definitions and are reported separately:
//!
//! - an export that is an import slot (the package re-exports a symbol it
//!   imports),
//! - a C++Builder package's local stub of an imported Pascal class or type,
//!   whose `ClassType` / VMT fields point at import thunks,
//! - an old-style `object` type, which is exported like a class but has a
//!   header-less VMT and is deliberately not reported as a class.
//!
//! Names are compared after the demangler's RTTI-style rendering; see
//! `same_type_name` for the remaining conventions (nested types, generic
//! arguments, truncated ShortString names).
//!
//! Results, every definition a Delphi-produced package exports found under
//! its own name:
//!
//! | Package set | Classes | Types |
//! |-------------|---------|-------|
//! | C++Builder 6 (104 packages, Delphi 6 compiler) | 6007 | 11320 |
//! | RAD Studio 10 Seattle Win32 (325 packages) | 22084 | 67930 |
//! | RAD Studio 10 Seattle Win64 (120 packages, Itanium names) | 14325 | 49578 |
//!
//! The only exports not found are the three non-definition kinds above
//! (C++Builder 6: 62 stubs and 4 `object` types; Seattle Win32: 32 stubs in
//! the C++Builder package `bcbsmpc230.bpl` and 3 `object` types).

use std::{env, fs, process};

use undelphi::{
    DelphiBinary,
    imports::{BorlandSymbol, SymbolKind},
};

fn main() {
    let paths: Vec<String> = env::args().skip(1).collect();
    if paths.is_empty() {
        eprintln!("usage: verify_exports <package.bpl>...");
        process::exit(2);
    }
    let mut totals = Totals::default();
    for path in &paths {
        let data = match fs::read(path) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("{path}: {e}");
                continue;
            }
        };
        let Ok(bin) = DelphiBinary::parse(&data) else {
            println!("{path}: not recognised");
            continue;
        };
        let report = verify(&bin);
        if !report.misses.is_empty() {
            println!(
                "{path}: classes {}/{} types {}/{}",
                report.classes_found, report.classes, report.types_found, report.types
            );
            for miss in &report.misses {
                println!("    {miss}");
            }
        }
        totals.add(&report);
    }
    println!(
        "total: classes {}/{} types {}/{}",
        totals.classes_found, totals.classes, totals.types_found, totals.types
    );
}

#[derive(Default)]
struct Totals {
    classes: usize,
    classes_found: usize,
    types: usize,
    types_found: usize,
}

impl Totals {
    fn add(&mut self, r: &Report) {
        self.classes = self.classes.saturating_add(r.classes);
        self.classes_found = self.classes_found.saturating_add(r.classes_found);
        self.types = self.types.saturating_add(r.types);
        self.types_found = self.types_found.saturating_add(r.types_found);
    }
}

#[derive(Default)]
struct Report {
    classes: usize,
    classes_found: usize,
    types: usize,
    types_found: usize,
    misses: Vec<String>,
}

fn verify(bin: &DelphiBinary<'_>) -> Report {
    let mut report = Report::default();
    let types: Vec<(u64, String)> = bin
        .types()
        .iter()
        .map(|t| (t.header().va, t.header().name().to_string()))
        .collect();
    let ptr_size = bin.ctx().pointer_size().unwrap_or(4);
    for (name, va) in bin.exports() {
        let Some(symbol) = BorlandSymbol::demangle(name) else {
            continue;
        };
        if bin.import_at(va).is_some() {
            continue;
        }
        let short = symbol.name.as_str();
        match symbol.kind {
            SymbolKind::ClassVmt => {
                report.classes = report.classes.saturating_add(1);
                // The export names the class pointer, just past the header.
                let class = bin.classes().iter().find(|c| {
                    let header = u64::from(c.header_slots()).saturating_mul(ptr_size as u64);
                    c.vmt_va() == va || c.vmt_va().saturating_add(header) == va
                });
                match class {
                    Some(c) if same_type_name(c.name(), short) => {
                        report.classes_found = report.classes_found.saturating_add(1)
                    }
                    Some(c) => report
                        .misses
                        .push(format!("class {name}: found as {}", c.name())),
                    None => report
                        .misses
                        .push(format!("class {name} at {va:#x} not found")),
                }
            }
            SymbolKind::TypeInfo => {
                report.types = report.types.saturating_add(1);
                let cell = bin
                    .ctx()
                    .va_to_file(va)
                    .and_then(|o| bin.ctx().data().get(o..o.saturating_add(ptr_size)))
                    .map(|b| {
                        b.iter()
                            .rev()
                            .fold(0u64, |acc, &byte| (acc << 8) | u64::from(byte))
                    });
                let found = types
                    .iter()
                    .find(|(t, _)| *t == va || Some(*t) == cell)
                    .map(|(_, n)| n.as_str());
                match found {
                    Some(n) if same_type_name(n, short) => {
                        report.types_found = report.types_found.saturating_add(1)
                    }
                    Some(n) => report.misses.push(format!("type {name}: found as {n}")),
                    None => report
                        .misses
                        .push(format!("type {name} at {va:#x} not found")),
                }
            }
            SymbolKind::Routine => {}
        }
    }
    report
}

/// Compares an RTTI or VMT name with the demangled export name (without
/// its unit). The demangler renders names the way RTTI does, so the
/// remaining differences are:
///
/// - Modern RTTI names a nested type with its outer type
///   (`TEditButton.TGlyph`, `:TApplication.:1`); the export's name part is
///   the innermost name.
/// - Generic instances: RTTI renders the type arguments with Delphi alias
///   names (`System.string`, `TArray<..>`), the export with the underlying
///   types (`System.UnicodeString`, `DynamicArray<..>`), so only the
///   generic base name and the nested suffix are compared.
/// - Delphi 6 names anonymous types `.N` where later compilers write `:N`.
/// - RTTI names are ShortStrings: a long generic instance name is cut short
///   (a typeinfo name at 255 bytes; a VMT class name, in the one Seattle
///   Win64 case, where `Unit.Class` reaches 255), so a name whose angle
///   brackets do not balance is compared by its generic base name only.
/// - A nested type can appear in the export flattened with `_`
///   (`TStyleManager_TStyleServicesHandle` in Seattle's Win64 `vcl230.bpl`).
fn same_type_name(rtti: &str, export: &str) -> bool {
    let truncated = rtti.matches('<').count() > rtti.matches('>').count();
    if truncated && let (Some(r), Some(e)) = (rtti.split_once('<'), export.split_once('<')) {
        return r.0.eq_ignore_ascii_case(e.0);
    }
    if !rtti.contains('<') && rtti.replace('.', "_").eq_ignore_ascii_case(export) {
        return true;
    }
    let outer = |name: &str| -> String {
        // `:N` (modern) and `.N` (Delphi 6) name the same anonymous type.
        let name = name.replace(':', ".");
        match (name.find('<'), name.rfind('>')) {
            (Some(open), Some(close)) => format!(
                "{}<>{}",
                name.get(..open).unwrap_or(""),
                name.get(close.saturating_add(1)..).unwrap_or("")
            ),
            _ => name,
        }
    };
    let (rtti, export) = (outer(rtti), outer(export));
    let lower = |s: &str| s.to_ascii_lowercase();
    lower(&rtti) == lower(&export) || lower(&rtti).ends_with(&format!(".{}", lower(&export)))
}
