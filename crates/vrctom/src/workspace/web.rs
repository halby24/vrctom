//! 一時作業領域の Web（OPFS）実装。
//!
//! 公開契約の入口は [`crate::workspace`] を参照。
//!
//! 配置は OPFS ルート直下の共有名前空間 `vrctom-workspaces-v1` の下に
//! 一意な `ws-*` を作る。OPFS ルート自体は作業領域に直接使わない。
//! 共有名前空間は空でも残し、既存領域の列挙・再利用・期限削除はしない
//! （残骸回収は範囲外）。
//!
//! キャンセル保証は、操作全体を所有する Future を [`Workspace`] 内部に保持して
//! 果たす。公開 Future の drop では実処理を失わず、次回操作または `close` で
//! 完了させてから進める。Future の drop による即時中止・ロールバック、
//! drop した書込が反映されないこと、未完了 Promise がある場合の `close` の
//! 時間上限、ページ破棄・Worker 終了時の完了は保証しない。

use std::future::Future;
use std::pin::Pin;

use futures::StreamExt as _;
use opfs::persistent::{DirectoryHandle, app_specific_dir};
use opfs::{
    CreateWritableOptions, DirectoryEntry, FileSystemRemoveOptions, GetDirectoryHandleOptions,
    GetFileHandleOptions,
};
use opfs::{DirectoryHandle as _, FileHandle as _, WritableFileStream as _};
use wasm_bindgen::{JsCast as _, JsValue};

use super::{Entry, EntryKind, WorkspaceError, validate_path};
/// OPFS ルート直下の共有名前空間。空でも削除しない。
const NAMESPACE: &str = "vrctom-workspaces-v1";
/// 領域ディレクトリ名の接頭辞。
const AREA_PREFIX: &str = "ws-";
/// 書込の1回あたりの上限。巨大データを JS へ一度に渡さない。
/// スパイクで108MB往復の実績がある区切りに合わせる。
const WRITE_CHUNK_BYTES: usize = 1024 * 1024;

/// 変換用の一時作業領域。
///
/// 生成は同期で遅延初期化し、実ストレージは最初の操作時に確保する。
/// 全操作は `&mut self` で直列化するため、複製や並行アクセスは提供しない。
pub struct Workspace {
    lifecycle: Lifecycle,
    state: Option<WebState>,
    pending: Option<PendingOp>,
    /// `close` の削除タスクを保持中の場合に真。
    /// 保持中の Future が通常操作か削除かの区別に使う。
    delete_held: bool,
}

// 保持中の Future は `Debug` を実装しないため手動実装する。
// パスや内容は診断に出さない。
impl std::fmt::Debug for Workspace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Workspace")
            .field("lifecycle", &self.lifecycle)
            .field("has_state", &self.state.is_some())
            .field("has_pending", &self.pending.is_some())
            .finish()
    }
}

/// 作業領域のライフサイクル。
///
/// `Closing` は `close` 開始後に通常操作へ戻さないための状態で、
/// 後始末の失敗後も維持する（再 `close` のみ受け付ける）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Lifecycle {
    /// 未初期化または使用可能。
    Active,
    /// `close` 開始済み（失敗を含む）。
    Closing,
    /// `close` 完了。
    Closed,
}

/// 実領域の所有情報。操作全体を所有する Future と共に移動する。
#[derive(Debug, Default)]
struct WebState {
    /// 共有名前空間のハンドル。
    parent: Option<DirectoryHandle>,
    /// 所有する自領域名（`ws-` と128bit乱数hex）。`close` の削除対象。
    /// 空き確認の直後に作成 Promise を開始する場合だけ記録する。
    /// 存在確認中の候補は保持せず、未所有の候補を削除対象にしない。
    name: Option<String>,
    /// 自領域のハンドル。
    dir: Option<DirectoryHandle>,
}

impl WebState {
    /// 親・名・実体のいずれも無い場合に真。
    fn is_pristine(&self) -> bool {
        self.parent.is_none() && self.name.is_none() && self.dir.is_none()
    }
}

