//! レンダラsidecarの契約テスト（公開 API のみを使う）。
//!
//! 合成JSONで版違い・書式違い・SHA不一致・未知や重複のメッシュ名・
//! 材質数不一致・null材質の失敗と、割当計画の決定性を固定する。
//! 実fixtureは使わず、対応付けは合成の中間メッシュ要約で行う。

use vrctom::vrm::{RendererLimits, RendererMeshView, RendererSidecar, plan_renderer_assignment};

// テスト名は日本語で書く（テストは仕様・契約であるため）。

/// 合成sidecarの雛形を作る。`models` と `renderers` は呼び出し側で渡す。
fn sidecar_json(models: &str, renderers: &str) -> String {
    format!(
        r#"{{"format":"vrctom.renderers","schemaVersion":1,"models":[{models}],"renderers":[{renderers}]}}"#
    )
}

/// 参照モデル1件のJSON断片を作る。
fn モデル断片(sha: &str) -> String {
    format!(
        r#"{{"path":"Assets/X/Y.fbx","guid":"9ef94258554e06b4ab03fbe9fc914c5c","sha256":"{sha}"}}"#
    )
}

/// 有効レンダラ1件のJSON断片を作る。`材質` は材質要素の列（`null` を含む）であり、
/// `数` はサブメッシュ数（材質列の長さと一致させる）である。
fn レンダラ断片(番号: u32, メッシュ名: &str, 材質: &str, 数: usize) -> String {
    format!(
        r#"{{"index":{番号},"path":"Root/{メッシュ名}","supported":true,"enabled":true,"activeInHierarchy":true,"mesh":{{"name":"{メッシュ名}","subMeshCount":{数}}},"materials":[{材質}]}}"#,
    )
}

