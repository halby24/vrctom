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

/// バイト列のSHA-256を小文字hexで返す。手書きsidecarの照合値の組み立てに使う。
fn sha256十六進(バイト列: &[u8]) -> String {
    use sha2::Digest as _;
    let 摘要 = sha2::Sha256::digest(バイト列);
    let mut 出力 = String::with_capacity(64);
    for バイト in 摘要 {
        出力.push(char::from_digit(u32::from(バイト >> 4), 16).expect("hexである"));
        出力.push(char::from_digit(u32::from(バイト & 0x0f), 16).expect("hexである"));
    }
    出力
}

/// 手書きsidecarのノード1件分のJSONを作る。参照ローカルは単位姿勢である。
///
/// 合成FBXのバインドは全て単位姿勢のため、単位姿勢の参照ローカルと一致する。
fn 手書きsidecarノードjson(
    識別子: &str,
    親: Option<&str>,
    名: &str,
    出所パス: &[&str],
) -> serde_json::Value {
    serde_json::json!({
        "id": 識別子,
        "parentId": 親,
        "name": 名,
        "siblingIndex": 0,
        "scenePath": 出所パス,
        "source": {"modelId": "synth", "path": 出所パス, "assetGuid": "guid", "localId": -1},
        "referenceLocal": {
            "position": [0.0, 0.0, 0.0],
            "rotation": [0.0, 0.0, 0.0, 1.0],
            "scale": [1.0, 1.0, 1.0],
        },
    })
}

/// 手書きsidecarのhumanoidの骨1件分のJSONを作る。T-poseは単位姿勢である。
fn 手書きhumanoid骨json(骨名: &str, ノード: &str) -> serde_json::Value {
    serde_json::json!({
        "bone": 骨名,
        "nodeId": ノード,
        "tPose": {
            "position": [0.0, 0.0, 0.0],
            "rotation": [0.0, 0.0, 0.0, 1.0],
            "scale": [1.0, 1.0, 1.0],
        },
    })
}

/// exact名の必須15骨をメタの対応項目にする。名前推測と同じ対応を明示する。
fn exact名の対応項目() -> Vec<String> {
    [
        ("Hips", "Hips"),
        ("Spine", "Spine"),
        ("Head", "Head"),
        ("LeftUpperArm", "LeftUpperArm"),
        ("RightUpperArm", "RightUpperArm"),
        ("LeftLowerArm", "LeftLowerArm"),
        ("RightLowerArm", "RightLowerArm"),
        ("LeftHand", "LeftHand"),
        ("RightHand", "RightHand"),
        ("LeftUpperLeg", "LeftUpperLeg"),
        ("RightUpperLeg", "RightUpperLeg"),
        ("LeftLowerLeg", "LeftLowerLeg"),
        ("RightLowerLeg", "RightLowerLeg"),
        ("LeftFoot", "LeftFoot"),
        ("RightFoot", "RightFoot"),
    ]
    .iter()
    .map(|(ノード名, 骨名)| format!("    - boneName: {ノード名}\n      humanName: {骨名}\n"))
    .collect()
}

/// 手書きsidecarの対応不一致はT-ポーズ段階で終了コード1になり、一致すれば書き出せる。
///
/// 合成FBX・隣の `.fbx.meta`・手書きの物理sidecarだけで再現し、実fixtureに依存しない。
/// 不一致のsidecarでは終了コード1・VRM出力なし・`humanoidの対応が一致しない` を
/// 標準エラーに出す。対照として、同じsidecarの `nodeId` を正しいノードにした場合は
/// 終了コード0でVRMを書き出し、`leftHand` が `LeftHand` ノードを指す。
#[test]
fn 手書きsidecarの対応不一致は終了コード1になり一致すれば書き出せる() {
    let fbxバイト列 = exact名fbxバイト列();
    let sha = sha256十六進(&fbxバイト列);
    let work = tempfile::TempDir::new().expect("作業場所は作れる");
    let 入力 = work.path().join("model.fbx");
    fs::write(&入力, &fbxバイト列).expect("入力の準備は成功する");
    // humanoidの検出は `.fbx.meta` の対応表で行い、名前推測の揺れを避ける。
    let mut メタ名 = 入力.as_os_str().to_owned();
    メタ名.push(".meta");
    fs::write(&メタ名, 対応表meta(&exact名の対応項目())).expect("対応表の準備は成功する");
    let 入力_arg = 入力.to_string_lossy().into_owned();
    // 比較の軸は `LeftHand` であり、正しい指す先は `n:1`、ずらし先は `n:2`（Head）である。
    // 座標ゲートの対象はPhysBoneが参照する `n:0` であり、単位姿勢で一致する。
    let sidecar文 = |左手ノード: &str| {
        serde_json::json!({
            "format": "vrctom.physics",
            "schemaVersion": 1,
            "generator": {"name": "試験", "version": "0.1"},
            "capture": {
                "statePolicy": "edit-mode",
                "coordinateSpace": "unity-avatar-root-local-meters",
                "avatarRootId": "n:0",
                "rootScale": [1.0, 1.0, 1.0],
            },
            "sourceModels": [{"id": "synth", "sha256": sha}],
            "nodes": [
                手書きsidecarノードjson("n:0", None, "Hips", &["Hips"]),
                手書きsidecarノードjson("n:1", Some("n:0"), "LeftHand", &["Hips", "Spine", "LeftUpperArm", "LeftLowerArm", "LeftHand"]),
                手書きsidecarノードjson("n:2", Some("n:0"), "Head", &["Hips", "Spine", "Head"]),
            ],
            "physBones": [{"id": "pb:0", "componentNodeId": "n:0", "rootNodeId": "n:0"}],
            "colliders": [],
            "humanoid": {
                "isHuman": true,
                "bones": [
                    手書きhumanoid骨json("Hips", "n:0"),
                    手書きhumanoid骨json("LeftHand", 左手ノード),
                ],
            },
            "unsupportedFeatures": [],
        })
        .to_string()
    };
    // 不一致のsidecarはT-pose適用で失敗し、出力先を作らない。
    let 不一致先 = work.path().join("mismatch.json");
    fs::write(&不一致先, sidecar文("n:2")).expect("不一致sidecarは書ける");
    let 不一致_arg = 不一致先.to_string_lossy().into_owned();
    let 出力先 = work.path().join("out.vrm");
    let 出力先_arg = 出力先.to_string_lossy().into_owned();
    let 不一致 = run(
        &[
            "export",
            &入力_arg,
            "-o",
            &出力先_arg,
            "--physics",
            &不一致_arg,
        ],
        work.path(),
    );
    let 不一致stderr = String::from_utf8_lossy(&不一致.stderr);
    assert_eq!(不一致.status.code(), Some(1), "stderr: {不一致stderr}");
    assert!(不一致.stdout.is_empty(), "標準出力は空である");
    assert!(!出力先.exists(), "失敗時は出力先を作らない");
    assert!(
        不一致stderr.contains("humanoidの対応が一致しない"),
        "対応不一致の診断を出す: {不一致stderr}"
    );
    assert!(
        不一致stderr.contains("LeftHand"),
        "Unity骨名を出す: {不一致stderr}"
    );
    // 対照の一致sidecarは成功し、出力humanoidは解決済みノードを指す。
    let 一致先 = work.path().join("match.json");
    fs::write(&一致先, sidecar文("n:1")).expect("一致sidecarは書ける");
    let 一致_arg = 一致先.to_string_lossy().into_owned();
    let 対照先 = work.path().join("control.vrm");
    let 対照先_arg = 対照先.to_string_lossy().into_owned();
    let 対照 = run(
        &[
            "export",
            &入力_arg,
            "-o",
            &対照先_arg,
            "--physics",
            &一致_arg,
        ],
        work.path(),
    );
    let 対照stderr = String::from_utf8_lossy(&対照.stderr);
    assert_eq!(対照.status.code(), Some(0), "stderr: {対照stderr}");
    assert!(対照.stdout.is_empty(), "標準出力は空である");
    let 対照json = glbのjsonを読む(&fs::read(&対照先).expect("対照は読める"));
    assert_eq!(
        humanoidノード索引(&対照json, "leftHand"),
        ノード索引(&対照json, "LeftHand"),
        "左手首は解決済みノードを指す"
    );
}

