//! Unityの `.mat` とテクスチャからVRM 1.0の材質を作る層。
//!
//! 入口は [`VrmMaterials::load_with`]（ターゲット非依存）であり、
//! [`crate::unity::AssetSource`] 越しにFBXの材質スロット名を `.mat` に対応付け、
//! 色・透明度・テクスチャを読み取る。
//! native向けの従来入口 [`VrmMaterials::load`] はこの読み手を使う薄いラッパであり、
//! `Assets` 配下だけを索引化する方針は変えない。
//! 得られた [`VrmMaterials`] は [`VrmExportOptions`](super::export::VrmExportOptions) に渡す。
//!
//! ```mermaid
//! flowchart TB
//!     fbxmeta[".fbx.meta<br/>externalObjects"] --> map["スロット名→.mat"]
//!     index["AssetIndex<br/>GUID→パス"] --> map
//!     map --> mat[".mat の読み取り<br/>lilToonプロパティ"]
//!     mat --> conv["MToonへの写像"]
//!     mat --> tex["テクスチャ解決<br/>.meta と画像バイト"]
//!     tex --> shrink["取り込み上限へ縮小<br/>maxTextureSize・NPOT"]
//!     shrink --> images["VrmImage列"]
//!     conv --> materials["VrmMaterials"]
//!     images --> materials
//! ```
//!
//! 画像のデコード・再エンコードは取り込み上限の縮小の範囲で行い、
//! テクスチャのベイク、lilToon以外のシェーダの解釈は行わない。
//! 形式は先頭バイトで判定し、PNG・JPEGとPSDの保存済み合成画像を受け付ける。
//! PSDはRaw・RLEの8bit RGB・Grayscaleだけを復号し、復号後にPNGへ符号化する。

use crate::escape_diag;

use std::collections::{HashMap, HashSet};
// `Path` はnative限定の薄いラッパだけで使う。
#[cfg(not(target_arch = "wasm32"))]
use std::path::Path;
use std::rc::Rc;

use super::bake::{
    AlphaMaskInput, BakeChannel, BakeImage, BakeInput, BakeSize, BakeSt, BaseColorJob, EmissionJob,
    EmissionMask, NormalJob, NpotScale, RepackJob, RimMultiplyJob, ShadeJob, bake_base_color,
    bake_channel_repack, bake_emission, bake_normal, bake_output_size, bake_rim_multiply,
    bake_shade, downscale_bake_image, downscale_bake_image_owned, encode_color_png,
    encode_data_png, encode_rgba_png, encode_rgba_png_limited, image_dimensions,
    import_fitted_size, load_bake_image, matcap_disc_average, shade_layer_weights,
};
use super::psd::{PsdError, decode_psd, is_psd, psd_dimensions};

use crate::unity::{
    AssetIndex, AssetKind, AssetSource, Guid, ParseOptions, SerializedFile, TypedObject,
};
// 索引化と正規化はnative限定の薄いラッパだけで使う。
#[cfg(not(target_arch = "wasm32"))]
use crate::unity::{FsAssetSource, IndexOptions, scan_directory};

/// 画像1枚の既定上限（64MiB）。
///
/// 等身大の4Kテクスチャが収まる大きさにし、破損由来の膨張を抑える。
const DEFAULT_MAX_IMAGE_BYTES: u64 = 64 * 1024 * 1024;
/// 画像合計の既定上限（512MiB）。
///
/// 実データの数百MBが収まる大きさにし、破損由来の膨張を抑える。
const DEFAULT_MAX_TOTAL_IMAGE_BYTES: u64 = 512 * 1024 * 1024;
/// 画像1枚の画素数の既定上限（8192x8192）。
///
/// 圧縮バイト数の上限だけでは巨大な画素寸法の復号時の確保を抑えられないため、
/// 確保前に寸法で判定する。等身大の8Kテクスチャが収まる大きさにする。
const DEFAULT_MAX_IMAGE_PIXELS: u64 = 67_108_864;
/// 復号結果の保持量の既定予算（画素数）。
///
/// 焼きの共有表が保持する画素数の目安であり、超えたら古い保持を捨てて
/// 再復号するため、変換は失敗しない。1枚上限の4枚分にしている。
const DEFAULT_MAX_TOTAL_IMAGE_PIXELS: u64 = 268_435_456;
/// 材質数の既定上限（1024件）。
///
/// 実データの数十件より十分大きく、破損由来の大量生成の暴走を抑える.
const DEFAULT_MAX_MATERIALS: usize = 1024;
/// `.mat` 1件の読み取り上限（8MiB）。
///
/// テキストYAMLの実測より十分大きく、破損由来の膨張を抑える。
const MAX_MATERIAL_BYTES: u64 = 8 * 1024 * 1024;
/// `.shader` 1件の読み取り上限（4MiB）。
///
/// 先頭のシェーダ名だけを見るため、本文全体は読まない。
const MAX_SHADER_BYTES: u64 = 4 * 1024 * 1024;

/// 不透明lilToonシェーダのGUID表（小文字）。
///
/// `.shader` が索引に無い場合の系列判定に使う。
const OPAQUE_SHADER_GUIDS: [&str; 4] = [
    "df12117ecd77c31469c224178886498e",
    "efa77a80ca0344749b4f19fdd5891cbe",
    "381af8ba8e1740a41b9768ccfb0416c2",
    "583a88005abb81a4ebbce757b4851a0d",
];
/// カットアウトlilToonシェーダのGUID表（小文字）。
const MASK_SHADER_GUIDS: [&str; 4] = [
    "85d6126cae43b6847aff4b13f4adb8ec",
    "3b4aa19949601f046a20ca8bdaee929f",
    "b957dce3d03ff5445ac989f8de643c7f",
    "8cf5267d397b04846856f6d3d9561da0",
];
/// 半透明lilToonシェーダのGUID表（小文字）。
const BLEND_SHADER_GUIDS: [&str; 4] = [
    "165365ab7100a044ca85fc8c33548a62",
    "3c79b10c7e0b2784aaa4c2f8dd17d55e",
    "0e3ece1bd59542743bccadb21f68318e",
    "1c12a37046f07ac4486881deaf0187ea",
];

/// アウトライン描画路を持つlilToonシェーダのGUID表（小文字）。
///
/// lilToon 2.3.4の `.shader.meta` で裏付けした6件であり、
/// シリーズ判定の表と直交する。参照実装の `HasOutline`
/// （名前に `Outline` を含み `OutlineOnly` を含まない）と対応する。
const OUTLINE_SHADER_GUIDS: [&str; 6] = [
    "efa77a80ca0344749b4f19fdd5891cbe",
    "583a88005abb81a4ebbce757b4851a0d",
    "3b4aa19949601f046a20ca8bdaee929f",
    "8cf5267d397b04846856f6d3d9561da0",
    "3c79b10c7e0b2784aaa4c2f8dd17d55e",
    "1c12a37046f07ac4486881deaf0187ea",
];

/// lilToon Lite系シェーダのGUID表（小文字）。
///
/// リム色のアルファ重み付けが通常lilToonと異なるため区別する。
/// 参照実装の `IsLite`（名前に `Lite` を含む）と対応する。
const LITE_SHADER_GUIDS: [&str; 6] = [
    "381af8ba8e1740a41b9768ccfb0416c2",
    "583a88005abb81a4ebbce757b4851a0d",
    "b957dce3d03ff5445ac989f8de643c7f",
    "8cf5267d397b04846856f6d3d9561da0",
    "0e3ece1bd59542743bccadb21f68318e",
    "1c12a37046f07ac4486881deaf0187ea",
];

/// PNGの先頭バイト。
const PNG_MAGIC: [u8; 8] = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
/// JPEGの先頭バイト。
const JPEG_MAGIC: [u8; 3] = [0xFF, 0xD8, 0xFF];

/// FBXの材質スロット名から引ける材質と画像の集合。
///
/// 材質は `.fbx.meta` の置換表の順序を保ち、画像はGUID単位で1件にまとめる。
/// 材質の内容（Unityの `.mat` からの写像）の詳細は設計文書を参照。
#[derive(Debug, Clone)]
pub struct VrmMaterials {
    entries: Vec<VrmMaterialEntry>,
    images: Vec<VrmImage>,
}

impl VrmMaterials {
    /// 材質列と画像列から集合を作る。
    pub fn new(entries: Vec<VrmMaterialEntry>, images: Vec<VrmImage>) -> Self {
        Self { entries, images }
    }

    /// FBXの材質スロット名で材質を引く。無い場合は `None` である。
    pub fn get(&self, slot: &str) -> Option<&VrmMaterial> {
        self.entries
            .iter()
            .find(|entry| entry.slot == slot)
            .map(|entry| &entry.material)
    }

    /// 材質列を返す。順序は `.fbx.meta` の置換表のまま保つ。
    pub fn entries(&self) -> &[VrmMaterialEntry] {
        &self.entries
    }

    /// 画像列を返す。順序は初出順である。
    pub fn images(&self) -> &[VrmImage] {
        &self.images
    }

    /// 資産の読み手から材質を読み取る（ターゲット非依存）。
    ///
    /// `source` は `Assets` 直下を基準とする読み手、`index` はその索引、
    /// `fbx_relative` は `Assets` 基準の `/` 区切りのFBXパスである。
    /// `.fbx.meta` の `UnityEngine:Material` 置換表を順に解決し、
    /// `.mat` とテクスチャを読む。
    ///
    /// 置換表が無い・FBXが索引に無い場合は材質なしとして `(None, 報告)` を返す。
    /// 未対応の材質は灰色材質へ落とし、警告を残して続ける。
    ///
    /// # Errors
    ///
    /// `.mat` や画像の読み込み失敗・上限の超過などの場合に返す。
    pub fn load_with(
        source: &dyn AssetSource,
        index: &AssetIndex,
        fbx_relative: &str,
        options: &MaterialOptions,
    ) -> Result<(Option<VrmMaterials>, MaterialReport), MaterialError> {
        let mut context = LoadContext::new(source, index, options);
        let Some(record) = index.get_by_path(fbx_relative) else {
            context.warn(format!("FBXが索引にないため材質を読まない: {fbx_relative}"));
            return Ok((None, context.report()));
        };
        let remaps = record.meta.external_object_remaps().map_err(|error| {
            MaterialError::new(
                MaterialErrorKind::Unity,
                format!("FBXの置換表を読めない: {error}"),
            )
            .with_source(error)
        })?;
        let remaps: Vec<_> = remaps
            .into_iter()
            .filter(|entry| entry.original_type == "UnityEngine:Material")
            .collect();
        if remaps.is_empty() {
            context.warn("FBXに材質の置換表がないため材質を読まない");
            return Ok((None, context.report()));
        }
        for entry in &remaps {
            context.load_slot(entry)?;
        }
        context.assign_render_queue_offsets();
        let (materials, report) = context.finish();
        Ok((Some(materials), report))
    }

    /// 展開済みディレクトリから材質を読み取る（Native限定）。
    ///
    /// `root` は `Assets` を含むUnityプロジェクトの配置を指し、
    /// `fbx_path` は `Assets` 配下のFBXを指す。`Assets` 配下だけを索引化して
    /// [`load_with`](Self::load_with) に委譲する。正規化・走査・相対化の
    /// 挙動とエラー分類は従来のまま保つ。
    ///
    /// 置換表が無い・FBXが索引に無い場合は材質なしとして `(None, 報告)` を返す。
    /// 未対応の材質は灰色材質へ落とし、警告を残して続ける。
    ///
    /// # Errors
    ///
    /// パスの解決失敗・索引の走査失敗・`.mat` や画像の読み込み失敗・
    /// 上限の超過などの場合に返す。`Assets` が無い場合も失敗する。
    #[cfg(not(target_arch = "wasm32"))]
    pub fn load(
        root: &Path,
        fbx_path: &Path,
        options: &MaterialOptions,
    ) -> Result<(Option<VrmMaterials>, MaterialReport), MaterialError> {
        let canonical_root = std::fs::canonicalize(root).map_err(|error| {
            MaterialError::new(
                MaterialErrorKind::Io,
                format!(
                    "材質の走査元を解決できない: {}",
                    crate::escape_diag(&error.to_string())
                ),
            )
            .with_path(root.to_string_lossy().into_owned())
            .with_source(error)
        })?;
        // `ProjectSettings` などmetaを持たない配置を巻き込まないため、
        // `Assets` 配下だけを走査する。
        let canonical_assets =
            std::fs::canonicalize(canonical_root.join("Assets")).map_err(|error| {
                MaterialError::new(
                    MaterialErrorKind::Io,
                    format!(
                        "材質のAssetsを解決できない: {}",
                        crate::escape_diag(&error.to_string())
                    ),
                )
                .with_path(canonical_root.join("Assets").to_string_lossy().into_owned())
                .with_source(error)
            })?;
        let canonical_fbx = std::fs::canonicalize(fbx_path).map_err(|error| {
            MaterialError::new(
                MaterialErrorKind::Io,
                format!(
                    "FBXを解決できない: {}",
                    crate::escape_diag(&error.to_string())
                ),
            )
            .with_path(fbx_path.to_string_lossy().into_owned())
            .with_source(error)
        })?;
        let index =
            scan_directory(&canonical_assets, &IndexOptions::default()).map_err(|error| {
                // `UnityError` の表示は既に逃がし済みであり、再適用は変えない。
                let path = error.path().map(str::to_owned);
                let mut failure = MaterialError::new(
                    MaterialErrorKind::Unity,
                    format!("材質の索引化に失敗した: {error}"),
                )
                .with_source(error);
                if let Some(path) = path {
                    failure = failure.with_path(path);
                }
                failure
            })?;
        let source = FsAssetSource::new(&canonical_assets);
        let relative = canonical_fbx
            .strip_prefix(&canonical_assets)
            .ok()
            .and_then(|path| {
                path.components()
                    .map(|component| component.as_os_str().to_str().map(str::to_owned).ok_or(()))
                    .collect::<Result<Vec<_>, _>>()
                    .ok()
            })
            .map(|parts| parts.join("/").replace('\\', "/"));
        let Some(relative) = relative else {
            let mut context = LoadContext::new(&source, &index, options);
            context.warn("FBXがAssets配下にないため材質を読まない");
            return Ok((None, context.report()));
        };
        Self::load_with(&source, &index, &relative, options)
    }
}

/// 材質スロット名と材質の対。
#[derive(Debug, Clone)]
pub struct VrmMaterialEntry {
    /// FBXの材質スロット名。
    pub slot: String,
    /// 対応する材質。
    pub material: VrmMaterial,
}

/// glTF材質と `VRMC_materials_mtoon` 拡張の元になる材質1件。
#[derive(Debug, Clone, PartialEq)]
pub struct VrmMaterial {
    /// 材質名。`.mat` の `m_Name` である。
    pub name: String,
    /// glTFの透過方式。
    pub alpha_mode: VrmAlphaMode,
    /// カットアウトの閾値。カットアウト以外では使わない。
    pub alpha_cutoff: f32,
    /// 両面描画するかどうか。
    pub double_sided: bool,
    /// glTFの基準色。RGBは線形、Aは `.mat` のままである。
    pub base_color_factor: [f32; 4],
    /// 基準色テクスチャの画像索引。無い場合は `None` である。
    pub base_color_image: Option<usize>,
    /// glTFの発光色。線形である。
    pub emissive_factor: [f32; 3],
    /// 発光テクスチャの画像索引。無い場合は `None` である。
    pub emissive_image: Option<usize>,
    /// 法線テクスチャの画像索引。`_UseBumpMap` が真で割り当てがある場合だけ `Some` である。
    pub normal_image: Option<usize>,
    /// 法線の強さ。`_BumpScale` であり、テクスチャが無い場合も書く。
    pub normal_scale: f32,
    /// MToonとして解釈できた場合だけ `Some` である。
    /// `None` は灰色PBRへ落ちた材質である。
    pub mtoon: Option<VrmMtoonParameters>,
}

/// glTFの透過方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VrmAlphaMode {
    /// 不透明。
    Opaque,
    /// カットアウト。
    Mask,
    /// 半透明。
    Blend,
}

/// `VRMC_materials_mtoon` 拡張の元になる値。
#[derive(Debug, Clone, PartialEq)]
pub struct VrmMtoonParameters {
    /// 半透明かつZ書き込みするかどうか。
    pub transparent_with_z_write: bool,
    /// 描画順の差分。アバター内の相対順から求める。
    pub render_queue_offset: i32,
    /// 陰影色。線形である。
    pub shade_color_factor: [f32; 3],
    /// 陰影乗算テクスチャの画像索引。無い場合は `None` である。
    pub shade_multiply_image: Option<usize>,
    /// 陰影境界のずれ。
    pub shading_shift_factor: f32,
    /// 陰影境界の硬さ。
    pub shading_toony_factor: f32,
    /// 全方位照明の均一化係数。常に0.9である。
    pub gi_equalization_factor: f32,
    /// マットキャップの色。線形であり、無効時は `[0, 0, 0]` である。
    pub matcap_factor: [f32; 3],
    /// マットキャップテクスチャの画像索引。無い場合は `None` である。
    pub matcap_image: Option<usize>,
    /// リム乗算テクスチャの画像索引。無い場合は `None` である。
    ///
    /// リム光とMatCapに掛かる共通の倍率であり、単一の素朴なマスクだけは
    /// 直載せし、それ以外（リム色テクスチャ・アルベドtint・1超の明るさ）は
    /// `RimMultiplyComposite` の焼きで載せる。
    pub rim_multiply_image: Option<usize>,
    /// アウトライン幅の方式。
    pub outline_width_mode: VrmOutlineWidthMode,
    /// アウトライン幅。メートルである。
    pub outline_width_factor: f32,
    /// アウトライン幅乗算テクスチャの画像索引。無い場合は `None` である。
    pub outline_width_multiply_image: Option<usize>,
    /// アウトラインの色。線形である。
    pub outline_color_factor: [f32; 3],
    /// アウトラインの光照混合係数。
    pub outline_lighting_mix_factor: f32,
    /// パラメトリックリムの色。線形であり、無効時は `[0, 0, 0]` である。
    pub parametric_rim_color_factor: [f32; 3],
    /// パラメトリックリムのフレネル強さ。無効時は5である。
    pub parametric_rim_fresnel_power_factor: f32,
    /// パラメトリックリムの持ち上げ。無効時は0である。
    pub parametric_rim_lift_factor: f32,
    /// リム照明の混合係数。常に1である。
    pub rim_lighting_mix_factor: f32,
    /// UVスクロールの横速度。UnityのUV単位毎秒である。
    pub uv_animation_scroll_x_speed_factor: f32,
    /// UVスクロールの縦速度。glTFのV向き（Unityと逆）に直した値である。
    pub uv_animation_scroll_y_speed_factor: f32,
    /// UV回転の角速度。ラジアン毎秒である。
    pub uv_animation_rotation_speed_factor: f32,
}

impl Default for VrmMtoonParameters {
    /// 仕様既定のMToon値を作る。
    ///
    /// 影色は白、境界は中間、GI均一化は0.9、アウトライン・
    /// マットキャップ・リム・UVアニメは無しである。
    fn default() -> Self {
        Self {
            transparent_with_z_write: false,
            render_queue_offset: 0,
            shade_color_factor: [1.0, 1.0, 1.0],
            shade_multiply_image: None,
            shading_shift_factor: 0.0,
            shading_toony_factor: 0.9,
            gi_equalization_factor: 0.9,
            matcap_factor: [0.0, 0.0, 0.0],
            matcap_image: None,
            rim_multiply_image: None,
            outline_width_mode: VrmOutlineWidthMode::None,
            outline_width_factor: 0.0,
            outline_width_multiply_image: None,
            outline_color_factor: [0.0, 0.0, 0.0],
            outline_lighting_mix_factor: 1.0,
            parametric_rim_color_factor: [0.0, 0.0, 0.0],
            parametric_rim_fresnel_power_factor: 5.0,
            parametric_rim_lift_factor: 0.0,
            rim_lighting_mix_factor: 1.0,
            uv_animation_scroll_x_speed_factor: 0.0,
            uv_animation_scroll_y_speed_factor: 0.0,
            uv_animation_rotation_speed_factor: 0.0,
        }
    }
}

/// アウトライン幅の方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VrmOutlineWidthMode {
    /// アウトライン無し。
    None,
    /// 世界座標系の幅。
    WorldCoordinates,
}

/// GLBに埋め込む画像1件。
pub struct VrmImage {
    /// 画像名。アセットパスのファイルstemである。
    pub name: String,
    /// MIME。PNGとJPEGだけを扱う。
    pub mime: VrmImageMime,
    /// 画像バイト列。元の形式のまま埋め込む。
    pub data: Vec<u8>,
    /// `.meta` から作ったサンプラ。
    pub sampler: VrmSampler,
}

