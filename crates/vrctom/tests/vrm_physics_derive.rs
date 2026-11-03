//! Prefabからの物理sidecar導出とUnity採取の等価契約（実fixtureを使う）。
//!
//! `VRCTOM_MILFY_CHECK` が指すUnityプロジェクト（`references/unity-milfy-vrm-check`）
//! が無い場合は跳ばす。跳ばした場合は完了証拠にしない。
//! 実行例: `VRCTOM_MILFY_CHECK=$PWD/references/unity-milfy-vrm-check cargo test -p vrctom --test vrm_physics_derive`

// テスト名は日本語で書く（テストは仕様・契約である）。
// 補助関数の頭文字はアバター名の固有名詞であり、snake_caseに従わない。
#![allow(non_snake_case)]

use std::path::{Path, PathBuf};

use vrctom::fbx::{FbxFile, FbxOptions};
use vrctom::unity::{ParseOptions, SerializedFile};
use vrctom::vrm::{PrefabDeriveInput, derive_physics_sidecar};

/// 実fixtureの場所を返す。無ければ `None`（跳ばす）。
fn 実fixture() -> Option<(PathBuf, PathBuf, PathBuf, PathBuf)> {
    let root = std::env::var_os("VRCTOM_MILFY_CHECK").map(PathBuf::from)?;
    let fbx = root.join("Assets/PLUSONE/Milfy/FBX/Milfy.fbx");
    let prefab = root.join("Assets/PLUSONE/Milfy/Prefab/Milfy.prefab");
    // 導出の入力は `Assets` を含む配置であり、`Assets` 自身ではない。
    let assets = root.clone();
    let truth =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../references/fixtures/milfy-physics.json");
    if !fbx.is_file() || !prefab.is_file() || !assets.join("Assets").is_dir() || !truth.is_file() {
        return None;
    }
    Some((fbx, prefab, assets, truth))
}

/// 導出sidecarを作る。失敗はそのまま返す。
fn 導出する(
    fbx_path: &Path,
    prefab_path: &Path,
    assets_root: &Path,
    fbx_asset_path: &str,
) -> Result<Vec<u8>, String> {
    let fbx_bytes = std::fs::read(fbx_path).map_err(|e| e.to_string())?;
    let fbx = FbxFile::parse(&fbx_bytes, &FbxOptions::default()).map_err(|e| e.to_string())?;
    let prefab_bytes = std::fs::read(prefab_path).map_err(|e| e.to_string())?;
    let prefab_label = format!("{prefab_path:?}");
    let prefab =
        SerializedFile::parse(&prefab_bytes, &ParseOptions::new()).map_err(|e| e.to_string())?;
    let input = PrefabDeriveInput {
        fbx_bytes: &fbx_bytes,
        fbx: &fbx,
        assets_root,
        prefab: &prefab,
        prefab_label: &prefab_label,
        fbx_asset_path,
    };
    derive_physics_sidecar(&input)
        .map(|derived| derived.json)
        .map_err(|e| e.to_string())
}

/// Milfyの導出sidecarを作る。既存試験の入口である。
fn Milfyを導出する(
    fbx_path: &Path,
    prefab_path: &Path,
    assets_root: &Path,
) -> Result<Vec<u8>, String> {
    導出する(
        fbx_path,
        prefab_path,
        assets_root,
        "Assets/PLUSONE/Milfy/FBX/Milfy.fbx",
    )
}

/// 四元数の角度差（度）を求める。符号の違いは無視する。
fn 角度差(a: &[f64], b: &[f64]) -> f64 {
    let la = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2] + a[3] * a[3]).sqrt();
    let lb = (b[0] * b[0] + b[1] * b[1] + b[2] * b[2] + b[3] * b[3]).sqrt();
    let dot = (a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3]) / (la * lb);
    let dot = dot.abs().clamp(-1.0, 1.0);
    2.0 * dot.acos().to_degrees()
}

