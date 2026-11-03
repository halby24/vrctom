//! UnityPackage展開の共通核（ターゲット非依存）。
//!
//! 公開契約の入口は [`crate::unitypackage`] を参照。
//!
//! gzip/tarを一回だけ読み、`asset` と `asset.meta` をGUID別に退避してから
//! 対応関係・パス・出力衝突を検証する。`asset` が `pathname` より先に現れても
//! 動く。内容は固定バッファで回し、退避の置き場（ファイル・メモリ）に依らず
//! 検証の順序と公開の条件は変えない。
//!
//! 退避先だけを [`StagingSink`] で差し替える。nativeは一時領域へ退避し、
//! webはメモリへ退避する。全件の検証が終わるまで成功したパッケージとして
//! 公開しない保証は、どちらの退避先でも同じである。
//!
//! 限界として、`asset` を持たない正常フォルダと `asset` の欠けた壊れた
//! ファイルはtar構造だけでは区別できない。`asset.meta` の意味解析や
//! meta内GUIDとの照合は行わない。

use std::collections::HashMap;
use std::io::{Read, Write};

use flate2::read::MultiGzDecoder;
use tar::{Archive, EntryType};

use super::{Entry, EntryKind, UnpackError, UnpackErrorKind, UnpackOptions, escape_diag};
use crate::unity::{AssetIndex, AssetMeta, IndexOptions, UnityError, UnityErrorKind};

/// 出力コピーに使う固定バッファの大きさ。
///
/// 資産の大きさに依らずこの大きさで回すことで、メモリ使用量を内容サイズに
/// 比例させない。値は性能上の目安であり、正確さには影響しない。
const COPY_BUFFER_SIZE: usize = 64 * 1024;

/// `pathname` 読み取りに使う固定バッファの大きさ。
const PATHNAME_BUFFER_SIZE: usize = 4096;

/// `pathname` の先頭成分として受け付ける名前。
const ASSETS_ROOT: &str = "Assets";

/// gzip圧縮tarのUnityPackageを読み、検証してからメモリ内に保持する。
///
/// tar走査・種別検査・上限・対応付け・パスと衝突の検証は
/// [`crate::unitypackage::unpack_to`] と同じ共通核で行い、退避先だけが
/// メモリになる。全件の検証が終わるまで成功したパッケージとして公開しない
/// 保証は同じであり、資源上限の意味も同じである。
/// `reader` には `Read` だけを要求し、`Seek` は使わない。
///
/// # Errors
///
/// 入力の読み取り失敗、tar構造や対応関係の破損、出力パスの契約違反、出力計画
/// の衝突、資源上限の超過の場合にエラーを返す。分類は `unpack_to` と同じである。
pub fn unpack_in_memory<R: Read>(
    reader: R,
    options: &UnpackOptions,
) -> Result<MemoryPackage, UnpackError> {
    let mut sink = MemorySink::default();
    let plan = scan_and_validate(reader, &mut sink, options).map_err(|failure| failure.error)?;
    MemoryPackage::from_validated(plan, &mut sink)
}

/// 検証済みUnityPackage内容の所有者。
///
/// [`unpack_in_memory`] が全件の検証を終えてから作る。未検証の項目は持たない。
/// 一覧と読取だけを提供し、可変な写像は公開しない。
#[derive(Debug, Clone)]
pub struct MemoryPackage {
    entries: Vec<Entry>,
    file_count: usize,
    directory_count: usize,
    output_bytes: u64,
    /// 出力相対パス（`Assets/...`・`Assets/....meta`）ごとの内容。
    /// ディレクトリ自体に内容はなく、`.meta` だけを持つ。
    files: HashMap<String, Vec<u8>>,
    /// `Assets` 基準の相対パスから実際の出力パスへの表。
    ///
    /// 先頭成分の大小文字違い（`assets/...` など）を吸収するための写像であり、
    /// キーは先頭成分を落とした相対パス、値は検証済みの出力パスである。
    /// `asset` と `asset.meta` の両方を登録し、`.meta` も読めるようにする。
    /// 衝突検証で相対パスが同じファイルは既に拒否されるため、キーは一意になる。
    asset_files: HashMap<String, String>,
}