impl std::fmt::Debug for VrmImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 数十MBのバイト列をそのまま出すと診断が膨らむため、長さだけ出す。
        f.debug_struct("VrmImage")
            .field("name", &self.name)
            .field("mime", &self.mime)
            .field("data_len", &self.data.len())
            .field("sampler", &self.sampler)
            .finish()
    }
}

impl Clone for VrmImage {
    fn clone(&self) -> Self {
        Self {
            name: self.name.clone(),
            mime: self.mime,
            data: self.data.clone(),
            sampler: self.sampler,
        }
    }
}

/// 画像のMIME。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VrmImageMime {
    /// PNG。
    Png,
    /// JPEG。
    Jpeg,
}

/// テクスチャのサンプラ。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VrmSampler {
    /// 横方向の繰り返し。
    pub wrap_u: VrmWrap,
    /// 縦方向の繰り返し。
    pub wrap_v: VrmWrap,
    /// フィルタ。
    pub filter: VrmFilter,
    /// mipmapを使うかどうか。
    pub mipmaps: bool,
}

impl Default for VrmSampler {
    /// Unityの既定（繰り返し・双線形・mipmapあり）を作る。
    fn default() -> Self {
        Self {
            wrap_u: VrmWrap::Repeat,
            wrap_v: VrmWrap::Repeat,
            filter: VrmFilter::Linear,
            mipmaps: true,
        }
    }
}

/// 繰り返し方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VrmWrap {
    /// 繰り返し。
    Repeat,
    /// 端で止める。
    ClampToEdge,
    /// 鏡像で繰り返す。
    MirroredRepeat,
}

/// フィルタ方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VrmFilter {
    /// 点補間。
    Nearest,
    /// 双線形補間。
    Linear,
    /// 三線形補間。
    Trilinear,
}

/// 材質読み取りの資源上限。
///
/// フィールドは非公開で、既定値は [`Default`] が設定する。
/// 上限の変更は `with_` で始まるメソッドで行う。
#[derive(Debug, Clone)]
pub struct MaterialOptions {
    max_image_bytes: u64,
    max_total_image_bytes: u64,
    max_materials: usize,
    max_image_pixels: u64,
    max_total_image_pixels: u64,
}

impl MaterialOptions {
    /// 既定の上限を持つ設定を生成する。
    ///
    /// 画像1枚64MiB、合計512MiB、材質1024件、
    /// 画像1枚67108864画素（8192x8192）の上限、
    /// 復号結果の保持量の予算268435456画素である。
    pub fn new() -> Self {
        Self {
            max_image_bytes: DEFAULT_MAX_IMAGE_BYTES,
            max_total_image_bytes: DEFAULT_MAX_TOTAL_IMAGE_BYTES,
            max_materials: DEFAULT_MAX_MATERIALS,
            max_image_pixels: DEFAULT_MAX_IMAGE_PIXELS,
            max_total_image_pixels: DEFAULT_MAX_TOTAL_IMAGE_PIXELS,
        }
    }

    /// 画像1枚の上限（バイト数）を変更する。
    pub fn with_max_image_bytes(mut self, bytes: u64) -> Self {
        self.max_image_bytes = bytes;
        self
    }

    /// 画像合計の上限（バイト数）を変更する。
    pub fn with_max_total_image_bytes(mut self, bytes: u64) -> Self {
        self.max_total_image_bytes = bytes;
        self
    }

    /// 材質数の上限を変更する。
    pub fn with_max_materials(mut self, count: usize) -> Self {
        self.max_materials = count;
        self
    }

    /// 画像1枚の画素数の上限を変更する。
    pub fn with_max_image_pixels(mut self, pixels: u64) -> Self {
        self.max_image_pixels = pixels;
        self
    }

    /// 復号結果の保持量の予算を変更する。
    ///
    /// 焼きの共有表が保持する画素数の目安であり、超えたら古い保持を捨てて
    /// 再復号する。予算の超過では失敗しない。
    pub fn with_max_total_image_pixels(mut self, pixels: u64) -> Self {
        self.max_total_image_pixels = pixels;
        self
    }
}

impl Default for MaterialOptions {
    fn default() -> Self {
        Self::new()
    }
}

/// 材質読み取りの警告報告。
///
/// 失敗ではない落とし（灰色材質への退避・テクスチャの省略など）を残す。
#[derive(Debug, Clone, Default)]
pub struct MaterialReport {
    warnings: Vec<String>,
}

impl MaterialReport {
    /// 警告文の列を返す。順序は読み取り順である。
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }
}

/// 材質読み取りの失敗エラー。
///
/// 対象パスが判明している範囲で保持し、表示だけが逃がす。
/// 詳細部はヒープに置き、`Result` の `Err` 側を小さく保つ。
#[derive(Debug)]
pub struct MaterialError {
    kind: MaterialErrorKind,
    message: Box<str>,
    path: Option<Box<str>>,
    source: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl MaterialError {
    pub(crate) fn new(kind: MaterialErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into().into_boxed_str(),
            path: None,
            source: None,
        }
    }

    pub(crate) fn with_path(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into().into_boxed_str());
        self
    }

    pub(crate) fn with_source(
        mut self,
        source: impl Into<Box<dyn std::error::Error + Send + Sync>>,
    ) -> Self {
        self.source = Some(source.into());
        self
    }

    /// エラーの種別を返す。
    pub fn kind(&self) -> MaterialErrorKind {
        self.kind
    }

    /// 判明している範囲で、対象のパスを返す。
    pub fn path(&self) -> Option<&str> {
        self.path.as_deref()
    }
}

impl std::fmt::Display for MaterialError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 文面は生成時に逃がし済みだが、表示時にも逃がして未逃がしの混入を防ぐ。
        // 逃がし済みの文面への再適用は変えない。
        write!(f, "{:?}: {}", self.kind, escape_diag(&self.message))?;
        if let Some(path) = &self.path {
            write!(f, " [path: {}]", escape_diag(path))?;
        }
        Ok(())
    }
}

impl std::error::Error for MaterialError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_ref()
            .map(|source| source.as_ref() as &(dyn std::error::Error + 'static))
    }
}

/// 材質読み取りの失敗エラーの種別。
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaterialErrorKind {
    /// Unity層の失敗（索引化・置換表など）。
    Unity,
    /// 入出力に失敗する場合。
    Io,
    /// 資源上限を超える場合。
    LimitExceeded,
    /// 材質として成立しない場合。
    InvalidMaterial,
}

/// ローダーと書き出しの共通の灰色材質。
///
/// 材質名はスロット名（空なら `Material`）、不透明、基準色は中間調の灰色、
/// 片面、発光なし、MToonなしである。
pub(crate) fn placeholder(slot: &str) -> VrmMaterial {
    VrmMaterial {
        name: if slot.is_empty() {
            "Material".to_string()
        } else {
            slot.to_string()
        },
        alpha_mode: VrmAlphaMode::Opaque,
        alpha_cutoff: 0.5,
        double_sided: false,
        base_color_factor: [0.8, 0.8, 0.8, 1.0],
        base_color_image: None,
        emissive_factor: [0.0, 0.0, 0.0],
        emissive_image: None,
        normal_image: None,
        normal_scale: 1.0,
        mtoon: None,
    }
}

/// lilToonシェーダの透明度系列。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShaderSeries {
    /// 不透明。
    Opaque,
    /// カットアウト。
    Mask,
    /// 半透明。
    Blend,
}

/// lilToonシェーダの能力。系列・アウトライン描画路・Multiかどうかを持つ。
///
/// 系列は透過方式、アウトライン描画路は `_UseOutline` に関係なく
/// 描画するかどうか、Multiは描画方式を使うかどうかに使う。
/// 参照実装の `ShaderCapabilityResult`（`RenderMode`・`HasOutline`・
/// `IsMulti`）と対応する。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ShaderCapability {
    /// 透明度系列。
    series: ShaderSeries,
    /// アウトライン描画路を持つかどうか。
    has_outline: bool,
    /// Multi系（描画方式が `_TransparentMode` で決まる）かどうか。
    is_multi: bool,
    /// Lite系（リム色にアルファを掛けない）かどうか。
    is_lite: bool,
}

/// 読み取り作業域。警告・材質・画像と重複排除表を持つ。
pub(crate) struct LoadContext<'a> {
    source: &'a dyn AssetSource,
    index: &'a AssetIndex,
    options: &'a MaterialOptions,
    warnings: Vec<String>,
    entries: Vec<VrmMaterialEntry>,
    seen_slots: HashSet<String>,
    /// 材質列と同じ順序の実効描画順。描画順差分の割り当てに使う。
    queues: Vec<i32>,
    images: Vec<VrmImage>,
    image_at: HashMap<Guid, usize>,
    total_image_bytes: u64,
    /// 焼きの共有表（`baked_inputs`）が現在保持している画素数の合計。
    ///
    /// 合計は保持量の予算であり、超えたら捨てて再復号する。ピークは
    /// 予算＋処理中の数枚に抑える。
    cached_image_pixels: u64,
    /// 焼き用に復号した画像の共有表。復号失敗時は `None` を覚えて直載せへ落とす。
    /// キーはGUIDと色・データ区分の対であり、同じGUIDでも用途区分が違えば
    /// 別の版（縮小の色空間が違う）として持つ。同じ対は1回だけ復号する。
    baked_inputs: HashMap<(Guid, bool), Option<Rc<BakeImage>>>,
    /// 焼き出力の内容ハッシュ（SHA-256）から画像索引への表。
    baked_outputs: HashMap<[u8; 32], usize>,
}

impl<'a> LoadContext<'a> {
    pub(crate) fn new(
        source: &'a dyn AssetSource,
        index: &'a AssetIndex,
        options: &'a MaterialOptions,
    ) -> Self {
        Self {
            source,
            index,
            options,
            warnings: Vec::new(),
            entries: Vec::new(),
            seen_slots: HashSet::new(),
            queues: Vec::new(),
            images: Vec::new(),
            image_at: HashMap::new(),
            total_image_bytes: 0,
            cached_image_pixels: 0,
            baked_inputs: HashMap::new(),
            baked_outputs: HashMap::new(),
        }
    }

    /// 警告を1件残す。
    fn warn(&mut self, message: impl Into<String>) {
        self.warnings.push(message.into());
    }

    /// 報告を作る。
    fn report(&mut self) -> MaterialReport {
        MaterialReport {
            warnings: std::mem::take(&mut self.warnings),
        }
    }

    /// 1枚の画素数が上限に収まることを確かめる。
    ///
    /// 復号の実体は [`load_bake_image`](super::bake::load_bake_image) 側でも確保前に
    /// 判定するため、二重の確認になる。超過は [`MaterialErrorKind::LimitExceeded`]
    /// であり、文面に寸法と上限を含む。`detail` は対象の表示（スロットと
    /// プロパティ）である。
    fn check_image_pixels(
        &self,
        width: u32,
        height: u32,
        detail: &str,
    ) -> Result<(), MaterialError> {
        let pixels = u64::from(width).saturating_mul(u64::from(height));
        if pixels > self.options.max_image_pixels {
            return Err(MaterialError::new(
                MaterialErrorKind::LimitExceeded,
                format!(
                    "画像の画素数が上限を超えた: {detail}: {width}x{height}（上限{}画素）",
                    self.options.max_image_pixels,
                ),
            ));
        }
        Ok(())
    }

    /// 復号結果を保持量の予算に入れる。
    ///
    /// 現在の保持量に新しい画像の画素数を足して予算（`max_total_image_pixels`）を
    /// 超えるなら、共有表（`baked_inputs`）を全消去して保持量を0に戻してから
    /// 入れる。失敗にしない。同じ画像を複数回・複数用途で復号すると復号の累計は
    /// 保持量より大きくなるため、累計での失敗はしない。消去時に処理中の画像は
    /// `Rc` で生きたまま残るため、処理中の画像は壊れない。
    /// ピークは予算＋処理中の数枚に抑える。
    fn admit_bake_cache(&mut self, width: u32, height: u32) {
        let pixels = u64::from(width).saturating_mul(u64::from(height));
        if self.cached_image_pixels.saturating_add(pixels) > self.options.max_total_image_pixels {
            self.baked_inputs.clear();
            self.cached_image_pixels = 0;
        }
        self.cached_image_pixels = self.cached_image_pixels.saturating_add(pixels);
    }

    /// 置換表1件を解決して材質列に積む。
    ///
    /// 解決できない材質は灰色材質へ落として警告し、書き出しは続ける。
    /// 材質数の上限超過だけは失敗として返す。
    fn load_slot(
        &mut self,
        entry: &crate::unity::ExternalObjectEntry,
    ) -> Result<(), MaterialError> {
        let slot = entry.original_name.clone();
        if !self.seen_slots.insert(slot.clone()) {
            self.warn(format!("材質スロットが重複するため最初を残す: {slot}"));
            return Ok(());
        }
        if self.entries.len() >= self.options.max_materials {
            return Err(MaterialError::new(
                MaterialErrorKind::LimitExceeded,
                format!("材質数が上限を超えた: {}", self.entries.len() + 1),
            ));
        }
        let (material, queue) = self.resolve_material(entry, &slot)?;
        self.entries.push(VrmMaterialEntry { slot, material });
        self.queues.push(queue);
        Ok(())
    }

