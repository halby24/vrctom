//! lookAt（bone型の視線）の契約テスト（公開 API のみを使う）。
//!
//! 合成モデルと合成sidecarで測定規則・既定値・検証を固定する。
//! 実fixtureを使う検証は `VRCTOM_FBX_FIXTURE`・`VRCTOM_PHYSICS_SIDECAR`・
//! `VRCTOM_UNITY_VRM` が指すファイルだけを読み、未設定なら跳ばす。
//! sidecarに視線記録が無い旧形式の場合は実fixture検証も跳ばす。

use std::collections::HashMap;

use vrctom::fbx::{FbxFile, FbxOptions, ObjectId};
use vrctom::vrm::{
    HumanoidBones, PhysicsLimits, PhysicsResolution, PhysicsSidecar, VrmExportOptions, VrmLookAt,
    VrmMeta, VrmModel, VrmMorphTarget, VrmNode, VrmPrimitive, VrmSkin, VrmSpringBones, VrmVertices,
    compile_springs, resolve_look_at,
};

// テスト名は日本語で書く（テストは仕様・契約であるため）。

/// 単位行列（列優先）。
fn 単位行列() -> [f64; 16] {
    [
        1.0, 0.0, 0.0, 0.0, //
        0.0, 1.0, 0.0, 0.0, //
        0.0, 0.0, 1.0, 0.0, //
        0.0, 0.0, 0.0, 1.0, //
    ]
}

/// 平行移動の行列（列優先）。
fn 平行移動(移動: [f64; 3]) -> [f64; 16] {
    [
        1.0, 0.0, 0.0, 0.0, //
        0.0, 1.0, 0.0, 0.0, //
        0.0, 0.0, 1.0, 0.0, //
        移動[0], 移動[1], 移動[2], 1.0, //
    ]
}

