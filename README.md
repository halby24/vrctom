# vrctom

VRChat向けアバター（Unity Setup）の `.unitypackage` や FBX を VRM 1.0 に変換する実験的プロジェクトである。
Rustのライブラリ `vrctom` を核に、補助CLIとブラウザ内で動くWebアプリを持つ。

## 構成

| パス | 内容 |
|---|---|
| `crates/vrctom` | 変換ライブラリ。UnityPackage・FBX・`.mat` を読み、VRM 1.0を組み立てる |
| `crates/vrctom-cli` | 補助CLI（バイナリ名 `vrctom`） |
| `crates/vrctom-wasm` | Webアプリとnpmパッケージ向けのWasmバインディング |
| `web` | Vite + TypeScript のWebアプリ |

## 必要なツール

ツールチェーンは [mise](https://mise.jdx.dev/) で固定している。

```sh
mise install
```

Rust 1.96（`wasm32-unknown-unknown` を含む）、Node、pnpm が入る。
Webアプリを触る場合は依存パッケージも入れる。

```sh
pnpm --dir web install
```

WebアプリとnpmパッケージのWasmを作るには `wasm-pack` 0.15.1 も要る（`mise install` で入る。`wasm-bindgen` は `Cargo.lock` に合わせて wasm-pack が用意する）。

## ビルドとテスト

```sh
mise run check   # cargo fmt --all --check と cargo clippy
mise run test    # cargo nextest run --workspace
```

## ライブラリとして使う

`vrctom` はライブラリとしても使える。FBXのバイト列からVRM 1.0のGLBバイト列を作る入口は
`vrctom::conversion::convert_fbx` である。

```rust
use vrctom::conversion::convert_fbx;
use vrctom::fbx::FbxOptions;
use vrctom::vrm::{VrmExportOptions, VrmMeta, VrmOptions};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let fbx = std::fs::read("avatar.fbx")?;
    let meta = VrmMeta::new("アバター名", vec!["作者名".to_owned()]);
    let (vrm, _report) = convert_fbx(
        &fbx,
        &FbxOptions::default(),
        &VrmOptions::default(),
        &VrmExportOptions::new(meta),
    )?;
    std::fs::write("avatar.vrm", vrm)?;
    Ok(())
}
```

材質は呼び出し側が読み取って `VrmExportOptions::with_materials` で渡す。
渡さない場合は灰色のPBR材質になる。途中の段階を分けたい場合は
`build_model` と `export_model` を個別に呼ぶ。

## CLI

```sh
# UnityPackageを展開する
cargo run -p vrctom-cli -- unpack avatar.unitypackage -o avatar-unpacked

# FBXからVRM 1.0を書き出す
cargo run -p vrctom-cli -- export avatar.fbx -o avatar.vrm \
  --name "アバター名" --author "作者名"
```

`--assets` に `Assets` を含むUnityプロジェクトのルートを渡すと、`.mat` とテクスチャを解決する。
省略した場合はFBXの祖先から探す。

### 主なオプション

| オプション | 既定 | 内容 |
|---|---|---|
| `--assets DIR` | FBXの祖先から探索 | `.mat` とテクスチャを探すUnityプロジェクトのルート |
| `--prefab FILE` | なし | Prefabの改変内容または直接記述から、材質割当・有効状態・物理・表情を導出する |
| `--expressions vrc\|descriptor` | `--prefab` 指定時は `descriptor` | 表情の生成方法。`descriptor` は `--prefab` が要る |
| `--no-expressions` | 無効 | 表情を作らない |
| `--no-baseline` | 無効 | Prefabの `SkinnedMeshRenderer.m_BlendShapeWeights` の焼き込みを行わない |
| `--no-physics` | 無効 | 物理を載せない |
| `--physics FILE` | なし | 検証用。Unityで採取した物理sidecarを読む |
| `--renderers FILE` | なし | レンダラsidecarを読む。`--prefab` とは併用できない |
| `--avatar-permission` ほか | Unityと同じ | VRMのmetaに書く権限。`--allow-*` で個別に上書きする |

標準出力には何も出さない。要約と失敗の診断は標準エラーに出す。
終了コードは成功で0、実行失敗で1、引数不正で2である。

## Webアプリ

```sh
pnpm --dir web dev      # http://localhost:5173/vrctom/
pnpm --dir web build    # web/dist へ出力
pnpm --dir web preview  # ビルド結果を確認する
```

mise経由なら `mise run web-dev`・`mise run web-build`・`mise run wasm`（Wasmだけ作り直す）。
`.unitypackage` をブラウザ内で読み込んで変換する。ファイルは外部へ送らない。
公開版は <https://halby24.github.io/vrctom/> にある。

## npmパッケージ

Wasmバインディングは `@halby/vrctom-wasm` として公開するための成果物を
`crates/vrctom-wasm/pkg/` に作る。利用者向けの説明は
[`crates/vrctom-wasm/README.md`](crates/vrctom-wasm/README.md)、
保守者向けの公開手順は [`crates/vrctom-wasm/PUBLISHING.md`](crates/vrctom-wasm/PUBLISHING.md) にある。

```sh
mise run wasm
npm pack ./crates/vrctom-wasm/pkg --dry-run
```

## 制限

- 出力はVRM 1.0のみである（VRM 0.xは書かない）。
- Matcap合成・デカール・lilToon材質拡張は未対応である。
- 2019.3より前のFBX取り込み方式（`.meta` の `fileIDToRecycleName`）は未対応である。
- WebアプリはデスクトップのChromeを想定し、モバイルは対象外である。
  Prefabの材質割当・有効状態・物理は反映しない（CLIの `--prefab` は反映する）。

## 開発

設計文書は [`design/`](design) に置く。構成や規約は [`AGENTS.md`](AGENTS.md) を参照する。

## ライセンス

MIT License。[`LICENSE`](LICENSE) を参照する。

このリポジトリは第三者コードから処理を取り込んでいる。
取り込んだ範囲とライセンスは [`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md) にまとめている。
