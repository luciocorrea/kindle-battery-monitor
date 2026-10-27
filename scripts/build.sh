#!/bin/sh
set -eu

if [ "$#" -ne 0 ]; then
    echo 'Usage: scripts/build.sh' >&2
    exit 2
fi

project=$(CDPATH= cd -P "$(dirname "$0")/.." && pwd)
cd "$project"
toolchain=$(sed -n 's/^channel = "\([0-9.]*\)"$/\1/p' rust-toolchain.toml)
[ -n "$toolchain" ] || { echo 'Missing pinned Rust toolchain' >&2; exit 1; }
compiler=$(rustup which --toolchain "$toolchain" rustc)
PATH="$(dirname "$compiler"):$PATH"
export PATH
export RUSTC_WRAPPER=
exec rustup run "$toolchain" cross build --locked --release \
    --target arm-unknown-linux-musleabihf --config 'build.rustc-wrapper=""'
