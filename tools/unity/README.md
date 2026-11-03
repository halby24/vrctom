# PhysicsSidecarExporter

> この sidecar は **検証専用**である。実変換は `vrctom export --prefab <prefab>` が
> Prefab から物理を導出するため、Unity を回す必要はない。ここで作る JSON は、
> 導出経路の答えを突き合わせるための参照データとして使う。

Unity 2022.3 + VRChat SDK 3.10.5 のプロジェクトで、スプリングボーン変換用の物理 sidecar JSON
(`format: vrctom.physics`, `schemaVersion: 1`) を書き出すエディタヘルパ。
Rust 側の変換が読む中間ファイルを作る。シーンや Prefab の保存・変更はしない。

## 置き場所

- `tools/unity/PhysicsSidecarExporter.cs` が正本。検証プロジェクトへのコピーは
  `references/unity-milfy-vrm-check/Assets/Editor/PhysicsSidecar/` にある
  （`references/` は gitignore 対象のため、正式な置き場は `tools/unity/`）。

## 導入手順

1. `tools/unity/PhysicsSidecarExporter.cs` を対象プロジェクトの
   `Assets/Editor/PhysicsSidecar/PhysicsSidecarExporter.cs` に置く。
2. その `Assets/Editor/` が asmdef の支配下にある場合（`MilfyAutoExport.asmdef` のように
   `autoReferenced: false` の独自アセンブリなど）は、同フォルダに Editor 専用の asmdef を置く。
   支配下にない場合は asmdef 不要。例:

   ```json
   {
       "name": "vrctom.PhysicsSidecar.Editor",
       "rootNamespace": "vrctom.PhysicsSidecar",
       "references": [],
       "includePlatforms": ["Editor"],
       "excludePlatforms": [],
       "allowUnsafeCode": false,
       "overrideReferences": false,
       "precompiledReferences": [],
       "autoReferenced": true,
       "defineConstraints": [],
       "versionDefines": [],
       "noEngineReferences": false
   }
   ```

3. Unity がスクリプトを再コンパイルするのを待つ。

要件: Unity 2022.3、`com.vrchat.base` / `com.vrchat.avatars` 3.10.5。

## 実行方法

GUI: メニュー `Tools/vrctom/Export Physics Sidecar`（環境変数に従う。既定 bind-pose）、
`Tools/vrctom/Export Physics Sidecar (Bind Pose)`、
`Tools/vrctom/Export Physics Sidecar (Edit Mode)`（エディタは終了しない）。

バッチ (`unity run` が `-executeMethod` に転送する):

```sh
# 既定 (bind-pose)
unity run references/unity-milfy-vrm-check -- -executeMethod PhysicsSidecarExporter.Run
# 明示指定
unity run references/unity-milfy-vrm-check -- -executeMethod PhysicsSidecarExporter.RunBindPose
unity run references/unity-milfy-vrm-check -- -executeMethod PhysicsSidecarExporter.RunEditMode
# 環境変数で切替 (Run のみ有効。RunBindPose/RunEditMode は環境変数より優先)
VRCTOM_PHYSICS_STATE=edit-mode \
  unity run references/unity-milfy-vrm-check -- -executeMethod PhysicsSidecarExporter.Run
```

`VRCTOM_PHYSICS_STATE` は `bind-pose`（既定・未設定時）か `edit-mode`。
それ以外の値はエラー終了する。

出力先の既定値は
`/private/var/folders/3_/9fhdxzs960qgg34dctd97b840000gn/T/opencode/unity-export/milfy-physics.json`。
環境変数 `VRCTOM_PHYSICS_OUT` で上書きできる:

```sh
VRCTOM_PHYSICS_OUT=/tmp/out/physics.json \
  unity run references/unity-milfy-vrm-check -- -executeMethod PhysicsSidecarExporter.Run
```

