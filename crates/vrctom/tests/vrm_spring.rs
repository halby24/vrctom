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

/// 線形チェーンを作り、分岐を記録する。最初の子だけを採用する。
#[test]
fn 線形チェーンを作り分岐を記録する() {
    let fbxバイト列 = b"fake-fbx";
    let モデル = 書き出し可能モデルを作る();
    let (_, _, コンパイル済み) = 解決してコンパイルする(fbxバイト列, &モデル);
    let 報告 = &コンパイル済み.report;
    assert_eq!(報告.spring_count, 1, "springは1本である");
    assert_eq!(報告.joint_count, 2, "関節はHairA・HairBの2件である");
    let spring = &コンパイル済み.bones.springs[0];
    assert_eq!(spring.name, "pb:0", "spring名はPhysBone IDである");
    assert!(spring.center.is_none(), "centerは今回付けない");
    let 関節: Vec<usize> = spring.joints.iter().map(|関節| 関節.node).collect();
    assert_eq!(関節, vec![3, 4], "HairAから最初の子HairBだけを辿る");
    // 分岐HairCは捨て、記録に残す。
    assert_eq!(報告.skipped_branches.len(), 1, "分岐は1件記録する");
    assert_eq!(報告.skipped_branches[0].spring, "pb:0");
    assert_eq!(報告.skipped_branches[0].node, 3);
    assert_eq!(報告.skipped_branches[0].skipped, vec![5]);
    // 第一近似の値を確かめる。
    for 関節 in &spring.joints {
        assert_eq!(関節.hit_radius, 0.02, "hit_radiusはbase値である");
        assert_eq!(関節.stiffness, 0.5, "stiffnessはbase値である");
        assert_eq!(関節.gravity_power, 0.0, "重力は今回使わない");
        assert_eq!(関節.gravity_dir, [0.0, -1.0, 0.0]);
        assert_eq!(関節.drag_force, 0.7, "dragは1-springである");
    }
    assert!(報告.curve_base_used.is_empty(), "空曲線では記録しない");
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
        "実sidecar生成: {} {}バイト spring{}件 関節{}件 collider{}件 group{}件 分岐{}件 曲線近似{}件 平面落下{}件",
        出力先.display(),
        バイト列.len(),
        コンパイル済み.report.spring_count,
        関節数,
        コンパイル済み.report.collider_count,
        コンパイル済み.report.group_count,
        コンパイル済み.report.skipped_branches.len(),
        コンパイル済み.report.curve_base_used.len(),
        コンパイル済み.report.plane_colliders_dropped.len(),
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

/// 同じ集合の順序違いはグループを共有し、他rootで切断する。
#[test]
fn 順序違いの参照集合はグループを共有する() {
    let fbxバイト列 = b"fake-fbx";
    let モデル = 書き出し可能モデルを作る();
    let mut 値: serde_json::Value =
        serde_json::from_str(&髪sidecarを作る(fbxバイト列, &モデル)).expect("合成sidecarは読める");
    let 追加 = serde_json::json!({
        "id": "pb:1",
        "componentNodeId": "n:5",
        "rootNodeId": "n:5",
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
        "colliderIds": ["c:1", "c:0"],
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
    // 重複IDを含む3本目はHead(n:2)を起点にし、他rootで止まる。
    // 集合としては同じため、新しいグループは作らない。
    let 重複分 = serde_json::json!({
        "id": "pb:2",
        "componentNodeId": "n:2",
        "rootNodeId": "n:2",
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
        "colliderIds": ["c:0", "c:0", "c:1"],
        "limitType": "None",
        "maxAngleX": {"value": 0.0, "keys": [], "preWrapMode": 8, "postWrapMode": 8},
        "maxAngleZ": {"value": 0.0, "keys": [], "preWrapMode": 8, "postWrapMode": 8},
        "limitRotation": [0.0, 0.0, 0.0],
        "isAnimated": false,
    });
    値["physBones"]
        .as_array_mut()
        .expect("physBonesは配列である")
        .push(重複分);
    let (_, _, コンパイル済み) =
        文から解決してコンパイルする(&値.to_string(), fbxバイト列, &モデル);
    assert_eq!(コンパイル済み.report.spring_count, 3);
    assert_eq!(
        コンパイル済み.report.group_count, 1,
        "順序違い・重複つきでも共有する"
    );
    for spring in &コンパイル済み.bones.springs {
        assert_eq!(spring.collider_groups, vec![0]);
    }
    assert_eq!(
        コンパイル済み.bones.collider_groups[0].colliders,
        vec![0, 1],
        "重複は除いて正準化する"
    );
    // pb:0の鎖は他root(n:5)で切れ、分岐記録ではなく切断記録になる。
    // pb:2の鎖も他root(n:3)で切れる。
    assert!(コンパイル済み.report.skipped_branches.is_empty());
    assert_eq!(コンパイル済み.report.cut_at_other_roots.len(), 2);
    assert_eq!(コンパイル済み.report.cut_at_other_roots[0].spring, "pb:0");
    assert_eq!(コンパイル済み.report.cut_at_other_roots[0].node, 5);
    assert_eq!(
        コンパイル済み.report.cut_at_other_roots[0].other_bone,
        "pb:1"
    );
    assert_eq!(コンパイル済み.report.cut_at_other_roots[1].spring, "pb:2");
    assert_eq!(コンパイル済み.report.cut_at_other_roots[1].node, 3);
    assert_eq!(
        コンパイル済み.report.cut_at_other_roots[1].other_bone,
        "pb:0"
    );
    // first-child化の近似はIgnore以外で記録する。
    assert_eq!(
        コンパイル済み.report.first_child_multichild,
        vec!["pb:1".to_string()]
    );
    assert!(コンパイル済み.report.center_missing.len() == 3);
    assert!(コンパイル済み.report.no_endpoint.len() == 3);
}

/// 無視指定の部分木には降りない。
#[test]
fn 無視指定の部分木には降りない() {
    let fbxバイト列 = b"fake-fbx";
    let モデル = 書き出し可能モデルを作る();
    let mut 値: serde_json::Value =
        serde_json::from_str(&髪sidecarを作る(fbxバイト列, &モデル)).expect("合成sidecarは読める");
    値["physBones"][0]["ignoreNodeIds"] = serde_json::json!(["n:4", "n:5"]);
    let (_, _, コンパイル済み) =
        文から解決してコンパイルする(&値.to_string(), fbxバイト列, &モデル);
    assert_eq!(コンパイル済み.bones.springs.len(), 1);
    let 関節: Vec<usize> = コンパイル済み.bones.springs[0]
        .joints
        .iter()
        .map(|関節| 関節.node)
        .collect();
    assert_eq!(関節, vec![3], "無視部分木には降りない");
}

/// rootを共有する2本目は空鎖で落とし、書き出しは通す。
#[test]
fn root共有の2本目は空鎖で落とす() {
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
        "空鎖はspringにしない"
    );
    assert_eq!(コンパイル済み.report.skipped_empty_chains.len(), 1);
    assert_eq!(コンパイル済み.report.skipped_empty_chains[0].spring, "pb:1");
    assert_eq!(コンパイル済み.report.skipped_empty_chains[0].owner, "pb:0");
    let mut モデル = モデル;
    モデル.spring_bones = コンパイル済み.bones;
    モデル
        .to_vrm(&書き出し設定を作る())
        .expect("空鎖を落とせば書き出せる");
}

/// 子だけが循環する階層でも止まり、重なりで切る。
#[test]
fn 子の循環でも止まって重なりで切る() {
    // 親は直線、子だけが循環する壊れた階層。public APIでは作れる。
    let ノード = vec![
        VrmNode {
            name: "A".to_string(),
            parent: None,
            children: vec![1],
            local: 単位行列(),
        },
        VrmNode {
            name: "B".to_string(),
            parent: Some(0),
            children: vec![0],
            local: 平行移動行列([0.0, 1.0, 0.0]),
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
            {"id": "n:0", "parentId": None::<String>, "name": "A", "siblingIndex": 0,
             "scenePath": ["Milfy", "A"], "source": 出所(&["A"]), "referenceLocal": trs([0.0, 0.0, 0.0])},
            {"id": "n:1", "parentId": "n:0", "name": "B", "siblingIndex": 0,
             "scenePath": ["Milfy", "A", "B"], "source": 出所(&["A", "B"]), "referenceLocal": trs([0.0, 1.0, 0.0])},
        ],
        "physBones": [{
            "id": "pb:0", "componentNodeId": "n:0", "rootNodeId": "n:0", "enabled": true,
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
    let 関節: Vec<usize> = コンパイル済み.bones.springs[0]
        .joints
        .iter()
        .map(|関節| 関節.node)
        .collect();
    assert_eq!(関節, vec![0, 1], "循環に戻らず止まる");
    assert_eq!(コンパイル済み.report.shared_joint_cuts.len(), 1);
}