/// 副階層を含む導出はUnity採取と等価である。
#[test]
fn 副階層を含む導出はunity採取と等価である() {
    let Some((fbx_path, prefab_path, assets_root, truth_path)) = 実fixture() else {
        eprintln!("実fixtureが無いため跳ばす（VRCTOM_MILFY_CHECK未設定）");
        return;
    };
    let derived = Milfyを導出する(&fbx_path, &prefab_path, &assets_root).expect("導出できる");
    let truth_bytes = std::fs::read(&truth_path).expect("正解を読める");
    let derived_json: serde_json::Value =
        serde_json::from_slice(&derived).expect("導出はJSONである");
    let truth_json: serde_json::Value =
        serde_json::from_slice(&truth_bytes).expect("正解はJSONである");

    // sourceModelsは完全一致する（採取メタのgenerator/capture以外）。
    assert_eq!(
        derived_json["sourceModels"], truth_json["sourceModels"],
        "sourceModelsが一致しない"
    );

    // ノードは287件で、対応・順序・出所が完全一致する。
    let derived_nodes = derived_json["nodes"].as_array().expect("nodesは配列である");
    let truth_nodes = truth_json["nodes"].as_array().expect("nodesは配列である");
    assert_eq!(derived_nodes.len(), truth_nodes.len(), "ノード件数");
    assert_eq!(derived_nodes.len(), 287, "ノード件数は287である");
    let mut 不一致 = Vec::new();
    for (want, got) in truth_nodes.iter().zip(derived_nodes.iter()) {
        let id = want["id"].as_str().unwrap_or("?");
        for key in [
            "id",
            "parentId",
            "siblingIndex",
            "scenePath",
            "source",
            "referenceSource",
        ] {
            if want[key] != got[key] {
                不一致.push(format!("{id} {key}: {:?} != {:?}", want[key], got[key]));
            }
        }
        let wp = &want["referenceLocal"]["position"];
        let gp = &got["referenceLocal"]["position"];
        let pos = ((wp[0].as_f64().unwrap_or(0.0) - gp[0].as_f64().unwrap_or(0.0)).powi(2)
            + (wp[1].as_f64().unwrap_or(0.0) - gp[1].as_f64().unwrap_or(0.0)).powi(2)
            + (wp[2].as_f64().unwrap_or(0.0) - gp[2].as_f64().unwrap_or(0.0)).powi(2))
        .sqrt();
        let wr = &want["referenceLocal"]["rotation"];
        let gr = &got["referenceLocal"]["rotation"];
        let to_vec = |v: &serde_json::Value| {
            [
                v[0].as_f64().unwrap_or(0.0),
                v[1].as_f64().unwrap_or(0.0),
                v[2].as_f64().unwrap_or(0.0),
                v[3].as_f64().unwrap_or(0.0),
            ]
        };
        let ang = 角度差(&to_vec(wr), &to_vec(gr));
        let ws = &want["referenceLocal"]["scale"];
        let gs = &got["referenceLocal"]["scale"];
        let scl = ((ws[0].as_f64().unwrap_or(0.0) - gs[0].as_f64().unwrap_or(0.0)).powi(2)
            + (ws[1].as_f64().unwrap_or(0.0) - gs[1].as_f64().unwrap_or(0.0)).powi(2)
            + (ws[2].as_f64().unwrap_or(0.0) - gs[2].as_f64().unwrap_or(0.0)).powi(2))
        .sqrt();
        if ang > 1e-5 || pos > 1e-6 || scl > 1e-5 {
            不一致.push(format!(
                "{id} 差: 回転{ang:.6}度 位置{pos:.2e} 拡大率{scl:.2e}"
            ));
        }
    }
    assert!(不一致.is_empty(), "ノードの不一致:\n{}", 不一致.join("\n"));

    // PhysBone・collider・制約・humanoid・lookAtは4ulp許容で一致する。
    for key in [
        "physBones",
        "colliders",
        "constraints",
        "humanoid",
        "lookAt",
    ] {
        jsonが等しいことを確かめる(&truth_json[key], &derived_json[key], key);
    }
    assert_eq!(
        derived_json["physBones"]
            .as_array()
            .expect("配列である")
            .len(),
        39
    );
    assert_eq!(
        derived_json["colliders"]
            .as_array()
            .expect("配列である")
            .len(),
        12
    );
    assert_eq!(
        derived_json["constraints"]
            .as_array()
            .expect("配列である")
            .len(),
        11
    );
    assert_eq!(
        derived_json["humanoid"]["bones"]
            .as_array()
            .expect("配列である")
            .len(),
        53
    );
}