impl MemoryPackage {
    /// 検証済みの計画と退避内容から組み立てる。全件検証の後にだけ呼ぶ。
    fn from_validated(plan: Plan, sink: &mut MemorySink) -> Result<Self, UnpackError> {
        let mut files = HashMap::with_capacity(plan.files.len());
        // `Assets` 基準の相対パスから実際の出力パスへの表。
        // `asset` と `asset.meta` の両方を登録し、native の読み手と同じく
        // `.meta` も読めるようにする。
        let mut asset_files = HashMap::with_capacity(plan.files.len());
        for (relative, spilled) in &plan.files {
            // 検証済みの `pathname` 由来であり、UTF-8であることが確定している。
            // 退避名もASCII確定である。いずれも欠ければ内部の不整合である。
            let Some(relative) = relative.to_str() else {
                return Err(UnpackError::new(
                    UnpackErrorKind::Storage,
                    "内部の出力相対パスがUTF-8でない",
                ));
            };
            let Some(spilled) = spilled.to_str() else {
                return Err(UnpackError::new(
                    UnpackErrorKind::Storage,
                    "内部の退避名がUTF-8でない",
                ));
            };
            let Some(bytes) = sink.files.remove(spilled) else {
                return Err(UnpackError::new(
                    UnpackErrorKind::Storage,
                    format!("内部の退避内容が見当たらない: {}", escape_diag(spilled)),
                ));
            };
            files.insert(relative.to_owned(), bytes);
            // `Assets` 自体（相対パスが無い）は表に含めない。
            if let Some(Some(key)) = strip_assets_root(relative) {
                asset_files.insert(key, relative.to_owned());
            }
        }
        // ディレクトリ自体に内容は無いが、表の照合（`asset_index`）のため登録する。
        // 読み取りは内容が無いため `NotFound` のままである。
        for entry in &plan.entries {
            if entry.kind == EntryKind::Directory
                && let Some(Some(key)) = strip_assets_root(&entry.pathname)
            {
                asset_files.insert(key, entry.pathname.clone());
            }
        }
        Ok(Self {
            file_count: plan
                .entries
                .iter()
                .filter(|entry| entry.kind == EntryKind::File)
                .count(),
            directory_count: plan
                .entries
                .iter()
                .filter(|entry| entry.kind == EntryKind::Directory)
                .count(),
            entries: plan.entries,
            output_bytes: plan.output_bytes,
            files,
            asset_files,
        })
    }

    /// 展開したエントリの一覧を返す。
    ///
    /// `pathname` のUTF-8バイト列順に並ぶ。[`crate::unitypackage::unpack_to`] の
    /// 報告と同じ順序・内容である。
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

    /// ファイルとmetaの合計バイト数を返す。
    pub fn output_bytes(&self) -> u64 {
        self.output_bytes
    }

    /// 出力相対パス（`Assets/...`・`Assets/....meta`）の内容を返す。
    ///
    /// ディレクトリ自体に内容はなく、対応する `.meta` だけが読める。
    /// ないパスには `None` を返す。
    pub fn read(&self, pathname: &str) -> Option<&[u8]> {
        self.files.get(pathname).map(Vec::as_slice)
    }

    /// メモリ内容から [`AssetIndex`] を構築する。
    ///
    /// `Assets` を索引のルートとして扱い、パスに含めない。`Assets` 自体は
    /// 件にしない。`.meta` を索引の正とし、実体の有無と種別の一致を確かめる。
    /// metaのない通常ファイル・大小文字のみ異なるパスの混在は拒否する。
    /// 規則は [`crate::unity::scan_directory`] に出力先の `Assets` 下を指した
    /// 場合と同じであり、決定的な順序で索引する。本文の読み込みは行わない。
    ///
    /// # Errors
    ///
    /// meta不良・対応不一致・重複・上限超過の場合にエラーを返す。
    /// 種別の分類は `scan_directory` と同じである。
    pub fn asset_index(&self, options: &IndexOptions) -> Result<AssetIndex, UnityError> {
        let mut metas: HashMap<String, AssetMeta> = HashMap::new();
        let mut assets: Vec<String> = Vec::new();
        let mut dirs: Vec<String> = Vec::new();
        let mut is_dir_of: HashMap<String, bool> = HashMap::new();
        let mut meta_total_bytes: u64 = 0;
        // `entries` は `pathname` 順であり、この順に積むことで決定的にする。
        for entry in &self.entries {
            // 共通核の検証済みのはずであり、想定外の形状は内部の不整合として拒否する。
            let Some(relative) = strip_assets_root(&entry.pathname) else {
                return Err(UnityError::new(
                    UnityErrorKind::InvalidPath,
                    "Assets基準の相対パスにできない",
                )
                .with_path(entry.pathname.clone()));
            };
            // `Assets` 自体は索引のルートであり、件にしない。
            let Some(relative) = relative else {
                continue;
            };
            // 読み取り側（`AssetSource::read_limited`）と同じ表で写せることを
            // 確かめる。表と項目が食い違う場合は内部の不整合として拒否する。
            let mapped = self.asset_files.get(&relative);
            if mapped.is_none_or(|actual| *actual != entry.pathname) {
                return Err(UnityError::new(
                    UnityErrorKind::InvalidPath,
                    "Assets基準の相対パスと出力パスが表と一致しない",
                )
                .with_path(entry.pathname.clone()));
            }
            match entry.kind {
                EntryKind::File => assets.push(relative.clone()),
                EntryKind::Directory => dirs.push(relative.clone()),
            }
            is_dir_of.insert(relative.clone(), matches!(entry.kind, EntryKind::Directory));
            // 上限の確かめ方はnativeの読み取り側と同じ順序にする。
            if metas.len() >= options.max_assets {
                return Err(UnityError::new(
                    UnityErrorKind::LimitExceeded,
                    format!("アセット数が上限を超えた: {}", metas.len() + 1),
                )
                .with_path(relative.clone()));
            }
            let meta_key = format!("{}.meta", entry.pathname);
            let Some(bytes) = self.files.get(&meta_key) else {
                return Err(UnityError::new(
                    UnityErrorKind::MissingMetadata,
                    "対応する実体がないmetaがある",
                )
                .with_path(relative.clone()));
            };
            let size = bytes.len() as u64;
            if size > options.max_meta_bytes {
                return Err(UnityError::new(
                    UnityErrorKind::LimitExceeded,
                    format!("metaが1件の上限を超えた: {size}バイト"),
                )
                .with_path(relative.clone()));
            }
            meta_total_bytes = meta_total_bytes.saturating_add(size);
            if meta_total_bytes > options.max_meta_total_bytes {
                return Err(UnityError::new(
                    UnityErrorKind::LimitExceeded,
                    format!("metaの合計が上限を超えた: {meta_total_bytes}"),
                )
                .with_path(relative.clone()));
            }
            let meta =
                AssetMeta::parse_with_limit(bytes, options.max_meta_bytes).map_err(|error| {
                    if error.path().is_none() {
                        error.with_path(relative.clone())
                    } else {
                        error
                    }
                })?;
            metas.insert(relative, meta);
        }
        AssetIndex::build_from_parts(metas, &assets, &dirs, |path| is_dir_of.get(path).copied())
    }
}

