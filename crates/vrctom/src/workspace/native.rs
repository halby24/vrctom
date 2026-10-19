//! 一時作業領域の Native 実装（`tempfile::TempDir` + `std::fs`）。
//!
//! 公開契約の入口は [`crate::workspace`] を参照。

use std::fs;
use std::path::{Path, PathBuf};

use tempfile::TempDir;

use super::{Entry, EntryKind, WorkspaceError, validate_path};

/// 変換用の一時作業領域。
///
/// 生成は同期で遅延初期化し、実ストレージは最初の操作時に確保する。
/// 全操作は `&mut self` で直列化するため、複製や並行アクセスは提供しない。
#[derive(Debug)]
pub struct Workspace {
    lifecycle: Lifecycle,
    temp: Option<TempDir>,
    /// テスト時のみ使う後始末の故障注入。一度だけ `close` を失敗させる。
    /// `Some` の場合は指定した種別の I/O エラーで削除を失敗させる。
    #[cfg(test)]
    inject_cleanup_failure: Option<std::io::ErrorKind>,
    /// テスト時のみ使う存在確認の故障注入。一度だけ `symlink_metadata` を
    /// `PermissionDenied` で失敗させる。削除失敗と組み合わせて、
    /// 削除未完了かつ確認不能の場合を再現する。
    #[cfg(test)]
    inject_metadata_failure: bool,
}

/// 作業領域のライフサイクル。
///
/// `Closing` は `close` 開始後に通常操作へ戻さないための状態で、
/// 後始末の失敗後も維持する（再 `close` のみ受け付ける）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Lifecycle {
    /// 未初期化または使用可能。
    Active,
    /// `close` 開始済み（失敗後を含む）。
    Closing,
    /// `close` 完了。
    Closed,
}

// 公開契約が `async` 操作を要求するため `async fn` とする。
// Native は poll したスレッドで同期 I/O を行い、完了前に yield しない。
// そのため Future を poll せずに drop した場合は何も起きず、
// poll した場合は中断点なく最後まで進む。
// `await` を含めないのは待機点が無いことが契約上の前提のためで、
// 将来ブロッキング箇所を非同期化する際はこの属性と共に見直す。
#[allow(clippy::unused_async)]
impl Workspace {
    /// 未初期化の一時作業領域を生成する。
    ///
    /// 実ストレージは最初の操作時に確保する。
    pub fn new() -> Self {
        Self {
            lifecycle: Lifecycle::Active,
            temp: None,
            #[cfg(test)]
            inject_cleanup_failure: None,
            #[cfg(test)]
            inject_metadata_failure: false,
        }
    }

    /// ファイル全体を読み出す。
    ///
    /// # Errors
    ///
    /// パスが契約に違反する場合は `InvalidPath`、
    /// 対象が存在しない場合は `NotFound`、
    /// 対象がディレクトリの場合は `WrongEntryType` のエラーを返す。
    /// `close` 開始後は `Closed` のエラーを返す。
    /// 権限などの I/O 失敗は `Storage` のエラーを返す。
    pub async fn read(&mut self, path: &str) -> Result<Vec<u8>, WorkspaceError> {
        self.read_sync(path)
    }

    /// ファイル全体を置換する。既存内容は切り詰め、必要な親ディレクトリを作成する。
    ///
    /// 空データも有効で、追記・部分更新・seek は提供しない。
    /// 失敗時は部分書き込みや作成済みの親が残る場合がある。
    ///
    /// # Errors
    ///
    /// `read` と同様の条件に加え、対象や途中の親がディレクトリの場合は
    /// `WrongEntryType` のエラーを返す。
    pub async fn write(&mut self, path: &str, bytes: &[u8]) -> Result<(), WorkspaceError> {
        self.write_sync(path, bytes)
    }

