//! Where a program's DCU declarations lie in the linked binary.
//!
//! The linker copies each declaration's part of a unit's block
//! ([`Unit::memory`]) into the image unchanged except at its relocations,
//! which it patches with the target's address in one of a few
//! [`Encoding`]s. So a routine (or typed constant, or RTTI record) is at an
//! address exactly when the image holds its bytes there with the
//! relocations masked, and once it is placed each of its relocations gives
//! the address of the declaration it names: a routine to check the same
//! way, or a variable, which has no bytes to check and is placed when every
//! relocation of the placed parts that names it agrees (so is a part that
//! is nothing but relocations). A generic instance the linker folded is
//! placed for each unit that holds a copy, so one address can carry
//! several placements.
//!
//! [`Locator`] runs that propagation from seeds (addresses known from the
//! binary, such as its classes' methods) through the units of a
//! [`Library`], following imports into the exporting unit by name and
//! stamp. Thread variables come out as offsets in the thread-local block.
//! [`Locator::name_init_table`] names the unit initialization table's slots,
//! which the binary leaves anonymous, by matching each unit's (and class's)
//! initialization and finalization routines, and places what they reach on
//! the way; [`locate`] does all of it for a binary.
//!
//! Two layout facts hold in every fixture program and back the naming of
//! routines that differ only in which variable they touch: the linker
//! keeps each unit's variables together, and in declaration order (dropping
//! the unused ones, so their offsets in the DCU do not predict addresses).
//!
//! The encodings were established against linker maps of programs built by
//! Delphi 6 (kinds `1` - `3`, DCU32INT's `fxAddr`, `fxJmpAddr` and
//! `fxDataAddr`, assumed for Delphi 2 - 2009 as well; Delphi 6's map lists
//! no thread variables, so kind `1` reaching one, as a TLS offset, is
//! unverified) and Delphi 13 (Win32 kinds `4`, `5`, `6`, `8`; Win64 kinds
//! `4`, `5`, `$0E`, `$15`, `$17`). A unit read out of a package (`.dcp`)
//! holds import thunks instead of its code, so none of it is placed.
//! Kinds not seen there and platforms other than Windows give no
//! [`Encoding`], so their relocations are masked but not followed.

use std::{
    collections::{BTreeSet, HashMap, HashSet, VecDeque},
    ops::Range,
    sync::Arc,
};

use crate::{
    dcu::{
        AddrTarget, DeclId, DeclKind, Fixup, Unit, UsesSection, Version, library::Library,
        version::Platform,
    },
    formats::BinaryContext,
    unitinit::{UnitEntry, UnitInitTable},
};

/// Upper bound on the declarations one propagation places; guards a
/// pathological library.
const MAX_PLACEMENTS: usize = 1 << 22;

/// The unmasked bytes a routine of a candidate's own unit must match for
/// it to tell that unit apart: an import thunk (`jmp [cell]`) matches
/// with two, and identical small stubs abound.
const MIN_DISTINCT_BYTES: usize = 8;

/// How a relocation's patched bytes encode its target's address in a linked
/// image. The bytes hold the encoded value plus the addend the DCU stores
/// at the same place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    /// The target's address.
    Absolute,
    /// The target's address minus the address just past the patched bytes.
    Relative,
    /// The address of a pointer cell that holds the target's address (a
    /// variable or typed constant of another unit, which a package could
    /// export).
    Indirect,
    /// [`Encoding::Indirect`], relative to the address just past the patched
    /// bytes.
    IndirectRelative,
    /// The target's address minus the image base (an RVA).
    ImageRelative,
    /// The target's offset in the thread-local storage block.
    ThreadOffset,
}

/// What a relocation's target is, as far as its [`Encoding`] depends on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetKind {
    /// A routine, type, VMT or anything else with a fixed address.
    Code,
    /// A variable, typed constant or other data a package could export.
    Data,
    /// A thread variable.
    Thread,
}

impl TargetKind {
    /// Classifies a target by its declaration kind.
    #[must_use]
    pub fn of(kind: &DeclKind) -> Self {
        match kind {
            DeclKind::ThreadVar(_) => Self::Thread,
            DeclKind::Var(_)
            | DeclKind::SpecialVar(_)
            | DeclKind::TypedConst(_)
            | DeclKind::ResourceString(_) => Self::Data,
            _ => Self::Code,
        }
    }
}

impl Unit {
    /// Returns how `fixup`'s patched bytes encode a target of kind
    /// `target` (see the module docs), or `None` for a marker, a kind not
    /// established, a platform other than Win32 and Win64, or a kind that
    /// reaches a thread variable other than by its TLS offset (or anything
    /// else by one).
    #[must_use]
    pub fn fixup_encoding(&self, fixup: &Fixup, target: TargetKind) -> Option<Encoding> {
        // A thread variable is reached by its TLS offset and nothing else
        // is; any other pairing is a kind this module misreads.
        self.raw_fixup_encoding(fixup, target)
            .filter(|e| (*e == Encoding::ThreadOffset) == (target == TargetKind::Thread))
    }

    /// [`Unit::fixup_encoding`] before the thread-variable check.
    fn raw_fixup_encoding(&self, fixup: &Fixup, target: TargetKind) -> Option<Encoding> {
        if self.msil || self.is_marker(fixup) {
            return None;
        }
        let thread_or = |plain| {
            if target == TargetKind::Thread {
                Encoding::ThreadOffset
            } else {
                plain
            }
        };
        if !self.version.delphi_at_least(Version::DELPHI_2010) {
            if self.platform != Platform::Win32 {
                return None;
            }
            return match fixup.kind {
                1 => Some(thread_or(Encoding::Absolute)),
                2 => Some(Encoding::Relative),
                3 => Some(Encoding::Indirect),
                _ => None,
            };
        }
        match (self.platform, fixup.kind) {
            (Platform::Win32, 4) => Some(thread_or(Encoding::Absolute)),
            (Platform::Win32, 5) | (Platform::Win64, 5) => Some(Encoding::Relative),
            (Platform::Win32, 6) => Some(Encoding::Indirect),
            (Platform::Win32, 8) | (Platform::Win64, 0x0E) => Some(Encoding::Absolute),
            (Platform::Win64, 4) => Some(Encoding::ThreadOffset),
            (Platform::Win64, 0x15) => Some(Encoding::ImageRelative),
            (Platform::Win64, 0x17) => Some(if target == TargetKind::Data {
                Encoding::IndirectRelative
            } else {
                Encoding::Relative
            }),
            _ => None,
        }
    }
}

