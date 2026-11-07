//! 共通物理適用関数の契約テスト（公開 API のみ使う）。
//!
//! 合成した `VrmModel` と合成sidecarで、解決・T-pose・視線・スプリング生成の
//! 順序と報告の集計・要約文面を固定する。失敗時はモデルを部分更新しない。

use vrctom::vrm::{
    HumanoidBone, HumanoidBones, PhysicsLimits, PhysicsSidecar, SpringCompileOptions, VrmErrorKind,
    VrmExpressions, VrmModel, VrmNode, VrmSpringBones, apply_physics_sidecar,
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

/// 書き出し可能な合成モデルを作る。humanoid必須15骨と髪の鎖を持つ。
fn 書き出し可能モデルを作る() -> VrmModel {
    let 名前 = [
        "Hips",
        "Spine",
        "Head",
        "HairA",
        "HairB",
        "HairC",
        "LeftUpperArm",
        "LeftLowerArm",
        "LeftHand",
        "RightUpperArm",
        "RightLowerArm",
        "RightHand",
        "LeftUpperLeg",
        "LeftLowerLeg",
        "LeftFoot",
        "RightUpperLeg",
        "RightLowerLeg",
        "RightFoot",
    ];
    let 親: Vec<Option<usize>> = vec![
        None,
        Some(0),
        Some(1),
        Some(2),
        Some(3),
        Some(3),
        Some(1),
        Some(6),
        Some(7),
        Some(1),
        Some(9),
        Some(10),
        Some(0),
        Some(12),
        Some(13),
        Some(0),
        Some(15),
        Some(16),
    ];
    let 位置 = [
        [0.0, 0.0, 0.0],
        [0.0, 0.1, 0.0],
        [0.0, 0.2, 0.0],
        [0.0, 0.1, -0.1],
        [0.0, 0.1, -0.1],
        [0.0, 0.1, -0.1],
        [0.1, 0.0, 0.0],
        [0.1, 0.0, 0.0],
        [0.1, 0.0, 0.0],
        [-0.1, 0.0, 0.0],
        [-0.1, 0.0, 0.0],
        [-0.1, 0.0, 0.0],
        [0.1, -0.1, 0.0],
        [0.0, -0.1, 0.0],
        [0.0, -0.1, 0.0],
        [-0.1, -0.1, 0.0],
        [0.0, -0.1, 0.0],
        [0.0, -0.1, 0.0],
    ];
    let mut 子: Vec<Vec<usize>> = vec![Vec::new(); 名前.len()];
    for (子索引, 親索引) in 親.iter().enumerate() {
        if let Some(親索引) = 親索引 {
            子[*親索引].push(子索引);
        }
    }
    let ノード = 名前
        .iter()
        .enumerate()
        .map(|(索引, 名)| VrmNode {
            name: (*名).to_string(),
            parent: 親[索引],
            children: 子[索引].clone(),
            local: 平行移動行列(位置[索引]),
        })
        .collect();
    VrmModel {
        root: 単位行列(),
        nodes: ノード,
        skins: Vec::new(),
        meshes: Vec::new(),
        expressions: VrmExpressions::default(),
        spring_bones: VrmSpringBones::default(),
        node_constraints: Vec::new(),
        look_at: None,
    }
}

/// モデル位置から参照位置を求める。今回は同一値でゲートを通す。
fn 参照位置(モデル: &VrmModel, 索引: usize) -> [f64; 3] {
    let 局所 = &モデル.nodes[索引].local;
    [局所[12], 局所[13], 局所[14]]
}

/// sidecarノード1件分のJSONを作る。
fn ノードjson(
    モデル: &VrmModel,
    id: &str,
    親: Option<&str>,
    索引: usize,
    現場パス: &[&str],
    出所パス: &[&str],
) -> serde_json::Value {
    let 名 = モデル.nodes[索引].name.clone();
    let 出所 = serde_json::json!({
        "modelId": "milfy",
        "path": 出所パス,
        "assetGuid": "guid",
        "localId": -1,
    });
    let 位置 = 参照位置(モデル, 索引);
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

/// 髪鎖のsidecar JSONを作る。humanoidと視線は持たない旧形式である。
fn 髪sidecarを作る(fbxバイト列: &[u8], モデル: &VrmModel) -> String {
    let ノード = vec![
        ノードjson(モデル, "n:0", None, 0, &["Milfy", "Hips"], &["Hips"]),
        ノードjson(
            モデル,
            "n:1",
            Some("n:0"),
            1,
            &["Milfy", "Hips", "Spine"],
            &["Hips", "Spine"],
        ),
        ノードjson(
            モデル,
            "n:2",
            Some("n:1"),
            2,
            &["Milfy", "Hips", "Spine", "Head"],
            &["Hips", "Spine", "Head"],
        ),
        ノードjson(
            モデル,
            "n:3",
            Some("n:2"),
            3,
            &["Milfy", "Hips", "Spine", "Head", "HairA"],
            &["Hips", "Spine", "Head", "HairA"],
        ),
        ノードjson(
            モデル,
            "n:4",
            Some("n:3"),
            4,
            &["Milfy", "Hips", "Spine", "Head", "HairA", "HairB"],
            &["Hips", "Spine", "Head", "HairA", "HairB"],
        ),
        ノードjson(
            モデル,
            "n:5",
            Some("n:3"),
            5,
            &["Milfy", "Hips", "Spine", "Head", "HairA", "HairC"],
            &["Hips", "Spine", "Head", "HairA", "HairC"],
        ),
    ];
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
        "physBones": [{
            "id": "pb:0",
            "componentNodeId": "n:3",
            "rootNodeId": "n:3",
            "enabled": true,
            "version": "1.1",
            "integrationType": "Simplified",
            "ignoreNodeIds": [],
            "ignoreOtherPhysBones": false,
            "endpointPosition": [0.0, 0.0, 0.0],
            "multiChildType": "First",
            "pull": {"value": 0.0, "keys": [], "preWrapMode": 8, "postWrapMode": 8},
            "spring": {"value": 0.3, "keys": [], "preWrapMode": 8, "postWrapMode": 8},
            "stiffness": {"value": 0.5, "keys": [], "preWrapMode": 8, "postWrapMode": 8},
            "gravity": {"value": 0.0, "keys": [], "preWrapMode": 8, "postWrapMode": 8},
            "gravityFalloff": {"value": 0.0, "keys": [], "preWrapMode": 8, "postWrapMode": 8},
            "immobileType": "AllMotion",
            "immobile": {"value": 0.0, "keys": [], "preWrapMode": 8, "postWrapMode": 8},
            "radius": {"value": 0.02, "keys": [], "preWrapMode": 8, "postWrapMode": 8},
            "allowCollision": true,
            "colliderIds": ["c:0", "c:1"],
            "limitType": "None",
            "maxAngleX": {"value": 0.0, "keys": [], "preWrapMode": 8, "postWrapMode": 8},
            "maxAngleZ": {"value": 0.0, "keys": [], "preWrapMode": 8, "postWrapMode": 8},
            "limitRotation": [0.0, 0.0, 0.0],
            "isAnimated": false,
        }],
        "colliders": [
            {
                "id": "c:0",
                "componentNodeId": "n:4",
                "rootNodeId": "n:4",
                "enabled": true,
                "shapeType": "Sphere",
                "insideBounds": false,
                "radius": 0.05,
                "height": 0.0,
                "position": [0.1, 0.2, 0.3],
                "rotation": [0.0, 0.0, 0.0, 1.0],
                "bonesAsSpheres": false,
            },
            {
                "id": "c:1",
                "componentNodeId": "n:3",
                "rootNodeId": "n:3",
                "enabled": true,
                "shapeType": "Capsule",
                "insideBounds": false,
                "radius": 0.05,
                "height": 0.4,
                "position": [0.1, 0.0, 0.0],
                "rotation": [0.0, 0.0, 0.0, 1.0],
                "bonesAsSpheres": false,
            },
        ],
        "unsupportedFeatures": [],
    })
    .to_string()
}

