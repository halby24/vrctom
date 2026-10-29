//! VRM書き出しの契約テスト（公開 API のみを使う）。
//!
//! 合成した中間モデルでGLB容器・JSON・拡張・失敗条件を固定する。
//! 実fixtureを使う検証は `VRCTOM_FBX_FIXTURE` が指すファイルだけを読み、
//! 未設定なら跳ばす。

use vrctom::fbx::{FbxFile, FbxOptions, ObjectId};
use vrctom::vrm::{
    HumanoidBone, HumanoidBones, MaterialOptions, VrmAlphaMode, VrmExportErrorKind,
    VrmExportOptions, VrmExpression, VrmExpressionOverride, VrmExpressions, VrmImage, VrmImageMime,
    VrmMaterial, VrmMaterialEntry, VrmMaterials, VrmMesh, VrmMeta, VrmModel, VrmMorphTarget,
    VrmMorphTargetBind, VrmMtoonParameters, VrmNode, VrmOutlineWidthMode, VrmPrimitive, VrmSampler,
    VrmSkin, VrmVertices,
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

/// 必須15骨を含む24骨の名前列。区切り付き名で検出を確かめる。
fn 骨名列() -> Vec<&'static str> {
    vec![
        "Hips",
        "Spine",
        "Chest",
        "Neck",
        "Head",
        "LeftEye",
        "RightEye",
        "Shoulder.L",
        "Shoulder.R",
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
        "Toe.L",
        "Toe.R",
    ]
}