#[test]
fn 隣のメタがあれば別名の手首でも書き出せる() {
    // 手首 `Wrist.L`/`Wrist.R`・足 `Foot.L`/`Foot.R` の命名は名前推測で
    // 対応付かない。隣の `.fbx.meta` が無いと失敗し、併用で成功することを確かめる。
    let work = tempfile::TempDir::new().expect("作業場所は作れる");
    let 入力 = work.path().join("model.fbx");
    fs::write(&入力, wol風fbxバイト列()).expect("入力の準備は成功する");
    let 入力引数 = 入力.to_string_lossy().into_owned();
    // メタ無しでは手首が無いため失敗する（受入の前提）。
    let 失敗先 = work.path().join("失敗.vrm");
    let 失敗先引数 = 失敗先.to_string_lossy().into_owned();
    let 失敗 = run(&["export", &入力引数, "-o", &失敗先引数], work.path());
    assert_eq!(
        失敗.status.code(),
        Some(1),
        "stderr: {}",
        String::from_utf8_lossy(&失敗.stderr)
    );
    assert!(失敗.stdout.is_empty(), "標準出力は空である");
    assert!(
        String::from_utf8_lossy(&失敗.stderr).contains("humanoidの対応付けに失敗した"),
        "humanoidの段階で失敗する: {}",
        String::from_utf8_lossy(&失敗.stderr)
    );
    assert!(!失敗先.exists(), "失敗時は出力先を作らない");
    // 隣の `.fbx.meta`（入力パスへ `.meta` を追加したパス）に15骨の対応表を置く。
    let mut メタ名 = 入力.as_os_str().to_owned();
    メタ名.push(".meta");
    fs::write(&メタ名, 対応表meta(&wol風の対応項目())).expect("対応表の準備は成功する");
    let 出力先 = work.path().join("out.vrm");
    let 出力先引数 = 出力先.to_string_lossy().into_owned();
    let output = run(&["export", &入力引数, "-o", &出力先引数], work.path());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(0), "stderr: {stderr}");
    assert!(output.stdout.is_empty(), "標準出力は空である");
    assert!(
        stderr.contains("書き出しが完了した"),
        "要約を出す: {stderr}"
    );
    let bytes = fs::read(&出力先).expect("出力は読める");
    assert_eq!(&bytes[..4], b"glTF", "GLBである");
    let 値 = glbのjsonを読む(&bytes);
    // メタの対応どおり手首が書かれる。
    assert_eq!(
        humanoidノード索引(&値, "leftHand"),
        ノード索引(&値, "Wrist.L"),
        "左手首はメタの対応先である"
    );
    assert_eq!(
        humanoidノード索引(&値, "rightHand"),
        ノード索引(&値, "Wrist.R"),
        "右手首はメタの対応先である"
    );
}

#[test]
fn 隣のメタが無ければ名前推測のまま書き出せる() {
    // exact名のFBXはメタ無しでも従来どおり名前推測で成功する。
    // メタの有無で既存の挙動が変わらないことを確かめる。
    let work = tempfile::TempDir::new().expect("作業場所は作れる");
    let 入力 = work.path().join("model.fbx");
    fs::write(&入力, exact名fbxバイト列()).expect("入力の準備は成功する");
    let 入力引数 = 入力.to_string_lossy().into_owned();
    let 出力先 = work.path().join("out.vrm");
    let 出力先引数 = 出力先.to_string_lossy().into_owned();
    let output = run(&["export", &入力引数, "-o", &出力先引数], work.path());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(0), "stderr: {stderr}");
    assert!(output.stdout.is_empty(), "標準出力は空である");
    assert!(
        stderr.contains("書き出しが完了した"),
        "要約を出す: {stderr}"
    );
    let bytes = fs::read(&出力先).expect("出力は読める");
    assert_eq!(&bytes[..4], b"glTF", "GLBである");
    let 値 = glbのjsonを読む(&bytes);
    assert_eq!(
        humanoidノード索引(&値, "leftHand"),
        ノード索引(&値, "LeftHand"),
        "左手首は名前推測の対応先である"
    );
}

#[test]
fn 壊れた隣のメタは終了コード1になり出力先を作らない() {
    // 単体では成功するexact名のFBXに形状不良の `.fbx.meta` を添えると失敗する。
    let work = tempfile::TempDir::new().expect("作業場所は作れる");
    let 入力 = work.path().join("model.fbx");
    fs::write(&入力, exact名fbxバイト列()).expect("入力の準備は成功する");
    let mut メタ名 = 入力.as_os_str().to_owned();
    メタ名.push(".meta");
    // `human` がシーケンスでない形状不良の対応表を置く。
    let 壊れた = "fileFormatVersion: 2\nguid: 30293896140a4d24a914010a2a8349fc\n\
        ModelImporter:\n  humanDescription:\n    human: {}\n";
    fs::write(&メタ名, 壊れた).expect("対応表の準備は成功する");
    let 入力引数 = 入力.to_string_lossy().into_owned();
    let 出力先 = work.path().join("out.vrm");
    let 出力先引数 = 出力先.to_string_lossy().into_owned();
    let output = run(&["export", &入力引数, "-o", &出力先引数], work.path());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "stderr: {stderr}");
    assert!(output.stdout.is_empty(), "標準出力は空である");
    assert!(
        stderr.contains("humanoidの対応表を読めない"),
        "対応表の段階で失敗する: {stderr}"
    );
    assert!(!出力先.exists(), "失敗時は出力先を作らない");
}

