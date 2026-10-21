//! Unity保存データ読み取りの失敗エラー。
//!
//! 公開契約の入口は [`crate::unity`] を参照。
//!
//! 原因の診断情報は保持するが、メッセージ文言や `source` の具体型は契約にしない。
//! 外部由来の文字列を含む診断は表示時に可視エスケープする。逃がし済みの文面への
//! 再適用は変えない。

use std::path::Path;

/// Unity保存データ読み取りの失敗エラー。
///
/// 診断の素（パス・fileID・classID・フィールド・行位置）は保持し、表示だけが
/// 逃がす。詳細部はヒープに置き、`Result` の `Err` 側を小さく保つ。
#[derive(Debug)]
pub struct UnityError {
    kind: UnityErrorKind,
    message: Box<str>,
    path: Option<Box<str>>,
    file_id: Option<i64>,
    class_id: Option<i32>,
    field: Option<Box<str>>,
    line: Option<u64>,
    source: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl UnityError {
    pub(crate) fn new(kind: UnityErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into().into_boxed_str(),
            path: None,
            file_id: None,
            class_id: None,
            field: None,
            line: None,
            source: None,
        }
    }

    pub(crate) fn with_path(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into().into_boxed_str());
        self
    }

    pub(crate) fn with_file_id(mut self, file_id: i64) -> Self {
        self.file_id = Some(file_id);
        self
    }

    pub(crate) fn with_class_id(mut self, class_id: i32) -> Self {
        self.class_id = Some(class_id);
        self
    }

    pub(crate) fn with_field(mut self, field: impl Into<String>) -> Self {
        self.field = Some(field.into().into_boxed_str());
        self
    }

    pub(crate) fn with_line(mut self, line: u64) -> Self {
        self.line = Some(line);
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
    pub fn kind(&self) -> UnityErrorKind {
        self.kind
    }

    /// 判明している範囲で、対象のパスを返す。
    pub fn path(&self) -> Option<&str> {
        self.path.as_deref()
    }

    /// 判明している範囲で、対象のfileIDを返す。
    pub fn file_id(&self) -> Option<i64> {
        self.file_id
    }

    /// 判明している範囲で、対象のclassIDを返す。
    pub fn class_id(&self) -> Option<i32> {
        self.class_id
    }

    /// 判明している範囲で、対象のフィールド名を返す。
    pub fn field(&self) -> Option<&str> {
        self.field.as_deref()
    }

    /// 判明している範囲で、対象の行位置（1始まり）を返す。
    pub fn line(&self) -> Option<u64> {
        self.line
    }
}

impl std::fmt::Display for UnityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 文面は生成時に逃がし済みだが、表示時にも逃がして未逃がしの混入を防ぐ。
        // 逃がし済みの文面への再適用は変えない。
        write!(f, "{:?}: {}", self.kind, crate::escape_diag(&self.message))?;
        // 診断の素は表示時に逃がす。付随情報の有無で主文面は変えない。
        if let Some(path) = &self.path {
            write!(f, " [path: {}]", crate::escape_diag(path))?;
        }
        if let Some(file_id) = self.file_id {
            write!(f, " [fileID: {file_id}]")?;
        }
        if let Some(class_id) = self.class_id {
            write!(f, " [classID: {class_id}]")?;
        }
        if let Some(field) = &self.field {
            write!(f, " [field: {}]", crate::escape_diag(field))?;
        }
        if let Some(line) = self.line {
            write!(f, " [line: {line}]")?;
        }
        Ok(())
    }
}

impl std::error::Error for UnityError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_ref()
            .map(|source| source.as_ref() as &(dyn std::error::Error + 'static))
    }
}

/// Unity保存データ読み取りの失敗エラーの種別。
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnityErrorKind {
    /// パスが契約に違反する場合（作業領域外の指定や symlink の追従拒否など）。
    InvalidPath,
    /// `.meta` の内容が壊れている場合（GUIDの形式不良や復元不能な数値化など）。
    InvalidMetadata,
    /// Unity YAMLの枠や構文が壊れている場合。
    InvalidYaml,
    /// 参照の形状が壊れている場合。
    InvalidReference,
    /// GUID・パス・fileIDの重複など、後勝ちにしない衝突の場合。
    Conflict,
    /// 対応する実体のないmetaやmetaのない通常ファイルの場合。
    MissingMetadata,
    /// 参照先の所在が索引にない場合。
    MissingObject,
    /// 既知クラスとclassIDの不一致や型付き読み取りの形状違いの場合。
    TypeMismatch,
    /// 親子対応の破損や循環など、階層として成立しない場合。
    InvalidHierarchy,
    /// `stripped` や `PrefabInstance` を含むため部分木を返さない場合。
    UnsupportedPrefabComposition,
    /// Unity YAMLでも `.meta` でもない入力の場合。
    UnsupportedFormat,
    /// 資源上限を超える場合。
    LimitExceeded,
    /// 入出力に失敗する場合。
    Storage,
}

/// IO失敗を [`UnityErrorKind::Storage`] に包む。
// Native走査だけで使う共有契約のため、対象外ターゲットでは未使用になり得る。
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) fn storage_failure(error: std::io::Error, context: String) -> UnityError {
    UnityError::new(
        UnityErrorKind::Storage,
        format!("{context}: {}", crate::escape_diag(&error.to_string())),
    )
    .with_source(error)
}

/// IO失敗をパス付きで [`UnityErrorKind::Storage`] に包む。
// Native走査だけで使う共有契約のため、対象外ターゲットでは未使用になり得る。
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) fn storage_failure_with_path(
    error: std::io::Error,
    context: String,
    path: &Path,
) -> UnityError {
    storage_failure(error, context).with_path(path.to_string_lossy().into_owned())
}
