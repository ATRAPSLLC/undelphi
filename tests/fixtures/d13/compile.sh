#!/usr/bin/env bash
# Runs inside the undelphi-bcb6 container (see build.sh): compiles every
# fixture in /work with the Delphi 13 compilers mounted at /bds, for Win32
# (dcc32) and Win64 (dcc64), and copies the units, packages, programs and
# linker maps to /out/win32 and /out/win64.
set -euo pipefail
cd /work

B='Z:\bds'
# Unit scope names, so `uses Classes` style sources resolve as in the IDE.
NS='-NSSystem;Winapi;Vcl;Data'

# Build all, quiet, detailed map (-GD); every output next to the sources so
# emit() finds it, the compiler's own resources (-R) for VCL programs.
dcc() {
    local exe=$1 lib=$2
    shift 2
    wine "$B\\bin\\$exe" -B -Q -GD "$NS" "-U$B\\lib\\$lib\\release;Z:\\work" \
        "-R$B\\lib\\$lib\\release" "-NUZ:\\work" "-LEZ:\\work" "-LNZ:\\work" "$@"
}

# Keeps a map's segment tables and its publics by name, minus the Win64
# unwind-data (PDATA) entries; the tests read nothing else, and the line
# tables would triple the committed size. It reads to the end so `tr` never
# sees a closed pipe.
trim_map() {
    tr -d '\r' <"$1" | awk '
        done { next }
        NR <= 8 && $4 == "PDATA" { pdata[substr($1, 1, 4)] = 1 }
        /Publics by Value/ { done = 1; next }
        /Publics by Name/ { pub = 1; print; next }
        !pub { print; next }
        !(substr($1, 1, 4) in pdata) { print }
    '
}

# Moves a build's outputs to /out/<platform> under a base name.
emit() {
    local platform=$1 from=$2 to=${3:-$2} ext
    mkdir -p "/out/$platform"
    for ext in exe bpl dcp dcu; do
        if [[ -f "$from.$ext" ]]; then
            mv "$from.$ext" "/out/$platform/$to.$ext"
        fi
    done
    if [[ -f "$from.map" ]]; then
        trim_map "$from.map" >"/out/$platform/$to.map"
        rm "$from.map"
    fi
}

wine "$B\\bin\\brcc32.exe" -fogtforms.res gtforms.rc >/dev/null

for target in "win32 dcc32.EXE" "win64 dcc64.exe"; do
    read -r platform exe <<<"$target"
    # Units on their own: the compiled units (.dcu) are the samples.
    dcc "$exe" "$platform" dcutypes.pas
    emit "$platform" dcutypes
    dcc "$exe" "$platform" modern.pas
    emit "$platform" modern
    # The same units in runtime packages: their exports are the mangled
    # names of every routine.
    dcc "$exe" "$platform" dcutypespkg.dpk
    emit "$platform" dcutypespkg
    dcc "$exe" "$platform" modernpkg.dpk
    emit "$platform" modernpkg

    # The C++Builder 6 fixture programs rebuilt (tests/programs.rs runs over
    # both compilers' builds).
    dcc "$exe" "$platform" known.dpr
    emit "$platform" known
    dcc "$exe" "$platform" -LUrtl known.dpr
    emit "$platform" known known_rtl
    dcc "$exe" "$platform" tables.dpr
    emit "$platform" tables
    dcc "$exe" "$platform" deep.dpr
    emit "$platform" deep
    dcc "$exe" "$platform" gtforms.dpr
    emit "$platform" gtforms
    # The programs' own units, so the DCU tests can place them in the
    # programs.
    for unit in gtframe gtmain gtchild; do
        emit "$platform" "$unit"
    done
    dcc "$exe" "$platform" gtpkg.dpk
    dcc "$exe" "$platform" '-LUrtl;gtpkg' usepkg.dpr
    emit "$platform" usepkg
    emit "$platform" gtpkg

    # The Delphi 2010+ binary fixture.
    dcc "$exe" "$platform" modernbin.dpr
    emit "$platform" modernbin
    emit "$platform" modernrt
    rm -f ./*.dcu
done

# Provenance: the exact compiler banners and hashes that produced /out.
{
    for exe in dcc32.EXE dcc64.exe; do
        (wine "$B\\bin\\$exe" 2>&1 || true) | sed -n 1p
    done
    (cd /bds/bin && sha256sum dcc32.EXE dcc64.exe brcc32.exe)
} | tr -d '\r' >/out/TOOLCHAIN.txt
