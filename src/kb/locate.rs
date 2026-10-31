//! Where a knowledge base's routines lie in a binary.
//!
//! A knowledge base records each library routine as its bytes with the
//! bytes its relocations patch masked out, and each relocation's target by
//! name ([`Dump`]). The linker copies a routine into a program unchanged
//! except at those bytes, so a routine is at an address exactly when the
//! image holds its masked bytes there - the test IDR's `ScanCode` applies
//! ([`Dump::matches`]).
//!
//! A byte pattern alone does not name a routine: several routines can share
//! one (overloads compiled alike, generic instances, small stubs), and a
//! short one recurs in any program by chance. Placement therefore starts
//! only where the evidence is unambiguous and grows from there along the
//! relocations, which name their targets:
//!
//! 1. **Scan.** Every routine whose pattern begins with a run of unmasked
//!    bytes is looked for at every address of the code sections. A pattern
//!    the code holds exactly once is placed there - a routine is linked at
//!    most once, so a pattern found twice is not the evidence of either -
//!    provided every relocation it names a routine through agrees: where the
//!    knowledge base holds a routine of that name with code, one of them
//!    must match at the address the relocation decodes to. The address must
//!    also be one the binary refers to: the smart linker drops a routine
//!    nothing refers to, so a linked routine is the target of a call or
//!    jump, or of a pointer (a VMT or method table slot, a unit
//!    initialization entry, an address loaded in code). A short pattern
//!    that recurs inside another routine's code is found once just as a
//!    real routine is, but nothing refers to the middle of a routine.
//!    Last, the routine's unit must be one the binary links, which shows as
//!    patterns no other unit has, found at more than one address: a
//!    program's own routine can compile to the bytes of a library routine
//!    of a unit it does not link (`Message.Result := 0` is one instruction
//!    pair in any message handler), and a pattern several units share says
//!    nothing about which of them is linked. And it must lie beside another
//!    routine of its unit: the linker lays a unit's code out in one piece,
//!    so a library routine the program does not link, whose bytes its own
//!    code repeats, is found among the program's routines, not its unit's.
//! 2. **Propagation.** Each placed routine's relocations decode to the
//!    addresses of the routines they name, and a routine of that name whose
//!    pattern matches there is placed in turn, however short - the name, not
//!    the bytes, identifies it. Routines no scan can tell apart (an `Exit`
//!    stub, an import thunk) are placed this way. An import thunk's own
//!    relocation names its import, and the slot it reads through must hold
//!    that import: two `external` stubs are one pattern, and only the slot
//!    tells `CompareStringA`'s from `CompareStringW`'s.
//!
//! Where a pattern matches at the address of a longer one, the longer
//! routine is the one there: the shorter is its prefix. Routines that share
//! a pattern are all placed where it is found, so one address can carry
//! several names (the knowledge base cannot tell which of them the program
//! linked); [`KbPlacement::shared`] says so, and from such an address only
//! the relocations all of them agree on are followed.
//!
//! The relocations decode through the DCU's table
//! ([`Encoding::of_kind`]), so this needs the release the knowledge base
//! was built for ([`Version`]) and the binary's platform.
//!
//! A [`KbIndex`] holds what the search needs of one knowledge base - its
//! routines' patterns, grouped and keyed, and each relocation's target
//! routines resolved by name - and each routine's declaration
//! ([`KbPrototype`]), and owns it, so a host builds it once per
//! knowledge base and places it in any number of binaries
//! ([`KbIndex::locate`]); [`locate`] does both for one binary.
//!
//! [`Dump`]: crate::kb::Dump
//! [`Dump::matches`]: crate::kb::Dump::matches

use std::{
    cmp::Reverse,
    collections::{BTreeMap, HashMap, HashSet, VecDeque, hash_map::Entry},
    ops::Range,
};

use crate::{
    dcu::{
        Platform, Version,
        locate::{Encoding, TargetKind},
    },
    formats::BinaryContext,
    imports,
    kb::{ArgTag, CallKind, Fixup, FixupKind, KnowledgeBase, Proc, ProcKind, TypeKind},
};

/// How a placement was established.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Reached {
    /// The routine's pattern occurs exactly once in the code sections, the
    /// binary refers to its address, every relocation that names a routine
    /// agrees, and its unit is one the binary links, beside another of its
    /// routines.
    Scan,
    /// A placed routine's relocation names it and decodes to its address,
    /// where its pattern matches.
    Reference,
}

/// One routine of a knowledge base placed in a binary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KbPlacement {
    /// The routine's virtual address.
    pub va: u64,
    /// The routine's index in the knowledge base's routine section.
    pub index: usize,
    /// The declaring module's ID.
    pub module: u16,
    /// The declaring unit's name ([`Module::declared_name`]).
    ///
    /// [`Module::declared_name`]: crate::kb::Module::declared_name
    pub unit: String,
    /// The routine's name (`Class.Method` for a method).
    pub name: String,
    /// The routine's size in bytes, its dump's length.
    pub size: u64,
    /// How it was placed.
    pub reached: Reached,
    /// Whether routines of other names share this pattern and are placed at
    /// the same address: the binary holds one of them, which the bytes
    /// cannot say.
    pub shared: bool,
}

/// The routines of a knowledge base placed in one binary.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KbLocation {
    /// Every placement, ordered by address and then routine index.
    pub placements: Vec<KbPlacement>,
}

impl KbLocation {
    /// Returns the placements at `va`.
    pub fn at(&self, va: u64) -> impl Iterator<Item = &KbPlacement> + '_ {
        let start = self.placements.partition_point(|p| p.va < va);
        self.placements
            .get(start..)
            .unwrap_or_default()
            .iter()
            .take_while(move |p| p.va == va)
    }

    /// Returns the number of placements the scan established, the measure
    /// [`identify`] ranks releases by.
    #[must_use]
    pub fn scanned(&self) -> usize {
        self.placements
            .iter()
            .filter(|p| p.reached == Reached::Scan)
            .count()
    }
}

impl Fixup<'_> {
    /// Returns the DCU relocation kind this fixup records, in a knowledge
    /// base of a release that is `modern` (Delphi 2010 or later), or `None`
    /// for a letter that names no kind.
    ///
    /// Before 2010 the kinds `1` - `3` are written `A`, `J` and `D`, and the
    /// block-end markers as their digits. From 2010 the relative kind `5` is
    /// written `J` and every other kind as its digit, `'0' + kind` - which
    /// makes the marker `0x11` an `A` and the kind `0x14` a `D`.
    #[must_use]
    pub fn dcu_kind(&self, modern: bool) -> Option<u8> {
        match (self.kind, modern) {
            (FixupKind::Jump, true) => Some(5),
            (FixupKind::Address, false) => Some(1),
            (FixupKind::Jump, false) => Some(2),
            (FixupKind::Data, false) => Some(3),
            (kind, _) => u8::from(kind).checked_sub(b'0'),
        }
    }
}

