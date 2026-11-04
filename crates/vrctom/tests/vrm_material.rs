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

/// 取り込み上限付きのテクスチャ資産と `.meta` を書く。
///
/// 直載せの縮小経路を通すため、全体の `maxTextureSize` を付ける。
/// プラットフォーム上書きが無い場合はこの全体値が使われる。
fn 縮小指定テクスチャを書く(
    root: &Path,
    相対: &str,
    guid: &str,
    内容: &[u8],
    上限: u32,
) {
    資産を書く(root, 相対, 内容);
    メタを書く(
        root,
        相対,
        guid,
        "TextureImporter",
        &format!(
            "  externalObjects: {{}}\n  maxTextureSize: {上限}\n  textureSettings:\n    filterMode: 1\n    wrapU: 0\n    wrapV: 0\n  mipmaps:\n    enableMipMap: 1\n"
        ),
    );
}

/// 指定寸法の単色RGBA8のPNGバイト列を作る。
///
/// べた塗りのため圧縮で小さく収まり、巨大なテスト用画像を置かずに
/// 寸法由来の上限を確認できる。
fn 単色pngを作る(幅: u32, 高さ: u32, 画素: [u8; 4]) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, 幅, 高さ);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().expect("PNGヘッダは書ける");
        let pixels = vec![画素; 幅 as usize * 高さ as usize].concat();
        writer.write_image_data(&pixels).expect("PNG画素は書ける");
    }
    out
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
fn 直載せでも画素上限を超えると失敗する() {
    // `maxTextureSize` も NPOT も無い直載せ配置では復号が起きないが、
    // 埋め込む前に寸法をヘッダから検査するため、2x2（4画素）の画像を
    // 上限3画素で読むと失敗する。
    let (_guard, root) = 作業域を作る();
    let mat = guid(0x11);
    let tex = guid(0x22);
    fbxメタを書く(&root, &guid(0x99), &[("Body", Some(&mat))]);
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
    テクスチャを書く(
        &root,
        "Assets/Textures/Body.png",
        &tex,
        &単色pngを作る(2, 2, [0x11, 0x22, 0x33, 0xFF]),
    );
    let 失敗 = VrmMaterials::load(
        &root,
        &root.join("Assets/Model.fbx"),
        &MaterialOptions::new().with_max_image_pixels(3),
    )
    .expect_err("画素上限の超過は失敗する");
    assert_eq!(失敗.kind(), vrctom::vrm::MaterialErrorKind::LimitExceeded);
    let 文面 = 失敗.to_string();
    assert!(文面.contains("2x2"), "寸法を含む: {文面}");
}

#[test]
fn 上限内の直載せ画像はそのまま埋め込まれる() {
    // 上の配置と同じ画像を上限4画素で読むと、縮小も焼きもせず埋め込む。
    let (_guard, root) = 作業域を作る();
    let mat = guid(0x11);
    let tex = guid(0x22);
    fbxメタを書く(&root, &guid(0x99), &[("Body", Some(&mat))]);
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
    let 原本 = 単色pngを作る(2, 2, [0x11, 0x22, 0x33, 0xFF]);
    テクスチャを書く(&root, "Assets/Textures/Body.png", &tex, &原本);
    let (materials, report) = VrmMaterials::load(
        &root,
        &root.join("Assets/Model.fbx"),
        &MaterialOptions::new().with_max_image_pixels(4),
    )
    .expect("上限内は読める");
    assert!(
        report.warnings().is_empty(),
        "警告は無い: {:?}",
        report.warnings()
    );
    let materials = materials.expect("置換表があるため材質がある");
    assert_eq!(materials.images().len(), 1);
    assert_eq!(materials.images()[0].data, 原本);
}

#[test]
fn ヘッダを読めない画像は警告して省き変換は続く() {
    // PNGは署名の直後に不正な長さを置き、JPEGは標識だけにする。
    // どちらも寸法を読めないため、未検査のまま埋め込まず省く。変換は続く。
    let (_guard, root) = 作業域を作る();
    let mat = guid(0x11);
    let first = guid(0x22);
    let second = guid(0x23);
    fbxメタを書く(&root, &guid(0x99), &[("Body", Some(&mat))]);
    // 同名は最初の1件が採用されるため、既定列の値を書き換える。
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
            ("_MainTex", 参照::割当(&first)),
            ("_EmissionMap", 参照::割当(&second)),
        ],
        &浮動,
        &基本色(),
    );
    // 署名の直後にファイル長を超える長さを置き、チャンク構造を読めなくする。
    let mut 壊れたpng = Vec::new();
    壊れたpng.extend_from_slice(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]);
    壊れたpng.extend_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF]);
    壊れたpng.extend_from_slice(b"IHDR");
    壊れたpng.extend_from_slice(&[0x00; 8]);
    テクスチャを書く(&root, "Assets/Textures/Body.png", &first, &壊れたpng);
    // 先頭標識だけのJPEGはヘッダとして成立しない。
    テクスチャを書く(
        &root,
        "Assets/Textures/Body.jpg",
        &second,
        &[0xFF, 0xD8, 0xFF, 0x00],
    );
    let (materials, report) = VrmMaterials::load(
        &root,
        &root.join("Assets/Model.fbx"),
        &MaterialOptions::new(),
    )
    .expect("読める");
    let materials = materials.expect("ある");
    // どちらも省き、画像は残らない。
    assert!(materials.images().is_empty(), "画像は省く");
    let material = materials.get("Body").expect("ある");
    assert_eq!(material.base_color_image, None);
    assert_eq!(material.emissive_image, None);
    assert_eq!(report.warnings().len(), 2, "警告: {:?}", report.warnings());
    for 警告 in report.warnings() {
        assert!(警告.contains("寸法"), "警告: {警告}");
    }
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
fn 縮小経路で1枚の画素上限を超えると失敗する() {
    // 1024x1024の単色PNGは圧縮で小さく収まるが画素数は約100万である。
    // 確保前の寸法判定により、小さな実ファイルでも上限超過として失敗する。
    let (_guard, root) = 作業域を作る();
    let mat = guid(0x11);
    let tex = guid(0x22);
    fbxメタを書く(&root, &guid(0x99), &[("Body", Some(&mat))]);
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
    let 大きい = 単色pngを作る(1024, 1024, [0x11, 0x22, 0x33, 0xFF]);
    assert!(
        大きい.len() < 100_000,
        "実ファイルは小さい: {}",
        大きい.len()
    );
    縮小指定テクスチャを書く(&root, "Assets/Textures/Body.png", &tex, &大きい, 16);
    let 失敗 = VrmMaterials::load(
        &root,
        &root.join("Assets/Model.fbx"),
        &MaterialOptions::new().with_max_image_pixels(1000),
    )
    .expect_err("画素上限の超過は失敗する");
    assert_eq!(失敗.kind(), vrctom::vrm::MaterialErrorKind::LimitExceeded);
    let 文面 = 失敗.to_string();
    assert!(文面.contains("1024x1024"), "寸法を含む: {文面}");
    assert!(文面.contains("1000"), "上限を含む: {文面}");
}

#[test]
fn 焼き経路で1枚の画素上限を超えると失敗する() {
    // アルファマスク付きの基準色は焼きのため復号する。取り込み上限が無く
    // 直載せの縮小が起きない配置でも、画素上限で失敗する。
    let (_guard, root) = 作業域を作る();
    let mat = guid(0x11);
    let tex = guid(0x22);
    let mask = guid(0x23);
    fbxメタを書く(&root, &guid(0x99), &[("Body", Some(&mat))]);
    let mut 浮動 = 基本浮動小数();
    浮動.push(("_AlphaMaskMode", 2.0));
    マテリアルを書く(
        &root,
        "Assets/Materials/Body.mat",
        &mat,
        "Body",
        Some(SHADER_OPAQUE),
        &[
            ("_MainTex", 参照::割当(&tex)),
            ("_AlphaMask", 参照::割当(&mask)),
        ],
        &浮動,
        &基本色(),
    );
    let 大きい = 単色pngを作る(1024, 1024, [0x44, 0x55, 0x66, 0xFF]);
    テクスチャを書く(&root, "Assets/Textures/Body.png", &tex, &大きい);
    テクスチャを書く(&root, "Assets/Textures/Mask.png", &mask, PNG_1X1);
    let 失敗 = VrmMaterials::load(
        &root,
        &root.join("Assets/Model.fbx"),
        &MaterialOptions::new().with_max_image_pixels(1000),
    )
    .expect_err("画素上限の超過は失敗する");
    assert_eq!(失敗.kind(), vrctom::vrm::MaterialErrorKind::LimitExceeded);
    let 文面 = 失敗.to_string();
    assert!(文面.contains("1024x1024"), "寸法を含む: {文面}");
    assert!(文面.contains("1000"), "上限を含む: {文面}");
}

#[test]
fn 保持予算を超えても捨てて再復号し成功する() {
    // 100x100（1万画素）の主テクスチャとマスクを基準色の焼きで読む。
    // 保持予算を1画素にしても、古い保持を捨てて再復号するため成功する。
    // 予算の超過では失敗しない。
    let (_guard, root) = 作業域を作る();
    let mat = guid(0x11);
    let main = guid(0x22);
    let mask = guid(0x23);
    fbxメタを書く(&root, &guid(0x99), &[("Body", Some(&mat))]);
    let mut 浮動 = 基本浮動小数();
    浮動.push(("_AlphaMaskMode", 2.0));
    マテリアルを書く(
        &root,
        "Assets/Materials/Body.mat",
        &mat,
        "Body",
        Some(SHADER_OPAQUE),
        &[
            ("_MainTex", 参照::割当(&main)),
            ("_AlphaMask", 参照::割当(&mask)),
        ],
        &浮動,
        &基本色(),
    );
    テクスチャを書く(
        &root,
        "Assets/Textures/Main.png",
        &main,
        &単色pngを作る(100, 100, [0x11, 0x22, 0x33, 0xFF]),
    );
    テクスチャを書く(
        &root,
        "Assets/Textures/Mask.png",
        &mask,
        &単色pngを作る(100, 100, [0x44, 0x55, 0x66, 0xFF]),
    );
    let (materials, report) = VrmMaterials::load(
        &root,
        &root.join("Assets/Model.fbx"),
        &MaterialOptions::new()
            .with_max_image_pixels(u64::MAX)
            .with_max_total_image_pixels(1),
    )
    .expect("保持予算の超過は失敗しない");
    assert!(
        report.warnings().is_empty(),
        "警告は無い: {:?}",
        report.warnings()
    );
    let materials = materials.expect("置換表があるため材質がある");
    assert_eq!(materials.entries().len(), 1);
    let material = materials.get("Body").expect("スロットで引ける");
    assert!(material.base_color_image.is_some(), "基準色は焼いて載る");
}

