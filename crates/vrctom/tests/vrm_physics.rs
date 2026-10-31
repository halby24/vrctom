//! 物理sidecarの読み取りと対応解決の契約テスト（公開 API のみ使う）。
//!
//! 合成した `VrmModel` と合成sidecarで、完全パス一致・左右の区別・
//! シーン追加ノード・検証失敗・座標ゲートを固定する。
//! 実fixtureを使う検証は `VRCTOM_FBX_FIXTURE` と `VRCTOM_PHYSICS_SIDECAR` が
//! 指すファイルだけを読み、未設定なら跳ばす。跳ばした場合は完了証拠にしない。

use std::collections::HashMap;

use vrctom::fbx::{FbxFile, FbxOptions};
use vrctom::vrm::{
    HumanoidBones, HumanoidPoseReport, PhysicsLimits, PhysicsResolution, PhysicsSidecar,
    SkippedHumanoidReason, SpringCompileOptions, VrmErrorKind, VrmExportOptions,
    VrmExpressionProfile, VrmMeta, VrmModel, VrmNode, VrmOptions, VrmSpringBones,
    apply_humanoid_t_pose, compile_springs, resolve_look_at,
};

// テスト名は日本語で書く（テストは仕様・契約である）。

/// SHA-256を小文字hexで返す。fixtureとの照合値の組み立てに使う。
fn sha256_hex(バイト列: &[u8]) -> String {
    use sha2::Digest;
    let 摘要 = sha2::Sha256::digest(バイト列);
    let mut 出力 = String::with_capacity(64);
    for バイト in 摘要 {
        出力.push(char::from_digit(u32::from(バイト >> 4), 16).expect("hexである"));
        出力.push(char::from_digit(u32::from(バイト & 0x0f), 16).expect("hexである"));
    }
    出力
}

/// 単位行列（列優先）。
fn 単位行列() -> [f64; 16] {
    [
        1.0, 0.0, 0.0, 0.0, //
        0.0, 1.0, 0.0, 0.0, //
        0.0, 0.0, 1.0, 0.0, //
        0.0, 0.0, 0.0, 1.0, //
    ]
}

/// 平行移動だけの行列（列優先）。
fn 平行移動行列(位置: [f64; 3]) -> [f64; 16] {
    [
        1.0, 0.0, 0.0, 0.0, //
        0.0, 1.0, 0.0, 0.0, //
        0.0, 0.0, 1.0, 0.0, //
        位置[0], 位置[1], 位置[2], 1.0, //
    ]
}

/// 親列から子列を求める。
fn 子列(親: &[Option<usize>]) -> Vec<Vec<usize>> {
    let mut 子 = vec![Vec::new(); 親.len()];
    for (子索引, 親索引) in 親.iter().enumerate() {
        if let Some(親索引) = 親索引 {
            子[*親索引].push(子索引);
        }
    }
    子
}

/// 合成モデルを作る。階層は名前列と親列で決め、ローカル行列は呼び出し側が渡す。
fn 合成モデルを作る(
    名前: &[&str],
    親: &[Option<usize>],
    ローカル: &[[f64; 16]],
) -> VrmModel {
    let 子 = 子列(親);
    let ノード = 名前
        .iter()
        .enumerate()
        .map(|(索引, 名)| VrmNode {
            name: (*名).to_string(),
            parent: 親[索引],
            children: 子[索引].clone(),
            local: ローカル[索引],
        })
        .collect();
    VrmModel {
        root: 単位行列(),
        nodes: ノード,
        skins: Vec::new(),
        meshes: Vec::new(),
        expressions: vrctom::vrm::VrmExpressions::default(),
        spring_bones: VrmSpringBones::default(),
        node_constraints: Vec::new(),
        look_at: None,
    }
}

/// sidecarノード1件分のJSONを作る。
fn ノードjson(
    id: &str,
    親: Option<&str>,
    名: &str,
    現場パス: &[&str],
    出所: Option<serde_json::Value>,
    位置: [f64; 3],
) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "parentId": 親,
        "name": 名,
        "siblingIndex": 0,
        "scenePath": 現場パス,
        "source": 出所,
        "referenceLocal": {
            "position": 位置,
            "rotation": [0.0, 0.0, 0.0, 1.0],
            "scale": [1.0, 1.0, 1.0],
        },
    })
}

/// FBX由来の出所JSONを作る。
fn 出所json(モデル: &str, パス: &[&str]) -> serde_json::Value {
    serde_json::json!({
        "modelId": モデル,
        "path": パス,
        "assetGuid": "guid",
        "localId": -1,
    })
}

/// 空カーブのJSONを作る。
fn 空カーブjson() -> serde_json::Value {
    serde_json::json!({"value": 0.0, "keys": [], "preWrapMode": 8, "postWrapMode": 8})
}

/// PhysBone1件分のJSONを作る。
fn ボーンjson(
    id: &str,
    付属: &str,
    起点: &str,
    無視: &[&str],
    参照collider: &[&str],
) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "componentNodeId": 付属,
        "rootNodeId": 起点,
        "enabled": true,
        "version": "1.1",
        "integrationType": "Simplified",
        "ignoreNodeIds": 無視,
        "ignoreOtherPhysBones": false,
        "endpointPosition": [0.0, 0.0, 0.0],
        "multiChildType": "Ignore",
        "pull": 空カーブjson(),
        "spring": 空カーブjson(),
        "stiffness": 空カーブjson(),
        "gravity": 空カーブjson(),
        "gravityFalloff": 空カーブjson(),
        "immobileType": "AllMotion",
        "immobile": 空カーブjson(),
        "radius": 空カーブjson(),
        "allowCollision": true,
        "colliderIds": 参照collider,
        "limitType": "None",
        "maxAngleX": 空カーブjson(),
        "maxAngleZ": 空カーブjson(),
        "limitRotation": [0.0, 0.0, 0.0],
        "isAnimated": false,
    })
}

/// collider1件分のJSONを作る。
fn 衝突体json(id: &str, 付属: &str, 基準: &str, 形状: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "componentNodeId": 付属,
        "rootNodeId": 基準,
        "enabled": true,
        "shapeType": 形状,
        "insideBounds": false,
        "radius": 0.1,
        "height": 0.3,
        "position": [0.0, 0.0, 0.0],
        "rotation": [0.0, 0.0, 0.0, 1.0],
        "bonesAsSpheres": false,
    })
}

/// 最小構成のsidecar JSONを作る。ノード・ボーン・colliderは呼び出し側が渡す。
fn sidecar_json(
    fbxバイト列: &[u8],
    ノード: Vec<serde_json::Value>,
    ボーン: Vec<serde_json::Value>,
    衝突体: Vec<serde_json::Value>,
) -> String {
    serde_json::json!({
        "format": "vrctom.physics",
        "schemaVersion": 1,
        "generator": {"name": "試験", "version": "0.1"},
        "capture": {
            "statePolicy": "edit-mode",
            "coordinateSpace": "unity-avatar-root-local-meters",
            "avatarRootId": "n:0",
            "rootScale": [1.0, 1.0, 1.0],
        },
        "sourceModels": [{"id": "milfy", "sha256": sha256_hex(fbxバイト列)}],
        "nodes": ノード,
        "physBones": ボーン,
        "colliders": 衝突体,
        "unsupportedFeatures": [],
    })
    .to_string()
}

/// 左右を区別する最小構成。モデルとsidecarの対応表を作る。
fn 左右の対応を作る(
    fbxバイト列: &[u8],
    モデル位置: [[f64; 3]; 4],
    参照位置: [[f64; 3]; 4],
) -> (VrmModel, String) {
    // 階層は Armature -> Hips -> {Arm.L, Arm.R} である。
    let 名前 = ["Armature", "Hips", "Arm.L", "Arm.R"];
    let 親 = [None, Some(0), Some(1), Some(1)];
    let ローカル: Vec<[f64; 16]> = モデル位置.iter().map(|位置| 平行移動行列(*位置)).collect();
    let モデル = 合成モデルを作る(&名前, &親, &ローカル);
    // sidecarはUnityの左手系である。モデルの右手系ローカルから、
    // X反転した参照ローカルを作る（回転はテストでは単位）。
    let 参照: Vec<[f64; 3]> = 参照位置
        .iter()
        .map(|位置| [-位置[0], 位置[1], 位置[2]])
        .collect();
    let ノード = vec![
        ノードjson(
            "n:0",
            None,
            "Armature",
            &["Milfy", "Armature"],
            Some(出所json("milfy", &["Armature"])),
            参照[0],
        ),
        ノードjson(
            "n:1",
            Some("n:0"),
            "Hips",
            &["Milfy", "Armature", "Hips"],
            Some(出所json("milfy", &["Armature", "Hips"])),
            参照[1],
        ),
        ノードjson(
            "n:2",
            Some("n:1"),
            "Arm.L",
            &["Milfy", "Armature", "Hips", "Arm.L"],
            Some(出所json("milfy", &["Armature", "Hips", "Arm.L"])),
            参照[2],
        ),
        ノードjson(
            "n:3",
            Some("n:1"),
            "Arm.R",
            &["Milfy", "Armature", "Hips", "Arm.R"],
            Some(出所json("milfy", &["Armature", "Hips", "Arm.R"])),
            参照[3],
        ),
    ];
    let 文 = sidecar_json(
        fbxバイト列,
        ノード,
        vec![ボーンjson("pb:0", "n:2", "n:2", &[], &["c:0"])],
        vec![衝突体json("c:0", "n:3", "n:3", "Capsule")],
    );
    (モデル, 文)
}

