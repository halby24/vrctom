#!/bin/sh
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
cd "$root"

if ! command -v wasm-pack >/dev/null 2>&1; then
  echo "wasm-pack が見つからない。mise install を実行してください。" >&2
  exit 1
fi

# npm に公開する成果物と、Webアプリが読む glue を同じビルドから作る。
# pkg/ は npm publish の入力であり、web/src/wasm/ はアプリの入力である。
rm -rf crates/vrctom-wasm/pkg
wasm-pack build crates/vrctom-wasm \
  --target web \
  --release \
  --out-dir pkg \
  --out-name vrctom_wasm \
  --scope halby \
  --no-gitignore \
  -- --locked

node crates/vrctom-wasm/scripts/finalize-package.mjs crates/vrctom-wasm/pkg

# 第三者の通知はルートに置かれているため、ある場合だけ同梱する。
if [ -f THIRD_PARTY_NOTICES.md ]; then
  cp THIRD_PARTY_NOTICES.md crates/vrctom-wasm/pkg/THIRD_PARTY_NOTICES.md
fi

rm -rf web/src/wasm
mkdir -p web/src/wasm
cp crates/vrctom-wasm/pkg/vrctom_wasm.js \
  crates/vrctom-wasm/pkg/vrctom_wasm.d.ts \
  crates/vrctom-wasm/pkg/vrctom_wasm_bg.wasm \
  crates/vrctom-wasm/pkg/vrctom_wasm_bg.wasm.d.ts \
  web/src/wasm/