/// Where a located declaration lies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Address {
    /// A virtual address in the image.
    Va(u64),
    /// An offset in the thread-local storage block (a thread variable).
    Thread(u64),
}

/// One located declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placement {
    /// The declaring unit's name.
    pub unit: String,
    /// The declaration (an index into that unit's [`Unit::decls`]).
    pub decl: DeclId,
    /// The declaration's name.
    pub name: String,
    /// Where it lies.
    pub address: Address,
    /// Whether the image holds the declaration's bytes there (a routine,
    /// typed constant or RTTI record), rather than relocations alone
    /// placing it (a variable).
    pub verified: bool,
}

/// A variable the relocations disagree on (see [`Locator::conflicts`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    /// The declaring unit's name.
    pub unit: String,
    /// The declaration (an index into that unit's [`Unit::decls`]).
    pub decl: DeclId,
    /// The declaration's name.
    pub name: String,
    /// Every address a relocation gave it.
    pub addresses: Vec<Address>,
}

/// A declaration of a loaded unit: an index into the locator's units and
/// a declaration of that unit.
type Key = (usize, DeclId);

/// An owner whose routines are at an initialization table slot's
/// addresses: the owner (an index into the owners) and the routine
/// placements that implies.
type Candidate = (usize, Vec<(Key, u64)>);

/// A relocation's target as decoded from a placed part.
#[derive(Debug, Clone, Copy)]
struct Target {
    key: Key,
    kind: TargetKind,
    has_part: bool,
    address: Address,
}

/// A loaded unit with what the propagation needs of it.
#[derive(Debug)]
struct Loaded {
    unit: Arc<Unit>,
    /// Each declaration's part of the block.
    parts: HashMap<DeclId, Range<usize>>,
    /// The relocations (not markers) sorted by offset.
    fixups: Vec<Fixup>,
}

/// Places a program's DCU declarations in its binary by following
/// relocations (see the module docs).
#[derive(Debug)]
pub struct Locator<'a> {
    library: &'a Library,
    ctx: &'a BinaryContext<'a>,
    units: Vec<Loaded>,
    by_name: HashMap<String, Option<usize>>,
    imports: HashMap<(usize, u32), Option<Key>>,
    placed: HashMap<Key, u64>,
    placed_at: HashMap<u64, Vec<Key>>,
    rejected: BTreeSet<(Key, u64)>,
    votes: HashMap<Key, BTreeSet<Address>>,
    queue: VecDeque<(Key, u64)>,
    /// Byte-less parts placed by a relocation, whose relocations are still
    /// to follow.
    unchecked: VecDeque<(Key, u64)>,
    /// Size of the image's thread-local storage template, when it has one.
    tls_size: Option<u64>,
}

impl<'a> Locator<'a> {
    /// Creates a locator for the binary `ctx` describes, reading units from
    /// `library` (which must be the release and platform that built it).
    #[must_use]
    pub fn new(library: &'a Library, ctx: &'a BinaryContext<'a>) -> Self {
        Self {
            library,
            ctx,
            units: Vec::new(),
            by_name: HashMap::new(),
            imports: HashMap::new(),
            placed: HashMap::new(),
            placed_at: HashMap::new(),
            rejected: BTreeSet::new(),
            votes: HashMap::new(),
            queue: VecDeque::new(),
            unchecked: VecDeque::new(),
            tls_size: ctx
                .pe()
                .and_then(|pe| pe.tls_data.as_ref())
                .and_then(|tls| {
                    let dir = &tls.image_tls_directory;
                    dir.end_address_of_raw_data
                        .checked_sub(dir.start_address_of_raw_data)?
                        .checked_add(u64::from(dir.size_of_zero_fill))
                }),
        }
    }

    /// Returns unit `name` from the library, loading it on first use.
    #[must_use]
    pub fn unit(&mut self, name: &str) -> Option<Arc<Unit>> {
        let index = self.load(name)?;
        self.units.get(index).map(|l| Arc::clone(&l.unit))
    }

    /// Loads unit `name`, returning its index.
    fn load(&mut self, name: &str) -> Option<usize> {
        let key = name.to_ascii_lowercase();
        if let Some(hit) = self.by_name.get(&key) {
            return *hit;
        }
        let index = self.library.unit(name).map(|unit| {
            // A unit read out of a package (`.dcp`) holds import thunks into
            // the package instead of its code: nothing of it can be placed.
            let parts = if unit.from_package {
                HashMap::new()
            } else {
                unit.memory()
                    .into_iter()
                    .map(|m| (m.decl, m.range))
                    .collect()
            };
            let mut fixups: Vec<Fixup> = unit
                .fixups
                .iter()
                .filter(|f| !unit.is_marker(f))
                .copied()
                .collect();
            fixups.sort_by_key(|f| f.offset);
            self.units.push(Loaded {
                unit,
                parts,
                fixups,
            });
            self.units.len().saturating_sub(1)
        });
        self.by_name.insert(key, index);
        index
    }

    /// Queues declaration `decl` of unit `unit` as lying at `va`; the
    /// placement takes effect in [`Locator::run`] if the image holds the
    /// declaration's bytes there. Returns `false` when the unit is not in
    /// the library or the declaration has no part of the block.
    pub fn seed(&mut self, unit: &str, decl: DeclId, va: u64) -> bool {
        let Some(u) = self.load(unit) else {
            return false;
        };
        if !self
            .units
            .get(u)
            .is_some_and(|l| l.parts.contains_key(&decl))
        {
            return false;
        }
        self.queue.push_back(((u, decl), va));
        true
    }

    /// Returns whether the image holds declaration `decl` of unit `unit`
    /// at `va`: its part of the block, relocations masked. A declaration
    /// without a part, or whose part is all relocations, is never there.
    pub fn holds(&mut self, unit: &str, decl: DeclId, va: u64) -> bool {
        self.load(unit)
            .is_some_and(|u| self.verify((u, decl), va).is_some())
    }

