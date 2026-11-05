# Third-party notices

このリポジトリ自体のライセンスはMITである（[`LICENSE`](LICENSE)）。
以下は、このリポジトリが処理を取り込んだ、または規則を参照した第三者コードと、
同梱する依存の通知である。取り込んだ箇所にはソースのコメントにも出典を記す。
各ライセンスの全文は末尾の[ライセンス本文](#ライセンス本文)にある。

## SoulFlame VRM Exporter

- Package: `com.soulflame.vrm-exporter` 0.6.9
- License: MIT — Copyright (c) 2026 CucumberWorks
- Source: 参照実装としてローカルの `references/com.soulflame.vrm-exporter-0.6.9` に置く
  （`references/.gitignore` により追跡しない）。配布元は <https://cucumberworks.github.io/vpm/>
- 取り込んだ範囲: `crates/vrctom` のうち、材質・ベイク・表情・視線・物理・書き出しの処理は、
  同エクスポーターのUnity実装（C#・HLSL）をRustへ写したものを含む。
  特に対応が明確な箇所は次のとおり。
  - `crates/vrctom/src/vrm/bake.rs` — `SoulFlameBake.shader` の各パス、`BakeJobBuilder` の組み方、
    `BakeExecutor.Bind` の渡し方、`ShadeLayers`、`CappedTextureSerializer`、`MToonMapping`
  - `crates/vrctom/src/vrm/material.rs` — `ShaderCapability`・`ShaderCapabilityResult`・
    `RenderQueueOffsets`・`ClassifyLilToon` などのlilToon判定
  - `crates/vrctom/src/vrm/descriptor.rs` — `VRCAvatarDescriptor` からの表情の既定生成
  - `crates/vrctom/src/vrm/look_at.rs` — `RigLookAtRange` の範囲写像
  - `crates/vrctom/src/vrm/physics.rs` — スプリング関節値の式
  - `crates/vrctom/src/vrm/export.rs` — MToon材質の書き出し
- 変更点: C#・HLSLをRustへ書き換えた。参照実装にない層として、Unityを実行せず
  `.unitypackage`・FBX・`.mat` を直接読む処理をRustで持つ。
  GPUで行うベイクをCPUの画素処理へ置き換える。
  Matcap合成・デカール・lilToon材質拡張・VRM 0.x出力・プレビュー再生は持たない。

## lilToon

- Package: `jp.lilxyzw.liltoon`（色調補正は1.8.5、シェーダ判定表は2.3.4の `.shader.meta`）
- License: MIT — Copyright (c) 2020-present lilxyzw
- Source: <https://github.com/lilxyzw/lilToon>
- 取り込んだ範囲:
  - `lilToneCorrection`（`Shader/Includes/lil_common_functions.hlsl`）。
    参照実装の `ToneCorrection`（`SoulFlameBake.shader`）を経て
    `tone_correction`（`crates/vrctom/src/vrm/bake.rs`）へ写した。
  - 陰影・リム・発光の式（`lerp(indirect, indirect * albedo, _MainStrength)` など）。
    同じく参照実装のシェーダを経て `crates/vrctom/src/vrm/bake.rs` へ写した。
  - 不透明・カットアウト・半透明のシェーダGUID表（`crates/vrctom/src/vrm/material.rs`）。

## UniVRM

- Packages: `com.vrmc.gltf`（UniGLTF）、`com.vrmc.vrm`（VRM-1.0）、
  `com.vrmc.univrm`（VRM 0.x・MToon 0.x）0.131.2
- License: MIT
  - Copyright (c) 2020 VRM Consortium（`com.vrmc.vrm`・`com.vrmc.univrm`）
  - Copyright (c) 2018 Masataka SUMI for MToon（`com.vrmc.vrm`）
  - Copyright (c) 2018 ousttrue（`com.vrmc.gltf`/UniGLTF）
- Source: <https://github.com/vrm-c/UniVRM>（tag `v0.131.2`）
- 参照した範囲: コードの複製はなく、次の規則をRustで実装する際の照合先にしている。
  - MToon材質への `KHR_materials_unlit` の付与（`crates/vrctom/src/vrm/export.rs`）。
    参照実装が同梱するUniGLTFの `glTF_KHR_materials_unlit` に対応する。
  - スプリングボーンのlimitの扱い（`crates/vrctom/src/vrm/physics.rs`、
    [`design/VRMスプリング.md`](design/VRMスプリング.md)）。
    `LimitSpaceOffset`・`StraightDownMargin` による除外は未対応である。

## UnityのfileID（仕様の参照）

- 出典: Unity Discussions "FBX Submesh FileIDs"、
  "How to control fileID using custom ScriptedImporter"、およびs&boxの
  `importunitypackage` の `Editor/Core/UnityFileId.cs`
  （<https://sbox.game/grubs/importunitypackage/source?file=Editor%2FCore%2FUnityFileId.cs>）
- 参照した範囲: 2019.3以降の取り込みfileIDがXXH64（seed 0）と取り込みルートの固定値で
  決まること。コードの複製はなく、`crates/vrctom/src/unity/file_id.rs` が対象文字列の
  組み立てとハッシュだけを実装する。

## 同梱される実行時依存

Webアプリのバンドルには次のnpmパッケージが含まれる。いずれもMITであり、
著作権表示と全文は各パッケージの `LICENSE` にある。

| Package | Version | License | Source |
|---|---|---|---|
| `three` | 0.187.0 | MIT | <https://github.com/mrdoob/three.js> |
| `@pixiv/three-vrm` | 3.5.5 | MIT | <https://github.com/pixiv/three-vrm> |

WasmとCLIに含まれるRustクレートの一覧とバージョンは `Cargo.lock` のとおりである。
クレートごとのライセンスと著作権表示は、配布物（crates.io）の `LICENSE` にある。

## 取り込んでいないもの

参照実装の `THIRD_PARTY_NOTICES.md` には、ほかにMochiya Avatar Tools・りるぽい・
Lucide・Feather・three.js・three-vrm・UniVRMの記述がある。
このうちvrctomが処理を取り込んだ、または照合先にしたのは、
上に挙げたSoulFlame VRM Exporter・lilToon・UniVRMの範囲だけである。
Mochiya Avatar Tools（lilToon材質拡張）・りるぽい（シェーダ変換）・LucideとFeather（UIアイコン）・
three.jsとthree-vrmのプレビュー処理（ヒューマノイドのリターゲット・ノード拘束・スプリング再生）は
vrctomに対応する実装を持たない。

## ライセンス本文

上のMITの項目（SoulFlame VRM Exporter・lilToon・UniVRM・`three`・`@pixiv/three-vrm`）には、
次の本文と各項目の著作権表示を適用する。

MIT License

Copyright (c) 2026 CucumberWorks
Copyright (c) 2020-present lilxyzw
Copyright (c) 2020 VRM Consortium
Copyright (c) 2018 Masataka SUMI for MToon
Copyright (c) 2018 ousttrue
Copyright © 2010-2026 three.js authors
Copyright (c) 2019-2026 pixiv Inc.

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