/// 四元数の角度差（度）を安定な式で求める。符号の違いは無視する。
/// `acos` は精度が足りないため、`2*atan2(|p-q|,|p+q|)` を使う。
fn 安定角度差(a: &[f64; 4], b: &[f64; 4]) -> f64 {
    let diff = ((a[0] - b[0]).powi(2)
        + (a[1] - b[1]).powi(2)
        + (a[2] - b[2]).powi(2)
        + (a[3] - b[3]).powi(2))
    .sqrt();
    let sum = ((a[0] + b[0]).powi(2)
        + (a[1] + b[1]).powi(2)
        + (a[2] + b[2]).powi(2)
        + (a[3] + b[3]).powi(2))
    .sqrt();
    let (小, 大) = (diff.min(sum), diff.max(sum));
    if 大 <= 0.0 || !大.is_finite() {
        return 0.0;
    }
    2.0 * (小 / 大).atan().to_degrees()
}

/// JSON値を比べ、4ulp以内の浮動小数点差だけを許す。
/// 戻り値は最初の不一致の説明であり、無ければ `None` である。
/// `serde_json` の10進解析が正しく丸めない既知の丸めを吸収する。
/// 例：`0.029999999329447746` を1ulp低く（正しくは `3f9eb851e0000000`、
/// 誤って `...dfffffff`）、`-0.23119299113750459` を1ulp高く読む。
/// 導出側の最短表記（例 `-0.23119299113750458`）も1ulp低く読むため、
/// 両側の誤読が重なると最大2ulpの差になる。余裕を見て4ulpまで許す。
/// 正しく丸める処理系（Python・Rustの `str::parse`）では両表記とも同じ値になる。
/// 実質の量子化誤差（float32由来の約1e-7以上、f64で数千ulp相当）は許さない。
fn json差を探す(
    経路: &str,
    want: &serde_json::Value,
    got: &serde_json::Value,
) -> Option<String> {
    use serde_json::Value as V;
    // 完全一致は速やかに通す。整数・真偽・文字列・nullはここで確定する。
    if want == got {
        return None;
    }
    match (want, got) {
        (V::Number(w), V::Number(g)) => {
            let wf = w.as_f64();
            let gf = g.as_f64();
            if let (Some(wf), Some(gf)) = (wf, gf) {
                if wf == gf {
                    return None;
                }
                // 符号・指数が同じで仮数4ulp以内の差だけを許す。
                // `0.03` のf32換算値で実測した1ulp（約3.5e-18）相当であり、
                // 曲線傾き `-0.23` 系で実測した2ulpも通す。
                // float32量子化の差（数千ulp相当）は通さない。
                let ずれ = wf.to_bits().abs_diff(gf.to_bits());
                if ずれ <= 4 {
                    return None;
                }
                Some(format!(
                    "{経路}: 浮動小数点が4ulpを超える got={gf}（{:016x}） want={wf}（{:016x}）",
                    gf.to_bits(),
                    wf.to_bits(),
                ))
            } else {
                Some(format!("{経路}: 数値の型が違う got={g} want={w}"))
            }
        }
        (V::Array(w), V::Array(g)) => {
            if w.len() != g.len() {
                return Some(format!(
                    "{経路}: 配列件数が違う got={} want={}",
                    g.len(),
                    w.len()
                ));
            }
            for (i, (w, g)) in w.iter().zip(g.iter()).enumerate() {
                if let Some(差) = json差を探す(&format!("{経路}[{i}]"), w, g) {
                    return Some(差);
                }
            }
            None
        }
        (V::Object(w), V::Object(g)) => {
            if w.len() != g.len() {
                return Some(format!(
                    "{経路}: 欄数が違う got={} want={}",
                    g.len(),
                    w.len()
                ));
            }
            for (k, wv) in w {
                match g.get(k) {
                    Some(gv) => {
                        if let Some(差) = json差を探す(&format!("{経路}.{k}"), wv, gv) {
                            return Some(差);
                        }
                    }
                    None => return Some(format!("{経路}: 欄が無い got側に{k}が無い")),
                }
            }
            None
        }
        _ => Some(format!("{経路}: 型か値が違う got={got} want={want}")),
    }
}

