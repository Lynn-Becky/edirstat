#!/usr/bin/env bash
# Build the eDirStat web frontend for production.
#
# Pipeline: cargo (release, wasm32, pinned nightly) -> wasm-bindgen (--target web) -> wasm-opt.
# The wasm binary is built with atomics/bulk-memory target features (see the
# cargo target rustflags), so wasm-opt must be told to allow those features.
#
# Nightly is required only here: wasm threads (`-C target-feature=+atomics`)
# need std rebuilt via the nightly-only `-Zbuild-std` (rust-src). The rest of
# the workspace builds on stable (see rust-toolchain.toml).
#
# Requires: rustup, wasm-bindgen-cli matching the lockfile's wasm-bindgen
# version, and binaryen (wasm-opt).
#
# Output: `crates/edirstat-gui/dist/`
#         (static files, serve with any web server, but requires CORS setup).
set -euo pipefail

cd "$(dirname "$0")/.."

if ! command -v rustup >/dev/null 2>&1; then
    echo "error: build_web.sh requires rustup (pinned nightly toolchain)" >&2
    exit 1
fi

# Pinned nightly with rust-src (for -Zbuild-std) and the wasm32 target.
# Idempotent no-op when already installed.
TOOLCHAIN="nightly-2026-07-15"
rustup toolchain install "$TOOLCHAIN" --component rust-src --target wasm32-unknown-unknown

GUI_CRATE="crates/edirstat-gui"
DIST="$GUI_CRATE/dist"
BIN_NAME="edirstat-web"
WASM_OPT_FEATURES=(
    --enable-bulk-memory
    --enable-threads
    --enable-nontrapping-float-to-int
    --enable-simd
    --enable-multivalue
)

echo "==> Building $BIN_NAME (release, wasm32-unknown-unknown, $TOOLCHAIN)"
RUSTFLAGS="${RUSTFLAGS:-} -D warnings" cargo +"$TOOLCHAIN" build -p edirstat-gui --bin "$BIN_NAME" --target wasm32-unknown-unknown --release

echo "==> Running wasm-bindgen"
rm -rf "$DIST"
mkdir -p "$DIST"
wasm-bindgen --target web --no-typescript \
    --out-dir "$DIST" \
    "target/wasm32-unknown-unknown/release/$BIN_NAME.wasm"

echo "==> Optimizing with wasm-opt"
wasm-opt "${WASM_OPT_FEATURES[@]}" -O4 -ol 100 -s 100 \
    -o "$DIST/${BIN_NAME}_bg.wasm" \
    "$DIST/${BIN_NAME}_bg.wasm"

cp "$GUI_CRATE/index.html" "$DIST/index.html"

echo "==> Done. Serve with e.g.: python3 -m http.server -d $DIST"