    /// Checks the image against a declaration's part at `va` and returns
    /// how many unmasked bytes matched, when they all do. A part with no
    /// unmasked bytes (all relocations) matches anywhere, so it is never
    /// placed by its bytes (see [`Self::verify`]), though it does not
    /// contradict an address either.
    fn compare(&self, key: Key, va: u64) -> Option<usize> {
        let loaded = self.units.get(key.0)?;
        let range = loaded.parts.get(&key.1)?;
        let code = loaded.unit.block.get(range.clone())?;
        let start = self.ctx.va_to_file(va)?;
        let len = code.len();
        if len == 0 {
            return Some(0);
        }
        let last = va.checked_add(u64::try_from(len.checked_sub(1)?).ok()?)?;
        if self.ctx.va_to_file(last)? != start.checked_add(len.checked_sub(1)?)? {
            return None;
        }
        let image = self.ctx.data().get(start..start.checked_add(len)?)?;
        let mut matched = 0usize;
        let mut at = 0usize;
        for f in Self::fixups_in(loaded, range) {
            let rel = usize::try_from(f.offset).ok()?.checked_sub(range.start)?;
            let Some(size) = loaded.unit.fixup_size(f) else {
                continue;
            };
            if rel < at {
                // Overlapping relocations: the earlier one already masked
                // part of this one.
                at = at.max(rel.checked_add(size)?);
                continue;
            }
            if code.get(at..rel)? != image.get(at..rel)? {
                return None;
            }
            matched = matched.checked_add(rel.checked_sub(at)?)?;
            at = rel.checked_add(size)?;
        }
        let tail = at.min(len);
        if code.get(tail..)? != image.get(tail..)? {
            return None;
        }
        matched.checked_add(len.checked_sub(tail)?)
    }

    /// [`Self::compare`], requiring at least one unmasked byte: whether the
    /// image holds the part at `va`, and how many bytes say so.
    fn verify(&self, key: Key, va: u64) -> Option<usize> {
        self.compare(key, va).filter(|&matched| matched > 0)
    }