/// JSON値が4ulp許容で等価であることを確かめる。
/// 不一致の場合は最初の差を添えて失敗する。
fn jsonが等しいことを確かめる(
    want: &serde_json::Value,
    got: &serde_json::Value,
    欄: &str,
) {
    if let Some(差) = json差を探す(欄, want, got) {
        panic!("{欄}が一致しない: {差}");
    }
}

/// 実fixtureの場所を返す。MANUKA用。無ければ `None`（跳ばす）。
fn Manuka実fixture() -> Option<(PathBuf, PathBuf, PathBuf, PathBuf)> {
    let root = std::env::var_os("VRCTOM_MILFY_CHECK").map(PathBuf::from)?;
    let fbx = root.join("Assets/MANUKA/FBX/MANUKA.fbx");
    let prefab = root.join("Assets/MANUKA/Prefab/MANUKA_lilToon.prefab");
    let assets = root.clone();
    let truth =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../references/fixtures/Manuka-physics.json");
    if !fbx.is_file() || !prefab.is_file() || !assets.join("Assets").is_dir() || !truth.is_file() {
        return None;
    }
    Some((fbx, prefab, assets, truth))
}

/// 実fixtureの場所を返す。Chiffon用。無ければ `None`（跳ばす）。
fn Chiffon実fixture() -> Option<(PathBuf, PathBuf, PathBuf, PathBuf)> {
    let root = std::env::var_os("VRCTOM_MILFY_CHECK").map(PathBuf::from)?;
    let fbx = root.join("Assets/Amatousagi/Chiffon/FBX/Chiffon_ver1.0.0.fbx");
    let prefab = root.join("Assets/Amatousagi/Chiffon/Chiffon.prefab");
    let assets = root.clone();
    let truth = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../references/fixtures/Chiffon-physics.json");
    if !fbx.is_file() || !prefab.is_file() || !assets.join("Assets").is_dir() || !truth.is_file() {
        return None;
    }
    Some((fbx, prefab, assets, truth))
}

/// 実fixtureの場所を返す。Ciel用。無ければ `None`（跳ばす）。
fn Ciel実fixture() -> Option<(PathBuf, PathBuf, PathBuf, PathBuf)> {
    let root = std::env::var_os("VRCTOM_MILFY_CHECK").map(PathBuf::from)?;
    let fbx = root.join("Assets/ArmoireLepus/VRC_ciel/ciel_FBX/Ciel_1.0.fbx");
    let prefab = root.join("Assets/ArmoireLepus/VRC_ciel/ciel_Prefab/Ciel_VRC_1.0.prefab");
    let assets = root.clone();
    let truth =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../references/fixtures/Ciel-physics.json");
    if !fbx.is_file() || !prefab.is_file() || !assets.join("Assets").is_dir() || !truth.is_file() {
        return None;
    }
    Some((fbx, prefab, assets, truth))
}