アバタールートの決定: `Selection.activeGameObject` が `VRCAvatarDescriptor` を持てばそれ、
なければシーン内の `VRCAvatarDescriptor` を1件探す。複数あればエラー。
バッチ実行で開いているシーンに無い場合のみ `Assets/Scenes/MilfyCheck.unity` を
（保存せずに）開いて再探索する。

成功時は出力パスと件数をログに出し、バッチでは終了コード 0 で終わる。
成功ログには視線の要約（`lookAt=eyeLook... left=... right=...`）と
humanoid の要約（`humanoid=骨数(tPose省略数)`）も出す。
失敗時は `Debug.LogError` して終了コード 1。

## 参照姿勢 (statePolicy)

- `edit-mode`: シーンの local をそのまま読む。従来動作。
- `bind-pose`（既定）: `referenceLocal` を Body の `SkinnedMeshRenderer.bindposes`
  から復元したローカル TRS に置き換える。FBX の bind と edit-mode のポーズ差
  （Milfy では Ahoge のみに微差）を吸収し、回転を正規化して出す。
  - 各ボーンの bind 世界行列 = `smr.transform.localToWorldMatrix * bindpose.inverse`。
  - 親が同じ SMR のボーンなら親の bind 世界行列、そうでなければ親の scene 現在行列を使い、
    `local = parentWorld.inverse * boneWorld` を分解する（剛体前提。反転は扱わない）。
  - bindposes に無いノード（Ground、コンタクト、カメラ等、メッシュ自身）は scene の local を使い
    `referenceSource: "scene"`。ボーンは `"bind"`。scene 値は `localPosition/localRotation/localScale` を
    そのまま写すため、負スケールがあれば保持する。
  - bind 行列の分解は鏡像（負スケール）を保持する。行列式が負のとき X 軸の scale を負にし、
    `Matrix4x4.TRS` で元行列に戻るか検証する。戻らない（せん断等）場合は `referenceSource` を
    `"bind-decomposed-approx"` にし、`unsupportedFeatures` に `"trs-decomposition:<nodeId>"` を記録する。
  - SMR 選択: アバター配下でボーン数が最多、同点なら名前 `Body`（VRChat の慣例）を優先、
    次に頂点数が最多、それも同点なら階層順で最初。`capture.bindPoseSource` に SMR の scenePath を入れる
    （edit-mode では `[]`）。
  - `physBones` の `endpointPosition`、`colliders` の `position/rotation` は
    どちらの方針でも edit-mode の値を出す（次単位の検討事項として残す）。
  - 成功ログに `bind`（bind 適用数）/`scene`（フォールバック数）/`changed`（scene と値が変わった数）、
    `maxAngle/maxPos/maxScale`（最大差）を出す。

## スキーマ (sidecar v1)

- `format: "vrctom.physics"`、`schemaVersion: 1`。
- `generator`: `{name: "vrctom PhysicsSidecarExporter", version: "1", unityVersion, sdkVersion}`。
  `sdkVersion` は `Packages/com.vrchat.base/package.json` の version。
- `capture`: `{statePolicy: "edit-mode" | "bind-pose", coordinateSpace: "unity-avatar-root-local-meters",
  avatarRootId: "n:0", rootScale, bindPoseSource}`。`bindPoseSource` は bind-pose 時のみ SMR の scenePath。
  `rootScale` はどちらの方針でも scene 値。
- `sourceModels[]`: ノードが参照する FBX ごとの `{id, sha256, assetGuid, assetPath}`。
  `id` はファイル名小文字、`sha256` は FBX 実体の SHA-256（小文字 hex 64桁）。
- `nodes[]`: アバタールート直下の Transform を DFS 順に `n:0..`（ルート自身は含めない）。
  - `referenceSource` は値の出所で `"bind"` か `"scene"`。bind-pose では
    ボーンが `"bind"`、edit-mode では全ノードが `"scene"`。
  - `referenceLocal` は方針により以下。`scenePath`/`source`/ID/順序は方針で変わらない。
  - `parentId` はルート直下で `null`、`scenePath` はアバタールート名を含む名前列。
  - `source` は FBX 由来ノードのみ。対応元（Prefab→FBX と遡った最外）を親に辿り、
    FBX ルート名を除いた名前列を `path` にする。Prefab 直付けやシーン追加のノード
    （Ground、コンタクト、カメラ等）は `source: null`。
    モデル直置きノード（path が空になる場合）は自身名を `path` にする例外規定あり。
  - `referenceLocal` は `{position, rotation(xyzw), scale}`。