/// 公開操作の種別。パスと書込データを所有 Future へ移すために使う。
enum OpKind {
    Read(String),
    Write { path: String, bytes: Vec<u8> },
    Delete(String),
    List(String),
    Close,
}

/// 操作全体の結果。種別は開始した操作と対応する。
enum OpResult {
    Bytes(Vec<u8>),
    Entries(Vec<Entry>),
    Unit,
}

/// 保持中の操作全体を所有する Future。
/// `WebState`・パス・書込データを所有し、成功・失敗の両方で `WebState` を返す。
type PendingOp = Pin<Box<dyn Future<Output = (WebState, Result<OpResult, WorkspaceError>)>>>;

/// JS 例外の `name` 分類。取得失敗・文字列でない・未知は `Other` とする。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExceptionKind {
    NotFound,
    TypeMismatch,
    Other,
}

impl Workspace {
    /// 未初期化の一時作業領域を生成する。
    ///
    /// 実ストレージは最初の操作時に確保する。
    pub fn new() -> Self {
        Self {
            lifecycle: Lifecycle::Active,
            state: None,
            pending: None,
            delete_held: false,
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
        validate_path(path, false)?;
        self.check_active()?;
        match self.run_op(OpKind::Read(path.to_owned())).await? {
            OpResult::Bytes(bytes) => Ok(bytes),
            _ => Err(WorkspaceError::storage(
                "読み取りに失敗: 操作結果の種別が想定外",
            )),
        }
    }

    /// ファイル全体を置換する。既存内容は切り詰め、必要な親ディレクトリを作成する。
    ///
    /// 空データも有効で、追記・部分更新・seek は提供しない。
    /// 失敗時は部分書き込みや作成済みの親が残る場合がある。
    ///
    /// # Errors
    ///
    /// `read` と同様の条件に加え、対象がディレクトリ、または途中の親が
    /// ファイルの場合は `WrongEntryType` のエラーを返す。
    /// `close` 開始後は `Closed` のエラーを返す。
    /// 権限などの I/O 失敗は `Storage` のエラーを返す。
    pub async fn write(&mut self, path: &str, bytes: &[u8]) -> Result<(), WorkspaceError> {
        validate_path(path, false)?;
        self.check_active()?;
        match self
            .run_op(OpKind::Write {
                path: path.to_owned(),
                bytes: bytes.to_vec(),
            })
            .await?
        {
            OpResult::Unit => Ok(()),
            _ => Err(WorkspaceError::storage(
                "書き込みに失敗: 操作結果の種別が想定外",
            )),
        }
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
        validate_path(path, false)?;
        self.check_active()?;
        match self.run_op(OpKind::Delete(path.to_owned())).await? {
            OpResult::Unit => Ok(()),
            _ => Err(WorkspaceError::storage(
                "削除に失敗: 操作結果の種別が想定外",
            )),
        }
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
        validate_path(directory, true)?;
        self.check_active()?;
        match self.run_op(OpKind::List(directory.to_owned())).await? {
            OpResult::Entries(entries) => Ok(entries),
            _ => Err(WorkspaceError::storage(
                "一覧の取得に失敗: 操作結果の種別が想定外",
            )),
        }
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
        if self.lifecycle == Lifecycle::Closed {
            return Ok(());
        }
        // `close` 開始後は通常操作へ戻さない。失敗してもこの状態を維持する。
        self.lifecycle = Lifecycle::Closing;
        if self.delete_held {
            // 前回の `close` が残した削除タスクを継続する。新規には開始しない。
            let outcome = self.settle_pending().await;
            // 完了結果を受けた同じ poll 内で保持解除する。
            self.delete_held = false;
            return self.finish_close(outcome);
        }
        // 残っている通常操作を完了させてから削除する。
        // 先行の結果は破棄するが所有情報は復元し、先行の失敗だけでは削除を中止しない。
        self.settle_pending().await;
        if self.is_pristine() {
            // 未初期化で開始済みタスクも無ければ、OPFS へ触れずに完了する。
            self.lifecycle = Lifecycle::Closed;
            return Ok(());
        }
        let state = self.state.take().unwrap_or_default();
        self.pending = Some(Box::pin(run_task(state, OpKind::Close)));
        self.delete_held = true;
        let outcome = self.settle_pending().await;
        // 完了結果を受けた同じ poll 内で保持解除する。
        self.delete_held = false;
        self.finish_close(outcome)
    }
}

impl Workspace {
    /// `close` 開始後に通常操作を受け付けない。
    fn check_active(&self) -> Result<(), WorkspaceError> {
        if self.lifecycle != Lifecycle::Active {
            return Err(WorkspaceError::closed("終了済みの領域は操作できない"));
        }
        Ok(())
    }

