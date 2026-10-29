#!/usr/bin/env bash
# Full Windows release build (every format) cross-compiled on Linux, both executables copied to dist/.
#   scripts/build-release.sh [DEPS_DIR]   (default: ./deps; built on first run, see build-deps.sh)
set -euo pipefail
cd "$(dirname "$0")/.."
DEPS=${1:-deps}
scripts/build-deps.sh "$DEPS"
# shellcheck disable=SC1091
source "$DEPS/env.sh"
T=x86_64-pc-windows-gnu
cargo build --release --target $T -p swcraft -p swcraft-scan \
    --features swcraft/occt,swcraft/assimp,swcraft/ifc,swcraft-scan/occt,swcraft-scan/assimp,swcraft-scan/ifc
mkdir -p dist
cp target/$T/release/modelcraft.exe target/$T/release/swcraft-scan.exe dist/
ls -l dist/*.exe
