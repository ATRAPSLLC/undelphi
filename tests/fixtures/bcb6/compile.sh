#!/usr/bin/env bash
# Runs inside the undelphi-bcb6 container (see build.sh): compiles every
# fixture in /work with the C++Builder 6 tools mounted at /bcb and copies
# the binaries, packages and linker maps to /out.
set -euo pipefail
cd /work

B='Z:\bcb'
LIB="$B\\Lib\\Obj"

# Delphi 6 compiler: build all, quiet, detailed map (-GD) as ground truth
# for every routine and VMT address.
dcc() {
    wine "$B\\Bin\\dcc32.exe" -B -Q -GD "-U$LIB;Z:\\work" "-R$LIB" "$@"
}

# Borland C++ 5.6: multithreaded console target, map with publics (-m) and
# detailed segments (-s).
bcc() {
    wine "$B\\Bin\\bcc32.exe" -q -tWM -lm -ls \
        "-I$B\\Include" "-I$B\\Include\\Vcl" \
        "-L$B\\Lib" "-L$B\\Lib\\Obj" "-L$B\\Lib\\Release" "$@"
}

# Moves a build's outputs to /out under a new base name.
emit() {
    local from=$1 to=$2 ext
    for ext in exe bpl dcp dcu map; do
        if [[ -f "$from.$ext" ]]; then
            mv "$from.$ext" "/out/$to.$ext"
        fi
    done
}

# A unit on its own: the compiled unit (.dcu) is the sample.
dcc dcutypes.pas
emit dcutypes dcutypes

# The same unit in a runtime package: its exports are the mangled names of
# every routine of the unit.
dcc dcutypespkg.dpk
emit dcutypespkg dcutypespkg

dcc known.dpr
emit known known
dcc -LUrtl known.dpr
emit known known_rtl
dcc tables.dpr
emit tables tables
dcc deep.dpr
emit deep deep

wine "$B\\Bin\\brcc32.exe" -fogtforms.res gtforms.rc
dcc gtforms.dpr
emit gtforms gtforms
# The program's own units, so the DCU tests can place them in the program.
for unit in gtframe gtmain gtchild; do
    emit "$unit" "$unit"
done

dcc gtpkg.dpk
dcc '-LUrtl;gtpkg' usepkg.dpr
emit usepkg usepkg
emit gtpkg gtpkg

bcc -tWV -tWC cppknown.cpp
emit cppknown cppknown
bcc -tWC cppplain.cpp
emit cppplain cppplain

# Provenance: the exact tool banners and hashes that produced /out.
# The tools exit nonzero when run without arguments, so capture first.
dcc_banner=$(wine "$B\\Bin\\dcc32.exe" 2>&1 || true)
bcc_banner=$(wine "$B\\Bin\\bcc32.exe" 2>&1 || true)
{
    sed -n 1p <<<"$dcc_banner"
    sed -n 1p <<<"$bcc_banner"
    (cd /bcb/Bin && sha256sum dcc32.exe bcc32.exe ilink32.exe brcc32.exe)
} | tr -d '\r' >/out/TOOLCHAIN.txt
