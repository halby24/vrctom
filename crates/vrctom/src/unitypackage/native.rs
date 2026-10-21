//! UnityPackage展開の Native 実装。
//!
//! 公開契約の入口は [`crate::unitypackage`] を参照。
//!
//! gzip/tarを一回だけ読み、`asset` と `asset.meta` をGUID別に一時領域へ
//! 退避してから対応関係・パス・出力衝突を検証する。`asset` が `pathname`
//! より先に現れても動く。内容は固定バッファで回し、メモリ使用量を内容
//! サイズに比例させない。
//!
//! 限界として、`asset` を持たない正常フォルダと `asset` の欠けた壊れた
//! ファイルはtar構造だけでは区別できない。`asset.meta` の意味解析や
//! meta内GUIDとの照合は行わない。

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use flate2::read::MultiGzDecoder;
use tar::{Archive, EntryType};
use tempfile::TempDir;

use super::{
    Entry, EntryKind, UnpackError, UnpackErrorKind, UnpackOptions, UnpackReport, escape_diag,
};

/// 出力コピーに使う固定バッファの大きさ。
///
/// 資産の大きさに依らずこの大きさで回すことで、メモリ使用量を内容サイズに
/// 比例させない。値は性能上の目安であり、正確さには影響しない。
const COPY_BUFFER_SIZE: usize = 64 * 1024;

/// `pathname` 読み取りに使う固定バッファの大きさ。
const PATHNAME_BUFFER_SIZE: usize = 4096;

/// `pathname` の先頭成分として受け付ける名前。
const ASSETS_ROOT: &str = "Assets";