/// 導出sidecarと正解sidecarのノード列を比べ、最大差を返す。
/// 戻り値は（不一致の説明文、最大回転差、最大位置差）である。
/// 回転差は安定な式 `2*atan2(|p-q|,|p+q|)` で評価する。
/// `keys` に挙げた欄だけを比べる（出所は経路により扱いが違う）。
fn ノード列を比べる(
    truth_json: &serde_json::Value,
    derived_json: &serde_json::Value,
    keys: &[&str],
) -> (Vec<String>, f64, f64) {
    let truth_nodes = truth_json["nodes"].as_array().expect("nodesは配列である");
    let derived_nodes = derived_json["nodes"].as_array().expect("nodesは配列である");
    assert_eq!(
        derived_nodes.len(),
        truth_nodes.len(),
        "ノード件数が一致しない"
    );
    let mut 不一致 = Vec::new();
    let mut 最大回転 = 0.0f64;
    let mut 最大位置 = 0.0f64;
    for (want, got) in truth_nodes.iter().zip(derived_nodes.iter()) {
        let id = want["id"].as_str().unwrap_or("?");
        for key in keys {
            if want[key] != got[key] {
                不一致.push(format!("{id} {key}: {:?} != {:?}", want[key], got[key]));
            }
        }
        let to_vec = |v: &serde_json::Value| {
            [
                v[0].as_f64().unwrap_or(0.0),
                v[1].as_f64().unwrap_or(0.0),
                v[2].as_f64().unwrap_or(0.0),
                v[3].as_f64().unwrap_or(0.0),
            ]
        };
        let wr = &want["referenceLocal"]["rotation"];
        let gr = &got["referenceLocal"]["rotation"];
        let 回転 = 安定角度差(&to_vec(wr), &to_vec(gr));
        if 回転 > 最大回転 {
            最大回転 = 回転;
        }
        let wp = &want["referenceLocal"]["position"];
        let gp = &got["referenceLocal"]["position"];
        let 位置 = (0..3)
            .map(|i| (wp[i].as_f64().unwrap_or(0.0) - gp[i].as_f64().unwrap_or(0.0)).abs())
            .fold(0.0f64, f64::max);
        最大位置 = 最大位置.max(位置);
    }
    (不一致, 最大回転, 最大位置)
}