/// humanoid対応付けを作る。T-pose適用の解決済み対応である。
fn 対応付けを作る() -> HumanoidBones {
    let mut 対応 = HumanoidBones::new();
    for (骨, 索引) in [
        (HumanoidBone::Hips, 0),
        (HumanoidBone::Spine, 1),
        (HumanoidBone::Head, 2),
    ] {
        対応.set(骨, 索引);
    }
    対応
}

/// 合成鎖に共通適用関数を通すと報告と要約が従来形式になる。
#[test]
fn 合成鎖の適用は報告と要約を返す() {
    let fbxバイト列 = b"fake-fbx";
    let mut モデル = 書き出し可能モデルを作る();
    let 文 = 髪sidecarを作る(fbxバイト列, &モデル);
    let sidecar =
        PhysicsSidecar::from_json(&文, &PhysicsLimits::new()).expect("合成sidecarは検証を通る");
    let 報告 = apply_physics_sidecar(
        &mut モデル,
        &sidecar,
        fbxバイト列,
        &対応付けを作る(),
        &SpringCompileOptions::new(),
    )
    .expect("合成鎖は適用できる");
    // 補助ノードは無く、T-poseと視線も無い旧形式である。
    assert!(報告.auxiliary_node_names.is_empty());
    assert_eq!(報告.auxiliary_message(), "物理: 補助ノードの追加は無い");
    assert_eq!(報告.tpose_applied, 0);
    assert!(報告.tpose_skipped.is_empty());
    assert!(報告.tpose_count_message().is_none());
    assert!(報告.look_at.is_none());
    assert!(報告.look_at_message().is_none());
    // スプリングの件数は従来の直接呼び出しと同じである。
    assert_eq!(報告.spring_count, 1);
    assert_eq!(報告.joint_count, 2);
    assert_eq!(報告.collider_count, 2);
    assert_eq!(報告.group_count, 1);
    assert_eq!(報告.branch_split_count, 1);
    assert_eq!(報告.gravity_zero_joints, 2);
    assert_eq!(報告.gravity_joints, 0);
    assert!((報告.stiffness_mean - 0.0).abs() < 1e-12);
    assert_eq!(報告.constraint_count, 0);
    assert!(報告.skipped_constraints.is_empty());
    // 要約は従来の書式と完全一致する。件数と丸めを固定する。
    let 要約 = 報告.summary();
    assert!(
        要約.starts_with("スプリング1件 関節2件 collider2件 グループ1件 分岐1件"),
        "要約の先頭は件数である: {要約}"
    );
    assert!(
        要約.contains("関節値(硬さ平均0.000 重力あり0件)"),
        "硬さ平均の丸めは第3位である: {要約}"
    );
    assert!(
        要約.ends_with("T-ポーズ0件"),
        "末尾はT-ポーズ件数である: {要約}"
    );
    // モデルは更新され、報告と整合する。
    assert_eq!(モデル.spring_bones.springs.len(), 1);
    assert!(モデル.look_at.is_none());
    assert_eq!(モデル.node_constraints.len(), 報告.constraint_count);
    // 構造化報告はシリアライズできる。
    let 値 = serde_json::to_value(&報告).expect("報告はシリアライズできる");
    assert_eq!(値["spring_count"], serde_json::json!(1));
    assert_eq!(値["tpose_applied"], serde_json::json!(0));
}

