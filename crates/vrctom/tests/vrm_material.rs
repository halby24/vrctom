//! 材質読み取りの契約テスト（公開 API のみを使う）。
//!
//! 手組みの最小配置でスロット解決・色空間・テクスチャ・上限を固定する。
//! 実fixtureを使う検証は `VRCTOM_FBX_FIXTURE` が指すファイルだけを読み、
//! 未設定なら跳ばす。

#![cfg(not(target_arch = "wasm32"))]

use std::fs;
use std::path::{Path, PathBuf};

use vrctom::fbx::{FbxFile, FbxOptions};
use vrctom::vrm::{
    MaterialOptions, VrmAlphaMode, VrmFilter, VrmImageMime, VrmMaterials, VrmOutlineWidthMode,
    VrmSampler, VrmWrap,
};

// テスト名は日本語で書く（テストは仕様・契約であるため）。

/// 1x1のPNGバイト列（デコードせず先頭判定だけに使う）。
const PNG_1X1: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4,
    0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78, 0xDA, 0x63, 0x64, 0x60, 0xF8, 0x5F,
    0x0F, 0x00, 0x02, 0x87, 0x01, 0x80, 0xEB, 0x47, 0xBA, 0x92, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45,
    0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
];

/// 不透明lilToonシェーダのGUID。
const SHADER_OPAQUE: &str = "df12117ecd77c31469c224178886498e";
/// アウトライン描画路を持つ不透明lilToonシェーダのGUID。
const SHADER_OUTLINE_OPAQUE: &str = "efa77a80ca0344749b4f19fdd5891cbe";
/// カットアウトlilToonシェーダのGUID。
const SHADER_MASK: &str = "85d6126cae43b6847aff4b13f4adb8ec";
/// 半透明lilToonシェーダのGUID。
const SHADER_BLEND: &str = "165365ab7100a044ca85fc8c33548a62";

/// テスト用GUID（32桁hex）を生成する。
fn guid(n: u128) -> String {
    format!("{n:032x}")
}

/// 作業域を作り、`Assets` への配置を受け付ける。
fn 作業域を作る() -> (tempfile::TempDir, PathBuf) {
    let guard = tempfile::TempDir::new().expect("作業域は作れる");
    let root = guard.path().to_path_buf();
    (guard, root)
}

/// 相対パスにバイト列を書き、親ディレクトリを作る。
fn 資産を書く(root: &Path, 相対: &str, 内容: &[u8]) {
    let 先 = root.join(相対);
    if let Some(親) = 先.parent() {
        fs::create_dir_all(親).expect("親ディレクトリは作れる");
    }
    fs::write(&先, 内容).expect("資産は書ける");
}

/// `.meta` を書く。`相対` は `.meta` を除いたアセットパスである。
fn メタを書く(root: &Path, 相対: &str, guid: &str, importer: &str, 本文: &str) {
    let mut out = format!("fileFormatVersion: 2\nguid: {guid}\n{importer}:\n");
    out.push_str(本文);
    資産を書く(root, &format!("{相対}.meta"), out.as_bytes());
}

/// FBXの `.meta` を置換表付きで書く。`置換表` は（スロット名、材質GUID）の列である。
/// 材質GUIDが `None` の場合はnull参照にする。
fn fbxメタを書く(root: &Path, guid: &str, 置換表: &[(&str, Option<&str>)]) {
    let mut entries = String::new();
    for (スロット, 材質) in 置換表 {
        let second = match 材質 {
            Some(guid) => format!("{{fileID: 2100000, guid: {guid}, type: 2}}"),
            None => "{fileID: 0}".to_string(),
        };
        entries.push_str(&format!(
            "  - first:\n      type: UnityEngine:Material\n      assembly: UnityEngine.CoreModule\n      name: {スロット}\n    second: {second}\n"
        ));
    }
    メタを書く(
        root,
        "Assets/Model.fbx",
        guid,
        "ModelImporter",
        &format!("  externalObjects:\n{entries}"),
    );
    資産を書く(root, "Assets/Model.fbx", b"");
}

/// テクスチャ環境の参照。
enum 参照<'a> {
    /// GUID付き参照。
    割当(&'a str),
    /// null参照。
    Null,
}

/// 最小の `.mat` を組み立てる。
#[allow(clippy::too_many_arguments)]
fn マテリアルを書く(
    root: &Path,
    相対: &str,
    guid: &str,
    名前: &str,
    シェーダ: Option<&str>,
    テクスチャ: &[(&str, 参照<'_>)],
    浮動小数: &[(&str, f64)],
    色: &[(&str, [f64; 4])],
) {
    let シェーダ欄 = match シェーダ {
        Some(guid) => format!("{{fileID: 4800000, guid: {guid}, type: 3}}"),
        None => "{fileID: 0}".to_string(),
    };
    let mut 環境 = String::new();
    for (名, 参照) in テクスチャ {
        let テクスチャ欄 = match 参照 {
            参照::割当(guid) => format!("{{fileID: 2800000, guid: {guid}, type: 3}}"),
            参照::Null => "{fileID: 0}".to_string(),
        };
        環境.push_str(&format!(
            "    - {名}:\n        m_Texture: {テクスチャ欄}\n        m_Scale: {{x: 1, y: 1}}\n        m_Offset: {{x: 0, y: 0}}\n"
        ));
    }
    // 空欄は `[]` と書く。値無しの `m_TexEnvs:` はnullになり読み取りが失敗する。
    let 環境欄 = if 環境.is_empty() {
        "    m_TexEnvs: []\n".to_string()
    } else {
        format!("    m_TexEnvs:\n{環境}")
    };
    let mut 浮動 = String::new();
    for (名, 値) in 浮動小数 {
        浮動.push_str(&format!("    - {名}: {値}\n"));
    }
    let mut 色欄 = String::new();
    for (名, 値) in 色 {
        色欄.push_str(&format!(
            "    - {名}: {{r: {}, g: {}, b: {}, a: {}}}\n",
            値[0], 値[1], 値[2], 値[3]
        ));
    }
    let 本文 = format!(
        "%YAML 1.1\n%TAG !u! tag:unity3d.com,2011:\n--- !u!21 &2100000\nMaterial:\n  m_Name: {名前}\n  m_Shader: {シェーダ欄}\n  m_SavedProperties:\n{環境欄}    m_Floats:\n{浮動}    m_Ints: []\n    m_Colors:\n{色欄}"
    );
    資産を書く(root, 相対, 本文.as_bytes());
    メタを書く(
        root,
        相対,
        guid,
        "NativeFormatImporter",
        "  externalObjects: {}\n  userData: \n",
    );
}

/// lilToonの既定に近い最小プロパティ列を作る。
fn 基本浮動小数() -> Vec<(&'static str, f64)> {
    vec![
        ("_Cutoff", 0.5),
        ("_Cull", 2.0),
        ("_ZWrite", 1.0),
        ("_UseShadow", 1.0),
        ("_ShadowStrength", 1.0),
        ("_ShadowBorder", 0.5),
        ("_ShadowBlur", 0.1),
        ("_UseMatCap", 0.0),
        ("_UseEmission", 0.0),
        ("_UseOutline", 0.0),
        ("_OutlineWidth", 0.05),
        ("_lilToonVersion", 45.0),
    ]
}

/// lilToonの既定に近い最小色列を作る。
fn 基本色() -> Vec<(&'static str, [f64; 4])> {
    vec![
        ("_Color", [1.0, 1.0, 1.0, 1.0]),
        ("_ShadowColor", [0.82, 0.76, 0.85, 1.0]),
        ("_MatCapColor", [1.0, 1.0, 1.0, 1.0]),
        ("_EmissionColor", [0.0, 0.0, 0.0, 1.0]),
        ("_OutlineColor", [0.0, 0.0, 0.0, 1.0]),
    ]
}

/// テクスチャ資産と `.meta` を書く。
fn テクスチャを書く(root: &Path, 相対: &str, guid: &str, 内容: &[u8]) {
    資産を書く(root, 相対, 内容);
    メタを書く(
        root,
        相対,
        guid,
        "TextureImporter",
        "  externalObjects: {}\n  textureSettings:\n    filterMode: 1\n    wrapU: 0\n    wrapV: 0\n  mipmaps:\n    enableMipMap: 1\n",
    );
}

/// スロット・材質・画像が1件ずつの最小配置を作る。
fn 最小配置を作る(シェーダ: &str) -> (tempfile::TempDir, PathBuf, String, String) {
    let (_guard, root) = 作業域を作る();
    let mat = guid(0x11);
    let tex = guid(0x22);
    fbxメタを書く(&root, &guid(0x99), &[("Body", Some(&mat))]);
    マテリアルを書く(
        &root,
        "Assets/Materials/Body.mat",
        &mat,
        "Body",
        Some(シェーダ),
        &[("_MainTex", 参照::割当(&tex)), ("_BumpMap", 参照::Null)],
        &基本浮動小数(),
        &基本色(),
    );
    テクスチャを書く(&root, "Assets/Textures/Body.png", &tex, PNG_1X1);
    (_guard, root, mat, tex)
}

#[test]
fn スロット解決と色の線形化とテクスチャと陰影値を読める() {
    let (_guard, root, _mat, _tex) = 最小配置を作る(SHADER_OPAQUE);
    let (materials, report) = VrmMaterials::load(
        &root,
        &root.join("Assets/Model.fbx"),
        &MaterialOptions::new(),
    )
    .expect("最小配置は読める");
    assert!(
        report.warnings().is_empty(),
        "警告は無い: {:?}",
        report.warnings()
    );
    let materials = materials.expect("置換表があるため材質がある");
    assert_eq!(materials.entries().len(), 1);
    assert_eq!(materials.images().len(), 1);
    let material = materials.get("Body").expect("スロットで引ける");
    assert_eq!(material.name, "Body");
    assert_eq!(material.alpha_mode, VrmAlphaMode::Opaque);
    // 白は線形化しても白のままである。
    assert_eq!(material.base_color_factor, [1.0, 1.0, 1.0, 1.0]);
    assert_eq!(material.base_color_image, Some(0));
    assert!(!material.double_sided);
    assert_eq!(material.emissive_factor, [0.0, 0.0, 0.0]);
    assert_eq!(material.emissive_image, None);
    let mtoon = material.mtoon.as_ref().expect("lilToonとして解釈できる");
    // `_ShadowColor` の線形化と `_ShadowStrength` の掛け合わせを確認する。
    // 線形(0.82)≒0.638 に 1-(1-c)*1.0 を掛けた値は約0.638である。
    assert!((mtoon.shade_color_factor[0] - 0.638).abs() < 0.005);
    assert!(!mtoon.transparent_with_z_write);
    assert_eq!(mtoon.render_queue_offset, 0);
    assert!((mtoon.shading_shift_factor - 0.0).abs() < 1e-6);
    assert!((mtoon.shading_toony_factor - 0.9).abs() < 1e-6);
    assert_eq!(mtoon.gi_equalization_factor, 0.9);
    // MatCapを使わない場合の係数は `[0, 0, 0]` である。
    assert_eq!(mtoon.matcap_factor, [0.0, 0.0, 0.0]);
    assert_eq!(mtoon.matcap_image, None);
    assert_eq!(mtoon.outline_width_mode, VrmOutlineWidthMode::None);
    assert_eq!(mtoon.outline_color_factor, [0.0, 0.0, 0.0]);
    // 画像は形式のまま埋め込み、サンプラは `.meta` の既定になる。
    let image = &materials.images()[0];
    assert_eq!(image.name, "Body");
    assert_eq!(image.mime, VrmImageMime::Png);
    assert_eq!(image.data, PNG_1X1);
    assert_eq!(
        image.sampler,
        VrmSampler {
            wrap_u: VrmWrap::Repeat,
            wrap_v: VrmWrap::Repeat,
            filter: VrmFilter::Linear,
            mipmaps: true,
        }
    );
}

#[test]
fn 付加チャンクを落として画素チャンクは不変に保つ() {
    let (_guard, root) = 作業域を作る();
    let mat = guid(0x11);
    let tex = guid(0x22);
    fbxメタを書く(&root, &guid(0x99), &[("Body", Some(&mat))]);
    // IHDRチャンク（署名8バイト＋25バイト）の直後に偽の `tEXt` チャンクを挿入する。
    // CRCは検証しないため任意の値でよい。
    let mut 拡張 = Vec::new();
    拡張.extend_from_slice(&PNG_1X1[..33]);
    拡張.extend_from_slice(&[0x00, 0x00, 0x00, 0x04]);
    拡張.extend_from_slice(b"tEXt");
    拡張.extend_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]);
    拡張.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
    拡張.extend_from_slice(&PNG_1X1[33..]);
    テクスチャを書く(&root, "Assets/Textures/Body.png", &tex, &拡張);
    マテリアルを書く(
        &root,
        "Assets/Materials/Body.mat",
        &mat,
        "Body",
        Some(SHADER_OPAQUE),
        &[("_MainTex", 参照::割当(&tex))],
        &基本浮動小数(),
        &基本色(),
    );
    let (materials, report) = VrmMaterials::load(
        &root,
        &root.join("Assets/Model.fbx"),
        &MaterialOptions::new(),
    )
    .expect("読める");
    assert!(
        report.warnings().is_empty(),
        "警告は無い: {:?}",
        report.warnings()
    );
    let materials = materials.expect("ある");
    // 付加チャンクだけが落ち、画素チャンクは元のまま残る。
    assert_eq!(materials.images()[0].data, PNG_1X1);
}

