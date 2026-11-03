//! Prefabからの物理sidecar導出とUnity採取の等価契約（実fixtureを使う）。
//!
//! `VRCTOM_MILFY_CHECK` が指すUnityプロジェクト（`references/unity-milfy-vrm-check`）
//! が無い場合は跳ばす。跳ばした場合は完了証拠にしない。
//! 実行例: `VRCTOM_MILFY_CHECK=$PWD/references/unity-milfy-vrm-check cargo test -p vrctom --test vrm_physics_derive`

use std::path::{Path, PathBuf};

use vrctom::fbx::{FbxFile, FbxOptions};
use vrctom::unity::{ParseOptions, SerializedFile};
use vrctom::vrm::{PrefabDeriveInput, derive_physics_sidecar};

// テスト名は日本語で書く（テストは仕様・契約である）。

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
) -> Result<Vec<u8>, String> {
    let fbx_bytes = std::fs::read(fbx_path).map_err(|e| e.to_string())?;
    let fbx = FbxFile::parse(&fbx_bytes, &FbxOptions::default()).map_err(|e| e.to_string())?;
    let prefab_bytes = std::fs::read(prefab_path).map_err(|e| e.to_string())?;
    let prefab_label = format!("{prefab_path:?}");
    let prefab =
        SerializedFile::parse(&prefab_bytes, &ParseOptions::new()).map_err(|e| e.to_string())?;
    let fbx_asset_path = "Assets/PLUSONE/Milfy/FBX/Milfy.fbx";
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
    let derived = 導出する(&fbx_path, &prefab_path, &assets_root).expect("導出できる");
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

    // PhysBone・collider・制約・humanoid・lookAtは完全一致する。
    for key in [
        "physBones",
        "colliders",
        "constraints",
        "humanoid",
        "lookAt",
    ] {
        assert_eq!(derived_json[key], truth_json[key], "{key}が完全一致しない");
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
