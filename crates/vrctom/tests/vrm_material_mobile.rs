//! VRChat SDKのMobile系シェーダの材質読み取りの契約テスト（公開 API のみを使う）。
//!
//! シェーダ本体はアバターのパッケージに含まれないため、既定ではGUID表で判定させる。

#![cfg(not(target_arch = "wasm32"))]

use std::fs;
use std::path::{Path, PathBuf};

use vrctom::vrm::{
    MaterialOptions, MaterialReport, VrmAlphaMode, VrmMaterial, VrmMaterials, VrmOutlineWidthMode,
};

// テスト名は日本語で書く（テストは仕様・契約であるため）。

/// `VRChat/Mobile/Toon Lit` のGUID。
const TOON_LIT: &str = "affc81f3d164d734d8f13053effb1c5c";
/// `VRChat/Mobile/MatCap Lit` のGUID。
const MATCAP_LIT: &str = "3ad043b7f9839cb48a75a9238d433dec";
/// `VRChat/Mobile/Diffuse` のGUID。
const DIFFUSE: &str = "2dcd9e0568e0a6f45b92c60ba2eb16a0";
/// `VRChat/Mobile/Bumped Diffuse` のGUID。
const BUMPED_DIFFUSE: &str = "f8c1f8ac363df824899534a0b30eef00";
/// `VRChat/Mobile/Standard Lite` のGUID。
const STANDARD_LITE: &str = "0b7113dea2069fc4e8943843eff19f70";
/// `VRChat/Mobile/Particles/Alpha Blended` のGUID。
const PARTICLE_ALPHA: &str = "8b39b95ac85682040beff730e0cfc77a";
/// `VRChat/Mobile/Particles/Multiply` のGUID。
const PARTICLE_MULTIPLY: &str = "d5b89f0c74ccf5049ba803c14a090378";
/// `VRChat/Mobile/Toon Standard` のGUID。
const TOON_STANDARD: &str = "e765db0afa7ecfc44ade2e4e2491f65a";
/// `VRChat/Mobile/Toon Standard (Outline)` のGUID。
const TOON_STANDARD_OUTLINE: &str = "051a0ed2f2aedd741aa8186ae92f97e0";
/// `VRChat/Mobile/Skybox` のGUID。
const SKYBOX: &str = "c0d3cb006bb294142bef136f492f2568";
/// SDK付属の `ShadowRampToon2Band.png` のGUID。
const RAMP_TOON_2BAND: &str = "dfafc89321615114fb6dbecdba0c8214";

/// テスト用GUID（32桁hex）を生成する。
fn guid(n: u128) -> String {
    format!("{n:032x}")
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

/// スロット `Body` を材質 `mat` に対応付けるFBXの `.meta` を書く。
fn fbxメタを書く(root: &Path, mat: &str) {
    メタを書く(
        root,
        "Assets/Model.fbx",
        &guid(0x99),
        "ModelImporter",
        &format!(
            "  externalObjects:\n  - first:\n      type: UnityEngine:Material\n      assembly: UnityEngine.CoreModule\n      name: Body\n    second: {{fileID: 2100000, guid: {mat}, type: 2}}\n"
        ),
    );
    資産を書く(root, "Assets/Model.fbx", b"");
}

/// 指定画素の単色RGBA8のPNG（4x4）を作る。
fn 単色pngを作る(画素: [u8; 4]) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, 4, 4);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().expect("PNGヘッダは書ける");
        let pixels = [画素; 16].concat();
        writer.write_image_data(&pixels).expect("PNG画素は書ける");
    }
    out
}

/// テクスチャ資産と `.meta` を書く。sRGB旗は既定（真）のままにする。
fn テクスチャを書く(root: &Path, 相対: &str, guid: &str, 画素: [u8; 4]) {
    資産を書く(root, 相対, &単色pngを作る(画素));
    メタを書く(
        root,
        相対,
        guid,
        "TextureImporter",
        "  externalObjects: {}\n  textureSettings:\n    filterMode: 1\n    wrapU: 0\n    wrapV: 0\n  mipmaps:\n    enableMipMap: 1\n",
    );
}

