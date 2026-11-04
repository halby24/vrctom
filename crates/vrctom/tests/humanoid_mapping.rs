//! Unityのhumanoid対応表による骨解決の契約テスト（公開 API のみを使う）。
//!
//! `.fbx.meta` の `humanDescription.human` を [`AssetMeta::human_bone_mappings`] で読み、
//! [`HumanoidBones::detect_with_mapping`] で解決する。メタ優先・部分補完・
//! ロック・警告・失敗条件と、合成FBX＋メタからGLBへの統合を固定する。

use vrctom::conversion::{build_model, export_model};
use vrctom::fbx::FbxOptions;
use vrctom::unity::{AssetMeta, HumanBoneMapping};
use vrctom::vrm::{
    HumanoidBone, HumanoidBones, VrmExportErrorKind, VrmExportOptions, VrmExpressions, VrmMeta,
    VrmModel, VrmNode, VrmSpringBones,
};

// テスト名は日本語で書く（テストは仕様・契約であるため）。

/// 単位行列を返す。
fn 単位行列() -> [f64; 16] {
    [
        1.0, 0.0, 0.0, 0.0, //
        0.0, 1.0, 0.0, 0.0, //
        0.0, 0.0, 1.0, 0.0, //
        0.0, 0.0, 0.0, 1.0, //
    ]
}

/// ノード名列だけの最小モデルを作る。親子は持たない。
fn 骨名モデル(名前列: &[&str]) -> VrmModel {
    VrmModel {
        root: 単位行列(),
        nodes: 名前列
            .iter()
            .map(|名前| VrmNode {
                name: (*名前).to_string(),
                parent: None,
                children: Vec::new(),
                local: 単位行列(),
            })
            .collect(),
        skins: Vec::new(),
        meshes: Vec::new(),
        expressions: VrmExpressions::default(),
        spring_bones: VrmSpringBones::default(),
        node_constraints: Vec::new(),
        look_at: None,
    }
}

/// 対応1件を作る。
fn 対応(骨名: &str, ノード名: &str) -> HumanBoneMapping {
    HumanBoneMapping {
        human_name: 骨名.to_string(),
        bone_name: ノード名.to_string(),
    }
}

/// VRM 1.0の必須15骨をexact名で列挙する。
fn 必須骨のexact名() -> Vec<&'static str> {
    vec![
        "Hips",
        "Spine",
        "Head",
        "LeftUpperArm",
        "RightUpperArm",
        "LeftLowerArm",
        "RightLowerArm",
        "LeftHand",
        "RightHand",
        "LeftUpperLeg",
        "RightUpperLeg",
        "LeftLowerLeg",
        "RightLowerLeg",
        "LeftFoot",
        "RightFoot",
    ]
}

#[test]
fn wol風の名前をメタで解決できる() {
    // 手首・指・肩・目のwol風命名は名前推測で対応付かない。
    let モデル = 骨名モデル(&[
        "Hips",
        "Spine",
        "Chest",
        "Neck",
        "Head",
        "Left Eye",
        "Right Eye",
        "Left Shoulder",
        "Right Shoulder",
        "UpperArm.L",
        "UpperArm.R",
        "LowerArm.L",
        "LowerArm.R",
        "Wrist.L",
        "Wrist.R",
        "UpperLeg.L",
        "UpperLeg.R",
        "LowerLeg.L",
        "LowerLeg.R",
        "Foot.L",
        "Foot.R",
        "Toe.L",
        "Toe.R",
        "Thumb.L",
        "Thumb.L.001",
        "Thumb.L.002",
        "Index,L",
        "Index,L.001",
        "Index,L.002",
        "JawBone",
        "FooNode",
    ]);
    let 対応表 = vec![
        対応("Hips", "Hips"),
        対応("Spine", "Spine"),
        対応("Chest", "Chest"),
        対応("Neck", "Neck"),
        対応("Head", "Head"),
        対応("LeftEye", "Left Eye"),
        対応("RightEye", "Right Eye"),
        対応("LeftShoulder", "Left Shoulder"),
        対応("RightShoulder", "Right Shoulder"),
        対応("LeftUpperArm", "UpperArm.L"),
        対応("RightUpperArm", "UpperArm.R"),
        対応("LeftLowerArm", "LowerArm.L"),
        対応("RightLowerArm", "LowerArm.R"),
        対応("LeftHand", "Wrist.L"),
        対応("RightHand", "Wrist.R"),
        対応("LeftUpperLeg", "UpperLeg.L"),
        対応("RightUpperLeg", "UpperLeg.R"),
        対応("LeftLowerLeg", "LowerLeg.L"),
        対応("RightLowerLeg", "LowerLeg.R"),
        対応("LeftFoot", "Foot.L"),
        対応("RightFoot", "Foot.R"),
        対応("Left Thumb Proximal", "Thumb.L"),
        対応("Left Thumb Intermediate", "Thumb.L.001"),
        対応("Left Thumb Distal", "Thumb.L.002"),
        対応("Left Index Proximal", "Index,L"),
        対応("Left Index Intermediate", "Index,L.001"),
        対応("Left Index Distal", "Index,L.002"),
        対応("Jaw", "JawBone"),
        対応("Foo Bone", "FooNode"),
    ];
    let (骨, 警告) = HumanoidBones::detect_with_mapping(&モデル, &対応表)
        .expect("wol風の命名は対応表で解決できる");
    let 索引 = |名前: &str| {
        モデル
            .nodes
            .iter()
            .position(|node| node.name == 名前)
            .expect("試験の名前はモデルにある")
    };
    // 手首・肩・目は対応表で解決する。
    assert_eq!(骨.get(HumanoidBone::LeftHand), Some(索引("Wrist.L")));
    assert_eq!(骨.get(HumanoidBone::RightHand), Some(索引("Wrist.R")));
    assert_eq!(
        骨.get(HumanoidBone::LeftShoulder),
        Some(索引("Left Shoulder"))
    );
    assert_eq!(骨.get(HumanoidBone::LeftEye), Some(索引("Left Eye")));
    // 親指は意味を変換する（Proximal→中手骨・Intermediate→基節骨）。
    assert_eq!(
        骨.get(HumanoidBone::LeftThumbMetacarpal),
        Some(索引("Thumb.L"))
    );
    assert_eq!(
        骨.get(HumanoidBone::LeftThumbProximal),
        Some(索引("Thumb.L.001"))
    );
    assert_eq!(
        骨.get(HumanoidBone::LeftThumbDistal),
        Some(索引("Thumb.L.002"))
    );
    // 人差し指（`,`・`.001` 付き）も対応表で解決する。
    assert_eq!(
        骨.get(HumanoidBone::LeftIndexProximal),
        Some(索引("Index,L"))
    );
    assert_eq!(
        骨.get(HumanoidBone::LeftIndexIntermediate),
        Some(索引("Index,L.001"))
    );
    assert_eq!(
        骨.get(HumanoidBone::LeftIndexDistal),
        Some(索引("Index,L.002"))
    );
    // 足先は対応表に無いため名前推測で補完する。
    assert_eq!(骨.get(HumanoidBone::LeftToes), Some(索引("Toe.L")));
    assert_eq!(骨.get(HumanoidBone::RightToes), Some(索引("Toe.R")));
    // 未知のhuman名とJawは警告して省く。
    assert_eq!(警告.len(), 2, "警告は2件である: {警告:?}");
    assert!(
        警告.iter().any(|警告| 警告.contains("Foo Bone")),
        "未知のhuman名を警告する: {警告:?}"
    );
    assert!(
        警告.iter().any(|警告| 警告.contains("Jaw")),
        "Jawの省略を警告する: {警告:?}"
    );
}