/// A relocation of a [`Routine`], as the search needs it.
#[derive(Debug, Clone)]
struct Reloc {
    /// The DCU relocation kind ([`Fixup::dcu_kind`]).
    kind: u8,
    /// Offset of the patched bytes in the routine.
    offset: usize,
    /// Number of patched bytes: the length of the mask's run there.
    width: usize,
    /// The bytes the routine holds there, the addend the linker adds the
    /// target's encoded address to.
    addend: i64,
    /// The routines with code of the name the relocation targets, other
    /// than the routine itself (indices into [`KbIndex::routines`]): those
    /// of the routine's own unit first, then those of the units it uses,
    /// then the rest - the order a name resolves in Pascal.
    targets: Vec<u32>,
    /// The lowercased name a relocation no other routine with code is named
    /// by targets: an import's for an `external` routine's stub (which names
    /// it by the routine's own name), so an import slot the relocation
    /// decodes to must hold an import of it.
    import: Option<Box<[u8]>>,
}

/// A routine's pattern: its length, and each run of bytes the image must
/// hold unchanged with where it lies.
type Pattern<'r> = (usize, Vec<(Range<usize>, &'r [u8])>);

/// A routine of the knowledge base with code.
#[derive(Debug, Clone)]
struct Routine {
    /// Its index in the knowledge base's routine section.
    index: usize,
    /// Its declaration.
    prototype: KbPrototype,
    /// The declaring module's ID.
    module: u16,
    /// Its name.
    name: String,
    /// Its bytes.
    code: Vec<u8>,
    /// The runs of bytes its relocations do not patch, which the image must
    /// hold unchanged.
    fixed: Vec<Range<usize>>,
    /// Its relocations.
    relocs: Vec<Reloc>,
}

impl Routine {
    /// Returns whether `bytes` starts with this routine, ignoring the bytes
    /// a relocation patches ([`Dump::matches`](crate::kb::Dump::matches)).
    fn matches(&self, bytes: &[u8]) -> bool {
        bytes.len() >= self.code.len()
            && self
                .fixed
                .iter()
                .all(|run| self.code.get(run.clone()) == bytes.get(run.clone()))
    }

    /// The number of unmasked bytes the routine begins with.
    fn lead(&self) -> usize {
        self.fixed
            .first()
            .filter(|run| run.start == 0)
            .map_or(0, |run| run.end)
    }

    /// The pattern: the bytes the image must hold, and where.
    fn pattern(&self) -> Pattern<'_> {
        let runs = self
            .fixed
            .iter()
            .map(|run| (run.clone(), self.code.get(run.clone()).unwrap_or_default()))
            .collect();
        (self.code.len(), runs)
    }

    /// Reads the routine from a knowledge-base record, with the lowercased
    /// name each of its relocations targets, in order.
    fn new(
        index: usize,
        module: u16,
        name: String,
        proc: &Proc<'_>,
        modern: bool,
    ) -> (Self, Vec<Vec<u8>>) {
        let dump = &proc.dump;
        let mut fixed = Vec::new();
        let mut start = None;
        for (i, &mask) in dump.relocs.iter().chain([&0xFF]).enumerate() {
            match (mask == 0xFF || i >= dump.code.len(), start) {
                (false, None) => start = Some(i),
                (true, Some(s)) => {
                    fixed.push(s..i);
                    start = None;
                }
                _ => {}
            }
        }
        let relocs = dump
            .fixups
            .iter()
            .filter_map(|fixup| {
                let offset = usize::try_from(fixup.offset).ok()?;
                let width = dump
                    .relocs
                    .get(offset..)?
                    .iter()
                    .take_while(|&&m| m == 0xFF)
                    .count();
                let bytes = dump.code.get(offset..)?;
                let addend = match width {
                    4 => i64::from(i32::from_le_bytes(bytes.get(..4)?.try_into().ok()?)),
                    8 => i64::from_le_bytes(bytes.get(..8)?.try_into().ok()?),
                    _ => return None,
                };
                Some((
                    Reloc {
                        kind: fixup.dcu_kind(modern)?,
                        offset,
                        width,
                        addend,
                        targets: Vec::new(),
                        import: None,
                    },
                    fixup.name.as_bytes().to_ascii_lowercase(),
                ))
            })
            .unzip();
        let (relocs, names): (Vec<Reloc>, Vec<Vec<u8>>) = relocs;
        (
            Self {
                index,
                prototype: KbPrototype::from(proc),
                module,
                name,
                code: dump.code.to_vec(),
                fixed,
                relocs,
            },
            names,
        )
    }
}

/// A routine's declaration as its knowledge base states it ([`Proc`]),
/// owned, so it outlives the knowledge base it was read from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KbPrototype {
    /// What the routine is: a procedure, a function, a method, a
    /// constructor or a destructor.
    pub kind: ProcKind,
    /// Its calling convention.
    pub call: CallKind,
    /// Whether it is declared `overload` (the DCU routine flag `0x1000`).
    pub overload: bool,
    /// A function's result type expression; empty for one with none.
    pub result: String,
    /// Its parameters in declaration order, `Self` and hidden ones included.
    pub params: Vec<KbParam>,
}

/// A parameter of a [`KbPrototype`] ([`Arg`](crate::kb::Arg)).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KbParam {
    /// How it is passed: by value or `const`, or by reference.
    pub tag: ArgTag,
    /// The DCU parameter flags ([`Local::flags`](crate::dcu::Local::flags)).
    pub flags: i32,
    /// The register number (`0` EAX, `1` EDX, `2` ECX) or the stack offset
    /// it is passed at.
    pub location: i32,
    /// Its name.
    pub name: String,
    /// Its type expression (`void ` for an untyped parameter).
    pub type_def: String,
}

impl From<&Proc<'_>> for KbPrototype {
    fn from(proc: &Proc<'_>) -> Self {
        let text = |t: &[u8]| String::from_utf8_lossy(t).into_owned();
        Self {
            kind: proc.kind,
            call: proc.call,
            overload: proc.vproc & 0x1000 != 0,
            result: text(proc.type_def.as_bytes()),
            params: proc
                .args
                .iter()
                .map(|arg| KbParam {
                    tag: arg.tag,
                    flags: arg.flags,
                    location: arg.location,
                    name: text(arg.name.as_bytes()),
                    type_def: text(arg.type_def.as_bytes()),
                })
                .collect(),
        }
    }
}

