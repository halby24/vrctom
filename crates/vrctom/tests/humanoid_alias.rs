//! Mixamo由来の別名によるhumanoid対応付けの契約試験（公開 API のみを使う）。
//!
//! [`HumanoidBones::detect`] が正確名・別名の2パスで対応付けることを、
//! ノード名だけの最小モデルで確かめる。2パス規則（正確名が勝つ・
//! 別名同士は推測しない）と既存命名の非退行を固定する。

use vrctom::vrm::{
    HumanoidBone, HumanoidBones, VrmExportErrorKind, VrmExpressions, VrmModel, VrmNode,
    VrmSpringBones,
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

/// VRM 1.0の必須15骨を列挙順で返す。
fn 必須骨() -> Vec<HumanoidBone> {
    vec![
        HumanoidBone::Hips,
        HumanoidBone::Spine,
        HumanoidBone::Head,
        HumanoidBone::LeftUpperArm,
        HumanoidBone::RightUpperArm,
        HumanoidBone::LeftLowerArm,
        HumanoidBone::RightLowerArm,
        HumanoidBone::LeftHand,
        HumanoidBone::RightHand,
        HumanoidBone::LeftUpperLeg,
        HumanoidBone::RightUpperLeg,
        HumanoidBone::LeftLowerLeg,
        HumanoidBone::RightLowerLeg,
        HumanoidBone::LeftFoot,
        HumanoidBone::RightFoot,
    ]
}

#[test]
fn 混合風の名前集合で必須骨が全て埋まる() {
    // Shaon由来のMixamo風命名であり、正確名（`UpperArm` など）を持たない。
    let model = 骨名モデル(&[
        "Hips",
        "Spine",
        "Chest",
        "Neck",
        "Head",
        "LeftArm",
        "RightArm",
        "LeftForeArm",
        "RightForeArm",
        "LeftHand",
        "RightHand",
        "LeftUpLeg",
        "RightUpLeg",
        "LeftLeg",
        "RightLeg",
        "LeftFoot",
        "RightFoot",
        "LeftToeBase",
        "RightToeBase",
    ]);
    let bones = HumanoidBones::detect(&model).expect("Mixamo風の命名は対応付く");
    // 必須15骨がすべて埋まる。
    for 骨 in 必須骨() {
        assert!(bones.get(骨).is_some(), "必須骨が埋まる: {}", 骨.name());
    }
    // 別名の対応先を固定する。
    let index_of = |名前: &str| {
        model
            .nodes
            .iter()
            .position(|node| node.name == 名前)
            .expect("試験の名前はモデルにある")
    };
    assert_eq!(
        bones.get(HumanoidBone::LeftUpperArm),
        Some(index_of("LeftArm"))
    );
    assert_eq!(
        bones.get(HumanoidBone::LeftLowerArm),
        Some(index_of("LeftForeArm"))
    );
    assert_eq!(
        bones.get(HumanoidBone::LeftUpperLeg),
        Some(index_of("LeftUpLeg"))
    );
    assert_eq!(
        bones.get(HumanoidBone::LeftLowerLeg),
        Some(index_of("LeftLeg"))
    );
    // 足先は必須でないが `toebase` で対応付く。
    assert_eq!(
        bones.get(HumanoidBone::LeftToes),
        Some(index_of("LeftToeBase"))
    );
}

#[test]
fn 正確名と別名が同時にある場合に正確名が勝つ() {
    // 正確名の必須集合に別名ノードを足す。別名は無視され失敗しない。
    let mut 名前列 = vec![
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
    ];
    let 正確な上腕 = 名前列
        .iter()
        .position(|名前| *名前 == "LeftUpperArm")
        .unwrap();
    let 正確な下腿 = 名前列
        .iter()
        .position(|名前| *名前 == "RightLowerLeg")
        .unwrap();
    名前列.push("LeftArm");
    名前列.push("RightLeg");
    let model = 骨名モデル(&名前列);
    let bones = HumanoidBones::detect(&model).expect("正確名がある場合は通る");
    assert_eq!(
        bones.get(HumanoidBone::LeftUpperArm),
        Some(正確な上腕),
        "別名より正確名が勝つ"
    );
    assert_eq!(
        bones.get(HumanoidBone::RightLowerLeg),
        Some(正確な下腿),
        "別名より正確名が勝つ"
    );
}

#[test]
fn 同枠に別名が2つある場合に推測しない() {
    // 左上腕の枠に `LeftArm` と `Arm_L` が当たり、先勝ちにせず失敗する。
    let model = 骨名モデル(&[
        "Hips",
        "Spine",
        "Head",
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
        "LeftArm",
        "Arm_L",
    ]);
    let error = HumanoidBones::detect(&model).expect_err("別名の重複は推測せず失敗する");
    assert_eq!(error.kind(), VrmExportErrorKind::DuplicateHumanoidBone);
    assert_eq!(error.bone(), Some(HumanoidBone::LeftUpperArm));
}

#[test]
fn 既存命名の正規化が退行しない() {
    // vrc由来（`_`・`.`・大小）とMMD由来（`pinky`・番号）の命名が従来どおり通る。
    let model = 骨名モデル(&[
        "Hips",
        "Spine",
        "Head",
        "Upper_arm.L",
        "Upper_arm.R",
        "Lower_arm.L",
        "Lower_arm.R",
        "Hand.L",
        "Hand.R",
        "Upper_leg.L",
        "Upper_leg.R",
        "Lower_leg.L",
        "Lower_leg.R",
        "Foot.L",
        "Foot.R",
        "Thumb Proximal.L",
        "Thumb Intermediate.L",
        "Thumb Distal.L",
        "pinky1.R",
        "Toe.R",
    ]);
    let bones = HumanoidBones::detect(&model).expect("既存の命名は対応付く");
    for 骨 in 必須骨() {
        assert!(bones.get(骨).is_some(), "必須骨が埋まる: {}", 骨.name());
    }
    let index_of = |名前: &str| {
        model
            .nodes
            .iter()
            .position(|node| node.name == 名前)
            .expect("試験の名前はモデルにある")
    };
    assert_eq!(
        bones.get(HumanoidBone::LeftUpperArm),
        Some(index_of("Upper_arm.L"))
    );
    // 親指の3本鎖は1つずらして対応する既存規則のままである。
    assert_eq!(
        bones.get(HumanoidBone::LeftThumbMetacarpal),
        Some(index_of("Thumb Proximal.L"))
    );
    assert_eq!(
        bones.get(HumanoidBone::RightLittleProximal),
        Some(index_of("pinky1.R"))
    );
    assert_eq!(bones.get(HumanoidBone::RightToes), Some(index_of("Toe.R")));
}