#[test]
fn 付加チャンクの無い画像はそのまま埋め込む() {
    let (_guard, root, _mat, _tex) = 最小配置を作る(SHADER_OPAQUE);
    let (materials, report) = VrmMaterials::load(
        &root,
        &root.join("Assets/Model.fbx"),
        &MaterialOptions::new(),
    )
    .expect("読める");
    assert!(
        report.warnings().is_empty(),
        "警告は無い: {:?}",
        report.warnings()
    );
    let materials = materials.expect("ある");
    assert_eq!(materials.images()[0].data, PNG_1X1);
}

#[test]
fn 壊れた画像はそのまま埋め込み警告を1件増やす() {
    let (_guard, root) = 作業域を作る();
    let mat = guid(0x11);
    let tex = guid(0x22);
    fbxメタを書く(&root, &guid(0x99), &[("Body", Some(&mat))]);
    // 署名の直後にファイル長を超える長さを置き、チャンク構造を読めなくする。
    let mut 壊れた = Vec::new();
    壊れた.extend_from_slice(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]);
    壊れた.extend_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF]);
    壊れた.extend_from_slice(b"IHDR");
    壊れた.extend_from_slice(&[0x00; 8]);
    テクスチャを書く(&root, "Assets/Textures/Body.png", &tex, &壊れた);
    マテリアルを書く(
        &root,
        "Assets/Materials/Body.mat",
        &mat,
        "Body",
        Some(SHADER_OPAQUE),
        &[("_MainTex", 参照::割当(&tex))],
        &基本浮動小数(),
        &基本色(),
    );
    let (materials, report) = VrmMaterials::load(
        &root,
        &root.join("Assets/Model.fbx"),
        &MaterialOptions::new(),
    )
    .expect("読める");
    let materials = materials.expect("ある");
    // 元のバイト列のまま入り、警告が1件増える。
    assert_eq!(materials.images()[0].data, 壊れた);
    assert_eq!(report.warnings().len(), 1, "警告: {:?}", report.warnings());
    assert!(
        report.warnings()[0].contains("チャンク構造"),
        "警告: {:?}",
        report.warnings()
    );
}

#[test]
fn シェーダ系列で透過方式が変わる() {
    for (シェーダ, 方式, 閾値あり) in [
        (SHADER_OPAQUE, VrmAlphaMode::Opaque, false),
        (SHADER_MASK, VrmAlphaMode::Mask, true),
        (SHADER_BLEND, VrmAlphaMode::Blend, false),
    ] {
        let (_guard, root, _mat, _tex) = 最小配置を作る(シェーダ);
        let (materials, _) = VrmMaterials::load(
            &root,
            &root.join("Assets/Model.fbx"),
            &MaterialOptions::new(),
        )
        .expect("読める");
        let material = materials.expect("ある").get("Body").expect("ある").clone();
        assert_eq!(material.alpha_mode, 方式, "シェーダ: {シェーダ}");
        if 閾値あり {
            assert!((material.alpha_cutoff - 0.5).abs() < f32::EPSILON);
        }
        let mtoon = material.mtoon.expect("ある");
        // 半透明かつ `_ZWrite` が真の場合だけZ書き込みする。
        assert_eq!(
            mtoon.transparent_with_z_write,
            matches!(方式, VrmAlphaMode::Blend)
        );
    }
}

#[test]
fn 半透明でz書き込み無効なら透過z書き込みしない() {
    let (_guard, root, mat, tex) = 最小配置を作る(SHADER_BLEND);
    let mut 浮動 = 基本浮動小数();
    for (名, 値) in &mut 浮動 {
        if *名 == "_ZWrite" {
            *値 = 0.0;
        }
    }
    マテリアルを書く(
        &root,
        "Assets/Materials/Body.mat",
        &mat,
        "Body",
        Some(SHADER_BLEND),
        &[("_MainTex", 参照::割当(&tex))],
        &浮動,
        &基本色(),
    );
    let (materials, _) = VrmMaterials::load(
        &root,
        &root.join("Assets/Model.fbx"),
        &MaterialOptions::new(),
    )
    .expect("読める");
    let material = materials.expect("ある").get("Body").expect("ある").clone();
    assert!(!material.mtoon.expect("ある").transparent_with_z_write);
}