#[test]
fn 左右の親指の意味を変換する() {
    // 親指は3本鎖であり、UnityのProximalはVRMの中手骨に当たる。
    let mut 名前列 = 必須骨のexact名();
    名前列.extend([
        "Thumb.L",
        "Thumb.L.001",
        "Thumb.L.002",
        "Thumb.R",
        "Thumb.R.001",
        "Thumb.R.002",
    ]);
    let モデル = 骨名モデル(&名前列);
    let 対応表 = vec![
        対応("Left Thumb Proximal", "Thumb.L"),
        対応("Left Thumb Intermediate", "Thumb.L.001"),
        対応("Left Thumb Distal", "Thumb.L.002"),
        対応("Right Thumb Proximal", "Thumb.R"),
        対応("Right Thumb Intermediate", "Thumb.R.001"),
        対応("Right Thumb Distal", "Thumb.R.002"),
    ];
    let (骨, 警告) =
        HumanoidBones::detect_with_mapping(&モデル, &対応表).expect("親指は対応表で解決できる");
    assert!(警告.is_empty(), "警告は無い: {警告:?}");
    let 索引 = |名前: &str| {
        モデル
            .nodes
            .iter()
            .position(|node| node.name == 名前)
            .expect("試験の名前はモデルにある")
    };
    assert_eq!(
        骨.get(HumanoidBone::LeftThumbMetacarpal),
        Some(索引("Thumb.L"))
    );
    assert_eq!(
        骨.get(HumanoidBone::LeftThumbProximal),
        Some(索引("Thumb.L.001"))
    );
    assert_eq!(
        骨.get(HumanoidBone::LeftThumbDistal),
        Some(索引("Thumb.L.002"))
    );
    assert_eq!(
        骨.get(HumanoidBone::RightThumbMetacarpal),
        Some(索引("Thumb.R"))
    );
    assert_eq!(
        骨.get(HumanoidBone::RightThumbProximal),
        Some(索引("Thumb.R.001"))
    );
    assert_eq!(
        骨.get(HumanoidBone::RightThumbDistal),
        Some(索引("Thumb.R.002"))
    );
}

#[test]
fn 同一対応の反復は許容する() {
    // 同一対応の反復は重複排除して許容する。
    let モデル = 骨名モデル(&必須骨のexact名());
    let 対応表 = vec![対応("LeftHand", "LeftHand"), 対応("LeftHand", "LeftHand")];
    let (骨, 警告) =
        HumanoidBones::detect_with_mapping(&モデル, &対応表).expect("同一対応の反復は通る");
    assert!(警告.is_empty(), "警告は無い: {警告:?}");
    assert_eq!(
        骨.get(HumanoidBone::LeftHand),
        HumanoidBones::detect(&モデル)
            .expect("名前推測は通る")
            .get(HumanoidBone::LeftHand),
        "反復しても確定は変わらない"
    );
}

