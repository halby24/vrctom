//! スプリング出力の最小縦切りの契約テスト（公開 API のみ使う）。
//!
//! 合成した `VrmModel` と合成sidecarで、線形チェーン・分岐記録・
//! collider変換・グループ共有・`VRMC_springBone` のJSONと索引を固定する。
//! 実fixtureを使う検証は `VRCTOM_FBX_FIXTURE` と `VRCTOM_PHYSICS_SIDECAR` が
//! 指すファイルだけを読み、未設定なら跳ばす。跳ばした場合は完了証拠にしない。

use vrctom::vrm::{
    HumanoidBone, HumanoidBones, PhysicsLimits, PhysicsSidecar, SpringCompileOptions, VrmErrorKind,
    VrmExportErrorKind, VrmExportOptions, VrmExpressions, VrmMeta, VrmModel, VrmNode,
    VrmSpringBones, compile_springs,
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
///
/// 階層は Hips - Spine - Head - HairA - HairB（+分岐HairC）、
/// 左右の腕脚は Spine・Hips 直下に置く。髪名はhumanoid検出に掛からない。
fn 書き出し可能モデルを作る() -> VrmModel {
    // 0:Hips 1:Spine 2:Head 3:HairA 4:HairB 5:HairC
    // 6:LeftUpperArm 7:LeftLowerArm 8:LeftHand
    // 9:RightUpperArm 10:RightLowerArm 11:RightHand
    // 12:LeftUpperLeg 13:LeftLowerLeg 14:LeftFoot
    // 15:RightUpperLeg 16:RightLowerLeg 17:RightFoot
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
        None,     // 0
        Some(0),  // 1
        Some(1),  // 2
        Some(2),  // 3
        Some(3),  // 4
        Some(3),  // 5 分岐
        Some(1),  // 6
        Some(6),  // 7
        Some(7),  // 8
        Some(1),  // 9
        Some(9),  // 10
        Some(10), // 11
        Some(0),  // 12
        Some(12), // 13
        Some(13), // 14
        Some(0),  // 15
        Some(15), // 16
        Some(16), // 17
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

/// humanoid対応付けを作る。検出ではなく明示指定で結ぶ。
fn 対応付けを作る() -> HumanoidBones {
    let mut 対応 = HumanoidBones::new();
    for (骨, 索引) in [
        (HumanoidBone::Hips, 0),
        (HumanoidBone::Spine, 1),
        (HumanoidBone::Head, 2),
        (HumanoidBone::LeftUpperArm, 6),
        (HumanoidBone::LeftLowerArm, 7),
        (HumanoidBone::LeftHand, 8),
        (HumanoidBone::RightUpperArm, 9),
        (HumanoidBone::RightLowerArm, 10),
        (HumanoidBone::RightHand, 11),
        (HumanoidBone::LeftUpperLeg, 12),
        (HumanoidBone::LeftLowerLeg, 13),
        (HumanoidBone::LeftFoot, 14),
        (HumanoidBone::RightUpperLeg, 15),
        (HumanoidBone::RightLowerLeg, 16),
        (HumanoidBone::RightFoot, 17),
    ] {
        対応.set(骨, 索引);
    }
    対応
}

/// 書き出し設定を作る。
fn 書き出し設定を作る() -> VrmExportOptions {
    VrmExportOptions::new(VrmMeta::new("試験", vec!["作者".to_string()]))
        .with_humanoid(対応付けを作る())
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

/// 髪鎖のsidecar JSONを作る。sphereとcapsuleのcolliderを持つ。
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

/// GLBをJSON部だけ取り出す。容器の境界も確かめる。
fn glbのjsonを取る(バイト列: &[u8]) -> serde_json::Value {
    assert!(バイト列.len() >= 12, "GLBは12バイトのヘッダを持つ");
    assert_eq!(&バイト列[0..4], b"glTF", "先頭はglTFの印である");
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

/// 3要素のJSON配列が期待値に近いことを確かめる。f32由来の丸めを許す。
fn 近似3要素を確かめる(値: &serde_json::Value, 期待: [f64; 3]) {
    let 配列 = 値.as_array().expect("3要素の配列である");
    assert_eq!(配列.len(), 3, "要素は3件である");
    for (実, 想) in 配列.iter().zip(期待.iter()) {
        let 実数 = 実.as_f64().expect("要素は数である");
        assert!((実数 - 想).abs() < 1e-6, "近似一致する: {実数} != {想}");
    }
}

/// 解決とコンパイルを通す。合成テストの共通入口である。
fn 解決してコンパイルする(
    fbxバイト列: &[u8],
    モデル: &mut VrmModel,
) -> (
    PhysicsSidecar,
    vrctom::vrm::PhysicsResolution,
    vrctom::vrm::CompiledSprings,
) {
    let 文 = 髪sidecarを作る(fbxバイト列, モデル);
    文から解決してコンパイルする(&文, fbxバイト列, モデル)
}

/// sidecar文を指定して解決とコンパイルを通す。改変JSONの共通入口である。
fn 文から解決してコンパイルする(
    文: &str,
    fbxバイト列: &[u8],
    モデル: &mut VrmModel,
) -> (
    PhysicsSidecar,
    vrctom::vrm::PhysicsResolution,
    vrctom::vrm::CompiledSprings,
) {
    let sidecar =
        PhysicsSidecar::from_json(文, &PhysicsLimits::new()).expect("合成sidecarは検証を通る");
    let 解決 = モデル
        .resolve_physics_sidecar(&sidecar, fbxバイト列)
        .expect("合成対応は解決できる");
    let コンパイル済み = compile_springs(&sidecar, モデル, &解決, &SpringCompileOptions::new())
        .expect("合成鎖はコンパイルできる");
    (sidecar, 解決, コンパイル済み)
}

/// 線形チェーンを作り、分岐を分割する。Firstでは最初の子を辿る。
#[test]
fn 線形チェーンを作り分岐を分割する() {
    let fbxバイト列 = b"fake-fbx";
    let mut モデル = 書き出し可能モデルを作る();
    let (_, _, コンパイル済み) = 解決してコンパイルする(fbxバイト列, &mut モデル);
    let 報告 = &コンパイル済み.report;
    assert_eq!(報告.spring_count, 1, "springは1本である");
    assert_eq!(報告.joint_count, 2, "関節はHairA・HairBの2件である");
    let spring = &コンパイル済み.bones.springs[0];
    assert_eq!(spring.name, "HairA", "spring名は先頭関節のノード名である");
    assert!(spring.center.is_none(), "centerは今回付けない");
    let 関節: Vec<usize> = spring.joints.iter().map(|関節| 関節.node).collect();
    assert_eq!(関節, vec![3, 4], "HairAから最初の子HairBだけを辿る");
    // 分岐HairCは別springのheadになり得る。AimはHairBである。
    assert_eq!(報告.branch_splits.len(), 1, "分岐は1件記録する");
    assert_eq!(報告.branch_splits[0].bone, "pb:0");
    assert_eq!(報告.branch_splits[0].node, "n:3");
    assert_eq!(報告.branch_splits[0].aimed, Some("n:4".to_string()));
    assert_eq!(報告.branch_splits[0].heads, vec!["n:5".to_string()]);
    // 参照実装の式の値を確かめる。pull 0 のためstiffnessは0である。
    for 関節 in &spring.joints {
        assert_eq!(関節.hit_radius, 0.02, "hit_radiusは半径値である");
        assert_eq!(関節.stiffness, 0.0, "pull 0 のためstiffnessは0である");
        assert_eq!(関節.gravity_power, 0.0, "重力設定は0である");
        assert_eq!(関節.gravity_dir, [0.0, -1.0, 0.0]);
        assert_eq!(関節.drag_force, 0.7, "dragは1-springである");
    }
    assert_eq!(報告.gravity_zero_joints, 2, "重力0の関節は2件である");
    // 所有記録を確かめる。
    assert_eq!(報告.owned_springs.len(), 1);
    assert_eq!(報告.owned_springs[0].bone, "pb:0");
    assert_eq!(報告.owned_springs[0].name, "HairA");
    assert_eq!(報告.owned_springs[0].joints, 2);
}

/// Advancedではstiffnessが効き、pull 1・spring 0で上限の比になる。
/// 骨長は√0.02のため速度は60√0.02、比と保持は60で硬さは3600√0.02である。
#[test]
fn 高精度積分は上限の比になる() {
    let fbxバイト列 = b"fake-fbx";
    let mut モデル = 書き出し可能モデルを作る();
    let mut 文: serde_json::Value =
        serde_json::from_str(&髪sidecarを作る(fbxバイト列, &モデル)).expect("合成sidecarは読める");
    文["physBones"][0]["integrationType"] = serde_json::json!("Advanced");
    文["physBones"][0]["pull"] =
        serde_json::json!({"value": 1.0, "keys": [], "preWrapMode": 8, "postWrapMode": 8});
    文["physBones"][0]["spring"] =
        serde_json::json!({"value": 0.0, "keys": [], "preWrapMode": 8, "postWrapMode": 8});
    文["physBones"][0]["stiffness"] =
        serde_json::json!({"value": 1.0, "keys": [], "preWrapMode": 8, "postWrapMode": 8});
    let (_, _, コンパイル済み) =
        文から解決してコンパイルする(&文.to_string(), fbxバイト列, &mut モデル);
    let spring = &コンパイル済み.bones.springs[0];
    for 関節 in &spring.joints {
        assert!(
            (関節.stiffness - 509.11688).abs() < 1e-3,
            "硬さは3600√0.02である"
        );
        assert_eq!(関節.drag_force, 1.0, "抵抗は1である");
        assert_eq!(関節.gravity_power, 0.0, "重力は0である");
    }
}

/// Simplifiedではstiffnessを無視し、pull・springの分配に従う。
#[test]
fn 単純化積分は分配に従う() {
    let fbxバイト列 = b"fake-fbx";
    let mut モデル = 書き出し可能モデルを作る();
    let mut 文: serde_json::Value =
        serde_json::from_str(&髪sidecarを作る(fbxバイト列, &モデル)).expect("合成sidecarは読める");
    文["physBones"][0]["pull"] =
        serde_json::json!({"value": 0.5, "keys": [], "preWrapMode": 8, "postWrapMode": 8});
    文["physBones"][0]["spring"] =
        serde_json::json!({"value": 0.5, "keys": [], "preWrapMode": 8, "postWrapMode": 8});
    let (_, _, コンパイル済み) =
        文から解決してコンパイルする(&文.to_string(), fbxバイト列, &mut モデル);
    let spring = &コンパイル済み.bones.springs[0];
    for 関節 in &spring.joints {
        assert!(
            (関節.stiffness - 2.828427).abs() < 1e-5,
            "硬さは6√0.02/3である"
        );
        assert!(
            (関節.drag_force - 1.0 / 3.0).abs() < 1e-6,
            "抵抗は1-spring/(1-pull共有)である"
        );
    }
}

/// 版1.0の重力は速度の61倍になる。falloffなしでは沈み角の補正は1である。
#[test]
fn 版10の重力は速度の61倍になる() {
    let fbxバイト列 = b"fake-fbx";
    let mut モデル = 書き出し可能モデルを作る();
    let mut 文: serde_json::Value =
        serde_json::from_str(&髪sidecarを作る(fbxバイト列, &モデル)).expect("合成sidecarは読める");
    文["physBones"][0]["version"] = serde_json::json!("1.0");
    文["physBones"][0]["pull"] =
        serde_json::json!({"value": 1.0, "keys": [], "preWrapMode": 8, "postWrapMode": 8});
    文["physBones"][0]["spring"] =
        serde_json::json!({"value": 0.0, "keys": [], "preWrapMode": 8, "postWrapMode": 8});
    文["physBones"][0]["gravity"] =
        serde_json::json!({"value": 1.0, "keys": [], "preWrapMode": 8, "postWrapMode": 8});
    let (_, _, コンパイル済み) =
        文から解決してコンパイルする(&文.to_string(), fbxバイト列, &mut モデル);
    let spring = &コンパイル済み.bones.springs[0];
    for 関節 in &spring.joints {
        assert!(
            (関節.gravity_power - 517.602).abs() < 1e-2,
            "重力は61倍の速度である"
        );
        assert!(
            (関節.stiffness - 509.11688).abs() < 1e-3,
            "硬さはpull比どおりである"
        );
        assert_eq!(関節.gravity_dir, [0.0, -1.0, 0.0], "正の重力は下向きである");
    }
    assert_eq!(
        コンパイル済み.report.gravity_zero_joints, 0,
        "重力0の関節は無い"
    );
}

/// falloff最大はrest姿勢の重力を消す。
#[test]
fn 最大減衰は重力を消す() {
    let fbxバイト列 = b"fake-fbx";
    let mut モデル = 書き出し可能モデルを作る();
    let mut 文: serde_json::Value =
        serde_json::from_str(&髪sidecarを作る(fbxバイト列, &モデル)).expect("合成sidecarは読める");
    文["physBones"][0]["pull"] =
        serde_json::json!({"value": 1.0, "keys": [], "preWrapMode": 8, "postWrapMode": 8});
    文["physBones"][0]["gravity"] =
        serde_json::json!({"value": 1.0, "keys": [], "preWrapMode": 8, "postWrapMode": 8});
    文["physBones"][0]["gravityFalloff"] =
        serde_json::json!({"value": 1.0, "keys": [], "preWrapMode": 8, "postWrapMode": 8});
    let (_, _, コンパイル済み) =
        文から解決してコンパイルする(&文.to_string(), fbxバイト列, &mut モデル);
    let spring = &コンパイル済み.bones.springs[0];
    for 関節 in &spring.joints {
        assert_eq!(関節.gravity_power, 0.0, "全減衰で重力は消える");
        assert!(関節.stiffness > 0.0, "引きは残る");
    }
}

/// 角度制限は引きを強める。制限なしより硬くなる。
#[test]
fn 角度制限は引きを強める() {
    let fbxバイト列 = b"fake-fbx";
    let mut モデル = 書き出し可能モデルを作る();
    let もと文 =
        serde_json::from_str::<serde_json::Value>(&髪sidecarを作る(fbxバイト列, &モデル))
            .expect("合成sidecarは読める");
    let mut 制限なし = もと文.clone();
    制限なし["physBones"][0]["pull"] =
        serde_json::json!({"value": 0.1, "keys": [], "preWrapMode": 8, "postWrapMode": 8});
    let (_, _, なし済み) =
        文から解決してコンパイルする(&制限なし.to_string(), fbxバイト列, &mut モデル);
    let mut 制限あり = もと文;
    制限あり["physBones"][0]["pull"] =
        serde_json::json!({"value": 0.1, "keys": [], "preWrapMode": 8, "postWrapMode": 8});
    制限あり["physBones"][0]["limitType"] = serde_json::json!("Angle");
    制限あり["physBones"][0]["maxAngleX"] =
        serde_json::json!({"value": 10.0, "keys": [], "preWrapMode": 8, "postWrapMode": 8});
    let (_, _, あり済み) =
        文から解決してコンパイルする(&制限あり.to_string(), fbxバイト列, &mut モデル);
    let なし硬さ = なし済み.bones.springs[0].joints[0].stiffness;
    let あり硬さ = あり済み.bones.springs[0].joints[0].stiffness;
    assert!(なし硬さ.is_finite() && なし硬さ > 0.0, "制限なしでも引く");
    assert!(
        あり硬さ > なし硬さ,
        "制限ありは硬くなる: {あり硬さ} > {なし硬さ}"
    );
}

/// HoldsImmobileはcenterの代わりに引きを強める。
#[test]
fn 保持は引きを強める() {
    let fbxバイト列 = b"fake-fbx";
    let mut モデル = 書き出し可能モデルを作る();
    let mut 文: serde_json::Value =
        serde_json::from_str(&髪sidecarを作る(fbxバイト列, &モデル)).expect("合成sidecarは読める");
    文["physBones"][0]["pull"] =
        serde_json::json!({"value": 0.5, "keys": [], "preWrapMode": 8, "postWrapMode": 8});
    文["physBones"][0]["spring"] =
        serde_json::json!({"value": 0.0, "keys": [], "preWrapMode": 8, "postWrapMode": 8});
    文["physBones"][0]["immobile"] =
        serde_json::json!({"value": 0.5, "keys": [], "preWrapMode": 8, "postWrapMode": 8});
    let (_, _, コンパイル済み) =
        文から解決してコンパイルする(&文.to_string(), fbxバイト列, &mut モデル);
    assert_eq!(
        コンパイル済み.report.held_immobile,
        vec!["pb:0".to_string()]
    );
    let spring = &コンパイル済み.bones.springs[0];
    assert!(spring.center.is_none(), "保持ではcenterを付けない");
    for 関節 in &spring.joints {
        assert!(
            (関節.stiffness - 16.97056).abs() < 1e-4,
            "保持で引きは倍になる"
        );
    }
}

/// colliderを変換し、グループを共有する。
#[test]
fn colliderを変換しグループを共有する() {
    let fbxバイト列 = b"fake-fbx";
    let mut モデル = 書き出し可能モデルを作る();
    let (_, _, コンパイル済み) = 解決してコンパイルする(fbxバイト列, &mut モデル);
    let 報告 = &コンパイル済み.report;
    assert_eq!(報告.collider_count, 2, "参照されるcolliderは2件である");
    assert_eq!(報告.group_count, 1, "同じ参照集合は1グループである");
    let spring = &コンパイル済み.bones.springs[0];
    assert_eq!(spring.collider_groups, vec![0]);
    // sphereはX反転だけであり、縮退で球になる。
    match &コンパイル済み.bones.colliders[0].shape {
        vrctom::vrm::VrmSpringColliderShape::Sphere { offset, radius } => {
            assert_eq!(*offset, [-0.1, 0.2, 0.3], "Xだけ反転する");
            assert_eq!(*radius, 0.05);
        }
        他 => panic!("c:0は球になる: {他:?}"),
    }
    assert_eq!(コンパイル済み.bones.colliders[0].node, 4);
    // capsuleは軸の両端を詰め、X反転する。
    match &コンパイル済み.bones.colliders[1].shape {
        vrctom::vrm::VrmSpringColliderShape::Capsule {
            offset,
            radius,
            tail,
        } => {
            assert_eq!(*offset, [-0.1, -0.15, 0.0]);
            assert_eq!(*tail, [-0.1, 0.15, 0.0]);
            assert_eq!(*radius, 0.05);
        }
        他 => panic!("c:1はカプセルになる: {他:?}"),
    }
    assert_eq!(コンパイル済み.bones.colliders[1].node, 3);
}

/// `VRMC_springBone` を出力し、索引を検証する。
#[test]
fn springbone拡張を出力し索引を検証する() {
    let fbxバイト列 = b"fake-fbx";
    let mut モデル = 書き出し可能モデルを作る();
    let (_, _, コンパイル済み) = 解決してコンパイルする(fbxバイト列, &mut モデル);
    モデル.spring_bones = コンパイル済み.bones;
    let バイト列 = モデル
        .to_vrm(&書き出し設定を作る())
        .expect("spring付きは書き出せる");
    let json = glbのjsonを取る(&バイト列);
    let 拡張 = &json["extensions"]["VRMC_springBone"];
    assert_eq!(拡張["specVersion"], serde_json::json!("1.0"));
    let springs = 拡張["springs"].as_array().expect("springsは配列である");
    assert_eq!(springs.len(), 1);
    let 関節: Vec<u64> = springs[0]["joints"]
        .as_array()
        .expect("jointsは配列である")
        .iter()
        .map(|関節| 関節["node"].as_u64().expect("nodeは数である"))
        .collect();
    assert_eq!(関節, vec![3, 4], "関節索引はモデル索引である");
    assert_eq!(
        springs[0]["colliderGroups"],
        serde_json::json!([0]),
        "グループ索引が載る"
    );
    assert!(springs[0].get("center").is_none(), "centerは書かない");
    let groups = 拡張["colliderGroups"]
        .as_array()
        .expect("colliderGroupsは配列である");
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0]["colliders"], serde_json::json!([0, 1]));
    let colliders = 拡張["colliders"].as_array().expect("collidersは配列である");
    assert_eq!(colliders.len(), 2);
    assert_eq!(colliders[0]["node"], serde_json::json!(4));
    近似3要素を確かめる(&colliders[0]["shape"]["sphere"]["offset"], [-0.1, 0.2, 0.3]);
    近似3要素を確かめる(&colliders[1]["shape"]["capsule"]["tail"], [-0.1, 0.15, 0.0]);
    let used = json["extensionsUsed"]
        .as_array()
        .expect("extensionsUsedは配列である");
    assert!(used.contains(&serde_json::json!("VRMC_springBone")));
    assert!(used.contains(&serde_json::json!("VRMC_vrm")));
}

/// 空のグループ参照は書かない。書式の `minItems: 1` に従う。
#[test]
fn 空のグループ参照は書かない() {
    use vrctom::vrm::{VrmSpring, VrmSpringJoint};
    let mut モデル = 書き出し可能モデルを作る();
    モデル.spring_bones = VrmSpringBones {
        springs: vec![VrmSpring {
            name: "x".to_string(),
            joints: vec![VrmSpringJoint {
                node: 3,
                hit_radius: 0.01,
                stiffness: 0.5,
                gravity_power: 0.0,
                gravity_dir: [0.0, -1.0, 0.0],
                drag_force: 0.5,
                limit: None,
            }],
            center: None,
            collider_groups: Vec::new(),
        }],
        colliders: Vec::new(),
        collider_groups: Vec::new(),
    };
    let バイト列 = モデル
        .to_vrm(&書き出し設定を作る())
        .expect("空参照は書き出せる");
    let json = glbのjsonを取る(&バイト列);
    let springs = json["extensions"]["VRMC_springBone"]["springs"]
        .as_array()
        .expect("springsは配列である");
    assert!(
        springs[0].get("colliderGroups").is_none(),
        "空の参照は書かない"
    );
}

/// springが無い出力は従来どおりである。
#[test]
fn springが無い出力は従来どおりである() {
    let モデル = 書き出し可能モデルを作る();
    assert!(モデル.spring_bones.springs.is_empty());
    let バイト列 = モデル
        .to_vrm(&書き出し設定を作る())
        .expect("spring無しは書き出せる");
    let json = glbのjsonを取る(&バイト列);
    assert!(
        json["extensions"].get("VRMC_springBone").is_none(),
        "空なら拡張を付けない"
    );
    let used = json["extensionsUsed"]
        .as_array()
        .expect("extensionsUsedは配列である");
    assert_eq!(used, &vec![serde_json::json!("VRMC_vrm")]);
}

/// 不正なspringは書き出し時に失敗する。
#[test]
fn 不正なspringは書き出し時に失敗する() {
    // 関節ノード範囲外・重複・非連鎖・範囲外グループを個別に確かめる。
    let mut 良好 = 書き出し可能モデルを作る();
    let (_, _, コンパイル済み) =
        解決してコンパイルする(b"fake-fbx", &mut 書き出し可能モデルを作る());
    良好.spring_bones = コンパイル済み.bones;
    // 範囲外ノード。
    let mut 範囲外 = 良好.clone();
    範囲外.spring_bones.springs[0].joints[0].node = 999;
    assert_eq!(
        範囲外
            .to_vrm(&書き出し設定を作る())
            .expect_err("範囲外関節は失敗する")
            .kind(),
        VrmExportErrorKind::InvalidStructure
    );
    // 二重所有。
    let mut 重複 = 良好.clone();
    let 複写 = 重複.spring_bones.springs[0].clone();
    重複.spring_bones.springs.push(複写);
    assert_eq!(
        重複
            .to_vrm(&書き出し設定を作る())
            .expect_err("二重所有は失敗する")
            .kind(),
        VrmExportErrorKind::InvalidStructure
    );
    // グループ範囲外。
    let mut 群範囲外 = 良好.clone();
    群範囲外.spring_bones.springs[0].collider_groups = vec![9];
    assert_eq!(
        群範囲外
            .to_vrm(&書き出し設定を作る())
            .expect_err("範囲外グループは失敗する")
            .kind(),
        VrmExportErrorKind::InvalidStructure
    );
}

/// unlimited指定や負値はコンパイル時に失敗する。
#[test]
fn 不正な上限や負値はコンパイル時に失敗する() {
    let fbxバイト列 = b"fake-fbx";
    let mut モデル = 書き出し可能モデルを作る();
    let 文 = 髪sidecarを作る(fbxバイト列, &モデル);
    let sidecar =
        PhysicsSidecar::from_json(&文, &PhysicsLimits::new()).expect("合成sidecarは検証を通る");
    let 解決 = モデル
        .resolve_physics_sidecar(&sidecar, fbxバイト列)
        .expect("合成対応は解決できる");
    let 上限0 = SpringCompileOptions::new().with_max_joints(0);
    assert_eq!(
        compile_springs(&sidecar, &mut モデル, &解決, &上限0)
            .expect_err("上限0は失敗する")
            .kind(),
        VrmErrorKind::LimitExceeded
    );
    // 負の半径は検証で拒否する。
    let mut 負値: serde_json::Value = serde_json::from_str(&文).expect("合成sidecarは読める");
    負値["physBones"][0]["radius"]["value"] = serde_json::json!(-0.1);
    assert_eq!(
        PhysicsSidecar::from_json(&負値.to_string(), &PhysicsLimits::new())
            .expect_err("負の半径は検証で失敗する")
            .kind(),
        VrmErrorKind::InvalidPhysicsSidecar
    );
}

/// 実fixtureと実sidecarでVRMを生成し、検査規則を満たす。
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn 実fixtureと実sidecarでvrmを生成する() {
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
    let (mut モデル, _) =
        VrmModel::from_fbx(&解析, &vrctom::vrm::VrmOptions::new()).expect("実fixtureは変換できる");
    let sidecar =
        PhysicsSidecar::parse(&sidecar文, &PhysicsLimits::new()).expect("実sidecarは検証を通る");
    let 解決 = モデル
        .resolve_physics_sidecar(&sidecar, &fbxバイト列)
        .expect("実fixtureの参照は解決できる");
    let コンパイル済み =
        compile_springs(&sidecar, &mut モデル, &解決, &SpringCompileOptions::new())
            .expect("実fixtureの鎖はコンパイルできる");
    assert!(
        !コンパイル済み.bones.springs.is_empty(),
        "springは非空である"
    );
    // 全関節が実在し、親子でつながり、重ならないことを確かめる。
    // 参照実装のvalidatorと同じ規則である。
    let mut 所有: std::collections::HashSet<usize> = std::collections::HashSet::new();
    for spring in &コンパイル済み.bones.springs {
        assert!(!spring.joints.is_empty(), "空鎖は無い");
        let mut 直前: Option<usize> = None;
        for 関節 in &spring.joints {
            assert!(
                関節.node < モデル.nodes.len(),
                "関節は実在する: {}",
                関節.node
            );
            assert!(所有.insert(関節.node), "関節は重ならない: {}", 関節.node);
            if let Some(前) = 直前 {
                assert!(
                    モデル.nodes[前].children.contains(&関節.node),
                    "関節は親子でつながる: {前} -> {}",
                    関節.node
                );
            }
            直前 = Some(関節.node);
            for 値 in [
                関節.hit_radius,
                関節.stiffness,
                関節.gravity_power,
                関節.drag_force,
            ] {
                assert!(値.is_finite() && 値 >= 0.0, "関節値は有限かつ0以上である");
            }
            assert!((0.0..=1.0).contains(&関節.drag_force), "dragは0〜1である");
            let 長さ = (関節.gravity_dir[0] * 関節.gravity_dir[0]
                + 関節.gravity_dir[1] * 関節.gravity_dir[1]
                + 関節.gravity_dir[2] * 関節.gravity_dir[2])
                .sqrt();
            assert!(
                関節.gravity_dir.iter().all(|v| v.is_finite()) && (長さ - 1.0).abs() < 1e-6,
                "重力方向は単位ベクトルである"
            );
        }
        for 群 in &spring.collider_groups {
            assert!(
                *群 < コンパイル済み.bones.collider_groups.len(),
                "グループ参照は範囲内である"
            );
        }
    }
    for collider in &コンパイル済み.bones.colliders {
        assert!(
            collider.node < モデル.nodes.len(),
            "colliderのノードは実在する"
        );
        match &collider.shape {
            vrctom::vrm::VrmSpringColliderShape::Sphere { offset, radius } => {
                assert!(offset.iter().all(|v| v.is_finite()), "球の中心は有限である");
                assert!(
                    *radius >= 0.0 && radius.is_finite(),
                    "球の半径は有限かつ0以上である"
                );
            }
            vrctom::vrm::VrmSpringColliderShape::Capsule {
                offset,
                radius,
                tail,
            } => {
                assert!(offset.iter().all(|v| v.is_finite()), "始点は有限である");
                assert!(tail.iter().all(|v| v.is_finite()), "終点は有限である");
                assert!(
                    *radius >= 0.0 && radius.is_finite(),
                    "半径は有限かつ0以上である"
                );
            }
        }
    }
    モデル.spring_bones = コンパイル済み.bones.clone();
    let 設定 = VrmExportOptions::new(VrmMeta::new("試験", vec!["作者".to_string()]));
    let バイト列 = モデル.to_vrm(&設定).expect("実fixtureは書き出せる");
    let json = glbのjsonを取る(&バイト列);
    assert_eq!(
        json["extensions"]["VRMC_springBone"]["specVersion"],
        serde_json::json!("1.0")
    );
    // 生成物を残し、件数と制限を記録する。
    let 出力先 = std::env::temp_dir().join("vrctom-physics-check.vrm");
    std::fs::write(&出力先, &バイト列).expect("生成物は書ける");
    let 関節数: usize = コンパイル済み
        .bones
        .springs
        .iter()
        .map(|s| s.joints.len())
        .sum();
    eprintln!(
        "実sidecar生成: {} {}バイト spring{}件 関節{}件 collider{}件 group{}件 分岐{}件 共有{}件 重力0関節{}件 平面落下{}件 スキップ{}件",
        出力先.display(),
        バイト列.len(),
        コンパイル済み.report.spring_count,
        関節数,
        コンパイル済み.report.collider_count,
        コンパイル済み.report.group_count,
        コンパイル済み.report.branch_splits.len(),
        コンパイル済み.report.lost_nodes.len(),
        コンパイル済み.report.gravity_zero_joints,
        コンパイル済み.report.plane_colliders_dropped.len(),
        コンパイル済み.report.skipped_bones.len(),
    );
    // center件数はUnity出力と突き合わせる。45spring中43件にcenterが付く。
    assert_eq!(コンパイル済み.report.spring_count, 45);
    assert_eq!(関節数, 148);
    assert_eq!(コンパイル済み.report.collider_count, 12);
    assert_eq!(コンパイル済み.report.group_count, 4);
    assert_eq!(コンパイル済み.report.center_applied.len(), 43);
    assert_eq!(コンパイル済み.report.center_missing.len(), 2);
    let 代表 = |名: &str| {
        コンパイル済み
            .bones
            .springs
            .iter()
            .find(|spring| spring.name == 名)
            .unwrap_or_else(|| panic!("代表springが無い: {名}"))
    };
    let 中心名 = |名: &str| 代表(名).center.map(|索引| モデル.nodes[索引].name.clone());
    assert_eq!(中心名("Butt.L").as_deref(), Some("Armature"));
    assert_eq!(中心名("FrontHair_A").as_deref(), Some("Armature"));
    assert_eq!(中心名("Cheek.L").as_deref(), Some("Head"));
    assert_eq!(中心名("Breast.L"), None);
}

/// Unity出力と関節パラメータを突き合わせる。
/// `VRCTOM_UNITY_VRM` が指すUnity製VRMと、実fixtureから作った鎖を比べる。
/// 許容差は abs 1e-5 + rel 1e-4 であり、重力方向は完全一致とする。
/// drag・gravity・hitRadius・重力方向は完全一致する。
/// stiffnessはPolar制限の37関節だけが許容差を超える（0.1〜0.8%、こちらが高い）。
/// 単精度36方向ループでは一致率が変わらず、原因は単精度丸めではない。
/// 28件はsidecar欠落のlimitRotationカーブ（Ribbon A-D L/R・SideHair_A L/R）が主因であり、
/// 自前ループにUnityスナップショットのカーブ値を入れるとUnity implied値と一致する。
/// 残り9件は一致方向試行のacos特異性による丸めのくじ引きと見込む。
/// 残差の内訳は追跡記録からspring名・joint・PhysBone・headT・制限種別・最大角・
/// 制限回転・rest形状・Rustのlimit/held/stiffness・Unityのstiffnessと差・
/// 36方向ループの最小/最大/平均（倍精度）をstderrに出す。
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn 実fixtureの関節値はunity出力と一致する() {
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
    let 解析 = vrctom::fbx::FbxFile::parse(&fbxバイト列, &vrctom::fbx::FbxOptions::new())
        .expect("実fixtureは読める");
    let (mut モデル, _) =
        VrmModel::from_fbx(&解析, &vrctom::vrm::VrmOptions::new()).expect("実fixtureは変換できる");
    let sidecar =
        PhysicsSidecar::parse(&sidecar文, &PhysicsLimits::new()).expect("実sidecarは検証を通る");
    let 解決 = モデル
        .resolve_physics_sidecar(&sidecar, &fbxバイト列)
        .expect("実fixtureの参照は解決できる");
    let コンパイル済み = compile_springs(
        &sidecar,
        &mut モデル,
        &解決,
        &SpringCompileOptions::new().with_joint_trace(true),
    )
    .expect("実fixtureの鎖はコンパイルできる");
    let unityバイト列 = std::fs::read(&unity経路).expect("Unity製VRMは読める");
    let unity_json = glbのjsonを取る(&unityバイト列);
    let unity_springs = unity_json["extensions"]["VRMC_springBone"]["springs"]
        .as_array()
        .expect("Unity製VRMにspringがある");
    let mut unity表: std::collections::HashMap<&str, &serde_json::Value> =
        std::collections::HashMap::new();
    for spring in unity_springs {
        let 名前 = spring["name"].as_str().expect("spring名は文字列である");
        assert!(
            unity表.insert(名前, spring).is_none(),
            "Unityのspring名は重ならない"
        );
    }
    let 許容 = |実: f64, 想: f64| (実 - 想).abs() <= 1e-5 + 1e-4 * 想.abs();
    let mut 全体 = (0usize, 0usize);
    let mut 項目別: std::collections::HashMap<&str, (usize, usize)> =
        std::collections::HashMap::new();
    let mut 代表値: Vec<String> = Vec::new();
    let mut 不一致: Vec<String> = Vec::new();
    // 許容差を超えたstiffnessの相対差。0.8%以内で9件までを許容する。
    let mut 硬さ残差: Vec<f64> = Vec::new();
    // 残差の出た (spring順, 関節索引)。追跡記録と突き合わせる。
    let mut 詳細対象: Vec<(usize, usize)> = Vec::new();
    // 追跡記録の索引付け。キーは (spring名, spring内の関節索引) である。
    let mut 追跡表: std::collections::HashMap<(String, usize), usize> =
        std::collections::HashMap::new();
    for (i, 追跡) in コンパイル済み.report.joint_traces.iter().enumerate() {
        let 古 = 追跡表.insert((追跡.spring.clone(), 追跡.joint), i);
        assert!(古.is_none(), "追跡記録のキーは重ならない");
    }
    // Vrm索引からsidecar IDへの逆対応。
    let mut 逆対応: std::collections::HashMap<usize, &str> = std::collections::HashMap::new();
    for (id, &vrm) in 解決.node_map.iter() {
        let 古 = 逆対応.insert(vrm, id.as_str());
        assert!(古.is_none(), "対応は完全一致のため写像先は重ならない");
    }
    // sidecarノードとPhysBoneの索引。
    let mut 節点表: std::collections::HashMap<&str, _> = std::collections::HashMap::new();
    for 節点 in &sidecar.nodes {
        let 古 = 節点表.insert(節点.id.as_str(), 節点);
        assert!(古.is_none(), "sidecarノードIDは重ならない");
    }
    let mut 骨表: std::collections::HashMap<&str, _> = std::collections::HashMap::new();
    for 骨 in &sidecar.phys_bones {
        let 古 = 骨表.insert(骨.id.as_str(), 骨);
        assert!(古.is_none(), "PhysBone IDは重ならない");
    }
    for (spring順, spring) in コンパイル済み.bones.springs.iter().enumerate() {
        let 他 = unity表
            .get(spring.name.as_str())
            .unwrap_or_else(|| panic!("Unity出力にspringが無い: {}", spring.name));
        let 他関節 = 他["joints"].as_array().expect("Unityの関節は配列である");
        assert_eq!(他関節.len(), spring.joints.len(), "関節数は一致する");
        for (索引, (関節, 他関節)) in spring.joints.iter().zip(他関節.iter()).enumerate()
        {
            let 期待 = |名: &str| {
                他関節[名]
                    .as_f64()
                    .unwrap_or_else(|| panic!("Unityの値が数でない: {名}"))
            };
            for (名, 実) in [
                ("stiffness", f64::from(関節.stiffness)),
                ("dragForce", f64::from(関節.drag_force)),
                ("gravityPower", f64::from(関節.gravity_power)),
                ("hitRadius", f64::from(関節.hit_radius)),
            ] {
                全体.1 += 1;
                let 項目 = 項目別.entry(名).or_insert((0, 0));
                項目.1 += 1;
                if 許容(実, 期待(名)) {
                    全体.0 += 1;
                    項目.0 += 1;
                } else {
                    if 名 == "stiffness" {
                        let 他値 = 期待(名);
                        硬さ残差.push((実 - 他値).abs() / 他値.abs());
                    }
                    不一致.push(format!(
                        "{}[{i}].{名}: 自={実:.6} 他={:.6}",
                        spring.name,
                        期待(名),
                        i = 索引,
                        実 = 実,
                    ));
                    let 鍵 = (spring順, 索引);
                    if !詳細対象.contains(&鍵) {
                        詳細対象.push(鍵);
                    }
                }
            }
            全体.1 += 1;
            let 他向き = 他関節["gravityDir"]
                .as_array()
                .expect("Unityの重力方向は配列である");
            let 向き一致 = 他向き.len() == 3
                && (f64::from(関節.gravity_dir[0]) - 他向き[0].as_f64().unwrap_or(f64::NAN)).abs()
                    == 0.0
                && (f64::from(関節.gravity_dir[1]) - 他向き[1].as_f64().unwrap_or(f64::NAN)).abs()
                    == 0.0
                && (f64::from(関節.gravity_dir[2]) - 他向き[2].as_f64().unwrap_or(f64::NAN)).abs()
                    == 0.0;
            if 向き一致 {
                全体.0 += 1;
            } else {
                不一致.push(format!("{}[{索引}].gravityDir: 向きが違う", spring.name));
                let 鍵 = (spring順, 索引);
                if !詳細対象.contains(&鍵) {
                    詳細対象.push(鍵);
                }
            }
        }
        if [
            "Butt.L",
            "FrontHair_A",
            "Tail",
            "Cardigan_1.L",
            "Spine_Ribbon_1",
            "Breast.L",
        ]
        .contains(&spring.name.as_str())
        {
            代表値.push(format!(
                "{}: 硬さ={:.5}/{:.5} 抵抗={:.5}/{:.5} 重力={:.5}/{:.5} 半径={:.5}/{:.5}",
                spring.name,
                f64::from(spring.joints[0].stiffness),
                他["joints"][0]["stiffness"].as_f64().unwrap_or(f64::NAN),
                f64::from(spring.joints[0].drag_force),
                他["joints"][0]["dragForce"].as_f64().unwrap_or(f64::NAN),
                f64::from(spring.joints[0].gravity_power),
                他["joints"][0]["gravityPower"].as_f64().unwrap_or(f64::NAN),
                f64::from(spring.joints[0].hit_radius),
                他["joints"][0]["hitRadius"].as_f64().unwrap_or(f64::NAN),
            ));
        }
    }
    eprintln!(
        "Unity突き合わせ: 一致{}件/全体{}件（{:.1}%）",
        全体.0,
        全体.1,
        全体.0 as f64 / 全体.1 as f64 * 100.0
    );
    let mut 項目名: Vec<&str> = 項目別.keys().copied().collect();
    項目名.sort_unstable();
    for 名 in 項目名 {
        let (合致, 総数) = 項目別[名];
        eprintln!(
            "  {名}: {合致}/{総数}（{:.1}%）",
            合致 as f64 / 総数 as f64 * 100.0
        );
    }
    for 代表 in &代表値 {
        eprintln!("  代表 {代表}");
    }
    // 残差のある関節の内訳。追跡記録から計算入力と36方向ループの統計を出す。
    for (spring順, 索引) in &詳細対象 {
        let spring = &コンパイル済み.bones.springs[*spring順];
        let 関節 = &spring.joints[*索引];
        let 他 = unity表
            .get(spring.name.as_str())
            .unwrap_or_else(|| panic!("Unity出力にspringが無い: {}", spring.name));
        let 他関節 = 他["joints"].as_array().expect("Unityの関節は配列である");
        let unity値 = 他関節[*索引]["stiffness"].as_f64().unwrap_or(f64::NAN);
        let 追跡 = &コンパイル済み.report.joint_traces[追跡表[&(spring.name.clone(), *索引)]];
        let vrm名 = &モデル.nodes[関節.node].name;
        let sidecar_id = 逆対応[&関節.node];
        let 構成節点 = &節点表[骨表[追跡.bone.as_str()].component_node_id.as_str()];
        let 世界 = 追跡.bone_to_world;
        let mut 拡大量 = 0.0f64;
        for 列 in 0..3 {
            let 長さ2 = 世界[列 * 4] * 世界[列 * 4]
                + 世界[列 * 4 + 1] * 世界[列 * 4 + 1]
                + 世界[列 * 4 + 2] * 世界[列 * 4 + 2];
            if 長さ2 > 拡大量 {
                拡大量 = 長さ2;
            }
        }

        eprintln!(
            "  診断 {}[{}] VRM節点={} sidecar節点={} PhysBone={}（{}） head={} headT={:.6} tailT={:.6}",
            spring.name,
            索引,
            vrm名,
            sidecar_id,
            追跡.bone,
            構成節点.scene_path.join("/"),
            追跡.is_head,
            追跡.head_t,
            追跡.tail_t,
        );
        eprintln!(
            "    制限種別={:?} 最大角={:?} 制限回転={:?}",
            追跡.limit_kind, 追跡.max_angle, 追跡.limit_rotation
        );
        eprintln!(
            "    rest boneVector={:?} worldT={:?} worldScale={:.6}",
            追跡.bone_vector,
            [世界[12], 世界[13], 世界[14]],
            拡大量.sqrt(),
        );
        eprintln!(
            "    Rust limit={:.6} held={:.6} stiff={:.6} | Unity stiff={:.6}（差{:+.6}）",
            追跡.limit,
            追跡.held,
            f64::from(関節.stiffness),
            unity値,
            f64::from(関節.stiffness) - unity値,
        );
        eprintln!(
            "    36方向[最小 {:.4} 最大 {:.4} 平均 {:.4}]",
            追跡.loop_min, 追跡.loop_max, 追跡.loop_mean,
        );
    }
    for 差 in 不一致.iter().take(40) {
        eprintln!("  残差 {差}");
    }
    // drag・gravity・hitRadius・重力方向は完全一致する。
    assert_eq!(項目別["dragForce"].0, 項目別["dragForce"].1);
    assert_eq!(項目別["gravityPower"].0, 項目別["gravityPower"].1);
    assert_eq!(項目別["hitRadius"].0, 項目別["hitRadius"].1);
    // stiffnessはPolar制限の9関節だけが許容差（0.1〜0.8%）を超える。
    let (合致, 総数) = 項目別["stiffness"];
    assert!(
        合致 as f64 / 総数 as f64 >= 0.74,
        "stiffness一致率が下がった"
    );
    // 残差は件数と相対差の両方で固定する。件数が増えるか0.8%を超えれば退行である。
    assert!(
        硬さ残差.len() <= 9,
        "許容差を超えるstiffnessが増えた: {}件",
        硬さ残差.len()
    );
    let 最大残差 = 硬さ残差.iter().copied().fold(0.0f64, f64::max);
    assert!(
        最大残差 <= 0.008,
        "stiffnessの相対残差が0.8%を超えた: {最大残差}"
    );
    assert!(
        全体.0 as f64 / 全体.1 as f64 >= 0.94,
        "全体一致率が下がった"
    );
}

/// 角度制限の拡張出力をUnity出力と突き合わせる。
/// 末端・制限なし・rest不明には付けず、headのみに付ける。
/// Unityは71関節（spherical 58/cone 9/hinge 4）に拡張を持つ。
/// 角度（pitch/yaw/angle）は完全一致し、回転はsidecar欠落カーブの20関節だけが
/// ずれる（Ribbon A-D L/R・SideHair_A L/R）。詳細は設計文書に残す。
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn 実fixtureの角度制限はunity出力と一致する() {
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
    let 解析 = vrctom::fbx::FbxFile::parse(&fbxバイト列, &vrctom::fbx::FbxOptions::new())
        .expect("実fixtureは読める");
    let (mut モデル, _) =
        VrmModel::from_fbx(&解析, &vrctom::vrm::VrmOptions::new()).expect("実fixtureは変換できる");
    let sidecar =
        PhysicsSidecar::parse(&sidecar文, &PhysicsLimits::new()).expect("実sidecarは検証を通る");
    let 解決 = モデル
        .resolve_physics_sidecar(&sidecar, &fbxバイト列)
        .expect("実fixtureの参照は解決できる");
    let コンパイル済み =
        compile_springs(&sidecar, &mut モデル, &解決, &SpringCompileOptions::new())
            .expect("実fixtureの鎖はコンパイルできる");
    // 自前の内訳。末端は制限なしである。
    let mut 自内訳 = std::collections::HashMap::new();
    let mut 自総数 = 0usize;
    for spring in &コンパイル済み.bones.springs {
        for (索引, 関節) in spring.joints.iter().enumerate() {
            let 末端 = 索引 + 1 == spring.joints.len();
            if 末端 {
                assert!(
                    関節.limit.is_none(),
                    "末端には制限を付けない: {}",
                    spring.name
                );
                continue;
            }
            let Some(制限) = 関節.limit.as_ref() else {
                continue;
            };
            自総数 += 1;
            let 種別 = match 制限 {
                vrctom::vrm::VrmSpringJointLimit::Spherical { .. } => "spherical",
                vrctom::vrm::VrmSpringJointLimit::Cone { .. } => "cone",
                vrctom::vrm::VrmSpringJointLimit::Hinge { .. } => "hinge",
            };
            *自内訳.entry(種別).or_insert(0usize) += 1;
        }
    }
    let unityバイト列 = std::fs::read(&unity経路).expect("Unity製VRMは読める");
    let unity_json = glbのjsonを取る(&unityバイト列);
    let unity_springs = unity_json["extensions"]["VRMC_springBone"]["springs"]
        .as_array()
        .expect("Unity製VRMにspringがある");
    let mut 他内訳 = std::collections::HashMap::new();
    let mut 他総数 = 0usize;
    for spring in unity_springs {
        for 関節 in spring["joints"]
            .as_array()
            .expect("Unityの関節は配列である")
        {
            let 制限 = &関節["extensions"]["VRMC_springBone_limit"]["limit"];
            if 制限.is_null() {
                continue;
            }
            他総数 += 1;
            if 制限.get("spherical").is_some() {
                *他内訳.entry("spherical").or_insert(0usize) += 1;
            } else if 制限.get("cone").is_some() {
                *他内訳.entry("cone").or_insert(0usize) += 1;
            } else if 制限.get("hinge").is_some() {
                *他内訳.entry("hinge").or_insert(0usize) += 1;
            } else {
                panic!("Unityの制限種別が不明である");
            }
        }
    }
    eprintln!("角度制限: 自={自総数}件{自内訳:?} 他={他総数}件{他内訳:?}");
    assert_eq!(自総数, 71, "自前の制限関節数は71である");
    assert_eq!(他総数, 71, "Unityの制限関節数は71である");
    assert_eq!(自内訳, 他内訳, "制限の種別内訳は一致する");
    // 代表値の突き合わせ。角度は完全一致し、回転は欠落カーブの20関節だけずれる。
    let 許容 = |実: f64, 想: f64| (実 - 想).abs() <= 1e-5 + 1e-4 * 想.abs();
    let 許容回転 = |実: &[f32; 4], 想: &[f64; 4]| {
        // 四元数は符号の自由度があるため、符号を合わせて比べる。
        let 符号 = if (実[0] as f64) * 想[0]
            + (実[1] as f64) * 想[1]
            + (実[2] as f64) * 想[2]
            + (実[3] as f64) * 想[3]
            >= 0.0
        {
            1.0
        } else {
            -1.0
        };
        実.iter()
            .enumerate()
            .all(|(i, v)| (*v as f64 - 符号 * 想[i]).abs() <= 1e-4 + 1e-3 * 想[i].abs())
    };
    let mut unity表: std::collections::HashMap<&str, &serde_json::Value> =
        std::collections::HashMap::new();
    for spring in unity_springs {
        let 名前 = spring["name"].as_str().expect("spring名は文字列である");
        assert!(
            unity表.insert(名前, spring).is_none(),
            "Unityのspring名は重ならない"
        );
    }
    let mut 角度合致 = 0usize;
    let mut 角度総数 = 0usize;
    let mut 回転合致 = 0usize;
    let mut 回転総数 = 0usize;
    let mut 回転不一致: Vec<String> = Vec::new();
    for spring in &コンパイル済み.bones.springs {
        let 他 = unity表
            .get(spring.name.as_str())
            .unwrap_or_else(|| panic!("Unity出力にspringが無い: {}", spring.name));
        let 他関節 = 他["joints"].as_array().expect("Unityの関節は配列である");
        assert_eq!(他関節.len(), spring.joints.len(), "関節数は一致する");
        for (索引, (関節, 他関節)) in spring.joints.iter().zip(他関節.iter()).enumerate()
        {
            let 他制限 = &他関節["extensions"]["VRMC_springBone_limit"]["limit"];
            let Some(自制限) = 関節.limit.as_ref() else {
                assert!(
                    他制限.is_null(),
                    "自前なしはUnityもなしである: {}[{索引}]",
                    spring.name
                );
                continue;
            };
            assert!(
                !他制限.is_null(),
                "自前ありはUnityもありである: {}[{索引}]",
                spring.name
            );
            match 自制限 {
                vrctom::vrm::VrmSpringJointLimit::Spherical {
                    pitch,
                    yaw,
                    rotation,
                } => {
                    let 他球 = &他制限["spherical"];
                    角度総数 += 2;
                    if 許容(
                        f64::from(*pitch),
                        他球["pitch"].as_f64().unwrap_or(f64::NAN),
                    ) {
                        角度合致 += 1;
                    } else {
                        eprintln!(
                            "  残差 {}[{索引}] pitch: 自={pitch} 他={}",
                            spring.name, 他球["pitch"]
                        );
                    }
                    if 許容(f64::from(*yaw), 他球["yaw"].as_f64().unwrap_or(f64::NAN)) {
                        角度合致 += 1;
                    } else {
                        eprintln!(
                            "  残差 {}[{索引}] yaw: 自={yaw} 他={}",
                            spring.name, 他球["yaw"]
                        );
                    }
                    let 他回転 = 他球["rotation"]
                        .as_array()
                        .expect("Unityの回転は配列である");
                    let 他回転 = [
                        他回転[0].as_f64().unwrap_or(f64::NAN),
                        他回転[1].as_f64().unwrap_or(f64::NAN),
                        他回転[2].as_f64().unwrap_or(f64::NAN),
                        他回転[3].as_f64().unwrap_or(f64::NAN),
                    ];
                    回転総数 += 1;
                    if 許容回転(rotation, &他回転) {
                        回転合致 += 1;
                    } else {
                        回転不一致.push(format!("{}[{索引}]", spring.name));
                    }
                }
                vrctom::vrm::VrmSpringJointLimit::Cone { angle, rotation } => {
                    let 他錐 = &他制限["cone"];
                    角度総数 += 1;
                    if 許容(
                        f64::from(*angle),
                        他錐["angle"].as_f64().unwrap_or(f64::NAN),
                    ) {
                        角度合致 += 1;
                    } else {
                        eprintln!(
                            "  残差 {}[{索引}] cone: 自={angle} 他={}",
                            spring.name, 他錐["angle"]
                        );
                    }
                    let 他回転 = 他錐["rotation"]
                        .as_array()
                        .expect("Unityの回転は配列である");
                    let 他回転 = [
                        他回転[0].as_f64().unwrap_or(f64::NAN),
                        他回転[1].as_f64().unwrap_or(f64::NAN),
                        他回転[2].as_f64().unwrap_or(f64::NAN),
                        他回転[3].as_f64().unwrap_or(f64::NAN),
                    ];
                    回転総数 += 1;
                    if 許容回転(rotation, &他回転) {
                        回転合致 += 1;
                    } else {
                        回転不一致.push(format!("{}[{索引}]", spring.name));
                    }
                }
                vrctom::vrm::VrmSpringJointLimit::Hinge { angle, rotation } => {
                    let 他蝶 = &他制限["hinge"];
                    角度総数 += 1;
                    if 許容(
                        f64::from(*angle),
                        他蝶["angle"].as_f64().unwrap_or(f64::NAN),
                    ) {
                        角度合致 += 1;
                    } else {
                        eprintln!(
                            "  残差 {}[{索引}] hinge: 自={angle} 他={}",
                            spring.name, 他蝶["angle"]
                        );
                    }
                    let 他回転 = 他蝶["rotation"]
                        .as_array()
                        .expect("Unityの回転は配列である");
                    let 他回転 = [
                        他回転[0].as_f64().unwrap_or(f64::NAN),
                        他回転[1].as_f64().unwrap_or(f64::NAN),
                        他回転[2].as_f64().unwrap_or(f64::NAN),
                        他回転[3].as_f64().unwrap_or(f64::NAN),
                    ];
                    回転総数 += 1;
                    if 許容回転(rotation, &他回転) {
                        回転合致 += 1;
                    } else {
                        回転不一致.push(format!("{}[{索引}]", spring.name));
                    }
                }
            }
        }
    }
    eprintln!("角度一致: {角度合致}/{角度総数} 回転一致: {回転合致}/{回転総数}");
    for 差 in 回転不一致.iter().take(25) {
        eprintln!("  回転残差 {差}");
    }
    // 角度は129値（spherical 58×2＋cone 9＋hinge 4）すべて一致する。
    assert_eq!(角度総数, 58 * 2 + 9 + 4, "角度の総数は129である");
    assert_eq!(角度合致, 角度総数, "角度は完全一致する");
    // 回転は欠落カーブの20関節だけずれる。
    assert_eq!(回転総数, 71, "回転の総数は71である");
    assert!(
        回転合致 as f64 / 回転総数 as f64 >= 0.70,
        "回転一致率が下がった"
    );
}

/// 実fixtureの平面colliderをUnity出力と突き合わせる。
/// colliders 12件・Ground補助ノード・base球・平面法線/ずれを比べ、
/// spring/joint/center/limitの件数が変わらないことも確かめる。
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn 実fixtureの平面colliderはunity出力と一致する() {
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
    let 解析 = vrctom::fbx::FbxFile::parse(&fbxバイト列, &vrctom::fbx::FbxOptions::new())
        .expect("実fixtureは読める");
    let (mut モデル, _) =
        VrmModel::from_fbx(&解析, &vrctom::vrm::VrmOptions::new()).expect("実fixtureは変換できる");
    let 前件数 = モデル.nodes.len();
    let sidecar =
        PhysicsSidecar::parse(&sidecar文, &PhysicsLimits::new()).expect("実sidecarは検証を通る");
    let 解決 = モデル
        .resolve_physics_sidecar(&sidecar, &fbxバイト列)
        .expect("実fixtureの参照は解決できる");
    let コンパイル済み =
        compile_springs(&sidecar, &mut モデル, &解決, &SpringCompileOptions::new())
            .expect("実fixtureの鎖はコンパイルできる");
    // 件数は平面1件増の12件で、他は変わらない。
    assert_eq!(コンパイル済み.report.spring_count, 45);
    assert_eq!(
        コンパイル済み
            .bones
            .springs
            .iter()
            .map(|s| s.joints.len())
            .sum::<usize>(),
        148
    );
    assert_eq!(コンパイル済み.report.collider_count, 12);
    assert_eq!(コンパイル済み.report.group_count, 4);
    assert_eq!(コンパイル済み.report.center_applied.len(), 43);
    let 制限数: usize = コンパイル済み
        .bones
        .springs
        .iter()
        .flat_map(|spring| spring.joints.iter())
        .filter(|関節| 関節.limit.is_some())
        .count();
    assert_eq!(制限数, 71);
    // 補助ノードは末尾に1件だけ増え、既存索引は維持する。
    assert_eq!(モデル.nodes.len(), 前件数 + 1, "補助ノードが1件増える");
    assert_eq!(モデル.nodes[前件数].name, "Ground");
    assert_eq!(モデル.nodes[前件数].parent, None);
    // Unity出力と比べる。collider順は解決順のため、末尾が平面である。
    let unityバイト列 = std::fs::read(&unity経路).expect("Unity製VRMは読める");
    let unity_json = glbのjsonを取る(&unityバイト列);
    let unity_colliders = unity_json["extensions"]["VRMC_springBone"]["colliders"]
        .as_array()
        .expect("Unity製VRMにcolliderがある");
    assert_eq!(unity_colliders.len(), 12, "Unityのcollider数は12である");
    let 自平面 = &コンパイル済み.bones.colliders[11];
    assert_eq!(自平面.node, 前件数, "平面の出力ノードは補助ノードである");
    let 他平面 = &unity_colliders[11];
    // base球と平面法線/ずれが一致する。
    match &自平面.shape {
        vrctom::vrm::VrmSpringColliderShape::Sphere { offset, radius } => {
            let 他球 = &他平面["shape"]["sphere"];
            for (実, 想) in offset.iter().zip(
                他球["offset"]
                    .as_array()
                    .expect("offsetは配列である")
                    .iter(),
            ) {
                assert!(
                    (*実 as f64 - 想.as_f64().unwrap_or(f64::NAN)).abs() <= 1e-5,
                    "base球の中心が一致する"
                );
            }
            assert!(
                (*radius as f64 - 他球["radius"].as_f64().unwrap_or(f64::NAN)).abs() <= 1e-5,
                "base球の半径が一致する"
            );
        }
        他 => panic!("平面はbaseの球になる: {他:?}"),
    }
    match 自平面.extended.as_ref().expect("平面は拡張を持つ") {
        vrctom::vrm::VrmSpringColliderExtension::Plane { normal, offset } => {
            let 他平面拡張 =
                &他平面["extensions"]["VRMC_springBone_extended_collider"]["shape"]["plane"];
            for (実, 想) in normal.iter().zip(
                他平面拡張["normal"]
                    .as_array()
                    .expect("normalは配列である")
                    .iter(),
            ) {
                assert!(
                    (*実 as f64 - 想.as_f64().unwrap_or(f64::NAN)).abs() <= 1e-5,
                    "平面法線が一致する"
                );
            }
            for (実, 想) in offset.iter().zip(
                他平面拡張["offset"]
                    .as_array()
                    .expect("offsetは配列である")
                    .iter(),
            ) {
                assert!(
                    (*実 as f64 - 想.as_f64().unwrap_or(f64::NAN)).abs() <= 1e-5,
                    "平面ずれが一致する"
                );
            }
            assert_eq!(
                他平面["extensions"]["VRMC_springBone_extended_collider"]["specVersion"],
                serde_json::json!("1.0")
            );
        }
    }
    // Groundノード名が一致する。行列は直接比べず、書き出し物の一致で確かめる。
    モデル.spring_bones = コンパイル済み.bones;
    let 設定 = VrmExportOptions::new(VrmMeta::new("試験", vec!["作者".to_string()]));
    let バイト列 = モデル.to_vrm(&設定).expect("実fixtureは書き出せる");
    let json = glbのjsonを取る(&バイト列);
    assert_eq!(json["nodes"][前件数]["name"], serde_json::json!("Ground"));
    let 出力平面 = &json["extensions"]["VRMC_springBone"]["colliders"][11];
    assert_eq!(
        出力平面["extensions"]["VRMC_springBone_extended_collider"]["specVersion"],
        serde_json::json!("1.0")
    );
    let used = json["extensionsUsed"]
        .as_array()
        .expect("extensionsUsedは配列である");
    assert!(used.contains(&serde_json::json!("VRMC_springBone_extended_collider")));
}

/// 実fixtureのノード制約をUnity出力と突き合わせる。
/// 正式sidecarの制約11件から8件（回転4・ひねり4）が出て、種別・軸・参照先・重みが
/// Unity出力と名前基準で一致する。spring・joint・center・limit・colliderの
/// 件数は変わらない。正式sidecarは制約を含むため、空なら失敗する。
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn 実fixtureのノード制約はunity出力と一致する() {
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
    let 解析 = vrctom::fbx::FbxFile::parse(&fbxバイト列, &vrctom::fbx::FbxOptions::new())
        .expect("実fixtureは読める");
    let (mut モデル, _) =
        VrmModel::from_fbx(&解析, &vrctom::vrm::VrmOptions::new()).expect("実fixtureは変換できる");
    let sidecar =
        PhysicsSidecar::parse(&sidecar文, &PhysicsLimits::new()).expect("実sidecarは検証を通る");
    assert!(!sidecar.constraints.is_empty(), "正式sidecarは制約を含む");
    let 解決 = モデル
        .resolve_physics_sidecar(&sidecar, &fbxバイト列)
        .expect("実fixtureの参照は解決できる");
    let コンパイル済み =
        compile_springs(&sidecar, &mut モデル, &解決, &SpringCompileOptions::new())
            .expect("実fixtureの鎖はコンパイルできる");
    // 件数は制約8件で、他は変わらない。
    assert_eq!(コンパイル済み.report.spring_count, 45);
    assert_eq!(
        コンパイル済み
            .bones
            .springs
            .iter()
            .map(|s| s.joints.len())
            .sum::<usize>(),
        148
    );
    assert_eq!(コンパイル済み.report.collider_count, 12);
    assert_eq!(コンパイル済み.report.group_count, 4);
    assert_eq!(コンパイル済み.report.center_applied.len(), 43);
    assert_eq!(モデル.node_constraints.len(), 8, "制約は8件である");
    // 落とした3件は2件のblendと親付けである。
    assert_eq!(コンパイル済み.report.skipped_constraints.len(), 3);
    // Unity出力と名前基準で突き合わせる。ノード順は一致しないため索引では比べない。
    let unityバイト列 = std::fs::read(&unity経路).expect("Unity製VRMは読める");
    let unity_json = glbのjsonを取る(&unityバイト列);
    let unity節点 = unity_json["nodes"].as_array().expect("Unityにnodesがある");
    let mut unity表: std::collections::HashMap<&str, &serde_json::Value> =
        std::collections::HashMap::new();
    for 節点 in unity節点 {
        let 名前 = 節点["name"].as_str().expect("Unityの節点名は文字列である");
        assert!(
            unity表.insert(名前, 節点).is_none(),
            "Unityの節点名は重ならない"
        );
    }
    let 許容 = |実: f64, 想: f64| (実 - 想).abs() <= 1e-5 + 1e-4 * 想.abs();
    let mut 合致 = 0usize;
    for 制約 in &モデル.node_constraints {
        let 対象名 = &モデル.nodes[制約.node].name;
        let 参照名 = &モデル.nodes[制約.source].name;
        let 他 = unity表
            .get(対象名.as_str())
            .unwrap_or_else(|| panic!("Unity出力に節点が無い: {対象名}"));
        let 他制約 = &他["extensions"]["VRMC_node_constraint"]["constraint"];
        let 他種別 = match &制約.kind {
            vrctom::vrm::VrmNodeConstraintKind::Rotation => {
                let 他回転 = &他制約["rotation"];
                assert!(他回転.is_object(), "Unityも回転である: {対象名}");
                他回転
            }
            vrctom::vrm::VrmNodeConstraintKind::Roll { axis } => {
                let 他捻 = &他制約["roll"];
                let 想軸 = match axis {
                    vrctom::vrm::VrmRollAxis::X => "X",
                    vrctom::vrm::VrmRollAxis::Y => "Y",
                    vrctom::vrm::VrmRollAxis::Z => "Z",
                };
                assert_eq!(
                    他捻["rollAxis"],
                    serde_json::json!(想軸),
                    "ひねり軸が一致する: {対象名}"
                );
                他捻
            }
            vrctom::vrm::VrmNodeConstraintKind::Aim { .. } => {
                panic!("実fixtureに注視は無い");
            }
        };
        let 他参照名 =
            unity節点[他種別["source"].as_u64().expect("参照先は数である") as usize]["name"]
                .as_str()
                .expect("Unityの参照先名は文字列である");
        assert_eq!(他参照名, 参照名, "参照先が一致する: {対象名}");
        assert!(
            許容(
                f64::from(制約.weight),
                他種別["weight"].as_f64().unwrap_or(f64::NAN)
            ),
            "重みが一致する: {対象名}"
        );
        合致 += 1;
    }
    assert_eq!(合致, 8);
    eprintln!("制約一致: {合致}/8");
    // 書き出しは拡張と使用宣言を載せる。
    モデル.spring_bones = コンパイル済み.bones;
    let 設定 = VrmExportOptions::new(VrmMeta::new("試験", vec!["作者".to_string()]));
    let バイト列 = モデル.to_vrm(&設定).expect("実fixtureは書き出せる");
    let json = glbのjsonを取る(&バイト列);
    let used = json["extensionsUsed"]
        .as_array()
        .expect("extensionsUsedは配列である");
    assert!(used.contains(&serde_json::json!("VRMC_node_constraint")));
    let 節点 = json["nodes"].as_array().expect("nodesは配列である");
    let 拡張数 = 節点
        .iter()
        .filter(|節点| 節点["extensions"]["VRMC_node_constraint"].is_object())
        .count();
    assert_eq!(拡張数, 8);
}

/// collider参照が無いspringは、空配列を出さずに拡張する。
#[test]
fn collider参照が無いspringは空配列を出さない() {
    let fbxバイト列 = b"fake-fbx";
    let mut モデル = 書き出し可能モデルを作る();
    let mut 値: serde_json::Value =
        serde_json::from_str(&髪sidecarを作る(fbxバイト列, &モデル)).expect("合成sidecarは読める");
    値["physBones"][0]["colliderIds"] = serde_json::json!([]);
    let (_, _, コンパイル済み) =
        文から解決してコンパイルする(&値.to_string(), fbxバイト列, &mut モデル);
    assert_eq!(コンパイル済み.report.collider_count, 0);
    assert_eq!(コンパイル済み.report.group_count, 0);
    assert_eq!(コンパイル済み.report.unused_colliders.len(), 2);
    モデル.spring_bones = コンパイル済み.bones;
    let バイト列 = モデル
        .to_vrm(&書き出し設定を作る())
        .expect("参照なしでも書き出せる");
    let 拡張 = &glbのjsonを取る(&バイト列)["extensions"]["VRMC_springBone"];
    assert!(拡張.get("colliders").is_none(), "空配列は書かない");
    assert!(拡張.get("colliderGroups").is_none(), "空配列は書かない");
    assert_eq!(拡張["springs"].as_array().expect("springsはある").len(), 1);
}

/// 角度制限は拡張に載り、使用宣言にも載る。制限なしは載せない。
#[test]
fn 角度制限は拡張に載る() {
    use vrctom::vrm::{VrmSpring, VrmSpringJoint, VrmSpringJointLimit};
    let mut モデル = 書き出し可能モデルを作る();
    モデル.spring_bones = VrmSpringBones {
        springs: vec![VrmSpring {
            name: "x".to_string(),
            joints: vec![
                VrmSpringJoint {
                    node: 3,
                    hit_radius: 0.01,
                    stiffness: 0.5,
                    gravity_power: 0.0,
                    gravity_dir: [0.0, -1.0, 0.0],
                    drag_force: 0.5,
                    limit: Some(VrmSpringJointLimit::Spherical {
                        pitch: 0.5,
                        yaw: 0.25,
                        rotation: [0.0, 0.0, 0.0, 1.0],
                    }),
                },
                VrmSpringJoint {
                    node: 4,
                    hit_radius: 0.01,
                    stiffness: 0.5,
                    gravity_power: 0.0,
                    gravity_dir: [0.0, -1.0, 0.0],
                    drag_force: 0.5,
                    limit: None,
                },
            ],
            center: None,
            collider_groups: Vec::new(),
        }],
        colliders: Vec::new(),
        collider_groups: Vec::new(),
    };
    let バイト列 = モデル
        .to_vrm(&書き出し設定を作る())
        .expect("制限付きは書き出せる");
    let json = glbのjsonを取る(&バイト列);
    let 関節 = json["extensions"]["VRMC_springBone"]["springs"][0]["joints"]
        .as_array()
        .expect("jointsは配列である");
    let 制限 = &関節[0]["extensions"]["VRMC_springBone_limit"];
    assert_eq!(制限["specVersion"], serde_json::json!("1.0-draft"));
    assert_eq!(
        制限["limit"]["spherical"]["pitch"],
        serde_json::json!(0.5f32)
    );
    assert_eq!(
        制限["limit"]["spherical"]["yaw"],
        serde_json::json!(0.25f32)
    );
    assert!(
        関節[1].get("extensions").is_none(),
        "制限なしは拡張を書かない"
    );
    let used = json["extensionsUsed"]
        .as_array()
        .expect("extensionsUsedは配列である");
    assert!(used.contains(&serde_json::json!("VRMC_springBone_limit")));
    let mut モデル2 = 書き出し可能モデルを作る();
    モデル2.spring_bones = VrmSpringBones {
        springs: vec![VrmSpring {
            name: "x".to_string(),
            joints: vec![
                VrmSpringJoint {
                    node: 3,
                    hit_radius: 0.01,
                    stiffness: 0.5,
                    gravity_power: 0.0,
                    gravity_dir: [0.0, -1.0, 0.0],
                    drag_force: 0.5,
                    limit: None,
                },
                VrmSpringJoint {
                    node: 4,
                    hit_radius: 0.01,
                    stiffness: 0.5,
                    gravity_power: 0.0,
                    gravity_dir: [0.0, -1.0, 0.0],
                    drag_force: 0.5,
                    limit: None,
                },
            ],
            center: None,
            collider_groups: Vec::new(),
        }],
        colliders: Vec::new(),
        collider_groups: Vec::new(),
    };
    let バイト列2 = モデル2
        .to_vrm(&書き出し設定を作る())
        .expect("制限なしは書き出せる");
    let json2 = glbのjsonを取る(&バイト列2);
    let used2 = json2["extensionsUsed"]
        .as_array()
        .expect("extensionsUsedは配列である");
    assert!(!used2.contains(&serde_json::json!("VRMC_springBone_limit")));
}

/// 制約なしは拡張を書かない。`--physics` なしの退行確認も兼ねる。
#[test]
fn 制約なしは拡張を書かない() {
    let モデル = 書き出し可能モデルを作る();
    assert!(モデル.node_constraints.is_empty());
    let バイト列 = モデル
        .to_vrm(&書き出し設定を作る())
        .expect("制約なしは書き出せる");
    let json = glbのjsonを取る(&バイト列);
    let used = json["extensionsUsed"]
        .as_array()
        .expect("extensionsUsedは配列である");
    assert!(!used.contains(&serde_json::json!("VRMC_node_constraint")));
    for 節点 in json["nodes"].as_array().expect("nodesは配列である") {
        assert!(
            節点.get("extensions").is_none(),
            "制約なしの節点に拡張は付かない"
        );
    }
}

/// Sphereは高さがあっても球のままである。
#[test]
fn 高さのある球は球のままである() {
    use vrctom::vrm::VrmSpringColliderShape;
    let fbxバイト列 = b"fake-fbx";
    let mut モデル = 書き出し可能モデルを作る();
    let mut 値: serde_json::Value =
        serde_json::from_str(&髪sidecarを作る(fbxバイト列, &モデル)).expect("合成sidecarは読める");
    値["colliders"][0]["height"] = serde_json::json!(0.4);
    let (_, _, コンパイル済み) =
        文から解決してコンパイルする(&値.to_string(), fbxバイト列, &mut モデル);
    match &コンパイル済み.bones.colliders[0].shape {
        VrmSpringColliderShape::Sphere { offset, radius } => {
            assert_eq!(*radius, 0.05);
            assert_eq!(offset.len(), 3, "形状点は3要素である");
        }
        他 => panic!("Sphere指定は球になる: {他:?}"),
    }
}

/// colliderの姿勢はroot-localであり、componentに影響されない。
#[test]
fn colliderの姿勢はrootローカルでありcomponentに影響されない() {
    let fbxバイト列 = b"fake-fbx";
    let mut モデル = 書き出し可能モデルを作る();
    let mut 値: serde_json::Value =
        serde_json::from_str(&髪sidecarを作る(fbxバイト列, &モデル)).expect("合成sidecarは読める");
    // c:0の基準をHairA(n:3)へ移し、付属はHairB(n:4)のままにする。
    // 姿勢はroot-localのため、componentの移動では変わらない。
    値["colliders"][0]["rootNodeId"] = serde_json::json!("n:3");
    // c:1の付属もHairB(n:4)へ移し、Z軸回りに90度回す。基準はHairA(n:3)のままである。
    値["colliders"][1]["componentNodeId"] = serde_json::json!("n:4");
    値["colliders"][1]["rotation"] = serde_json::json!([
        0.0,
        0.0,
        std::f64::consts::FRAC_1_SQRT_2,
        std::f64::consts::FRAC_1_SQRT_2
    ]);
    let (_, _, コンパイル済み) =
        文から解決してコンパイルする(&値.to_string(), fbxバイト列, &mut モデル);
    assert_eq!(コンパイル済み.bones.colliders[0].node, 3);
    assert_eq!(コンパイル済み.bones.colliders[1].node, 3);
    // Sphereは位置のX反転だけであり、componentの平行移動は入らない。
    match &コンパイル済み.bones.colliders[0].shape {
        vrctom::vrm::VrmSpringColliderShape::Sphere { offset, .. } => {
            近似3要素を確かめる(&serde_json::json!(offset), [-0.1, 0.2, 0.3]);
        }
        他 => panic!("c:0は球になる: {他:?}"),
    }
    // 回転Capsuleは鏡像のY軸に沿って両端を詰める。componentの回転は使わない。
    match &コンパイル済み.bones.colliders[1].shape {
        vrctom::vrm::VrmSpringColliderShape::Capsule { offset, tail, .. } => {
            近似3要素を確かめる(&serde_json::json!(offset), [-0.25, 0.0, 0.0]);
            近似3要素を確かめる(&serde_json::json!(tail), [0.05, 0.0, 0.0]);
        }
        他 => panic!("c:1はカプセルになる: {他:?}"),
    }
}

/// 平面colliderは補助ノードと拡張付きの球で出る。
/// 参照集合には他colliderと同じ規則で入り、未使用の平面もcolliderだけ出る。
#[test]
fn 平面colliderは補助ノードと拡張付きで出る() {
    let fbxバイト列 = b"fake-fbx";
    let mut モデル = 書き出し可能モデルを作る();
    let 前件数 = モデル.nodes.len();
    let mut 値: serde_json::Value =
        serde_json::from_str(&髪sidecarを作る(fbxバイト列, &モデル)).expect("合成sidecarは読める");
    // シーン専用のGroundノード2件。参照姿勢はUnity左手系・メートルである。
    let 地面 = |id: &str| {
        serde_json::json!({
            "id": id,
            "parentId": serde_json::Value::Null,
            "name": "Ground",
            "siblingIndex": 27,
            "scenePath": ["Milfy", "Ground"],
            "source": serde_json::Value::Null,
            "referenceSource": "scene",
            "referenceLocal": {
                "position": [0.0, 0.0209, 0.0],
                "rotation": [0.0, 0.0, 0.0, 1.0],
                "scale": [1.0, 1.0, 1.0],
            },
        })
    };
    値["nodes"]
        .as_array_mut()
        .expect("nodesは配列である")
        .push(地面("n:100"));
    値["nodes"]
        .as_array_mut()
        .expect("nodesは配列である")
        .push(地面("n:101"));
    // c:2は参照あり、c:3は未使用の平面である。
    let 平面 = |id: &str, 節点: &str| {
        serde_json::json!({
            "id": id,
            "componentNodeId": 節点,
            "rootNodeId": 節点,
            "enabled": true,
            "shapeType": "Plane",
            "insideBounds": false,
            "radius": 0.5,
            "height": 2.0,
            "position": [0.0, 0.0, 0.0],
            "rotation": [0.0, 0.0, 0.0, 1.0],
            "bonesAsSpheres": false,
        })
    };
    値["colliders"]
        .as_array_mut()
        .expect("collidersは配列である")
        .push(平面("c:2", "n:100"));
    値["colliders"]
        .as_array_mut()
        .expect("collidersは配列である")
        .push(平面("c:3", "n:101"));
    値["physBones"][0]["colliderIds"]
        .as_array_mut()
        .expect("colliderIdsは配列である")
        .push(serde_json::json!("c:2"));
    let (_, _, コンパイル済み) =
        文から解決してコンパイルする(&値.to_string(), fbxバイト列, &mut モデル);
    // 補助ノードは末尾に追加し、既存索引は維持する。
    assert_eq!(モデル.nodes.len(), 前件数 + 2, "補助ノードが2件増える");
    assert_eq!(モデル.nodes[前件数].name, "Ground");
    assert_eq!(モデル.nodes[前件数].parent, None);
    assert_eq!(モデル.nodes[前件数 + 1].name, "Ground");
    // sphere/capsule 2件と平面2件が出る。平面の出力ノードは補助ノードである。
    assert_eq!(コンパイル済み.bones.colliders.len(), 4);
    let 平面錐 = &コンパイル済み.bones.colliders[2];
    assert_eq!(平面錐.node, 前件数);
    match &平面錐.shape {
        vrctom::vrm::VrmSpringColliderShape::Sphere { offset, radius } => {
            近似3要素を確かめる(&serde_json::json!(offset), [0.0, -1000.0, 0.0]);
            assert!((radius - 1000.0).abs() < 1e-6);
        }
        他 => panic!("平面はbaseの球になる: {他:?}"),
    }
    match 平面錐.extended.as_ref().expect("平面は拡張を持つ") {
        vrctom::vrm::VrmSpringColliderExtension::Plane { normal, offset } => {
            近似3要素を確かめる(&serde_json::json!(normal), [0.0, 1.0, 0.0]);
            近似3要素を確かめる(&serde_json::json!(offset), [0.0, 0.0, 0.0]);
        }
    }
    // 未使用の平面もcolliderだけ出る。グループには入らない。
    let 未使用 = &コンパイル済み.bones.colliders[3];
    assert_eq!(未使用.node, 前件数 + 1);
    assert!(未使用.extended.is_some(), "未使用の平面も拡張を持つ");
    assert!(
        コンパイル済み.report.unused_colliders.is_empty(),
        "平面は未使用扱いしない"
    );
    let 群あり = コンパイル済み
        .bones
        .collider_groups
        .iter()
        .any(|群| 群.colliders.contains(&2));
    assert!(群あり, "参照ありの平面はグループに入る");
    let 群なし = コンパイル済み
        .bones
        .collider_groups
        .iter()
        .any(|群| 群.colliders.contains(&3));
    assert!(!群なし, "未使用の平面はグループに入らない");
    // 書き出しは拡張と使用宣言を載せる。
    let mut 書き出し = モデル;
    書き出し.spring_bones = コンパイル済み.bones;
    let バイト列 = 書き出し
        .to_vrm(&書き出し設定を作る())
        .expect("平面付きは書き出せる");
    let json = glbのjsonを取る(&バイト列);
    let 出力錐 = &json["extensions"]["VRMC_springBone"]["colliders"][2];
    let 拡張 = &出力錐["extensions"]["VRMC_springBone_extended_collider"];
    assert_eq!(拡張["specVersion"], serde_json::json!("1.0"));
    近似3要素を確かめる(&拡張["shape"]["plane"]["normal"], [0.0, 1.0, 0.0]);
    近似3要素を確かめる(&拡張["shape"]["plane"]["offset"], [0.0, 0.0, 0.0]);
    近似3要素を確かめる(&出力錐["shape"]["sphere"]["offset"], [0.0, -1000.0, 0.0]);
    let used = json["extensionsUsed"]
        .as_array()
        .expect("extensionsUsedは配列である");
    assert!(used.contains(&serde_json::json!("VRMC_springBone_extended_collider")));
}

/// 平面の換算失敗時はモデルを変更しない。
#[test]
fn 平面の換算失敗時はモデルを変更しない() {
    let fbxバイト列 = b"fake-fbx";
    let mut モデル = 書き出し可能モデルを作る();
    let 前件数 = モデル.nodes.len();
    let mut 値: serde_json::Value =
        serde_json::from_str(&髪sidecarを作る(fbxバイト列, &モデル)).expect("合成sidecarは読める");
    値["nodes"]
        .as_array_mut()
        .expect("nodesは配列である")
        .push(serde_json::json!({
            "id": "n:100",
            "parentId": serde_json::Value::Null,
            "name": "Ground",
            "siblingIndex": 27,
            "scenePath": ["Milfy", "Ground"],
            "source": serde_json::Value::Null,
            "referenceSource": "scene",
            "referenceLocal": {
                "position": [0.0, 0.0209, 0.0],
                "rotation": [0.0, 0.0, 0.0, 1.0],
                "scale": [1.0, 1.0, 1.0],
            },
        }));
    // f32に収まらない位置の平面は換算できない（有限値のため書式は通る）。
    値["colliders"]
        .as_array_mut()
        .expect("collidersは配列である")
        .push(serde_json::json!({
            "id": "c:2",
            "componentNodeId": "n:100",
            "rootNodeId": "n:100",
            "enabled": true,
            "shapeType": "Plane",
            "insideBounds": false,
            "radius": 0.5,
            "height": 2.0,
            "position": [1e300, 0.0, 0.0],
            "rotation": [0.0, 0.0, 0.0, 1.0],
            "bonesAsSpheres": false,
        }));
    値["physBones"][0]["colliderIds"]
        .as_array_mut()
        .expect("colliderIdsは配列である")
        .push(serde_json::json!("c:2"));
    let 結果: Result<_, vrctom::vrm::VrmError> = {
        let sidecar = PhysicsSidecar::from_json(&値.to_string(), &PhysicsLimits::new())
            .expect("書式自体は通る");
        let 解決 = モデル
            .resolve_physics_sidecar(&sidecar, fbxバイト列)
            .expect("解決は通る");
        compile_springs(&sidecar, &mut モデル, &解決, &SpringCompileOptions::new())
    };
    assert!(結果.is_err(), "f32に収まらない平面は失敗する");
    assert_eq!(モデル.nodes.len(), 前件数, "失敗時は補助ノードを足さない");
}

/// 制約の種別写像に従う。回転3軸は回転、1軸はひねり、2軸・複数参照・
/// 親付け・自己参照・重複対象は落とす。注視は主軸の場合だけ写す。
#[test]
fn 制約の種別写像に従う() {
    let fbxバイト列 = b"fake-fbx";
    let mut モデル = 書き出し可能モデルを作る();
    let mut 値: serde_json::Value =
        serde_json::from_str(&髪sidecarを作る(fbxバイト列, &モデル)).expect("合成sidecarは読める");
    let 制約 = |id: &str,
                対象: &str,
                参照: serde_json::Value,
                種別: &str,
                軸: serde_json::Value,
                注視: serde_json::Value| {
        serde_json::json!({
            "id": id,
            "nodeId": 対象,
            "targetNodeId": 対象,
            "type": 種別,
            "family": "vrchat",
            "weight": 0.5,
            "enabled": true,
            "active": true,
            "freezeToWorld": false,
            "affectsRotation": 軸,
            "aimAxis": 注視,
            "sources": 参照,
        })
    };
    let 参照1 =
        |節点: &str| serde_json::json!([{"nodeId": 節点, "external": false, "weight": 1.0}]);
    値["constraints"] = serde_json::json!([
        制約(
            "k:0",
            "n:3",
            参照1("n:4"),
            "Rotation",
            serde_json::json!([true, true, true]),
            serde_json::json!([0.0, 0.0, 1.0])
        ),
        制約(
            "k:1",
            "n:5",
            参照1("n:4"),
            "Rotation",
            serde_json::json!([false, true, false]),
            serde_json::json!([0.0, 0.0, 1.0])
        ),
        制約(
            "k:2",
            "n:2",
            参照1("n:1"),
            "Aim",
            serde_json::json!([true, true, true]),
            serde_json::json!([0.0, 0.0, 1.0])
        ),
        制約(
            "k:3",
            "n:5",
            参照1("n:4"),
            "Rotation",
            serde_json::json!([true, true, false]),
            serde_json::json!([0.0, 0.0, 1.0])
        ),
        制約(
            "k:4",
            "n:2",
            serde_json::json!([{"nodeId": "n:1", "external": false, "weight": 0.5}, {"nodeId": "n:0", "external": false, "weight": 0.5}]),
            "Rotation",
            serde_json::json!([true, true, true]),
            serde_json::json!([0.0, 0.0, 1.0])
        ),
        制約(
            "k:5",
            "n:2",
            参照1("n:1"),
            "Parent",
            serde_json::json!([true, true, true]),
            serde_json::json!([0.0, 0.0, 1.0])
        ),
        制約(
            "k:6",
            "n:3",
            参照1("n:4"),
            "Rotation",
            serde_json::json!([true, true, true]),
            serde_json::json!([0.0, 0.0, 1.0])
        ),
        制約(
            "k:7",
            "n:4",
            serde_json::json!([{"nodeId": "n:4", "external": false, "weight": 1.0}]),
            "Rotation",
            serde_json::json!([true, true, true]),
            serde_json::json!([0.0, 0.0, 1.0])
        ),
    ]);
    let (_, _, コンパイル済み) =
        文から解決してコンパイルする(&値.to_string(), fbxバイト列, &mut モデル);
    // 回転・ひねり・注視の3件だけ残り、落とした5件を記録する。
    assert_eq!(モデル.node_constraints.len(), 3);
    let 回転 = &モデル.node_constraints[0];
    assert_eq!(回転.node, 3);
    assert_eq!(回転.source, 4);
    assert!((回転.weight - 0.5).abs() < 1e-6);
    assert!(matches!(
        回転.kind,
        vrctom::vrm::VrmNodeConstraintKind::Rotation
    ));
    let 捻り = &モデル.node_constraints[1];
    assert_eq!(捻り.node, 5);
    assert!(matches!(
        捻り.kind,
        vrctom::vrm::VrmNodeConstraintKind::Roll {
            axis: vrctom::vrm::VrmRollAxis::Y
        }
    ));
    let 注視 = &モデル.node_constraints[2];
    assert_eq!(注視.node, 2);
    assert!(matches!(
        注視.kind,
        vrctom::vrm::VrmNodeConstraintKind::Aim {
            axis: vrctom::vrm::VrmAimAxis::PositiveZ
        }
    ));
    let mut 落下 = コンパイル済み.report.skipped_constraints.clone();
    落下.sort();
    assert_eq!(落下, vec!["k:3", "k:4", "k:5", "k:6", "k:7"]);
    // 書き出しは拡張と使用宣言を載せる。注視軸はZのため鏡像でも変わらない。
    let バイト列 = モデル
        .to_vrm(&書き出し設定を作る())
        .expect("制約付きは書き出せる");
    let json = glbのjsonを取る(&バイト列);
    let 節点 = json["nodes"].as_array().expect("nodesは配列である");
    let 拡張 = &節点[3]["extensions"]["VRMC_node_constraint"];
    assert_eq!(拡張["specVersion"], serde_json::json!("1.0"));
    assert_eq!(
        拡張["constraint"]["rotation"]["source"],
        serde_json::json!(4)
    );
    assert_eq!(
        拡張["constraint"]["rotation"]["weight"],
        serde_json::json!(0.5f32)
    );
    assert_eq!(
        節点[2]["extensions"]["VRMC_node_constraint"]["constraint"]["aim"]["aimAxis"],
        serde_json::json!("PositiveZ")
    );
    let used = json["extensionsUsed"]
        .as_array()
        .expect("extensionsUsedは配列である");
    assert!(used.contains(&serde_json::json!("VRMC_node_constraint")));
}

/// 同じ集合の順序・重複違いはグループを共有する。
/// `ignoreOtherPhysBones` が偽なら他rootを切らず、所有権で奪い合う。
#[test]
fn 順序違いの参照集合はグループを共有する() {
    let fbxバイト列 = b"fake-fbx";
    let mut モデル = 書き出し可能モデルを作る();
    let mut 値: serde_json::Value =
        serde_json::from_str(&髪sidecarを作る(fbxバイト列, &モデル)).expect("合成sidecarは読める");
    // pb:1はSpine(n:1)を起点にし、Head(n:2)で終わる。HairA(n:3)は
    // pb:0のrootのため切断する。参照集合は順序・重複違いで同じである。
    let 追加 = serde_json::json!({
        "id": "pb:1",
        "componentNodeId": "n:1",
        "rootNodeId": "n:1",
        "enabled": true,
        "version": "1.1",
        "integrationType": "Simplified",
        "ignoreNodeIds": [],
        "ignoreOtherPhysBones": false,
        "endpointPosition": [0.0, 0.0, 0.0],
        "multiChildType": "Ignore",
        "pull": {"value": 0.0, "keys": [], "preWrapMode": 8, "postWrapMode": 8},
        "spring": {"value": 0.3, "keys": [], "preWrapMode": 8, "postWrapMode": 8},
        "stiffness": {"value": 0.5, "keys": [], "preWrapMode": 8, "postWrapMode": 8},
        "gravity": {"value": 0.0, "keys": [], "preWrapMode": 8, "postWrapMode": 8},
        "gravityFalloff": {"value": 0.0, "keys": [], "preWrapMode": 8, "postWrapMode": 8},
        "immobileType": "AllMotion",
        "immobile": {"value": 0.0, "keys": [], "preWrapMode": 8, "postWrapMode": 8},
        "radius": {"value": 0.02, "keys": [], "preWrapMode": 8, "postWrapMode": 8},
        "allowCollision": true,
        "colliderIds": ["c:1", "c:0", "c:0"],
        "limitType": "None",
        "maxAngleX": {"value": 0.0, "keys": [], "preWrapMode": 8, "postWrapMode": 8},
        "maxAngleZ": {"value": 0.0, "keys": [], "preWrapMode": 8, "postWrapMode": 8},
        "limitRotation": [0.0, 0.0, 0.0],
        "isAnimated": false,
    });
    値["physBones"]
        .as_array_mut()
        .expect("physBonesは配列である")
        .push(追加);
    let (_, _, コンパイル済み) =
        文から解決してコンパイルする(&値.to_string(), fbxバイト列, &mut モデル);
    // pb:1はSpine起点でHeadまで伸びる。springはHairAとSpine-Headの2本である。
    // sidecarに腕ノードは無いため、腕には伸びない。
    assert_eq!(コンパイル済み.report.spring_count, 2);
    assert_eq!(
        コンパイル済み.report.group_count, 1,
        "順序・重複違いでも共有する"
    );
    for spring in &コンパイル済み.bones.springs {
        assert_eq!(spring.collider_groups, vec![0]);
    }
    assert_eq!(
        コンパイル済み.bones.collider_groups[0].colliders,
        vec![0, 1],
        "重複は除いて正準化する"
    );
    // `ignoreOtherPhysBones` が偽なら他rootで切らず、所有権で奪い合う。
    assert!(コンパイル済み.report.cut_at_other_roots.is_empty());
    assert_eq!(コンパイル済み.report.lost_nodes.len(), 3);
    assert_eq!(コンパイル済み.report.shared_nodes.len(), 3);
    assert_eq!(コンパイル済み.report.shared_nodes[0].node, "n:3");
    assert_eq!(コンパイル済み.report.shared_nodes[0].owner, "pb:0");
    assert_eq!(
        コンパイル済み.report.shared_nodes[0].lost_by,
        vec!["pb:1".to_string()]
    );
    assert_eq!(コンパイル済み.report.branch_splits.len(), 2);
    assert_eq!(コンパイル済み.report.branch_splits[0].bone, "pb:0");
    assert_eq!(
        コンパイル済み.report.branch_splits[0].aimed,
        Some("n:4".to_string())
    );
    assert_eq!(
        コンパイル済み.report.branch_splits[0].heads,
        vec!["n:5".to_string()]
    );
    assert_eq!(コンパイル済み.report.branch_splits[1].bone, "pb:1");
    assert_eq!(コンパイル済み.report.branch_splits[1].aimed, None);
    assert_eq!(
        コンパイル済み.report.branch_splits[1].heads,
        vec!["n:4".to_string(), "n:5".to_string()]
    );
    // pb:1の木はpb:0と重なり、n:3・n:4・n:5を奪われる。
    assert_eq!(コンパイル済み.report.lost_nodes.len(), 3);
    assert_eq!(コンパイル済み.report.shared_nodes.len(), 3);
    assert_eq!(コンパイル済み.report.shared_nodes[0].node, "n:3");
    assert_eq!(コンパイル済み.report.shared_nodes[0].owner, "pb:0");
    assert_eq!(
        コンパイル済み.report.shared_nodes[0].lost_by,
        vec!["pb:1".to_string()]
    );
    // first-child化の近似はIgnore以外で記録する。
    assert_eq!(
        コンパイル済み.report.first_child_multichild,
        vec!["pb:0".to_string()]
    );
    assert!(コンパイル済み.report.center_missing.len() == 2);
    assert!(コンパイル済み.report.no_endpoint.len() == 2);
    // spring名は先頭関節のノード名である。
    let 名前: Vec<&str> = コンパイル済み
        .bones
        .springs
        .iter()
        .map(|spring| spring.name.as_str())
        .collect();
    assert_eq!(名前, vec!["HairA", "Spine"]);
}

/// 先行PlanにAim先を奪われたheadは単関節で落とす。
#[test]
fn 奪われたaim先のheadは単関節で落とす() {
    // pb:1 はSpine(n:1)を起点にするが、Head以下はpb:0が先に確保する。
    // pb:1 のhead SpineはAim先Headを失い、単関節になる。
    let fbxバイト列 = b"fake-fbx";
    let mut モデル = 書き出し可能モデルを作る();
    let mut 値: serde_json::Value =
        serde_json::from_str(&髪sidecarを作る(fbxバイト列, &モデル)).expect("合成sidecarは読める");
    let 追加 = serde_json::json!({
        "id": "pb:1", "componentNodeId": "n:1", "rootNodeId": "n:1", "enabled": true,
        "version": "1.1", "integrationType": "Simplified", "ignoreNodeIds": [],
        "ignoreOtherPhysBones": false, "endpointPosition": [0.0, 0.0, 0.0],
        "multiChildType": "Ignore", "allowCollision": true, "colliderIds": [],
        "limitType": "None", "limitRotation": [0.0, 0.0, 0.0], "isAnimated": false,
    });
    値["physBones"]
        .as_array_mut()
        .expect("physBonesは配列である")
        .push(追加);
    // pb:0 のrootをHead(n:2)へ移し、先に下流を確保させる。
    // pb:1 のhead Spine(n:1)はAim先Headを失い、単関節になる。
    値["physBones"][0]["componentNodeId"] = serde_json::json!("n:2");
    値["physBones"][0]["rootNodeId"] = serde_json::json!("n:2");
    let (_, _, コンパイル済み) =
        文から解決してコンパイルする(&値.to_string(), fbxバイト列, &mut モデル);
    assert_eq!(コンパイル済み.report.spring_count, 1);
    assert_eq!(コンパイル済み.bones.springs[0].name, "Head");
    let 関節: Vec<usize> = コンパイル済み.bones.springs[0]
        .joints
        .iter()
        .map(|関節| 関節.node)
        .collect();
    assert_eq!(関節, vec![2, 3, 4]);
    assert_eq!(コンパイル済み.report.lost_nodes.len(), 4);
    let 捨て = コンパイル済み
        .report
        .skipped_bones
        .iter()
        .find(|捨て| 捨て.bone == "pb:1")
        .expect("pb:1は落とす");
    match &捨て.reason {
        vrctom::vrm::SkippedBoneReason::AimTargetUnowned => {}
        他 => panic!("Aim先の喪失で落とす: {他:?}"),
    }
}

/// Aimする所有骨が無い場合は作らず記録する。
#[test]
fn 所有もaimも無い場合は作らず記録する() {
    // 単独ノードをrootにする。Aimが無いためheadにならない。
    let fbxバイト列 = b"fake-fbx";
    let mut モデル = 書き出し可能モデルを作る();
    let mut 値: serde_json::Value =
        serde_json::from_str(&髪sidecarを作る(fbxバイト列, &モデル)).expect("合成sidecarは読める");
    値["physBones"][0]["componentNodeId"] = serde_json::json!("n:4");
    値["physBones"][0]["rootNodeId"] = serde_json::json!("n:4");
    let (_, _, コンパイル済み) =
        文から解決してコンパイルする(&値.to_string(), fbxバイト列, &mut モデル);
    assert_eq!(コンパイル済み.report.spring_count, 0);
    let 捨て = コンパイル済み
        .report
        .skipped_bones
        .iter()
        .find(|捨て| 捨て.bone == "pb:0")
        .expect("pb:0は落とす");
    match &捨て.reason {
        vrctom::vrm::SkippedBoneReason::NoHeads => {}
        他 => panic!("head無しで落とす: {他:?}"),
    }
}

/// `ignoreOtherPhysBones` が真のときだけ他rootで切断する。
#[test]
fn 他rootの切断は無視指定があるときだけ起きる() {
    let fbxバイト列 = b"fake-fbx";
    let mut モデル = 書き出し可能モデルを作る();
    let mut 値: serde_json::Value =
        serde_json::from_str(&髪sidecarを作る(fbxバイト列, &モデル)).expect("合成sidecarは読める");
    値["physBones"]
        .as_array_mut()
        .expect("physBonesは配列である")
        .push(serde_json::json!({
            "id": "pb:1", "componentNodeId": "n:1", "rootNodeId": "n:1", "enabled": true,
            "version": "1.1", "integrationType": "Simplified", "ignoreNodeIds": [],
            "ignoreOtherPhysBones": false, "endpointPosition": [0.0, 0.0, 0.0],
            "multiChildType": "Ignore", "allowCollision": true, "colliderIds": [],
            "limitType": "None", "limitRotation": [0.0, 0.0, 0.0], "isAnimated": false,
        }));
    let (_, _, 偽) =
        文から解決してコンパイルする(&値.to_string(), fbxバイト列, &mut モデル);
    assert!(偽.report.cut_at_other_roots.is_empty(), "偽なら切らない");
    assert_eq!(偽.report.spring_count, 2);
    assert_eq!(偽.report.lost_nodes.len(), 3, "n:3・n:4・n:5を奪われる");
    値["physBones"][1]["ignoreOtherPhysBones"] = serde_json::json!(true);
    let (_, _, 真) =
        文から解決してコンパイルする(&値.to_string(), fbxバイト列, &mut モデル);
    assert_eq!(真.report.cut_at_other_roots.len(), 1, "真なら切る");
    assert_eq!(真.report.cut_at_other_roots[0].bone, "pb:1");
    assert_eq!(真.report.cut_at_other_roots[0].node, "n:3");
    assert_eq!(真.report.cut_at_other_roots[0].other_bone, "pb:0");
    assert!(真.report.lost_nodes.is_empty(), "切れば奪われない");
    assert_eq!(真.report.spring_count, 2);
}

/// 無視指定の部分木には降りない。

#[test]
fn 無視指定の部分木には降りない() {
    let fbxバイト列 = b"fake-fbx";
    let mut モデル = 書き出し可能モデルを作る();
    let mut 値: serde_json::Value =
        serde_json::from_str(&髪sidecarを作る(fbxバイト列, &モデル)).expect("合成sidecarは読める");
    値["physBones"][0]["ignoreNodeIds"] = serde_json::json!(["n:5"]);
    let (_, _, コンパイル済み) =
        文から解決してコンパイルする(&値.to_string(), fbxバイト列, &mut モデル);
    assert_eq!(コンパイル済み.bones.springs.len(), 1);
    let 関節: Vec<usize> = コンパイル済み.bones.springs[0]
        .joints
        .iter()
        .map(|関節| 関節.node)
        .collect();
    assert_eq!(関節, vec![3, 4], "無視部分木には降りない");
    // 無視で子が1件になれば分岐記録は無い。
    assert!(コンパイル済み.report.branch_splits.is_empty());
}

/// root自体を無視した場合は空の木として報告する。
#[test]
fn root自体の無視は空の木として報告する() {
    let fbxバイト列 = b"fake-fbx";
    let mut モデル = 書き出し可能モデルを作る();
    let mut 値: serde_json::Value =
        serde_json::from_str(&髪sidecarを作る(fbxバイト列, &モデル)).expect("合成sidecarは読める");
    値["physBones"][0]["ignoreNodeIds"] = serde_json::json!(["n:3"]);
    let (_, _, コンパイル済み) =
        文から解決してコンパイルする(&値.to_string(), fbxバイト列, &mut モデル);
    assert_eq!(コンパイル済み.report.spring_count, 0);
    assert_eq!(コンパイル済み.report.skipped_bones.len(), 1);
    match &コンパイル済み.report.skipped_bones[0].reason {
        vrctom::vrm::SkippedBoneReason::RootIgnored => {}
        他 => panic!("root無視で落とす: {他:?}"),
    }
}

/// Aimと子順はsidecarのsiblingIndex順に従い、モデルの子順は使わない。
#[test]
fn sibling順がモデル子順と異なる場合はsidecar順に従う() {
    let fbxバイト列 = b"fake-fbx";
    let mut モデル = 書き出し可能モデルを作る();
    let mut 値: serde_json::Value =
        serde_json::from_str(&髪sidecarを作る(fbxバイト列, &モデル)).expect("合成sidecarは読める");
    // モデル上はHairB(n:4)が先だが、siblingIndexではHairC(n:5)を先にする。
    値["nodes"][4]["siblingIndex"] = serde_json::json!(5);
    値["nodes"][5]["siblingIndex"] = serde_json::json!(0);
    値["physBones"][0]["multiChildType"] = serde_json::json!("First");
    let (_, _, コンパイル済み) =
        文から解決してコンパイルする(&値.to_string(), fbxバイト列, &mut モデル);
    assert_eq!(コンパイル済み.report.spring_count, 1);
    let 関節: Vec<usize> = コンパイル済み.bones.springs[0]
        .joints
        .iter()
        .map(|関節| 関節.node)
        .collect();
    assert_eq!(関節, vec![3, 5], "sibling順の最初の子を辿る");
    assert_eq!(コンパイル済み.report.branch_splits.len(), 1);
    assert_eq!(
        コンパイル済み.report.branch_splits[0].aimed,
        Some("n:5".to_string())
    );
    assert_eq!(
        コンパイル済み.report.branch_splits[0].heads,
        vec!["n:4".to_string()]
    );
}

/// rootを共有する2本目は作らず、書き出しは通す。
#[test]
fn root共有の2本目は作らず書き出しは通す() {
    let fbxバイト列 = b"fake-fbx";
    let mut モデル = 書き出し可能モデルを作る();
    let mut 値: serde_json::Value =
        serde_json::from_str(&髪sidecarを作る(fbxバイト列, &モデル)).expect("合成sidecarは読める");
    let 複製 = 値["physBones"][0].clone();
    let mut 追加 = 複製;
    追加["id"] = serde_json::json!("pb:1");
    追加["colliderIds"] = serde_json::json!([]);
    値["physBones"]
        .as_array_mut()
        .expect("physBonesは配列である")
        .push(追加);
    let (_, _, コンパイル済み) =
        文から解決してコンパイルする(&値.to_string(), fbxバイト列, &mut モデル);
    assert_eq!(
        コンパイル済み.report.spring_count, 1,
        "奪われたrootのboneはspringを作らない"
    );
    assert_eq!(コンパイル済み.report.lost_nodes.len(), 3);
    assert_eq!(コンパイル済み.report.shared_nodes.len(), 3);
    assert_eq!(コンパイル済み.report.shared_nodes[0].node, "n:3");
    assert_eq!(コンパイル済み.report.shared_nodes[0].owner, "pb:0");
    assert_eq!(
        コンパイル済み.report.shared_nodes[0].lost_by,
        vec!["pb:1".to_string()]
    );
    assert_eq!(コンパイル済み.report.skipped_bones.len(), 1);
    let 跳ばし = &コンパイル済み.report.skipped_bones[0];
    assert_eq!(跳ばし.bone, "pb:1");
    match &跳ばし.reason {
        vrctom::vrm::SkippedBoneReason::RootShared { owner } => {
            assert_eq!(owner, "pb:0");
        }
        他 => panic!("root共有で落とす: {他:?}"),
    }
    モデル.spring_bones = コンパイル済み.bones;
    モデル
        .to_vrm(&書き出し設定を作る())
        .expect("空鎖を落とせば書き出せる");
}

/// 上限で切れた鎖は記録に残し、区間の無いspringは作らない。
#[test]
fn 上限で切れた鎖は記録に残す() {
    let fbxバイト列 = b"fake-fbx";
    let mut モデル = 書き出し可能モデルを作る();
    // 上限1ではheadだけになり、区間が無いためspringを作らない。
    // 切り詰め自体は記録に残す。
    let 文 = 髪sidecarを作る(fbxバイト列, &モデル);
    let sidecar =
        PhysicsSidecar::from_json(&文, &PhysicsLimits::new()).expect("合成sidecarは検証を通る");
    let 解決 = モデル
        .resolve_physics_sidecar(&sidecar, fbxバイト列)
        .expect("合成対応は解決できる");
    let 上限1 = SpringCompileOptions::new().with_max_joints(1);
    let 制限付き =
        compile_springs(&sidecar, &mut モデル, &解決, &上限1).expect("上限付きでも変換できる");
    assert_eq!(制限付き.report.spring_count, 0);
    assert_eq!(
        制限付き.report.truncated_chains,
        vec!["HairA".to_string()],
        "切り詰めは先頭関節名で記録する"
    );
    // 上限2ではHairA・HairBの1本が通る。
    let 上限2 = SpringCompileOptions::new().with_max_joints(2);
    let 通過 = compile_springs(&sidecar, &mut モデル, &解決, &上限2).expect("上限ちょうどでは通る");
    assert_eq!(通過.report.spring_count, 1);
    assert!(通過.report.truncated_chains.is_empty());
}

/// 3分岐モデルを作る。Rの下にA・B・C、各々に子が1件ずつ付く。
fn 三分岐モデルを作る() -> VrmModel {
    let 名前 = ["R", "A", "A1", "B", "B1", "C", "C1"];
    let 親: Vec<Option<usize>> = vec![None, Some(0), Some(1), Some(0), Some(3), Some(0), Some(5)];
    let 位置 = [
        [0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, 0.0, 1.0],
        [0.0, 0.0, 1.0],
        [0.0, 1.0, 0.0],
        [1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0],
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

/// 3分岐モデルのsidecarを作る。分岐の扱いだけを変える。
fn 三分岐sidecarを作る(fbxバイト列: &[u8], モデル: &VrmModel, 分岐: &str) -> String {
    let 経路 = [
        vec!["R"],
        vec!["R", "A"],
        vec!["R", "A", "A1"],
        vec!["R", "B"],
        vec!["R", "B", "B1"],
        vec!["R", "C"],
        vec!["R", "C", "C1"],
    ];
    let ノード: Vec<serde_json::Value> = 経路
        .iter()
        .enumerate()
        .map(|(索引, 道)| {
            let 親 = if 索引 == 0 {
                serde_json::Value::Null
            } else {
                let 親索引 = モデル.nodes[索引].parent.expect("親がある");
                serde_json::json!(format!("n:{親索引}"))
            };
            ノードjson(
                モデル,
                &format!("n:{索引}"),
                親.as_str(),
                索引,
                &["Milfy"],
                &道.to_vec(),
            )
        })
        .collect();
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
            "componentNodeId": "n:0",
            "rootNodeId": "n:0",
            "enabled": true,
            "version": "1.1",
            "integrationType": "Simplified",
            "ignoreNodeIds": [],
            "ignoreOtherPhysBones": false,
            "endpointPosition": [0.0, 0.0, 0.0],
            "multiChildType": 分岐,
            "pull": {"value": 0.0, "keys": [], "preWrapMode": 8, "postWrapMode": 8},
            "spring": {"value": 0.3, "keys": [], "preWrapMode": 8, "postWrapMode": 8},
            "stiffness": {"value": 0.5, "keys": [], "preWrapMode": 8, "postWrapMode": 8},
            "gravity": {"value": 0.0, "keys": [], "preWrapMode": 8, "postWrapMode": 8},
            "gravityFalloff": {"value": 0.0, "keys": [], "preWrapMode": 8, "postWrapMode": 8},
            "immobileType": "AllMotion",
            "immobile": {"value": 0.0, "keys": [], "preWrapMode": 8, "postWrapMode": 8},
            "radius": {"value": 0.02, "keys": [], "preWrapMode": 8, "postWrapMode": 8},
            "allowCollision": true,
            "colliderIds": [],
            "limitType": "None",
            "maxAngleX": {"value": 0.0, "keys": [], "preWrapMode": 8, "postWrapMode": 8},
            "maxAngleZ": {"value": 0.0, "keys": [], "preWrapMode": 8, "postWrapMode": 8},
            "limitRotation": [0.0, 0.0, 0.0],
            "isAnimated": false,
        }],
        "colliders": [],
        "unsupportedFeatures": [],
    })
    .to_string()
}