/// 姿勢上書きを適用した導出はUnity採取と等価である（MANUKA）。
#[test]
fn 姿勢上書きを適用した導出はunity採取と等価である() {
    let Some((fbx_path, prefab_path, assets_root, truth_path)) = Manuka実fixture() else {
        eprintln!("実fixtureが無いため跳ばす（VRCTOM_MILFY_CHECK未設定）");
        return;
    };
    let derived = 導出する(
        &fbx_path,
        &prefab_path,
        &assets_root,
        "Assets/MANUKA/FBX/MANUKA.fbx",
    )
    .expect("導出できる");
    let truth_bytes = std::fs::read(&truth_path).expect("正解を読める");
    let derived_json: serde_json::Value =
        serde_json::from_slice(&derived).expect("導出はJSONである");
    let truth_json: serde_json::Value =
        serde_json::from_slice(&truth_bytes).expect("正解はJSONである");
    // sourceModelsは完全一致する。
    assert_eq!(
        derived_json["sourceModels"], truth_json["sourceModels"],
        "sourceModelsが一致しない"
    );
    // ノードは268件で、対応・順序・出所が完全一致する。
    let derived_nodes = derived_json["nodes"].as_array().expect("nodesは配列である");
    assert_eq!(derived_nodes.len(), 268, "ノード件数は268である");
    let (不一致, 最大回転, 最大位置) = ノード列を比べる(
        &truth_json,
        &derived_json,
        &[
            "id",
            "parentId",
            "siblingIndex",
            "scenePath",
            "source",
            "referenceSource",
        ],
    );
    // 参照出所はbind分解の境界で11件だけ違う。
    // Unity側だけが申告する10件（脚鎖）と、こちらだけが申告する1件
    // （`n:36`）であり、値の差と合わせて下で確かめる。
    let mut 出所不一致: Vec<&str> = Vec::new();
    let mut 実質不一致 = Vec::new();
    for 文 in &不一致 {
        if 文.contains("referenceSource") {
            出所不一致.push(文.as_str());
        } else {
            実質不一致.push(文.as_str());
        }
    }
    assert!(
        実質不一致.is_empty(),
        "ノードの不一致:\n{}",
        実質不一致.join("\n")
    );
    let mut 出所集合: Vec<&str> = 出所不一致
        .iter()
        .map(|文| 文.split_whitespace().next().unwrap_or("?"))
        .collect();
    出所集合.sort_unstable();
    assert_eq!(
        出所集合,
        vec![
            "n:228", "n:229", "n:230", "n:231", "n:232", "n:236", "n:237", "n:238", "n:239",
            "n:240", "n:36",
        ],
        "参照出所の不一致は既知の11件である"
    );
    // 参照ローカルの差は、bind分解の境界由来の既知のずれに収まる。
    // Manuka_pants_ribbon系の2件が最大0.05度、脚鎖の10件が分解可否の違いである。
    // 位置は0.3mm以内に収まる。
    assert!(
        最大回転 < 0.05,
        "回転差が既知のずれを超える: {最大回転:.6}度"
    );
    assert!(
        最大位置 < 5.0e-4,
        "位置差が既知のずれを超える: {最大位置:.2e}"
    );
    // PhysBone・collider・制約・humanoid・lookAtは4ulp許容で一致する。
    for key in [
        "physBones",
        "colliders",
        "constraints",
        "humanoid",
        "lookAt",
    ] {
        jsonが等しいことを確かめる(&truth_json[key], &derived_json[key], key);
    }
    assert_eq!(
        derived_json["physBones"]
            .as_array()
            .expect("配列である")
            .len(),
        31
    );
    assert_eq!(
        derived_json["colliders"]
            .as_array()
            .expect("配列である")
            .len(),
        16
    );
    assert_eq!(
        derived_json["constraints"]
            .as_array()
            .expect("配列である")
            .len(),
        4
    );
    assert_eq!(
        derived_json["humanoid"]["bones"]
            .as_array()
            .expect("配列である")
            .len(),
        51
    );
    // 未対応機能の差は、TRS分解の境界由来の既知の11件である。
    // Unity側だけが申告する脚鎖10件と、導出側だけが申告する `n:36` 1件である。
    // `n:36` は参照出所の不一致（11件）と同じ節点であり、値の差と合わせて上で確かめた。
    let truth_unsupported: Vec<String> =
        serde_json::from_value(truth_json["unsupportedFeatures"].clone()).expect("配列である");
    let derived_unsupported: Vec<String> =
        serde_json::from_value(derived_json["unsupportedFeatures"].clone()).expect("配列である");
    let only_derived: Vec<&String> = derived_unsupported
        .iter()
        .filter(|entry| !truth_unsupported.contains(entry))
        .collect();
    assert_eq!(
        only_derived,
        vec!["trs-decomposition:n:36"],
        "導出だけの申告は既知の1件である"
    );
    let only_truth: Vec<&String> = truth_unsupported
        .iter()
        .filter(|entry| !derived_unsupported.contains(entry))
        .collect();
    assert_eq!(only_truth.len(), 10, "採取だけの申告は10件である");
    for entry in &only_truth {
        assert!(
            entry.starts_with("trs-decomposition:"),
            "採取だけの申告はTRS分解である: {entry}"
        );
    }
}

