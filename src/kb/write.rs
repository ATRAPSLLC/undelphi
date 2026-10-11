//! Writing a knowledge base: the record encoders' primitives and
//! [`Contents::to_bytes`].

use std::{cmp::Ordering, error, fmt};

use crate::kb::{
    Const, Contents, DESCRIPTION_LEN, Dump, ENTRY_SIZE, Format, HEADER_SIZE, Module, Proc,
    RESERVED_OFFSET, Record, ResString, SIGNATURE_LEN, Text, Type, Var, cmp_ignore_case,
};

/// Why a knowledge base could not be written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteError {
    /// A string, list or block exceeds the width of its length field; the
    /// value names what.
    TooLarge(&'static str),
}

impl fmt::Display for WriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge(what) => write!(f, "{what} too large for the knowledge-base format"),
        }
    }
}

impl error::Error for WriteError {}

/// Appends a `u16`.
pub fn put_u16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_le_bytes());
}

/// Appends a `u32`.
pub fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

/// Appends an `i32`.
pub fn put_i32(out: &mut Vec<u8>, v: i32) {
    out.extend_from_slice(&v.to_le_bytes());
}

/// Appends a list count as a `u16`.
pub fn put_count(out: &mut Vec<u8>, n: usize, what: &'static str) -> Result<(), WriteError> {
    put_u16(
        out,
        u16::try_from(n).map_err(|_| WriteError::TooLarge(what))?,
    );
    Ok(())
}

/// Appends a length as a `u32`.
fn put_len(out: &mut Vec<u8>, n: usize, what: &'static str) -> Result<(), WriteError> {
    put_u32(
        out,
        u32::try_from(n).map_err(|_| WriteError::TooLarge(what))?,
    );
    Ok(())
}

/// Appends a string: `u16` length, bytes, NUL.
pub fn put_text(out: &mut Vec<u8>, t: &Text<'_>) -> Result<(), WriteError> {
    put_count(out, t.as_bytes().len(), "string")?;
    out.extend_from_slice(t.as_bytes());
    out.push(0);
    Ok(())
}

/// Appends a block preceded by its `u32` size.
pub fn put_block(out: &mut Vec<u8>, body: &[u8]) -> Result<(), WriteError> {
    put_len(out, body.len(), "block")?;
    out.extend_from_slice(body);
    Ok(())
}

/// Appends a block that `fill` writes, preceded by its `u32` size.
fn sized(
    out: &mut Vec<u8>,
    what: &'static str,
    fill: impl FnOnce(&mut Vec<u8>) -> Result<(), WriteError>,
) -> Result<(), WriteError> {
    let at = out.len();
    out.extend_from_slice(&[0; 4]);
    fill(out)?;
    let size = out
        .len()
        .checked_sub(at)
        .and_then(|n| n.checked_sub(4))
        .and_then(|n| u32::try_from(n).ok())
        .ok_or(WriteError::TooLarge(what))?;
    let end = at.checked_add(4).ok_or(WriteError::TooLarge(what))?;
    if let Some(slot) = out.get_mut(at..end) {
        slot.copy_from_slice(&size.to_le_bytes());
    }
    Ok(())
}

/// Appends a counted list block: `u32` total, `u16` count, the items.
pub fn put_list<T>(
    out: &mut Vec<u8>,
    items: &[T],
    what: &'static str,
    mut item: impl FnMut(&mut Vec<u8>, &T) -> Result<(), WriteError>,
) -> Result<(), WriteError> {
    sized(out, what, |out| {
        put_count(out, items.len(), what)?;
        items.iter().try_for_each(|i| item(out, i))
    })
}

/// Appends a [`Dump`].
pub fn put_dump(out: &mut Vec<u8>, d: &Dump<'_>) -> Result<(), WriteError> {
    if d.relocs.len() != d.code.len() {
        return Err(WriteError::TooLarge(
            "relocation mask (differs from the dump)",
        ));
    }
    sized(out, "dump", |out| {
        put_len(out, d.code.len(), "dump")?;
        put_u32(out, d.fixup_count);
        out.extend_from_slice(&d.code);
        out.extend_from_slice(&d.relocs);
        for f in &d.fixups {
            out.push(f.kind.into());
            put_u32(out, f.offset);
            put_text(out, &f.name)?;
        }
        Ok(())
    })
}

/// A section written to the record area, before its table is.
struct Written {
    /// `(offset, size)` of each record, in record order.
    spans: Vec<(u32, u32)>,
    /// Record indices in (module, name) order.
    by_module: Vec<u32>,
    /// Record indices in name order.
    by_name: Vec<u32>,
}