/// What the search needs of one knowledge base, owned, so it is built once
/// and placed in any number of binaries (see the module docs).
#[derive(Debug, Clone)]
pub struct KbIndex {
    /// The release the knowledge base was built for.
    version: Version,
    /// The routines with code.
    routines: Vec<Routine>,
    /// The routines grouped by pattern; the first of each stands for it.
    groups: Vec<Vec<u32>>,
    /// The groups whose pattern begins with eight unmasked bytes, by them.
    by8: HashMap<[u8; 8], Vec<u32>>,
    /// The groups whose pattern begins with four to seven, by the first
    /// four.
    by4: HashMap<[u8; 4], Vec<u32>>,
    /// The module ID of the knowledge base's `SysInit` unit.
    sysinit: Option<u16>,
    /// Each module's declared name, by ID.
    units: HashMap<u16, String>,
    /// The modules declaring a class of each (lowercased) name.
    classes: HashMap<String, Vec<u16>>,
    /// The type records of each (lowercased) name, for
    /// [`KbIndex::type_of`].
    types: HashMap<String, Vec<TypeRecord>>,
    /// Each module's `initialization` and `finalization` routines.
    sections: HashMap<u16, Sections>,
}

/// What [`KbIndex::type_of`] reads of a knowledge base's type record.
#[derive(Debug, Clone)]
struct TypeRecord {
    /// The declaring module's ID.
    module: u16,
    /// The kind of definition.
    kind: TypeKind,
    /// The size of a value of the type.
    size: u32,
    /// The declaration's right-hand side, as DCU32INT prints it.
    decl: String,
}

/// How a value of a knowledge base's type is held where a routine takes or
/// returns it, as its type record states it ([`KbIndex::type_of`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KbStorage {
    /// An integer or an enumeration of `bytes`, signed when its range's low
    /// bound is negative.
    Int {
        /// The value's size.
        bytes: u32,
        /// Whether the range reaches below zero.
        signed: bool,
    },
    /// A boolean of `bytes` (a `False..True` range).
    Bool {
        /// The value's size.
        bytes: u32,
    },
    /// A character of `bytes` (a range of characters).
    Char {
        /// The value's size.
        bytes: u32,
    },
    /// A floating-point number of `bytes`.
    Float {
        /// The value's size.
        bytes: u32,
    },
    /// An address: a typed pointer, a procedure that is not `of object`, a
    /// class or an interface.
    Pointer,
}

/// What a knowledge base's records say a type is ([`KbIndex::type_of`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KbType {
    /// A value held as the records state.
    Storage(KbStorage),
    /// The name an alias leads to that no record declares: a type the
    /// compiler knows without a declaration (`LongWord`, `Integer`), which
    /// the host reads as it reads that type anywhere.
    Intrinsic(String),
}

/// A unit's `initialization` routine - the one named after its unit - and
/// its `Finalization`, as routine indices of a [`KbIndex`].
#[derive(Debug, Clone, Copy, Default)]
struct Sections {
    init: Option<u32>,
    finalize: Option<u32>,
}

impl KbIndex {
    /// Builds the index of `kb`, a knowledge base of release `version`.
    ///
    /// Records that do not decode are left out, as a record whose bytes are
    /// data ([`Proc::holds_code`]) or empty is: nothing can be placed by
    /// them.
    #[must_use]
    pub fn new(kb: &KnowledgeBase<'_>, version: Version) -> Self {
        let modern = version.delphi_at_least(Version::DELPHI_2010);
        let section = kb.procs();
        let mut routines = Vec::new();
        let mut reloc_names: Vec<Vec<Vec<u8>>> = Vec::new();
        for index in 0..section.len() {
            let Ok(proc) = section.get(index) else {
                continue;
            };
            if !proc.holds_code(version) || proc.dump.is_empty() {
                continue;
            }
            let name = String::from_utf8_lossy(proc.name.as_bytes()).into_owned();
            let (routine, names) = Routine::new(index, proc.module, name, &proc, modern);
            routines.push(routine);
            reloc_names.push(names);
        }
        let mut by_name: HashMap<Vec<u8>, Vec<u32>> = HashMap::new();
        for (id, routine) in (0u32..).zip(&routines) {
            by_name
                .entry(routine.name.as_bytes().to_ascii_lowercase())
                .or_default()
                .push(id);
        }
        let uses: HashMap<u16, HashSet<u16>> = kb
            .modules()
            .iter()
            .filter_map(Result::ok)
            .map(|m| (m.id, m.uses.iter().map(|u| u.id).collect()))
            .collect();
        let modules: Vec<u16> = routines.iter().map(|r| r.module).collect();
        for (id, (routine, names)) in (0u32..).zip(routines.iter_mut().zip(&reloc_names)) {
            let own = routine.name.as_bytes().to_ascii_lowercase();
            let module = routine.module;
            let rank = |t: u32| {
                let target = modules.get(t as usize).copied();
                if target == Some(module) {
                    0
                } else if target.is_some_and(|m| uses.get(&module).is_some_and(|u| u.contains(&m)))
                {
                    1
                } else {
                    2
                }
            };
            for (reloc, name) in routine.relocs.iter_mut().zip(names) {
                if *name != own {
                    let mut targets: Vec<u32> = by_name
                        .get(name)
                        .map(|ids| ids.iter().copied().filter(|&t| t != id).collect())
                        .unwrap_or_default();
                    targets.sort_by_key(|&t| (rank(t), t));
                    reloc.targets = targets;
                }
                // An `external` stub's relocation names its import by the
                // routine's own name, as a reference into its own dump does;
                // only the address it decodes to tells them apart, which
                // `Locator::matches` reads.
                if reloc.targets.is_empty() {
                    reloc.import = Some(name.clone().into_boxed_slice());
                }
            }
        }
        let mut by_pattern: HashMap<Pattern<'_>, Vec<u32>> = HashMap::new();
        for (id, routine) in (0u32..).zip(&routines) {
            by_pattern.entry(routine.pattern()).or_default().push(id);
        }
        let mut groups: Vec<Vec<u32>> = by_pattern.into_values().collect();
        groups.sort_unstable();
        let mut by8: HashMap<[u8; 8], Vec<u32>> = HashMap::new();
        let mut by4: HashMap<[u8; 4], Vec<u32>> = HashMap::new();
        for (g, group) in (0u32..).zip(&groups) {
            let Some(routine) = group.first().and_then(|&id| routines.get(id as usize)) else {
                continue;
            };
            let lead = routine.lead();
            if lead >= 8 {
                if let Some(key) = routine.code.get(..8).and_then(|k| k.try_into().ok()) {
                    by8.entry(key).or_default().push(g);
                }
            } else if lead >= 4
                && let Some(key) = routine.code.get(..4).and_then(|k| k.try_into().ok())
            {
                by4.entry(key).or_default().push(g);
            }
        }
        let sysinit = kb.module_named("SysInit").ok().flatten().map(|m| m.id);
        let units: HashMap<u16, String> = kb
            .modules()
            .iter()
            .filter_map(Result::ok)
            .map(|m| (m.id, m.declared_name()))
            .collect();
        let mut classes: HashMap<String, Vec<u16>> = HashMap::new();
        let mut types: HashMap<String, Vec<TypeRecord>> = HashMap::new();
        for t in kb.types().iter().filter_map(Result::ok) {
            let name = String::from_utf8_lossy(t.name.as_bytes()).to_ascii_lowercase();
            if t.kind == TypeKind::Class {
                classes.entry(name.clone()).or_default().push(t.module);
            }
            types.entry(name).or_default().push(TypeRecord {
                module: t.module,
                kind: t.kind,
                size: t.size,
                decl: String::from_utf8_lossy(t.decl.as_bytes()).into_owned(),
            });
        }
        let mut sections: HashMap<u16, Sections> = HashMap::new();
        for (id, routine) in (0u32..).zip(&routines) {
            let unit = units.get(&routine.module).map(String::as_str);
            let entry = sections.entry(routine.module).or_default();
            if unit.is_some_and(|u| u.eq_ignore_ascii_case(&routine.name)) {
                entry.init = Some(id);
            } else if routine.name.eq_ignore_ascii_case("Finalization") {
                entry.finalize = Some(id);
            }
        }
        Self {
            version,
            routines,
            groups,
            by8,
            by4,
            sysinit,
            units,
            classes,
            types,
            sections,
        }
    }

