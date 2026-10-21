//! CLIの結合テスト。バイナリを実行して終了コードと副作用を確認する。
//!
//! 小さなパッケージはテスト内で生成する。

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use flate2::Compression;
use flate2::write::GzEncoder;

// テスト名は日本語で書く（テストは仕様・契約であるため）.

/// テスト用の最小tarブロックを組み立てる。
fn raw_block(name: &[u8], typeflag: u8, size: u64) -> [u8; 512] {
    let mut block = [0u8; 512];
    let name_len = name.len().min(100);
    block[..name_len].copy_from_slice(&name[..name_len]);
    block[100..108].copy_from_slice(b"0000777\0");
    block[108..116].copy_from_slice(b"0000000\0");
    block[116..124].copy_from_slice(b"0000000\0");
    let size_field = format!("{size:011o}\0");
    block[124..136].copy_from_slice(size_field.as_bytes());
    block[136..148].copy_from_slice(b"00000000000\0");
    block[148..156].copy_from_slice(b"        ");
    block[156] = typeflag;
    block[257..262].copy_from_slice(b"ustar");
    block[263..265].copy_from_slice(b"00");
    let sum: u32 = block.iter().map(|byte| *byte as u32).sum();
    let cksum_field = format!("{sum:06o}\0 ");
    block[148..156].copy_from_slice(cksum_field.as_bytes());
    block
}

/// ファイル1件とフォルダ1件の `.unitypackage` を生成する。
fn tiny_package() -> Vec<u8> {
    let file_guid = "0123456789abcdef0123456789abcdef";
    let dir_guid = "fedcba9876543210fedcba9876543210";
    let mut tar = Vec::new();
    for (path, content) in [
        (format!("{file_guid}/pathname"), b"Assets/a.txt".as_slice()),
        (format!("{file_guid}/asset"), b"hello".as_slice()),
        (format!("{file_guid}/asset.meta"), b"meta-a".as_slice()),
        (format!("{dir_guid}/pathname"), b"Assets/Empty".as_slice()),
        (format!("{dir_guid}/asset.meta"), b"meta-empty".as_slice()),
    ] {
        tar.extend_from_slice(&raw_block(path.as_bytes(), b'0', content.len() as u64));
        tar.extend_from_slice(content);
        tar.extend_from_slice(&vec![0u8; (512 - content.len() % 512) % 512]);
    }
    tar.extend_from_slice(&raw_block(format!("{dir_guid}/").as_bytes(), b'5', 0));
    tar.extend_from_slice(&[0u8; 1024]);
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&tar).expect("gzip書き込みは成功する");
    encoder.finish().expect("gzipは完成する")
}

/// テスト用の一時作業場所を作り、入力ファイルを置く。
fn setup(input_name: &str) -> (tempfile::TempDir, PathBuf) {
    let work = tempfile::TempDir::new().expect("作業場所は作れる");
    let input = work.path().join(input_name);
    fs::write(&input, tiny_package()).expect("入力の準備は成功する");
    (work, input)
}

/// ビルド済みバイナリの `unpack` を実行する。
fn run(package: &Path, cwd: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_vrctom"))
        .args(["unpack".as_ref(), package])
        .current_dir(cwd)
        .output()
        .expect("バイナリは実行できる")
}

#[test]
fn 既定の出力先に展開できる() {
    let (work, input) = setup("Tiny.unitypackage");
    let before = fs::read(&input).expect("入力は読める");
    let output = run(input.as_path(), work.path());
    assert_eq!(output.status.code(), Some(0));
    // 標準出力は空で、要約は標準エラーに出る。
    assert!(output.stdout.is_empty());
    assert!(!output.stderr.is_empty());
    let destination = work.path().join("Tiny-unpacked");
    assert_eq!(
        fs::read(destination.join("Assets/a.txt")).expect("出力は読める"),
        b"hello"
    );
    assert!(destination.join("Assets/Empty").is_dir());
    // 元ファイルは変更されない。
    assert_eq!(fs::read(&input).expect("入力は読める"), before);
}

