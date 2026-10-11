//! Where each declaration's code and data lie in a unit's block.
//!
//! A DCU keeps all of a unit's machine code, typed constants and RTTI in one
//! block (`drCBlock`, [`Unit::block`]) and says which part belongs to which
//! declaration only through its fixup table: a fixup of the start or end
//! kind at offset `o` naming address `h` opens a part that runs to the next
//! such fixup and belongs to declaration `h` (DCU32INT `LoadFixups` /
//! `TUnit.SetDeclMem`). A routine takes its own size from the part and hands
//! any rest to the routine declared after it (an `{$L}` object file can hold
//! several); a type takes its RTTI; a variable, typed constant or string
//! constant takes the part. The block after the last marker is not assigned
//! by a marker.
//!
//! The start and end kinds depend on the release (DCU32INT `SetupFixups`):
//! `3` / `4` in Delphi 2, `5` / `6` in Delphi 3 - 6 and Kylix 1 - 2, `6` /
//! `7` in Delphi 7 - 2005 and Kylix 3, `$0C` / `$0D` in 2006 - 2009, `0` /
//! `1` from 2010. Every other kind patches the bytes at its offset with an
//! address; [`Unit::fixup_size`] says how many.

use std::ops::Range;

use crate::dcu::{AddrTarget, DeclId, DeclKind, Fixup, Unit, Version, version::Platform};

/// The part of a unit's block one declaration occupies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclMemory {
    /// The declaration (an index into [`Unit::decls`]).
    pub decl: DeclId,
    /// Its byte range in [`Unit::block`].
    pub range: Range<usize>,
}

impl Unit {
    /// The fixup kinds that mark the start and the end of a declaration's
    /// part of the block.
    fn marker_kinds(&self) -> (u8, u8) {
        let v = self.version;
        if self.msil {
            (0x0B, 0x0C)
        } else if v == Version::DELPHI_2 {
            (3, 4)
        } else if v.delphi_at_least(Version::DELPHI_2010) {
            (0, 1)
        } else if v.delphi_at_least(Version::DELPHI_2006) {
            (0x0C, 0x0D)
        } else if v.delphi_at_least(Version::DELPHI_7) || v == Version::KYLIX_3 {
            (6, 7)
        } else {
            (5, 6)
        }
    }

    /// Returns whether `fixup` marks the start or end of a declaration's
    /// part.
    #[must_use]
    pub fn is_marker(&self, fixup: &Fixup) -> bool {
        let (start, end) = self.marker_kinds();
        fixup.kind == start || fixup.kind == end
    }

    /// Returns the number of bytes `fixup` patches at its offset (the
    /// address it stands for), or `None` for a marker or a kind this
    /// release does not define. Before 2010 every address kind patches 4 bytes; from 2010 the
    /// sizes are DCU32INT's `fxSizeXE32` / `fxSizeXE64` tables (8 for the
    /// 64-bit address kinds `$0E`, `$11` and `$13` on Win64).
    #[must_use]
    pub fn fixup_size(&self, fixup: &Fixup) -> Option<usize> {
        const XE32: [i8; 24] = [
            0, 0, -1, -1, 4, 4, 4, -1, 4, -1, -1, -1, -1, -1, 8, -1, -1, -1, 4, -1, -1, -1, -1, -1,
        ];
        const XE64: [i8; 24] = [
            0, 0, -1, -1, 4, 4, 4, -1, 4, -1, -1, -1, -1, -1, 8, -1, -1, 8, -1, 8, 0, 4, -1, 4,
        ];
        if self.is_marker(fixup) {
            return None;
        }
        if self.version.delphi_at_least(Version::DELPHI_2010) {
            let table = if self.platform == Platform::Win64 {
                &XE64
            } else {
                &XE32
            };
            let size = *table.get(usize::from(fixup.kind))?;
            return usize::try_from(size).ok().filter(|&s| s > 0);
        }
        let (start, _) = self.marker_kinds();
        (fixup.kind < start).then_some(4)
    }

    /// Assigns the block's parts to their declarations (see the module
    /// docs). Parts whose marker names something other than a declaration
    /// (an import's RTTI), and the rest of a part no declaration takes, are
    /// left out.
    #[must_use]
    pub fn memory(&self) -> Vec<DeclMemory> {
        let mut out = Vec::new();
        let markers: Vec<&Fixup> = self.fixups.iter().filter(|f| self.is_marker(f)).collect();
        for (i, marker) in markers.iter().enumerate() {
            let start = usize::try_from(marker.offset).unwrap_or(usize::MAX);
            let end = markers
                .get(i.saturating_add(1))
                .map_or(self.block.len(), |next| {
                    usize::try_from(next.offset).unwrap_or(usize::MAX)
                });
            if start >= end || end > self.block.len() {
                continue;
            }
            let Some(AddrTarget::Decl(mut id)) = self.addr(marker.target) else {
                continue;
            };
            let mut at = start;
            // Bounded: a part holds the routines of one object file at most.
            for _ in 0..1024 {
                let Some(decl) = self.decls.get(id) else {
                    break;
                };
                let take = match &decl.kind {
                    DeclKind::Proc(p) if !self.from_package => usize::try_from(p.size)
                        .unwrap_or(usize::MAX)
                        .min(end.saturating_sub(at)),
                    _ => end.saturating_sub(at),
                };
                out.push(DeclMemory {
                    decl: id,
                    range: at..at.saturating_add(take),
                });
                at = at.saturating_add(take);
                if at >= end || !matches!(decl.kind, DeclKind::Proc(_)) {
                    break;
                }
                // The rest belongs to the routine declared next.
                match self
                    .decls
                    .iter()
                    .enumerate()
                    .skip(id.saturating_add(1))
                    .find(|(_, d)| matches!(d.kind, DeclKind::Proc(_)))
                {
                    Some((next, _)) => id = next,
                    None => break,
                }
            }
        }
        out
    }
}