/// 完全パスで左右を区別して対応付ける。シーン追加ノードは報告に残る。
#[test]
fn 完全パスで左右を区別して対応付ける() {
    // 左右非対称の点で、名前の取り違えが座標ゲートに響く配置にする。
    let fbxバイト列 = b"fake-fbx";
    let 位置 = [
        [0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.3, 1.0, 0.1],
        [-0.5, 1.0, 0.2],
    ];
    let (モデル, 文) = 左右の対応を作る(fbxバイト列, 位置, 位置);
    // Groundは参照されないシーン追加ノードであり、解決後も報告に残る。
    let mut 値: serde_json::Value = serde_json::from_str(&文).expect("合成sidecarは読める");
    値["nodes"]
        .as_array_mut()
        .expect("nodesは配列である")
        .push(ノードjson(
            "n:9",
            Some("n:0"),
            "Ground",
            &["Milfy", "Ground"],
            None,
            [0.0, 0.0, 0.0],
        ));
    let sidecar = PhysicsSidecar::from_json(&値.to_string(), &PhysicsLimits::new())
        .expect("合成sidecarは検証を通る");
    let 解決: PhysicsResolution = モデル
        .resolve_physics_sidecar(&sidecar, fbxバイト列)
        .expect("左右は完全パスで対応付く");
    assert_eq!(解決.matched_source_model, "milfy");
    assert_eq!(解決.fbx_sha256, sha256_hex(fbxバイト列));
    // 左右の取り違えが無いことを確かめる。
    assert_eq!(解決.node_map.get("n:2"), Some(&2));
    assert_eq!(解決.node_map.get("n:3"), Some(&3));
    assert_eq!(解決.phys_bones.len(), 1);
    assert_eq!(解決.phys_bones[0].component_node, 2);
    assert_eq!(解決.colliders.len(), 1);
    assert_eq!(解決.colliders[0].component_node, 3);
    assert_eq!(解決.unmapped_nodes.len(), 1);
    assert_eq!(解決.unmapped_nodes[0].id, "n:9");
    // 単位行列の対応では変換も単位行列になる。
    assert_eq!(解決.coordinate_transform, 単位行列());
}

/// 存在しないパスは未解決で失敗する。接尾辞での自動補完はしない。
#[test]
fn 存在しないパスは未解決で失敗する() {
    let fbxバイト列 = b"fake-fbx";
    let 位置 = [[0.0, 0.0, 0.0]; 4];
    let (モデル, _) = 左右の対応を作る(fbxバイト列, 位置, 位置);
    let mut 値: serde_json::Value =
        serde_json::from_str(&左右の対応を作る(fbxバイト列, 位置, 位置).1)
            .expect("合成sidecarは読める");
    // 片側だけ別名に変え、接尾辞一致でも救わないことを確かめる。
    値["nodes"][3]["source"] = 出所json("milfy", &["Armature", "Hips", "ArmX.R"]);
    let sidecar =
        PhysicsSidecar::from_json(&値.to_string(), &PhysicsLimits::new()).expect("書式自体は通る");
    let エラー = モデル
        .resolve_physics_sidecar(&sidecar, fbxバイト列)
        .expect_err("存在しないパスは未解決になる");
    assert_eq!(エラー.kind(), VrmErrorKind::UnresolvedPhysicsNode);
}

/// 重複パスは複数一致で失敗する。
#[test]
fn 重複パスは複数一致で失敗する() {
    // 同名の子を2件持ち、完全パスが重なるモデルを作る。
    let fbxバイト列 = b"fake-fbx";
    let 名前 = ["Hips", "Dup", "Dup"];
    let 親 = [None, Some(0), Some(0)];
    let ローカル = [平行移動行列([0.0, 0.0, 0.0]); 3];
    let モデル = 合成モデルを作る(&名前, &親, &ローカル);
    let ノード = vec![
        ノードjson(
            "n:0",
            None,
            "Hips",
            &["Milfy", "Hips"],
            Some(出所json("milfy", &["Hips"])),
            [0.0, 0.0, 0.0],
        ),
        ノードjson(
            "n:1",
            Some("n:0"),
            "Dup",
            &["Milfy", "Hips", "Dup"],
            Some(出所json("milfy", &["Hips", "Dup"])),
            [0.0, 0.0, 0.0],
        ),
    ];
    let 文 = sidecar_json(fbxバイト列, ノード, Vec::new(), Vec::new());
    let sidecar = PhysicsSidecar::from_json(&文, &PhysicsLimits::new()).expect("書式自体は通る");
    let エラー = モデル
        .resolve_physics_sidecar(&sidecar, fbxバイト列)
        .expect_err("重複パスは複数一致になる");
    assert_eq!(エラー.kind(), VrmErrorKind::UnresolvedPhysicsNode);
}

/// 参照されるシーン追加ノードは補助ノード保留で失敗する。
#[test]
fn 参照されるシーン追加ノードは補助ノード保留で失敗する() {
    let fbxバイト列 = b"fake-fbx";
    let 位置 = [[0.0, 0.0, 0.0]; 4];
    let (モデル, 文) = 左右の対応を作る(fbxバイト列, 位置, 位置);
    let mut 値: serde_json::Value = serde_json::from_str(&文).expect("合成sidecarは読める");
    let 追加 = ノードjson(
        "n:9",
        Some("n:0"),
        "Cardigan_Con",
        &["Milfy", "Cardigan_Con"],
        None,
        [0.0, 0.0, 0.0],
    );
    値["nodes"]
        .as_array_mut()
        .expect("nodesは配列である")
        .push(追加);
    // PhysBoneの起点をシーン追加ノードに向け、勝手な近似をしないことを確かめる。
    値["physBones"][0]["rootNodeId"] = serde_json::json!("n:9");
    let sidecar =
        PhysicsSidecar::from_json(&値.to_string(), &PhysicsLimits::new()).expect("書式自体は通る");
    let エラー = モデル
        .resolve_physics_sidecar(&sidecar, fbxバイト列)
        .expect_err("参照される追加ノードは保留になる");
    assert_eq!(エラー.kind(), VrmErrorKind::UnresolvedPhysicsNode);
}

/// SHA-256が一致しない場合は失敗する。一致が複数ある場合も失敗する。
#[test]
fn shaが一致しない場合は失敗する() {
    let fbxバイト列 = b"fake-fbx";
    let 位置 = [[0.0, 0.0, 0.0]; 4];
    let (モデル, 文) = 左右の対応を作る(fbxバイト列, 位置, 位置);
    let sidecar =
        PhysicsSidecar::from_json(&文, &PhysicsLimits::new()).expect("合成sidecarは検証を通る");
    let エラー = モデル
        .resolve_physics_sidecar(&sidecar, b"other-fbx")
        .expect_err("SHA不一致は失敗する");
    assert_eq!(エラー.kind(), VrmErrorKind::InvalidPhysicsSidecar);

    // 同一SHAの参照モデルが2件ある場合も、どれを使うか決められないため失敗する。
    let mut 値: serde_json::Value = serde_json::from_str(&文).expect("合成sidecarは読める");
    値["sourceModels"]
        .as_array_mut()
        .expect("sourceModelsは配列である")
        .push(serde_json::json!({"id": "milfy2", "sha256": sha256_hex(fbxバイト列)}));
    let 重複 =
        PhysicsSidecar::from_json(&値.to_string(), &PhysicsLimits::new()).expect("書式自体は通る");
    let エラー = モデル
        .resolve_physics_sidecar(&重複, fbxバイト列)
        .expect_err("一致2件は失敗する");
    assert_eq!(エラー.kind(), VrmErrorKind::InvalidPhysicsSidecar);
}

/// sidecar JSONへの変形。破損パターンの表しに使う。
type Sidecar変形 = Box<dyn Fn(&mut serde_json::Value)>;

