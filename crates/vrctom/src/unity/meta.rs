//! `.meta` の読み取りとアセット種別の分類。
//!
//! 公開契約の入口は [`crate::unity`] を参照。
//!
//! 画像のデコードやFBXの解析はしない。保持するのはGUID・フォルダ識別・
//! importer名・代表設定を含むimporter全体の汎用データまでで、
//! 本文（画像・メッシュ）の読み込みは責務外である。

use yaml_serde::{Mapping, Value};

use super::{Guid, ObjectRef, UnityError, UnityErrorKind, read_guid};

/// `.meta` 1件の既定上限（バイト）。
///
/// 通常のmeta（数十KB以下）に十分な余裕を持たせた仕様値。
const DEFAULT_MAX_META_BYTES: u64 = 4 * 1024 * 1024;

/// `.meta` の読み取り結果。
///
/// GUID・フォルダ識別・importerを型付きで持ち、importer全体の汎用データを
/// [`Importer::settings`] に残す。未知のimporterも捨てずに保持する。
#[derive(Debug, Clone)]
pub struct AssetMeta {
    guid: Guid,
    is_folder: bool,
    importer: Importer,
}

impl AssetMeta {
    /// `.meta` のバイト列を読み取る。
    ///
    /// 画像やFBXの本文には触れず、meta自体だけをYAMLとして読む。
    /// 1件の上限は既定値（4MiB）を使い、変える場合は `parse_with_limit` を使う。
    ///
    /// # Errors
    ///
    /// 既定上限（4MiB）超過・YAML構文不良・GUID不良・importer欠落などの場合に
    /// エラーを返す。未知のimporterはエラーにしない。
    pub fn parse(bytes: &[u8]) -> Result<Self, UnityError> {
        Self::parse_with_limit(bytes, DEFAULT_MAX_META_BYTES)
    }

    /// 1件の上限を指定して `.meta` のバイト列を読み取る。
    ///
    /// 検証の条件は [`parse`](Self::parse) と同じであり、上限だけが変わる。
    /// [`scan_directory`](super::scan_directory) は [`IndexOptions`](super::IndexOptions)
    /// の上限をここへ渡す。
    ///
    /// # Errors
    ///
    /// 上限超過・YAML構文不良・GUID不良・importer欠落などの場合にエラーを返す。
    /// 未知のimporterはエラーにしない。
    pub(crate) fn parse_with_limit(bytes: &[u8], max_meta_bytes: u64) -> Result<Self, UnityError> {
        if bytes.len() as u64 > max_meta_bytes {
            return Err(UnityError::new(
                UnityErrorKind::LimitExceeded,
                format!("metaが上限を超えた: {}バイト", bytes.len()),
            ));
        }
        let text = std::str::from_utf8(bytes).map_err(|error| {
            UnityError::new(
                UnityErrorKind::InvalidMetadata,
                format!(
                    "metaがUTF-8でない: {}",
                    crate::escape_diag(&error.to_string())
                ),
            )
            .with_source(error)
        })?;
        let value: Value = super::yaml::parse_value(text).map_err(|error| {
            let line = error.location().map(|location| location.line() as u64);
            let mut failure = UnityError::new(
                UnityErrorKind::InvalidYaml,
                format!(
                    "metaのYAMLが読めない: {}",
                    crate::escape_diag(&error.to_string())
                ),
            )
            .with_source(error);
            if let Some(line) = line {
                failure = failure.with_line(line);
            }
            failure
        })?;
        let mapping = value.as_mapping().ok_or_else(|| {
            UnityError::new(
                UnityErrorKind::InvalidMetadata,
                "metaのトップレベルがマッピングでない",
            )
        })?;
        Self::decode(mapping)
    }