    /// sidecar指定の材質1件をGUIDで読み取り、材質列に積む（レンダラ割当用）。
    ///
    /// 解決できない材質は灰色材質へ落として警告し、書き出しは続ける。
    /// 材質数の上限超過と画像の読み込み失敗だけは失敗として返す。
    /// `slot` は材質プールの共有名であり、重複時は最初の読み取りを残す。
    /// `mat_path` はsidecarの `.mat` 資産パスであり、索引と食い違う場合は
    /// 警告して索引側を使う。
    // レンダラ経路はnative限定のため、対象外ターゲットでは未使用になる。
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) fn load_renderer_material(
        &mut self,
        guid_text: &str,
        mat_path: &str,
        slot: &str,
    ) -> Result<(), MaterialError> {
        if !self.seen_slots.insert(slot.to_owned()) {
            return Ok(());
        }
        if self.entries.len() >= self.options.max_materials {
            return Err(MaterialError::new(
                MaterialErrorKind::LimitExceeded,
                format!("材質数が上限を超えた: {}", self.entries.len() + 1),
            ));
        }
        let guid = match Guid::parse(guid_text) {
            Ok(guid) => guid,
            Err(_) => {
                self.warn(format!(
                    "レンダラ材質のGUIDが不正のため灰色材質にする: {slot}: {mat_path}"
                ));
                self.entries.push(VrmMaterialEntry {
                    slot: slot.to_owned(),
                    material: placeholder(slot),
                });
                self.queues.push(-1);
                return Ok(());
            }
        };
        let (material, queue) =
            self.load_material_by_guid(&guid, Some(mat_path), "レンダラ材質", slot)?;
        self.entries.push(VrmMaterialEntry {
            slot: slot.to_owned(),
            material,
        });
        self.queues.push(queue);
        Ok(())
    }

    /// 読み取り作業域から材質集合と報告を取り出す。
    pub(crate) fn finish(mut self) -> (VrmMaterials, MaterialReport) {
        let materials = VrmMaterials::new(
            std::mem::take(&mut self.entries),
            std::mem::take(&mut self.images),
        );
        let report = self.report();
        (materials, report)
    }

    /// 全材質の描画順差分を割り当てる。
    ///
    /// 参照実装の `RenderQueueOffsets::Assign` と対応する。絶対値ではなく
    /// 実効描画順の相対順だけを使い、不透明・カットアウトは0、
    /// 半透明かつZ書き込みありは描画順の昇順に0から、
    /// 半透明かつZ書き込みなしは描画順の降順に0から下げる。
    /// 範囲はMToon仕様（Z書き込みありは0〜9、なしは-9〜0）に収める。
    pub(crate) fn assign_render_queue_offsets(&mut self) {
        let mut zwrite: Vec<i32> = Vec::new();
        let mut nozwrite: Vec<i32> = Vec::new();
        for (entry, queue) in self.entries.iter().zip(self.queues.iter()) {
            let Some(mtoon) = entry.material.mtoon.as_ref() else {
                continue;
            };
            if entry.material.alpha_mode != VrmAlphaMode::Blend {
                continue;
            }
            if mtoon.transparent_with_z_write {
                if !zwrite.contains(queue) {
                    zwrite.push(*queue);
                }
            } else if !nozwrite.contains(queue) {
                nozwrite.push(*queue);
            }
        }
        zwrite.sort_unstable();
        nozwrite.sort_unstable();
        for (entry, queue) in self.entries.iter_mut().zip(self.queues.iter()) {
            let Some(mtoon) = entry.material.mtoon.as_mut() else {
                continue;
            };
            if entry.material.alpha_mode != VrmAlphaMode::Blend {
                continue;
            }
            if mtoon.transparent_with_z_write {
                let rank = zwrite.iter().position(|known| known == queue).unwrap_or(0);
                mtoon.render_queue_offset = rank.min(9) as i32;
            } else {
                let rank = nozwrite
                    .iter()
                    .position(|known| known == queue)
                    .unwrap_or(0);
                mtoon.render_queue_offset = (rank as i32 - (nozwrite.len() as i32 - 1)).max(-9);
            }
        }
    }

    /// 置換表1件から材質を作る。未対応は灰色材質へ落として警告する。
    ///
    /// 描画順の算出に使う実効描画順も返す。灰色材質へ落ちた場合は-1であり、
    /// 描画順の割り当てでは使わない。
    fn resolve_material(
        &mut self,
        entry: &crate::unity::ExternalObjectEntry,
        slot: &str,
    ) -> Result<(VrmMaterial, i32), MaterialError> {
        // null参照は置換なしと同じ扱いにする。
        if entry.replacement.is_null() {
            self.warn(format!("材質の置換先がnullのため灰色材質にする: {slot}"));
            return Ok((placeholder(slot), -1));
        }
        let Some(guid) = entry.replacement.guid.clone() else {
            self.warn(format!("材質の置換先GUIDがないため灰色材質にする: {slot}"));
            return Ok((placeholder(slot), -1));
        };
        self.load_material_by_guid(&guid, None, "材質の置換先", slot)
    }

    /// GUID指定で `.mat` から材質を作る。未対応は灰色材質へ落として警告する。
    ///
    /// 描画順の算出に使う実効描画順も返す。灰色材質へ落ちた場合は-1であり、
    /// 描画順の割り当てでは使わない。`expected_path` がある場合は索引の
    /// パスと突き合わせ、食い違う場合は警告して索引側を使う。
    /// `subject` は警告文の主語（置換表経路かレンダラ経路か）である。
    fn load_material_by_guid(
        &mut self,
        guid: &Guid,
        expected_path: Option<&str>,
        subject: &str,
        slot: &str,
    ) -> Result<(VrmMaterial, i32), MaterialError> {
        let Some(record) = self.index.get(guid) else {
            self.warn(format!("{subject}が索引にないため灰色材質にする: {slot}"));
            return Ok((placeholder(slot), -1));
        };
        if record.kind != AssetKind::Material {
            self.warn(format!("{subject}が材質でないため灰色材質にする: {slot}"));
            return Ok((placeholder(slot), -1));
        };
        let path = record.path.clone();
        // sidecarの資産パスは `Assets/` 始まり、索引は `Assets` 配下の相対のため、
        // 接頭辞を除いて突き合わせる。食い違う場合は警告して索引側を使う。
        let expected =
            expected_path.map(|expected| expected.strip_prefix("Assets/").unwrap_or(expected));
        if let Some(expected) = expected
            && expected != path
        {
            self.warn(format!(
                "{subject}のパスが索引と一致しないため索引を使う: {slot}: {expected}に対して{path}"
            ));
        }
        let bytes = match self.source.read_limited(&path, MAX_MATERIAL_BYTES) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::InvalidData => {
                return Err(MaterialError::new(
                    MaterialErrorKind::LimitExceeded,
                    format!("材質が上限を超えた: {slot}: {path}"),
                )
                .with_path(path)
                .with_source(error));
            }
            Err(error) => {
                return Err(MaterialError::new(
                    MaterialErrorKind::Io,
                    format!(
                        "材質を読めない: {slot}: {}",
                        crate::escape_diag(&error.to_string())
                    ),
                )
                .with_path(path)
                .with_source(error));
            }
        };
        let file = match SerializedFile::parse(&bytes, &ParseOptions::new()) {
            Ok(file) => file,
            Err(error) => {
                self.warn(format!(
                    "材質のYAMLが読めないため灰色材質にする: {slot}: {error}"
                ));
                return Ok((placeholder(slot), -1));
            }
        };
        let mut found = None;
        let mut decode_error = None;
        for document in file.documents() {
            match document.decode() {
                Ok(TypedObject::Material(material)) => {
                    found = Some(material);
                    break;
                }
                // 材質以外の文書は読み飛ばす。材質の読み取り失敗は覚えて警告に載せる。
                Ok(_) => {}
                Err(error) => {
                    decode_error = Some(error);
                }
            }
        }
        let Some(material) = found else {
            if let Some(error) = decode_error {
                self.warn(format!(
                    "材質の読み取りに失敗したため灰色材質にする: {slot}: {error}"
                ));
            } else {
                self.warn(format!("材質文書がないため灰色材質にする: {slot}: {path}"));
            }
            return Ok((placeholder(slot), -1));
        };
        // 写像時の `clamp01` や `.max(0.0)` が非有限を正常値に潰す前に検出する。
        // 写像後の `validate_material` は有限の範囲外だけを拾う役割に残す。
        if has_non_finite(&material) {
            self.warn(format!("材質に非有限値があるため灰色材質にする: {slot}"));
            return Ok((placeholder(slot), -1));
        }
        let Some(capability) = self.shader_capability(&material, slot) else {
            self.warn(format!(
                "未対応シェーダのため灰色材質にする: {slot}: {}",
                material.name
            ));
            return Ok((placeholder(slot), effective_queue(&material, None)));
        };
        let queue = effective_queue(&material, Some(capability.series));
        let mapped = self.map_material(&material, slot, capability)?;
        if let Err(field) = validate_material(&mapped) {
            self.warn(format!("材質値が不正なため灰色材質にする: {slot}: {field}"));
            return Ok((placeholder(slot), queue));
        }
        Ok((mapped, queue))
    }

    /// シェーダ参照から能力（系列・アウトライン・Multi/Lite）を求める。
    /// lilToonでなければ `None` である。
    ///
    /// `.shader` が索引にあれば先頭のシェーダ名で判定し、無ければ既知のGUID表で
    /// 判定する。`.shader` が読めない・上限を超える・UTF-8でない場合は
    /// 名前判定を諦めてGUID表へ進み、その旨を警告する。
    /// どちらも不明で `_lilToonVersion` を持つ場合だけ、描画状態から
    /// 不透明か半透明かを推定する（カットアウトは判定できない）。
    /// アウトライン描画路は名前に `Outline` を含み `OutlineOnly` を含まない
    /// 場合に持ち、Multi系は `_UseOutline` で切り替える。
    /// 参照実装の `ShaderCapability::ClassifyLilToon` と対応する。
    fn shader_capability(
        &mut self,
        material: &crate::unity::Material,
        slot: &str,
    ) -> Option<ShaderCapability> {
        if !material.shader.is_null()
            && let Some(guid) = material.shader.guid.clone()
        {
            if let Some(record) = self.index.get(&guid)
                && record.kind == AssetKind::Shader
            {
                match self.source.read_limited(&record.path, MAX_SHADER_BYTES) {
                    Ok(bytes) => match std::str::from_utf8(&bytes) {
                        Ok(text) => {
                            if let Some(name) = first_shader_name(text)
                                && let Some(capability) = capability_of_name(&name, material)
                            {
                                return Some(capability);
                            }
                        }
                        Err(_) => {
                            self.warn(format!("シェーダ名を読めないためGUID表で判定する: {slot}"));
                        }
                    },
                    Err(_) => {
                        self.warn(format!("シェーダ名を読めないためGUID表で判定する: {slot}"));
                    }
                }
            }
            let guid_text = guid.as_str();
            let series = if OPAQUE_SHADER_GUIDS.contains(&guid_text) {
                Some(ShaderSeries::Opaque)
            } else if MASK_SHADER_GUIDS.contains(&guid_text) {
                Some(ShaderSeries::Mask)
            } else if BLEND_SHADER_GUIDS.contains(&guid_text) {
                Some(ShaderSeries::Blend)
            } else {
                None
            };
            if let Some(series) = series {
                return Some(ShaderCapability {
                    series,
                    has_outline: OUTLINE_SHADER_GUIDS.contains(&guid_text),
                    is_multi: false,
                    is_lite: LITE_SHADER_GUIDS.contains(&guid_text),
                });
            }
        }
        // GUIDが未知の場合だけ版数で推定する。カットアウトは区別できない。
        if has_float(material, "_lilToonVersion") {
            self.warn(format!(
                "シェーダ系列を判定できないため描画状態から推定しカットアウトは区別しない: {slot}"
            ));
            let dst = number(material, "_DstBlend").unwrap_or(0.0);
            return Some(ShaderCapability {
                series: if dst == 0.0 {
                    ShaderSeries::Opaque
                } else {
                    ShaderSeries::Blend
                },
                has_outline: false,
                is_multi: false,
                is_lite: false,
            });
        }
        None
    }

    /// lilToonプロパティをVRM材質に写像する。
    ///
    /// テクスチャの画像読み込みだけが入出力失敗として `Err` を返し、
    /// それ以外の未対応は警告に落として続ける。
    fn map_material(
        &mut self,
        material: &crate::unity::Material,
        slot: &str,
        capability: ShaderCapability,
    ) -> Result<VrmMaterial, MaterialError> {
        let base = color(material, "_Color").unwrap_or([1.0, 1.0, 1.0, 1.0]);
        let mut base_rgb = [
            srgb_to_linear(base[0]),
            srgb_to_linear(base[1]),
            srgb_to_linear(base[2]),
        ];
        let alpha_mode = match capability.series {
            ShaderSeries::Opaque => VrmAlphaMode::Opaque,
            ShaderSeries::Mask => VrmAlphaMode::Mask,
            ShaderSeries::Blend => VrmAlphaMode::Blend,
        };
        let alpha_cutoff = clamp01(number(material, "_Cutoff").unwrap_or(0.5)) as f32;
        let cull = number(material, "_Cull").unwrap_or(2.0);
        let double_sided = cull != 2.0;
        if cull == 1.0 {
            self.warn(format!("前面カリングは両面で近似する: {slot}"));
        }
        let zwrite = number(material, "_ZWrite").unwrap_or(1.0) != 0.0;
        // 主テクスチャの `_ST`。焼きの入力はこれとの合成で読む。
        // 焼きを伴う合成は単位変換の共有を前提にするため、
        // 非単位の場合は焼かずに直載せへ落とす（警告は解決側が出す）。
        let main_st = bake_st_of(texture(material, "_MainTex"));
        let st_clean = main_st.is_identity();

        // 参照実装の `ReadBase` と対応する。焼きの対象は
        // `BaseColorBake` が再現する入力（アルファマスク）だけであり、
        // 色調補正や2nd・3rd・グラデーションが有効な場合は焼かずに直載せする。
        let hsvg_active =
            color(material, "_MainTexHSVG").is_some_and(|hsvg| hsvg != [0.0, 1.0, 1.0, 1.0]);
        let alpha_bake = matches!(
            number(material, "_AlphaMaskMode").unwrap_or(0.0) as i64,
            1..=4
        ) && has_texture(material, "_AlphaMask");
        let layers_active = number(material, "_UseMain2ndTex").unwrap_or(0.0) != 0.0
            && has_texture(material, "_Main2ndTex")
            || number(material, "_UseMain3rdTex").unwrap_or(0.0) != 0.0
                && has_texture(material, "_Main3rdTex");
        let base_bake = alpha_bake && !hsvg_active && !layers_active && st_clean;
        if alpha_bake && !base_bake {
            self.warn(format!(
                "アルファマスクを含む基準色の焼き込みは未対応のため基準色テクスチャを直載せする: {slot}"
            ));
        }
        let (base_color_image, baked_albedo) = if base_bake {
            let (index, albedo) = self.bake_base_image(material, slot, &main_st)?;
            (Some(index), albedo)
        } else {
            (self.resolve_texture(material, slot, "_MainTex")?, None)
        };

        // 参照実装の `ReadNormal` と対応する。`_UseBumpMap` が真のときだけ読み、
        // 2nd法線は `NormalComposite` で合成する。
        let use_bump = number(material, "_UseBumpMap").unwrap_or(0.0) != 0.0;
        let bump2nd = number(material, "_UseBump2ndMap").unwrap_or(0.0) != 0.0
            && has_texture(material, "_Bump2ndMap");
        let (normal_image, normal_scale) = if bump2nd {
            self.bake_normal_image(material, slot, &main_st)?
        } else if use_bump {
            (
                self.resolve_texture(material, slot, "_BumpMap")?,
                number(material, "_BumpScale").unwrap_or(1.0) as f32,
            )
        } else {
            (None, number(material, "_BumpScale").unwrap_or(1.0) as f32)
        };

        let use_shadow = number(material, "_UseShadow").unwrap_or(1.0) != 0.0;
        // MToonの影側は `shadeColorFactor × shadeMultiplyTexture` のため、
        // 焼かない場合は影色テクスチャの有無で従来の近似へ落とす。
        // 焼く場合は係数を白にし、参照実装と同じく報告は省く。
        let (
            mut shade_color_factor,
            shading_shift_factor,
            shading_toony_factor,
            shade_multiply_image,
        ) = if !use_shadow {
            // 影を使わない場合、陰影は基準色と同じにし、境界は見せない。
            (base_rgb, 0.0, 0.9, base_color_image)
        } else {
            self.map_shade(
                material,
                slot,
                &base,
                base_color_image,
                baked_albedo.clone(),
                &main_st,
            )?
        };

        // 参照実装の `MapRimAndMatcap` のうち枠の判定に対応する。MToonの枠は
        // ひとつだけであり、どのMatCapを入れるかは `SlotMatcapPrefix` と対応する。
        // 乗算（`BlendMode == 3`）でマスク越しの場合はMToonの加算MatCapで
        // 表せないため枠から外す。減光（`Covers`）は下の色を暗くして足し戻す。
        let use_rim = number(material, "_UseRim").unwrap_or(0.0) != 0.0;
        let matcap_slot = matcap_slot(material);
        let matcap_prefix = match matcap_slot {
            MatcapSlot::First => Some("_MatCap"),
            MatcapSlot::Second => Some("_MatCap2nd"),
            MatcapSlot::None => None,
        };
        let matcap_blend_mode = matcap_prefix
            .map(|prefix| number(material, &format!("{prefix}BlendMode")).unwrap_or(1.0) as i64)
            .unwrap_or(1);
        let multiplies = matcap_blend_mode == 3;
        let rim_textured = use_rim && has_texture(material, "_RimColorTex");
        let covers = matcap_prefix
            .map(|prefix| matcap_covers(material, prefix))
            .unwrap_or(0.0);
        let has_mask = matcap_prefix
            .map(|prefix| has_texture(material, &format!("{prefix}BlendMask")))
            .unwrap_or(false);
        // 参照実装の `matcapExported`
        // （本体があり、乗算でなければ出すか、減光で足し戻せる場合だけ出す）と対応する。
        // 覆う量があり、マスクが無く、リムにテクスチャが無い場合だけ
        // 下の色を暗くして足し戻す。候補に無いMatCapは解決も埋め込みもしない。
        let darkens_candidate = covers > 0.0 && !has_mask && !rim_textured;
        let matcap_candidate = matcap_prefix.is_some() && (!multiplies || darkens_candidate);
        if multiplies && matcap_prefix.is_some() && !matcap_candidate {
            let reason = if has_mask {
                "マスク付きの減光は再現できない"
            } else if rim_textured {
                "リムの色テクスチャを共有するため、下の色を均一に暗くして足し戻せない"
            } else if covers == 0.0 {
                "強度が0で色を変えない"
            } else {
                "MToonの加算MatCapでは減光を表せない"
            };
            let remedy = if has_mask {
                "マスクを外すかAddへ"
            } else if rim_textured {
                "リムの色テクスチャを外すかAddへ"
            } else {
                "Addへ"
            };
            self.warn(format!(
                "MatCapの乗算はMToonの加算MatCapでは表せず、{reason}ため書き出さない。{remedy}: {slot}"
            ));
        }
        // MatCap本体は書き出し候補の場合だけ解決し、使わない画像を埋め込まない。
        // 解決できなければ減光も書き出しもしない（係数は `[0, 0, 0]`）。
        let matcap_image = if matcap_candidate {
            match matcap_prefix {
                Some("_MatCap") => self.resolve_texture(material, slot, "_MatCapTex")?,
                Some(_) => self.resolve_texture(material, slot, "_MatCap2ndTex")?,
                None => None,
            }
        } else {
            None
        };
        let darkens = darkens_candidate && matcap_image.is_some();
        let matcap_exported = matcap_candidate && matcap_image.is_some();
        if matcap_exported && !multiplies && matcap_blend_mode != 1 && !darkens {
            let kind = if matcap_blend_mode == 0 {
                "Normal"
            } else {
                "Screen"
            };
            self.warn(format!("{kind} MatCapは加算として書き出す: {slot}"));
        }
        if matcap_slot == MatcapSlot::Second
            && has_matcap(material, "_MatCap")
            && matcap_contributes(material, "_MatCap")
        {
            self.warn(format!(
                "Firstは乗算マスクのため書き出さず、MatCap2ndを枠に入れる: {slot}"
            ));
        }

        let (emissive_factor, emissive_image) =
            self.map_emission(material, slot, &main_st, &base, baked_albedo.clone())?;

        // 参照実装の `ReadOutline` と対応する。Outline系の変種は
        // `_UseOutline` に関係なく描画し、Multi系のときだけ
        // `_UseOutline` を見る。幅は `_OutlineWidth × 0.01`（m）であり、
        // MToonの世界座標幅として書く。
        let outline_enabled = if capability.is_multi {
            number(material, "_UseOutline").unwrap_or(0.0) != 0.0
        } else {
            capability.has_outline
        };
        let (
            outline_width_mode,
            outline_width_factor,
            outline_color_factor,
            outline_width_multiply_image,
            outline_lighting_mix_factor,
        ) = if outline_enabled {
            let width = number(material, "_OutlineWidth").unwrap_or(0.08).max(0.0);
            // 参照実装の代替値は線形で宣言されているため、sRGB復号しない。
            let outline = linear_color_or(material, "_OutlineColor", [0.6, 0.56, 0.73]);
            if has_texture(material, "_OutlineTex") {
                self.warn(format!("アウトライン色テクスチャは写像しない: {slot}"));
            }
            // 参照実装の `MapOutline` と対応する。幅マスクはMToonが読むGへ
            // `ChannelRepack` で載せ替える。直載せはしない。
            let mask = if has_texture(material, "_OutlineWidthMask") {
                self.bake_outline_width(material, slot)?
            } else {
                None
            };
            (
                VrmOutlineWidthMode::WorldCoordinates,
                (width * 0.01) as f32,
                outline,
                mask,
                clamp01(number(material, "_OutlineEnableLighting").unwrap_or(1.0)) as f32,
            )
        } else {
            (VrmOutlineWidthMode::None, 0.0, [0.0, 0.0, 0.0], None, 1.0)
        };

        // 参照実装の `ReadRim` のうち直接値に対応する。リムが無効な場合は
        // 既定（色 `[0, 0, 0]`・フレネル5・持ち上げ0・混合1）を書く。
        // 有効な場合は色・フレネル・混合を入れ、境界・ぼかしの合わせ込み
        // （参照実装の `RimFit`）は次の段として警告する。
        // 色テクスチャとMatCapマスクはリム倍率テクスチャへ焼く。
        let (rim_color, rim_lighting_mix, parametric_rim_fresnel_power_factor) = if use_rim {
            // 参照実装の代替値は線形で宣言されている。
            let rim = linear_color_or(material, "_RimColor", [0.66, 0.5, 0.48]);
            let alpha = color(material, "_RimColor").map_or(1.0, |rim| rim[3]);
            // 通常lilToonは `RimColor.a` を重みに掛け、Lite系は掛けない。
            let weight = if capability.is_lite {
                1.0
            } else {
                clamp01(alpha)
            } as f32;
            self.warn(format!(
                "リムライトの境界・ぼかしの合わせ込みは近似する: {slot}"
            ));
            (
                [rim[0] * weight, rim[1] * weight, rim[2] * weight],
                clamp01(number(material, "_RimEnableLighting").unwrap_or(1.0)) as f32,
                number(material, "_RimFresnelPower").unwrap_or(3.5).max(0.0) as f32,
            )
        } else {
            ([0.0, 0.0, 0.0], 1.0, 5.0)
        };

        // 参照実装の `ReadBase` のUVアニメと `MapUvAnimation` に対応する。
        // `_MainTex_ScrollRotate` の横・縦・回転を読み、縦はglTFのV向き
        // （Unityと逆）に直す。回転はラジアン毎秒のまま書く。
        // MToonは全テクスチャを動かすため、動作時は近似として警告する。
        let scroll = color(material, "_MainTex_ScrollRotate").unwrap_or([0.0, 0.0, 0.0, 0.0]);
        let uv_animation_scroll_x_speed_factor = scroll[0] as f32;
        // `-0.0` を出さないため、0はそのままにする。
        let uv_animation_scroll_y_speed_factor = if scroll[1] == 0.0 {
            0.0
        } else {
            -(scroll[1] as f32)
        };
        let uv_animation_rotation_speed_factor = scroll[3] as f32;
        if scroll[0] != 0.0 || scroll[1] != 0.0 || scroll[3] != 0.0 {
            self.warn(format!(
                "UVスクロール・回転はMToonでは全テクスチャに適用されるため近似する: {slot}"
            ));
        }

        self.warn_unsupported_features(material, slot);

        // 参照実装の `PlanRimMultiply` と対応する。リム光とMatCapの
        // 係数・共有テクスチャ・tint・明るさを決め、減光は下の色へ戻す。
        let plan = self.plan_rim_multiply(
            material,
            slot,
            use_rim,
            rim_color,
            rim_lighting_mix,
            matcap_prefix,
            matcap_exported,
            &base_rgb,
            base_color_image,
            &main_st,
        )?;
        // 焼きのtintへ渡すアルベド色は減光前の線形基準色である。
        // 参照実装の `AlbedoColor` と対応するが、vrctomの基準色ベイクは
        // `base_tint` を白にして `_Color` を含めないため、焼き済み経路でも
        // 線形の `_Color` を渡す。
        let albedo_base_rgb = base_rgb;
        if darkens && plan.matcap.is_some() {
            // 参照実装の `MapMatcapDarkening` と対応する。下の色を
            // `1 - Covers` に暗くし、MatCapで足し戻す。MToonのMatCapを
            // 描くアプリではUnityと同じに見え、無視するアプリでは暗く見える。
            let kept = 1.0 - covers;
            base_rgb = [base_rgb[0] * kept, base_rgb[1] * kept, base_rgb[2] * kept];
            shade_color_factor = [
                shade_color_factor[0] * kept,
                shade_color_factor[1] * kept,
                shade_color_factor[2] * kept,
            ];
            let taken = covers * 100.0;
            let left = kept * 100.0;
            let kind = if multiplies { "Multiply" } else { "Normal" };
            let times = if multiplies {
                "MatCapは基準色を掛けて足し戻される。"
            } else {
                ""
            };
            self.warn(format!(
                "MToonのMatCapは加算のみなので、下の色を{taken:.0}%取る{kind} MatCapは基準色と影色を{left:.0}%に暗くし、MatCapで足し戻す。{times}MToonのMatCapを描くVRMアプリではUnityと同じに見え、無視するアプリでは暗く見える: {slot}"
            ));
        }
        let parametric_rim_color_factor = match &plan.rim {
            Some(rim) => self.clamp_rim_factor(slot, "リム色", rim.factor),
            None => [0.0, 0.0, 0.0],
        };
        let matcap_factor = self.clamp_rim_factor(slot, "MatCap色", plan.matcap_factor);
        // 参照実装の `MapRimAndMatcap` の混合係数と対応する。最初の
        // シェア（リムがあればリム、無ければMatCap）の値を使い、
        // 複数で違う場合は近似として警告する。
        let first_mix = plan
            .rim
            .as_ref()
            .or(plan.matcap.as_ref())
            .map(|share| share.lighting_mix)
            .unwrap_or(1.0);
        let rim_lighting_mix_factor = clamp01(f64::from(first_mix)) as f32;
        let mixes_differ = [&plan.rim, &plan.matcap]
            .into_iter()
            .flatten()
            .any(|share| (share.lighting_mix - first_mix).abs() > 1e-3);
        if mixes_differ {
            self.warn(format!(
                "MToonはリムとMatCapにひとつの光照混合係数を使うため、最初の値を使う: {slot}"
            ));
        }
        let rim_multiply_image = self.bind_rim_multiply(
            material,
            slot,
            &plan,
            baked_albedo,
            albedo_base_rgb,
            base[3] as f32,
        )?;

        Ok(VrmMaterial {
            name: material.name.clone(),
            alpha_mode,
            alpha_cutoff,
            double_sided,
            base_color_factor: [base_rgb[0], base_rgb[1], base_rgb[2], base[3] as f32],
            base_color_image,
            emissive_factor,
            emissive_image,
            normal_image,
            normal_scale,
            mtoon: Some(VrmMtoonParameters {
                transparent_with_z_write: matches!(capability.series, ShaderSeries::Blend)
                    && zwrite,
                render_queue_offset: 0,
                shade_color_factor,
                shade_multiply_image,
                shading_shift_factor,
                shading_toony_factor,
                gi_equalization_factor: 0.9,
                matcap_factor,
                matcap_image,
                rim_multiply_image,
                outline_width_mode,
                outline_width_factor,
                outline_width_multiply_image,
                outline_color_factor,
                outline_lighting_mix_factor,
                parametric_rim_color_factor,
                parametric_rim_fresnel_power_factor,
                parametric_rim_lift_factor: 0.0,
                rim_lighting_mix_factor,
                uv_animation_scroll_x_speed_factor,
                uv_animation_scroll_y_speed_factor,
                uv_animation_rotation_speed_factor,
            }),
        })
    }

    /// 写像しない機能の使用を警告する。
    ///
    /// いずれも使用時（真・既定外・割り当てあり）のみ警告し、
    /// 未使用の既定値は黙って通す。
    fn warn_unsupported_features(&mut self, material: &crate::unity::Material, slot: &str) {
        if number(material, "_UseBacklight").unwrap_or(0.0) != 0.0 {
            self.warn(format!("バックライトは写像しない: {slot}"));
        }
        // 2nd法線は `NormalComposite` で合成するため、ここでは警告しない。
        // アルファマスクは単独では基準色へ焼き込むため、ここでは警告しない。
        // 焼けない組み合わせの警告は写像側が出す。
        // `_MainTexHSVG` は `m_Colors` のベクトルとして書かれる。
        if let Some(hsvg) = color(material, "_MainTexHSVG")
            && hsvg != [0.0, 1.0, 1.0, 1.0]
        {
            self.warn(format!("色相・彩度・明度の調整は写像しない: {slot}"));
        }
        if number(material, "_MainGradationStrength").unwrap_or(0.0) > 0.0 {
            self.warn(format!("グラデーションは写像しない: {slot}"));
        }
        // `_Main2ndTex` を使う機能の有無は `_UseMain2ndTex` で判定する
        // （lilToonのshader定義で確認した名称）。
        if number(material, "_UseMain2ndTex").unwrap_or(0.0) != 0.0
            && has_texture(material, "_Main2ndTex")
        {
            self.warn(format!("2ndテクスチャは写像しない: {slot}"));
        }
    }

    /// テクスチャ1件を解決して画像索引を返す。
    ///
    /// プロパティ無し・未割り当ては `None`、索引に無い・形式外は警告して `None`、
    /// 画像の読み込み失敗・上限超過だけは失敗として返す。
    /// 埋め込む画像は焼き・縮小の要否にかかわらず寸法をヘッダから検査し、
    /// 未検査のまま埋め込まない。寸法を読めない画像は警告して省く。
    /// 同じGUIDの画像は1件だけ読み、索引を共有する。
    fn resolve_texture(
        &mut self,
        material: &crate::unity::Material,
        slot: &str,
        property: &str,
    ) -> Result<Option<usize>, MaterialError> {
        let Some(environment) = texture(material, property) else {
            return Ok(None);
        };
        if environment.texture.is_null() {
            return Ok(None);
        }
        // `_ST` はMToonの単一UV変換と組み合わせた正しさを検証できないため、
        // 反映せず警告する。
        if environment.scale != [1.0, 1.0] || environment.offset != [0.0, 0.0] {
            self.warn(format!(
                "テクスチャのスケール・オフセットは反映しない: {slot}: {property}"
            ));
        }
        let Some(guid) = environment.texture.guid.clone() else {
            self.warn(format!(
                "テクスチャの参照先が解決できないため省く: {slot}: {property}"
            ));
            return Ok(None);
        };
        if let Some(&index) = self.image_at.get(&guid) {
            return Ok(Some(index));
        }
        let Some(record) = self.index.get(&guid) else {
            self.warn(format!(
                "テクスチャが索引にないため省く: {slot}: {property}"
            ));
            return Ok(None);
        };
        if record.kind != AssetKind::Texture {
            self.warn(format!(
                "テクスチャの参照先が画像でないため省く: {slot}: {property}"
            ));
            return Ok(None);
        }
        let path = record.path.clone();
        // 上限を超える分は読む前に打ち切り、超過は失敗として返す。
        let bytes = match self
            .source
            .read_limited(&path, self.options.max_image_bytes)
        {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::InvalidData => {
                return Err(MaterialError::new(
                    MaterialErrorKind::LimitExceeded,
                    format!("画像が上限を超えた: {slot}: {property}"),
                )
                .with_path(path)
                .with_source(error));
            }
            Err(error) => {
                return Err(MaterialError::new(
                    MaterialErrorKind::Io,
                    format!(
                        "画像を読めない: {slot}: {property}: {}",
                        crate::escape_diag(&error.to_string())
                    ),
                )
                .with_path(path)
                .with_source(error));
            }
        };
        let Some(source) = sniff_source(&bytes) else {
            self.warn(format!(
                "未対応の画像形式のため省く: {slot}: {property}: {path}"
            ));
            return Ok(None);
        };
        // PSDは保存済みcompositeを復号してPNGへ符号化し直す。
        // PNG・JPEGの素通し経路は変えない。
        if source == SourceImageFormat::Psd {
            return self.resolve_psd_texture(slot, property, &path, &record.meta, guid, bytes);
        }
        let mime = mime_of(source).expect("PSD以外は埋め込みMIMEを持つ");
        // 焼き・縮小の要否にかかわらず、埋め込む画像の寸法をヘッダから検査する。
        // 画素データの確保は行わない。上限超過は失敗して対象パスを付け、
        // ヘッダを読めない画像は未検査のまま埋め込まず警告して省く。
        // 変換全体は失敗させない。
        match image_dimensions(&bytes, mime) {
            Ok((width, height)) => {
                if let Err(error) =
                    self.check_image_pixels(width, height, &format!("{slot}: {property}"))
                {
                    return Err(error.with_path(path.clone()));
                }
            }
            Err(error) => {
                self.warn(format!(
                    "画像の寸法を読めないため省く: {slot}: {property}: {path}: {error}"
                ));
                return Ok(None);
            }
        };
        // PNGは付加チャンクを落としてから埋め込む。JPEGはそのまま扱う。
        // チャンク構造が読めないPNGは壊さず元のまま残し、警告する。
        // 合計バイト数と `VrmImage.data` は縮小後・除去後のバイト列で数える。
        // 復号時の画素上限の超過だけは失敗として返し、対象パスを付ける。
        let (bytes, mime) = match self.shrink_passthrough(slot, property, bytes, mime, &record.meta)
        {
            Ok(done) => done,
            Err(error) => {
                return Err(error.with_path(path.clone()));
            }
        };
        let sampler = sampler_of(&record.meta);
        Ok(Some(self.push_direct_image(
            slot, property, &path, guid, bytes, mime, sampler,
        )?))
    }

    /// 直載せ画像を画像列へ載せ、索引を返す。
    ///
    /// 合計上限の超過は失敗として返し、対象パスを付ける。
    /// 名前はアセットパスのファイルstem、サンプラは呼び出し側が `.meta` から作る。
    // 引数は画像登録にそのまま使うため束ねない。
    #[allow(clippy::too_many_arguments)]
    fn push_direct_image(
        &mut self,
        slot: &str,
        property: &str,
        path: &str,
        guid: Guid,
        bytes: Vec<u8>,
        mime: VrmImageMime,
        sampler: VrmSampler,
    ) -> Result<usize, MaterialError> {
        let total = self.total_image_bytes.saturating_add(bytes.len() as u64);
        if total > self.options.max_total_image_bytes {
            return Err(MaterialError::new(
                MaterialErrorKind::LimitExceeded,
                format!("画像の合計が上限を超えた: {slot}: {property}"),
            )
            .with_path(path.to_owned()));
        }
        let name = path
            .rsplit('/')
            .next()
            .and_then(|file| file.rsplit_once('.').map(|(stem, _)| stem.to_owned()))
            .unwrap_or_default();
        let index = self.images.len();
        self.images.push(VrmImage {
            name,
            mime,
            data: bytes,
            sampler,
        });
        self.image_at.insert(guid, index);
        self.total_image_bytes = total;
        Ok(index)
    }

    /// PSDの失敗を振り分ける。
    ///
    /// 警告扱い（対象外・破損・composite不在）は文面を残して `None` を返し、
    /// 画素上限の超過だけ対象パス付きで失敗として返す。入出力失敗は
    /// 呼び出し側の読み取りが既に分類しているため、ここには来ない。
    fn split_psd<T>(
        &mut self,
        result: Result<T, PsdError>,
        slot: &str,
        property: &str,
        path: &str,
    ) -> Result<Option<T>, MaterialError> {
        match result {
            Ok(value) => Ok(Some(value)),
            Err(PsdError::Skip { reason, detail }) => {
                self.warn(format!("{reason}: {slot}: {property}: {path}: {detail}"));
                Ok(None)
            }
            Err(PsdError::Limit(error)) => Err(error.with_path(path.to_owned())),
        }
    }

    /// PSDテクスチャ1件を直載せして画像索引を返す。
    ///
    /// 入力判別後の流れは `元寸法の上限検査（ヘッダ）→compositeのRGBA8復号→
    /// 共通の `.meta` 解像度合わせ→PNG符号化（縮小後の1回だけ）→
    /// 合計出力バイト上限・サンプラ・画像登録` であり、焼きは行わない。
    /// PSD用PNG出力は残り合計予算に対して制限付きで書き、超過を
    /// [`MaterialErrorKind::LimitExceeded`] にする。
    /// 対象外・破損・composite不在は警告して `None` を返す。
    /// 画像の読み込み失敗・上限の超過だけは失敗として返す。
    /// 同じGUIDの画像は1件だけ読み、索引を共有する。
    fn resolve_psd_texture(
        &mut self,
        slot: &str,
        property: &str,
        path: &str,
        meta: &crate::unity::AssetMeta,
        guid: Guid,
        bytes: Vec<u8>,
    ) -> Result<Option<usize>, MaterialError> {
        // 元寸法の上限検査はヘッダで行う。画素データの確保は起きない。
        let Some((width, height)) = self.split_psd(
            psd_dimensions(&bytes, self.options.max_image_pixels),
            slot,
            property,
            path,
        )?
        else {
            return Ok(None);
        };
        if let Err(error) = self.check_image_pixels(width, height, &format!("{slot}: {property}")) {
            return Err(error.with_path(path.to_owned()));
        }
        // 保存済みcompositeをRGBA8へ復号する。レイヤー合成へ落ちない。
        let Some(decoded) = self.split_psd(
            decode_psd(&bytes, self.options.max_image_pixels),
            slot,
            property,
            path,
        )?
        else {
            return Ok(None);
        };
        // 復号済み画像へ共通の `.meta` 解像度合わせを適用する。
        // PNG・JPEGの素通し経路とは別に、復号済み画素から直接載せる。
        let image = BakeImage::from_rgba8(
            decoded.width(),
            decoded.height(),
            decoded.into_pixels(),
            srgb_of(meta),
            sampler_of(meta),
        )
        .map_err(|error| error.with_path(path.to_owned()))?;
        if let Err(error) = self.check_image_pixels(
            image.width(),
            image.height(),
            &format!("{slot}: {property}"),
        ) {
            return Err(error.with_path(path.to_owned()));
        }
        let (max_size, npot, unknown_npot) = import_limit_of(meta);
        if unknown_npot {
            self.warn(format!(
                "NPOT指定が読めないためそのままの寸法で扱う: {slot}: {property}"
            ));
        }
        let (fitted_w, fitted_h) =
            import_fitted_size(image.width(), image.height(), max_size, npot);
        // `.meta` 適用後の寸法も確保前に上限検査する。
        if let Err(error) =
            self.check_image_pixels(fitted_w, fitted_h, &format!("{slot}: {property}"))
        {
            return Err(error.with_path(path.to_owned()));
        }
        let linear = !is_color_property(property);
        // 等倍時は所有権を移して複製しない。縮小時だけ画素を作り直す。
        let small = downscale_bake_image_owned(image, fitted_w, fitted_h, linear);
        let remaining = self
            .options
            .max_total_image_bytes
            .saturating_sub(self.total_image_bytes);
        let bytes = encode_rgba_png_limited(
            small.pixels(),
            BakeSize {
                width: fitted_w,
                height: fitted_h,
            },
            remaining,
            &format!("{slot}: {property}"),
        )
        .map_err(|error| {
            if error.path().is_none() {
                error.with_path(path.to_owned())
            } else {
                error
            }
        })?;
        let sampler = sampler_of(meta);
        Ok(Some(self.push_direct_image(
            slot,
            property,
            path,
            guid,
            bytes,
            VrmImageMime::Png,
            sampler,
        )?))
    }

    /// 直載せ画像を取り込み後の寸法に縮小する。
    ///
    /// Unityの取り込み（`TextureImporter`）と同じく、長辺を上限以内に収め、
    /// NPOTを適用する。縮小が必要な場合だけ復号・再符号化し、
    /// 縮小後はRGBAのPNGとして埋め込む（JPEGの縮小もPNGになる）。
    /// 取り込み上限が無い場合と復号できない画像は、付加チャンクだけを落とした
    /// 元のバイト列のまま残す。縮小用の復号結果は保持しないため、
    /// 保持量の予算の対象にはならない。
    ///
    /// 復号時の1枚の画素上限の超過だけは呼び出し側へ伝播し、それ以外の
    /// 復号失敗は警告して元のバイト列へ落とす。
    fn shrink_passthrough(
        &mut self,
        slot: &str,
        property: &str,
        bytes: Vec<u8>,
        mime: VrmImageMime,
        meta: &crate::unity::AssetMeta,
    ) -> Result<(Vec<u8>, VrmImageMime), MaterialError> {
        let (max_size, npot, unknown_npot) = import_limit_of(meta);
        if unknown_npot {
            self.warn(format!(
                "NPOT指定が読めないためそのままの寸法で扱う: {slot}: {property}"
            ));
        }
        if max_size.is_some() || npot != NpotScale::None {
            match load_bake_image(
                &bytes,
                mime,
                srgb_of(meta),
                sampler_of(meta),
                self.options.max_image_pixels,
            ) {
                Ok(image) => {
                    self.check_image_pixels(
                        image.width(),
                        image.height(),
                        &format!("{slot}: {property}"),
                    )?;
                    let (fitted_w, fitted_h) =
                        import_fitted_size(image.width(), image.height(), max_size, npot);
                    if (fitted_w, fitted_h) != (image.width(), image.height()) {
                        let linear = !is_color_property(property);
                        let small = downscale_bake_image(&image, fitted_w, fitted_h, linear);
                        return Ok((
                            encode_rgba_png(
                                small.pixels(),
                                BakeSize {
                                    width: fitted_w,
                                    height: fitted_h,
                                },
                            ),
                            VrmImageMime::Png,
                        ));
                    }
                }
                Err(error) if error.kind() == MaterialErrorKind::LimitExceeded => {
                    return Err(error);
                }
                Err(error) => {
                    self.warn(format!(
                        "直載せの縮小用に復号できないため元のまま埋め込む: {slot}: {property}: {error}"
                    ));
                }
            }
        }
        // PNGは画素に影響しない付加チャンクを落としてから埋め込む。JPEGはそのまま扱う。
        // チャンク構造が読めないPNGは壊さず元のまま残し、警告する。
        if mime == VrmImageMime::Png {
            match strip_png_ancillary_chunks(&bytes) {
                Some(stripped) => Ok((stripped, mime)),
                None => {
                    self.warn(format!(
                        "PNGのチャンク構造が読めないためそのまま埋め込む: {slot}: {property}"
                    ));
                    Ok((bytes, mime))
                }
            }
        } else {
            Ok((bytes, mime))
        }
    }

    /// 焼きの入力1件を復号して共有する。
    ///
    /// 復号失敗は警告して `None`、同じGUIDの画像は1回だけ復号し、
    /// 参照数で共有する。`st` はUV0に対する `_ST` である。
    // 引数は `BakeInput` の組み立てにそのまま使うため束ねない。
    #[allow(clippy::too_many_arguments)]
    fn bake_input(
        &mut self,
        slot: &str,
        property: &str,
        guid: Option<Guid>,
        bytes: Vec<u8>,
        mime: VrmImageMime,
        srgb: bool,
        sampler: VrmSampler,
        st: BakeSt,
        channel: BakeChannel,
        invert: bool,
        max_size: Option<u32>,
        npot: NpotScale,
    ) -> Result<Option<BakeInput>, MaterialError> {
        let Some(guid) = guid else {
            self.warn(format!(
                "テクスチャの参照先が解決できないため省く: {slot}: {property}"
            ));
            return Ok(None);
        };
        // 色は符号化済み空間で、データ（マスク・法線）は線形空間で縮小する。
        // 同じGUIDでも用途区分が違えば別の版として持つ。
        let linear = !is_color_property(property);
        let key = (guid.clone(), linear);
        if let Some(cached) = self.baked_inputs.get(&key) {
            return Ok(cached.clone().map(|image| BakeInput {
                image,
                st,
                channel,
                invert,
            }));
        }
        let image =
            match load_bake_image(&bytes, mime, srgb, sampler, self.options.max_image_pixels) {
                Ok(image) => {
                    // 焼きの入力も取り込み後の寸法にし、焼きの出力は縮小後の
                    // 入力に従う（`bake_output_size` は縮小後を見る）。
                    // 1枚の画素上限の超過は直載せへ落とさず失敗として返す。
                    // 対象パスは呼び出し側（`bake_input_of`）が付ける。
                    self.check_image_pixels(
                        image.width(),
                        image.height(),
                        &format!("{slot}: {property}"),
                    )?;
                    let (fitted_w, fitted_h) =
                        import_fitted_size(image.width(), image.height(), max_size, npot);
                    // 等倍時は所有権を移して複製しない。
                    let shrunk = Rc::new(downscale_bake_image_owned(
                        image, fitted_w, fitted_h, linear,
                    ));
                    // 縮小後の実寸で保持量の予算に入れる。超えたら古い保持を
                    // 捨てて再復号する。失敗にしない。
                    self.admit_bake_cache(shrunk.width(), shrunk.height());
                    shrunk
                }
                Err(error) if error.kind() == MaterialErrorKind::LimitExceeded => {
                    return Err(error);
                }
                Err(error) => {
                    self.warn(format!(
                        "焼き用に復号できないため直載せする: {slot}: {property}: {error}"
                    ));
                    self.baked_inputs.insert(key, None);
                    return Ok(None);
                }
            };
        self.baked_inputs.insert(key, Some(Rc::clone(&image)));
        Ok(Some(BakeInput {
            image,
            st,
            channel,
            invert,
        }))
    }

    /// PSDの保存済みcompositeから焼きの入力を作る。
    ///
    /// 復号済みRGBA8から直接載せ、中間PNGを作らない。以降の流れ
    /// （取り込み解像度合わせ・保持量の予算・用途区分ごとの共有）は
    /// PNG・JPEGの入力と同じである。復号失敗は警告して `None` を返し、
    /// 同じGUIDの再復号を避けるため共有表に覚える。
    /// 画素上限の超過と対象パス付きの失敗はそのまま返す。
    // 引数は `BakeInput` の組み立てにそのまま使うため束ねない。
    #[allow(clippy::too_many_arguments)]
    fn bake_psd_input(
        &mut self,
        slot: &str,
        property: &str,
        path: &str,
        guid: Option<Guid>,
        bytes: Vec<u8>,
        srgb: bool,
        sampler: VrmSampler,
        st: BakeSt,
        channel: BakeChannel,
        invert: bool,
        max_size: Option<u32>,
        npot: NpotScale,
    ) -> Result<Option<BakeInput>, MaterialError> {
        let Some(guid) = guid else {
            self.warn(format!(
                "テクスチャの参照先が解決できないため省く: {slot}: {property}"
            ));
            return Ok(None);
        };
        // 色は符号化済み空間で、データ（マスク・法線）は線形空間で縮小する。
        // 同じGUIDでも用途区分が違えば別の版として持つ。
        let linear = !is_color_property(property);
        let key = (guid.clone(), linear);
        if let Some(cached) = self.baked_inputs.get(&key) {
            return Ok(cached.clone().map(|image| BakeInput {
                image,
                st,
                channel,
                invert,
            }));
        }
        let image = match decode_psd(&bytes, self.options.max_image_pixels) {
            Ok(decoded) => {
                let image = BakeImage::from_rgba8(
                    decoded.width(),
                    decoded.height(),
                    decoded.into_pixels(),
                    srgb,
                    sampler,
                )
                .map_err(|error| error.with_path(path.to_owned()))?;
                // 焼きの入力も取り込み後の寸法にし、焼きの出力は縮小後の
                // 入力に従う。1枚の画素上限の超過は失敗として返す。
                self.check_image_pixels(
                    image.width(),
                    image.height(),
                    &format!("{slot}: {property}"),
                )
                .map_err(|error| error.with_path(path.to_owned()))?;
                let (fitted_w, fitted_h) =
                    import_fitted_size(image.width(), image.height(), max_size, npot);
                // `.meta` 適用後の寸法も確保前に上限検査する。
                self.check_image_pixels(fitted_w, fitted_h, &format!("{slot}: {property}"))
                    .map_err(|error| error.with_path(path.to_owned()))?;
                // 等倍時は所有権を移して複製しない。
                let shrunk = Rc::new(downscale_bake_image_owned(
                    image, fitted_w, fitted_h, linear,
                ));
                // 縮小後の実寸で保持量の予算に入れる。超えたら古い保持を
                // 捨てて再復号する。失敗にしない。
                self.admit_bake_cache(shrunk.width(), shrunk.height());
                shrunk
            }
            Err(PsdError::Skip { reason, detail }) => {
                self.warn(format!(
                    "焼き用に復号できないため直載せする: {slot}: {property}: {reason}: {detail}"
                ));
                self.baked_inputs.insert(key, None);
                return Ok(None);
            }
            Err(PsdError::Limit(error)) => {
                return Err(error.with_path(path.to_owned()));
            }
        };
        self.baked_inputs.insert(key, Some(Rc::clone(&image)));
        Ok(Some(BakeInput {
            image,
            st,
            channel,
            invert,
        }))
    }

    /// テクスチャ環境から焼きの入力を読む。未割り当ては `None` である。
    ///
    /// `_ST` は環境の値そのまま（主変換との合成は呼び出し側で行う）。
    /// 画像の形式判定・sRGB旗・サンプラは `.meta` と先頭バイトから求める。
    /// 索引外・形式外は警告して `None`、読み込み失敗・上限超過は失敗させる。
    fn bake_input_of(
        &mut self,
        material: &crate::unity::Material,
        slot: &str,
        property: &str,
        channel: BakeChannel,
        invert: bool,
    ) -> Result<Option<BakeInput>, MaterialError> {
        let Some(environment) = texture(material, property) else {
            return Ok(None);
        };
        if environment.texture.is_null() {
            return Ok(None);
        }
        let guid = environment.texture.guid.clone();
        let st = BakeSt {
            scale: environment.scale,
            offset: environment.offset,
        };
        let snapshot = environment
            .texture
            .guid
            .clone()
            .and_then(|guid| self.index.get(&guid).cloned());
        let record = match snapshot {
            Some(record) if record.kind == AssetKind::Texture => record,
            Some(_) => {
                self.warn(format!(
                    "テクスチャの参照先が画像でないため省く: {slot}: {property}"
                ));
                return Ok(None);
            }
            None => {
                self.warn(format!(
                    "テクスチャが索引にないため省く: {slot}: {property}"
                ));
                return Ok(None);
            }
        };
        // 上限を超える分は読む前に打ち切り、超過は失敗として返す。
        let path = record.path.clone();
        let bytes = match self
            .source
            .read_limited(&path, self.options.max_image_bytes)
        {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::InvalidData => {
                return Err(MaterialError::new(
                    MaterialErrorKind::LimitExceeded,
                    format!("画像が上限を超えた: {slot}: {property}"),
                )
                .with_path(path)
                .with_source(error));
            }
            Err(error) => {
                return Err(MaterialError::new(
                    MaterialErrorKind::Io,
                    format!(
                        "画像を読めない: {slot}: {property}: {}",
                        crate::escape_diag(&error.to_string())
                    ),
                )
                .with_path(path)
                .with_source(error));
            }
        };
        let Some(source) = sniff_source(&bytes) else {
            self.warn(format!(
                "未対応の画像形式のため省く: {slot}: {property}: {path}"
            ));
            return Ok(None);
        };
        // PSDは保存済みcompositeを復号し、RGBA8から直接焼き入力にする。
        // 中間PNGを作らない。PNG・JPEGの復号経路は変えない。
        if source == SourceImageFormat::Psd {
            let srgb = srgb_of(&record.meta);
            let sampler = sampler_of(&record.meta);
            let (max_size, npot, unknown_npot) = import_limit_of(&record.meta);
            if unknown_npot {
                self.warn(format!(
                    "NPOT指定が読めないためそのままの寸法で扱う: {slot}: {property}"
                ));
            }
            return self
                .bake_psd_input(
                    slot, property, &path, guid, bytes, srgb, sampler, st, channel, invert,
                    max_size, npot,
                )
                .map_err(|error| {
                    if error.path().is_none() {
                        error.with_path(path.clone())
                    } else {
                        error
                    }
                });
        }
        let mime = mime_of(source).expect("PSD以外は埋め込みMIMEを持つ");
        let srgb = srgb_of(&record.meta);
        let sampler = sampler_of(&record.meta);
        let (max_size, npot, unknown_npot) = import_limit_of(&record.meta);
        if unknown_npot {
            self.warn(format!(
                "NPOT指定が読めないためそのままの寸法で扱う: {slot}: {property}"
            ));
        }
        self.bake_input(
            slot, property, guid, bytes, mime, srgb, sampler, st, channel, invert, max_size, npot,
        )
        .map_err(|error| {
            // 画素上限の超過は対象パスを付けて失敗として返す。
            // 復号失敗の警告は `bake_input` 側が残す。
            if error.path().is_none() {
                error.with_path(path.clone())
            } else {
                error
            }
        })
    }

    /// 焼き出力を画像列へ載せ、索引を返す。同じ内容は共有する。
    ///
    /// 名前は `bake:{slot}:{用途}` であり、決定的である。
    /// 内容ハッシュ（SHA-256）が一致する出力は同じ索引を共有する。
    fn push_baked_image(
        &mut self,
        slot: &str,
        target: &str,
        bytes: Vec<u8>,
    ) -> Result<usize, MaterialError> {
        use sha2::Digest as _;
        let mut hash = sha2::Sha256::new();
        hash.update(&bytes);
        let digest: [u8; 32] = hash.finalize().into();
        if let Some(&index) = self.baked_outputs.get(&digest) {
            return Ok(index);
        }
        let total = self.total_image_bytes.saturating_add(bytes.len() as u64);
        if total > self.options.max_total_image_bytes {
            return Err(MaterialError::new(
                MaterialErrorKind::LimitExceeded,
                format!("画像の合計が上限を超えた: {slot}: 焼き込み"),
            ));
        }
        let index = self.images.len();
        self.images.push(VrmImage {
            name: format!("bake:{slot}:{target}"),
            mime: VrmImageMime::Png,
            data: bytes,
            sampler: VrmSampler::default(),
        });
        self.baked_outputs.insert(digest, index);
        self.total_image_bytes = total;
        Ok(index)
    }

    /// 基準色を焼いて画像索引を返す（`BaseColorComposite` と対応）。
    ///
    /// 主入力とアルファマスクだけを合成する。色調補正・カラーレイヤー・
    /// デカールは対象外であり、呼び出し側が焼かない判断をする。
    fn bake_base_image(
        &mut self,
        material: &crate::unity::Material,
        slot: &str,
        main_st: &BakeSt,
    ) -> Result<(usize, Option<Rc<BakeImage>>), MaterialError> {
        let source = self.bake_input_of(material, slot, "_MainTex", BakeChannel::Rgb, false)?;
        let mask_mode = number(material, "_AlphaMaskMode").unwrap_or(0.0) as i64;
        let alpha_mask =
            match self.bake_input_of(material, slot, "_AlphaMask", BakeChannel::R, false)? {
                Some(mask) => {
                    let own = bake_st_of(texture(material, "_AlphaMask"));
                    let composed = compose_st(main_st, &own);
                    if !composed.is_identity() {
                        self.warn(format!(
                            "アルファマスクのスケール・オフセットは反映しない: {slot}"
                        ));
                    }
                    Some(AlphaMaskInput {
                        mask: BakeInput {
                            st: composed,
                            ..mask
                        },
                        mode: mask_mode,
                        scale: number(material, "_AlphaMaskScale").unwrap_or(1.0) as f32,
                        value: number(material, "_AlphaMaskValue").unwrap_or(0.0) as f32,
                    })
                }
                None => None,
            };
        let mut extents = Vec::new();
        if let Some(source) = &source {
            extents.push((
                source.image.width(),
                source.image.height(),
                source.st.scale[0].abs(),
                source.st.scale[1].abs(),
            ));
        }
        if let Some(alpha) = &alpha_mask {
            extents.push((
                alpha.mask.image.width(),
                alpha.mask.image.height(),
                alpha.mask.st.scale[0].abs(),
                alpha.mask.st.scale[1].abs(),
            ));
        }
        let output_id = format!("bake:{slot}:BaseColor");
        let size = bake_output_size(&extents, &output_id)?;
        let pixels = bake_base_color(&BaseColorJob {
            source,
            base_tint: [1.0, 1.0, 1.0, 1.0],
            hsvg: None,
            color_adjust_mask: None,
            alpha_mask,
            target: [1.0, 1.0, 0.0, 0.0],
            size,
        });
        let bytes = encode_color_png(&pixels, size);
        let index = self.push_baked_image(slot, "BaseColor", bytes.clone())?;
        // 焼き結果の復号は後の陰影・発光のアルベド入力に使い回す。
        // 埋め込んだバイト列そのものを復号するため、参照実装が焼き済み
        // テクスチャを読むのと同じ内容になる。
        let albedo = load_bake_image(
            &bytes,
            VrmImageMime::Png,
            true,
            VrmSampler::default(),
            self.options.max_image_pixels,
        )
        .map(Rc::new)
        .ok();
        Ok((index, albedo))
    }

    /// 陰影を写像する（`MapShade` と対応）。
    ///
    /// 焼く場合は係数を白にして焼き画像を返し、焼かない場合は
    /// 従来どおり係数へ載せる。
    fn map_shade(
        &mut self,
        material: &crate::unity::Material,
        slot: &str,
        base: &[f64; 4],
        base_color_image: Option<usize>,
        baked_albedo: Option<Rc<BakeImage>>,
        main_st: &BakeSt,
    ) -> Result<([f32; 3], f32, f32, Option<usize>), MaterialError> {
        let border = clamp01(number(material, "_ShadowBorder").unwrap_or(0.5));
        let blur = clamp01(number(material, "_ShadowBlur").unwrap_or(0.1));
        let low = clamp01(border - blur / 2.0);
        let high = clamp01(border + blur / 2.0);
        // MToonのlinearstepは幅0で割れないため、硬い境界は中心まわりに広げる。
        let toony = 1.0 - (high - low).max(0.001);
        let shift = 1.0 - (low + high);
        let strength = clamp01(number(material, "_ShadowStrength").unwrap_or(1.0));
        // lilToonの既定影色（`_ShadowColor` 未設定時の代替値）。
        let shadow = color(material, "_ShadowColor").unwrap_or([0.82, 0.76, 0.85, 1.0]);
        let shadow_lin = [
            srgb_to_linear(shadow[0]),
            srgb_to_linear(shadow[1]),
            srgb_to_linear(shadow[2]),
        ];
        // 参照実装の `BlendedShadeColor` と対応する。2層目以降は
        // アルファが引き込み量であり、色テクスチャは色で代用する。
        let extra = shade_extra_layers(material);
        let weights = shade_layer_weights(
            border as f32,
            blur as f32,
            &extra
                .iter()
                .map(|layer| (layer.border, layer.blur, layer.alpha))
                .collect::<Vec<_>>(),
        );
        let mut blended = shadow_lin;
        if !extra.is_empty() {
            let first = weights[0] as f32;
            let mut rest = [0.0f32; 3];
            for (layer, weight) in extra.iter().zip(weights.iter().skip(1)) {
                rest[0] += *weight as f32 * layer.color[0];
                rest[1] += *weight as f32 * layer.color[1];
                rest[2] += *weight as f32 * layer.color[2];
            }
            blended = [
                shadow_lin[0] * first + rest[0],
                shadow_lin[1] * first + rest[1],
                shadow_lin[2] * first + rest[2],
            ];
        }
        let tint = [
            (1.0 - (1.0 - blended[0]) * strength as f32),
            (1.0 - (1.0 - blended[1]) * strength as f32),
            (1.0 - (1.0 - blended[2]) * strength as f32),
        ];
        // 参照実装の `ReadShadowMask` と対応する。Flatは陰影を平らにせず、
        // SDFは通常陰影として扱い、強さだけAチャンネルで読む。
        let mask_type = number(material, "_ShadowMaskType").unwrap_or(0.0) as i64;
        let strength_mask = match mask_type {
            1 => {
                if has_texture(material, "_ShadowStrengthMask") {
                    self.warn(format!("Flatマスクは通常陰影として写像する: {slot}"));
                }
                None
            }
            2 => {
                let version = number(material, "_lilToonVersion").unwrap_or(0.0) as i64;
                self.warn(format!("SDFマスクは通常陰影として写像する: {slot}"));
                if version >= 45 {
                    self.bake_input_of(
                        material,
                        slot,
                        "_ShadowStrengthMask",
                        BakeChannel::A,
                        false,
                    )?
                } else {
                    None
                }
            }
            _ => {
                self.bake_input_of(material, slot, "_ShadowStrengthMask", BakeChannel::R, false)?
            }
        };
        let main_strength = number(material, "_ShadowMainStrength").unwrap_or(0.0) as f32;
        let shadow_tex = has_texture(material, "_ShadowColorTex");
        let bake =
            strength_mask.is_some() || main_strength != 0.0 || (shadow_tex && !extra.is_empty());
        if !bake {
            if shadow_tex {
                self.warn(format!(
                    "影色テクスチャは基準色との混合ではなく乗算として近似する: {slot}"
                ));
                let image = self.resolve_texture(material, slot, "_ShadowColorTex")?;
                return Ok((tint, shift as f32, toony as f32, image));
            }
            // 影色テクスチャが無い場合、影色と基準色の積を影側の色にする。
            let mixed = [
                tint[0] * base[0] as f32,
                tint[1] * base[1] as f32,
                tint[2] * base[2] as f32,
            ];
            return Ok((mixed, shift as f32, toony as f32, base_color_image));
        }
        if !main_st.is_identity() {
            self.warn(format!(
                "陰影の焼き込みに未対応の変換があるため係数へ載せる: {slot}"
            ));
            if shadow_tex {
                let image = self.resolve_texture(material, slot, "_ShadowColorTex")?;
                return Ok((tint, shift as f32, toony as f32, image));
            }
            let mixed = [
                tint[0] * base[0] as f32,
                tint[1] * base[1] as f32,
                tint[2] * base[2] as f32,
            ];
            return Ok((mixed, shift as f32, toony as f32, base_color_image));
        }
        // 焼く場合のアルベドは焼いた基準色があればそれを使い、
        // なければ主テクスチャをそのまま読む（参照実装の `AlbedoInput` と対応）。
        // 焼いた基準色を使う場合は係数も白にする（`AlbedoColor` と対応）。
        let (albedo, albedo_color) = match baked_albedo {
            Some(image) => (
                Some(BakeInput {
                    image,
                    st: BakeSt::identity(),
                    channel: BakeChannel::Rgb,
                    invert: false,
                }),
                [1.0, 1.0, 1.0, 1.0],
            ),
            None => (
                self.bake_input_of(material, slot, "_MainTex", BakeChannel::Rgb, false)?,
                [
                    base[0] as f32,
                    base[1] as f32,
                    base[2] as f32,
                    base[3] as f32,
                ],
            ),
        };
        let source =
            self.bake_input_of(material, slot, "_ShadowColorTex", BakeChannel::Rgb, false)?;
        let mut extents = Vec::new();
        for input in [&source, &albedo, &strength_mask].into_iter().flatten() {
            extents.push((
                input.image.width(),
                input.image.height(),
                input.st.scale[0].abs(),
                input.st.scale[1].abs(),
            ));
        }
        let output_id = format!("bake:{slot}:ShadeMultiply");
        let size = bake_output_size(&extents, &output_id)?;
        let first_weight = if extra.is_empty() {
            1.0
        } else {
            weights[0] as f32
        };
        let mut rest = [0.0f32; 3];
        for (layer, weight) in extra.iter().zip(weights.iter().skip(1)) {
            rest[0] += *weight as f32 * layer.color[0];
            rest[1] += *weight as f32 * layer.color[1];
            rest[2] += *weight as f32 * layer.color[2];
        }
        let shadow_color_lin = color(material, "_ShadowColor").unwrap_or([0.82, 0.76, 0.85, 1.0]);
        let pixels = bake_shade(&ShadeJob {
            source,
            albedo,
            albedo_color,
            shadow_color: [
                srgb_to_linear(shadow_color_lin[0]),
                srgb_to_linear(shadow_color_lin[1]),
                srgb_to_linear(shadow_color_lin[2]),
                shadow_color_lin[3] as f32,
            ],
            strength: strength as f32,
            shadow_weight: first_weight,
            shadow_extra: rest,
            main_strength,
            strength_mask,
            target: [1.0, 1.0, 0.0, 0.0],
            size,
        });
        let bytes = encode_color_png(&pixels, size);
        let image = self.push_baked_image(slot, "ShadeMultiply", bytes)?;
        Ok(([1.0, 1.0, 1.0], shift as f32, toony as f32, Some(image)))
    }

    /// 発光を写像する（`MapEmission` と `MapEmissionWithMatcap` に対応）。
    ///
    /// 焼く場合は係数を白にして焼き画像を返し、焼かない場合は
    /// 従来どおり係数へ載せる。
    fn map_emission(
        &mut self,
        material: &crate::unity::Material,
        slot: &str,
        main_st: &BakeSt,
        base: &[f64; 4],
        baked_albedo: Option<Rc<BakeImage>>,
    ) -> Result<([f32; 3], Option<usize>), MaterialError> {
        let use_emission = number(material, "_UseEmission").unwrap_or(0.0) != 0.0;
        let fold = self.matcap_fold(material, slot, main_st, use_emission)?;
        // 焼いた基準色があればアルベドに使い、係数は白にする。
        let baked_albedo_input = baked_albedo.map(|image| BakeInput {
            image,
            st: BakeSt::identity(),
            channel: BakeChannel::Rgb,
            invert: false,
        });
        let albedo_color = if baked_albedo_input.is_some() {
            [1.0, 1.0, 1.0, 1.0]
        } else {
            [
                base[0] as f32,
                base[1] as f32,
                base[2] as f32,
                base[3] as f32,
            ]
        };
        if let Some(fold) = fold {
            // 参照実装の `MapEmissionWithMatcap` と対応する。発光が無効な場合は
            // 始点を定数にするため、発光マップは読まない。
            let source = if use_emission {
                self.bake_input_of(material, slot, "_EmissionMap", BakeChannel::Rgb, false)?
            } else {
                None
            };
            let main_strength = number(material, "_EmissionMainStrength").unwrap_or(0.0) as f32;
            // 参照実装の `ApplyEmissionInput` と対応する。アルベドは主強度がある
            // 場合だけ結び、焼きの寸法にもその場合だけ数える。
            let albedo = if main_strength != 0.0 {
                match baked_albedo_input.clone() {
                    Some(input) => Some(input),
                    None => {
                        self.bake_input_of(material, slot, "_MainTex", BakeChannel::Rgb, false)?
                    }
                }
            } else {
                None
            };
            let mask = self
                .bake_input_of(
                    material,
                    slot,
                    "_EmissionBlendMask",
                    BakeChannel::Rgb,
                    false,
                )?
                .map(|input| EmissionMask {
                    input,
                    scalar: false,
                });
            // 参照実装の `SizeFrom` と対応する。折り込むMatCap本体の寸法は
            // 視線で標本するため数えず、混合マスクだけを数える。
            let mut extents = Vec::new();
            for input in [&source, &albedo].into_iter().flatten() {
                extents.push((
                    input.image.width(),
                    input.image.height(),
                    input.st.scale[0].abs(),
                    input.st.scale[1].abs(),
                ));
            }
            if let Some(mask) = &mask {
                extents.push((
                    mask.input.image.width(),
                    mask.input.image.height(),
                    mask.input.st.scale[0].abs(),
                    mask.input.st.scale[1].abs(),
                ));
            }
            if let Some(fold_mask) = &fold.mask {
                extents.push((
                    fold_mask.image.width(),
                    fold_mask.image.height(),
                    fold_mask.st.scale[0].abs(),
                    fold_mask.st.scale[1].abs(),
                ));
            }
            let output_id = format!("bake:{slot}:Emissive");
            let size = bake_output_size(&extents, &output_id)?;
            let (emission_factor, clamped) = emission_linear(material);
            if clamped {
                self.warn(format!("発光色が1を超えるため1に丸める: {slot}"));
            }
            let emission_tint = if use_emission {
                emission_factor
            } else {
                [0.0, 0.0, 0.0]
            };
            let peak = (fold.color[0] + emission_tint[0])
                .max(fold.color[1] + emission_tint[1])
                .max(fold.color[2] + emission_tint[2]);
            if peak > 1.0 {
                self.warn(format!("発光の焼き込みは1で頭打ちになる: {slot}"));
            }
            let pixels = bake_emission(&EmissionJob {
                source,
                emission_tint,
                albedo,
                albedo_color,
                main_strength,
                mask,
                scale: None,
                fold_color: Some(fold.color),
                fold_mask: fold.mask.map(|input| EmissionMask {
                    input,
                    scalar: false,
                }),
                target: [1.0, 1.0, 0.0, 0.0],
                size,
            });
            let bytes = encode_color_png(&pixels, size);
            let image = self.push_baked_image(slot, "Emissive", bytes)?;
            return Ok(([1.0, 1.0, 1.0], Some(image)));
        }
        if !use_emission {
            return Ok(([0.0, 0.0, 0.0], None));
        }
        let main_strength = number(material, "_EmissionMainStrength").unwrap_or(0.0);
        let blend_mask = has_texture(material, "_EmissionBlendMask");
        if (main_strength != 0.0 || blend_mask) && main_st.is_identity() {
            let source =
                self.bake_input_of(material, slot, "_EmissionMap", BakeChannel::Rgb, false)?;
            // 参照実装の `ApplyEmissionInput` と対応する。アルベドは主強度がある
            // 場合だけ結び、焼きの寸法にもその場合だけ数える。
            let albedo = if main_strength != 0.0 {
                match baked_albedo_input.clone() {
                    Some(input) => Some(input),
                    None => {
                        self.bake_input_of(material, slot, "_MainTex", BakeChannel::Rgb, false)?
                    }
                }
            } else {
                None
            };
            let mask = self
                .bake_input_of(
                    material,
                    slot,
                    "_EmissionBlendMask",
                    BakeChannel::Rgb,
                    false,
                )?
                .map(|input| EmissionMask {
                    input,
                    scalar: false,
                });
            // 焼かない係数に色を残す場合、焼きの発光色は白である
            // （`BakeRequest.EmissionTint` が空の場合と対応）。
            // 寸法は参照実装の `SizeFrom` と同じく始点・強さ・混合マスクだけを数える。
            let mut extents = Vec::new();
            for input in [&source, &albedo].into_iter().flatten() {
                extents.push((
                    input.image.width(),
                    input.image.height(),
                    input.st.scale[0].abs(),
                    input.st.scale[1].abs(),
                ));
            }
            if let Some(mask) = &mask {
                extents.push((
                    mask.input.image.width(),
                    mask.input.image.height(),
                    mask.input.st.scale[0].abs(),
                    mask.input.st.scale[1].abs(),
                ));
            }
            let output_id = format!("bake:{slot}:Emissive");
            let size = bake_output_size(&extents, &output_id)?;
            let pixels = bake_emission(&EmissionJob {
                source,
                emission_tint: [1.0, 1.0, 1.0],
                albedo,
                albedo_color,
                main_strength: main_strength as f32,
                mask,
                scale: None,
                fold_color: None,
                fold_mask: None,
                target: [1.0, 1.0, 0.0, 0.0],
                size,
            });
            let bytes = encode_color_png(&pixels, size);
            let image = self.push_baked_image(slot, "Emissive", bytes)?;
            let (factor, clamped) = emission_linear(material);
            if clamped {
                self.warn(format!("発光色が1を超えるため1に丸める: {slot}"));
            }
            return Ok((factor, Some(image)));
        }
        let (factor, clamped) = emission_linear(material);
        if clamped {
            self.warn(format!("発光色が1を超えるため1に丸める: {slot}"));
        }
        let image = self.resolve_texture(material, slot, "_EmissionMap")?;
        Ok((factor, image))
    }

    /// 発光へ折り込むMatCap2ndを求める（`FoldMatcap2nd` と対応）。
    ///
    /// 枠が空いている場合や乗算・発光超過で折り込めない場合は `None` を返し、
    /// 省く理由を警告する。発光が無効でも折り込みは行う。
    fn matcap_fold(
        &mut self,
        material: &crate::unity::Material,
        slot: &str,
        main_st: &BakeSt,
        use_emission: bool,
    ) -> Result<Option<MatcapFold>, MaterialError> {
        if matcap_slot(material) != MatcapSlot::First
            || !has_matcap(material, "_MatCap2nd")
            || !matcap_contributes(material, "_MatCap2nd")
        {
            return Ok(None);
        }
        // 枠の所有権・リムとの両立・混合方式・発光超過の順に判定する。
        if matcap_costs_the_rim(material) && !matcap_left_out(material, "_MatCap2nd") {
            self.warn(format!("MatCap2ndはリムと両立しないため省く: {slot}"));
            return Ok(None);
        }
        let mode = number(material, "_MatCap2ndBlendMode").unwrap_or(1.0) as i64;
        if mode == 3 {
            self.warn(format!(
                "MatCap2ndの乗算は発光へ折り込めないため省く: {slot}"
            ));
            return Ok(None);
        }
        if use_emission {
            let (factor, _) = emission_linear(material);
            let strength = number(material, "_EmissionBlend").unwrap_or(1.0);
            if factor[0] * strength as f32 > 1.0
                || factor[1] * strength as f32 > 1.0
                || factor[2] * strength as f32 > 1.0
            {
                self.warn(format!(
                    "発光が1を超えるためMatCap2ndは折り込まない: {slot}"
                ));
                return Ok(None);
            }
        }
        let source =
            self.bake_input_of(material, slot, "_MatCap2ndTex", BakeChannel::Rgb, false)?;
        let Some(fold_texture) = source else {
            return Ok(None);
        };
        let weight = matcap_weight(material, "_MatCap2nd");
        let average = matcap_disc_average(&fold_texture.image, &fold_texture.st);
        let color = [
            average[0] * weight[0],
            average[1] * weight[1],
            average[2] * weight[2],
        ];
        let mask = match self.bake_input_of(
            material,
            slot,
            "_MatCap2ndBlendMask",
            BakeChannel::Rgb,
            false,
        )? {
            Some(input) => {
                let own = bake_st_of(texture(material, "_MatCap2ndBlendMask"));
                let composed = compose_st(main_st, &own);
                Some(BakeInput {
                    st: composed,
                    ..input
                })
            }
            None => None,
        };
        Ok(Some(MatcapFold { color, mask }))
    }

    /// リム倍率テクスチャ1件の読み先を作る（`AtMainUvTiled` と対応）。
    ///
    /// 主変換の後に個別変換を合成し、GUID・`_ST`・チャンネル・反転で
    /// 同一性を比べられるように持つ。読めない場合は警告して `None` を返す。
    fn rim_factor_input(
        &mut self,
        material: &crate::unity::Material,
        slot: &str,
        main_st: &BakeSt,
        property: &str,
    ) -> Result<Option<RimFactor>, MaterialError> {
        let own = bake_st_of(texture(material, property));
        let composed = compose_st(main_st, &own);
        let Some(input) = self.bake_input_of(material, slot, property, BakeChannel::Rgb, false)?
        else {
            return Ok(None);
        };
        let plain = !input.invert;
        let texture_id = texture(material, property)
            .and_then(|environment| environment.texture.guid.clone())
            .map(|guid| guid.as_str().to_owned());
        Ok(Some(RimFactor {
            input: BakeInput {
                st: composed,
                ..input
            },
            property: property.to_owned(),
            texture_id,
            plain,
        }))
    }

    /// 共有1要素の係数とtintを決める（`ShareOf` と対応）。
    ///
    /// tintが定数なら係数へ折り込み、非定数で基準色テクスチャが無い場合は
    /// 基準色での値を係数へ折り込む。基準色テクスチャがある場合だけ
    /// tintを残し、係数には最大値を掛ける。
    // 引数は共有要素の組み立てにそのまま使うため束ねない。
    #[allow(clippy::too_many_arguments)]
    fn share_of(
        name: &'static str,
        color: [f32; 3],
        textures: Vec<RimFactor>,
        tint: AlbedoTint,
        lighting_mix: f32,
        masks_only: bool,
        base_rgb: [f32; 3],
        has_base_texture: bool,
    ) -> RimShare {
        if tint.is_constant() {
            return RimShare {
                name,
                textures,
                tint: None,
                factor: [
                    color[0] * tint.constant,
                    color[1] * tint.constant,
                    color[2] * tint.constant,
                ],
                lighting_mix,
                masks_only,
            };
        }
        if !has_base_texture {
            let tinted = tint.at_color(base_rgb);
            return RimShare {
                name,
                textures,
                tint: None,
                factor: [
                    color[0] * tinted[0],
                    color[1] * tinted[1],
                    color[2] * tinted[2],
                ],
                lighting_mix,
                masks_only,
            };
        }
        let peak = tint.peak();
        RimShare {
            name,
            textures,
            tint: Some(tint),
            factor: [color[0] * peak, color[1] * peak, color[2] * peak],
            lighting_mix,
            masks_only,
        }
    }

    /// リム光とMatCapのリム倍率の計画を作る（`PlanRimMultiply` と対応）。
    ///
    /// lilToonではリム光（tint無し）とMatCap（`_MatCapMainStrength` の
    /// アルベドtint）の2要素だけを扱う。参照実装の反転リム・減光・
    /// テクスチャ上限の切り捨ては対象外であり、3件を超える構成は無い。
    // 引数は計画の組み立てにそのまま使うため束ねない。
    #[allow(clippy::too_many_arguments)]
    fn plan_rim_multiply(
        &mut self,
        material: &crate::unity::Material,
        slot: &str,
        use_rim: bool,
        rim_color: [f32; 3],
        rim_lighting_mix: f32,
        matcap_prefix: Option<&str>,
        matcap_exported: bool,
        base_rgb: &[f32; 3],
        base_color_image: Option<usize>,
        main_st: &BakeSt,
    ) -> Result<RimMultiplyPlan, MaterialError> {
        let rim = if use_rim {
            let textures = match self.rim_factor_input(material, slot, main_st, "_RimColorTex")? {
                Some(input) => vec![input],
                None => Vec::new(),
            };
            Some(Self::share_of(
                "リム光",
                rim_color,
                textures,
                AlbedoTint::none(),
                rim_lighting_mix,
                false,
                *base_rgb,
                base_color_image.is_some(),
            ))
        } else {
            None
        };
        let matcap = match (matcap_prefix, matcap_exported) {
            (Some(prefix), true) => {
                let textures = match self.rim_factor_input(
                    material,
                    slot,
                    main_st,
                    &format!("{prefix}BlendMask"),
                )? {
                    Some(input) => vec![input],
                    None => Vec::new(),
                };
                let multiplies =
                    number(material, &format!("{prefix}BlendMode")).unwrap_or(1.0) as i64 == 3;
                let mix = clamp01(number(material, &format!("{prefix}MainStrength")).unwrap_or(0.0))
                    as f32;
                // 乗算は下の色を掛けて足し戻し、加算系は混ぜ具合だけを見る。
                let tint = AlbedoTint {
                    mix,
                    constant: if multiplies { 0.0 } else { 1.0 },
                    albedo: if multiplies { 1.0 } else { 0.0 },
                };
                Some(Self::share_of(
                    "MatCap",
                    matcap_weight(material, prefix),
                    textures,
                    tint,
                    if multiplies {
                        1.0
                    } else {
                        clamp01(number(material, &format!("{prefix}EnableLighting")).unwrap_or(1.0))
                            as f32
                    },
                    true,
                    *base_rgb,
                    base_color_image.is_some(),
                ))
            }
            _ => None,
        };
        // lilToonでは反転リムが無いため `matcap_factor` はMatCap層だけである
        // （参照実装の `ComposeMatcap` と対応）。
        let matcap_factor = matcap
            .as_ref()
            .map_or([0.0, 0.0, 0.0], |share| share.factor);
        let mut plan = RimMultiplyPlan {
            rim,
            matcap,
            textures: Vec::new(),
            tint: None,
            tinted: Vec::new(),
            gain: 1.0,
            matcap_factor,
        };
        self.share_tint(slot, &mut plan);
        self.report_shared_textures(slot, &plan);
        for share in [&plan.rim, &plan.matcap].into_iter().flatten() {
            for texture in &share.textures {
                if !plan.textures.iter().any(|known| texture.same_as(known)) {
                    plan.textures.push(texture.clone());
                }
            }
        }
        self.move_brightness_into_texture(slot, &mut plan);
        Ok(plan)
    }

    /// 共有tintの持ち主を決める（`ShareTint` と対応）。
    ///
    /// lilToonではtintはMatCapだけが持つ。単独ならそのまま運び、
    /// リムと両立する場合はMatCapを所有者にし、リムにも同じtintが
    /// 掛かる近似として警告する。参照実装の `TintRatio` はtint無しの
    /// 追従では100%になるため、係数は変えない。
    fn share_tint(&mut self, slot: &str, plan: &mut RimMultiplyPlan) {
        let rim_tint = plan.rim.as_ref().and_then(|share| share.tint);
        let matcap_tint = plan.matcap.as_ref().and_then(|share| share.tint);
        match (rim_tint, matcap_tint) {
            (Some(rim), Some(matcap)) if rim.same_shape(matcap) => {
                // 同じ変わり方をするtintはそのまま運ぶ。
                plan.tint = Some(matcap);
                plan.tinted = vec!["リム光", "MatCap"];
            }
            (Some(_), Some(_)) => {
                // lilToonの読み取りでは起きない組み合わせであり、先勝ちにしない。
                // 両方にtintがある場合はMatCapを所有者にする。
                plan.tint = matcap_tint;
                plan.tinted = vec!["MatCap"];
                self.warn(format!(
                    "MToonはリムとMatCapをひとつのテクスチャで乗算するため、リムとMatCapに同じアルベドtint（Main Color Power）が掛かる近似になる: {slot}"
                ));
            }
            (None, Some(_)) => {
                plan.tint = matcap_tint;
                plan.tinted = vec!["MatCap"];
                if plan.rim.is_some() {
                    self.warn(format!(
                        "MToonはリムとMatCapをひとつのテクスチャで乗算するため、リムにもMatCapのアルベドtint（Main Color Power）が掛かる近似になる: {slot}"
                    ));
                }
            }
            (Some(_), None) => {
                plan.tint = rim_tint;
                plan.tinted = vec!["リム光"];
            }
            (None, None) => {}
        }
    }

    /// 片方だけが持つテクスチャを報告する（`ReportSharedTextures` と対応）。
    ///
    /// MToonはリムとMatCapを1枚で乗算するため、相手にも掛かる旨を警告する。
    fn report_shared_textures(&mut self, slot: &str, plan: &RimMultiplyPlan) {
        let shares: Vec<&RimShare> = [&plan.rim, &plan.matcap].into_iter().flatten().collect();
        if shares.len() < 2 {
            return;
        }
        let mut parts = Vec::new();
        for (index, owner) in shares.iter().enumerate() {
            let own: Vec<&RimFactor> = owner
                .textures
                .iter()
                .filter(|texture| {
                    !shares[..index]
                        .iter()
                        .any(|earlier| earlier.textures.iter().any(|known| texture.same_as(known)))
                })
                .collect();
            for (other_index, other) in shares.iter().enumerate() {
                if other_index == index {
                    continue;
                }
                let foreign: Vec<&RimFactor> = own
                    .iter()
                    .copied()
                    .filter(|texture| !other.textures.iter().any(|known| texture.same_as(known)))
                    .collect();
                if foreign.is_empty() {
                    continue;
                }
                let kinds: Vec<&str> = foreign
                    .iter()
                    .map(|texture| texture.kind(owner.masks_only))
                    .collect();
                let kinds = kinds.join("と");
                parts.push(format!("{}にも{}の{kinds}が掛かる", other.name, owner.name));
            }
        }
        if parts.is_empty() {
            return;
        }
        self.warn(format!(
            "MToonはリム光とMatCapを1枚のテクスチャで乗算するため、{}: {slot}",
            parts.join("、")
        ));
    }

    /// 1超の明るさをテクスチャへ移す（`MoveBrightnessIntoTexture` と対応）。
    ///
    /// 両方がある場合は丸めた係数が相手も暗くするため何もしない。
    /// 片方だけの場合は最大値で割り、テクスチャ側で掛け直す。
    fn move_brightness_into_texture(&mut self, slot: &str, plan: &mut RimMultiplyPlan) {
        if !plan.per_texel() {
            return;
        }
        if plan.rim.is_some() && plan.matcap.is_some() {
            return;
        }
        let peak = if let Some(rim) = &plan.rim {
            rim.factor[0].max(rim.factor[1]).max(rim.factor[2])
        } else {
            plan.matcap_factor[0]
                .max(plan.matcap_factor[1])
                .max(plan.matcap_factor[2])
        };
        if peak <= 1.0 {
            return;
        }
        plan.gain = peak;
        if let Some(rim) = plan.rim.as_mut() {
            rim.factor = [
                rim.factor[0] / peak,
                rim.factor[1] / peak,
                rim.factor[2] / peak,
            ];
            self.warn(format!(
                "リム光の明るさが1を超えるためテクスチャへ移して上限で切る: {slot}"
            ));
        } else {
            plan.matcap_factor = [
                plan.matcap_factor[0] / peak,
                plan.matcap_factor[1] / peak,
                plan.matcap_factor[2] / peak,
            ];
            self.warn(format!(
                "MatCapの明るさが1を超えるためテクスチャへ移して上限で切る: {slot}"
            ));
        }
    }

    /// 色係数を0〜1へ丸める（`ClampColor` と対応）。
    ///
    /// 範囲外の場合は警告する。
    fn clamp_rim_factor(&mut self, slot: &str, field: &str, factor: [f32; 3]) -> [f32; 3] {
        let clamped = [
            factor[0].clamp(0.0, 1.0),
            factor[1].clamp(0.0, 1.0),
            factor[2].clamp(0.0, 1.0),
        ];
        if clamped != factor {
            self.warn(format!("{field}が0〜1の範囲外のため丸める: {slot}"));
        }
        clamped
    }

    /// リム倍率テクスチャを結び付ける（`BindRimMultiply` と対応）。
    ///
    /// 画素ごとの内容が無い場合は `None`、単位変換の単一の素朴なマスクだけの
    /// 場合は直載せし、それ以外は `RimMultiplyComposite` を焼く。
    /// 焼く場合はアルベドtintと1超の明るさを報告する。
    // 引数は焼きの組み立てにそのまま使うため束ねない。
    #[allow(clippy::too_many_arguments)]
    fn bind_rim_multiply(
        &mut self,
        material: &crate::unity::Material,
        slot: &str,
        plan: &RimMultiplyPlan,
        baked_albedo: Option<Rc<BakeImage>>,
        albedo_base_rgb: [f32; 3],
        base_alpha: f32,
    ) -> Result<Option<usize>, MaterialError> {
        if !plan.per_texel() {
            return Ok(None);
        }
        let gain = plan.gain / plan.tint.map_or(1.0, |tint| tint.peak());
        if plan.tint.is_none()
            && (gain - 1.0).abs() < 1e-6
            && plan.textures.len() == 1
            && plan.textures[0].plain
            && plan.textures[0].input.st.is_identity()
        {
            // 単一の素朴なマスクはそのまま載せる。
            // チャンネル違いは `ChannelRepack` でRGBへ広げるが、
            // lilToonのマスクはRGBのため通常は通らない。
            let property = plan.textures[0].property.clone();
            if plan.textures[0].input.channel == BakeChannel::Rgb {
                return self.resolve_texture(material, slot, &property);
            }
            let source = plan.textures[0].input.clone();
            let output_id = format!("bake:{slot}:RimMultiply");
            let size = bake_output_size(
                &[(
                    source.image.width(),
                    source.image.height(),
                    source.st.scale[0].abs(),
                    source.st.scale[1].abs(),
                )],
                &output_id,
            )?;
            let pixels = bake_channel_repack(&RepackJob {
                source,
                target: [1.0, 1.0, 0.0, 0.0],
                size,
            });
            let bytes = encode_color_png(&pixels, size);
            return Ok(Some(self.push_baked_image(slot, "RimMultiply", bytes)?));
        }
        // 焼く場合のアルベドは焼いた基準色があればそれを使い、
        // なければ主テクスチャをそのまま読む（参照実装の `AlbedoInput` と対応）。
        // アルベド色はどちらの場合も減光前の線形基準色である。vrctomの
        // 基準色ベイクは `_Color` を含めないため、焼き済みでも白にしない。
        let albedo_color = [
            albedo_base_rgb[0],
            albedo_base_rgb[1],
            albedo_base_rgb[2],
            base_alpha,
        ];
        let albedo = match baked_albedo {
            Some(image) => Some(BakeInput {
                image,
                st: BakeSt::identity(),
                channel: BakeChannel::Rgb,
                invert: false,
            }),
            None => self.bake_input_of(material, slot, "_MainTex", BakeChannel::Rgb, false)?,
        };
        let mut extents = Vec::new();
        for texture in &plan.textures {
            extents.push((
                texture.input.image.width(),
                texture.input.image.height(),
                texture.input.st.scale[0].abs(),
                texture.input.st.scale[1].abs(),
            ));
        }
        if plan.tint.is_some()
            && let Some(albedo) = &albedo
        {
            extents.push((
                albedo.image.width(),
                albedo.image.height(),
                albedo.st.scale[0].abs(),
                albedo.st.scale[1].abs(),
            ));
        }
        let output_id = format!("bake:{slot}:RimMultiply");
        let size = bake_output_size(&extents, &output_id)?;
        let mut inputs = plan.textures.iter();
        let pixels = bake_rim_multiply(&RimMultiplyJob {
            source: inputs.next().map(|texture| texture.input.clone()),
            mask: inputs.next().map(|texture| texture.input.clone()),
            scale: inputs.next().map(|texture| texture.input.clone()),
            albedo,
            albedo_color,
            tint: plan.tint,
            gain,
            target: [1.0, 1.0, 0.0, 0.0],
            size,
        });
        let bytes = encode_color_png(&pixels, size);
        let image = self.push_baked_image(slot, "RimMultiply", bytes)?;
        if plan.tint.is_some() {
            let owners = plan.tinted.join("と");
            self.warn(format!(
                "{owners}のアルベドtint（Main Color Power）をリム倍率テクスチャへ焼く: {slot}"
            ));
        }
        Ok(Some(image))
    }

    /// アウトライン幅マスクをGへ載せ替える（`MapOutline` と対応）。
    ///
    /// 割り当てがあるのに読めない場合は警告して省き、書き出しは続ける。
    fn bake_outline_width(
        &mut self,
        material: &crate::unity::Material,
        slot: &str,
    ) -> Result<Option<usize>, MaterialError> {
        let source =
            self.bake_input_of(material, slot, "_OutlineWidthMask", BakeChannel::R, false)?;
        let Some(source) = source else {
            self.warn(format!("アウトライン幅マスクを読めないため省く: {slot}"));
            return Ok(None);
        };
        let output_id = format!("bake:{slot}:OutlineWidthMultiply");
        let size = bake_output_size(
            &[(
                source.image.width(),
                source.image.height(),
                source.st.scale[0].abs(),
                source.st.scale[1].abs(),
            )],
            &output_id,
        )?;
        let pixels = bake_channel_repack(&RepackJob {
            source,
            target: [1.0, 1.0, 0.0, 0.0],
            size,
        });
        let bytes = encode_data_png(&pixels, size);
        Ok(Some(self.push_baked_image(
            slot,
            "OutlineWidthMultiply",
            bytes,
        )?))
    }

    /// 2nd法線を合成して画像索引を返す（`NormalComposite` と対応）。
    ///
    /// 2ndが読めない場合は1stの直載せへ落とし、両方読めない場合は省く。
    fn bake_normal_image(
        &mut self,
        material: &crate::unity::Material,
        slot: &str,
        main_st: &BakeSt,
    ) -> Result<(Option<usize>, f32), MaterialError> {
        let first = self.bake_input_of(material, slot, "_BumpMap", BakeChannel::Rgb, false)?;
        let first_scale = first
            .as_ref()
            .map(|_| number(material, "_BumpScale").unwrap_or(1.0) as f32)
            .unwrap_or(1.0);
        let second = self.bake_input_of(material, slot, "_Bump2ndMap", BakeChannel::Rgb, false)?;
        // 2ndが読めない場合は `NormalComposite` を組まず、1stの直載せへ落とす
        // （参照実装は失敗させるが、書き出しを止めない方を選ぶ）。
        let Some(second) = second else {
            self.warn(format!(
                "2nd法線マップを読めないため1stを直載せする: {slot}"
            ));
            let image = self.resolve_texture(material, slot, "_BumpMap")?;
            let scale = number(material, "_BumpScale").unwrap_or(1.0) as f32;
            return Ok((image, scale));
        };
        let second_scale = number(material, "_Bump2ndScale").unwrap_or(1.0) as f32;
        let scale_mask =
            match self.bake_input_of(material, slot, "_Bump2ndScaleMask", BakeChannel::R, false)? {
                Some(input) => {
                    let own = bake_st_of(texture(material, "_Bump2ndScaleMask"));
                    let composed = compose_st(main_st, &own);
                    Some(BakeInput {
                        st: composed,
                        ..input
                    })
                }
                None => None,
            };
        let second_opt = Some(second);
        let mut extents = Vec::new();
        for input in [&first, &second_opt].into_iter().flatten() {
            extents.push((
                input.image.width(),
                input.image.height(),
                input.st.scale[0].abs(),
                input.st.scale[1].abs(),
            ));
        }
        if let Some(mask) = &scale_mask {
            extents.push((
                mask.image.width(),
                mask.image.height(),
                mask.st.scale[0].abs(),
                mask.st.scale[1].abs(),
            ));
        }
        let output_id = format!("bake:{slot}:Normal");
        let size = bake_output_size(&extents, &output_id)?;
        let pixels = bake_normal(&NormalJob {
            first,
            first_scale,
            second: second_opt,
            second_scale,
            scale_mask,
            target: [1.0, 1.0, 0.0, 0.0],
            size,
        });
        let bytes = encode_data_png(&pixels, size);
        let index = self.push_baked_image(slot, "Normal", bytes)?;
        Ok((Some(index), 1.0))
    }
}