    /// The relocations inside `range` of a loaded unit's block.
    fn fixups_in<'l>(loaded: &'l Loaded, range: &Range<usize>) -> &'l [Fixup] {
        let lo = loaded
            .fixups
            .partition_point(|f| usize::try_from(f.offset).unwrap_or(usize::MAX) < range.start);
        let hi = loaded
            .fixups
            .partition_point(|f| usize::try_from(f.offset).unwrap_or(usize::MAX) < range.end);
        loaded.fixups.get(lo..hi).unwrap_or_default()
    }

    /// Resolves address-table index `index` of loaded unit `u` to a
    /// declaration, following an import into the exporting unit.
    fn resolve(&mut self, u: usize, index: u32) -> Option<Key> {
        if let Some(hit) = self.imports.get(&(u, index)) {
            return *hit;
        }
        let unit = Arc::clone(&self.units.get(u)?.unit);
        let found = match unit.addr(index)? {
            AddrTarget::Decl(id) => Some((u, id)),
            AddrTarget::Import { unit: ui, symbol } => {
                let import = unit.uses.get(ui)?;
                let sym = import.symbols.get(symbol)?;
                if import.section == UsesSection::Dll {
                    None
                } else {
                    let name = import.name.clone();
                    self.load(&name).and_then(|t| {
                        let target = &self.units.get(t)?.unit;
                        Self::exported(target, &sym.name, sym.stamp).map(|id| (t, id))
                    })
                }
            }
            _ => None,
        };
        self.imports.insert((u, index), found);
        found
    }

    /// The interface declaration of `unit` named `name` with stamp
    /// `stamp`, or the only one named `name` when no stamp matches.
    fn exported(unit: &Unit, name: &str, stamp: u32) -> Option<DeclId> {
        let named = || {
            unit.main.iter().copied().filter(move |&id| {
                unit.decls
                    .get(id)
                    .is_some_and(|d| d.name.eq_ignore_ascii_case(name))
            })
        };
        let mut stamped = named().filter(|&id| {
            unit.decls
                .get(id)
                .and_then(|d| d.flags)
                .is_some_and(|f| f.stamp == stamp)
        });
        if let Some(id) = stamped.next() {
            return stamped.next().is_none().then_some(id);
        }
        let mut all = named();
        let id = all.next()?;
        all.next().is_none().then_some(id)
    }

    /// Reads the little-endian signed value of `size` bytes at `bytes`.
    fn read_signed(bytes: &[u8], size: usize) -> Option<i64> {
        match size {
            4 => Some(i64::from(i32::from_le_bytes(
                bytes.get(..4)?.try_into().ok()?,
            ))),
            8 => Some(i64::from_le_bytes(bytes.get(..8)?.try_into().ok()?)),
            _ => None,
        }
    }

    /// Reads the pointer cell at `va`.
    fn read_cell(&self, va: u64) -> Option<u64> {
        let size = self.ctx.pointer_size()?;
        let at = self.ctx.va_to_file(va)?;
        let bytes = self.ctx.data().get(at..at.checked_add(size)?)?;
        match size {
            4 => Some(u64::from(u32::from_le_bytes(bytes.try_into().ok()?))),
            8 => Some(u64::from_le_bytes(bytes.try_into().ok()?)),
            _ => None,
        }
    }

    /// Decodes the address relocation `fixup` of a part placed at
    /// `part_va` (the part starting at block offset `part_start`) gives its
    /// target, `None` when it gives none.
    fn target_address(
        &self,
        loaded: &Loaded,
        fixup: &Fixup,
        part_va: u64,
        part_start: usize,
        kind: TargetKind,
    ) -> Option<Address> {
        let unit = &loaded.unit;
        let size = unit.fixup_size(fixup)?;
        let encoding = unit.fixup_encoding(fixup, kind)?;
        let offset = usize::try_from(fixup.offset).ok()?;
        let addend = Self::read_signed(unit.block.get(offset..)?, size)?;
        let site = part_va.checked_add(u64::try_from(offset.checked_sub(part_start)?).ok()?)?;
        let file = self.ctx.va_to_file(site)?;
        let patched = Self::read_signed(self.ctx.data().get(file..)?, size)?;
        let end = i64::try_from(site.checked_add(u64::try_from(size).ok()?)?).ok()?;
        let value = patched.checked_sub(addend)?;
        let address = |v: i64| -> Option<u64> {
            if self.ctx.pointer_size() == Some(4) {
                u32::try_from(v & 0xFFFF_FFFF).ok().map(u64::from)
            } else {
                u64::try_from(v).ok()
            }
        };
        match encoding {
            Encoding::Absolute => address(value).map(Address::Va),
            Encoding::Relative => address(value.checked_add(end)?).map(Address::Va),
            Encoding::ImageRelative => {
                let base = i64::try_from(self.ctx.image_base()?).ok()?;
                address(value.checked_add(base)?).map(Address::Va)
            }
            Encoding::Indirect => self.read_cell(address(value)?).map(Address::Va),
            Encoding::IndirectRelative => self
                .read_cell(address(value.checked_add(end)?)?)
                .map(Address::Va),
            Encoding::ThreadOffset => address(value).map(Address::Thread),
        }
    }

    /// Runs the propagation from the queued seeds until nothing new is
    /// placed.
    pub fn run(&mut self) {
        loop {
            let Some((key, va)) = self.queue.pop_front() else {
                match self.unchecked.pop_front() {
                    Some((key, va)) => {
                        self.follow(key, va);
                        continue;
                    }
                    None => break,
                }
            };
            if self.placed.len() >= MAX_PLACEMENTS {
                break;
            }
            if self.placed.contains_key(&key) || self.rejected.contains(&(key, va)) {
                continue;
            }
            if self.verify(key, va).is_none() {
                self.rejected.insert((key, va));
                continue;
            }
            self.placed.insert(key, va);
            self.placed_at.entry(va).or_default().push(key);
            self.follow(key, va);
        }
    }

    /// Follows the relocations of a declaration placed at `va`.
    fn follow(&mut self, key: Key, va: u64) {
        for t in self.targets(key, va) {
            match t.address {
                // A part with bytes to check is verified when dequeued; one
                // without (a typed constant that is a single pointer, an
                // empty routine) is placed like a variable, by agreement.
                Address::Va(at) if t.has_part && self.compare(t.key, at) != Some(0) => {
                    self.queue.push_back((t.key, at));
                }
                _ if t.has_part || t.kind != TargetKind::Code => {
                    let votes = self.votes.entry(t.key).or_default();
                    let first = votes.is_empty();
                    votes.insert(t.address);
                    // A byte-less part (a table of routine pointers, say)
                    // still leads on: what its relocations reach is
                    // verified by its own bytes.
                    if let (true, true, Address::Va(at)) = (first, t.has_part, t.address) {
                        self.unchecked.push_back((t.key, at));
                    }
                }
                _ => {}
            }
        }
    }

    /// Decodes every relocation of a declaration's part as if it lay at
    /// `va`, returning the targets it gives addresses for.
    fn targets(&mut self, key: Key, va: u64) -> Vec<Target> {
        let mut out = Vec::new();
        let Some(loaded) = self.units.get(key.0) else {
            return out;
        };
        let Some(range) = loaded.parts.get(&key.1).cloned() else {
            return out;
        };
        let fixups: Vec<Fixup> = Self::fixups_in(loaded, &range).to_vec();
        for f in fixups {
            let Some(target) = self.resolve(key.0, f.target) else {
                continue;
            };
            let Some(tunit) = self.units.get(target.0) else {
                continue;
            };
            let Some(tdecl) = tunit.unit.decls.get(target.1) else {
                continue;
            };
            // `SysInit.TlsLast` is an ordinary variable the linker moves to
            // the end of the thread-local block (SysInit.pas).
            let kind = if tdecl.name == "TlsLast" && tunit.unit.name == "SysInit" {
                TargetKind::Thread
            } else {
                TargetKind::of(&tdecl.kind)
            };
            let has_part = tunit.parts.contains_key(&target.1);
            let Some(loaded) = self.units.get(key.0) else {
                break;
            };
            let address = self.target_address(loaded, &f, va, range.start, kind);
            // An address outside the image is a misread (an import cell
            // holding another module's address, say), never a placement;
            // so is an offset past the thread-local template, whose end is
            // where `TlsLast` lies.
            if let Some(address) = address.filter(|a| match a {
                Address::Va(at) => self.ctx.is_image_va(*at),
                Address::Thread(offset) => self.tls_size.is_none_or(|size| *offset <= size),
            }) {
                out.push(Target {
                    key: target,
                    kind,
                    has_part,
                    address,
                });
            }
        }
        out
    }

    /// Returns where declaration `decl` of unit `unit` lies, when the
    /// propagation placed it: verified by its bytes, or a variable every
    /// relocation agrees on.
    #[must_use]
    pub fn address_of(&self, unit: &str, decl: DeclId) -> Option<Address> {
        let u = (*self.by_name.get(&unit.to_ascii_lowercase())?)?;
        self.address((u, decl))
    }

    /// [`Locator::address_of`] by key.
    fn address(&self, key: Key) -> Option<Address> {
        if let Some(&va) = self.placed.get(&key) {
            return Some(Address::Va(va));
        }
        let votes = self.votes.get(&key)?;
        let mut it = votes.iter();
        let first = *it.next()?;
        it.next().is_none().then_some(first)
    }

    /// Returns every located declaration, ordered by unit and declaration.
    #[must_use]
    pub fn placements(&self) -> Vec<Placement> {
        let mut keys: BTreeSet<Key> = self.placed.keys().copied().collect();
        keys.extend(self.votes.keys().copied());
        keys.into_iter()
            .filter_map(|key| {
                let address = self.address(key)?;
                let unit = &self.units.get(key.0)?.unit;
                Some(Placement {
                    unit: unit.name.clone(),
                    decl: key.1,
                    name: unit.decls.get(key.1)?.name.clone(),
                    address,
                    verified: self.placed.contains_key(&key),
                })
            })
            .collect()
    }

    /// Returns the variables whose relocations disagree on their address,
    /// which the propagation therefore leaves unplaced: a sign of an
    /// encoding this module gets wrong (none on the fixture programs).
    #[must_use]
    pub fn conflicts(&self) -> Vec<Conflict> {
        self.votes
            .iter()
            .filter(|(key, v)| v.len() > 1 && !self.placed.contains_key(key))
            .filter_map(|(key, v)| {
                let unit = &self.units.get(key.0)?.unit;
                Some(Conflict {
                    unit: unit.name.clone(),
                    decl: key.1,
                    name: unit.decls.get(key.1)?.name.clone(),
                    addresses: v.iter().copied().collect(),
                })
            })
            .collect()
    }

    /// Names the slots of a unit initialization table (`entries`, in table
    /// order) after the units of `units` - the candidates, typically every
    /// unit the program could link - and queues and places each named
    /// slot's routines on the way. A program's table ends with the
    /// program's own slot, which no DCU explains: pass the table without it.
    ///
    /// A slot belongs to a unit (its `initialization` routine, named after
    /// the unit, and its `Finalization`) or, from Delphi 2010, to a class
    /// (the `<Class>.<name>@` wrappers of its class constructor and class
    /// destructor). A candidate explains a slot when its routines are at
    /// the slot's addresses and agree with everything placed so far: every
    /// relocation's target is where the propagation put it, verifies at
    /// the address the relocation gives, or - for a variable - fits the
    /// variables already placed, given that the linker keeps each unit's
    /// variables together in declaration order. It must also agree with
    /// the initialization order: a unit's slot follows the slot of every
    /// unit it uses that does not use it back (directly or not), and a
    /// class's slot precedes its unit's.
    ///
    /// A slot is named when exactly one candidate explains it and either
    /// `complete` is set - `units` includes every unit the table holds, as
    /// a self-contained program's PACKAGEINFO lists them - or the candidate
    /// is told apart from any unit outside `units`: by a relocation into
    /// its own unit that is where the propagation put it or verifies, or by
    /// a variable of its own that lies among the unit's placed variables.
    /// That last rule tells apart the counter routines (`sub [counter], 1`
    /// / `inc [counter]`), identical in every unit but for the counter. A
    /// unit the propagation found linked also takes the one slot left that
    /// it can explain, if just one is: every linked unit has a slot (one
    /// without routines when the linker drops trivial ones), while a class
    /// can have none. Naming a slot places more, so the rounds repeat until
    /// none names another slot.
    ///
    /// Returns one entry per slot, `None` where no rule names it.
    pub fn name_init_table(
        &mut self,
        entries: &[UnitEntry],
        units: &[&str],
        complete: bool,
    ) -> Vec<Option<InitOwner>> {
        self.run();
        let owners = self.owners(units);
        let before = self.order(&owners);
        let precedes = |a: usize, b: usize| {
            before
                .get(a)
                .and_then(|row| row.get(b))
                .copied()
                .unwrap_or(false)
        };
        let mut named: Vec<Option<usize>> = vec![None; entries.len()];
        let mut taken = vec![false; owners.len()];
        // Whether owner `o` at `slot` keeps the order with the named slots.
        let in_order = |named: &[Option<usize>], o: usize, slot: usize| {
            named.iter().enumerate().all(|(j, p)| match p {
                Some(p) => !(precedes(o, *p) && slot > j || precedes(*p, o) && slot < j),
                None => true,
            })
        };
        // The byte matches never change; the agreement checks are redone
        // each round against what the propagation placed since.
        let matches: Vec<Vec<Candidate>> = entries
            .iter()
            .map(|entry| {
                owners
                    .iter()
                    .enumerate()
                    .filter_map(|(i, owner)| Some((i, self.assignments(owner, entry)?)))
                    .collect()
            })
            .collect();
        // Bounded: every round but the last names a slot.
        for _ in 0..=entries.len() {
            let layout = self.layout();
            let mut found = None;
            // A slot exactly one candidate explains.
            for (slot, candidates) in matches.iter().enumerate() {
                if named.get(slot).is_some_and(Option::is_some) {
                    continue;
                }
                let mut agreeing = Vec::new();
                for (owner, routines) in candidates {
                    if taken.get(*owner).copied().unwrap_or(true) || !in_order(&named, *owner, slot)
                    {
                        continue;
                    }
                    if let Some(strong) = self.agrees_all(routines, &layout) {
                        agreeing.push((*owner, strong));
                    }
                }
                if let [(owner, strong)] = agreeing[..]
                    && (strong || complete)
                {
                    found = Some((slot, owner));
                    break;
                }
            }
            // A linked unit with only one slot left that it can explain.
            if found.is_none() {
                let linked = self.linked();
                for (owner, o) in owners.iter().enumerate() {
                    // Every linked unit has a slot; not every class does. A
                    // routine with a relocation of unknown width could fail
                    // to match its own slot, so such a unit is left out.
                    if taken.get(owner).copied().unwrap_or(true)
                        || !matches!(o.owner, InitOwner::Unit(_))
                        || !o.exact
                        || !linked.contains(&o.unit)
                    {
                        continue;
                    }
                    let mut possible = Vec::new();
                    for (slot, entry) in entries.iter().enumerate() {
                        if named.get(slot).is_some_and(Option::is_some) {
                            continue;
                        }
                        let explains = if entry.init_va.is_none() && entry.finalize_va.is_none() {
                            true
                        } else {
                            match matches
                                .get(slot)
                                .and_then(|c| c.iter().find(|(i, _)| *i == owner))
                            {
                                Some((_, routines)) => self.agrees_all(routines, &layout).is_some(),
                                None => false,
                            }
                        };
                        if explains && in_order(&named, owner, slot) {
                            possible.push(slot);
                            if possible.len() > 1 {
                                break;
                            }
                        }
                    }
                    if let [slot] = possible[..] {
                        found = Some((slot, owner));
                        break;
                    }
                }
            }
            let Some((slot, owner)) = found else {
                break;
            };
            if let Some(t) = taken.get_mut(owner) {
                *t = true;
            }
            if let Some(n) = named.get_mut(slot) {
                *n = Some(owner);
            }
            if let Some((_, routines)) = matches
                .get(slot)
                .and_then(|c| c.iter().find(|(i, _)| *i == owner))
            {
                self.queue.extend(routines.iter().copied());
                self.run();
            }
        }
        named
            .into_iter()
            .map(|n| n.and_then(|i| owners.get(i)).map(|o| o.owner.clone()))
            .collect()
    }

    /// The order the initialization imposes on `owners`: entry `[a][b]`
    /// says owner `a`'s slot precedes owner `b`'s.
    ///
    /// The table lists units depth first, each after the units it uses
    /// (interface, then implementation), and a unit's class slots just
    /// before its own. When unit `A` uses `B` and `B` does not reach `A`
    /// through uses, `B` is finished when `A` comes to it or is finished
    /// then, so all of `B`'s slots precede `A`'s; a use cycle leaves the
    /// order to the visit, which is not known. A unit whose uses reach a
    /// unit the library lacks may reach anything, so it orders nothing.
    fn order(&mut self, owners: &[Owner]) -> Vec<Vec<bool>> {
        let mut reach: HashMap<usize, Option<BTreeSet<usize>>> = HashMap::new();
        let mut units: Vec<usize> = owners.iter().map(|o| o.unit).collect();
        units.sort_unstable();
        units.dedup();
        for &u in &units {
            self.reach(u, &mut reach);
        }
        // The units forced before each owner unit, through chains of uses
        // edges that are not part of a cycle.
        let mut forced: HashMap<usize, BTreeSet<usize>> = HashMap::new();
        for &u in &units {
            self.forced_before(u, &mut reach, &mut forced, 0);
        }
        owners
            .iter()
            .map(|a| {
                owners
                    .iter()
                    .map(|b| {
                        if a.unit == b.unit {
                            matches!(a.owner, InitOwner::Class { .. })
                                && matches!(b.owner, InitOwner::Unit(_))
                        } else {
                            forced.get(&b.unit).is_some_and(|f| f.contains(&a.unit))
                        }
                    })
                    .collect()
            })
            .collect()
    }

    /// The loaded units `u`'s uses reach (itself not included unless
    /// through a cycle), or `None` when they reach a unit the library
    /// lacks.
    fn reach(
        &mut self,
        u: usize,
        memo: &mut HashMap<usize, Option<BTreeSet<usize>>>,
    ) -> Option<BTreeSet<usize>> {
        if let Some(hit) = memo.get(&u) {
            return hit.clone();
        }
        let mut seen = BTreeSet::new();
        let mut todo = vec![u];
        let mut complete = true;
        while let Some(at) = todo.pop() {
            for name in self.uses_of(at) {
                match self.load(&name) {
                    Some(next) => {
                        if seen.insert(next) {
                            todo.push(next);
                        }
                    }
                    None => complete = false,
                }
            }
        }
        let out = complete.then_some(seen);
        memo.insert(u, out.clone());
        out
    }

    /// The names of the units loaded unit `u` uses (not DLLs).
    fn uses_of(&self, u: usize) -> Vec<String> {
        self.units
            .get(u)
            .map(|l| {
                l.unit
                    .uses
                    .iter()
                    .filter(|i| i.section != UsesSection::Dll)
                    .map(|i| i.name.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Collects into `forced[u]` the units whose slots must precede unit
    /// `u`'s (see [`Locator::order`]).
    fn forced_before(
        &mut self,
        u: usize,
        reach: &mut HashMap<usize, Option<BTreeSet<usize>>>,
        forced: &mut HashMap<usize, BTreeSet<usize>>,
        depth: usize,
    ) -> BTreeSet<usize> {
        if let Some(hit) = forced.get(&u) {
            return hit.clone();
        }
        let mut out = BTreeSet::new();
        // Bounded: the edges followed form a DAG, so the depth is at most
        // the number of units; this guards a malformed library.
        if depth < 4096 {
            for name in self.uses_of(u) {
                let Some(b) = self.load(&name) else {
                    continue;
                };
                let acyclic = self.reach(b, reach).is_some_and(|r| !r.contains(&u));
                if acyclic && b != u {
                    out.insert(b);
                    out.extend(self.forced_before(b, reach, forced, depth.saturating_add(1)));
                }
            }
        }
        forced.insert(u, out.clone());
        out
    }

    /// Collects the initialization table owners `units` declare.
    fn owners(&mut self, units: &[&str]) -> Vec<Owner> {
        let mut out = Vec::new();
        for name in units {
            let Some(u) = self.load(name) else {
                continue;
            };
            let Some(loaded) = self.units.get(u) else {
                continue;
            };
            let unit = &loaded.unit;
            let routine = |id: &DeclId| {
                loaded.parts.contains_key(id)
                    && unit
                        .decls
                        .get(*id)
                        .is_some_and(|d| matches!(d.kind, DeclKind::Proc(_)))
            };
            let named = |n: &str| {
                unit.main
                    .iter()
                    .copied()
                    .filter(routine)
                    .find(|&id| unit.decls.get(id).is_some_and(|d| d.name == n))
            };
            let exact = |id: DeclId| {
                loaded.parts.get(&id).is_some_and(|range| {
                    Self::fixups_in(loaded, range)
                        .iter()
                        .all(|f| unit.fixup_size(f).is_some())
                })
            };
            let init = named(&unit.name);
            let fini = named("Finalization");
            if init.is_some() || fini.is_some() {
                out.push(Owner {
                    owner: InitOwner::Unit(unit.name.clone()),
                    routines: [init, fini],
                    ordered: true,
                    exact: [init, fini].iter().flatten().all(|&id| exact(id)),
                    unit: u,
                });
            }
            // Class constructor and destructor wrappers, by class.
            let mut classes: Vec<(String, Vec<DeclId>)> = Vec::new();
            for &id in unit.main.iter().filter(|id| routine(id)) {
                let Some(d) = unit.decls.get(id) else {
                    continue;
                };
                let Some((class, _)) = d.name.strip_suffix('@').and_then(|n| n.rsplit_once('.'))
                else {
                    continue;
                };
                match classes.iter_mut().find(|(c, _)| c == class) {
                    Some((_, ids)) => ids.push(id),
                    None => classes.push((class.to_string(), vec![id])),
                }
            }
            for (class, ids) in classes {
                if let [first, rest @ ..] = ids.as_slice()
                    && rest.len() <= 1
                {
                    out.push(Owner {
                        owner: InitOwner::Class {
                            unit: unit.name.clone(),
                            class,
                        },
                        routines: [Some(*first), rest.first().copied()],
                        ordered: false,
                        exact: ids.iter().all(|&id| exact(id)),
                        unit: u,
                    });
                }
            }
        }
        out
    }

    /// The ways `owner`'s routines are at `entry`'s addresses (bytes only),
    /// as the routine placements they imply; `None` when there is none, or
    /// more than one.
    fn assignments(&self, owner: &Owner, entry: &UnitEntry) -> Option<Vec<(Key, u64)>> {
        let [a, b] = owner.routines;
        let orders: &[[Option<DeclId>; 2]] = if owner.ordered {
            &[[a, b]]
        } else {
            &[[a, b], [b, a]]
        };
        let mut found = None;
        for [init, fini] in orders {
            let side = |decl: Option<DeclId>, va: Option<u64>| match (decl, va) {
                (Some(d), Some(va)) => self
                    .verify((owner.unit, d), va)
                    .map(|_| Some(((owner.unit, d), va))),
                (_, None) => Some(None),
                (None, Some(_)) => None,
            };
            let (Some(i), Some(f)) = (side(*init, entry.init_va), side(*fini, entry.finalize_va))
            else {
                continue;
            };
            let routines: Vec<(Key, u64)> = [i, f].into_iter().flatten().collect();
            if routines.is_empty() {
                continue;
            }
            match &found {
                Some(other) if *other != routines => return None,
                _ => found = Some(routines),
            }
        }
        found
    }

    /// [`Locator::agrees`] over all of a candidate's routines: `None` when
    /// one disagrees, else whether any gives discriminating evidence.
    fn agrees_all(&mut self, routines: &[(Key, u64)], layout: &Layout) -> Option<bool> {
        let mut strong = false;
        for &(key, va) in routines {
            strong |= self.agrees(key, va, layout)?;
        }
        Some(strong)
    }

    /// Checks a part placed at `va` against everything placed so far (see
    /// [`Locator::name_init_table`]): `None` when a relocation contradicts
    /// it, else whether one tells its unit apart from any other - a target
    /// of the same unit (not the part itself) that is where the propagation
    /// put it or verifies at the address given, or a variable of the same
    /// unit that lies inside the unit's run of placed variables.
    fn agrees(&mut self, key: Key, va: u64, layout: &Layout) -> Option<bool> {
        if self.placed.get(&key).is_some_and(|&at| at != va) {
            return None;
        }
        // Another unit's routine verified at the same address excludes this
        // one; a generic instance can be folded with its copies in other
        // units, so it excludes nothing.
        if let Some(there) = self.placed_at.get(&va)
            && there
                .iter()
                .any(|&other| other.0 != key.0 && !self.is_generic(other))
            && !self.is_generic(key)
        {
            return None;
        }
        let mut strong = false;
        for t in self.targets(key, va) {
            // Evidence for the unit: a declaration of its own, not a
            // generic instance (which other units hold copies of).
            let own = t.key.0 == key.0 && t.key != key && !self.is_generic(t.key);
            if let Some(known) = self.address(t.key) {
                if known != t.address {
                    return None;
                }
                strong |= own;
                continue;
            }
            match t.address {
                Address::Va(at) if t.has_part => {
                    if t.key != key {
                        let matched = self.compare(t.key, at)?;
                        strong |= own && matched >= MIN_DISTINCT_BYTES;
                    }
                }
                Address::Va(at) if t.kind == TargetKind::Data => {
                    strong |= layout.fits(t.key, at)? && own;
                }
                _ => {}
            }
        }
        Some(strong)
    }

    /// Returns whether a loaded declaration is a generic instance (named
    /// `{Unit}Type<...>...`), which the linker folds with the copies other
    /// units hold.
    fn is_generic(&self, key: Key) -> bool {
        self.units
            .get(key.0)
            .and_then(|l| l.unit.decls.get(key.1))
            .is_some_and(|d| d.name.starts_with('{'))
    }

    /// The loaded units with a declaration placed: those linked into the
    /// image.
    fn linked(&self) -> HashSet<usize> {
        self.placed
            .keys()
            .chain(self.votes.keys())
            .map(|k| k.0)
            .collect()
    }

    /// Snapshots the variables placed so far by image section, in address
    /// order.
    fn layout(&self) -> Layout {
        let ranges = self.section_ranges();
        let mut sections: HashMap<usize, Vec<(u64, Key)>> = HashMap::new();
        for (&key, votes) in &self.votes {
            if self.placed.contains_key(&key) {
                continue;
            }
            let [Address::Va(va)] = votes.iter().copied().collect::<Vec<_>>()[..] else {
                continue;
            };
            // A generic instance's class variables are folded across
            // units, so they say nothing about one unit's layout.
            let is_var = !self.is_generic(key)
                && self
                    .units
                    .get(key.0)
                    .and_then(|l| l.unit.decls.get(key.1))
                    .is_some_and(|d| matches!(d.kind, DeclKind::Var(_) | DeclKind::SpecialVar(_)));
            let section = ranges.iter().position(|&(s, e)| va >= s && va < e);
            if let (true, Some(section)) = (is_var, section) {
                sections.entry(section).or_default().push((va, key));
            }
        }
        let mut runs: HashMap<(usize, usize), UnitRun> = HashMap::new();
        for (&section, vars) in &mut sections {
            vars.sort_unstable();
            for &(va, (unit, decl)) in vars.iter() {
                let run = runs.entry((section, unit)).or_default();
                run.at.push(va);
                run.max_decl.push(decl);
                run.min_decl.push(decl);
            }
        }
        for run in runs.values_mut() {
            let mut max = 0;
            for d in &mut run.max_decl {
                max = max.max(*d);
                *d = max;
            }
            let mut min = DeclId::MAX;
            for d in run.min_decl.iter_mut().rev() {
                min = min.min(*d);
                *d = min;
            }
        }
        Layout {
            sections,
            runs,
            ranges,
        }
    }

    /// The `(start, end)` VA range of each PE section, by section index.
    fn section_ranges(&self) -> Vec<(u64, u64)> {
        let Some(pe) = self.ctx.pe() else {
            return Vec::new();
        };
        let base = pe.image_base;
        pe.sections
            .iter()
            .map(|s| {
                let start = base.saturating_add(u64::from(s.virtual_address));
                let size = s.virtual_size.max(s.size_of_raw_data);
                (start, start.saturating_add(u64::from(size)))
            })
            .collect()
    }
}

/// A candidate owner of an initialization table slot.
#[derive(Debug, Clone)]
struct Owner {
    owner: InitOwner,
    /// The loaded unit that declares the routines.
    unit: usize,
    /// The routines: for a unit its initialization and finalization, for a
    /// class its two wrappers in either order.
    routines: [Option<DeclId>; 2],
    /// Whether `routines` is in table order (initialization first).
    ordered: bool,
    /// Whether every relocation in the routines has a known width, so the
    /// routines match their slot wherever it is.
    exact: bool,
}

/// The variables placed so far, by image section, for
/// [`Locator::name_init_table`]'s fit rule.
#[derive(Debug)]
struct Layout {
    /// Each section's placed variables in address order.
    sections: HashMap<usize, Vec<(u64, Key)>>,
    /// Each unit's placed variables, by section and unit.
    runs: HashMap<(usize, usize), UnitRun>,
    /// Each section's VA range.
    ranges: Vec<(u64, u64)>,
}

/// One unit's placed variables in one section, for [`Layout::fits`].
#[derive(Debug, Default)]
struct UnitRun {
    /// Their addresses, in order.
    at: Vec<u64>,
    /// At `i`, the highest declaration among the first `i + 1`.
    max_decl: Vec<DeclId>,
    /// At `i`, the lowest declaration from `i` on.
    min_decl: Vec<DeclId>,
}

impl Layout {
    /// Returns whether variable `key` can lie at `va`: the linker keeps a
    /// unit's variables together, in declaration order, so the unit's
    /// placed variables declared before it must lie below `va` and those
    /// declared after above, with no other unit's variable in between, and
    /// `va` must not lie inside another unit's run. `None` when it cannot,
    /// else whether `va` lies inside the unit's own run (placed variables
    /// of the unit on both sides).
    fn fits(&self, key: Key, va: u64) -> Option<bool> {
        let Some(section) = self.ranges.iter().position(|&(s, e)| va >= s && va < e) else {
            return Some(false);
        };
        let Some(vars) = self.sections.get(&section) else {
            return Some(false);
        };
        let (unit, decl) = key;
        let split = vars.partition_point(|&(at, _)| at < va);
        if vars.get(split).is_some_and(|&(at, _)| at == va) {
            return None;
        }
        // The unit's nearest placed variables below and above `va`. Those
        // below must all be declared before it, those above none.
        let (lo, hi) = match self.runs.get(&(section, unit)) {
            Some(run) => {
                let i = run.at.partition_point(|&at| at < va);
                let below = i.checked_sub(1);
                if below
                    .and_then(|j| run.max_decl.get(j))
                    .is_some_and(|&d| d >= decl)
                    || run.min_decl.get(i).is_some_and(|&d| d < decl)
                {
                    return None;
                }
                (
                    below.and_then(|j| run.at.get(j)).copied(),
                    run.at.get(i).copied(),
                )
            }
            None => (None, None),
        };
        // Nothing lies between them but `va`: any variable there belongs
        // to another unit, inside this unit's run.
        let from = lo.unwrap_or(va);
        let to = hi.unwrap_or(va);
        let after_from = vars.partition_point(|&(at, _)| at <= from);
        if vars.get(after_from).is_some_and(|&(at, _)| at < to) {
            return None;
        }
        let below = split.checked_sub(1).and_then(|i| vars.get(i));
        let above = vars.get(split);
        if matches!((below, above), (Some(&(_, (a, _))), Some(&(_, (b, _)))) if a == b && a != unit)
        {
            return None;
        }
        Some(lo.is_some() && hi.is_some())
    }
}

/// Who a unit initialization table slot belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InitOwner {
    /// A unit: its `initialization` and `finalization` sections.
    Unit(String),
    /// A class's class constructor and class destructor (Delphi 2010 and
    /// later), which take a slot of their own before the declaring unit's.
    Class {
        /// The declaring unit.
        unit: String,
        /// The class name.
        class: String,
    },
}

/// What [`crate::DelphiBinary::locate`] finds.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Location {
    /// Every declaration placed, ordered by unit and declaration.
    pub placements: Vec<Placement>,
    /// The owner of each slot of the unit initialization table, in table
    /// order (`None` where no rule names it, and for a program's own last
    /// slot).
    pub init_table: Vec<Option<InitOwner>>,
    /// The variables left unplaced because their relocations disagree
    /// ([`Locator::conflicts`]).
    pub conflicts: Vec<Conflict>,
}

impl Location {
    /// Returns the placed thread variables, with their offsets in the
    /// thread-local storage block.
    pub fn thread_variables(&self) -> impl Iterator<Item = (&Placement, u64)> + '_ {
        self.placements.iter().filter_map(|p| match p.address {
            Address::Thread(offset) => Some((p, offset)),
            Address::Va(_) => None,
        })
    }

    /// Returns the placement of `name` in unit `unit` (both compared
    /// without regard to ASCII case), when exactly one declaration of that
    /// name is placed.
    #[must_use]
    pub fn find(&self, unit: &str, name: &str) -> Option<&Placement> {
        let mut it = self
            .placements
            .iter()
            .filter(|p| p.unit.eq_ignore_ascii_case(unit) && p.name.eq_ignore_ascii_case(name));
        let first = it.next()?;
        it.next().is_none().then_some(first)
    }
}