#[test]
fn 同一骨への異なる対応は失敗する() {
    // 同一Unity骨に異なる `boneName` は先勝ちにせず失敗する。
    let mut 名前列 = 必須骨のexact名();
    名前列.push("Wrist.L");
    let モデル = 骨名モデル(&名前列);
    let 対応表 = vec![対応("LeftHand", "LeftHand"), 対応("LeftHand", "Wrist.L")];
    let 失敗 = HumanoidBones::detect_with_mapping(&モデル, &対応表)
        .expect_err("同一骨への異なる対応は失敗する");
    assert_eq!(失敗.kind(), VrmExportErrorKind::DuplicateHumanoidBone);
    assert_eq!(失敗.bone(), Some(HumanoidBone::LeftHand));
}

#[test]
fn 同一ノードへの異なる割当ては失敗する() {
    // 同一ノードを別の骨へ割当てることは先勝ち・後勝ちにせず失敗する。
    let モデル = 骨名モデル(&必須骨のexact名());
    let 対応表 = vec![対応("LeftHand", "LeftHand"), 対応("RightHand", "LeftHand")];
    let 失敗 = HumanoidBones::detect_with_mapping(&モデル, &対応表)
        .expect_err("同一ノードへの異なる割当ては失敗する");
    assert_eq!(失敗.kind(), VrmExportErrorKind::DuplicateHumanoidBone);
    assert_eq!(失敗.bone(), Some(HumanoidBone::RightHand));
}

#[test]
fn 対応先ノードが無い場合は失敗する() {
    // 対応対象の `boneName` がノードに存在しない場合は失敗する。
    // 明示の不整合を警告だけで名前推測に置換しない。
    let モデル = 骨名モデル(&必須骨のexact名());
    let 対応表 = vec![対応("LeftHand", "Wrist.L")];
    let 失敗 =
        HumanoidBones::detect_with_mapping(&モデル, &対応表).expect_err("対応先の欠落は失敗する");
    assert_eq!(失敗.kind(), VrmExportErrorKind::MissingHumanoidBone);
    assert_eq!(失敗.bone(), Some(HumanoidBone::LeftHand));
}

#[test]
fn 同名ノードが複数ある場合は失敗する() {
    // 対応対象の `boneName` が複数のノードに一致する場合は失敗する。
    let mut 名前列 = 必須骨のexact名();
    名前列.push("Wrist.L");
    名前列.push("Wrist.L");
    let モデル = 骨名モデル(&名前列);
    let 対応表 = vec![対応("LeftHand", "Wrist.L")];
    let 失敗 =
        HumanoidBones::detect_with_mapping(&モデル, &対応表).expect_err("対応先の重複は失敗する");
    assert_eq!(失敗.kind(), VrmExportErrorKind::DuplicateHumanoidBone);
    assert_eq!(失敗.bone(), Some(HumanoidBone::LeftHand));
}

#[test]
fn ロック骨の名前候補は無視する() {
    // メタで確定した骨に対する名前候補は複数あっても無視し、
    // メタの確定を名前の曖昧さで失敗させない。
    let mut 名前列 = 必須骨のexact名();
    名前列.push("Wrist.L");
    let モデル = 骨名モデル(&名前列);
    let 対応表 = vec![対応("LeftHand", "Wrist.L")];
    let (骨, 警告) =
        HumanoidBones::detect_with_mapping(&モデル, &対応表).expect("ロック骨の名前候補は無視する");
    assert!(警告.is_empty(), "警告は無い: {警告:?}");
    let 腕時計 = モデル
        .nodes
        .iter()
        .position(|node| node.name == "Wrist.L")
        .expect("試験の名前はモデルにある");
    assert_eq!(
        骨.get(HumanoidBone::LeftHand),
        Some(腕時計),
        "メタの確定が残る"
    );
}

#[test]
fn ロック骨に複数の名前候補があっても曖昧エラーにならない() {
    // メタで確定した骨に対する名前候補が複数あっても無視し、
    // 名前同士の曖昧さで確定を失敗させない。
    let mut 名前列 = 必須骨のexact名();
    名前列.push("Wrist.L");
    名前列.push("LEFTHAND");
    let モデル = 骨名モデル(&名前列);
    let 対応表 = vec![対応("LeftHand", "Wrist.L")];
    let (骨, 警告) = HumanoidBones::detect_with_mapping(&モデル, &対応表)
        .expect("複数の名前候補があっても確定できる");
    assert!(警告.is_empty(), "警告は無い: {警告:?}");
    let 腕時計 = モデル
        .nodes
        .iter()
        .position(|node| node.name == "Wrist.L")
        .expect("試験の名前はモデルにある");
    assert_eq!(
        骨.get(HumanoidBone::LeftHand),
        Some(腕時計),
        "メタの確定が残る"
    );
}

#[test]
fn 部分的な対応表は名前推測で補完する() {
    // メタに無い骨だけ名前推測で補完する。欠落のみ補完する。
    let モデル = 骨名モデル(&必須骨のexact名());
    let 対応表 = vec![対応("LeftHand", "LeftHand"), 対応("RightHand", "RightHand")];
    let (骨, 警告) =
        HumanoidBones::detect_with_mapping(&モデル, &対応表).expect("部分的な対応表は補完できる");
    assert!(警告.is_empty(), "警告は無い: {警告:?}");
    for 骨名 in [
        HumanoidBone::Hips,
        HumanoidBone::Spine,
        HumanoidBone::Head,
        HumanoidBone::LeftHand,
        HumanoidBone::RightHand,
        HumanoidBone::LeftFoot,
        HumanoidBone::RightFoot,
    ] {
        assert!(骨.get(骨名).is_some(), "必須骨が埋まる: {}", 骨名.name());
    }
}