#[test]
fn 上限内の画素数は縮小を伴っても読める() {
    // 4x4を取り込み上限2で縮小する。復号と合計の計数は通るが、上限内のため読める。
    let (_guard, root) = 作業域を作る();
    let mat = guid(0x11);
    let tex = guid(0x22);
    fbxメタを書く(&root, &guid(0x99), &[("Body", Some(&mat))]);
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
    縮小指定テクスチャを書く(
        &root,
        "Assets/Textures/Body.png",
        &tex,
        &単色pngを作る(4, 4, [0x11, 0x22, 0x33, 0xFF]),
        2,
    );
    let (materials, report) = VrmMaterials::load(
        &root,
        &root.join("Assets/Model.fbx"),
        &MaterialOptions::new()
            .with_max_image_pixels(100)
            .with_max_total_image_pixels(100),
    )
    .expect("上限内は読める");
    assert!(
        report.warnings().is_empty(),
        "警告は無い: {:?}",
        report.warnings()
    );
    let materials = materials.expect("置換表があるため材質がある");
    assert_eq!(materials.entries().len(), 1);
    assert_eq!(materials.images().len(), 1);
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

// ===== メモリ内パッケージ経由の読み取り（`load_with`）の契約 =====
// 以下のテストは、同じ配置をファイル経由（`VrmMaterials::load`）と
// メモリ経由（`unpack_in_memory` → `asset_index` → `VrmMaterials::load_with`）の
// 両方で読み、結果が一致することを確かめる。

use vrctom::unity::AssetSource;

/// `Assets` 配下の通常ファイルを集める。
/// 戻り値は（`Assets` 基準の相対パス、実体バイト列、metaバイト列）の列である。
/// `.meta` の無い資産があればテストの組み立て不良として失敗する。
fn 資産一覧を作る(root: &Path) -> Vec<(String, Vec<u8>, Vec<u8>)> {
    use std::collections::HashMap;
    let mut 表: HashMap<String, Vec<u8>> = HashMap::new();
    let mut 辿る = vec![PathBuf::from("Assets")];
    while let Some(相対) = 辿る.pop() {
        let mut 子: Vec<fs::DirEntry> = fs::read_dir(root.join(&相対))
            .expect("一覧は読める")
            .collect::<Result<_, _>>()
            .expect("項目は読める");
        子.sort_by_key(|項目| 項目.file_name());
        for 項目 in 子 {
            let 子相対 = 相対.join(項目.file_name());
            let 種別 = 項目.file_type().expect("種別は読める");
            if 種別.is_dir() {
                辿る.push(子相対);
            } else if 種別.is_file() {
                let 文字列 = 子相対
                    .iter()
                    .map(|部| 部.to_str().expect("名前はUTF-8である"))
                    .collect::<Vec<_>>()
                    .join("/");
                let 基準 = 文字列
                    .strip_prefix("Assets/")
                    .expect("Assets配下である")
                    .to_owned();
                let 内容 = fs::read(root.join(&子相対)).expect("資産は読める");
                表.insert(基準, 内容);
            }
        }
    }
    // 索引は `.meta` 内容から作るため、資産とmetaを対にして返す。
    let mut 鍵: Vec<String> = 表.keys().cloned().collect();
    鍵.sort();
    let mut 対応 = Vec::new();
    for 相対 in 鍵 {
        if 相対.ends_with(".meta") {
            continue;
        }
        let 実体 = 表.remove(&相対).expect("列挙元である");
        let メタ = 表
            .remove(&format!("{相対}.meta"))
            .unwrap_or_else(|| panic!("metaが無い資産がある: {相対}"));
        対応.push((相対, 実体, メタ));
    }
    assert!(表.is_empty(), "資産の無いmetaがある: {:?}", 表.keys());
    対応
}

/// 作業域の配置をtar+gzipのunitypackageバイト列にする。
/// tar区画用のGUIDは採番し直す。索引は `.meta` 内容から作るため、
/// 材質の解決結果には影響しない。
fn パッケージ化する(root: &Path) -> Vec<u8> {
    先頭付きでパッケージ化する(root, "Assets")
}

/// 作業域の配置を指定した先頭成分で包んだunitypackageバイト列にする。
///
/// 同一内容を `Assets/` と `assets/` で包んで比べるために使う。
/// 読み元は `Assets` 配置のまま変えず、先頭成分だけを置き換える。
/// tar区画用のGUIDは採番し直す。
fn 先頭付きでパッケージ化する(root: &Path, 先頭: &str) -> Vec<u8> {
    use std::io::Write as _;
    let 一覧 = 資産一覧を作る(root);
    let mut 作成 = tar::Builder::new(Vec::new());
    for (番号, (相対, 実体, メタ)) in 一覧.iter().enumerate() {
        let 区画 = format!("{番号:032x}");
        for (名, 内容) in [
            ("pathname", format!("{先頭}/{相対}").into_bytes()),
            ("asset", 実体.clone()),
            ("asset.meta", メタ.clone()),
        ] {
            let mut 頭 = tar::Header::new_gnu();
            頭.set_size(内容.len() as u64);
            頭.set_mode(0o644);
            頭.set_cksum();
            作成
                .append_data(&mut 頭, format!("{区画}/{名}"), &内容[..])
                .expect("tarは書ける");
        }
    }
    let 固まり = 作成.into_inner().expect("tarは完成する");
    let mut 符号 = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    符号.write_all(&固まり).expect("gzipは書ける");
    符号.finish().expect("gzipは完成する")
}

/// 同じ作業域をファイル経由とメモリ経由の両方で読む。
/// 戻り値は（ファイル経路の結果、メモリ経路の結果）である。
#[allow(clippy::type_complexity)]
fn 両経路で読む(
    root: &Path,
    選択: &MaterialOptions,
) -> (
    Result<(Option<VrmMaterials>, vrctom::vrm::MaterialReport), vrctom::vrm::MaterialError>,
    Result<(Option<VrmMaterials>, vrctom::vrm::MaterialReport), vrctom::vrm::MaterialError>,
) {
    let ファイル = VrmMaterials::load(root, &root.join("Assets/Model.fbx"), 選択);
    let メモリ = {
        let 塊 = パッケージ化する(root);
        let 包 = vrctom::unitypackage::unpack_in_memory(
            &塊[..],
            &vrctom::unitypackage::UnpackOptions::new(),
        )
        .expect("展開できる");
        let 索引 = 包
            .asset_index(&vrctom::unity::IndexOptions::new())
            .expect("索引化できる");
        VrmMaterials::load_with(&包, &索引, "Model.fbx", 選択)
    };
    (ファイル, メモリ)
}

/// 両経路の成功結果が一致することを確かめる。
/// 材質名・透過方式・基準色・画像のMIMEとバイト列・警告・画像枚数を見る。
/// 材質の等価性は `VrmMaterial` の等価性でまとめて確かめる。
fn 成功が一致する(
    ファイル: (Option<VrmMaterials>, vrctom::vrm::MaterialReport),
    メモリ: (Option<VrmMaterials>, vrctom::vrm::MaterialReport),
) {
    let (ファイル材質, ファイル報告) = ファイル;
    let (メモリ材質, メモリ報告) = メモリ;
    assert_eq!(
        ファイル報告.warnings(),
        メモリ報告.warnings(),
        "警告が一致する"
    );
    match (ファイル材質, メモリ材質) {
        (None, None) => {}
        (Some(甲), Some(乙)) => {
            assert_eq!(甲.entries().len(), 乙.entries().len(), "材質件数が一致する");
            for (甲欄, 乙欄) in 甲.entries().iter().zip(乙.entries().iter()) {
                assert_eq!(甲欄.slot, 乙欄.slot, "スロットが一致する");
                assert_eq!(
                    甲欄.material, 乙欄.material,
                    "材質が一致する: {}",
                    甲欄.slot
                );
            }
            assert_eq!(甲.images().len(), 乙.images().len(), "画像枚数が一致する");
            for (甲画, 乙画) in 甲.images().iter().zip(乙.images().iter()) {
                assert_eq!(甲画.name, 乙画.name, "画像名が一致する");
                assert_eq!(甲画.mime, 乙画.mime, "画像MIMEが一致する");
                assert_eq!(甲画.data, 乙画.data, "画像バイト列が一致する");
                assert_eq!(甲画.sampler, 乙画.sampler, "サンプラが一致する");
            }
        }
        (甲, 乙) => panic!("材質の有無が一致しない: ファイル{甲:?} メモリ{乙:?}"),
    }
}

/// 両経路の失敗結果が一致することを確かめる。種別と対象パスを見る。
fn 失敗が一致する(
    ファイル: Result<
        (Option<VrmMaterials>, vrctom::vrm::MaterialReport),
        vrctom::vrm::MaterialError,
    >,
    メモリ: Result<(Option<VrmMaterials>, vrctom::vrm::MaterialReport), vrctom::vrm::MaterialError>,
) {
    let ファイル = ファイル.expect_err("ファイル経路は失敗する");
    let メモリ = メモリ.expect_err("メモリ経路は失敗する");
    assert_eq!(ファイル.kind(), メモリ.kind(), "失敗種別が一致する");
    assert_eq!(ファイル.path(), メモリ.path(), "対象パスが一致する");
}

#[test]
fn メモリ読み取りはファイル読み取りと一致する() {
    let (_guard, root, _mat, _tex) = 最小配置を作る(SHADER_OPAQUE);
    let (ファイル, メモリ) = 両経路で読む(&root, &MaterialOptions::new());
    let ファイル = ファイル.expect("ファイル経路は読める");
    let メモリ = メモリ.expect("メモリ経路は読める");
    // 最小配置は警告なし・材質1件・画像1件である。
    assert!(
        ファイル.1.warnings().is_empty(),
        "警告は無い: {:?}",
        ファイル.1.warnings()
    );
    assert_eq!(
        ファイル.0.as_ref().expect("材質がある").entries().len(),
        1,
        "材質は1件ある"
    );
    assert_eq!(
        ファイル.0.as_ref().expect("材質がある").images().len(),
        1,
        "画像は1枚ある"
    );
    成功が一致する(ファイル, メモリ);
}

#[test]
fn 置換表なしでもメモリ読み取りはファイル読み取りと一致する() {
    let (_guard, root) = 作業域を作る();
    資産を書く(&root, "Assets/Model.fbx", b"");
    メタを書く(
        &root,
        "Assets/Model.fbx",
        &guid(0x99),
        "ModelImporter",
        "  externalObjects: {}\n",
    );
    let (ファイル, メモリ) = 両経路で読む(&root, &MaterialOptions::new());
    let ファイル = ファイル.expect("ファイル経路は読める");
    let メモリ = メモリ.expect("メモリ経路は読める");
    // どちらも材質なしで警告を残す。
    assert!(ファイル.0.is_none(), "ファイル経路は材質なしである");
    assert!(!ファイル.1.warnings().is_empty(), "警告はある");
    成功が一致する(ファイル, メモリ);
}

#[test]
fn 未対応シェーダでもメモリ読み取りはファイル読み取りと一致する() {
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
    let (ファイル, メモリ) = 両経路で読む(&root, &MaterialOptions::new());
    let ファイル = ファイル.expect("ファイル経路は読める");
    let メモリ = メモリ.expect("メモリ経路は読める");
    // どちらも灰色材質へ落ちる。
    let 灰色 = ファイル
        .0
        .as_ref()
        .expect("材質がある")
        .get("Body")
        .expect("ある");
    assert!(灰色.mtoon.is_none(), "灰色PBRへ落ちる");
    成功が一致する(ファイル, メモリ);
}

#[test]
fn 参照切れでもメモリ読み取りはファイル読み取りと一致する() {
    let (_guard, root, mat, _tex) = 最小配置を作る(SHADER_OPAQUE);
    // 索引にないGUIDとTGA形式の2件を割り当てる。
    テクスチャを書く(&root, "Assets/Textures/Body.tga", &guid(0x33), b"TGA-fake");
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
    let (ファイル, メモリ) = 両経路で読む(&root, &MaterialOptions::new());
    let ファイル = ファイル.expect("ファイル経路は読める");
    let メモリ = メモリ.expect("メモリ経路は読める");
    // どちらも材質自体は続け、テクスチャだけ省く。
    let 材質 = ファイル
        .0
        .as_ref()
        .expect("材質がある")
        .get("Body")
        .expect("ある");
    assert_eq!(材質.base_color_image, None);
    assert_eq!(材質.emissive_image, None);
    成功が一致する(ファイル, メモリ);
}

#[test]
fn 上限超過でもメモリ読み取りはファイル読み取りと一致する() {
    let (_guard, root, _mat, _tex) = 最小配置を作る(SHADER_OPAQUE);
    for 選択 in [
        MaterialOptions::new().with_max_image_bytes(10),
        MaterialOptions::new().with_max_total_image_bytes(10),
    ] {
        let (ファイル, メモリ) = 両経路で読む(&root, &選択);
        assert_eq!(
            ファイル
                .as_ref()
                .expect_err("画像サイズ超過は失敗する")
                .kind(),
            vrctom::vrm::MaterialErrorKind::LimitExceeded
        );
        失敗が一致する(ファイル, メモリ);
    }
}

#[test]
fn jpegでもメモリ読み取りはファイル読み取りと一致する() {
    let (_guard, root) = 作業域を作る();
    let mat = guid(0x11);
    let tex = guid(0x22);
    fbxメタを書く(&root, &guid(0x99), &[("Body", Some(&mat))]);
    // 先頭判定だけに使う最小の標識列である。直載せでは復号しない。
    const JPEG_最小: &[u8] = &[
        0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, 0x4A, 0x46, 0x49, 0x46, 0x00, 0xFF, 0xD9,
    ];
    テクスチャを書く(&root, "Assets/Textures/Body.jpg", &tex, JPEG_最小);
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
    let (ファイル, メモリ) = 両経路で読む(&root, &MaterialOptions::new());
    let ファイル = ファイル.expect("ファイル経路は読める");
    let メモリ = メモリ.expect("メモリ経路は読める");
    // 標識だけのJPEGは寸法を読めないため、両経路とも省いて警告する。
    assert!(
        ファイル.0.as_ref().expect("材質がある").images().is_empty(),
        "画像は省く"
    );
    assert_eq!(
        ファイル.1.warnings().len(),
        1,
        "警告: {:?}",
        ファイル.1.warnings()
    );
    assert!(
        ファイル.1.warnings()[0].contains("寸法"),
        "警告: {:?}",
        ファイル.1.warnings()
    );
    成功が一致する(ファイル, メモリ);
}

#[test]
fn 先頭成分の大小文字違いでも索引と材質読取が同じ結果になる() {
    // 展開の共通核は先頭成分を大小文字無視で `Assets` と認める。
    // 読み取り側が常に `Assets/` を探すと `assets/` の配置で `NotFound` になる
    // 回帰として残す。PNGテクスチャと `.mat` を含む最小配置で比べる。
    let (_guard, root, _mat, _tex) = 最小配置を作る(SHADER_OPAQUE);
    let 選択 = MaterialOptions::new();
    let 読む = |先頭: &str| {
        let 塊 = 先頭付きでパッケージ化する(&root, 先頭);
        let 包 = vrctom::unitypackage::unpack_in_memory(
            &塊[..],
            &vrctom::unitypackage::UnpackOptions::new(),
        )
        .unwrap_or_else(|_| panic!("{先頭}は展開できる"));
        let 索引 = 包
            .asset_index(&vrctom::unity::IndexOptions::new())
            .unwrap_or_else(|_| panic!("{先頭}は索引化できる"));
        // `Assets` 基準の相対パスで同じ資産が見える。
        for 相対 in ["Model.fbx", "Materials/Body.mat", "Textures/Body.png"] {
            assert!(
                索引.get_by_path(相対).is_some(),
                "{先頭}の索引に{相対}がある"
            );
        }
        let 結果 = VrmMaterials::load_with(&包, &索引, "Model.fbx", &選択)
            .unwrap_or_else(|_| panic!("{先頭}は読める"));
        (結果, 索引)
    };
    let ((大文字読み, 大文字索引), (小文字読み, 小文字索引)) = (読む("Assets"), 読む("assets"));
    // 索引の相対パスと種別が一致する。
    for 相対 in ["Model.fbx", "Materials/Body.mat", "Textures/Body.png"] {
        let 大 = 大文字索引.get_by_path(相対).expect("大文字にある");
        let 小 = 小文字索引.get_by_path(相対).expect("小文字にある");
        assert_eq!(大.path, 小.path, "相対パスが一致する: {相対}");
        assert_eq!(大.kind, 小.kind, "種別が一致する: {相対}");
    }
    // 材質と画像の読み取り結果が一致する。
    成功が一致する(大文字読み, 小文字読み);
}

#[test]
fn 上限超過時は切り詰めず失敗する() {
    let (_guard, root, _mat, _tex) = 最小配置を作る(SHADER_OPAQUE);
    let 塊 = パッケージ化する(&root);
    let 包 = vrctom::unitypackage::unpack_in_memory(
        &塊[..],
        &vrctom::unitypackage::UnpackOptions::new(),
    )
    .expect("展開できる");
    let 全文 = 包.read("Assets/Textures/Body.png").expect("画像はある");
    let 長さ = 全文.len() as u64;
    // 上限ちょうどは内容のまま成功する。
    let 読めた = 包
        .read_limited("Textures/Body.png", 長さ)
        .expect("上限ちょうどは読める");
    assert_eq!(読めた, 全文);
    // 1バイト足りない上限は切り詰めず `InvalidData` で失敗する。
    let 失敗 = 包
        .read_limited("Textures/Body.png", 長さ - 1)
        .expect_err("超過は失敗する");
    assert_eq!(失敗.kind(), std::io::ErrorKind::InvalidData);
    // 存在しない資産は `NotFound` である。
    let 失敗 = 包
        .read_limited("Textures/無い.png", 長さ)
        .expect_err("欠落は失敗する");
    assert_eq!(失敗.kind(), std::io::ErrorKind::NotFound);
}

/// 小さいPSDを組み立てる。規定値は幅x高さのRGB・Rawである。
/// `平面` はR・G・B（Grayは1平面）の順に高さ行ずつのバイト列である。
#[allow(clippy::too_many_arguments)]
fn psdを組み立てる(
    幅: u32,
    高さ: u32,
    チャンネル数: u16,
    形式: u16,
    深度: u16,
    版: u16,
    資源: &[(u16, &[u8])],
    圧縮: u16,
    平面: &[u8],
) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"8BPS");
    out.extend_from_slice(&版.to_be_bytes());
    out.extend_from_slice(&[0; 6]);
    out.extend_from_slice(&チャンネル数.to_be_bytes());
    out.extend_from_slice(&高さ.to_be_bytes());
    out.extend_from_slice(&幅.to_be_bytes());
    out.extend_from_slice(&深度.to_be_bytes());
    out.extend_from_slice(&形式.to_be_bytes());
    out.extend_from_slice(&0u32.to_be_bytes());
    let mut resources = Vec::new();
    for (id, data) in 資源 {
        resources.extend_from_slice(b"8BIM");
        resources.extend_from_slice(&id.to_be_bytes());
        resources.extend_from_slice(&[0, 0]);
        resources.extend_from_slice(&(data.len() as u32).to_be_bytes());
        resources.extend_from_slice(data);
        if data.len() % 2 == 1 {
            resources.push(0);
        }
    }
    out.extend_from_slice(&(resources.len() as u32).to_be_bytes());
    out.extend_from_slice(&resources);
    out.extend_from_slice(&0u32.to_be_bytes());
    out.extend_from_slice(&圧縮.to_be_bytes());
    out.extend_from_slice(平面);
    out
}