/// 視線検証用の合成モデルを作る。必須15骨を持ち、頭と目だけ平行移動する。
///
/// 頭は `(0, 1.5, 0)`、左目は `(-0.03, 1.55, 0.02)`、右目は `(0.03, 1.55, 0.02)` の
/// 世界位置になり、頭基準ずれは `(0, 0.05, 0.02)` になる。
fn 視線モデルを作る() -> VrmModel {
    let 定義: &[(&str, Option<usize>, [f64; 16])] = &[
        ("Hips", None, 単位行列()),
        ("Spine", Some(0), 単位行列()),
        ("Head", Some(1), 平行移動([0.0, 1.5, 0.0])),
        ("LeftEye", Some(2), 平行移動([-0.03, 0.05, 0.02])),
        ("RightEye", Some(2), 平行移動([0.03, 0.05, 0.02])),
        ("LeftUpperArm", Some(1), 単位行列()),
        ("LeftLowerArm", Some(5), 単位行列()),
        ("LeftHand", Some(6), 単位行列()),
        ("RightUpperArm", Some(1), 単位行列()),
        ("RightLowerArm", Some(8), 単位行列()),
        ("RightHand", Some(9), 単位行列()),
        ("LeftUpperLeg", Some(0), 単位行列()),
        ("LeftLowerLeg", Some(11), 単位行列()),
        ("LeftFoot", Some(12), 単位行列()),
        ("RightUpperLeg", Some(0), 単位行列()),
        ("RightLowerLeg", Some(14), 単位行列()),
        ("RightFoot", Some(15), 単位行列()),
        ("Body", None, 単位行列()),
    ];
    let mut 子列: Vec<Vec<usize>> = vec![Vec::new(); 定義.len()];
    for (子, (_, 親, _)) in 定義.iter().enumerate() {
        if let Some(親) = 親 {
            子列[*親].push(子);
        }
    }
    let ノード: Vec<VrmNode> = 定義
        .iter()
        .map(|(名, 親, 局所)| VrmNode {
            name: (*名).to_string(),
            parent: *親,
            children: 子列[定義
                .iter()
                .position(|d| d.0 == *名)
                .expect("定義は一意である")]
            .clone(),
            local: *局所,
        })
        .collect();
    let メッシュノード = 定義.len() - 1;
    let 関節: Vec<usize> = (0..メッシュノード).collect();
    VrmModel {
        root: 単位行列(),
        nodes: ノード,
        skins: vec![VrmSkin {
            joints: 関節.clone(),
            inverse_bind_matrices: 関節.iter().map(|_| 単位行列()).collect(),
            joint_models: 関節.iter().map(|i| ObjectId::new(*i as i64 + 1)).collect(),
        }],
        meshes: vec![vrctom::vrm::VrmMesh {
            node: メッシュノード,
            geometry: ObjectId::new(100),
            model: ObjectId::new(200),
            name: "BodyMesh".to_string(),
            skin: Some(0),
            material_slots: vec!["Body".to_string()],
            vertices: VrmVertices {
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
            primitives: vec![VrmPrimitive {
                material: 0,
                indices: vec![0, 1, 2],
            }],
            targets: Vec::<VrmMorphTarget>::new(),
        }],
        expressions: vrctom::vrm::VrmExpressions::default(),
        spring_bones: VrmSpringBones::default(),
        node_constraints: Vec::new(),
        look_at: None,
    }
}

/// 書き出し設定を作る。
fn 書き出し設定を作る() -> VrmExportOptions {
    VrmExportOptions::new(VrmMeta::new("試験", vec!["作者".to_string()]))
}

/// GLBのJSON塊を値として取り出す。
fn glbのjsonを取る(バイト列: &[u8]) -> serde_json::Value {
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

/// 合成sidecarの本文を作る。視線断片が `None` の場合は `lookAt` を省く。
fn sidecar文を作る(視線: Option<&str>) -> String {
    let 視線部 = 視線.map_or(String::new(), |断片| format!(",\n  \"lookAt\": {断片}"));
    format!(
        r#"{{
  "format": "vrctom.physics",
  "schemaVersion": 1,
  "generator": {{"name": "試験", "version": "1"}},
  "capture": {{
    "statePolicy": "bind-pose",
    "coordinateSpace": "unity-avatar-root-local-meters",
    "avatarRootId": "n:0",
    "rootScale": [1.0, 1.0, 1.0]
  }},
  "sourceModels": [{{"id": "m", "sha256": "d95486711f12f69d68450c8b777f5bb4ec01a44a063d078216644e8a25f7b4ec"}}],
  "nodes": [
    {{"id": "n:2", "name": "Head", "referenceLocal": {{"position": [0.0, 0.0, 0.0], "rotation": [0.0, 0.0, 0.0, 1.0], "scale": [1.0, 1.0, 1.0]}}}},
    {{"id": "n:3", "name": "LeftEye", "referenceLocal": {{"position": [0.0, 0.0, 0.0], "rotation": [0.0, 0.0, 0.0, 1.0], "scale": [1.0, 1.0, 1.0]}}}},
    {{"id": "n:4", "name": "RightEye", "referenceLocal": {{"position": [0.0, 0.0, 0.0], "rotation": [0.0, 0.0, 0.0, 1.0], "scale": [1.0, 1.0, 1.0]}}}}
  ],
  "physBones": [],
  "colliders": []{視線部}
}}"#
    )
}

/// 実測相当の5姿勢の視線断片を作る。角度はMilfy実測のクォータニオンである。
///
/// 上 `9.424714` 度・下 `9.716058` 度・左右 `11.0` 度（親回転が恒等の場合）。
fn 実測視線断片(連結: bool, 有効: bool) -> String {
    let straight = "[0.0, 0.0, 0.0, 1.0]";
    let up = "[-0.08215345, 0.0, 0.0, 0.99661970]";
    let down = "[0.08468705, 0.0, 0.0, 0.99640763]";
    let left = "[0.0, -0.09584575, 0.0, 0.99539620]";
    let right = "[0.0, 0.09584575, 0.0, 0.99539620]";
    let pose = |値: &str| {
        if 有効 {
            format!(
                r#"{{"linked": {連結}, "left": {値}, "right": {値}}}"#,
                連結 = 連結,
                値 = 値
            )
        } else {
            r#"{"linked": true, "left": [0.0, 0.0, 0.0, 0.0], "right": [0.0, 0.0, 0.0, 1.0]}"#
                .to_string()
        }
    };
    format!(
        r#"{{"eyeLook": true, "leftEyeNodeId": "n:3", "rightEyeNodeId": "n:4", "eyesLookingStraight": {straight}, "eyesLookingUp": {up}, "eyesLookingDown": {down}, "eyesLookingLeft": {left}, "eyesLookingRight": {right}}}"#,
        straight = pose(straight),
        up = pose(up),
        down = pose(down),
        left = pose(left),
        right = pose(right),
    )
}