/// 発光へ折り込むMatCap2ndの内容。
struct MatcapFold {
    /// 平均色×重み（線形）。
    color: [f32; 3],
    /// 平均色に掛けるマスク（RGB）。
    mask: Option<BakeInput>,
}

/// リム光やMatCapが混ぜ先のアルベドで変わる度合い（`AlbedoTint` と対応）。
///
/// 式は `lerp(1, albedo, mix) * (constant + albedo係数 * albedo)` であり、
/// lilToonではtintはMatCapの `_MatCapMainStrength` だけが持つ。
/// リムはtintを持たない。`None`相当は呼び出し側で `Option` の無しとして扱う。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct AlbedoTint {
    /// 基準色の混ざり具合（0〜1）。
    pub(crate) mix: f32,
    /// アルベドに依らない持ち分。
    pub(crate) constant: f32,
    /// アルベドに比例する持ち分。
    pub(crate) albedo: f32,
}

impl AlbedoTint {
    /// tint無し（定数1）。
    fn none() -> Self {
        Self {
            mix: 0.0,
            constant: 1.0,
            albedo: 0.0,
        }
    }

    /// アルベドに依らず一定かどうか。
    pub(crate) fn is_constant(self) -> bool {
        self.mix == 0.0 && self.albedo == 0.0
    }