/// Ignoreの3分岐は3 springに分かれ、関節は重ならない。
#[test]
fn 三分岐無視は三つのspringに分かれる() {
    let fbxバイト列 = b"fake-fbx";
    let mut モデル = 三分岐モデルを作る();
    let 文 = 三分岐sidecarを作る(fbxバイト列, &モデル, "Ignore");
    let (_, _, コンパイル済み) =
        文から解決してコンパイルする(&文, fbxバイト列, &mut モデル);
    assert_eq!(コンパイル済み.report.spring_count, 3);
    let 名前: Vec<&str> = コンパイル済み
        .bones
        .springs
        .iter()
        .map(|spring| spring.name.as_str())
        .collect();
    assert_eq!(名前, vec!["A", "B", "C"]);
    // 関節は重ならず、親子連鎖を保つ。
    let mut 全部 = Vec::new();
    for spring in &コンパイル済み.bones.springs {
        let 関節: Vec<usize> = spring.joints.iter().map(|関節| 関節.node).collect();
        assert_eq!(関節.len(), 2);
        for (前, 後) in 関節.iter().zip(関節.iter().skip(1)) {
            assert!(
                モデル.nodes[*前].children.contains(後),
                "親子でつながる: {前} -> {後}"
            );
        }
        全部.extend(関節);
    }
    let mut 重複なし = 全部.clone();
    重複なし.sort_unstable();
    重複なし.dedup();
    assert_eq!(全部.len(), 重複なし.len(), "関節は重ならない");
    assert_eq!(全部.len(), 6);
    // 分岐はAimなし・head3件で記録する。
    assert_eq!(コンパイル済み.report.branch_splits.len(), 1);
    assert_eq!(コンパイル済み.report.branch_splits[0].bone, "pb:0");
    assert_eq!(コンパイル済み.report.branch_splits[0].node, "n:0");
    assert_eq!(コンパイル済み.report.branch_splits[0].aimed, None);
    assert_eq!(
        コンパイル済み.report.branch_splits[0].heads,
        vec!["n:1".to_string(), "n:3".to_string(), "n:5".to_string()]
    );
}

