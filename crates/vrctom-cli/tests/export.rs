//! `export` サブコマンドの結合テスト。バイナリを実行して終了コードと副作用を確認する。

use std::fs;
use std::path::Path;
use std::process::Command;

// テスト名は日本語で書く（テストは仕様・契約であるため）。

/// ビルド済みバイナリに引数を渡して実行する。
fn run(args: &[&str], cwd: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_vrctom"))
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("バイナリは実行できる")
}

#[test]
fn 引数不足と未知の選択肢は終了コード2になる() {
    let work = tempfile::TempDir::new().expect("作業場所は作れる");
    // 入力無し。
    let bare = run(&["export"], work.path());
    assert_eq!(bare.status.code(), Some(2));
    assert!(bare.stdout.is_empty());
    assert!(!bare.stderr.is_empty());
    // 未知の選択肢。
    let unknown = run(&["export", "model.fbx", "--unknown-flag"], work.path());
    assert_eq!(unknown.status.code(), Some(2));
    assert!(unknown.stdout.is_empty());
    assert!(!unknown.stderr.is_empty());
}

#[test]
fn 存在しない入力は終了コード1になる() {
    let work = tempfile::TempDir::new().expect("作業場所は作れる");
    let missing = work.path().join("存在しない.fbx");
    let missing_arg = missing.to_string_lossy().into_owned();
    let output = run(&["export", &missing_arg, "-o", "out.vrm"], work.path());
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(!output.stderr.is_empty());
}

#[test]
fn 既存の出力先は終了コード1になり内容が変わらない() {
    let work = tempfile::TempDir::new().expect("作業場所は作れる");
    let input = work.path().join("dummy.fbx");
    fs::write(&input, b"not a fbx stream").expect("入力の準備は成功する");
    let destination = work.path().join("out.vrm");
    fs::write(&destination, b"sentinel").expect("出力先の準備は成功する");
    let input_arg = input.to_string_lossy().into_owned();
    let output_arg = destination.to_string_lossy().into_owned();
    let output = run(&["export", &input_arg, "-o", &output_arg], work.path());
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(!output.stderr.is_empty());
    // 既存ファイルは上書きしない。
    assert_eq!(fs::read(&destination).expect("出力先は読める"), b"sentinel");
}

#[test]
fn 実fbxがあればvrmを書き出せる() {
    let fixture = match std::env::var("VRCTOM_FBX_FIXTURE") {
        Ok(value) if !value.trim().is_empty() => value,
        _ => {
            eprintln!("skip: VRCTOM_FBX_FIXTURE 未設定のため読み飛ばす");
            return;
        }
    };
    let work = tempfile::TempDir::new().expect("作業場所は作れる");
    let destination = work.path().join("out.vrm");
    let destination_arg = destination.to_string_lossy().into_owned();
    let output = run(&["export", &fixture, "-o", &destination_arg], work.path());
    assert_eq!(
        output.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    // 標準出力は空で、要約は標準エラーに出る。
    assert!(output.stdout.is_empty());
    let bytes = fs::read(&destination).expect("出力は読める");
    // GLBの先頭4バイトが `glTF` である。
    assert!(bytes.len() >= 4, "出力が短すぎる");
    assert_eq!(&bytes[..4], b"glTF");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("書き出しが完了した"),
        "要約を出す: {stderr}"
    );
    // バイト数を診断に出す。件数の固定値は確かめない。
    assert!(
        stderr.contains(&bytes.len().to_string()),
        "バイト数を出す: {stderr}"
    );
}