/// 書式・版・列挙・ID・親子・有限値・範囲・曲線の破損を拒否する。
#[test]
fn 破損したsidecarを拒否する() {
    let fbxバイト列 = b"fake-fbx";
    let 位置 = [[0.0, 0.0, 0.0]; 4];
    let (_, 文) = 左右の対応を作る(fbxバイト列, 位置, 位置);
    let 基底: serde_json::Value = serde_json::from_str(&文).expect("合成sidecarは読める");
    let 場合: Vec<(&str, Sidecar変形)> = vec![
        (
            "書式名",
            Box::new(|値| 値["format"] = serde_json::json!("other")),
        ),
        (
            "版",
            Box::new(|値| 値["schemaVersion"] = serde_json::json!(2)),
        ),
        (
            "未知enum",
            Box::new(|値| 値["physBones"][0]["integrationType"] = serde_json::json!("Magic")),
        ),
        (
            "重複ID",
            Box::new(|値| 値["nodes"][1]["id"] = serde_json::json!("n:0")),
        ),
        (
            "循環親",
            Box::new(|値| {
                値["nodes"][0]["parentId"] = serde_json::json!("n:1");
            }),
        ),
        (
            "未知wrap",
            Box::new(|値| 値["physBones"][0]["pull"]["preWrapMode"] = serde_json::json!(99)),
        ),
        (
            "重み付き",
            Box::new(|値| {
                値["physBones"][0]["pull"]["keys"] = serde_json::json!([{
                    "time": 0.0, "value": 0.0,
                    "inSlope": 0.0, "outSlope": 0.0,
                    "weightedMode": 1, "inWeight": 0.0, "outWeight": 0.0,
                }]);
            }),
        ),
        (
            "負半径",
            Box::new(|値| 値["physBones"][0]["radius"]["value"] = serde_json::json!(-1.0)),
        ),
        (
            "欠落参照",
            Box::new(|値| 値["physBones"][0]["componentNodeId"] = serde_json::json!("n:99")),
        ),
    ];
    for (名, 変形) in 場合 {
        let mut 値 = 基底.clone();
        変形(&mut 値);
        let 結果 = PhysicsSidecar::from_json(&値.to_string(), &PhysicsLimits::new());
        assert!(
            matches!(
                結果.as_ref().map(|_| ()).map_err(|e| e.kind()),
                Err(VrmErrorKind::InvalidPhysicsSidecar | VrmErrorKind::LimitExceeded)
            ),
            "{名}は拒否する",
        );
    }
    // JSONとして読めない非有限値も拒否する。
    let 壊れ = 文
        .replace("\"value\": 0.0", "\"value\": NaN")
        .replace("0.0,", "NaN,");
    assert!(
        PhysicsSidecar::from_json(&壊れ, &PhysicsLimits::new()).is_err(),
        "非有限値は拒否する",
    );
}

/// 資源上限の超過を拒否する。
#[test]
fn 資源上限の超過を拒否する() {
    let fbxバイト列 = b"fake-fbx";
    let 位置 = [[0.0, 0.0, 0.0]; 4];
    let (_, 文) = 左右の対応を作る(fbxバイト列, 位置, 位置);
    let 制限 = PhysicsLimits::new().with_max_nodes(3);
    let エラー = PhysicsSidecar::from_json(&文, &制限).expect_err("ノード上限の超過は失敗する");
    assert_eq!(エラー.kind(), VrmErrorKind::LimitExceeded);
}

/// 制約の破損（未知種別・重複ID・空対象・非有限・上限）を拒否する。
#[test]
fn 制約の破損を拒否する() {
    let fbxバイト列 = b"fake-fbx";
    let 位置 = [[0.0, 0.0, 0.0]; 4];
    let (_, 文) = 左右の対応を作る(fbxバイト列, 位置, 位置);
    let 基底: serde_json::Value = serde_json::from_str(&文).expect("合成sidecarは読める");
    let 正常 = |id: &str| {
        serde_json::json!({
            "id": id,
            "nodeId": "n:2",
            "targetNodeId": "n:2",
            "type": "Rotation",
            "family": "vrchat",
            "weight": 0.5,
            "enabled": true,
            "active": true,
            "freezeToWorld": false,
            "affectsRotation": [true, true, true],
            "aimAxis": [0.0, 0.0, 1.0],
            "sources": [{"nodeId": "n:3", "external": false, "weight": 1.0}],
        })
    };
    // 正常な1件は通る。
    let mut 通る = 基底.clone();
    通る["constraints"] = serde_json::json!([正常("k:0")]);
    PhysicsSidecar::from_json(&通る.to_string(), &PhysicsLimits::new()).expect("正常な制約は通る");
    // 未知種別・重複ID・空対象は拒否する。
    let 場合: Vec<(&str, serde_json::Value)> = vec![
        (
            "未知種別",
            serde_json::json!([{
                "id": "k:0", "nodeId": "n:2", "targetNodeId": "n:2", "type": "Magic",
                "family": "vrchat", "weight": 0.5, "enabled": true, "active": true,
                "freezeToWorld": false, "affectsRotation": [true, true, true],
                "aimAxis": [0.0, 0.0, 1.0],
                "sources": [{"nodeId": "n:3", "external": false, "weight": 1.0}],
            }]),
        ),
        ("重複ID", serde_json::json!([正常("k:0"), 正常("k:0")])),
        (
            "空対象",
            serde_json::json!([{
                "id": "k:0", "nodeId": "", "targetNodeId": "", "type": "Rotation",
                "family": "vrchat", "weight": 0.5, "enabled": true, "active": true,
                "freezeToWorld": false, "affectsRotation": [true, true, true],
                "aimAxis": [0.0, 0.0, 1.0],
                "sources": [{"nodeId": "n:3", "external": false, "weight": 1.0}],
            }]),
        ),
    ];
    for (名, 制約列) in 場合 {
        let mut 値 = 基底.clone();
        値["constraints"] = 制約列;
        let 結果 = PhysicsSidecar::from_json(&値.to_string(), &PhysicsLimits::new());
        assert!(
            matches!(
                結果.as_ref().map(|_| ()).map_err(|e| e.kind()),
                Err(VrmErrorKind::InvalidPhysicsSidecar | VrmErrorKind::LimitExceeded)
            ),
            "{名}は拒否する",
        );
    }
    // 非有限の重みはJSONとして読めず拒否する。
    let mut 非有限 = 基底.clone();
    非有限["constraints"] = serde_json::json!([正常("k:0")]);
    let 壊れ = 非有限
        .to_string()
        .replace("\"weight\":0.5", "\"weight\":NaN");
    assert!(
        PhysicsSidecar::from_json(&壊れ, &PhysicsLimits::new()).is_err(),
        "非有限の重みは拒否する",
    );
    // 制約数の上限超過は失敗する。
    let 制限 = PhysicsLimits::new().with_max_constraints(0);
    let エラー =
        PhysicsSidecar::from_json(&通る.to_string(), &制限).expect_err("制約上限の超過は失敗する");
    assert_eq!(エラー.kind(), VrmErrorKind::LimitExceeded);
}

/// 参照姿勢が編集時でない場合は未対応で失敗する。
#[test]
fn 編集時以外の参照姿勢は未対応で失敗する() {
    let fbxバイト列 = b"fake-fbx";
    let 位置 = [[0.0, 0.0, 0.0]; 4];
    let (モデル, 文) = 左右の対応を作る(fbxバイト列, 位置, 位置);
    let mut 値: serde_json::Value = serde_json::from_str(&文).expect("合成sidecarは読める");
    値["capture"]["statePolicy"] = serde_json::json!("fx-default");
    let sidecar =
        PhysicsSidecar::from_json(&値.to_string(), &PhysicsLimits::new()).expect("書式自体は通る");
    let エラー = モデル
        .resolve_physics_sidecar(&sidecar, fbxバイト列)
        .expect_err("FX既定は今回未対応である");
    assert_eq!(エラー.kind(), VrmErrorKind::UnsupportedPhysicsFeature);
}

/// 平面colliderは今回の出力対象外として除外し、参照解決は続行する。
#[test]
fn 平面colliderは未対応として除外する() {
    let fbxバイト列 = b"fake-fbx";
    let 位置 = [[0.0, 0.0, 0.0]; 4];
    let (モデル, 文) = 左右の対応を作る(fbxバイト列, 位置, 位置);
    let mut 値: serde_json::Value = serde_json::from_str(&文).expect("合成sidecarは読める");
    値["colliders"][0]["shapeType"] = serde_json::json!("Plane");
    let sidecar =
        PhysicsSidecar::from_json(&値.to_string(), &PhysicsLimits::new()).expect("書式自体は通る");
    let 解決 = モデル
        .resolve_physics_sidecar(&sidecar, fbxバイト列)
        .expect("平面は除外して続行する");
    assert!(
        解決.colliders.is_empty(),
        "平面は解決済みcolliderに含めない"
    );
    assert_eq!(解決.skipped_colliders.len(), 1, "平面を除外として報告する");
}

