# Sample Corpus

Research-only corpus for exercising the Delphi parser. Samples live in
`tests/samples/`. Each file is documented with its provenance so it can be
re-downloaded (or removed) cleanly.

**Policy.** These binaries are redistributed-in-place for research; we do not
republish them. See the provenance column for the original upstream URL and
the SHA-256 check column for integrity.

## Corpus summary

| # | Compiler | Target | Arch | Format | Local path | TPF0 forms |
|---|----------|--------|------|--------|------------|------------|
| 1 | C++Builder 6 (Borland C++ 5.6, Delphi 6 RTL) | Windows | x86 | PE32 GUI | `tests/samples/idr-builds/Idr64.exe` | 18 |
| 2 | Delphi 12.3 Athens | Windows | x86_64 | PE32+ GUI | `tests/samples/heidisql/portable_x64/heidisql.exe` | 42 |
| 3 | FPC 3.2.2 / Lazarus | macOS | aarch64 | Mach-O | `tests/samples/heidisql/macos/heidisql.app/Contents/MacOS/heidisql` | scan TBD |
| 4 | FPC 3.2.2 / Lazarus | Windows | x86 | PE32 GUI | `tests/samples/doublecmd/win32/doublecmd/doublecmd.exe` | 114 |
| 5 | FPC 3.0.4 / Lazarus | Windows | x86_64 | PE32+ GUI | `tests/samples/cheatengine/bin/cheatengine-x86_64.exe` | 159 |
| 6 | FPC 3.2.2 / Lazarus | macOS | x86_64 | Mach-O | `tests/samples/doublecmd/macos_x86_64/doublecmd.macho` | 117 |
| 12 | FPC 3.2.2 / Lazarus | Linux | x86_64 | ELF | `tests/samples/doublecmd/linux_x86_64/doublecmd.elf` | 120 |
| 7 | Delphi (32-bit) | Windows | x86 | PE32 DLL | `tests/samples/others/IDR-cpp-dis.dll` | 0 |
| 8 | Delphi 7 | Windows | x86 | PE32 GUI | `tests/samples/lightalloy/LA.exe` | 33 (of 34 magic hits; 1 non-RCDATA coincidence) |
| 9 | Delphi XE5, UPX-packed | Windows | x86 | PE32 GUI | `tests/samples/heidisql/portable_x86_xe5/heidisql.exe` | - (rejected, see notes) |
| 10 | Delphi XE5 (UPX-unpacked copy of #9) | Windows | x86 | PE32 GUI | `tests/samples/heidisql/portable_x86_xe5/heidisql.unpacked.exe` | 33 (of 36 magic hits; 3 non-RCDATA coincidences) |
| 11 | Delphi 12.2 Athens | Windows | x86 | PE32 BPL | `tests/samples/delphilint/DelphiLintClient-1.3.0-Athens.bpl` | 6 (of 6 magic hits) |

Compiler fingerprints were verified by `strings` grep:

- `heidisql.exe` (x64) - `Embarcadero Delphi for Win64 compiler version 36.0 (29.0.55362.2017)`
- `doublecmd.exe` - `FPC 3.2.2 [2021/05/15] for i386 - Win32`
- `cheatengine-x86_64.exe` - `FPC 3.0.4 [2019/10/27] for x86_64 - Win64`
- `heidisql (mac)` - `FPC 3.2.2 [2021/05/16] for aarch64 - Darwin`
- `doublecmd.macho` (macOS x86_64) - `FPC 3.2.2 [2021/05/16] for x86_64 - Darwin`
- `doublecmd.elf` (Linux x86_64) - `FPC 3.2.2 [2024/05/03] for x86_64 - Linux`
- `DelphiLintClient-1.3.0-Athens.bpl` - `Embarcadero Delphi for Win32 compiler version 36.0 (29.0.52161.7750)`
- `LA.exe` - no explicit compiler version string (pre-XE Delphi did not emit one); Borland Delphi RTL registry markers present (`SOFTWARE\Borland\Delphi\RTL`, `Delphi Component`, `Delphi Picture`), and the upstream README requires Borland Delphi 7 to build.
- `heidisql.unpacked.exe` (9.5) - no explicit compiler version string, but namespaced unit names (`System.Masks`, `Vcl.ActnList`, `Vcl.ComCtrls`, …) confirm Delphi XE2+; maintainer forum post identifies the XE5 migration as the build target for this release.

## Own compiled fixtures

`tests/samples/bcb6/` holds binaries we compiled ourselves from the sources in
`tests/fixtures/bcb6/` with the C++Builder 6 command-line tools (Delphi 6
`dcc32` 14.0 and Borland C++ 5.6), each with its linker map. Unlike the corpus
above they are committed, their ground truth is exact, and `tests/programs.rs`
requires them. See `tests/fixtures/bcb6/README.md` for the matrix and how to
rebuild. `tests/dcu.rs` reads the compiled units and packages.

| Sample | Compiler | Format |
|--------|----------|--------|
| `known.exe`, `tables.exe`, `deep.exe`, `gtforms.exe` | Delphi 6 (static RTL) | PE32 |
| `gtframe.dcu`, `gtmain.dcu`, `gtchild.dcu` | Delphi 6 | DCU: `gtforms.exe`'s own units (`tests/locate.rs`) |
| `known_rtl.exe`, `usepkg.exe` | Delphi 6 (runtime packages) | PE32 |
| `gtpkg.bpl` / `gtpkg.dcp` / `gtpkg.dcu` | Delphi 6 | PE32 BPL / DCP / DCU |
| `dcutypes.dcu` | Delphi 6 | DCU (a unit compiled on its own) |
| `dcutypespkg.bpl` / `.dcp` / `.dcu` | Delphi 6 | PE32 BPL around `dcutypes` (export-name answer key) |
| `cppknown.exe` | C++Builder 6 (VCL) | PE32 |
| `cppplain.exe` | C++Builder 6 (no VCL) | PE32 |

`tests/samples/d13/` holds units, packages and programs compiled by Delphi
13 (`dcc32` / `dcc64` 37.0) from `tests/fixtures/d13/src` and the Pascal
sources above, for Win32 and Win64 (`win32/`, `win64/`), each program and
package with its (trimmed) linker map; `tests/dcu.rs`, `tests/programs.rs`
and `tests/modern_rtti.rs` require them. See
`tests/fixtures/d13/README.md`.

| Sample | Compiler | Format |
|--------|----------|--------|
| `modern.dcu` | Delphi 13 (Win32, Win64) | DCU: documentation comments, type spellings, generics, anonymous methods |
| `modernbin.exe` | Delphi 13 (Win32, Win64) | PE32 / PE32+: extended RTTI, attributes, indexed properties, record methods, a managed record, a delayed import (`tests/modern_rtti.rs`) |
| `modernrt.dcu`, `gtframe.dcu`, `gtmain.dcu`, `gtchild.dcu` | Delphi 13 (Win32, Win64) | DCU: `modernbin.exe`'s and `gtforms.exe`'s own units (`tests/locate.rs`) |
| `known.exe`, `known_rtl.exe`, `tables.exe`, `deep.exe`, `gtforms.exe`, `gtpkg.bpl`, `usepkg.exe` | Delphi 13 (Win32, Win64) | The C++Builder 6 fixture programs rebuilt (`tests/programs.rs` runs over both compilers' builds) |
| `modernpkg.bpl` / `.dcp` | Delphi 13 (Win32, Win64) | BPL around `modern` (export-name answer key); the Win64 DCP is `PKX1` |
| `dcutypes.dcu`, `dcutypespkg.bpl` / `.dcp` | Delphi 13 (Win32, Win64) | The Delphi 6 fixture unit rebuilt, a cross-version control |

## Provenance (how to re-download)

### 1 & 7 - IDR (Interactive Delphi Reconstructor) builds

`Idr64.exe` and `IDR-cpp-dis.dll` are ready-built binaries shipped inside the
IDR project repositories. `Idr64.exe` is C++Builder 6 output - the project is
IDR64's `Idr64.bpr` with `.cpp` sources, and the binary carries `Borland C++ - Copyright 2002 Borland
Corporation` and the C++ runtime's `fb:C++HOOK` marker next to the Delphi 6
RTL's `SOFTWARE\Borland\Delphi\RTL`. Being a VCL application, it is
conveniently its own test subject. `IDR-cpp-dis.dll` carries none of those
markers; its toolchain is unverified.

```sh
git clone --depth 1 https://github.com/crypto2011/IDR64
git clone --depth 1 https://github.com/crypto2011/IDR
cp IDR64/Idr64.exe tests/samples/idr-builds/Idr64.exe
cp IDR/dis.dll tests/samples/others/IDR-cpp-dis.dll
```

Source: [github.com/crypto2011/IDR64](https://github.com/crypto2011/IDR64),
[github.com/crypto2011/IDR](https://github.com/crypto2011/IDR). MIT licensed.

### 2 - HeidiSQL 12.17 (Windows x64) - Delphi 12.3 Athens

```sh
curl -L -o tests/samples/heidisql/HeidiSQL_12.17_64_Portable.zip \
  "https://github.com/HeidiSQL/HeidiSQL/releases/download/12.17/HeidiSQL_12.17_64_Portable.zip"
unzip -q tests/samples/heidisql/HeidiSQL_12.17_64_Portable.zip \
  -d tests/samples/heidisql/portable_x64/
```

Source: [github.com/HeidiSQL/HeidiSQL](https://github.com/HeidiSQL/HeidiSQL),
GPL-2.0. The Windows version is "built with Delphi 12.3".

### 3 - HeidiSQL 12.17 (macOS ARM64) - FPC/Lazarus

```sh
curl -L -o tests/samples/heidisql/HeidiSQL_12.17_macos.zip \
  "https://github.com/HeidiSQL/HeidiSQL/releases/download/12.17/heidisql_12.17_macos_app.zip"
unzip -q tests/samples/heidisql/HeidiSQL_12.17_macos.zip \
  -d tests/samples/heidisql/macos/
```

HeidiSQL's website notes: "Lazarus v4.4 is used for compiling the Linux and
macOS releases" - though our downloaded binary carries FPC 3.2.2.

### 4 & 6 - Double Commander 1.1.32 - FPC/Lazarus

```sh
curl -L -o tests/samples/doublecmd/doublecmd-1.1.32.i386-win32.zip \
  "https://github.com/doublecmd/doublecmd/releases/download/v1.1.32/doublecmd-1.1.32.i386-win32.zip"
curl -L -o tests/samples/doublecmd/doublecmd-1.1.32.cocoa.x86_64.dmg \
  "https://github.com/doublecmd/doublecmd/releases/download/v1.1.32/doublecmd-1.1.32.cocoa.x86_64.dmg"
unzip -q tests/samples/doublecmd/doublecmd-1.1.32.i386-win32.zip \
  -d tests/samples/doublecmd/win32/

# Linux x86_64 ELF (sample #12) - extract the `doublecmd` binary from the gtk2 tarball:
curl -L -o /tmp/dc-linux.tar.xz \
  "https://github.com/doublecmd/doublecmd/releases/download/v1.1.32/doublecmd-1.1.32.gtk2.x86_64.tar.xz"
mkdir -p tests/samples/doublecmd/linux_x86_64
tar -xJf /tmp/dc-linux.tar.xz -C tests/samples/doublecmd/linux_x86_64 doublecmd/doublecmd
mv tests/samples/doublecmd/linux_x86_64/doublecmd/doublecmd \
   tests/samples/doublecmd/linux_x86_64/doublecmd.elf
rmdir tests/samples/doublecmd/linux_x86_64/doublecmd

# macOS x86_64 Mach-O (sample #6) - mount the DMG and copy the executable.
# The DMG itself is not kept in-tree (re-downloadable); only the extracted
# Mach-O binary is committed.
curl -L -o /tmp/dc.dmg \
  "https://github.com/doublecmd/doublecmd/releases/download/v1.1.32/doublecmd-1.1.32.cocoa.x86_64.dmg"
MNT=$(hdiutil attach -nobrowse -readonly /tmp/dc.dmg | grep -oE '/Volumes/.*' | head -1)
mkdir -p tests/samples/doublecmd/macos_x86_64
cp "$MNT/Double Commander.app/Contents/MacOS/doublecmd" \
   tests/samples/doublecmd/macos_x86_64/doublecmd.macho
hdiutil detach "$MNT"
# On a non-macOS host, substitute `7z x dc.dmg` to extract the HFS image.
```

Source: [github.com/doublecmd/doublecmd](https://github.com/doublecmd/doublecmd),
GPL-2.0. Project description: "Double Commander is a free cross platform open
source file manager ... entire codebase is written in Free Pascal and compiled
with Lazarus".

### 5 - Cheat Engine 7.0 (pcy190 fork) - FPC/Lazarus x86_64

```sh
curl -L -o tests/samples/cheatengine/CheatEngine70-bin.zip \
  "https://github.com/pcy190/Cheat-Engine/releases/download/7.0/bin.zip"
unzip -q tests/samples/cheatengine/CheatEngine70-bin.zip -d tests/samples/cheatengine/
```

Source: [github.com/pcy190/Cheat-Engine](https://github.com/pcy190/Cheat-Engine) - a
public mirror of the main Cheat Engine project providing pre-built binaries.

### 8 - Light Alloy Classic - Delphi 7 Win32

```sh
curl -L -o tests/samples/lightalloy/LA.exe \
  "https://raw.githubusercontent.com/Ta2i4/lightalloy-classic/master/Binary/LA.exe"
```

Source: [github.com/Ta2i4/lightalloy-classic](https://github.com/Ta2i4/lightalloy-classic),
GPL-2.0. Project README states: "To compile Light Alloy from source code you
will need plain Borland Delphi 7 (compile with any other version is impossible)".
The binary is committed in-tree rather than attached to a GitHub release.

### 9 & 10 - HeidiSQL 9.5 Portable (Win32, Delphi XE5)

```sh
curl -L -o tests/samples/heidisql/HeidiSQL_9.5_Portable.zip \
  "https://github.com/HeidiSQL/HeidiSQL/releases/download/9.5/HeidiSQL_9.5_Portable.zip"
unzip -q tests/samples/heidisql/HeidiSQL_9.5_Portable.zip \
  -d tests/samples/heidisql/portable_x86_xe5/
# The shipped heidisql.exe is UPX-compressed. Keep the packed copy for
# "identify but don't crash" tests and produce a sibling unpacked copy:
cp tests/samples/heidisql/portable_x86_xe5/heidisql.exe \
   tests/samples/heidisql/portable_x86_xe5/heidisql.unpacked.exe
upx -d tests/samples/heidisql/portable_x86_xe5/heidisql.unpacked.exe
```

Source: [github.com/HeidiSQL/HeidiSQL](https://github.com/HeidiSQL/HeidiSQL),
GPL-2.0. Release 9.5 was published 2017-12-19. The maintainer identifies the
build's Delphi version on the project forum:
[heidisql.com/forum.php?t=24817](https://www.heidisql.com/forum.php?t=24817).

### 11 - DelphiLint 1.3.0 - Delphi 12 Athens BPL

```sh
curl -L -o tests/samples/delphilint/DelphiLint-1.3.0-12Athens.zip \
  "https://github.com/integrated-application-development/delphilint/releases/download/v1.3.0/DelphiLint-1.3.0-12Athens.zip"
unzip -q tests/samples/delphilint/DelphiLint-1.3.0-12Athens.zip \
  -d /tmp/delphilint-extract/
mv /tmp/delphilint-extract/DelphiLint-1.3.0-Athens/DelphiLintClient-1.3.0-Athens.bpl \
   tests/samples/delphilint/
```

Source: [github.com/integrated-application-development/delphilint](https://github.com/integrated-application-development/delphilint),
LGPL-3.0. The release bundle also ships `-11Alexandria` and `-13Florence`
variants if a second/third BPL diff point is wanted later.

## IDR knowledge bases

`tests/kb.rs` reads every published IDR knowledge base under
`tests/samples/idr/` (gitignored, about 1.1 GB unpacked) and skips when the
directory is absent: the Win32 sets of IDR (`kb2` ... `kb7`, `kb2005` ...
`kb2014`, each with its `syskb`) in `win32/`, and IDR64's Win64 system sets
(`syskb2012` ... `syskb2014`) in `win64/`.

```sh
mkdir -p tests/samples/idr/win32 tests/samples/idr/win64
for v in 2 3 4 5 6 7 2005 2006 2007 2009 2010 2011 2012 2013 2014; do
  curl -L -o "tests/samples/idr/win32/kb$v.7z" \
    "https://raw.githubusercontent.com/crypto2011/IDR/master/kb$v.7z"
  7z x -otests/samples/idr/win32 "tests/samples/idr/win32/kb$v.7z" && rm "tests/samples/idr/win32/kb$v.7z"
  curl -L -o "tests/samples/idr/win32/syskb$v.bin" \
    "https://raw.githubusercontent.com/crypto2011/IDR/master/syskb$v.bin"
done
for v in 2012 2013 2014; do
  curl -L -o "tests/samples/idr/win64/syskb$v.bin" \
    "https://raw.githubusercontent.com/crypto2011/IDR64/master/syskb$v.bin"
done
```

Source: [github.com/crypto2011/IDR](https://github.com/crypto2011/IDR),
[github.com/crypto2011/IDR64](https://github.com/crypto2011/IDR64). MIT
licensed.