#[test]
fn カリングで両面が変わり前面カリングは警告する() {
    for (カリング, 両面, 警告あり) in [(0.0, true, false), (1.0, true, true), (2.0, false, false)]
    {
        let (_guard, root, mat, tex) = 最小配置を作る(SHADER_OPAQUE);
        let mut 浮動 = 基本浮動小数();
        for (名, 値) in &mut 浮動 {
            if *名 == "_Cull" {
                *値 = カリング;
            }
        }
        マテリアルを書く(
            &root,
            "Assets/Materials/Body.mat",
            &mat,
            "Body",
            Some(SHADER_OPAQUE),
            &[("_MainTex", 参照::割当(&tex))],
            &浮動,
            &基本色(),
        );
        let (materials, report) = VrmMaterials::load(
            &root,
            &root.join("Assets/Model.fbx"),
            &MaterialOptions::new(),
        )
        .expect("読める");
        let material = materials.expect("ある").get("Body").expect("ある").clone();
        assert_eq!(material.double_sided, 両面, "カリング: {カリング}");
        assert_eq!(
            report
                .warnings()
                .iter()
                .any(|警告| 警告.contains("前面カリング")),
            警告あり,
            "警告: {:?}",
            report.warnings()
        );
    }
}

#[test]
fn 未対応シェーダは灰色材質に落ち警告する() {
    let (_guard, root, _mat, _tex) = 最小配置を作る(SHADER_OPAQUE);
    // 索引にないシェーダGUIDに差し替え、版数も持たせない。
    マテリアルを書く(
        &root,
        "Assets/Materials/Body.mat",
        &guid(0x11),
        "Body",
        Some("ffffffffffffffffffffffffffffffff"),
        &[],
        &[("_Cutoff", 0.5)],
        &[("_Color", [1.0, 0.0, 0.0, 1.0])],
    );
    let (materials, report) = VrmMaterials::load(
        &root,
        &root.join("Assets/Model.fbx"),
        &MaterialOptions::new(),
    )
    .expect("読める");
    let material = materials.expect("ある").get("Body").expect("ある").clone();
    assert_eq!(material.name, "Body");
    assert_eq!(material.base_color_factor, [0.8, 0.8, 0.8, 1.0]);
    assert!(material.mtoon.is_none(), "灰色PBRへ落ちる");
    assert!(
        report
            .warnings()
            .iter()
            .any(|警告| 警告.contains("未対応シェーダ")),
        "警告: {:?}",
        report.warnings()
    );
}

#[test]
fn 未知シェーダでも版数があれば描画状態から推定する() {
    let (_guard, root, _mat, _tex) = 最小配置を作る(SHADER_OPAQUE);
    マテリアルを書く(
        &root,
        "Assets/Materials/Body.mat",
        &guid(0x11),
        "Body",
        Some("ffffffffffffffffffffffffffffffff"),
        &[],
        &[("_lilToonVersion", 45.0), ("_DstBlend", 2.0)],
        &[("_Color", [1.0, 1.0, 1.0, 1.0])],
    );
    let (materials, report) = VrmMaterials::load(
        &root,
        &root.join("Assets/Model.fbx"),
        &MaterialOptions::new(),
    )
    .expect("読める");
    let material = materials.expect("ある").get("Body").expect("ある").clone();
    assert_eq!(material.alpha_mode, VrmAlphaMode::Blend);
    assert!(material.mtoon.is_some());
    assert!(
        report
            .warnings()
            .iter()
            .any(|警告| 警告.contains("カットアウト")),
        "警告: {:?}",
        report.warnings()
    );
}

#[test]
fn 存在しないテクスチャと未対応形式は省いて警告する() {
    let (_guard, root, mat, _tex) = 最小配置を作る(SHADER_OPAQUE);
    // 索引にないGUIDとTGA形式の2件を割り当てる。
    テクスチャを書く(&root, "Assets/Textures/Body.tga", &guid(0x33), b"TGA-fake");
    マテリアルを書く(
        &root,
        "Assets/Materials/Body.mat",
        &mat,
        "Body",
        Some(SHADER_OPAQUE),
        &[
            ("_MainTex", 参照::割当(&guid(0xdead))),
            ("_EmissionMap", 参照::割当(&guid(0x33))),
        ],
        &基本浮動小数(),
        &基本色(),
    );
    // `_UseEmission` を有効にして発光テクスチャの解決を確かめる。
    let mut 浮動 = 基本浮動小数();
    for (名, 値) in &mut 浮動 {
        if *名 == "_UseEmission" {
            *値 = 1.0;
        }
    }
    マテリアルを書く(
        &root,
        "Assets/Materials/Body.mat",
        &mat,
        "Body",
        Some(SHADER_OPAQUE),
        &[
            ("_MainTex", 参照::割当(&guid(0xdead))),
            ("_EmissionMap", 参照::割当(&guid(0x33))),
        ],
        &浮動,
        &基本色(),
    );
    let (materials, report) = VrmMaterials::load(
        &root,
        &root.join("Assets/Model.fbx"),
        &MaterialOptions::new(),
    )
    .expect("読める");
    let material = materials.expect("ある").get("Body").expect("ある").clone();
    assert_eq!(material.base_color_image, None);
    assert_eq!(material.emissive_image, None);
    assert!(material.mtoon.is_some(), "材質自体は続ける");
    assert!(
        report
            .warnings()
            .iter()
            .any(|警告| 警告.contains("索引にない")),
        "警告: {:?}",
        report.warnings()
    );
    assert!(
        report
            .warnings()
            .iter()
            .any(|警告| 警告.contains("未対応の画像形式")),
        "警告: {:?}",
        report.warnings()
    );
}

#[test]
fn サンプラ設定とスケールの警告を読める() {
    let (_guard, root) = 作業域を作る();
    let mat = guid(0x11);
    let tex = guid(0x22);
    fbxメタを書く(&root, &guid(0x99), &[("Body", Some(&mat))]);
    // 三線形・端止め・鏡像・mipmap無しの設定にする。
    資産を書く(&root, "Assets/Textures/Body.png", PNG_1X1);
    メタを書く(
        &root,
        "Assets/Textures/Body.png",
        &tex,
        "TextureImporter",
        "  externalObjects: {}\n  textureSettings:\n    filterMode: 2\n    wrapU: 1\n    wrapV: 2\n  mipmaps:\n    enableMipMap: false\n",
    );
    マテリアルを書く(
        &root,
        "Assets/Materials/Body.mat",
        &mat,
        "Body",
        Some(SHADER_OPAQUE),
        &[("_MainTex", 参照::割当(&tex))],
        &基本浮動小数(),
        &基本色(),
    );
    let (materials, report) = VrmMaterials::load(
        &root,
        &root.join("Assets/Model.fbx"),
        &MaterialOptions::new(),
    )
    .expect("読める");
    assert!(
        report.warnings().is_empty(),
        "警告は無い: {:?}",
        report.warnings()
    );
    let 所有 = materials.expect("ある");
    let image = &所有.images()[0];
    assert_eq!(
        image.sampler,
        VrmSampler {
            wrap_u: VrmWrap::ClampToEdge,
            wrap_v: VrmWrap::MirroredRepeat,
            filter: VrmFilter::Trilinear,
            mipmaps: false,
        }
    );
}

#[test]
fn 材質数と画像サイズの上限を超えると失敗する() {
    let (_guard, root, _mat, _tex) = 最小配置を作る(SHADER_OPAQUE);
    fbxメタを書く(
        &root,
        &guid(0x99),
        &[("Body", Some(&guid(0x11))), ("Face", Some(&guid(0x11)))],
    );
    let 失敗 = VrmMaterials::load(
        &root,
        &root.join("Assets/Model.fbx"),
        &MaterialOptions::new().with_max_materials(1),
    )
    .expect_err("材質数超過は失敗する");
    assert_eq!(失敗.kind(), vrctom::vrm::MaterialErrorKind::LimitExceeded);

    let 失敗 = VrmMaterials::load(
        &root,
        &root.join("Assets/Model.fbx"),
        &MaterialOptions::new().with_max_image_bytes(10),
    )
    .expect_err("画像サイズ超過は失敗する");
    assert_eq!(失敗.kind(), vrctom::vrm::MaterialErrorKind::LimitExceeded);

    let 失敗 = VrmMaterials::load(
        &root,
        &root.join("Assets/Model.fbx"),
        &MaterialOptions::new().with_max_total_image_bytes(10),
    )
    .expect_err("画像合計超過は失敗する");
    assert_eq!(失敗.kind(), vrctom::vrm::MaterialErrorKind::LimitExceeded);
}

#[test]
fn 置換表が無い場合は材質なしになる() {
    let (_guard, root) = 作業域を作る();
    資産を書く(&root, "Assets/Model.fbx", b"");
    メタを書く(
        &root,
        "Assets/Model.fbx",
        &guid(0x99),
        "ModelImporter",
        "  externalObjects: {}\n",
    );
    let (materials, report) = VrmMaterials::load(
        &root,
        &root.join("Assets/Model.fbx"),
        &MaterialOptions::new(),
    )
    .expect("読める");
    assert!(materials.is_none());
    assert!(!report.warnings().is_empty());
}