    /// アルベド1成分に対する倍率。
    pub(crate) fn at(self, albedo: f32) -> f32 {
        (1.0 - self.mix + self.mix * albedo) * (self.constant + self.albedo * albedo)
    }

    /// アルベドRGBに対する倍率。
    pub(crate) fn at_color(self, albedo: [f32; 3]) -> [f32; 3] {
        [self.at(albedo[0]), self.at(albedo[1]), self.at(albedo[2])]
    }

    /// アルベド0〜1での最大倍率。
    ///
    /// 式はアルベドの二次式であり、頂点が0〜1に入る場合はそこも見る。
    pub(crate) fn peak(self) -> f32 {
        let mut peak = self.at(0.0).max(self.at(1.0));
        let curvature = self.mix * self.albedo;
        if curvature != 0.0 {
            let vertex =
                -((1.0 - self.mix) * self.albedo + self.mix * self.constant) / (2.0 * curvature);
            if vertex > 0.0 && vertex < 1.0 {
                peak = peak.max(self.at(vertex));
            }
        }
        peak.max(0.0)
    }

    /// 両方のtintがアルベドで同じ変わり方をするかどうか。
    ///
    /// 正規化（最大値で割る）後の `mix`・`constant`・`albedo` が
    /// 許容差以内で一致する場合に真になる。定数同士は同じ形とみなす。
    pub(crate) fn same_shape(self, other: Self) -> bool {
        const TOLERANCE: f32 = 1e-4;
        if self.is_constant() || other.is_constant() {
            return self.is_constant() && other.is_constant();
        }
        let peak = self.peak();
        let other_peak = other.peak();
        if peak <= 0.0 || other_peak <= 0.0 {
            return peak <= 0.0 && other_peak <= 0.0;
        }
        (self.mix - other.mix).abs() <= TOLERANCE
            && (self.constant / peak - other.constant / other_peak).abs() <= TOLERANCE
            && (self.albedo / peak - other.albedo / other_peak).abs() <= TOLERANCE
    }
}

