# PhysicsSidecarExporter

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
- 決定性: ノード DFS 順、コンポーネントは Transform 順（同 Transform 内は取得順）。
  同じ状態からはバイト同一の JSON になる（同一コードの再実行で md5 一致を確認）。
- bind-pose の復元は Unity 空間（インポート適用後）の行列で行う。FBX 生値との突合せは
  Rust 側の座標ゲートで行う。回転の正規化はこちらで済ませて出す。
- SMR の `bones` と `bindposes` の数が合わないメッシュはエラー終了する（壊れたメッシュ扱い）。