- `physBones[]`: 有効な `VRCPhysBone` を Transform 順に `pb:0..`。
  数値・カーブは `pull/spring/stiffness/gravity/gravityFalloff/immobile/radius/maxAngleX/maxAngleZ`、
  カーブは `{value, keys[{time,value,inSlope,outSlope,weightedMode,inWeight,outWeight}],
  preWrapMode, postWrapMode}`（`AnimationCurve` をそのまま写す）。
  `limitRotation`（float[3]）に加え、軸別カーブ
  `limitRotationXCurve/limitRotationYCurve/limitRotationZCurve` を同形式で出す。
  軸別カーブは単独の float 値を持たないため `value` は既定の `1`（空 keys 時は1倍扱い）。
  enum は文字列（`integrationType/multiChildType/immobileType/limitType`、
  `version` は `Version_1_1`→`"1.1"` に読み替える）。
- `colliders[]`: 有効な `VRCPhysBoneCollider` を Transform 順に `c:0..`。
  `shapeType/radius/height/position/rotation/insideBounds/bonesAsSpheres` を収録。
- `constraints[]`: VRChat 制約（`VRCConstraintBase` 派生）と Unity 制約（`IConstraint`）を
  Transform 順に出す。`{id, nodeId, targetNodeId, type, family, weight, enabled, active,
  freezeToWorld, affectsRotation[3], aimAxis[3], sources[]}` の形式である。
  `type` は `Rotation`・`Position`・`Scale`・`Parent`・`Aim`・`LookAt`、
  `family` は `vrchat`・`unity`。`nodeId`/`targetNodeId`/`sources[].nodeId` は
  ノードと同じ `n:N` の ID 体系であり、解決できない参照は `nodeId: ""` にし、
  `external: true` と `weight` を保持する（`targetNodeId` に外部参照は無く、
  アバター外の対象は収録しない）。VRChat 制約の種別別の軸は
  `VRCRotationConstraintBase.AffectsRotationX/Y/Z`、`VRCAimConstraintBase.AffectsRotationX/Y/Z` と
  `AimAxis`、`VRCLookAtConstraintBase` は3軸真、`VRCParentConstraintBase` は
  `AffectsRotationX/Y/Z`、Position/Scale は `affectsRotation` 全偽である。
  Unity 制約も同形式であり（Rotation/RotationAxes→`affectsRotation`、Aim/LookAt/Parent も同様、
  Position/Scale は全偽、`aimAxis` は Aim の `aimVector` のみ、他は既定の `[0,0,1]`）、
  `targetNodeId` は常に自ノードである。VRChat 制約に bound された Unity 制約
  （`GetBoundUnityConstraint`）は二重に出さない。取得できない場合はその旨をログに出す。
  ID は決定的であり、VRChat 制約は `vc:{scenePath}#{componentIndex}`、
  Unity 制約は `uc:{scenePath}#{componentIndex}`（`scenePath` は `/` 区切り、
  `componentIndex` は同 Transform 上の取得順）。
  出力可否の選別（2軸・複数参照・親付け・外部参照などの除外）は Rust 側が行い、
  こちらは読み取った値をそのまま出す。