    /// 未初期化で開始済みタスクも無い場合に真。
    /// この場合は OPFS へ触れずに `close` を完了できる。
    fn is_pristine(&self) -> bool {
        self.pending.is_none() && self.state.as_ref().is_none_or(WebState::is_pristine)
    }

    /// 保持中の先行操作があれば完了まで待つ。
    /// 完了結果を受けた同じ poll 内で状態復元と保持解除を行う。
    /// 成功・失敗いずれの結果も呼び出し側へ返す（所有情報は失わない）。
    async fn settle_pending(&mut self) -> Option<Result<OpResult, WorkspaceError>> {
        let pending = self.pending.as_mut()?;
        // `pending.take().await` は禁止：公開 Future の drop で実処理まで失われる。
        // 借りて待つことで、drop 後も実処理は保持され次回操作で継続できる。
        let (state, outcome) = pending.await;
        self.state = Some(state);
        self.pending = None;
        Some(outcome)
    }

    /// 新規操作を開始し、完了まで待つ。
    /// 先行操作は完了させてから開始する。先行の結果は破棄するが、
    /// 所有情報は復元するため削除対象を失わない。
    /// 先行の失敗を理由に新規操作の開始を見送ることはしない。
    async fn run_op(&mut self, op: OpKind) -> Result<OpResult, WorkspaceError> {
        self.settle_pending().await;
        let state = self.state.take().unwrap_or_default();
        self.pending = Some(Box::pin(run_task(state, op)));
        // 直上で格納したため必ず完了結果がある。
        let Some(outcome) = self.settle_pending().await else {
            return Err(WorkspaceError::storage(
                "操作に失敗: 開始した操作の結果が無い",
            ));
        };
        outcome
    }