#[test]
fn メタ使用済みノードは名前検出から除外する() {
    // メタで使ったノードを名前検出が再利用しない。
    // `LeftArm` は別名で左上腕に当たるが、メタで右手に使うため除外する。
    let mut 名前列 = 必須骨のexact名();
    名前列.retain(|名前| *名前 != "LeftUpperArm");
    名前列.push("LeftArm");
    let モデル = 骨名モデル(&名前列);
    let 対応表 = vec![対応("RightHand", "LeftArm")];
    let 失敗 = HumanoidBones::detect_with_mapping(&モデル, &対応表)
        .expect_err("使用済みノードの再利用はしない");
    assert_eq!(失敗.kind(), VrmExportErrorKind::MissingHumanoidBone);
    assert_eq!(失敗.bone(), Some(HumanoidBone::LeftUpperArm));
}

#[test]
fn 空の対応表は名前推測と同じになる() {
    // 対応表が空の場合は名前推測だけになる。
    let モデル = 骨名モデル(&必須骨のexact名());
    let 推測 = HumanoidBones::detect(&モデル).expect("名前推測は通る");
    let (対応付け, 警告) =
        HumanoidBones::detect_with_mapping(&モデル, &[]).expect("空の対応表は通る");
    assert!(警告.is_empty(), "警告は無い: {警告:?}");
    for 必須 in [
        HumanoidBone::Hips,
        HumanoidBone::Spine,
        HumanoidBone::Head,
        HumanoidBone::LeftUpperArm,
        HumanoidBone::LeftHand,
        HumanoidBone::RightFoot,
    ] {
        assert_eq!(
            対応付け.get(必須),
            推測.get(必須),
            "空の対応表は名前推測と同じである: {}",
            必須.name()
        );
    }
    assert_eq!(
        対応付け.iter().count(),
        推測.iter().count(),
        "空の対応表は名前推測と同じ件数である"
    );
}

/// wol風の階層モデルを作る。手首・指はメタ解決が必要な名前である。
fn wol風モデルを作る() -> VrmModel {
    let 名前と親: &[(&str, Option<usize>)] = &[
        ("Hips", None),
        ("Spine", Some(0)),
        ("Chest", Some(1)),
        ("Neck", Some(2)),
        ("Head", Some(3)),
        ("Left Eye", Some(4)),
        ("Right Eye", Some(4)),
        ("Left Shoulder", Some(2)),
        ("Right Shoulder", Some(2)),
        ("UpperArm.L", Some(7)),
        ("UpperArm.R", Some(8)),
        ("LowerArm.L", Some(9)),
        ("LowerArm.R", Some(10)),
        ("Wrist.L", Some(11)),
        ("Wrist.R", Some(12)),
        ("UpperLeg.L", Some(0)),
        ("UpperLeg.R", Some(0)),
        ("LowerLeg.L", Some(15)),
        ("LowerLeg.R", Some(16)),
        ("Foot.L", Some(17)),
        ("Foot.R", Some(18)),
        ("Toe.L", Some(19)),
        ("Toe.R", Some(20)),
        ("Thumb.L", Some(13)),
        ("Thumb.L.001", Some(23)),
        ("Thumb.L.002", Some(24)),
        ("Index,L", Some(13)),
        ("Index,L.001", Some(26)),
        ("Index,L.002", Some(27)),
        ("JawBone", Some(4)),
        ("FooNode", Some(0)),
    ];
    let 子列 = {
        let mut 子 = vec![Vec::new(); 名前と親.len() + 1];
        for (子索引, (_, 親索引)) in 名前と親.iter().enumerate() {
            if let Some(親索引) = 親索引 {
                子[*親索引].push(子索引);
            }
        }
        子
    };
    let mut ノード: Vec<VrmNode> = 名前と親
        .iter()
        .enumerate()
        .map(|(索引, (名前, 親))| VrmNode {
            name: (*名前).to_string(),
            parent: *親,
            children: 子列[索引].clone(),
            local: 単位行列(),
        })
        .collect();
    // メッシュ所有ノードは親なし・子なし・単位行列である。
    ノード.push(VrmNode {
        name: "Body".to_string(),
        parent: None,
        children: Vec::new(),
        local: 単位行列(),
    });
    let メッシュノード = ノード.len() - 1;
    let 関節: Vec<usize> = (0..メッシュノード).collect();
    VrmModel {
        root: 単位行列(),
        nodes: ノード,
        skins: vec![vrctom::vrm::VrmSkin {
            joints: 関節.clone(),
            inverse_bind_matrices: 関節.iter().map(|_| 単位行列()).collect(),
            joint_models: 関節
                .iter()
                .map(|索引| vrctom::fbx::ObjectId::new(*索引 as i64 + 1))
                .collect(),
        }],
        meshes: vec![vrctom::vrm::VrmMesh {
            node: メッシュノード,
            geometry: vrctom::fbx::ObjectId::new(100),
            model: vrctom::fbx::ObjectId::new(200),
            name: "BodyMesh".to_string(),
            skin: Some(0),
            material_slots: vec!["Body".to_string()],
            vertices: vrctom::vrm::VrmVertices {
                positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
                normals: Some(vec![[0.0, 0.0, 1.0], [0.0, 0.0, 1.0], [0.0, 0.0, 1.0]]),
                uv_sets: vec![vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]]],
                joints: Some(vec![[0, 0, 0, 0], [1, 0, 0, 0], [2, 0, 0, 0]]),
                weights: Some(vec![
                    [1.0, 0.0, 0.0, 0.0],
                    [1.0, 0.0, 0.0, 0.0],
                    [1.0, 0.0, 0.0, 0.0],
                ]),
            },
            primitives: vec![vrctom::vrm::VrmPrimitive {
                material: 0,
                indices: vec![0, 1, 2],
            }],
            targets: Vec::new(),
        }],
        expressions: VrmExpressions::default(),
        spring_bones: VrmSpringBones::default(),
        node_constraints: Vec::new(),
        look_at: None,
    }
}

