//! CLIとWebが共有する基本変換の入口。
//!
//! FBX解析から中間モデルの生成を経てVRM書き出しに至る純粋な経路だけを受け持ち、
//! レポート付きで結果を返す。ファイル配置・表示・終了コード・FBX選択は
//! 呼び出し側に残し、材質の読み取りは呼び出し側が [`crate::vrm::VrmExportOptions`] に渡す。
//! Prefab・sidecarを使う特殊経路は含めない。

use crate::fbx::{FbxError, FbxFile, FbxOptions};
use crate::vrm::{VrmError, VrmExportError, VrmExportOptions, VrmModel, VrmOptions, VrmReport};

/// 中間モデルとその件数報告の束ね。
///
/// 特殊経路（物理・割当のためにモデルを保持する呼び出し側）が
/// [`build_model`] の結果を使い回すための形である。
#[derive(Debug)]
pub struct BuiltModel {
    /// 中間モデル。
    pub model: VrmModel,
    /// 中間モデル生成の件数報告。
    pub report: VrmReport,
}

/// 基本変換の失敗エラー。
///
/// 段階ごとにバリアントを分け、表示は各層の既存エラーの表示をそのまま保つ。
/// 原因の診断情報は保持するが、メッセージ文面や `source` の具体型は契約にしない。
#[derive(Debug)]
pub enum ConversionError {
    /// FBX解析の失敗。
    Fbx(FbxError),
    /// 中間モデル生成の失敗。
    Model(VrmError),
    /// VRM書き出しの失敗。
    Export(VrmExportError),
}

impl ConversionError {
    /// 失敗した段階を返す。
    ///
    /// 呼び出し側が診断の見出し（`FBXの解析に失敗した` など）を
    /// 付け替えるための判定に使う。文言自体は呼び出し側が持つ。
    pub fn stage(&self) -> &'static str {
        match self {
            Self::Fbx(_) => "FBXの解析",
            Self::Model(_) => "中間モデルの生成",
            Self::Export(_) => "VRMの書き出し",
        }
    }
}

impl std::fmt::Display for ConversionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 各層の既存表示を壊さないため、内側の表示にそのまま委ねる。
        match self {
            Self::Fbx(error) => write!(f, "{error}"),
            Self::Model(error) => write!(f, "{error}"),
            Self::Export(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for ConversionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Fbx(error) => Some(error),
            Self::Model(error) => Some(error),
            Self::Export(error) => Some(error),
        }
    }
}

impl From<FbxError> for ConversionError {
    fn from(error: FbxError) -> Self {
        Self::Fbx(error)
    }
}

impl From<VrmError> for ConversionError {
    fn from(error: VrmError) -> Self {
        Self::Model(error)
    }
}

impl From<VrmExportError> for ConversionError {
    fn from(error: VrmExportError) -> Self {
        Self::Export(error)
    }
}

/// FBXバイト列から中間モデルと件数報告を作る。
///
/// 読み取りは一度だけ行い、入力バイト列は保持しない。
/// 材質の読み取りは呼び出し側の責務であり、ここでは扱わない。
///
/// # Errors
///
/// FBX解析に失敗した場合は [`ConversionError::Fbx`]、
/// 中間モデルの生成に失敗した場合は [`ConversionError::Model`] を返す。
pub fn build_model(
    fbx_bytes: &[u8],
    fbx_options: &FbxOptions,
    vrm_options: &VrmOptions,
) -> Result<BuiltModel, ConversionError> {
    let file = FbxFile::parse(fbx_bytes, fbx_options).map_err(ConversionError::Fbx)?;
    let (model, report) = VrmModel::from_fbx(&file, vrm_options).map_err(ConversionError::Model)?;
    Ok(BuiltModel { model, report })
}

/// 中間モデルからVRM 1.0のGLBバイト列を作る。
///
/// [`VrmModel::to_vrm`] と同じバイト列を返す。材質は `options` に含まれるものを使う。
///
/// # Errors
///
/// 書き出しに失敗した場合は [`ConversionError::Export`] を返す。
pub fn export_model(
    model: &VrmModel,
    options: &VrmExportOptions,
) -> Result<Vec<u8>, ConversionError> {
    model.to_vrm(options).map_err(ConversionError::Export)
}

/// FBXバイト列からVRMバイト列と件数報告を作る。
///
/// [`build_model`] と [`export_model`] の合成であり、読み取りは一度だけ行う。
/// 材質の読み取りは呼び出し側の責務であり、`export_options` に渡す。
///
/// # Errors
///
/// FBX解析・中間モデル・書き出しの失敗を段階ごとのバリアントで返す。
pub fn convert_fbx(
    fbx_bytes: &[u8],
    fbx_options: &FbxOptions,
    vrm_options: &VrmOptions,
    export_options: &VrmExportOptions,
) -> Result<(Vec<u8>, VrmReport), ConversionError> {
    let built = build_model(fbx_bytes, fbx_options, vrm_options)?;
    let bytes = export_model(&built.model, export_options)?;
    Ok((bytes, built.report))
}