/// 解決用の対応付けを作る。sidecarの目をモデルの目に写す。
fn 対応付けを作る() -> PhysicsResolution {
    PhysicsResolution {
        matched_source_model: "m".to_string(),
        fbx_sha256: String::new(),
        node_map: HashMap::from([
            ("n:2".to_string(), 2),
            ("n:3".to_string(), 3),
            ("n:4".to_string(), 4),
        ]),
        auxiliary_nodes: Vec::new(),
        phys_bones: Vec::new(),
        colliders: Vec::new(),
        skipped_colliders: Vec::new(),
    }
}

/// 解決する。視線記録がある合成sidecarを使う。
fn 解決する(
    モデル: &VrmModel,
    視線: Option<&str>,
    対応付け: &PhysicsResolution,
) -> Option<(VrmLookAt, vrctom::vrm::LookAtReport)> {
    let 文 = sidecar文を作る(視線);
    let sidecar =
        PhysicsSidecar::from_json(&文, &PhysicsLimits::new()).expect("合成sidecarは読める");
    let humanoid = HumanoidBones::detect(モデル).expect("合成モデルは対応付けられる");
    resolve_look_at(&sidecar, モデル, 対応付け, &humanoid).expect("合成条件は解決できる")
}

#[test]
fn 左右同値の姿勢から4値が出る() {
    let モデル = 視線モデルを作る();
    let 対応付け = 対応付けを作る();
    for 連結 in [true, false] {
        let (視線, 報告) = 解決する(&モデル, Some(&実測視線断片(連結, true)), &対応付け)
            .expect("視線記録があるため解決できる");
        match &視線 {
            VrmLookAt::Bone { measured, .. } => {
                assert!(measured, "有効な姿勢は測定値を使う: 連結={連結}");
            }
            VrmLookAt::Expression { .. } => panic!("目があるため骨視線になる"),
        }
        // 参照実装の式どおりの値（親回転が恒等の場合）。許容は単精度丸めの範囲である。
        let 許容 = 1e-4;
        assert!(
            (報告.range.vertical_up - 9.424714).abs() < 許容,
            "上: {}",
            報告.range.vertical_up
        );
        assert!(
            (報告.range.vertical_down - 9.716058).abs() < 許容,
            "下: {}",
            報告.range.vertical_down
        );
        assert!(
            (報告.range.horizontal_inner - 11.0).abs() < 許容,
            "内: {}",
            報告.range.horizontal_inner
        );
        assert!(
            (報告.range.horizontal_outer - 11.0).abs() < 許容,
            "外: {}",
            報告.range.horizontal_outer
        );
        assert_eq!(報告.look_type, "bone");
        assert_eq!(報告.offset_from_head.len(), 3, "ずれは3要素である");
        for (実, 想) in 報告
            .offset_from_head
            .iter()
            .zip([0.0f32, 0.05, 0.02].iter())
        {
            assert!(
                (実 - 想).abs() < 1e-6,
                "頭基準ずれは眼の中点である: {:?}",
                報告.offset_from_head
            );
        }
    }
}

#[test]
fn 非連結の左右差は平均になる() {
    // 上視だけ左右で変える（左10度・右14度）。下左右は同値である。
    let 断片 = r#"{"eyeLook": true, "leftEyeNodeId": "n:3", "rightEyeNodeId": "n:4",
      "eyesLookingStraight": {"linked": true, "left": [0.0, 0.0, 0.0, 1.0], "right": [0.0, 0.0, 0.0, 1.0]},
      "eyesLookingUp": {"linked": false, "left": [-0.08715578, 0.0, 0.0, 0.99619472], "right": [-0.12186934, 0.0, 0.0, 0.99254615]},
      "eyesLookingDown": {"linked": true, "left": [0.07845910, 0.0, 0.0, 0.99691700], "right": [0.07845910, 0.0, 0.0, 0.99691700]},
      "eyesLookingLeft": {"linked": true, "left": [0.0, -0.09584575, 0.0, 0.99539620], "right": [0.0, -0.09584575, 0.0, 0.99539620]},
      "eyesLookingRight": {"linked": true, "left": [0.0, 0.09584575, 0.0, 0.99539620], "right": [0.0, 0.09584575, 0.0, 0.99539620]}}"#;
    let モデル = 視線モデルを作る();
    let (視線, _) =
        解決する(&モデル, Some(断片), &対応付けを作る()).expect("視線記録があるため解決できる");
    match 視線 {
        VrmLookAt::Bone {
            range, measured, ..
        } => {
            assert!(measured);
            assert!(
                (range.vertical_up - 12.0).abs() < 1e-3,
                "左右の平均になる: {}",
                range.vertical_up
            );
        }
        VrmLookAt::Expression { .. } => panic!("目があるため骨視線になる"),
    }
}

