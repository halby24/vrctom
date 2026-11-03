//! Workerから呼ぶ変換の中核（ターゲット非依存）。
//!
//! `MemoryPackage` と `AssetIndex` を保持する [`Package`] が、候補列挙・変換・
//! オプション解釈・レポート生成を担う。`JsValue` を使わないためnativeで
//! テストできる。JS境界の薄い写しは [`crate::WebPackage`] を参照。

use serde::{Deserialize, Serialize};
use vrctom::conversion::{ConversionError, convert_fbx};
use vrctom::fbx::FbxOptions;
use vrctom::unity::{AssetIndex, IndexOptions};
use vrctom::unitypackage::{EntryKind, MemoryPackage, UnpackOptions, unpack_in_memory};
use vrctom::vrm::{MaterialOptions, VrmExportOptions, VrmExpressionProfile, VrmMeta, VrmOptions};

/// Web向けの資源上限。
///
/// 既定値はWebの目安であり、代表パッケージの計測で見直す。
/// 個別の上限は `with_` で始まるメソッドで差し替え、テストでは
/// 小さい値を使う。既存の入口（[`Package::open`]・[`Package::convert`]）は
/// 既定値を使うため、呼び出し側の変更は要らない。
#[derive(Debug, Clone)]
pub struct Limits {
    max_input_bytes: u64,
    max_total_bytes: u64,
    max_entries: u64,
    max_fbx_bytes: u64,
    max_image_bytes: u64,
    max_total_image_bytes: u64,
    max_image_pixels: u64,
    max_total_image_pixels: u64,
    max_output_bytes: usize,
}

impl Limits {
    /// Web向けの既定の上限を作る。
    ///
    /// 圧縮入力512MiB・展開後2GiB・tar10万件・FBX1件128MiB・
    /// 画像1枚32MiB・合計256MiB・画素1枚16777216（4096²）・
    /// 復号結果の保持量の予算67108864（4096²x4、RGBAで256MiB）・
    /// VRM出力512MiBである。
    ///
    /// 保持量の予算は復号結果の共有表が持つ画素数の目安であり、
    /// 超えたら保持を捨てて再復号する。上限は安全のための
    /// 目安であり、代表パッケージの計測で見直す。
    pub fn new() -> Self {
        Self {
            max_input_bytes: 512 * 1024 * 1024,
            max_total_bytes: 2 * 1024 * 1024 * 1024,
            max_entries: 100_000,
            max_fbx_bytes: 128 * 1024 * 1024,
            max_image_bytes: 32 * 1024 * 1024,
            max_total_image_bytes: 256 * 1024 * 1024,
            max_image_pixels: 16_777_216,
            max_total_image_pixels: 67_108_864,
            max_output_bytes: 512 * 1024 * 1024,
        }
    }

    /// 圧縮入力の上限（バイト数）を変更する。
    pub fn with_max_input_bytes(mut self, bytes: u64) -> Self {
        self.max_input_bytes = bytes;
        self
    }

    /// 展開後総量の上限（バイト数）を変更する。
    pub fn with_max_total_bytes(mut self, bytes: u64) -> Self {
        self.max_total_bytes = bytes;
        self
    }

    /// tarエントリ数の上限を変更する。
    pub fn with_max_entries(mut self, count: u64) -> Self {
        self.max_entries = count;
        self
    }

    /// FBX1件の上限（バイト数）を変更する。
    pub fn with_max_fbx_bytes(mut self, bytes: u64) -> Self {
        self.max_fbx_bytes = bytes;
        self
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

    /// 画像1枚の画素数の上限を変更する。
    pub fn with_max_image_pixels(mut self, pixels: u64) -> Self {
        self.max_image_pixels = pixels;
        self
    }

    /// 復号結果の保持量の予算を変更する。
    ///
    /// 焼きの共有表が保持する画素数の目安であり、超えたら保持を捨てて
    /// 再復号する。予算の超過では失敗しない。
    pub fn with_max_total_image_pixels(mut self, pixels: u64) -> Self {
        self.max_total_image_pixels = pixels;
        self
    }

    /// VRM出力の上限（バイト数）を変更する。
    pub fn with_max_output_bytes(mut self, bytes: usize) -> Self {
        self.max_output_bytes = bytes;
        self
    }

    /// 材質読み取りの設定を作る。画素上限はここから渡す。
    fn material_options(&self) -> MaterialOptions {
        MaterialOptions::new()
            .with_max_image_bytes(self.max_image_bytes)
            .with_max_total_image_bytes(self.max_total_image_bytes)
            .with_max_image_pixels(self.max_image_pixels)
            .with_max_total_image_pixels(self.max_total_image_pixels)
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self::new()
    }
}

/// 失敗した段階。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// パッケージの展開・索引化。
    Package,
    /// 変換オプションJSONの解釈。
    Options,
    /// FBXの取り出し。
    Fbx,
    /// FBX解析・中間モデル生成。
    Model,
    /// 材質の読み取り。
    Material,
    /// VRMの書き出し。
    Export,
}