/// lilToon材質1件のJSON断片を作る。
fn 材質断片(guid: &str, file: i64) -> String {
    format!(r#"{{"name":"M","guid":"{guid}","fileID":{file},"path":"Assets/X/M.mat"}}"#)
}

/// バイト列のSHA-256を小文字hexで返す。
fn sha256_hex(バイト列: &[u8]) -> String {
    use sha2::Digest as _;
    let 摘要 = sha2::Sha256::digest(バイト列);
    let mut 外 = String::with_capacity(64);
    for バイト in 摘要 {
        外.push_str(&format!("{バイト:02x}"));
    }
    外
}

/// 32桁hexのGUID断片を作る。
fn guid(末尾: char) -> String {
    format!("9ef94258554e06b4ab03fbe9fc914c5{末尾}")
}

#[test]
fn 版違いは失敗し版番号が出る() {
    let 本文 = sidecar_json(&モデル断片(&"0".repeat(64)), "");
    let 本文 = 本文.replace("\"schemaVersion\":1", "\"schemaVersion\":2");
    let 失敗 =
        RendererSidecar::from_json(&本文, &RendererLimits::new()).expect_err("版違いは失敗する");
    assert!(失敗.to_string().contains('2'), "版番号が文面にある: {失敗}");
}

#[test]
fn 書式違いは失敗し実際の書式名が出る() {
    let 本文 = sidecar_json(&モデル断片(&"0".repeat(64)), "")
        .replace("vrctom.renderers", "vrctom.physics");
    let 失敗 =
        RendererSidecar::from_json(&本文, &RendererLimits::new()).expect_err("書式違いは失敗する");
    assert!(
        失敗.to_string().contains("vrctom.physics"),
        "実際の書式名が文面にある: {失敗}"
    );
}

#[test]
fn sha不一致は失敗し入力の摘要が出る() {
    let 入力 = b"dummy fbx bytes";
    let 別物 = sha256_hex(b"other bytes");
    let 本文 = sidecar_json(
        &モデル断片(&別物),
        &レンダラ断片(0, "Body", &材質断片(&guid('a'), 2100000), 1),
    );
    let 側車 = RendererSidecar::from_json(&本文, &RendererLimits::new()).expect("形は正しい");
    let 概要 = [RendererMeshView {
        model_name: "Body",
        slot_count: 1,
    }];
    let 失敗 = plan_renderer_assignment(&側車, 入力, &概要).expect_err("SHA不一致は失敗する");
    assert!(
        失敗.to_string().contains(&sha256_hex(入力)),
        "入力の摘要が文面にある: {失敗}"
    );
}

#[test]
fn 未知のメッシュ名は失敗し対象名が出る() {
    let 入力 = b"dummy fbx bytes";
    let 本文 = sidecar_json(
        &モデル断片(&sha256_hex(入力)),
        &レンダラ断片(0, "無いメッシュ", &材質断片(&guid('b'), 2100000), 1),
    );
    let 側車 = RendererSidecar::from_json(&本文, &RendererLimits::new()).expect("形は正しい");
    let 概要 = [RendererMeshView {
        model_name: "Body",
        slot_count: 1,
    }];
    let 失敗 = plan_renderer_assignment(&側車, 入力, &概要).expect_err("未知名は失敗する");
    assert!(
        失敗.to_string().contains("無いメッシュ"),
        "対象名が文面にある: {失敗}"
    );
}

#[test]
fn 重複するモデル名は失敗し候補数が出る() {
    let 入力 = b"dummy fbx bytes";
    let 本文 = sidecar_json(
        &モデル断片(&sha256_hex(入力)),
        &レンダラ断片(0, "Body", &材質断片(&guid('c'), 2100000), 1),
    );
    let 側車 = RendererSidecar::from_json(&本文, &RendererLimits::new()).expect("形は正しい");
    let 概要 = [
        RendererMeshView {
            model_name: "Body",
            slot_count: 1,
        },
        RendererMeshView {
            model_name: "Body",
            slot_count: 1,
        },
    ];
    let 失敗 = plan_renderer_assignment(&側車, 入力, &概要).expect_err("重複は失敗する");
    assert!(
        失敗.to_string().contains("Body") && 失敗.to_string().contains('2'),
        "対象名と候補数が文面にある: {失敗}"
    );
}

#[test]
fn 材質数の不一致は失敗し両方の件数が出る() {
    let 入力 = b"dummy fbx bytes";
    let 材質2 = format!(
        "{},{}",
        材質断片(&guid('d'), 2100000),
        材質断片(&guid('e'), 2100000)
    );
    let 本文 = sidecar_json(
        &モデル断片(&sha256_hex(入力)),
        &レンダラ断片(0, "Body", &材質2, 2),
    );
    let 側車 = RendererSidecar::from_json(&本文, &RendererLimits::new()).expect("形は正しい");
    let 概要 = [RendererMeshView {
        model_name: "Body",
        slot_count: 1,
    }];
    let 失敗 = plan_renderer_assignment(&側車, 入力, &概要).expect_err("件数不一致は失敗する");
    let 文面 = 失敗.to_string();
    assert!(
        文面.contains('2') && 文面.contains('1'),
        "両方の件数が文面にある: {失敗}"
    );
}

#[test]
fn null材質は失敗しレンダラのパスが出る() {
    let 入力 = b"dummy fbx bytes";
    let 本文 = sidecar_json(
        &モデル断片(&sha256_hex(入力)),
        &レンダラ断片(0, "Body", "null", 1),
    );
    let 側車 = RendererSidecar::from_json(&本文, &RendererLimits::new()).expect("形は正しい");
    let 概要 = [RendererMeshView {
        model_name: "Body",
        slot_count: 1,
    }];
    let 失敗 = plan_renderer_assignment(&側車, 入力, &概要).expect_err("null材質は失敗する");
    assert!(
        失敗.to_string().contains("Body"),
        "レンダラのパスが文面にある: {失敗}"
    );
}

#[test]
fn 無効レンダラは除外され材質プールは初出順になる() {
    let 入力 = b"dummy fbx bytes";
    // 番号1は無効（enabled偽）のため除外対象になり、番号0と2だけが残る。
    // 番号2の1スロット目は番号0と同じ材質であり、プールは3件になる。
    let 材質a = 材質断片(&guid('a'), 2100000);
    let 材質b = 材質断片(&guid('b'), 2100000);
    let 材質c = 材質断片(&guid('c'), 2100000);
    let 除外材質 = 材質断片(&guid('d'), 2100000);
    let 描画 = format!(
        "{}, {}, {}",
        レンダラ断片(0, "Body", &format!("{材質a},{材質b}"), 2),
        レンダラ断片(1, "Leg", &除外材質, 1).replace("\"enabled\":true", "\"enabled\":false"),
        レンダラ断片(2, "Head", &format!("{材質a},{材質c}"), 2),
    );
    let 本文 = sidecar_json(&モデル断片(&sha256_hex(入力)), &描画);
    let 側車 = RendererSidecar::from_json(&本文, &RendererLimits::new()).expect("形は正しい");
    let 概要 = [
        RendererMeshView {
            model_name: "Body",
            slot_count: 2,
        },
        RendererMeshView {
            model_name: "Leg",
            slot_count: 1,
        },
        RendererMeshView {
            model_name: "Head",
            slot_count: 2,
        },
    ];
    let 計画 = plan_renderer_assignment(&側車, 入力, &概要).expect("解決できる");
    assert_eq!(計画.active_count(), 2);
    assert_eq!(計画.excluded_names, vec!["Leg".to_string()]);
    assert_eq!(計画.excluded_meshes, vec![1]);
    // 材質プールはレンダラ順→サブメッシュ順の初出順（a・b・c）である。
    let 鍵: Vec<(String, i64)> = 計画
        .pool
        .iter()
        .map(|項目| (項目.guid.clone(), 項目.file_id))
        .collect();
    assert_eq!(
        鍵,
        vec![
            (guid('a'), 2100000),
            (guid('b'), 2100000),
            (guid('c'), 2100000),
        ]
    );
    // Headの1スロット目はBodyの1スロット目と同じプール索引である。
    assert_eq!(計画.assignments[0].pool_indices, vec![0, 1]);
    assert_eq!(計画.assignments[1].pool_indices, vec![0, 2]);
    // 同一内容から同じ計画になり、列挙順に依存しない。
    let 再計画 = plan_renderer_assignment(&側車, 入力, &概要).expect("再解決できる");
    let 再鍵: Vec<(String, i64)> = 再計画
        .pool
        .iter()
        .map(|項目| (項目.guid.clone(), 項目.file_id))
        .collect();
    assert_eq!(鍵, 再鍵);
}