/// 追加物の階層を含む導出はUnity採取と等価である（Chiffon）。
#[test]
fn 追加物の階層を含む導出はunity採取と等価である() {
    let Some((fbx_path, prefab_path, assets_root, truth_path)) = Chiffon実fixture() else {
        eprintln!("実fixtureが無いため跳ばす（VRCTOM_MILFY_CHECK未設定）");
        return;
    };
    let derived = 導出する(
        &fbx_path,
        &prefab_path,
        &assets_root,
        "Assets/Amatousagi/Chiffon/FBX/Chiffon_ver1.0.0.fbx",
    )
    .expect("導出できる");
    let truth_bytes = std::fs::read(&truth_path).expect("正解を読める");
    let derived_json: serde_json::Value =
        serde_json::from_slice(&derived).expect("導出はJSONである");
    let truth_json: serde_json::Value =
        serde_json::from_slice(&truth_bytes).expect("正解はJSONである");
    assert_eq!(
        derived_json["sourceModels"], truth_json["sourceModels"],
        "sourceModelsが一致しない"
    );
    let derived_nodes = derived_json["nodes"].as_array().expect("nodesは配列である");
    assert_eq!(derived_nodes.len(), 231, "ノード件数は231である");
    let (不一致, 最大回転, 最大位置) = ノード列を比べる(
        &truth_json,
        &derived_json,
        &[
            "id",
            "parentId",
            "siblingIndex",
            "scenePath",
            "source",
            "referenceSource",
        ],
    );
    assert!(不一致.is_empty(), "ノードの不一致:\n{}", 不一致.join("\n"));
    // 参照ローカルはfloat32由来の丸めに収まる。
    assert!(最大回転 < 1e-4, "回転差が丸めを超える: {最大回転:.6}度");
    assert!(最大位置 < 2e-6, "位置差が丸めを超える: {最大位置:.2e}");
    for key in [
        "physBones",
        "colliders",
        "constraints",
        "humanoid",
        "lookAt",
        "unsupportedFeatures",
    ] {
        jsonが等しいことを確かめる(&truth_json[key], &derived_json[key], key);
    }
    assert_eq!(
        derived_json["physBones"]
            .as_array()
            .expect("配列である")
            .len(),
        19
    );
    assert_eq!(
        derived_json["colliders"]
            .as_array()
            .expect("配列である")
            .len(),
        7
    );
    assert_eq!(
        derived_json["constraints"]
            .as_array()
            .expect("配列である")
            .len(),
        0
    );
    assert_eq!(
        derived_json["humanoid"]["bones"]
            .as_array()
            .expect("配列である")
            .len(),
        53
    );
}

