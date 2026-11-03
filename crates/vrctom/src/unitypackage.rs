//! UnityPackageの展開。
//!
//! 入口は [`crate::unitypackage::unpack_to()`]（nativeの出力先展開）と
//! [`crate::unitypackage::unpack_in_memory()`]（ターゲット非依存のメモリ内保持）
//! で、gzip圧縮tarを読むだけの展開処理を提供する。
//! 展開計画の検証を終えるまで出力先を作らず、出力開始後の失敗では自分が
//! 作った出力先を削除して残存の有無をエラーに添える。ディレクトリ全体の
//! 不可分な公開は保証しない。
//! メモリ内保持も、全件の検証が終わるまで成功したパッケージとして公開しない。
//!
//! 展開に使う一時領域と最終出力先は呼び出し側が指定した場所の責務であり、
//! [`crate::workspace`] の一時作業領域は使わない。詳細は設計文書を参照。

use std::path::{Path, PathBuf};

mod core;
#[cfg(not(target_arch = "wasm32"))]
mod native;

pub use core::{MemoryPackage, unpack_in_memory};
#[cfg(not(target_arch = "wasm32"))]
pub use native::unpack_to;

/// tarエントリ数の既定上限。
///
/// 通常のUnityPackage（千件規模）に十分な余裕を持たせた仕様値。
const DEFAULT_MAX_ENTRIES: u64 = 100_000;
/// gzip展開後の総読み取り量の既定上限（バイト）。
///
/// ヘッダや読み捨て分も含めた仕様値。
const DEFAULT_MAX_TOTAL_BYTES: u64 = 8 * 1024 * 1024 * 1024;
/// `pathname`一件の既定上限（バイト）。
///
/// 通常のUnityパス（数百バイト）に十分な余裕を持たせた仕様値。
const DEFAULT_MAX_PATHNAME_BYTES: usize = 4 * 1024;
/// `pathname`合計の既定上限（バイト）。
///
/// GUID数に比例する管理情報だけをメモリに載せるための仕様値。
const DEFAULT_MAX_PATHNAME_TOTAL_BYTES: u64 = 16 * 1024 * 1024;

/// `unpack_to` の資源上限。
///
/// フィールドは非公開で、既定値は [`Default`] が設定する。
/// 上限の変更は `with_` で始まるメソッドで行い、CLIは既定値をそのまま使う。
#[derive(Debug, Clone)]
pub struct UnpackOptions {
    pub(crate) max_entries: u64,
    pub(crate) max_total_bytes: u64,
    pub(crate) max_pathname_bytes: usize,
    pub(crate) max_pathname_total_bytes: u64,
    // 以下の故障注入は単体テストでのみ使い、通常ビルドには存在しない。
    // `workspace` の後始末注入と同様に、呼び出し単位で一度だけ失敗させる。
    /// テスト用：何番目の出力ファイル書き込みで失敗させるか（0始まり）。
    #[cfg(test)]
    pub(crate) inject_file_error_at: Option<usize>,
    /// テスト用：出力先の削除を失敗させるか。
    #[cfg(test)]
    pub(crate) inject_cleanup_error: bool,
    /// テスト用：一時領域の削除を失敗させるか。
    #[cfg(test)]
    pub(crate) inject_staging_cleanup_error: bool,
}

impl UnpackOptions {
    /// 既定の上限を持つ設定を生成する。
    pub fn new() -> Self {
        Self {
            max_entries: DEFAULT_MAX_ENTRIES,
            max_total_bytes: DEFAULT_MAX_TOTAL_BYTES,
            max_pathname_bytes: DEFAULT_MAX_PATHNAME_BYTES,
            max_pathname_total_bytes: DEFAULT_MAX_PATHNAME_TOTAL_BYTES,
            #[cfg(test)]
            inject_file_error_at: None,
            #[cfg(test)]
            inject_cleanup_error: false,
            #[cfg(test)]
            inject_staging_cleanup_error: false,
        }
    }

    /// tarエントリ数の上限を変更する。
    pub fn with_max_entries(mut self, max_entries: u64) -> Self {
        self.max_entries = max_entries;
        self
    }

    /// gzip展開後の総読み取り量の上限（バイト）を変更する。
    pub fn with_max_total_bytes(mut self, max_total_bytes: u64) -> Self {
        self.max_total_bytes = max_total_bytes;
        self
    }

    /// `pathname`一件の上限（バイト）を変更する。
    pub fn with_max_pathname_bytes(mut self, max_pathname_bytes: usize) -> Self {
        self.max_pathname_bytes = max_pathname_bytes;
        self
    }

    /// `pathname`合計の上限（バイト）を変更する。
    pub fn with_max_pathname_total_bytes(mut self, max_pathname_total_bytes: u64) -> Self {
        self.max_pathname_total_bytes = max_pathname_total_bytes;
        self
    }
}

impl Default for UnpackOptions {
    fn default() -> Self {
        Self::new()
    }
}

/// 診断表示のための可視エスケープはクレート共通で持つ。
///
/// `unity` でも表示時の逃がしに使うため、配置は [`crate::escape_diag`] に寄せる。
pub(crate) use crate::escape_diag;

/// 展開結果の一覧と集計。
///
/// 一覧は `pathname` のUTF-8バイト列順に並べる。
/// バイト列などの内容は保持せず、一覧と集計だけを返す。
#[derive(Debug, Clone)]
pub struct UnpackReport {
    entries: Vec<Entry>,
    file_count: usize,
    directory_count: usize,
    output_bytes: u64,
}