    /// ファイルまたはディレクトリ以下を再帰削除する。
    ///
    /// 存在しない対象は成功扱いとし、削除対象の親ディレクトリは残す。
    /// 失敗時は部分削除や作成済みの親が残る場合がある。
    ///
    /// # Errors
    ///
    /// パスが契約に違反する場合は `InvalidPath`、
    /// 途中の親がファイルの場合は `WrongEntryType` のエラーを返す。
    /// `close` 開始後は `Closed` のエラーを返す。
    pub async fn delete(&mut self, path: &str) -> Result<(), WorkspaceError> {
        self.delete_sync(path)
    }

    /// 指定ディレクトリの直下のみを返す。
    ///
    /// 空文字列 `""` はルート指定として受け付ける。
    /// エントリは領域ルート相対パスと種別のみを持ち、相対パスの UTF-8 バイト列で
    /// 昇順に並べる。ルート自身は含めない。
    ///
    /// # Errors
    ///
    /// 対象が存在しない場合は `NotFound`、
    /// 対象がファイルの場合は `WrongEntryType` のエラーを返す。
    pub async fn list(&mut self, directory: &str) -> Result<Vec<Entry>, WorkspaceError> {
        self.list_sync(directory)
    }

    /// 領域と内容の削除を完了する。
    ///
    /// `close` 開始後は通常操作を `Closed` で拒否する。
    /// 成功後の再 `close` は成功する。未使用領域の `close` はストレージを
    /// 新規作成せずに成功する。失敗後は通常操作へ戻さず、再 `close` が可能。
    ///
    /// # Errors
    ///
    /// 削除に失敗した場合は `Storage` のエラーを返す。
    pub async fn close(&mut self) -> Result<(), WorkspaceError> {
        self.close_sync()
    }
}

impl Workspace {
    fn read_sync(&mut self, path: &str) -> Result<Vec<u8>, WorkspaceError> {
        validate_path(path, false)?;
        let root = self.ensure_root()?;
        let target = self.resolve_with_strict_parents(&root, path)?;
        let Some(target) = target else {
            return Err(WorkspaceError::not_found(format!(
                "存在しないファイルは読めない: {path}"
            )));
        };
        let metadata = fs::symlink_metadata(&target).map_err(|e| map_io(e, path))?;
        // シンボリックリンクは作成せず、検出してもたどらない。
        if metadata.file_type().is_symlink() || metadata.is_dir() {
            return Err(WorkspaceError::wrong_entry_type(format!(
                "ファイルではないため読めない: {path}"
            )));
        }
        fs::read(&target).map_err(|e| map_io(e, path))
    }

    fn write_sync(&mut self, path: &str, bytes: &[u8]) -> Result<(), WorkspaceError> {
        validate_path(path, false)?;
        let root = self.ensure_root()?;
        let target = self.resolve_with_created_parents(&root, path)?;
        match fs::symlink_metadata(&target) {
            Ok(metadata) => {
                // ファイルとディレクトリを自動で相互置換しない。
                // シンボリックリンクも置換せず、たどらない。
                if !metadata.is_file() || metadata.file_type().is_symlink() {
                    return Err(WorkspaceError::wrong_entry_type(format!(
                        "ディレクトリ等には書き込めない: {path}"
                    )));
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(map_io(e, path)),
        }
        // `fs::write` は既存内容を切り詰める。非 atomic は契約として許容する。
        fs::write(&target, bytes).map_err(|e| map_io(e, path))
    }

    fn delete_sync(&mut self, path: &str) -> Result<(), WorkspaceError> {
        validate_path(path, false)?;
        let root = self.ensure_root()?;
        let target = match self.resolve_with_strict_parents(&root, path)? {
            Some(target) => target,
            // 不存在は成功扱いにする。
            None => return Ok(()),
        };
        let metadata = match fs::symlink_metadata(&target) {
            Ok(metadata) => metadata,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(map_io(e, path)),
        };
        if metadata.is_dir() && !metadata.file_type().is_symlink() {
            match fs::remove_dir_all(&target) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(map_io(e, path)),
            }
        } else {
            // ファイルとシンボリックリンクはリンク自体を削除し、たどらない。
            match fs::remove_file(&target) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(map_io(e, path)),
            }
        }
    }