#[test]
fn 無効な姿勢は既定に戻る() {
    let モデル = 視線モデルを作る();
    let 対応付け = 対応付けを作る();
    // ゼロ長の姿勢は無効であり、測定せず既定の骨視線になる。
    let (視線, 報告) = 解決する(&モデル, Some(&実測視線断片(true, false)), &対応付け)
        .expect("視線記録があるため解決できる");
    match 視線 {
        VrmLookAt::Bone {
            range, measured, ..
        } => {
            assert!(!measured, "無効な姿勢は測定しない");
            for 値 in [
                range.horizontal_inner,
                range.horizontal_outer,
                range.vertical_down,
                range.vertical_up,
            ] {
                assert!((値 - 10.0).abs() < 1e-6, "既定の出力角である: {値}");
            }
        }
        VrmLookAt::Expression { .. } => panic!("目があるため骨視線になる"),
    }
    assert!(!報告.measured);
}

#[test]
fn 範囲外の方向だけ既定に戻る() {
    // 左右視だけ100度（上限超え）にする。上下は実測値である。
    // ピッチはasinで±90度に収まるため、上限超えはヨー方向で起こす。
    let 断片 = r#"{"eyeLook": true, "leftEyeNodeId": "n:3", "rightEyeNodeId": "n:4",
      "eyesLookingStraight": {"linked": true, "left": [0.0, 0.0, 0.0, 1.0], "right": [0.0, 0.0, 0.0, 1.0]},
      "eyesLookingUp": {"linked": true, "left": [-0.08215345, 0.0, 0.0, 0.99661970], "right": [-0.08215345, 0.0, 0.0, 0.99661970]},
      "eyesLookingDown": {"linked": true, "left": [0.08468705, 0.0, 0.0, 0.99640763], "right": [0.08468705, 0.0, 0.0, 0.99640763]},
      "eyesLookingLeft": {"linked": true, "left": [0.0, -0.76604444, 0.0, 0.64278761], "right": [0.0, -0.76604444, 0.0, 0.64278761]},
      "eyesLookingRight": {"linked": true, "left": [0.0, 0.76604444, 0.0, 0.64278761], "right": [0.0, 0.76604444, 0.0, 0.64278761]}}"#;
    let モデル = 視線モデルを作る();
    let (視線, _) =
        解決する(&モデル, Some(断片), &対応付けを作る()).expect("視線記録があるため解決できる");
    match 視線 {
        VrmLookAt::Bone {
            range, measured, ..
        } => {
            assert!(measured, "測定自体は使う");
            assert!(
                (range.horizontal_inner - 10.0).abs() < 1e-6
                    && (range.horizontal_outer - 10.0).abs() < 1e-6,
                "上限超えは既定に戻る: 内={} 外={}",
                range.horizontal_inner,
                range.horizontal_outer
            );
            assert!(
                (range.vertical_up - 9.424714).abs() < 1e-3
                    && (range.vertical_down - 9.716058).abs() < 1e-3,
                "他方向は実測値である: 上={} 下={}",
                range.vertical_up,
                range.vertical_down
            );
        }
        VrmLookAt::Expression { .. } => panic!("目があるため骨視線になる"),
    }
}

#[test]
fn 目が一致しない場合は既定に戻る() {
    let モデル = 視線モデルを作る();
    let 対応付け = 対応付けを作る();
    // sidecarの目が別ノードを指す。humanoidの目と一致しない。
    let 断片 = 実測視線断片(true, true)
        .replace("\"leftEyeNodeId\": \"n:3\"", "\"leftEyeNodeId\": \"n:2\"");
    let (視線, 報告) =
        解決する(&モデル, Some(&断片), &対応付け).expect("視線記録があるため解決できる");
    match 視線 {
        VrmLookAt::Bone { measured, .. } => assert!(!measured, "不一致は測定しない"),
        VrmLookAt::Expression { .. } => panic!("目があるため骨視線になる"),
    }
    assert!(!報告.measured);
}

