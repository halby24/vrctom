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

// パス昇順の Prefab 一覧。例: { candidates: [{ prefab: "Assets/Model.prefab",
//   state: "resolved", fbx: { path: "Assets/Model.fbx", ... }, ... }], unreadable: [] }
const targets = JSON.parse(pkg.prefabTargets());

// `resolved` の Prefab を選んで変換する。
const resolved = targets.candidates.find((candidate) => candidate.state === "resolved");
if (resolved === undefined) {
  throw new Error("変換できる Prefab がありません");
}
const converted = pkg.convertPrefab(
  resolved.prefab,
  JSON.stringify({
    name: "作品名",
    authors: ["作者名"],
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
| `pkg.prefabTargets()` | 全 Prefab の解決状態を JSON 文字列で返す（パス昇順） |
| `pkg.convertPrefab(prefabPath, optionsJson)` | 選んだ Prefab を VRM に変換する |
| `pkg.fbxCandidates()` | FBX 候補を JSON 文字列で返す（パス昇順。互換用） |
| `pkg.prefabCandidates(fbxPath)` | 対象 FBX を参照する Prefab 候補を JSON 文字列で返す（互換用） |
| `pkg.convert(fbxPath, optionsJson)` | 選んだ FBX を VRM に変換する（互換用） |
| `converted.takeVrm()` | GLB バイト列を複製せず取り出す。呼び出し後は空になる |
| `converted.vrm` | GLB バイト列を複製して返す（互換用） |
| `converted.report` | レポートを JSON 文字列で返す |
| `.free()` | wasm 側のメモリを解放する |

`WebPackage` は展開済みの内容を保持します。開き直す前に古い `WebPackage` を
`free()` してください。同じ `WebPackage` からは何度でも `convertPrefab`・`convert` できます。

## 変換オプション

Prefab 入口（`convertPrefab`）のオプションは次のとおりです。

```json
{
  "name": "作品名",
  "authors": ["作者A", "作者B"],
  "expressions": "descriptor",
  "baseline": true,
  "physics": true
}
```

- `name` … 空・未指定なら解決した FBX のファイル名（拡張子を除く）になる
- `authors` … 空要素を除き、残らなければ `["unknown"]` になる
- `expressions` … 省略時は `"descriptor"` と同じで、descriptor 由来の表情を作る。`"none"` で作らない
- `baseline` … 省略時は真。偽で基準姿勢の焼き込みを止める
- `physics` … 省略時は真で、選んだ Prefab から物理を導出して適用する。偽で物理なしにする

旧 FBX 入口（`convert`）のオプションは次のとおりです。互換のため残ります。

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
- `physics` … 省略時は `"auto"`。候補1件なら自動で使い、0件なら警告して物理なし
  （レポートの`selection`は`"undetected"`）になる。次の場合は`auto`のまま変換すると
  `stage=physics`で失敗する：候補が複数ある、解析できないPrefabがある。
  失敗時は`{"prefab": "Assets/.../X.prefab"}`で明示選択するか、`"none"`で物理なしにする。
  従来`{}`や`{"name": ...}`だけで成功した呼び出しも、候補の状態によっては失敗する。
  明示選択は候補一致が必須であり、候補外や読めない指定は失敗する。
  `"none"`は警告なしで物理なしになる

## レポート

Prefab 入口（`convertPrefab`）のレポートは次のとおりです。

```json
{
  "prefab": "Assets/Avatar.prefab",
  "fbx": "Assets/Model.fbx",
  "fbxGuid": "0123456789abcdef0123456789abcdef",
  "nestedPrefabs": ["Assets/Phone.prefab"],
  "bytes": 123456,
  "nodes": 10,
  "meshes": 2,
  "skins": 1,
  "materials": 3,
  "images": 4,
  "expressions": 8,
  "warnings": ["ネストPrefabのジオメトリは出力しない: Assets/Phone.prefab"],
  "assignment": {
    "mode": "instance",
    "activeMeshes": 2,
    "excludedMeshes": 1,
    "excludedNames": ["Hidden"],
    "materialPool": 3
  },
  "baseline": {
    "meshes": 1,
    "shapes": 2
  },
  "physics": {
    "prefab": "Assets/Avatar.prefab",
    "selection": "target",
    "springs": 2,
    "joints": 6,
    "colliders": 3,
    "groups": 1,
    "tposeApplied": 4,
    "tposeSkipped": 1,
    "lookAt": "bone"
  }
}
```

`assignment.mode` は割当の出所（`instance`=改変内容、`direct`=直接記述）であり、
無い場合は `null` です。`nestedPrefabs` は空でも必ず出ます。
`physics.selection` は Prefab 入口では `"target"` であり、
物理なしの場合は `prefab` と `lookAt` が `null` になります。
`baseline` は焼き込んだメッシュ数と非ゼロチャンネル数です。

旧 FBX 入口（`convert`）のレポートは次のとおりです。互換のため残ります。

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
  "warnings": ["材質を読めなかったため灰色で出力する"],
  "physics": {
    "prefab": "Assets/Avatar.prefab",
    "selection": "auto",
    "springs": 2,
    "joints": 6,
    "colliders": 3,
    "groups": 1,
    "tpose_applied": 4,
    "tpose_skipped": 1,
    "look_at": "bone"
  }
}
```

警告は `warnings` に入り、変換の失敗とは区別されます。
旧 FBX 入口の `physics.selection` は `"auto"`・`"explicit"`・`"none"`・`"undetected"` のいずれかであり、
`prefab` と `look_at` は物理なしの場合に `null` になります。

## エラー

失敗すると `stage` と、分かる場合は `path` を付けた `Error` を投げます。

| `stage` | 内容 |
|---|---|
| `package` | 展開・索引化 |
| `options` | オプション JSON の解釈 |
| `prefab` | Prefab対象の解決・本文読み取り・表情・基準姿勢の解決 |
| `fbx` | FBX の取り出し・解析（旧 FBX 入口） |
| `model` | 中間モデルの生成 |
| `material` | 材質・画像の読み取り |
| `assignment` | Prefab由来の材質割当の解決・適用（Prefab 入口） |
| `export` | VRM の書き出し |
| `physics` | Prefab物理（旧入口の候補走査・導出・適用、Prefab入口の導出・適用） |

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
| Prefab本文 1 件 | 64 MiB |
| 物理の依存資産合計 | 512 MiB |
| VRM 出力 | 512 MiB |

上限は JS から変更できません。

## 制限

- 入力は `.unitypackage` のみです。FBX 単体は受け付けません。
- ネスト Prefab のジオメトリは出力しません。直接 FBX が複数の Prefab の合成・
  Prefab Variant の汎用解決は未対応です。物理は選んだ Prefab から導出し、使わない再変換もできます。
- 生成した VRM が第三者製の validator をすべて通ることは保証しません。
- 現行版は `0.x` であり、API は変更される可能性があります。

旧 FBX 入口（`fbxCandidates`・`prefabCandidates`・`convert`）は互換のため残ります。
Web UI は Prefab 入口（`prefabTargets`・`convertPrefab`）を使い、
選んだ Prefab が直接参照する FBX を変換します。

## ライセンス

MIT。第三者の通知は
[`THIRD_PARTY_NOTICES.md`](https://github.com/halby24/vrctom/blob/main/THIRD_PARTY_NOTICES.md)
を参照してください。