impl UnpackReport {
    /// 展開したエントリの一覧を返す。
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// アセット（ファイル）の件数を返す。
    pub fn file_count(&self) -> usize {
        self.file_count
    }

    /// フォルダの件数を返す。
    pub fn directory_count(&self) -> usize {
        self.directory_count
    }

    /// 出力したファイルの合計バイト数を返す。
    pub fn output_bytes(&self) -> u64 {
        self.output_bytes
    }
}

/// 展開した1件の対応関係。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// 小文字に統一した32桁ASCII hexのGUID。
    pub guid: String,
    /// 出力先からの相対パス。区切りは `/`。
    pub pathname: String,
    /// ファイルとディレクトリの種別。
    pub kind: EntryKind,
}

/// 展開エントリの種別。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    /// `asset` を持つ通常のファイル。
    File,
    /// `asset` を持たないフォルダ。
    Directory,
}

/// UnityPackage展開の失敗エラー。
///
/// 原因の診断情報は保持するが、メッセージ文言や `source` の具体型は契約にしない。
/// 外部入力を含む診断文は表示用にエスケープする。
/// 詳細部はヒープに置き、`Result` の `Err` 側を小さく保つ。
#[derive(Debug)]
pub struct UnpackError {
    kind: UnpackErrorKind,
    message: Box<str>,
    guid: Option<Box<str>>,
    pathname: Option<Box<str>>,
    // 出力後始末の印と残存報告はnativeの契約であり、対象外ターゲットでは未使用になり得る。
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    output_created: bool,
    output_path: Option<Box<Path>>,
    temp_path: Option<Box<Path>>,
    source: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl UnpackError {
    pub(crate) fn new(kind: UnpackErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into().into_boxed_str(),
            guid: None,
            pathname: None,
            output_created: false,
            output_path: None,
            temp_path: None,
            source: None,
        }
    }

    pub(crate) fn with_guid(mut self, guid: impl Into<String>) -> Self {
        self.guid = Some(guid.into().into_boxed_str());
        self
    }

    pub(crate) fn with_pathname(mut self, pathname: impl Into<String>) -> Self {
        self.pathname = Some(pathname.into().into_boxed_str());
        self
    }

    pub(crate) fn with_source(
        mut self,
        source: impl Into<Box<dyn std::error::Error + Send + Sync>>,
    ) -> Self {
        self.source = Some(source.into());
        self
    }

    // 出力後始末の印と残存報告はnativeの契約であり、対象外ターゲットでは未使用になり得る。
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) fn with_output_created(mut self) -> Self {
        self.output_created = true;
        self
    }

    // 出力後始末の印と残存報告はnativeの契約であり、対象外ターゲットでは未使用になり得る。
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) fn take_output_created(&mut self) -> bool {
        std::mem::replace(&mut self.output_created, false)
    }

    // 出力後始末の印と残存報告はnativeの契約であり、対象外ターゲットでは未使用になり得る。
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) fn set_partial_output(&mut self, path: PathBuf) {
        self.output_path = Some(path.into_boxed_path());
    }

    // 出力後始末の印と残存報告はnativeの契約であり、対象外ターゲットでは未使用になり得る。
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) fn set_partial_temp(&mut self, path: PathBuf) {
        self.temp_path = Some(path.into_boxed_path());
    }

    /// 失敗の後始末自体が失敗した場合に、主原因の文面へ併記する。
    // 出力後始末の印と残存報告はnativeの契約であり、対象外ターゲットでは未使用になり得る。
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) fn append_cleanup(&mut self, detail: impl Into<String>) {
        let mut message = self.message.to_string();
        message.push_str("; ");
        message.push_str(&detail.into());
        self.message = message.into_boxed_str();
    }

    /// エラーの種別を返す。
    pub fn kind(&self) -> UnpackErrorKind {
        self.kind
    }

    /// 判明している範囲で、対象のGUIDを返す。
    pub fn guid(&self) -> Option<&str> {
        self.guid.as_deref()
    }

    /// 判明している範囲で、対象の `pathname` を返す。
    pub fn pathname(&self) -> Option<&str> {
        self.pathname.as_deref()
    }

    /// 残存し得る出力先を返す。
    ///
    /// 出力開始前の失敗や削除成功の場合は `None` を返す。
    pub fn partial_output(&self) -> Option<&Path> {
        self.output_path.as_deref()
    }

    /// 残存し得る一時領域を返す。
    ///
    /// 削除成功の場合は `None` を返す。
    pub fn partial_temp_dir(&self) -> Option<&Path> {
        self.temp_path.as_deref()
    }
}

impl std::fmt::Display for UnpackError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 文面は生成時に逃がし済みだが、表示時にも逃がして未逃がしの混入を防ぐ。
        // 逃がし済みの文面への再適用は変えない。
        write!(f, "{:?}: {}", self.kind, escape_diag(&self.message))
    }
}

impl std::error::Error for UnpackError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_ref()
            .map(|source| source.as_ref() as &(dyn std::error::Error + 'static))
    }
}

/// UnityPackage展開の失敗エラーの種別。
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnpackErrorKind {
    /// 呼び出し側リーダーの読み取りに失敗する場合。
    Input,
    /// tar構造や対応関係が壊れている場合。
    InvalidPackage,
    /// 出力パスが契約に違反する場合。
    InvalidPath,
    /// 出力計画が出力先や計画同士で衝突する場合。
    Conflict,
    /// 資源上限を超える場合。
    LimitExceeded,
    /// 一時領域や出力先の I/O に失敗する場合。
    Storage,
}
