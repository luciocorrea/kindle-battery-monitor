#!/bin/sh
set -eu
if [ "$#" -ne 0 ]; then
    echo 'Usage: scripts/test.sh' >&2
    exit 2
fi
project=$(CDPATH= cd -P "$(dirname "$0")/.." && pwd)
cd "$project"
toolchain=$(sed -n 's/^channel = "\([0-9.]*\)"$/\1/p' rust-toolchain.toml)
[ -n "$toolchain" ] || { echo 'Missing pinned Rust toolchain' >&2; exit 1; }
compiler=$(rustup which --toolchain "$toolchain" rustc)
PATH="$(dirname "$compiler"):$PATH"
export PATH
PYTHON=${PYTHON:-python3}
export PYTHON
"$PYTHON" -c 'import sys; sys.exit(sys.version_info < (3, 11))' || {
    echo 'Tests require Python 3.11 or newer; set PYTHON to its executable.' >&2
    exit 1
}
RUSTC_WRAPPER= RUSTDOCFLAGS="${RUSTDOCFLAGS:+$RUSTDOCFLAGS }-D warnings" \
    rustup run "$toolchain" cargo doc --workspace --no-deps --locked
RUSTC_WRAPPER= rustup run "$toolchain" cargo test --workspace --locked
"$PYTHON" -m unittest discover -s packaging/kual/tests -v
"$PYTHON" -m unittest discover -s packaging/tests -v
