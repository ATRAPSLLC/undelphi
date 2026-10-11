#!/usr/bin/env bash
# Builds the C++Builder 6 fixture matrix into tests/samples/bcb6/.
#
# Usage: tests/fixtures/bcb6/build.sh [path/to/CBuilder6]
#
# The toolchain is proprietary and never committed: point this at a licensed
# install, as the argument or UNDELPHI_BCB6 (the tests read the same
# variable).
# Sources are copied to a scratch directory so .dcu/.obj files never land in
# the fixture tree, and the output replaces tests/samples/bcb6/ only when the
# whole matrix built.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../../.." && pwd)
bcb=${1:-${UNDELPHI_BCB6:-}}
out="$repo/tests/samples/bcb6"

if [[ -z "$bcb" ]]; then
    echo "error: give the C++Builder 6 install as the argument or UNDELPHI_BCB6" >&2
    exit 1
fi

if [[ ! -f "$bcb/Bin/dcc32.exe" ]]; then
    echo "error: no Bin/dcc32.exe under '$bcb'" >&2
    exit 1
fi

docker build -q -t undelphi-bcb6 --build-arg "UID=$(id -u)" "$here" >/dev/null

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/src" "$work/out"
cp -r "$here/src/." "$work/src/"

docker run --rm \
    -v "$bcb":/bcb:ro \
    -v "$work/src":/work \
    -v "$work/out":/out \
    -v "$here/compile.sh":/compile.sh:ro \
    undelphi-bcb6 bash /compile.sh

# Replace the committed samples only after every build step succeeded.
rm -rf "$out"
mkdir -p "$out"
cp "$work/out/"* "$out/"
echo "fixtures written to $out"