    fn list_sync(&mut self, directory: &str) -> Result<Vec<Entry>, WorkspaceError> {
        validate_path(directory, true)?;
        let root = self.ensure_root()?;
        let target = if directory.is_empty() {
            root.clone()
        } else {
            match self.resolve_with_strict_parents(&root, directory)? {
                Some(target) => target,
                None => {
                    return Err(WorkspaceError::not_found(format!(
                        "存在しないディレクトリは一覧できない: {directory}"
                    )));
                }
            }
        };
        let metadata = fs::symlink_metadata(&target).map_err(|e| map_io(e, directory))?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(WorkspaceError::wrong_entry_type(format!(
                "ディレクトリではないため一覧できない: {directory}"
            )));
        }
        let mut entries = Vec::new();
        for entry in fs::read_dir(&target).map_err(|e| map_io(e, directory))? {
            let entry = entry.map_err(|e| WorkspaceError::storage_error(e, "一覧の取得に失敗"))?;
            let file_type = entry
                .file_type()
                .map_err(|e| WorkspaceError::storage_error(e, "一覧の取得に失敗"))?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                return Err(WorkspaceError::storage(
                    "UTF-8 で表せない名前があるため一覧できない",
                ));
            };
            // シンボリックリンクはたどらないため、種別はリンク自体で判定する。
            let kind = if file_type.is_dir() {
                EntryKind::Directory
            } else {
                EntryKind::File
            };
            let path = if directory.is_empty() {
                name.to_owned()
            } else {
                format!("{directory}/{name}")
            };
            entries.push(Entry { path, kind });
        }
        // `String` の順序は UTF-8 バイト列の辞書式順序と一致する。
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(entries)
    }

    fn close_sync(&mut self) -> Result<(), WorkspaceError> {
        if self.lifecycle == Lifecycle::Closed {
            return Ok(());
        }
        // `close` 開始後は通常操作へ戻さない。失敗してもこの状態を維持する。
        self.lifecycle = Lifecycle::Closing;
        let Some(dir) = self.temp.take() else {
            // 未使用領域の `close` はストレージを新規作成せずに成功する。
            self.lifecycle = Lifecycle::Closed;
            return Ok(());
        };
        let path = dir.path().to_path_buf();
        // `TempDir::close()` は失敗時に所有権を失うため使わない。
        // 削除対象のパスを保持したまま削除し、失敗時は
        // `TempDir` を持ち続けて再 `close` 可能にする。
        if let Err(error) = self.remove_tree(&path) {
            match self.metadata_after_remove(&path) {
                // `Path::exists` は不存在と確認失敗（権限不足等）のどちらでも
                // 偽になるため、削除完了の判定には使わない。
                // `NotFound` の場合だけ削除完了とみなし、それ以外は
                // 未削除または確認不能として所有情報を保持し、再試行に備える。
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                _ => {
                    self.temp = Some(dir);
                    // 未削除／確認不能の分岐では削除エラーの種別に依らず
                    // `Storage` とする。削除エラーが `NotFound` の場合も
                    // 削除完了を確認できていないため `map_io` を通さない。
                    return Err(WorkspaceError::storage_error(error, "領域の削除に失敗"));
                }
            }
        }
        // `remove_dir_all` の成功後または削除対象の不存在が確定した後は、
        // Drop 時の再削除が走らないよう後始末を無効化して所有権を解放する。
        let mut dir = dir;
        dir.disable_cleanup(true);
        drop(dir);
        self.lifecycle = Lifecycle::Closed;
        Ok(())
    }

    /// 後始末の削除。テスト時は故障注入で失敗させ、通常の失敗経路を通す。
    ///
    /// 注入は `temp` の取り出し後に効かせることで、失敗時の所有情報の
    /// 復元と再 `close` を通常経路で検証できるようにする。
    fn remove_tree(&mut self, path: &Path) -> std::io::Result<()> {
        #[cfg(test)]
        if let Some(kind) = self.inject_cleanup_failure.take() {
            // 故障注入は一度だけ失敗させ、再 `close` では通常経路に戻す。
            return Err(std::io::Error::new(
                kind,
                "後始末に失敗（テストの故障注入）",
            ));
        }
        fs::remove_dir_all(path)
    }

    /// 削除後の存在確認。テスト時は故障注入で一度だけ失敗させる。
    fn metadata_after_remove(&mut self, path: &Path) -> std::io::Result<fs::Metadata> {
        #[cfg(test)]
        if self.inject_metadata_failure {
            // 故障注入は一度だけ失敗させ、再 `close` では通常経路に戻す。
            // `NotFound` 以外で失敗させることで、確認不能の場合を再現する。
            self.inject_metadata_failure = false;
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "存在確認に失敗（テストの故障注入）",
            ));
        }
        fs::symlink_metadata(path)
    }

    /// 通常操作の入口で `Closed` を拒否し、実ストレージを遅延確保する。
    fn ensure_root(&mut self) -> Result<PathBuf, WorkspaceError> {
        if self.lifecycle != Lifecycle::Active {
            return Err(WorkspaceError::closed("終了済みの領域は操作できない"));
        }
        if self.temp.is_none() {
            let dir = TempDir::with_prefix("vrctom-").map_err(|e| map_io(e, "領域の確保に失敗"))?;
            self.temp = Some(dir);
        }
        // 直前で初期化済みのため、ここでは必ず存在する。
        let Some(dir) = self.temp.as_ref() else {
            return Err(WorkspaceError::storage("領域の確保に失敗"));
        };
        Ok(dir.path().to_path_buf())
    }

    /// 中間成分がすべて実ディレクトリである場合だけ対象パスを返す。
    ///
    /// 存在しない中間成分がある場合は `None` を返し、呼び出し側が
    /// `NotFound` と成功を使い分ける。ファイルやシンボリックリンクの
    /// 中間成分は `WrongEntryType` とし、たどらない。
    fn resolve_with_strict_parents(
        &self,
        root: &Path,
        path: &str,
    ) -> Result<Option<PathBuf>, WorkspaceError> {
        let mut current = root.to_path_buf();
        let mut components = path.split('/').peekable();
        while let Some(component) = components.next() {
            if components.peek().is_none() {
                current.push(component);
                return Ok(Some(current));
            }
            current.push(component);
            match fs::symlink_metadata(&current) {
                Ok(metadata) => {
                    if !metadata.is_dir() || metadata.file_type().is_symlink() {
                        return Err(WorkspaceError::wrong_entry_type(format!(
                            "途中の親がディレクトリではない: {path}"
                        )));
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(e) => return Err(map_io(e, path)),
            }
        }
        // `validate_path` が空パスを拒否するため、ここには到達しない。
        Err(WorkspaceError::invalid_path("空のパスは指定できない"))
    }

    /// `write` 用に、存在しない中間ディレクトリを作成しながら対象パスを返す。
    ///
    /// ファイルやシンボリックリンクの中間成分は作成せず `WrongEntryType` とする。
    fn resolve_with_created_parents(
        &self,
        root: &Path,
        path: &str,
    ) -> Result<PathBuf, WorkspaceError> {
        let mut current = root.to_path_buf();
        let mut components = path.split('/').peekable();
        while let Some(component) = components.next() {
            if components.peek().is_none() {
                current.push(component);
                return Ok(current);
            }
            current.push(component);
            match fs::symlink_metadata(&current) {
                Ok(metadata) => {
                    if !metadata.is_dir() || metadata.file_type().is_symlink() {
                        return Err(WorkspaceError::wrong_entry_type(format!(
                            "途中の親がディレクトリではない: {path}"
                        )));
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    fs::create_dir(&current).map_err(|e| map_io(e, path))?;
                }
                Err(e) => return Err(map_io(e, path)),
            }
        }
        // `validate_path` が空パスを拒否するため、ここには到達しない。
        Err(WorkspaceError::invalid_path("空のパスは指定できない"))
    }
}

impl Default for Workspace {
    fn default() -> Self {
        Self::new()
    }
}

/// I/O 失敗を契約のエラー種別へ振り分ける。
fn map_io(error: std::io::Error, context: impl Into<String>) -> WorkspaceError {
    if error.kind() == std::io::ErrorKind::NotFound {
        WorkspaceError::not_found(context)
    } else {
        WorkspaceError::storage_error(error, context)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::WorkspaceErrorKind;

    /// テスト名は日本語で書く（テストは仕様・契約であるため）。

    #[test]
    fn 未使用の終了は実領域を作らずに成功する() {
        let mut workspace = Workspace::new();
        futures::executor::block_on(workspace.close()).expect("未使用の終了は成功する");
        // 遅延初期化のため、実ストレージは確保されていない。
        assert!(workspace.temp.is_none());
        // 成功後の再終了も成功する。
        futures::executor::block_on(workspace.close()).expect("再終了は成功する");
    }

    #[test]
    fn 終了成功後は実領域が存在しない() {
        let mut workspace = Workspace::new();
        futures::executor::block_on(workspace.write("a.txt", b"data")).expect("書き込みは成功する");
        // 確認用の実パスは公開 API に足さず、私有状態から取得する。
        let path = workspace
            .temp
            .as_ref()
            .expect("操作後は実領域がある")
            .path()
            .to_path_buf();
        assert!(path.exists());
        futures::executor::block_on(workspace.close()).expect("終了は成功する");
        assert!(!path.exists());
    }

    #[test]
    fn 後始末の失敗後は所有情報を保持し再試行できる() {
        let mut workspace = Workspace::new();
        futures::executor::block_on(workspace.write("a.txt", b"data")).expect("書き込みは成功する");
        let path = workspace
            .temp
            .as_ref()
            .expect("操作後は実領域がある")
            .path()
            .to_path_buf();
        // 故障注入は `temp` の取り出し後に効き、通常の失敗経路を通る。
        workspace.inject_cleanup_failure = Some(std::io::ErrorKind::Other);
        let error =
            futures::executor::block_on(workspace.close()).expect_err("注入した失敗はエラーになる");
        assert_eq!(error.kind(), WorkspaceErrorKind::Storage);
        // 故障注入が通常の失敗経路を通ったことを確かめる。
        // 取り出し前の早期復帰では `source` を持たない。
        assert!(std::error::Error::source(&error).is_some());
        // 失敗後は通常操作へ戻さず、削除対象を失わない。
        let error = futures::executor::block_on(workspace.read("a.txt"))
            .expect_err("失敗後の通常操作は拒否される");
        assert_eq!(error.kind(), WorkspaceErrorKind::Closed);
        assert!(workspace.temp.is_some());
        assert!(path.exists());
        // 再試行で終了できる。
        futures::executor::block_on(workspace.close()).expect("再終了は成功する");
        assert!(!path.exists());
    }

    #[test]
    fn 削除失敗かつ存在確認も失敗した場合は所有情報を保持し再試行できる() {
        // 削除失敗後の存在確認が `PermissionDenied` 等になる場合の回帰テスト。
        // 旧実装の `path.exists()` に戻すと、不存在と確認失敗を区別できず
        // 削除完了と誤認して成功するため、このテストは落ちる。
        // 実パスは外部で消して `path.exists()` を偽にし、存在確認だけを
        // 故障注入で確認不能にすることで、この誤認を再現する。
        // 削除エラーを `NotFound` 種別にすることで、未削除／確認不能の分岐が
        // `map_io` を通さず `Storage` を返すことも確かめる。
        let mut workspace = Workspace::new();
        futures::executor::block_on(workspace.write("a.txt", b"data")).expect("書き込みは成功する");
        let path = workspace
            .temp
            .as_ref()
            .expect("操作後は実領域がある")
            .path()
            .to_path_buf();
        // 実パスを消して旧 `path.exists()` を偽にし、存在確認の失敗は
        // 故障注入で再現する。確認不能のため `Storage` が正しい。
        fs::remove_dir_all(&path).expect("外部削除は成功する");
        workspace.inject_cleanup_failure = Some(std::io::ErrorKind::NotFound);
        workspace.inject_metadata_failure = true;
        let error = futures::executor::block_on(workspace.close())
            .expect_err("確認不能な失敗はエラーになる");
        assert_eq!(error.kind(), WorkspaceErrorKind::Storage);
        // 原因の診断情報を保持する。
        assert!(std::error::Error::source(&error).is_some());
        // 失敗後は通常操作へ戻さず、削除対象を失わない。
        let error = futures::executor::block_on(workspace.read("a.txt"))
            .expect_err("失敗後の通常操作は拒否される");
        assert_eq!(error.kind(), WorkspaceErrorKind::Closed);
        assert!(workspace.temp.is_some());
        // 故障注入は一度だけのため、再試行で終了できる。
        futures::executor::block_on(workspace.close()).expect("再終了は成功する");
        assert!(!path.exists());
    }

    #[test]
    fn 削除対象が既に無い場合の終了は成功する() {
        let mut workspace = Workspace::new();
        futures::executor::block_on(workspace.write("a.txt", b"data")).expect("書き込みは成功する");
        let path = workspace
            .temp
            .as_ref()
            .expect("操作後は実領域がある")
            .path()
            .to_path_buf();
        // 外部で削除済みの場合も削除完了とみなす。
        fs::remove_dir_all(&path).expect("外部削除は成功する");
        futures::executor::block_on(workspace.close()).expect("終了は成功する");
        futures::executor::block_on(workspace.close()).expect("再終了は成功する");
    }

    #[test]
    fn 不正パスでは実領域を確保しない() {
        let mut workspace = Workspace::new();
        // いずれも検証段階で拒否されるため、実ストレージに触れない。
        for path in [
            "CON",
            "com¹.txt",
            "LPT³/log",
            "a/../b.txt",
            "back\\slash.txt",
            "x.txt ",
            "",
        ] {
            let error = futures::executor::block_on(workspace.read(path))
                .expect_err(&format!("読み取りを拒否する: {path:?}"));
            assert_eq!(error.kind(), WorkspaceErrorKind::InvalidPath);
            let error = futures::executor::block_on(workspace.write(path, b"x"))
                .expect_err(&format!("書き込みを拒否する: {path:?}"));
            assert_eq!(error.kind(), WorkspaceErrorKind::InvalidPath);
            let error = futures::executor::block_on(workspace.delete(path))
                .expect_err(&format!("削除を拒否する: {path:?}"));
            assert_eq!(error.kind(), WorkspaceErrorKind::InvalidPath);
        }
        for path in ["CON", "com².txt", "a/../b.txt"] {
            let error = futures::executor::block_on(workspace.list(path))
                .expect_err(&format!("一覧を拒否する: {path:?}"));
            assert_eq!(error.kind(), WorkspaceErrorKind::InvalidPath);
        }
        // 不正パスのみでは遅延初期化が進まず、実領域は確保されない。
        assert!(workspace.temp.is_none());
    }

    #[test]
    fn 予約しただけの操作を取り消しても終了できる() {
        let mut workspace = Workspace::new();
        // poll せずに drop した Future は何も開始しない。
        drop(workspace.write("a.txt", b"data"));
        drop(workspace.close());
        futures::executor::block_on(workspace.close()).expect("終了は成功する");
        assert!(workspace.temp.is_none());
    }
}