impl crate::unity::AssetSource for MemoryPackage {
    /// `Assets` 基準の相対パスの内容を上限付きで読む。
    ///
    /// `path` は [`crate::unity::AssetSource`] の契約どおり
    /// `Assets` 直下基準の `/` 区切りであり、先頭成分の大小文字違いは
    /// [`MemoryPackage::from_validated`] で作った表で実際の出力パスへ写す。
    /// `limit` を超える内容は切り詰めず `InvalidData` で失敗し、
    /// 存在しない場合は `NotFound` で失敗する。
    fn read_limited(&self, path: &str, limit: u64) -> std::io::Result<Vec<u8>> {
        let Some(actual) = self.asset_files.get(path) else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("メモリ内パッケージに無い: {}", escape_diag(path)),
            ));
        };
        let Some(bytes) = self.files.get(actual) else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("メモリ内パッケージに無い: {}", escape_diag(path)),
            ));
        };
        if bytes.len() as u64 > limit {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("読み取り上限を超えた: {limit}バイト"),
            ));
        }
        Ok(bytes.clone())
    }
}

/// 先頭成分（`Assets` の大小文字違いを含む）を落とし、基準相対パスにする。
///
/// `Assets` 自体は内側の `None` を返し、索引の件にしない。
/// 想定外の形状（共通核の検証済みのはず）は外側の `None` を返す。
fn strip_assets_root(pathname: &str) -> Option<Option<String>> {
    let Some((first, rest)) = pathname.split_once('/') else {
        if pathname.eq_ignore_ascii_case(ASSETS_ROOT) {
            return Some(None);
        }
        return None;
    };
    if !first.eq_ignore_ascii_case(ASSETS_ROOT) {
        return None;
    }
    if rest.is_empty() {
        return Some(None);
    }
    Some(Some(rest.to_owned()))
}

/// 退避先の差し替え点。tar走査・検証の順序は共通核に固定し、置き場だけを変える。
///
/// nativeは一時領域のファイルへ、webはメモリへ退避する。
pub(crate) trait StagingSink {
    /// `asset` の内容を退避し、バイト数を返す。
    fn spill_asset<R2: Read>(
        &mut self,
        entry: &mut tar::Entry<'_, R2>,
        guid: &str,
        raw_path: &[u8],
    ) -> Result<u64, Failure>;

    /// `asset.meta` の内容を退避し、バイト数を返す。
    fn spill_meta<R2: Read>(
        &mut self,
        entry: &mut tar::Entry<'_, R2>,
        guid: &str,
        raw_path: &[u8],
    ) -> Result<u64, Failure>;
}

/// メモリへの退避。検証が終わるまで公開しない一時置き場である。
#[derive(Debug, Default)]
pub(crate) struct MemorySink {
    files: HashMap<String, Vec<u8>>,
}

impl MemorySink {
    /// `asset`・`asset.meta` の内容をメモリへ退避し、バイト数を返す。
    fn spill_to_memory<R2: Read>(
        &mut self,
        entry: &mut tar::Entry<'_, R2>,
        guid: &str,
        raw_path: &[u8],
        suffix: &str,
    ) -> Result<u64, Failure> {
        let mut output = Vec::new();
        let moved = copy_entry_to_writer(entry, &mut output, guid, raw_path, "退避内容を書けない")?;
        // 退避先の重複は呼び出し側の検証で先に弾くため、ここでは予期しない扱いにする。
        // nativeの `File::create_new` に対応し、上書きせず失敗にする。
        let name = staging_name(guid, suffix);
        if self.files.contains_key(&name) {
            return Err(Failure::new(UnpackError::new(
                UnpackErrorKind::Storage,
                format!("一時退避先が重複する: {name:?}"),
            )));
        }
        self.files.insert(name, output);
        Ok(moved)
    }
}

impl StagingSink for MemorySink {
    fn spill_asset<R2: Read>(
        &mut self,
        entry: &mut tar::Entry<'_, R2>,
        guid: &str,
        raw_path: &[u8],
    ) -> Result<u64, Failure> {
        self.spill_to_memory(entry, guid, raw_path, ".asset")
    }

