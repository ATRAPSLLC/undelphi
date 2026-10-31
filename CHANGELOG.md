# Changelog

## 0.5.0

Library recognition release. undelphi places an IDR knowledge base's
routines in a binary and names the release that built a statically linked
one, and the knowledge bases it builds for Delphi 2010 and later now record
relocations as IDR's files do. Code entry points cover every method Delphi
2010+ RTTI names, and `structure_extents` states each class's init table.
A few API changes break compatibility; see **Changed**.

### Added

- **Placing knowledge-base routines in a binary.** `kb::locate::KbIndex`
  holds what the search needs of one knowledge base, owned, so it is built
  once and placed in any number of binaries (`KbIndex::locate`;
  `kb::locate::locate` for one). A routine's pattern (its bytes with the
  relocated ones masked) is placed where the code holds it exactly once,
  the binary refers to the address, every relocation that names a routine
  decodes to one of that name, and its unit is linked and laid out beside
  it; each placed routine's relocations then place the routines they name.
  `KbPlacement` carries the routine, its declaring unit
  (`Module::declared_name`), how it was reached
  (`Reached::{Scan, Reference}`) and whether routines of other names share
  its bytes (`shared`). `References` / `identify_with` /
  `KbIndex::locate_with` let several knowledge bases be placed in one
  binary reading its image once. On the Delphi 6 fixtures with `kb6.bin`,
  3 170 of `gtforms`' 3 572 code publics are named as the linker map
  names them; on Delphi 13 Win32 / Win64 with knowledge bases built from
  its units, 7 210 of 13 232 and 7 518 of 13 759 (the rest is the
  program's own code and generic instances); no placement names a routine the map names
  otherwise, except where the two routines are byte for byte the same
  (`tests/kblocate.rs`).
- **A placed routine's declaration.** `KbIndex::prototype` returns the
  `KbPrototype` of the routine a `KbPlacement` places, owned: its kind,
  calling convention, `overload` flag, result type and parameters (`KbParam`:
  passing, DCU flags, location, name and type expression), `Self` and hidden
  parameters included. Overloads the bytes tell apart keep their own
  declarations: a Delphi 6 program's two `IntToStr` placements take
  `System.Integer` and `System.Int64`.
- **Code entry points for every method RTTI names.** `code_entrypoints`
  labels a class's Delphi 2010+ extended-RTTI methods of every visibility
  (`EntrypointKind::ExtendedMethod`, `Class.Method`; an `abstract` method,
  whose entry is `System.@AbstractError`, is left out) and every record
  type's methods, constructors and operators
  (`EntrypointKind::RecordMethod`, `Record.Method`). Before, only published
  methods were labelled, so a modern class's private, protected and public
  methods went unnamed. A class's `automated` section is labelled too: its
  methods as `EntrypointKind::AutomatedMethod`, its property accessors as
  getters and setters.
- **A method's declared prototype.** `MethodSignature::declared_params`
  gives the parameters the source declares, without the compiler's hidden
  ones and without Delphi's implicit `Self`, which (unlike FPC's) carries no
  flag and is the first parameter of a method that has one
  (`MethodSignature::has_self`). `DelphiBinary::type_at` decodes the type any
  `PPTypeInfo` cell names, a parameter's, result's or record field's among
  them, and `DelphiBinary::declaring_units` maps each RTTI type to the unit
  that declares it (from the 2010+ type table), which a record's own RTTI
  does not state. `rtti::RecordMethod::method_kind` and
  `has_self` read a record method's kind and `Self` the same way, and
  `rtti::SignatureParam::param_flags` its parameters' passing.
  `ParamMode`, `DispatchKind` and `extrtti::Visibility` gain a
  stable `as_str` label (`ParamMode::by_ref` says which modes pass by
  reference).
- **Attributes as source spells them.** `extrtti::DecodedAttribute` and
  `AttributeArg` implement `Display`: `[TagAttribute('method', 3)]`, a
  string quoted with `''` escaping, a set as `[1, 3]`, a class or type-info
  reference by its type's name.
- **How a type's value is held.** `rtti::TypeDetail::storage` reports the
  machine shape of a scalar, string or reference type (`rtti::Storage`: an
  integer of a width and signedness, `Boolean`, a character, a float, or a
  pointer to characters, a class instance, an interface - managed, unlike a
  class reference - or nothing a consumer can lay out),
  with `Extended` 10 bytes on Win32 and a `Double` on Win64 and `Comp` /
  `Currency` a signed 64-bit integer. A type described by its name or
  declaration (a record, an array, a set, a typed pointer) has none.
  `rtti::OrdinalType::width` gives an ordinal's width and signedness.