/// 2x2のRGB・RawのPSDを作る。画素は行優先でR・G・Bの順に平面を並べる。
fn rgbのpsdを作る(画素: [[u8; 3]; 4]) -> Vec<u8> {
    let mut 平面 = Vec::with_capacity(12);
    for ch in 0..3 {
        for pixel in &画素 {
            平面.push(pixel[ch]);
        }
    }
    psdを組み立てる(2, 2, 3, 3, 8, 1, &[], 0, &平面)
}

/// RLEの行（PackBitsの直写）を作る。
fn 直写行(画素: &[u8]) -> Vec<u8> {
    let mut out = vec![(画素.len() - 1) as u8];
    out.extend_from_slice(画素);
    out
}

/// 2x2のRGB・RLEのPSDを作る。
fn rleのpsdを作る(画素: [[u8; 3]; 4]) -> Vec<u8> {
    let mut 表 = Vec::new();
    let mut 本体 = Vec::new();
    for ch in [0, 1, 2] {
        let 行0 = 直写行(&[画素[0][ch], 画素[1][ch]]);
        let 行1 = 直写行(&[画素[2][ch], 画素[3][ch]]);
        for 行 in [&行0, &行1] {
            表.extend_from_slice(&(行.len() as u16).to_be_bytes());
            本体.extend_from_slice(行);
        }
    }
    psdを組み立てる(2, 2, 3, 3, 8, 1, &[], 1, &[表, 本体].concat())
}

