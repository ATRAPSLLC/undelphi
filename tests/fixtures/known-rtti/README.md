# Known-RTTI golden fixtures

`known.pas` is a controlled source whose every type and member is known
exactly, so the golden test (`tests/known_rtti.rs`) can assert *precise*
extraction - exact enum values, record fields, property types, ancestry,
event signatures - against binaries we fully control. This catches accuracy
regressions and version/target differences that the real-world corpus (where
we don't know ground truth) can't.

## Build matrix

Built on a Windows box (FPC via `choco install freepascal --version=<v>`;
win64 cross add-on `fpc-<v>.i386-win32.cross.x86_64-win64.exe` from
SourceForge → install into the FPC dir with
`start /wait <cross>.exe /VERYSILENT /DIR=C:\tools\freepascal`). Generate
`fpc.cfg` once with `fpcmkcfg -d basepath=<fpcdir> -o <fpcdir>\bin\i386-win32\fpc.cfg`.

```cmd
fpc -O2 -Twin32 -Pi386   -oknown.win32.exe   known.pas
fpc -O2 -Twin64 -Px86_64 -oknown.win64.exe   known.pas
```

Compiled samples live in `tests/samples/known-rtti/`:

| File | Compiler | Target |
|------|----------|--------|
| `known.win32.exe`    | FPC 3.2.2 | i386-win32   |
| `known.win64.exe`    | FPC 3.2.2 | x86_64-win64 |
| `known304.win32.exe` | FPC 3.0.4 | i386-win32   |

## Findings surfaced by this fixture (2026-06-11)

- **FPC 3.2.2 extraction is exact** on both win32 and win64 - classes,
  ancestry, published property names *and types*, published methods, the
  `TColor` enum's value names, and the `TProgressEvent` event signature all
  match the source.
- **FPC 3.0.4 differs from 3.2.2.** Classes / ancestry / published method
  names decode the same, but published **property types don't resolve**
  (`prop_type_ref` reads a non-null but non-resolving VA - the 3.0.4
  `TPropInfo` layout differs from 3.2.2). Because the property types don't
  resolve, the type closure isn't seeded and `TColor` / `TProgressEvent` are
  not surfaced either. Tracked in `TODO.md`.
- **Standalone FPC types are not surfaced.** FPC emits `tkRecord` /
  `tkSet` / `tkDynArray` / `tkInterface` records for `TVertex`, `TColors`,
  `TStringArray`, `IWidget` (referenced only via `TypeInfo()`), but
  `types()` reaches none - FPC has no self-cell pattern and the closure only
  reaches class-member types. See `RESEARCH.md §5.8`. Tracked in `TODO.md`.

Keep `known.pas` and the golden assertions in lockstep.
