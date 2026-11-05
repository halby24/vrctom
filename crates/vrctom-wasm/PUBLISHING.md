# npm 公開手順

`@halby24/vrctom-wasm` を npm へ公開するための保守者向け手順です。
利用者向けの説明は [README.md](README.md) にあります。

## 前提

- `mise install` で `wasm-pack` 0.15.1 が入っていること
- npm の `@halby24` スコープを所有していること
- リポジトリの Secrets に `NPM_TOKEN`（Automation Token）があること

## ローカルで作る

```sh
mise run wasm
```

`crates/vrctom-wasm/pkg/` に npm へ出す成果物ができ、
同じ glue が `web/src/wasm/` へ同期されます。
`pkg/` と `web/src/wasm/` はどちらもコミットしません。

wasm-pack が作る `package.json` は `scripts/finalize-package.mjs` が
`exports`・`files`・`publishConfig` などを補います。

中身を確認するには次を使います。

```sh
npm pack ./crates/vrctom-wasm/pkg --dry-run
```

## 版を上げる

`Cargo.toml` の workspace 版を上げます。

```toml
[workspace.package]
version = "0.1.1"
```

`crates/vrctom-wasm/Cargo.toml` は `version.workspace = true` のため、
個別の版は編集しません。`package.json` の版は wasm-pack が Cargo から作ります。

## 公開する

```sh
git tag v0.1.1
git push origin v0.1.1
```

`.github/workflows/npm.yml` が次を行います。

1. Rust のテスト
2. `web/scripts/build-wasm.sh` による wasm のビルド
3. タグと `pkg/package.json` の版の一致確認
4. `npm publish --access public --provenance`

手動で確認したい場合は Actions から `npm` を実行します。
`dry_run` が既定で true のため、そのままでは公開されません。
本公開するときだけ false にします。

## ローカルから公開する場合

CI を介さずに公開することもできます。

```sh
mise run wasm
npm publish ./crates/vrctom-wasm/pkg --access public
```

`--provenance` は CI 上でのみ意味があるため、ローカルでは付けません。

## 注意

- `crates/vrctom-wasm/LICENSE` はルートの `LICENSE` の写しです。
  ライセンス本文を変えるときは両方を更新してください。
- ルートの `THIRD_PARTY_NOTICES.md` は、存在する場合だけ pkg に同梱されます。
  公開前に必ずリポジトリへコミットしてください。
- `wasm-pack` の版は `mise.toml` と `.github/workflows/npm.yml` で固定しています。
  更新するときは両方を揃えてください。
- `wasm-bindgen` の版は `Cargo.lock` に従い、wasm-pack が用意します。
- npm の `@halby24` スコープは初回公開前に取得が必要です。