/// wol風の対応表付きmetaを組み立てる。skeletonの有無を切り替えられる。
fn wol風meta(skeletonあり: bool) -> Vec<u8> {
    let guid = "30293896140a4d24a914010a2a8349fc";
    let mut 項目 = String::new();
    for (骨名, ノード名) in [
        ("Hips", "Hips"),
        ("Spine", "Spine"),
        ("Chest", "Chest"),
        ("Neck", "Neck"),
        ("Head", "Head"),
        ("LeftEye", "Left Eye"),
        ("RightEye", "Right Eye"),
        ("LeftShoulder", "Left Shoulder"),
        ("RightShoulder", "Right Shoulder"),
        ("LeftUpperArm", "UpperArm.L"),
        ("RightUpperArm", "UpperArm.R"),
        ("LeftLowerArm", "LowerArm.L"),
        ("RightLowerArm", "LowerArm.R"),
        ("LeftHand", "Wrist.L"),
        ("RightHand", "Wrist.R"),
        ("LeftUpperLeg", "UpperLeg.L"),
        ("RightUpperLeg", "UpperLeg.R"),
        ("LeftLowerLeg", "LowerLeg.L"),
        ("RightLowerLeg", "LowerLeg.R"),
        ("LeftFoot", "Foot.L"),
        ("RightFoot", "Foot.R"),
        ("Left Thumb Proximal", "Thumb.L"),
        ("Left Thumb Intermediate", "Thumb.L.001"),
        ("Left Thumb Distal", "Thumb.L.002"),
        ("Left Index Proximal", "Index,L"),
        ("Left Index Intermediate", "Index,L.001"),
        ("Left Index Distal", "Index,L.002"),
        ("Jaw", "JawBone"),
        ("Foo Bone", "FooNode"),
    ] {
        項目.push_str(&format!(
            "    - boneName: {ノード名}\n      humanName: {骨名}\n"
        ));
    }
    let skeleton = if skeletonあり {
        "    skeleton:\n    - name: Hips\n      position: {x: 0, y: 1, z: 0}\n      rotation: {x: 0, y: 0, z: 0, w: 1}\n      scale: {x: 1, y: 1, z: 1}\n"
    } else {
        ""
    };
    format!(
        "fileFormatVersion: 2\nguid: {guid}\nModelImporter:\n  humanDescription:\n    serializedVersion: 3\n    human:\n{項目}{skeleton}  animationType: 3\n"
    )
    .into_bytes()
}

/// GLBのJSON塊を取り出す。
fn glbのjsonを取り出す(バイト列: &[u8]) -> serde_json::Value {
    let json長 =
        u32::from_le_bytes(バイト列[12..16].try_into().expect("塊長は4バイトである")) as usize;
    let json = &バイト列[20..20 + json長];
    let 終端 = json
        .iter()
        .rposition(|b| *b != b' ')
        .map(|位置| 位置 + 1)
        .unwrap_or(0);
    serde_json::from_slice(&json[..終端]).expect("JSON塊は読める")
}