- `lookAt`: descriptor の Eye Look を写した視線記録。
  `{eyeLook, leftEyeNodeId, rightEyeNodeId,
  eyesLookingStraight, eyesLookingUp, eyesLookingDown, eyesLookingLeft, eyesLookingRight}` の
  形式であり、参照姿勢の方針によらず descriptor の生値を出す。
  目のIDはノードと同じ `n:N` のID体系であり、未設定・アバター外は `""` になる。
  姿勢は `{linked, left[4], right[4]}` であり、値は正規化しない生値、
  `linked` が真の場合は左の値を両目に使う。未初期化の姿勢は非 linked の恒等回転になる。
  Rust 側は目の参照の存在と姿勢の有限値・4要素を検査し、正規化は測定時に行う。
- `humanoid`: humanoid の T-pose 記録。
  `{isHuman, bones[{bone, nodeId, tPose}]}` の形式であり、
  参照姿勢の方針によらず descriptor の生の T-pose を出す。
  参照実装の capture と同じく `avatar.humanDescription.skeleton` を
  Transform 名で引いて `Animator.GetBoneTransform(HumanBodyBones)` の
  各骨について記録する。`bone` は Unity の `HumanBodyBones` 名、
  `nodeId` はノードと同じ `n:N` のID体系であり、スナップショットの
  `n:0/0/...` とは別物である。`tPose` は `{position[3], rotation[4], scale[3]}` であり、
  参照実装の `RigCompiler.Humanoid.PoseHumanoid` と同じく回転だけを使う。
  Rust 側は参照姿勢からの差分δを右から掛けて適用するため、
  `Armature` の -90度X を焼き込まないこちらのフレームでも同じ姿勢変化になる。
  `skeleton` に無い骨は `tPose` を省き（JSON では `null`）、警告をログに出す。
  Rust 側はその骨を上書きしない。`isHuman` は
  `avatar != null && avatar.isHuman` の写しである。
  Rust 側は骨名とノード参照の存在、T-pose の位置・拡大率の有限値、
  回転の有限値と非ゼロを検査し、正規化は適用時に行う。
- `unsupportedFeatures[]`: VRM に持っていけない要素の記録（ソート済み）。
  `plane-colliders`（Plane コライダーあり）、
  `stretch-motion:pb:N`、`allow-grabbing:pb:N:値`、`allow-posing:pb:N:値`、
  `external-colliders:pb:N`（アバター外コライダー参照）、
  `root-transform-outside:pb:N`（rootTransform がアバター外→自ノード読替）。

## 制限（次単位としないもの）

- 収録対象の PhysBone・コライダーは Edit Mode で有効なものだけ
  （`component.enabled && activeInHierarchy`）。無効な SmartPhone 系は出ない。
  制約は有効・無効を問わず収録し、`enabled`/`active` で区別する（選別は Rust 側）。
- `allowCollision` は SDK 3.10.5 では `AdvancedBool` enum（False/True/Other）のため、
  `False` 以外を `true` に丸めて bool で出す（VRM 側に中間値が無いため）。
- `allowGrabbing` / `allowPosing` / `stretchMotion` 系（maxStretch/maxSquish 含む）、
  `grabMovement` / `snapToHand` / `parameter` / `resetWhenDisabled`、
  `staticFreezeAxis`、各種 Filter は sidecar に出さない。
  把握・伸縮は `unsupportedFeatures` にだけ残す。
- FX レイヤーのデフォルト値は収集しない。
  Plane コライダーは記録し、`plane-colliders` として申告する（Rust 側は補助ノードと
  `VRMC_springBone_extended_collider` で出力する）。
- Prefab にレンダラが直接書かれ、FBX のインスタンスを持たないアバター（シエル・ミルク・カリン）でも、
  骨（`SkinnedMeshRenderer.bones`）とレンダラを持つノードは、配下または祖先のメッシュから
  FBX を決めて `source` を付ける。`sourceModels` は空にならない。
  - Prefab で追加した補助オブジェクト（`PBCollider_*` 等）は骨でもレンダラでもないため
    `source: null` のまま出す（Rust 側が補助ノードとして作る）。
  - Unity が同名の兄弟に付ける ` 1` のような接尾辞は、FBX 側の名前と一致しないため
    対応付けず `source: null` にする（推測で名前を削らない）。
  - Prefab がメッシュを作者の親ノード（`髪` など）の下へ移している場合（シエル）、
    経路が FBX と一致せず一部のノードが対応付かない。この場合は導出経路を使う。
