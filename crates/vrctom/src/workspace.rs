//! 変換用の一時作業領域。
//!
//! 公開するのは汎用ファイルシステムではなく、作業単位ごとの隔離と後始末までを
//! 含めた一時置き場という小さな具体型である。保存先の種別やバックエンド依存の
//! 型は公開契約に含めない。
//!
//! 使い方は [`crate::workspace::Workspace`] の生成から始まり、操作の完了後に
//! [`crate::workspace::Workspace::close`] で片付ける。後始末の必須経路は `close` であり、強制終了時の削除完了は保証しない。

#[cfg(not(target_arch = "wasm32"))]
mod native;

#[cfg(not(target_arch = "wasm32"))]
pub use native::Workspace;

/// 作業領域内のエントリ。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// 領域ルートからの相対パス。区切りは `/`。
    pub path: String,
    /// ファイルとディレクトリの種別。
    pub kind: EntryKind,
}

/// 作業領域内のエントリ種別。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    /// ファイル。
    File,
    /// ディレクトリ。
    Directory,
}

/// 作業領域の操作失敗エラー。
///
/// 原因の診断情報は保持するが、メッセージ文言や `source` の具体型は契約にしない。
#[derive(Debug)]
pub struct WorkspaceError {
    kind: WorkspaceErrorKind,
    message: String,
    source: Option<Box<dyn std::error::Error + Send + Sync>>,
}

// バックエンド実装が使う共有契約の生成子を含むため、
// Web 実装（`workspace/web.rs`）でも再利用する。
// `Workspace` を持たないターゲットでは未使用になる。
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
impl WorkspaceError {
    /// エラーの種別を返す。
    pub fn kind(&self) -> WorkspaceErrorKind {
        self.kind
    }

    pub(crate) fn invalid_path(message: impl Into<String>) -> Self {
        Self {
            kind: WorkspaceErrorKind::InvalidPath,
            message: message.into(),
            source: None,
        }
    }

    pub(crate) fn not_found(message: impl Into<String>) -> Self {
        Self {
            kind: WorkspaceErrorKind::NotFound,
            message: message.into(),
            source: None,
        }
    }

    pub(crate) fn wrong_entry_type(message: impl Into<String>) -> Self {
        Self {
            kind: WorkspaceErrorKind::WrongEntryType,
            message: message.into(),
            source: None,
        }
    }

    pub(crate) fn closed(message: impl Into<String>) -> Self {
        Self {
            kind: WorkspaceErrorKind::Closed,
            message: message.into(),
            source: None,
        }
    }

    pub(crate) fn storage(message: impl Into<String>) -> Self {
        Self {
            kind: WorkspaceErrorKind::Storage,
            message: message.into(),
            source: None,
        }
    }

    pub(crate) fn storage_error(source: std::io::Error, context: impl Into<String>) -> Self {
        Self {
            kind: WorkspaceErrorKind::Storage,
            message: context.into(),
            source: Some(Box::new(source)),
        }
    }
}

impl std::fmt::Display for WorkspaceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.message)
    }
}

impl std::error::Error for WorkspaceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_ref()
            .map(|source| source.as_ref() as &(dyn std::error::Error + 'static))
    }
}

/// 作業領域の操作失敗エラーの種別。
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceErrorKind {
    /// パスが契約に違反する。ストレージ操作の開始前に返す。
    InvalidPath,
    /// 対象または一覧対象のディレクトリが存在しない。
    NotFound,
    /// ファイルとディレクトリの種別が操作と合わない。
    WrongEntryType,
    /// `close` 開始後、または `close` 完了後に操作した。
    Closed,
    /// 権限・容量不足などの I/O 失敗。
    Storage,
}

/// Windows予約名の検証表。
///
/// OS の `Path::components` だけに任せず、全ターゲットで同じ判定にする。
/// 拡張子より前（最初の `.` より前）の部分を ASCII 大文字小文字を区別せずに照合する。
// バックエンド間で共有する検証表で、Web 実装でも再利用する。
// `Workspace` を持たないターゲットでは未使用になる。
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
const RESERVED_FILE_STEMS: &[&str] = &[
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
    // 上付き数字の予約名（`COM¹` なども Windows の予約名として扱う）。
    "com¹", "com²", "com³", "lpt¹", "lpt²", "lpt³",
];

/// 作業領域内の相対パスを検証する。
///
/// 正規化や補正はしない。不正な入力はストレージ操作の開始前に
/// [`WorkspaceErrorKind::InvalidPath`] で拒否する。
/// `allow_empty` が真の場合だけ、空文字列 `""` をルート指定として受け付ける。
// バックエンド間で共有する検証で、Web 実装でも再利用する。
// `Workspace` を持たないターゲットでは未使用になる。
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) fn validate_path(path: &str, allow_empty: bool) -> Result<(), WorkspaceError> {
    if path.is_empty() {
        if allow_empty {
            return Ok(());
        }
        return Err(WorkspaceError::invalid_path("空のパスは指定できない"));
    }
    // `\` は全ターゲットで区切りや有効文字として扱わない。
    if path.contains('\\') {
        return Err(WorkspaceError::invalid_path(
            "パスに `\\` は使えない。区切りは `/` のみ",
        ));
    }
    for component in path.split('/') {
        validate_component(component)?;
    }
    Ok(())
}

/// `/` で区切った1成分を検証する。
fn validate_component(component: &str) -> Result<(), WorkspaceError> {
    // 空成分は先頭 `/`（絶対パス）・末尾 `/`・連続 `/` のいずれかを意味する。
    if component.is_empty() {
        return Err(WorkspaceError::invalid_path(
            "空の成分を含むパスは指定できない",
        ));
    }
    if component == "." || component == ".." {
        return Err(WorkspaceError::invalid_path(
            "`.` や `..` を含むパスは指定できない",
        ));
    }
    if component.chars().any(|c| c.is_control()) {
        return Err(WorkspaceError::invalid_path(
            "制御文字を含むパスは指定できない",
        ));
    }
    if component
        .chars()
        .any(|c| matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*'))
    {
        return Err(WorkspaceError::invalid_path(
            "Windowsで使えない文字を含むパスは指定できない",
        ));
    }
    // 拡張子より前を予約名と照合する（`AUX.txt` なども拒否する）。
    let stem = component.split('.').next().unwrap_or_default();
    if RESERVED_FILE_STEMS
        .iter()
        .any(|reserved| stem.eq_ignore_ascii_case(reserved))
    {
        return Err(WorkspaceError::invalid_path(
            "Windowsの予約名を含むパスは指定できない",
        ));
    }
    // 末尾の空白やドットは全ターゲットで拒否する。
    if component.ends_with(' ') || component.ends_with('.') {
        return Err(WorkspaceError::invalid_path(
            "末尾が空白やドットの成分を含むパスは指定できない",
        ));
    }
    Ok(())
}