/// 指定の `.meta` 本文でPSDテクスチャ資産を書く。
fn psdテクスチャを書く(
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

/// PSDの直載せに使う最小配置を作る。`.meta` 断片を指定できる。
fn psd配置を作る(内容: &[u8], 上限部: &str) -> (tempfile::TempDir, PathBuf, String) {
    let (_guard, root) = 作業域を作る();
    let mat = guid(0x11);
    let tex = guid(0x22);
    fbxメタを書く(&root, &guid(0x99), &[("Body", Some(&mat))]);
    psdテクスチャを書く(&root, "Assets/Textures/Body.psd", &tex, 内容, 上限部);
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
    (_guard, root, tex)
}

/// PSDの直載せではcompositeを復号してPNG化し、画素が一致する。
#[test]
fn psdの直載せは復号してpng化する() {
    let 画素 = [[10, 20, 30], [40, 50, 60], [70, 80, 90], [100, 110, 120]];
    let (_guard, root, _tex) = psd配置を作る(
        &rgbのpsdを作る(画素),
        "  textureSettings:\n    filterMode: 1\n    wrapU: 0\n    wrapV: 0\n  mipmaps:\n    enableMipMap: 1\n    sRGBTexture: 1\n",
    );
    let (materials, report) = VrmMaterials::load(
        &root,
        &root.join("Assets/Model.fbx"),
        &MaterialOptions::new(),
    )
    .expect("読める");
    assert!(
        report.warnings().is_empty(),
        "成功したPSD変換は警告にしない: {:?}",
        report.warnings()
    );
    let 所有 = materials.expect("ある");
    assert_eq!(所有.images().len(), 1);
    let image = &所有.images()[0];
    assert_eq!(image.mime, VrmImageMime::Png);
    assert_eq!(image.name, "Body");
    assert_eq!(
        image.sampler,
        VrmSampler {
            wrap_u: VrmWrap::Repeat,
            wrap_v: VrmWrap::Repeat,
            filter: VrmFilter::Linear,
            mipmaps: true,
        }
    );
    let (幅, 高さ, 復号) = pngを復号する(&image.data);
    assert_eq!((幅, 高さ), (2, 2));
    // 復号コードと独立した固定値（組み立て時の画素＋不透明アルファ）と合う。
    let mut 期待 = Vec::new();
    for pixel in &画素 {
        期待.extend_from_slice(&[pixel[0], pixel[1], pixel[2], 255]);
    }
    assert_eq!(復号, 期待);
    let 材質 = 所有.get("Body").expect("ある");
    assert_eq!(材質.base_color_image, Some(0));
}

/// RLEのPSDも同じ画素に復号する。
#[test]
fn rleのpsdも同じ画素に復号する() {
    let 画素 = [[10, 20, 30], [40, 50, 60], [70, 80, 90], [100, 110, 120]];
    let (_guard, root, _tex) = psd配置を作る(
        &rleのpsdを作る(画素),
        "  textureSettings:\n    filterMode: 1\n    wrapU: 0\n    wrapV: 0\n  mipmaps:\n    enableMipMap: 1\n    sRGBTexture: 1\n",
    );
    let (materials, report) = VrmMaterials::load(
        &root,
        &root.join("Assets/Model.fbx"),
        &MaterialOptions::new(),
    )
    .expect("読める");
    assert!(
        report.warnings().is_empty(),
        "成功したPSD変換は警告にしない: {:?}",
        report.warnings()
    );
    let 所有 = materials.expect("ある");
    let (幅, 高さ, 復号) = pngを復号する(&所有.images()[0].data);
    assert_eq!((幅, 高さ), (2, 2));
    let mut 期待 = Vec::new();
    for pixel in &画素 {
        期待.extend_from_slice(&[pixel[0], pixel[1], pixel[2], 255]);
    }
    assert_eq!(復号, 期待);
}

/// GrayscaleのPSDは複写して不透明にする。
#[test]
fn grayscaleのpsdは複写する() {
    let (_guard, root, _tex) = psd配置を作る(
        &psdを組み立てる(2, 1, 1, 1, 8, 1, &[], 0, &[0, 255]),
        "  textureSettings:\n    filterMode: 1\n    wrapU: 0\n    wrapV: 0\n  mipmaps:\n    enableMipMap: 1\n    sRGBTexture: 1\n",
    );
    let (materials, report) = VrmMaterials::load(
        &root,
        &root.join("Assets/Model.fbx"),
        &MaterialOptions::new(),
    )
    .expect("読める");
    assert!(
        report.warnings().is_empty(),
        "成功したPSD変換は警告にしない: {:?}",
        report.warnings()
    );
    let 所有 = materials.expect("ある");
    let (幅, 高さ, 復号) = pngを復号する(&所有.images()[0].data);
    assert_eq!((幅, 高さ), (2, 1));
    assert_eq!(復号, vec![0, 0, 0, 255, 255, 255, 255, 255]);
}

/// 同じGUIDのPSDは1件だけ読んで索引を共有する。
#[test]
fn 同じpsdは索引を共有する() {
    let (_guard, root) = 作業域を作る();
    let mat_a = guid(0x11);
    let mat_b = guid(0x12);
    let tex = guid(0x22);
    fbxメタを書く(
        &root,
        &guid(0x99),
        &[("A", Some(&mat_a)), ("B", Some(&mat_b))],
    );
    let 画素 = [[1, 2, 3], [4, 5, 6], [7, 8, 9], [10, 11, 12]];
    psdテクスチャを書く(
        &root,
        "Assets/Textures/Body.psd",
        &tex,
        &rgbのpsdを作る(画素),
        "  textureSettings:\n    filterMode: 1\n    wrapU: 0\n    wrapV: 0\n  mipmaps:\n    enableMipMap: 1\n    sRGBTexture: 1\n",
    );
    for (mat, 名) in [(&mat_a, "A"), (&mat_b, "B")] {
        マテリアルを書く(
            &root,
            &format!("Assets/Materials/{名}.mat"),
            mat,
            名,
            Some(SHADER_OPAQUE),
            &[("_MainTex", 参照::割当(&tex))],
            &基本浮動小数(),
            &基本色(),
        );
    }
    let (materials, report) = VrmMaterials::load(
        &root,
        &root.join("Assets/Model.fbx"),
        &MaterialOptions::new(),
    )
    .expect("読める");
    assert!(
        report.warnings().is_empty(),
        "成功したPSD変換は警告にしない: {:?}",
        report.warnings()
    );
    let 所有 = materials.expect("ある");
    assert_eq!(所有.images().len(), 1);
    assert_eq!(所有.get("A").expect("ある").base_color_image, Some(0));
    assert_eq!(所有.get("B").expect("ある").base_color_image, Some(0));
}

/// `.meta` の上限でPSD直載せが縮小され、NPOTとsRGB旗とサンプラが効く。
#[test]
fn psdの取り込み上限とnpotとサンプラが効く() {
    // 4x4の各2x2塊をべた塗りにし、面積平均の手計算と合わせる。
    // R・G・Bとも同じ面割りにする。
    let 塊 = [
        10, 10, 200, 200, 10, 10, 200, 200, 30, 30, 90, 90, 30, 30, 90, 90,
    ];
    let 平面 = [塊.as_slice(), 塊.as_slice(), 塊.as_slice()].concat();
    let (_guard, root, _tex) = psd配置を作る(
        &psdを組み立てる(4, 4, 3, 3, 8, 1, &[], 0, &平面),
        "  maxTextureSize: 2\n  nPOTScale: 1\n  textureSettings:\n    filterMode: 0\n    wrapU: 1\n    wrapV: 2\n  mipmaps:\n    enableMipMap: 0\n    sRGBTexture: 1\n",
    );
    let (materials, report) = VrmMaterials::load(
        &root,
        &root.join("Assets/Model.fbx"),
        &MaterialOptions::new(),
    )
    .expect("読める");
    assert!(
        report.warnings().is_empty(),
        "成功したPSD変換は警告にしない: {:?}",
        report.warnings()
    );
    let 所有 = materials.expect("ある");
    let image = &所有.images()[0];
    assert_eq!(image.mime, VrmImageMime::Png);
    assert_eq!(
        image.sampler,
        VrmSampler {
            wrap_u: VrmWrap::ClampToEdge,
            wrap_v: VrmWrap::MirroredRepeat,
            filter: VrmFilter::Nearest,
            mipmaps: false,
        }
    );
    let (幅, 高さ, 復号) = pngを復号する(&image.data);
    assert_eq!((幅, 高さ), (2, 2));
    // R・G・Bとも各塊の平均（左上10・右上200・左下30・右下90）である。
    assert_eq!(
        復号,
        vec![
            10, 10, 10, 255, //
            200, 200, 200, 255, //
            30, 30, 30, 255, //
            90, 90, 90, 255, //
        ]
    );
}

/// NPOTの切り上げでPSDが拡大する。
#[test]
fn psdのnpot切り上げが効く() {
    // 3x3のべた塗りは切り上げで4x4になり、画素値は変わらない。
    let (_guard, root, _tex) = psd配置を作る(
        &psdを組み立てる(3, 3, 3, 3, 8, 1, &[], 0, &[77; 27]),
        "  nPOTScale: 2\n",
    );
    let (materials, report) = VrmMaterials::load(
        &root,
        &root.join("Assets/Model.fbx"),
        &MaterialOptions::new(),
    )
    .expect("読める");
    assert!(
        report.warnings().is_empty(),
        "成功したPSD変換は警告にしない: {:?}",
        report.warnings()
    );
    let 所有 = materials.expect("ある");
    let (幅, 高さ, 復号) = pngを復号する(&所有.images()[0].data);
    assert_eq!((幅, 高さ), (4, 4));
    assert!(復号.chunks_exact(4).all(|pixel| pixel == [77, 77, 77, 255]));
}

/// 法線用途の縮小ではsRGB旗で平均空間が変わる。
#[test]
fn psdの法線縮小はsrgb旗で平均空間が変わる() {
    for (旗, 期待) in [(true, 92u8), (false, 64u8)] {
        let (_guard, root) = 作業域を作る();
        let mat = guid(0x11);
        let tex = guid(0x22);
        fbxメタを書く(&root, &guid(0x99), &[("Body", Some(&mat))]);
        psdテクスチャを書く(
            &root,
            "Assets/Textures/Bump.psd",
            &tex,
            &psdを組み立てる(2, 1, 1, 1, 8, 1, &[], 0, &[0, 128]),
            &format!(
                "  maxTextureSize: 1\n  textureSettings:\n    filterMode: 1\n    wrapU: 0\n    wrapV: 0\n  mipmaps:\n    enableMipMap: 1\n    sRGBTexture: {}\n",
                if 旗 { 1 } else { 0 }
            ),
        );
        let mut 浮動 = 基本浮動小数();
        浮動.push(("_UseBumpMap", 1.0));
        マテリアルを書く(
            &root,
            "Assets/Materials/Body.mat",
            &mat,
            "Body",
            Some(SHADER_OPAQUE),
            &[("_BumpMap", 参照::割当(&tex))],
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
            "成功したPSD変換は警告にしない: {:?}",
            report.warnings()
        );
        let 所有 = materials.expect("ある");
        let 材質 = 所有.get("Body").expect("ある");
        let 索引 = 材質.normal_image.expect("法線は載る");
        let (幅, 高さ, 復号) = pngを復号する(&所有.images()[索引].data);
        assert_eq!((幅, 高さ), (1, 1));
        // sRGB標識は復号して平均して符号化し直す（92）、
        // リニア標識は符号化済みのまま平均する（64）。
        assert_eq!(復号, vec![期待, 期待, 期待, 255], "sRGB旗{旗}の結果が合う");
    }
}

/// 対象外のPSDは警告して省き、変換は続ける。
#[test]
fn 対象外のpsdは警告して省く() {
    // ZIP圧縮。
    let (_guard, root, _tex) = psd配置を作る(
        &psdを組み立てる(2, 2, 3, 3, 8, 1, &[], 2, &[0; 8]),
        "  textureSettings:\n    filterMode: 1\n    wrapU: 0\n    wrapV: 0\n  mipmaps:\n    enableMipMap: 1\n    sRGBTexture: 1\n",
    );
    let (materials, report) = VrmMaterials::load(
        &root,
        &root.join("Assets/Model.fbx"),
        &MaterialOptions::new(),
    )
    .expect("対象外でも変換は続ける");
    let 警告 = report.warnings().join("\n");
    assert!(
        警告.contains("PSDの圧縮方式が未対応のため省く"),
        "ZIPの警告がある: {警告}"
    );
    assert!(警告.contains("Body.psd"), "資産の所在がある: {警告}");
    let 所有 = materials.expect("ある");
    assert_eq!(所有.images().len(), 0);
    assert_eq!(所有.get("Body").expect("ある").base_color_image, None);
}

/// 版情報が偽のPSDは有効な合成画像なしとして省く。
#[test]
fn 結合なしのpsdは警告して省く() {
    let mut 版情報 = vec![0, 0, 0, 1, 0];
    版情報.extend_from_slice(b"reader");
    let (_guard, root, _tex) = psd配置を作る(
        &psdを組み立てる(2, 2, 3, 3, 8, 1, &[(1057, &版情報)], 0, &[0xFF; 12]),
        "  textureSettings:\n    filterMode: 1\n    wrapU: 0\n    wrapV: 0\n  mipmaps:\n    enableMipMap: 1\n    sRGBTexture: 1\n",
    );
    let (materials, report) = VrmMaterials::load(
        &root,
        &root.join("Assets/Model.fbx"),
        &MaterialOptions::new(),
    )
    .expect("対象外でも変換は続ける");
    let 警告 = report.warnings().join("\n");
    assert!(
        警告.contains("PSDに有効な合成画像がないため省く"),
        "合成画像なしの警告がある: {警告}"
    );
    assert!(
        警告.contains("Maximize Compatibility"),
        "再保存の案内がある: {警告}"
    );
    let 所有 = materials.expect("ある");
    assert_eq!(所有.images().len(), 0);
}

/// 追加チャンネルのPSDはRGBAとして扱わず省く。
#[test]
fn 追加チャンネルのpsdは省く() {
    let (_guard, root, _tex) = psd配置を作る(
        &psdを組み立てる(2, 2, 4, 3, 8, 1, &[], 0, &[7; 16]),
        "  textureSettings:\n    filterMode: 1\n    wrapU: 0\n    wrapV: 0\n  mipmaps:\n    enableMipMap: 1\n    sRGBTexture: 1\n",
    );
    let (materials, report) = VrmMaterials::load(
        &root,
        &root.join("Assets/Model.fbx"),
        &MaterialOptions::new(),
    )
    .expect("対象外でも変換は続ける");
    let 警告 = report.warnings().join("\n");
    assert!(
        警告.contains("PSDのチャンネル数が未対応のため省く"),
        "追加チャンネルの警告がある: {警告}"
    );
    let 所有 = materials.expect("ある");
    assert_eq!(所有.images().len(), 0);
}

/// 16bitのPSDは警告して省く。
#[test]
fn 高深度のpsdは省く() {
    let (_guard, root, _tex) = psd配置を作る(
        &psdを組み立てる(2, 2, 3, 3, 16, 1, &[], 0, &[0; 24]),
        "  textureSettings:\n    filterMode: 1\n    wrapU: 0\n    wrapV: 0\n  mipmaps:\n    enableMipMap: 1\n    sRGBTexture: 1\n",
    );
    let (materials, report) = VrmMaterials::load(
        &root,
        &root.join("Assets/Model.fbx"),
        &MaterialOptions::new(),
    )
    .expect("対象外でも変換は続ける");
    let 警告 = report.warnings().join("\n");
    assert!(
        警告.contains("PSDの色深度が未対応のため省く"),
        "深度の警告がある: {警告}"
    );
    let 所有 = materials.expect("ある");
    assert_eq!(所有.images().len(), 0);
}

/// PSDの画素上限は一致で通り1超過で失敗する。
#[test]
fn psdの画素上限は一致で通り超過で失敗する() {
    let 内容 = rgbのpsdを作る([[1, 2, 3], [4, 5, 6], [7, 8, 9], [10, 11, 12]]);
    let (_guard, root, _tex) = psd配置を作る(
        &内容,
        "  textureSettings:\n    filterMode: 1\n    wrapU: 0\n    wrapV: 0\n  mipmaps:\n    enableMipMap: 1\n    sRGBTexture: 1\n",
    );
    VrmMaterials::load(
        &root,
        &root.join("Assets/Model.fbx"),
        &MaterialOptions::new().with_max_image_pixels(4),
    )
    .expect("上限一致は通る");
    let 失敗 = VrmMaterials::load(
        &root,
        &root.join("Assets/Model.fbx"),
        &MaterialOptions::new().with_max_image_pixels(3),
    )
    .expect_err("1超過は失敗する");
    assert_eq!(
        失敗.kind(),
        vrctom::vrm::MaterialErrorKind::LimitExceeded,
        "{失敗}"
    );
}

/// PSDの入力バイト上限と出力合計上限は失敗する。
#[test]
fn psdのバイト上限超過は失敗する() {
    let 内容 = rgbのpsdを作る([[1, 2, 3], [4, 5, 6], [7, 8, 9], [10, 11, 12]]);
    let 長さ = 内容.len() as u64;
    let (_guard, root, _tex) = psd配置を作る(
        &内容,
        "  textureSettings:\n    filterMode: 1\n    wrapU: 0\n    wrapV: 0\n  mipmaps:\n    enableMipMap: 1\n    sRGBTexture: 1\n",
    );
    // 入力上限の一致は通り、1不足は失敗する。
    VrmMaterials::load(
        &root,
        &root.join("Assets/Model.fbx"),
        &MaterialOptions::new().with_max_image_bytes(長さ),
    )
    .expect("入力上限一致は通る");
    let 失敗 = VrmMaterials::load(
        &root,
        &root.join("Assets/Model.fbx"),
        &MaterialOptions::new().with_max_image_bytes(長さ - 1),
    )
    .expect_err("入力1不足は失敗する");
    assert_eq!(
        失敗.kind(),
        vrctom::vrm::MaterialErrorKind::LimitExceeded,
        "{失敗}"
    );
    // 出力合計が残り予算に収まらない場合はPNG符号化で失敗する。
    let 失敗 = VrmMaterials::load(
        &root,
        &root.join("Assets/Model.fbx"),
        &MaterialOptions::new().with_max_total_image_bytes(10),
    )
    .expect_err("出力超過は失敗する");
    assert_eq!(
        失敗.kind(),
        vrctom::vrm::MaterialErrorKind::LimitExceeded,
        "{失敗}"
    );
}

/// PSDを主入力にする焼き（アルファマスク）が実行できる。
#[test]
fn psdの焼き込みはマスクを反映する() {
    let (_guard, root) = 作業域を作る();
    let mat = guid(0x11);
    let 主 = guid(0x22);
    let 覆 = guid(0x33);
    fbxメタを書く(&root, &guid(0x99), &[("Body", Some(&mat))]);
    let 主画素 = [
        [200, 100, 50],
        [200, 100, 50],
        [200, 100, 50],
        [200, 100, 50],
    ];
    psdテクスチャを書く(
        &root,
        "Assets/Textures/Main.psd",
        &主,
        &rgbのpsdを作る(主画素),
        "  textureSettings:\n    filterMode: 1\n    wrapU: 0\n    wrapV: 0\n  mipmaps:\n    enableMipMap: 1\n    sRGBTexture: 1\n",
    );
    // マスクはRだけを使い、アルファ置換（方式1）で載せる。
    let 覆平面 = vec![0, 64, 128, 255];
    psdテクスチャを書く(
        &root,
        "Assets/Textures/Mask.psd",
        &覆,
        &psdを組み立てる(2, 2, 1, 1, 8, 1, &[], 0, &覆平面),
        "  textureSettings:\n    filterMode: 1\n    wrapU: 0\n    wrapV: 0\n  mipmaps:\n    enableMipMap: 1\n    sRGBTexture: 0\n",
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
            ("_MainTex", 参照::割当(&主)),
            ("_AlphaMask", 参照::割当(&覆)),
        ],
        &浮動,
        &基本色(),
    );
    let (materials, report) = VrmMaterials::load(
        &root,
        &root.join("Assets/Model.fbx"),
        &MaterialOptions::new(),
    )
    .expect("PSDの焼きは実行できる");
    assert!(
        report.warnings().is_empty(),
        "成功したPSD変換は警告にしない: {:?}",
        report.warnings()
    );
    let 所有 = materials.expect("ある");
    let 材質 = 所有.get("Body").expect("ある");
    let 索引 = 材質.base_color_image.expect("焼いた基準色は載る");
    assert_eq!(所有.images()[索引].name, "bake:Body:BaseColor");
    let (幅, 高さ, 復号) = pngを復号する(&所有.images()[索引].data);
    assert_eq!((幅, 高さ), (2, 2));
    // RGBは主入力、マスク方式1の置換でアルファはマスクのRである。
    // 主入力はsRGB標識のため復号で線形化されるが、焼き出力はsRGBへ戻す。
    // 200・100・50はsRGBの往復で量子化以内に収まる。
    let 期待 = [
        [200, 100, 50, 0],
        [200, 100, 50, 64],
        [200, 100, 50, 128],
        [200, 100, 50, 255],
    ];
    let 実際: Vec<[u8; 4]> = 復号
        .chunks_exact(4)
        .map(|p| [p[0], p[1], p[2], p[3]])
        .collect();
    for (得, 想) in 実際.iter().zip(期待.iter()) {
        assert_eq!(得[3], 想[3], "アルファはマスク通り: {実際:?}");
        for ch in 0..3 {
            assert!(
                (i16::from(得[ch]) - i16::from(想[ch])).abs() <= 1,
                "色は量子化以内: {実際:?}"
            );
        }
    }
}