/// 失敗時はモデルを部分更新せず段階の見出しを返す。
#[test]
fn 解決失敗はモデルを変えず段階を返す() {
    let fbxバイト列 = b"fake-fbx";
    let mut モデル = 書き出し可能モデルを作る();
    let もとノード数 = モデル.nodes.len();
    let もとローカル: Vec<[f64; 16]> = モデル.nodes.iter().map(|n| n.local).collect();
    // SHA-256が一致しないsidecarは解決で失敗する。
    let mut 文: serde_json::Value =
        serde_json::from_str(&髪sidecarを作る(fbxバイト列, &モデル)).expect("合成sidecarは読める");
    文["sourceModels"][0]["sha256"] = serde_json::json!("0".repeat(64));
    let sidecar =
        PhysicsSidecar::from_json(&文.to_string(), &PhysicsLimits::new()).expect("書式自体は通る");
    let 失敗 = apply_physics_sidecar(
        &mut モデル,
        &sidecar,
        fbxバイト列,
        &対応付けを作る(),
        &SpringCompileOptions::new(),
    )
    .expect_err("SHA不一致は失敗する");
    assert_eq!(失敗.stage(), "物理sidecarの解決に失敗した");
    assert_eq!(失敗.error().kind(), VrmErrorKind::InvalidPhysicsSidecar);
    assert!(失敗.auxiliary_message().is_none());
    assert!(失敗.tpose_skipped().is_empty());
    // モデルは呼び出し前と同じである。
    assert_eq!(モデル.nodes.len(), もとノード数);
    for (ノード, 期待) in モデル.nodes.iter().zip(もとローカル.iter()) {
        assert_eq!(&ノード.local, 期待);
    }
    assert!(モデル.spring_bones.springs.is_empty());
    assert!(モデル.look_at.is_none());
    // Webは内部のVrmErrorに戻せる。
    let 原因: vrctom::vrm::VrmError = 失敗.into_inner();
    assert_eq!(原因.kind(), VrmErrorKind::InvalidPhysicsSidecar);
}

