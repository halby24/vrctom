//! Unityの `.mat` とテクスチャからVRM 1.0の材質を作る層。
//!
//! 入口は [`VrmMaterials::load`]（Native限定）であり、展開済みディレクトリの
//! `Assets` 配下だけを索引化してFBXの材質スロット名を `.mat` に対応付け、
//! 色・透明度・テクスチャを読み取る。
//! 型と写像規則はターゲット非依存であり、wasmでは読み取りだけを使わない。
//! 得られた [`VrmMaterials`] は [`VrmExportOptions`](super::export::VrmExportOptions) に渡す。
//!
//! ```mermaid
//! flowchart TB
//!     fbxmeta[".fbx.meta<br/>externalObjects"] --> map["スロット名→.mat"]
//!     index["AssetIndex<br/>GUID→パス"] --> map
//!     map --> mat[".mat の読み取り<br/>lilToonプロパティ"]
//!     mat --> conv["MToonへの写像"]
//!     mat --> tex["テクスチャ解決<br/>.meta と画像バイト"]
//!     tex --> images["VrmImage列"]
//!     conv --> materials["VrmMaterials"]
//!     images --> materials
//! ```
//!
//! 画像のデコード・再エンコード、テクスチャのベイク、lilToon以外のシェーダの解釈は
//! 行わない。MIMEは先頭バイトで判定し、PNGとJPEGだけを受け付ける。

use crate::escape_diag;

#[cfg(not(target_arch = "wasm32"))]
use std::collections::{HashMap, HashSet};
#[cfg(not(target_arch = "wasm32"))]
use std::path::Path;

#[cfg(not(target_arch = "wasm32"))]
use crate::unity::{
    AssetIndex, AssetKind, Guid, IndexOptions, ParseOptions, SerializedFile, TypedObject,
    scan_directory,
};

/// 画像1枚の既定上限（64MiB）。
///
/// 等身大の4Kテクスチャが収まる大きさにし、破損由来の膨張を抑える。
const DEFAULT_MAX_IMAGE_BYTES: u64 = 64 * 1024 * 1024;
/// 画像合計の既定上限（512MiB）。
///
/// 実データの数百MBが収まる大きさにし、破損由来の膨張を抑える。
const DEFAULT_MAX_TOTAL_IMAGE_BYTES: u64 = 512 * 1024 * 1024;
/// 材質数の既定上限（1024件）。
///
/// 実データの数十件より十分大きく、破損由来の大量生成の暴走を抑える.
const DEFAULT_MAX_MATERIALS: usize = 1024;
/// `.mat` 1件の読み取り上限（8MiB）。
///
/// テキストYAMLの実測より十分大きく、破損由来の膨張を抑える。
#[cfg(not(target_arch = "wasm32"))]
const MAX_MATERIAL_BYTES: u64 = 8 * 1024 * 1024;
/// `.shader` 1件の読み取り上限（4MiB）。
///
/// 先頭のシェーダ名だけを見るため、本文全体は読まない。
#[cfg(not(target_arch = "wasm32"))]
const MAX_SHADER_BYTES: u64 = 4 * 1024 * 1024;

/// 不透明lilToonシェーダのGUID表（小文字）。
///
/// `.shader` が索引に無い場合の系列判定に使う。
#[cfg(not(target_arch = "wasm32"))]
const OPAQUE_SHADER_GUIDS: [&str; 4] = [
    "df12117ecd77c31469c224178886498e",
    "efa77a80ca0344749b4f19fdd5891cbe",
    "381af8ba8e1740a41b9768ccfb0416c2",
    "583a88005abb81a4ebbce757b4851a0d",
];
/// カットアウトlilToonシェーダのGUID表（小文字）。
#[cfg(not(target_arch = "wasm32"))]
const MASK_SHADER_GUIDS: [&str; 4] = [
    "85d6126cae43b6847aff4b13f4adb8ec",
    "3b4aa19949601f046a20ca8bdaee929f",
    "b957dce3d03ff5445ac989f8de643c7f",
    "8cf5267d397b04846856f6d3d9561da0",
];
/// 半透明lilToonシェーダのGUID表（小文字）。
#[cfg(not(target_arch = "wasm32"))]
const BLEND_SHADER_GUIDS: [&str; 4] = [
    "165365ab7100a044ca85fc8c33548a62",
    "3c79b10c7e0b2784aaa4c2f8dd17d55e",
    "0e3ece1bd59542743bccadb21f68318e",
    "1c12a37046f07ac4486881deaf0187ea",
];