/// 骨階層の親列。`None` はルートである。
fn 骨親列() -> Vec<Option<usize>> {
    vec![
        None,     // 0 Hips
        Some(0),  // 1 Spine
        Some(1),  // 2 Chest
        Some(2),  // 3 Neck
        Some(3),  // 4 Head
        Some(4),  // 5 LeftEye
        Some(4),  // 6 RightEye
        Some(2),  // 7 LeftShoulder
        Some(2),  // 8 RightShoulder
        Some(7),  // 9 LeftUpperArm
        Some(8),  // 10 RightUpperArm
        Some(9),  // 11 LeftLowerArm
        Some(10), // 12 RightLowerArm
        Some(11), // 13 LeftHand
        Some(12), // 14 RightHand
        Some(0),  // 15 LeftUpperLeg
        Some(0),  // 16 RightUpperLeg
        Some(15), // 17 LeftLowerLeg
        Some(16), // 18 RightLowerLeg
        Some(17), // 19 LeftFoot
        Some(18), // 20 RightFoot
        Some(19), // 21 LeftToes
        Some(20), // 22 RightToes
        None,     // 23 Body（メッシュ所有ノード）
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

/// 合成の中間モデルを作る。24骨の階層・1メッシュ・1スキンを持つ。
fn 合成モデルを作る() -> VrmModel {
    let 名前 = 骨名列();
    let 親 = 骨親列();
    let 子 = 子列(&親);
    let mut ノード = Vec::new();
    for (索引, 名) in 名前.iter().enumerate() {
        ノード.push(VrmNode {
            name: 名.to_string(),
            parent: 親[索引],
            children: 子[索引].clone(),
            local: 単位行列(),
        });
    }
    // メッシュ所有ノードは親なし・子なし・単位行列である。
    ノード.push(VrmNode {
        name: "Body".to_string(),
        parent: None,
        children: Vec::new(),
        local: 単位行列(),
    });
    let メッシュノード = ノード.len() - 1;
    let 関節: Vec<usize> = (0..23).collect();
    let スキン = VrmSkin {
        joints: 関節.clone(),
        inverse_bind_matrices: 関節.iter().map(|_| 単位行列()).collect(),
        joint_models: 関節.iter().map(|i| ObjectId::new(*i as i64 + 1)).collect(),
    };
    let メッシュ = VrmMesh {
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
        targets: Vec::new(),
    };
    VrmModel {
        root: 単位行列(),
        nodes: ノード,
        skins: vec![スキン],
        meshes: vec![メッシュ],
        expressions: VrmExpressions::default(),
    }
}

/// 書き出し設定を作る。
fn 書き出し設定を作る() -> VrmExportOptions {
    VrmExportOptions::new(VrmMeta::new("試験", vec!["作者".to_string()]))
}

/// GLBをヘッダ・JSON・BINに分ける。境界と型も確かめる。
fn glbを分解する(バイト列: &[u8]) -> (serde_json::Value, Vec<u8>) {
    assert!(バイト列.len() >= 12, "GLBは12バイトのヘッダを持つ");
    assert_eq!(&バイト列[0..4], b"glTF", "先頭はglTFの印である");
    assert_eq!(
        u32::from_le_bytes(バイト列[4..8].try_into().expect("版は4バイトである")),
        2,
        "版は2である"
    );
    let 全体 =
        u32::from_le_bytes(バイト列[8..12].try_into().expect("長さは4バイトである")) as usize;
    assert_eq!(全体, バイト列.len(), "ヘッダの長さは全体と一致する");
    let json長 =
        u32::from_le_bytes(バイト列[12..16].try_into().expect("塊長は4バイトである")) as usize;
    assert_eq!(
        u32::from_le_bytes(バイト列[16..20].try_into().expect("塊種は4バイトである")),
        0x4E4F534A,
        "JSON塊の種別はJSONである"
    );
    assert_eq!(json長 % 4, 0, "JSON塊は4バイト境界である");
    let json = &バイト列[20..20 + json長];
    // 末尾の詰めは空白であり、JSONとして読める範囲に含まない。
    let json終端 = json
        .iter()
        .rposition(|b| *b != b' ')
        .map(|位置| 位置 + 1)
        .unwrap_or(0);
    let 値: serde_json::Value =
        serde_json::from_slice(&json[..json終端]).expect("JSON塊はJSONとして読める");
    let mut 残り = &バイト列[20 + json長..];
    let mut bin = Vec::new();
    if !残り.is_empty() {
        assert!(残り.len() >= 8, "BIN塊は長さと種別を持つ");
        let bin長 =
            u32::from_le_bytes(残り[0..4].try_into().expect("塊長は4バイトである")) as usize;
        assert_eq!(
            u32::from_le_bytes(残り[4..8].try_into().expect("塊種は4バイトである")),
            0x004E4942,
            "BIN塊の種別はBINである"
        );
        assert_eq!(bin長 % 4, 0, "BIN塊は4バイト境界である");
        残り = &残り[8..];
        assert!(残り.len() >= bin長, "BIN塊の長さは残りに収まる");
        bin.extend_from_slice(&残り[..bin長]);
        // 末尾の詰めは0である。
        assert!(
            残り[bin長..].iter().all(|b| *b == 0),
            "BIN塊の詰めは0である"
        );
        assert!(
            残り[bin長..].len() + bin長 == 残り.len(),
            "BIN塊の後に余りは無い"
        );
    }
    (値, bin)
}

/// `gltf::json::Root` に読み直し、検証報告が空であることを確かめる。
fn 検証報告が空であることを確かめる(json: &[u8]) {
    let 根: gltf::json::Root =
        gltf::json::Root::from_slice(json).expect("JSONはgltf::json::Rootに読める");
    let mut 報告 = Vec::new();
    gltf::json::validation::Validate::validate(
        &根,
        &根,
        gltf::json::path::Path::new,
        &mut |道: &dyn Fn() -> gltf::json::Path, 種: gltf::json::validation::Error| {
            報告.push(format!("{}: {種:?}", 道().as_str()));
        },
    );
    assert!(報告.is_empty(), "検証報告は空である: {報告:?}");
}

/// JSON塊の生バイト列を取り出す。検証用の入力である。
fn json塊を取り出す(バイト列: &[u8]) -> Vec<u8> {
    let json長 =
        u32::from_le_bytes(バイト列[12..16].try_into().expect("塊長は4バイトである")) as usize;
    バイト列[20..20 + json長].to_vec()
}

#[test]
fn 合成モデルをvrmのglbに書き出せる() {
    let モデル = 合成モデルを作る();
    let バイト列 = モデル
        .to_vrm(&書き出し設定を作る())
        .expect("合成モデルは書き出せる");
    let (値, bin) = glbを分解する(&バイト列);
    assert!(!bin.is_empty(), "頂点があるためBIN塊がある");
    // JSON塊の生バイト列でも検証報告が空である。
    検証報告が空であることを確かめる(&json塊を取り出す(&バイト列));
    // バッファ長は詰め前のBIN長である。
    let バッファ長 = 値
        .get("buffers")
        .and_then(|値| 値.get(0))
        .and_then(|値| 値.get("byteLength"))
        .and_then(serde_json::Value::as_u64)
        .expect("buffers[0].byteLengthがある");
    assert_eq!(
        バッファ長 as usize,
        bin.len(),
        "buffers[0].byteLengthはBIN長である"
    );
}

#[test]
fn 拡張とmetaとhumanoidが仕様どおりに入る() {
    let モデル = 合成モデルを作る();
    let バイト列 = モデル
        .to_vrm(&書き出し設定を作る())
        .expect("合成モデルは書き出せる");
    let (値, _) = glbを分解する(&バイト列);
    let 使用拡張 = 値
        .get("extensionsUsed")
        .and_then(serde_json::Value::as_array)
        .expect("extensionsUsedがある");
    assert!(
        使用拡張.iter().any(|値| 値.as_str() == Some("VRMC_vrm")),
        "VRMC_vrmを使う"
    );
    let 拡張 = 値
        .get("extensions")
        .and_then(|値| 値.get("VRMC_vrm"))
        .expect("VRMC_vrm拡張がある");
    assert_eq!(
        拡張.get("specVersion").and_then(serde_json::Value::as_str),
        Some("1.0"),
        "specVersionは1.0である"
    );
    let meta = 拡張.get("meta").expect("metaがある");
    assert_eq!(
        meta.get("name").and_then(serde_json::Value::as_str),
        Some("試験"),
        "metaのnameが入る"
    );
    assert!(
        meta.get("authors")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|作者| 作者.iter().any(|値| 値.as_str() == Some("作者"))),
        "metaのauthorsが入る"
    );
    assert_eq!(
        meta.get("licenseUrl").and_then(serde_json::Value::as_str),
        Some("https://vrm.dev/licenses/1.0/"),
        "metaのlicenseUrlは既定値である"
    );
    let 骨 = 拡張
        .get("humanoid")
        .and_then(|値| 値.get("humanBones"))
        .and_then(serde_json::Value::as_object)
        .expect("humanBonesがある");
    // 必須15骨がすべてnode付きで存在する。
    let 必須 = [
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
    ];
    for 名 in 必須 {
        let 項目 = 骨.get(名).unwrap_or_else(|| panic!("必須骨が無い: {名}"));
        assert!(
            項目
                .get("node")
                .and_then(serde_json::Value::as_u64)
                .is_some(),
            "必須骨はnodeを持つ: {名}"
        );
    }
}

#[test]
fn 骨の名前と必須が仕様どおりである() {
    assert_eq!(HumanoidBone::Hips.name(), "hips");
    assert_eq!(HumanoidBone::UpperChest.name(), "upperChest");
    assert_eq!(HumanoidBone::LeftUpperArm.name(), "leftUpperArm");
    assert_eq!(HumanoidBone::RightToes.name(), "rightToes");
    // 必須は15個である。
    let 必須数 = [
        HumanoidBone::Hips,
        HumanoidBone::Spine,
        HumanoidBone::Chest,
        HumanoidBone::UpperChest,
        HumanoidBone::Neck,
        HumanoidBone::Head,
        HumanoidBone::LeftEye,
        HumanoidBone::RightEye,
        HumanoidBone::LeftShoulder,
        HumanoidBone::RightShoulder,
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
        HumanoidBone::LeftToes,
        HumanoidBone::RightToes,
    ]
    .into_iter()
    .filter(|骨| 骨.is_required())
    .count();
    assert_eq!(必須数, 15, "必須骨は15個である");
    assert!(!HumanoidBone::Chest.is_required(), "chestは任意である");
    assert!(!HumanoidBone::LeftEye.is_required(), "目は任意である");
}

#[test]
fn 区切り付き名をhumanoidに対応付けられる() {
    let モデル = 合成モデルを作る();
    let 骨 = HumanoidBones::detect(&モデル).expect("区切り付き名は対応付けられる");
    // 24骨すべてに対応付く。メッシュノード（Body）は対象外である。
    assert_eq!(骨.iter().count(), 23, "24骨のうち23骨に対応付く");
    assert_eq!(骨.get(HumanoidBone::Hips), Some(0));
    assert_eq!(骨.get(HumanoidBone::LeftShoulder), Some(7));
    assert_eq!(骨.get(HumanoidBone::LeftUpperArm), Some(9));
    assert_eq!(骨.get(HumanoidBone::LeftLowerArm), Some(11));
    assert_eq!(骨.get(HumanoidBone::LeftHand), Some(13));
    assert_eq!(骨.get(HumanoidBone::LeftUpperLeg), Some(15));
    assert_eq!(骨.get(HumanoidBone::LeftLowerLeg), Some(17));
    assert_eq!(骨.get(HumanoidBone::LeftFoot), Some(19));
    assert_eq!(骨.get(HumanoidBone::LeftToes), Some(21));
    // 明示指定の上書きと反復ができる。
    let mut 上書き = 骨;
    上書き.set(HumanoidBone::Hips, 5);
    assert_eq!(上書き.get(HumanoidBone::Hips), Some(5));
    assert!(上書き.iter().all(|(_, 索引)| 索引 < モデル.nodes.len()));
}

#[test]
fn 必須骨の欠落で失敗する() {
    let mut モデル = 合成モデルを作る();
    // Headを検出対象外の名前に変える。
    モデル.nodes[4].name = "Helmet".to_string();
    let 失敗 = モデル
        .to_vrm(&書き出し設定を作る())
        .expect_err("必須骨の欠落は失敗する");
    assert_eq!(失敗.kind(), VrmExportErrorKind::MissingHumanoidBone);
    assert_eq!(失敗.bone(), Some(HumanoidBone::Head));
}

#[test]
fn 同名の重複で失敗する() {
    let mut モデル = 合成モデルを作る();
    // メッシュノードをHipsと同名にする。
    モデル.nodes[23].name = "Hips".to_string();
    let 失敗 = モデル
        .to_vrm(&書き出し設定を作る())
        .expect_err("同名の重複は失敗する");
    assert_eq!(失敗.kind(), VrmExportErrorKind::DuplicateHumanoidBone);
    assert_eq!(失敗.bone(), Some(HumanoidBone::Hips));
}

#[test]
fn 同じノードの重複割当で失敗する() {
    let モデル = 合成モデルを作る();
    let mut 骨 = HumanoidBones::detect(&モデル).expect("合成モデルは対応付けられる");
    // LeftHandをRightHandと同じノードに割り当てる。
    骨.set(HumanoidBone::LeftHand, 14);
    let mut 設定 = 書き出し設定を作る();
    設定.humanoid = Some(骨);
    let 失敗 = モデル.to_vrm(&設定).expect_err("重複割当は失敗する");
    assert_eq!(失敗.kind(), VrmExportErrorKind::DuplicateHumanoidBone);
}

#[test]
fn 親子不一致で失敗する() {
    let mut モデル = 合成モデルを作る();
    // LeftHandの親をHipsに付け替え、最も近いhumanoid祖先をずらす。
    モデル.nodes[13].parent = Some(0);
    let 失敗 = モデル
        .to_vrm(&書き出し設定を作る())
        .expect_err("親子不一致は失敗する");
    assert_eq!(失敗.kind(), VrmExportErrorKind::InvalidHumanoidParent);
    assert_eq!(失敗.bone(), Some(HumanoidBone::LeftHand));
}

#[test]
fn 非正の拡大率で失敗する() {
    // 零拡大の場合は列長が正でない。
    let mut 零 = 合成モデルを作る();
    零.nodes[0].local = [0.0; 16];
    let 失敗 = 零
        .to_vrm(&書き出し設定を作る())
        .expect_err("零拡大は失敗する");
    assert_eq!(失敗.kind(), VrmExportErrorKind::NonPositiveHumanoidScale);
    assert_eq!(失敗.bone(), Some(HumanoidBone::Hips));
    // 鏡像の場合は行列式が正でない。
    let mut 鏡像 = 合成モデルを作る();
    鏡像.nodes[0].local = [
        -1.0, 0.0, 0.0, 0.0, //
        0.0, 1.0, 0.0, 0.0, //
        0.0, 0.0, 1.0, 0.0, //
        0.0, 0.0, 0.0, 1.0, //
    ];
    let 失敗 = 鏡像
        .to_vrm(&書き出し設定を作る())
        .expect_err("鏡像は失敗する");
    assert_eq!(失敗.kind(), VrmExportErrorKind::NonPositiveHumanoidScale);
}

#[test]
fn 頂点属性の長さ不一致で失敗する() {
    let mut モデル = 合成モデルを作る();
    モデル.meshes[0].vertices.normals = Some(vec![[0.0, 0.0, 1.0]]);
    let 失敗 = モデル
        .to_vrm(&書き出し設定を作る())
        .expect_err("属性長不一致は失敗する");
    assert_eq!(失敗.kind(), VrmExportErrorKind::InvalidStructure);
}

#[test]
fn 非有限の基準色で失敗する() {
    // 公開APIから渡された非有限値は書き出し時に拒否する。
    let モデル = 合成モデルを作る();
    let 材質 = VrmMaterials::new(
        vec![VrmMaterialEntry {
            slot: "Body".to_string(),
            material: VrmMaterial {
                name: "Body".to_string(),
                alpha_mode: VrmAlphaMode::Opaque,
                alpha_cutoff: 0.5,
                double_sided: false,
                base_color_factor: [f32::NAN, 1.0, 1.0, 1.0],
                base_color_image: None,
                emissive_factor: [0.0, 0.0, 0.0],
                emissive_image: None,
                mtoon: None,
            },
        }],
        vec![],
    );
    let 設定 = 書き出し設定を作る().with_materials(材質);
    let 失敗 = モデル.to_vrm(&設定).expect_err("非有限の基準色は失敗する");
    assert_eq!(失敗.kind(), VrmExportErrorKind::InvalidStructure);
}

#[test]
fn 範囲外のカットオフで失敗する() {
    // 有限でも範囲外の値は書き出し時に拒否する。
    let モデル = 合成モデルを作る();
    let 材質 = VrmMaterials::new(
        vec![VrmMaterialEntry {
            slot: "Body".to_string(),
            material: VrmMaterial {
                name: "Body".to_string(),
                alpha_mode: VrmAlphaMode::Mask,
                alpha_cutoff: 2.0,
                double_sided: false,
                base_color_factor: [1.0, 1.0, 1.0, 1.0],
                base_color_image: None,
                emissive_factor: [0.0, 0.0, 0.0],
                emissive_image: None,
                mtoon: None,
            },
        }],
        vec![],
    );
    let 設定 = 書き出し設定を作る().with_materials(材質);
    let 失敗 = モデル
        .to_vrm(&設定)
        .expect_err("範囲外のカットオフは失敗する");
    assert_eq!(失敗.kind(), VrmExportErrorKind::InvalidStructure);
}

#[test]
fn metaの必須項目の欠落で失敗する() {
    let モデル = 合成モデルを作る();
    // nameが空の場合は失敗する。
    let mut 空名 = VrmMeta::new("", vec!["作者".to_string()]);
    空名.license_url = "https://vrm.dev/licenses/1.0/".to_string();
    let 失敗 = モデル
        .to_vrm(&VrmExportOptions::new(空名))
        .expect_err("空のnameは失敗する");
    assert_eq!(失敗.kind(), VrmExportErrorKind::InvalidMeta);
    // authorsに空でない要素が無い場合は失敗する。
    let 空作者 = VrmMeta::new("試験", vec!["".to_string()]);
    let 失敗 = モデル
        .to_vrm(&VrmExportOptions::new(空作者))
        .expect_err("空のauthorsは失敗する");
    assert_eq!(失敗.kind(), VrmExportErrorKind::InvalidMeta);
    // licenseUrlが空の場合は失敗する。
    let mut 空許諾 = VrmMeta::new("試験", vec!["作者".to_string()]);
    空許諾.license_url = "".to_string();
    let 失敗 = モデル
        .to_vrm(&VrmExportOptions::new(空許諾))
        .expect_err("空のlicenseUrlは失敗する");
    assert_eq!(失敗.kind(), VrmExportErrorKind::InvalidMeta);
}

#[test]
fn 材質スロットが二つで材質が二つになる() {
    let mut モデル = 合成モデルを作る();
    モデル.meshes[0].material_slots = vec!["Body".to_string(), "Face".to_string()];
    モデル.meshes[0].vertices.positions = vec![
        [0.0, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, 0.0, 1.0],
        [1.0, 0.0, 1.0],
        [0.0, 1.0, 1.0],
    ];
    モデル.meshes[0].vertices.normals = Some(vec![[0.0, 0.0, 1.0]; 6]);
    モデル.meshes[0].vertices.uv_sets = vec![vec![[0.0, 0.0]; 6]];
    モデル.meshes[0].vertices.joints = Some(vec![[0, 0, 0, 0]; 6]);
    モデル.meshes[0].vertices.weights = Some(vec![[1.0, 0.0, 0.0, 0.0]; 6]);
    モデル.meshes[0].primitives = vec![
        VrmPrimitive {
            material: 0,
            indices: vec![0, 1, 2],
        },
        VrmPrimitive {
            material: 1,
            indices: vec![3, 4, 5],
        },
    ];
    let バイト列 = モデル
        .to_vrm(&書き出し設定を作る())
        .expect("2材質は書き出せる");
    let (値, _) = glbを分解する(&バイト列);
    検証報告が空であることを確かめる(&json塊を取り出す(&バイト列));
    let 材質 = 値
        .get("materials")
        .and_then(serde_json::Value::as_array)
        .expect("materialsがある");
    assert_eq!(材質.len(), 2, "スロットごとに材質ができる");
    let 名前: Vec<&str> = 材質
        .iter()
        .map(|値| {
            値.get("name")
                .and_then(serde_json::Value::as_str)
                .expect("材質名がある")
        })
        .collect();
    assert_eq!(名前, vec!["Body", "Face"], "材質名はスロット名である");
    let 基本色: Vec<[f32; 4]> = 材質
        .iter()
        .map(|値| {
            let 係数 = 値
                .get("pbrMetallicRoughness")
                .and_then(|値| 値.get("baseColorFactor"))
                .expect("baseColorFactorがある");
            [
                係数
                    .get(0)
                    .and_then(serde_json::Value::as_f64)
                    .expect("要素がある") as f32,
                係数
                    .get(1)
                    .and_then(serde_json::Value::as_f64)
                    .expect("要素がある") as f32,
                係数
                    .get(2)
                    .and_then(serde_json::Value::as_f64)
                    .expect("要素がある") as f32,
                係数
                    .get(3)
                    .and_then(serde_json::Value::as_f64)
                    .expect("要素がある") as f32,
            ]
        })
        .collect();
    assert_eq!(
        基本色,
        vec![[0.8, 0.8, 0.8, 1.0], [0.8, 0.8, 0.8, 1.0]],
        "基本色は中間調の灰色である"
    );
    let メッシュ = 値
        .get("meshes")
        .and_then(serde_json::Value::as_array)
        .expect("meshesがある");
    let プリミティブ = メッシュ[0]
        .get("primitives")
        .and_then(serde_json::Value::as_array)
        .expect("primitivesがある");
    assert_eq!(プリミティブ.len(), 2, "プリミティブは2件である");
    for (番号, 期待) in [(0, "Body"), (1, "Face")] {
        let 材質索引 = プリミティブ[番号]
            .get("material")
            .and_then(serde_json::Value::as_u64)
            .expect("primitiveはmaterialを持つ") as usize;
        assert_eq!(
            材質[材質索引]
                .get("name")
                .and_then(serde_json::Value::as_str),
            Some(期待),
            "primitiveのmaterialは対応するスロットを指す"
        );
        assert!(
            プリミティブ[番号]
                .get("mode")
                .and_then(serde_json::Value::as_u64)
                .is_none_or(|種| 種 == 4),
            "primitiveのmodeはTriangles（既定値の省略を含む）である"
        );
    }
}

#[test]
fn 出力上限の超過で失敗する() {
    let モデル = 合成モデルを作る();
    let 設定 = 書き出し設定を作る().with_max_output_bytes(10);
    let 失敗 = モデル.to_vrm(&設定).expect_err("上限超過は失敗する");
    assert_eq!(失敗.kind(), VrmExportErrorKind::LimitExceeded);
}

#[test]
fn binが必要量を超える上限では組み立て途中で失敗する() {
    // BIN単体の必要量より小さい上限では、最終段を待たず追加前に打ち切る。
    // 失敗種別が上限超過であり、部分的な出力を返さないことを確かめる。
    let モデル = 合成モデルを作る();
    let 既定出力 = モデル
        .to_vrm(&書き出し設定を作る())
        .expect("合成モデルは書き出せる");
    let (_, bin) = glbを分解する(&既定出力);
    assert!(!bin.is_empty(), "前提としてBINがある");
    let 上限 = bin.len() / 2;
    assert!(上限 > 0 && 上限 < bin.len(), "上限はBIN必要量の内側である");
    let 設定 = 書き出し設定を作る().with_max_output_bytes(上限);
    // `to_vrm` は `Err` を返すため、部分的に巨大な出力を返すことはない。
    let 失敗 = モデル.to_vrm(&設定).expect_err("BIN超過は失敗する");
    assert_eq!(失敗.kind(), VrmExportErrorKind::LimitExceeded);
    // 全体の必要量ちょうどの上限では通るため、過って拒まない。
    let 通過設定 = 書き出し設定を作る().with_max_output_bytes(既定出力.len());
    モデル
        .to_vrm(&通過設定)
        .expect("必要量ちょうどの上限では通る");
}

#[test]
fn humanoidのノード索引は中間モデルのままである() {
    // 合成ルートを置かないため、+1シフトは不要である。
    let モデル = 合成モデルを作る();
    let バイト列 = モデル
        .to_vrm(&書き出し設定を作る())
        .expect("合成モデルは書き出せる");
    let (値, _) = glbを分解する(&バイト列);
    let 骨 = 値
        .get("extensions")
        .and_then(|値| 値.get("VRMC_vrm"))
        .and_then(|値| 値.get("humanoid"))
        .and_then(|値| 値.get("humanBones"))
        .and_then(serde_json::Value::as_object)
        .expect("humanBonesがある");
    // 検出結果（Hips=0・LeftHand=13）と一致する。
    assert_eq!(
        骨.get("hips")
            .and_then(|値| 値.get("node"))
            .and_then(serde_json::Value::as_u64),
        Some(0),
        "hipsは中間モデルの索引のままである"
    );
    assert_eq!(
        骨.get("leftHand")
            .and_then(|値| 値.get("node"))
            .and_then(serde_json::Value::as_u64),
        Some(13),
        "leftHandは中間モデルの索引のままである"
    );
}

#[test]
fn 逆バインド行列のビューにtargetが無い() {
    // 逆バインド行列は頂点属性ではないため、`target` を付けない。
    let モデル = 合成モデルを作る();
    let バイト列 = モデル
        .to_vrm(&書き出し設定を作る())
        .expect("合成モデルは書き出せる");
    let 根: gltf::json::Root =
        gltf::json::Root::from_slice(&json塊を取り出す(&バイト列)).expect("JSONは読める");
    let 逆バインド = 根.skins[0]
        .inverse_bind_matrices
        .expect("逆バインド行列がある");
    let 逆バインドビュー = 根.accessors[逆バインド.value()]
        .buffer_view
        .expect("逆バインド行列はビューを持つ");
    assert!(
        根.buffer_views[逆バインドビュー.value()].target.is_none(),
        "逆バインド行列のビューにtargetは無い"
    );
    // 頂点属性と索引は用途どおりのtargetを持つ。
    let 位置 = 根.meshes[0].primitives[0]
        .attributes
        .get(&gltf::json::validation::Checked::Valid(
            gltf::json::mesh::Semantic::Positions,
        ))
        .expect("位置がある");
    let 位置ビュー = 根.accessors[位置.value()]
        .buffer_view
        .expect("位置はビューを持つ");
    assert_eq!(
        根.buffer_views[位置ビュー.value()].target,
        Some(gltf::json::validation::Checked::Valid(
            gltf::json::buffer::Target::ArrayBuffer
        )),
        "位置のビューはARRAY_BUFFERである"
    );
    let 索引 = 根.meshes[0].primitives[0].indices.expect("索引がある");
    let 索引ビュー = 根.accessors[索引.value()]
        .buffer_view
        .expect("索引はビューを持つ");
    assert_eq!(
        根.buffer_views[索引ビュー.value()].target,
        Some(gltf::json::validation::Checked::Valid(
            gltf::json::buffer::Target::ElementArrayBuffer
        )),
        "索引のビューはELEMENT_ARRAY_BUFFERである"
    );
}

#[test]
fn スキンメッシュのノードはsceneのルートである() {
    // スキンメッシュのノードを合成ルートの下に置くと警告になるため、
    // 親なしのままscene直下に置く。
    let モデル = 合成モデルを作る();
    let メッシュノード = モデル.meshes[0].node;
    assert!(
        モデル.nodes[メッシュノード].parent.is_none(),
        "前提としてメッシュノードは親なしである"
    );
    let バイト列 = モデル
        .to_vrm(&書き出し設定を作る())
        .expect("合成モデルは書き出せる");
    let 根: gltf::json::Root =
        gltf::json::Root::from_slice(&json塊を取り出す(&バイト列)).expect("JSONは読める");
    let 場面 = &根.scenes[根.scene.as_ref().expect("sceneがある").value()];
    assert!(
        場面.nodes.iter().any(|索引| 索引.value() == メッシュノード),
        "メッシュノードはscene.nodesに含まれる"
    );
    // 他のノードの子になっていない（親が無い）。
    assert!(
        !根.nodes
            .iter()
            .filter_map(|節| 節.children.as_ref())
            .any(|子ら| 子ら.iter().any(|子| 子.value() == メッシュノード)),
        "メッシュノードはどのノードの子でもない"
    );
    let 節 = &根.nodes[メッシュノード];
    assert!(節.mesh.is_some(), "メッシュノードはmeshを持つ");
    assert!(節.skin.is_some(), "メッシュノードはskinを持つ");
}

#[test]
fn 単位行列のノードはmatrixが無い() {
    // glTFの既定値が単位行列のため、単位行列の箇所は省略する。
    let モデル = 合成モデルを作る();
    let バイト列 = モデル
        .to_vrm(&書き出し設定を作る())
        .expect("合成モデルは書き出せる");
    let (値, _) = glbを分解する(&バイト列);
    let 節ら = 値
        .get("nodes")
        .and_then(serde_json::Value::as_array)
        .expect("nodesがある");
    assert_eq!(節ら.len(), モデル.nodes.len(), "ノード数は一致する");
    // 合成モデルはroot・localとも単位行列のため、すべて省略される。
    for (番号, 節) in 節ら.iter().enumerate() {
        assert!(
            節.get("matrix").is_none(),
            "単位行列のノードにmatrixは無い: {番号}"
        );
    }
}

#[test]
fn 親なしノードはrootとlocalの積を持つ() {
    // `root` は列優先・列ベクトル規約のため、左から掛ける。
    let mut モデル = 合成モデルを作る();
    モデル.root = [
        2.0, 0.0, 0.0, 0.0, //
        0.0, 2.0, 0.0, 0.0, //
        0.0, 0.0, 2.0, 0.0, //
        0.0, 0.0, 0.0, 1.0, //
    ];
    let バイト列 = モデル
        .to_vrm(&書き出し設定を作る())
        .expect("拡大ルートは書き出せる");
    let (値, _) = glbを分解する(&バイト列);
    let 節ら = 値
        .get("nodes")
        .and_then(serde_json::Value::as_array)
        .expect("nodesがある");
    // Hips（親なし）の行列は `root * local = root` である。
    let 腰 = 節ら[0].get("matrix").expect("親なしノードはmatrixを持つ");
    let 対角: Vec<f64> = 腰
        .as_array()
        .expect("matrixは配列である")
        .iter()
        .map(|値| 値.as_f64().expect("要素は数である"))
        .collect();
    assert_eq!(
        対角,
        vec![
            2.0, 0.0, 0.0, 0.0, //
            0.0, 2.0, 0.0, 0.0, //
            0.0, 0.0, 2.0, 0.0, //
            0.0, 0.0, 0.0, 1.0, //
        ],
        "親なしノードの行列はrootとlocalの積である"
    );
    // Spine（親あり・local単位行列）は省略される。
    assert!(
        節ら[1].get("matrix").is_none(),
        "local単位行列の非ルートにmatrixは無い"
    );
}

/// 1x1のPNGバイト列。GLBへそのまま埋め込む。
const PNG_1X1: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4,
    0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78, 0xDA, 0x63, 0x64, 0x60, 0xF8, 0x5F,
    0x0F, 0x00, 0x02, 0x87, 0x01, 0x80, 0xEB, 0x47, 0xBA, 0x92, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45,
    0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
];