/// 書き出す `.mat` の中身。
#[derive(Default)]
struct 材質<'a> {
    /// シェーダのGUID。
    シェーダ: &'a str,
    /// `Material` 直下に足すキーワード欄（YAMLの行）。
    キーワード欄: &'a str,
    /// テクスチャ環境（名前、テクスチャGUID）。
    テクスチャ: Vec<(&'a str, &'a str)>,
    /// 浮動小数点プロパティ。
    浮動小数: Vec<(&'a str, f64)>,
    /// 整数プロパティ。
    整数: Vec<(&'a str, i64)>,
    /// 色プロパティ。
    色: Vec<(&'a str, [f64; 4])>,
}

/// `Assets/Materials/Body.mat` を書き、スロット `Body` から引ける配置にする。
fn 材質を書く(root: &Path, 中身: &材質<'_>) {
    let mat = guid(0x11);
    fbxメタを書く(root, &mat);
    let mut 環境 = String::new();
    for (名, guid) in &中身.テクスチャ {
        環境.push_str(&format!(
            "    - {名}:\n        m_Texture: {{fileID: 2800000, guid: {guid}, type: 3}}\n        m_Scale: {{x: 1, y: 1}}\n        m_Offset: {{x: 0, y: 0}}\n"
        ));
    }
    let 一覧 = |中: String| {
        if 中.is_empty() {
            " []\n".to_string()
        } else {
            format!("\n{中}")
        }
    };
    let 浮動 = 中身
        .浮動小数
        .iter()
        .map(|(名, 値)| format!("    - {名}: {値}\n"))
        .collect::<String>();
    let 整数 = 中身
        .整数
        .iter()
        .map(|(名, 値)| format!("    - {名}: {値}\n"))
        .collect::<String>();
    let 色 = 中身
        .色
        .iter()
        .map(|(名, 値)| {
            format!(
                "    - {名}: {{r: {}, g: {}, b: {}, a: {}}}\n",
                値[0], 値[1], 値[2], 値[3]
            )
        })
        .collect::<String>();
    let 本文 = format!(
        "%YAML 1.1\n%TAG !u! tag:unity3d.com,2011:\n--- !u!21 &2100000\nMaterial:\n  m_Name: Body\n  m_Shader: {{fileID: 4800000, guid: {}, type: 3}}\n{}  m_SavedProperties:\n    m_TexEnvs:{}    m_Ints:{}    m_Floats:{}    m_Colors:{}",
        中身.シェーダ,
        中身.キーワード欄,
        一覧(環境),
        一覧(整数),
        一覧(浮動),
        一覧(色),
    );
    資産を書く(root, "Assets/Materials/Body.mat", 本文.as_bytes());
    メタを書く(
        root,
        "Assets/Materials/Body.mat",
        &mat,
        "NativeFormatImporter",
        "  externalObjects: {}\n  userData: \n",
    );
}

/// 配置を読み、スロット `Body` の材質と報告を返す。
fn 読む(root: &Path) -> (VrmMaterial, MaterialReport) {
    let (materials, report) = VrmMaterials::load(
        root,
        &root.join("Assets/Model.fbx"),
        &MaterialOptions::new(),
    )
    .expect("読める");
    let material = materials
        .expect("置換表があるため材質がある")
        .get("Body")
        .expect("スロットで引ける")
        .clone();
    (material, report)
}

/// 作業域を作る。
fn 作業域を作る() -> (tempfile::TempDir, PathBuf) {
    let guard = tempfile::TempDir::new().expect("作業域は作れる");
    let root = guard.path().to_path_buf();
    (guard, root)
}

/// 警告に部分文字列を含むものがあるかどうかを返す。
fn 警告がある(report: &MaterialReport, 部分: &str) -> bool {
    report
        .warnings()
        .iter()
        .any(|warning| warning.contains(部分))
}

fn 近い(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-4
}

#[test]
fn toon_litは既知のguidで判定し陰影を付けない() {
    let (_guard, root) = 作業域を作る();
    let tex = guid(0x22);
    テクスチャを書く(&root, "Assets/Textures/Body.png", &tex, [200, 100, 50, 255]);
    材質を書く(
        &root,
        &材質 {
            シェーダ: TOON_LIT,
            テクスチャ: vec![("_MainTex", &tex)],
            ..Default::default()
        },
    );
    let (material, report) = 読む(&root);
    assert!(
        report.warnings().is_empty(),
        "警告は無い: {:?}",
        report.warnings()
    );
    assert_eq!(material.alpha_mode, VrmAlphaMode::Opaque);
    assert!(!material.double_sided);
    assert_eq!(material.base_color_factor, [1.0, 1.0, 1.0, 1.0]);
    assert_eq!(material.base_color_image, Some(0));
    let mtoon = material.mtoon.as_ref().expect("MToonになる");
    // 光の向きを使わないため、影色は基準色と同じである。
    assert_eq!(mtoon.shade_color_factor, [1.0, 1.0, 1.0]);
    assert_eq!(mtoon.shade_multiply_image, Some(0));
    assert_eq!(mtoon.outline_width_mode, VrmOutlineWidthMode::None);
}

#[test]
fn シェーダ名でもmobile系を判定する() {
    let (_guard, root) = 作業域を作る();
    let shader = guid(0x55);
    資産を書く(
        &root,
        "Assets/Shader/ToonLit.shader",
        b"Shader \"VRChat/Mobile/Toon Lit\"\n{\n}\n",
    );
    メタを書く(
        &root,
        "Assets/Shader/ToonLit.shader",
        &shader,
        "ShaderImporter",
        "  defaultTextures: []\n",
    );
    材質を書く(
        &root,
        &材質 {
            シェーダ: &shader,
            ..Default::default()
        },
    );
    let (material, _) = 読む(&root);
    let mtoon = material.mtoon.as_ref().expect("MToonになる");
    assert_eq!(mtoon.shade_color_factor, [1.0, 1.0, 1.0]);
}

#[test]
fn diffuseはlambertに合わせる() {
    let (_guard, root) = 作業域を作る();
    材質を書く(
        &root,
        &材質 {
            シェーダ: DIFFUSE,
            ..Default::default()
        },
    );
    let (material, _) = 読む(&root);
    let mtoon = material.mtoon.as_ref().expect("MToonになる");
    // `linearstep(-0.5, 0.5, N·L - 0.5)` は `saturate(N·L)` に等しい。
    assert_eq!(mtoon.shade_color_factor, [0.0, 0.0, 0.0]);
    assert!(近い(mtoon.shading_shift_factor, -0.5));
    assert!(近い(mtoon.shading_toony_factor, 0.5));
    assert_eq!(material.normal_image, None);
}

#[test]
fn bumped_diffuseは法線を載せる() {
    let (_guard, root) = 作業域を作る();
    let bump = guid(0x23);
    テクスチャを書く(
        &root,
        "Assets/Textures/Bump.png",
        &bump,
        [128, 128, 255, 255],
    );
    材質を書く(
        &root,
        &材質 {
            シェーダ: BUMPED_DIFFUSE,
            テクスチャ: vec![("_BumpMap", &bump)],
            ..Default::default()
        },
    );
    let (material, _) = 読む(&root);
    assert_eq!(material.normal_image, Some(0));
    assert_eq!(material.normal_scale, 1.0);
}

#[test]
fn matcap_litは平均色を基準色に掛ける() {
    let (_guard, root) = 作業域を作る();
    let matcap = guid(0x24);
    テクスチャを書く(
        &root,
        "Assets/Textures/MatCap.png",
        &matcap,
        [128, 128, 128, 255],
    );
    材質を書く(
        &root,
        &材質 {
            シェーダ: MATCAP_LIT,
            テクスチャ: vec![("_MatCap", &matcap)],
            ..Default::default()
        },
    );
    let (material, report) = 読む(&root);
    assert!(警告がある(&report, "乗算MatCap"), "{:?}", report.warnings());
    // sRGBの128/255は線形で約0.2158である。
    for channel in &material.base_color_factor[..3] {
        assert!((channel - 0.2158).abs() < 0.002, "{channel}");
    }
    let mtoon = material.mtoon.as_ref().expect("MToonになる");
    assert_eq!(
        mtoon.shade_color_factor,
        [
            material.base_color_factor[0],
            material.base_color_factor[1],
            material.base_color_factor[2],
        ]
    );
}

#[test]
fn standard_liteは発光をキーワードで切り替える() {
    let (_guard, root) = 作業域を作る();
    let emission = guid(0x25);
    テクスチャを書く(
        &root,
        "Assets/Textures/Emission.png",
        &emission,
        [255, 255, 255, 255],
    );
    let mut 中身 = 材質 {
        シェーダ: STANDARD_LITE,
        テクスチャ: vec![("_EmissionMap", &emission)],
        浮動小数: vec![("_BumpScale", 0.5)],
        色: vec![
            ("_Color", [0.5, 0.5, 0.5, 1.0]),
            ("_EmissionColor", [1.0, 0.0, 0.0, 1.0]),
        ],
        ..Default::default()
    };
    材質を書く(&root, &中身);
    let (material, _) = 読む(&root);
    // `_EMISSION` が無ければ発光しない。
    assert_eq!(material.emissive_factor, [0.0, 0.0, 0.0]);
    assert!((material.base_color_factor[0] - 0.214).abs() < 0.001);
    assert!(近い(material.normal_scale, 0.5));

    中身.キーワード欄 = "  m_ShaderKeywords: _EMISSION\n";
    材質を書く(&root, &中身);
    let (material, _) = 読む(&root);
    assert_eq!(material.emissive_factor, [1.0, 0.0, 0.0]);
    assert_eq!(material.emissive_image, Some(0));
}

#[test]
fn alpha_blendedパーティクルは両面の半透明になる() {
    let (_guard, root) = 作業域を作る();
    材質を書く(
        &root,
        &材質 {
            シェーダ: PARTICLE_ALPHA,
            ..Default::default()
        },
    );
    let (material, _) = 読む(&root);
    assert_eq!(material.alpha_mode, VrmAlphaMode::Blend);
    assert!(material.double_sided);
    let mtoon = material.mtoon.as_ref().expect("MToonになる");
    assert!(!mtoon.transparent_with_z_write);
}

#[test]
fn 乗算パーティクルとワールド用は灰色材質にする() {
    for (shader, 部分) in [(PARTICLE_MULTIPLY, "乗算合成"), (SKYBOX, "ワールド用")] {
        let (_guard, root) = 作業域を作る();
        材質を書く(
            &root,
            &材質 {
                シェーダ: shader,
                ..Default::default()
            },
        );
        let (material, report) = 読む(&root);
        assert!(material.mtoon.is_none(), "{shader}");
        assert!(警告がある(&report, 部分), "{:?}", report.warnings());
    }
}

/// Toon Standardの基本の中身を作る。`キーワード欄` は呼び出し側で足す。
fn toon_standardの中身<'a>(main: &'a str, bump: &'a str, matcap: &'a str) -> 材質<'a> {
    材質 {
        シェーダ: TOON_STANDARD,
        テクスチャ: vec![
            ("_MainTex", main),
            ("_BumpMap", bump),
            ("_Matcap", matcap),
            ("_Ramp", RAMP_TOON_2BAND),
        ],
        浮動小数: vec![
            ("_ShadowBoost", 0.0),
            ("_ShadowAlbedo", 0.5),
            ("_BumpScale", 0.7),
            ("_MatcapStrength", 0.8),
            ("_RimIntensity", 0.5),
            ("_RimRange", 0.3),
            ("_RimEnvironmental", 0.0),
        ],
        整数: vec![("_Culling", 0), ("_MatcapType", 0)],
        色: vec![
            ("_Color", [1.0, 1.0, 1.0, 1.0]),
            ("_RimColor", [1.0, 1.0, 1.0, 1.0]),
            ("_EmissionColor", [0.0, 0.0, 0.0, 1.0]),
        ],
        ..Default::default()
    }
}