/// PNGの先頭バイト。
#[cfg(not(target_arch = "wasm32"))]
const PNG_MAGIC: [u8; 8] = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
/// JPEGの先頭バイト。
#[cfg(not(target_arch = "wasm32"))]
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

    /// 展開済みディレクトリから材質を読み取る（Native限定）。
    ///
    /// `root` は `Assets` を含むUnityプロジェクトの配置を指し、
    /// `fbx_path` は `Assets` 配下のFBXを指す。`Assets` 配下だけを索引化して
    /// `.fbx.meta` の `UnityEngine:Material` 置換表を順に解決し、
    /// `.mat` とテクスチャを読む。
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
        let mut context = LoadContext::new(&canonical_assets, &index, options);
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
            context.warn("FBXがAssets配下にないため材質を読まない");
            return Ok((None, context.report()));
        };
        let Some(record) = index.get_by_path(&relative) else {
            context.warn(format!("FBXが索引にないため材質を読まない: {relative}"));
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
        let materials = VrmMaterials::new(
            std::mem::take(&mut context.entries),
            std::mem::take(&mut context.images),
        );
        let report = context.report();
        Ok((Some(materials), report))
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
    /// 描画順の差分。常に0である。
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
    /// マットキャップの色。線形である。
    pub matcap_factor: [f32; 3],
    /// マットキャップテクスチャの画像索引。無い場合は `None` である。
    pub matcap_image: Option<usize>,
    /// アウトライン幅の方式。
    pub outline_width_mode: VrmOutlineWidthMode,
    /// アウトライン幅。メートルである。
    pub outline_width_factor: f32,
    /// アウトライン幅乗算テクスチャの画像索引。無い場合は `None` である。
    pub outline_width_multiply_image: Option<usize>,
    /// アウトラインの色。線形である。
    pub outline_color_factor: [f32; 3],
    /// アウトラインの光照混合係数。常に1である。
    pub outline_lighting_mix_factor: f32,
}