    fn spill_meta<R2: Read>(
        &mut self,
        entry: &mut tar::Entry<'_, R2>,
        guid: &str,
        raw_path: &[u8],
    ) -> Result<u64, Failure> {
        self.spill_to_memory(entry, guid, raw_path, ".asset.meta")
    }
}

/// 展開途中の失敗。nativeでは出力先を作った印を [`UnpackError`] 側に保持する。
#[derive(Debug)]
pub(crate) struct Failure {
    pub(crate) error: UnpackError,
}

impl Failure {
    pub(crate) fn new(error: UnpackError) -> Self {
        Self { error }
    }

    /// 出力先を作った後の失敗に印を付け、呼び出し側の後始末に使う。
    // nativeの出力後始末だけで使う共有契約のため、対象外ターゲットでは未使用になり得る。
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) fn with_output_created(error: UnpackError) -> Self {
        Self {
            error: error.with_output_created(),
        }
    }
}

/// 呼び出し側リーダーの転送失敗である印。gzipより下で付けて上で剥がす。
///
/// flate2は内側の読み取り失敗をそのまま返すため、印は層を越えて残る。
#[derive(Debug)]
struct InputMarker(std::io::Error);

impl std::fmt::Display for InputMarker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for InputMarker {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}

/// 総読み取り量の上限到達の印。[`Meter`] が上限超過を読んだときの失敗に付ける。
#[derive(Debug)]
struct LimitMarker;

impl std::fmt::Display for LimitMarker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "総読み取り量の上限に達した")
    }
}

impl std::error::Error for LimitMarker {}

/// 呼び出し側リーダーの転送失敗に印を付ける読み取り層。
struct TaggedReader<R>(R);

impl<R> TaggedReader<R> {
    fn new(reader: R) -> Self {
        Self(reader)
    }
}

impl<R: Read> Read for TaggedReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.0
            .read(buf)
            .map_err(|error| std::io::Error::new(error.kind(), InputMarker(error)))
    }
}

/// 展開後バイト数を数え、上限超過を上限到達印の失敗で打ち切る読み取り層。
///
/// gzipの外側に置くことで、ヘッダ・padding・読み捨て分も含めて数える。
/// 上限ちょうどの入力は成功させ、上限を超える読み取りだけを失敗にする。
struct Meter<R> {
    inner: R,
    total: u64,
    max: u64,
}

impl<R> Meter<R> {
    fn new(inner: R, max: u64) -> Self {
        Self {
            inner,
            total: 0,
            max,
        }
    }
}

impl<R: Read> Read for Meter<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let read = self.inner.read(buf)?;
        if read > 0 {
            self.total = self.total.saturating_add(read as u64);
            if self.total > self.max {
                return Err(std::io::Error::other(LimitMarker));
            }
        }
        Ok(read)
    }
}

/// エラー連鎖から指定の印を探す。
///
/// `io::Error::source()` は Custom の payload 自体を飛ばして payload の
/// `source()` を返すため、`get_ref()` で payload を直接見る。
/// tar/flate2 は読み取り失敗を包まずに返すため、外側の `io::Error` 連鎖を
/// `get_ref()` で辿れば十分である。
fn find_marker<M>(error: &std::io::Error) -> Option<&M>
where
    M: std::error::Error + Send + Sync + 'static,
{
    use std::error::Error as _;
    let mut current: Option<&std::io::Error> = Some(error);
    while let Some(io_error) = current {
        if let Some(payload) = io_error.get_ref()
            && let Some(marker) = payload.downcast_ref::<M>()
        {
            return Some(marker);
        }
        current = match io_error.source() {
            Some(source) => source.downcast_ref::<std::io::Error>(),
            None => None,
        };
    }
    None
}

/// 読み取り失敗の原因分類。呼び出し側が文脈の診断文を付ける。
fn classify_read(error: &std::io::Error) -> UnpackErrorKind {
    if find_marker::<LimitMarker>(error).is_some() {
        return UnpackErrorKind::LimitExceeded;
    }
    if let Some(marker) = find_marker::<InputMarker>(error) {
        // 末尾欠けは壊れたパッケージとして扱い、それ以外の転送失敗は `Input` とする。
        if marker.0.kind() == std::io::ErrorKind::UnexpectedEof {
            return UnpackErrorKind::InvalidPackage;
        }
        return UnpackErrorKind::Input;
    }
    UnpackErrorKind::InvalidPackage
}

/// 外部入力のバイト列を表示用にエスケープする。
///
/// 引用符で囲んで示し、中身は [`escape_diag`] と同じ可視エスケープにする。
/// 非UTF-8は代替文字に置き換える。GUIDは32桁ASCII hex検証済みのため、
/// `{:?}` のままで制御文字を含まない。
fn escape_bytes(bytes: &[u8]) -> String {
    format!("\"{}\"", escape_diag(&String::from_utf8_lossy(bytes)))
}

/// GUID別の退避状況。メンバー順やGUID間交錯に依らず検証できるよう保持する。
#[derive(Debug, Default)]
pub(crate) struct Slot {
    has_dir: bool,
    pathname: Option<Vec<u8>>,
    has_asset: bool,
    asset_size: u64,
    has_meta: bool,
    meta_size: u64,
    has_preview: bool,
}