#[test]
fn 重複スロットは最初を残しnull置換は灰色にする() {
    let (_guard, root) = 作業域を作る();
    let first = guid(0x11);
    let second = guid(0x12);
    fbxメタを書く(
        &root,
        &guid(0x99),
        &[
            ("Body", Some(&first)),
            ("Body", Some(&second)),
            ("Face", None),
        ],
    );
    マテリアルを書く(
        &root,
        "Assets/Materials/First.mat",
        &first,
        "First",
        Some(SHADER_OPAQUE),
        &[],
        &基本浮動小数(),
        &基本色(),
    );
    マテリアルを書く(
        &root,
        "Assets/Materials/Second.mat",
        &second,
        "Second",
        Some(SHADER_OPAQUE),
        &[],
        &基本浮動小数(),
        &基本色(),
    );
    let (materials, report) = VrmMaterials::load(
        &root,
        &root.join("Assets/Model.fbx"),
        &MaterialOptions::new(),
    )
    .expect("読める");
    let materials = materials.expect("ある");
    assert_eq!(materials.entries().len(), 2);
    assert_eq!(materials.get("Body").expect("ある").name, "First");
    let face = materials.get("Face").expect("ある");
    assert!(face.mtoon.is_none(), "null置換は灰色材質である");
    assert!(
        report.warnings().iter().any(|警告| 警告.contains("重複")),
        "警告: {:?}",
        report.warnings()
    );
}

#[test]
fn 未対応機能の使用は警告する() {
    let (_guard, root, mat, tex) = 最小配置を作る(SHADER_OPAQUE);
    let mut 浮動 = 基本浮動小数();
    浮動.push(("_UseBacklight", 1.0));
    浮動.push(("_UseRim", 1.0));
    浮動.push(("_UseBumpMap", 1.0));
    浮動.push(("_AlphaMaskMode", 2.0));
    浮動.push(("_MainGradationStrength", 0.5));
    浮動.push(("_UseMain2ndTex", 1.0));
    let mut 色 = 基本色();
    色.push(("_MainTexHSVG", [0.1, 1.0, 1.0, 1.0]));
    マテリアルを書く(
        &root,
        "Assets/Materials/Body.mat",
        &mat,
        "Body",
        Some(SHADER_OPAQUE),
        &[
            ("_MainTex", 参照::割当(&tex)),
            ("_AlphaMask", 参照::割当(&tex)),
            ("_Main2ndTex", 参照::割当(&tex)),
        ],
        &浮動,
        &色,
    );
    let (_materials, report) = VrmMaterials::load(
        &root,
        &root.join("Assets/Model.fbx"),
        &MaterialOptions::new(),
    )
    .expect("読める");
    for 期待 in [
        "バックライト",
        "リムライト",
        "色相",
        "アルファマスク",
        "グラデーション",
        "2ndテクスチャ",
    ] {
        assert!(
            report.warnings().iter().any(|警告| 警告.contains(期待)),
            "{期待}の警告が無い: {:?}",
            report.warnings()
        );
    }
}

#[test]
fn 発光とマットキャップとアウトラインを読める() {
    // アウトラインは描画路を持つ変種（Outline系）でのみ有効になる。
    let (_guard, root, mat, tex) = 最小配置を作る(SHADER_OUTLINE_OPAQUE);
    let mut 浮動 = 基本浮動小数();
    for (名, 値) in &mut 浮動 {
        match *名 {
            "_UseMatCap" => *値 = 1.0,
            "_UseEmission" => *値 = 1.0,
            "_UseOutline" => *値 = 1.0,
            _ => {}
        }
    }
    let mut 色 = 基本色();
    for (名, 値) in &mut 色 {
        match *名 {
            "_EmissionColor" => *値 = [2.0, 0.5, 0.0, 1.0],
            "_MatCapColor" => *値 = [1.0, 0.0, 0.0, 1.0],
            "_OutlineColor" => *値 = [0.0, 1.0, 0.0, 1.0],
            _ => {}
        }
    }
    マテリアルを書く(
        &root,
        "Assets/Materials/Body.mat",
        &mat,
        "Body",
        Some(SHADER_OUTLINE_OPAQUE),
        &[
            ("_MainTex", 参照::割当(&tex)),
            ("_EmissionMap", 参照::割当(&tex)),
            ("_MatCapTex", 参照::割当(&tex)),
            ("_OutlineWidthMask", 参照::割当(&tex)),
            ("_ShadowColorTex", 参照::割当(&tex)),
        ],
        &浮動,
        &色,
    );
    let (materials, report) = VrmMaterials::load(
        &root,
        &root.join("Assets/Model.fbx"),
        &MaterialOptions::new(),
    )
    .expect("読める");
    let material = materials.expect("ある").get("Body").expect("ある").clone();
    // 発光色は線形化後に1へ丸め、警告する。線形(0.5)≒0.214である。
    assert_eq!(material.emissive_factor[0], 1.0);
    assert!((material.emissive_factor[1] - 0.214).abs() < 0.005);
    assert_eq!(material.emissive_factor[2], 0.0);
    assert_eq!(material.emissive_image, Some(0));
    assert!(
        report.warnings().iter().any(|警告| 警告.contains("発光色")),
        "警告: {:?}",
        report.warnings()
    );
    let mtoon = material.mtoon.expect("ある");
    // 同じGUIDの画像は共有する。幅マスクだけはGへ載せ替えて焼くため別画像になる。
    assert_eq!(mtoon.matcap_image, Some(0));
    assert_eq!(mtoon.shade_multiply_image, Some(0));
    assert_eq!(mtoon.outline_width_multiply_image, Some(1));
    assert_eq!(mtoon.matcap_factor, [1.0, 0.0, 0.0]);
    assert_eq!(
        mtoon.outline_width_mode,
        VrmOutlineWidthMode::WorldCoordinates
    );
    // アウトライン幅は `_OutlineWidth × 0.01`（m）で書く。
    assert!((mtoon.outline_width_factor - 0.0005).abs() < 1e-7);
    assert!(
        report
            .warnings()
            .iter()
            .any(|警告| 警告.contains("乗算として近似")),
        "警告: {:?}",
        report.warnings()
    );
}

#[test]
fn 影を使わない場合は陰影が基準色になる() {
    let (_guard, root, mat, tex) = 最小配置を作る(SHADER_OPAQUE);
    let mut 浮動 = 基本浮動小数();
    for (名, 値) in &mut 浮動 {
        if *名 == "_UseShadow" {
            *値 = 0.0;
        }
    }
    マテリアルを書く(
        &root,
        "Assets/Materials/Body.mat",
        &mat,
        "Body",
        Some(SHADER_OPAQUE),
        &[("_MainTex", 参照::割当(&tex))],
        &浮動,
        &基本色(),
    );
    let (materials, _) = VrmMaterials::load(
        &root,
        &root.join("Assets/Model.fbx"),
        &MaterialOptions::new(),
    )
    .expect("読める");
    let mtoon = materials
        .expect("ある")
        .get("Body")
        .expect("ある")
        .mtoon
        .clone()
        .expect("ある");
    assert_eq!(mtoon.shade_color_factor, [1.0, 1.0, 1.0]);
    assert!((mtoon.shading_shift_factor - 0.0).abs() < f32::EPSILON);
    assert!((mtoon.shading_toony_factor - 0.9).abs() < f32::EPSILON);
    assert_eq!(mtoon.shade_multiply_image, Some(0));
}

/// `Assets` 外のmeta無しファイルがあっても読み取れる。
#[test]
fn assets外のmeta無し配置があっても読み取れる() {
    let (_guard, root, _mat, _tex) = 最小配置を作る(SHADER_OPAQUE);
    資産を書く(
        &root,
        "ProjectSettings/ProjectVersion.txt",
        b"m_EditorVersion: 1",
    );
    let (materials, report) = VrmMaterials::load(
        &root,
        &root.join("Assets/Model.fbx"),
        &MaterialOptions::new(),
    )
    .expect("Assets外の配置は巻き込まない");
    assert!(materials.expect("ある").get("Body").is_some());
    assert!(
        report.warnings().is_empty(),
        "警告は無い: {:?}",
        report.warnings()
    );
}

/// lilToonを含まないシェーダ名は未対応として灰色材質にする。
#[test]
fn 他社transparentシェーダはliltoonとして扱わない() {
    let (_guard, root) = 作業域を作る();
    let mat = guid(0x11);
    let shader = guid(0x44);
    fbxメタを書く(&root, &guid(0x99), &[("Body", Some(&mat))]);
    // 他社シェーダ名は `Transparent` を含むが `lilToon` を含まない。
    資産を書く(
        &root,
        "Assets/Shaders/Other.shader",
        b"Shader \"OtherVendor/Transparent\"\n{\n}\n",
    );
    メタを書く(
        &root,
        "Assets/Shaders/Other.shader",
        &shader,
        "ShaderImporter",
        "  externalObjects: {}\n",
    );
    マテリアルを書く(
        &root,
        "Assets/Materials/Body.mat",
        &mat,
        "Body",
        Some(&shader),
        &[],
        &[("_Cutoff", 0.5)],
        &[("_Color", [1.0, 1.0, 1.0, 1.0])],
    );
    let (materials, report) = VrmMaterials::load(
        &root,
        &root.join("Assets/Model.fbx"),
        &MaterialOptions::new(),
    )
    .expect("読める");
    let material = materials.expect("ある").get("Body").expect("ある").clone();
    assert!(material.mtoon.is_none(), "灰色PBRへ落ちる");
    assert!(
        report
            .warnings()
            .iter()
            .any(|警告| 警告.contains("未対応シェーダ")),
        "警告: {:?}",
        report.warnings()
    );
}