- **Naming the unit initialization table.** `KbIndex::init_owners` names
  each slot after the unit whose section routine is placed at its address
  (`OwnerBasis::Placed`), or whose identical initialization or finalization
  pattern matches right after the unit's placed code with nothing but
  alignment fill between (`OwnerBasis::Adjacent`), one slot per unit. On
  the fixtures 94 of the 96 slots it names are the linker map's; the two it
  misnames are units with no code of their own, which share the previous
  unit's bytes. `InitOwner::va` is the address that holds the owner's
  routine: the slot's initialization address, or its finalization address
  when the slot was named by that. `KbIndex::class_units` names the units
  declaring a class.
- **Which knowledge-base records hold code.** The Delphi 4 - 7 knowledge
  bases file the compiler's unnamed blocks (`_NF__<index>`: string
  literals, resource string and exception tables) as code unless a typed
  constant refers to them, and the Borland C++ type descriptors of
  `MidasLib` and the dbExpress drivers as code under the name `_DF_.`.
  `Proc::holds_code` tells them apart: an unnamed block from Delphi 4 on
  needs a call or jump relocation (`Dump::calls`), which the interface
  method tables' thunks have, and a `_DF_.` record whose dump has the shape
  of a descriptor (`Dump::is_type_descriptor`) is data. `KbIndex` places
  only records that hold code, so no pointer table is named as a routine
  (`SysUtils._NF__AD4` in a Delphi 6 program).
- `kb::published_release` gives the release a knowledge base's file name
  stands for (`kb6.bin`, `syskb2014.bin`, `syskb-37.0.bin`) and the
  compiler version `dcc` reports for it, and `dcu::Version::of_compiler`
  the release whose compiler reports a version.
- **Release identification.** `kb::locate::identify` ranks `syskb`
  knowledge bases for a binary by how many of their routines the scan
  places, counting only those that place a routine of their `SysInit`
  unit: the Delphi 6 and C++Builder 6 fixtures rank Delphi 6 first, the
  Delphi 13 ones Delphi 13, and Free Pascal binaries none.
