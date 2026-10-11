//! Delphi `resourcestring`s.
//!
//! A `resourcestring` constant compiles to two pieces (Delphi 6
//! `System.pas`):
//!
//! ```text
//!   TResStringRec = packed record
//!     Module:     ^HMODULE;   address of the module's HInstance variable
//!     Identifier: Integer;    RT_STRING identifier (pointer-sized on 64-bit)
//!   end;
//! ```
//!
//! plus the text, stored in the module's `RT_STRING` table under
//! `Identifier` ([`crate::resources::string_table`]). `LoadResString`
//! fetches the text with `LoadString(FindResourceHInstance(Module^),
//! Identifier)`; an `Identifier` of 64K or more is instead a pointer to the
//! text itself.
//!
//! Every record of a module shares one `Module` value - the address of its
//! `HInstance` - so the records are found structurally: pointer-aligned
//! `(Module, Identifier)` pairs whose identifier names an existing string,
//! keeping those whose `Module` is the value most of them share. Empirically
//! Delphi 6 places the records in CODE, ahead of the routines that use them
//! (`tests/samples/bcb6/tables.exe`: 117 strings, 117 records, all pointing
//! at the map's `HInstance`); on 64-bit both fields are pointer-sized.

use std::collections::{BTreeMap, HashMap};

use crate::{formats::BinaryContext, resources::string_table, util::read_ptr};

/// One string of the module's string table, with the `resourcestring`
/// record that names it when one exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceString {
    /// `RT_STRING` identifier.
    pub id: u32,
    /// The text.
    pub text: String,
    /// VA of the `TResStringRec` that loads it, `None` for a string no
    /// `resourcestring` refers to (e.g. one compiled from a `.rc` file).
    pub record_va: Option<u64>,
}

/// Decode the module's string table and attach each string's
/// `resourcestring` record. Empty for non-PE input.
pub fn resource_strings(ctx: &BinaryContext<'_>) -> Vec<ResourceString> {
    let table = string_table(ctx);
    if table.is_empty() {
        return Vec::new();
    }
    let ids: HashMap<u32, usize> = table
        .iter()
        .enumerate()
        .map(|(i, (id, _))| (*id, i))
        .collect();
    let records = find_records(ctx, &ids);
    table
        .into_iter()
        .map(|(id, text)| ResourceString {
            id,
            text,
            record_va: records.get(&id).copied(),
        })
        .collect()
}

/// Finds the `TResStringRec`s: pointer-aligned `(Module, Identifier)` pairs
/// in the code and data sections whose identifier is in `ids`, keeping the
/// `Module` value most pairs share. Maps identifier to record VA (the first
/// record when several load the same string).
fn find_records(ctx: &BinaryContext<'_>, ids: &HashMap<u32, usize>) -> BTreeMap<u32, u64> {
    let Some(ptr_size) = ctx.pointer_size() else {
        return BTreeMap::new();
    };
    let data = ctx.data();
    let mut candidates: Vec<(u64, u64, u32)> = Vec::new();
    for range in &ctx.sections().scan_targets {
        let end = range.offset.saturating_add(range.size).min(data.len());
        let mut off = range.offset;
        while off.saturating_add(ptr_size.saturating_mul(2)) <= end {
            if let (Some(module), Some(ident)) = (
                read_ptr(data, off, ptr_size),
                read_ptr(data, off.saturating_add(ptr_size), ptr_size),
            ) && let Ok(id) = u32::try_from(ident)
                && ids.contains_key(&id)
                && module != 0
                && ctx.is_image_va(module)
            {
                let va = range
                    .va
                    .saturating_add(u64::try_from(off.saturating_sub(range.offset)).unwrap_or(0));
                candidates.push((module, va, id));
            }
            off = off.saturating_add(ptr_size);
        }
    }
    let mut votes: HashMap<u64, usize> = HashMap::new();
    for &(module, _, _) in &candidates {
        let count = votes.entry(module).or_default();
        *count = count.saturating_add(1);
    }
    let Some((&module, _)) = votes.iter().max_by_key(|&(_, &count)| count) else {
        return BTreeMap::new();
    };
    let mut out = BTreeMap::new();
    for (m, va, id) in candidates {
        if m == module {
            out.entry(id).or_insert(va);
        }
    }
    out
}
