//! Prints the structural facts of a DCU in a line format that can be
//! compared with DCU32INT's `-S` output: every type definition with its
//! size, RTTI size and RTTI address, every field with its type, flags and
//! offset, and every routine with its parameters and result.
//!
//! ```sh
//! cargo run --release --example dcu_facts -- Unit.dcu
//! ```
//!
//! Indices are printed in hexadecimal and offsets in decimal, as DCU32INT
//! prints them:
//!
//! ```text
//! T <type index> sz=<size> rtti=<rtti size> addr=<rtti address>
//! F <name> T<type index> f=<flags> ofs=<offset>
//! P <name> b0=<flags> sz=<size> vproc=<vproc> res=T<type index>
//! A <routine> <val|var> <name> T<type index> f=<flags> ofs=<location>
//! ```

use std::{env, fs, process};

use undelphi::dcu::{DeclKind, TypeKind, Unit};

fn main() {
    let Some(path) = env::args().nth(1) else {
        eprintln!("usage: dcu_facts <unit.dcu>");
        process::exit(2);
    };
    let data = match fs::read(&path) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("{path}: {e}");
            process::exit(1);
        }
    };
    let unit = match Unit::parse(&data) {
        Ok(u) => u,
        Err(e) => {
            eprintln!("{path}: {e}");
            process::exit(1);
        }
    };
    for t in &unit.type_defs {
        println!(
            "T {:X} sz={:X} rtti={:X} addr={:X}",
            t.type_index, t.size as u32, t.rtti_size, t.name_decl
        );
        let members = match &t.kind {
            TypeKind::Record { members, .. } | TypeKind::Object { members, .. } => members,
            TypeKind::Class(c) => &c.members,
            _ => continue,
        };
        for d in unit.decls_of(members) {
            if let DeclKind::Local(l) = &d.kind
                && d.tag == 0x2C
            {
                println!(
                    "F {} T{:X} f={:X} ofs={}",
                    d.name, l.type_index, l.flags, l.location
                );
            }
        }
    }
    for d in &unit.decls {
        let DeclKind::Proc(p) = &d.kind else {
            continue;
        };
        if p.unnamed {
            continue;
        }
        println!(
            "P {} b0={:X} sz={:X} vproc={:X} res=T{:X}",
            d.name, p.proc_flags, p.size, p.vproc, p.result_type
        );
        for a in unit.decls_of(&p.params) {
            if let DeclKind::Local(l) = &a.kind {
                let mode = if a.tag == 0x22 { "var" } else { "val" };
                println!(
                    "A {} {mode} {} T{:X} f={:X} ofs={}",
                    d.name, a.name, l.type_index, l.flags, l.location
                );
            }
        }
    }
}