/// 手組みのMToon材質1件を作る。画像は1x1のPNG1件を共有する。
fn mtoon材質を作る(
    スロット: &str,
    方式: VrmAlphaMode,
    閾値: f32,
    mtoon: VrmMtoonParameters,
) -> VrmMaterials {
    VrmMaterials::new(
        vec![VrmMaterialEntry {
            slot: スロット.to_string(),
            material: VrmMaterial {
                name: format!("{スロット}の材質"),
                alpha_mode: 方式,
                alpha_cutoff: 閾値,
                double_sided: true,
                base_color_factor: [1.0, 0.0, 0.0, 1.0],
                base_color_image: Some(0),
                emissive_factor: [0.0, 1.0, 0.0],
                emissive_image: Some(0),
                mtoon: Some(mtoon),
            },
        }],
        vec![VrmImage {
            name: "Body".to_string(),
            mime: VrmImageMime::Png,
            data: PNG_1X1.to_vec(),
            sampler: VrmSampler::default(),
        }],
    )
}

#[test]
fn mtoon材質を書き出せる() {
    let モデル = 合成モデルを作る();
    let mtoon = VrmMtoonParameters {
        shade_color_factor: [0.5, 0.5, 0.5],
        outline_width_mode: VrmOutlineWidthMode::WorldCoordinates,
        outline_width_factor: 0.02,
        ..Default::default()
    };
    let 材質 = mtoon材質を作る("Body", VrmAlphaMode::Mask, 0.5, mtoon);
    let 設定 = 書き出し設定を作る().with_materials(材質);
    let バイト列 = モデル.to_vrm(&設定).expect("MToon材質は書き出せる");
    let (値, bin) = glbを分解する(&バイト列);
    検証報告が空であることを確かめる(&json塊を取り出す(&バイト列));
    // 拡張一覧にMToonが入る。
    let 使用拡張 = 値
        .get("extensionsUsed")
        .and_then(serde_json::Value::as_array)
        .expect("extensionsUsedがある");
    assert!(
        使用拡張
            .iter()
            .any(|値| 値.as_str() == Some("VRMC_materials_mtoon")),
        "VRMC_materials_mtoonを使う"
    );
    // 画像・テクスチャ・サンプラが入る。
    let 画像 = 値
        .get("images")
        .and_then(serde_json::Value::as_array)
        .expect("imagesがある");
    assert_eq!(画像.len(), 1, "画像は1件である");
    assert_eq!(
        画像[0].get("mimeType").and_then(serde_json::Value::as_str),
        Some("image/png"),
        "MIMEはPNGである"
    );
    let テクスチャ = 値
        .get("textures")
        .and_then(serde_json::Value::as_array)
        .expect("texturesがある");
    assert_eq!(テクスチャ.len(), 1, "テクスチャは画像ごとに1件である");
    assert_eq!(
        テクスチャ[0]
            .get("source")
            .and_then(serde_json::Value::as_u64),
        Some(0),
        "テクスチャは画像0を指す"
    );
    let サンプラ = 値
        .get("samplers")
        .and_then(serde_json::Value::as_array)
        .expect("samplersがある");
    assert_eq!(サンプラ.len(), 1, "サンプラは値の組で重複排除する");
    assert_eq!(
        サンプラ[0].get("wrapS").and_then(serde_json::Value::as_u64),
        Some(10497),
        "wrapSはREPEATである"
    );
    assert_eq!(
        サンプラ[0].get("wrapT").and_then(serde_json::Value::as_u64),
        Some(10497),
        "wrapTはREPEATである"
    );
    assert_eq!(
        サンプラ[0]
            .get("magFilter")
            .and_then(serde_json::Value::as_u64),
        Some(9729),
        "magFilterはLINEARである"
    );
    assert_eq!(
        サンプラ[0]
            .get("minFilter")
            .and_then(serde_json::Value::as_u64),
        Some(9985),
        "minFilterはmipmapありのLINEAR_MIPMAP_NEARESTである"
    );
    // 材質の主要値を確認する。
    let 材質 = 値
        .get("materials")
        .and_then(serde_json::Value::as_array)
        .expect("materialsがある");
    assert_eq!(材質.len(), 1);
    assert_eq!(
        材質[0].get("name").and_then(serde_json::Value::as_str),
        Some("Bodyの材質"),
        "材質名は読み取り結果である"
    );
    assert_eq!(
        材質[0].get("alphaMode").and_then(serde_json::Value::as_str),
        Some("MASK"),
        "透過方式はカットアウトである"
    );
    assert_eq!(
        材質[0]
            .get("alphaCutoff")
            .and_then(serde_json::Value::as_f64),
        Some(0.5),
        "カットアウトの閾値が入る"
    );
    assert_eq!(
        材質[0]
            .get("doubleSided")
            .and_then(serde_json::Value::as_bool),
        Some(true),
        "両面である"
    );
    let 基本色 = 材質[0]
        .get("pbrMetallicRoughness")
        .and_then(|値| 値.get("baseColorFactor"))
        .expect("baseColorFactorがある");
    assert_eq!(基本色, &serde_json::json!([1.0, 0.0, 0.0, 1.0]));
    assert_eq!(
        材質[0]
            .get("pbrMetallicRoughness")
            .and_then(|値| 値.get("baseColorTexture"))
            .and_then(|値| 値.get("index"))
            .and_then(serde_json::Value::as_u64),
        Some(0),
        "基準色テクスチャが入る"
    );
    assert_eq!(
        材質[0]
            .get("emissiveTexture")
            .and_then(|値| 値.get("index"))
            .and_then(serde_json::Value::as_u64),
        Some(0),
        "発光テクスチャは同じ画像を共有する"
    );
    let 拡張 = 材質[0]
        .get("extensions")
        .and_then(|値| 値.get("VRMC_materials_mtoon"))
        .expect("MToon拡張がある");
    assert_eq!(
        拡張.get("specVersion").and_then(serde_json::Value::as_str),
        Some("1.0")
    );
    assert_eq!(
        拡張.get("shadeColorFactor"),
        Some(&serde_json::json!([0.5, 0.5, 0.5]))
    );
    assert_eq!(
        拡張
            .get("outlineWidthMode")
            .and_then(serde_json::Value::as_str),
        Some("worldCoordinates")
    );
    let 幅 = 拡張
        .get("outlineWidthFactor")
        .and_then(serde_json::Value::as_f64)
        .expect("outlineWidthFactorがある");
    assert!(
        (幅 - 0.02).abs() < 1e-6,
        "outlineWidthFactorは0.02である: {幅}"
    );
    // 画像のbufferViewにtargetが無い。
    let 根: gltf::json::Root =
        gltf::json::Root::from_slice(&json塊を取り出す(&バイト列)).expect("JSONは読める");
    let ビュー = &根.buffer_views[根.images[0]
        .buffer_view
        .expect("画像はビューを持つ")
        .value()];
    assert!(ビュー.target.is_none(), "画像のビューにtargetは無い");
    assert!(!bin.is_empty());
}

