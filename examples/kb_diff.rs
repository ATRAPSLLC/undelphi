//! Compares two IDR knowledge bases record by record: for each section, how
//! many records match exactly, differ, or exist on one side only, and the
//! first differences. Records are paired by unit name, record name and
//! occurrence (overloads in file order); module IDs are compared through the
//! names they stand for, so files with different unit sets compare.
//!
//! ```sh
//! cargo run --release --example kb_diff -- reference.bin generated.bin [section] [count]
//! ```
//!
//! `section` limits the listing to one of `modules`, `consts`, `types`,
//! `vars`, `resstrings`, `procs`; `count` is the number of differences
//! listed per section (default 5). Only units present in both files are
//! compared.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    env, fmt, fs, process,
};

use undelphi::kb::{KnowledgeBase, Record, Section};

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let (Some(a), Some(b)) = (args.first(), args.get(1)) else {
        eprintln!("usage: kb_diff <reference.bin> <other.bin> [section] [count]");
        process::exit(2);
    };
    let only = args.get(2).map(String::as_str);
    let count: usize = args.get(3).and_then(|c| c.parse().ok()).unwrap_or(5);
    let (da, db) = (read(a), read(b));
    let (ka, kb) = (parse(a, &da), parse(b, &db));
    let (na, nb) = (module_names(&ka), module_names(&kb));
    let common: HashSet<String> = na
        .values()
        .filter(|n| nb.values().any(|m| m == *n))
        .cloned()
        .collect();
    println!(
        "units: {} reference, {} other, {} common",
        na.len(),
        nb.len(),
        common.len()
    );
    let ctx = Ctx {
        na: &na,
        nb: &nb,
        common: &common,
        only,
        count,
    };
    ctx.section("modules", &ka.modules(), &kb.modules(), |m, names| {
        let uses: Vec<String> = m
            .uses
            .iter()
            .map(|u| format!("{}#{}", u.name, names_of(names, u.id)))
            .collect();
        format!("{} {} uses [{}]", m.name, m.file_name, uses.join(", "))
    });
    ctx.section("consts", &ka.consts(), &kb.consts(), |c, _| {
        let mut c = c.clone();
        c.module = 0;
        format!("{c:?}")
    });
    ctx.section("types", &ka.types(), &kb.types(), |t, _| {
        let mut t = t.clone();
        t.module = 0;
        format!("{t:?}")
    });
    ctx.section("vars", &ka.vars(), &kb.vars(), |v, _| {
        let mut v = v.clone();
        v.module = 0;
        format!("{v:?}")
    });
    ctx.section(
        "resstrings",
        &ka.res_strings(),
        &kb.res_strings(),
        |r, _| {
            let mut r = r.clone();
            r.module = 0;
            format!("{r:?}")
        },
    );
    ctx.section("procs", &ka.procs(), &kb.procs(), |p, _| {
        let mut p = p.clone();
        p.module = 0;
        format!("{p:?}")
    });
}

fn read(path: &str) -> Vec<u8> {
    fs::read(path).unwrap_or_else(|e| {
        eprintln!("{path}: {e}");
        process::exit(1);
    })
}

fn parse<'a>(path: &str, data: &'a [u8]) -> KnowledgeBase<'a> {
    KnowledgeBase::parse(data).unwrap_or_else(|e| {
        eprintln!("{path}: {e}");
        process::exit(1);
    })
}

fn module_names(kb: &KnowledgeBase<'_>) -> HashMap<u16, String> {
    kb.modules()
        .iter()
        .flatten()
        .map(|m| (m.id, m.name.to_string().to_ascii_uppercase()))
        .collect()
}

fn names_of(names: &HashMap<u16, String>, id: u16) -> String {
    names.get(&id).cloned().unwrap_or_else(|| format!("{id}"))
}

struct Ctx<'c> {
    na: &'c HashMap<u16, String>,
    nb: &'c HashMap<u16, String>,
    common: &'c HashSet<String>,
    only: Option<&'c str>,
    count: usize,
}

impl Ctx<'_> {
    /// Pairs and compares one section; `show` renders a record with its
    /// module ID neutralised.
    fn section<'a, R: Record<'a> + fmt::Debug>(
        &self,
        what: &str,
        a: &Section<'_, 'a, R>,
        b: &Section<'_, 'a, R>,
        show: impl Fn(&R, &HashMap<u16, String>) -> String,
    ) {
        if self.only.is_some_and(|o| o != what) {
            return;
        }
        let ra = self.keyed(a, self.na);
        let rb = self.keyed(b, self.nb);
        let (mut same, mut differ, mut only_a, mut only_b) = (0usize, 0usize, 0usize, 0usize);
        let mut shown_diff = 0usize;
        let mut missing = Vec::new();
        let mut extra = Vec::new();
        for (key, x) in &ra {
            match rb.get(key) {
                Some(y) => {
                    let (sx, sy) = (show(x, self.na), show(y, self.nb));
                    if sx == sy {
                        same = same.saturating_add(1);
                    } else {
                        differ = differ.saturating_add(1);
                        if shown_diff < self.count {
                            shown_diff = shown_diff.saturating_add(1);
                            println!("  {what} differs {key:?}\n    ref {sx}\n    got {sy}");
                        }
                    }
                }
                None => {
                    only_a = only_a.saturating_add(1);
                    missing.push(key);
                }
            }
        }
        for key in rb.keys() {
            if !ra.contains_key(key) {
                only_b = only_b.saturating_add(1);
                extra.push(key);
            }
        }
        for key in missing.iter().take(self.count) {
            println!("  {what} missing {key:?}");
        }
        for key in extra.iter().take(self.count) {
            println!("  {what} extra {key:?}");
        }
        println!("{what}: {same} same, {differ} differ, {only_a} missing, {only_b} extra");
    }

    /// The section's records of common units, keyed by (unit, name,
    /// occurrence).
    fn keyed<'a, R: Record<'a>>(
        &self,
        s: &Section<'_, 'a, R>,
        names: &HashMap<u16, String>,
    ) -> BTreeMap<(String, String, usize), R> {
        let mut out = BTreeMap::new();
        let mut seen: HashMap<(String, String), usize> = HashMap::new();
        for r in s.iter().flatten() {
            let unit = names_of(names, r.module());
            if !self.common.contains(&unit) {
                continue;
            }
            let name = r.name().to_string();
            let n = seen.entry((unit.clone(), name.clone())).or_default();
            out.insert((unit, name, *n), r);
            *n = n.saturating_add(1);
        }
        out
    }
}