#[test]
fn 目が無い場合は表情視線になる() {
    let mut モデル = 視線モデルを作る();
    // 目を検出対象外の名前に変える。必須骨は残るため書き出しは通る。
    モデル.nodes[3].name = "Eye_L_dummy".to_string();
    モデル.nodes[4].name = "Eye_R_dummy".to_string();
    let (視線, 報告) = 解決する(&モデル, Some(&実測視線断片(true, true)), &対応付けを作る())
        .expect("視線記録があるため解決できる");
    match 視線 {
        VrmLookAt::Expression { offset_from_head } => {
            assert_eq!(offset_from_head, [0.0, 0.06, 0.0], "頭上6cmが原点である");
        }
        VrmLookAt::Bone { .. } => panic!("目が無いため表情視線になる"),
    }
    assert_eq!(報告.look_type, "expression");
    for 値 in [
        報告.range.horizontal_inner,
        報告.range.horizontal_outer,
        報告.range.vertical_down,
        報告.range.vertical_up,
    ] {
        assert!((値 - 1.0).abs() < 1e-6, "出力は重み1.0である: {値}");
    }
}

#[test]
fn 目が無ければ検出名の付け替えでhumanoidから外れる() {
    // 上の試験の前提（付け替え後の目検出の欠落）を直接確かめる。
    let mut モデル = 視線モデルを作る();
    モデル.nodes[3].name = "Eye_L_dummy".to_string();
    モデル.nodes[4].name = "Eye_R_dummy".to_string();
    let humanoid = HumanoidBones::detect(&モデル).expect("必須骨は残る");
    assert!(humanoid.get(vrctom::vrm::HumanoidBone::LeftEye).is_none());
    assert!(humanoid.get(vrctom::vrm::HumanoidBone::RightEye).is_none());
}

#[test]
fn 眼注視が無効の場合は既定の骨視線になる() {
    let 断片 = 実測視線断片(true, true).replace("\"eyeLook\": true", "\"eyeLook\": false");
    let モデル = 視線モデルを作る();
    let (視線, 報告) =
        解決する(&モデル, Some(&断片), &対応付けを作る()).expect("視線記録があるため解決できる");
    match 視線 {
        VrmLookAt::Bone {
            range, measured, ..
        } => {
            assert!(!measured);
            for 値 in [
                range.horizontal_inner,
                range.horizontal_outer,
                range.vertical_down,
                range.vertical_up,
            ] {
                assert!((値 - 10.0).abs() < 1e-6);
            }
        }
        VrmLookAt::Expression { .. } => panic!("目があるため骨視線になる"),
    }
    assert!(!報告.measured);
}

#[test]
fn 視線記録が無いsidecarは解決しない() {
    let モデル = 視線モデルを作る();
    assert!(
        解決する(&モデル, None, &対応付けを作る()).is_none(),
        "旧形式は後方互換で何もしない"
    );
}

#[test]
fn 視線の目の参照が無いsidecarは検証で失敗する() {
    let 断片 = 実測視線断片(true, true).replace("\"n:3\"", "\"n:9\"");
    let 文 = sidecar文を作る(Some(&断片));
    let 失敗 = PhysicsSidecar::from_json(&文, &PhysicsLimits::new())
        .expect_err("存在しない目の参照は失敗する");
    assert_eq!(
        失敗.kind(),
        vrctom::vrm::VrmErrorKind::InvalidPhysicsSidecar
    );
}

#[test]
fn 視線の姿勢の要素数が4でないsidecarは検証で失敗する() {
    let 断片 = 実測視線断片(true, true).replace(
        "\"left\": [0.0, 0.0, 0.0, 1.0]",
        "\"left\": [0.0, 0.0, 1.0]",
    );
    let 文 = sidecar文を作る(Some(&断片));
    PhysicsSidecar::from_json(&文, &PhysicsLimits::new()).expect_err("要素数3の姿勢は失敗する");
}

#[test]
fn 視線の姿勢に非有限値があるsidecarは検証で失敗する() {
    // JSONでは非有限値を書けないため、構造体を直接作って検証する。
    let 文 = sidecar文を作る(Some(&実測視線断片(true, true)));
    let mut sidecar =
        PhysicsSidecar::from_json(&文, &PhysicsLimits::new()).expect("合成sidecarは読める");
    let 視線 = sidecar.look_at.as_mut().expect("視線記録がある");
    視線.eyes_looking_up.left = [f64::NAN, 0.0, 0.0, 1.0];
    // 再検証は公開 API が無いため、測定側の無効扱いを確かめる。
    // 非有限の姿勢は正規化できず、測定せず既定に戻る。
    let モデル = 視線モデルを作る();
    let humanoid = HumanoidBones::detect(&モデル).expect("合成モデルは対応付けられる");
    let (_, 報告) = resolve_look_at(&sidecar, &モデル, &対応付けを作る(), &humanoid)
        .expect("解決は通る")
        .expect("視線記録があるため解決できる");
    assert!(!報告.measured, "非有限の姿勢は測定しない");
}