/// 一時退避ファイルの名前を組み立てる。GUIDは小文字hexに統一済みのため安全。
pub(crate) fn staging_name(guid: &str, suffix: &str) -> String {
    format!("{guid}{suffix}")
}

/// 検証済みGUIDの出力計画。
#[derive(Debug)]
struct PlannedEntry {
    guid: String,
    pathname: String,
    kind: EntryKind,
    asset_size: u64,
    meta_size: u64,
}

/// 出力計画全体。ファイルは退避元と対にし、ディレクトリは相対パスのみ持つ。
#[derive(Debug)]
pub(crate) struct Plan {
    pub(crate) entries: Vec<Entry>,
    pub(crate) files: Vec<(std::path::PathBuf, std::path::PathBuf)>,
    // 計画内のディレクトリ作成はnativeの書き出しだけで使う。
    // メモリ経路では暗黙の親も含めて作らない。
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) dirs: Vec<std::path::PathBuf>,
    pub(crate) output_bytes: u64,
}

/// 読み取りから検証までの共通本体。退避先だけが `sink` で変わる。
///
/// 全件の検証が終わるまで成功として公開しない保証は、どちらの退避先でも同じである。
pub(crate) fn scan_and_validate<R: Read, S: StagingSink>(
    reader: R,
    sink: &mut S,
    options: &UnpackOptions,
) -> Result<Plan, Failure> {
    let meter = Meter::new(
        MultiGzDecoder::new(TaggedReader::new(reader)),
        options.max_total_bytes,
    );
    let mut archive = Archive::new(meter);
    let mut slots: HashMap<String, Slot> = HashMap::new();
    let mut entry_count: u64 = 0;
    let mut pathname_total: u64 = 0;
    {
        let mut raw_entries = archive
            .entries()
            .map_err(|error| invalid_package_read(error, "tar一覧を開始できない"))?
            .raw(true);
        // `raw(true)` で拡張解決を無効化し、GNU・PAX等の拡張型も本来の種別で見る。
        while let Some(mut entry) = raw_entries
            .next()
            .transpose()
            .map_err(|error| invalid_package_read(error, "tarエントリを読めない"))?
        {
            entry_count = entry_count.saturating_add(1);
            if entry_count > options.max_entries {
                return Err(Failure::new(UnpackError::new(
                    UnpackErrorKind::LimitExceeded,
                    format!("tarエントリ数が上限を超えた: {entry_count}"),
                )));
            }
            collect_entry(&mut entry, &mut slots, sink, options, &mut pathname_total)?;
        }
    }
    // tar終端で早期成功せずgzip終端まで読み、CRC・途中切れと終端後データを確認する。
    let mut meter = archive.into_inner();
    drain_trailing(&mut meter).map_err(|error| {
        let kind = classify_read(&error);
        Failure::new(
            UnpackError::new(
                kind,
                format!(
                    "終端後の読み取りに失敗した: {}",
                    escape_diag(&error.to_string())
                ),
            )
            .with_source(error),
        )
    })?;
    validate_and_plan(slots)
}

/// tar一覧開始やエントリ取得の読み取り失敗を分類する。
fn invalid_package_read(error: std::io::Error, context: &str) -> Failure {
    let kind = classify_read(&error);
    let detail = format!("{}: {}", context, escape_diag(&error.to_string()));
    Failure::new(UnpackError::new(kind, detail).with_source(error))
}