#[test]
fn toon_standardは影rampとキーワードの機能を写像する() {
    let (_guard, root) = 作業域を作る();
    let (main, bump, matcap) = (guid(0x22), guid(0x23), guid(0x24));
    テクスチャを書く(
        &root,
        "Assets/Textures/Body.png",
        &main,
        [255, 255, 255, 255],
    );
    テクスチャを書く(
        &root,
        "Assets/Textures/Bump.png",
        &bump,
        [128, 128, 255, 255],
    );
    テクスチャを書く(
        &root,
        "Assets/Textures/MatCap.png",
        &matcap,
        [64, 64, 64, 255],
    );
    let mut 中身 = toon_standardの中身(&main, &bump, &matcap);
    中身.キーワード欄 = "  m_ValidKeywords:\n  - USE_NORMAL_MAPS\n  - USE_MATCAP\n  - USE_RIMLIGHT\n  m_InvalidKeywords: []\n";
    材質を書く(&root, &中身);
    let (material, report) = 読む(&root);
    // `_Culling` 0（Off）は両面である。
    assert!(material.double_sided);
    assert_eq!(material.base_color_factor, [1.0, 1.0, 1.0, 1.0]);
    assert!(material.normal_image.is_some());
    assert!(近い(material.normal_scale, 0.7));
    let mtoon = material.mtoon.as_ref().expect("MToonになる");
    // 2段のRampは中央の硬い境界であり、影側は `lerp(白 × 0.5, 1, 0)` の0.5である。
    assert!(近い(mtoon.shading_shift_factor, 0.0));
    assert!(近い(mtoon.shading_toony_factor, 0.95));
    for channel in mtoon.shade_color_factor {
        assert!(近い(channel, 0.5), "{:?}", mtoon.shade_color_factor);
    }
    assert!(
        警告がある(&report, "影色のアルベド掛け"),
        "{:?}",
        report.warnings()
    );
    // 加算MatCapはMToonのMatCapへ載せる。
    assert!(mtoon.matcap_image.is_some());
    assert_eq!(mtoon.matcap_factor, [0.8, 0.8, 0.8]);
    // リムは `1 - _RimRange` で半分になるフレネル指数で近似する。
    assert_eq!(mtoon.parametric_rim_color_factor, [0.5, 0.5, 0.5]);
    let power = 0.5f32.ln() / 0.7f32.ln();
    assert!(近い(mtoon.parametric_rim_fresnel_power_factor, power));
    assert_eq!(mtoon.rim_lighting_mix_factor, 0.0);
}

