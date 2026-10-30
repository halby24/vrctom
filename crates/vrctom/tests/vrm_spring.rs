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
    モデル: &VrmModel,
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
    モデル: &VrmModel,
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
    let モデル = 書き出し可能モデルを作る();
    let (_, _, コンパイル済み) = 解決してコンパイルする(fbxバイト列, &モデル);
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
    // 第一近似の値を確かめる。
    for 関節 in &spring.joints {
        assert_eq!(関節.hit_radius, 0.02, "hit_radiusはbase値である");
        assert_eq!(関節.stiffness, 0.5, "stiffnessはbase値である");
        assert_eq!(関節.gravity_power, 0.0, "重力は今回使わない");
        assert_eq!(関節.gravity_dir, [0.0, -1.0, 0.0]);
        assert_eq!(関節.drag_force, 0.7, "dragは1-springである");
    }
    assert!(報告.curve_base_used.is_empty(), "空曲線では記録しない");
    // 所有記録を確かめる。
    assert_eq!(報告.owned_springs.len(), 1);
    assert_eq!(報告.owned_springs[0].bone, "pb:0");
    assert_eq!(報告.owned_springs[0].name, "HairA");
    assert_eq!(報告.owned_springs[0].joints, 2);
}

/// colliderを変換し、グループを共有する。
#[test]
fn colliderを変換しグループを共有する() {
    let fbxバイト列 = b"fake-fbx";
    let モデル = 書き出し可能モデルを作る();
    let (_, _, コンパイル済み) = 解決してコンパイルする(fbxバイト列, &モデル);
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
    let (_, _, コンパイル済み) = 解決してコンパイルする(fbxバイト列, &モデル);
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
        解決してコンパイルする(b"fake-fbx", &書き出し可能モデルを作る());
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
    let モデル = 書き出し可能モデルを作る();
    let 文 = 髪sidecarを作る(fbxバイト列, &モデル);
    let sidecar =
        PhysicsSidecar::from_json(&文, &PhysicsLimits::new()).expect("合成sidecarは検証を通る");
    let 解決 = モデル
        .resolve_physics_sidecar(&sidecar, fbxバイト列)
        .expect("合成対応は解決できる");
    let 上限0 = SpringCompileOptions::new().with_max_joints(0);
    assert_eq!(
        compile_springs(&sidecar, &モデル, &解決, &上限0)
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
    let コンパイル済み = compile_springs(&sidecar, &モデル, &解決, &SpringCompileOptions::new())
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
        "実sidecar生成: {} {}バイト spring{}件 関節{}件 collider{}件 group{}件 分岐{}件 共有{}件 曲線近似{}件 平面落下{}件 スキップ{}件",
        出力先.display(),
        バイト列.len(),
        コンパイル済み.report.spring_count,
        関節数,
        コンパイル済み.report.collider_count,
        コンパイル済み.report.group_count,
        コンパイル済み.report.branch_splits.len(),
        コンパイル済み.report.lost_nodes.len(),
        コンパイル済み.report.curve_base_used.len(),
        コンパイル済み.report.plane_colliders_dropped.len(),
        コンパイル済み.report.skipped_bones.len(),
    );
}

/// collider参照が無いspringは、空配列を出さずに拡張する。
#[test]
fn collider参照が無いspringは空配列を出さない() {
    let fbxバイト列 = b"fake-fbx";
    let モデル = 書き出し可能モデルを作る();
    let mut 値: serde_json::Value =
        serde_json::from_str(&髪sidecarを作る(fbxバイト列, &モデル)).expect("合成sidecarは読める");
    値["physBones"][0]["colliderIds"] = serde_json::json!([]);
    let (_, _, コンパイル済み) =
        文から解決してコンパイルする(&値.to_string(), fbxバイト列, &モデル);
    assert_eq!(コンパイル済み.report.collider_count, 0);
    assert_eq!(コンパイル済み.report.group_count, 0);
    assert_eq!(コンパイル済み.report.unused_colliders.len(), 2);
    let mut モデル = モデル;
    モデル.spring_bones = コンパイル済み.bones;
    let バイト列 = モデル
        .to_vrm(&書き出し設定を作る())
        .expect("参照なしでも書き出せる");
    let 拡張 = &glbのjsonを取る(&バイト列)["extensions"]["VRMC_springBone"];
    assert!(拡張.get("colliders").is_none(), "空配列は書かない");
    assert!(拡張.get("colliderGroups").is_none(), "空配列は書かない");
    assert_eq!(拡張["springs"].as_array().expect("springsはある").len(), 1);
}