/// 軸回転と単位換算が単一の変換行列に一致する。
#[test]
fn 軸回転と単位換算が単一の変換に一致する() {
    // sidecarはメートル・Y-up、モデルはセンチ・Z-up相当の回転をルートに持つ。
    // 全関節が同じ変換を共有するため、ゲートは通る。
    let fbxバイト列 = b"fake-fbx";
    let 原点 = [[0.0, 0.0, 0.0]; 4];
    let (mut モデル, 文) = 左右の対応を作る(fbxバイト列, 原点, 原点);
    // X軸+90度回転と100倍（cm→m換算の逆向き）を合成したルートである。
    モデル.root = [
        100.0, 0.0, 0.0, 0.0, //
        0.0, 0.0, 100.0, 0.0, //
        0.0, -100.0, 0.0, 0.0, //
        0.0, 0.0, 0.0, 1.0, //
    ];
    let sidecar =
        PhysicsSidecar::from_json(&文, &PhysicsLimits::new()).expect("合成sidecarは検証を通る");
    let 解決 = モデル
        .resolve_physics_sidecar(&sidecar, fbxバイト列)
        .expect("全関節で共通の変換は通る");
    assert_eq!(解決.coordinate_transform, モデル.root);
}

/// 一部の関節だけ変換が違う場合は失敗する。
#[test]
fn 一部だけ変換が違う場合は失敗する() {
    let fbxバイト列 = b"fake-fbx";
    let モデル位置 = [[0.0, 0.0, 0.0]; 4];
    // 参照側だけ右腕をずらし、単一変換に一致しない配置にする。
    let 参照位置 = [
        [0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.3, 1.0, 0.1],
        [9.9, 9.9, 9.9],
    ];
    let (モデル, 文) = 左右の対応を作る(fbxバイト列, モデル位置, 参照位置);
    let sidecar = PhysicsSidecar::from_json(&文, &PhysicsLimits::new()).expect("書式自体は通る");
    let エラー = モデル
        .resolve_physics_sidecar(&sidecar, fbxバイト列)
        .expect_err("単一変換に一致しない配置は失敗する");
    assert_eq!(エラー.kind(), VrmErrorKind::PhysicsCoordinateMismatch);
}

/// 既定のspring_bonesは空であり、書き出しは退行しない。
#[test]
fn 既定のスプリングは空で書き出せる() {
    use vrctom::vrm::{VrmExportOptions, VrmExpressions, VrmMeta, VrmNode};
    let ノード = vec![
        VrmNode {
            name: "Hips".to_string(),
            parent: None,
            children: vec![1],
            local: 単位行列(),
        },
        VrmNode {
            name: "Spine".to_string(),
            parent: Some(0),
            children: Vec::new(),
            local: 単位行列(),
        },
    ];
    let モデル = VrmModel {
        root: 単位行列(),
        nodes: ノード,
        skins: Vec::new(),
        meshes: Vec::new(),
        expressions: VrmExpressions::default(),
        spring_bones: VrmSpringBones::default(),
        node_constraints: Vec::new(),
        look_at: None,
    };
    assert!(モデル.spring_bones.springs.is_empty());
    assert!(モデル.spring_bones.colliders.is_empty());
    assert!(モデル.spring_bones.collider_groups.is_empty());
    // humanoid必須骨が無いため書き出しは失敗するが、失敗種別は従来どおりである。
    let 設定 = VrmExportOptions::new(VrmMeta::new("試験", vec!["作者".to_string()]));
    let エラー = モデル
        .to_vrm(&設定)
        .expect_err("骨不足は従来どおり失敗する");
    assert_eq!(
        エラー.kind(),
        vrctom::vrm::VrmExportErrorKind::MissingHumanoidBone
    );
}

/// 実fixtureと実sidecarで参照解決を通す。未設定なら跳ばす。
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn 実fixtureと実sidecarで参照を解決する() {
    let (Some(fbx経路), Some(sidecar経路)) = (
        std::env::var_os("VRCTOM_FBX_FIXTURE"),
        std::env::var_os("VRCTOM_PHYSICS_SIDECAR"),
    ) else {
        eprintln!("VRCTOM_FBX_FIXTUREまたはVRCTOM_PHYSICS_SIDECAR未設定のため跳ばす");
        return;
    };
    let fbxバイト列 = std::fs::read(&fbx経路).expect("実fixtureは読める");
    let sidecar文 = std::fs::read(&sidecar経路).expect("実sidecarは読める");
    let 解析 = vrctom::fbx::FbxFile::parse(&fbxバイト列, &vrctom::fbx::FbxOptions::new())
        .expect("実fixtureは読める");
    let (モデル, _) =
        VrmModel::from_fbx(&解析, &vrctom::vrm::VrmOptions::new()).expect("実fixtureは変換できる");
    let sidecar =
        PhysicsSidecar::parse(&sidecar文, &PhysicsLimits::new()).expect("実sidecarは検証を通る");
    let 解決 = モデル
        .resolve_physics_sidecar(&sidecar, &fbxバイト列)
        .expect("実fixtureの参照は解決できる");
    // 期待件数は環境変数で上書きできる。既定はUnityヘルパの報告値である。
    let 期待ボーン: usize = std::env::var("VRCTOM_PHYSICS_EXPECT_BONES")
        .ok()
        .and_then(|文| 文.parse().ok())
        .unwrap_or(39);
    let 期待衝突体: usize = std::env::var("VRCTOM_PHYSICS_EXPECT_COLLIDERS")
        .ok()
        .and_then(|文| 文.parse().ok())
        .unwrap_or(11);
    assert_eq!(解決.phys_bones.len(), 期待ボーン, "PhysBone件数が一致する");
    assert_eq!(
        解決.skipped_colliders.len(),
        1,
        "Groundの平面colliderを除外として報告する"
    );
    // body collider相当の解決件数を確かめる。シーン追加由来は報告に残る。
    let 解決済み衝突体 = 解決.colliders.len();
    assert_eq!(解決済み衝突体, 期待衝突体, "collider件数が一致する");
    // 対応表・未解決・座標行列を記録し、完了証拠にする。
    let 対応表: HashMap<&str, usize> = 解決
        .node_map
        .iter()
        .map(|(id, index)| (id.as_str(), *index))
        .collect();
    eprintln!(
        "実sidecar解決: 対応{}件 未解決{}件 座標{:?} 参照モデル{}",
        対応表.len(),
        解決.unmapped_nodes.len(),
        解決.coordinate_transform,
        解決.matched_source_model,
    );
    for 未解決 in &解決.unmapped_nodes {
        eprintln!("未解決: {} {}", 未解決.id, 未解決.name);
    }
}

/// humanoid付きsidecar JSONを作る。基の文に `humanoid` を差し込む。
fn humanoid付きsidecar_json(文: &str, humanoid: serde_json::Value) -> String {
    let mut 値: serde_json::Value = serde_json::from_str(文).expect("合成sidecarは読める");
    値["humanoid"] = humanoid;
    値.to_string()
}

/// humanoidの骨1件分のJSONを作る。回転が `None` の場合は `tPose` を省く。
fn humanoid骨json(骨名: &str, ノード: &str, 回転: Option<[f64; 4]>) -> serde_json::Value {
    let mut 値 = serde_json::json!({"bone": 骨名, "nodeId": ノード});
    if let Some(回転) = 回転 {
        値["tPose"] = serde_json::json!({
            "position": [0.0, 0.0, 0.0],
            "rotation": 回転,
            "scale": [1.0, 1.0, 1.0],
        });
    }
    値
}

/// 単一ノードの対応を作る。モデルもsidecarも `Armature` 1件であり、
/// 座標ゲートは単一の対応で通る。
fn 単一ノードの対応を作る(
    fbxバイト列: &[u8],
    モデルローカル: [f64; 16],
    参照位置: [f64; 3],
) -> (VrmModel, String) {
    let モデル = 合成モデルを作る(&["Armature"], &[None], &[モデルローカル]);
    let ノード = vec![ノードjson(
        "n:0",
        None,
        "Armature",
        &["Milfy", "Armature"],
        Some(出所json("milfy", &["Armature"])),
        参照位置,
    )];
    let 文 = sidecar_json(
        fbxバイト列,
        ノード,
        vec![ボーンjson("pb:0", "n:0", "n:0", &[], &[])],
        Vec::new(),
    );
    (モデル, 文)
}