#[test]
fn toon_standardはキーワードが無ければ機能を使わない() {
    let (_guard, root) = 作業域を作る();
    let (main, bump, matcap) = (guid(0x22), guid(0x23), guid(0x24));
    テクスチャを書く(
        &root,
        "Assets/Textures/Body.png",
        &main,
        [255, 255, 255, 255],
    );
    テクスチャを書く(
        &root,
        "Assets/Textures/Bump.png",
        &bump,
        [128, 128, 255, 255],
    );
    テクスチャを書く(
        &root,
        "Assets/Textures/MatCap.png",
        &matcap,
        [64, 64, 64, 255],
    );
    材質を書く(&root, &toon_standardの中身(&main, &bump, &matcap));
    let (material, _) = 読む(&root);
    assert_eq!(material.normal_image, None);
    let mtoon = material.mtoon.as_ref().expect("MToonになる");
    assert_eq!(mtoon.matcap_image, None);
    assert_eq!(mtoon.parametric_rim_color_factor, [0.0, 0.0, 0.0]);
}

#[test]
fn toon_standardは旧形式のキーワード文字列も読む() {
    let (_guard, root) = 作業域を作る();
    let (main, bump, matcap) = (guid(0x22), guid(0x23), guid(0x24));
    テクスチャを書く(
        &root,
        "Assets/Textures/Body.png",
        &main,
        [255, 255, 255, 255],
    );
    テクスチャを書く(
        &root,
        "Assets/Textures/Bump.png",
        &bump,
        [128, 128, 255, 255],
    );
    let mut 中身 = toon_standardの中身(&main, &bump, &matcap);
    中身.キーワード欄 = "  m_ShaderKeywords: USE_NORMAL_MAPS USE_SPECULAR\n";
    材質を書く(&root, &中身);
    let (material, report) = 読む(&root);
    assert!(material.normal_image.is_some());
    assert!(
        警告がある(&report, "鏡面反射は写像しない"),
        "{:?}",
        report.warnings()
    );
}

#[test]
fn toon_standard_outlineは世界座標のアウトラインになる() {
    let (_guard, root) = 作業域を作る();
    材質を書く(
        &root,
        &材質 {
            シェーダ: TOON_STANDARD_OUTLINE,
            浮動小数: vec![("_OutlineThickness", 0.2), ("_OutlineFromAlbedo", 0.0)],
            色: vec![("_OutlineColor", [0.5, 0.5, 0.5, 1.0])],
            ..Default::default()
        },
    );
    let (material, _) = 読む(&root);
    // `_Culling` の既定は背面カリングである。
    assert!(!material.double_sided);
    let mtoon = material.mtoon.as_ref().expect("MToonになる");
    assert_eq!(
        mtoon.outline_width_mode,
        VrmOutlineWidthMode::WorldCoordinates
    );
    assert!(近い(mtoon.outline_width_factor, 0.002));
    assert!((mtoon.outline_color_factor[0] - 0.214).abs() < 0.001);
    // 線色は照明を受けない。
    assert_eq!(mtoon.outline_lighting_mix_factor, 0.0);
    // Rampが未割り当て（シェーダ既定の白）なら陰影は付かない。
    assert_eq!(mtoon.shade_color_factor, [1.0, 1.0, 1.0]);
}