impl Stage {
    /// JSへ渡す段階名を返す。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Package => "package",
            Self::Options => "options",
            Self::Fbx => "fbx",
            Self::Model => "model",
            Self::Material => "material",
            Self::Export => "export",
        }
    }
}

/// 変換の失敗。段階・日本語の診断・対象パスを持つ。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreError {
    stage: Stage,
    message: String,
    path: Option<String>,
}

impl CoreError {
    fn new(stage: Stage, message: impl Into<String>) -> Self {
        Self {
            stage,
            message: message.into(),
            path: None,
        }
    }

    fn with_path(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into());
        self
    }

    /// 失敗した段階を返す。
    pub fn stage(&self) -> Stage {
        self.stage
    }

    /// 日本語の診断文を返す。
    pub fn message(&self) -> &str {
        &self.message
    }

    /// 判明している範囲で対象パスを返す。
    pub fn path(&self) -> Option<&str> {
        self.path.as_deref()
    }
}

impl std::fmt::Display for CoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.stage.as_str(), self.message)?;
        if let Some(path) = &self.path {
            write!(f, " [path: {path}]")?;
        }
        Ok(())
    }
}

impl std::error::Error for CoreError {}

/// 検証済みパッケージとその索引の所有者。
///
/// [`unpack_in_memory`] と [`MemoryPackage::asset_index`] が成功した内容だけを
/// 持ち、入力バイト列自体は保持しない。
#[derive(Debug)]
pub struct Package {
    package: MemoryPackage,
    index: AssetIndex,
}

/// FBX候補1件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FbxCandidate {
    /// 出力パス（`Assets/...`）。
    pub path: String,
    /// 実体のバイト数。
    pub size: u64,
}

/// 変換結果。VRMバイト列とレポートJSONを持つ。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConvertedPackage {
    /// VRM 1.0のGLBバイト列。
    pub vrm: Vec<u8>,
    /// レポートのJSON文字列。
    pub report_json: String,
}

impl Package {
    /// 入力をWeb向け上限で展開し、索引まで作る。
    ///
    /// 検証に失敗したら開かない。
    ///
    /// # Errors
    ///
    /// 入力上限の超過・展開の失敗・索引化の失敗の場合に段階付きで返す。
    pub fn open(bytes: &[u8]) -> Result<Self, CoreError> {
        Self::open_with_limits(bytes, &Limits::new())
    }

    /// 上限を指定して開く。テストで小さな上限を使うための入口である。
    fn open_with_limits(bytes: &[u8], limits: &Limits) -> Result<Self, CoreError> {
        if bytes.len() as u64 > limits.max_input_bytes {
            return Err(CoreError::new(
                Stage::Package,
                format!(
                    "入力が上限を超えた: {}バイト（上限{}バイト）",
                    bytes.len(),
                    limits.max_input_bytes,
                ),
            ));
        }
        let options = web_unpack_options(limits);
        let package = unpack_in_memory(bytes, &options).map_err(|error| {
            let path = error.pathname().map(str::to_owned);
            let mut failure = CoreError::new(
                Stage::Package,
                format!("パッケージの展開に失敗した: {error}"),
            );
            if let Some(path) = path {
                failure = failure.with_path(path);
            }
            failure
        })?;
        let index = package
            .asset_index(&IndexOptions::default())
            .map_err(|error| {
                let path = error.path().map(str::to_owned);
                let mut failure = CoreError::new(
                    Stage::Package,
                    format!("パッケージの索引化に失敗した: {error}"),
                );
                if let Some(path) = path {
                    failure = failure.with_path(path);
                }
                failure
            })?;
        Ok(Self { package, index })
    }

    /// FBX候補をパス昇順で返す。
    ///
    /// `entries()` 自体がパス順のため、その順序を保つ。
    /// 拡張子の判定はバイト位置ではなく文字境界で行い、
    /// 日本語を含む非FBXがあってもパニックしない。
    pub fn fbx_candidates(&self) -> Vec<FbxCandidate> {
        self.package
            .entries()
            .iter()
            .filter(|entry| entry.kind == EntryKind::File && is_fbx_path(&entry.pathname))
            .map(|entry| FbxCandidate {
                path: entry.pathname.clone(),
                size: self
                    .package
                    .read(&entry.pathname)
                    .map(|bytes| bytes.len() as u64)
                    .unwrap_or(0),
            })
            .collect()
    }

