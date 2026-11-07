//! Prefab導出のfs経路とメモリ経路の等価契約（実fixtureを使う）。
//!
//! 同じ資産スナップショットをファイルシステム経由の互換入口と
//! メモリ読み手を使った共通入口で読み、生成sidecar JSONが意味的に一致する。
//! `VRCTOM_MILFY_CHECK` が指すUnityプロジェクト（`references/unity-milfy-vrm-check`）
//! が無い場合は跳ばす。跳ばした場合は完了証拠にしない。
//! 実行例: `VRCTOM_MILFY_CHECK=$PWD/references/unity-milfy-vrm-check cargo nextest run -p vrctom --test vrm_physics_derive_source`

// テスト名は日本語で書く（テストは仕様・契約である）。
#![allow(non_snake_case)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use vrctom::fbx::{FbxFile, FbxOptions};
use vrctom::unity::{AssetIndex, AssetMeta, AssetSource, ParseOptions, SerializedFile};
use vrctom::vrm::{
    PhysicsDeriveLimits, PrefabDeriveInput, PrefabDeriveSource, derive_physics_sidecar,
    derive_physics_sidecar_with,
};

/// 実fixtureの場所を返す。無ければ `None`（跳ばす）。
fn 実fixture() -> Option<(PathBuf, PathBuf, PathBuf)> {
    let root = std::env::var_os("VRCTOM_MILFY_CHECK").map(PathBuf::from)?;
    let fbx = root.join("Assets/PLUSONE/Milfy/FBX/Milfy.fbx");
    let prefab = root.join("Assets/PLUSONE/Milfy/Prefab/Milfy.prefab");
    let assets = root.clone();
    if !fbx.is_file() || !prefab.is_file() || !assets.join("Assets").is_dir() {
        return None;
    }
    Some((fbx, prefab, assets))
}

/// 試験用のメモリ読み手。`Assets` 直下基準の相対パスで引く。
struct 試験メモリ読み手 {
    表: HashMap<String, Vec<u8>>,
}

