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
/// CLIの制限付き読み取りもこの値を使う（二重管理しない）。
pub const MAX_META_BYTES: u64 = 4 * 1024 * 1024;

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
        Self::parse_with_limit(bytes, MAX_META_BYTES)
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

    /// `humanDescription.human` の対応表を読み取る。
    ///
    /// Unityのhumanoid設定（`humanName` と `boneName` の組）を順序通りに返す。
    /// VRM型には依存せず、呼び出し側がVRMの骨へ写す。
    /// `skeleton`（T-pose）は読まない。通常の書き出しのrest姿勢には使わない。
    ///
    /// 空の対応（`boneName`・`humanName` のいずれかが空）は補完対象として無視する。
    /// 明示的に非Humanoidの `animationType`・`humanDescription` の欠落や空は
    /// 空の対応表として返す。名前推測への置換は呼び出し側が決める。
    ///
    /// # Errors
    ///
    /// `animationType`・`humanDescription`・`human` の形状が壊れている場合に
    /// [`UnityErrorKind::InvalidMetadata`] を返す。
    pub fn human_bone_mappings(&self) -> Result<Vec<HumanBoneMapping>, UnityError> {
        let settings = self.importer.settings.as_mapping().ok_or_else(|| {
            UnityError::new(
                UnityErrorKind::InvalidMetadata,
                "importerの設定がマッピングでない",
            )
            .with_field(self.importer.name.clone())
        })?;
        // 取り込み方式が明示的に非Humanoidの場合は対応表を採用しない。
        // 省略時は有効な対応表があれば採用する。
        if let Some(value) = settings.get("animationType")
            && !is_human_animation_type(value)?
        {
            return Ok(Vec::new());
        }
        let description = match settings.get("humanDescription") {
            None | Some(Value::Null) => return Ok(Vec::new()),
            Some(value) => value,
        };
        let description = description.as_mapping().ok_or_else(|| {
            UnityError::new(
                UnityErrorKind::InvalidMetadata,
                "humanDescriptionがマッピングでない",
            )
            .with_field("humanDescription")
        })?;
        let human = match description.get("human") {
            None | Some(Value::Null) => return Ok(Vec::new()),
            Some(value) => value,
        };
        let sequence = human.as_sequence().ok_or_else(|| {
            UnityError::new(
                UnityErrorKind::InvalidMetadata,
                "humanDescription.humanがシーケンスでない",
            )
            .with_field("humanDescription.human")
        })?;
        let mut out = Vec::with_capacity(sequence.len());
        for (index, item) in sequence.iter().enumerate() {
            let field = format!("humanDescription.human[{index}]");
            let entry = item.as_mapping().ok_or_else(|| {
                UnityError::new(
                    UnityErrorKind::InvalidMetadata,
                    "humanDescription.humanの項目がマッピングでない",
                )
                .with_field(field.clone())
            })?;
            let bone_name = read_optional_text(entry, "boneName", &field)?;
            let human_name = read_optional_text(entry, "humanName", &field)?;
            // 空の対応は無視し、その骨は名前推測の補完対象に残す。
            if bone_name.is_empty() || human_name.is_empty() {
                continue;
            }
            out.push(HumanBoneMapping {
                human_name,
                bone_name,
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

/// `.fbx.meta` の `humanDescription.human` 1件の対応付け。
///
/// Unityの骨名（`humanName`）と対応するノード名（`boneName`）の組である。
/// VRM型には依存しない。呼び出し側がVRMの骨へ写す。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HumanBoneMapping {
    /// Unityの骨名（`humanName`）。空白を含む表記（`Left Thumb Proximal`）のまま保つ。
    ///
    /// 照合時の空白除去は呼び出し側が行う。
    pub human_name: String,
    /// 対応するノード名（`boneName`）。モデルのノード名と完全一致で突き合わせる。
    ///
    /// 空白・大文字小文字の折り畳みや部分一致はしない。
    pub bone_name: String,
}

/// `animationType` がHumanoid（`3`）かどうかを返す。省略相当のnullは真とする。
///
/// 明示的に非Humanoidの値は偽であり、呼び出し側は対応表を採用しない。
/// 解釈できない形状の場合は [`UnityErrorKind::InvalidMetadata`] を返す。
fn is_human_animation_type(value: &Value) -> Result<bool, UnityError> {
    const FIELD: &str = "animationType";
    match value {
        Value::Null => Ok(true),
        Value::Number(_) => {
            // 整数だけを受け付け、小数（`3.0` を含む）・非有限値・整数範囲外は拒否する。
            // `3.0` を `3` へ丸めない。整数値の既存判定範囲は広げない。
            if let Some(generation) = value.as_i64() {
                Ok(generation == 3)
            } else if let Some(generation) = value.as_u64() {
                generation
                    .try_into()
                    .map(|generation: i64| generation == 3)
                    .map_err(|_| {
                        UnityError::new(
                            UnityErrorKind::InvalidMetadata,
                            "animationTypeが解釈できない",
                        )
                        .with_field(FIELD)
                    })
            } else {
                Err(UnityError::new(
                    UnityErrorKind::InvalidMetadata,
                    "animationTypeが解釈できない",
                )
                .with_field(FIELD))
            }
        }
        Value::String(text) => {
            let trimmed = text.trim();
            if trimmed.eq_ignore_ascii_case("human") || trimmed == "3" {
                Ok(true)
            } else if trimmed.eq_ignore_ascii_case("none")
                || trimmed.eq_ignore_ascii_case("legacy")
                || trimmed.eq_ignore_ascii_case("generic")
                || ["0", "1", "2"].contains(&trimmed)
            {
                Ok(false)
            } else {
                Err(UnityError::new(
                    UnityErrorKind::InvalidMetadata,
                    "animationTypeが解釈できない",
                )
                .with_field(FIELD))
            }
        }
        _ => Err(UnityError::new(
            UnityErrorKind::InvalidMetadata,
            "animationTypeが数値でも文字列でもない",
        )
        .with_field(FIELD)),
    }
}

/// `humanDescription.human` の項目から任意欄の文字列を読む。
///
/// 欠落・nullは空として扱い、呼び出し側が無視する。
/// 文字列以外の形状は [`UnityErrorKind::InvalidMetadata`] を返す。
fn read_optional_text(entry: &Mapping, key: &str, field: &str) -> Result<String, UnityError> {
    match entry.get(key) {
        None | Some(Value::Null) => Ok(String::new()),
        Some(Value::String(text)) => Ok(text.clone()),
        Some(_) => Err(UnityError::new(
            UnityErrorKind::InvalidMetadata,
            format!("humanDescription.humanの項目に{key}が文字列でない"),
        )
        .with_field(format!("{field}.{key}"))),
    }
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