    /// FBX候補のJSON文字列を返す。
    pub fn candidates_json(&self) -> String {
        serde_json::to_string(&self.fbx_candidates()).unwrap_or_else(|_| "[]".to_owned())
    }

    /// 選んだFBXを変換する。
    ///
    /// `fbx_path` は `Assets/...` の出力パス、`options_json` は
    /// `{"name","authors","expressions"}` のJSONである。
    /// 資源上限はWeb向けの既定値を使う。
    ///
    /// # Errors
    ///
    /// オプション不正・FBX不在・材質読み取り・変換の失敗の場合に段階付きで返す。
    pub fn convert(
        &self,
        fbx_path: &str,
        options_json: &str,
    ) -> Result<ConvertedPackage, CoreError> {
        self.convert_with_limits(fbx_path, options_json, &Limits::new())
    }

    /// 上限を指定して変換する。テストで小さな上限を使うための入口である。
    fn convert_with_limits(
        &self,
        fbx_path: &str,
        options_json: &str,
        limits: &Limits,
    ) -> Result<ConvertedPackage, CoreError> {
        let options = parse_options(options_json)?;
        let fbx_bytes = self.fbx_bytes(fbx_path, limits.max_fbx_bytes)?;
        let relative = fbx_relative(fbx_path)?;
        let (materials, material_report) = vrctom::vrm::VrmMaterials::load_with(
            &self.package,
            &self.index,
            &relative,
            &limits.material_options(),
        )
        .map_err(|error| {
            let path = error.path().map(str::to_owned);
            let mut failure = CoreError::new(
                Stage::Material,
                format!("材質の読み取りに失敗した: {error}"),
            );
            if let Some(path) = path {
                failure = failure.with_path(path);
            }
            failure
        })?;
        let mut warnings: Vec<String> = material_report.warnings().to_vec();
        if materials.is_none() {
            warnings.push("材質を読めなかったため灰色で出力する".to_owned());
        }
        let material_count = materials.as_ref().map_or(0, |found| found.entries().len());
        let image_count = materials.as_ref().map_or(0, |found| found.images().len());
        let mut vrm_options = VrmOptions::new();
        if options.expressions_vrc {
            vrm_options = vrm_options.with_expressions(VrmExpressionProfile::vrchat());
        }
        let meta = VrmMeta::new(options.name(&relative), options.authors());
        let mut export_options =
            VrmExportOptions::new(meta).with_max_output_bytes(limits.max_output_bytes);
        if let Some(materials) = materials {
            export_options = export_options.with_materials(materials);
        }
        let (vrm, model_report) = convert_fbx(
            &fbx_bytes,
            &FbxOptions::default(),
            &vrm_options,
            &export_options,
        )
        .map_err(|error| conversion_error(&error, fbx_path))?;
        let report = Report {
            fbx: fbx_path.to_owned(),
            bytes: vrm.len() as u64,
            nodes: model_report.node_count,
            meshes: model_report.mesh_count,
            skins: model_report.skin_count,
            materials: material_count,
            images: image_count,
            expressions: model_report.expression_count,
            warnings,
        };
        let report_json = serde_json::to_string(&report).map_err(|error| {
            CoreError::new(
                Stage::Export,
                format!("レポートのJSON化に失敗した: {error}"),
            )
            .with_path(fbx_path.to_owned())
        })?;
        Ok(ConvertedPackage { vrm, report_json })
    }

    /// 出力パスからFBX実体を取り出す。ファイル項目に無い場合と
    /// 上限を超える場合は失敗する。
    ///
    /// 借用した内容の長さを先に検査し、上限内の場合だけ複製する。
    /// 上限超過時に上限級の複製が起きない。
    fn fbx_bytes(&self, fbx_path: &str, max_fbx_bytes: u64) -> Result<Vec<u8>, CoreError> {
        let is_file = self
            .package
            .entries()
            .iter()
            .any(|entry| entry.kind == EntryKind::File && entry.pathname == fbx_path);
        if !is_file {
            return Err(CoreError::new(
                Stage::Fbx,
                format!("パッケージ内にFBXが見つからない: {fbx_path}"),
            )
            .with_path(fbx_path.to_owned()));
        }
        let stored = self.package.read(fbx_path).ok_or_else(|| {
            CoreError::new(
                Stage::Fbx,
                format!("パッケージ内にFBXが見つからない: {fbx_path}"),
            )
            .with_path(fbx_path.to_owned())
        })?;
        if stored.len() as u64 > max_fbx_bytes {
            return Err(CoreError::new(
                Stage::Fbx,
                format!(
                    "FBXが上限を超えた: {}バイト（上限{max_fbx_bytes}バイト）",
                    stored.len(),
                ),
            )
            .with_path(fbx_path.to_owned()));
        }
        Ok(stored.to_vec())
    }
}

