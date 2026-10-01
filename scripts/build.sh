#!/usr/bin/env bash
# 构建引擎 wasm 并同步到 wasm/(GitHub Pages 按仓库布局原样出页面)
set -euo pipefail
cd "$(dirname "$0")/.."
RUSTFLAGS="-C target-feature=+simd128" cargo build --release --target wasm32-unknown-unknown
cp target/wasm32-unknown-unknown/release/aether_renju.wasm wasm/
echo "wasm -> wasm/aether_renju.wasm ($(stat -c%s wasm/aether_renju.wasm) bytes, gzip $(gzip -9c wasm/aether_renju.wasm | wc -c) bytes)"