/// 影色テクスチャが無い場合は影色と基準色の積になり基準色を落とさない。
#[test]
fn 影色テクスチャが無い場合は影色と基準色の積になる() {
    let (_guard, root, mat, tex) = 最小配置を作る(SHADER_OPAQUE);
    // 基準色を赤にして積の影響を見えるようにする。
    let mut 色 = 基本色();
    for (名, 値) in &mut 色 {
        if *名 == "_Color" {
            *値 = [1.0, 0.0, 0.0, 1.0];
        }
    }
    マテリアルを書く(
        &root,
        "Assets/Materials/Body.mat",
        &mat,
        "Body",
        Some(SHADER_OPAQUE),
        &[("_MainTex", 参照::割当(&tex))],
        &基本浮動小数(),
        &色,
    );
    let (materials, report) = VrmMaterials::load(
        &root,
        &root.join("Assets/Model.fbx"),
        &MaterialOptions::new(),
    )
    .expect("読める");
    let material = materials.expect("ある").get("Body").expect("ある").clone();
    let mtoon = material.mtoon.expect("ある");
    // 基準色テクスチャを影側にも使い、影色は基準色との積になる。
    assert_eq!(mtoon.shade_multiply_image, Some(0));
    // `_ShadowColor` の既定値から作る色と赤の積である。G/Bは0になる。
    assert!(
        mtoon.shade_color_factor[0] > 0.5,
        "{:?}",
        mtoon.shade_color_factor
    );
    assert_eq!(mtoon.shade_color_factor[1], 0.0);
    assert_eq!(mtoon.shade_color_factor[2], 0.0);
    assert!(
        !report
            .warnings()
            .iter()
            .any(|警告| 警告.contains("乗算として近似")),
        "影色テクスチャが無いため乗算の警告は無い: {:?}",
        report.warnings()
    );
}

/// 基準色テクスチャが無い場合の影側はテクスチャ無しになる。
#[test]
fn 基準色が無い場合の影側はテクスチャ無しになる() {
    let (_guard, root, mat, _tex) = 最小配置を作る(SHADER_OPAQUE);
    let mut 浮動 = 基本浮動小数();
    for (名, 値) in &mut 浮動 {
        if *名 == "_UseShadow" {
            *値 = 0.0;
        }
    }
    マテリアルを書く(
        &root,
        "Assets/Materials/Body.mat",
        &mat,
        "Body",
        Some(SHADER_OPAQUE),
        &[],
        &浮動,
        &基本色(),
    );
    let (materials, _) = VrmMaterials::load(
        &root,
        &root.join("Assets/Model.fbx"),
        &MaterialOptions::new(),
    )
    .expect("読める");
    let mtoon = materials
        .expect("ある")
        .get("Body")
        .expect("ある")
        .mtoon
        .clone()
        .expect("ある");
    assert_eq!(mtoon.shade_multiply_image, None);
    assert_eq!(mtoon.shade_color_factor, [1.0, 1.0, 1.0]);
}

/// 非有限・範囲外の材質値は灰色材質に落ちる。
#[test]
fn 不正な材質値は灰色材質に落ちる() {
    // `_Color` のアルファに非数を入れる。
    let (_guard, root) = 作業域を作る();
    let mat = guid(0x11);
    fbxメタを書く(&root, &guid(0x99), &[("Body", Some(&mat))]);
    マテリアルを書く(
        &root,
        "Assets/Materials/Body.mat",
        &mat,
        "Body",
        Some(SHADER_OPAQUE),
        &[],
        &基本浮動小数(),
        &[("_Color", [1.0, 1.0, 1.0, f64::NAN])],
    );
    // Rustの `Display` は `NaN` と書くが、Unity YAMLの非数は `.nan` である。
    // そのままでは文字列扱いで材質文書が読めないため、置き換える。
    let mat_path = root.join("Assets/Materials/Body.mat");
    let 本文 = fs::read_to_string(&mat_path).expect("matは読める");
    let 本文 = 本文.replace("NaN", ".nan");
    fs::write(&mat_path, 本文.as_bytes()).expect("matは書ける");
    let (materials, report) = VrmMaterials::load(
        &root,
        &root.join("Assets/Model.fbx"),
        &MaterialOptions::new(),
    )
    .expect("読める");
    let material = materials.expect("ある").get("Body").expect("ある").clone();
    assert!(material.mtoon.is_none(), "灰色PBRへ落ちる");
    // 非有限は写像前の検査で落とすため、写像後の文面ではなく先行検査の文面になる。
    assert!(
        report
            .warnings()
            .iter()
            .any(|警告| 警告.contains("非有限値")),
        "警告: {:?}",
        report.warnings()
    );

    // アウトライン幅の範囲外でも同様に落ちる。
    // Outline系の変種でのみ幅を読むため、アウトライン描画路で確かめる。
    let (_guard, root, mat, _tex) = 最小配置を作る(SHADER_OUTLINE_OPAQUE);
    let mut 浮動 = 基本浮動小数();
    for (名, 値) in &mut 浮動 {
        if *名 == "_UseOutline" {
            *値 = 1.0;
        }
    }
    浮動.push(("_OutlineWidth", 1e300));
    マテリアルを書く(
        &root,
        "Assets/Materials/Body.mat",
        &mat,
        "Body",
        Some(SHADER_OUTLINE_OPAQUE),
        &[],
        &浮動,
        &基本色(),
    );
    // `Display` は巨大値を十進の桁列で書くため、指数表記の `1e300` に置き換える。
    let mat_path = root.join("Assets/Materials/Body.mat");
    let 本文 = fs::read_to_string(&mat_path).expect("matは読める");
    let mut 置換 = String::new();
    for 行 in 本文.lines() {
        if 行.contains("_OutlineWidth") {
            置換.push_str("    - _OutlineWidth: 1e300\n");
        } else {
            置換.push_str(行);
            置換.push('\n');
        }
    }
    fs::write(&mat_path, 置換.as_bytes()).expect("matは書ける");
    let (materials, report) = VrmMaterials::load(
        &root,
        &root.join("Assets/Model.fbx"),
        &MaterialOptions::new(),
    )
    .expect("読める");
    let material = materials.expect("ある").get("Body").expect("ある").clone();
    assert!(material.mtoon.is_none(), "灰色PBRへ落ちる");
    assert!(
        report
            .warnings()
            .iter()
            .any(|警告| 警告.contains("outline_width_factor")),
        "警告: {:?}",
        report.warnings()
    );
}

/// 色の無限値は写像前の検査で灰色材質に落ちる。
#[test]
fn 色の無限値は灰色材質に落ちる() {
    // 写像時の `clamp01` が無限を1に潰す前に検出し、灰色材質へ落とす。
    let (_guard, root) = 作業域を作る();
    let mat = guid(0x11);
    fbxメタを書く(&root, &guid(0x99), &[("Body", Some(&mat))]);
    マテリアルを書く(
        &root,
        "Assets/Materials/Body.mat",
        &mat,
        "Body",
        Some(SHADER_OPAQUE),
        &[],
        &基本浮動小数(),
        &[("_Color", [f64::INFINITY, 1.0, 1.0, 1.0])],
    );
    // Rustの `Display` は `inf` と書くが、Unity YAMLの無限は `.inf` である。
    // そのままでは文字列扱いで材質文書が読めないため、置き換える。
    let mat_path = root.join("Assets/Materials/Body.mat");
    let 本文 = fs::read_to_string(&mat_path).expect("matは読める");
    let 本文 = 本文.replace(": inf", ": .inf");
    fs::write(&mat_path, 本文.as_bytes()).expect("matは書ける");
    let (materials, report) = VrmMaterials::load(
        &root,
        &root.join("Assets/Model.fbx"),
        &MaterialOptions::new(),
    )
    .expect("読める");
    let material = materials.expect("ある").get("Body").expect("ある").clone();
    assert_eq!(material.base_color_factor, [0.8, 0.8, 0.8, 1.0]);
    assert!(material.mtoon.is_none(), "灰色PBRへ落ちる");
    assert!(
        report
            .warnings()
            .iter()
            .any(|警告| 警告.contains("非有限値")),
        "警告: {:?}",
        report.warnings()
    );
}

