# C++Builder 6 fixtures

Controlled sources compiled with the Borland C++Builder 6 command-line tools
(Delphi 6 `dcc32` 14.0, Borland C++ 5.6 `bcc32` / `ilink32`), so that
`tests/programs.rs` can assert exact extraction - every type, member, form
value and code address - against binaries whose contents we fully control.
This is the first Borland-compiled ground truth in the crate; the
`known-rtti` fixtures cover FPC. The Pascal sources are also built by
Delphi 13 for Win32 and Win64 (`tests/fixtures/d13/`), and
`tests/programs.rs` runs each assertion over every build of the program.

The compiled samples and their linker maps are committed in
`tests/samples/bcb6/`; `TOOLCHAIN.txt` there records the tool banners and
SHA-256 hashes that produced them. The toolchain itself is proprietary and
never committed.

## Building

```sh
tests/fixtures/bcb6/build.sh [path/to/CBuilder6]
```

The install is the argument or `UNDELPHI_BCB6`. The tests that need the
whole Delphi 6 library read the same variable, and `UNDELPHI_BCB6_PACKAGES`
for the folder holding its runtime packages (`rtl60.bpl`, ...); without
them they skip.

## Matrix

| Sample | Source | Tool | What it pins |
|--------|--------|------|--------------|
| `known.exe` | `known.dpr` | dcc32 | Port of the FPC `known.pas`: classes, properties, enum, set, record, dynamic array, event, interface |
| `known_rtl.exe` | `known.dpr` | dcc32 `-LUrtl` | Same, linked against `rtl60.bpl`: external parent, imported typeinfo |
| `deep.exe` | `deep.dpr` | dcc32 | Invokable-interface method info, dispinterface, direct / stdcall / virtual interface thunks, `implements` (field and getter), enum subrange, boolean-likes, `string[N]`, `Int64` range, static array, nested and dynamic-array records |
| `tables.exe` | `tables.dpr` | dcc32 | Every VMT table (interface, auto, init, field + class table, method, dynamic), every publishable property kind and specifier, string literals, resourcestrings, a TP `object` |
| `gtforms.exe` | `gtforms.dpr` + `gt*.pas/dfm` | dcc32, brcc32 | Every TPF0 value type, inline frame, visual form inheritance, child position, VERSIONINFO |
| `gtframe.dcu`, `gtmain.dcu`, `gtchild.dcu` | `gt*.pas` | dcc32 (with `gtforms.exe`) | The program's own units, which `tests/locate.rs` places in `gtforms.exe` |
| `gtpkg.bpl`, `gtpkg.dcp`, `gtpkg.dcu` | `gtpkg.dpk`, `gtshared.pas` | dcc32 | A runtime package, its compile-time counterpart and the package's own unit |
| `dcutypes.dcu` | `dcutypes.pas` | dcc32 | A unit compiled on its own (`tests/dcu.rs`): records without RTTI (plain, packed, variant, nested), every simple type kind, aliases and a type copy, procedural types, a class with private members, an interface, overloads, default parameters, every calling convention, an external routine, constants, resourcestrings, a threadvar, a nested routine; the `Size*` constants state the compiler's own sizes |
| `dcutypespkg.bpl`, `.dcp`, `.dcu` | `dcutypespkg.dpk` | dcc32 | `dcutypes` in a runtime package: its exports are the mangled names of every routine of the unit, the answer key for `dcu::mangle` |
| `usepkg.exe` | `usepkg.dpr` | dcc32 `-LUrtl;gtpkg` | A class whose parent lives in `gtpkg.bpl` |
| `cppknown.exe` | `cppknown.cpp` | bcc32 `-tWV -tWC` | VCL classes declared in C++ (`__published`, `__property`, `__closure`), plus a plain C++ class |
| `cppplain.exe` | `cppplain.cpp` | bcc32 `-tWC` | C++Builder output without the VCL |

Each `.exe` / `.bpl` has a `.map` (a unit compiled on its own has none): dcc32 `-GD` (detailed, segment-relative
offsets, CODE at image base + `0x1000`) or ilink32 `-m -s` (absolute segment
base in the header).