/// gzip圧縮tarのUnityPackageを読み、検証してから出力先へ展開する。
///
/// `reader` には `Read` だけを要求し、`Seek` は使わない。ファイルオープンや
/// 入力名の診断は呼び出し側の責務である。
/// 出力先の親は既存ディレクトリであることを要求し、出力先自体は不存在必須
/// とする（空ディレクトリやぶら下がりリンクも受け付けない）。
///
/// # Errors
///
/// 入力の読み取り失敗、tar構造や対応関係の破損、出力パスの契約違反、出力計画
/// の衝突、資源上限の超過、一時領域や出力先の I/O 失敗の場合にエラーを返す。
/// 出力開始後の失敗では自分が作った出力先の削除を試み、削除失敗は主原因に
/// 併記して残存の有無を [`UnpackError::partial_output`] などに添える。
pub fn unpack_to<R: Read>(
    reader: R,
    destination: impl AsRef<Path>,
    options: &UnpackOptions,
) -> Result<UnpackReport, UnpackError> {
    let destination = destination.as_ref();
    // 呼び出し側指定の親を基準点に固定し、アーカイブ由来パスでの逸脱を防ぐ。
    // `canonicalize` で親のシンボリックリンクも解決する。
    let parent = destination
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let canonical_parent = parent.canonicalize().map_err(|error| {
        UnpackError::new(
            UnpackErrorKind::Storage,
            format!(
                "出力先の親を解決できない: {parent:?}: {}",
                escape_diag(&error.to_string())
            ),
        )
        .with_source(error)
    })?;
    let Some(file_name) = destination.file_name() else {
        return Err(UnpackError::new(
            UnpackErrorKind::InvalidPath,
            format!("出力先の名前が空である: {destination:?}"),
        ));
    };
    let canonical_destination = canonical_parent.join(file_name);
    // 出力先自体は不存在必須とし、空ディレクトリやぶら下がりリンクも受け付けない。
    // `symlink_metadata` はリンク先をたどらないため、ぶら下がりも検出する。
    if fs::symlink_metadata(&canonical_destination).is_ok() {
        return Err(UnpackError::new(
            UnpackErrorKind::Conflict,
            format!("出力先が既に存在する: {canonical_destination:?}"),
        ));
    }
    // 所有権が明確な一時領域を出力先の親に作り、退避と検証にだけ使う。
    // `workspace` は中間データ用であり、最終出力先の責務と混ぜない。
    let staging = TempDir::new_in(&canonical_parent).map_err(|error| {
        UnpackError::new(
            UnpackErrorKind::Storage,
            format!(
                "一時領域を作れない: {canonical_parent:?}: {}",
                escape_diag(&error.to_string())
            ),
        )
        .with_source(error)
    })?;
    let staging_path = staging.path().to_path_buf();
    let outcome = run(reader, &canonical_destination, staging.path(), options);
    match outcome {
        Ok(report) => {
            // 一時領域を明示的に削除する。失敗しても自分が作った出力先の削除を
            // 試み、出力の残存有無を `partial_output` に正しく反映する。
            // 出力を消せれば残存報告はせず、出力の削除にも失敗したときだけ残存を報告する。
            #[cfg(test)]
            let staging_result = if options.inject_staging_cleanup_error {
                // 故障注入は削除を行わず、削除失敗の診断経路だけを再現する。
                // `TempDir` はこの分岐の終わりで破棄されるため、領域自体は残らない。
                Err(io::Error::other("一時領域の削除に失敗（テストの故障注入）"))
            } else {
                staging.close()
            };
            #[cfg(not(test))]
            let staging_result = staging.close();
            if let Err(error) = staging_result {
                let mut failure = UnpackError::new(
                    UnpackErrorKind::Storage,
                    format!(
                        "一時領域を削除できない: {staging_path:?}: {}",
                        escape_diag(&error.to_string())
                    ),
                )
                .with_source(error);
                failure.set_partial_temp(staging_path.clone());
                if let Err(cleanup) = remove_output(&canonical_destination, options) {
                    failure.append_cleanup(format!(
                        "出力先の削除に失敗したため残存し得る: {canonical_destination:?}: {}",
                        escape_diag(&cleanup.to_string())
                    ));
                    failure.set_partial_output(canonical_destination.clone());
                }
                return Err(failure);
            }
            Ok(report)
        }
        Err(mut failure) => {
            if failure.error.take_output_created() {
                let remove = remove_output(&canonical_destination, options);
                if let Err(cleanup) = remove {
                    failure.error.append_cleanup(format!(
                        "出力先の削除に失敗したため残存し得る: {canonical_destination:?}: {}",
                        escape_diag(&cleanup.to_string())
                    ));
                    failure
                        .error
                        .set_partial_output(canonical_destination.clone());
                }
            }
            if let Err(cleanup) = staging.close() {
                failure.error.append_cleanup(format!(
                    "一時領域の削除に失敗したため残存し得る: {staging_path:?}: {}",
                    escape_diag(&cleanup.to_string())
                ));
                failure.error.set_partial_temp(staging_path.clone());
            }
            Err(failure.error)
        }
    }
}

/// 展開途中の失敗。出力先を作った印は [`UnpackError`] 側に保持する。
#[derive(Debug)]
struct Failure {
    error: UnpackError,
}

impl Failure {
    fn new(error: UnpackError) -> Self {
        Self { error }
    }

    /// 出力先を作った後の失敗に印を付け、呼び出し側の後始末に使う。
    fn with_output_created(error: UnpackError) -> Self {
        Self {
            error: error.with_output_created(),
        }
    }
}

