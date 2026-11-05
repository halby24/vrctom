# npm 公開手順

`@halby/vrctom-wasm` を npm へ公開するための保守者向け手順です。
利用者向けの説明は [README.md](README.md) にあります。

## 前提

- `mise install` で `wasm-pack` 0.15.1 が入っていること
- npm の `@halby` スコープを所有していること
- 初回公開（0.1.0）だけは `NPM_TOKEN`（Automation Token）が必要であること
- 2回目以降は npm の Trusted Publishing（OIDC）を使う

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

## 初回公開（0.1.0）

Trusted Publishing は既存のパッケージにしか設定できません。
そのため 0.1.0 だけは `NPM_TOKEN` で公開します。

1. リポジトリの Secrets に `NPM_TOKEN` を登録する。
2. workspace の版が `0.1.0` であることを確認する。
3. タグを push する。

```sh
git tag v0.1.0
git push origin v0.1.0
```

`.github/workflows/npm.yml` がテスト・ビルド・公開を行います。

## Trusted Publishing への移行（0.1.1 以降）

0.1.0 の公開後、npmjs.com の `@halby/vrctom-wasm` → Settings →
Trusted Publisher に GitHub Actions を登録します。

| 項目 | 値 |
|---|---|
| Organization or user（GitHub） | `halby24` |
| Repository | `vrctom` |
| Workflow filename | `npm.yml` |
| Environment name | 空欄 |
| Allowed actions | `npm publish` |

要件は次のとおりです。

- GitHub-hosted runner で実行すること
- workflow に `id-token: write` があること
- npm CLI 11.5.1 以上、Node 22.14 以上であること
- `package.json` の `repository.url` がリポジトリと一致すること

登録した Trusted Publisher は、2日以内に最初の公開を成功させないと失効します。
0.1.1 を出す直前に登録してください。

移行時は workflow から token を外します。

```diff
       - name: npmへ公開する
-        env:
-          NODE_AUTH_TOKEN: ${{ secrets.NPM_TOKEN }}
         run: |
-          args="--access public --provenance"
+          args="--access public"
           if [ "${{ inputs.dry_run }}" = "true" ]; then
             args="$args --dry-run"
           fi
           npm publish ./crates/vrctom-wasm/pkg $args
```

`--provenance` は Trusted Publishing では自動で付くため不要です。

0.1.1 を公開して OIDC が通ることを確認したら、次を行います。

1. リポジトリの `NPM_TOKEN` を削除する
2. npm の Settings → Publishing access を
   「Require 2FA and disallow tokens」に変更する

## 通常の公開（Trusted Publishing 移行後）

```sh
git tag v0.1.1
git push origin v0.1.1
```

`.github/workflows/npm.yml` が次を行います。

1. Rust のテスト
2. `web/scripts/build-wasm.sh` による wasm のビルド
3. タグと `pkg/package.json` の版の一致確認
4. `npm publish --access public`

手動で確認したい場合は Actions から `npm` を実行します。
`dry_run` が既定で true のため、そのままでは公開されません。
本公開するときだけ false にします。

## ローカルから公開する場合

CI を介さずに公開することもできます。
初回は `npm login` してから実行します。

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
- npm の `@halby` スコープは `halby` アカウントのスコープです。
  別のアカウントで公開する場合は、そのアカウントのスコープに合わせて
  `web/scripts/build-wasm.sh` の `--scope` を変更してください。