#[test]
fn 半透明材質はカットオフを書かず透過z書き込みする() {
    let モデル = 合成モデルを作る();
    let mtoon = VrmMtoonParameters {
        transparent_with_z_write: true,
        ..Default::default()
    };
    let 材質 = mtoon材質を作る("Body", VrmAlphaMode::Blend, 0.7, mtoon);
    let 設定 = 書き出し設定を作る().with_materials(材質);
    let バイト列 = モデル.to_vrm(&設定).expect("半透明材質は書き出せる");
    let (値, _) = glbを分解する(&バイト列);
    検証報告が空であることを確かめる(&json塊を取り出す(&バイト列));
    let 材質 = 値
        .get("materials")
        .and_then(serde_json::Value::as_array)
        .expect("materialsがある");
    assert_eq!(
        材質[0].get("alphaMode").and_then(serde_json::Value::as_str),
        Some("BLEND")
    );
    assert!(
        材質[0].get("alphaCutoff").is_none(),
        "半透明にalphaCutoffは無い"
    );
    let 拡張 = 材質[0]
        .get("extensions")
        .and_then(|値| 値.get("VRMC_materials_mtoon"))
        .expect("MToon拡張がある");
    assert_eq!(
        拡張
            .get("transparentWithZWrite")
            .and_then(serde_json::Value::as_bool),
        Some(true)
    );
}

