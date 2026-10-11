//! Dumps a decoded DCU: header, uses, the type table and every declaration
//! with its decoded fields, for inspecting the decoder's view of a unit.
//!
//! ```sh
//! cargo run --release --example dcu_dump -- Unit.dcu [name-filter]
//! ```
//!
//! With a filter, only declarations whose name contains it are printed.

use std::{env, fs, process};

use undelphi::dcu::{TypeTarget, Unit};

fn main() {
    let mut args = env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("usage: dcu_dump <unit.dcu> [name-filter]");
        process::exit(2);
    };
    let filter = args.next();
    let data = fs::read(&path).unwrap_or_else(|e| {
        eprintln!("{path}: {e}");
        process::exit(1);
    });
    let unit = Unit::parse(&data).unwrap_or_else(|e| {
        eprintln!("{path}: {e}");
        process::exit(1);
    });
    if filter.is_none() {
        println!(
            "unit {} ({}, {:?}) stamp={:#x} flags={:#x} priority={}",
            unit.name,
            unit.version.product_name(),
            unit.platform,
            unit.stamp,
            unit.flags,
            unit.priority
        );
        for u in &unit.uses {
            println!(
                "uses {} {:?} addr={} symbols={}",
                u.name,
                u.section,
                u.addr,
                u.symbols.len()
            );
        }
        for i in 1..=unit.type_count() {
            let Ok(index) = u32::try_from(i) else { break };
            let Some(slot) = unit.type_slot(index) else {
                continue;
            };
            let what = match &slot.target {
                TypeTarget::Def(id) => unit
                    .type_defs
                    .get(*id)
                    .map(|t| format!("{:?}", t.kind))
                    .unwrap_or_default(),
                TypeTarget::Import { .. } => {
                    format!("import from {}", unit.type_unit(index).unwrap_or("?"))
                }
                TypeTarget::None => "none".to_string(),
            };
            let what: String = what.chars().take(160).collect();
            println!(
                "type #{index:X} {} = {what}",
                unit.type_name(index).unwrap_or("")
            );
        }
    }
    for (id, d) in unit.decls.iter().enumerate() {
        if let Some(f) = &filter
            && !d.name.contains(f.as_str())
        {
            continue;
        }
        let mut kind: String = format!("{:?}", d.kind).chars().take(240).collect();
        if !d.modifiers.is_empty() {
            let m: String = format!(" {:?}", d.modifiers).chars().take(300).collect();
            kind.push_str(&m);
        }
        println!(
            "decl {id} @{:#x} tag={:#04x} addr={:#x} {:?} {kind}",
            d.offset, d.tag, d.addr, d.name
        );
    }
}