#[test]
fn skeletonは通常経路のローカル行列を変えない() {
    // メタの `skeleton` は通常の書き出しのrest姿勢に使わない。
    // 有無で解決結果とGLBが変わらない。
    let モデル = wol風モデルを作る();
    let あり = AssetMeta::parse(&wol風meta(true)).expect("skeleton付きも読める");
    let なし = AssetMeta::parse(&wol風meta(false)).expect("skeletonなしも読める");
    let 対応あり = あり.human_bone_mappings().expect("対応表は読める");
    let 対応なし = なし.human_bone_mappings().expect("対応表は読める");
    assert_eq!(対応あり, 対応なし, "skeletonの有無で対応表は変わらない");
    let (骨あり, 警告あり) =
        HumanoidBones::detect_with_mapping(&モデル, &対応あり).expect("skeleton付きで解決できる");
    let (骨なし, 警告なし) =
        HumanoidBones::detect_with_mapping(&モデル, &対応なし).expect("skeletonなしで解決できる");
    for 骨名 in [
        HumanoidBone::Hips,
        HumanoidBone::Spine,
        HumanoidBone::Chest,
        HumanoidBone::Neck,
        HumanoidBone::Head,
        HumanoidBone::LeftEye,
        HumanoidBone::LeftShoulder,
        HumanoidBone::LeftUpperArm,
        HumanoidBone::LeftLowerArm,
        HumanoidBone::LeftHand,
        HumanoidBone::RightHand,
        HumanoidBone::LeftThumbMetacarpal,
        HumanoidBone::LeftThumbProximal,
        HumanoidBone::LeftIndexProximal,
        HumanoidBone::LeftToes,
    ] {
        assert_eq!(
            骨あり.get(骨名),
            骨なし.get(骨名),
            "skeletonの有無で解決は変わらない: {}",
            骨名.name()
        );
    }
    assert_eq!(警告あり, 警告なし, "skeletonの有無で警告は変わらない");
    let 設定 = VrmExportOptions::new(VrmMeta::new("試験", vec!["作者".to_string()]));
    let mut 明示あり = 設定.clone();
    明示あり.humanoid = Some(骨あり);
    let mut 明示なし = 設定;
    明示なし.humanoid = Some(骨なし);
    let 前 = モデル.to_vrm(&明示あり).expect("skeleton付きで書き出せる");
    let 後 = モデル.to_vrm(&明示なし).expect("skeletonなしで書き出せる");
    assert_eq!(前, 後, "skeletonの有無でGLBは変わらない");
    // ノード行列はモデルのローカルのまま（単位行列は省略する）。
    let 値 = glbのjsonを取り出す(&前);
    let 節ら = 値
        .get("nodes")
        .and_then(serde_json::Value::as_array)
        .expect("nodesがある");
    assert_eq!(節ら.len(), モデル.nodes.len(), "ノード数は一致する");
    for (番号, 節) in 節ら.iter().enumerate() {
        assert!(
            節.get("matrix").is_none(),
            "単位行列のノードにmatrixは無い: {番号}"
        );
    }
    // 手首・親指・人差し指の対応がGLBに入る。
    let 骨表 = 値
        .get("extensions")
        .and_then(|値| 値.get("VRMC_vrm"))
        .and_then(|値| 値.get("humanoid"))
        .and_then(|値| 値.get("humanBones"))
        .and_then(serde_json::Value::as_object)
        .expect("humanBonesがある");
    let node_of = |名: &str| {
        骨表
            .get(名)
            .and_then(|値| 値.get("node"))
            .and_then(serde_json::Value::as_u64)
            .expect("nodeがある")
    };
    let 索引 = |名前: &str| {
        モデル
            .nodes
            .iter()
            .position(|node| node.name == 名前)
            .expect("試験の名前はモデルにある") as u64
    };
    assert_eq!(node_of("leftHand"), 索引("Wrist.L"));
    assert_eq!(node_of("rightHand"), 索引("Wrist.R"));
    assert_eq!(node_of("leftThumbMetacarpal"), 索引("Thumb.L"));
    assert_eq!(node_of("leftThumbProximal"), 索引("Thumb.L.001"));
    assert_eq!(node_of("leftIndexProximal"), 索引("Index,L"));
    assert_eq!(
        node_of("leftToes"),
        索引("Toe.L"),
        "足先は名前推測で補完する"
    );
    assert!(!骨表.contains_key("jaw"), "JawはhumanBonesに書かない");
}

/// 論理プロパティ列。件数は組み立て時に数える。
#[derive(Default)]
struct Props {
    buf: Vec<u8>,
    count: u32,
}

impl Props {
    fn prop(&mut self, code: u8, body: &[u8]) {
        self.buf.push(code);
        self.buf.extend_from_slice(body);
        self.count += 1;
    }

    fn l(&mut self, v: i64) {
        self.prop(b'L', &v.to_le_bytes());
    }

    fn s(&mut self, v: &[u8]) {
        let mut body = (v.len() as u32).to_le_bytes().to_vec();
        body.extend_from_slice(v);
        self.prop(b'S', &body);
    }

    /// `f64` 列をrawの `d` 配列として書く。
    fn d_raw(&mut self, values: &[f64]) {
        let mut payload = Vec::with_capacity(values.len() * 8);
        for v in values {
            payload.extend_from_slice(&v.to_le_bytes());
        }
        let mut body = (values.len() as u32).to_le_bytes().to_vec();
        body.extend_from_slice(&0u32.to_le_bytes());
        body.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        body.extend_from_slice(&payload);
        self.prop(b'd', &body);
    }

    /// `i32` 列をrawの `i` 配列として書く。
    fn i_raw(&mut self, values: &[i32]) {
        let mut payload = Vec::with_capacity(values.len() * 4);
        for v in values {
            payload.extend_from_slice(&v.to_le_bytes());
        }
        let mut body = (values.len() as u32).to_le_bytes().to_vec();
        body.extend_from_slice(&0u32.to_le_bytes());
        body.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        body.extend_from_slice(&payload);
        self.prop(b'i', &body);
    }
}

/// 論理ノード。シリアライズ時に絶対オフセットを確定させる。
struct TNode {
    name: Vec<u8>,
    props: Props,
    children: Vec<TNode>,
}

impl TNode {
    fn leaf(name: &[u8], props: Props) -> Self {
        Self {
            name: name.to_vec(),
            props,
            children: Vec::new(),
        }
    }

