# FPC extended-method-RTTI fixture

`methodrtti.pas` was an attempt to force FPC to emit a populated
`TVmtMethodExTable` - the extended *class*-method RTTI (full signatures: param
names, types, modes, return type) for the decoder in TODO task #7.

## Finding (2026-06-11): FPC 3.2.2 does NOT emit `TVmtMethodExTable`

Built on a Windows box with FPC 3.2.2 (`choco install freepascal --version=3.2.2`)
and tested empirically:

- `{$M+}` + `{$RTTI EXPLICIT METHODS([...])}` on non-published methods →
  `TRttiContext.GetMethods` returns **empty**; the class has `method_table = 0`
  (no method table at all). The `{$RTTI}` methods directive is effectively a
  no-op in 3.2.2.
- **Published** methods → the *basic* `TVmtMethodTable` (names + code addresses)
  is emitted and decodes fine, but the bytes immediately after it are unrelated
  data (no `ExCount`/entries) - **no extended table follows**.

Conclusion: extended method RTTI for FPC is a **3.3.x / trunk** feature (the
cloned `reference/fpc-source` is trunk and has full support). No FPC ≤ 3.2.x
binary - including our entire FPC corpus - can carry `TVmtMethodExTable`, so a
decoder has no real-world binary to act on and is **deferred** until a 3.3.x+
sample is available. The realistic FPC method-signature win for current
binaries is the `tkMethod` (method-pointer/event type) decoder, which FPC 3.2.2
*does* emit and which is implemented and validated (`RESEARCH.md §14.18`).

The `.pas` and build steps below are kept for if/when FPC 3.3.x is used.

## Build (on a Windows box with FPC 3.2.2)

Install FPC 3.2.2 to match the corpus (doublecmd/cheatengine):

- Official installer (recommended, version-pinned):
  <https://sourceforge.net/projects/freepascal/files/Win32/3.2.2/> →
  `fpc-3.2.2.x86_64-win64.exe` (ships both win64 and i386-win32 targets), or
- `choco install freepascal` / `scoop install fpc` (version may differ).

Then:

```cmd
fpc -O2 methodrtti.pas                  rem -> methodrtti.exe  (x86_64-win64)
fpc -O2 -Twin32 -Pi386 methodrtti.pas   rem -> methodrtti.exe  (i386-win32)
```

Running `methodrtti.exe` should print each method with its parameters, e.g.
`DoWork(Title: AnsiString; Count: LongInt; Done: Boolean; )`. That confirms the
extended method table is present and walkable.

## Use

Copy the resulting `methodrtti.exe` into `tests/samples/fpc-methodrtti/` (keep
both win64 and win32 if built). They become the validation fixtures for the
`TVmtMethodExTable` decoder. Record the exact FPC version used (`fpc -iV`) so
the version-variant entry layout (`InvokeHelper` / `VER3_2` fields) is pinned.