/// T-pose不一致は補助の確定を残して段階を返す。
#[test]
fn 対応不一致は補助まで出して段階を返す() {
    let fbxバイト列 = b"fake-fbx";
    let mut モデル = 書き出し可能モデルを作る();
    let もとノード数 = モデル.nodes.len();
    let 文 = 髪sidecarを作る(fbxバイト列, &モデル);
    // humanoid付きに差し替え、LeftHandの指す先をずらして不一致にする。
    let mut 値: serde_json::Value = serde_json::from_str(&文).expect("合成sidecarは読める");
    値["humanoid"] = serde_json::json!({
        "isHuman": true,
        "bones": [
            {"bone": "Hips", "nodeId": "n:0", "tPose": {"position": [0.0, 0.0, 0.0], "rotation": [0.0, 0.0, 0.0, 1.0], "scale": [1.0, 1.0, 1.0]}},
            {"bone": "LeftHand", "nodeId": "n:2", "tPose": {"position": [0.0, 0.0, 0.0], "rotation": [0.0, 0.0, 0.0, 1.0], "scale": [1.0, 1.0, 1.0]}},
        ],
    });
    let sidecar = PhysicsSidecar::from_json(&値.to_string(), &PhysicsLimits::new())
        .expect("humanoid付きは検証を通る");
    // 解決済みはLeftHandを8番に置き、sidecarのn:2（Head）と食い違わせる。
    let mut 解決済み = 対応付けを作る();
    解決済み.set(HumanoidBone::LeftHand, 8);
    let 失敗 = apply_physics_sidecar(
        &mut モデル,
        &sidecar,
        fbxバイト列,
        &解決済み,
        &SpringCompileOptions::new(),
    )
    .expect_err("対応不一致は失敗する");
    assert_eq!(失敗.stage(), "T-ポーズの適用に失敗した");
    // 補助の確定は呼び出し側の表示順のために残る。
    assert!(失敗.auxiliary_message().is_some());
    // モデル自体は呼び出し前に戻る。
    assert_eq!(モデル.nodes.len(), もとノード数);
    assert!(モデル.spring_bones.springs.is_empty());
}