#[test]
fn uvの向きをgltfに合わせて反転する() {
    // 合成モデルは左下原点のUV `[[0,0],[1,0],[0,1]]` を持つ。
    let モデル = 合成モデルを作る();
    let バイト列 = モデル
        .to_vrm(&書き出し設定を作る())
        .expect("合成モデルは書き出せる");
    let json塊 = json塊を取り出す(&バイト列);
    検証報告が空であることを確かめる(&json塊);
    let 根: gltf::json::Root = gltf::json::Root::from_slice(&json塊).expect("JSONは読める");
    let (_, bin) = glbを分解する(&バイト列);
    let 属性 = &根.meshes[0].primitives[0].attributes;
    let 索引 = 属性
        .get(&gltf::json::validation::Checked::Valid(
            gltf::json::mesh::Semantic::TexCoords(0),
        ))
        .expect("TEXCOORD_0がある");
    let アクセサ = &根.accessors[索引.value()];
    let ビュー = &根.buffer_views[アクセサ.buffer_view.expect("ビューがある").value()];
    let 開始 = ビュー
        .byte_offset
        .as_ref()
        .map(|値| 値.0 as usize)
        .unwrap_or(0);
    let mut 値 = Vec::new();
    for 番号 in 0..アクセサ.count.0 as usize {
        let 位置 = 開始 + 番号 * 8;
        let u = f32::from_le_bytes(bin[位置..位置 + 4].try_into().expect("4バイトある"));
        let v = f32::from_le_bytes(bin[位置 + 4..位置 + 8].try_into().expect("4バイトある"));
        値.push([u, v]);
    }
    // Vだけが反転し、Uは変わらない。
    assert_eq!(値, vec![[0.0, 1.0], [1.0, 1.0], [0.0, 0.0]]);
}