/// tポーズは差分デルタを右から掛けてローカル回転だけ変える。位置・拡大率は不変である。
#[test]
fn tポーズは差分デルタを右から掛けてローカル回転だけ変える() {
    // モデルは平行移動 [1,2,3]・Z軸90度回転・拡大率 [2,1,0.5] の行列を持つ。
    // sidecarの参照回転は恒等であり、tPoseはUnity空間のY軸90度回転である。
    // δ = tPose、VRM空間ではY軸-90度になる。ローカル行列に回転行列を
    // 右から掛けるため、平行移動は保ち、3x3部だけが回る。
    let モデルローカル = [
        0.0, 2.0, 0.0, 0.0, //
        -1.0, 0.0, 0.0, 0.0, //
        0.0, 0.0, 0.5, 0.0, //
        1.0, 2.0, 3.0, 1.0, //
    ];
    let fbxバイト列 = b"fake-fbx";
    let (mut モデル, 文) =
        単一ノードの対応を作る(fbxバイト列, モデルローカル, [-1.0, 2.0, 3.0]);
    let 回転 = [
        0.0,
        std::f64::consts::FRAC_1_SQRT_2,
        0.0,
        std::f64::consts::FRAC_1_SQRT_2,
    ];
    let humanoid = serde_json::json!({
        "isHuman": true,
        "bones": [humanoid骨json("Hips", "n:0", Some(回転))],
    });
    let sidecar = PhysicsSidecar::from_json(
        &humanoid付きsidecar_json(&文, humanoid),
        &PhysicsLimits::new(),
    )
    .expect("humanoid付きsidecarは検証を通る");
    let 解決 = モデル
        .resolve_physics_sidecar(&sidecar, fbxバイト列)
        .expect("単一ノードは解決できる");
    let 報告: HumanoidPoseReport =
        apply_humanoid_t_pose(&sidecar, &mut モデル, &解決, sidecar.humanoid.as_ref())
            .expect("tポーズは適用できる");
    assert_eq!(報告.applied.len(), 1, "1骨を上書きする");
    assert_eq!(報告.applied[0].bone, "Hips");
    assert_eq!(報告.applied[0].node, 0);
    assert!(報告.skipped.is_empty(), "飛ばしは無い");
    assert_eq!(報告.changed.len(), 1, "差分90度の1骨を記録する");
    assert_eq!(報告.changed[0].bone, "Hips");
    assert!(
        (報告.changed[0].delta_deg - 90.0).abs() < 1e-9,
        "差分は90度である: {}",
        報告.changed[0].delta_deg
    );
    // local * R(Y軸-90度) の合成行列。平行移動は保つ。
    let 期待 = [
        0.0, 0.0, 0.5, 0.0, //
        -1.0, 0.0, 0.0, 0.0, //
        0.0, -2.0, 0.0, 0.0, //
        1.0, 2.0, 3.0, 1.0, //
    ];
    for (i, (実, 想)) in モデル.nodes[0].local.iter().zip(期待.iter()).enumerate() {
        assert!(
            (実 - 想).abs() < 1e-12,
            "ローカル行列が差分合成になる [{i}]: {実} != {想}"
        );
    }
}

/// 差分が無い骨は回転を変えず、変化記録にも残らない。
#[test]
fn 差分が無い骨は回転を変えない() {
    let モデルローカル = [
        0.0, 2.0, 0.0, 0.0, //
        -1.0, 0.0, 0.0, 0.0, //
        0.0, 0.0, 0.5, 0.0, //
        1.0, 2.0, 3.0, 1.0, //
    ];
    let fbxバイト列 = b"fake-fbx";
    let (mut モデル, 文) =
        単一ノードの対応を作る(fbxバイト列, モデルローカル, [-1.0, 2.0, 3.0]);
    // 参照回転（恒等）と同じtPoseであり、δは恒等になる。
    let humanoid = serde_json::json!({
        "isHuman": true,
        "bones": [humanoid骨json("Hips", "n:0", Some([0.0, 0.0, 0.0, 1.0]))],
    });
    let sidecar = PhysicsSidecar::from_json(
        &humanoid付きsidecar_json(&文, humanoid),
        &PhysicsLimits::new(),
    )
    .expect("humanoid付きsidecarは検証を通る");
    let 解決 = モデル
        .resolve_physics_sidecar(&sidecar, fbxバイト列)
        .expect("単一ノードは解決できる");
    let 報告: HumanoidPoseReport =
        apply_humanoid_t_pose(&sidecar, &mut モデル, &解決, sidecar.humanoid.as_ref())
            .expect("tポーズは適用できる");
    assert_eq!(報告.applied.len(), 1, "上書き自体は行う");
    assert!(報告.changed.is_empty(), "変化記録は無い");
    for (i, (実, 想)) in モデルローカル
        .iter()
        .zip(モデル.nodes[0].local.iter())
        .enumerate()
    {
        assert!(
            (実 - 想).abs() < 1e-12,
            "ローカル行列は変わらない [{i}]: {実} != {想}"
        );
    }
}

/// 未解決ノードとT-pose無し骨は記録して飛ばす。モデルは変えない。
#[test]
fn 未解決とtポーズ無しは記録して飛ばす() {
    let モデルローカル = 単位行列();
    let fbxバイト列 = b"fake-fbx";
    let (mut モデル, 文) =
        単一ノードの対応を作る(fbxバイト列, モデルローカル, [0.0, 0.0, 0.0]);
    let humanoid = serde_json::json!({
        "isHuman": true,
        "bones": [
            humanoid骨json("Hips", "n:9", Some([0.0, 0.0, 0.0, 1.0])),
            humanoid骨json("Spine", "n:0", None),
        ],
    });
    // `n:9` はsidecarに無いため検証で失敗する。適用の飛ばしを確かめるには
    // 検証を通す必要があり、ここでは検証の失敗自体を固定する。
    let 結果 = PhysicsSidecar::from_json(
        &humanoid付きsidecar_json(&文, humanoid),
        &PhysicsLimits::new(),
    );
    assert_eq!(
        結果
            .expect_err("未知ノードのhumanoidは検証で失敗する")
            .kind(),
        VrmErrorKind::InvalidPhysicsSidecar
    );
    // T-pose無し骨だけのsidecarは検証を通り、適用で飛ばして記録する。
    let humanoid = serde_json::json!({
        "isHuman": true,
        "bones": [humanoid骨json("Spine", "n:0", None)],
    });
    let sidecar = PhysicsSidecar::from_json(
        &humanoid付きsidecar_json(&文, humanoid),
        &PhysicsLimits::new(),
    )
    .expect("T-pose無し骨は検証を通る");
    let 解決 = モデル
        .resolve_physics_sidecar(&sidecar, fbxバイト列)
        .expect("単一ノードは解決できる");
    let 報告: HumanoidPoseReport =
        apply_humanoid_t_pose(&sidecar, &mut モデル, &解決, sidecar.humanoid.as_ref())
            .expect("T-pose無し骨は飛ばす");
    assert!(報告.applied.is_empty(), "上書きは無い");
    assert_eq!(報告.skipped.len(), 1, "1骨を飛ばす");
    assert_eq!(報告.skipped[0].bone, "Spine");
    assert_eq!(報告.skipped[0].reason, SkippedHumanoidReason::MissingTPose);
    assert_eq!(モデル.nodes[0].local, 単位行列(), "モデルは変わらない");
}

/// 同じノードへの重複は失敗し、部分更新しない。
#[test]
fn 重複ノードのtポーズは失敗して部分更新しない() {
    let モデルローカル = 単位行列();
    let fbxバイト列 = b"fake-fbx";
    let (mut モデル, 文) =
        単一ノードの対応を作る(fbxバイト列, モデルローカル, [0.0, 0.0, 0.0]);
    let humanoid = serde_json::json!({
        "isHuman": true,
        "bones": [
            humanoid骨json("Hips", "n:0", Some([0.0, 0.0, 0.0, 1.0])),
            humanoid骨json("Spine", "n:0", Some([0.0, 0.0, 0.0, 1.0])),
        ],
    });
    let sidecar = PhysicsSidecar::from_json(
        &humanoid付きsidecar_json(&文, humanoid),
        &PhysicsLimits::new(),
    )
    .expect("重複自体は検証を通る");
    let 解決 = モデル
        .resolve_physics_sidecar(&sidecar, fbxバイト列)
        .expect("単一ノードは解決できる");
    let エラー = apply_humanoid_t_pose(&sidecar, &mut モデル, &解決, sidecar.humanoid.as_ref())
        .expect_err("重複ノードは失敗する");
    assert_eq!(エラー.kind(), VrmErrorKind::InvalidPhysicsSidecar);
    assert_eq!(モデル.nodes[0].local, 単位行列(), "部分更新しない");
}

/// humanoid無しの旧sidecarでは何も起きない。
#[test]
fn humanoid無しの旧sidecarでは何もしない() {
    let モデルローカル = 単位行列();
    let fbxバイト列 = b"fake-fbx";
    let (mut モデル, 文) =
        単一ノードの対応を作る(fbxバイト列, モデルローカル, [0.0, 0.0, 0.0]);
    let sidecar =
        PhysicsSidecar::from_json(&文, &PhysicsLimits::new()).expect("旧sidecarは検証を通る");
    assert!(sidecar.humanoid.is_none(), "humanoidが無い");
    let 解決 = モデル
        .resolve_physics_sidecar(&sidecar, fbxバイト列)
        .expect("単一ノードは解決できる");
    let 報告: HumanoidPoseReport =
        apply_humanoid_t_pose(&sidecar, &mut モデル, &解決, sidecar.humanoid.as_ref())
            .expect("humanoid無しでも成功する");
    assert!(報告.applied.is_empty(), "上書きは無い");
    assert!(報告.skipped.is_empty(), "飛ばしは無い");
    assert_eq!(モデル.nodes[0].local, 単位行列(), "モデルは変わらない");
}