/// 小さい保持予算で退避が起きてもPSDの焼き出力は同じになる。
#[test]
fn 小さい保持予算でもpsdの焼き出力は同じになる() {
    // 焼きを2回（基準色と陰影の主入力）起こし、予算1画素で毎回退避させる。
    let 配置 = |予算: u64| {
        let (_guard, root) = 作業域を作る();
        let mat = guid(0x11);
        let 主 = guid(0x22);
        let 覆 = guid(0x33);
        fbxメタを書く(&root, &guid(0x99), &[("Body", Some(&mat))]);
        let 主画素 = [[200, 100, 50], [10, 20, 30], [40, 50, 60], [70, 80, 90]];
        psdテクスチャを書く(
            &root,
            "Assets/Textures/Main.psd",
            &主,
            &rgbのpsdを作る(主画素),
            "  textureSettings:\n    filterMode: 1\n    wrapU: 0\n    wrapV: 0\n  mipmaps:\n    enableMipMap: 1\n    sRGBTexture: 1\n",
        );
        let 覆平面 = vec![0, 64, 128, 255];
        psdテクスチャを書く(
            &root,
            "Assets/Textures/Mask.psd",
            &覆,
            &psdを組み立てる(2, 2, 1, 1, 8, 1, &[], 0, &覆平面),
            "  textureSettings:\n    filterMode: 1\n    wrapU: 0\n    wrapV: 0\n  mipmaps:\n    enableMipMap: 1\n    sRGBTexture: 0\n",
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
                ("_MainTex", 参照::割当(&主)),
                ("_AlphaMask", 参照::割当(&覆)),
            ],
            &浮動,
            &基本色(),
        );
        let (materials, _) = VrmMaterials::load(
            &root,
            &root.join("Assets/Model.fbx"),
            &MaterialOptions::new().with_max_total_image_pixels(予算),
        )
        .expect("予算が小さくても失敗しない");
        let 所有 = materials.expect("ある");
        let 一覧: Vec<Vec<u8>> = 所有
            .images()
            .iter()
            .map(|image| image.data.clone())
            .collect();
        (_guard, 一覧)
    };
    let (_保, 通常) = 配置(u64::MAX);
    let (_退, 退避) = 配置(1);
    assert_eq!(通常, 退避, "退避しても出力は同じになる");
}

/// MatCapマスクだけ・リム無効・MainStrength 0はマスクを直載せし焼かない。
#[test]
fn マスクだけなら直載せし焼かない() {
    // MatCap本体とマスクと主テクスチャを別のGUIDにし、直載せ先を名前で確かめる。
    let (_guard, root) = 作業域を作る();
    let mat = guid(0x11);
    let main = guid(0x22);
    let 本体 = guid(0x23);
    let mask = guid(0x24);
    fbxメタを書く(&root, &guid(0x99), &[("Body", Some(&mat))]);
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
        &[
            ("_MainTex", 参照::割当(&main)),
            ("_MatCapTex", 参照::割当(&本体)),
            ("_MatCapBlendMask", 参照::割当(&mask)),
        ],
        &浮動,
        &基本色(),
    );
    テクスチャを書く(&root, "Assets/Textures/Main.png", &main, PNG_1X1);
    テクスチャを書く(&root, "Assets/Textures/MatCap.png", &本体, PNG_1X1);
    テクスチャを書く(&root, "Assets/Textures/Mask.png", &mask, PNG_1X1);
    let (materials, report) = VrmMaterials::load(
        &root,
        &root.join("Assets/Model.fbx"),
        &MaterialOptions::new(),
    )
    .expect("読める");
    let 所有 = materials.expect("ある");
    let mtoon = 所有.get("Body").expect("ある").mtoon.clone().expect("ある");
    // 焼き画像は増えず、マスク画像をそのまま指す。
    assert_eq!(所有.images().len(), 3, "直載せ3枚で焼きは無い");
    assert!(
        所有
            .images()
            .iter()
            .all(|image| !image.name.starts_with("bake:")),
        "焼き画像は無い"
    );
    let 索引 = mtoon.rim_multiply_image.expect("マスクを載せる");
    assert_eq!(所有.images()[索引].name, "Mask");
    assert_eq!(mtoon.matcap_factor, [1.0, 1.0, 1.0]);
    assert!(
        !report.warnings().iter().any(|警告| 警告.contains("焼")),
        "焼きの警告は無い: {:?}",
        report.warnings()
    );
}

/// MatCapマスク＋MainStrength 1はアルベドtintを焼き、係数は最大値になる。
#[test]
fn 主強度のtintは焼いて係数は最大値になる() {
    // 主テクスチャの模様と白マスクの積が焼き出力になる。
    let (_guard, root) = 作業域を作る();
    let mat = guid(0x11);
    let main = guid(0x22);
    let 本体 = guid(0x23);
    let mask = guid(0x24);
    fbxメタを書く(&root, &guid(0x99), &[("Body", Some(&mat))]);
    let mut 浮動 = 基本浮動小数();
    for (名, 値) in &mut 浮動 {
        if *名 == "_UseMatCap" {
            *値 = 1.0;
        }
    }
    浮動.push(("_MatCapMainStrength", 1.0));
    マテリアルを書く(
        &root,
        "Assets/Materials/Body.mat",
        &mat,
        "Body",
        Some(SHADER_OPAQUE),
        &[
            ("_MainTex", 参照::割当(&main)),
            ("_MatCapTex", 参照::割当(&本体)),
            ("_MatCapBlendMask", 参照::割当(&mask)),
        ],
        &浮動,
        &基本色(),
    );
    let 主 = 模様pngを作る(2, 2);
    テクスチャを書く(&root, "Assets/Textures/Main.png", &main, &主);
    テクスチャを書く(&root, "Assets/Textures/MatCap.png", &本体, PNG_1X1);
    テクスチャを書く(
        &root,
        "Assets/Textures/Mask.png",
        &mask,
        &単色pngを作る(2, 2, [0xFF, 0xFF, 0xFF, 0xFF]),
    );
    let (materials, report) = VrmMaterials::load(
        &root,
        &root.join("Assets/Model.fbx"),
        &MaterialOptions::new(),
    )
    .expect("読める");
    let 所有 = materials.expect("ある");
    let mtoon = 所有.get("Body").expect("ある").mtoon.clone().expect("ある");
    // tintの最大値は1のため、係数は重みのままになる。
    assert_eq!(mtoon.matcap_factor, [1.0, 1.0, 1.0]);
    let 索引 = mtoon.rim_multiply_image.expect("焼いて載せる");
    assert_eq!(所有.images()[索引].name, "bake:Body:RimMultiply");
    // 白マスク×主テクスチャの線形値であり、符号化の往復以内で一致する。
    let (幅, 高さ, 復号) = pngを復号する(&所有.images()[索引].data);
    assert_eq!((幅, 高さ), (2, 2));
    let (_, _, 主画素) = pngを復号する(&主);
    for (実測, 目標) in 復号.chunks_exact(4).zip(主画素.chunks_exact(4)) {
        for (実測, 目標) in 実測.iter().zip(目標.iter()) {
            assert!(
                (i16::from(*実測) - i16::from(*目標)).abs() <= 2,
                "画素が合う: {復号:?} と {主画素:?}"
            );
        }
    }
    assert!(
        report
            .warnings()
            .iter()
            .any(|警告| 警告.contains("アルベドtint")),
        "tintの警告がある: {:?}",
        report.warnings()
    );
}