#[test]
fn 材質が無い場合は灰色pbrでmtoon拡張は無い() {
    let モデル = 合成モデルを作る();
    let バイト列 = モデル
        .to_vrm(&書き出し設定を作る())
        .expect("合成モデルは書き出せる");
    let (値, _) = glbを分解する(&バイト列);
    let 使用拡張 = 値
        .get("extensionsUsed")
        .and_then(serde_json::Value::as_array)
        .expect("extensionsUsedがある");
    assert!(
        !使用拡張
            .iter()
            .any(|値| 値.as_str() == Some("VRMC_materials_mtoon")),
        "MToon材質が無いため拡張一覧に入らない"
    );
    let 材質 = 値
        .get("materials")
        .and_then(serde_json::Value::as_array)
        .expect("materialsがある");
    assert!(
        材質[0].get("extensions").is_none(),
        "灰色PBRに材質拡張は無い"
    );
    let 基本色 = 材質[0]
        .get("pbrMetallicRoughness")
        .and_then(|値| 値.get("baseColorFactor"))
        .expect("baseColorFactorがある");
    assert_eq!(基本色, &serde_json::json!([0.8, 0.8, 0.8, 1.0]));
}

/// 実fixtureの材質を読み書き出しに含める。
///
/// 祖先の `Assets` から配置を求め、`load` で読んだ材質でGLBを組み立てる。
/// 固定件数には依存せず、画像とMToon材質の存在と検証報告の空を確かめる。
/// バイト数・材質数・画像数を標準エラーに出す。
#[test]
#[cfg(not(target_arch = "wasm32"))]
fn 実fixtureの材質を読み書き出しに含める() {
    let Ok(パス) = std::env::var("VRCTOM_FBX_FIXTURE") else {
        eprintln!("VRCTOM_FBX_FIXTUREが未設定のため跳ばす");
        return;
    };
    let fbx = std::path::PathBuf::from(パス);
    let mut root = None;
    for 祖先 in fbx.ancestors() {
        if 祖先.join("Assets").is_dir() {
            root = Some(祖先.to_path_buf());
            break;
        }
    }
    let Some(root) = root else {
        panic!("実fixtureの祖先にAssetsが無い");
    };
    let (materials, report) =
        VrmMaterials::load(&root, &fbx, &MaterialOptions::new()).expect("実fixtureは読める");
    for 警告 in report.warnings() {
        eprintln!("実fixture材質警告: {警告}");
    }
    let materials = materials.expect("置換表があるため材質がある");
    let バイト列 = std::fs::read(&fbx).expect("実fixtureは読める");
    let 読み取り = FbxFile::parse(&バイト列, &FbxOptions::new()).expect("実fixtureは読める");
    let (モデル, _) = VrmModel::from_fbx(&読み取り, &vrctom::vrm::VrmOptions::new())
        .expect("実fixtureは変換できる");
    let 設定 = VrmExportOptions::new(VrmMeta::new("Milfy", vec!["PLUSONE".to_string()]))
        .with_materials(materials.clone());
    let 出力 = モデル.to_vrm(&設定).expect("実fixtureは書き出せる");
    eprintln!(
        "実fixture材質書き出し: {}バイト 材質{}件 画像{}件",
        出力.len(),
        materials.entries().len(),
        materials.images().len()
    );
    let (値, _) = glbを分解する(&出力);
    let json塊 = json塊を取り出す(&出力);
    検証報告が空であることを確かめる(&json塊);
    assert!(
        値.get("images")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|画像| !画像.is_empty()),
        "画像が入る"
    );
    let 使用拡張 = 値
        .get("extensionsUsed")
        .and_then(serde_json::Value::as_array)
        .expect("extensionsUsedがある");
    assert!(
        使用拡張
            .iter()
            .any(|値| 値.as_str() == Some("VRMC_materials_mtoon")),
        "MToon材質があるため拡張一覧に入る"
    );
    let 材質 = 値
        .get("materials")
        .and_then(serde_json::Value::as_array)
        .expect("materialsがある");
    assert!(
        材質.iter().any(|材質| 材質
            .get("extensions")
            .and_then(|拡張| 拡張.get("VRMC_materials_mtoon"))
            .is_some()),
        "MToon材質が1件以上ある"
    );

    // 外部の検証用に一時ディレクトリへ書き出す。
    let 作業 = tempfile::TempDir::new().expect("一時ディレクトリは作れる");
    let 書き出し先 = 作業.path().join("milfy-material.vrm");
    std::fs::write(&書き出し先, &出力).expect("一時ファイルに書ける");
    eprintln!("vrm_material_export_path={}", 書き出し先.display());
    // `作業` をここで落とすと一時ファイルが消える。パスだけを知らせる運用であり、
    // 検証中に消えないよう明示的に保持する。
    std::mem::forget(作業);
}