/// ゼロ回転のT-poseは検証で失敗する。
#[test]
fn ゼロ回転のtポーズは検証で失敗する() {
    let fbxバイト列 = b"fake-fbx";
    let (_, 文) = 単一ノードの対応を作る(fbxバイト列, 単位行列(), [0.0, 0.0, 0.0]);
    let humanoid = serde_json::json!({
        "isHuman": true,
        "bones": [humanoid骨json("Hips", "n:0", Some([0.0, 0.0, 0.0, 0.0]))],
    });
    let エラー = PhysicsSidecar::from_json(
        &humanoid付きsidecar_json(&文, humanoid),
        &PhysicsLimits::new(),
    )
    .expect_err("ゼロ回転は検証で失敗する");
    assert_eq!(エラー.kind(), VrmErrorKind::InvalidPhysicsSidecar);
}

/// GLBバイト列からJSON塊を取り出す。
fn glbのjsonを取る(バイト列: &[u8]) -> serde_json::Value {
    assert!(バイト列.len() >= 20, "GLBは短すぎない");
    assert_eq!(&バイト列[..4], b"glTF", "GLBである");
    let json長 =
        u32::from_le_bytes(バイト列[12..16].try_into().expect("塊長は4バイトである")) as usize;
    let json = &バイト列[20..20 + json長];
    let json終端 = json
        .iter()
        .rposition(|b| *b != b' ')
        .map(|位置| 位置 + 1)
        .unwrap_or(0);
    serde_json::from_slice(&json[..json終端]).expect("JSON塊はJSONとして読める")
}

/// 列優先行列の回転部を単位四元数 `[x, y, z, w]` で取り出す。
/// TRS合成が前提であり、列長がゼロ・反射・非有限の場合は `None` である。
fn 行列から回転を求める(行列: &[f64; 16]) -> Option<[f64; 4]> {
    let mut 回転 = [[0.0; 3]; 3];
    for 列 in 0..3 {
        let 長さ = (行列[列 * 4] * 行列[列 * 4]
            + 行列[列 * 4 + 1] * 行列[列 * 4 + 1]
            + 行列[列 * 4 + 2] * 行列[列 * 4 + 2])
            .sqrt();
        if !長さ.is_finite() || 長さ <= 0.0 {
            return None;
        }
        for 行 in 0..3 {
            回転[行][列] = 行列[列 * 4 + 行] / 長さ;
        }
    }
    let 行列式 = 回転[0][0] * (回転[1][1] * 回転[2][2] - 回転[1][2] * 回転[2][1])
        - 回転[0][1] * (回転[1][0] * 回転[2][2] - 回転[1][2] * 回転[2][0])
        + 回転[0][2] * (回転[1][0] * 回転[2][1] - 回転[1][1] * 回転[2][0]);
    if !行列式.is_finite() || 行列式 <= 0.0 {
        return None;
    }
    let 跡 = 回転[0][0] + 回転[1][1] + 回転[2][2];
    let 四元数 = if 跡 > 0.0 {
        let s = (跡 + 1.0).sqrt() * 2.0;
        [
            (回転[2][1] - 回転[1][2]) / s,
            (回転[0][2] - 回転[2][0]) / s,
            (回転[1][0] - 回転[0][1]) / s,
            0.25 * s,
        ]
    } else if 回転[0][0] > 回転[1][1] && 回転[0][0] > 回転[2][2] {
        let s = (1.0 + 回転[0][0] - 回転[1][1] - 回転[2][2]).sqrt() * 2.0;
        [
            0.25 * s,
            (回転[0][1] + 回転[1][0]) / s,
            (回転[0][2] + 回転[2][0]) / s,
            (回転[2][1] - 回転[1][2]) / s,
        ]
    } else if 回転[1][1] > 回転[2][2] {
        let s = (1.0 + 回転[1][1] - 回転[0][0] - 回転[2][2]).sqrt() * 2.0;
        [
            (回転[0][1] + 回転[1][0]) / s,
            0.25 * s,
            (回転[1][2] + 回転[2][1]) / s,
            (回転[0][2] - 回転[2][0]) / s,
        ]
    } else {
        let s = (1.0 + 回転[2][2] - 回転[0][0] - 回転[1][1]).sqrt() * 2.0;
        [
            (回転[0][2] + 回転[2][0]) / s,
            (回転[1][2] + 回転[2][1]) / s,
            0.25 * s,
            (回転[1][0] - 回転[0][1]) / s,
        ]
    };
    if 四元数.iter().all(|v| v.is_finite()) {
        Some(四元数)
    } else {
        None
    }
}

/// 2つの四元数の角度差（ラジアン）。符号は同一視し、
/// 正規化してから比べる（f32由来の丸めによる見かけの差を除く）。
fn 四元数の角度差(甲: [f64; 4], 乙: [f64; 4]) -> f64 {
    let 正規化 = |q: [f64; 4]| {
        let 長さ2 = q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3];
        if 長さ2 < 1e-8 {
            return None;
        }
        let 長さ = 長さ2.sqrt();
        Some([q[0] / 長さ, q[1] / 長さ, q[2] / 長さ, q[3] / 長さ])
    };
    let (Some(甲), Some(乙)) = (正規化(甲), 正規化(乙)) else {
        return f64::NAN;
    };
    let 内積 = (甲[0] * 乙[0] + 甲[1] * 乙[1] + 甲[2] * 乙[2] + 甲[3] * 乙[3]).abs();
    2.0 * 内積.clamp(0.0, 1.0).acos()
}

/// 位置・回転・拡大率から列優先の4x4行列を作る。`T * R * S` の順である。
fn trs行列を作る(位置: [f64; 3], 回転: [f64; 4], 拡大率: [f64; 3]) -> [f64; 16] {
    let (x, y, z, w) = (回転[0], 回転[1], 回転[2], 回転[3]);
    let (sx, sy, sz) = (拡大率[0], 拡大率[1], 拡大率[2]);
    let r00 = 1.0 - 2.0 * (y * y + z * z);
    let r10 = 2.0 * (x * y + z * w);
    let r20 = 2.0 * (x * z - y * w);
    let r01 = 2.0 * (x * y - z * w);
    let r11 = 1.0 - 2.0 * (x * x + z * z);
    let r21 = 2.0 * (y * z + x * w);
    let r02 = 2.0 * (x * z + y * w);
    let r12 = 2.0 * (y * z - x * w);
    let r22 = 1.0 - 2.0 * (x * x + y * y);
    [
        r00 * sx,
        r10 * sx,
        r20 * sx,
        0.0, //
        r01 * sy,
        r11 * sy,
        r21 * sy,
        0.0, //
        r02 * sz,
        r12 * sz,
        r22 * sz,
        0.0, //
        位置[0],
        位置[1],
        位置[2],
        1.0, //
    ]
}

/// 行列積 `a * b` を返す。列ベクトル規約であり、`b` を先に適用する。
fn 行列を掛ける(a: &[f64; 16], b: &[f64; 16]) -> [f64; 16] {
    let mut 出力 = [0.0; 16];
    for 行 in 0..4 {
        for 列 in 0..4 {
            let mut 和 = 0.0;
            for k in 0..4 {
                和 += a[k * 4 + 行] * b[列 * 4 + k];
            }
            出力[列 * 4 + 行] = 和;
        }
    }
    出力
}