- 決定性: ノード DFS 順、コンポーネントは Transform 順（同 Transform 内は取得順）。
  同じ状態からはバイト同一の JSON になる（同一コードの再実行で md5 一致を確認）。
- bind-pose の復元は Unity 空間（インポート適用後）の行列で行う。FBX 生値との突合せは
  Rust 側の座標ゲートで行う。回転の正規化はこちらで済ませて出す。
- SMR の `bones` と `bindposes` の数が合わないメッシュはエラー終了する（壊れたメッシュ扱い）。

---

# RendererSidecarExporter

Unity 2022.3 のプロジェクトで、アバター Prefab を実体化して得られる
レンダラ割当の sidecar JSON
(`format: vrctom.renderers`, `schemaVersion: 1`) を書き出すエディタヘルパ。
Rust 側が材質割当と出力対象レンダラを正確に再現するための入力である。
購入アバターの材質割当は Prefab にあり、`.fbx.meta` の置換表だけでは足りないため、
Unity が解決した結果をそのまま受け取る。シーンや Prefab の保存・変更はしない。
edit-mode の初期状態を採取する（Play Mode・Animator・VRChat 実行時は対象外）。

## 置き場所

- `tools/unity/RendererSidecarExporter.cs` が正本。検証プロジェクトへのコピーは
  `references/unity-milfy-vrm-check/Assets/Editor/Renderers/` にある
  （`references/` は gitignore 対象のため、正式な置き場は `tools/unity/`）。

## 導入手順

1. `tools/unity/RendererSidecarExporter.cs` を対象プロジェクトの
   `Assets/Editor/Renderers/RendererSidecarExporter.cs` に置く。
2. その `Assets/Editor/` が asmdef の支配下にある場合（`MilfyAutoExport.asmdef` のように
   `autoReferenced: false` の独自アセンブリなど）は、同フォルダに Editor 専用の asmdef を置く。
   支配下にない場合は asmdef 不要。例:

    ```json
    {
        "name": "vrctom.Renderers.Editor",
        "rootNamespace": "vrctom.Renderers",
        "references": [],
        "includePlatforms": ["Editor"],
        "excludePlatforms": [],
        "allowUnsafeCode": false,
        "overrideReferences": false,
        "precompiledReferences": [],
        "autoReferenced": true,
        "defineConstraints": [],
        "versionDefines": [],
        "noEngineReferences": false
    }
    ```

3. Unity がスクリプトを再コンパイルするのを待つ。

要件: Unity 2022.3。

## 実行方法

GUI: メニュー `Tools/vrctom/Export Renderer Sidecar`（エディタは終了しない）。

バッチ (`unity run` が `-executeMethod` に転送する):

```sh
VRCTOM_RENDERERS_PREFAB=Assets/MANUKA/Prefab/MANUKA_lilToon.prefab \
VRCTOM_RENDERERS_OUT=/tmp/out/renderers.json \
  unity run references/unity-milfy-vrm-check -- -executeMethod RendererSidecarExporter.Run
```

`-quit` は渡さない（ラッパが管理していてエラーになる。スクリプト内で
`EditorApplication.Exit(0)` する）。

- `VRCTOM_RENDERERS_PREFAB`（必須）: 採取する Prefab のパス。
  空シーンへ `PrefabUtility.InstantiatePrefab` で実体化する。シーンは保存しない。
- `VRCTOM_RENDERERS_OUT`（必須）: 出力 JSON パス。
- 失敗時は `Debug.LogError` して終了コード 1。成功で 0。

## 採取手順