impl Default for VrmMtoonParameters {
    /// 仕様既定のMToon値を作る。
    ///
    /// 影色は白、境界は中間、GI均一化は0.9、アウトラインは無しである。
    fn default() -> Self {
        Self {
            transparent_with_z_write: false,
            render_queue_offset: 0,
            shade_color_factor: [1.0, 1.0, 1.0],
            shade_multiply_image: None,
            shading_shift_factor: 0.0,
            shading_toony_factor: 0.9,
            gi_equalization_factor: 0.9,
            matcap_factor: [1.0, 1.0, 1.0],
            matcap_image: None,
            outline_width_mode: VrmOutlineWidthMode::None,
            outline_width_factor: 0.0,
            outline_width_multiply_image: None,
            outline_color_factor: [0.0, 0.0, 0.0],
            outline_lighting_mix_factor: 1.0,
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
}

impl MaterialOptions {
    /// 既定の上限を持つ設定を生成する。
    ///
    /// 画像1枚64MiB、合計512MiB、材質1024件である。
    pub fn new() -> Self {
        Self {
            max_image_bytes: DEFAULT_MAX_IMAGE_BYTES,
            max_total_image_bytes: DEFAULT_MAX_TOTAL_IMAGE_BYTES,
            max_materials: DEFAULT_MAX_MATERIALS,
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
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn new(kind: MaterialErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into().into_boxed_str(),
            path: None,
            source: None,
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn with_path(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into().into_boxed_str());
        self
    }

    #[cfg(not(target_arch = "wasm32"))]
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
        mtoon: None,
    }
}

/// lilToonシェーダの透明度系列。
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShaderSeries {
    /// 不透明。
    Opaque,
    /// カットアウト。
    Mask,
    /// 半透明。
    Blend,
}

/// 読み取り作業域。警告・材質・画像と重複排除表を持つ。
#[cfg(not(target_arch = "wasm32"))]
struct LoadContext<'a> {
    root: &'a Path,
    index: &'a AssetIndex,
    options: &'a MaterialOptions,
    warnings: Vec<String>,
    entries: Vec<VrmMaterialEntry>,
    seen_slots: HashSet<String>,
    images: Vec<VrmImage>,
    image_at: HashMap<Guid, usize>,
    total_image_bytes: u64,
}

#[cfg(not(target_arch = "wasm32"))]
impl<'a> LoadContext<'a> {
    fn new(root: &'a Path, index: &'a AssetIndex, options: &'a MaterialOptions) -> Self {
        Self {
            root,
            index,
            options,
            warnings: Vec::new(),
            entries: Vec::new(),
            seen_slots: HashSet::new(),
            images: Vec::new(),
            image_at: HashMap::new(),
            total_image_bytes: 0,
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
        let material = self.resolve_material(entry, &slot)?;
        self.entries.push(VrmMaterialEntry { slot, material });
        Ok(())
    }

    /// 置換表1件から材質を作る。未対応は灰色材質へ落として警告する。
    fn resolve_material(
        &mut self,
        entry: &crate::unity::ExternalObjectEntry,
        slot: &str,
    ) -> Result<VrmMaterial, MaterialError> {
        // null参照は置換なしと同じ扱いにする。
        if entry.replacement.is_null() {
            self.warn(format!("材質の置換先がnullのため灰色材質にする: {slot}"));
            return Ok(placeholder(slot));
        }
        let Some(guid) = entry.replacement.guid.clone() else {
            self.warn(format!("材質の置換先GUIDがないため灰色材質にする: {slot}"));
            return Ok(placeholder(slot));
        };
        let Some(record) = self.index.get(&guid) else {
            self.warn(format!(
                "材質の置換先が索引にないため灰色材質にする: {slot}"
            ));
            return Ok(placeholder(slot));
        };
        if record.kind != AssetKind::Material {
            self.warn(format!(
                "材質の置換先が材質でないため灰色材質にする: {slot}"
            ));
            return Ok(placeholder(slot));
        };
        let path = record.path.clone();
        let bytes = match read_limited(&self.root.join(&path), MAX_MATERIAL_BYTES) {
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
                return Ok(placeholder(slot));
            }
        };
        let mut found = None;
        for document in file.documents() {
            if let Ok(TypedObject::Material(material)) = document.decode() {
                found = Some(material);
                break;
            }
        }
        let Some(material) = found else {
            self.warn(format!("材質文書がないため灰色材質にする: {slot}: {path}"));
            return Ok(placeholder(slot));
        };
        // 写像時の `clamp01` や `.max(0.0)` が非有限を正常値に潰す前に検出する。
        // 写像後の `validate_material` は有限の範囲外だけを拾う役割に残す。
        if has_non_finite(&material) {
            self.warn(format!("材質に非有限値があるため灰色材質にする: {slot}"));
            return Ok(placeholder(slot));
        }
        let Some(series) = self.shader_series(&material, slot) else {
            self.warn(format!(
                "未対応シェーダのため灰色材質にする: {slot}: {}",
                material.name
            ));
            return Ok(placeholder(slot));
        };
        let mapped = self.map_material(&material, slot, series)?;
        if let Err(field) = validate_material(&mapped) {
            self.warn(format!("材質値が不正なため灰色材質にする: {slot}: {field}"));
            return Ok(placeholder(slot));
        }
        Ok(mapped)
    }

    /// シェーダ参照から透明度系列を求める。lilToonでなければ `None` である。
    ///
    /// `.shader` が索引にあれば先頭のシェーダ名で判定し、無ければ既知のGUID表で
    /// 判定する。`.shader` が読めない・上限を超える・UTF-8でない場合は
    /// 名前判定を諦めてGUID表へ進み、その旨を警告する。
    /// どちらも不明で `_lilToonVersion` を持つ場合だけ、描画状態から
    /// 不透明か半透明かを推定する（カットアウトは判定できない）。
    fn shader_series(
        &mut self,
        material: &crate::unity::Material,
        slot: &str,
    ) -> Option<ShaderSeries> {
        if !material.shader.is_null()
            && let Some(guid) = material.shader.guid.clone()
        {
            if let Some(record) = self.index.get(&guid)
                && record.kind == AssetKind::Shader
            {
                match read_limited(&self.root.join(&record.path), MAX_SHADER_BYTES) {
                    Ok(bytes) => match std::str::from_utf8(&bytes) {
                        Ok(text) => {
                            if let Some(name) = first_shader_name(text)
                                && let Some(series) = series_of_name(&name)
                            {
                                return Some(series);
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
            if OPAQUE_SHADER_GUIDS.contains(&guid_text) {
                return Some(ShaderSeries::Opaque);
            }
            if MASK_SHADER_GUIDS.contains(&guid_text) {
                return Some(ShaderSeries::Mask);
            }
            if BLEND_SHADER_GUIDS.contains(&guid_text) {
                return Some(ShaderSeries::Blend);
            }
        }
        // GUIDが未知の場合だけ版数で推定する。カットアウトは区別できない。
        if has_float(material, "_lilToonVersion") {
            self.warn(format!(
                "シェーダ系列を判定できないため描画状態から推定しカットアウトは区別しない: {slot}"
            ));
            let dst = number(material, "_DstBlend").unwrap_or(0.0);
            return Some(if dst == 0.0 {
                ShaderSeries::Opaque
            } else {
                ShaderSeries::Blend
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
        series: ShaderSeries,
    ) -> Result<VrmMaterial, MaterialError> {
        let base = color(material, "_Color").unwrap_or([1.0, 1.0, 1.0, 1.0]);
        let base_rgb = [
            srgb_to_linear(base[0]),
            srgb_to_linear(base[1]),
            srgb_to_linear(base[2]),
        ];
        let alpha_mode = match series {
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
        let base_color_image = self.resolve_texture(material, slot, "_MainTex")?;

        let use_shadow = number(material, "_UseShadow").unwrap_or(1.0) != 0.0;
        // MToonの影側は `shadeColorFactor × shadeMultiplyTexture` のため、
        // 影を使わない場合と影色テクスチャが無い場合は基準色テクスチャを落とさず、
        // 影色と基準色の積にする。
        let (shade_color_factor, shading_shift_factor, shading_toony_factor, shade_multiply_image) =
            if !use_shadow {
                // 影を使わない場合、陰影は基準色と同じにし、境界は見せない。
                (base_rgb, 0.0, 0.9, base_color_image)
            } else {
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
                let tint = [
                    (1.0 - (1.0 - f64::from(srgb_to_linear(shadow[0]))) * strength) as f32,
                    (1.0 - (1.0 - f64::from(srgb_to_linear(shadow[1]))) * strength) as f32,
                    (1.0 - (1.0 - f64::from(srgb_to_linear(shadow[2]))) * strength) as f32,
                ];
                let shadow_image = self.resolve_texture(material, slot, "_ShadowColorTex")?;
                if let Some(image) = shadow_image {
                    self.warn(format!(
                        "影色テクスチャは基準色との混合ではなく乗算として近似する: {slot}"
                    ));
                    (tint, shift as f32, toony as f32, Some(image))
                } else {
                    // 影色テクスチャが無い場合、影色と基準色の積を影側の色にする。
                    let mixed = [
                        tint[0] * base_rgb[0],
                        tint[1] * base_rgb[1],
                        tint[2] * base_rgb[2],
                    ];
                    (mixed, shift as f32, toony as f32, base_color_image)
                }
            };

        let use_matcap = number(material, "_UseMatCap").unwrap_or(0.0) != 0.0;
        let matcap_image = if use_matcap {
            self.resolve_texture(material, slot, "_MatCapTex")?
        } else {
            None
        };
        let matcap_factor = if use_matcap {
            let cap = color(material, "_MatCapColor").unwrap_or([1.0, 1.0, 1.0, 1.0]);
            [
                srgb_to_linear(cap[0]),
                srgb_to_linear(cap[1]),
                srgb_to_linear(cap[2]),
            ]
        } else {
            [1.0, 1.0, 1.0]
        };

        let use_emission = number(material, "_UseEmission").unwrap_or(0.0) != 0.0;
        let (emissive_factor, emissive_image) = if use_emission {
            let emission = color(material, "_EmissionColor").unwrap_or([1.0, 1.0, 1.0, 1.0]);
            // 丸め前の線形値で1超を検出し、警告してから丸める。
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
            if clamped {
                self.warn(format!("発光色が1を超えるため1に丸める: {slot}"));
            }
            (
                factor,
                self.resolve_texture(material, slot, "_EmissionMap")?,
            )
        } else {
            ([0.0, 0.0, 0.0], None)
        };

        let use_outline = number(material, "_UseOutline").unwrap_or(0.0) != 0.0;
        let (
            outline_width_mode,
            outline_width_factor,
            outline_color_factor,
            outline_width_multiply_image,
        ) = if use_outline {
            let width = number(material, "_OutlineWidth").unwrap_or(0.05).max(0.0);
            // lilToonの既定アウトライン色（`_OutlineColor` 未設定時の代替値）。
            let outline = color(material, "_OutlineColor").unwrap_or([0.6, 0.56, 0.73, 1.0]);
            (
                VrmOutlineWidthMode::WorldCoordinates,
                width as f32,
                [
                    srgb_to_linear(outline[0]),
                    srgb_to_linear(outline[1]),
                    srgb_to_linear(outline[2]),
                ],
                self.resolve_texture(material, slot, "_OutlineWidthMask")?,
            )
        } else {
            (VrmOutlineWidthMode::None, 0.0, [0.0, 0.0, 0.0], None)
        };

        self.warn_unsupported_features(material, slot);

        Ok(VrmMaterial {
            name: material.name.clone(),
            alpha_mode,
            alpha_cutoff,
            double_sided,
            base_color_factor: [base_rgb[0], base_rgb[1], base_rgb[2], base[3] as f32],
            base_color_image,
            emissive_factor,
            emissive_image,
            mtoon: Some(VrmMtoonParameters {
                transparent_with_z_write: matches!(series, ShaderSeries::Blend) && zwrite,
                render_queue_offset: 0,
                shade_color_factor,
                shade_multiply_image,
                shading_shift_factor,
                shading_toony_factor,
                gi_equalization_factor: 0.9,
                matcap_factor,
                matcap_image,
                outline_width_mode,
                outline_width_factor,
                outline_width_multiply_image,
                outline_color_factor,
                outline_lighting_mix_factor: 1.0,
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
        if number(material, "_UseRim").unwrap_or(0.0) != 0.0 {
            self.warn(format!("リムライトは写像しない: {slot}"));
        }
        if number(material, "_UseBumpMap").unwrap_or(0.0) != 0.0 {
            self.warn(format!("バンプマップは写像しない: {slot}"));
        }
        // `_MainTexHSVG` は `m_Colors` のベクトルとして書かれる。
        if let Some(hsvg) = color(material, "_MainTexHSVG")
            && hsvg != [0.0, 1.0, 1.0, 1.0]
        {
            self.warn(format!("色相・彩度・明度の調整は写像しない: {slot}"));
        }
        let mask_mode = number(material, "_AlphaMaskMode").unwrap_or(0.0);
        if (1.0..=4.0).contains(&mask_mode) && has_texture(material, "_AlphaMask") {
            self.warn(format!("アルファマスクは写像しない: {slot}"));
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
        // `_MainTex_ScrollRotate` は `m_Colors` のベクトルとして書かれる。
        if let Some(scroll) = color(material, "_MainTex_ScrollRotate")
            && scroll != [0.0, 0.0, 0.0, 0.0]
        {
            self.warn(format!("UVスクロール・回転は写像しない: {slot}"));
        }
    }

    /// テクスチャ1件を解決して画像索引を返す。
    ///
    /// プロパティ無し・未割り当ては `None`、索引に無い・形式外は警告して `None`、
    /// 画像の読み込み失敗・上限超過だけは失敗として返す。
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
        let bytes = match read_limited(&self.root.join(&path), self.options.max_image_bytes) {
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
        let Some(mime) = sniff_mime(&bytes) else {
            self.warn(format!(
                "未対応の画像形式のため省く: {slot}: {property}: {path}"
            ));
            return Ok(None);
        };
        // PNGは付加チャンクを落としてから埋め込む。JPEGはそのまま扱う。
        // チャンク構造が読めないPNGは壊さず元のまま残し、警告する。
        // 合計バイト数と `VrmImage.data` は除去後のバイト列で数える。
        let bytes = if mime == VrmImageMime::Png {
            match strip_png_ancillary_chunks(&bytes) {
                Some(stripped) => stripped,
                None => {
                    self.warn(format!(
                        "PNGのチャンク構造が読めないためそのまま埋め込む: {slot}: {property}"
                    ));
                    bytes
                }
            }
        } else {
            bytes
        };
        let total = self.total_image_bytes.saturating_add(bytes.len() as u64);
        if total > self.options.max_total_image_bytes {
            return Err(MaterialError::new(
                MaterialErrorKind::LimitExceeded,
                format!("画像の合計が上限を超えた: {slot}: {property}"),
            )
            .with_path(path));
        }
        let sampler = sampler_of(&record.meta);
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
        Ok(Some(index))
    }
}

/// シェーダ本文の先頭の `Shader "..."` 名を返す。
#[cfg(not(target_arch = "wasm32"))]
fn first_shader_name(text: &str) -> Option<String> {
    text.lines().find_map(|line| {
        line.trim_start()
            .strip_prefix("Shader \"")
            .and_then(|rest| rest.split('"').next().map(str::to_owned))
    })
}

/// シェーダ名から透明度系列を求める。lilToon名でなければ `None` である。
#[cfg(not(target_arch = "wasm32"))]
fn series_of_name(name: &str) -> Option<ShaderSeries> {
    // `Cutout`/`Transparent` を含む他社シェーダをlilToonとして扱わないため、
    // 名前に `lilToon` を含むものだけを対象にする。
    if !name.contains("lilToon") {
        return None;
    }
    if name.contains("Cutout") {
        Some(ShaderSeries::Mask)
    } else if name.contains("Transparent") {
        Some(ShaderSeries::Blend)
    } else {
        Some(ShaderSeries::Opaque)
    }
}

/// 上限付きでファイルを読み取る。`limit` を超えたら `InvalidData` で失敗する。
#[cfg(not(target_arch = "wasm32"))]
fn read_limited(path: &Path, limit: u64) -> std::io::Result<Vec<u8>> {
    use std::io::Read as _;
    let file = std::fs::File::open(path)?;
    let mut limited = file.take(limit.saturating_add(1));
    let mut bytes = Vec::new();
    limited.read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("読み取り上限を超えた: {limit}バイト"),
        ));
    }
    Ok(bytes)
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
#[cfg(not(target_arch = "wasm32"))]
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
#[cfg(not(target_arch = "wasm32"))]
fn has_float(material: &crate::unity::Material, name: &str) -> bool {
    material.floats.iter().any(|entry| entry.name == name)
        || material.ints.iter().any(|entry| entry.name == name)
}

/// 数値プロパティを読む。浮動小数点を先に、整数も受け付ける。
/// 同名は最初の1件を採用する。
#[cfg(not(target_arch = "wasm32"))]
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
#[cfg(not(target_arch = "wasm32"))]
fn color(material: &crate::unity::Material, name: &str) -> Option<[f64; 4]> {
    material
        .colors
        .iter()
        .find(|entry| entry.name == name)
        .map(|entry| entry.value)
}

/// テクスチャ環境を読む。同名は最初の1件を採用する。
#[cfg(not(target_arch = "wasm32"))]
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
#[cfg(not(target_arch = "wasm32"))]
fn has_texture(material: &crate::unity::Material, name: &str) -> bool {
    texture(material, name).is_some_and(|environment| !environment.texture.is_null())
}

/// 先頭バイトからMIMEを求める。PNGとJPEGだけを受け付ける。
#[cfg(not(target_arch = "wasm32"))]
fn sniff_mime(bytes: &[u8]) -> Option<VrmImageMime> {
    if bytes.starts_with(&PNG_MAGIC) {
        Some(VrmImageMime::Png)
    } else if bytes.starts_with(&JPEG_MAGIC) {
        Some(VrmImageMime::Jpeg)
    } else {
        None
    }
}

/// PNGの必須チャンク名の許可表。
///
/// 大文字小文字ではなく名前で判定し、小文字を含む `tRNS` も残す。
#[cfg(not(target_arch = "wasm32"))]
const PNG_ESSENTIAL_CHUNKS: [[u8; 4]; 5] = [*b"IHDR", *b"PLTE", *b"tRNS", *b"IDAT", *b"IEND"];

/// PNGから画素に影響しない付加チャンクを落とす。
///
/// 必須チャンクだけを元のバイト列のまま（長さ・型・データ・CRCをそのまま）残し、
/// 先頭8バイトの署名もそのまま保つ。再エンコードはしない。
/// チャンク構造（長さ・型・データ・CRC・IENDまで）が読めない場合は `None` を返す。
#[cfg(not(target_arch = "wasm32"))]
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
#[cfg(not(target_arch = "wasm32"))]
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
#[cfg(not(target_arch = "wasm32"))]
fn number_of(mapping: &yaml_serde::Mapping, key: &str) -> Option<i64> {
    mapping.get(key).and_then(|value| value.as_i64())
}

/// 繰り返し設定値を写像する。0/1/2以外は繰り返しに倒す。
#[cfg(not(target_arch = "wasm32"))]
fn wrap_of(value: Option<i64>) -> VrmWrap {
    match value {
        Some(1) => VrmWrap::ClampToEdge,
        Some(2) => VrmWrap::MirroredRepeat,
        _ => VrmWrap::Repeat,
    }
}

/// フィルタ設定値を写像する。0点・1双線形・2三線形、他は双線形に倒す。
#[cfg(not(target_arch = "wasm32"))]
fn filter_of(value: Option<i64>) -> VrmFilter {
    match value {
        Some(0) => VrmFilter::Nearest,
        Some(2) => VrmFilter::Trilinear,
        _ => VrmFilter::Linear,
    }
}

/// 0〜1に丸める。
#[cfg(not(target_arch = "wasm32"))]
fn clamp01(value: f64) -> f64 {
    value.clamp(0.0, 1.0)
}

/// sRGB符号化値を線形に直し、0〜1に丸める。
#[cfg(not(target_arch = "wasm32"))]
fn srgb_to_linear(value: f64) -> f32 {
    clamp01(srgb_to_linear_raw(value)) as f32
}

/// sRGB符号化値を線形に直す。丸めは呼び出し側で行う。
#[cfg(not(target_arch = "wasm32"))]
fn srgb_to_linear_raw(value: f64) -> f64 {
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}