#[test]
fn 骨視線を書き出せる() {
    let mut モデル = 視線モデルを作る();
    モデル.look_at = Some(VrmLookAt::Bone {
        left_eye: 3,
        right_eye: 4,
        offset_from_head: [0.0, 0.05, 0.02],
        range: vrctom::vrm::VrmLookAtRange {
            horizontal_inner: 11.0,
            horizontal_outer: 11.0,
            vertical_down: 9.716058,
            vertical_up: 9.424714,
        },
        measured: true,
    });
    let バイト列 = モデル
        .to_vrm(&書き出し設定を作る())
        .expect("骨視線は書き出せる");
    let 値 = glbのjsonを取る(&バイト列);
    let 視線 = 値
        .get("extensions")
        .and_then(|値| 値.get("VRMC_vrm"))
        .and_then(|値| 値.get("lookAt"))
        .expect("lookAtがある");
    assert_eq!(
        視線.get("type").and_then(serde_json::Value::as_str),
        Some("bone")
    );
    let ずれ = 視線
        .get("offsetFromHeadBone")
        .and_then(serde_json::Value::as_array)
        .expect("ずれは配列である");
    for (実, 想) in ずれ.iter().zip([0.0f64, 0.05, 0.02].iter()) {
        let 実 = 実.as_f64().expect("要素は数である");
        assert!(
            (実 - 想).abs() < 1e-6,
            "頭基準ずれが一致する: 実={実} 想={想}"
        );
    }
    // 測定値は入力最大角と出力スケールが同値である。
    for (名, 期待) in [
        ("rangeMapHorizontalInner", 11.0),
        ("rangeMapHorizontalOuter", 11.0),
        ("rangeMapVerticalDown", 9.716058),
        ("rangeMapVerticalUp", 9.424714),
    ] {
        let 写像 = 視線.get(名).expect("範囲写像がある");
        let 入力 = 写像
            .get("inputMaxValue")
            .and_then(serde_json::Value::as_f64)
            .expect("入力最大値がある");
        let 出力 = 写像
            .get("outputScale")
            .and_then(serde_json::Value::as_f64)
            .expect("出力スケールがある");
        assert!(
            (入力 - 期待).abs() < 1e-5 && (出力 - 期待).abs() < 1e-5,
            "{名}は同値である: 入力={入力} 出力={出力}"
        );
    }
}

#[test]
fn 既定の骨視線は入力90度出力10度になる() {
    let mut モデル = 視線モデルを作る();
    モデル.look_at = Some(VrmLookAt::Bone {
        left_eye: 3,
        right_eye: 4,
        offset_from_head: [0.0, 0.05, 0.02],
        range: vrctom::vrm::VrmLookAtRange {
            horizontal_inner: 10.0,
            horizontal_outer: 10.0,
            vertical_down: 10.0,
            vertical_up: 10.0,
        },
        measured: false,
    });
    let バイト列 = モデル
        .to_vrm(&書き出し設定を作る())
        .expect("既定値も書き出せる");
    let 値 = glbのjsonを取る(&バイト列);
    let 視線 = 値
        .get("extensions")
        .and_then(|値| 値.get("VRMC_vrm"))
        .and_then(|値| 値.get("lookAt"))
        .expect("lookAtがある");
    for 名 in [
        "rangeMapHorizontalInner",
        "rangeMapHorizontalOuter",
        "rangeMapVerticalDown",
        "rangeMapVerticalUp",
    ] {
        let 写像 = 視線.get(名).expect("範囲写像がある");
        assert_eq!(
            写像
                .get("inputMaxValue")
                .and_then(serde_json::Value::as_f64),
            Some(90.0),
            "{名}の入力は90度である"
        );
        assert_eq!(
            写像.get("outputScale").and_then(serde_json::Value::as_f64),
            Some(10.0),
            "{名}の出力は10度である"
        );
    }
}