/// 1エントリを検証して退避する。内容の対応付けは全件読み終えてから行う。
fn collect_entry<R2: Read, S: StagingSink>(
    entry: &mut tar::Entry<'_, R2>,
    slots: &mut HashMap<String, Slot>,
    sink: &mut S,
    options: &UnpackOptions,
    pathname_total: &mut u64,
) -> Result<(), Failure> {
    // 通常ファイルとディレクトリだけを受け付け、symlink・hardlink・特殊・sparse・
    // 未知やGNU・PAX等の拡張型は拒否する。`raw(true)` のため解決前の種別が見える。
    let entry_type = entry.header().entry_type();
    if !matches!(entry_type, EntryType::Regular | EntryType::Directory) {
        return Err(Failure::new(UnpackError::new(
            UnpackErrorKind::InvalidPackage,
            format!(
                "未対応のtar種別である: {}: 0x{:02x}",
                escape_bytes(&entry.path_bytes()),
                entry_type.as_byte(),
            ),
        )));
    }
    let is_dir = matches!(entry_type, EntryType::Directory);
    let raw_path = entry.path_bytes().into_owned();
    // ディレクトリの末尾 `/` はtarの慣習であり、構造の一部として扱わない。
    let body = if is_dir {
        raw_path.strip_suffix(b"/").unwrap_or(&raw_path)
    } else {
        &raw_path[..]
    };
    if body.is_empty() || body.split(|byte| *byte == b'/').any(|part| part.is_empty()) {
        return Err(Failure::new(UnpackError::new(
            UnpackErrorKind::InvalidPackage,
            format!(
                "tar内パスが空または空成分を含む: {}",
                escape_bytes(&raw_path)
            ),
        )));
    }
    let parts: Vec<&[u8]> = body.split(|byte| *byte == b'/').collect();
    if (is_dir && parts.len() != 1) || (!is_dir && parts.len() != 2) {
        return Err(Failure::new(UnpackError::new(
            UnpackErrorKind::InvalidPackage,
            format!("tar内パスの階層が不正である: {}", escape_bytes(&raw_path)),
        )));
    }
    // GUIDは32桁ASCII hexとし、小文字に統一する。
    // hex検証済みのためASCII確定であり、 UTF-8変換に失敗しない。
    let guid_raw = parts[0];
    if guid_raw.len() != 32 || !guid_raw.iter().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(Failure::new(UnpackError::new(
            UnpackErrorKind::InvalidPackage,
            format!("GUIDが32桁ASCII hexでない: {}", escape_bytes(&raw_path)),
        )));
    }
    let guid: String = guid_raw
        .to_ascii_lowercase()
        .iter()
        .map(|byte| *byte as char)
        .collect();
    let slot = slots.entry(guid.clone()).or_default();
    if parts.len() == 1 {
        if slot.has_dir {
            return Err(Failure::new(
                UnpackError::new(
                    UnpackErrorKind::InvalidPackage,
                    format!("GUIDディレクトリが重複する: {}", escape_bytes(&raw_path)),
                )
                .with_guid(guid),
            ));
        }
        slot.has_dir = true;
        return Ok(());
    }
    let member = parts[1];
    if member == b"pathname" {
        if slot.pathname.is_some() {
            return Err(duplicate_member(&guid, &raw_path, "pathname"));
        }
        let content = read_bounded(entry, &guid, &raw_path, options, pathname_total)?;
        slot.pathname = Some(content);
    } else if member == b"asset" {
        if slot.has_asset {
            return Err(duplicate_member(&guid, &raw_path, "asset"));
        }
        slot.asset_size = sink.spill_asset(entry, &guid, &raw_path)?;
        slot.has_asset = true;
    } else if member == b"asset.meta" {
        if slot.has_meta {
            return Err(duplicate_member(&guid, &raw_path, "asset.meta"));
        }
        slot.meta_size = sink.spill_meta(entry, &guid, &raw_path)?;
        slot.has_meta = true;
    } else if member == b"preview.png" {
        if slot.has_preview {
            return Err(duplicate_member(&guid, &raw_path, "preview.png"));
        }
        drain(entry, &guid, &raw_path)?;
        slot.has_preview = true;
    } else {
        return Err(Failure::new(
            UnpackError::new(
                UnpackErrorKind::InvalidPackage,
                format!("未対応のメンバーである: {}", escape_bytes(&raw_path)),
            )
            .with_guid(guid),
        ));
    }
    Ok(())
}

/// 同一GUID・同一メンバーの再出現を拒否する。
fn duplicate_member(guid: &str, raw_path: &[u8], member: &str) -> Failure {
    Failure::new(
        UnpackError::new(
            UnpackErrorKind::InvalidPackage,
            format!(
                "同一GUIDの同一メンバーが重複する: {member}: {}",
                escape_bytes(raw_path)
            ),
        )
        .with_guid(guid.to_owned()),
    )
}

/// `pathname` を上限付きで読む。合計量もここで積算する。
fn read_bounded<R2: Read>(
    entry: &mut tar::Entry<'_, R2>,
    guid: &str,
    raw_path: &[u8],
    options: &UnpackOptions,
    pathname_total: &mut u64,
) -> Result<Vec<u8>, Failure> {
    let mut content = Vec::new();
    let mut chunk = [0u8; PATHNAME_BUFFER_SIZE];
    loop {
        let read = entry.read(&mut chunk).map_err(|error| {
            let kind = classify_read(&error);
            Failure::new(
                UnpackError::new(
                    kind,
                    format!(
                        "pathnameを読めない: {}: {}",
                        escape_bytes(raw_path),
                        escape_diag(&error.to_string())
                    ),
                )
                .with_guid(guid.to_owned())
                .with_source(error),
            )
        })?;
        if read == 0 {
            break;
        }
        if content.len().saturating_add(read) > options.max_pathname_bytes {
            return Err(Failure::new(
                UnpackError::new(
                    UnpackErrorKind::LimitExceeded,
                    format!("pathnameが一件の上限を超えた: {}", escape_bytes(raw_path)),
                )
                .with_guid(guid.to_owned()),
            ));
        }
        content.extend_from_slice(&chunk[..read]);
    }
    *pathname_total = pathname_total.saturating_add(content.len() as u64);
    if *pathname_total > options.max_pathname_total_bytes {
        return Err(Failure::new(
            UnpackError::new(
                UnpackErrorKind::LimitExceeded,
                format!("pathnameの合計が上限を超えた: {pathname_total}"),
            )
            .with_guid(guid.to_owned()),
        ));
    }
    Ok(content)
}

/// 読み捨てメンバーを固定バッファで読み切る。総量の計数は [`Meter`] が行う。
fn drain<R2: Read>(
    entry: &mut tar::Entry<'_, R2>,
    guid: &str,
    raw_path: &[u8],
) -> Result<(), Failure> {
    let mut chunk = [0u8; COPY_BUFFER_SIZE];
    loop {
        let read = entry.read(&mut chunk).map_err(|error| {
            let kind = classify_read(&error);
            Failure::new(
                UnpackError::new(
                    kind,
                    format!(
                        "読み捨て内容を読めない: {}: {}",
                        escape_bytes(raw_path),
                        escape_diag(&error.to_string())
                    ),
                )
                .with_guid(guid.to_owned())
                .with_source(error),
            )
        })?;
        if read == 0 {
            return Ok(());
        }
    }
}