/// 影強度の無限値は写像前の検査で灰色材質に落ちる。
#[test]
fn 影強度の無限値は灰色材質に落ちる() {
    // 写像時の `clamp01` が無限を1に潰す前に検出し、灰色材質へ落とす。
    let (_guard, root) = 作業域を作る();
    let mat = guid(0x11);
    fbxメタを書く(&root, &guid(0x99), &[("Body", Some(&mat))]);
    let mut 浮動 = 基本浮動小数();
    for (名, 値) in &mut 浮動 {
        if *名 == "_ShadowStrength" {
            *値 = f64::INFINITY;
        }
    }
    マテリアルを書く(
        &root,
        "Assets/Materials/Body.mat",
        &mat,
        "Body",
        Some(SHADER_OPAQUE),
        &[],
        &浮動,
        &基本色(),
    );
    let mat_path = root.join("Assets/Materials/Body.mat");
    let 本文 = fs::read_to_string(&mat_path).expect("matは読める");
    let 本文 = 本文.replace(": inf", ": .inf");
    fs::write(&mat_path, 本文.as_bytes()).expect("matは書ける");
    let (materials, report) = VrmMaterials::load(
        &root,
        &root.join("Assets/Model.fbx"),
        &MaterialOptions::new(),
    )
    .expect("読める");
    let material = materials.expect("ある").get("Body").expect("ある").clone();
    assert!(material.mtoon.is_none(), "灰色PBRへ落ちる");
    assert!(
        report
            .warnings()
            .iter()
            .any(|警告| 警告.contains("非有限値")),
        "警告: {:?}",
        report.warnings()
    );
}

/// アウトライン幅の非数は写像前の検査で灰色材質に落ちる。
#[test]
fn アウトライン幅の非数は灰色材質に落ちる() {
    // 写像時の `.max(0.0)` が非数をそのまま通す前に検出し、灰色材質へ落とす。
    let (_guard, root) = 作業域を作る();
    let mat = guid(0x11);
    fbxメタを書く(&root, &guid(0x99), &[("Body", Some(&mat))]);
    let mut 浮動 = 基本浮動小数();
    for (名, 値) in &mut 浮動 {
        if *名 == "_UseOutline" {
            *値 = 1.0;
        }
        if *名 == "_OutlineWidth" {
            *値 = f64::NAN;
        }
    }
    マテリアルを書く(
        &root,
        "Assets/Materials/Body.mat",
        &mat,
        "Body",
        Some(SHADER_OPAQUE),
        &[],
        &浮動,
        &基本色(),
    );
    let mat_path = root.join("Assets/Materials/Body.mat");
    let 本文 = fs::read_to_string(&mat_path).expect("matは読める");
    let 本文 = 本文.replace("NaN", ".nan");
    fs::write(&mat_path, 本文.as_bytes()).expect("matは書ける");
    let (materials, report) = VrmMaterials::load(
        &root,
        &root.join("Assets/Model.fbx"),
        &MaterialOptions::new(),
    )
    .expect("読める");
    let material = materials.expect("ある").get("Body").expect("ある").clone();
    assert!(material.mtoon.is_none(), "灰色PBRへ落ちる");
    assert!(
        report
            .warnings()
            .iter()
            .any(|警告| 警告.contains("非有限値")),
        "警告: {:?}",
        report.warnings()
    );
}

/// UVスクロール・回転の設定は写像せず警告する。
#[test]
fn uvスクロールの設定は警告する() {
    let (_guard, root, mat, tex) = 最小配置を作る(SHADER_OPAQUE);
    let mut 色 = 基本色();
    色.push(("_MainTex_ScrollRotate", [0.1, 0.0, 0.0, 0.0]));
    マテリアルを書く(
        &root,
        "Assets/Materials/Body.mat",
        &mat,
        "Body",
        Some(SHADER_OPAQUE),
        &[("_MainTex", 参照::割当(&tex))],
        &基本浮動小数(),
        &色,
    );
    let (_, report) = VrmMaterials::load(
        &root,
        &root.join("Assets/Model.fbx"),
        &MaterialOptions::new(),
    )
    .expect("読める");
    assert!(
        report
            .warnings()
            .iter()
            .any(|警告| 警告.contains("UVスクロール")),
        "警告: {:?}",
        report.warnings()
    );
}

/// 実fixtureの全スロットを材質に解決する。
///
/// 固定件数には依存せず、置換表の全スロットに材質があり、画像が1件以上あることを
/// 確かめる。警告は標準エラーに出す。
#[test]
fn 実fixtureの全スロットを材質に解決する() {
    let Ok(パス) = std::env::var("VRCTOM_FBX_FIXTURE") else {
        eprintln!("VRCTOM_FBX_FIXTURE 未設定のため実fixture観測を跳ばす");
        return;
    };
    let fbx = PathBuf::from(パス);
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
        eprintln!("実fixture警告: {警告}");
    }
    let materials = materials.expect("置換表があるため材質がある");
    assert!(!materials.entries().is_empty(), "材質は1件以上ある");
    assert!(!materials.images().is_empty(), "画像は1件以上ある");
    // FBXの全スロットに材質が対応する。
    let バイト列 = fs::read(&fbx).expect("実fixtureは読める");
    let 読み取り = FbxFile::parse(&バイト列, &FbxOptions::new()).expect("実fixtureは読める");
    let (モデル, _) = vrctom::vrm::VrmModel::from_fbx(&読み取り, &vrctom::vrm::VrmOptions::new())
        .expect("実fixtureは変換できる");
    let mut 欠落 = Vec::new();
    for メッシュ in &モデル.meshes {
        for スロット in &メッシュ.material_slots {
            if materials.get(スロット).is_none() {
                欠落.push(スロット.clone());
            }
        }
    }
    assert!(欠落.is_empty(), "対応の無いスロットがある: {欠落:?}");
    eprintln!(
        "実fixture材質: {}件 画像{}件 警告{}件",
        materials.entries().len(),
        materials.images().len(),
        report.warnings().len()
    );
}

/// Outline系の変種は `_UseOutline` に関係なく描画し、幅は百分の1（m）になる。
#[test]
fn outline変種はトグルに関係なく有効で幅は百分の1になる() {
    let (_guard, root, mat, tex) = 最小配置を作る(SHADER_OUTLINE_OPAQUE);
    // `_UseOutline` は既定の0のままにし、幅だけ3cmに変える。
    let mut 浮動 = 基本浮動小数();
    for (名, 値) in &mut 浮動 {
        if *名 == "_OutlineWidth" {
            *値 = 0.03;
        }
    }
    マテリアルを書く(
        &root,
        "Assets/Materials/Body.mat",
        &mat,
        "Body",
        Some(SHADER_OUTLINE_OPAQUE),
        &[("_MainTex", 参照::割当(&tex))],
        &浮動,
        &基本色(),
    );
    let (materials, _) = VrmMaterials::load(
        &root,
        &root.join("Assets/Model.fbx"),
        &MaterialOptions::new(),
    )
    .expect("読める");
    let mtoon = materials
        .expect("ある")
        .get("Body")
        .expect("ある")
        .mtoon
        .clone()
        .expect("ある");
    assert_eq!(
        mtoon.outline_width_mode,
        VrmOutlineWidthMode::WorldCoordinates
    );
    assert!(
        (mtoon.outline_width_factor - 0.0003).abs() < 1e-7,
        "幅は0.03×0.01である: {}",
        mtoon.outline_width_factor
    );
}

/// Multi系は `_TransparentMode` で系列が決まり、アウトラインは `_UseOutline` で切り替わる。
#[test]
fn multiは描画方式で系列が決まりトグルでアウトラインが切り替わる() {
    let 判定 = |トグル: f64| {
        let (_guard, root) = 作業域を作る();
        let mat = guid(0x11);
        let shader = guid(0x44);
        fbxメタを書く(&root, &guid(0x99), &[("Body", Some(&mat))]);
        資産を書く(
            &root,
            "Assets/Shaders/Multi.shader",
            b"Shader \"_lil/lilToonMulti\"\n{\n}\n",
        );
        メタを書く(
            &root,
            "Assets/Shaders/Multi.shader",
            &shader,
            "ShaderImporter",
            "  externalObjects: {}\n",
        );
        let mut 浮動 = 基本浮動小数();
        浮動.push(("_TransparentMode", 2.0));
        // `_UseOutline` は既定列にあるため値を変える（同名は最初の1件が勝つ）。
        for (名, 値) in &mut 浮動 {
            if *名 == "_UseOutline" {
                *値 = トグル;
            }
        }
        マテリアルを書く(
            &root,
            "Assets/Materials/Body.mat",
            &mat,
            "Body",
            Some(&shader),
            &[],
            &浮動,
            &基本色(),
        );
        let (materials, _) = VrmMaterials::load(
            &root,
            &root.join("Assets/Model.fbx"),
            &MaterialOptions::new(),
        )
        .expect("読める");
        materials
            .expect("ある")
            .get("Body")
            .expect("ある")
            .mtoon
            .clone()
            .expect("ある")
    };
    // `_UseOutline` が0の場合は描画しない。
    let 無し = 判定(0.0);
    assert_eq!(無し.outline_width_mode, VrmOutlineWidthMode::None);
    assert_eq!(無し.outline_width_factor, 0.0);
    // `_UseOutline` が真の場合は描画し、幅は百分の1になる。
    let 有り = 判定(1.0);
    assert_eq!(
        有り.outline_width_mode,
        VrmOutlineWidthMode::WorldCoordinates
    );
    assert!(
        (有り.outline_width_factor - 0.0005).abs() < 1e-7,
        "幅は0.05×0.01である: {}",
        有り.outline_width_factor
    );
}