/// 呼び出し側リーダーの転送失敗である印。gzipより下で付けて上で剥がす。
///
/// flate2は内側の読み取り失敗をそのまま返すため、印は層を越えて残る。
#[derive(Debug)]
struct InputMarker(io::Error);

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
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0
            .read(buf)
            .map_err(|error| io::Error::new(error.kind(), InputMarker(error)))
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
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let read = self.inner.read(buf)?;
        if read > 0 {
            self.total = self.total.saturating_add(read as u64);
            if self.total > self.max {
                return Err(io::Error::other(LimitMarker));
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
fn find_marker<M>(error: &io::Error) -> Option<&M>
where
    M: std::error::Error + Send + Sync + 'static,
{
    use std::error::Error as _;
    let mut current: Option<&io::Error> = Some(error);
    while let Some(io_error) = current {
        if let Some(payload) = io_error.get_ref()
            && let Some(marker) = payload.downcast_ref::<M>()
        {
            return Some(marker);
        }
        current = match io_error.source() {
            Some(source) => source.downcast_ref::<io::Error>(),
            None => None,
        };
    }
    None
}

/// 読み取り失敗の原因分類。呼び出し側が文脈の診断文を付ける。
fn classify_read(error: &io::Error) -> UnpackErrorKind {
    if find_marker::<LimitMarker>(error).is_some() {
        return UnpackErrorKind::LimitExceeded;
    }
    if let Some(marker) = find_marker::<InputMarker>(error) {
        // 末尾欠けは壊れたパッケージとして扱い、それ以外の転送失敗は `Input` とする。
        if marker.0.kind() == io::ErrorKind::UnexpectedEof {
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
struct Slot {
    has_dir: bool,
    pathname: Option<Vec<u8>>,
    has_asset: bool,
    asset_size: u64,
    has_meta: bool,
    meta_size: u64,
    has_preview: bool,
}

/// 一時退避ファイルの名前を組み立てる。GUIDは小文字hexに統一済みのため安全。
fn staging_name(guid: &str, suffix: &str) -> String {
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
struct Plan {
    entries: Vec<Entry>,
    files: Vec<(PathBuf, PathBuf)>,
    dirs: Vec<PathBuf>,
    output_bytes: u64,
}

/// 読み取りから出力までの本体。出力先の後始末要否は [`Failure`] で返す。
fn run<R: Read>(
    reader: R,
    destination: &Path,
    staging: &Path,
    options: &UnpackOptions,
) -> Result<UnpackReport, Failure> {
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
            collect_entry(
                &mut entry,
                &mut slots,
                staging,
                options,
                &mut pathname_total,
            )?;
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
    let plan = validate_and_plan(slots)?;
    create_output(destination, staging, &plan, options)?;
    Ok(UnpackReport {
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
    })
}

/// tar一覧開始やエントリ取得の読み取り失敗を分類する。
fn invalid_package_read(error: io::Error, context: &str) -> Failure {
    let kind = classify_read(&error);
    let detail = format!("{}: {}", context, escape_diag(&error.to_string()));
    Failure::new(UnpackError::new(kind, detail).with_source(error))
}

/// 1エントリを検証して退避する。内容の対応付けは全件読み終えてから行う。
fn collect_entry<R2: Read>(
    entry: &mut tar::Entry<'_, R2>,
    slots: &mut HashMap<String, Slot>,
    staging: &Path,
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
        slot.asset_size = spill(entry, &guid, &raw_path, staging, ".asset")?;
        slot.has_asset = true;
    } else if member == b"asset.meta" {
        if slot.has_meta {
            return Err(duplicate_member(&guid, &raw_path, "asset.meta"));
        }
        slot.meta_size = spill(entry, &guid, &raw_path, staging, ".asset.meta")?;
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

/// `asset` と `asset.meta` を一時領域へ退避し、バイト数を返す。
fn spill<R2: Read>(
    entry: &mut tar::Entry<'_, R2>,
    guid: &str,
    raw_path: &[u8],
    staging: &Path,
    suffix: &str,
) -> Result<u64, Failure> {
    let target = staging.join(staging_name(guid, suffix));
    // 退避先の重複は呼び出し側の検証で先に弾くため、ここでは予期しない扱いにする。
    let mut output = File::create_new(&target).map_err(|error| {
        Failure::new(
            UnpackError::new(
                UnpackErrorKind::Storage,
                format!(
                    "一時退避先を作れない: {target:?}: {}",
                    escape_diag(&error.to_string())
                ),
            )
            .with_guid(guid.to_owned())
            .with_source(error),
        )
    })?;
    copy_stream(entry, &mut output, guid, raw_path, "退避内容を書けない")
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
fn copy_stream<R2: Read>(
    input: &mut tar::Entry<'_, R2>,
    output: &mut File,
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
fn drain_trailing<R: Read>(reader: &mut R) -> io::Result<()> {
    let mut chunk = [0u8; COPY_BUFFER_SIZE];
    loop {
        let read = reader.read(&mut chunk)?;
        if read == 0 {
            return Ok(());
        }
        if chunk[..read].iter().any(|byte| *byte != 0) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
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
    let mut files_out: Vec<(PathBuf, PathBuf)> = Vec::new();
    let mut output_bytes: u64 = 0;
    for entry in &planned {
        if entry.kind == EntryKind::File {
            files_out.push((
                PathBuf::from(&entry.pathname),
                PathBuf::from(staging_name(&entry.guid, ".asset")),
            ));
            output_bytes = output_bytes.saturating_add(entry.asset_size);
        }
        files_out.push((
            PathBuf::from(format!("{}.meta", entry.pathname)),
            PathBuf::from(staging_name(&entry.guid, ".asset.meta")),
        ));
        output_bytes = output_bytes.saturating_add(entry.meta_size);
    }
    files_out.sort_by(|a, b| a.0.cmp(&b.0));
    let mut dirs_out: Vec<PathBuf> = dirs.into_values().map(PathBuf::from).collect();
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

/// 出力先を排他的に新規作成し、検証済み計画を書き出す。
fn create_output(
    destination: &Path,
    staging: &Path,
    plan: &Plan,
    options: &UnpackOptions,
) -> Result<(), Failure> {
    // 全検証完了まで出力先を作らない。ここでの存在は競合による予期しない状態である。
    if let Err(error) = fs::create_dir(destination) {
        if error.kind() == io::ErrorKind::AlreadyExists {
            return Err(Failure::new(UnpackError::new(
                UnpackErrorKind::Conflict,
                format!("出力先が既に存在する: {destination:?}"),
            )));
        }
        return Err(Failure::new(storage_failure(
            error,
            format!("出力先を作れない: {destination:?}"),
        )));
    }
    // 計画内ディレクトリは一度だけ作り、予期しない `AlreadyExists` を成功扱いにしない。
    for dir in &plan.dirs {
        let target = destination.join(dir);
        if let Err(error) = fs::create_dir(&target) {
            return Err(Failure::with_output_created(storage_failure(
                error,
                format!("出力ディレクトリを作れない: {target:?}"),
            )));
        }
    }
    let mut buffer = [0u8; COPY_BUFFER_SIZE];
    for (index, (rel, spill)) in plan.files.iter().enumerate() {
        if let Some(failure) = injected_file_failure(options, index, rel) {
            return Err(failure);
        }
        let target = destination.join(rel);
        let source = staging.join(spill);
        let mut input = File::open(&source).map_err(|error| {
            Failure::with_output_created(storage_failure(
                error,
                format!("退避内容を開けない: {source:?}"),
            ))
        })?;
        let mut output = File::create_new(&target).map_err(|error| {
            Failure::with_output_created(storage_failure(
                error,
                format!("出力ファイルを作れない: {target:?}"),
            ))
        })?;
        loop {
            let read = input.read(&mut buffer).map_err(|error| {
                Failure::with_output_created(storage_failure(
                    error,
                    format!("退避内容を読めない: {source:?}"),
                ))
            })?;
            if read == 0 {
                break;
            }
            output.write_all(&buffer[..read]).map_err(|error| {
                Failure::with_output_created(storage_failure(
                    error,
                    format!("出力ファイルを書けない: {target:?}"),
                ))
            })?;
        }
    }
    Ok(())
}

/// テスト時の故障注入点を集約する。通常ビルドでは常に `None` を返す。
#[cfg(test)]
fn injected_file_failure(options: &UnpackOptions, index: usize, rel: &Path) -> Option<Failure> {
    // 故障注入は一度だけ失敗させ、後始末の通常経路を通す。
    if options.inject_file_error_at == Some(index) {
        return Some(Failure::with_output_created(UnpackError::new(
            UnpackErrorKind::Storage,
            format!("出力ファイルを書けない（テストの故障注入）: {rel:?}"),
        )));
    }
    None
}

/// テスト時の故障注入点を集約する。通常ビルドでは常に `None` を返す。
#[cfg(not(test))]
fn injected_file_failure(_options: &UnpackOptions, _index: usize, _rel: &Path) -> Option<Failure> {
    None
}

/// 出力先の削除。テスト時のみ故障注入で失敗させ、後始末の通常経路を通す。
fn remove_output(destination: &Path, options: &UnpackOptions) -> io::Result<()> {
    #[cfg(test)]
    if options.inject_cleanup_error {
        // 故障注入は削除を行わず、削除失敗の診断経路だけを再現する。
        // 出力先は残るため、呼び出し側のテストが後始末する。
        return Err(io::Error::other("出力先の削除に失敗（テストの故障注入）"));
    }
    // 通常ビルドでは設定を使わないが、後始末の故障注入点として引数は残す。
    let _ = options.max_entries;
    fs::remove_dir_all(destination)
}

/// ストレージ失敗を [`UnpackError`] に包む。
fn storage_failure(error: io::Error, context: String) -> UnpackError {
    UnpackError::new(
        UnpackErrorKind::Storage,
        format!("{context}: {}", escape_diag(&error.to_string())),
    )
    .with_source(error)
}

#[cfg(test)]
mod tests {
    use super::*;

    // テスト名は日本語で書く（テストは仕様・契約であるため）。

    /// 単体テスト用の最小パッケージを組み立てる。
    fn minimal_package_bytes() -> Vec<u8> {
        use flate2::Compression;
        use flate2::write::GzEncoder;
        use tar::{Builder, Header};

        let guid = "0123456789abcdef0123456789abcdef";
        let mut builder = Builder::new(Vec::new());
        let mut dir_header = Header::new_gnu();
        dir_header.set_entry_type(EntryType::Directory);
        dir_header.set_size(0);
        dir_header
            .set_path(format!("{guid}/"))
            .expect("短いパスのため成功する");
        dir_header.set_cksum();
        builder
            .append(&dir_header, io::empty())
            .expect("書き込みは成功する");
        for (member, content) in [
            ("pathname", b"Assets/min.txt".as_slice()),
            ("asset", b"hello".as_slice()),
            ("asset.meta", b"meta".as_slice()),
        ] {
            let mut header = Header::new_gnu();
            header.set_size(content.len() as u64);
            header
                .set_path(format!("{guid}/{member}"))
                .expect("短いパスのため成功する");
            header.set_cksum();
            builder
                .append(&header, content)
                .expect("書き込みは成功する");
        }
        let tar_bytes = builder.into_inner().expect("tarは完成する");
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder
            .write_all(&tar_bytes)
            .expect("gzip書き込みは成功する");
        encoder.finish().expect("gzipは完成する")
    }

    #[test]
    fn 出力ファイルの失敗では出力先を削除して残存報告しない() {
        let fixture = minimal_package_bytes();
        let parent = TempDir::new().expect("親領域は作れる");
        let destination = parent.path().join("out");
        let mut options = UnpackOptions::new();
        options.inject_file_error_at = Some(0);
        let error = unpack_to(&fixture[..], &destination, &options)
            .expect_err("注入した失敗はエラーになる");
        assert_eq!(error.kind(), UnpackErrorKind::Storage);
        // 出力開始後の失敗では自分が作った出力先を残さない。
        assert!(!destination.exists());
        assert!(error.partial_output().is_none());
    }

    #[test]
    fn 出力先の削除失敗では主原因と併記し残存を報告する() {
        let fixture = minimal_package_bytes();
        let parent = TempDir::new().expect("親領域は作れる");
        let destination = parent.path().join("out");
        let mut options = UnpackOptions::new();
        options.inject_file_error_at = Some(0);
        options.inject_cleanup_error = true;
        let error = unpack_to(&fixture[..], &destination, &options)
            .expect_err("注入した失敗はエラーになる");
        assert_eq!(error.kind(), UnpackErrorKind::Storage);
        // 削除失敗のため出力先は残り、主原因と併記される。
        assert!(destination.exists());
        // macOSでは一時領域がシンボリックリンク配下のため、正規化して比べる。
        let expected = fs::canonicalize(&destination).expect("正規化は成功する");
        assert_eq!(error.partial_output(), Some(expected.as_path()));
        let message = format!("{error}");
        assert!(message.contains("出力ファイルを書けない"));
        assert!(message.contains("出力先の削除に失敗"));
        // 故障注入で残した出力先はテスト側で片付ける。
        fs::remove_dir_all(&destination).expect("後始末は成功する");
    }

    #[test]
    fn 一時領域の削除失敗では出力先を削除して残存報告しない() {
        let fixture = minimal_package_bytes();
        let parent = TempDir::new().expect("親領域は作れる");
        let destination = parent.path().join("out");
        let mut options = UnpackOptions::new();
        options.inject_staging_cleanup_error = true;
        let error = unpack_to(&fixture[..], &destination, &options)
            .expect_err("注入した失敗はエラーになる");
        assert_eq!(error.kind(), UnpackErrorKind::Storage);
        // 成功した展開の出力先も自分が作ったものであるため、削除して残さない。
        assert!(!destination.exists());
        assert!(error.partial_output().is_none());
        // 一時領域の削除失敗自体は残存として報告する。
        assert!(error.partial_temp_dir().is_some());
        let message = format!("{error}");
        assert!(message.contains("一時領域を削除できない"));
        assert!(!message.contains("出力先の削除に失敗"));
    }

    #[test]
    fn 一時領域と出力先の削除失敗では両方の残存を報告する() {
        let fixture = minimal_package_bytes();
        let parent = TempDir::new().expect("親領域は作れる");
        let destination = parent.path().join("out");
        let mut options = UnpackOptions::new();
        options.inject_staging_cleanup_error = true;
        options.inject_cleanup_error = true;
        let error = unpack_to(&fixture[..], &destination, &options)
            .expect_err("注入した失敗はエラーになる");
        assert_eq!(error.kind(), UnpackErrorKind::Storage);
        // 出力先の削除にも失敗したときだけ残存を報告する。
        assert!(destination.exists());
        // macOSでは一時領域がシンボリックリンク配下のため、正規化して比べる。
        let expected = fs::canonicalize(&destination).expect("正規化は成功する");
        assert_eq!(error.partial_output(), Some(expected.as_path()));
        assert!(error.partial_temp_dir().is_some());
        let message = format!("{error}");
        assert!(message.contains("一時領域を削除できない"));
        assert!(message.contains("出力先の削除に失敗"));
        // 故障注入で残した出力先はテスト側で片付ける。
        fs::remove_dir_all(&destination).expect("後始末は成功する");
    }

    #[test]
    fn 診断の可視エスケープは生の制御文字を残さない() {
        // 改行・復帰・タブ・NUL・ESC・DEL・方向制御が可視化され、日本語は保つ。
        let escaped = super::super::escape_diag("a\nb\rc\td\0e\x1bf\x7fg\u{202e}hあ");
        assert_eq!(escaped, "a\\nb\\rc\\td\\0e\\x1bf\\x7fg\\u{202e}hあ");
        // 逃がし済みの文面への再適用は変えない。
        assert_eq!(super::super::escape_diag(&escaped), escaped);
    }

    #[test]
    fn 不可視書式文字も可視エスケープし再適用は変えない() {
        // U+061C・U+2060・U+2028・U+2029 などは `is_control()` が偽になるため、
        // 制御文字だけの検査では素通りして生のUTF-8が残る。回帰として残す。
        // 対象の各文字が `\u{...}` 形になることを範囲ごとに確かめる。
        let cases = [
            ('\u{ad}', "\\u{ad}"),
            ('\u{61c}', "\\u{61c}"),
            ('\u{180e}', "\\u{180e}"),
            ('\u{200b}', "\\u{200b}"),
            ('\u{200c}', "\\u{200c}"),
            ('\u{200d}', "\\u{200d}"),
            ('\u{200e}', "\\u{200e}"),
            ('\u{200f}', "\\u{200f}"),
            ('\u{2028}', "\\u{2028}"),
            ('\u{2029}', "\\u{2029}"),
            ('\u{202a}', "\\u{202a}"),
            ('\u{202e}', "\\u{202e}"),
            ('\u{2060}', "\\u{2060}"),
            ('\u{2064}', "\\u{2064}"),
            ('\u{2066}', "\\u{2066}"),
            ('\u{206a}', "\\u{206a}"),
            ('\u{206f}', "\\u{206f}"),
            ('\u{feff}', "\\u{feff}"),
            ('\u{fff9}', "\\u{fff9}"),
            ('\u{fffb}', "\\u{fffb}"),
            ('\u{e0001}', "\\u{e0001}"),
            ('\u{e0020}', "\\u{e0020}"),
            ('\u{e007f}', "\\u{e007f}"),
        ];
        for (raw, expected) in cases {
            // `is_control()` だけでは捉えられないことが素通り不具合の原因である。
            assert!(
                !raw.is_control(),
                "U+{:04X} は制御文字検査では捉えられない",
                raw as u32
            );
            let escaped = super::super::escape_diag(&raw.to_string());
            assert_eq!(escaped, expected);
            // 逃がし済みの文面への再適用は変えない（冪等）。
            assert_eq!(super::super::escape_diag(&escaped), escaped);
        }
        // 代表の不可視文字が `\u{...}` 形になり、日本語は保つ。
        let escaped = super::super::escape_diag("a\u{61c}b\u{2060}c\u{2028}d\u{2029}eあ");
        assert_eq!(escaped, "a\\u{61c}b\\u{2060}c\\u{2028}d\\u{2029}eあ");
        // 文字の含有検査はそのUTF-8バイト列の含有検査と等しく、生バイト残留の有無になる。
        for c in ['\u{61c}', '\u{2060}', '\u{2028}', '\u{2029}'] {
            assert!(!escaped.contains(c), "生の不可視文字が残らない");
        }
        // 逃がし済みの文面への再適用は変えない（冪等）。
        assert_eq!(super::super::escape_diag(&escaped), escaped);
    }

    #[test]
    fn 不可視文字を含む失敗表示は生のバイトを残さない() {
        // `Display` は生成時の逃がしに加えて表示時にも逃がし、未逃がしの混入を防ぐ。
        // U+061C等は `is_control()` が偽のため制御文字検査だけでは残る。回帰として残す。
        let raw = "h\u{61c}e\u{2060}l\u{2028}l\u{2029}o";
        let error = UnpackError::new(UnpackErrorKind::InvalidPackage, format!("不正:{raw}"))
            .with_pathname(raw.to_owned());
        let shown = format!("{error}");
        for c in ['\u{61c}', '\u{2060}', '\u{2028}', '\u{2029}'] {
            assert!(!shown.contains(c), "Displayに生の不可視文字が残らない");
        }
        // 診断の素は保持し、表示だけが逃がす。
        assert_eq!(error.pathname(), Some(raw));
        // 逃がし済みの表示への再適用は変えない（冪等）。
        assert_eq!(super::super::escape_diag(&shown), shown);
    }
}