#[test]
fn 表情視線を書き出せる() {
    let mut モデル = 視線モデルを作る();
    モデル.nodes[3].name = "Eye_L_dummy".to_string();
    モデル.nodes[4].name = "Eye_R_dummy".to_string();
    モデル.look_at = Some(VrmLookAt::Expression {
        offset_from_head: [0.0, 0.06, 0.0],
    });
    let バイト列 = モデル
        .to_vrm(&書き出し設定を作る())
        .expect("表情視線は書き出せる");
    let 値 = glbのjsonを取る(&バイト列);
    let 視線 = 値
        .get("extensions")
        .and_then(|値| 値.get("VRMC_vrm"))
        .and_then(|値| 値.get("lookAt"))
        .expect("lookAtがある");
    assert_eq!(
        視線.get("type").and_then(serde_json::Value::as_str),
        Some("expression")
    );
    for 名 in [
        "rangeMapHorizontalInner",
        "rangeMapHorizontalOuter",
        "rangeMapVerticalDown",
        "rangeMapVerticalUp",
    ] {
        let 写像 = 視線.get(名).expect("範囲写像がある");
        assert_eq!(
            写像
                .get("inputMaxValue")
                .and_then(serde_json::Value::as_f64),
            Some(90.0)
        );
        assert_eq!(
            写像.get("outputScale").and_then(serde_json::Value::as_f64),
            Some(1.0)
        );
    }
}

#[test]
fn 視線が無い場合はlook_atを書かない() {
    let モデル = 視線モデルを作る();
    assert!(モデル.look_at.is_none());
    let バイト列 = モデル
        .to_vrm(&書き出し設定を作る())
        .expect("視線無しは書き出せる");
    let 値 = glbのjsonを取る(&バイト列);
    assert!(
        値.get("extensions")
            .and_then(|値| 値.get("VRMC_vrm"))
            .and_then(|値| 値.get("lookAt"))
            .is_none(),
        "lookAtは省略する（従来どおり）"
    );
}

#[test]
fn 視線の目の索引が範囲外の場合は失敗する() {
    let mut モデル = 視線モデルを作る();
    モデル.look_at = Some(VrmLookAt::Bone {
        left_eye: 99,
        right_eye: 4,
        offset_from_head: [0.0, 0.05, 0.02],
        range: vrctom::vrm::VrmLookAtRange {
            horizontal_inner: 10.0,
            horizontal_outer: 10.0,
            vertical_down: 10.0,
            vertical_up: 10.0,
        },
        measured: false,
    });
    let 失敗 = モデル
        .to_vrm(&書き出し設定を作る())
        .expect_err("範囲外の目は失敗する");
    assert_eq!(
        失敗.kind(),
        vrctom::vrm::VrmExportErrorKind::InvalidStructure
    );
}

#[test]
fn 視線の左右が同じ場合と非有限の範囲は失敗する() {
    let 範囲 = || vrctom::vrm::VrmLookAtRange {
        horizontal_inner: 10.0,
        horizontal_outer: 10.0,
        vertical_down: 10.0,
        vertical_up: 10.0,
    };
    // 同じノード。
    let mut モデル = 視線モデルを作る();
    モデル.look_at = Some(VrmLookAt::Bone {
        left_eye: 3,
        right_eye: 3,
        offset_from_head: [0.0, 0.05, 0.02],
        range: 範囲(),
        measured: false,
    });
    assert_eq!(
        モデル
            .to_vrm(&書き出し設定を作る())
            .expect_err("同じ目は失敗する")
            .kind(),
        vrctom::vrm::VrmExportErrorKind::InvalidStructure
    );
    // 上限超え。
    let mut モデル = 視線モデルを作る();
    モデル.look_at = Some(VrmLookAt::Bone {
        left_eye: 3,
        right_eye: 4,
        offset_from_head: [0.0, 0.05, 0.02],
        range: vrctom::vrm::VrmLookAtRange {
            vertical_up: 100.0,
            ..範囲()
        },
        measured: true,
    });
    assert_eq!(
        モデル
            .to_vrm(&書き出し設定を作る())
            .expect_err("上限超えは失敗する")
            .kind(),
        vrctom::vrm::VrmExportErrorKind::InvalidStructure
    );
    // 非有限のずれ。
    let mut モデル = 視線モデルを作る();
    モデル.look_at = Some(VrmLookAt::Bone {
        left_eye: 3,
        right_eye: 4,
        offset_from_head: [f32::NAN, 0.0, 0.0],
        range: 範囲(),
        measured: false,
    });
    assert_eq!(
        モデル
            .to_vrm(&書き出し設定を作る())
            .expect_err("非有限のずれは失敗する")
            .kind(),
        vrctom::vrm::VrmExportErrorKind::InvalidStructure
    );
}