/// MatCapマスク＋リム有効（色テクスチャなし）は直載せし共有を警告する。
#[test]
fn リム有効でもマスクは直載せし共有を警告する() {
    let (_guard, root) = 作業域を作る();
    let mat = guid(0x11);
    let main = guid(0x22);
    let 本体 = guid(0x23);
    let mask = guid(0x24);
    fbxメタを書く(&root, &guid(0x99), &[("Body", Some(&mat))]);
    let mut 浮動 = 基本浮動小数();
    for (名, 値) in &mut 浮動 {
        if *名 == "_UseMatCap" {
            *値 = 1.0;
        }
    }
    浮動.push(("_UseRim", 1.0));
    マテリアルを書く(
        &root,
        "Assets/Materials/Body.mat",
        &mat,
        "Body",
        Some(SHADER_OPAQUE),
        &[
            ("_MainTex", 参照::割当(&main)),
            ("_MatCapTex", 参照::割当(&本体)),
            ("_MatCapBlendMask", 参照::割当(&mask)),
        ],
        &浮動,
        &基本色(),
    );
    テクスチャを書く(&root, "Assets/Textures/Main.png", &main, PNG_1X1);
    テクスチャを書く(&root, "Assets/Textures/MatCap.png", &本体, PNG_1X1);
    テクスチャを書く(&root, "Assets/Textures/Mask.png", &mask, PNG_1X1);
    let (materials, report) = VrmMaterials::load(
        &root,
        &root.join("Assets/Model.fbx"),
        &MaterialOptions::new(),
    )
    .expect("読める");
    let 所有 = materials.expect("ある");
    let mtoon = 所有.get("Body").expect("ある").mtoon.clone().expect("ある");
    // リムにテクスチャが無いため、マスクのまま載る。
    let 索引 = mtoon.rim_multiply_image.expect("マスクを載せる");
    assert_eq!(所有.images()[索引].name, "Mask");
    // リム色は既定（線形の代替値）のまま係数になる。
    assert!((mtoon.parametric_rim_color_factor[0] - 0.66).abs() < 1e-6);
    assert!((mtoon.parametric_rim_color_factor[1] - 0.5).abs() < 1e-6);
    assert!((mtoon.parametric_rim_color_factor[2] - 0.48).abs() < 1e-6);
    assert!(
        report
            .warnings()
            .iter()
            .any(|警告| 警告.contains("リム光にもMatCapのマスクが掛かる")),
        "共有の警告がある: {:?}",
        report.warnings()
    );
}

/// MatCapマスク＋リム色テクスチャは2枚を1枚に焼き共有を警告する。
#[test]
fn リム色とマスクは積で1枚に焼く() {
    let (_guard, root) = 作業域を作る();
    let mat = guid(0x11);
    let main = guid(0x22);
    let 本体 = guid(0x23);
    let mask = guid(0x24);
    let rim = guid(0x25);
    fbxメタを書く(&root, &guid(0x99), &[("Body", Some(&mat))]);
    let mut 浮動 = 基本浮動小数();
    for (名, 値) in &mut 浮動 {
        if *名 == "_UseMatCap" {
            *値 = 1.0;
        }
    }
    浮動.push(("_UseRim", 1.0));
    マテリアルを書く(
        &root,
        "Assets/Materials/Body.mat",
        &mat,
        "Body",
        Some(SHADER_OPAQUE),
        &[
            ("_MainTex", 参照::割当(&main)),
            ("_MatCapTex", 参照::割当(&本体)),
            ("_MatCapBlendMask", 参照::割当(&mask)),
            ("_RimColorTex", 参照::割当(&rim)),
        ],
        &浮動,
        &基本色(),
    );
    テクスチャを書く(&root, "Assets/Textures/Main.png", &main, PNG_1X1);
    テクスチャを書く(&root, "Assets/Textures/MatCap.png", &本体, PNG_1X1);
    // どちらも128の灰色（線形で約0.216）のため、積は約0.047（符号化で約61）になる。
    テクスチャを書く(
        &root,
        "Assets/Textures/Mask.png",
        &mask,
        &単色pngを作る(2, 2, [0x80, 0x80, 0x80, 0xFF]),
    );
    テクスチャを書く(
        &root,
        "Assets/Textures/Rim.png",
        &rim,
        &単色pngを作る(2, 2, [0x80, 0x80, 0x80, 0xFF]),
    );
    let (materials, report) = VrmMaterials::load(
        &root,
        &root.join("Assets/Model.fbx"),
        &MaterialOptions::new(),
    )
    .expect("読める");
    let 所有 = materials.expect("ある");
    let mtoon = 所有.get("Body").expect("ある").mtoon.clone().expect("ある");
    let 索引 = mtoon.rim_multiply_image.expect("焼いて載せる");
    assert_eq!(所有.images()[索引].name, "bake:Body:RimMultiply");
    let (幅, 高さ, 復号) = pngを復号する(&所有.images()[索引].data);
    assert_eq!((幅, 高さ), (2, 2));
    assert!(
        復号
            .chunks_exact(4)
            .all(|画素| (i16::from(画素[0]) - 61).abs() <= 3
                && (i16::from(画素[1]) - 61).abs() <= 3
                && (i16::from(画素[2]) - 61).abs() <= 3),
        "積になる: {復号:?}"
    );
    let 警告 = report.warnings().join("\n");
    assert!(
        警告.contains("MatCapにもリム光のテクスチャが掛かる"),
        "共有の警告がある: {警告}"
    );
    assert!(
        警告.contains("リム光にもMatCapのマスクが掛かる"),
        "共有の警告がある: {警告}"
    );
}

/// 乗算＋マスクのMatCapは書き出さず係数もゼロになる。
#[test]
fn 乗算マスクのmatcapは書き出さない() {
    let (_guard, root) = 作業域を作る();
    let mat = guid(0x11);
    let main = guid(0x22);
    let 本体 = guid(0x23);
    let mask = guid(0x24);
    fbxメタを書く(&root, &guid(0x99), &[("Body", Some(&mat))]);
    let mut 浮動 = 基本浮動小数();
    for (名, 値) in &mut 浮動 {
        if *名 == "_UseMatCap" {
            *値 = 1.0;
        }
    }
    浮動.push(("_MatCapBlendMode", 3.0));
    マテリアルを書く(
        &root,
        "Assets/Materials/Body.mat",
        &mat,
        "Body",
        Some(SHADER_OPAQUE),
        &[
            ("_MainTex", 参照::割当(&main)),
            ("_MatCapTex", 参照::割当(&本体)),
            ("_MatCapBlendMask", 参照::割当(&mask)),
        ],
        &浮動,
        &基本色(),
    );
    テクスチャを書く(&root, "Assets/Textures/Main.png", &main, PNG_1X1);
    テクスチャを書く(&root, "Assets/Textures/MatCap.png", &本体, PNG_1X1);
    テクスチャを書く(&root, "Assets/Textures/Mask.png", &mask, PNG_1X1);
    let (materials, report) = VrmMaterials::load(
        &root,
        &root.join("Assets/Model.fbx"),
        &MaterialOptions::new(),
    )
    .expect("読める");
    let 所有 = materials.expect("ある");
    let mtoon = 所有.get("Body").expect("ある").mtoon.clone().expect("ある");
    assert_eq!(mtoon.matcap_image, None);
    assert_eq!(mtoon.matcap_factor, [0.0, 0.0, 0.0]);
    assert_eq!(mtoon.rim_multiply_image, None);
    assert!(
        report.warnings().iter().any(|警告| 警告.contains("乗算")),
        "乗算の警告がある: {:?}",
        report.warnings()
    );
}

/// マスクなしNormalの減光は基準色と影色を暗くして足し戻す。
#[test]
fn マスクなしnormalの減光は下の色を暗くする() {
    let (_guard, root) = 作業域を作る();
    let mat = guid(0x11);
    let main = guid(0x22);
    let 本体 = guid(0x23);
    fbxメタを書く(&root, &guid(0x99), &[("Body", Some(&mat))]);
    let mut 浮動 = 基本浮動小数();
    for (名, 値) in &mut 浮動 {
        if *名 == "_UseMatCap" {
            *値 = 1.0;
        }
    }
    浮動.push(("_MatCapBlendMode", 0.0));
    浮動.push(("_MatCapBlend", 0.5));
    マテリアルを書く(
        &root,
        "Assets/Materials/Body.mat",
        &mat,
        "Body",
        Some(SHADER_OPAQUE),
        &[
            ("_MainTex", 参照::割当(&main)),
            ("_MatCapTex", 参照::割当(&本体)),
        ],
        &浮動,
        &基本色(),
    );
    テクスチャを書く(&root, "Assets/Textures/Main.png", &main, PNG_1X1);
    テクスチャを書く(&root, "Assets/Textures/MatCap.png", &本体, PNG_1X1);
    let (materials, report) = VrmMaterials::load(
        &root,
        &root.join("Assets/Model.fbx"),
        &MaterialOptions::new(),
    )
    .expect("読める");
    let 所有 = materials.expect("ある");
    let 材質 = 所有.get("Body").expect("ある");
    let mtoon = 材質.mtoon.clone().expect("ある");
    // 白の半分が下に残り、MatCap係数も重みのままになる。
    assert_eq!(材質.base_color_factor, [0.5, 0.5, 0.5, 1.0]);
    assert_eq!(mtoon.matcap_factor, [0.5, 0.5, 0.5]);
    // 影色の既定値（線形で約0.638・0.538・0.692）の半分になる。
    assert!(
        (mtoon.shade_color_factor[0] - 0.319).abs() < 0.005,
        "{:?}",
        mtoon.shade_color_factor
    );
    assert!(
        (mtoon.shade_color_factor[1] - 0.269).abs() < 0.005,
        "{:?}",
        mtoon.shade_color_factor
    );
    assert!(
        (mtoon.shade_color_factor[2] - 0.346).abs() < 0.005,
        "{:?}",
        mtoon.shade_color_factor
    );
    assert!(
        report.warnings().iter().any(|警告| 警告.contains("暗くし")),
        "減光の警告がある: {:?}",
        report.warnings()
    );
}

/// 重み1超のマスクは明るさをテクスチャへ移して係数を丸める。
#[test]
fn 重み1超のマスクは明るさを移して焼く() {
    let (_guard, root) = 作業域を作る();
    let mat = guid(0x11);
    let main = guid(0x22);
    let 本体 = guid(0x23);
    let mask = guid(0x24);
    fbxメタを書く(&root, &guid(0x99), &[("Body", Some(&mat))]);
    let mut 浮動 = 基本浮動小数();
    for (名, 値) in &mut 浮動 {
        if *名 == "_UseMatCap" {
            *値 = 1.0;
        }
    }
    浮動.push(("_MatCapBlend", 2.0));
    マテリアルを書く(
        &root,
        "Assets/Materials/Body.mat",
        &mat,
        "Body",
        Some(SHADER_OPAQUE),
        &[
            ("_MainTex", 参照::割当(&main)),
            ("_MatCapTex", 参照::割当(&本体)),
            ("_MatCapBlendMask", 参照::割当(&mask)),
        ],
        &浮動,
        &基本色(),
    );
    テクスチャを書く(
        &root,
        "Assets/Textures/Main.png",
        &main,
        &単色pngを作る(1, 1, [0xFF, 0xFF, 0xFF, 0xFF]),
    );
    テクスチャを書く(
        &root,
        "Assets/Textures/MatCap.png",
        &本体,
        &単色pngを作る(1, 1, [0xFF, 0xFF, 0xFF, 0xFF]),
    );
    テクスチャを書く(
        &root,
        "Assets/Textures/Mask.png",
        &mask,
        &単色pngを作る(1, 1, [0xFF, 0xFF, 0xFF, 0xFF]),
    );
    let (materials, report) = VrmMaterials::load(
        &root,
        &root.join("Assets/Model.fbx"),
        &MaterialOptions::new(),
    )
    .expect("読める");
    let 所有 = materials.expect("ある");
    let mtoon = 所有.get("Body").expect("ある").mtoon.clone().expect("ある");
    // 明るさ2は係数に入らず、1に丸めた係数と上限で切った焼きになる。
    assert_eq!(mtoon.matcap_factor, [1.0, 1.0, 1.0]);
    let 索引 = mtoon.rim_multiply_image.expect("焼いて載せる");
    assert_eq!(所有.images()[索引].name, "bake:Body:RimMultiply");
    let (幅, 高さ, 復号) = pngを復号する(&所有.images()[索引].data);
    assert_eq!((幅, 高さ), (1, 1));
    assert_eq!(復号, vec![0xFF, 0xFF, 0xFF, 0xFF]);
    assert!(
        report
            .warnings()
            .iter()
            .any(|警告| 警告.contains("上限で切る")),
        "明るさの警告がある: {:?}",
        report.warnings()
    );
}