/// リム倍率に掛かるテクスチャ1件（`RimFactorTexture` と対応）。
///
/// lilToonではリム色テクスチャとMatCapのマスクだけがあり、
/// アルファ掛け（Poiyomiの発光マップのアルファ）は使わない。
/// `invert` は常に偽、`channel` は `Rgb` の場合がほとんどである。
#[derive(Debug, Clone)]
struct RimFactor {
    /// 読み先。`_ST` は主変換との合成済みである。
    input: BakeInput,
    /// 由来のプロパティ（`_RimColorTex`・`{prefix}BlendMask`）。
    property: String,
    /// 由来テクスチャのGUID文字列。同一性の判定に使う。
    texture_id: Option<String>,
    /// 反転なしでMToonがそのまま読めるかどうか（`IsPlain` と対応）。
    /// vrctomにアルファ掛けは無いため、反転の有無だけで決める。
    plain: bool,
}

impl RimFactor {
    /// 同じ読み先かどうか（GUID・`_ST`・チャンネル・反転で比べる）。
    fn same_as(&self, other: &Self) -> bool {
        self.texture_id == other.texture_id
            && self.input.st == other.input.st
            && self.input.channel == other.input.channel
            && self.input.invert == other.input.invert
    }

    /// 警告文での呼び名。MatCap由来はマスク、リム由来はテクスチャと呼ぶ。
    fn kind(&self, masks_only: bool) -> &'static str {
        if masks_only || self.input.channel != BakeChannel::Rgb {
            "マスク"
        } else {
            "テクスチャ"
        }
    }
}

