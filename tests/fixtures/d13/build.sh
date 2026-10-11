#!/usr/bin/env bash
# Builds the Delphi 13 fixture matrix into tests/samples/d13/.
#
# Usage: tests/fixtures/d13/build.sh [path/to/RAD-Studio-37.0]
#
# The toolchain is proprietary and never committed: point this at a licensed
# install, as the argument or UNDELPHI_D13 (the tests read the same
# variable). It reuses the C++Builder 6
# fixtures' Wine image (tests/fixtures/bcb6/Dockerfile). Sources are copied
# to a scratch directory, and the output replaces tests/samples/d13/ only
# when the whole matrix built.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../../.." && pwd)
bds=${1:-${UNDELPHI_D13:-}}
out="$repo/tests/samples/d13"

if [[ -z "$bds" ]]; then
    echo "error: give the RAD Studio 13 install as the argument or UNDELPHI_D13" >&2
    exit 1
fi

if [[ ! -f "$bds/bin/dcc32.EXE" || ! -f "$bds/bin/dcc64.exe" ]]; then
    echo "error: no bin/dcc32.EXE and bin/dcc64.exe under '$bds'" >&2
    exit 1
fi

docker build -q -t undelphi-bcb6 --build-arg "UID=$(id -u)" "$here/../bcb6" >/dev/null

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/src" "$work/out"
cp -r "$here/src/." "$work/src/"
# The C++Builder 6 fixtures' Pascal sources, rebuilt here so the same
# assertions run over both compilers' output.
for f in "$here/../bcb6/src/"*; do
    case "$f" in
        *.cpp) ;;
        *) cp "$f" "$work/src/" ;;
    esac
done

docker run --rm \
    -v "$bds":/bds:ro \
    -v "$work/src":/work \
    -v "$work/out":/out \
    -v "$here/compile.sh":/compile.sh:ro \
    undelphi-bcb6 bash /compile.sh

# Replace the committed samples only after every build step succeeded.
rm -rf "$out"
mkdir -p "$out"
cp -r "$work/out/." "$out/"
echo "fixtures written to $out"