/// wol風の命名（手首 `Wrist.L`・足 `Foot.L`）の必須骨定義。
/// 名前推測では手首が対応付かず、メタ併用でのみ書き出せる。
fn wol風の定義() -> Vec<(i64, &'static str, Option<i64>)> {
    vec![
        (1, "Hips", None),
        (2, "Spine", Some(1)),
        (3, "Head", Some(2)),
        (4, "LeftUpperArm", Some(2)),
        (5, "LeftLowerArm", Some(4)),
        (6, "Wrist.L", Some(5)),
        (7, "RightUpperArm", Some(2)),
        (8, "RightLowerArm", Some(7)),
        (9, "Wrist.R", Some(8)),
        (10, "LeftUpperLeg", Some(1)),
        (11, "LeftLowerLeg", Some(10)),
        (12, "Foot.L", Some(11)),
        (13, "RightUpperLeg", Some(1)),
        (14, "RightLowerLeg", Some(13)),
        (15, "Foot.R", Some(14)),
        (16, "Index,L", Some(6)),
        (17, "Owner", Some(1)),
    ]
}

/// exact名の必須骨定義。メタ無しでも名前推測で書き出せる。
fn exact名の定義() -> Vec<(i64, &'static str, Option<i64>)> {
    vec![
        (1, "Hips", None),
        (2, "Spine", Some(1)),
        (3, "Head", Some(2)),
        (4, "LeftUpperArm", Some(2)),
        (5, "LeftLowerArm", Some(4)),
        (6, "LeftHand", Some(5)),
        (7, "RightUpperArm", Some(2)),
        (8, "RightLowerArm", Some(7)),
        (9, "RightHand", Some(8)),
        (10, "LeftUpperLeg", Some(1)),
        (11, "LeftLowerLeg", Some(10)),
        (12, "LeftFoot", Some(11)),
        (13, "RightUpperLeg", Some(1)),
        (14, "RightLowerLeg", Some(13)),
        (15, "RightFoot", Some(14)),
        (16, "Index,L", Some(6)),
        (17, "Owner", Some(1)),
    ]
}

/// wol風の必須15骨をメタの対応項目にする。
fn wol風の対応項目() -> Vec<String> {
    [
        ("Hips", "Hips"),
        ("Spine", "Spine"),
        ("Head", "Head"),
        ("LeftUpperArm", "LeftUpperArm"),
        ("RightUpperArm", "RightUpperArm"),
        ("LeftLowerArm", "LeftLowerArm"),
        ("RightLowerArm", "RightLowerArm"),
        ("Wrist.L", "LeftHand"),
        ("Wrist.R", "RightHand"),
        ("LeftUpperLeg", "LeftUpperLeg"),
        ("RightUpperLeg", "RightUpperLeg"),
        ("LeftLowerLeg", "LeftLowerLeg"),
        ("RightLowerLeg", "RightLowerLeg"),
        ("Foot.L", "LeftFoot"),
        ("Foot.R", "RightFoot"),
    ]
    .iter()
    .map(|(ノード名, 骨名)| format!("    - boneName: {ノード名}\n      humanName: {骨名}\n"))
    .collect()
}

/// human対応表を持つ `.fbx.meta` を組み立てる。
fn 対応表meta(項目ら: &[String]) -> Vec<u8> {
    let mut 本文 = String::from("  humanDescription:\n    serializedVersion: 3\n    human:\n");
    for 項目 in 項目ら {
        本文.push_str(項目);
    }
    本文.push_str("  animationType: 3\n");
    format!("fileFormatVersion: 2\nguid: 30293896140a4d24a914010a2a8349fc\nModelImporter:\n{本文}")
        .into_bytes()
}

/// GLBのJSON塊を値として読む。
fn glbのjsonを読む(バイト列: &[u8]) -> serde_json::Value {
    let json長 =
        u32::from_le_bytes(バイト列[12..16].try_into().expect("塊長は4バイトである")) as usize;
    let json = &バイト列[20..20 + json長];
    let 終端 = json
        .iter()
        .rposition(|b| *b != b' ')
        .map(|位置| 位置 + 1)
        .unwrap_or(0);
    serde_json::from_slice(&json[..終端]).expect("JSON塊は読める")
}

/// GLBのhumanBonesにある骨の対応ノード索引を読む。
fn humanoidノード索引(値: &serde_json::Value, 骨名: &str) -> u64 {
    値.get("extensions")
        .and_then(|値| 値.get("VRMC_vrm"))
        .and_then(|値| 値.get("humanoid"))
        .and_then(|値| 値.get("humanBones"))
        .and_then(|値| 値.get(骨名))
        .and_then(|値| 値.get("node"))
        .and_then(serde_json::Value::as_u64)
        .expect("humanBonesに対応ノードがある")
}

/// GLBのnodes配列にある名前の索引を読む。
fn ノード索引(値: &serde_json::Value, 名前: &str) -> u64 {
    値.get("nodes")
        .and_then(serde_json::Value::as_array)
        .expect("nodesがある")
        .iter()
        .position(|ノード| ノード.get("name").and_then(serde_json::Value::as_str) == Some(名前))
        .expect("試験の名前はGLBにある") as u64
}

/// wol風の名前を持つ必須骨と三角形1枚のFBXを組み立てる。
fn wol風fbxバイト列() -> Vec<u8> {
    fbxを組み立てる(&wol風の定義())
}

/// exact名の必須骨と三角形1枚のFBXを組み立てる。
fn exact名fbxバイト列() -> Vec<u8> {
    fbxを組み立てる(&exact名の定義())
}