#[test]
fn 短いo選択肢と長いoutput選択肢で指定した出力先に展開できる() {
    for flag in ["-o", "--output"] {
        let (work, input) = setup("Tiny.unitypackage");
        let destination = work.path().join(format!("custom-{flag}"));
        let flag_owned = flag.to_owned();
        let output = Command::new(env!("CARGO_BIN_EXE_vrctom"))
            .args([
                "unpack".as_ref(),
                input.as_path(),
                flag_owned.as_ref(),
                destination.as_path(),
            ])
            .current_dir(work.path())
            .output()
            .expect("バイナリは実行できる");
        assert_eq!(output.status.code(), Some(0), "flag: {flag}");
        assert!(output.stdout.is_empty());
        assert_eq!(
            fs::read(destination.join("Assets/a.txt")).expect("出力は読める"),
            b"hello"
        );
    }
}

#[test]
fn 存在しない入力は終了コード1になる() {
    let work = tempfile::TempDir::new().expect("作業場所は作れる");
    let missing = work.path().join("存在しない.unitypackage");
    let output = run(missing.as_path(), work.path());
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(!output.stderr.is_empty());
}

#[test]
fn 既存の出力先は終了コード1になる() {
    let (work, input) = setup("Tiny.unitypackage");
    // 既定の出力先を先に作っておく。
    let first = run(input.as_path(), work.path());
    assert_eq!(first.status.code(), Some(0));
    let second = run(input.as_path(), work.path());
    assert_eq!(second.status.code(), Some(1));
    assert!(second.stdout.is_empty());
    assert!(!second.stderr.is_empty());
}

#[test]
fn 悪意あるヘッダ名でも標準エラーに生の制御文字が出ない() {
    // 改行・ESCを含むヘッダ名で失敗させ、標準エラーが偽装行を作らないことを確かめる。
    let file_guid = "0123456789abcdef0123456789abcdef";
    let evil_guid = "fedcba9876543210fedcba9876543210";
    let mut tar = Vec::new();
    for (path, content) in [
        (format!("{file_guid}/pathname"), b"Assets/a.txt".as_slice()),
        (format!("{file_guid}/asset"), b"hello".as_slice()),
        (format!("{file_guid}/asset.meta"), b"meta-a".as_slice()),
    ] {
        tar.extend_from_slice(&raw_block(path.as_bytes(), b'0', content.len() as u64));
        tar.extend_from_slice(content);
        tar.extend_from_slice(&vec![0u8; (512 - content.len() % 512) % 512]);
    }
    // 未対応メンバーとして失敗させる。診断文面に制御文字が混ざる。
    let evil = format!("{evil_guid}/bad\n\x1b[2JINJECTED");
    let content = b"x";
    tar.extend_from_slice(&raw_block(evil.as_bytes(), b'0', content.len() as u64));
    tar.extend_from_slice(content);
    tar.extend_from_slice(&vec![0u8; (512 - content.len() % 512) % 512]);
    tar.extend_from_slice(&[0u8; 1024]);
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&tar).expect("gzip書き込みは成功する");
    let pkg = encoder.finish().expect("gzipは完成する");

    let work = tempfile::TempDir::new().expect("作業場所は作れる");
    let input = work.path().join("Evil.unitypackage");
    fs::write(&input, pkg).expect("入力の準備は成功する");
    let output = run(input.as_path(), work.path());
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    // 標準エラーに生のESC・NULを出さない。
    assert!(!output.stderr.contains(&0x1b), "ESCをそのまま出さない");
    assert!(!output.stderr.contains(&0x00), "NULをそのまま出さない");
    let stderr = String::from_utf8_lossy(&output.stderr);
    // 偽装行を作らず、改行は可視化して同じ行に留める。
    assert!(
        !stderr
            .lines()
            .any(|line| line.contains("INJECTED") && !line.contains("\\n")),
        "偽装行を作らない: {stderr}"
    );
    assert!(stderr.contains("INJECTED"), "診断自体は出す");

    // 原因を持つ失敗でも `原因:` 行に生の制御文字を出さない。
    let corrupt = work.path().join("Corrupt.unitypackage");
    fs::write(&corrupt, b"not a gzip stream").expect("入力の準備は成功する");
    let output = run(corrupt.as_path(), work.path());
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(!output.stderr.contains(&0x1b), "ESCをそのまま出さない");
    assert!(!output.stderr.contains(&0x00), "NULをそのまま出さない");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("原因:"), "原因を表示する: {stderr}");
    for line in stderr.lines() {
        assert!(
            !line.chars().any(|c| c.is_control()),
            "標準エラーに生の制御文字を出さない"
        );
    }
}