    /// YAML値を `AssetMeta` に読み替える。
    fn decode(mapping: &Mapping) -> Result<Self, UnityError> {
        let guid = mapping
            .get("guid")
            .ok_or_else(|| UnityError::new(UnityErrorKind::InvalidMetadata, "metaにguidがない"))
            .and_then(read_guid)?;
        let is_folder = match mapping.get("folderAsset") {
            None | Some(Value::Null) => false,
            // YAML 1.1では `yes` も真偽値として読まれる。
            Some(Value::Bool(flag)) => *flag,
            Some(Value::String(text)) => matches!(
                text.to_ascii_lowercase().as_str(),
                "y" | "yes" | "true" | "1" | "on"
            ),
            Some(Value::Number(number)) => number.as_i64().is_some_and(|n| n != 0),
            Some(_) => {
                return Err(UnityError::new(
                    UnityErrorKind::InvalidMetadata,
                    "metaのfolderAssetが真偽値でない",
                ));
            }
        };
        // importerは予約キー以外の唯一のブロックである。未知名も受け付ける。
        let mut importer: Option<(&str, &Value)> = None;
        for (key, value) in mapping {
            let name = key.as_str().ok_or_else(|| {
                UnityError::new(
                    UnityErrorKind::InvalidMetadata,
                    "metaのトップレベルに文字列でないキーがある",
                )
            })?;
            if matches!(name, "fileFormatVersion" | "guid" | "folderAsset") {
                continue;
            }
            if importer.is_some() {
                return Err(UnityError::new(
                    UnityErrorKind::InvalidMetadata,
                    "metaにimporterが複数ある",
                ));
            }
            importer = Some((name, value));
        }
        let (name, settings) = importer.ok_or_else(|| {
            UnityError::new(UnityErrorKind::InvalidMetadata, "metaにimporterがない")
        })?;
        Ok(Self {
            guid,
            is_folder,
            importer: Importer {
                name: name.to_owned(),
                settings: settings.clone(),
            },
        })
    }

    /// アセットのGUIDを返す。
    pub fn guid(&self) -> &Guid {
        &self.guid
    }

    /// フォルダのmetaかどうかを返す。
    pub fn is_folder(&self) -> bool {
        self.is_folder
    }

    /// importerを返す。
    pub fn importer(&self) -> &Importer {
        &self.importer
    }

    /// importer設定の `externalObjects` にある置換表を読み取る。
    ///
    /// ModelImporterのmaterial remap（元の型・assembly・名前と置換先参照）を
    /// 順序通りに返す。`externalObjects` が空や欠落の場合は空になる。
    /// 名前だけで即適用はしない。呼び出し側が解決方針を決める。
    ///
    /// # Errors
    ///
    /// 置換表の形状が壊れている場合に [`UnityErrorKind::InvalidMetadata`] を返す。
    pub fn external_object_remaps(&self) -> Result<Vec<ExternalObjectEntry>, UnityError> {
        let settings = self.importer.settings.as_mapping().ok_or_else(|| {
            UnityError::new(
                UnityErrorKind::InvalidMetadata,
                "importerの設定がマッピングでない",
            )
            .with_field(self.importer.name.clone())
        })?;
        let external = match settings.get("externalObjects") {
            None | Some(Value::Null) => return Ok(Vec::new()),
            Some(value) => value,
        };
        // 空マッピング `{}` は置換なしと同じ扱いにする。
        if let Some(mapping) = external.as_mapping() {
            if mapping.is_empty() {
                return Ok(Vec::new());
            }
            return Err(UnityError::new(
                UnityErrorKind::InvalidMetadata,
                "externalObjectsがシーケンスでない",
            )
            .with_field("externalObjects"));
        }
        let sequence = external.as_sequence().ok_or_else(|| {
            UnityError::new(
                UnityErrorKind::InvalidMetadata,
                "externalObjectsがシーケンスでない",
            )
            .with_field("externalObjects")
        })?;
        let mut out = Vec::with_capacity(sequence.len());
        for item in sequence {
            let entry = item.as_mapping().ok_or_else(|| {
                UnityError::new(
                    UnityErrorKind::InvalidMetadata,
                    "externalObjectsの項目がマッピングでない",
                )
                .with_field("externalObjects")
            })?;
            let first = entry
                .get("first")
                .and_then(|value| value.as_mapping())
                .ok_or_else(|| {
                    UnityError::new(
                        UnityErrorKind::InvalidMetadata,
                        "externalObjectsの項目にfirstがない",
                    )
                    .with_field("externalObjects")
                })?;
            let read_text = |key: &str| {
                first
                    .get(key)
                    .and_then(|value| value.as_str())
                    .ok_or_else(|| {
                        UnityError::new(
                            UnityErrorKind::InvalidMetadata,
                            format!("externalObjectsのfirstに{key}がない"),
                        )
                        .with_field("externalObjects")
                    })
                    .map(str::to_owned)
            };
            let replacement = entry
                .get("second")
                .ok_or_else(|| {
                    UnityError::new(
                        UnityErrorKind::InvalidMetadata,
                        "externalObjectsの項目にsecondがない",
                    )
                    .with_field("externalObjects")
                })
                .and_then(ObjectRef::decode)?;
            out.push(ExternalObjectEntry {
                original_type: read_text("type")?,
                original_assembly: read_text("assembly")?,
                original_name: read_text("name")?,
                replacement,
            });
        }
        Ok(out)
    }
}