1. Prefab を空シーンへ実体化する。
2. インスタンス配下の全 `Renderer`（`Renderer` 派生: SkinnedMeshRenderer /
   MeshRenderer / ParticleSystemRenderer など）をシーン走査順に列挙する。
   対応しない種類（SkinnedMeshRenderer / MeshRenderer 以外）も「未対応」として
   必ず記録し、黙って飛ばさない（`supported: false`、`mesh: null`、
   `materials: []`、`note` に理由）。
3. `enabled` と `activeInHierarchy` を記録する。
   除外判定は Rust 側で行うので、無効も全部出す。

## スキーマ (sidecar v1)

- `format: "vrctom.renderers"`、`schemaVersion: 1`。
- `unityVersion`: Unity のバージョン。
- `sourcePrefab`: `{path, guid}`。採取した Prefab。
- `models[]`: レンダラの所有ノードとメッシュが参照する FBX ごとの
  `{path, guid, sha256}`（`path` 順にソート）。
  `sha256` はファイルの生バイトの SHA-256（小文字 hex 64桁）。
- `renderers[]`: シーン走査順に `index: 0..`。
  - `rootPath` / `path`: Prefab インスタンスのルートからのパス（`/` 区切り）。
    両者は同じ値である（複数ルート対応のためインスタンス基準で統一）。
  - `type`: レンダラの型名（`SkinnedMeshRenderer` など）。
  - `supported`: SkinnedMeshRenderer / MeshRenderer のとき真。
  - `enabled` / `activeInHierarchy`: 除外判定前の生値。
  - `model`: どの FBX のものか。`{path, guid, nodePath}`。
    `nodePath` は FBX 内の所有ノードのパス（後述）。判定できなければ `null`。
  - `mesh`: `{name, path, guid, fileID, subMeshCount, vertexCount, boneCount}`。
    `fileID` は 64bit（丸めない。浮動小数や指数表記にしない）。
    `boneCount` は SkinnedMeshRenderer の `bones` 長、MeshRenderer では 0。
    `sharedMesh` が無ければ `null`。
    MeshRenderer のメッシュは `MeshFilter.sharedMesh` から取る。
  - `materials`: `sharedMaterials` をサブメッシュ順に。
    要素は `{name, guid, fileID, path, shader}`、null は明示的な `null`。
    未対応種類では `[]`。
  - `note`: 未対応・判定不能の理由など。無ければ `null`。
- JSON は決定的に（並びは renderer のシーン走査順、キー順固定、インデント2）。
  時刻は入れない。

### `model.nodePath` の決め方

Rust 側は FBX 内のノードパスでメッシュを照合する
（Unity の fileID は FBX の ObjectId と無関係）。

- レンダラの GameObject から親をたどり、どのモデル（FBX アセット）に属するかを
  `PrefabUtility.GetCorrespondingObjectFromSource` と
  `AssetDatabase.GetAssetPath` で判定する。
  シーン→Prefab→FBX と対応元を遡り、最も外側の FBX 由来オブジェクトを使う。
  Prefab YAML からは解決できない形（FBX 本体を source にする PrefabInstance）も
  Unity の解決結果として受け取る。
- そのモデルのルート（Unity が FBX から生成するルート GameObject）を特定し、
  そこからの相対パスを `/` 区切りで出す。
- ルートが特定できない場合（Prefab 直付け・シーン追加のノード、
  最外対応元が FBX でない、自身が FBX ルート等）は `model` を null にし、
  `note` に理由を書く。推測したパスを出さない。
- 同じ FBX が複数インスタンス化されている場合は `nodePath` が重複し得る。
  その事実は Rust 側で検出させるため、重複を潰さずそのまま出す。

## 制限（次単位としないもの）

- 収録対象はインスタンス配下の全 `Renderer`。有効・無効を問わず収録し、
  `enabled` / `activeInHierarchy` で区別する（選別は Rust 側）。
- 決定性: レンダラは `GetComponentsInChildren` の走査順、
  `models` は `path` 順。同じ状態からはバイト同一の JSON になる。
- `materials` の `guid` / `fileID` が取れない組み込み材質等は
  `guid: ""`・`fileID: 0` で出す。
