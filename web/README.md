# vrctom-web

`.unitypackage` をブラウザ内で読み込んで VRM を作る Web アプリ。Rust の変換処理を Wasm にして Vite + TypeScript から呼び出す。

## 必要なツール

- mise
- Rust 1.96（`wasm32-unknown-unknown` ターゲット、`rustfmt`・`clippy` を含む）
- wasm-bindgen 0.2.129（`Cargo.lock` と合わせる）
- Node 24
- pnpm 12

## セットアップ

```sh
mise install
pnpm --dir web install
```

## 開発サーバ

```sh
pnpm --dir web dev
```

`http://localhost:5173/vrctom/` を開く。

## ビルドとプレビュー

```sh
pnpm --dir web build
pnpm --dir web preview
```

プレビューは `http://localhost:4173/vrctom/` を開く。

## Wasm だけ作り直す

```sh
mise run wasm
```

`web/scripts/build-wasm.sh` がリリースビルドした `vrctom-wasm` に `wasm-bindgen` をかけて `web/src/wasm` に glue を出す。`web/src/wasm` と `web/dist` は生成物のためコミットしない。

## GitHub Pages の初回設定

リポジトリの Settings → Pages → Build and deployment で Source を「GitHub Actions」にする。以降は `main` への push（`crates/`・`Cargo.toml`・`Cargo.lock`・`web/`・`mise.toml`・Pages ワークフローの変更）で `.github/workflows/pages.yml` がビルドして `https://halby24.github.io/vrctom/` に配信する。

## 実測の目安

108MB の `.unitypackage`（16.8MB の FBX、4096 角のテクスチャを含む17枚）で、
展開は約1.5秒、変換は約25〜30秒、VRM は約49MB だった。
headless Chrome のプロセスツリーの RSS はピーク約2.7GB である。
デスクトップの Chrome を想定し、モバイルは対象外とする。

## 制限

- Prefab の材質割当や有効状態、揺れなどの物理は反映されない
- ファイルの読み込みと変換はブラウザ内で完結する