/// importer名と設定全体。
///
/// 未知のimporterも名前と汎用データを捨てずに保持する。
/// 代表設定（TextureImporterのtextureTypeなど）もこの汎用データに残る。
#[derive(Debug, Clone)]
pub struct Importer {
    /// importerブロック名（`TextureImporter` など）。
    pub name: String,
    /// importerブロック全体の汎用データ。
    pub settings: Value,
}

/// `externalObjects` の置換1件。
///
/// 置換元の型・assembly・名前と、置換先の参照を対で保持する。
#[derive(Debug, Clone)]
pub struct ExternalObjectEntry {
    /// 置換元の型名（`UnityEngine:Material` など）。
    pub original_type: String,
    /// 置換元のassembly名。
    pub original_assembly: String,
    /// 置換元の名前。
    pub original_name: String,
    /// 置換先の参照。
    pub replacement: ObjectRef,
}

/// アセット種別。
///
/// 拡張子とimporter名から分類する。どれにも当てはまらないものは `Other` とする。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AssetKind {
    /// フォルダのmeta。
    Folder,
    /// `.prefab`。
    Prefab,
    /// `.mat`。
    Material,
    /// 画像（TextureImporter）。
    Texture,
    /// FBXなどのモデル（ModelImporter）。
    Model,
    /// `.anim`。
    AnimationClip,
    /// `.controller`。
    AnimatorController,
    /// `.shader`。
    Shader,
    /// `.renderTexture`。
    RenderTexture,
    /// `.asset`。
    ScriptableObject,
    /// 上記のいずれにも当てはまらないもの。
    Other,
}

/// 相対パスとmetaから種別を分類する。
///
/// 拡張子を先に見て、該当がない場合だけimporter名で補う。
/// フォルダは常に [`AssetKind::Folder`] とする。
pub(crate) fn classify(relative_path: &str, meta: &AssetMeta) -> AssetKind {
    if meta.is_folder() {
        return AssetKind::Folder;
    }
    let extension = relative_path
        .rsplit('/')
        .next()
        .and_then(|file| file.rsplit_once('.'))
        .filter(|(stem, _)| !stem.is_empty())
        .map(|(_, extension)| extension.to_ascii_lowercase());
    match extension.as_deref() {
        Some("prefab") => AssetKind::Prefab,
        Some("mat") => AssetKind::Material,
        Some("anim") => AssetKind::AnimationClip,
        Some("controller") => AssetKind::AnimatorController,
        Some("shader") => AssetKind::Shader,
        Some("rendertexture") => AssetKind::RenderTexture,
        Some("asset") => AssetKind::ScriptableObject,
        Some(
            "png" | "jpg" | "jpeg" | "tga" | "bmp" | "psd" | "gif" | "exr" | "hdr" | "tif" | "tiff"
            | "webp",
        ) => AssetKind::Texture,
        Some("fbx" | "obj" | "dae" | "3ds" | "dxf") => AssetKind::Model,
        _ => match meta.importer().name.as_str() {
            "ModelImporter" => AssetKind::Model,
            "TextureImporter" => AssetKind::Texture,
            "ShaderImporter" => AssetKind::Shader,
            _ => AssetKind::Other,
        },
    }
}