#[test]
fn 引数不足と未知の選択肢は終了コード2になる() {
    let work = tempfile::TempDir::new().expect("作業場所は作れる");
    // サブコマンド無し。
    let bare = Command::new(env!("CARGO_BIN_EXE_vrctom"))
        .current_dir(work.path())
        .output()
        .expect("バイナリは実行できる");
    assert_eq!(bare.status.code(), Some(2));
    // 未知の選択肢。
    let unknown = Command::new(env!("CARGO_BIN_EXE_vrctom"))
        .args(["unpack", "--unknown-flag"])
        .current_dir(work.path())
        .output()
        .expect("バイナリは実行できる");
    assert_eq!(unknown.status.code(), Some(2));
}

#[test]
fn 未知の選択肢に不可視書式文字が混ざっても標準エラーに生で残さない() {
    // clapの引数エラー経路が可視エスケープを通ることを確かめる。
    // U+061C/U+2060/U+2028/U+2029 は `char::is_control()` が偽になるため、
    // 逃がさないと終了コード2のstderrに生UTF-8が残り表示偽装に使える。
    let work = tempfile::TempDir::new().expect("作業場所は作れる");
    let evil = "--unknown-\u{61c}-\u{2060}-\u{2028}-\u{2029}-flag".to_owned();
    let output = Command::new(env!("CARGO_BIN_EXE_vrctom"))
        .args(["unpack", &evil])
        .current_dir(work.path())
        .output()
        .expect("バイナリは実行できる");
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    for raw in ['\u{61c}', '\u{2060}', '\u{2028}', '\u{2029}'] {
        let mut buf = [0u8; 4];
        let needle = raw.encode_utf8(&mut buf);
        assert!(
            !output
                .stderr
                .windows(needle.len())
                .any(|window| window == needle.as_bytes()),
            "生の U+{:04X} をstderrに出さない",
            raw as u32
        );
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    // 可視エスケープされた形で診断自体は出す。
    for escaped in ["\\u{61c}", "\\u{2060}", "\\u{2028}", "\\u{2029}"] {
        assert!(
            stderr.contains(escaped),
            "可視化して出す {escaped}: {stderr}"
        );
    }
    // 改行も逃がすため偽装行を作らず1行にまとまる。
    assert_eq!(stderr.trim_end().lines().count(), 1, "{stderr}");
}

#[test]
fn helpとversionは終了コード0で通常表示になる() {
    // 引数エラー経路の変更で `--help`/`--version` の挙動を壊さないことを確かめる。
    let work = tempfile::TempDir::new().expect("作業場所は作れる");
    for args in [vec!["--help"], vec!["--version"], vec!["unpack", "--help"]] {
        let output = Command::new(env!("CARGO_BIN_EXE_vrctom"))
            .args(&args)
            .current_dir(work.path())
            .output()
            .expect("バイナリは実行できる");
        assert_eq!(output.status.code(), Some(0), "args: {args:?}");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(!stdout.is_empty(), "通常の表示を出す args: {args:?}");
    }
    let help = Command::new(env!("CARGO_BIN_EXE_vrctom"))
        .args(["--help"])
        .current_dir(work.path())
        .output()
        .expect("バイナリは実行できる");
    let help_text = String::from_utf8_lossy(&help.stdout);
    assert!(
        help_text.contains("unpack"),
        "使い方にサブコマンドが出る: {help_text}"
    );
}