/// 同じ入力のリム倍率焼きは同じ画像索引を共有する。
#[test]
fn 同じ入力の焼きは画像索引を共有する() {
    // 2スロットで同じGUID・同じ値を参照し、内容ハッシュで1枚にまとめる。
    let (_guard, root) = 作業域を作る();
    let a = guid(0x11);
    let b = guid(0x12);
    let main = guid(0x22);
    let 本体 = guid(0x23);
    let mask = guid(0x24);
    fbxメタを書く(&root, &guid(0x99), &[("A", Some(&a)), ("B", Some(&b))]);
    for mat in [&a, &b] {
        let mut 浮動 = 基本浮動小数();
        for (名, 値) in &mut 浮動 {
            if *名 == "_UseMatCap" {
                *値 = 1.0;
            }
        }
        浮動.push(("_MatCapMainStrength", 1.0));
        マテリアルを書く(
            &root,
            &format!("Assets/Materials/{mat}.mat"),
            mat,
            mat,
            Some(SHADER_OPAQUE),
            &[
                ("_MainTex", 参照::割当(&main)),
                ("_MatCapTex", 参照::割当(&本体)),
                ("_MatCapBlendMask", 参照::割当(&mask)),
            ],
            &浮動,
            &基本色(),
        );
        メタを書く(
            &root,
            &format!("Assets/Materials/{mat}.mat"),
            mat,
            "NativeFormatImporter",
            "  externalObjects: {}\n  userData: \n",
        );
    }
    テクスチャを書く(
        &root,
        "Assets/Textures/Main.png",
        &main,
        &単色pngを作る(1, 1, [0xFF, 0xFF, 0xFF, 0xFF]),
    );
    テクスチャを書く(&root, "Assets/Textures/MatCap.png", &本体, PNG_1X1);
    テクスチャを書く(
        &root,
        "Assets/Textures/Mask.png",
        &mask,
        &単色pngを作る(1, 1, [0xFF, 0xFF, 0xFF, 0xFF]),
    );
    let (materials, _) = VrmMaterials::load(
        &root,
        &root.join("Assets/Model.fbx"),
        &MaterialOptions::new(),
    )
    .expect("読める");
    let 所有 = materials.expect("ある");
    let 甲 = 所有.get("A").expect("ある").mtoon.clone().expect("ある");
    let 乙 = 所有.get("B").expect("ある").mtoon.clone().expect("ある");
    let 甲焼き = 甲.rim_multiply_image.expect("焼いて載せる");
    let 乙焼き = 乙.rim_multiply_image.expect("焼いて載せる");
    assert_eq!(甲焼き, 乙焼き, "同じ内容は同じ索引になる");
    assert_eq!(
        所有
            .images()
            .iter()
            .filter(|image| image.name.starts_with("bake:"))
            .count(),
        1,
        "焼き画像は1枚だけ増える"
    );
}

/// 本体画像が無いMatCapは減光も書き出しもしない。
#[test]
fn 本体が無いmatcapは減光も書き出しもしない() {
    // `_MatCapTex` は割り当てるが資産を書かない。索引に無いため省く。
    let (_guard, root) = 作業域を作る();
    let mat = guid(0x11);
    let main = guid(0x22);
    fbxメタを書く(&root, &guid(0x99), &[("Body", Some(&mat))]);
    let mut 浮動 = 基本浮動小数();
    for (名, 値) in &mut 浮動 {
        if *名 == "_UseMatCap" {
            *値 = 1.0;
        }
    }
    浮動.push(("_MatCapBlendMode", 0.0));
    浮動.push(("_MatCapBlend", 0.5));
    マテリアルを書く(
        &root,
        "Assets/Materials/Body.mat",
        &mat,
        "Body",
        Some(SHADER_OPAQUE),
        &[
            ("_MainTex", 参照::割当(&main)),
            ("_MatCapTex", 参照::割当(&guid(0x30))),
        ],
        &浮動,
        &基本色(),
    );
    テクスチャを書く(&root, "Assets/Textures/Main.png", &main, PNG_1X1);
    let (materials, report) = VrmMaterials::load(
        &root,
        &root.join("Assets/Model.fbx"),
        &MaterialOptions::new(),
    )
    .expect("読める");
    let 所有 = materials.expect("ある");
    let 材質 = 所有.get("Body").expect("ある");
    let mtoon = 材質.mtoon.clone().expect("ある");
    // 本体が無いため下の色は暗くならず、MatCapも載らない。
    assert_eq!(材質.base_color_factor, [1.0, 1.0, 1.0, 1.0]);
    assert!(
        (mtoon.shade_color_factor[0] - 0.638).abs() < 0.005,
        "{:?}",
        mtoon.shade_color_factor
    );
    assert!(
        (mtoon.shade_color_factor[1] - 0.538).abs() < 0.005,
        "{:?}",
        mtoon.shade_color_factor
    );
    assert!(
        (mtoon.shade_color_factor[2] - 0.692).abs() < 0.005,
        "{:?}",
        mtoon.shade_color_factor
    );
    assert_eq!(mtoon.matcap_image, None);
    assert_eq!(mtoon.matcap_factor, [0.0, 0.0, 0.0]);
    assert_eq!(mtoon.rim_multiply_image, None);
    let 警告 = report.warnings().join("\n");
    assert!(!警告.contains("暗くし"), "減光しない: {警告}");
}

/// 本体が無いAddではマスクだけをリム倍率に載せない。
#[test]
fn 本体が無いaddではマスクだけを載せない() {
    // 本体は欠落、マスクだけある。Addでもマスク単独では載せない。
    let (_guard, root) = 作業域を作る();
    let mat = guid(0x11);
    let main = guid(0x22);
    let mask = guid(0x24);
    fbxメタを書く(&root, &guid(0x99), &[("Body", Some(&mat))]);
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
        &[
            ("_MainTex", 参照::割当(&main)),
            ("_MatCapTex", 参照::割当(&guid(0x30))),
            ("_MatCapBlendMask", 参照::割当(&mask)),
        ],
        &浮動,
        &基本色(),
    );
    テクスチャを書く(&root, "Assets/Textures/Main.png", &main, PNG_1X1);
    テクスチャを書く(
        &root,
        "Assets/Textures/Mask.png",
        &mask,
        &単色pngを作る(1, 1, [0xFF, 0xFF, 0xFF, 0xFF]),
    );
    let (materials, report) = VrmMaterials::load(
        &root,
        &root.join("Assets/Model.fbx"),
        &MaterialOptions::new(),
    )
    .expect("読める");
    let 所有 = materials.expect("ある");
    let mtoon = 所有.get("Body").expect("ある").mtoon.clone().expect("ある");
    assert_eq!(mtoon.matcap_image, None);
    assert_eq!(mtoon.matcap_factor, [0.0, 0.0, 0.0]);
    assert_eq!(mtoon.rim_multiply_image, None);
    let 警告 = report.warnings().join("\n");
    assert!(
        !警告.contains("1枚のテクスチャで乗算"),
        "共有しない: {警告}"
    );
}

/// tintのアルベド色は線形の基準色になる。
#[test]
fn tintのアルベド色は線形基準色になる() {
    // `_Color` 0.5（線形で約0.214）に白マスクを掛け、符号化で約128になる。
    let (_guard, root) = 作業域を作る();
    let mat = guid(0x11);
    let main = guid(0x22);
    let 本体 = guid(0x23);
    let mask = guid(0x24);
    fbxメタを書く(&root, &guid(0x99), &[("Body", Some(&mat))]);
    let mut 浮動 = 基本浮動小数();
    for (名, 値) in &mut 浮動 {
        if *名 == "_UseMatCap" {
            *値 = 1.0;
        }
    }
    浮動.push(("_MatCapMainStrength", 1.0));
    let mut 色 = 基本色();
    for (名, 値) in &mut 色 {
        if *名 == "_Color" {
            *値 = [0.5, 0.5, 0.5, 1.0];
        }
    }
    マテリアルを書く(
        &root,
        "Assets/Materials/Body.mat",
        &mat,
        "Body",
        Some(SHADER_OPAQUE),
        &[
            ("_MainTex", 参照::割当(&main)),
            ("_MatCapTex", 参照::割当(&本体)),
            ("_MatCapBlendMask", 参照::割当(&mask)),
        ],
        &浮動,
        &色,
    );
    テクスチャを書く(
        &root,
        "Assets/Textures/Main.png",
        &main,
        &単色pngを作る(1, 1, [0xFF, 0xFF, 0xFF, 0xFF]),
    );
    テクスチャを書く(&root, "Assets/Textures/MatCap.png", &本体, PNG_1X1);
    テクスチャを書く(
        &root,
        "Assets/Textures/Mask.png",
        &mask,
        &単色pngを作る(1, 1, [0xFF, 0xFF, 0xFF, 0xFF]),
    );
    let (materials, _) = VrmMaterials::load(
        &root,
        &root.join("Assets/Model.fbx"),
        &MaterialOptions::new(),
    )
    .expect("読める");
    let 所有 = materials.expect("ある");
    let mtoon = 所有.get("Body").expect("ある").mtoon.clone().expect("ある");
    let 索引 = mtoon.rim_multiply_image.expect("焼いて載せる");
    let (_, _, 復号) = pngを復号する(&所有.images()[索引].data);
    assert!(
        復号
            .chunks_exact(4)
            .all(|画素| (i16::from(画素[0]) - 128).abs() <= 2
                && (i16::from(画素[1]) - 128).abs() <= 2
                && (i16::from(画素[2]) - 128).abs() <= 2),
        "線形基準色で焼く: {復号:?}"
    );
}

/// 焼いた基準色でもtintのアルベド色は線形基準色になる。
#[test]
fn 焼いた基準色でもtintは線形基準色になる() {
    // アルファマスクで基準色を焼く経路でも、白ベイクに線形の `_Color` を掛ける。
    let (_guard, root) = 作業域を作る();
    let mat = guid(0x11);
    let main = guid(0x22);
    let 本体 = guid(0x23);
    let mask = guid(0x24);
    let alpha = guid(0x25);
    fbxメタを書く(&root, &guid(0x99), &[("Body", Some(&mat))]);
    let mut 浮動 = 基本浮動小数();
    for (名, 値) in &mut 浮動 {
        if *名 == "_UseMatCap" {
            *値 = 1.0;
        }
    }
    浮動.push(("_MatCapMainStrength", 1.0));
    浮動.push(("_AlphaMaskMode", 1.0));
    let mut 色 = 基本色();
    for (名, 値) in &mut 色 {
        if *名 == "_Color" {
            *値 = [0.5, 0.5, 0.5, 1.0];
        }
    }
    マテリアルを書く(
        &root,
        "Assets/Materials/Body.mat",
        &mat,
        "Body",
        Some(SHADER_OPAQUE),
        &[
            ("_MainTex", 参照::割当(&main)),
            ("_MatCapTex", 参照::割当(&本体)),
            ("_MatCapBlendMask", 参照::割当(&mask)),
            ("_AlphaMask", 参照::割当(&alpha)),
        ],
        &浮動,
        &色,
    );
    テクスチャを書く(
        &root,
        "Assets/Textures/Main.png",
        &main,
        &単色pngを作る(1, 1, [0xFF, 0xFF, 0xFF, 0xFF]),
    );
    テクスチャを書く(&root, "Assets/Textures/MatCap.png", &本体, PNG_1X1);
    テクスチャを書く(
        &root,
        "Assets/Textures/Mask.png",
        &mask,
        &単色pngを作る(1, 1, [0xFF, 0xFF, 0xFF, 0xFF]),
    );
    テクスチャを書く(
        &root,
        "Assets/Textures/Alpha.png",
        &alpha,
        &単色pngを作る(1, 1, [0xFF, 0xFF, 0xFF, 0xFF]),
    );
    let (materials, _) = VrmMaterials::load(
        &root,
        &root.join("Assets/Model.fbx"),
        &MaterialOptions::new(),
    )
    .expect("読める");
    let 所有 = materials.expect("ある");
    let 材質 = 所有.get("Body").expect("ある");
    // 基準色は焼かれている。
    assert!(材質.base_color_image.is_some());
    let mtoon = 材質.mtoon.clone().expect("ある");
    let 索引 = mtoon.rim_multiply_image.expect("焼いて載せる");
    let (_, _, 復号) = pngを復号する(&所有.images()[索引].data);
    assert!(
        復号
            .chunks_exact(4)
            .all(|画素| (i16::from(画素[0]) - 128).abs() <= 2
                && (i16::from(画素[1]) - 128).abs() <= 2
                && (i16::from(画素[2]) - 128).abs() <= 2),
        "焼いた基準色でも線形基準色で焼く: {復号:?}"
    );
}