    fn branch(name: &[u8], props: Props, children: Vec<TNode>) -> Self {
        Self {
            name: name.to_vec(),
            props,
            children,
        }
    }
}

/// 論理ノードを絶対オフセット付きで書き出す。
fn serialise(node: &TNode, buf: &mut Vec<u8>) {
    let start = buf.len();
    buf.extend_from_slice(&[0u8; 13]);
    buf.extend_from_slice(&node.name);
    buf.extend_from_slice(&node.props.buf);
    for child in &node.children {
        serialise(child, buf);
    }
    if !node.children.is_empty() {
        buf.extend_from_slice(&[0u8; 13]);
    }
    let end = buf.len() as u32;
    buf[start..start + 4].copy_from_slice(&end.to_le_bytes());
    buf[start + 4..start + 8].copy_from_slice(&node.props.count.to_le_bytes());
    buf[start + 8..start + 12].copy_from_slice(&(node.props.buf.len() as u32).to_le_bytes());
    buf[start + 12] = node.name.len() as u8;
}

/// 直下ノード列からバイナリFBXを組み立てる。
fn binary_fbx(top: Vec<TNode>) -> Vec<u8> {
    let mut buf = b"Kaydara FBX Binary  \x00\x1a\x00".to_vec();
    buf.extend_from_slice(&7400u32.to_le_bytes());
    for node in &top {
        serialise(node, &mut buf);
    }
    buf.extend_from_slice(&[0u8; 13]);
    buf
}

/// `Model` ノードを組み立てる。
fn model_node(id: i64, name: &str, subtype: &str) -> TNode {
    let mut props = Props::default();
    props.l(id);
    props.s(format!("{name}\x00\x01Model").as_bytes());
    props.s(subtype.as_bytes());
    TNode::branch(b"Model", props, Vec::new())
}

/// `Geometry`（Mesh）ノードを組み立てる。
fn geometry_node(id: i64, name: &str, children: Vec<TNode>) -> TNode {
    let mut props = Props::default();
    props.l(id);
    props.s(format!("{name}\x00\x01Geometry").as_bytes());
    props.s(b"Mesh");
    TNode::branch(b"Geometry", props, children)
}

/// `PoseNode` を組み立てる。行列は16値の `d` 配列である。
fn pose_node(model: i64, matrix: &[f64; 16]) -> TNode {
    let mut matrix_props = Props::default();
    matrix_props.d_raw(matrix);
    let mut node_props = Props::default();
    node_props.l(model);
    TNode::branch(
        b"PoseNode",
        Props::default(),
        vec![
            TNode::leaf(b"Node", node_props),
            TNode::leaf(b"Matrix", matrix_props),
        ],
    )
}

/// 平行移動だけが異なる行優先のバインド行列を組み立てる。
fn bind_matrix(x: f64, y: f64, z: f64) -> [f64; 16] {
    [
        1.0, 0.0, 0.0, 0.0, //
        0.0, 1.0, 0.0, 0.0, //
        0.0, 0.0, 1.0, 0.0, //
        x, y, z, 1.0, //
    ]
}

/// 接続1件を組み立てる。
fn conn(child: i64, parent: i64) -> TNode {
    let mut props = Props::default();
    props.s(b"OO");
    props.l(child);
    props.l(parent);
    TNode::leaf(b"C", props)
}