/// ノード定義からバイナリFBXを組み立てる。
/// 親子関係は書き出し層の許容（上腕は脊椎の下など）を満たす。
fn fbxを組み立てる(定義: &[(i64, &str, Option<i64>)]) -> Vec<u8> {
    let mut objs: Vec<TNode> = 定義
        .iter()
        .map(|(id, name, _)| {
            let subtype = if *name == "Owner" { "Mesh" } else { "LimbNode" };
            model_node(*id, name, subtype)
        })
        .collect();
    let mut vertices = Props::default();
    vertices.d_raw(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
    let mut pvi = Props::default();
    pvi.i_raw(&[0, 1, -3]);
    objs.push(geometry_node(
        100,
        "Tri",
        vec![
            TNode::leaf(b"Vertices", vertices),
            TNode::leaf(b"PolygonVertexIndex", pvi),
        ],
    ));
    let mut pose_props = Props::default();
    pose_props.l(200);
    pose_props.s(b"Bind\x00\x01Pose");
    pose_props.s(b"BindPose");
    let binds: Vec<TNode> = 定義
        .iter()
        .map(|(id, _, _)| pose_node(*id, &bind_matrix(0.0, 0.0, 0.0)))
        .collect();
    objs.push(TNode::branch(b"Pose", pose_props, binds));
    let mut conns: Vec<TNode> = 定義
        .iter()
        .filter_map(|(id, _, parent)| parent.map(|parent| conn(*id, parent)))
        .collect();
    conns.push(conn(100, 17));
    conns.push(conn(1, 0));
    binary_fbx(vec![
        TNode::branch(b"Objects", Props::default(), objs),
        TNode::branch(b"Connections", Props::default(), conns),
    ])
}

/// 論理プロパティ列。件数は組み立て時に数える。
#[derive(Default)]
struct Props {
    buf: Vec<u8>,
    count: u32,
}

impl Props {
    fn prop(&mut self, code: u8, body: &[u8]) {
        self.buf.push(code);
        self.buf.extend_from_slice(body);
        self.count += 1;
    }

    fn l(&mut self, v: i64) {
        self.prop(b'L', &v.to_le_bytes());
    }

    fn s(&mut self, v: &[u8]) {
        let mut body = (v.len() as u32).to_le_bytes().to_vec();
        body.extend_from_slice(v);
        self.prop(b'S', &body);
    }

    /// `f64` 列をrawの `d` 配列として書く。
    fn d_raw(&mut self, values: &[f64]) {
        let mut payload = Vec::with_capacity(values.len() * 8);
        for v in values {
            payload.extend_from_slice(&v.to_le_bytes());
        }
        let mut body = (values.len() as u32).to_le_bytes().to_vec();
        body.extend_from_slice(&0u32.to_le_bytes());
        body.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        body.extend_from_slice(&payload);
        self.prop(b'd', &body);
    }

    /// `i32` 列をrawの `i` 配列として書く。
    fn i_raw(&mut self, values: &[i32]) {
        let mut payload = Vec::with_capacity(values.len() * 4);
        for v in values {
            payload.extend_from_slice(&v.to_le_bytes());
        }
        let mut body = (values.len() as u32).to_le_bytes().to_vec();
        body.extend_from_slice(&0u32.to_le_bytes());
        body.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        body.extend_from_slice(&payload);
        self.prop(b'i', &body);
    }
}

/// 論理ノード。シリアライズ時に絶対オフセットを確定させる。
struct TNode {
    name: Vec<u8>,
    props: Props,
    children: Vec<TNode>,
}

impl TNode {
    fn leaf(name: &[u8], props: Props) -> Self {
        Self {
            name: name.to_vec(),
            props,
            children: Vec::new(),
        }
    }

    fn branch(name: &[u8], props: Props, children: Vec<TNode>) -> Self {
        Self {
            name: name.to_vec(),
            props,
            children,
        }
    }
}

/// 論理ノードを絶対オフセット付きで書き出す。
fn serialise(node: &TNode, buf: &mut Vec<u8>) {
    let start = buf.len();
    buf.extend_from_slice(&[0u8; 13]);
    buf.extend_from_slice(&node.name);
    buf.extend_from_slice(&node.props.buf);
    for child in &node.children {
        serialise(child, buf);
    }
    if !node.children.is_empty() {
        buf.extend_from_slice(&[0u8; 13]);
    }
    let end = buf.len() as u32;
    buf[start..start + 4].copy_from_slice(&end.to_le_bytes());
    buf[start + 4..start + 8].copy_from_slice(&node.props.count.to_le_bytes());
    buf[start + 8..start + 12].copy_from_slice(&(node.props.buf.len() as u32).to_le_bytes());
    buf[start + 12] = node.name.len() as u8;
}

/// 直下ノード列からバイナリFBXを組み立てる。
fn binary_fbx(top: Vec<TNode>) -> Vec<u8> {
    let mut buf = b"Kaydara FBX Binary  \x00\x1a\x00".to_vec();
    buf.extend_from_slice(&7400u32.to_le_bytes());
    for node in &top {
        serialise(node, &mut buf);
    }
    buf.extend_from_slice(&[0u8; 13]);
    buf
}

/// `Model` ノードを組み立てる。
fn model_node(id: i64, name: &str, subtype: &str) -> TNode {
    let mut props = Props::default();
    props.l(id);
    props.s(format!("{name}\x00\x01Model").as_bytes());
    props.s(subtype.as_bytes());
    TNode::branch(b"Model", props, Vec::new())
}

/// `Geometry`（Mesh）ノードを組み立てる。
fn geometry_node(id: i64, name: &str, children: Vec<TNode>) -> TNode {
    let mut props = Props::default();
    props.l(id);
    props.s(format!("{name}\x00\x01Geometry").as_bytes());
    props.s(b"Mesh");
    TNode::branch(b"Geometry", props, children)
}

/// `PoseNode` を組み立てる。行列は16値の `d` 配列である。
fn pose_node(model: i64, matrix: &[f64; 16]) -> TNode {
    let mut matrix_props = Props::default();
    matrix_props.d_raw(matrix);
    let mut node_props = Props::default();
    node_props.l(model);
    TNode::branch(
        b"PoseNode",
        Props::default(),
        vec![
            TNode::leaf(b"Node", node_props),
            TNode::leaf(b"Matrix", matrix_props),
        ],
    )
}

/// 平行移動だけが異なる行優先のバインド行列を組み立てる。
fn bind_matrix(x: f64, y: f64, z: f64) -> [f64; 16] {
    [
        1.0, 0.0, 0.0, 0.0, //
        0.0, 1.0, 0.0, 0.0, //
        0.0, 0.0, 1.0, 0.0, //
        x, y, z, 1.0, //
    ]
}

/// 接続1件を組み立てる。
fn conn(child: i64, parent: i64) -> TNode {
    let mut props = Props::default();
    props.s(b"OO");
    props.l(child);
    props.l(parent);
    TNode::leaf(b"C", props)
}

/// 環境変数の追加指定付きでビルド済みバイナリを実行する。
fn run_env(args: &[&str], cwd: &Path, extra: &[(&str, &str)]) -> std::process::Output {
    let mut 命令 = Command::new(env!("CARGO_BIN_EXE_vrctom"));
    命令.args(args).current_dir(cwd);
    for (鍵, 値) in extra {
        命令.env(鍵, 値);
    }
    命令.output().expect("バイナリは実行できる")
}

/// 入力FBXの隣の `.fbx.meta` からGUIDを読む。無ければ `None` である。
fn fbxのguidを読む(fbx: &Path) -> Option<String> {
    let mut メタ名 = fbx.as_os_str().to_owned();
    メタ名.push(".meta");
    let 本文 = fs::read_to_string(メタ名).ok()?;
    本文.lines().find_map(|行| {
        let 残り = 行.trim().strip_prefix("guid:")?;
        let guid = 残り.trim().to_owned();
        (!guid.is_empty()).then_some(guid)
    })
}

/// `Assets` 配下で入力FBXのGUIDを参照する最初の `.prefab` を探す。
///
/// 辞書順で走査し、内容にGUIDを含む最初のPrefabを選ぶ。無ければ `None` である。
/// 辞書順先頭を無条件に選ぶと入力FBXを参照しないPrefabを掴むため、GUID参照で絞る。
fn fbxを参照するprefabを探す(assets: &Path, guid: &str) -> Option<std::path::PathBuf> {
    let mut 積み = vec![assets.to_path_buf()];
    let mut 候補 = Vec::new();
    while let Some(場所) = 積み.pop() {
        let Ok(項目ら) = fs::read_dir(&場所) else {
            continue;
        };
        for 項目 in 項目ら.flatten() {
            let 道 = 項目.path();
            if 道.is_dir() {
                積み.push(道);
            } else if 道
                .extension()
                .is_some_and(|拡張| 拡張.eq_ignore_ascii_case("prefab"))
            {
                候補.push(道);
            }
        }
    }
    候補.sort();
    候補
        .into_iter()
        .find(|道| fs::read_to_string(道).is_ok_and(|本文| 本文.contains(guid)))
}

/// 対象FBXを参照するPrefab由来の不一致はT-ポーズ段階で終了コード1になる。
///
/// 入力FBXの隣の `.fbx.meta` からGUIDを読み、`Assets` 配下でそのGUIDを参照する
/// Prefabを辞書順で選ぶ。入力FBXを参照しないPrefabは選ばず、見つからなければ
/// 理由を出して読み飛ばす。選んだPrefabから物理sidecarを導出し
/// （`--prefab` 全体の成功は問わない）、humanoidの1骨の指す先をずらした改変sidecarで
/// `export --physics` を実行する。T-ポーズ段階の不一致で終了コード1・VRM出力なし・
/// `humanoidの対応が一致しない` の診断になることを確かめる。
/// 導出JSONの存在だけを成功条件にしない。
/// 実FBX・Prefab・Assetsを使うため、環境変数が無ければ読み飛ばす。
#[test]
fn 対象fbxを参照するprefab由来の不一致はtポーズ段階で失敗する() {
    // 実FBX・Prefab・Assetsを使うため、環境変数が無ければ読み飛ばす。
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
    let 資材 = 資材親.join("Assets");
    // 辞書順先頭のPrefabを無条件に選ぶと、入力FBXを参照しないPrefabを掴み、
    // 導出の段階で失敗して不一致の検証まで到達しない。
    // 入力FBXのGUIDを参照するPrefabに絞って選ぶ。
    let Some(guid) = fbxのguidを読む(fbx) else {
        eprintln!("skip: 入力FBXの隣にGUID付きの `.fbx.meta` が無い");
        return;
    };
    let Some(prefab) = fbxを参照するprefabを探す(&資材, &guid) else {
        eprintln!("skip: 入力FBXのGUIDを参照するPrefabがAssets配下に無い");
        return;
    };
    let prefab_arg = prefab.to_string_lossy().into_owned();
    let assets_arg = 資材.to_string_lossy().into_owned();
    let work = tempfile::TempDir::new().expect("作業場所は作れる");
    // Prefabから物理sidecarを導出し、検証用の記録も残す。
    // 後段（材質スロット等）で失敗し得るため終了コードは問わない。
    // 導出sidecarの記録が残ることが、導出成功の証拠になる。
    let 導出先 = work.path().join("derived.vrm");
    let 導出先_arg = 導出先.to_string_lossy().into_owned();
    let 記録 = work.path().join("derived.json");
    let 記録_arg = 記録.to_string_lossy().into_owned();
    let 導出 = run_env(
        &[
            "export",
            &fixture,
            "-o",
            &導出先_arg,
            "--prefab",
            &prefab_arg,
            "--assets",
            &assets_arg,
        ],
        work.path(),
        &[("VRCTOM_PHYSICS_DUMP", 記録_arg.as_str())],
    );
    let 導出stderr = String::from_utf8_lossy(&導出.stderr);
    assert!(記録.exists(), "導出sidecarの記録は残る: {導出stderr}");
    let 記録文 = fs::read_to_string(&記録).expect("導出sidecarの記録は読める");
    let 記録json: serde_json::Value =
        serde_json::from_str(&記録文).expect("導出sidecarはJSONである");
    // 導出sidecarのhumanoidを軸に、指す先のずらしで不一致を作る。
    let Some(骨ら) = 記録json["humanoid"]["bones"].as_array() else {
        eprintln!("skip: 導出sidecarにhumanoidが無い");
        return;
    };
    // humanoidが参照しない先頭ノードを、ずらし先にする。
    let 参照済み: std::collections::HashSet<&str> =
        骨ら.iter().filter_map(|骨| 骨["nodeId"].as_str()).collect();
    let Some(被害者) = 記録json["nodes"]
        .as_array()
        .expect("nodesがある")
        .iter()
        .find(|ノード| {
            ノード["id"]
                .as_str()
                .is_some_and(|id| !参照済み.contains(id))
        })
        .and_then(|ノード| ノード["id"].as_str())
    else {
        eprintln!("skip: 未参照ノードが無い");
        return;
    };
    // 比較対象になる骨を探す。未知骨やVRMに対応の無い骨は比較対象外のため、
    // ずらしても成功する。その場合は次の骨を試し、最初に不一致を検出した骨で確かめる。
    // 固定の骨名に依存せず、軸名は診断から取る。
    let mut 検出: Option<(String, String)> = None;
    for (試行, 候補) in 骨ら.iter().filter_map(|骨| 骨["bone"].as_str()).enumerate() {
        let 候補ノード = 骨ら
            .iter()
            .find(|骨| 骨["bone"].as_str() == Some(候補))
            .and_then(|骨| 骨["nodeId"].as_str())
            .expect("候補のnodeIdがある");
        if 候補ノード == 被害者 {
            continue;
        }
        let mut 改変 = 記録json.clone();
        for 骨 in 改変["humanoid"]["bones"]
            .as_array_mut()
            .expect("bonesがある")
        {
            if 骨["bone"].as_str() == Some(候補) {
                骨["nodeId"] = serde_json::Value::String(被害者.to_owned());
            }
        }
        let 改変先 = work.path().join(format!("modified-{試行}.json"));
        fs::write(&改変先, serde_json::to_string(&改変).expect("改変は書ける"))
            .expect("改変sidecarは書ける");
        let 改変_arg = 改変先.to_string_lossy().into_owned();
        // 不一致のsidecarはT-pose適用で失敗し、出力先を作らない。
        let 出力先 = work.path().join(format!("out-{試行}.vrm"));
        let 出力先_arg = 出力先.to_string_lossy().into_owned();
        let 結果 = run(
            &[
                "export",
                &fixture,
                "-o",
                &出力先_arg,
                "--physics",
                &改変_arg,
            ],
            work.path(),
        );
        let 文面 = String::from_utf8_lossy(&結果.stderr).into_owned();
        if 結果.status.code() == Some(1)
            && 結果.stdout.is_empty()
            && !出力先.exists()
            && 文面.contains("humanoidの対応が一致しない")
            && 文面.contains(候補)
        {
            検出 = Some((候補.to_owned(), 文面));
            break;
        }
        if 結果.status.code() == Some(0) {
            // 比較対象外の骨であり、ずらしても通る。次の骨を試す。
            continue;
        }
        panic!("想定外の失敗であり、不一致の検証にならない: {文面}");
    }
    let Some((対象骨名, 文面)) = 検出 else {
        panic!("いずれの骨でも対応不一致を検出できない");
    };
    assert!(文面.contains(被害者), "診断にずらし先が入る: {文面}");
    eprintln!("不一致を検出した骨: {対象骨名}");
}

/// 倍精度浮動小数点のプロパティを書く。`DeformPercent` に使う。
impl Props {
    fn d(&mut self, v: f64) {
        self.prop(b'D', &v.to_le_bytes());
    }
}

/// 表情チャンネル付きのFBXを組み立てる。exact名の必須骨と三角形1枚を持ち、書き出しまで通る。
fn 表情付きfbxバイト列(チャンネル名ら: &[&str]) -> Vec<u8> {
    表情付きfbxバイト列_幾何名(チャンネル名ら, "Tri")
}

/// ジオメトリ名を指定して表情チャンネル付きFBXを組み立てる。診断エスケープの確認に使う。
fn 表情付きfbxバイト列_幾何名(
    チャンネル名ら: &[&str], 幾何名: &str
) -> Vec<u8> {
    let 定義 = exact名の定義();
    let mut objs: Vec<TNode> = 定義
        .iter()
        .map(|(id, name, _)| {
            let subtype = if *name == "Owner" { "Mesh" } else { "LimbNode" };
            model_node(*id, name, subtype)
        })
        .collect();
    let mut vertices = Props::default();
    vertices.d_raw(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
    let mut pvi = Props::default();
    pvi.i_raw(&[0, 1, -3]);
    objs.push(geometry_node(
        100,
        幾何名,
        vec![
            TNode::leaf(b"Vertices", vertices),
            TNode::leaf(b"PolygonVertexIndex", pvi),
        ],
    ));
    objs.push(TNode::branch(
        b"Deformer",
        {
            let mut props = Props::default();
            props.l(3000);
            props.s(b"Blend\x00\x01Deformer");
            props.s(b"BlendShape");
            props
        },
        vec![],
    ));
    let mut 追加接続 = Vec::new();
    for (offset, 名前) in チャンネル名ら.iter().enumerate() {
        let shape_id = 1000 + offset as i64;
        let channel_id = 2000 + offset as i64;
        let mut channel_full = 名前.as_bytes().to_vec();
        channel_full.extend_from_slice(b"\x00\x01SubDeformer");
        let mut shape_full = format!("{名前}-shape").into_bytes();
        shape_full.extend_from_slice(b"\x00\x01Shape");
        let mut index_props = Props::default();
        index_props.i_raw(&[0]);
        let mut delta_props = Props::default();
        delta_props.d_raw(&[0.5, 0.0, 0.0]);
        let mut shape_props = Props::default();
        shape_props.l(shape_id);
        shape_props.s(&shape_full);
        shape_props.s(b"Shape");
        objs.push(TNode::branch(
            b"Geometry",
            shape_props,
            vec![
                TNode::leaf(b"Indexes", index_props),
                TNode::leaf(b"Vertices", delta_props),
            ],
        ));
        let mut deform_props = Props::default();
        deform_props.d(0.0);
        let mut channel_props = Props::default();
        channel_props.l(channel_id);
        channel_props.s(&channel_full);
        channel_props.s(b"BlendShapeChannel");
        objs.push(TNode::branch(
            b"Deformer",
            channel_props,
            vec![TNode::leaf(b"DeformPercent", deform_props)],
        ));
        追加接続.push(conn(shape_id, channel_id));
        追加接続.push(conn(channel_id, 3000));
    }
    追加接続.push(conn(3000, 100));
    let mut pose_props = Props::default();
    pose_props.l(200);
    pose_props.s(b"Bind\x00\x01Pose");
    pose_props.s(b"BindPose");
    let binds: Vec<TNode> = 定義
        .iter()
        .map(|(id, _, _)| pose_node(*id, &bind_matrix(0.0, 0.0, 0.0)))
        .collect();
    objs.push(TNode::branch(b"Pose", pose_props, binds));
    let mut conns: Vec<TNode> = 定義
        .iter()
        .filter_map(|(id, _, parent)| parent.map(|parent| conn(*id, parent)))
        .collect();
    conns.push(conn(100, 17));
    conns.push(conn(1, 0));
    conns.extend(追加接続);
    binary_fbx(vec![
        TNode::branch(b"Objects", Props::default(), objs),
        TNode::branch(b"Connections", Props::default(), conns),
    ])
}

/// GLBのプリセット式名を読み出す。
fn glbの式名ら(バイト列: &[u8]) -> Vec<String> {
    let json = glbのjsonを読む(バイト列);
    let preset = &json["extensions"]["VRMC_vrm"]["expressions"]["preset"];
    let mut 名ら: Vec<String> = preset
        .as_object()
        .map(|map| map.keys().cloned().collect())
        .unwrap_or_default();
    名ら.sort();
    名ら
}

#[test]
fn 部分一致は終了コード0で欠落警告を要約より前に出す() {
    // `vrc.v_aa`・`vrc.v_ih` だけがある場合は2式を作り、残り6件を警告する。
    let fbxバイト列 = 表情付きfbxバイト列(&["vrc.v_aa", "vrc.v_ih"]);
    let work = tempfile::TempDir::new().expect("作業場所は作れる");
    let 入力 = work.path().join("model.fbx");
    fs::write(&入力, &fbxバイト列).expect("入力の準備は成功する");
    let 入力_arg = 入力.to_string_lossy().into_owned();
    let 出力先 = work.path().join("out.vrm");
    let 出力先_arg = 出力先.to_string_lossy().into_owned();
    let 出力 = run(
        &[
            "export",
            &入力_arg,
            "-o",
            &出力先_arg,
            "--expressions",
            "vrc",
        ],
        work.path(),
    );
    let stderr = String::from_utf8_lossy(&出力.stderr);
    assert_eq!(出力.status.code(), Some(0), "stderr: {stderr}");
    assert!(出力.stdout.is_empty(), "標準出力は空である");
    let バイト列 = fs::read(&出力先).expect("出力は読める");
    assert_eq!(&バイト列[..4], b"glTF");
    // GLBの式は適用できた2件だけである。
    assert_eq!(
        glbの式名ら(&バイト列),
        vec!["aa".to_owned(), "ih".to_owned()]
    );
    // 欠落6件の警告がそれぞれ一度だけ出る。
    for 欠落 in [
        "vrc.v_ou",
        "vrc.v_e",
        "vrc.v_oh",
        "vrc.blink",
        "vrc.looking_up",
        "vrc.looking_down",
    ] {
        assert_eq!(
            stderr.matches(欠落).count(),
            1,
            "欠落警告は一度だけ出す: {欠落}\nstderr: {stderr}"
        );
    }
    assert!(
        !stderr.contains("vrc.v_aa（"),
        "適用できた表記の欠落警告は出さない: {stderr}"
    );
    // 警告は要約より前に出す。
    let 警告位置 = stderr.find("中間モデルの警告:").expect("見出しがある");
    let 要約位置 = stderr.find("書き出しが完了した").expect("要約がある");
    assert!(警告位置 < 要約位置, "警告は要約より前である: {stderr}");
}

#[test]
fn 一致0件は終了コード0で全要求を警告する() {
    // 要求のどれにも一致しない場合は式なしで成功し、8件すべてを警告する。
    let fbxバイト列 = 表情付きfbxバイト列(&["t1"]);
    let work = tempfile::TempDir::new().expect("作業場所は作れる");
    let 入力 = work.path().join("model.fbx");
    fs::write(&入力, &fbxバイト列).expect("入力の準備は成功する");
    let 入力_arg = 入力.to_string_lossy().into_owned();
    let 出力先 = work.path().join("out.vrm");
    let 出力先_arg = 出力先.to_string_lossy().into_owned();
    let 出力 = run(
        &[
            "export",
            &入力_arg,
            "-o",
            &出力先_arg,
            "--expressions",
            "vrc",
        ],
        work.path(),
    );
    let stderr = String::from_utf8_lossy(&出力.stderr);
    assert_eq!(出力.status.code(), Some(0), "stderr: {stderr}");
    assert!(出力.stdout.is_empty(), "標準出力は空である");
    let バイト列 = fs::read(&出力先).expect("出力は読める");
    assert!(glbの式名ら(&バイト列).is_empty(), "式は作らない");
    for 要求 in [
        "vrc.v_aa",
        "vrc.v_ih",
        "vrc.v_ou",
        "vrc.v_e",
        "vrc.v_oh",
        "vrc.blink",
        "vrc.looking_up",
        "vrc.looking_down",
    ] {
        assert_eq!(
            stderr.matches(要求).count(),
            1,
            "全要求を一度だけ警告する: {要求}\nstderr: {stderr}"
        );
    }
}

#[test]
fn 特殊経路でも中間モデルの警告を要約より前に出す() {
    // `--physics` の特殊経路でも通常経路と同じ警告表示になる。
    let fbxバイト列 = 表情付きfbxバイト列(&["vrc.v_aa", "vrc.v_ih"]);
    let sha = sha256十六進(&fbxバイト列);
    let work = tempfile::TempDir::new().expect("作業場所は作れる");
    let 入力 = work.path().join("model.fbx");
    fs::write(&入力, &fbxバイト列).expect("入力の準備は成功する");
    let mut メタ名 = 入力.as_os_str().to_owned();
    メタ名.push(".meta");
    fs::write(&メタ名, 対応表meta(&exact名の対応項目())).expect("対応表の準備は成功する");
    let 入力_arg = 入力.to_string_lossy().into_owned();
    let sidecar文 = serde_json::json!({
        "format": "vrctom.physics",
        "schemaVersion": 1,
        "generator": {"name": "試験", "version": "0.1"},
        "capture": {
            "statePolicy": "edit-mode",
            "coordinateSpace": "unity-avatar-root-local-meters",
            "avatarRootId": "n:0",
            "rootScale": [1.0, 1.0, 1.0],
        },
        "sourceModels": [{"id": "synth", "sha256": sha}],
        "nodes": [
            手書きsidecarノードjson("n:0", None, "Hips", &["Hips"]),
            手書きsidecarノードjson("n:1", Some("n:0"), "LeftHand", &["Hips", "Spine", "LeftUpperArm", "LeftLowerArm", "LeftHand"]),
            手書きsidecarノードjson("n:2", Some("n:0"), "Head", &["Hips", "Spine", "Head"]),
        ],
        "physBones": [{"id": "pb:0", "componentNodeId": "n:0", "rootNodeId": "n:0"}],
        "colliders": [],
        "humanoid": {
            "isHuman": true,
            "bones": [
                手書きhumanoid骨json("Hips", "n:0"),
                手書きhumanoid骨json("LeftHand", "n:1"),
            ],
        },
        "unsupportedFeatures": [],
    })
    .to_string();
    let sidecar先 = work.path().join("sidecar.json");
    fs::write(&sidecar先, sidecar文).expect("sidecarは書ける");
    let sidecar_arg = sidecar先.to_string_lossy().into_owned();
    let 出力先 = work.path().join("out.vrm");
    let 出力先_arg = 出力先.to_string_lossy().into_owned();
    let 出力 = run(
        &[
            "export",
            &入力_arg,
            "-o",
            &出力先_arg,
            "--expressions",
            "vrc",
            "--physics",
            &sidecar_arg,
        ],
        work.path(),
    );
    let stderr = String::from_utf8_lossy(&出力.stderr);
    assert_eq!(出力.status.code(), Some(0), "stderr: {stderr}");
    assert!(出力.stdout.is_empty(), "標準出力は空である");
    assert_eq!(
        glbの式名ら(&fs::read(&出力先).expect("出力は読める")),
        vec!["aa".to_owned(), "ih".to_owned()]
    );
    assert_eq!(
        stderr.matches("vrc.blink").count(),
        1,
        "欠落警告は一度だけ出す: {stderr}"
    );
    let 警告位置 = stderr.find("中間モデルの警告:").expect("見出しがある");
    let 要約位置 = stderr.find("書き出しが完了した").expect("要約がある");
    assert!(警告位置 < 要約位置, "警告は要約より前である: {stderr}");
}

#[test]
fn 中間モデルの警告は診断エスケープする() {
    // ジオメトリ名に改行があっても、警告表示は行偽装しない。
    // 部分一致にして採用メッシュ名を警告文に含める。
    let fbxバイト列 = 表情付きfbxバイト列_幾何名(&["vrc.v_aa"], "G\nevil");
    let work = tempfile::TempDir::new().expect("作業場所は作れる");
    let 入力 = work.path().join("model.fbx");
    fs::write(&入力, &fbxバイト列).expect("入力の準備は成功する");
    let 入力_arg = 入力.to_string_lossy().into_owned();
    let 出力先 = work.path().join("out.vrm");
    let 出力先_arg = 出力先.to_string_lossy().into_owned();
    let 出力 = run(
        &[
            "export",
            &入力_arg,
            "-o",
            &出力先_arg,
            "--expressions",
            "vrc",
        ],
        work.path(),
    );
    let stderr = String::from_utf8_lossy(&出力.stderr);
    assert_eq!(出力.status.code(), Some(0), "stderr: {stderr}");
    assert!(
        stderr.contains("G\\nevil"),
        "改行は可視エスケープする: {stderr}"
    );
    // 警告行に素の改行は混じらない（行末以外の改行で割れない）。
    for 行 in stderr.lines() {
        if 行.contains("中間モデルの警告:") {
            assert!(行.contains("G\\nevil"), "警告行に逃がし済み名がある: {行}");
        }
    }
}

/// 同率2メッシュの表情FBXを組み立てる。先頭と後続の差分を変えられる。
/// チャンネル名はVRCの要求名に含まれるものを使い、同率の限定解消を起こす。
fn 同率表情fbxバイト列(先頭差分: [f64; 3], 後続差分: [f64; 3]) -> Vec<u8> {
    表情チャンネル列を持つfbxバイト列(
        &[("vrc.v_aa", 先頭差分)],
        &[("vrc.v_aa", 後続差分)],
    )
}

/// 2メッシュに表情チャンネルを持つFBXを組み立てる。exact名の必須骨で書き出せる。
fn 表情チャンネル列を持つfbxバイト列(
    先頭ら: &[(&str, [f64; 3])],
    後続ら: &[(&str, [f64; 3])],
) -> Vec<u8> {
    let 定義 = exact名の定義();
    let mut objs: Vec<TNode> = 定義
        .iter()
        .map(|(id, name, _)| {
            let subtype = if *name == "Owner" { "Mesh" } else { "LimbNode" };
            model_node(*id, name, subtype)
        })
        .collect();
    let mut vertices = Props::default();
    vertices.d_raw(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
    let mut pvi = Props::default();
    pvi.i_raw(&[0, 1, -3]);
    objs.push(geometry_node(
        100,
        "Tri",
        vec![
            TNode::leaf(b"Vertices", vertices),
            TNode::leaf(b"PolygonVertexIndex", pvi),
        ],
    ));
    objs.push(TNode::branch(
        b"Deformer",
        {
            let mut props = Props::default();
            props.l(3000);
            props.s(b"Blend\x00\x01Deformer");
            props.s(b"BlendShape");
            props
        },
        vec![],
    ));
    let mut 追加接続 = Vec::new();
    for (offset, (名前, 差分)) in 先頭ら.iter().enumerate() {
        let shape_id = 1000 + offset as i64;
        let channel_id = 2000 + offset as i64;
        追加接続.extend(表情チャンネル部品(
            &mut objs, shape_id, channel_id, 名前, 差分, 3000,
        ));
    }
    let mut 後続vertices = Props::default();
    後続vertices.d_raw(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
    let mut 後続pvi = Props::default();
    後続pvi.i_raw(&[0, 1, -3]);
    objs.push(geometry_node(
        101,
        "Second",
        vec![
            TNode::leaf(b"Vertices", 後続vertices),
            TNode::leaf(b"PolygonVertexIndex", 後続pvi),
        ],
    ));
    objs.push(TNode::branch(
        b"Deformer",
        {
            let mut props = Props::default();
            props.l(3001);
            props.s(b"SecondBlend\x00\x01Deformer");
            props.s(b"BlendShape");
            props
        },
        vec![],
    ));
    // 所有モデルを追加し、後続メッシュに結ぶ。
    objs.push(model_node(18, "Owner2", "Mesh"));
    for (offset, (名前, 差分)) in 後続ら.iter().enumerate() {
        let shape_id = 1100 + offset as i64;
        let channel_id = 2100 + offset as i64;
        追加接続.extend(表情チャンネル部品(
            &mut objs, shape_id, channel_id, 名前, 差分, 3001,
        ));
    }
    追加接続.push(conn(3000, 100));
    追加接続.push(conn(3001, 101));
    let mut pose_props = Props::default();
    pose_props.l(200);
    pose_props.s(b"Bind\x00\x01Pose");
    pose_props.s(b"BindPose");
    let binds: Vec<TNode> = 定義
        .iter()
        .map(|(id, _, _)| pose_node(*id, &bind_matrix(0.0, 0.0, 0.0)))
        .chain(std::iter::once(pose_node(18, &bind_matrix(0.0, 0.0, 0.0))))
        .collect();
    objs.push(TNode::branch(b"Pose", pose_props, binds));
    let mut conns: Vec<TNode> = 定義
        .iter()
        .filter_map(|(id, _, parent)| parent.map(|parent| conn(*id, parent)))
        .collect();
    conns.push(conn(100, 17));
    conns.push(conn(1, 0));
    conns.push(conn(18, 1));
    conns.push(conn(101, 18));
    conns.extend(追加接続);
    binary_fbx(vec![
        TNode::branch(b"Objects", Props::default(), objs),
        TNode::branch(b"Connections", Props::default(), conns),
    ])
}

/// 表情チャンネル1件分のノード群を足し、接続2件を返す。
fn 表情チャンネル部品(
    objs: &mut Vec<TNode>,
    shape_id: i64,
    channel_id: i64,
    名前: &str,
    差分: &[f64; 3],
    blend_id: i64,
) -> Vec<TNode> {
    let mut channel_full = 名前.as_bytes().to_vec();
    channel_full.extend_from_slice(b"\x00\x01SubDeformer");
    let mut shape_full = format!("{名前}-shape").into_bytes();
    shape_full.extend_from_slice(b"\x00\x01Shape");
    let mut index_props = Props::default();
    index_props.i_raw(&[0]);
    let mut delta_props = Props::default();
    delta_props.d_raw(差分);
    let mut shape_props = Props::default();
    shape_props.l(shape_id);
    shape_props.s(&shape_full);
    shape_props.s(b"Shape");
    objs.push(TNode::branch(
        b"Geometry",
        shape_props,
        vec![
            TNode::leaf(b"Indexes", index_props),
            TNode::leaf(b"Vertices", delta_props),
        ],
    ));
    let mut deform_props = Props::default();
    deform_props.d(0.0);
    let mut channel_props = Props::default();
    channel_props.l(channel_id);
    channel_props.s(&channel_full);
    channel_props.s(b"BlendShapeChannel");
    objs.push(TNode::branch(
        b"Deformer",
        channel_props,
        vec![TNode::leaf(b"DeformPercent", deform_props)],
    ));
    vec![conn(shape_id, channel_id), conn(channel_id, blend_id)]
}

#[test]
fn 同率で唯一非ゼロなら採用して選択警告を要約より前に出す() {
    // 後続メッシュの一致ターゲットがすべてゼロなら先頭を採用する。
    let fbxバイト列 = 同率表情fbxバイト列([0.5, 0.0, 0.0], [0.0, 0.0, 0.0]);
    let work = tempfile::TempDir::new().expect("作業場所は作れる");
    let 入力 = work.path().join("model.fbx");
    fs::write(&入力, &fbxバイト列).expect("入力の準備は成功する");
    let 入力_arg = 入力.to_string_lossy().into_owned();
    let 出力先 = work.path().join("out.vrm");
    let 出力先_arg = 出力先.to_string_lossy().into_owned();
    let 出力 = run(
        &[
            "export",
            &入力_arg,
            "-o",
            &出力先_arg,
            "--expressions",
            "vrc",
        ],
        work.path(),
    );
    let stderr = String::from_utf8_lossy(&出力.stderr);
    assert_eq!(出力.status.code(), Some(0), "stderr: {stderr}");
    assert!(出力.stdout.is_empty(), "標準出力は空である");
    // 選択警告が欠落警告より前に一度だけ出て、要約より前である。
    assert_eq!(
        stderr.matches("同率候補から").count(),
        1,
        "選択警告は一度だけ出す: {stderr}"
    );
    let 選択位置 = stderr.find("同率候補から").expect("選択警告がある");
    let 欠落位置 = stderr.find("適用しない").expect("欠落警告がある");
    assert!(選択位置 < 欠落位置, "選択警告は欠落警告より前である");
    let 要約位置 = stderr.find("書き出しが完了した").expect("要約がある");
    assert!(選択位置 < 要約位置, "選択警告は要約より前である");
}

#[test]
fn no_baselineはprefabなしで終了コード2になる() {
    // `--no-baseline` は `--prefab` 必須であり、無い場合は引数不正になる。
    let work = tempfile::TempDir::new().expect("作業場所は作れる");
    let output = run(
        &["export", "dummy.fbx", "--no-baseline", "-o", "out.vrm"],
        work.path(),
    );
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(!output.stderr.is_empty());
}