/// 非単位変換の単一マスクは直載せせず焼いて変換を反映する。
#[test]
fn 非単位変換のマスクは焼いて反映する() {
    let (_guard, root) = 作業域を作る();
    let mat = guid(0x11);
    let main = guid(0x22);
    let 本体 = guid(0x23);
    let mask = guid(0x24);
    fbxメタを書く(&root, &guid(0x99), &[("Body", Some(&mat))]);
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
        &[
            ("_MainTex", 参照::割当(&main)),
            ("_MatCapTex", 参照::割当(&本体)),
            ("_MatCapBlendMask", 参照::割当(&mask)),
        ],
        &浮動,
        &基本色(),
    );
    // マスクのオフセットを x=0.5 にずらす。
    let 場所 = root.join("Assets/Materials/Body.mat");
    let 本文 = fs::read_to_string(&場所).expect("matは読める");
    let 標的 = format!(
        "    - _MatCapBlendMask:\n        m_Texture: {{fileID: 2800000, guid: {mask}, type: 3}}\n        m_Scale: {{x: 1, y: 1}}\n        m_Offset: {{x: 0, y: 0}}"
    );
    assert!(本文.contains(&標的), "書式が合う");
    fs::write(
        &場所,
        本文
            .replace(&標的, &標的.replace("{x: 0, y: 0}", "{x: 0.5, y: 0}"))
            .as_bytes(),
    )
    .expect("matは書ける");
    テクスチャを書く(&root, "Assets/Textures/Main.png", &main, PNG_1X1);
    テクスチャを書く(&root, "Assets/Textures/MatCap.png", &本体, PNG_1X1);
    // 点補間・繰り返しの2x2模様。x=0.5 のずれで列が入れ替わる。
    資産を書く(&root, "Assets/Textures/Mask.png", &模様pngを作る(2, 2));
    メタを書く(
        &root,
        "Assets/Textures/Mask.png",
        &mask,
        "TextureImporter",
        "  externalObjects: {}\n  textureSettings:\n    filterMode: 0\n    wrapU: 0\n    wrapV: 0\n  mipmaps:\n    enableMipMap: 1\n",
    );
    let (materials, _) = VrmMaterials::load(
        &root,
        &root.join("Assets/Model.fbx"),
        &MaterialOptions::new(),
    )
    .expect("読める");
    let 所有 = materials.expect("ある");
    let mtoon = 所有.get("Body").expect("ある").mtoon.clone().expect("ある");
    let 索引 = mtoon.rim_multiply_image.expect("焼いて載せる");
    assert_eq!(所有.images()[索引].name, "bake:Body:RimMultiply");
    let (幅, 高さ, 復号) = pngを復号する(&所有.images()[索引].data);
    assert_eq!((幅, 高さ), (2, 2));
    // 列が入れ替わる（Rは64・0、Gは行のまま、Bは128）。
    let 期待 = [
        64u8, 0, 128, 255, 0, 0, 128, 255, 64, 64, 128, 255, 0, 64, 128, 255,
    ];
    for (実測, 目標) in 復号.iter().zip(期待.iter()) {
        assert!(
            (i16::from(*実測) - i16::from(*目標)).abs() <= 2,
            "ずらしが反映される: {復号:?}"
        );
    }
}

/// リム色テクスチャの縮小は色として平均する。
#[test]
fn リム色テクスチャの縮小は色として平均する() {
    // 2x1 の [0, 128] を上限1で縮める。符号化済み平均は64、線形平均は約92になる。
    let (_guard, root) = 作業域を作る();
    let mat = guid(0x11);
    let main = guid(0x22);
    let 本体 = guid(0x23);
    let mask = guid(0x24);
    let rim = guid(0x25);
    fbxメタを書く(&root, &guid(0x99), &[("Body", Some(&mat))]);
    let mut 浮動 = 基本浮動小数();
    for (名, 値) in &mut 浮動 {
        if *名 == "_UseMatCap" {
            *値 = 1.0;
        }
    }
    浮動.push(("_UseRim", 1.0));
    マテリアルを書く(
        &root,
        "Assets/Materials/Body.mat",
        &mat,
        "Body",
        Some(SHADER_OPAQUE),
        &[
            ("_MainTex", 参照::割当(&main)),
            ("_MatCapTex", 参照::割当(&本体)),
            ("_MatCapBlendMask", 参照::割当(&mask)),
            ("_RimColorTex", 参照::割当(&rim)),
        ],
        &浮動,
        &基本色(),
    );
    テクスチャを書く(
        &root,
        "Assets/Textures/Main.png",
        &main,
        &単色pngを作る(1, 1, [0xFF, 0xFF, 0xFF, 0xFF]),
    );
    テクスチャを書く(&root, "Assets/Textures/MatCap.png", &本体, PNG_1X1);
    テクスチャを書く(
        &root,
        "Assets/Textures/Mask.png",
        &mask,
        &単色pngを作る(1, 1, [0xFF, 0xFF, 0xFF, 0xFF]),
    );
    // 2x1 の R=G=B=[0, 128] を上限1で縮小する。
    let mut raw = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut raw, 2, 1);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().expect("PNGは書ける");
        writer
            .write_image_data(&[0, 0, 0, 0xFF, 0x80, 0x80, 0x80, 0xFF])
            .expect("画素は書ける");
        drop(writer);
    }
    縮小指定テクスチャを書く(&root, "Assets/Textures/Rim.png", &rim, &raw, 1);
    let (materials, _) = VrmMaterials::load(
        &root,
        &root.join("Assets/Model.fbx"),
        &MaterialOptions::new(),
    )
    .expect("読める");
    let 所有 = materials.expect("ある");
    let mtoon = 所有.get("Body").expect("ある").mtoon.clone().expect("ある");
    let 索引 = mtoon.rim_multiply_image.expect("焼いて載せる");
    let (幅, 高さ, 復号) = pngを復号する(&所有.images()[索引].data);
    assert_eq!((幅, 高さ), (1, 1));
    assert!(
        復号
            .chunks_exact(4)
            .all(|画素| (i16::from(画素[0]) - 64).abs() <= 2
                && (i16::from(画素[1]) - 64).abs() <= 2
                && (i16::from(画素[2]) - 64).abs() <= 2),
        "色として縮小する: {復号:?}"
    );
}

/// 乗算＋マスクなし＋リム色ありは本体を解決せず書き出さない。
#[test]
fn リム色ありの乗算は本体を埋め込まず書き出さない() {
    // 減光で足し戻せないため候補から外し、本体資産があっても埋め込まない。
    let (_guard, root) = 作業域を作る();
    let mat = guid(0x11);
    let main = guid(0x22);
    let rim = guid(0x25);
    fbxメタを書く(&root, &guid(0x99), &[("Body", Some(&mat))]);
    let mut 浮動 = 基本浮動小数();
    for (名, 値) in &mut 浮動 {
        if *名 == "_UseMatCap" {
            *値 = 1.0;
        }
    }
    浮動.push(("_MatCapBlendMode", 3.0));
    浮動.push(("_UseRim", 1.0));
    マテリアルを書く(
        &root,
        "Assets/Materials/Body.mat",
        &mat,
        "Body",
        Some(SHADER_OPAQUE),
        &[
            ("_MainTex", 参照::割当(&main)),
            ("_MatCapTex", 参照::割当(&guid(0x30))),
            ("_RimColorTex", 参照::割当(&rim)),
        ],
        &浮動,
        &基本色(),
    );
    テクスチャを書く(&root, "Assets/Textures/Main.png", &main, PNG_1X1);
    // 本体資産があっても候補から外すため解決・埋め込みしない。
    テクスチャを書く(&root, "Assets/Textures/MatCap.png", &guid(0x30), PNG_1X1);
    テクスチャを書く(
        &root,
        "Assets/Textures/Rim.png",
        &rim,
        &単色pngを作る(1, 1, [0xFF, 0xFF, 0xFF, 0xFF]),
    );
    let (materials, report) = VrmMaterials::load(
        &root,
        &root.join("Assets/Model.fbx"),
        &MaterialOptions::new(),
    )
    .expect("本体が無くても省略して続ける");
    let 所有 = materials.expect("ある");
    let mtoon = 所有.get("Body").expect("ある").mtoon.clone().expect("ある");
    assert_eq!(mtoon.matcap_image, None);
    assert_eq!(mtoon.matcap_factor, [0.0, 0.0, 0.0]);
    assert!(
        !所有.images().iter().any(|image| image.name == "MatCap"),
        "本体を埋め込まない: {:?}",
        所有
            .images()
            .iter()
            .map(|image| &image.name)
            .collect::<Vec<_>>()
    );
    let 警告 = report.warnings().join("\n");
    assert!(
        警告.contains("リムの色テクスチャを共有するため"),
        "原因の警告がある: {警告}"
    );
}

/// 強度0の乗算MatCapはリム倍率に載らず係数もゼロになる。
#[test]
fn 強度0の乗算はリムを暗くしない() {
    // `covers == 0` のため候補から外れ、Multiplyのtintでリムを暗くしない。
    let (_guard, root) = 作業域を作る();
    let mat = guid(0x11);
    let main = guid(0x22);
    let 本体 = guid(0x23);
    fbxメタを書く(&root, &guid(0x99), &[("Body", Some(&mat))]);
    let mut 浮動 = 基本浮動小数();
    for (名, 値) in &mut 浮動 {
        if *名 == "_UseMatCap" {
            *値 = 1.0;
        }
    }
    浮動.push(("_MatCapBlendMode", 3.0));
    浮動.push(("_MatCapBlend", 0.0));
    浮動.push(("_UseRim", 1.0));
    マテリアルを書く(
        &root,
        "Assets/Materials/Body.mat",
        &mat,
        "Body",
        Some(SHADER_OPAQUE),
        &[
            ("_MainTex", 参照::割当(&main)),
            ("_MatCapTex", 参照::割当(&本体)),
        ],
        &浮動,
        &基本色(),
    );
    テクスチャを書く(
        &root,
        "Assets/Textures/Main.png",
        &main,
        &単色pngを作る(1, 1, [0xFF, 0xFF, 0xFF, 0xFF]),
    );
    テクスチャを書く(&root, "Assets/Textures/MatCap.png", &本体, PNG_1X1);
    let (materials, report) = VrmMaterials::load(
        &root,
        &root.join("Assets/Model.fbx"),
        &MaterialOptions::new(),
    )
    .expect("読める");
    let 所有 = materials.expect("ある");
    let mtoon = 所有.get("Body").expect("ある").mtoon.clone().expect("ある");
    assert_eq!(mtoon.matcap_image, None);
    assert_eq!(mtoon.matcap_factor, [0.0, 0.0, 0.0]);
    assert_eq!(mtoon.rim_multiply_image, None);
    let 警告 = report.warnings().join("\n");
    assert!(
        警告.contains("強度が0で色を変えない"),
        "原因の警告がある: {警告}"
    );
}
