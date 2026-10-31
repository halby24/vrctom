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

/// GLBのJSON塊に指定の断片が含まれるかどうかを返す。
fn json塊に含まれるか(バイト列: &[u8], 断片: &str) -> bool {
    if バイト列.len() < 20 {
        return false;
    }
    let json長 =
        u32::from_le_bytes(バイト列[12..16].try_into().expect("塊長は4バイトである")) as usize;
    let json = &バイト列[20..20 + json長];
    let json終端 = json
        .iter()
        .rposition(|b| *b != b' ')
        .map(|位置| 位置 + 1)
        .unwrap_or(0);
    String::from_utf8_lossy(&json[..json終端]).contains(断片)
}

#[test]
fn 資材指定と自動検出で材質入りvrmを書き出せる() {
    let fixture = match std::env::var("VRCTOM_FBX_FIXTURE") {
        Ok(value) if !value.trim().is_empty() => value,
        _ => {
            eprintln!("skip: VRCTOM_FBX_FIXTURE 未設定のため読み飛ばす");
            return;
        }
    };
    let fbx = Path::new(&fixture);
    let mut 資材親 = None;
    for 祖先 in fbx.ancestors() {
        if 祖先.join("Assets").is_dir() {
            資材親 = Some(祖先.to_path_buf());
            break;
        }
    }
    let Some(資材親) = 資材親 else {
        eprintln!("skip: 実fixtureの祖先にAssetsが無い");
        return;
    };
    // 明示指定では `Assets` 自体を渡し、親を使う分岐を通す。
    let 資材 = 資材親.join("Assets");
    for (名, 資材指定) in [
        ("明示", Some(資材.to_string_lossy().into_owned())),
        ("自動", None),
    ] {
        let work = tempfile::TempDir::new().expect("作業場所は作れる");
        let destination = work.path().join("out.vrm");
        let destination_arg = destination.to_string_lossy().into_owned();
        let mut 引数 = vec!["export", &fixture, "-o", &destination_arg];
        let 資材所有;
        if let Some(指定) = &資材指定 {
            資材所有 = 指定.clone();
            引数.push("--assets");
            引数.push(&資材所有);
        }
        let output = run(&引数, work.path());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(0), "{名}: stderr: {stderr}");
        assert!(output.stdout.is_empty(), "{名}: 標準出力は空である");
        assert!(stderr.contains("材質"), "{名}: 材質数を出す: {stderr}");
        let bytes = fs::read(&destination).expect("出力は読める");
        assert_eq!(&bytes[..4], b"glTF", "{名}: GLBである");
        assert!(json塊に含まれるか(&bytes, "\"images\""), "{名}: 画像が入る");
    }
}

#[test]
fn 存在しない資材指定は終了コード1になる() {
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
    let missing = work.path().join("存在しない資材");
    let missing_arg = missing.to_string_lossy().into_owned();
    let output = run(
        &[
            "export",
            &fixture,
            "-o",
            &destination_arg,
            "--assets",
            &missing_arg,
        ],
        work.path(),
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(!output.stderr.is_empty());
}

#[test]
fn 相対パス入力でも自動検出で材質入りvrmを書き出せる() {
    // プロジェクトルートをcwdにしてFBXを相対パスで渡す使い方で、
    // 空パスの祖先をカレントとして解決できることを確かめる。
    let fixture = match std::env::var("VRCTOM_FBX_FIXTURE") {
        Ok(value) if !value.trim().is_empty() => value,
        _ => {
            eprintln!("skip: VRCTOM_FBX_FIXTURE 未設定のため読み飛ばす");
            return;
        }
    };
    let fbx = Path::new(&fixture);
    let mut 計画親 = None;
    for 祖先 in fbx.ancestors() {
        if 祖先.join("Assets").is_dir() {
            計画親 = Some(祖先.to_path_buf());
            break;
        }
    }
    let Some(計画親) = 計画親 else {
        eprintln!("skip: 実fixtureの祖先にAssetsが無い");
        return;
    };
    // 区切りはOSに任せ、接頭辞の除去でルートからの相対パスを作る。
    let 相対 = match fbx.strip_prefix(&計画親) {
        Ok(相対) => 相対.to_string_lossy().into_owned(),
        Err(_) => {
            eprintln!("skip: 実fixtureが計画親の配下に無い");
            return;
        }
    };
    // 出力先は一時場所の絶対パスにし、プロジェクト直下を汚さない。
    // cwdは計画親のままにし、相対パス入力の解決を確かめる。
    let work = tempfile::TempDir::new().expect("作業場所は作れる");
    let destination = work.path().join("out.vrm");
    let destination_arg = destination.to_string_lossy().into_owned();
    let output = run(&["export", &相対, "-o", &destination_arg], &計画親);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(0), "stderr: {stderr}");
    assert!(output.stdout.is_empty(), "標準出力は空である");
    assert!(stderr.contains("材質"), "材質数を出す: {stderr}");
    let bytes = fs::read(&destination).expect("出力は読める");
    assert_eq!(&bytes[..4], b"glTF", "GLBである");
    assert!(json塊に含まれるか(&bytes, "\"images\""), "画像が入る");
}