/// 実fixtureのtポーズ適用後、Unity出力と姿勢・件数が一致する。
///
/// `VRCTOM_FBX_FIXTURE`・`VRCTOM_PHYSICS_SIDECAR`・`VRCTOM_UNITY_VRM` が
/// 指すファイルだけを読み、未設定なら跳ばす。跳ばした場合は完了証拠にしない。
/// 差分δ（`inverse(reference) * tPose`）を右から掛けて適用し、
/// Hips のフレーム差を保ったまま親指だけを正しく回す。
/// 全共通ノードの絶対ワールド回転差が0.01度以下であることを確かめる。
/// 例外はルートのメッシュノード群と `Armature`（軸変換の表現差）だけであり、
/// 根元相対ではなく絶対で比べ、90度の全体回転を隠さない。
/// 件数（spring 45・joint 148・collider 12・
/// center 43・limit 71・constraint 8・targetNames 57・meta 12）も不変である。
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn 実fixtureのtポーズはunity出力と一致する() {
    let (Some(fbx経路), Some(sidecar経路), Some(unity経路)) = (
        std::env::var_os("VRCTOM_FBX_FIXTURE"),
        std::env::var_os("VRCTOM_PHYSICS_SIDECAR"),
        std::env::var_os("VRCTOM_UNITY_VRM"),
    ) else {
        eprintln!("VRCTOM_FBX_FIXTURE/VRCTOM_PHYSICS_SIDECAR/VRCTOM_UNITY_VRM未設定のため跳ばす");
        return;
    };
    let fbxバイト列 = std::fs::read(&fbx経路).expect("実fixtureは読める");
    let sidecar文 = std::fs::read(&sidecar経路).expect("実sidecarは読める");
    let 解析 = FbxFile::parse(&fbxバイト列, &FbxOptions::new()).expect("実fixtureは読める");
    let 選択 = VrmOptions::new().with_expressions(VrmExpressionProfile::vrchat());
    let (mut モデル, _) = VrmModel::from_fbx(&解析, &選択).expect("実fixtureは変換できる");
    let sidecar =
        PhysicsSidecar::parse(&sidecar文, &PhysicsLimits::new()).expect("実sidecarは検証を通る");
    let humanoid記録 = sidecar
        .humanoid
        .as_ref()
        .expect("実sidecarにhumanoidがある");
    assert_eq!(humanoid記録.bones.len(), 53, "humanoidは53骨である");
    let 解決 = モデル
        .resolve_physics_sidecar(&sidecar, &fbxバイト列)
        .expect("実fixtureの参照は解決できる");
    // CLIと同じ順序であり、視線の測定とスプリングの生成より前に適用する。
    let 報告 = apply_humanoid_t_pose(&sidecar, &mut モデル, &解決, Some(humanoid記録))
        .expect("実fixtureのT-poseは適用できる");
    assert_eq!(報告.applied.len(), 53, "53骨を上書きする");
    assert!(報告.skipped.is_empty(), "飛ばしは無い");
    // 差分δが0.01度以上の骨は親指の4骨だけである。bind-pose の sidecar では
    // 肩・中指の参照姿勢が T-pose と一致するため、旧 snapshot の8件にはならない。
    // （旧 snapshot は edit-mode 取得であり、肩 0.018度・中指 0.02度があった。）
    let mut 変化名: Vec<&str> = 報告.changed.iter().map(|骨| 骨.bone.as_str()).collect();
    変化名.sort_unstable();
    assert_eq!(
        変化名,
        [
            "LeftThumbIntermediate",
            "LeftThumbProximal",
            "RightThumbIntermediate",
            "RightThumbProximal",
        ],
        "差分0.01度以上は親指の4骨である",
    );
    for 変化 in &報告.changed {
        let 期待 = if 変化.bone.contains("Intermediate") {
            3.4068
        } else {
            7.2746
        };
        assert!(
            (変化.delta_deg - 期待).abs() < 1e-3,
            "差分角度が既知値と一致する: {} = {}",
            変化.bone,
            変化.delta_deg
        );
    }
    eprintln!(
        "tポーズ差分: {}件 {}",
        報告.changed.len(),
        報告
            .changed
            .iter()
            .map(|骨| format!("{}(δ={:.4}度)", 骨.bone, 骨.delta_deg))
            .collect::<Vec<_>>()
            .join(" "),
    );
    // Unity製VRMを読む。ノードはTRSであり、名前は重ならない。
    let unityバイト列 = std::fs::read(&unity経路).expect("Unity製VRMは読める");
    let unity_json = glbのjsonを取る(&unityバイト列);
    let unityノード = unity_json["nodes"]
        .as_array()
        .expect("Unity製VRMにノードがある");
    let mut unity索引: HashMap<&str, usize> = HashMap::new();
    for (i, ノード) in unityノード.iter().enumerate() {
        let 名 = ノード["name"].as_str().expect("ノード名は文字列である");
        assert!(
            unity索引.insert(名, i).is_none(),
            "Unityのノード名は重ならない: {名}"
        );
    }
    let unity骨 = unity_json["extensions"]["VRMC_vrm"]["humanoid"]["humanBones"]
        .as_object()
        .expect("Unity製VRMにhumanoidがある");
    assert_eq!(unity骨.len(), 53, "Unityのhumanoidは53骨である");
    // 親指のワールド一致は世界行列の組み立て後に確かめる（後段）。
    // 全共通ノードの絶対ワールド回転差が0.01度以下である。差分δの適用は
    // 参照実装と同じ姿勢変化のため、根元相対ではなく絶対で比べ、
    // 90度の全体回転を隠さない。自モデルの親なしノード
    // （メッシュ所有ノード群と `Armature` 自体）は軸変換の表現差のため除外し、
    // 名前だけ報告する。メッシュ所有ノードは頂点ベイクで切り離されており、
    // 親変換の影響を受けない対応である。
    let mut unity親 = vec![None; unityノード.len()];
    for (i, ノード) in unityノード.iter().enumerate() {
        for 子 in ノード
            .get("children")
            .and_then(|v| v.as_array())
            .map(Vec::as_slice)
            .unwrap_or(&[])
        {
            unity親[子.as_u64().expect("子は数である") as usize] = Some(i);
        }
    }
    let mut unity世界 = vec![[0.0; 16]; unityノード.len()];
    let mut unity済 = vec![false; unityノード.len()];
    for 開始 in 0..unityノード.len() {
        let mut 鎖 = Vec::new();
        let mut 現在 = 開始;
        loop {
            if unity済[現在] {
                break;
            }
            鎖.push(現在);
            match unity親[現在] {
                Some(親) => 現在 = 親,
                None => break,
            }
        }
        for &節点 in 鎖.iter().rev() {
            let ノード = &unityノード[節点];
            let 位置 = ノード
                .get("translation")
                .and_then(|v| v.as_array())
                .map(|v| {
                    [
                        v[0].as_f64().unwrap_or(0.0),
                        v[1].as_f64().unwrap_or(0.0),
                        v[2].as_f64().unwrap_or(0.0),
                    ]
                })
                .unwrap_or([0.0; 3]);
            let 回転 = ノード
                .get("rotation")
                .and_then(|v| v.as_array())
                .map(|v| {
                    [
                        v[0].as_f64().unwrap_or(0.0),
                        v[1].as_f64().unwrap_or(0.0),
                        v[2].as_f64().unwrap_or(0.0),
                        v[3].as_f64().unwrap_or(1.0),
                    ]
                })
                .unwrap_or([0.0, 0.0, 0.0, 1.0]);
            let 拡大率 = ノード
                .get("scale")
                .and_then(|v| v.as_array())
                .map(|v| {
                    [
                        v[0].as_f64().unwrap_or(1.0),
                        v[1].as_f64().unwrap_or(1.0),
                        v[2].as_f64().unwrap_or(1.0),
                    ]
                })
                .unwrap_or([1.0; 3]);
            let ローカル = trs行列を作る(位置, 回転, 拡大率);
            unity世界[節点] = match unity親[節点] {
                Some(親) => 行列を掛ける(&unity世界[親], &ローカル),
                None => ローカル,
            };
            unity済[節点] = true;
        }
    }
    let mut 自世界 = vec![[0.0; 16]; モデル.nodes.len()];
    let mut 自済 = vec![false; モデル.nodes.len()];
    for 開始 in 0..モデル.nodes.len() {
        let mut 鎖 = Vec::new();
        let mut 現在 = 開始;
        loop {
            if 自済[現在] {
                break;
            }
            鎖.push(現在);
            match モデル.nodes[現在].parent {
                Some(親) => 現在 = 親,
                None => break,
            }
        }
        for &節点 in 鎖.iter().rev() {
            自世界[節点] = match モデル.nodes[節点].parent {
                Some(親) => 行列を掛ける(&自世界[親], &モデル.nodes[節点].local),
                None => 行列を掛ける(&モデル.root, &モデル.nodes[節点].local),
            };
            自済[節点] = true;
        }
    }
    let mut 共通 = 0usize;
    let mut 除外 = Vec::new();
    let mut 最大度 = 0.0f64;
    let mut 超過 = Vec::new();
    // 絶対ワールド回転で比べる。差分δの適用は参照実装と同じ姿勢変化であり、
    // 根元の表現差を超えて全体が90度回ることは無い。
    for (i, ノード) in モデル.nodes.iter().enumerate() {
        let Some(&unity節点) = unity索引.get(ノード.name.as_str()) else {
            continue;
        };
        共通 += 1;
        if ノード.parent.is_none() {
            除外.push(ノード.name.clone());
            continue;
        }
        let 自回転 = 行列から回転を求める(&自世界[i])
            .unwrap_or_else(|| panic!("自世界の回転が取れない: {}", ノード.name));
        let 他回転 = 行列から回転を求める(&unity世界[unity節点])
            .unwrap_or_else(|| panic!("Unity世界の回転が取れない: {}", ノード.name));
        let 度 = 四元数の角度差(自回転, 他回転).to_degrees();
        最大度 = 最大度.max(度);
        if 度 > 0.01 {
            超過.push((ノード.name.clone(), 度));
        }
    }
    assert!(超過.is_empty(), "ワールド回転差0.01度超えが残る: {超過:?}");
    eprintln!(
        "ワールド一致: 共通{共通}件 最大{最大度:.6}度 除外{}件({})",
        除外.len(),
        除外.join(","),
    );
    // 親指の絶対ワールド回転差が0.01度以下になる。差分δの適用対象であり、
    // Unity出力（T-pose 適用済み）と一致しなければならない。
    let mut 親指最大度 = 0.0f64;
    for 骨名 in [
        "LeftThumbProximal",
        "LeftThumbIntermediate",
        "RightThumbProximal",
        "RightThumbIntermediate",
    ] {
        let 骨 = humanoid記録
            .bones
            .iter()
            .find(|骨| 骨.bone == 骨名)
            .unwrap_or_else(|| panic!("親指の骨が無い: {骨名}"));
        let 自節点 = 解決
            .node_map
            .get(骨.node_id.as_str())
            .unwrap_or_else(|| panic!("親指の対応が無い: {骨名}"));
        let unity節点 = unity索引
            .get(モデル.nodes[*自節点].name.as_str())
            .unwrap_or_else(|| panic!("Unityに親指の節点が無い: {骨名}"));
        let 自回転 = 行列から回転を求める(&自世界[*自節点])
            .unwrap_or_else(|| panic!("自世界の回転が取れない: {骨名}"));
        let 他回転 = 行列から回転を求める(&unity世界[*unity節点])
            .unwrap_or_else(|| panic!("Unity世界の回転が取れない: {骨名}"));
        let 度 = 四元数の角度差(自回転, 他回転).to_degrees();
        親指最大度 = 親指最大度.max(度);
        assert!(
            度 <= 0.01,
            "親指のワールド回転差が0.01度以下である: {骨名} ({度}度)"
        );
    }
    eprintln!("親指ワールド一致: 4骨の最大角度差 {親指最大度:.6}度");
    // スプリング・視線・書き出しはCLIと同じ順序で行い、件数の不変を確かめる。
    let humanoid = HumanoidBones::detect(&モデル).expect("実fixtureは対応付けられる");
    let (視線, 視線報告) = resolve_look_at(&sidecar, &モデル, &解決, &humanoid)
        .expect("実fixtureの視線は解決できる")
        .expect("視線記録があるため解決できる");
    let コンパイル済み =
        compile_springs(&sidecar, &mut モデル, &解決, &SpringCompileOptions::new())
            .expect("実fixtureの鎖はコンパイルできる");
    let 関節数: usize = コンパイル済み
        .bones
        .springs
        .iter()
        .map(|s| s.joints.len())
        .sum();
    let 制限数: usize = コンパイル済み
        .bones
        .springs
        .iter()
        .flat_map(|s| s.joints.iter())
        .filter(|j| j.limit.is_some())
        .count();
    モデル.look_at = Some(視線);
    モデル.spring_bones = コンパイル済み.bones.clone();
    let 設定 = VrmExportOptions::new(VrmMeta::new("試験", vec!["作者".to_string()]));
    let バイト列 = モデル.to_vrm(&設定).expect("実fixtureは書き出せる");
    let json = glbのjsonを取る(&バイト列);
    let mut 対象名数 = 0usize;
    for メッシュ in json["meshes"].as_array().expect("メッシュは配列である") {
        if let Some(名前列) = メッシュ
            .get("extras")
            .and_then(|v| v.get("targetNames"))
            .and_then(|v| v.as_array())
        {
            対象名数 += 名前列.len();
        }
    }
    let meta数 = json["extensions"]["VRMC_vrm"]["meta"]
        .as_object()
        .expect("metaがある")
        .len();
    // 視線はT-pose適用後の測定値であり、Unity出力と突き合わせる。
    // 頭鎖の骨はT-pose差分が無いため、適用前後で値は変わらない。
    let 出力視線 = json["extensions"]["VRMC_vrm"]["lookAt"]
        .as_object()
        .expect("lookAtがある");
    let 期待視線 = unity_json["extensions"]["VRMC_vrm"]["lookAt"]
        .as_object()
        .expect("Unity製VRMにlookAtがある");
    assert_eq!(
        出力視線["type"],
        serde_json::json!("bone"),
        "種別はboneである"
    );
    let 出ずれ = 出力視線["offsetFromHeadBone"]
        .as_array()
        .expect("ずれは配列である");
    let 想ずれ = 期待視線["offsetFromHeadBone"]
        .as_array()
        .expect("ずれは配列である");
    for (番号, (実, 想)) in 出ずれ.iter().zip(想ずれ.iter()).enumerate() {
        let (実, 想) = (
            実.as_f64().expect("要素は数である"),
            想.as_f64().expect("要素は数である"),
        );
        assert!(
            (実 - 想).abs() <= 1e-6 + 1e-4 * 想.abs(),
            "ずれ[{番号}]が一致する: 実={実} 想={想}"
        );
    }
    // 視線の4値は差分δ適用後の測定値であり、Unity出力と突き合わせる。
    // 差分は親指だけのため頭鎖の測定は変わらず、Unityと一致する。
    for 名 in [
        "rangeMapHorizontalInner",
        "rangeMapHorizontalOuter",
        "rangeMapVerticalDown",
        "rangeMapVerticalUp",
    ] {
        for 項目 in ["inputMaxValue", "outputScale"] {
            let 実 = 出力視線[名][項目].as_f64().expect("値は数である");
            let 想 = 期待視線[名][項目].as_f64().expect("値は数である");
            assert!(
                (実 - 想).abs() <= 1e-6 + 1e-4 * 想.abs(),
                "{名}.{項目}が一致する: 実={実} 想={想}"
            );
        }
    }
    // 関節パラメータの一致率。T-pose適用はモデル回転だけを変え、
    // 関節値の入力（sidecar世界）は変わらないため、適用前後で同一のはずである。
    // 許容差は既存試験と同じ abs 1e-5 + rel 1e-4 であり、重力方向は完全一致とする。
    let unity_springs = unity_json["extensions"]["VRMC_springBone"]["springs"]
        .as_array()
        .expect("Unity製VRMにspringがある");
    let mut unity表: HashMap<&str, &serde_json::Value> = HashMap::new();
    for spring in unity_springs {
        let 名前 = spring["name"].as_str().expect("spring名は文字列である");
        assert!(
            unity表.insert(名前, spring).is_none(),
            "Unityのspring名は重ならない: {名前}"
        );
    }
    let 許容 = |実: f64, 想: f64| (実 - 想).abs() <= 1e-5 + 1e-4 * 想.abs();
    let mut 一致 = 0usize;
    let mut 全体 = 0usize;
    for spring in &コンパイル済み.bones.springs {
        let 他 = unity表
            .get(spring.name.as_str())
            .unwrap_or_else(|| panic!("Unity出力にspringが無い: {}", spring.name));
        let 他関節 = 他["joints"].as_array().expect("Unityの関節は配列である");
        assert_eq!(他関節.len(), spring.joints.len(), "関節数は一致する");
        for (関節, 他関節) in spring.joints.iter().zip(他関節.iter()) {
            for (名, 実) in [
                ("stiffness", f64::from(関節.stiffness)),
                ("dragForce", f64::from(関節.drag_force)),
                ("gravityPower", f64::from(関節.gravity_power)),
                ("hitRadius", f64::from(関節.hit_radius)),
            ] {
                全体 += 1;
                let 想 = 他関節[名]
                    .as_f64()
                    .unwrap_or_else(|| panic!("Unityの値が数でない: {名}"));
                if 許容(実, 想) {
                    一致 += 1;
                }
            }
            全体 += 1;
            let 他向き = 他関節["gravityDir"]
                .as_array()
                .expect("Unityの重力方向は配列である");
            if 他向き.len() == 3
                && (f64::from(関節.gravity_dir[0]) - 他向き[0].as_f64().unwrap_or(f64::NAN)).abs()
                    == 0.0
                && (f64::from(関節.gravity_dir[1]) - 他向き[1].as_f64().unwrap_or(f64::NAN)).abs()
                    == 0.0
                && (f64::from(関節.gravity_dir[2]) - 他向き[2].as_f64().unwrap_or(f64::NAN)).abs()
                    == 0.0
            {
                一致 += 1;
            }
        }
    }
    eprintln!(
        "T-pose後件数: spring{}件 関節{}件 collider{}件 center{}件 limit{}件 constraint{}件 targetNames{}件 meta{}件 {}バイト 関節値一致{一致}/{全体}件 視線({} 内{:.3} 外{:.3} 下{:.3} 上{:.3} ずれ{:?})",
        コンパイル済み.report.spring_count,
        関節数,
        コンパイル済み.report.collider_count,
        コンパイル済み.report.center_applied.len(),
        制限数,
        モデル.node_constraints.len(),
        対象名数,
        meta数,
        バイト列.len(),
        視線報告.look_type,
        視線報告.range.horizontal_inner,
        視線報告.range.horizontal_outer,
        視線報告.range.vertical_down,
        視線報告.range.vertical_up,
        視線報告.offset_from_head,
    );
    assert_eq!(コンパイル済み.report.spring_count, 45);
    assert_eq!(関節数, 148);
    assert_eq!(コンパイル済み.report.collider_count, 12);
    assert_eq!(コンパイル済み.report.center_applied.len(), 43);
    assert_eq!(制限数, 71);
    assert_eq!(モデル.node_constraints.len(), 8);
    assert_eq!(対象名数, 57);
    assert_eq!(meta数, 12);
}