- **Reading the notation back.** `kb::notation` reads the type
  expressions, field and parameter types and method prototypes a knowledge
  base stores as DCU32INT prints them into structure: `TypeExpr` (named,
  pointer, array, range, enumeration, set, file, short string, float, class
  reference, class, interface, procedural type), `Prototype` with
  `RoutineKind`, `Signature`, `Param` and `Pass`. Generic instances read as
  one qualified name from both the published files' spelling
  (`TArray<System.TClass>System.TArray`1<System.TClass>`) and `build`'s
  (`{System}TArray<System.TClass>`). Every type expression and prototype of
  `kb6` reads to structure; of `kb2014`, all but IDR's anonymous `:n` type
  references.
- `Fixup::dcu_kind` gives the DCU relocation kind a knowledge-base fixup
  letter records, and `dcu::locate::Encoding::of_kind` decodes a kind for a
  release and platform, so DCUs and knowledge bases share one table.
- `BinaryContext::code_ranges` gives the executable sections' address
  ranges, the ones `BinaryContext::is_code_va` tests against.
- **Init table extents.** `structure_extents` states each Delphi class's init
  table (`StructureKind::InitTable`, `RecordInfo::init_table_extent`): the
  `vmtInitTable` record's Kind byte, empty name, size, managed-field count
  and one (type, offset) pair per field, measured from the stored count.
  Delphi 11 (compiler 35.0) and later end the table with the operator
  section every named record carries, `NumOps: Byte` and `RecOps[NumOps]`;
  `InitTableLayout` states which a binary has (`DelphiBinary::init_table_layout`),
  and the decode reads the section into `RecordInfo::operators`. A binary
  whose compiler version is known is dated by it
  (`InitTableLayout::of_compiler`): measured on 33.0 and 34.0 binaries (no
  section) and on 35.0, 36.0 and 37.0 (the section follows every table), so
  the next structure starts exactly at each table's end. A Delphi or
  C++Builder binary no build string or RTL package dates (a stripped build
  string, or the versionless `Embarcadero RAD Studio 37.0` marker of the RAD
  Studio 13 command-line compiler) shows its layout by its own tables
  (`InitTableLayout::shown_by`): the operator section is read only when,
  read with it, every table decodes and every table that something follows
  is abutted by the next structure, with at least one followed; otherwise
  the tables end at their managed fields. On 1,514 Delphi binaries the
  measurement agrees with the compiler version on all 1,445 dated ones, and
  every init table of all 1,514 is followed by the next structure at its
  stated end. The measurement costs undated binaries two extra decodes of
  their init tables (one under each layout). Light Alloy's 124 tables
  (3 640 bytes) lie among its routines, where a disassembler decoded them as
  code. `RecordInfo::extent` gives any record decode's extent. Free Pascal's
  init record is not measured.

### Changed

- **`initfini::UnitInitProc::unit_name` is an `Option`.** `None` where the
  table names no unit (every Delphi table, an FPC build without
  `FPC_INITFINAL_HASUNITNAME`) or the name is not UTF-8, where it was the
  placeholder `"<unknown>"` / `"<non-ascii>"`, which `code_entrypoints`
  wrote into labels as if it were a unit (`<unknown>.Initialization`). An
  unnamed slot's entry points are labelled by position
  (`unit[3].Initialization`).
- **New variants on exhaustive enums.** `EntrypointKind::ExtendedMethod`,
  `RecordMethod` and `AutomatedMethod`, `StructureKind::InitTable`, and
  `TypeKind::Helper` and `TypeKind::File` (Free Pascal's `tkHelper` and
  `tkFile`, which were `TypeKind::Unknown`). A `match` over these enums
  needs the new arms.
- **`RecordInfo::from_init_table` takes an `InitTableLayout`**, the section
  a Delphi 11 or later init table ends with; `DelphiBinary::init_table`
  passes its binary's.
  `RecordInfo::from_va` reads an unnamed record to its managed fields only;
  init tables go through `from_init_table`.

### Fixed

- `kb::build` names a module after the unit's own source file. It took the
  first source the unit lists, which from 10 Seattle is an include file:
  `System`'s routines were filed under a module `LocaleData`.
- `kb::build` writes a Delphi 2010+ relocation as IDR does: the relative
  kind `5` as `J` and every other kind as its digit (`'0' + kind`). It wrote
  the kind's byte, which no published file holds. Built from the Seattle
  units, `System` and `SysInit` now carry the fixup letters of the
  published `syskb2014` on every routine whose code is the same (973 Win32,
  981 Win64), save the block-end marker of the two units' initialization.
- `TypeHeader::from_va` no longer returns a header of kind
  `TypeKind::Unknown`, which its own documentation says is not a valid type
  info. When a `jmp [reg*4 + table]` is followed by its jump table, the
  table's address is the operand's address plus 4, so the self-cell pass of
  `types()` took the operand for a type record's self-referencing cell and the
  table's first entry (a code address, whose low byte is no Kind) for the
  record. Light Alloy (Delphi 7) had two such records (`0x4201d4`,
  `0x4afea8`), `Idr64.exe` two and the C++Builder 6 `deep.exe` one, each with
  a one-character name, and `structure_extents` stated each as an RTTI record
  inside a routine. No type of any sample is of kind `Unknown` now.
  `TypeKind::Unknown` means only a Kind byte the flavor does not assign: FPC's
  `tkHelper` and `tkFile`, which it used to absorb, are `TypeKind::Helper` and
  `TypeKind::File`, decoded to their header (`TypeDetail::Other`) as the
  other kinds without a detail decoder are.

## 0.4.0

Capability release. undelphi now reads Delphi's compiled units (`.dcu`,
`.dcp`) as well as binaries, places a program's DCU declarations in its
binary, reads and writes IDR knowledge bases and builds them from DCUs, and
names what a binary imports from runtime packages. Delphi 2010+ metadata
(attributes, record methods, indexed properties) and Delphi 13 are covered,
and `structure_extents` tells a disassembler where the metadata lies in the
code section. A few API changes break compatibility; see **Changed**.

### Added

- **Compiled units.** The `dcu` module decodes Delphi compiled units
  (`.dcu`, `dcu::Unit`) and compiled packages (`.dcp`, `dcu::package`) from
  Delphi 2 to 13: every declaration of the interface and implementation,
  every type with its full layout (records without RTTI, private class
  fields, enumeration names, procedural types), routines with parameters,
  results and calling conventions, uses lists with imported symbols, and the
  type and address tables that resolve every cross-reference.
  `Unit::instance_size` gives a class's VMT instance size (the declared size
  plus the Delphi 2009+ monitor slot). The records added after 10.3 are read
  too: `Modifier::Documentation` (XML documentation comments),
  `Modifier::DeclaredType` with `TypeSpelling` (the type as the source spells
  it), `spellings` on `Modifier::TemplateParams` and
  `TypeKind::TemplateCall`, the C++ header records (`Modifier::CppFlags`,
  `Modifier::CppNames`) and `Unit::image_base`. Verified on every unit of
  C++Builder 6, RAD Studio 10 Seattle and RAD Studio 13 (Win32, Win64,
  Win64x), and fact for fact against DCU32INT. `dcu::library::Library`
  indexes a compiler's unit directories and resolves types across units.
  Examples `dcu_check`, `dcu_dump`, `dcu_facts`.
- **DCU code.** `dcu::Unit::block` keeps a unit's code and data block,
  `Unit::memory` assigns each routine, typed constant and RTTI record its
  part of it, and `Unit::fixup_size` gives each relocation's width, so a DCU
  routine can be matched against a binary byte for byte.
- **Export names.** `dcu::mangle::Mangler` generates the mangled export name
  of a DCU routine - Borland on Win32 (pre-2009 and modern dialects),
  Itanium on Win64 (`dcu::itanium`) - so each export of a runtime package
  binds to its declaration, overloads and generic instances included: 98 -
  99 % of Seattle's and Delphi 6's exports, 99.6 % of Delphi 13's Win32
  exports with a generated routine path and 98.3 % of its Win64 exports.
  Long Borland names are shortened as the compiler does (MD5, base64).
  Example `mangle_check`.
- **Locating DCU declarations in a binary.** `DelphiBinary::locate` (and
  `dcu::locate::Locator` underneath) places a program's routines, typed
  constants, RTTI records, variables and thread variables in its binary with
  the compiler's units: each routine is verified byte for byte, and its
  relocations (`dcu::locate::Encoding`) lead to the next. It names the unit
  initialization table's slots (`dcu::locate::InitOwner`: a unit or a class
  constructor pair) and the thread variables with their TLS offsets
  (`Location::thread_variables`). On the fixture programs about 4 300
  (Delphi 13) and 1 200 (Delphi 6) placements per program check against the
  linker maps, and none is wrong.
- **IDR knowledge bases.** The `kb` module reads IDR's `kb*.bin` /
  `syskb*.bin`: `KnowledgeBase::parse` reads one without copying - units,
  constants, types (fields, properties, methods), variables, resource
  strings and routines (prototype, code, relocation mask, named fixups) -
  with name and unit lookups through the file's own indexes.
  `Contents::to_bytes` writes the format byte for byte as IDR did (the
  lookup orders sorted as Delphi's `TList.Sort` sorts); every published file
  reads and writes back unchanged. `Dump::matches` tests code against a
  record ignoring relocated bytes. `kb::build` makes the records from a
  release's DCUs the way IDR's maker did - DCU32INT's type notation, the
  maker's value format and its naming of compiler-generated declarations;
  built from the Delphi 6 library they match kb6's wherever the units are
  the same. Examples `kb_dump`, `kb_build`, `kb_diff`.
- **Runtime-package imports and exports.** The `imports` module and
  `DelphiBinary::{imports, import_at, required_packages, external_parent,
  field_class_import, exports}` name the types, classes and routines a
  binary imports from runtime packages (or a package exports) by demangling
  Borland and Itanium package symbols (`@$xp$14System@Integer`,
  `@Classes@TPersistent@`, `_ZTVN6System7Classes11TPersistentE`), delay-loaded
  imports included (`Import::is_delayed`). `DetectionSource::PackageImports`
  identifies the compiler from an `rtlNN.bpl` / `vclNN.bpl` import and
  `DetectionSource::CppRuntime` from the C++ runtime's markers.
- **Delphi 2010+ binary metadata.** `DelphiBinary::decode_attribute`
  resolves an attribute's type and constructor and decodes its arguments;
  attributes are exposed on fields (`ExtendedField::attributes`), properties
  (`ExtendedProperty::attributes`), methods and parameters
  (`MethodSignature::attributes`, `MethodParam::attributes`), records and
  record fields. Records carry their methods, constructors and operators with
  signatures (`RecordInfo::methods`, `rtti::RecordMethod`) and a managed
  record's operator table (`rtti::RecordOperators`); `tkMRecord` decodes as a
  record. `DelphiBinary::indexed_properties` reads indexed (array) properties
  and `DelphiBinary::extended_methods` the extended-method entries, whose
  flags decode through `signatures::ExtendedMethodInfo`. Delphi 13 is
  detected by name (`Delphi 13 Florence`, `rtl370.bpl`).
- **Runtime tables.** `DelphiBinary::unit_init_table` decodes the unit
  initialization table (`unitinit`): each unit's initialization and
  finalization in startup order, Delphi 2010+ class constructor pairs, and
  the 2010+ type table. `DelphiBinary::resource_strings` joins the string
  table with each `resourcestring` record (`resstrings`), and
  `DelphiBinary::interface_rtti` decodes the method table of an interface
  compiled with method info (`intfrtti`).
- **Published-field and automation tables.** `DelphiBinary::field_class` /
  `fields::Field::class_table` resolve legacy published fields through the
  field class table, so `field_type` and `field_type_detail` type them (0 ->
  100 % of published fields on the Delphi 7 and Delphi 12 samples).
  `DelphiBinary::auto_entries` / `vmttables::AutoEntry` decode the
  `vmtAutoTable` of `automated` sections.
- **Structure extents.** `DelphiBinary::structure_extents` returns where each
  structure the crate reads lies in the image (`extents::StructureExtent`,
  `extents::StructureKind`): VMT headers, virtual-method arrays, class names,
  the published method, field, field class, dynamic, interface and
  automation tables, interface vtables and RTTI records, each measured by the
  walk that decodes it, plus the string constants (`StringLiteral`),
  `resourcestring` records, the unit initialization table and GUID constants
  Delphi places in the code section - so a disassembler can keep them out of
  the code it decodes.
- **Own compiled fixtures.** C++Builder 6 / Delphi 6 programs, units and
  packages (`tests/fixtures/bcb6/`, samples in `tests/samples/bcb6/`) and
  Delphi 13 Win32 / Win64 builds of them plus `modernbin.exe` and `modern`
  units for the 2010+ metadata (`tests/fixtures/d13/`, `tests/samples/d13/`),
  with build scripts, linker maps and toolchain provenance;
  `tests/programs.rs`, `tests/modern_rtti.rs`, `tests/dcu.rs`,
  `tests/locate.rs`, `tests/extents.rs` and `tests/kb.rs` assert against them.

### Changed

- `signatures::MethodParam`, `rtti::SignatureParam` and `rtti::RecordField`
  are no longer `Copy` (they carry attributes).
- `ExtendedProperty::attributes` and `ExtendedField::attributes` are decoded
  `Vec<AttributeEntry>` fields, like those of records, methods and
  parameters, instead of methods taking a pointer size;
  `ExtendedProperty::attributes_raw` is gone and `ExtendedField` is no longer
  `Copy`. Every `TAttrData` block is read by one strict reader
  (`extrtti::read_attr_data`).
- Removed `AttributeEntry::iter_at` and `iter_class_attributes` (a greedy
  scan nothing used) and `Visibility::Unknown` (never produced).
- Indexed-property accessors resolve by table position:
  `DelphiExtMethod::index` is an entry's position in the extended-method
  table, and `ArrayProperty::getter` / `setter` look `read_index` /
  `write_index` up by it, so an unreadable entry no longer shifts the ones
  after it.
- `AccessTarget::UnresolvedSlot` carries the VMT byte offset unchanged as a
  `u64` (it was truncated to `u16`).
- Identifier checks accept UTF-8 letters, as Delphi 2009+ writes
  identifiers; a non-ASCII name no longer ends a class, field, method,
  property or record walk.
- The extended-RTTI accessors of `DelphiBinary` (`extended_properties`,
  `extended_fields`, `extended_methods`, `indexed_properties`,
  `class_attributes`, and the extended source of `method_signatures`) return
  nothing when the build string or RTL package dates the compiler before
  Delphi 2010, which the Delphi 2009 VMT header alone cannot tell apart.
- `DelphiBinary::string_layout` and `import_at` are cached and indexed, and
  `ClassSet::find_by_type_info` finds a class by its type info:
  `decode_attribute` is about six times faster on large binaries. Naming the
  unit initialization table (`DelphiBinary::locate`) checks variable layout
  by binary search, and thread-variable offsets past the TLS template are
  rejected as misreads.
- `AccessKind::Virtual` values are VMT byte offsets (as stored on disk), and
  `Access::resolve` resolves them correctly; it previously treated them as
  slot indexes.
- A Delphi getter / setter of 0 decodes as `AccessKind::None` and a
  `stored True` / `False` constant as `AccessKind::Const` (1 / 0), not
  `Static`; `code_entrypoints` no longer emits entrypoints at VA 0 / 1.
- C++Builder output is reported as `Compiler::CppBuilder` (detected from
  `fb:C++HOOK` / `___CPPdebugHook`).
- `EnumInfo::unit_name` is `None` when no plausible unit name follows the
  values (C++Builder `bool`); nameless `tkMethod` records (C++Builder
  `__closure` types) are accepted.
- The unit init table's documentation states that from Delphi 2010 its
  entries include class constructor / destructor pairs, not only units.

### Fixed

- Win64 `TObject` header virtuals were named three slots off: Win64 puts the
  `CPP_ABI_1` .. `CPP_ABI_3` virtuals in front of `Destroy`, so the slot
  named `Destroy` was a C++ ABI stub (every Win64 binary since XE2). Delphi
  2009+'s `ToString`, `GetHashCode` and `Equals` slots are now named too.
- A class without virtual methods of its own no longer reports the data
  after its VMT as a virtual method.
- `tkProcedure` signatures with two or more parameters decode (the
  per-parameter attribute block was not skipped).
- A class with attributes but no extended properties keeps its attributes.
- Win64 automation tables (`automated` sections) decode.
- A typeinfo import in the C++ global namespace (`@$xp$7Boolean`) demangles,
  with an empty unit.
- Form `Extended` values (`DfmValue::as_f64`) round to the nearest `f64`
  instead of scaling through `powi`, so subnormal results and values near
  `f64`'s limits convert exactly.
- `Idr64.exe` is identified as C++Builder 6 (it is a C++Builder project), not
  Delphi.

## 0.3.3

Maintenance release. No API or behavior changes.

### Fixed

- **Clippy on current stable.** UTF-16 decoding in `DfmValue::as_text`,
  `DfmValue::as_text_strict`, the DFM renderer, and PE resource-name reading
  now uses `as_chunks::<2>()` instead of `chunks_exact(2)` plus a fallible
  array conversion, satisfying the new `chunks_exact_to_as_chunks` lint.
  `as_chunks` is stable since Rust 1.88, so the MSRV is unchanged.

### Changed

- Replaced em-dashes with plain hyphens throughout rustdoc, comments, the
  README, `SAMPLES.md`, and this changelog, including the crate description.
  Two lines that began with a dash were reworded so they no longer render as
  stray list items.

### Added

- Source and build notes for two controlled test fixtures:
  `tests/fixtures/known-rtti/` (the `known.pas` golden source behind
  `tests/known_rtti.rs`, with the FPC 3.2.2 / 3.0.4 build matrix and the
  extraction differences it surfaced) and `tests/fixtures/fpc-methodrtti/`
  (documents that FPC 3.2.2 does not emit `TVmtMethodExTable`).

### Dependencies

- Refreshed `Cargo.lock` (`log` 0.4.33 -> 0.4.34, `scroll_derive` 0.13.1 ->
  0.13.2, which adds `syn` 3.0.5). Direct dependencies (`goblin`, `scroll`,
  `tracing`) were already current.

## 0.3.2

Security and ownership release.

### Fixed

- **Unbounded recursion in the DFM reader.** The form stream is untrusted input
  carved out of the analyzed binary, and neither the `read_object`/`read_children`
  cycle nor `read_value`'s list arm bounded recursion - one nesting byte bought one
  stack frame, so a crafted DFM could exhaust the stack and abort the process.
  Nesting is now capped at `MAX_DFM_DEPTH` (64), far above any legitimate form.

### Changed

- Recorded ATRAPS LLC as copyright holder and added a `NOTICE` file.
- Dropped the deprecated `authors` field and repointed `repository` at the organisation.
- Refreshed dependencies (`cargo update`).
- Publishing now uses crates.io trusted publishing instead of a stored registry token.

## 0.3.1

Maintenance release. Fixes the crates.io repository link and refreshes the
dependency lockfile. No API or behavior changes.

### Fixed

- **crates.io `repository` link** pointed at `BinFlip/delphi` instead of
  `BinFlip/undelphi`, so the "Repository" link redirected to the wrong project
  ([#1](https://github.com/ATRAPSLLC/undelphi/issues/1)).

### Dependencies

- Refreshed `Cargo.lock` to the latest semver-compatible transitive versions
  (`log` 0.4.29 → 0.4.33, `quote` 1.0.45 → 1.0.46, `syn` 2.0.117 → 2.0.118).
  Direct dependencies (`goblin`, `scroll`, `tracing`) were already current.

## 0.3.0

Capability release. The headline is **`DelphiBinary::types()`**, a complete
RTTI type dictionary, plus **method-signature decoding** and a wave of new
per-Kind RTTI detail. Two real decoding bugs affecting FPC on x86-64 ELF /
Mach-O and FPC event-type signatures are fixed. Container coverage gains its
first Linux ELF and macOS x86-64 Mach-O test binaries.

### Added

- **`DelphiBinary::types()`** - enumerate every RTTI type record in the
  binary, not just the ones hanging off a class. It runs a transitive
  closure over `PPTypeInfo` references (class parents, property / field
  types, method-signature param/return types, extended-property types, enum
  base types, set/array/dynarray element types, pointer targets, record
  fields, interface parents) and then, on Delphi, a **self-cell pass** (every
  Delphi `PTypeInfo` is preceded by a self-referencing `PPTypeInfo` cell -
  the `vmtSelfPtr` analogue) that recovers types referenced only from code.
  Reaches ~97 % of all `PTypeInfo` in a Delphi image (e.g. the enum
  dictionary on HeidiSQL 12 grows from ~250 to ~880). Returns
  `Vec<TypeDetail>`; bounded by `limits::MAX_RTTI_TYPES`.
- **Method signatures** - `DelphiBinary::method_signatures(class)` returns an
  era-tagged `signatures::SignatureReport` (`Decoded` / `Absent` /
  `Unsupported`) of `MethodSignature` (name, `MethodKind`, `CallConv`, ordered
  `MethodParam`s with name / resolved type / `ParamMode`, return type, code
  VA). Sources: the Delphi 2010+ extended-method section of the
  `vmtMethodTable` (the primary source) and the published-method trailer
  (`{$METHODINFO ON}`). New `signatures` module.
- **New / richer RTTI decoders** (all reachable via `types()` and
  `rtti::TypeDetail`):
  - `tkPointer` and `tkArray` now decode (`TypeDetail::Pointer` / `Array`
    with `PointerInfo` / `ArrayInfo` and their target / element types).
  - `RecordInfo::fields` - the full Delphi 2010+ record field table (every
    field's name, resolved type, offset, and visibility flags), not just
    managed fields. New `rtti::RecordField`.
  - `ProcedureInfo` now decodes its inline `TProcedureSignature` (calling
    convention, parameters, return type) instead of just the header. New
    `rtti::SignatureParam`.
  - `MethodInfo::param_type_refs` / `result_type_ref` - the modern `tkMethod`
    per-parameter `PPTypeInfo` references (cross-validated against the
    name-based parameter types).
  - `TypeDetail::referenced_pptrs()` - the onward type references a record
    points at.
- **`forms()` raw-magic fallback** - when the PE `RT_RCDATA` and FPC
  internal-resources passes find nothing, `forms()` now scans for `TPF0` /
  `TPF1` streams and re-parses each candidate, recovering forms from stripped
  or unconventionally-packaged binaries. New `dfm::scan_streams`.
- `detection::TargetArch::fpc_requires_proper_alignment()`.
- `examples/corpus_scan` - a re-runnable bulk triage / regression tool that
  reports parse outcomes, compiler / format histograms, and extraction
  anomalies across a directory tree.
- First Linux ELF (`doublecmd` gtk2 x86-64, FPC) and macOS x86-64 Mach-O
  (`doublecmd` cocoa, FPC) test binaries, with integration tests.

### Fixed

- **FPC `TTypeData` alignment on x86-64 ELF / Mach-O.** The decoder gated
  natural-alignment on "is this *not* PE?" rather than on the architecture,
  so it inserted phantom padding on x86-64 ELF / Mach-O (which FPC packs).
  Every unit name and field table on those targets decoded to garbage. Now
  keyed on `TargetArch` (`FPC_REQUIRES_PROPER_ALIGNMENT` is an
  ARM/AArch64/PPC/SPARC property). Recovers ~2 500 unit names and the field
  tables on the new ELF / Mach-O x86-64 test binaries.
- **FPC `tkMethod` parameter flags.** FPC's `TParamFlag` is a 12-element set
  (2 bytes); Delphi's is 7 (1 byte). The decoder read 1 byte for both,
  shifting every following field and garbling FPC event-type signatures
  (`TNotifyEvent` etc.). Now flavor-dependent.

### Breaking changes

- **`rtti::TypeDetail` is now `#[non_exhaustive]`** and gains `Pointer` and
  `Array` variants. Exhaustive matches must add a wildcard arm. Marking the
  enum non-exhaustive means future per-Kind decoders won't be breaking.
- **New public fields** on existing structs: `rtti::RecordInfo::fields`,
  `rtti::MethodInfo::{param_type_refs, result_type_ref}`, and several fields
  on `rtti::ProcedureInfo` (previously header-only). Struct patterns that
  destructure these without `..` need updating; field reads and the parser
  output are unaffected.

## 0.2.1

Ergonomic release driven by downstream-consumer feedback. This version keeps
the public API additive and leaves the larger `AccessTarget` narrowing for a
future breaking release.

### Added

- PE image-base and VA-to-RVA helpers: `DelphiBinary::image_base`,
  `DelphiBinary::va_to_rva`, `BinaryContext::image_base`, and
  `BinaryContext::va_to_rva`.
- Method and entrypoint RVA helpers: `DelphiBinary::method_rva`,
  `MethodEntry::method_rva`, `InterfaceMethod::method_rva`, and
  `CodeEntrypoint::rva`.
- Stable `as_str()` labels and `Display` impls for `Confidence`, `Compiler`,
  `TargetArch`, `TargetOs`, `DetectionSource`, `Edition`, and
  `EntrypointKind`.
- Batched class-member accessors: `properties_with_types`,
  `fields_with_types`, `interfaces_with_methods`, and
  `class_attributes_with_string_args`.
- Handle-based class lookup helpers: `DelphiBinary::class_by_index` and
  `DelphiBinary::parent_class`.
- DFM value helpers: `DfmValue::kind_str()` and strict text decoding via
  `DfmValue::as_text_strict()`.

## 0.2.0

Major API revision. Most existing call sites need small migrations
(see "Breaking changes"); in exchange the crate gains a single
aggregated entrypoint for disassembler-driving consumers, hides the
raw-bytes/`&str` distinction behind a consistent accessor pattern, and
exposes interface-method, class-attribute, and unit init/finalize
walkers that weren't reachable before.

### Breaking changes

- **`DelphiBinary::parse` now returns `Result<Self, ParseError>`**
  instead of `Option<Self>`. Variants: `NotRecognized` (the quiet
  no-Delphi-markers case - safe to ignore), `TruncatedContainer`
  (recognised magic but malformed headers - worth logging), and
  `UnrecognizedFormat` (no known container magic at all). `ParseError`
  implements `Display` and `std::error::Error`.
- **Name accessors return `&str` by default**, lossily decoded with a
  `"<non-ascii>"` fallback. Affected: `MethodEntry`, `Property`,
  `Field`, `DfmObject`, `DfmProperty`, `Vmt`, `TkClassInfo`,
  `TypeHeader`, `MethodParam`, `EnumInfo`, `DynArrayInfo`,
  `InterfaceTypeInfo`, `InterfaceEntry`. The raw `&[u8]` byte-fields
  are now hidden; use the `*_bytes()` companion accessors when you
  need the raw bytes. Migration: `foo.name_str()` → `foo.name()`,
  `foo.class_name_str()` → `foo.class_name()`, etc.
- **`BinaryFormat` is split by bitness.** `Pe` / `Elf` / `MachO` are
  replaced by `Pe32` / `Pe64` / `Elf32` / `Elf64` / `MachO32` /
  `MachO64`. Use the `is_pe()` / `is_elf()` / `is_macho()` /
  `is_64bit()` / `bitness()` helpers for variant-agnostic code. The
  enum is `#[non_exhaustive]`.
- **`DelphiBinary` is now `Send + Sync`.** `forms_cache` switched
  from `std::cell::OnceCell` to `std::sync::OnceLock`, which unblocks
  holding `&bin` across `.await` points in async runtimes.
- **`EmbeddedBlob` carries borrowed component / property
  references** (`component: &DfmObject`, `property: &DfmProperty`)
  instead of `String` fields. The dotted `path` remains as a
  precomputed `String`; `property_name` is now the method
  `b.property_name()`.

### Aggregated entrypoint

- **`bin.code_entrypoints()`** - single call producing every code VA
  the crate can confidently label, tagged by `EntrypointKind`
  (`PublishedMethod`, `VmtSlot`, `DynamicMessage`, `InterfaceGetter`,
  `InterfaceMethod`, `PropertyGetter`, `PropertySetter`,
  `PropertyStored`, `AttributeCtor`, `UnitInit`, `UnitFinalize`) and
  carrying a `name_hint` like `TForm1.Button1Click`. Replaces ~10
  separate walks for disassembler-driving consumers; resolves
  `AccessKind::Virtual` slot indices to code VAs internally.

### New walkers

- **`bin.interface_methods(entry)`** - walks an interface's vtable
  pointer-by-pointer, terminating at the first slot that doesn't
  contain a plausible code VA. Slot count + code VAs are recovered
  on every Delphi and FPC binary, including stripped builds. Method
  *names* are populated when the `tkInterface` RTTI carries
  per-method records - recovered via a binary-wide GUID-keyed
  RTTI index built lazily on first call. The new
  [`rtti::IntfMethodTable`] / [`rtti::IntfMethodEntry`] types expose
  the parsed table directly when needed.
- **`bin.class_attributes(class)`** - walks past the classic
  `TPropData` and the extended-property block to find the modern
  `AttrData` trailer, then decodes its packed `[attribute]`
  entries. Layout-driven; returns whatever the trailer actually
  contains regardless of compiler family.
- **`bin.unit_init_procs()`** - locates the FPC `INITFINAL` table via
  symbol lookup (`INITFINAL` / `_INITFINAL` / `FPC_INITFINAL` across
  ELF symtab, Mach-O `LC_SYMTAB`, and PE exports), with a heuristic
  shape scan over data sections as a fallback for stripped builds.
  Returns `Vec<UnitInitProc { unit_name, init_va, finalize_va }>`.
  Delphi-compiled binaries return empty by design - Delphi inlines
  unit init into the entry-point startup sequence; consumers wanting
  the unit list use [`DelphiBinary::package_info`] instead.

### Ergonomics

- `Access::resolve(virtual_methods)` returning a new `AccessTarget`
  enum (`CodeVa` / `FieldOffset` / `Constant` / `UnresolvedSlot` /
  `Missing`). Hides the VMT slot-index → VA lookup.
- `Class::parent(&self, set)` - resolves `parent_index` against a
  `ClassSet`.
- `ClassSet::iter_with_parents()` - yields
  `(class, Option<&Class>)` pairs.
- `DfmValue::as_text()` - unifies `String` and `Utf16` variants
  behind a single `Cow<'_, str>` accessor.
- `DfmValue::as_f64()` - decodes the 10-byte Intel 80-bit extended
  (lossy).
- `DfmObject::walk_with_path()` - depth-first walker yielding
  `(dotted_path, &DfmObject)`.
- `FormFlavor` enum (`Tpf0` / `Tpf1`) on `DfmObject`.
- `Deref<Target = Property>` on `ExtendedProperty`. `ep.name()` now
  works without going through `ep.info.…`.
- `Display` impls on `Guid` (no `String` allocation, in hot paths)
  and `BlobKind`. `Guid` also gains `Hash`, `PartialOrd`, `Ord`.
- `bin.target_os()` / `bin.target_arch()` - fall back to
  container-level inference (PE → Windows, Mach-O → Darwin, ELF →
  Linux; arch from PE `Machine` / ELF `e_machine` / Mach-O
  `cputype`) when the compiler build-string is absent.
- `bin.ctx().container_parsed()` - whether goblin walked the
  container cleanly. Drives `ParseError::TruncatedContainer`.
- `bin.ctx().is_code_va(va)` - whether `va` lies inside the binary's
  primary code section. Used by `interface_methods` and the
  `unit_init_procs` heuristic.

### Documentation

- Top-level module docs gained a code-vs-data VA classification
  table and the "subtract image base for RVA" note.

### Optional features

- **`tracing`** (off by default). Sub-parsers (`iter_methods`,
  `iter_properties`, `iter_fields`, `iter_interfaces`, `dfm::parse`)
  emit `warn!` events when they bail out on truncated or malformed
  input. Lets consumers distinguish "no methods declared" from
  "method table corrupt" without changing return types.

## 0.1.0

Initial release.