/// 実fixtureと実sidecarとUnity出力で視線を突き合わせる。
///
/// sidecarに視線記録が無い旧形式の場合は跳ばす。Unity出力の4値・ずれと
/// 相対1e-4（絶対1e-6併用）で一致し、件数が不変であることを確かめる。
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn 実fixtureの視線はunity出力と一致する() {
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
    let (mut モデル, _) =
        VrmModel::from_fbx(&解析, &vrctom::vrm::VrmOptions::new()).expect("実fixtureは変換できる");
    let sidecar =
        PhysicsSidecar::parse(&sidecar文, &PhysicsLimits::new()).expect("実sidecarは検証を通る");
    let Some(_) = sidecar.look_at.as_ref() else {
        eprintln!("実sidecarに視線記録が無い旧形式のため跳ばす");
        return;
    };
    let 解決 = モデル
        .resolve_physics_sidecar(&sidecar, &fbxバイト列)
        .expect("実fixtureの参照は解決できる");
    let humanoid = HumanoidBones::detect(&モデル).expect("実fixtureは対応付けられる");
    let (視線, 報告) = resolve_look_at(&sidecar, &モデル, &解決, &humanoid)
        .expect("実fixtureの視線は解決できる")
        .expect("視線記録があるため解決できる");
    assert_eq!(報告.look_type, "bone", "Milfyは骨視線である");
    assert!(報告.measured, "Milfyは測定値を使う");
    let コンパイル済み = compile_springs(
        &sidecar,
        &mut モデル,
        &解決,
        &vrctom::vrm::SpringCompileOptions::new(),
    )
    .expect("実fixtureの鎖はコンパイルできる");
    モデル.look_at = Some(視線);
    モデル.spring_bones = コンパイル済み.bones.clone();
    let 設定 = VrmExportOptions::new(VrmMeta::new("試験", vec!["作者".to_string()]));
    let バイト列 = モデル.to_vrm(&設定).expect("実fixtureは書き出せる");
    let json = glbのjsonを取る(&バイト列);
    let 出力 = json["extensions"]["VRMC_vrm"]["lookAt"]
        .as_object()
        .expect("lookAtがある");
    assert_eq!(出力["type"], serde_json::json!("bone"), "種別はboneである");
    let unityバイト列 = std::fs::read(&unity経路).expect("Unity製VRMは読める");
    let unity_json = glbのjsonを取る(&unityバイト列);
    let 期待 = unity_json["extensions"]["VRMC_vrm"]["lookAt"]
        .as_object()
        .expect("Unity製VRMにlookAtがある");
    assert_eq!(期待["type"], serde_json::json!("bone"));
    // ずれは相対1e-4（絶対1e-6併用）で一致する。
    let 出ずれ = 出力["offsetFromHeadBone"]
        .as_array()
        .expect("ずれは配列である");
    let 想ずれ = 期待["offsetFromHeadBone"]
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
    // 4つの範囲値は相対1e-4で一致する。
    for 名 in [
        "rangeMapHorizontalInner",
        "rangeMapHorizontalOuter",
        "rangeMapVerticalDown",
        "rangeMapVerticalUp",
    ] {
        for 項目 in ["inputMaxValue", "outputScale"] {
            let 実 = 出力[名][項目].as_f64().expect("値は数である");
            let 想 = 期待[名][項目].as_f64().expect("値は数である");
            assert!(
                (実 - 想).abs() <= 1e-6 + 1e-4 * 想.abs(),
                "{名}.{項目}が一致する: 実={実} 想={想}"
            );
        }
    }
    // 件数は不変である。
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
    eprintln!(
        "実fixture視線: 型={} 測定={} 内={:.6} 外={:.6} 下={:.6} 上={:.6} ずれ={:?} spring{}件 関節{}件 collider{}件 center{}件 limit{}件 constraint{}件",
        報告.look_type,
        報告.measured,
        報告.range.horizontal_inner,
        報告.range.horizontal_outer,
        報告.range.vertical_down,
        報告.range.vertical_up,
        報告.offset_from_head,
        コンパイル済み.report.spring_count,
        関節数,
        コンパイル済み.report.collider_count,
        コンパイル済み.report.center_applied.len(),
        制限数,
        モデル.node_constraints.len(),
    );
    assert_eq!(コンパイル済み.report.spring_count, 45);
    assert_eq!(関節数, 148);
    assert_eq!(コンパイル済み.report.collider_count, 12);
    assert_eq!(コンパイル済み.report.center_applied.len(), 43);
    assert_eq!(制限数, 71);
    assert_eq!(モデル.node_constraints.len(), 8);
}