/// 直接記述の導出はUnity採取と等価である（Ciel）。
#[test]
fn 直接記述の導出はunity採取と等価である() {
    let Some((fbx_path, prefab_path, assets_root, truth_path)) = Ciel実fixture() else {
        eprintln!("実fixtureが無いため跳ばす（VRCTOM_MILFY_CHECK未設定）");
        return;
    };
    let derived = 導出する(
        &fbx_path,
        &prefab_path,
        &assets_root,
        "Assets/ArmoireLepus/VRC_ciel/ciel_FBX/Ciel_1.0.fbx",
    )
    .expect("導出できる");
    let truth_bytes = std::fs::read(&truth_path).expect("正解を読める");
    let derived_json: serde_json::Value =
        serde_json::from_slice(&derived).expect("導出はJSONである");
    let truth_json: serde_json::Value =
        serde_json::from_slice(&truth_bytes).expect("正解はJSONである");
    // Cielの正解は `sourceModels` が空であり、Unity側がFBXを特定できていない。
    // 導出側はメッシュのGUIDから埋めるため、内容の一致だけ確かめる。
    let derived_models = derived_json["sourceModels"].as_array().expect("配列である");
    assert_eq!(derived_models.len(), 2, "sourceModelsは2件である");
    assert_eq!(derived_models[0]["id"], "ciel_1.0");
    assert_eq!(derived_models[1]["id"], "ciel_stick");
    let derived_nodes = derived_json["nodes"].as_array().expect("nodesは配列である");
    assert_eq!(derived_nodes.len(), 288, "ノード件数は288である");
    // 親子・名前・兄弟順・シーンパスは完全一致する。出所はUnity側が
    // 空のため、一致件数だけ数える（対応が取れた節点は埋まる）。
    let truth_nodes = truth_json["nodes"].as_array().expect("nodesは配列である");
    let mut source_一致 = 0;
    for (want, got) in truth_nodes.iter().zip(derived_nodes.iter()) {
        let id = want["id"].as_str().unwrap_or("?");
        for key in ["id", "parentId", "siblingIndex", "scenePath"] {
            assert_eq!(want[key], got[key], "{id} {key}が一致しない");
        }
        if want["source"] == got["source"] {
            source_一致 += 1;
        }
    }
    // 出所が一致するのは対応が取れない9件（Transformのみ）である。
    // 残り279件は導出側だけが埋める（Unity側は空）。
    assert_eq!(source_一致, 9, "出所の一致は未対応の9件である");
    let (不一致, 最大回転, 最大位置) = ノード列を比べる(
        &truth_json,
        &derived_json,
        &[
            "id",
            "parentId",
            "siblingIndex",
            "scenePath",
            "referenceSource",
        ],
    );
    // 出所の差は想定内であり、ここでは比べない（上で件数を確認済み）。
    // 参照出所はbind分解の境界で22件だけ違う。
    // Unity側だけが申告するツインテール・スカート鎖であり、値の差と合わせて下で確かめる。
    let mut 出所不一致: Vec<&str> = Vec::new();
    let mut 実質不一致 = Vec::new();
    for 文 in &不一致 {
        if 文.contains("referenceSource") {
            出所不一致.push(文.as_str());
        } else {
            実質不一致.push(文.as_str());
        }
    }
    assert!(
        実質不一致.is_empty(),
        "ノードの不一致:\n{}",
        実質不一致.join("\n")
    );
    let mut 出所集合: Vec<&str> = 出所不一致
        .iter()
        .map(|文| 文.split_whitespace().next().unwrap_or("?"))
        .collect();
    出所集合.sort_unstable();
    assert_eq!(
        出所集合,
        vec![
            "n:213", "n:216", "n:219", "n:225", "n:228", "n:231", "n:234", "n:237", "n:243",
            "n:246", "n:249", "n:252", "n:47", "n:48", "n:49", "n:50", "n:51", "n:55", "n:56",
            "n:57", "n:58", "n:59",
        ],
        "参照出所の不一致は既知の22件である"
    );
    // 参照ローカルの差は、bind分解の境界由来の既知のずれに収まる。
    assert!(
        最大回転 < 0.06,
        "回転差が既知のずれを超える: {最大回転:.6}度"
    );
    assert!(
        最大位置 < 5.0e-4,
        "位置差が既知のずれを超える: {最大位置:.2e}"
    );
    // PhysBone・collider・制約・humanoid・lookAtは4ulp許容で一致する。
    for key in [
        "physBones",
        "colliders",
        "constraints",
        "humanoid",
        "lookAt",
    ] {
        jsonが等しいことを確かめる(&truth_json[key], &derived_json[key], key);
    }
    assert_eq!(
        derived_json["physBones"]
            .as_array()
            .expect("配列である")
            .len(),
        45
    );
    assert_eq!(
        derived_json["colliders"]
            .as_array()
            .expect("配列である")
            .len(),
        5
    );
    assert_eq!(
        derived_json["constraints"]
            .as_array()
            .expect("配列である")
            .len(),
        0
    );
    assert_eq!(
        derived_json["humanoid"]["bones"]
            .as_array()
            .expect("配列である")
            .len(),
        51
    );
    // 未対応機能の差は、Unity側だけが申告するTRS分解である。
    // 導出側は分解できるため申告しない。由来が逆の差分が無いことを確かめる。
    let truth_unsupported: Vec<String> =
        serde_json::from_value(truth_json["unsupportedFeatures"].clone()).expect("配列である");
    let derived_unsupported: Vec<String> =
        serde_json::from_value(derived_json["unsupportedFeatures"].clone()).expect("配列である");
    for entry in &derived_unsupported {
        assert!(
            truth_unsupported.contains(entry),
            "導出だけの未対応機能がある: {entry}"
        );
    }
    for entry in truth_unsupported
        .iter()
        .filter(|entry| !derived_unsupported.contains(entry))
    {
        assert!(
            entry.starts_with("trs-decomposition:"),
            "採取だけの申告はTRS分解である: {entry}"
        );
    }
}