/// MatCapの重みは `_MatCapColor.rgb × (_MatCapBlend × saturate(a))` になる。
#[test]
fn matcap重みは混合率とアルファを掛ける() {
    let (_guard, root, mat, tex) = 最小配置を作る(SHADER_OPAQUE);
    let mut 浮動 = 基本浮動小数();
    for (名, 値) in &mut 浮動 {
        if *名 == "_UseMatCap" {
            *値 = 1.0;
        }
    }
    // `_MatCapBlend` は既定列に無いため追加する。
    浮動.push(("_MatCapBlend", 0.5));
    let mut 色 = 基本色();
    for (名, 値) in &mut 色 {
        if *名 == "_MatCapColor" {
            *値 = [0.5, 0.25, 0.125, 0.5];
        }
    }
    マテリアルを書く(
        &root,
        "Assets/Materials/Body.mat",
        &mat,
        "Body",
        Some(SHADER_OPAQUE),
        &[
            ("_MainTex", 参照::割当(&tex)),
            ("_MatCapTex", 参照::割当(&tex)),
        ],
        &浮動,
        &色,
    );
    let (materials, _) = VrmMaterials::load(
        &root,
        &root.join("Assets/Model.fbx"),
        &MaterialOptions::new(),
    )
    .expect("読める");
    let mtoon = materials
        .expect("ある")
        .get("Body")
        .expect("ある")
        .mtoon
        .clone()
        .expect("ある");
    // 重みは0.5×0.5=0.25であり、RGBは線形化してから掛ける。
    // 線形(0.5)≒0.214、線形(0.25)≒0.051、線形(0.125)≒0.013である。
    let 期待 = [0.214 * 0.25, 0.051 * 0.25, 0.013 * 0.25];
    for (実測, 目標) in mtoon.matcap_factor.iter().zip(期待.iter()) {
        assert!(
            (実測 - 目標).abs() < 0.003,
            "重みが合わない: {実測:?} と {期待:?}",
            実測 = mtoon.matcap_factor,
            期待 = 期待
        );
    }
    assert_eq!(mtoon.matcap_image, Some(0));
}

/// `_MatCapTex` の割り当てが無い場合はMatCap無効で係数はゼロになる。
#[test]
fn matcapテクスチャ無しは無効で係数はゼロになる() {
    let (_guard, root, mat, tex) = 最小配置を作る(SHADER_OPAQUE);
    let mut 浮動 = 基本浮動小数();
    for (名, 値) in &mut 浮動 {
        if *名 == "_UseMatCap" {
            *値 = 1.0;
        }
    }
    マテリアルを書く(
        &root,
        "Assets/Materials/Body.mat",
        &mat,
        "Body",
        Some(SHADER_OPAQUE),
        &[("_MainTex", 参照::割当(&tex)), ("_MatCapTex", 参照::Null)],
        &浮動,
        &基本色(),
    );
    let (materials, _) = VrmMaterials::load(
        &root,
        &root.join("Assets/Model.fbx"),
        &MaterialOptions::new(),
    )
    .expect("読める");
    let mtoon = materials
        .expect("ある")
        .get("Body")
        .expect("ある")
        .mtoon
        .clone()
        .expect("ある");
    assert_eq!(mtoon.matcap_factor, [0.0, 0.0, 0.0]);
    assert_eq!(mtoon.matcap_image, None);
}

/// `_BumpMap` は `_UseBumpMap` が真のときだけ法線に写像し、強さは `_BumpScale` になる。
#[test]
fn バンプマップは使う場合だけ法線に写像する() {
    let 判定 = |使う: f64| {
        let (_guard, root, mat, tex) = 最小配置を作る(SHADER_OPAQUE);
        let mut 浮動 = 基本浮動小数();
        浮動.push(("_UseBumpMap", 使う));
        浮動.push(("_BumpScale", 0.6));
        マテリアルを書く(
            &root,
            "Assets/Materials/Body.mat",
            &mat,
            "Body",
            Some(SHADER_OPAQUE),
            &[
                ("_MainTex", 参照::割当(&tex)),
                ("_BumpMap", 参照::割当(&tex)),
            ],
            &浮動,
            &基本色(),
        );
        let (materials, _) = VrmMaterials::load(
            &root,
            &root.join("Assets/Model.fbx"),
            &MaterialOptions::new(),
        )
        .expect("読める");
        materials.expect("ある").get("Body").expect("ある").clone()
    };
    let 使う = 判定(1.0);
    assert_eq!(使う.normal_image, Some(0));
    assert!((使う.normal_scale - 0.6).abs() < f32::EPSILON);
    let 使わない = 判定(0.0);
    assert_eq!(使わない.normal_image, None);
}

/// 描画順差分は実効描画順の相対順から決まる。
#[test]
fn 描画順差分は半透明の相対順から決まる() {
    let (_guard, root) = 作業域を作る();
    let 順序付きで書く = |mat: &str, スロット: &str, 描画順: Option<i32>, z書き込み: f64| {
        let mut 浮動 = 基本浮動小数();
        for (名, 値) in &mut 浮動 {
            if *名 == "_ZWrite" {
                *値 = z書き込み;
            }
        }
        マテリアルを書く(
            &root,
            &format!("Assets/Materials/{スロット}.mat"),
            mat,
            スロット,
            Some(SHADER_BLEND),
            &[],
            &浮動,
            &基本色(),
        );
        // `m_CustomRenderQueue` は共通欄に無いため、後から差し込む。
        if let Some(順序) = 描画順 {
            let 場所 = root.join(format!("Assets/Materials/{スロット}.mat"));
            let 本文 = fs::read_to_string(&場所).expect("matは読める");
            let 本文 = 本文.replacen(
                "  m_Shader:",
                &format!("  m_CustomRenderQueue: {順序}\n  m_Shader:"),
                1,
            );
            fs::write(&場所, 本文.as_bytes()).expect("matは書ける");
        }
    };
    let a = guid(0x11);
    let b = guid(0x12);
    let c = guid(0x13);
    let d = guid(0x14);
    fbxメタを書く(
        &root,
        &guid(0x99),
        &[
            ("A", Some(&a)),
            ("B", Some(&b)),
            ("C", Some(&c)),
            ("D", Some(&d)),
        ],
    );
    for (mat, スロット) in [(&a, "A"), (&b, "B"), (&c, "C"), (&d, "D")] {
        メタを書く(
            &root,
            &format!("Assets/Materials/{スロット}.mat"),
            mat,
            "NativeFormatImporter",
            "  externalObjects: {}\n  userData: \n",
        );
    }
    // AはZ書き込みあり・既定3000、BはZ書き込みあり・2900、
    // CはZ書き込みなし・2900、DはZ書き込みなし・2800である。
    順序付きで書く(&a, "A", None, 1.0);
    順序付きで書く(&b, "B", Some(2900), 1.0);
    順序付きで書く(&c, "C", Some(2900), 0.0);
    順序付きで書く(&d, "D", Some(2800), 0.0);
    let (materials, _) = VrmMaterials::load(
        &root,
        &root.join("Assets/Model.fbx"),
        &MaterialOptions::new(),
    )
    .expect("読める");
    let materials = materials.expect("ある");
    let 差分 = |スロット: &str| {
        materials
            .get(スロット)
            .expect("ある")
            .mtoon
            .clone()
            .expect("ある")
            .render_queue_offset
    };
    // Z書き込みありは昇順に0から数える。B(2900)が0、A(3000)が1である。
    assert_eq!(差分("A"), 1);
    assert_eq!(差分("B"), 0);
    // Z書き込みなしは降順に0から下げる。C(2900)が0、D(2800)が-1である。
    assert_eq!(差分("C"), 0);
    assert_eq!(差分("D"), -1);
}

/// リム無効時とUVアニメ停止時の既定値を読める。
#[test]
fn リムとuvアニメの既定値を読める() {
    let (_guard, root, _mat, _tex) = 最小配置を作る(SHADER_OPAQUE);
    let (materials, report) = VrmMaterials::load(
        &root,
        &root.join("Assets/Model.fbx"),
        &MaterialOptions::new(),
    )
    .expect("読める");
    assert!(
        report.warnings().is_empty(),
        "警告は無い: {:?}",
        report.warnings()
    );
    let mtoon = materials
        .expect("ある")
        .get("Body")
        .expect("ある")
        .mtoon
        .clone()
        .expect("ある");
    assert_eq!(mtoon.parametric_rim_color_factor, [0.0, 0.0, 0.0]);
    assert!((mtoon.parametric_rim_fresnel_power_factor - 5.0).abs() < f32::EPSILON);
    assert_eq!(mtoon.parametric_rim_lift_factor, 0.0);
    assert!((mtoon.rim_lighting_mix_factor - 1.0).abs() < f32::EPSILON);
    assert_eq!(mtoon.uv_animation_scroll_x_speed_factor, 0.0);
    assert_eq!(mtoon.uv_animation_scroll_y_speed_factor, 0.0);
    assert_eq!(mtoon.uv_animation_rotation_speed_factor, 0.0);
}