/// リム倍率を分け合う1要素（リム光かMatCap、`RimShare` と対応）。
///
/// 反転リム（Poiyomiの後ろ向きリム）・減光はlilToonに無いため持たない。
#[derive(Debug, Clone)]
struct RimShare {
    /// 警告文での呼び名（「リム光」・「MatCap」）。
    name: &'static str,
    /// 掛かるテクスチャ列。
    textures: Vec<RimFactor>,
    /// 画素ごとに変わるアルベドtint。定数は係数へ折り込むため持たない。
    tint: Option<AlbedoTint>,
    /// MToonの色係数（線形）。tintの最大値と合わせ込み・丸めの後で持つ。
    factor: [f32; 3],
    /// 光照混合係数。
    lighting_mix: f32,
    /// 由来がすべてマスクかどうか。警告文の呼び名に使う。
    masks_only: bool,
}

/// リム光とMatCapが分け合うリム倍率の計画（`RimMultiplyPlan` と対応）。
#[derive(Debug, Clone)]
struct RimMultiplyPlan {
    /// リム光。無効時は `None` である。
    rim: Option<RimShare>,
    /// MatCap。書き出さない場合は `None` である。
    matcap: Option<RimShare>,
    /// 共有テクスチャ列（3件まで）。
    textures: Vec<RimFactor>,
    /// テクスチャが運ぶアルベドtint。無い場合は掛けない。
    tint: Option<AlbedoTint>,
    /// tintをそのまま運ぶ要素名列。
    tinted: Vec<&'static str>,
    /// 色係数に入らない明るさ。テクスチャ側で掛ける。
    gain: f32,
    /// MToonのMatCap係数（MatCap層の持ち分）。
    matcap_factor: [f32; 3],
}

impl RimMultiplyPlan {
    /// 画素ごとに変わる内容（テクスチャかtint）があるかどうか。
    fn per_texel(&self) -> bool {
        !self.textures.is_empty() || self.tint.is_some()
    }
}

/// 陰影の2層目以降1層分（`ShadeLayer` と対応）。
struct ShadeExtraLayer {
    /// 境界・ぼかし。
    border: f32,
    /// ぼかし。
    blur: f32,
    /// 引き込み量（色のアルファ）。
    alpha: f32,
    /// 層の色（線形RGB）。
    color: [f32; 3],
}

/// 陰影の2層目以降を読む（`ReadShadowLayer` と対応）。
///
/// アルファが0より大きい層だけを2層目・3層目の順に返す。
/// 色テクスチャは色で代用するため読まない。
fn shade_extra_layers(material: &crate::unity::Material) -> Vec<ShadeExtraLayer> {
    let mut layers = Vec::new();
    for (prefix, border_default, blur_default) in [
        ("_Shadow2ndColor", 0.15, 0.1),
        ("_Shadow3rdColor", 0.25, 0.1),
    ] {
        let color = color(material, prefix).unwrap_or([0.0, 0.0, 0.0, 0.0]);
        if color[3] <= 0.0 {
            continue;
        }
        let border_name = prefix.replace("Color", "Border");
        let blur_name = prefix.replace("Color", "Blur");
        layers.push(ShadeExtraLayer {
            border: number(material, &border_name).unwrap_or(border_default) as f32,
            blur: number(material, &blur_name).unwrap_or(blur_default) as f32,
            alpha: color[3] as f32,
            color: [
                srgb_to_linear(color[0]),
                srgb_to_linear(color[1]),
                srgb_to_linear(color[2]),
            ],
        });
    }
    layers
}

/// 発光色を線形で読む。1超の有無も返す（`ReadEmission` の係数と対応）。
fn emission_linear(material: &crate::unity::Material) -> ([f32; 3], bool) {
    let emission = color(material, "_EmissionColor").unwrap_or([1.0, 1.0, 1.0, 1.0]);
    let raw = [
        srgb_to_linear_raw(emission[0]),
        srgb_to_linear_raw(emission[1]),
        srgb_to_linear_raw(emission[2]),
    ];
    let mut factor = [0.0f32; 3];
    let mut clamped = false;
    for (channel, value) in factor.iter_mut().zip(raw.iter()) {
        if *value > 1.0 {
            *channel = 1.0;
            clamped = true;
        } else {
            *channel = clamp01(*value) as f32;
        }
    }
    (factor, clamped)
}

/// MatCapが下の色をどれだけ取るか（`Matcap.Covers` と対応）。
///
/// Normal・Multiply混合だけが0より大きくなり、加算系は0である。
fn matcap_covers(material: &crate::unity::Material, prefix: &str) -> f32 {
    let mode = number(material, &format!("{prefix}BlendMode")).unwrap_or(1.0) as i64;
    if mode != 0 && mode != 3 {
        return 0.0;
    }
    let color = color(material, &format!("{prefix}Color")).unwrap_or([1.0, 1.0, 1.0, 1.0]);
    (number(material, &format!("{prefix}Blend")).unwrap_or(1.0) * clamp01(color[3])) as f32
}

/// テクスチャ環境から `_ST` を読む。環境が無ければ単位変換である。
fn bake_st_of(environment: Option<&crate::unity::TextureEnvironment>) -> BakeSt {
    match environment {
        Some(environment) => BakeSt {
            scale: environment.scale,
            offset: environment.offset,
        },
        None => BakeSt::identity(),
    }
}

/// 主変換の後に個別変換を合成する（`BakeJobBuilder.Compose` と対応）。
fn compose_st(main: &BakeSt, own: &BakeSt) -> BakeSt {
    BakeSt {
        scale: [main.scale[0] * own.scale[0], main.scale[1] * own.scale[1]],
        offset: [
            main.offset[0] * own.scale[0] + own.offset[0],
            main.offset[1] * own.scale[1] + own.offset[1],
        ],
    }
}

/// `.meta` からsRGB旗を読む。無ければ真（Unityの既定）に倒す。
fn srgb_of(meta: &crate::unity::AssetMeta) -> bool {
    let srgb = meta
        .importer()
        .settings
        .as_mapping()
        .and_then(|mapping| mapping.get("mipmaps"))
        .and_then(|value| value.as_mapping())
        .and_then(|mapping| mapping.get("sRGBTexture"));
    match srgb {
        Some(yaml_serde::Value::Bool(flag)) => *flag,
        Some(yaml_serde::Value::Number(number)) => number.as_i64().is_some_and(|value| value != 0),
        _ => true,
    }
}

/// `.meta` のTextureImporter設定から取り込み上限を読む。
///
/// `platformSettings` の `DefaultTexturePlatform` を既定に使い、無ければ
/// グローバル値を使う。プラットフォーム上書き（Standaloneなど）は使わない。
/// 戻り値は（上限、NPOT、不明なNPOT値があったか）である。
/// 上限 `None` は制限なし（0・未設定の場合）を表し、
/// 不明なNPOT値は `None` として扱う。警告は呼び出し側が出す。
fn import_limit_of(meta: &crate::unity::AssetMeta) -> (Option<u32>, NpotScale, bool) {
    let settings = meta.importer().settings.as_mapping();
    let global = settings
        .and_then(|mapping| mapping.get("maxTextureSize"))
        .and_then(as_u32);
    let mut platform_default = None;
    if let Some(sequence) = settings
        .and_then(|mapping| mapping.get("platformSettings"))
        .and_then(|value| value.as_sequence())
    {
        for item in sequence {
            let Some(mapping) = item.as_mapping() else {
                continue;
            };
            let is_default = mapping
                .get("buildTarget")
                .and_then(|value| value.as_str())
                .is_some_and(|target| target == "DefaultTexturePlatform");
            if is_default {
                platform_default = mapping.get("maxTextureSize").and_then(as_u32);
                break;
            }
        }
    }
    let max = platform_default.or(global).filter(|size| *size > 0);
    // 実キーは `nPOTScale` であり、文書表記の `npotScale` も受け付ける。
    let raw = settings
        .and_then(|mapping| {
            mapping
                .get("nPOTScale")
                .or_else(|| mapping.get("npotScale"))
        })
        .and_then(|value| value.as_i64());
    let (npot, unknown) = match raw {
        None | Some(0) => (NpotScale::None, false),
        Some(1) => (NpotScale::ToNearest, false),
        Some(2) => (NpotScale::ToLarger, false),
        Some(3) => (NpotScale::ToSmaller, false),
        Some(_) => (NpotScale::None, true),
    };
    (max, npot, unknown)
}

/// 汎用YAML値から非負整数を読む。数値以外・負値・範囲外は `None` である。
fn as_u32(value: &yaml_serde::Value) -> Option<u32> {
    value.as_i64().and_then(|number| u32::try_from(number).ok())
}

/// 縮小で色（符号化済み空間で平均）とデータ（線形空間で平均）を分ける。
///
/// 色は基準色・発光・MatCap・影色、データはマスク・法線・幅などである。
/// 焼きの入力と直載せの両方で同じ区分を使い、Unityの取り込みに近づける。
fn is_color_property(property: &str) -> bool {
    matches!(
        property,
        "_MainTex"
            | "_Main2ndTex"
            | "_Main3rdTex"
            | "_ShadowColorTex"
            | "_EmissionMap"
            | "_MatCapTex"
            | "_MatCap2ndTex"
            | "_RimColorTex"
    )
}

/// MToonのひとつのMatCap枠に入れるlilToon MatCap。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MatcapSlot {
    /// ひとつ目のMatCap（`_MatCap` 系）。
    First,
    /// ふたつ目のMatCap（`_MatCap2nd` 系）。
    Second,
    /// 無効。MToonの係数は `[0, 0, 0]` になる。
    None,
}