#[test]
fn 相対の資材指定でも材質入りvrmを書き出せる() {
    // プロジェクトルートをcwdにして `--assets Assets` と相対で渡す使い方で、
    // 末尾 `Assets` の親が空パスになる場合でもカレントを走査元として
    // 材質を読み込めることを確かめる。
    let fixture = match std::env::var("VRCTOM_FBX_FIXTURE") {
        Ok(value) if !value.trim().is_empty() => value,
        _ => {
            eprintln!("skip: VRCTOM_FBX_FIXTURE 未設定のため読み飛ばす");
            return;
        }
    };
    let fbx = Path::new(&fixture);
    let mut 計画親 = None;
    for 祖先 in fbx.ancestors() {
        if 祖先.join("Assets").is_dir() {
            計画親 = Some(祖先.to_path_buf());
            break;
        }
    }
    let Some(計画親) = 計画親 else {
        eprintln!("skip: 実fixtureの祖先にAssetsが無い");
        return;
    };
    let 相対 = match fbx.strip_prefix(&計画親) {
        Ok(相対) => 相対.to_string_lossy().into_owned(),
        Err(_) => {
            eprintln!("skip: 実fixtureが計画親の配下に無い");
            return;
        }
    };
    let work = tempfile::TempDir::new().expect("作業場所は作れる");
    let destination = work.path().join("out.vrm");
    let destination_arg = destination.to_string_lossy().into_owned();
    let output = run(
        &[
            "export",
            &相対,
            "-o",
            &destination_arg,
            "--assets",
            "Assets",
        ],
        &計画親,
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(0), "stderr: {stderr}");
    assert!(output.stdout.is_empty(), "標準出力は空である");
    assert!(stderr.contains("材質"), "材質数を出す: {stderr}");
    let bytes = fs::read(&destination).expect("出力は読める");
    assert_eq!(&bytes[..4], b"glTF", "GLBである");
    assert!(json塊に含まれるか(&bytes, "\"images\""), "画像が入る");
}

#[test]
fn physics指定でスプリング入りvrmを書き出せる() {
    let (fixture, sidecar) = match (
        std::env::var("VRCTOM_FBX_FIXTURE"),
        std::env::var("VRCTOM_PHYSICS_SIDECAR"),
    ) {
        (Ok(fbx), Ok(sidecar)) if !fbx.trim().is_empty() && !sidecar.trim().is_empty() => {
            (fbx, sidecar)
        }
        _ => {
            eprintln!("skip: VRCTOM_FBX_FIXTUREまたはVRCTOM_PHYSICS_SIDECAR未設定のため読み飛ばす");
            return;
        }
    };
    let work = tempfile::TempDir::new().expect("作業場所は作れる");
    let destination = work.path().join("out.vrm");
    let destination_arg = destination.to_string_lossy().into_owned();
    let output = run(
        &[
            "export",
            &fixture,
            "-o",
            &destination_arg,
            "--physics",
            &sidecar,
        ],
        work.path(),
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(0), "stderr: {stderr}");
    assert!(output.stdout.is_empty(), "標準出力は空である");
    assert!(
        stderr.contains("スプリング"),
        "スプリング件数を出す: {stderr}"
    );
    assert!(
        stderr.contains("T-ポーズ53件"),
        "T-ポーズ適用53件を出す: {stderr}"
    );
    let bytes = fs::read(&destination).expect("出力は読める");
    assert!(
        json塊に含まれるか(&bytes, "\"VRMC_springBone\""),
        "spring拡張が入る"
    );
}

#[test]
fn physics指定の失敗は終了コード1になり出力先を作らない() {
    let fixture = match std::env::var("VRCTOM_FBX_FIXTURE") {
        Ok(value) if !value.trim().is_empty() => value,
        _ => {
            eprintln!("skip: VRCTOM_FBX_FIXTURE 未設定のため読み飛ばす");
            return;
        }
    };
    // 存在しないsidecarは解決前に失敗する。
    let work = tempfile::TempDir::new().expect("作業場所は作れる");
    let destination = work.path().join("out.vrm");
    let destination_arg = destination.to_string_lossy().into_owned();
    let missing = work.path().join("存在しない.json");
    let missing_arg = missing.to_string_lossy().into_owned();
    let output = run(
        &[
            "export",
            &fixture,
            "-o",
            &destination_arg,
            "--physics",
            &missing_arg,
        ],
        work.path(),
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(!output.stderr.is_empty());
    assert!(!destination.exists(), "失敗時は出力先を作らない");
}