/// Web向けの展開上限。
fn web_unpack_options(limits: &Limits) -> UnpackOptions {
    UnpackOptions::new()
        .with_max_total_bytes(limits.max_total_bytes)
        .with_max_entries(limits.max_entries)
}

/// 出力パスのファイル名の拡張子がFBXかどうかを返す。
///
/// バイト位置ではなく最後の `/` 以降のファイル名を `rsplit_once('.')` で
/// 判定するため、非ASCIIを含むパスでもパニックしない。
fn is_fbx_path(pathname: &str) -> bool {
    let file = pathname.rsplit('/').next().unwrap_or(pathname);
    file.rsplit_once('.')
        .is_some_and(|(_, ext)| ext.eq_ignore_ascii_case("fbx"))
}

/// 出力パス（`Assets/...`）から索引用の相対パスを作る。
fn fbx_relative(fbx_path: &str) -> Result<String, CoreError> {
    let Some((_, relative)) = fbx_path.split_once('/') else {
        return Err(CoreError::new(
            Stage::Fbx,
            format!("FBXのパスがAssets配下にない: {fbx_path}"),
        )
        .with_path(fbx_path.to_owned()));
    };
    if relative.is_empty() {
        return Err(CoreError::new(
            Stage::Fbx,
            format!("FBXのパスがAssets配下にない: {fbx_path}"),
        )
        .with_path(fbx_path.to_owned()));
    }
    Ok(relative.to_owned())
}

/// 変換失敗を段階付きエラーへ写す。
fn conversion_error(error: &ConversionError, fbx_path: &str) -> CoreError {
    let stage = match error {
        ConversionError::Fbx(_) => Stage::Fbx,
        ConversionError::Model(_) => Stage::Model,
        ConversionError::Export(_) => Stage::Export,
    };
    let stage_ja = match error {
        ConversionError::Fbx(_) => "FBXの解析",
        ConversionError::Model(_) => "中間モデルの生成",
        ConversionError::Export(_) => "VRMの書き出し",
    };
    CoreError::new(stage, format!("{stage_ja}に失敗した: {error}")).with_path(fbx_path.to_owned())
}

/// 変換オプションのJSONの読み取り結果。
#[derive(Debug, Clone, Default, Deserialize)]
struct RawOptions {
    /// 作品名。空・未指定ならFBXのstemになる。
    #[serde(default)]
    name: Option<String>,
    /// 作者列。空・未指定なら `["unknown"]` になる。
    #[serde(default)]
    authors: Option<Vec<String>>,
    /// 表情指定。`"vrc"` だけがVRChat向け8件になり、それ以外は既定（無し）である。
    #[serde(default)]
    expressions: Option<String>,
}

/// 解釈済みの変換オプション。
#[derive(Debug, Clone)]
struct ResolvedOptions {
    name: Option<String>,
    authors: Vec<String>,
    expressions_vrc: bool,
}

impl ResolvedOptions {
    /// 作品名を決める。空・未指定ならFBXのstem、stemも空なら `model` である。
    ///
    /// `VrmMeta` が空のnameを受け付けないため、空では返さない。
    fn name(&self, relative: &str) -> String {
        if let Some(name) = self.name.as_deref().map(str::trim)
            && !name.is_empty()
        {
            return name.to_owned();
        }
        let file = relative.rsplit('/').next().unwrap_or(relative);
        let stem = file.rsplit_once('.').map_or(file, |(stem, _)| stem);
        if stem.is_empty() {
            return "model".to_owned();
        }
        stem.to_owned()
    }

    /// 作者列を決める。空要素を除き、残らなければ `["unknown"]` である。
    fn authors(&self) -> Vec<String> {
        let kept: Vec<String> = self
            .authors
            .iter()
            .map(|author| author.trim().to_owned())
            .filter(|author| !author.is_empty())
            .collect();
        if kept.is_empty() {
            vec!["unknown".to_owned()]
        } else {
            kept
        }
    }
}