/// `_UseXXX` が真でテクスチャの割り当てがあるかどうか。
fn has_matcap(material: &crate::unity::Material, prefix: &str) -> bool {
    let bare = prefix.trim_start_matches('_');
    let toggle = format!("_Use{bare}");
    let tex = format!("{prefix}Tex");
    number(material, &toggle).unwrap_or(0.0) != 0.0 && has_texture(material, &tex)
}

/// lilToonのMatCapが色を変えるかどうか。
///
/// 重み（`_MatCapBlend × saturate(_MatCapColor.a)`）が0より大きく、
/// 加算・スクリーン系なら最大輝度が0より大きい場合に真になる。
/// 参照実装の `Contributes` と対応する。
fn matcap_contributes(material: &crate::unity::Material, prefix: &str) -> bool {
    let blend = format!("{prefix}Blend");
    let color_name = format!("{prefix}Color");
    let blend_value = number(material, &blend).unwrap_or(1.0);
    let color = color(material, &color_name).unwrap_or([1.0, 1.0, 1.0, 1.0]);
    if blend_value * clamp01(color[3]) <= 0.0 {
        return false;
    }
    // 混合方式1は加算であり、0置換・3乗算は常に色を変える。
    // 2スクリーンは加算と同様に扱う。
    let mode = number(material, &format!("{prefix}BlendMode")).unwrap_or(1.0) as i64;
    mode == 0 || mode == 3 || color[0].max(color[1]).max(color[2]) > 0.0
}

/// lilToonのMatCapが乗算でマスク越しに掛かるかどうか。
///
/// MToonのMatCapは加算だけであり、均一な減光は表せても
/// マスク越しの乗算は表せない。参照実装の `LeftOut` と対応する。
fn matcap_left_out(material: &crate::unity::Material, prefix: &str) -> bool {
    let mode = number(material, &format!("{prefix}BlendMode")).unwrap_or(1.0) as i64;
    mode == 3 && has_texture(material, &format!("{prefix}BlendMask"))
}

/// MToonのMatCap枠が2ndに空いているかどうか。
///
/// ひとつ目が無い・色を変えない・枠から外れる（`LeftOut`）場合に真になる。
/// 参照実装の `SlotFreeFor2nd` と対応する。
fn matcap_slot_free_for_2nd(material: &crate::unity::Material) -> bool {
    !has_matcap(material, "_MatCap")
        || !matcap_contributes(material, "_MatCap")
        || matcap_left_out(material, "_MatCap") && !matcap_left_out(material, "_MatCap2nd")
}

/// 2ndのMatCapをMToonの枠に入れるとリムを損なうかどうか。
///
/// MToonはリム倍率テクスチャをMatCapにも掛けるため、
/// マスク付きの2ndは色テクスチャ・マスク付きのリムを損なう。
/// 参照実装の `CostsTheRim` と対応する。
fn matcap_costs_the_rim(material: &crate::unity::Material) -> bool {
    if !has_texture(material, "_MatCap2ndBlendMask") {
        return false;
    }
    if number(material, "_UseRim").unwrap_or(0.0) == 0.0 {
        return false;
    }
    let rim = linear_color_or(material, "_RimColor", [0.66, 0.5, 0.48]);
    rim[0].max(rim[1]).max(rim[2]) > 0.0
}

/// MToonのMatCap枠に入れるlilToon MatCapを決める。
///
/// 参照実装の `SlotMatcapPrefix` と対応する。2ndが色を変える場合だけ
/// 枠を争い、枠が空いていてリムを損なわない場合に2ndが入る。
/// 2ndが枠から外れる（`LeftOut`）場合は2ndを返し、呼び出し側で省く。
fn matcap_slot(material: &crate::unity::Material) -> MatcapSlot {
    let first = if has_matcap(material, "_MatCap") {
        Some(MatcapSlot::First)
    } else {
        None
    };
    if !has_matcap(material, "_MatCap2nd")
        || !matcap_contributes(material, "_MatCap2nd")
        || !matcap_slot_free_for_2nd(material)
    {
        return first.unwrap_or(MatcapSlot::None);
    }
    if matcap_costs_the_rim(material) && !matcap_left_out(material, "_MatCap2nd") {
        return first.unwrap_or(MatcapSlot::None);
    }
    MatcapSlot::Second
}

/// lilToonのMatCap重みを求める。
///
/// `_MatCapColor.rgb × (_MatCapBlend × saturate(_MatCapColor.a))` であり、
/// 参照実装の `MatcapWeight` と対応する。RGBは線形、重みはそのまま掛ける。
fn matcap_weight(material: &crate::unity::Material, prefix: &str) -> [f32; 3] {
    let color_name = format!("{prefix}Color");
    let blend_name = format!("{prefix}Blend");
    let linear = linear_color_or(material, &color_name, [1.0, 1.0, 1.0]);
    let alpha = color(material, &color_name).map_or(1.0, |color| color[3]);
    let weight = (number(material, &blend_name).unwrap_or(1.0) * clamp01(alpha)) as f32;
    [linear[0] * weight, linear[1] * weight, linear[2] * weight]
}

/// 色プロパティを線形で読む。未設定の場合は線形の代替値をそのまま使う。
///
/// 参照実装の代替値は線形で宣言されているため、sRGB復号しない。
/// 呼び出し側は代替値を線形で渡す。
fn linear_color_or(material: &crate::unity::Material, name: &str, fallback: [f32; 3]) -> [f32; 3] {
    match color(material, name) {
        Some(value) => [
            srgb_to_linear(value[0]),
            srgb_to_linear(value[1]),
            srgb_to_linear(value[2]),
        ],
        None => fallback,
    }
}

/// シェーダ本文の先頭の `Shader "..."` 名を返す。
fn first_shader_name(text: &str) -> Option<String> {
    text.lines().find_map(|line| {
        line.trim_start()
            .strip_prefix("Shader \"")
            .and_then(|rest| rest.split('"').next().map(str::to_owned))
    })
}

/// シェーダ名から能力を求める。lilToon名でなければ `None` である。
fn capability_of_name(name: &str, material: &crate::unity::Material) -> Option<ShaderCapability> {
    // `Cutout`/`Transparent` を含む他社シェーダをlilToonとして扱わないため、
    // 名前に `lilToon` を含むものだけを対象にする。
    if !name.contains("lilToon") {
        return None;
    }
    let is_multi = name.contains("Multi");
    let has_outline = name.contains("Outline") && !name.contains("OutlineOnly");
    let is_lite = name.contains("Lite");
    // Multi系は描画方式（`_TransparentMode`）で系列が決まる。
    // 0不透明・1カットアウト・2半透明以外（屈折・宝石・ファー）は扱わない。
    // 参照実装の `ApplyMultiTransparentMode` と対応する。
    let series = if is_multi {
        match number(material, "_TransparentMode").unwrap_or(0.0) as i64 {
            0 => ShaderSeries::Opaque,
            1 => ShaderSeries::Mask,
            2 => ShaderSeries::Blend,
            _ => return None,
        }
    } else if name.contains("Cutout") {
        ShaderSeries::Mask
    } else if name.contains("Transparent") {
        ShaderSeries::Blend
    } else {
        ShaderSeries::Opaque
    };
    Some(ShaderCapability {
        series,
        has_outline,
        is_multi,
        is_lite,
    })
}

/// `.mat` から実効描画順を求める。
///
/// `m_CustomRenderQueue` が-1（未設定）の場合はシェーダ系列の既定
/// （不透明2000・カットアウト2450・半透明3000）を使い、Unityの
/// `material.renderQueue`（未設定時はシェーダ既定を返す）と対応させる。
/// 系列が不明な場合は-1であり、描画順の割り当てでは使わない。
fn effective_queue(material: &crate::unity::Material, series: Option<ShaderSeries>) -> i32 {
    if material.custom_render_queue != -1 {
        return material.custom_render_queue;
    }
    match series {
        Some(ShaderSeries::Opaque) => 2000,
        Some(ShaderSeries::Mask) => 2450,
        Some(ShaderSeries::Blend) => 3000,
        None => -1,
    }
}

/// 材質値が有限かつ範囲内であることを確かめる。不正ならフィールド名を返す。
pub(crate) fn validate_material(material: &VrmMaterial) -> Result<(), &'static str> {
    fn finite01(value: f32) -> bool {
        value.is_finite() && (0.0..=1.0).contains(&value)
    }
    for value in material.base_color_factor {
        if !finite01(value) {
            return Err("base_color_factor");
        }
    }
    if !finite01(material.alpha_cutoff) {
        return Err("alpha_cutoff");
    }
    for value in material.emissive_factor {
        if !finite01(value) {
            return Err("emissive_factor");
        }
    }
    let Some(mtoon) = &material.mtoon else {
        return Ok(());
    };
    for value in mtoon.shade_color_factor {
        if !finite01(value) {
            return Err("shade_color_factor");
        }
    }
    if !(mtoon.shading_shift_factor.is_finite()
        && (-1.0..=1.0).contains(&mtoon.shading_shift_factor))
    {
        return Err("shading_shift_factor");
    }
    if !finite01(mtoon.shading_toony_factor) {
        return Err("shading_toony_factor");
    }
    if !finite01(mtoon.gi_equalization_factor) {
        return Err("gi_equalization_factor");
    }
    for value in mtoon.matcap_factor {
        if !finite01(value) {
            return Err("matcap_factor");
        }
    }
    if !(mtoon.outline_width_factor.is_finite() && mtoon.outline_width_factor >= 0.0) {
        return Err("outline_width_factor");
    }
    for value in mtoon.outline_color_factor {
        if !finite01(value) {
            return Err("outline_color_factor");
        }
    }
    if !finite01(mtoon.outline_lighting_mix_factor) {
        return Err("outline_lighting_mix_factor");
    }
    for value in mtoon.parametric_rim_color_factor {
        if !finite01(value) {
            return Err("parametric_rim_color_factor");
        }
    }
    if !(mtoon.parametric_rim_fresnel_power_factor.is_finite()
        && mtoon.parametric_rim_fresnel_power_factor >= 0.0)
    {
        return Err("parametric_rim_fresnel_power_factor");
    }
    if !mtoon.parametric_rim_lift_factor.is_finite() {
        return Err("parametric_rim_lift_factor");
    }
    if !finite01(mtoon.rim_lighting_mix_factor) {
        return Err("rim_lighting_mix_factor");
    }
    if !mtoon.uv_animation_scroll_x_speed_factor.is_finite()
        || !mtoon.uv_animation_scroll_y_speed_factor.is_finite()
        || !mtoon.uv_animation_rotation_speed_factor.is_finite()
    {
        return Err("uv_animation_speed_factor");
    }
    if !material.normal_scale.is_finite() {
        return Err("normal_scale");
    }
    if !(-9..=9).contains(&mtoon.render_queue_offset) {
        return Err("render_queue_offset");
    }
    Ok(())
}

/// 数値プロパティに非有限値があるかを返す。
///
/// 写像時の `clamp01` や `.max(0.0)` が非有限を正常値に潰す前に検出する。
/// `floats` の値・`colors` の各成分・`tex_envs` の `scale`/`offset` を見る。
/// 整数プロパティは非有限を取り得ないため対象外である。
fn has_non_finite(material: &crate::unity::Material) -> bool {
    if material.floats.iter().any(|entry| !entry.value.is_finite()) {
        return true;
    }
    if material
        .colors
        .iter()
        .any(|entry| entry.value.iter().any(|value| !value.is_finite()))
    {
        return true;
    }
    material.tex_envs.iter().any(|entry| {
        entry
            .value
            .scale
            .iter()
            .chain(entry.value.offset.iter())
            .any(|value| !value.is_finite())
    })
}

/// 浮動小数点プロパティの有無を返す。
fn has_float(material: &crate::unity::Material, name: &str) -> bool {
    material.floats.iter().any(|entry| entry.name == name)
        || material.ints.iter().any(|entry| entry.name == name)
}

/// 数値プロパティを読む。浮動小数点を先に、整数も受け付ける。
/// 同名は最初の1件を採用する。
fn number(material: &crate::unity::Material, name: &str) -> Option<f64> {
    if let Some(entry) = material.floats.iter().find(|entry| entry.name == name) {
        return Some(entry.value);
    }
    material
        .ints
        .iter()
        .find(|entry| entry.name == name)
        .map(|entry| entry.value as f64)
}

/// 色プロパティを読む。同名は最初の1件を採用する。
fn color(material: &crate::unity::Material, name: &str) -> Option<[f64; 4]> {
    material
        .colors
        .iter()
        .find(|entry| entry.name == name)
        .map(|entry| entry.value)
}

/// テクスチャ環境を読む。同名は最初の1件を採用する。
fn texture<'m>(
    material: &'m crate::unity::Material,
    name: &str,
) -> Option<&'m crate::unity::TextureEnvironment> {
    material
        .tex_envs
        .iter()
        .find(|entry| entry.name == name)
        .map(|entry| &entry.value)
}

/// テクスチャ参照の有無を返す。割り当て済み（nullでない）かどうかを見る。
fn has_texture(material: &crate::unity::Material, name: &str) -> bool {
    texture(material, name).is_some_and(|environment| !environment.texture.is_null())
}

/// 先頭バイトから入力形式を求める。
///
/// PNG・JPEGとPSDの保存済み合成画像だけを受け付ける。
/// PSDは復号してPNGへ符号化し直すため、埋め込みのMIMEはPNGになる。
/// 埋め込みの [`VrmImageMime`] と [`VrmImage`] はPNG・JPEGのまま保ち、
/// 生RGBAを公開型に足さない。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SourceImageFormat {
    /// PNG。
    Png,
    /// JPEG。
    Jpeg,
    /// PSD（保存済みcompositeを復号して使う）。
    Psd,
}

/// 先頭バイトから入力形式を求める。PNG・JPEG・PSDだけを受け付ける。
fn sniff_source(bytes: &[u8]) -> Option<SourceImageFormat> {
    if bytes.starts_with(&PNG_MAGIC) {
        Some(SourceImageFormat::Png)
    } else if bytes.starts_with(&JPEG_MAGIC) {
        Some(SourceImageFormat::Jpeg)
    } else if is_psd(bytes) {
        Some(SourceImageFormat::Psd)
    } else {
        None
    }
}

/// 入力形式から埋め込みMIMEを求める。PSDは復号後にPNGになるため持たない。
fn mime_of(source: SourceImageFormat) -> Option<VrmImageMime> {
    match source {
        SourceImageFormat::Png => Some(VrmImageMime::Png),
        SourceImageFormat::Jpeg => Some(VrmImageMime::Jpeg),
        SourceImageFormat::Psd => None,
    }
}

/// PNGの必須チャンク名の許可表。
///
/// 大文字小文字ではなく名前で判定し、小文字を含む `tRNS` も残す。
const PNG_ESSENTIAL_CHUNKS: [[u8; 4]; 5] = [*b"IHDR", *b"PLTE", *b"tRNS", *b"IDAT", *b"IEND"];

/// PNGから画素に影響しない付加チャンクを落とす。
///
/// 必須チャンクだけを元のバイト列のまま（長さ・型・データ・CRCをそのまま）残し、
/// 先頭8バイトの署名もそのまま保つ。再エンコードはしない。
/// チャンク構造（長さ・型・データ・CRC・IENDまで）が読めない場合は `None` を返す。
fn strip_png_ancillary_chunks(bytes: &[u8]) -> Option<Vec<u8>> {
    if !bytes.starts_with(&PNG_MAGIC) {
        return None;
    }
    let mut stripped = Vec::with_capacity(bytes.len());
    stripped.extend_from_slice(&PNG_MAGIC);
    let mut pos = PNG_MAGIC.len();
    loop {
        let header = bytes.get(pos..pos + 8)?;
        let length = u32::from_be_bytes(header[0..4].try_into().ok()?) as usize;
        let chunk_type: [u8; 4] = header[4..8].try_into().ok()?;
        let end = pos.checked_add(8)?.checked_add(length)?.checked_add(4)?;
        let chunk = bytes.get(pos..end)?;
        if PNG_ESSENTIAL_CHUNKS.contains(&chunk_type) {
            stripped.extend_from_slice(chunk);
        }
        pos = end;
        if chunk_type == *b"IEND" {
            break;
        }
    }
    // IENDの後に余りがある形式は壊れたものとして扱う。
    if pos != bytes.len() {
        return None;
    }
    Some(stripped)
}

/// `.meta` のTextureImporter設定からサンプラを作る。
///
/// 設定が無い項目は既定（繰り返し・双線形・mipmapあり）に倒す。
fn sampler_of(meta: &crate::unity::AssetMeta) -> VrmSampler {
    let mut sampler = VrmSampler::default();
    let settings = meta.importer().settings.as_mapping();
    let texture_settings = settings
        .and_then(|mapping| mapping.get("textureSettings"))
        .and_then(|value| value.as_mapping());
    if let Some(mapping) = texture_settings {
        sampler.wrap_u = wrap_of(number_of(mapping, "wrapU"));
        sampler.wrap_v = wrap_of(number_of(mapping, "wrapV"));
        sampler.filter = filter_of(number_of(mapping, "filterMode"));
    }
    let mipmaps = settings
        .and_then(|mapping| mapping.get("mipmaps"))
        .and_then(|value| value.as_mapping())
        .and_then(|mapping| mapping.get("enableMipMap"));
    if let Some(value) = mipmaps {
        sampler.mipmaps = match value {
            yaml_serde::Value::Bool(flag) => *flag,
            yaml_serde::Value::Number(number) => number.as_i64().is_some_and(|n| n != 0),
            _ => true,
        };
    }
    sampler
}

/// 汎用YAML写像から整数を読む。数値以外は `None` である。
fn number_of(mapping: &yaml_serde::Mapping, key: &str) -> Option<i64> {
    mapping.get(key).and_then(|value| value.as_i64())
}

/// 繰り返し設定値を写像する。0/1/2以外は繰り返しに倒す。
fn wrap_of(value: Option<i64>) -> VrmWrap {
    match value {
        Some(1) => VrmWrap::ClampToEdge,
        Some(2) => VrmWrap::MirroredRepeat,
        _ => VrmWrap::Repeat,
    }
}

/// フィルタ設定値を写像する。0点・1双線形・2三線形、他は双線形に倒す。
fn filter_of(value: Option<i64>) -> VrmFilter {
    match value {
        Some(0) => VrmFilter::Nearest,
        Some(2) => VrmFilter::Trilinear,
        _ => VrmFilter::Linear,
    }
}

/// 0〜1に丸める。
fn clamp01(value: f64) -> f64 {
    value.clamp(0.0, 1.0)
}

/// sRGB符号化値を線形に直し、0〜1に丸める。
fn srgb_to_linear(value: f64) -> f32 {
    clamp01(srgb_to_linear_raw(value)) as f32
}

/// sRGB符号化値を線形に直す。丸めは呼び出し側で行う。
fn srgb_to_linear_raw(value: f64) -> f64 {
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::unity::{AssetIndex, AssetSource};

    // テスト名は日本語で書く（テストは仕様・契約であるため）。

    /// 何も持たない読み手。保持量の単体確認だけに使う。
    struct EmptySource;

    impl AssetSource for EmptySource {
        fn read_limited(&self, _path: &str, _limit: u64) -> std::io::Result<Vec<u8>> {
            Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "持たない",
            ))
        }
    }

    #[test]
    fn 保持予算を超えると古い保持を捨てて失敗しない() {
        let options = MaterialOptions::new()
            .with_max_image_pixels(100)
            .with_max_total_image_pixels(100);
        let source = EmptySource;
        let index = AssetIndex::default();
        let mut context = LoadContext::new(&source, &index, &options);
        let guid = Guid::parse("00000000000000000000000000000001").expect("GUIDは読める");
        // 10x10（100画素）を入れて共有表に載せる。
        context.admit_bake_cache(10, 10);
        context.baked_inputs.insert((guid, false), None);
        assert_eq!(context.cached_image_pixels, 100);
        // 予算を超える挿入では古い保持を全消去し、失敗にしない。
        context.admit_bake_cache(10, 10);
        assert!(
            context.baked_inputs.is_empty(),
            "古い保持は捨てる: {:?}",
            context.baked_inputs.keys()
        );
        assert_eq!(context.cached_image_pixels, 100);
        // 1枚の上限は従来どおり失敗する。
        let 失敗 = context
            .check_image_pixels(11, 10, "検査")
            .expect_err("1枚の上限超過は失敗する");
        assert_eq!(失敗.kind(), MaterialErrorKind::LimitExceeded);
    }
}