/// Sphereは高さがあっても球のままである。
#[test]
fn 高さのある球は球のままである() {
    use vrctom::vrm::VrmSpringColliderShape;
    let fbxバイト列 = b"fake-fbx";
    let モデル = 書き出し可能モデルを作る();
    let mut 値: serde_json::Value =
        serde_json::from_str(&髪sidecarを作る(fbxバイト列, &モデル)).expect("合成sidecarは読める");
    値["colliders"][0]["height"] = serde_json::json!(0.4);
    let (_, _, コンパイル済み) =
        文から解決してコンパイルする(&値.to_string(), fbxバイト列, &モデル);
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
    let モデル = 書き出し可能モデルを作る();
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
        文から解決してコンパイルする(&値.to_string(), fbxバイト列, &モデル);
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

/// 同じ集合の順序・重複違いはグループを共有する。
/// `ignoreOtherPhysBones` が偽なら他rootを切らず、所有権で奪い合う。
#[test]
fn 順序違いの参照集合はグループを共有する() {
    let fbxバイト列 = b"fake-fbx";
    let モデル = 書き出し可能モデルを作る();
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
        文から解決してコンパイルする(&値.to_string(), fbxバイト列, &モデル);
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
    let モデル = 書き出し可能モデルを作る();
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
        文から解決してコンパイルする(&値.to_string(), fbxバイト列, &モデル);
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
    let モデル = 書き出し可能モデルを作る();
    let mut 値: serde_json::Value =
        serde_json::from_str(&髪sidecarを作る(fbxバイト列, &モデル)).expect("合成sidecarは読める");
    値["physBones"][0]["componentNodeId"] = serde_json::json!("n:4");
    値["physBones"][0]["rootNodeId"] = serde_json::json!("n:4");
    let (_, _, コンパイル済み) =
        文から解決してコンパイルする(&値.to_string(), fbxバイト列, &モデル);
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
    let モデル = 書き出し可能モデルを作る();
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
        文から解決してコンパイルする(&値.to_string(), fbxバイト列, &モデル);
    assert!(偽.report.cut_at_other_roots.is_empty(), "偽なら切らない");
    assert_eq!(偽.report.spring_count, 2);
    assert_eq!(偽.report.lost_nodes.len(), 3, "n:3・n:4・n:5を奪われる");
    値["physBones"][1]["ignoreOtherPhysBones"] = serde_json::json!(true);
    let (_, _, 真) =
        文から解決してコンパイルする(&値.to_string(), fbxバイト列, &モデル);
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
    let モデル = 書き出し可能モデルを作る();
    let mut 値: serde_json::Value =
        serde_json::from_str(&髪sidecarを作る(fbxバイト列, &モデル)).expect("合成sidecarは読める");
    値["physBones"][0]["ignoreNodeIds"] = serde_json::json!(["n:5"]);
    let (_, _, コンパイル済み) =
        文から解決してコンパイルする(&値.to_string(), fbxバイト列, &モデル);
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
    let モデル = 書き出し可能モデルを作る();
    let mut 値: serde_json::Value =
        serde_json::from_str(&髪sidecarを作る(fbxバイト列, &モデル)).expect("合成sidecarは読める");
    値["physBones"][0]["ignoreNodeIds"] = serde_json::json!(["n:3"]);
    let (_, _, コンパイル済み) =
        文から解決してコンパイルする(&値.to_string(), fbxバイト列, &モデル);
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
    let モデル = 書き出し可能モデルを作る();
    let mut 値: serde_json::Value =
        serde_json::from_str(&髪sidecarを作る(fbxバイト列, &モデル)).expect("合成sidecarは読める");
    // モデル上はHairB(n:4)が先だが、siblingIndexではHairC(n:5)を先にする。
    値["nodes"][4]["siblingIndex"] = serde_json::json!(5);
    値["nodes"][5]["siblingIndex"] = serde_json::json!(0);
    値["physBones"][0]["multiChildType"] = serde_json::json!("First");
    let (_, _, コンパイル済み) =
        文から解決してコンパイルする(&値.to_string(), fbxバイト列, &モデル);
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
    let モデル = 書き出し可能モデルを作る();
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
        文から解決してコンパイルする(&値.to_string(), fbxバイト列, &モデル);
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
    let mut モデル = モデル;
    モデル.spring_bones = コンパイル済み.bones;
    モデル
        .to_vrm(&書き出し設定を作る())
        .expect("空鎖を落とせば書き出せる");
}

/// 上限で切れた鎖は記録に残し、区間の無いspringは作らない。
#[test]
fn 上限で切れた鎖は記録に残す() {
    let fbxバイト列 = b"fake-fbx";
    let モデル = 書き出し可能モデルを作る();
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
        compile_springs(&sidecar, &モデル, &解決, &上限1).expect("上限付きでも変換できる");
    assert_eq!(制限付き.report.spring_count, 0);
    assert_eq!(
        制限付き.report.truncated_chains,
        vec!["HairA".to_string()],
        "切り詰めは先頭関節名で記録する"
    );
    // 上限2ではHairA・HairBの1本が通る。
    let 上限2 = SpringCompileOptions::new().with_max_joints(2);
    let 通過 = compile_springs(&sidecar, &モデル, &解決, &上限2).expect("上限ちょうどでは通る");
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
    let モデル = 三分岐モデルを作る();
    let 文 = 三分岐sidecarを作る(fbxバイト列, &モデル, "Ignore");
    let (_, _, コンパイル済み) =
        文から解決してコンパイルする(&文, fbxバイト列, &モデル);
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
    let モデル = VrmModel {
        root: 単位行列(),
        nodes: ノード,
        skins: Vec::new(),
        meshes: Vec::new(),
        expressions: VrmExpressions::default(),
        spring_bones: VrmSpringBones::default(),
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
        文から解決してコンパイルする(&文.to_string(), fbxバイト列, &モデル);
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
    let モデル = VrmModel {
        root: 単位行列(),
        nodes: ノード,
        skins: Vec::new(),
        meshes: Vec::new(),
        expressions: VrmExpressions::default(),
        spring_bones: VrmSpringBones::default(),
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
        文から解決してコンパイルする(&文.to_string(), fbxバイト列, &モデル);
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
    let モデル = VrmModel {
        root: 単位行列(),
        nodes: ノード,
        skins: Vec::new(),
        meshes: Vec::new(),
        expressions: VrmExpressions::default(),
        spring_bones: VrmSpringBones::default(),
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
        文から解決してコンパイルする(&文, fbxバイト列, &モデル);
    assert_eq!(コンパイル済み.report.spring_count, 1);
    assert_eq!(コンパイル済み.bones.springs[0].name, "A");
    let 関節: Vec<usize> = コンパイル済み.bones.springs[0]
        .joints
        .iter()
        .map(|関節| 関節.node)
        .collect();
    assert_eq!(関節, vec![1, 2], "avatarの子から始まる");
}

/// 未マップのノードを含む鎖は落とし、飛ばして接続しない。
#[test]
fn 未マップを含む鎖は落とす() {
    // Gはscene追加で未マップ。Hの下にぶら下がる。
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
    let モデル = VrmModel {
        root: 単位行列(),
        nodes: ノード,
        skins: Vec::new(),
        meshes: Vec::new(),
        expressions: VrmExpressions::default(),
        spring_bones: VrmSpringBones::default(),
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
        文から解決してコンパイルする(&文, fbxバイト列, &モデル);
    assert_eq!(コンパイル済み.report.spring_count, 0);
    assert_eq!(コンパイル済み.report.skipped_bones.len(), 1);
    match &コンパイル済み.report.skipped_bones[0].reason {
        vrctom::vrm::SkippedBoneReason::UnmappedJoint { node } => {
            assert_eq!(node, "n:2");
        }
        他 => panic!("未マップで落とす: {他:?}"),
    }
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
    let モデル = VrmModel {
        root: 単位行列(),
        nodes: ノード,
        skins: Vec::new(),
        meshes: Vec::new(),
        expressions: VrmExpressions::default(),
        spring_bones: VrmSpringBones::default(),
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
        文から解決してコンパイルする(&文.to_string(), fbxバイト列, &モデル);
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
    let mut モデル = モデル;
    モデル.spring_bones = コンパイル済み.bones;
    モデル
        .to_vrm(&書き出し設定を作る())
        .expect_err("humanoid不足は従来どおり失敗する");
}
