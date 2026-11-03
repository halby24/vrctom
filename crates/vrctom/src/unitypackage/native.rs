//! UnityPackage展開の Native 実装。
//!
//! 公開契約の入口は [`crate::unitypackage`] を参照。
//!
//! tar走査・検証は共通核（[`super`](crate::unitypackage) の `core`）に任せ、
//! ここでは一時領域への退避と出力先の排他的新規作成・後始末だけを受け持つ。
//! `asset` が `pathname` より先に現れても動く。内容は固定バッファで回し、
//! メモリ使用量を内容サイズに比例させない。
//!
//! 限界として、`asset` を持たない正常フォルダと `asset` の欠けた壊れた
//! ファイルはtar構造だけでは区別できない。`asset.meta` の意味解析や
//! meta内GUIDとの照合は行わない。

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::Path;

use tempfile::TempDir;

use super::core::{
    Failure, Plan, StagingSink, copy_entry_to_writer, scan_and_validate, staging_name,
};
use super::{EntryKind, UnpackError, UnpackErrorKind, UnpackOptions, UnpackReport, escape_diag};

/// 出力コピーに使う固定バッファの大きさ。
///
/// 資産の大きさに依らずこの大きさで回すことで、メモリ使用量を内容サイズに
/// 比例させない。値は性能上の目安であり、正確さには影響しない。
/// tar走査側の大きさは共通核が持つ。
const COPY_BUFFER_SIZE: usize = 64 * 1024;

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

/// 一時領域への退避。共通核の差し替え点のnative側である。
struct FileSink<'a> {
    staging: &'a Path,
}

impl StagingSink for FileSink<'_> {
    fn spill_asset<R2: Read>(
        &mut self,
        entry: &mut tar::Entry<'_, R2>,
        guid: &str,
        raw_path: &[u8],
    ) -> Result<u64, Failure> {
        self.spill(entry, guid, raw_path, ".asset")
    }

    fn spill_meta<R2: Read>(
        &mut self,
        entry: &mut tar::Entry<'_, R2>,
        guid: &str,
        raw_path: &[u8],
    ) -> Result<u64, Failure> {
        self.spill(entry, guid, raw_path, ".asset.meta")
    }
}

impl FileSink<'_> {
    /// `asset` と `asset.meta` を一時領域へ退避し、バイト数を返す。
    fn spill<R2: Read>(
        &self,
        entry: &mut tar::Entry<'_, R2>,
        guid: &str,
        raw_path: &[u8],
        suffix: &str,
    ) -> Result<u64, Failure> {
        let target = self.staging.join(staging_name(guid, suffix));
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
        copy_entry_to_writer(entry, &mut output, guid, raw_path, "退避内容を書けない")
    }
}

/// 読み取りから出力までの本体。出力先の後始末要否は [`Failure`] で返す。
///
/// tar走査・検証は共通核で行い、ここでは検証済み計画の書き出しだけを行う。
/// 全検証完了まで出力先を作らない保証は共通核と [`create_output`] で保つ。
fn run<R: Read>(
    reader: R,
    destination: &Path,
    staging: &Path,
    options: &UnpackOptions,
) -> Result<UnpackReport, Failure> {
    let mut sink = FileSink { staging };
    let plan = scan_and_validate(reader, &mut sink, options)?;
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
    use tar::EntryType;

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
