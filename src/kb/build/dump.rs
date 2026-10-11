//! Dumps: a declaration's bytes in the unit's block, the bytes its
//! relocations patch, and the relocations by target name.

use std::borrow::Cow;

use crate::{
    dcu::{DeclId, Version},
    kb::{Dump, Fixup, FixupKind},
};

use super::{UnitBuilder, text};

impl UnitBuilder<'_> {
    /// A declaration's bytes, relocation mask and relocations.
    pub fn dump(&self, id: DeclId) -> Dump<'static> {
        let Some(range) = self.memory.get(&id) else {
            return Dump::default();
        };
        let code = self
            .unit
            .block
            .get(range.clone())
            .unwrap_or_default()
            .to_vec();
        let mut relocs = vec![0u8; code.len()];
        let mut fixups = Vec::new();
        for f in &self.unit.fixups {
            let Ok(at) = usize::try_from(f.offset) else {
                continue;
            };
            // The marker that ends the unit's block (after the unit's
            // initialization routine) is kept, as its kind's digit.
            if self.unit.is_marker(f) && at == range.end && at == self.unit.block.len() {
                fixups.push(Fixup {
                    kind: FixupKind::Other(f.kind.saturating_add(b'0')),
                    offset: u32::try_from(at.saturating_sub(range.start)).unwrap_or(u32::MAX),
                    name: text(&self.addr_name(f.target)),
                });
                continue;
            }
            if !range.contains(&at) {
                continue;
            }
            let Some(size) = self.unit.fixup_size(f) else {
                continue;
            };
            let offset = at.saturating_sub(range.start);
            let end = offset.saturating_add(size).min(relocs.len());
            if let Some(mask) = relocs.get_mut(offset..end) {
                mask.fill(0xFF);
            }
            fixups.push(Fixup {
                kind: self.fixup_kind(f.kind),
                offset: u32::try_from(offset).unwrap_or(u32::MAX),
                name: text(&self.addr_name(f.target)),
            });
        }
        Dump {
            code: Cow::Owned(code),
            relocs: Cow::Owned(relocs),
            fixup_count: u32::try_from(fixups.len()).unwrap_or(u32::MAX),
            fixups,
        }
    }

    /// The knowledge-base letter of a DCU fixup kind.
    pub fn fixup_kind(&self, kind: u8) -> FixupKind {
        if self.unit.version.delphi_at_least(Version::DELPHI_2010) {
            FixupKind::Other(kind)
        } else {
            match kind {
                1 => FixupKind::Address,
                2 => FixupKind::Jump,
                3 => FixupKind::Data,
                other => FixupKind::Other(other),
            }
        }
    }
}