/// 変換オプションJSONを解釈する。
fn parse_options(options_json: &str) -> Result<ResolvedOptions, CoreError> {
    let raw: RawOptions = serde_json::from_str(options_json).map_err(|error| {
        CoreError::new(
            Stage::Options,
            format!("変換オプションのJSONを読めない: {error}"),
        )
    })?;
    Ok(ResolvedOptions {
        name: raw.name,
        authors: raw.authors.unwrap_or_default(),
        expressions_vrc: raw.expressions.as_deref() == Some("vrc"),
    })
}

/// UIへ返すレポート。
#[derive(Debug, Clone, Serialize)]
struct Report {
    fbx: String,
    bytes: u64,
    nodes: usize,
    meshes: usize,
    skins: usize,
    materials: usize,
    images: usize,
    expressions: usize,
    warnings: Vec<String>,
}

#[cfg(test)]
#[allow(non_snake_case)]
mod tests {
    use super::*;
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use std::io::Write;

    // テスト名は日本語で書く（テストは仕様・契約であるため）。

    /// テスト用メンバー。順序は `Vec` の並びでtarへ書き込む。
    enum Member {
        Pathname { guid: String, content: Vec<u8> },
        Asset { guid: String, content: Vec<u8> },
        Meta { guid: String, content: Vec<u8> },
    }

    /// 32桁ASCII hexのテスト用GUIDを生成する。
    fn guid(n: u128) -> String {
        format!("{n:032x}")
    }

    /// tarの1ブロックを組み立てる。通常のustar配置でチェックサムを付ける。
    fn raw_block(name: &[u8], typeflag: u8, size: u64) -> [u8; 512] {
        let mut block = [0u8; 512];
        let name_len = name.len().min(100);
        block[..name_len].copy_from_slice(&name[..name_len]);
        block[100..108].copy_from_slice(b"0000777\0");
        block[108..116].copy_from_slice(b"0000000\0");
        block[116..124].copy_from_slice(b"0000000\0");
        let size_field = format!("{size:011o}\0");
        block[124..136].copy_from_slice(size_field.as_bytes());
        block[136..148].copy_from_slice(b"00000000000\0");
        block[148..156].copy_from_slice(b"        ");
        block[156] = typeflag;
        block[257..262].copy_from_slice(b"ustar");
        block[263..265].copy_from_slice(b"00");
        let sum: u32 = block.iter().map(|byte| *byte as u32).sum();
        let cksum_field = format!("{sum:06o}\0 ");
        block[148..156].copy_from_slice(cksum_field.as_bytes());
        block
    }