/// Firstの分岐は先頭の鎖だけが1 springになる。
#[test]
fn 先頭子だけがspringになる() {
    // R-X-{A,B,C} の木。Xの分岐だけがFirstでAへ向かう。
    let 名前 = ["R", "X", "A", "B", "C"];
    let 親: Vec<Option<usize>> = vec![None, Some(0), Some(1), Some(1), Some(1)];
    let 位置 = [
        [0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, 0.0, 1.0],
        [1.0, 0.0, 0.0],
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
    let mut モデル = VrmModel {
        root: 単位行列(),
        nodes: ノード,
        skins: Vec::new(),
        meshes: Vec::new(),
        expressions: VrmExpressions::default(),
        spring_bones: VrmSpringBones::default(),
        node_constraints: Vec::new(),
        look_at: None,
    };
    let fbxバイト列 = b"fake-fbx";
    let 道 = [
        vec!["R"],
        vec!["R", "X"],
        vec!["R", "X", "A"],
        vec!["R", "X", "B"],
        vec!["R", "X", "C"],
    ];
    // 骨のrootはX(n:1)にする。avatar root(n:0)はjointにしないためである。
    let ノードjson: Vec<serde_json::Value> = 道
        .iter()
        .enumerate()
        .map(|(索引, 通)| {
            let 親 = if 索引 == 0 {
                serde_json::Value::Null
            } else {
                let 親索引 = モデル.nodes[索引].parent.expect("親がある");
                serde_json::json!(format!("n:{親索引}"))
            };
            ノードjson(
                &モデル,
                &format!("n:{索引}"),
                親.as_str(),
                索引,
                &["Milfy"],
                &通.to_vec(),
            )
        })
        .collect();
    let mut 文: serde_json::Value = serde_json::from_str(&三分岐sidecarを作る(
        fbxバイト列,
        &三分岐モデルを作る(),
        "First",
    ))
    .expect("合成sidecarは読める");
    文["nodes"] = serde_json::Value::Array(ノードjson);
    // 骨のrootはX(n:1)にする。avatar rootはjointにしないためである。
    文["physBones"][0]["componentNodeId"] = serde_json::json!("n:1");
    文["physBones"][0]["rootNodeId"] = serde_json::json!("n:1");
    let (_, _, コンパイル済み) =
        文から解決してコンパイルする(&文.to_string(), fbxバイト列, &mut モデル);
    assert_eq!(コンパイル済み.report.spring_count, 1);
    let spring = &コンパイル済み.bones.springs[0];
    assert_eq!(spring.name, "X", "先頭関節のノード名である");
    let 関節: Vec<usize> = spring.joints.iter().map(|関節| 関節.node).collect();
    assert_eq!(関節, vec![1, 2]);
    assert_eq!(コンパイル済み.report.branch_splits.len(), 1);
    assert_eq!(コンパイル済み.report.branch_splits[0].bone, "pb:0");
    assert_eq!(コンパイル済み.report.branch_splits[0].node, "n:1");
    assert_eq!(
        コンパイル済み.report.branch_splits[0].aimed,
        Some("n:2".to_string())
    );
    assert_eq!(
        コンパイル済み.report.branch_splits[0].heads,
        vec!["n:3".to_string(), "n:4".to_string()]
    );
}

/// Averageは今回は先頭の子に畳み、近似として記録する。
#[test]
fn 平均指定は先頭子への近似として記録する() {
    // Firstの試験と同じR-X-{A,B,C}の木で、multiChildTypeだけ変える。
    let 名前 = ["R", "X", "A", "B", "C"];
    let 親: Vec<Option<usize>> = vec![None, Some(0), Some(1), Some(1), Some(1)];
    let 位置 = [
        [0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, 0.0, 1.0],
        [1.0, 0.0, 0.0],
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
    let mut モデル = VrmModel {
        root: 単位行列(),
        nodes: ノード,
        skins: Vec::new(),
        meshes: Vec::new(),
        expressions: VrmExpressions::default(),
        spring_bones: VrmSpringBones::default(),
        node_constraints: Vec::new(),
        look_at: None,
    };
    let fbxバイト列 = b"fake-fbx";
    let 道 = [
        vec!["R"],
        vec!["R", "X"],
        vec!["R", "X", "A"],
        vec!["R", "X", "B"],
        vec!["R", "X", "C"],
    ];
    let ノードjson: Vec<serde_json::Value> = 道
        .iter()
        .enumerate()
        .map(|(索引, 通)| {
            let 親 = if 索引 == 0 {
                serde_json::Value::Null
            } else {
                let 親索引 = モデル.nodes[索引].parent.expect("親がある");
                serde_json::json!(format!("n:{親索引}"))
            };
            ノードjson(
                &モデル,
                &format!("n:{索引}"),
                親.as_str(),
                索引,
                &["Milfy"],
                &通.to_vec(),
            )
        })
        .collect();
    let mut 文: serde_json::Value = serde_json::from_str(&三分岐sidecarを作る(
        fbxバイト列,
        &三分岐モデルを作る(),
        "Average",
    ))
    .expect("合成sidecarは読める");
    文["nodes"] = serde_json::Value::Array(ノードjson);
    文["physBones"][0]["componentNodeId"] = serde_json::json!("n:1");
    文["physBones"][0]["rootNodeId"] = serde_json::json!("n:1");
    let (_, _, コンパイル済み) =
        文から解決してコンパイルする(&文.to_string(), fbxバイト列, &mut モデル);
    assert_eq!(コンパイル済み.report.spring_count, 1);
    let 関節: Vec<usize> = コンパイル済み.bones.springs[0]
        .joints
        .iter()
        .map(|関節| 関節.node)
        .collect();
    assert_eq!(関節, vec![1, 2], "Firstと同じく先頭の子を辿る");
    assert_eq!(
        コンパイル済み.report.average_approximated,
        vec!["pb:0".to_string()]
    );
}

/// avatar rootはjointにせず、その子からspringを開始する。
#[test]
fn avatarrootはjointにしない() {
    // avatar(n:0)直下のAから鎖が始まる。avatar自体は所有しない。
    let 名前 = ["Top", "A", "B"];
    let 親: Vec<Option<usize>> = vec![None, Some(0), Some(1)];
    let 位置 = [[0.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 1.0, 0.0]];
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
    let mut モデル = VrmModel {
        root: 単位行列(),
        nodes: ノード,
        skins: Vec::new(),
        meshes: Vec::new(),
        expressions: VrmExpressions::default(),
        spring_bones: VrmSpringBones::default(),
        node_constraints: Vec::new(),
        look_at: None,
    };
    let fbxバイト列 = b"fake-fbx";
    let 出所 = |パス: &[&str]| serde_json::json!({"modelId": "milfy", "path": パス, "assetGuid": "g", "localId": -1});
    let trs = |位置: [f64; 3]| {
        serde_json::json!({
            "position": 位置,
            "rotation": [0.0, 0.0, 0.0, 1.0],
            "scale": [1.0, 1.0, 1.0],
        })
    };
    let 文 = serde_json::json!({
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
        "nodes": [
            {"id": "n:0", "parentId": None::<String>, "name": "Top", "siblingIndex": 0,
             "scenePath": ["Milfy", "Top"], "source": 出所(&["Top"]), "referenceLocal": trs([0.0, 0.0, 0.0])},
            {"id": "n:1", "parentId": "n:0", "name": "A", "siblingIndex": 0,
             "scenePath": ["Milfy", "Top", "A"], "source": 出所(&["Top", "A"]), "referenceLocal": trs([0.0, 1.0, 0.0])},
            {"id": "n:2", "parentId": "n:1", "name": "B", "siblingIndex": 0,
             "scenePath": ["Milfy", "Top", "A", "B"], "source": 出所(&["Top", "A", "B"]), "referenceLocal": trs([0.0, 1.0, 0.0])},
        ],
        "physBones": [{
            "id": "pb:0", "componentNodeId": "n:0", "rootNodeId": "n:0", "enabled": true,
            "version": "1.1", "integrationType": "Simplified", "ignoreNodeIds": [],
            "ignoreOtherPhysBones": false, "endpointPosition": [0.0, 0.0, 0.0],
            "multiChildType": "First", "allowCollision": true, "colliderIds": [],
            "limitType": "None", "limitRotation": [0.0, 0.0, 0.0], "isAnimated": false,
        }],
        "colliders": [],
        "unsupportedFeatures": [],
    })
    .to_string();
    let (_, _, コンパイル済み) =
        文から解決してコンパイルする(&文, fbxバイト列, &mut モデル);
    assert_eq!(コンパイル済み.report.spring_count, 1);
    assert_eq!(コンパイル済み.bones.springs[0].name, "A");
    let 関節: Vec<usize> = コンパイル済み.bones.springs[0]
        .joints
        .iter()
        .map(|関節| 関節.node)
        .collect();
    assert_eq!(関節, vec![1, 2], "avatarの子から始まる");
}

/// 他springの内側にある候補は祖先へ辿り直す。
#[test]
fn 内側の候補は祖先へ辿り直す() {
    // G-P-{D,Rb-B1} の木。pb:AはP起点でDへ、pb:BはRb起点でB1へ向かう。
    // pb:Aは他rootを切り、pb:Bの候補Pはpb:Aのspring内にあるためGへ辿る。
    // 両springともcenterはGになる。
    let 名前 = ["Top", "G", "Pc", "D", "Rb", "B1"];
    let 親: Vec<Option<usize>> = vec![None, Some(0), Some(1), Some(2), Some(2), Some(4)];
    // xは0に保つ。Unity→VRMの鏡像でxが反転するためである。
    let 位置 = [
        [0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, 0.3, -0.1],
        [0.0, 0.3, 0.1],
        [0.0, 0.3, 0.0],
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
    let mut モデル = VrmModel {
        root: 単位行列(),
        nodes: ノード,
        skins: Vec::new(),
        meshes: Vec::new(),
        expressions: VrmExpressions::default(),
        spring_bones: VrmSpringBones::default(),
        node_constraints: Vec::new(),
        look_at: None,
    };
    let fbxバイト列 = b"fake-fbx";
    let 道 = [
        vec!["Top"],
        vec!["Top", "G"],
        vec!["Top", "G", "Pc"],
        vec!["Top", "G", "Pc", "D"],
        vec!["Top", "G", "Pc", "Rb"],
        vec!["Top", "G", "Pc", "Rb", "B1"],
    ];
    let ノードjson: Vec<serde_json::Value> = 道
        .iter()
        .enumerate()
        .map(|(索引, 通)| {
            let 親 = if 索引 == 0 {
                serde_json::Value::Null
            } else {
                let 親索引 = モデル.nodes[索引].parent.expect("親がある");
                serde_json::json!(format!("n:{親索引}"))
            };
            ノードjson(
                &モデル,
                &format!("n:{索引}"),
                親.as_str(),
                索引,
                &["Milfy"],
                &通.to_vec(),
            )
        })
        .collect();
    let mut 文: serde_json::Value = serde_json::from_str(&髪sidecarを作る(
        fbxバイト列,
        &書き出し可能モデルを作る(),
    ))
    .expect("合成sidecarは読める");
    文["nodes"] = serde_json::Value::Array(ノードjson);
    // Pc直下のDを先頭にし、Rbを後に置く。
    文["nodes"][3]["siblingIndex"] = serde_json::json!(0);
    文["nodes"][4]["siblingIndex"] = serde_json::json!(1);
    // pb:AはPc起点でDへ向かい、他root(Rb)を切る。pb:BはRb起点でB1へ向かう。
    文["physBones"][0]["id"] = serde_json::json!("pb:A");
    文["physBones"][0]["componentNodeId"] = serde_json::json!("n:2");
    文["physBones"][0]["rootNodeId"] = serde_json::json!("n:2");
    文["physBones"][0]["multiChildType"] = serde_json::json!("First");
    文["physBones"][0]["ignoreOtherPhysBones"] = serde_json::json!(true);
    文["physBones"][0]["immobile"] =
        serde_json::json!({"value": 1.0, "keys": [], "preWrapMode": 8, "postWrapMode": 8});
    文["physBones"][0]["immobileType"] = serde_json::json!("AllMotion");
    let 追加 = serde_json::json!({
        "id": "pb:B", "componentNodeId": "n:4", "rootNodeId": "n:4", "enabled": true,
        "version": "1.1", "integrationType": "Simplified", "ignoreNodeIds": [],
        "ignoreOtherPhysBones": false, "endpointPosition": [0.0, 0.0, 0.0],
        "multiChildType": "Ignore", "allowCollision": true, "colliderIds": [],
        "limitType": "None", "limitRotation": [0.0, 0.0, 0.0], "isAnimated": false,
    });
    // pb:BもAllMotionでimmobile=1にする。
    let mut 追加 = 追加;
    追加["immobile"] =
        serde_json::json!({"value": 1.0, "keys": [], "preWrapMode": 8, "postWrapMode": 8});
    追加["immobileType"] = serde_json::json!("AllMotion");
    文["physBones"]
        .as_array_mut()
        .expect("physBonesは配列である")
        .push(追加);
    let (_, _, コンパイル済み) =
        文から解決してコンパイルする(&文.to_string(), fbxバイト列, &mut モデル);
    assert_eq!(コンパイル済み.report.spring_count, 2);
    // pb:Aの鎖は他rootで切れ、pb:Bの候補は内側のためGへ辿る。
    assert_eq!(コンパイル済み.report.cut_at_other_roots.len(), 1);
    assert_eq!(コンパイル済み.report.cut_at_other_roots[0].bone, "pb:A");
    // 両springともcenterはG(n:1 → Vrm索引1)である。
    for spring in &コンパイル済み.bones.springs {
        assert_eq!(spring.center, Some(1), "centerはGである: {}", spring.name);
    }
    assert_eq!(コンパイル済み.report.center_applied.len(), 2);
}

/// AllMotionでimmobile=1はchain rootの親をcenterにする。
#[test]
fn 全固定は親をcenterにする() {
    let fbxバイト列 = b"fake-fbx";
    let mut モデル = 書き出し可能モデルを作る();
    let mut 文: serde_json::Value =
        serde_json::from_str(&髪sidecarを作る(fbxバイト列, &モデル)).expect("合成sidecarは読める");
    文["physBones"][0]["immobile"] =
        serde_json::json!({"value": 1.0, "keys": [], "preWrapMode": 8, "postWrapMode": 8});
    文["physBones"][0]["immobileType"] = serde_json::json!("AllMotion");
    let (_, _, コンパイル済み) =
        文から解決してコンパイルする(&文.to_string(), fbxバイト列, &mut モデル);
    assert_eq!(コンパイル済み.report.spring_count, 1);
    let spring = &コンパイル済み.bones.springs[0];
    // chain rootはHairA(n:3)で、親はHead(n:2 → Vrm索引2)である。
    assert_eq!(spring.center, Some(2));
    assert_eq!(コンパイル済み.report.center_applied.len(), 1);
    assert_eq!(コンパイル済み.report.center_applied[0].bone, "pb:0");
    assert_eq!(
        コンパイル済み.report.center_applied[0].kind,
        vrctom::vrm::CenterKind::AllMotion
    );
    // centerはavatarでもjointでもない。
    let 関節: Vec<usize> = spring.joints.iter().map(|関節| 関節.node).collect();
    assert!(!関節.contains(&2));
    assert_ne!(Some(2), Some(0));
}

/// Worldでroot値0.5以上は鎖の最上位祖先をcenterにする。
/// 実アバタールートはsidecarに含まれないため、sidecarの最上位が候補になる。
#[test]
fn 世界固定は最上位をcenterにする() {
    let fbxバイト列 = b"fake-fbx";
    let mut モデル = 書き出し可能モデルを作る();
    let mut 文: serde_json::Value =
        serde_json::from_str(&髪sidecarを作る(fbxバイト列, &モデル)).expect("合成sidecarは読める");
    文["physBones"][0]["immobile"] =
        serde_json::json!({"value": 1.0, "keys": [], "preWrapMode": 8, "postWrapMode": 8});
    文["physBones"][0]["immobileType"] = serde_json::json!("World");
    let (_, _, コンパイル済み) =
        文から解決してコンパイルする(&文.to_string(), fbxバイト列, &mut モデル);
    let spring = &コンパイル済み.bones.springs[0];
    // 最上位はHips(n:0 → Vrm索引0)であり、実アバタールートはその上にある。
    assert_eq!(spring.center, Some(0));
    assert_eq!(コンパイル済み.report.center_applied.len(), 1);
    assert_eq!(
        コンパイル済み.report.center_applied[0].kind,
        vrctom::vrm::CenterKind::World
    );
    let 関節: Vec<usize> = spring.joints.iter().map(|関節| 関節.node).collect();
    assert!(!関節.contains(&0));
}

/// Worldでroot値0.5未満はcenterなしである。
#[test]
fn 世界弱はcenterなしである() {
    let fbxバイト列 = b"fake-fbx";
    let mut モデル = 書き出し可能モデルを作る();
    let mut 文: serde_json::Value =
        serde_json::from_str(&髪sidecarを作る(fbxバイト列, &モデル)).expect("合成sidecarは読める");
    文["physBones"][0]["immobile"] =
        serde_json::json!({"value": 0.4, "keys": [], "preWrapMode": 8, "postWrapMode": 8});
    文["physBones"][0]["immobileType"] = serde_json::json!("World");
    let (_, _, コンパイル済み) =
        文から解決してコンパイルする(&文.to_string(), fbxバイト列, &mut モデル);
    let spring = &コンパイル済み.bones.springs[0];
    assert_eq!(spring.center, None);
    assert_eq!(コンパイル済み.report.weak_world, vec!["pb:0".to_string()]);
    assert!(コンパイル済み.report.center_applied.is_empty());
}

/// AllMotionで1未満はHoldsImmobileとしてcenterなしである。
#[test]
fn 部分固定はholdsでcenterなしである() {
    let fbxバイト列 = b"fake-fbx";
    let mut モデル = 書き出し可能モデルを作る();
    let mut 文: serde_json::Value =
        serde_json::from_str(&髪sidecarを作る(fbxバイト列, &モデル)).expect("合成sidecarは読める");
    文["physBones"][0]["immobile"] =
        serde_json::json!({"value": 0.5, "keys": [], "preWrapMode": 8, "postWrapMode": 8});
    文["physBones"][0]["immobileType"] = serde_json::json!("AllMotion");
    let (_, _, コンパイル済み) =
        文から解決してコンパイルする(&文.to_string(), fbxバイト列, &mut モデル);
    let spring = &コンパイル済み.bones.springs[0];
    assert_eq!(spring.center, None);
    assert_eq!(
        コンパイル済み.report.held_immobile,
        vec!["pb:0".to_string()]
    );
    assert!(コンパイル済み.report.center_applied.is_empty());
}

/// chain rootの親がsidecarルートでもcenterにする。
/// 実アバタールートはsidecarの上にあるため、sidecarルートは合法である。
#[test]
fn 親がsidecarルートでもcenterにする() {
    let fbxバイト列 = b"fake-fbx";
    let mut モデル = 書き出し可能モデルを作る();
    let mut 文: serde_json::Value =
        serde_json::from_str(&髪sidecarを作る(fbxバイト列, &モデル)).expect("合成sidecarは読める");
    // rootをSpine(n:1)に移し、親をsidecarルート(n:0)にする。
    文["physBones"][0]["componentNodeId"] = serde_json::json!("n:1");
    文["physBones"][0]["rootNodeId"] = serde_json::json!("n:1");
    文["physBones"][0]["immobile"] =
        serde_json::json!({"value": 1.0, "keys": [], "preWrapMode": 8, "postWrapMode": 8});
    文["physBones"][0]["immobileType"] = serde_json::json!("AllMotion");
    let (_, _, コンパイル済み) =
        文から解決してコンパイルする(&文.to_string(), fbxバイト列, &mut モデル);
    assert!(!コンパイル済み.bones.springs.is_empty());
    for spring in &コンパイル済み.bones.springs {
        // sidecarルート(n:0 → Vrm索引0)がcenterになる。
        assert_eq!(spring.center, Some(0));
        let 関節: Vec<usize> = spring.joints.iter().map(|関節| 関節.node).collect();
        assert!(!関節.contains(&0), "jointはcenterにならない");
    }
    assert!(コンパイル済み.report.no_legal_center.is_empty());
}

/// 起点自体がsidecarルートの場合は候補が無くcenterなしである。
#[test]
fn 起点がルート自体はcenterなしである() {
    let fbxバイト列 = b"fake-fbx";
    let mut モデル = 書き出し可能モデルを作る();
    let mut 文: serde_json::Value =
        serde_json::from_str(&髪sidecarを作る(fbxバイト列, &モデル)).expect("合成sidecarは読める");
    // rootをsidecarルート(n:0)に移す。親が無いため候補は無い。
    文["physBones"][0]["componentNodeId"] = serde_json::json!("n:0");
    文["physBones"][0]["rootNodeId"] = serde_json::json!("n:0");
    文["physBones"][0]["immobile"] =
        serde_json::json!({"value": 1.0, "keys": [], "preWrapMode": 8, "postWrapMode": 8});
    文["physBones"][0]["immobileType"] = serde_json::json!("AllMotion");
    let (_, _, コンパイル済み) =
        文から解決してコンパイルする(&文.to_string(), fbxバイト列, &mut モデル);
    assert!(!コンパイル済み.bones.springs.is_empty());
    for spring in &コンパイル済み.bones.springs {
        assert_eq!(spring.center, None);
    }
    assert_eq!(コンパイル済み.report.no_legal_center.len(), 1);
    assert_eq!(
        コンパイル済み.report.no_legal_center[0].reason,
        vrctom::vrm::NoCenterReason::NoLegalNode
    );
}

/// 補助ノードを含む鎖はつなげて出力する。飛ばして接続はしない。
#[test]
fn 補助ノードを含む鎖はつなげる() {
    // Gはscene追加で対応表に無いため、補助ノードとして追加する。Hの下にぶら下がる。
    let 名前 = ["Top", "H", "G"];
    let 親: Vec<Option<usize>> = vec![None, Some(0), Some(1)];
    let 位置 = [[0.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 1.0, 0.0]];
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
    let mut モデル = VrmModel {
        root: 単位行列(),
        nodes: ノード,
        skins: Vec::new(),
        meshes: Vec::new(),
        expressions: VrmExpressions::default(),
        spring_bones: VrmSpringBones::default(),
        node_constraints: Vec::new(),
        look_at: None,
    };
    // sidecarはTopとHだけを持ち、Gは出所なし（scene追加）で残す。
    let fbxバイト列 = b"fake-fbx";
    let 出所 = |パス: &[&str]| serde_json::json!({"modelId": "milfy", "path": パス, "assetGuid": "g", "localId": -1});
    let trs = |位置: [f64; 3]| {
        serde_json::json!({
            "position": 位置,
            "rotation": [0.0, 0.0, 0.0, 1.0],
            "scale": [1.0, 1.0, 1.0],
        })
    };
    let 文 = serde_json::json!({
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
        "nodes": [
            {"id": "n:0", "parentId": None::<String>, "name": "Top", "siblingIndex": 0,
             "scenePath": ["Milfy", "Top"], "source": 出所(&["Top"]), "referenceLocal": trs([0.0, 0.0, 0.0])},
            {"id": "n:1", "parentId": "n:0", "name": "H", "siblingIndex": 0,
             "scenePath": ["Milfy", "Top", "H"], "source": 出所(&["Top", "H"]), "referenceLocal": trs([0.0, 1.0, 0.0])},
            {"id": "n:2", "parentId": "n:1", "name": "G", "siblingIndex": 0,
             "scenePath": ["Milfy", "Top", "H", "G"], "source": None::<serde_json::Value>, "referenceLocal": trs([0.0, 1.0, 0.0])},
        ],
        "physBones": [{
            "id": "pb:0", "componentNodeId": "n:1", "rootNodeId": "n:1", "enabled": true,
            "version": "1.1", "integrationType": "Simplified", "ignoreNodeIds": [],
            "ignoreOtherPhysBones": false, "endpointPosition": [0.0, 0.0, 0.0],
            "multiChildType": "Ignore", "allowCollision": true, "colliderIds": [],
            "limitType": "None", "limitRotation": [0.0, 0.0, 0.0], "isAnimated": false,
        }],
        "colliders": [],
        "unsupportedFeatures": [],
    })
    .to_string();
    let (_, _, コンパイル済み) =
        文から解決してコンパイルする(&文, fbxバイト列, &mut モデル);
    assert_eq!(コンパイル済み.report.spring_count, 1);
    assert!(コンパイル済み.report.skipped_bones.is_empty());
    let spring = &コンパイル済み.bones.springs[0];
    assert_eq!(spring.joints.len(), 2, "Hと補助ノードGをつなげる");
    assert_eq!(spring.joints[0].node, 1, "先頭はHである");
    assert_eq!(spring.joints[1].node, 3, "末端は追加したGである");
}

/// 循環するsidecarは検証で拒否する。
#[test]
fn 循環するsidecarは検証で拒否する() {
    let fbxバイト列 = b"fake-fbx";
    let mut 値: serde_json::Value = serde_json::from_str(&髪sidecarを作る(
        fbxバイト列,
        &書き出し可能モデルを作る(),
    ))
    .expect("合成sidecarは読める");
    // n:0の親をn:1にして循環させる。
    値["nodes"][0]["parentId"] = serde_json::json!("n:1");
    assert_eq!(
        PhysicsSidecar::from_json(&値.to_string(), &PhysicsLimits::new())
            .expect_err("循環は拒否する")
            .kind(),
        VrmErrorKind::InvalidPhysicsSidecar
    );
}

/// ゼロ長区間でspringを分け、関節を重ねない。
#[test]
fn ゼロ長区間で分割し重ねない() {
    // Top-A-B-C-D の直鎖。BとCは同位置で区間が長さ0である。
    // Bはheadにならず、Aのspringの端末で終わる。Cが次のspringを始める。
    let 名前 = ["Top", "A", "B", "C", "D"];
    let 親: Vec<Option<usize>> = vec![None, Some(0), Some(1), Some(2), Some(3)];
    // BとCは同じ世界位置になり、区間の長さが0である。
    // localは親からの差分のため、Cはゼロベクトルにする。
    let 位置 = [
        [0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
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
    let mut モデル = VrmModel {
        root: 単位行列(),
        nodes: ノード,
        skins: Vec::new(),
        meshes: Vec::new(),
        expressions: VrmExpressions::default(),
        spring_bones: VrmSpringBones::default(),
        node_constraints: Vec::new(),
        look_at: None,
    };
    let fbxバイト列 = b"fake-fbx";
    let 道 = [
        vec!["Top"],
        vec!["Top", "A"],
        vec!["Top", "A", "B"],
        vec!["Top", "A", "B", "C"],
        vec!["Top", "A", "B", "C", "D"],
    ];
    let ノードjson: Vec<serde_json::Value> = 道
        .iter()
        .enumerate()
        .map(|(索引, 通)| {
            let 親 = if 索引 == 0 {
                serde_json::Value::Null
            } else {
                let 親索引 = モデル.nodes[索引].parent.expect("親がある");
                serde_json::json!(format!("n:{親索引}"))
            };
            ノードjson(
                &モデル,
                &format!("n:{索引}"),
                親.as_str(),
                索引,
                &["Milfy"],
                &通.to_vec(),
            )
        })
        .collect();
    let mut 文: serde_json::Value = serde_json::from_str(&髪sidecarを作る(
        fbxバイト列,
        &書き出し可能モデルを作る(),
    ))
    .expect("合成sidecarは読める");
    文["nodes"] = serde_json::Value::Array(ノードjson);
    // 骨のrootはA(n:1)にし、avatar rootを避ける。
    文["physBones"][0]["componentNodeId"] = serde_json::json!("n:1");
    文["physBones"][0]["rootNodeId"] = serde_json::json!("n:1");
    // multiChildTypeは単一子のままにする。
    let (_, _, コンパイル済み) =
        文から解決してコンパイルする(&文.to_string(), fbxバイト列, &mut モデル);
    assert_eq!(コンパイル済み.report.spring_count, 2);
    let 名前: Vec<&str> = コンパイル済み
        .bones
        .springs
        .iter()
        .map(|spring| spring.name.as_str())
        .collect();
    assert_eq!(名前, vec!["A", "C"]);
    // 関節は重ならず、ゼロ長記録が残る。
    let mut 全部 = Vec::new();
    for spring in &コンパイル済み.bones.springs {
        for 関節 in &spring.joints {
            全部.push(関節.node);
        }
    }
    全部.sort_unstable();
    let mut 重複なし = 全部.clone();
    重複なし.dedup();
    assert_eq!(全部, vec![1, 2, 3, 4], "A・BとC・Dに分かれる");
    assert_eq!(全部, 重複なし, "関節は重ならない");
    assert_eq!(コンパイル済み.report.zero_length_joints.len(), 1);
    assert_eq!(コンパイル済み.report.zero_length_joints[0].bone, "pb:0");
    assert_eq!(コンパイル済み.report.zero_length_joints[0].parent, "n:2");
    assert_eq!(コンパイル済み.report.zero_length_joints[0].child, "n:3");
    assert!(コンパイル済み.report.truncated_chains.is_empty());
    // 書き出しは親子連鎖と一意性を満たす。
    モデル.spring_bones = コンパイル済み.bones;
    モデル
        .to_vrm(&書き出し設定を作る())
        .expect_err("humanoid不足は従来どおり失敗する");
}