/// Appends a section's records and computes its two orders.
fn section<'a, R: Record<'a>>(
    out: &mut Vec<u8>,
    records: &[R],
    format: Format,
) -> Result<Written, WriteError> {
    let mut spans = Vec::with_capacity(records.len());
    for r in records {
        let at = out.len();
        r.encode(out, format)?;
        let size = out.len().saturating_sub(at);
        spans.push((
            u32::try_from(at).map_err(|_| WriteError::TooLarge("file"))?,
            u32::try_from(size).map_err(|_| WriteError::TooLarge("record"))?,
        ));
    }
    // IDR's maker sorted with Delphi's `TList.Sort`: the module order
    // from file order, the name order from the module order. The same
    // QuickSort reproduces how it leaves records with equal keys
    // (overloads).
    let names: Vec<&[u8]> = records.iter().map(|r| r.name().as_bytes()).collect();
    let modules: Vec<u16> = records.iter().map(Record::module).collect();
    let name = |i: usize| names.get(i).copied().unwrap_or_default();
    let module = |i: usize| modules.get(i).copied().unwrap_or_default();
    let mut by_module: Vec<usize> = (0..records.len()).collect();
    delphi_quick_sort(&mut by_module, |a, b| {
        module(a)
            .cmp(&module(b))
            .then_with(|| cmp_ignore_case(name(a), name(b)))
    });
    let mut by_name = by_module.clone();
    delphi_quick_sort(&mut by_name, |a, b| {
        cmp_ignore_case(name(a), name(b)).then_with(|| module(a).cmp(&module(b)))
    });
    let to_u32 = |v: Vec<usize>| -> Result<Vec<u32>, WriteError> {
        v.into_iter()
            .map(|i| u32::try_from(i).map_err(|_| WriteError::TooLarge("section")))
            .collect()
    };
    Ok(Written {
        by_module: to_u32(by_module)?,
        by_name: to_u32(by_name)?,
        spans,
    })
}

/// Delphi's `Classes.QuickSort` (what `TList.Sort` runs), with its middle
/// pivot and Hoare partition, so records with equal keys end up where
/// Delphi leaves them. The recursion on the left part is an explicit
/// stack.
fn delphi_quick_sort(list: &mut [usize], cmp: impl Fn(usize, usize) -> Ordering) {
    let Some(last) = list.len().checked_sub(1) else {
        return;
    };
    let at = |list: &[usize], i: usize| list.get(i).copied().unwrap_or_default();
    let mut stack = vec![(0usize, last)];
    while let Some((mut l, r)) = stack.pop() {
        loop {
            let (mut i, mut j) = (l, r);
            let pivot = at(list, l.midpoint(r));
            loop {
                while cmp(at(list, i), pivot) == Ordering::Less {
                    i = i.saturating_add(1);
                }
                while cmp(at(list, j), pivot) == Ordering::Greater {
                    j = j.saturating_sub(1);
                }
                if i <= j {
                    list.swap(i, j);
                    i = i.saturating_add(1);
                    // `j` reaching below zero ends the partition.
                    match j.checked_sub(1) {
                        Some(next) => j = next,
                        None => break,
                    }
                }
                if i > j {
                    break;
                }
            }
            // Delphi recurses into [l, j] first, then loops on [i, r].
            if l < j {
                stack.push((l, j));
            }
            l = i;
            if i >= r {
                break;
            }
        }
    }
}

/// Appends a section's table.
fn table(out: &mut Vec<u8>, w: &Written) -> Result<(), WriteError> {
    let count = i32::try_from(w.spans.len()).map_err(|_| WriteError::TooLarge("section"))?;
    let max = w.spans.iter().map(|&(_, size)| size).max().unwrap_or(0);
    put_i32(out, count);
    put_u32(out, max);
    out.reserve(w.spans.len().saturating_mul(ENTRY_SIZE));
    for ((&(offset, size), &by_module), &by_name) in
        w.spans.iter().zip(&w.by_module).zip(&w.by_name)
    {
        put_u32(out, offset);
        put_u32(out, size);
        put_u32(out, by_module);
        put_u32(out, by_name);
    }
    Ok(())
}

/// Appends `bytes` NUL-padded to `width` (both signatures are shorter than
/// their field, so they keep the NUL IDR's `strcmp` needs).
fn padded(
    out: &mut Vec<u8>,
    bytes: &[u8],
    width: usize,
    what: &'static str,
) -> Result<(), WriteError> {
    if bytes.len() > width {
        return Err(WriteError::TooLarge(what));
    }
    out.extend_from_slice(bytes);
    out.resize(
        out.len().saturating_add(width.saturating_sub(bytes.len())),
        0,
    );
    Ok(())
}

impl Contents<'_> {
    /// Encodes every record into a knowledge base in the format of
    /// [`Header::format`](crate::kb::Header::format).
    ///
    /// Records are written in their order here; both lookup orders are
    /// computed (ties keep record order).
    ///
    /// # Errors
    ///
    /// Returns [`WriteError::TooLarge`] when a string, list, dump or the
    /// file exceeds the format's length fields.
    pub fn to_bytes(&self) -> Result<Vec<u8>, WriteError> {
        let format = self.header.format;
        let mut out = Vec::new();
        padded(&mut out, format.signature(), SIGNATURE_LEN, "signature")?;
        out.push(u8::from(self.header.is_msil));
        put_i32(&mut out, self.header.delphi_version);
        put_u32(&mut out, self.header.crc);
        padded(
            &mut out,
            self.header.description.as_bytes(),
            DESCRIPTION_LEN,
            "description",
        )?;
        put_u32(&mut out, format.version());
        // The 16 reserved bytes end the header.
        out.resize(RESERVED_OFFSET.max(out.len()), 0);
        out.resize(HEADER_SIZE, 0);

        let sections = [
            section::<Module<'_>>(&mut out, &self.modules, format)?,
            section::<Const<'_>>(&mut out, &self.consts, format)?,
            section::<Type<'_>>(&mut out, &self.types, format)?,
            section::<Var<'_>>(&mut out, &self.vars, format)?,
            section::<ResString<'_>>(&mut out, &self.res_strings, format)?,
            section::<Proc<'_>>(&mut out, &self.procs, format)?,
        ];
        let tables = u32::try_from(out.len()).map_err(|_| WriteError::TooLarge("file"))?;
        for s in &sections {
            table(&mut out, s)?;
        }
        put_u32(&mut out, tables);
        Ok(out)
    }
}