    /// 削除タスクの完了結果を `close` の終了として確定する。
    /// 失敗時は `Closing` と所有情報を保持し、再 `close` に備える。
    fn finish_close(
        &mut self,
        outcome: Option<Result<OpResult, WorkspaceError>>,
    ) -> Result<(), WorkspaceError> {
        match outcome {
            Some(Ok(_)) => {
                self.lifecycle = Lifecycle::Closed;
                Ok(())
            }
            Some(Err(error)) => Err(error),
            None => Err(WorkspaceError::storage("領域の削除結果が取得できない")),
        }
    }
}

impl Default for Workspace {
    fn default() -> Self {
        Self::new()
    }
}

/// 操作全体を所有する Future の本体。
/// 成功・失敗の両方で `WebState` を返す。
async fn run_task(mut state: WebState, op: OpKind) -> (WebState, Result<OpResult, WorkspaceError>) {
    let outcome = match op {
        OpKind::Read(path) => read_task(&mut state, &path).await.map(OpResult::Bytes),
        OpKind::Write { path, bytes } => write_task(&mut state, &path, &bytes)
            .await
            .map(|()| OpResult::Unit),
        OpKind::Delete(path) => delete_task(&mut state, &path)
            .await
            .map(|()| OpResult::Unit),
        OpKind::List(directory) => list_task(&mut state, &directory)
            .await
            .map(OpResult::Entries),
        OpKind::Close => close_task(&mut state).await.map(|()| OpResult::Unit),
    };
    (state, outcome)
}

/// 実領域を遅延確保する。既に確保済みなら何もしない。
///
/// 削除対象（`state.name`）に記録するのは、空き確認の直後に作成 Promise を
/// 開始する場合だけである。存在確認中の候補はローカル変数に保持し、
/// 衝突後の再生成失敗・存在確認の失敗では所有状態を変更しない。
/// 作成開始前の drop では何も所有していないため、他者の領域を消さない。
async fn ensure_area(state: &mut WebState) -> Result<(), WorkspaceError> {
    if state.dir.is_some() {
        return Ok(());
    }
    let parent = match state.parent.clone() {
        Some(parent) => parent,
        None => {
            let root = app_specific_dir()
                .await
                .map_err(|error| js_storage("領域の確保に失敗", "OPFSルートの取得", error))?;
            let namespace = root
                .get_directory_handle_with_options(
                    NAMESPACE,
                    &GetDirectoryHandleOptions { create: true },
                )
                .await
                .map_err(|error| js_storage("領域の確保に失敗", NAMESPACE, error))?;
            state.parent = Some(namespace.clone());
            namespace
        }
    };
    // 候補は所有と分離してローカルに保持する。作成開始後に失敗した再試行では、
    // 前回の所有候補を起点に再検証する（前回の作成が実体を残した場合は引き継ぐ）。
    let mut candidate = match state.name.clone() {
        Some(owned) => owned,
        None => generate_area_name()?,
    };
    // `create:true` に排他的作成は無く、事前確認は原子的でない。
    // 衝突回避は128bit暗号学的乱数に依存する。ここでは同名の有無だけを確認する。
    loop {
        match parent
            .get_directory_handle_with_options(
                &candidate,
                &GetDirectoryHandleOptions { create: false },
            )
            .await
        {
            Ok(_) => {
                // 前回の作成試行が実体を残した場合は所有候補として引き継ぐ。
                // 未記録の候補の衝突では再生成し、失敗時は所有を変更しない。
                if state.name.as_deref() == Some(candidate.as_str()) {
                    break;
                }
                candidate = generate_area_name()?;
            }
            Err(error) => match classify(&error) {
                // 同名ファイルがあるため再生成する。
                ExceptionKind::TypeMismatch => candidate = generate_area_name()?,
                // 空き名のため作成へ進む。
                ExceptionKind::NotFound => break,
                ExceptionKind::Other => {
                    return Err(js_storage("領域の確保に失敗", &candidate, error));
                }
            },
        }
    }
    // 空き確認の直後にだけ所有（削除対象）として記録し、作成 Promise を開始する。
    // 作成開始前の drop では何も所有していないため、他者の領域を消さない。
    // 公開 Future を drop しても実処理は保持されるため、記録後の削除対象は失わない。
    state.name = Some(candidate.clone());
    let area = parent
        .get_directory_handle_with_options(&candidate, &GetDirectoryHandleOptions { create: true })
        .await
        .map_err(|error| js_storage("領域の確保に失敗", &candidate, error))?;
    state.dir = Some(area);
    Ok(())
}

/// `ws-` に続く128bit乱数の小文字hexを返す。
///
/// `crypto.getRandomValues` を使い、時刻や `Math.random` へフォールバックしない。
/// 利用不能は `Storage` とする。
fn generate_area_name() -> Result<String, WorkspaceError> {
    let global = js_sys::global();
    let crypto_value = js_sys::Reflect::get(&global, &JsValue::from_str("crypto"))
        .map_err(|_| WorkspaceError::storage("領域名の生成に失敗: cryptoを取得できない"))?;
    if crypto_value.is_undefined() || crypto_value.is_null() {
        return Err(WorkspaceError::storage(
            "領域名の生成に失敗: cryptoが利用できない",
        ));
    }
    let crypto: web_sys::Crypto = crypto_value
        .dyn_into()
        .map_err(|_| WorkspaceError::storage("領域名の生成に失敗: cryptoの型が想定外"))?;
    let mut bytes = [0u8; 16];
    crypto
        .get_random_values_with_u8_array(&mut bytes)
        .map_err(|error| js_storage("領域名の生成に失敗", "crypto.getRandomValues", error))?;
    Ok(format_area_name(&bytes))
}

/// `ws-` に続く128bit乱数の小文字hexを組み立てる。
fn format_area_name(bytes: &[u8; 16]) -> String {
    let mut name = String::with_capacity(AREA_PREFIX.len() + bytes.len() * 2);
    name.push_str(AREA_PREFIX);
    for byte in bytes {
        name.push_str(&format!("{byte:02x}"));
    }
    name
}

/// JS 例外の `name` を分類する。
/// `instanceof DOMException` は必須にせず、`name` 文字列だけで判定する。
fn classify(error: &JsValue) -> ExceptionKind {
    match exception_name(error).as_deref() {
        Some("NotFoundError") => ExceptionKind::NotFound,
        Some("TypeMismatchError") => ExceptionKind::TypeMismatch,
        _ => ExceptionKind::Other,
    }
}

/// JS 例外の `name` を文字列で取得する。
/// 取得失敗・文字列でない・未知は `None` とし、不明な例外を不存在と推測しない。
fn exception_name(error: &JsValue) -> Option<String> {
    js_sys::Reflect::get(error, &JsValue::from_str("name"))
        .ok()
        .and_then(|value| value.as_string())
}

/// JS 例外を診断付きの `Storage` 等へ変換する。
/// 操作・パス・例外 `name`/`message` を Rust 文字列として保持する。
/// `JsValue` を `source` には入れない。
fn js_storage(op: &str, target: &str, error: JsValue) -> WorkspaceError {
    let name = exception_name(&error).unwrap_or_else(|| String::from("unknown"));
    let message = js_sys::Reflect::get(&error, &JsValue::from_str("message"))
        .ok()
        .and_then(|value| value.as_string())
        .unwrap_or_default();
    if message.is_empty() {
        WorkspaceError::storage(format!("{op}: {target}（{name}）"))
    } else {
        WorkspaceError::storage(format!("{op}: {target}（{name}: {message}）"))
    }
}

/// 確保済みの自領域ハンドルを取り出す。
fn area_dir(state: &WebState, op: &str) -> Result<DirectoryHandle, WorkspaceError> {
    // `ensure_area` の成功後は必ず存在する。
    state
        .dir
        .clone()
        .ok_or_else(|| WorkspaceError::storage(format!("{op}に失敗: 領域が確保できない")))
}

/// 中間成分がすべて実ディレクトリである場合だけ最終成分の親を返す。
///
/// 存在しない中間成分がある場合は `None` を返し、呼び出し側が
/// `NotFound` と成功を使い分ける。ファイルの中間成分は `WrongEntryType` とする。
async fn resolve_strict(
    root: &DirectoryHandle,
    parts: &[&str],
    path: &str,
) -> Result<Option<DirectoryHandle>, WorkspaceError> {
    let mut current = root.clone();
    for part in parts {
        match current
            .get_directory_handle_with_options(part, &GetDirectoryHandleOptions { create: false })
            .await
        {
            Ok(next) => current = next,
            Err(error) => match classify(&error) {
                ExceptionKind::TypeMismatch => {
                    return Err(WorkspaceError::wrong_entry_type(format!(
                        "途中の親がディレクトリではない: {path}"
                    )));
                }
                ExceptionKind::NotFound => return Ok(None),
                ExceptionKind::Other => return Err(js_storage("探索に失敗", path, error)),
            },
        }
    }
    Ok(Some(current))
}

/// `write` 用に、存在しない中間ディレクトリを作成しながら最終成分の親を返す。
///
/// ファイルの中間成分は作成せず `WrongEntryType` とする。
async fn resolve_created(
    root: &DirectoryHandle,
    parts: &[&str],
    path: &str,
) -> Result<DirectoryHandle, WorkspaceError> {
    let mut current = root.clone();
    for part in parts {
        current = current
            .get_directory_handle_with_options(part, &GetDirectoryHandleOptions { create: true })
            .await
            .map_err(|error| match classify(&error) {
                ExceptionKind::TypeMismatch => WorkspaceError::wrong_entry_type(format!(
                    "途中の親がディレクトリではない: {path}"
                )),
                // `create:true` のため通常は起きない。その他と同様に扱う。
                ExceptionKind::NotFound | ExceptionKind::Other => {
                    js_storage("親ディレクトリの作成に失敗", path, error)
                }
            })?;
    }
    Ok(current)
}

async fn read_task(state: &mut WebState, path: &str) -> Result<Vec<u8>, WorkspaceError> {
    ensure_area(state).await?;
    let root = area_dir(state, "読み取り")?;
    // `validate_path` 済みのため成分は1つ以上ある。
    let mut parts: Vec<&str> = path.split('/').collect();
    // 直前行の条件から必ず存在する。
    let Some(leaf) = parts.pop() else {
        return Err(WorkspaceError::invalid_path("空のパスは指定できない"));
    };
    let Some(parent) = resolve_strict(&root, &parts, path).await? else {
        return Err(WorkspaceError::not_found(format!(
            "存在しないファイルは読めない: {path}"
        )));
    };
    let file = parent
        .get_file_handle_with_options(leaf, &GetFileHandleOptions { create: false })
        .await
        .map_err(|error| match classify(&error) {
            ExceptionKind::TypeMismatch => {
                WorkspaceError::wrong_entry_type(format!("ファイルではないため読めない: {path}"))
            }
            ExceptionKind::NotFound => {
                WorkspaceError::not_found(format!("存在しないファイルは読めない: {path}"))
            }
            ExceptionKind::Other => js_storage("読み取りに失敗", path, error),
        })?;
    file.read()
        .await
        .map_err(|error| js_storage("読み取りに失敗", path, error))
}

async fn write_task(state: &mut WebState, path: &str, bytes: &[u8]) -> Result<(), WorkspaceError> {
    ensure_area(state).await?;
    let root = area_dir(state, "書き込み")?;
    // `validate_path` 済みのため成分は1つ以上ある。
    let mut parts: Vec<&str> = path.split('/').collect();
    // 直前行の条件から必ず存在する。
    let Some(leaf) = parts.pop() else {
        return Err(WorkspaceError::invalid_path("空のパスは指定できない"));
    };
    let parent = resolve_created(&root, &parts, path).await?;
    let mut file = parent
        .get_file_handle_with_options(leaf, &GetFileHandleOptions { create: true })
        .await
        .map_err(|error| match classify(&error) {
            // ファイルとディレクトリを自動で相互置換しない。
            ExceptionKind::TypeMismatch => {
                WorkspaceError::wrong_entry_type(format!("ディレクトリ等には書き込めない: {path}"))
            }
            ExceptionKind::Other => js_storage("書き込みに失敗", path, error),
            // `create:true` のため通常は起きない。その他と同様に扱う。
            ExceptionKind::NotFound => js_storage("書き込みに失敗", path, error),
        })?;
    let mut writer = file
        .create_writable_with_options(&CreateWritableOptions {
            keep_existing_data: false,
        })
        .await
        .map_err(|error| js_storage("書き込みの開始に失敗", path, error))?;
    // 巨大データを JS へ一度に渡さない。
    // writer 作成後に write が失敗しても即抜けせず、`close()` の完了まで待つ。
    let mut write_error: Option<JsValue> = None;
    for chunk in bytes.chunks(WRITE_CHUNK_BYTES) {
        if let Err(error) = writer.write_at_cursor_pos(chunk).await {
            write_error = Some(error);
            break;
        }
    }
    let close_result = writer.close().await;
    if let Some(error) = write_error {
        return Err(js_storage("書き込みに失敗", path, error));
    }
    // 空データでも `close()` まで実施する。`truncate` は使わない。
    close_result.map_err(|error| js_storage("書き込みの確定に失敗", path, error))
}

async fn delete_task(state: &mut WebState, path: &str) -> Result<(), WorkspaceError> {
    ensure_area(state).await?;
    let root = area_dir(state, "削除")?;
    // `validate_path` 済みのため成分は1つ以上ある。
    let mut parts: Vec<&str> = path.split('/').collect();
    // 直前行の条件から必ず存在する。
    let Some(leaf) = parts.pop() else {
        return Err(WorkspaceError::invalid_path("空のパスは指定できない"));
    };
    let Some(parent) = resolve_strict(&root, &parts, path).await? else {
        // 存在しない対象の削除は成功扱いにする。
        return Ok(());
    };
    let mut parent = parent;
    match parent
        .remove_entry_with_options(leaf, &FileSystemRemoveOptions { recursive: true })
        .await
    {
        Ok(()) => Ok(()),
        // 存在しない対象の削除は成功扱いにする。
        Err(error) if classify(&error) == ExceptionKind::NotFound => Ok(()),
        Err(error) => Err(js_storage("削除に失敗", path, error)),
    }
}

async fn list_task(state: &mut WebState, directory: &str) -> Result<Vec<Entry>, WorkspaceError> {
    ensure_area(state).await?;
    let root = area_dir(state, "一覧の取得")?;
    let target = if directory.is_empty() {
        root
    } else {
        let parts: Vec<&str> = directory.split('/').collect();
        let mut current = root;
        for part in parts {
            current = current
                .get_directory_handle_with_options(
                    part,
                    &GetDirectoryHandleOptions { create: false },
                )
                .await
                .map_err(|error| match classify(&error) {
                    ExceptionKind::TypeMismatch => WorkspaceError::wrong_entry_type(format!(
                        "ディレクトリではないため一覧できない: {directory}"
                    )),
                    ExceptionKind::NotFound => WorkspaceError::not_found(format!(
                        "存在しないディレクトリは一覧できない: {directory}"
                    )),
                    ExceptionKind::Other => js_storage("一覧の取得に失敗", directory, error),
                })?;
        }
        current
    };
    let stream = target
        .entries()
        .await
        .map_err(|error| js_storage("一覧の取得に失敗", directory, error))?;
    futures::pin_mut!(stream);
    let mut entries = Vec::new();
    // 途中失敗で部分一覧を返さない。最後まで収集してから返す。
    while let Some(item) = stream.next().await {
        let (name, entry) =
            item.map_err(|error| js_storage("一覧の取得に失敗", directory, error))?;
        let kind = match entry {
            DirectoryEntry::File(_) => EntryKind::File,
            DirectoryEntry::Directory(_) => EntryKind::Directory,
        };
        let path = if directory.is_empty() {
            name
        } else {
            format!("{directory}/{name}")
        };
        entries.push(Entry { path, kind });
    }
    // `String` の順序は UTF-8 バイト列の辞書式順序と一致する。
    // JS 既定sort・locale比較・`PathBuf`・Unicode正規化は使わない。
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(entries)
}

async fn close_task(state: &mut WebState) -> Result<(), WorkspaceError> {
    let (mut parent, name) = match (state.parent.clone(), state.name.clone()) {
        (Some(parent), Some(name)) => (parent, name),
        // 削除対象の名前が無い場合は作成前に失敗している。
        // 何も作っていないため所有情報を解放して完了する。
        _ => {
            state.parent = None;
            state.name = None;
            state.dir = None;
            return Ok(());
        }
    };
    // 先行操作の失敗だけでは削除を中止しない（呼び出し側で結果を破棄済み）。
    let _ = parent
        .remove_entry_with_options(&name, &FileSystemRemoveOptions { recursive: true })
        .await;
    // 成功・失敗にかかわらず存在確認する。
    match parent
        .get_directory_handle_with_options(&name, &GetDirectoryHandleOptions { create: false })
        .await
    {
        // 削除完了。所有情報を解放する。
        Err(error) if classify(&error) == ExceptionKind::NotFound => {
            state.parent = None;
            state.name = None;
            state.dir = None;
            Ok(())
        }
        // 削除対象が残存する。`Closing` と所有情報を保持し、再 `close` に備える。
        Ok(_) => Err(WorkspaceError::storage(format!(
            "領域の削除を確認できない（対象が残存）: {name}"
        ))),
        // `TypeMismatchError` も不存在と解釈しない。確認不能の場合も同様に保持する。
        Err(error) => Err(js_storage("領域の削除を確認できない", &name, error)),
    }
}