/// 入力から出力へ固定バッファで写し、バイト数を返す。
///
/// 退避先（ファイル・メモリ）に依らず内容サイズの保持をここに作らない。
/// 書き込み失敗は一時領域の異常として扱う。
pub(crate) fn copy_entry_to_writer<R2: Read, W: Write>(
    input: &mut tar::Entry<'_, R2>,
    output: &mut W,
    guid: &str,
    raw_path: &[u8],
    context: &str,
) -> Result<u64, Failure> {
    let mut moved: u64 = 0;
    let mut chunk = [0u8; COPY_BUFFER_SIZE];
    loop {
        let read = input.read(&mut chunk).map_err(|error| {
            let kind = classify_read(&error);
            Failure::new(
                UnpackError::new(
                    kind,
                    format!(
                        "{context}: {}: {}",
                        escape_bytes(raw_path),
                        escape_diag(&error.to_string())
                    ),
                )
                .with_guid(guid.to_owned())
                .with_source(error),
            )
        })?;
        if read == 0 {
            return Ok(moved);
        }
        output.write_all(&chunk[..read]).map_err(|error| {
            Failure::new(
                UnpackError::new(
                    UnpackErrorKind::Storage,
                    format!(
                        "{context}: {}: {}",
                        escape_bytes(raw_path),
                        escape_diag(&error.to_string())
                    ),
                )
                .with_guid(guid.to_owned())
                .with_source(error),
            )
        })?;
        moved = moved.saturating_add(read as u64);
    }
}

/// gzip終端まで読み切り、tar終端後の非ゼロデータを拒否する。
fn drain_trailing<R: Read>(reader: &mut R) -> std::io::Result<()> {
    let mut chunk = [0u8; COPY_BUFFER_SIZE];
    loop {
        let read = reader.read(&mut chunk)?;
        if read == 0 {
            return Ok(());
        }
        if chunk[..read].iter().any(|byte| *byte != 0) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "tar終端後に非ゼロデータがある",
            ));
        }
    }
}