/// What a binary tells [`locate`] about itself.
#[derive(Debug, Clone, Default)]
pub struct Module<'s> {
    /// The units the module contains, as its PACKAGEINFO lists them, the
    /// program's own main unit left out (it has no DCU).
    pub contains: Vec<&'s str>,
    /// Whether the module is a program that requires no runtime package:
    /// its initialization table then holds exactly its contained units and
    /// the program. A package's or a package-linked program's table also
    /// holds the units of the packages it requires.
    pub self_contained: bool,
    /// Whether the table ends with the program's own slot (a program, not
    /// a package or library).
    pub is_program: bool,
    /// The units the binary's classes name in their RTTI.
    pub class_units: Vec<&'s str>,
    /// Routines the binary's own metadata places, as (unit, routine name as
    /// the DCU spells it - `TClass.Method` -, address); a name several
    /// routines share is skipped.
    pub methods: Vec<(&'s str, String, u64)>,
}

/// Locates a binary's DCU declarations (see the module docs) with the
/// units of `library`: seeds the propagation with `module`'s methods and
/// names the unit initialization table's slots
/// ([`Locator::name_init_table`]), placing what their routines reach.
///
/// The candidates are the contained units, those a Delphi 2010+ type table
/// lists (the units with RTTI types), those the classes name, and
/// everything they reach through uses from `SysInit` and `System`. They
/// are complete - every slot's owner among them - when the module is
/// self-contained and the library has every contained unit.
#[must_use]
pub fn locate(
    library: &Library,
    ctx: &BinaryContext<'_>,
    table: &UnitInitTable<'_>,
    module: &Module<'_>,
) -> Location {
    let mut locator = Locator::new(library, ctx);
    for (unit, name, va) in &module.methods {
        let Some(u) = locator.unit(unit) else {
            continue;
        };
        let mut found = u.main.iter().copied().filter(|&id| {
            u.decls
                .get(id)
                .is_some_and(|d| matches!(d.kind, DeclKind::Proc(_)) && d.name == *name)
        });
        if let (Some(id), None) = (found.next(), found.next()) {
            locator.seed(unit, id, *va);
        }
    }
    let complete = module.self_contained
        && !module.contains.is_empty()
        && module
            .contains
            .iter()
            .all(|name| locator.unit(name).is_some());
    let mut todo: Vec<String> = ["SysInit", "System"]
        .into_iter()
        .chain(module.contains.iter().copied())
        .chain(module.class_units.iter().copied())
        .map(str::to_string)
        .chain(table.types.iter().flatten().map(|t| t.name().to_string()))
        .collect();
    let mut seen = HashSet::new();
    let mut names: Vec<String> = Vec::new();
    while let Some(name) = todo.pop() {
        if !seen.insert(name.to_ascii_lowercase()) {
            continue;
        }
        let Some(unit) = locator.unit(&name) else {
            continue;
        };
        todo.extend(
            unit.uses
                .iter()
                .filter(|i| i.section != UsesSection::Dll)
                .map(|i| i.name.clone()),
        );
        names.push(unit.name.clone());
    }
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let slots = if module.is_program {
        table.units.split_last().map_or(&[][..], |(_, rest)| rest)
    } else {
        &table.units[..]
    };
    let mut init_table = locator.name_init_table(slots, &refs, complete);
    if module.is_program && !table.units.is_empty() {
        init_table.push(None);
    }
    Location {
        placements: locator.placements(),
        init_table,
        conflicts: locator.conflicts(),
    }
}