/// wol風の名前を持つ必須骨と三角形1枚のFBXを組み立てる。
fn wol風fbxを組み立てる() -> Vec<u8> {
    // 親子関係は書き出し層の許容（上腕は脊椎の下など）を満たす。
    let 定義: &[(i64, &str, Option<i64>)] = &[
        (1, "Hips", None),
        (2, "Spine", Some(1)),
        (3, "Head", Some(2)),
        (4, "LeftUpperArm", Some(2)),
        (5, "LeftLowerArm", Some(4)),
        (6, "Wrist.L", Some(5)),
        (7, "RightUpperArm", Some(2)),
        (8, "RightLowerArm", Some(7)),
        (9, "Wrist.R", Some(8)),
        (10, "LeftUpperLeg", Some(1)),
        (11, "LeftLowerLeg", Some(10)),
        (12, "Foot.L", Some(11)),
        (13, "RightUpperLeg", Some(1)),
        (14, "RightLowerLeg", Some(13)),
        (15, "Foot.R", Some(14)),
        (16, "Index,L", Some(6)),
        (17, "Owner", Some(1)),
    ];
    let mut objs: Vec<TNode> = 定義
        .iter()
        .map(|(id, name, _)| {
            let subtype = if *name == "Owner" { "Mesh" } else { "LimbNode" };
            model_node(*id, name, subtype)
        })
        .collect();
    let mut vertices = Props::default();
    vertices.d_raw(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
    let mut pvi = Props::default();
    pvi.i_raw(&[0, 1, -3]);
    objs.push(geometry_node(
        100,
        "Tri",
        vec![
            TNode::leaf(b"Vertices", vertices),
            TNode::leaf(b"PolygonVertexIndex", pvi),
        ],
    ));
    let mut pose_props = Props::default();
    pose_props.l(200);
    pose_props.s(b"Bind\x00\x01Pose");
    pose_props.s(b"BindPose");
    let binds: Vec<TNode> = 定義
        .iter()
        .map(|(id, _, _)| pose_node(*id, &bind_matrix(0.0, 0.0, 0.0)))
        .collect();
    objs.push(TNode::branch(b"Pose", pose_props, binds));
    let mut conns: Vec<TNode> = 定義
        .iter()
        .filter_map(|(id, _, parent)| parent.map(|parent| conn(*id, parent)))
        .collect();
    conns.push(conn(100, 17));
    conns.push(conn(1, 0));
    binary_fbx(vec![
        TNode::branch(b"Objects", Props::default(), objs),
        TNode::branch(b"Connections", Props::default(), conns),
    ])
}

#[test]
fn 合成fbxとメタからglbのhumanoid対応を検査する() {
    // Wol相当（手首 `Wrist.L`・指 `Index,L`）の名前は名前推測で対応付かず、
    // メタの対応表で解決してGLBに書く。
    let fbx_bytes = wol風fbxを組み立てる();
    let built = build_model(
        &fbx_bytes,
        &FbxOptions::new(),
        &vrctom::vrm::VrmOptions::new(),
    )
    .expect("wol風FBXは中間モデルを作れる");
    // 合成FBXにあるノードだけを対応付ける。
    let 対応表 = vec![
        対応("Hips", "Hips"),
        対応("Spine", "Spine"),
        対応("Head", "Head"),
        対応("LeftUpperArm", "LeftUpperArm"),
        対応("RightUpperArm", "RightUpperArm"),
        対応("LeftLowerArm", "LeftLowerArm"),
        対応("RightLowerArm", "RightLowerArm"),
        対応("LeftHand", "Wrist.L"),
        対応("RightHand", "Wrist.R"),
        対応("LeftUpperLeg", "LeftUpperLeg"),
        対応("RightUpperLeg", "RightUpperLeg"),
        対応("LeftLowerLeg", "LeftLowerLeg"),
        対応("RightLowerLeg", "RightLowerLeg"),
        対応("LeftFoot", "Foot.L"),
        対応("RightFoot", "Foot.R"),
        対応("Left Index Proximal", "Index,L"),
    ];
    // 対応表は `.fbx.meta` のYAMLとして読み直す。
    let mut 項目 = String::from("  humanDescription:\n    serializedVersion: 3\n    human:\n");
    for 対応 in &対応表 {
        項目.push_str(&format!(
            "    - boneName: {}\n      humanName: {}\n",
            対応.bone_name, 対応.human_name
        ));
    }
    項目.push_str("  animationType: 3\n");
    let meta_bytes = format!(
        "fileFormatVersion: 2\nguid: 30293896140a4d24a914010a2a8349fc\nModelImporter:\n{項目}"
    )
    .into_bytes();
    let meta = AssetMeta::parse(&meta_bytes).expect("合成metaは読める");
    let 対応表 = meta.human_bone_mappings().expect("対応表は読める");
    let (骨, 警告) =
        HumanoidBones::detect_with_mapping(&built.model, &対応表).expect("対応表で解決できる");
    // このmetaに未知・Jawは無いため警告は無い。
    assert!(警告.is_empty(), "警告は無い: {警告:?}");
    let mut 設定 = VrmExportOptions::new(VrmMeta::new("試験", vec!["作者".to_string()]));
    設定.humanoid = Some(骨);
    let glb = export_model(&built.model, &設定).expect("解決済みで書き出せる");
    let 値 = glbのjsonを取り出す(&glb);
    let 骨表 = 値
        .get("extensions")
        .and_then(|値| 値.get("VRMC_vrm"))
        .and_then(|値| 値.get("humanoid"))
        .and_then(|値| 値.get("humanBones"))
        .and_then(serde_json::Value::as_object)
        .expect("humanBonesがある");
    let node_of = |名: &str| {
        骨表
            .get(名)
            .and_then(|値| 値.get("node"))
            .and_then(serde_json::Value::as_u64)
            .expect("nodeがある")
    };
    let 索引 = |名前: &str| {
        built
            .model
            .nodes
            .iter()
            .position(|node| node.name == 名前)
            .expect("試験の名前はモデルにある") as u64
    };
    // 名前推測では落ちる手首・指がメタで解決して書かれる。
    assert_eq!(node_of("leftHand"), 索引("Wrist.L"));
    assert_eq!(node_of("rightHand"), 索引("Wrist.R"));
    assert_eq!(node_of("leftIndexProximal"), 索引("Index,L"));
    // 必須骨がすべて書かれる。
    for 名 in [
        "hips",
        "spine",
        "head",
        "leftUpperArm",
        "rightUpperArm",
        "leftLowerArm",
        "rightLowerArm",
        "leftHand",
        "rightHand",
        "leftUpperLeg",
        "rightUpperLeg",
        "leftLowerLeg",
        "rightLowerLeg",
        "leftFoot",
        "rightFoot",
    ] {
        assert!(骨表.contains_key(名), "必須骨が書かれる: {名}");
    }
    // 対応表なしでは手首が無いため書き出せない（受入の前提）。
    let 失敗 = built
        .model
        .to_vrm(&VrmExportOptions::new(VrmMeta::new(
            "試験",
            vec!["作者".to_string()],
        )))
        .expect_err("対応表なしでは手首が無いため失敗する");
    assert_eq!(失敗.kind(), VrmExportErrorKind::MissingHumanoidBone);
    assert_eq!(失敗.bone(), Some(HumanoidBone::LeftHand));
}