/// R=x*64・G=y*64・B=128・A=255の模様のRGBA8のPNGバイト列を作る。
fn 模様pngを作る(幅: u32, 高さ: u32) -> Vec<u8> {
    let mut raw = Vec::with_capacity(幅 as usize * 高さ as usize * 4);
    for y in 0..高さ {
        for x in 0..幅 {
            raw.extend_from_slice(&[((x * 64) % 256) as u8, ((y * 64) % 256) as u8, 128, 255]);
        }
    }
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, 幅, 高さ);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().expect("PNGは書ける");
    writer.write_image_data(&raw).expect("画素は書ける");
    drop(writer);
    out
}

/// 取り込み上限付きのテクスチャ資産と `.meta` を書く。
/// `上限部` は `TextureImporter:` の直下に置くYAML断片である。
fn 上限テクスチャを書く(
    root: &Path, 相対: &str, guid: &str, 内容: &[u8], 上限部: &str
) {
    資産を書く(root, 相対, 内容);
    メタを書く(
        root,
        相対,
        guid,
        "TextureImporter",
        &format!("  externalObjects: {{}}\n{上限部}"),
    );
}

/// PNGバイト列をRGBA8へ復号する。寸法と画素列を返す。
fn pngを復号する(バイト列: &[u8]) -> (u32, u32, Vec<u8>) {
    let decoder = png::Decoder::new(std::io::Cursor::new(バイト列));
    let mut reader = decoder.read_info().expect("PNGは読める");
    let mut raw = vec![0u8; reader.output_buffer_size().expect("寸法は読める")];
    let info = reader.next_frame(&mut raw).expect("画素は読める");
    assert_eq!(info.color_type, png::ColorType::Rgba);
    assert_eq!(info.bit_depth, png::BitDepth::Eight);
    (info.width, info.height, raw)
}

/// 基準色の直載せに使う上限付き配置を作る。`上限部` は `.meta` の断片である。
fn 上限配置を作る(上限部: &str) -> (tempfile::TempDir, PathBuf) {
    let (_guard, root) = 作業域を作る();
    let mat = guid(0x11);
    let tex = guid(0x22);
    fbxメタを書く(&root, &guid(0x99), &[("Body", Some(&mat))]);
    上限テクスチャを書く(
        &root,
        "Assets/Textures/Body.png",
        &tex,
        &模様pngを作る(4, 4),
        上限部,
    );
    マテリアルを書く(
        &root,
        "Assets/Materials/Body.mat",
        &mat,
        "Body",
        Some(SHADER_OPAQUE),
        &[("_MainTex", 参照::割当(&tex))],
        &基本浮動小数(),
        &基本色(),
    );
    (_guard, root)
}

/// 取り込み上限で直載せ画像が縮小され、Standalone上書きを使わない。
#[test]
fn 取り込み上限で直載せ画像が縮小される() {
    // Default 2（Standalone 2048は使わない）で4x4を2x2へ縮める。
    // 縮小結果が2x2であることが、Default優先の証拠になる。
    let (_guard, root) = 上限配置を作る(
        "  maxTextureSize: 2048\n  nPOTScale: 1\n  textureSettings:\n    filterMode: 1\n    wrapU: 0\n    wrapV: 0\n  mipmaps:\n    enableMipMap: 1\n    sRGBTexture: 1\n  platformSettings:\n  - buildTarget: DefaultTexturePlatform\n    maxTextureSize: 2\n  - buildTarget: Standalone\n    maxTextureSize: 2048\n",
    );
    let (materials, report) = VrmMaterials::load(
        &root,
        &root.join("Assets/Model.fbx"),
        &MaterialOptions::new(),
    )
    .expect("読める");
    assert!(
        report.warnings().is_empty(),
        "警告は無い: {:?}",
        report.warnings()
    );
    let 所有 = materials.expect("ある");
    assert_eq!(所有.images().len(), 1);
    let image = &所有.images()[0];
    assert_eq!(image.mime, VrmImageMime::Png);
    assert_eq!(image.name, "Body");
    let (幅, 高さ, 画素) = pngを復号する(&image.data);
    assert_eq!((幅, 高さ), (2, 2));
    // 面積平均（符号化済み）の手計算と合う。Rは列平均（32・160）、Gは行平均である。
    assert_eq!(
        画素,
        vec![
            32, 32, 128, 255, //
            160, 32, 128, 255, //
            32, 160, 128, 255, //
            160, 160, 128, 255, //
        ],
        "{画素:?}"
    );
}

/// グローバル上限がDefault欠落時の代替になり、拡大はしない。
#[test]
fn グローバル上限が欠落時の代替になる() {
    // platformSettingsが無い場合はグローバル値を使い、上限以下では拡大しない。
    let (_guard, root) = 上限配置を作る(
        "  maxTextureSize: 2\n  nPOTScale: 1\n  textureSettings:\n    filterMode: 1\n    wrapU: 0\n    wrapV: 0\n  mipmaps:\n    enableMipMap: 1\n    sRGBTexture: 1\n",
    );
    let (materials, report) = VrmMaterials::load(
        &root,
        &root.join("Assets/Model.fbx"),
        &MaterialOptions::new(),
    )
    .expect("読める");
    assert!(
        report.warnings().is_empty(),
        "警告は無い: {:?}",
        report.warnings()
    );
    let 所有 = materials.expect("ある");
    let (幅, 高さ, _) = pngを復号する(&所有.images()[0].data);
    assert_eq!((幅, 高さ), (2, 2));
}

/// 不明なNPOT値は警告して等倍として扱う。
#[test]
fn 不明なnpot値は警告して等倍になる() {
    let (_guard, root) = 上限配置を作る(
        "  nPOTScale: 9\n  textureSettings:\n    filterMode: 1\n    wrapU: 0\n    wrapV: 0\n  mipmaps:\n    enableMipMap: 1\n    sRGBTexture: 1\n",
    );
    let (materials, report) = VrmMaterials::load(
        &root,
        &root.join("Assets/Model.fbx"),
        &MaterialOptions::new(),
    )
    .expect("読める");
    assert!(
        report.warnings().iter().any(|警告| 警告.contains("NPOT")),
        "警告: {:?}",
        report.warnings()
    );
    let 所有 = materials.expect("ある");
    let (幅, 高さ, _) = pngを復号する(&所有.images()[0].data);
    assert_eq!((幅, 高さ), (4, 4));
}

/// 焼きの出力寸法は縮小後の入力に従う。
#[test]
fn 焼きの出力は縮小後の入力に従う() {
    // 主とアルファマスクを同じ4x4・上限2にし、基準色の焼きが2x2になることを確かめる。
    // 用途区分（色とデータ）が違うため、同じGUIDでも別の版として縮小する。
    let (_guard, root) = 作業域を作る();
    let mat = guid(0x11);
    let tex = guid(0x22);
    fbxメタを書く(&root, &guid(0x99), &[("Body", Some(&mat))]);
    上限テクスチャを書く(
        &root,
        "Assets/Textures/Body.png",
        &tex,
        &模様pngを作る(4, 4),
        "  maxTextureSize: 2\n  nPOTScale: 0\n  textureSettings:\n    filterMode: 1\n    wrapU: 0\n    wrapV: 0\n  mipmaps:\n    enableMipMap: 1\n    sRGBTexture: 1\n",
    );
    let mut 浮動 = 基本浮動小数();
    浮動.push(("_AlphaMaskMode", 1.0));
    マテリアルを書く(
        &root,
        "Assets/Materials/Body.mat",
        &mat,
        "Body",
        Some(SHADER_OPAQUE),
        &[
            ("_MainTex", 参照::割当(&tex)),
            ("_AlphaMask", 参照::割当(&tex)),
        ],
        &浮動,
        &基本色(),
    );
    let (materials, report) = VrmMaterials::load(
        &root,
        &root.join("Assets/Model.fbx"),
        &MaterialOptions::new(),
    )
    .expect("読める");
    assert!(
        report.warnings().is_empty(),
        "警告は無い: {:?}",
        report.warnings()
    );
    let 所有 = materials.expect("ある");
    let material = 所有.get("Body").expect("ある");
    let 索引 = material.base_color_image.expect("焼いた基準色がある");
    let (幅, 高さ, _) = pngを復号する(&所有.images()[索引].data);
    assert_eq!((幅, 高さ), (2, 2));
}
