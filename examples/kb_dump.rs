//! Dumps an IDR knowledge base: its header, section sizes and units, and
//! with a name argument every constant, type, variable, resource string
//! and routine of that name.
//!
//! ```sh
//! cargo run --release --example kb_dump -- kb6.bin [name]
//! ```

use std::{env, fs, process};

use undelphi::kb::{ArgTag, CallKind, KnowledgeBase, Proc, ProcKind};

fn main() {
    let mut args = env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("usage: kb_dump <kb.bin> [name]");
        process::exit(2);
    };
    let name = args.next();
    let data = fs::read(&path).unwrap_or_else(|e| {
        eprintln!("{path}: {e}");
        process::exit(1);
    });
    let kb = KnowledgeBase::parse(&data).unwrap_or_else(|e| {
        eprintln!("{path}: {e}");
        process::exit(1);
    });
    let h = kb.header();
    println!(
        "{:?} msil={} delphi={} crc={:#x}",
        h.format, h.is_msil, h.delphi_version, h.crc
    );
    println!(
        "modules {} consts {} types {} vars {} resstrings {} procs {}",
        kb.modules().len(),
        kb.consts().len(),
        kb.types().len(),
        kb.vars().len(),
        kb.res_strings().len(),
        kb.procs().len()
    );
    let module_name = |id: u16| {
        kb.module(id)
            .ok()
            .flatten()
            .map_or_else(|| format!("#{id}"), |m| m.name.to_string())
    };
    let Some(name) = name else {
        for m in kb.modules().iter().flatten() {
            let uses: Vec<String> = m.uses.iter().map(|u| u.name.to_string()).collect();
            println!(
                "unit {} {} ({}) uses {}",
                m.id,
                m.name,
                m.file_name,
                uses.join(", ")
            );
        }
        return;
    };
    let key = name.as_bytes();
    for c in kb
        .consts()
        .named(key)
        .into_iter()
        .flat_map(|i| kb.consts().get(i))
    {
        println!(
            "const {}.{} {:?} : {} = {} ({} bytes)",
            module_name(c.module),
            c.name,
            c.kind,
            c.type_def,
            c.value,
            c.dump.code.len()
        );
    }
    for t in kb
        .types()
        .named(key)
        .into_iter()
        .flat_map(|i| kb.types().get(i))
    {
        println!(
            "type {}.{} {:?} size={} vmt={} = {} ({} bytes)",
            module_name(t.module),
            t.name,
            t.kind,
            t.size,
            t.vmt_count,
            t.decl,
            t.dump.code.len()
        );
        for f in &t.fields {
            println!(
                "  field {:?} +{} {}: {}",
                f.scope, f.offset, f.name, f.type_def
            );
        }
        for p in &t.properties {
            println!(
                "  property {:?} {}: {} read {} write {}",
                p.scope, p.name, p.type_def, p.read, p.write
            );
        }
        for m in &t.methods {
            println!("  method {:?} {}", m.scope, m.prototype);
        }
    }
    for v in kb
        .vars()
        .named(key)
        .into_iter()
        .flat_map(|i| kb.vars().get(i))
    {
        println!(
            "var {}.{} {:?}: {}",
            module_name(v.module),
            v.name,
            v.kind,
            v.type_def
        );
    }
    for r in kb
        .res_strings()
        .named(key)
        .into_iter()
        .flat_map(|i| kb.res_strings().get(i))
    {
        println!(
            "resourcestring {}.{} = {}",
            module_name(r.module),
            r.name,
            r.context
        );
    }
    for p in kb
        .procs()
        .named(key)
        .into_iter()
        .flat_map(|i| kb.procs().get(i))
    {
        println!("{}: {}", module_name(p.module), prototype(&p));
        let d = &p.dump;
        println!("  code   {}", hex(&d.code));
        println!("  relocs {}", hex(&d.relocs));
        for f in &d.fixups {
            println!("  fixup {:?} +{} {}", f.kind, f.offset, f.name);
        }
    }
}

/// Renders a routine's prototype the way IDR's `GetProcPrototype` does.
fn prototype(p: &Proc<'_>) -> String {
    let kind = match p.kind {
        ProcKind::Constructor => "constructor ",
        ProcKind::Destructor => "destructor ",
        ProcKind::Function => "function ",
        ProcKind::Procedure => "procedure ",
        _ => "",
    };
    let args: Vec<String> = p
        .args
        .iter()
        .map(|a| {
            let var = if a.tag == ArgTag::Var { "var " } else { "" };
            format!("{var}{}:{}", a.name, a.type_def)
        })
        .collect();
    let args = if args.is_empty() {
        String::new()
    } else {
        format!("({})", args.join("; "))
    };
    let result = if p.kind == ProcKind::Function {
        format!(":{}", p.type_def)
    } else {
        String::new()
    };
    let call = match p.call {
        CallKind::Register => "",
        CallKind::Cdecl => " cdecl;",
        CallKind::Pascal => " pascal;",
        CallKind::Stdcall => " stdcall;",
        CallKind::Safecall => " safecall;",
        CallKind::Other(_) => " ?;",
    };
    format!("{kind}{}{args}{result};{call}", p.name)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