/// 実fixtureからVRMを書き出し、中間モデルと一致することを確かめる。
///
/// 固定件数には依存せず、ノード・メッシュ・スキンの件数、humanoidの必須骨、
/// 材質数の対応を検証する。バイト数を標準エラーに出す。
#[test]
#[cfg(not(target_arch = "wasm32"))]
fn 実fixtureをvrmに書き出し中間モデルと一致する() {
    let Ok(パス) = std::env::var("VRCTOM_FBX_FIXTURE") else {
        eprintln!("VRCTOM_FBX_FIXTUREが未設定のため跳ばす");
        return;
    };
    let バイト列 = std::fs::read(&パス).expect("実fixtureは読める");
    let 読み取り = FbxFile::parse(&バイト列, &FbxOptions::new()).expect("実fixtureは読める");
    let (モデル, _) = VrmModel::from_fbx(&読み取り, &vrctom::vrm::VrmOptions::new())
        .expect("実fixtureは変換できる");
    let 設定 = VrmExportOptions::new(VrmMeta::new("Milfy", vec!["PLUSONE".to_string()]));
    let 出力 = モデル.to_vrm(&設定).expect("実fixtureは書き出せる");
    eprintln!("vrm_export_bytes={}", 出力.len());
    let (値, bin) = glbを分解する(&出力);
    let json塊 = json塊を取り出す(&出力);
    検証報告が空であることを確かめる(&json塊);

    // ノード・メッシュ・スキン数が中間モデルと一致する。合成ルートは置かない。
    let 根: gltf::json::Root = gltf::json::Root::from_slice(&json塊).expect("JSONは読める");
    assert_eq!(
        根.nodes.len(),
        モデル.nodes.len(),
        "ノード数は中間モデルと一致する"
    );
    assert_eq!(根.meshes.len(), モデル.meshes.len(), "メッシュ数は一致する");
    assert_eq!(根.skins.len(), モデル.skins.len(), "スキン数は一致する");

    // humanoidの必須15骨がすべてnode付きで存在する。
    let 骨 = 値
        .get("extensions")
        .and_then(|値| 値.get("VRMC_vrm"))
        .and_then(|値| 値.get("humanoid"))
        .and_then(|値| 値.get("humanBones"))
        .and_then(serde_json::Value::as_object)
        .expect("humanBonesがある");
    for 必須 in [
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
    ] {
        assert!(
            骨.get(必須.name())
                .and_then(|値| 値.get("node"))
                .and_then(serde_json::Value::as_u64)
                .is_some(),
            "必須骨がnode付きで存在する: {}",
            必須.name()
        );
    }

    // 材質数はスロット名の一意数と一致する。
    let mut スロット = std::collections::HashSet::new();
    for メッシュ in &モデル.meshes {
        for 名 in &メッシュ.material_slots {
            スロット.insert(名.clone());
        }
    }
    let 材質数 = 値
        .get("materials")
        .and_then(serde_json::Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    assert_eq!(
        材質数,
        スロット.len(),
        "材質数はスロット名の一意数と一致する"
    );

    // BIN長とバッファ長の対応を確かめる。
    let バッファ長 = 値
        .get("buffers")
        .and_then(|値| 値.get(0))
        .and_then(|値| 値.get("byteLength"))
        .and_then(serde_json::Value::as_u64)
        .expect("buffers[0].byteLengthがある") as usize;
    assert_eq!(バッファ長, bin.len(), "buffers[0].byteLengthはBIN長である");

    // 外部のBlender検証用に一時ディレクトリへ書き出す。
    let 作業 = tempfile::TempDir::new().expect("一時ディレクトリは作れる");
    let 書き出し先 = 作業.path().join("milfy.vrm");
    std::fs::write(&書き出し先, &出力).expect("一時ファイルに書ける");
    eprintln!("vrm_export_path={}", 書き出し先.display());
    // `作業` をここで落とすと一時ファイルが消える。パスだけを知らせる運用であり、
    // 検証中に消えないよう明示的に保持する。
    std::mem::forget(作業);
}

/// 表情とモーフターゲットをGLBのJSONへ書き出す。
#[test]
fn 表情とモーフターゲットを書き出す() {
    let mut model = 合成モデルを作る();
    let node = model.meshes[0].node;
    model.meshes[0].targets = vec![VrmMorphTarget {
        name: "vrc.v_aa".to_owned(),
        positions: vec![[0.01, 0.0, 0.0]; 3],
    }];
    model.expressions = VrmExpressions {
        preset: vec![VrmExpression {
            name: "aa".to_owned(),
            morph_target_binds: vec![VrmMorphTargetBind {
                mesh: 0,
                target: 0,
                weight: 1.0,
            }],
            is_binary: false,
            override_blink: VrmExpressionOverride::None,
            override_look_at: VrmExpressionOverride::None,
            override_mouth: VrmExpressionOverride::None,
        }],
    };

    let bytes = model
        .to_vrm(&書き出し設定を作る())
        .expect("表情付きモデルは書き出せる");
    let (json, _) = glbを分解する(&bytes);

    let mesh = &json["meshes"][0];
    assert_eq!(mesh["weights"], serde_json::json!([0.0]));
    assert_eq!(
        mesh["extras"]["targetNames"],
        serde_json::json!(["vrc.v_aa"])
    );
    let target = &mesh["primitives"][0]["targets"][0]["POSITION"];
    let accessor = target.as_u64().expect("POSITIONアクセサがある") as usize;
    assert_eq!(json["accessors"][accessor]["count"], serde_json::json!(3));
    assert_eq!(
        json["accessors"][accessor]["type"],
        serde_json::json!("VEC3")
    );
    let min = json["accessors"][accessor]["min"][0]
        .as_f64()
        .expect("minがある");
    assert!((min - 0.01).abs() < 1e-6, "min={min}");

    let preset = &json["extensions"]["VRMC_vrm"]["expressions"]["preset"]["aa"];
    assert_eq!(
        preset["morphTargetBinds"],
        serde_json::json!([{"node": node, "index": 0, "weight": 1.0}])
    );
    assert_eq!(preset["isBinary"], serde_json::json!(false));
    assert_eq!(preset["overrideBlink"], serde_json::json!("none"));
}

/// 表情の重みとプリセット名の不正を拒否する。
#[test]
fn 表情の重みと名前の不正を拒否する() {
    for (name, weight) in [("aa", 1.5), ("not-a-preset", 1.0)] {
        let mut model = 合成モデルを作る();
        model.meshes[0].targets = vec![VrmMorphTarget {
            name: "vrc.v_aa".to_owned(),
            positions: vec![[0.0, 0.0, 0.0]; 3],
        }];
        model.expressions = VrmExpressions {
            preset: vec![VrmExpression {
                name: name.to_owned(),
                morph_target_binds: vec![VrmMorphTargetBind {
                    mesh: 0,
                    target: 0,
                    weight,
                }],
                is_binary: false,
                override_blink: VrmExpressionOverride::None,
                override_look_at: VrmExpressionOverride::None,
                override_mouth: VrmExpressionOverride::None,
            }],
        };
        assert_eq!(
            model
                .to_vrm(&書き出し設定を作る())
                .expect_err("不正な表情は拒否する")
                .kind(),
            VrmExportErrorKind::InvalidStructure,
            "name={name} weight={weight}"
        );
    }
}

/// モーフターゲットの非有限値を拒否する。
#[test]
fn モーフターゲットの非有限値を拒否する() {
    let mut model = 合成モデルを作る();
    model.meshes[0].targets = vec![VrmMorphTarget {
        name: "vrc.v_aa".to_owned(),
        positions: vec![[f32::NAN, 0.0, 0.0]; 3],
    }];
    assert_eq!(
        model
            .to_vrm(&書き出し設定を作る())
            .expect_err("非有限のモーフは拒否する")
            .kind(),
        VrmExportErrorKind::InvalidStructure
    );
}

/// モーフターゲットの長さ不一致を拒否する。
#[test]
fn モーフターゲットの長さ不一致を拒否する() {
    let mut model = 合成モデルを作る();
    model.meshes[0].targets = vec![VrmMorphTarget {
        name: "vrc.v_aa".to_owned(),
        positions: vec![[0.0, 0.0, 0.0]; 2],
    }];
    assert_eq!(
        model
            .to_vrm(&書き出し設定を作る())
            .expect_err("長さ不一致は拒否する")
            .kind(),
        VrmExportErrorKind::InvalidStructure
    );
}

/// 表情束ねのメッシュ索引・ターゲット索引の範囲外を拒否する。
#[test]
fn 表情束ねの索引範囲外を拒否する() {
    for (mesh, target) in [(1usize, 0usize), (0, 1)] {
        let mut model = 合成モデルを作る();
        model.meshes[0].targets = vec![VrmMorphTarget {
            name: "vrc.v_aa".to_owned(),
            positions: vec![[0.0, 0.0, 0.0]; 3],
        }];
        model.expressions = VrmExpressions {
            preset: vec![VrmExpression {
                name: "aa".to_owned(),
                morph_target_binds: vec![VrmMorphTargetBind {
                    mesh,
                    target,
                    weight: 1.0,
                }],
                is_binary: false,
                override_blink: VrmExpressionOverride::None,
                override_look_at: VrmExpressionOverride::None,
                override_mouth: VrmExpressionOverride::None,
            }],
        };
        assert_eq!(
            model
                .to_vrm(&書き出し設定を作る())
                .expect_err("範囲外は拒否する")
                .kind(),
            VrmExportErrorKind::InvalidStructure,
            "mesh={mesh} target={target}"
        );
    }
}

/// 表情名の空と重複を拒否する。
#[test]
fn 表情名の空と重複を拒否する() {
    for names in [["", "aa"], ["aa", "aa"]] {
        let model = VrmModel {
            expressions: VrmExpressions {
                preset: names
                    .iter()
                    .map(|name| VrmExpression {
                        name: (*name).to_owned(),
                        morph_target_binds: Vec::new(),
                        is_binary: false,
                        override_blink: VrmExpressionOverride::None,
                        override_look_at: VrmExpressionOverride::None,
                        override_mouth: VrmExpressionOverride::None,
                    })
                    .collect(),
            },
            ..合成モデルを作る()
        };
        assert_eq!(
            model
                .to_vrm(&書き出し設定を作る())
                .expect_err("空・重複は拒否する")
                .kind(),
            VrmExportErrorKind::InvalidStructure,
            "names={names:?}"
        );
    }
}
