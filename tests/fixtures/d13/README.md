# Delphi 13 fixtures

Controlled sources compiled with the RAD Studio 13 Florence command-line
compilers (`dcc32` and `dcc64` 37.0) for Win32 and Win64. Four test files
read them, each against sources whose every fact we know:

- `tests/dcu.rs`: the DCU records added after 10.3 Rio - which DCU32INT,
  the reference parser, predates - (XML documentation comments, type
  spellings, generic instances, the `PKX1` Win64 package container with its
  image base) and the export names of every routine.
- `tests/programs.rs`: the C++Builder 6 fixture programs
  (`tests/fixtures/bcb6/src/`) rebuilt here, so the same assertions run over
  the Delphi 6, Delphi 13 Win32 and Delphi 13 Win64 builds.
- `tests/locate.rs`: the programs' DCU declarations, init slots and thread
  variables located in them, against their maps.
- `tests/modern_rtti.rs`: `modernbin.exe`, a program built for the Delphi
  2010+ metadata a binary carries - extended RTTI of every visibility,
  attributes with arguments, indexed properties, records with methods and
  operators, a managed record, generic instances, an invokable interface,
  an anonymous-method type, a class constructor, string literals, a
  resourcestring and a delay-loaded import.

The tests only read the committed samples; nothing is compiled when they
run. The compiled samples are committed in `tests/samples/d13/` (`win32/`,
`win64/`); `TOOLCHAIN.txt` there records the compiler banners and SHA-256
hashes that produced them. The toolchain itself is proprietary and never
committed.

## Building

```sh
tests/fixtures/d13/build.sh [path/to/RAD-Studio-37.0]
```

The install is the argument or `UNDELPHI_D13`, which the tests needing the
Delphi 13 library read too (they skip without it): an install's
`bin/` and `lib/` directories are enough. The script reuses the C++Builder 6
fixtures' Debian + Wine image (`tests/fixtures/bcb6/Dockerfile`, the
toolchain bind-mounted read-only), runs `compile.sh` inside it on a scratch
copy of `src/` plus the C++Builder 6 fixtures' Pascal sources, and replaces
`tests/samples/d13/` only when the whole matrix built. Rebuilds are not
byte-identical (timestamps change), so rebuild only when a source changes.

Each program, package and unit build keeps its linker map (`-GD`), trimmed
to the segment tables and the publics by name - minus Win64's `$pdata`
unwind entries - which is all the tests read; the line tables would triple
the committed size.

## Matrix

Every sample exists for Win32 (`win32/`) and Win64 (`win64/`).

| Sample | Source | What it pins |
|--------|--------|--------------|
| `modern.dcu` | `modern.pas` | Documentation comments (`drConstAddInfo` `0x08`), type spellings (`0x17`: `TCount` for Integer, `TBytes` by name), `TBox<TCount>` with its argument spelling, `TList<T>` instances and their nested types, a generic method, an anonymous method, `IUnknown` for `IInterface` |
| `modernpkg.bpl` / `.dcp` / `.map` | `modernpkg.dpk` | Every routine's export name: Borland on Win32 (one shortened to 253 characters with its MD5 suffix), Itanium on Win64; the Win64 DCP is `PKX1` and its unit carries the image base 0x400000 |
| `dcutypes.dcu`, `dcutypespkg.bpl` / `.dcp` / `.map` | `../bcb6/src/dcutypes.pas`, `dcutypespkg.dpk` | The Delphi 6 fixture unit rebuilt: the same layouts, and its export names under Delphi 13's rules (an open array's `High` is `long long` on Win64) |
| `known.exe`, `known_rtl.exe`, `tables.exe`, `deep.exe`, `gtforms.exe`, `gtpkg.bpl` / `.dcp`, `usepkg.exe` (with maps) | `../bcb6/src/` | The C++Builder 6 fixture programs rebuilt (see `../bcb6/README.md`): `known_rtl` and `usepkg` against `rtl370.bpl`; what differs from Delphi 6 is the RTL (Unicode `string`, a larger `TComponent`), the VMT header (22 slots, Win64 25 with the C++ ABI slots), and the unit init table (class constructor pairs) |
| `modernrt.dcu`, `gtframe.dcu`, `gtmain.dcu`, `gtchild.dcu` | `modernrt.pas`, `../bcb6/src/gt*.pas` | The programs' own units, which `tests/locate.rs` places in `modernbin.exe` and `gtforms.exe` (`modernrt.dcu` alone, without the toolchain: its routines, `TWidget`'s class constructor slot and its own) |
| `modernbin.exe` / `.map` | `modernbin.dpr`, `modernrt.pas` | Delphi 2010+ binary metadata: `{$RTTI EXPLICIT}` fields, methods and properties of every visibility, attributes with arguments (class, field, method, property, record, record field), an indexed property, record methods / constructor / operator, a `tkMRecord` with `Initialize` / `Finalize`, `TBox<Integer>` / `TBox<string>`, an invokable interface, `reference to procedure`, a scoped enumeration, a class constructor in the init table, string literals, a resourcestring, a threadvar, a `delayed` import |
