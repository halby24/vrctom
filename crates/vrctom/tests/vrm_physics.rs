//! 物理sidecarの読み取りと対応解決の契約テスト（公開 API のみ使う）。
//!
//! 合成した `VrmModel` と合成sidecarで、完全パス一致・左右の区別・
//! シーン追加ノード・検証失敗・座標ゲートを固定する。
//! 実fixtureを使う検証は `VRCTOM_FBX_FIXTURE` と `VRCTOM_PHYSICS_SIDECAR` が
//! 指すファイルだけを読み、未設定なら跳ばす。跳ばした場合は完了証拠にしない。

use std::collections::HashMap;

use vrctom::vrm::{
    PhysicsLimits, PhysicsResolution, PhysicsSidecar, VrmErrorKind, VrmModel, VrmNode,
    VrmSpringBones,
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