/// 対応関係・パス・出力衝突を検証し、出力計画を作る。
fn validate_and_plan(slots: HashMap<String, Slot>) -> Result<Plan, Failure> {
    let mut planned: Vec<PlannedEntry> = Vec::with_capacity(slots.len());
    for (guid, slot) in &slots {
        let Some(pathname_raw) = &slot.pathname else {
            return Err(Failure::new(
                UnpackError::new(
                    UnpackErrorKind::InvalidPackage,
                    format!("pathnameが無いGUIDがある: {guid:?}"),
                )
                .with_guid(guid.clone()),
            ));
        };
        if !slot.has_meta {
            return Err(Failure::new(
                UnpackError::new(
                    UnpackErrorKind::InvalidPackage,
                    format!("asset.metaが無いGUIDがある: {guid:?}"),
                )
                .with_guid(guid.clone()),
            ));
        }
        let pathname = String::from_utf8(pathname_raw.clone()).map_err(|_| {
            Failure::new(
                UnpackError::new(
                    UnpackErrorKind::InvalidPath,
                    format!("pathnameがUTF-8でない: {}", escape_bytes(pathname_raw)),
                )
                .with_guid(guid.clone()),
            )
        })?;
        // `workspace` の検証を内部で再利用し、UnityPackage用に包み直す。公開はしない。
        if let Err(workspace) = crate::workspace::validate_path(&pathname, false) {
            return Err(Failure::new(
                UnpackError::new(
                    UnpackErrorKind::InvalidPath,
                    format!(
                        "出力パスが契約に違反する: {}: {workspace}",
                        escape_bytes(pathname.as_bytes())
                    ),
                )
                .with_guid(guid.clone())
                .with_pathname(pathname.clone()),
            ));
        }
        let first = pathname.split('/').next().unwrap_or_default();
        if !first.eq_ignore_ascii_case(ASSETS_ROOT) {
            return Err(Failure::new(
                UnpackError::new(
                    UnpackErrorKind::InvalidPath,
                    format!(
                        "先頭成分がAssetsでない: {}",
                        escape_bytes(pathname.as_bytes())
                    ),
                )
                .with_guid(guid.clone())
                .with_pathname(pathname.clone()),
            ));
        }
        let kind = if slot.has_asset {
            EntryKind::File
        } else {
            EntryKind::Directory
        };
        if pathname.eq_ignore_ascii_case(ASSETS_ROOT) && kind == EntryKind::File {
            return Err(Failure::new(
                UnpackError::new(
                    UnpackErrorKind::InvalidPath,
                    format!(
                        "Assetsそのものをファイルにはできない: {}",
                        escape_bytes(pathname.as_bytes())
                    ),
                )
                .with_guid(guid.clone())
                .with_pathname(pathname.clone()),
            ));
        }
        planned.push(PlannedEntry {
            guid: guid.clone(),
            pathname,
            kind,
            asset_size: slot.asset_size,
            meta_size: slot.meta_size,
        });
    }
    // 別GUIDが同じpathnameを使う場合も内容同一でも拒否する。
    // ASCII大小文字だけ異なる名前も衝突扱いにする。
    let mut files: HashMap<String, String> = HashMap::new();
    let mut insert_file = |rel: &str, guid: &str| -> Result<(), Failure> {
        let key = rel.to_ascii_lowercase();
        if let Some(existing) = files.get(&key) {
            return Err(Failure::new(
                UnpackError::new(
                    UnpackErrorKind::Conflict,
                    format!(
                        "出力パスが重複する: {} と {}",
                        escape_bytes(rel.as_bytes()),
                        escape_bytes(existing.as_bytes())
                    ),
                )
                .with_guid(guid.to_owned())
                .with_pathname(rel.to_owned()),
            ));
        }
        files.insert(key, rel.to_owned());
        Ok(())
    };
    for entry in &planned {
        if entry.kind == EntryKind::File {
            insert_file(&entry.pathname, &entry.guid)?;
        }
        insert_file(&format!("{}.meta", entry.pathname), &entry.guid)?;
    }
    let mut dirs: HashMap<String, String> = HashMap::new();
    let mut insert_dir = |rel: &str, guid: &str| -> Result<(), Failure> {
        let key = rel.to_ascii_lowercase();
        if let Some(existing) = files.get(&key) {
            return Err(Failure::new(
                UnpackError::new(
                    UnpackErrorKind::Conflict,
                    format!(
                        "ファイルとディレクトリが衝突する: {} と {}",
                        escape_bytes(rel.as_bytes()),
                        escape_bytes(existing.as_bytes())
                    ),
                )
                .with_guid(guid.to_owned())
                .with_pathname(rel.to_owned()),
            ));
        }
        if let Some(existing) = dirs.get(&key) {
            if existing != rel {
                return Err(Failure::new(
                    UnpackError::new(
                        UnpackErrorKind::Conflict,
                        format!(
                            "出力パスが重複する: {} と {}",
                            escape_bytes(rel.as_bytes()),
                            escape_bytes(existing.as_bytes())
                        ),
                    )
                    .with_guid(guid.to_owned())
                    .with_pathname(rel.to_owned()),
                ));
            }
            return Ok(());
        }
        dirs.insert(key, rel.to_owned());
        Ok(())
    };
    for entry in &planned {
        // 暗黙の親ディレクトリも計画に含め、親がファイルになる構造を拒否する。
        let components: Vec<&str> = entry.pathname.split('/').collect();
        let mut prefix = String::new();
        for (index, component) in components.iter().enumerate() {
            if index + 1 == components.len() {
                break;
            }
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(component);
            insert_dir(&prefix, &entry.guid)?;
        }
        if entry.kind == EntryKind::Directory {
            insert_dir(&entry.pathname, &entry.guid)?;
        }
    }
    for entry in &planned {
        // 明示フォルダ自体がファイル計画と衝突しないことを確かめる。
        if entry.kind == EntryKind::Directory
            && let Some(existing) = files.get(&entry.pathname.to_ascii_lowercase())
        {
            return Err(Failure::new(
                UnpackError::new(
                    UnpackErrorKind::Conflict,
                    format!(
                        "ファイルとディレクトリが衝突する: {} と {}",
                        escape_bytes(entry.pathname.as_bytes()),
                        escape_bytes(existing.as_bytes())
                    ),
                )
                .with_guid(entry.guid.clone())
                .with_pathname(entry.pathname.clone()),
            ));
        }
    }
    let mut report_entries: Vec<Entry> = planned
        .iter()
        .map(|entry| Entry {
            guid: entry.guid.clone(),
            pathname: entry.pathname.clone(),
            kind: entry.kind,
        })
        .collect();
    // `String` の順序はUTF-8バイト列の辞書式順序と一致する。
    report_entries.sort_by(|a, b| a.pathname.cmp(&b.pathname));
    let mut files_out: Vec<(std::path::PathBuf, std::path::PathBuf)> = Vec::new();
    let mut output_bytes: u64 = 0;
    for entry in &planned {
        if entry.kind == EntryKind::File {
            files_out.push((
                std::path::PathBuf::from(&entry.pathname),
                std::path::PathBuf::from(staging_name(&entry.guid, ".asset")),
            ));
            output_bytes = output_bytes.saturating_add(entry.asset_size);
        }
        files_out.push((
            std::path::PathBuf::from(format!("{}.meta", entry.pathname)),
            std::path::PathBuf::from(staging_name(&entry.guid, ".asset.meta")),
        ));
        output_bytes = output_bytes.saturating_add(entry.meta_size);
    }
    files_out.sort_by(|a, b| a.0.cmp(&b.0));
    let mut dirs_out: Vec<std::path::PathBuf> =
        dirs.into_values().map(std::path::PathBuf::from).collect();
    dirs_out.sort_by(|a, b| {
        a.components()
            .count()
            .cmp(&b.components().count())
            .then_with(|| a.cmp(b))
    });
    Ok(Plan {
        entries: report_entries,
        files: files_out,
        dirs: dirs_out,
        output_bytes,
    })
}
