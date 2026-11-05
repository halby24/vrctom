# @halby/vrctom-wasm

`.unitypackage`（またはその中の FBX）をブラウザ内で VRM 1.0 に変換する Wasm パッケージです。
入力ファイルをサーバへ送らず、変換はすべてブラウザ内で完結します。

## インストール

```sh
npm install @halby/vrctom-wasm
```

このパッケージは ESM 専用です（`require()` では読み込めません）。

## 使い方

```ts
import init, { WebPackage } from "@halby/vrctom-wasm";

// 最初に一度だけ初期化する。二度目以降は完了済みの Promise が返る。
await init();

const pkg = new WebPackage(new Uint8Array(unityPackageBytes));

// パス昇順の FBX 候補。例: [{ path: "Assets/Model.fbx", size: 12345 }]
const candidates = JSON.parse(pkg.fbxCandidates());

const converted = pkg.convert(
  candidates[0].path,
  JSON.stringify({
    name: "作品名",
    authors: ["作者名"],
    expressions: "vrc",
  }),
);

// 大きな VRM では複製しない takeVrm() を使う。
const vrm: Uint8Array = converted.takeVrm();
const report = JSON.parse(converted.report);

converted.free();
pkg.free();
```

`init()` を省略した場合は、このパッケージの JS の隣にある
`vrctom_wasm_bg.wasm` を `fetch` します。バンドラが wasm の配置を解決できない場合は、
wasm を配信してその URL を渡してください。

```ts
await init({ module_or_path: "/assets/vrctom_wasm_bg.wasm" });
```

### Worker 内で使う

`WebPackage` の生成と `convert` は同期的で、大きな入力では数秒から数十秒かかります。
UI を止めないため、Web Worker 内で呼び出してください。
中断は Worker の `terminate()` で行います。wasm 側に専用の中断 API はありません。

## API

| API | 説明 |
|---|---|
| `new WebPackage(bytes)` | パッケージを検証・展開・索引化する。失敗したら例外 |
| `pkg.fbxCandidates()` | FBX 候補を JSON 文字列で返す（パス昇順） |
| `pkg.convert(fbxPath, optionsJson)` | 選んだ FBX を VRM に変換する |
| `converted.takeVrm()` | GLB バイト列を複製せず取り出す。呼び出し後は空になる |
| `converted.vrm` | GLB バイト列を複製して返す（互換用） |
| `converted.report` | レポートを JSON 文字列で返す |
| `.free()` | wasm 側のメモリを解放する |

`WebPackage` は展開済みの内容を保持します。開き直す前に古い `WebPackage` を
`free()` してください。同じ `WebPackage` からは何度でも `convert` できます。

## 変換オプション

```json
{
  "name": "作品名",
  "authors": ["作者A", "作者B"],
  "expressions": "vrc"
}
```

- `name` … 空・未指定なら FBX のファイル名（拡張子を除く）になる
- `authors` … 空要素を除き、残らなければ `["unknown"]` になる
- `expressions` … `"vrc"` のときだけ VRChat 向けの 8 表情を作る

## レポート

```json
{
  "fbx": "Assets/Model.fbx",
  "bytes": 123456,
  "nodes": 10,
  "meshes": 2,
  "skins": 1,
  "materials": 3,
  "images": 4,
  "expressions": 8,
  "warnings": ["材質を読めなかったため灰色で出力する"]
}
```

警告は `warnings` に入り、変換の失敗とは区別されます。

## エラー

失敗すると `stage` と、分かる場合は `path` を付けた `Error` を投げます。

| `stage` | 内容 |
|---|---|
| `package` | 展開・索引化 |
| `options` | オプション JSON の解釈 |
| `fbx` | FBX の取り出し・解析 |
| `model` | 中間モデルの生成 |
| `material` | 材質・画像の読み取り |
| `export` | VRM の書き出し |

```ts
try {
  pkg.convert(path, "{}");
} catch (error) {
  if (error instanceof Error) {
    console.error(error.message);
    console.error(Reflect.get(error, "stage"));
    console.error(Reflect.get(error, "path"));
  }
}
```

## 資源上限

| 項目 | 上限 |
|---|---|
| 圧縮入力 | 512 MiB |
| 展開後総量 | 2 GiB |
| tar エントリ数 | 100,000 |
| FBX 1 件 | 128 MiB |
| 画像 1 枚 | 32 MiB |
| 画像合計 | 256 MiB |
| 画像 1 枚の画素数 | 4096² |
| VRM 出力 | 512 MiB |

上限は JS から変更できません。

## 制限

- 入力は `.unitypackage` のみです。FBX 単体は受け付けません。
- Prefab 由来の材質割当・有効状態・物理（揺れ）は反映しません。
- 生成した VRM が第三者製の validator をすべて通ることは保証しません。
- 現行版は `0.x` であり、API は変更される可能性があります。

## ライセンス

MIT。第三者の通知は
[`THIRD_PARTY_NOTICES.md`](https://github.com/halby24/vrctom/blob/main/THIRD_PARTY_NOTICES.md)
を参照してください。