    /// Returns the declaration of the routine `placement` places, as the
    /// knowledge base states it, or `None` for a placement this index did
    /// not make.
    #[must_use]
    pub fn prototype(&self, placement: &KbPlacement) -> Option<&KbPrototype> {
        let at = self
            .routines
            .binary_search_by_key(&placement.index, |routine| routine.index)
            .ok()?;
        self.routines.get(at).map(|routine| &routine.prototype)
    }

    /// Returns the number of routines with code the index holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.routines.len()
    }

    /// Returns whether the index holds no routine with code.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.routines.is_empty()
    }

    /// Returns the release the knowledge base was built for.
    #[must_use]
    pub fn version(&self) -> Version {
        self.version
    }

    /// Places the knowledge base's routines in the binary `ctx` describes
    /// (see the module docs).
    ///
    /// Empty for a binary that is neither 32- nor 64-bit Windows, which no
    /// published or built knowledge base targets.
    #[must_use]
    pub fn locate(&self, ctx: &BinaryContext<'_>) -> KbLocation {
        self.locate_with(ctx, &References::of(ctx))
    }

    /// [`KbIndex::locate`] with the binary's [`References`] already
    /// collected, so placing several knowledge bases in one binary reads
    /// its image for them once.
    #[must_use]
    pub fn locate_with(&self, ctx: &BinaryContext<'_>, references: &References) -> KbLocation {
        let platform = match (ctx.pe().is_some(), ctx.pointer_size()) {
            (true, Some(4)) => Platform::Win32,
            (true, Some(8)) => Platform::Win64,
            _ => return KbLocation::default(),
        };
        Locator {
            index: self,
            ctx,
            platform,
            references,
            // A slot imported by ordinal (`ORDINAL n`, as both import readers
            // spell it) states no name, so it says nothing about the name a
            // stub's relocation gives its import.
            imports: imports::collect(ctx)
                .into_iter()
                .filter(|import| !import.name().starts_with("ORDINAL "))
                .map(|import| (import.iat_va, import.name().as_bytes().to_ascii_lowercase()))
                .collect(),
            placed: HashMap::new(),
            at: HashMap::new(),
            queue: VecDeque::new(),
        }
        .run()
    }

    /// Returns the declared name of the knowledge base's module `id`.
    #[must_use]
    pub fn unit_name(&self, id: u16) -> Option<&str> {
        self.units.get(&id).map(String::as_str)
    }

    /// Returns the units that declare a class named `name` (any case).
    pub fn class_units(&self, name: &str) -> impl Iterator<Item = &str> + '_ {
        self.classes
            .get(&name.to_ascii_lowercase())
            .into_iter()
            .flatten()
            .filter_map(|id| self.unit_name(*id))
    }

    /// What the type `name` (`HWND`, or qualified, `Windows.HWND`) is, as
    /// the knowledge base's records of it state, following an alias to the
    /// type it names: how its values are held, or the intrinsic an alias
    /// leads to. `None` for a record or another value no register holds
    /// whole, for a name no record declares, and for a name several units
    /// declare differently.
    #[must_use]
    pub fn type_of(&self, name: &str) -> Option<KbType> {
        self.type_within(name, &mut HashSet::new())
    }

    /// [`Self::type_of`], where `seen` holds the names the aliases followed
    /// so far reached: a name reached again is a cycle, which states
    /// nothing.
    fn type_within(&self, name: &str, seen: &mut HashSet<String>) -> Option<KbType> {
        let name = name.trim();
        if !seen.insert(name.to_ascii_lowercase()) {
            return None;
        }
        let (unit, bare) = match name.rsplit_once('.') {
            Some((unit, bare)) => (Some(unit), bare),
            None => (None, name),
        };
        let mut found = None;
        for record in self
            .types
            .get(&bare.to_ascii_lowercase())?
            .iter()
            .filter(|record| {
                unit.is_none_or(|unit| {
                    self.unit_name(record.module)
                        .is_some_and(|declared| declared.eq_ignore_ascii_case(unit))
                })
            })
        {
            let resolved = self.record_type(bare, record, seen)?;
            match &found {
                Some(earlier) if *earlier != resolved => return None,
                _ => found = Some(resolved),
            }
        }
        found
    }

    /// Whether a record declares a type named `bare` in `unit` (in any unit
    /// when `None`).
    fn declares(&self, unit: Option<&str>, bare: &str) -> bool {
        self.types
            .get(&bare.to_ascii_lowercase())
            .is_some_and(|records| {
                records.iter().any(|record| {
                    unit.is_none_or(|unit| {
                        self.unit_name(record.module)
                            .is_some_and(|declared| declared.eq_ignore_ascii_case(unit))
                    })
                })
            })
    }

    /// What the type `record`, a record of the name `name`, describes is.
    fn record_type(
        &self,
        name: &str,
        record: &TypeRecord,
        seen: &mut HashSet<String>,
    ) -> Option<KbType> {
        // An alias names the type it renames; so does a range the knowledge
        // base records by its base type (`DWORD = Integer`) rather than by its
        // bounds (`low..high`, or an enumeration's `(a,b)`).
        let names_a_type = match record.kind {
            TypeKind::Alias => true,
            TypeKind::Range => {
                let decl = record.decl.trim();
                !decl.contains("..") && !decl.starts_with('(')
            }
            _ => false,
        };
        if names_a_type {
            let target = record.decl.trim();
            let target = target.strip_prefix("type ").unwrap_or(target);
            let (unit, bare) = match target.rsplit_once('.') {
                Some((unit, bare)) => (Some(unit), bare),
                None => (None, target),
            };
            // A record naming itself (`System`'s `LongBool = LongBool`) is how
            // the knowledge base records an intrinsic; `Windows`'s
            // `DWORD = Types.DWORD` names another unit's type.
            let itself = bare.eq_ignore_ascii_case(name)
                && unit.is_none_or(|unit| {
                    self.unit_name(record.module)
                        .is_some_and(|own| own.eq_ignore_ascii_case(unit))
                });
            return if !itself && self.declares(unit, bare) {
                self.type_within(target, seen)
            } else {
                Some(KbType::Intrinsic(target.to_string()))
            };
        }
        Self::record_storage(record).map(KbType::Storage)
    }

    /// How a value of the type `record` describes is held, for a record
    /// that is no alias.
    fn record_storage(record: &TypeRecord) -> Option<KbStorage> {
        let bytes = record.size;
        match record.kind {
            TypeKind::Pointer | TypeKind::Class | TypeKind::Interface => Some(KbStorage::Pointer),
            // A method pointer (`of object`) is the code and the instance.
            TypeKind::ProcType if !record.decl.trim_end().ends_with("of object") => {
                Some(KbStorage::Pointer)
            }
            TypeKind::Float if bytes > 0 => Some(KbStorage::Float { bytes }),
            TypeKind::Range if bytes > 0 => {
                // `low..high`, or an enumeration's `(a,b)`: the low bound
                // says what the range is of.
                let low = record.decl.split("..").next()?.trim();
                let first = low.chars().next()?;
                // A boolean's bounds are its values, `False..True`, or as
                // `LongBool`'s are recorded, `true{$80000000}..true{$7FFFFFFF}`.
                let lowered = low.to_ascii_lowercase();
                if lowered.starts_with("false") || lowered.starts_with("true") {
                    Some(KbStorage::Bool { bytes })
                } else if first == '#' || first == '\'' {
                    Some(KbStorage::Char { bytes })
                } else if first == '-' {
                    Some(KbStorage::Int {
                        bytes,
                        signed: true,
                    })
                } else if first == '$' || first == '(' || first.is_ascii_digit() {
                    Some(KbStorage::Int {
                        bytes,
                        signed: false,
                    })
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Names the unit each slot of the unit initialization table belongs to,
    /// with the routine that says so, from `location` (this knowledge base
    /// placed in the binary `ctx` describes). `slots` holds each slot's
    /// `initialization` and `finalization` addresses.
    ///
    /// A slot is a unit's when one of the unit's own section routines is
    /// placed at its address. Most units' initialization routines share one
    /// pattern - counting the unit in and out - so the scan places few of
    /// them; but the linker lays a unit's code out in one piece, its
    /// initialization last, so the routine placed before a slot's address
    /// names a candidate unit, and the slot is that unit's when its section
    /// pattern matches there. A unit has one initialization: where it
    /// matches at several slots (a unit with no code of its own follows the
    /// previous unit's, whose pattern it shares), the slot nearest the
    /// unit's own code is its, and the others stay unnamed. The basis says
    /// which way a slot was named ([`OwnerBasis`]): a unit with no code of
    /// its own whose section follows another unit's directly is
    /// indistinguishable from that unit's by its bytes, so a name by
    /// adjacency is an inference, measured right on 94 of the 96 fixture
    /// slots it names.
    #[must_use]
    pub fn init_owners(
        &self,
        location: &KbLocation,
        ctx: &BinaryContext<'_>,
        slots: &[(Option<u64>, Option<u64>)],
    ) -> Vec<Option<InitOwner>> {
        // Each slot's candidate: the unit, the routine, and the distance
        // from the end of the unit's preceding placed code.
        let candidate = |va: u64, pick: fn(&Sections) -> Option<u32>| {
            let placed = location.at(va).find(|p| {
                self.sections
                    .get(&p.module)
                    .and_then(pick)
                    .and_then(|id| self.routines.get(id as usize))
                    .is_some_and(|r| r.index == p.index)
            });
            if let Some(p) = placed {
                return Some((
                    p.module,
                    p.unit.clone(),
                    p.name.clone(),
                    va,
                    0,
                    OwnerBasis::Placed,
                ));
            }
            let before = location.placements.partition_point(|p| p.va < va);
            let previous = location.placements.get(before.checked_sub(1)?)?;
            let id = self.sections.get(&previous.module).and_then(pick)?;
            let routine = self.routines.get(id as usize)?;
            let bytes = ctx.va_to_file(va).and_then(|f| ctx.data().get(f..))?;
            let end = previous.va.checked_add(previous.size)?;
            let gap = va.checked_sub(end)?;
            // Adjacent: nothing but the compiler's alignment fill between
            // the unit's code and the slot's routine.
            let fill = ctx.va_to_file(end).and_then(|f| {
                ctx.data()
                    .get(f..f.checked_add(usize::try_from(gap).ok()?)?)
            })?;
            if !is_fill(fill) {
                return None;
            }
            routine.matches(bytes).then(|| {
                (
                    previous.module,
                    previous.unit.clone(),
                    routine.name.clone(),
                    va,
                    gap,
                    OwnerBasis::Adjacent,
                )
            })
        };
        let candidates: Vec<Option<Candidate>> = slots
            .iter()
            .map(|&(init, finalize)| {
                init.and_then(|va| candidate(va, |s| s.init))
                    .or_else(|| finalize.and_then(|va| candidate(va, |s| s.finalize)))
            })
            .collect();
        let mut nearest: HashMap<(u16, String), u64> = HashMap::new();
        for (module, _, routine, _, gap, _) in candidates.iter().flatten() {
            nearest
                .entry((*module, routine.to_ascii_lowercase()))
                .and_modify(|g| *g = (*g).min(*gap))
                .or_insert(*gap);
        }
        let mut claimed: HashSet<(u16, String)> = HashSet::new();
        candidates
            .into_iter()
            .map(|candidate| {
                let (module, unit, routine, va, gap, basis) = candidate?;
                let key = (module, routine.to_ascii_lowercase());
                (nearest.get(&key) == Some(&gap) && claimed.insert(key)).then_some(InitOwner {
                    unit,
                    routine,
                    va,
                    basis,
                })
            })
            .collect()
    }

    /// Returns whether `location` places a routine of the knowledge base's
    /// `SysInit` unit (see [`identify`]).
    #[must_use]
    pub fn places_sysinit(&self, location: &KbLocation) -> bool {
        self.sysinit
            .is_some_and(|id| location.placements.iter().any(|p| p.module == id))
    }
}

/// The code addresses a binary refers to (see [`References::of`]): what a
/// scan placement's address must be among. A property of the binary alone,
/// so it is collected once however many knowledge bases are placed in it.
#[derive(Debug, Clone, Default)]
pub struct References(HashSet<u64>);

impl References {
    /// Collects the code addresses the binary refers to: the targets of the
    /// `call` / `jmp rel32` (and on 64-bit Windows `lea reg, [rip + rel32]`)
    /// instructions anywhere in the code sections, and every pointer-sized
    /// value in the image that is a code address - a VMT, method table or
    /// unit initialization slot in data, an `offset` operand in code.
    ///
    /// The search reads every byte offset rather than decoding
    /// instructions, so it also collects addresses no instruction refers to;
    /// that only lets a placement through which the rest of its evidence
    /// (a pattern found exactly once, agreeing relocations) already admits.
    #[must_use]
    pub fn of(ctx: &BinaryContext<'_>) -> Self {
        let mut refs = HashSet::new();
        let wide = ctx.pointer_size() == Some(8);
        let width = if wide { 8 } else { 4 };
        let pointer = |bytes: &[u8]| -> Option<u64> {
            if wide {
                Some(u64::from_le_bytes(bytes.get(..8)?.try_into().ok()?))
            } else {
                Some(u64::from(u32::from_le_bytes(
                    bytes.get(..4)?.try_into().ok()?,
                )))
            }
        };
        let rel32 = |bytes: &[u8], next: u64| -> Option<u64> {
            let rel = i32::from_le_bytes(bytes.get(..4)?.try_into().ok()?);
            next.checked_add_signed(i64::from(rel))
        };
        let Some(pe) = ctx.pe() else {
            return Self(refs);
        };
        let base = pe.image_base;
        for section in &pe.sections {
            let start = base.saturating_add(u64::from(section.virtual_address));
            let Some(bytes) = ctx
                .va_to_file(start)
                .and_then(|at| ctx.data().get(at..))
                .and_then(|b| b.get(..usize::try_from(section.size_of_raw_data).ok()?))
            else {
                continue;
            };
            let code = ctx.is_code_va(start);
            for (at, va) in (0..bytes.len()).zip(start..) {
                let Some(rest) = bytes.get(at..) else {
                    break;
                };
                // A pointer in data is aligned to its width; one in code is
                // an operand, at any offset.
                if (code || at.checked_rem(width) == Some(0))
                    && let Some(target) = pointer(rest)
                    && ctx.is_code_va(target)
                {
                    refs.insert(target);
                }
                if !code {
                    continue;
                }
                // The operand's offset in the instruction, and the address
                // just past it that a `rel32` counts from.
                let operand: Option<(usize, u64)> = match rest {
                    [0xE8 | 0xE9, ..] => Some((1, 5)),
                    [0x48 | 0x4C, 0x8D, modrm, ..] if wide && modrm & 0xC7 == 0x05 => Some((3, 7)),
                    _ => None,
                };
                if let Some((operand, end)) = operand
                    && let Some(target) = rest
                        .get(operand..)
                        .and_then(|b| rel32(b, va.checked_add(end)?))
                    && ctx.is_code_va(target)
                {
                    refs.insert(target);
                }
            }
        }
        Self(refs)
    }

    /// Returns whether the binary refers to `va`.
    #[must_use]
    pub fn contains(&self, va: u64) -> bool {
        self.0.contains(&va)
    }
}

/// The unit a slot of the unit initialization table belongs to, as
/// [`KbIndex::init_owners`] names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitOwner {
    /// The unit.
    pub unit: String,
    /// The routine of the unit the slot's address holds: the one named after
    /// the unit (its `initialization`) or `Finalization`.
    pub routine: String,
    /// The address that holds [`Self::routine`]: the slot's initialization
    /// address, or its finalization address when the slot was named by that.
    pub va: u64,
    /// How the slot was named.
    pub basis: OwnerBasis,
}

/// How [`KbIndex::init_owners`] named a slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OwnerBasis {
    /// The unit's section routine is placed at the slot's address.
    Placed,
    /// The unit's section pattern matches at the address, which follows the
    /// unit's placed code with nothing but alignment fill between.
    Adjacent,
}

/// A slot's candidate owner: module, unit, routine, the address holding the
/// routine, distance from the unit's code and basis.
type Candidate = (u16, String, String, u64, u64, OwnerBasis);

/// Returns whether `bytes` is nothing but the fill Delphi aligns routines
/// with: `nop` (`90`), `mov eax, eax` (`8B C0`) and `lea eax, [eax + 0]`
/// (`8D 40 00`) on Win32, `int3` (`CC`) on Win64.
fn is_fill(mut bytes: &[u8]) -> bool {
    while !bytes.is_empty() {
        bytes = match bytes {
            [0x90 | 0xCC, rest @ ..] | [0x8B, 0xC0, rest @ ..] | [0x8D, 0x40, 0x00, rest @ ..] => {
                rest
            }
            _ => return false,
        };
    }
    true
}

/// One binary being searched for one knowledge base's routines.
struct Locator<'i, 'c> {
    index: &'i KbIndex,
    ctx: &'c BinaryContext<'c>,
    platform: Platform,
    references: &'i References,
    /// The lowercased name of the import each import slot holds, by the
    /// slot's address.
    imports: HashMap<u64, Vec<u8>>,
    /// Placed routines: routine to address and how it was reached.
    placed: HashMap<u32, (u64, Reached)>,
    /// The routines placed at each address.
    at: HashMap<u64, Vec<u32>>,
    /// Routines to follow the relocations of.
    queue: VecDeque<(u32, u64)>,
}

impl Locator<'_, '_> {
    /// Returns the routine `id`.
    fn routine(&self, id: u32) -> Option<&Routine> {
        self.index.routines.get(id as usize)
    }

    /// Returns the image bytes from `va` to the end of the file.
    fn bytes_at(&self, va: u64) -> Option<&[u8]> {
        let at = self.ctx.va_to_file(va)?;
        self.ctx.data().get(at..)
    }

    /// Returns whether routine `id` is one the image can hold at `va`: its
    /// pattern matches there, and every relocation of it that names an
    /// import and decodes to an import slot finds that import in the slot.
    ///
    /// The relocation's bytes are masked, so the pattern alone cannot tell
    /// two `external` stubs apart (`CompareStringA`'s from
    /// `CompareStringW`'s): the slot it reads through can.
    fn matches(&self, id: u32, va: u64) -> bool {
        match (self.routine(id), self.bytes_at(va)) {
            (Some(routine), Some(bytes)) => {
                routine.matches(bytes)
                    && routine.relocs.iter().all(|reloc| {
                        reloc.import.as_deref().is_none_or(|name| {
                            self.address(reloc, va)
                                .and_then(|slot| self.imports.get(&slot))
                                .is_none_or(|held| held.as_slice() == name)
                        })
                    })
            }
            _ => false,
        }
    }

    /// Finds every address of the code sections where each indexed pattern
    /// occurs.
    ///
    /// A pattern is indexed when it begins with at least four unmasked
    /// bytes; one beginning with eight is keyed by those, which keeps the
    /// common prologues apart.
    fn occurrences(&self) -> Vec<Vec<u64>> {
        let mut found = vec![Vec::new(); self.index.groups.len()];
        for &(start, end) in self.ctx.code_ranges() {
            let Some(bytes) = self.bytes_at(start) else {
                continue;
            };
            let len = usize::try_from(end.saturating_sub(start))
                .unwrap_or(usize::MAX)
                .min(bytes.len());
            let bytes = bytes.get(..len).unwrap_or_default();
            for (at, va) in (0..len).zip(start..) {
                let Some(rest) = bytes.get(at..) else {
                    break;
                };
                let keyed8 = rest
                    .get(..8)
                    .and_then(|k| <[u8; 8]>::try_from(k).ok())
                    .and_then(|k| self.index.by8.get(&k));
                let keyed4 = rest
                    .get(..4)
                    .and_then(|k| <[u8; 4]>::try_from(k).ok())
                    .and_then(|k| self.index.by4.get(&k));
                for &g in keyed8.into_iter().chain(keyed4).flatten() {
                    let first = self
                        .index
                        .groups
                        .get(g as usize)
                        .and_then(|group| group.first());
                    if first
                        .and_then(|&id| self.routine(id))
                        .is_some_and(|r| r.matches(rest))
                        && let Some(vas) = found.get_mut(g as usize)
                    {
                        vas.push(va);
                    }
                }
            }
        }
        found
    }

    /// Decodes the code address relocation `reloc` of a routine placed at
    /// `va` gives; `None` when it gives none.
    fn target(&self, reloc: &Reloc, va: u64) -> Option<u64> {
        self.address(reloc, va)
            .filter(|&target| self.ctx.is_code_va(target))
    }

    /// Decodes the address relocation `reloc` of a routine placed at `va`
    /// gives, code or not (an import slot); `None` when it gives none.
    fn address(&self, reloc: &Reloc, va: u64) -> Option<u64> {
        let encoding = Encoding::of_kind(
            self.index.version,
            self.platform,
            reloc.kind,
            TargetKind::Code,
        )?;
        let read = |bytes: &[u8]| -> Option<i64> {
            match reloc.width {
                4 => Some(i64::from(i32::from_le_bytes(
                    bytes.get(..4)?.try_into().ok()?,
                ))),
                8 => Some(i64::from_le_bytes(bytes.get(..8)?.try_into().ok()?)),
                _ => None,
            }
        };
        let site = va.checked_add(u64::try_from(reloc.offset).ok()?)?;
        let patched = read(self.bytes_at(site)?)?;
        let value = patched.checked_sub(reloc.addend)?;
        let end = i64::try_from(site.checked_add(u64::try_from(reloc.width).ok()?)?).ok()?;
        let address = |v: i64| -> Option<u64> {
            if self.ctx.pointer_size() == Some(4) {
                u32::try_from(v & 0xFFFF_FFFF).ok().map(u64::from)
            } else {
                u64::try_from(v).ok()
            }
        };
        let target = match encoding {
            Encoding::Absolute => address(value)?,
            Encoding::Relative => address(value.checked_add(end)?)?,
            Encoding::ImageRelative => {
                address(value.checked_add(i64::try_from(self.ctx.image_base()?).ok()?)?)?
            }
            Encoding::Indirect | Encoding::IndirectRelative | Encoding::ThreadOffset => {
                return None;
            }
        };
        Some(target)
    }

    /// Returns whether every relocation of routine `id` at `va` that names
    /// a routine with code decodes to an address where one of that name
    /// matches.
    fn agrees(&self, id: u32, va: u64) -> bool {
        let Some(routine) = self.routine(id) else {
            return false;
        };
        routine.relocs.iter().all(|reloc| {
            if reloc.targets.is_empty() {
                return true;
            }
            match self.target(reloc, va) {
                Some(target) => reloc.targets.iter().any(|&t| self.matches(t, target)),
                // A relocation this table does not decode to code (data, a
                // thread variable) says nothing either way.
                None => true,
            }
        })
    }

    /// Places routine `id` at `va` and queues its relocations.
    fn place(&mut self, id: u32, va: u64, reached: Reached) {
        if let Entry::Vacant(e) = self.placed.entry(id) {
            e.insert((va, reached));
            self.at.entry(va).or_default().push(id);
            self.queue.push_back((id, va));
        }
    }

    /// Returns whether every routine of another name placed at `va` has a
    /// relocation at `reloc`'s offset naming the same routines: where
    /// routines of different names share a pattern, the binary holds one of
    /// them, so only what they all say about their relocations holds.
    fn shared_by_all(&self, id: u32, va: u64, reloc: &Reloc) -> bool {
        let Some(name) = self.routine(id).map(|r| r.name.as_str()) else {
            return false;
        };
        self.at.get(&va).into_iter().flatten().all(|&other| {
            self.routine(other).is_some_and(|o| {
                o.name.eq_ignore_ascii_case(name)
                    || o.relocs
                        .iter()
                        .any(|r| r.offset == reloc.offset && r.targets == reloc.targets)
            })
        })
    }

    /// Follows the relocations of the placed routines until nothing new is
    /// placed.
    fn propagate(&mut self) {
        while let Some((id, va)) = self.queue.pop_front() {
            let Some(routine) = self.routine(id) else {
                continue;
            };
            let mut reached = Vec::new();
            for reloc in &routine.relocs {
                if reloc.targets.is_empty() || !self.shared_by_all(id, va, reloc) {
                    continue;
                }
                let Some(target) = self.target(reloc, va) else {
                    continue;
                };
                // The first routine of the name that matches, in the order
                // the name resolves; one already placed there settles it.
                if let Some(&t) = reloc.targets.iter().find(|&&t| self.matches(t, target))
                    && !self.placed.contains_key(&t)
                {
                    reached.push((t, target));
                }
            }
            for (t, target) in reached {
                self.place(t, target, Reached::Reference);
            }
        }
    }

    /// Runs the scan and the propagation.
    fn run(mut self) -> KbLocation {
        let found = self.occurrences();
        let len_of = |group: &Vec<u32>| {
            group
                .first()
                .and_then(|&id| self.routine(id))
                .map_or(0, |r| r.code.len())
        };
        // The longest pattern matching at an address is the routine there.
        let mut longest: HashMap<u64, usize> = HashMap::new();
        for (group, vas) in self.index.groups.iter().zip(&found) {
            let len = len_of(group);
            for &va in vas {
                longest
                    .entry(va)
                    .and_modify(|l| *l = (*l).max(len))
                    .or_insert(len);
            }
        }
        let mut scanned: Vec<(&Vec<u32>, u64)> = Vec::new();
        for (group, vas) in self.index.groups.iter().zip(&found) {
            let [va] = vas[..] else {
                continue;
            };
            let Some(&first) = group.first() else {
                continue;
            };
            if longest.get(&va).is_some_and(|&l| len_of(group) < l)
                || !self.references.contains(va)
                || !self.agrees(first, va)
            {
                continue;
            }
            scanned.push((group, va));
        }
        // A unit the binary links shows more than one routine of its own: a
        // pattern no other unit has, found at more than one address. One
        // found alone is a pattern the program's own code shares with a unit
        // it does not link, and a pattern other units share says nothing
        // about which of them is linked.
        let modules_of = |group: &Vec<u32>| -> HashSet<u16> {
            group
                .iter()
                .filter_map(|&id| self.routine(id).map(|r| r.module))
                .collect()
        };
        let mut evidence: HashMap<u16, HashSet<u64>> = HashMap::new();
        for &(group, va) in &scanned {
            let modules = modules_of(group);
            if let (1, Some(&module)) = (modules.len(), modules.iter().next()) {
                evidence.entry(module).or_default().insert(va);
            }
        }
        let linked: HashSet<u16> = evidence
            .into_iter()
            .filter(|(_, vas)| vas.len() > 1)
            .map(|(module, _)| module)
            .collect();
        let placements: Vec<(u32, u64)> = scanned
            .iter()
            .flat_map(|&(group, va)| group.iter().map(move |&id| (id, va)))
            .filter(|&(id, _)| self.routine(id).is_some_and(|r| linked.contains(&r.module)))
            .collect();
        // The linker lays a unit's code out in one piece. So a placement
        // between two of another unit's is not that unit's routine (and stops
        // nothing else being next to its own), and a routine of a unit lies
        // next to another of the same unit: the placement before or after its
        // address carries its unit.
        let mut units_at: BTreeMap<u64, HashSet<u16>> = BTreeMap::new();
        for &(id, va) in &placements {
            if let Some(routine) = self.routine(id) {
                units_at.entry(va).or_default().insert(routine.module);
            }
        }
        loop {
            let sandwiched: Vec<u64> = units_at
                .iter()
                .filter(|&(&va, units)| {
                    let before = units_at.range(..va).next_back();
                    let after = units_at.range(va.saturating_add(1)..).next();
                    let (Some((_, before)), Some((_, after))) = (before, after) else {
                        return false;
                    };
                    before.intersection(after).any(|unit| !units.contains(unit))
                })
                .map(|(&va, _)| va)
                .collect();
            if sandwiched.is_empty() {
                break;
            }
            for va in sandwiched {
                units_at.remove(&va);
            }
        }
        let beside = |va: u64, module: u16| {
            let before = units_at.range(..va).next_back();
            let after = units_at.range(va.saturating_add(1)..).next();
            units_at.contains_key(&va)
                && before
                    .into_iter()
                    .chain(after)
                    .any(|(_, units)| units.contains(&module))
        };
        let placements: Vec<(u32, u64)> = placements
            .into_iter()
            .filter(|&(id, va)| self.routine(id).is_some_and(|r| beside(va, r.module)))
            .collect();
        for (id, va) in placements {
            self.place(id, va, Reached::Scan);
        }
        self.propagate();
        self.into_location()
    }

    /// Collects the placements, marking addresses that carry more than one
    /// name.
    fn into_location(self) -> KbLocation {
        let mut names_at: HashMap<u64, HashSet<String>> = HashMap::new();
        for (&id, &(va, _)) in &self.placed {
            if let Some(routine) = self.routine(id) {
                names_at
                    .entry(va)
                    .or_default()
                    .insert(routine.name.to_ascii_lowercase());
            }
        }
        let mut placements: Vec<KbPlacement> = self
            .placed
            .iter()
            .filter_map(|(&id, &(va, reached))| {
                let routine = self.routine(id)?;
                Some(KbPlacement {
                    va,
                    index: routine.index,
                    module: routine.module,
                    unit: self
                        .index
                        .units
                        .get(&routine.module)
                        .cloned()
                        .unwrap_or_default(),
                    name: routine.name.clone(),
                    size: routine.code.len() as u64,
                    reached,
                    shared: names_at.get(&va).is_some_and(|n| n.len() > 1),
                })
            })
            .collect();
        placements.sort_unstable_by_key(|p| (p.va, p.index));
        KbLocation { placements }
    }
}

/// Places the routines of `kb`, a knowledge base of release `version`, in
/// the binary `ctx` describes (see the module docs): [`KbIndex::new`] and
/// [`KbIndex::locate`] for one binary.
#[must_use]
pub fn locate(kb: &KnowledgeBase<'_>, version: Version, ctx: &BinaryContext<'_>) -> KbLocation {
    KbIndex::new(kb, version).locate(ctx)
}

/// Ranks candidate releases for the binary `ctx` describes: each candidate
/// is the index of a knowledge base (a `syskb` suffices), and its score is
/// the number of its routines the scan places ([`KbLocation::scanned`]).
///
/// A candidate counts only when it places a routine of its `SysInit` unit:
/// every module the release builds links that unit - a Delphi program,
/// library or package enters through it, and a C++Builder one links it for
/// the VCL's startup (`VclInit`) - so a binary in which no candidate places
/// one was not built by any of these releases. A Free Pascal binary shares
/// a few `System` routines' bytes with each release by chance, and none of
/// `SysInit`'s.
///
/// Returns the counting candidates' positions, best first, with their
/// scores. The release's `System` unit differs between releases, so the
/// knowledge base of the release that built the binary places the most of
/// it; equal scores mean the knowledge bases cannot tell those releases
/// apart for this binary.
#[must_use]
pub fn identify(ctx: &BinaryContext<'_>, candidates: &[&KbIndex]) -> Vec<(usize, usize)> {
    identify_with(ctx, &References::of(ctx), candidates)
}

/// [`identify`] with the binary's [`References`] already collected.
#[must_use]
pub fn identify_with(
    ctx: &BinaryContext<'_>,
    references: &References,
    candidates: &[&KbIndex],
) -> Vec<(usize, usize)> {
    let mut ranked: Vec<(usize, usize)> = candidates
        .iter()
        .enumerate()
        .filter_map(|(i, index)| {
            let location = index.locate_with(ctx, references);
            index
                .places_sysinit(&location)
                .then(|| (i, location.scanned()))
        })
        .collect();
    ranked.sort_by_key(|&(i, score)| (Reverse(score), i));
    ranked
}