    /// メンバー列を `.unitypackage` バイト列（gzip tar）にする。
    fn package_bytes(members: &[Member]) -> Vec<u8> {
        let mut tar = Vec::new();
        for member in members {
            let (path, content): (Vec<u8>, &[u8]) = match member {
                Member::Pathname { guid, content } => {
                    (format!("{guid}/pathname").into_bytes(), content)
                }
                Member::Asset { guid, content } => (format!("{guid}/asset").into_bytes(), content),
                Member::Meta { guid, content } => {
                    (format!("{guid}/asset.meta").into_bytes(), content)
                }
            };
            tar.extend_from_slice(&raw_block(&path, b'0', content.len() as u64));
            tar.extend_from_slice(content);
            tar.extend_from_slice(&vec![0u8; (512 - content.len() % 512) % 512]);
        }
        tar.extend_from_slice(&[0u8; 1024]);
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&tar).expect("gzip書き込みは成功する");
        encoder.finish().expect("gzipは完成する")
    }

    /// 最小のファイルmetaを組み立てる。GUIDはtar側と合わせる。
    fn file_meta(n: u128, importer: &str, body: &str) -> Vec<u8> {
        format!(
            "fileFormatVersion: 2\nguid: {}\n{importer}:\n{body}",
            guid(n)
        )
        .into_bytes()
    }

    /// ダミーのFBXとmetaを含む小さなパッケージを作る。
    ///
    /// FBX実体は壊れた内容であり、候補列挙までの確認に使う。
    /// 実際に変換する場合は `VRCTOM_FBX_FIXTURE` の実FBXを使う。
    fn dummy_package() -> Vec<u8> {
        package_bytes(&[
            Member::Pathname {
                guid: guid(1),
                content: b"Assets/Model.fbx".to_vec(),
            },
            Member::Asset {
                guid: guid(1),
                content: b"dummy-fbx".to_vec(),
            },
            Member::Meta {
                guid: guid(1),
                content: file_meta(1, "ModelImporter", "  externalObjects: {}\n"),
            },
            Member::Pathname {
                guid: guid(2),
                content: b"Assets/Sub/Other.fbx".to_vec(),
            },
            Member::Asset {
                guid: guid(2),
                content: b"other".to_vec(),
            },
            Member::Meta {
                guid: guid(2),
                content: file_meta(2, "ModelImporter", "  externalObjects: {}\n"),
            },
            Member::Pathname {
                guid: guid(3),
                content: b"Assets/Note.txt".to_vec(),
            },
            Member::Asset {
                guid: guid(3),
                content: b"note".to_vec(),
            },
            Member::Meta {
                guid: guid(3),
                content: file_meta(3, "DefaultImporter", "  externalObjects: {}\n"),
            },
        ])
    }

    #[test]
    fn ダミーパッケージを開いて候補をパス順に挙げる() {
        let bytes = dummy_package();
        let package = Package::open(&bytes).expect("ダミーパッケージは開ける");
        let candidates = package.fbx_candidates();
        // FBXだけがパス昇順で挙がり、txtは含まれない。
        assert_eq!(
            candidates
                .iter()
                .map(|entry| entry.path.as_str())
                .collect::<Vec<_>>(),
            vec!["Assets/Model.fbx", "Assets/Sub/Other.fbx"],
        );
        assert_eq!(candidates[0].size, b"dummy-fbx".len() as u64);
        assert_eq!(candidates[1].size, b"other".len() as u64);
        let json = package.candidates_json();
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("JSONは読める");
        assert_eq!(
            parsed,
            serde_json::json!([
                {"path": "Assets/Model.fbx", "size": b"dummy-fbx".len()},
                {"path": "Assets/Sub/Other.fbx", "size": b"other".len()},
            ]),
        );
    }

    #[test]
    fn 日本語を含む非FBXがあっても候補列挙はパニックしない() {
        // 拡張子の判定がバイト境界を仮定すると、日本語を含む短い非FBX名で
        // パニックする。文字境界での判定に直したことの回帰として残す。
        let mut members = vec![
            Member::Pathname {
                guid: guid(1),
                content: b"Assets/Model.fbx".to_vec(),
            },
            Member::Asset {
                guid: guid(1),
                content: b"dummy-fbx".to_vec(),
            },
            Member::Meta {
                guid: guid(1),
                content: file_meta(1, "ModelImporter", "  externalObjects: {}\n"),
            },
        ];
        for (n, name) in [(2u128, "Assets/日本語.cs"), (3u128, "Assets/あ.fbx")] {
            members.push(Member::Pathname {
                guid: guid(n),
                content: name.as_bytes().to_vec(),
            });
            members.push(Member::Asset {
                guid: guid(n),
                content: b"dummy".to_vec(),
            });
            members.push(Member::Meta {
                guid: guid(n),
                content: file_meta(n, "DefaultImporter", "  externalObjects: {}\n"),
            });
        }
        let package = Package::open(&package_bytes(&members)).expect("開ける");
        // FBXだけが挙がり、日本語の非FBXは含まれない。
        assert_eq!(
            package
                .fbx_candidates()
                .iter()
                .map(|entry| entry.path.as_str())
                .collect::<Vec<_>>(),
            vec!["Assets/Model.fbx", "Assets/あ.fbx"],
        );
    }

    /// 1x1のRGBA8のPNGバイト列。材質付きテストのテクスチャに使う。
    const TEST_PNG_1X1: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F,
        0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78, 0xDA, 0x63, 0x64,
        0x60, 0xF8, 0x5F, 0x0F, 0x00, 0x02, 0x87, 0x01, 0x80, 0xEB, 0x47, 0xBA, 0x92, 0x00, 0x00,
        0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];

    /// 不透明lilToonシェーダのGUID。`.shader` が無い場合の系列判定に使う。
    const TEST_SHADER_OPAQUE: &str = "df12117ecd77c31469c224178886498e";

    /// 材質読み取りまで届く最小パッケージを作る。
    ///
    /// FBX実体はダミーであり、材質の上限確認に使う。実際に変換する場合は
    /// `VRCTOM_FBX_FIXTURE` の実FBXを使う。
    fn material_package() -> Vec<u8> {
        let fbx = guid(9);
        let mat = guid(0x11);
        let tex = guid(0x22);
        let fbx_meta = format!(
            "fileFormatVersion: 2\nguid: {fbx}\nModelImporter:\n  externalObjects:\n  - first:\n      type: UnityEngine:Material\n      assembly: UnityEngine.CoreModule\n      name: Body\n    second: {{fileID: 2100000, guid: {mat}, type: 2}}\n"
        );
        let mat_body = format!(
            "%YAML 1.1\n%TAG !u! tag:unity3d.com,2011:\n--- !u!21 &2100000\nMaterial:\n  m_Name: Body\n  m_Shader: {{fileID: 4800000, guid: {TEST_SHADER_OPAQUE}, type: 3}}\n  m_SavedProperties:\n    m_TexEnvs:\n    - _MainTex:\n        m_Texture: {{fileID: 2800000, guid: {tex}, type: 3}}\n        m_Scale: {{x: 1, y: 1}}\n        m_Offset: {{x: 0, y: 0}}\n    m_Floats:\n    - _Cutoff: 0.5\n    - _Cull: 2.0\n    - _ZWrite: 1.0\n    m_Ints: []\n    m_Colors:\n    - _Color: {{r: 1, g: 1, b: 1, a: 1}}\n"
        );
        let mat_meta = format!(
            "fileFormatVersion: 2\nguid: {mat}\nNativeFormatImporter:\n  externalObjects: {{}}\n  userData: \n"
        );
        let tex_meta = format!(
            "fileFormatVersion: 2\nguid: {tex}\nTextureImporter:\n  externalObjects: {{}}\n  textureSettings:\n    filterMode: 1\n    wrapU: 0\n    wrapV: 0\n  mipmaps:\n    enableMipMap: 1\n"
        );
        package_bytes(&[
            Member::Pathname {
                guid: guid(9),
                content: b"Assets/Model.fbx".to_vec(),
            },
            Member::Asset {
                guid: guid(9),
                content: b"dummy-fbx".to_vec(),
            },
            Member::Meta {
                guid: guid(9),
                content: fbx_meta.into_bytes(),
            },
            Member::Pathname {
                guid: guid(0x11),
                content: b"Assets/Materials/Body.mat".to_vec(),
            },
            Member::Asset {
                guid: guid(0x11),
                content: mat_body.into_bytes(),
            },
            Member::Meta {
                guid: guid(0x11),
                content: mat_meta.into_bytes(),
            },
            Member::Pathname {
                guid: guid(0x22),
                content: b"Assets/Textures/Body.png".to_vec(),
            },
            Member::Asset {
                guid: guid(0x22),
                content: TEST_PNG_1X1.to_vec(),
            },
            Member::Meta {
                guid: guid(0x22),
                content: tex_meta.into_bytes(),
            },
        ])
    }

    #[test]
    fn FBX上限の超過はfbx段階で失敗する() {
        let package = Package::open(&material_package()).expect("材質付きパッケージは開ける");
        let limits = Limits::new().with_max_fbx_bytes(1);
        let error = package
            .convert_with_limits("Assets/Model.fbx", "{}", &limits)
            .expect_err("FBX上限の超過は失敗する");
        assert_eq!(error.stage(), Stage::Fbx);
        assert_eq!(error.stage().as_str(), "fbx");
        assert_eq!(error.path(), Some("Assets/Model.fbx"));
    }

    #[test]
    fn 画像上限の超過はmaterial段階で失敗する() {
        let package = Package::open(&material_package()).expect("材質付きパッケージは開ける");
        let limits = Limits::new().with_max_image_bytes(1);
        let error = package
            .convert_with_limits("Assets/Model.fbx", "{}", &limits)
            .expect_err("画像上限の超過は失敗する");
        assert_eq!(error.stage(), Stage::Material);
        assert_eq!(error.stage().as_str(), "material");
    }

    #[test]
    fn 壊れたパッケージはpackage段階で失敗する() {
        let mut bytes = dummy_package();
        bytes.truncate(bytes.len() / 2);
        let error = Package::open(&bytes).expect_err("途中切れは失敗する");
        assert_eq!(error.stage(), Stage::Package);
        assert_eq!(error.stage().as_str(), "package");
        assert!(!error.message().is_empty());
    }

    #[test]
    fn 入力上限の超過はpackage段階で失敗する() {
        let bytes = dummy_package();
        let limits = Limits::new().with_max_input_bytes(8);
        let error = Package::open_with_limits(&bytes, &limits).expect_err("上限超過は失敗する");
        assert_eq!(error.stage(), Stage::Package);
    }

    #[test]
    fn 展開上限の超過はpackage段階で失敗する() {
        let bytes = dummy_package();
        let limits = Limits::new().with_max_entries(1);
        let error =
            Package::open_with_limits(&bytes, &limits).expect_err("エントリ数超過は失敗する");
        assert_eq!(error.stage(), Stage::Package);
    }

    #[test]
    fn 存在しないFBXの変換はfbx段階で失敗する() {
        let bytes = dummy_package();
        let package = Package::open(&bytes).expect("ダミーパッケージは開ける");
        let error = package
            .convert("Assets/Missing.fbx", "{}")
            .expect_err("FBX不在は失敗する");
        assert_eq!(error.stage(), Stage::Fbx);
        assert_eq!(error.stage().as_str(), "fbx");
        assert_eq!(error.path(), Some("Assets/Missing.fbx"));
    }

    #[test]
    fn オプションの既定値はstemとunknownとnoneになる() {
        let resolved = parse_options("{}").expect("空オブジェクトは読める");
        assert_eq!(resolved.name("Model.fbx"), "Model");
        assert_eq!(resolved.authors(), vec!["unknown".to_owned()]);
        assert!(!resolved.expressions_vrc);
        // authorsが空配列・空文字だけの場合もunknownになる。
        let resolved = parse_options(r#"{"authors":[]}"#).expect("空配列は読める");
        assert_eq!(resolved.authors(), vec!["unknown".to_owned()]);
        let resolved = parse_options(r#"{"authors":["", "  "]}"#).expect("空文字だけは読める");
        assert_eq!(resolved.authors(), vec!["unknown".to_owned()]);
        // nameが空ならstem、expressionsがvrc以外なら無効である。
        let resolved = parse_options(r#"{"name":"","expressions":"none"}"#).expect("空名は読める");
        assert_eq!(resolved.name("Model.fbx"), "Model");
        assert!(!resolved.expressions_vrc);
        let resolved = parse_options(r#"{"expressions":"vrc"}"#).expect("vrcは読める");
        assert!(resolved.expressions_vrc);
    }

    #[test]
    fn 不正なオプションJSONはoptions段階で失敗する() {
        for json in [
            "{壊れた",
            "[1,2]",
            r#"{"authors":"ひとり"}"#,
            r#"{"name":42}"#,
        ] {
            let error = parse_options(json).expect_err(&format!("不正JSONは失敗する: {json}"));
            assert_eq!(error.stage(), Stage::Options, "入力: {json}");
            assert_eq!(error.stage().as_str(), "options");
        }
    }

    /// 実FBXを包んだパッケージで変換し、GLBマジックとレポートを確認する。
    ///
    /// `VRCTOM_FBX_FIXTURE` が指すFBXだけを読み、未設定なら跳ばす。
    /// 既存テストの慣習に合わせ、相対指定はリポジトリルート基準に解決する。
    #[test]
    fn 実FBXを包んだパッケージで変換できる() {
        let Ok(path) = std::env::var("VRCTOM_FBX_FIXTURE") else {
            eprintln!("VRCTOM_FBX_FIXTURE未設定のため実FBX変換を跳ばす");
            return;
        };
        let fixture = std::path::PathBuf::from(path);
        let fixture = if fixture.is_absolute() {
            fixture
        } else {
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .and_then(|parent| parent.parent())
                .expect("CARGO_MANIFEST_DIRに二つ上の親がある")
                .join(fixture)
        };
        let fbx_bytes = std::fs::read(&fixture).expect("実FBXは読める");
        let bytes = package_bytes(&[
            Member::Pathname {
                guid: guid(1),
                content: b"Assets/Model.fbx".to_vec(),
            },
            Member::Asset {
                guid: guid(1),
                content: fbx_bytes,
            },
            Member::Meta {
                guid: guid(1),
                content: file_meta(1, "ModelImporter", "  externalObjects: {}\n"),
            },
        ]);
        let package = Package::open(&bytes).expect("実FBXのパッケージは開ける");
        assert_eq!(
            package
                .fbx_candidates()
                .iter()
                .map(|entry| entry.path.as_str())
                .collect::<Vec<_>>(),
            vec!["Assets/Model.fbx"],
        );
        let converted = package
            .convert("Assets/Model.fbx", r#"{"expressions":"none"}"#)
            .expect("実FBXの変換は成功する");
        // GLB容器のマジックを確認する。
        assert!(converted.vrm.len() > 4, "VRMバイト列はマジックより長い",);
        assert_eq!(&converted.vrm[..4], b"glTF", "GLBマジックがある");
        let report: serde_json::Value =
            serde_json::from_str(&converted.report_json).expect("レポートはJSONである");
        assert_eq!(report["fbx"], serde_json::json!("Assets/Model.fbx"));
        assert_eq!(
            report["bytes"],
            serde_json::json!(converted.vrm.len() as u64),
            "bytesはVRMバイト列の大きさである",
        );
        for key in [
            "nodes",
            "meshes",
            "skins",
            "materials",
            "images",
            "expressions",
        ] {
            assert!(report[key].is_number(), "{key}は数値である");
        }
        assert!(report["warnings"].is_array(), "warningsは配列である");
    }
}