impl AssetSource for 試験メモリ読み手 {
    fn read_limited(&self, path: &str, limit: u64) -> std::io::Result<Vec<u8>> {
        let Some(bytes) = self.表.get(path) else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("試験メモリに無い: {path}"),
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

/// `Assets` 配下を再帰走査し、相対パス（`/` 区切り）と内容の表を作る。
fn 資産表を作る(assets_dir: &Path) -> Result<HashMap<String, Vec<u8>>, String> {
    let mut 表 = HashMap::new();
    let mut 積み = vec![PathBuf::new()];
    while let Some(相対) = 積み.pop() {
        let 実体 = assets_dir.join(&相対);
        let 一覧 = std::fs::read_dir(&実体).map_err(|e| e.to_string())?;
        for 項目 in 一覧 {
            let 項目 = 項目.map_err(|e| e.to_string())?;
            let 名前 = 項目
                .file_name()
                .into_string()
                .map_err(|_| "UTF-8でない名前がある".to_owned())?;
            let 子相対 = if 相対.as_os_str().is_empty() {
                PathBuf::from(&名前)
            } else {
                相対.join(&名前)
            };
            let 種別 = 項目.file_type().map_err(|e| e.to_string())?;
            if 種別.is_dir() {
                積み.push(子相対);
                continue;
            }
            if !種別.is_file() {
                continue;
            }
            let 内容 = std::fs::read(項目.path()).map_err(|e| e.to_string())?;
            let 相対文 = 子相対
                .components()
                .map(|部品| {
                    部品
                        .as_os_str()
                        .to_str()
                        .map(str::to_owned)
                        .ok_or_else(|| "UTF-8でない名前がある".to_owned())
                })
                .collect::<Result<Vec<_>, _>>()?
                .join("/");
            表.insert(相対文, 内容);
        }
    }
    Ok(表)
}

/// メモリ表から索引を作る。`.meta` を正とし、決定的な順序で挿入する。
fn 索引を作る(表: &HashMap<String, Vec<u8>>) -> Result<AssetIndex, String> {
    let mut 記録: Vec<(String, AssetMeta)> = Vec::new();
    for (相対, 内容) in 表 {
        let Some(資産) = 相対.strip_suffix(".meta") else {
            continue;
        };
        let meta = AssetMeta::parse(内容).map_err(|e| e.to_string())?;
        記録.push((資産.to_owned(), meta));
    }
    記録.sort_by(|a, b| a.0.cmp(&b.0));
    let mut 索引 = AssetIndex::new();
    for (資産, meta) in 記録 {
        索引.insert(&資産, meta).map_err(|e| e.to_string())?;
    }
    Ok(索引)
}

/// 同じスナップショットのfs導出とメモリ導出は意味的に一致する。
#[test]
fn 同じ資産のfs導出とメモリ導出は一致する() {
    let Some((fbx_path, prefab_path, assets_root)) = 実fixture() else {
        eprintln!("実fixtureが無いため跳ばす（VRCTOM_MILFY_CHECK未設定）");
        return;
    };
    let fbx_asset_path = "Assets/PLUSONE/Milfy/FBX/Milfy.fbx";
    let fbx_bytes = std::fs::read(&fbx_path).expect("実FBXは読める");
    let fbx = FbxFile::parse(&fbx_bytes, &FbxOptions::default()).expect("実FBXは解析できる");
    let prefab_bytes = std::fs::read(&prefab_path).expect("実Prefabは読める");
    let prefab_label = format!("{prefab_path:?}");
    let prefab =
        SerializedFile::parse(&prefab_bytes, &ParseOptions::new()).expect("実Prefabは解析できる");

    // fs経路の導出。既存CLIと同じ互換入口を使う。
    let fs入力 = PrefabDeriveInput {
        fbx_bytes: &fbx_bytes,
        fbx: &fbx,
        assets_root: &assets_root,
        prefab: &prefab,
        prefab_label: &prefab_label,
        fbx_asset_path,
    };
    let fs結果 = derive_physics_sidecar(&fs入力).expect("fs導出できる");

    // メモリ経路の導出。同じ配置を表に写し、共通入口で読む。
    let assets_dir = assets_root.join("Assets");
    let 表 = 資産表を作る(&assets_dir).expect("資産表を作れる");
    let 索引 = 索引を作る(&表).expect("索引を作れる");
    let 読み手 = 試験メモリ読み手 { 表 };
    let メモリ入力 = PrefabDeriveSource {
        fbx_bytes: &fbx_bytes,
        fbx: &fbx,
        source: &読み手,
        index: &索引,
        prefab: &prefab,
        prefab_label: &prefab_label,
        fbx_asset_path,
        limits: PhysicsDeriveLimits::default(),
    };
    let メモリ結果 = derive_physics_sidecar_with(&メモリ入力).expect("メモリ導出できる");

    // 生成sidecar JSONは意味的に一致する。鍵順や空白の違いを吸収するため値で比べる。
    let fs値: serde_json::Value = serde_json::from_slice(&fs結果.json).expect("fs導出はJSONである");
    let メモリ値: serde_json::Value =
        serde_json::from_slice(&メモリ結果.json).expect("メモリ導出はJSONである");
    assert_eq!(メモリ値, fs値, "メモリ導出はfs導出と意味的に一致する");
    assert_eq!(メモリ結果.ignored_types, fs結果.ignored_types);
    assert_eq!(メモリ結果.unmatched_nodes, fs結果.unmatched_nodes);
}

/// 主FBXの上限超過は切り詰めず失敗する。
#[test]
fn 主FBXの上限超過は共通入口で失敗する() {
    let Some((fbx_path, prefab_path, assets_root)) = 実fixture() else {
        eprintln!("実fixtureが無いため跳ばす（VRCTOM_MILFY_CHECK未設定）");
        return;
    };
    let fbx_asset_path = "Assets/PLUSONE/Milfy/FBX/Milfy.fbx";
    let fbx_bytes = std::fs::read(&fbx_path).expect("実FBXは読める");
    let fbx = FbxFile::parse(&fbx_bytes, &FbxOptions::default()).expect("実FBXは解析できる");
    let prefab_bytes = std::fs::read(&prefab_path).expect("実Prefabは読める");
    let prefab_label = format!("{prefab_path:?}");
    let prefab =
        SerializedFile::parse(&prefab_bytes, &ParseOptions::new()).expect("実Prefabは解析できる");
    let assets_dir = assets_root.join("Assets");
    let 表 = 資産表を作る(&assets_dir).expect("資産表を作れる");
    let 索引 = 索引を作る(&表).expect("索引を作れる");
    let 読み手 = 試験メモリ読み手 { 表 };
    // 実測（主FBX約17MB）に届かない上限では失敗する。
    let 制限 = PhysicsDeriveLimits::new().with_max_main_fbx_bytes(1);
    let 入力 = PrefabDeriveSource {
        fbx_bytes: &fbx_bytes,
        fbx: &fbx,
        source: &読み手,
        index: &索引,
        prefab: &prefab,
        prefab_label: &prefab_label,
        fbx_asset_path,
        limits: 制限,
    };
    let 失敗 = match derive_physics_sidecar_with(&入力) {
        Ok(_) => panic!("主FBXの上限超過は失敗する"),
        Err(失敗) => 失敗,
    };
    assert_eq!(
        失敗.kind(),
        vrctom::vrm::VrmErrorKind::LimitExceeded,
        "上限超過はLimitExceededである"
    );
}
