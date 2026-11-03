//! Prefab・FBX・`.fbx.meta` から物理sidecarをUnityなしで導出する。
//!
//! 入口は [`derive_physics_sidecar`] であり、FBXバイト列・Assets配置・
//! Prefab文書からUnityヘルパと同一内容のsidecar JSONを組み立てる。
//! 型判定はスクリプトfileIDのMD4計算で行い、`(guid, fileID)` の表を持たない。
//! 旧方式（2019.3より前の取り込みfileID）・参照切れ・未知型は黙って落とさず失敗する。

#[cfg(not(target_arch = "wasm32"))]
use std::collections::{HashMap, HashSet};

#[cfg(not(target_arch = "wasm32"))]
use yaml_serde::Value;

use super::error::{VrmError, VrmErrorKind};
use crate::fbx::FbxFile;
#[cfg(not(target_arch = "wasm32"))]
use crate::fbx::{FbxOptions, LocalTransform};
use crate::unity::SerializedFile;
#[cfg(not(target_arch = "wasm32"))]
use crate::unity::{AssetMeta, FileId, TypedObject};

/// 参照ローカル1件。位置・回転・拡大率と出所（`bind`・`scene` 等）の組である。
/// 戻り値や引数の型が長くなるため別名にする。
#[cfg(not(target_arch = "wasm32"))]
type ReferenceLocal = ([f64; 3], [f64; 4], [f64; 3], &'static str);

/// float32のTRS1件。分解・合成の中間形である。
#[cfg(not(target_arch = "wasm32"))]
type TrsF32 = ([f32; 3], [f32; 4], [f32; 3]);

/// float64のTRS1件。skeletonのT-pose等の中間形である。
#[cfg(not(target_arch = "wasm32"))]
type TrsF64 = ([f64; 3], [f64; 4], [f64; 3]);

/// 視線の1方向。linked・左・右の組である。
#[cfg(not(target_arch = "wasm32"))]
type EyePose = ([bool; 1], [f64; 4], [f64; 4]);

#[cfg(not(target_arch = "wasm32"))]
/// MD4の既知ベクタ（空文字）。依存の取り違えを検出する。単体テストで使う。
#[allow(dead_code)]
const MD4_EMPTY: &str = "31d6cfe0d16ae931b73c59d7e0c089c0";

#[cfg(not(target_arch = "wasm32"))]
/// MD4ハッシュを求める。RFC 1320の純Rust実装であり、外部依存を増やさない。
fn md4(message: &[u8]) -> [u8; 16] {
    fn rol(x: u32, n: u32) -> u32 {
        x.rotate_left(n)
    }
    fn f(x: u32, y: u32, z: u32) -> u32 {
        (x & y) | (!x & z)
    }
    fn g(x: u32, y: u32, z: u32) -> u32 {
        (x & y) | (x & z) | (y & z)
    }
    fn h(x: u32, y: u32, z: u32) -> u32 {
        x ^ y ^ z
    }
    let bit_len = (message.len() as u64).wrapping_mul(8);
    let mut padded = message.to_vec();
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_le_bytes());
    let mut a: u32 = 0x67452301;
    let mut b: u32 = 0xefcdab89;
    let mut c: u32 = 0x98badcfe;
    let mut d: u32 = 0x10325476;
    for block in padded.chunks_exact(64) {
        let mut x = [0u32; 16];
        for (i, slot) in x.iter_mut().enumerate() {
            let o = i * 4;
            *slot = u32::from_le_bytes([block[o], block[o + 1], block[o + 2], block[o + 3]]);
        }
        let (aa, bb, cc, dd) = (a, b, c, d);
        let s1 = [3, 7, 11, 19];
        for i in 0..16 {
            let (k, s) = (i, s1[i % 4]);
            let v = match i % 4 {
                0 => a.wrapping_add(f(b, c, d)).wrapping_add(x[k]),
                1 => d.wrapping_add(f(a, b, c)).wrapping_add(x[k]),
                2 => c.wrapping_add(f(d, a, b)).wrapping_add(x[k]),
                _ => b.wrapping_add(f(c, d, a)).wrapping_add(x[k]),
            };
            let r = rol(v, s);
            match i % 4 {
                0 => a = r,
                1 => d = r,
                2 => c = r,
                _ => b = r,
            }
        }
        let s2 = [3, 5, 9, 13];
        let order2 = [0, 4, 8, 12, 1, 5, 9, 13, 2, 6, 10, 14, 3, 7, 11, 15];
        for (i, k) in order2.iter().enumerate() {
            let s = s2[i % 4];
            let v = match i % 4 {
                0 => a
                    .wrapping_add(g(b, c, d))
                    .wrapping_add(x[*k])
                    .wrapping_add(0x5a827999),
                1 => d
                    .wrapping_add(g(a, b, c))
                    .wrapping_add(x[*k])
                    .wrapping_add(0x5a827999),
                2 => c
                    .wrapping_add(g(d, a, b))
                    .wrapping_add(x[*k])
                    .wrapping_add(0x5a827999),
                _ => b
                    .wrapping_add(g(c, d, a))
                    .wrapping_add(x[*k])
                    .wrapping_add(0x5a827999),
            };
            let r = rol(v, s);
            match i % 4 {
                0 => a = r,
                1 => d = r,
                2 => c = r,
                _ => b = r,
            }
        }
        let s3 = [3, 9, 11, 15];
        let order3 = [0, 8, 4, 12, 2, 10, 6, 14, 1, 9, 5, 13, 3, 11, 7, 15];
        for (i, k) in order3.iter().enumerate() {
            let s = s3[i % 4];
            let v = match i % 4 {
                0 => a
                    .wrapping_add(h(b, c, d))
                    .wrapping_add(x[*k])
                    .wrapping_add(0x6ed9eba1),
                1 => d
                    .wrapping_add(h(a, b, c))
                    .wrapping_add(x[*k])
                    .wrapping_add(0x6ed9eba1),
                2 => c
                    .wrapping_add(h(d, a, b))
                    .wrapping_add(x[*k])
                    .wrapping_add(0x6ed9eba1),
                _ => b
                    .wrapping_add(h(c, d, a))
                    .wrapping_add(x[*k])
                    .wrapping_add(0x6ed9eba1),
            };
            let r = rol(v, s);
            match i % 4 {
                0 => a = r,
                1 => d = r,
                2 => c = r,
                _ => b = r,
            }
        }
        a = a.wrapping_add(aa);
        b = b.wrapping_add(bb);
        c = c.wrapping_add(cc);
        d = d.wrapping_add(dd);
    }
    let mut out = [0u8; 16];
    out[0..4].copy_from_slice(&a.to_le_bytes());
    out[4..8].copy_from_slice(&b.to_le_bytes());
    out[8..12].copy_from_slice(&c.to_le_bytes());
    out[12..16].copy_from_slice(&d.to_le_bytes());
    out
}

#[cfg(not(target_arch = "wasm32"))]
/// テスト用の16進表示。単体テストで使う。
#[allow(dead_code)]
fn md4_hex(message: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digest = md4(message);
    let mut out = String::with_capacity(32);
    for byte in digest {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

#[cfg(not(target_arch = "wasm32"))]
/// スクリプトfileIDをMD4で計算する。
fn script_file_id(namespace: &str, class_name: &str) -> i32 {
    let mut input = Vec::with_capacity(4 + namespace.len() + class_name.len());
    input.extend_from_slice(&[b's', 0, 0, 0]);
    input.extend_from_slice(namespace.as_bytes());
    input.extend_from_slice(class_name.as_bytes());
    let digest = md4(&input);
    i32::from_le_bytes([digest[0], digest[1], digest[2], digest[3]])
}

#[cfg(not(target_arch = "wasm32"))]
/// VRChatのDLLとみなすGUID集合。
const VRCHAT_DLL_GUIDS: &[&str] = &[
    "2a2c05204084d904aa4945ccff20d8e5",
    "58e2f01a24261a14cb82e6d3399e8b16",
    "67cc4cb7839cd3741b63733d5adf0442",
    "80f1b8067b0760e4bb45023bc2e9de66",
    "4ecd63eff847044b68db9453ce219299",
];

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KnownScript {
    PhysBone,
    PhysBoneCollider,
    RotationConstraint,
    AvatarDescriptor,
    HeadChop,
    ContactReceiver,
    PipelineManager,
    ParentConstraint,
    PositionConstraint,
    ScaleConstraint,
    AimConstraint,
    LookAtConstraint,
}

#[cfg(not(target_arch = "wasm32"))]
fn script_identity(kind: KnownScript) -> (&'static str, &'static str) {
    match kind {
        KnownScript::PhysBone => ("VRC.SDK3.Dynamics.PhysBone.Components", "VRCPhysBone"),
        KnownScript::PhysBoneCollider => (
            "VRC.SDK3.Dynamics.PhysBone.Components",
            "VRCPhysBoneCollider",
        ),
        KnownScript::RotationConstraint => (
            "VRC.SDK3.Dynamics.Constraint.Components",
            "VRCRotationConstraint",
        ),
        KnownScript::ParentConstraint => (
            "VRC.SDK3.Dynamics.Constraint.Components",
            "VRCParentConstraint",
        ),
        KnownScript::PositionConstraint => (
            "VRC.SDK3.Dynamics.Constraint.Components",
            "VRCPositionConstraint",
        ),
        KnownScript::ScaleConstraint => (
            "VRC.SDK3.Dynamics.Constraint.Components",
            "VRCScaleConstraint",
        ),
        KnownScript::AimConstraint => (
            "VRC.SDK3.Dynamics.Constraint.Components",
            "VRCAimConstraint",
        ),
        KnownScript::LookAtConstraint => (
            "VRC.SDK3.Dynamics.Constraint.Components",
            "VRCLookAtConstraint",
        ),
        KnownScript::AvatarDescriptor => ("VRC.SDK3.Avatars.Components", "VRCAvatarDescriptor"),
        KnownScript::HeadChop => ("VRC.SDK3.Avatars.Components", "VRCHeadChop"),
        KnownScript::ContactReceiver => {
            ("VRC.SDK3.Dynamics.Contact.Components", "VRCContactReceiver")
        }
        // 実測fileID（-1427037861）で裏付けた名前空間とクラス名である。
        KnownScript::PipelineManager => ("VRC.Core", "PipelineManager"),
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScriptJudgment {
    Known(KnownScript),
    UnknownDll,
    External,
}

#[cfg(not(target_arch = "wasm32"))]
fn judge_script(guid: &str, file_id: i32) -> ScriptJudgment {
    let guid_lower = guid.to_ascii_lowercase();
    if !VRCHAT_DLL_GUIDS.contains(&guid_lower.as_str()) {
        return ScriptJudgment::External;
    }
    for kind in [
        KnownScript::PhysBone,
        KnownScript::PhysBoneCollider,
        KnownScript::RotationConstraint,
        KnownScript::ParentConstraint,
        KnownScript::PositionConstraint,
        KnownScript::ScaleConstraint,
        KnownScript::AimConstraint,
        KnownScript::LookAtConstraint,
        KnownScript::AvatarDescriptor,
        KnownScript::HeadChop,
        KnownScript::ContactReceiver,
        KnownScript::PipelineManager,
    ] {
        let (ns, class) = script_identity(kind);
        if script_file_id(ns, class) == file_id {
            return ScriptJudgment::Known(kind);
        }
    }
    ScriptJudgment::UnknownDll
}

/// 導出時の入力。
pub struct PrefabDeriveInput<'a> {
    /// 入力FBXのバイト列。SHA-256照合に使う。
    pub fbx_bytes: &'a [u8],
    /// 読み取り済みFBX。階層とbind行列に使う。
    pub fbx: &'a FbxFile<'a>,
    /// `Assets` を含むプロジェクトのルート。`.meta` の読み取りに使う。
    pub assets_root: &'a std::path::Path,
    /// 対象Prefabの文書列。
    pub prefab: &'a SerializedFile,
    /// 診断用のPrefab表示名。
    pub prefab_label: &'a str,
    /// 入力FBXの資産相対パス（`Assets/...` 形式）。
    pub fbx_asset_path: &'a str,
}

/// 導出結果。
pub struct DerivedSidecar {
    /// 生成したsidecar JSON（BOMなしUTF-8）。
    pub json: Vec<u8>,
    /// 無視した型名（Contactなど）。レポートに出す。
    pub ignored_types: Vec<String>,
    /// FBXと対応が取れなかったノード名。Transformのみで出力した記録である。
    /// 物理に参照されるものがあれば導出前に失敗するため、ここにあるのは
    /// 物理に無関係なものだけである。レポートに出す。
    pub unmatched_nodes: Vec<String>,
}

pub fn derive_physics_sidecar(input: &PrefabDeriveInput<'_>) -> Result<DerivedSidecar, VrmError> {
    derive_inner(input)
}

// 以下、導出の本体。読みやすさのため小さな関数に分ける。

fn derive_inner(input: &PrefabDeriveInput<'_>) -> Result<DerivedSidecar, VrmError> {
    // wasmではファイル走査ができないため明示的に失敗する。
    #[cfg(target_arch = "wasm32")]
    {
        let _ = input;
        return Err(VrmError::new(
            VrmErrorKind::UnsupportedPhysicsFeature,
            "Prefabからの導出はwasmでは未対応である",
        ));
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        derive_native(input)
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn derive_native(input: &PrefabDeriveInput<'_>) -> Result<DerivedSidecar, VrmError> {
    use crate::unity::{IndexOptions, scan_directory};

    let label = input.prefab_label;
    // Assets配置の正規化。CLIの材質経路と同じ手順（`VrmMaterials::load` 対応）を使う。
    let canonical_root = std::fs::canonicalize(input.assets_root).map_err(|e| {
        VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!(
                "材質の走査元を解決できない: {}: {e}",
                crate::escape_diag(&format!("{:?}", input.assets_root)),
            ),
        )
    })?;
    let canonical_assets = std::fs::canonicalize(canonical_root.join("Assets")).map_err(|e| {
        VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!(
                "材質のAssetsを解決できない: {:?}: {e}",
                canonical_root.join("Assets"),
            ),
        )
    })?;
    let index = scan_directory(&canonical_assets, &IndexOptions::default()).map_err(|e| {
        VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!("材質の索引化に失敗した: {e}"),
        )
    })?;
    // 入力FBXのGUIDと置換表を索引から引く。
    // 索引のパスはAssets相対（`PLUSONE/...`）であり、`Assets/` 接頭辞は付けない。
    // 呼び出し側が `Assets/...` 形で渡した場合は接頭辞を落として引く。
    let indexed_path = input
        .fbx_asset_path
        .strip_prefix("Assets/")
        .or_else(|| input.fbx_asset_path.strip_prefix("Assets\\"))
        .unwrap_or(input.fbx_asset_path);
    let Some(record) = index.get_by_path(indexed_path) else {
        return Err(VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!(
                "Prefab割当のFBXが索引にない: {}",
                crate::escape_diag(input.fbx_asset_path),
            ),
        ));
    };
    let fbx_guid = record.meta.guid().as_str().to_owned();
    check_file_ids_generation(&record.meta, label)?;
    // Prefab文書の型付き読み取り。
    let parsed = parse_prefab_docs(input.prefab, label)?;
    // FBX階層の構築（ソート付き）。
    let milfy_hier = build_fbx_hierarchy(input.fbx, label)?;
    // 入力FBXのGUIDに対応するPrefabInstanceを探す。
    // 1件あればインスタンス経路、0件（文書が直接記述）なら直接経路、
    // 複数は先勝ちにせず失敗する。
    let instances = find_instances(&parsed, &fbx_guid);
    if instances.len() == 1 {
        derive_instance_native(
            input,
            label,
            &index,
            &canonical_assets,
            &parsed,
            &milfy_hier,
            &fbx_guid,
        )
    } else if parsed.instances.is_empty() {
        derive_direct_native(
            input,
            label,
            &index,
            &canonical_assets,
            &parsed,
            &milfy_hier,
            &fbx_guid,
        )
    } else {
        Err(VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!(
                "入力FBXへのPrefabInstanceが{}件あるため先勝ちにしない（{}）",
                instances.len(),
                crate::escape_diag(label),
            ),
        ))
    }
}

/// FBXの `.meta` を読み取り、取り込み方式を確かめる。
/// 索引と同じくAssets相対で引く。`Assets/` 接頭辞は落とす。
#[cfg(not(target_arch = "wasm32"))]
fn read_fbx_meta(
    input: &PrefabDeriveInput<'_>,
    canonical_assets: &std::path::Path,
    label: &str,
) -> Result<AssetMeta, VrmError> {
    let indexed_path = input
        .fbx_asset_path
        .strip_prefix("Assets/")
        .or_else(|| input.fbx_asset_path.strip_prefix("Assets\\"))
        .unwrap_or(input.fbx_asset_path);
    let meta_path = canonical_assets.join(indexed_path);
    let meta_bytes = std::fs::read(format!("{}.meta", meta_path.display())).map_err(|e| {
        VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!("FBXの.metaを読めない: {e}"),
        )
    })?;
    let meta = AssetMeta::parse(&meta_bytes).map_err(|e| {
        VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!("FBXの.metaを読めない: {e}"),
        )
    })?;
    check_file_ids_generation(&meta, label)?;
    Ok(meta)
}

/// sidecarの参照元FBX1件。`sourceModels` の組み立てに使う。
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone)]
struct SourceModelEntry {
    id: String,
    sha256: String,
    asset_guid: String,
    asset_path: String,
}

#[cfg(not(target_arch = "wasm32"))]
impl SourceModelEntry {
    /// 入力FBXの項目を作る。SHA-256は入力バイト列から求める。
    fn from_input(input: &PrefabDeriveInput<'_>, model_id: &str, fbx_guid: &str) -> Self {
        use sha2::Digest;
        let digest = sha2::Sha256::digest(input.fbx_bytes);
        let mut sha = String::with_capacity(64);
        for byte in digest {
            sha.push(char::from_digit(u32::from(byte >> 4), 16).expect("hexである"));
            sha.push(char::from_digit(u32::from(byte & 0x0f), 16).expect("hexである"));
        }
        Self {
            id: model_id.to_owned(),
            sha256: sha,
            asset_guid: fbx_guid.to_owned(),
            asset_path: input.fbx_asset_path.to_owned(),
        }
    }
}

/// インスタンス経路の導出。PrefabInstanceが1件ある場合に使う。
#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::too_many_arguments)]
fn derive_instance_native(
    input: &PrefabDeriveInput<'_>,
    label: &str,
    index: &crate::unity::AssetIndex,
    canonical_assets: &std::path::Path,
    parsed: &ParsedPrefab,
    milfy_hier: &FbxHierarchy,
    fbx_guid: &str,
) -> Result<DerivedSidecar, VrmError> {
    // ネストした副階層の解決。主FBX以外のインスタンス（副Prefab）の
    // 実体をAssetsから読み、副FBXと合わせて graft の材料を作る。
    let phone = resolve_phone_subtree(index, canonical_assets, parsed, fbx_guid, label)?;
    // 1930年代方式ではなく、2019.3以降のXXH64方式であることを確かめる。
    // fileIDの一致で解決できない対象があれば旧方式として失敗する。
    // 主インスタンスの改変内容のうち、姿勢の上書きは対象ノードの
    // ローカル変換へ適用し、構造変更は明示的に失敗させる。
    let overlays = collect_main_instance_overlays(parsed, milfy_hier, &phone, fbx_guid, label)?;
    // ノード階層の組み立て。
    let model_id_main = model_id_from_asset_path(input.fbx_asset_path);
    let assembly = TreeAssembly {
        fbx: input.fbx,
        hier: milfy_hier,
        parsed,
        phone: &phone,
        main_fbx_guid: fbx_guid,
        model_id_main: &model_id_main,
        overlays: &overlays,
        label,
    };
    let tree = assemble_tree(&assembly)?;
    // bind選択（Body優先）。
    let bind = select_bind(input.fbx, milfy_hier, label)?;
    // 参照ローカルの計算（f32でUnityと同順）。姿勢の上書きを反映する。
    let locals = compute_reference_locals(
        input.fbx, milfy_hier, &tree, &bind, &phone, &overlays, label,
    )?;
    // 成分の判別と収集。
    let collector = collect_components(parsed, &tree, &phone, fbx_guid, label)?;
    // humanoid・lookAtのための.meta読み取り。
    let meta = read_fbx_meta(input, canonical_assets, label)?;
    // sourceModelsは主FBXと副FBX（ある場合）を資産パス順に並べる。
    // Unityの `SortedDictionary` と同じ決定性規則である。
    let mut sources = vec![SourceModelEntry::from_input(
        input,
        &model_id_main,
        fbx_guid,
    )];
    if let Some(phone) = phone.as_ref() {
        sources.push(SourceModelEntry {
            id: model_id_from_asset_path(&phone.fbx_asset_path),
            sha256: phone.fbx_sha256.clone(),
            asset_guid: phone.fbx_guid.clone(),
            asset_path: phone.fbx_asset_path.clone(),
        });
    }
    // sidecar JSONの組み立て。
    emit_sidecar(
        input,
        &tree,
        &locals,
        &collector,
        &meta,
        &sources,
        &model_id_main,
        &bind,
        label,
        &[],
    )
}

/// 直接記述のPrefabから導出する。PrefabInstanceが無い場合に使う。
/// ノードはPrefabのTransform文書から組み、FBXとの対応は
/// `m_Mesh` のfileID（`Type:Mesh->{モデル名}0`）で取る。
/// 対応が取れないノードはTransformのみで出力するが、物理に参照される
/// ものがあれば明示的に失敗させる。黙って落とさず件数を報告する。
#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::too_many_arguments)]
fn derive_direct_native(
    input: &PrefabDeriveInput<'_>,
    label: &str,
    index: &crate::unity::AssetIndex,
    canonical_assets: &std::path::Path,
    parsed: &ParsedPrefab,
    milfy_hier: &FbxHierarchy,
    fbx_guid: &str,
) -> Result<DerivedSidecar, VrmError> {
    // 参照メッシュのGUIDごとにFBXを読み取り、所有側へ写す。
    // 入力FBXは読み取り済みのものを使い、他は索引から探して読む。
    let mut extra_fbx: Vec<ExtraFbx> = Vec::new();
    {
        let mut seen: HashSet<String> = HashSet::new();
        seen.insert(fbx_guid.to_ascii_lowercase());
        for smr in &parsed.smrs {
            let Some(guid) = smr.mesh_guid.as_deref() else {
                continue;
            };
            if !seen.insert(guid.to_owned()) {
                continue;
            }
            extra_fbx.push(load_extra_fbx(index, canonical_assets, guid, label)?);
        }
    }
    // ノード階層の組み立て。FBXとの対応付けもここで行う。
    let model_id_main = model_id_from_asset_path(input.fbx_asset_path);
    let build = build_direct_tree(
        parsed,
        input.fbx,
        milfy_hier,
        &extra_fbx,
        &model_id_main,
        fbx_guid,
        label,
    )?;
    // bind選択（Body優先）。入力FBXで選ぶ。
    let bind = select_bind(input.fbx, milfy_hier, label)?;
    // 空の上書き表。直接記述にはPrefabInstanceの改変が無い。
    let overlays = MainInstanceOverlays {
        trs: HashMap::new(),
        active: HashMap::new(),
        names: HashMap::new(),
        root_name: None,
    };
    let no_phone: Option<PhoneSubtree> = None;
    // 参照ローカルの計算（f32でUnityと同順）。
    let locals = compute_reference_locals(
        input.fbx,
        milfy_hier,
        &build.tree,
        &bind,
        &no_phone,
        &overlays,
        label,
    )?;
    // 成分の判別と収集。
    let collector = collect_components(parsed, &build.tree, &no_phone, fbx_guid, label)?;
    // 物理に参照される未対応ノードがあれば失敗させる。
    // 平面colliderだけの参照は平面経路が直接扱うため除く。
    check_direct_physics_refs(&build.tree, &collector, label)?;
    // humanoid・lookAtのための.meta読み取り。
    let meta = read_fbx_meta(input, canonical_assets, label)?;
    // sourceModelsはメッシュのGUIDから埋める。入力FBXと参照FBXを資産パス順に並べる。
    let mut sources = vec![SourceModelEntry::from_input(
        input,
        &model_id_main,
        fbx_guid,
    )];
    for extra in &extra_fbx {
        sources.push(SourceModelEntry {
            id: extra.model_id.clone(),
            sha256: extra.sha256.clone(),
            asset_guid: extra.guid.clone(),
            asset_path: extra.asset_path.clone(),
        });
    }
    // sidecar JSONの組み立て。
    emit_sidecar(
        input,
        &build.tree,
        &locals,
        &collector,
        &meta,
        &sources,
        &model_id_main,
        &bind,
        label,
        &build.unmatched,
    )
}

/// 直接記述のPrefabが参照する入力以外のFBX1件の所有側の写し。
/// 階層の組み立てと出所の特定に使う。bind計算には使わない
/// （参照姿勢が `scene` のため）。
#[cfg(not(target_arch = "wasm32"))]
struct ExtraFbx {
    guid: String,
    asset_path: String,
    model_id: String,
    sha256: String,
    models: Vec<ExtraFbxModel>,
    children_sorted: Vec<Vec<usize>>,
    roots_sorted: Vec<usize>,
    /// メッシュfileIDからモデル索引への表。
    mesh_at: HashMap<FileId, usize>,
}

/// 参照FBXのモデル1件の所有側の写し。
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone)]
struct ExtraFbxModel {
    name: String,
    parent: Option<usize>,
    /// ジオメトリを持つか（Unityのメッシュサブアセットになるか）。
    has_mesh: bool,
}

/// 参照FBXを索引から探して読み取り、所有側へ写す。
#[cfg(not(target_arch = "wasm32"))]
fn load_extra_fbx(
    index: &crate::unity::AssetIndex,
    canonical_assets: &std::path::Path,
    guid: &str,
    label: &str,
) -> Result<ExtraFbx, VrmError> {
    let Ok(guid_key) = crate::unity::Guid::parse(guid) else {
        return Err(VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!(
                "メッシュ参照のGUIDが不正である: {guid}（{}）",
                crate::escape_diag(label)
            ),
        ));
    };
    let Some(record) = index.get(&guid_key) else {
        return Err(VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!(
                "メッシュ参照の資産が索引にない: {guid}（{}）",
                crate::escape_diag(label)
            ),
        ));
    };
    if !record.path.ends_with(".fbx") && !record.path.ends_with(".FBX") {
        return Err(VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!(
                "メッシュ参照がFBXでないため未対応である: {}（{}）",
                crate::escape_diag(&record.path),
                crate::escape_diag(label),
            ),
        ));
    }
    let full = canonical_assets.join(&record.path);
    let bytes = std::fs::read(&full).map_err(|e| {
        VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!("参照FBXを読めない: {e}"),
        )
    })?;
    let asset_path = format!("Assets/{}", record.path);
    let sha256 = {
        use sha2::Digest;
        let digest = sha2::Sha256::digest(&bytes);
        let mut sha = String::with_capacity(64);
        for byte in digest {
            sha.push(char::from_digit(u32::from(byte >> 4), 16).expect("hexである"));
            sha.push(char::from_digit(u32::from(byte & 0x0f), 16).expect("hexである"));
        }
        sha
    };
    let file = FbxFile::parse(&bytes, &FbxOptions::default()).map_err(|e| {
        VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!("参照FBXを読めない（{}）: {e}", crate::escape_diag(label)),
        )
    })?;
    let file_models = file.models();
    let at: HashMap<i64, usize> = file_models
        .iter()
        .enumerate()
        .map(|(i, m)| (m.id.value(), i))
        .collect();
    let mut models = Vec::with_capacity(file_models.len());
    for model in file_models {
        let has_mesh = model.mesh.is_some();
        if model.name.contains('/') {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!(
                    "参照FBXのモデル名に `/` を含むため階層パスが作れない: {}（{}）",
                    crate::escape_diag(&model.name),
                    crate::escape_diag(label),
                ),
            ));
        }
        let parent = match model.parent {
            None => None,
            Some(p) => Some(at.get(&p.value()).copied().ok_or_else(|| {
                VrmError::new(VrmErrorKind::InvalidPrefab, "参照FBXの親モデルが一覧にない")
            })?),
        };
        models.push(ExtraFbxModel {
            name: model.name.clone(),
            parent,
            has_mesh,
        });
    }
    let mut children_sorted: Vec<Vec<usize>> = vec![Vec::new(); models.len()];
    let mut roots: Vec<usize> = Vec::new();
    for (i, model) in models.iter().enumerate() {
        match model.parent {
            Some(p) => children_sorted[p].push(i),
            None => roots.push(i),
        }
    }
    for children in children_sorted.iter_mut() {
        children.sort_by(|a, b| {
            models[*a]
                .name
                .to_ascii_lowercase()
                .cmp(&models[*b].name.to_ascii_lowercase())
                .then(models[*a].name.cmp(&models[*b].name))
        });
    }
    roots.sort_by(|a, b| {
        models[*a]
            .name
            .to_ascii_lowercase()
            .cmp(&models[*b].name.to_ascii_lowercase())
            .then(models[*a].name.cmp(&models[*b].name))
    });
    let mut mesh_at = HashMap::new();
    for (i, model) in models.iter().enumerate() {
        // Unityのメッシュサブアセットは、ジオメトリを持つノードにだけ作られる。
        // 骨（ジオメトリ無し）が同名でも衝突しない。
        if !model.has_mesh {
            continue;
        }
        let fid = crate::unity::mesh_file_id(&model.name);
        // 同名メッシュの重複は先勝ちにせず失敗する。
        if mesh_at.contains_key(&fid) {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!(
                    "参照FBXのメッシュIDが重複するため先勝ちにしない: {}（{}）",
                    crate::escape_diag(&model.name),
                    crate::escape_diag(label),
                ),
            ));
        }
        mesh_at.insert(fid, i);
    }
    Ok(ExtraFbx {
        guid: guid.to_owned(),
        asset_path: asset_path.clone(),
        model_id: model_id_from_asset_path(&asset_path),
        sha256,
        models,
        children_sorted,
        roots_sorted: roots,
        mesh_at,
    })
}

/// 直接記述ツリーの組み立て結果。未対応ノード名の報告を含む。
#[cfg(not(target_arch = "wasm32"))]
struct DirectBuild {
    tree: Vec<TreeNode>,
    /// FBXと対応が取れなかったノード名。Transformのみで出力した記録である。
    unmatched: Vec<String>,
}

/// 直接記述のツリーを組み立てる。PrefabInstanceが無いPrefab用である。
///
/// アバタールート（AvatarDescriptorの所属先）を除き、PrefabのTransform文書を
/// `m_Children` 順にDFSで出す。FBXとの対応は `m_Mesh` のfileIDでSMR所有者を
/// 先に確定し、残りを名前経路で結ぶ。名前経路は入力FBXを優先し、参照FBXも使う。
/// 対応が取れないノードは `source: null` のTransformのみで出し、名前を報告する。
/// アバタールート名はルートGameObjectの名前を使う。
#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::too_many_arguments)]
fn build_direct_tree(
    parsed: &ParsedPrefab,
    fbx: &FbxFile<'_>,
    hier: &FbxHierarchy,
    extra_fbx: &[ExtraFbx],
    model_id_main: &str,
    main_fbx_guid: &str,
    label: &str,
) -> Result<DirectBuild, VrmError> {
    // AvatarDescriptorは1件に定める。所属先がアバタールートである。
    // 型判定はスクリプトfileIDのMD4計算で行う。
    let avatar_fid = script_file_id("VRC.SDK3.Avatars.Components", "VRCAvatarDescriptor");
    let mut descriptor_go: Option<FileId> = None;
    for mono in &parsed.monos {
        if mono.script_fid == avatar_fid
            && VRCHAT_DLL_GUIDS.contains(&mono.script_guid.to_ascii_lowercase().as_str())
        {
            if descriptor_go.is_some() {
                return Err(VrmError::new(
                    VrmErrorKind::InvalidPrefab,
                    format!(
                        "VRCAvatarDescriptorが複数ある（{}）",
                        crate::escape_diag(label)
                    ),
                ));
            }
            descriptor_go = Some(mono.go);
        }
    }
    let Some(root_go) = descriptor_go else {
        return Err(VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!(
                "VRCAvatarDescriptorが無いため直接記述を組み立てられない（{}）",
                crate::escape_diag(label)
            ),
        ));
    };
    let root_go_doc = parsed.local_go.get(&root_go).ok_or_else(|| {
        VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!(
                "アバタールートのゲームオブジェクトがない（{}）",
                crate::escape_diag(label)
            ),
        )
    })?;
    // アバタールートのTransformを求める。親を持たないことを確かめる。
    let root_tr = parsed
        .local_tr
        .iter()
        .find(|(_, tr)| tr.go == root_go)
        .map(|(fid, _)| *fid)
        .ok_or_else(|| {
            VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!(
                    "アバタールートのTransformがない（{}）",
                    crate::escape_diag(label)
                ),
            )
        })?;
    let root_local = parsed.local_tr.get(&root_tr).expect("存在は確かめた");
    if root_local.father != 0 {
        return Err(VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!(
                "アバタールートが入れ子のため未対応である（{}）",
                crate::escape_diag(label)
            ),
        ));
    }
    let avatar_name = root_go_doc.name.clone();
    // SMR所有者の対応付け。`m_Mesh` のfileIDでFBXモデルを引く。
    // 入力FBXを優先し、参照FBXも使う。重複は先勝ちにせず失敗する。
    let mut input_mesh_at: HashMap<FileId, usize> = HashMap::new();
    for (i, model) in fbx.models().iter().enumerate() {
        // ジオメトリを持つノードだけがUnityのメッシュサブアセットになる。
        if model.mesh.is_none() {
            continue;
        }
        let fid = crate::unity::mesh_file_id(&model.name);
        if input_mesh_at.contains_key(&fid) {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!(
                    "入力FBXのメッシュIDが重複するため先勝ちにしない: {}（{}）",
                    crate::escape_diag(&model.name),
                    crate::escape_diag(label),
                ),
            ));
        }
        input_mesh_at.insert(fid, i);
    }
    // SMR所有者のPrefabノード集合。GO fileIDからSMR文書を引く。
    let mut smr_by_go: HashMap<FileId, &ParsedSmr> = HashMap::new();
    for smr in &parsed.smrs {
        // 同一GOの複数SMRは先勝ちにせず失敗する。
        if smr_by_go.contains_key(&smr.go) {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!(
                    "同一オブジェクトに複数の描画設定がある（{}）",
                    crate::escape_diag(label)
                ),
            ));
        }
        smr_by_go.insert(smr.go, smr);
    }
    // 名前経路の対応付けに使うFBX一覧。入力を先頭にする。
    struct FbxView {
        models: Vec<DirectModel>,
        children_sorted: Vec<Vec<usize>>,
        roots_sorted: Vec<usize>,
        model_id: String,
        asset_guid: String,
        is_input: bool,
    }
    #[derive(Debug, Clone)]
    struct DirectModel {
        name: String,
        parent: Option<usize>,
    }
    let mut views: Vec<FbxView> = Vec::new();
    {
        let models = fbx
            .models()
            .iter()
            .map(|m| {
                let parent = match m.parent {
                    None => None,
                    Some(p) => fbx.models().iter().position(|x| x.id == p),
                };
                DirectModel {
                    name: m.name.clone(),
                    parent,
                }
            })
            .collect::<Vec<_>>();
        // hier.children_sortedは名前順であり、そのまま使う。
        let mut roots: Vec<usize> = Vec::new();
        for (i, m) in models.iter().enumerate() {
            if m.parent.is_none() {
                roots.push(i);
            }
        }
        roots.sort_by(|a, b| {
            models[*a]
                .name
                .to_ascii_lowercase()
                .cmp(&models[*b].name.to_ascii_lowercase())
                .then(models[*a].name.cmp(&models[*b].name))
        });
        views.push(FbxView {
            models,
            children_sorted: hier.children_sorted.clone(),
            roots_sorted: roots,
            model_id: model_id_main.to_owned(),
            asset_guid: main_fbx_guid.to_owned(),
            is_input: true,
        });
    }
    for extra in extra_fbx {
        views.push(FbxView {
            models: extra
                .models
                .iter()
                .map(|m| DirectModel {
                    name: m.name.clone(),
                    parent: m.parent,
                })
                .collect(),
            children_sorted: extra.children_sorted.clone(),
            roots_sorted: extra.roots_sorted.clone(),
            model_id: extra.model_id.clone(),
            asset_guid: extra.guid.clone(),
            is_input: false,
        });
    }
    // GO fileIDからSMR所有者のFBX対応を引く表を作る。
    // (view索引, モデル索引) の組である。
    let mut owner_match: HashMap<FileId, (usize, usize)> = HashMap::new();
    for (go_fid, smr) in smr_by_go.iter() {
        if smr.mesh_fid == 0 {
            continue;
        }
        let guid = smr.mesh_guid.as_deref().unwrap_or("");
        // 入力FBXのGUIDと一致すれば入力FBXで引く。
        if guid.eq_ignore_ascii_case(main_fbx_guid) {
            if let Some(&model_index) = input_mesh_at.get(&smr.mesh_fid) {
                owner_match.insert(*go_fid, (0, model_index));
                continue;
            }
            // 入力FBXに対応が無ければ未対応として残す。
            continue;
        }
        // 参照FBXで引く。
        for (view_index, extra) in extra_fbx.iter().enumerate() {
            if !extra.guid.eq_ignore_ascii_case(guid) {
                continue;
            }
            if let Some(&model_index) = extra.mesh_at.get(&smr.mesh_fid) {
                // views[0] は入力FBXであり、extra は 1 件ずれる。
                owner_match.insert(*go_fid, (view_index + 1, model_index));
            }
            break;
        }
    }
    // DFSでツリーを組み立てる。対応はSMR所有者・名前経路の順で決める。
    let mut nodes: Vec<TreeNode> = Vec::new();
    let mut unmatched: Vec<String> = Vec::new();
    // (tree索引, FBX view, FBXモデル) の対応表。source無しは含まれない。
    let mut node_fbx: HashMap<usize, (usize, usize)> = HashMap::new();
    // 反復DFS。子を逆順で積む。親は子より先に確定する。
    struct DirectFrame {
        tr: FileId,
        parent: Option<usize>,
        scene_path: Vec<String>,
    }
    let mut stack: Vec<DirectFrame> = Vec::new();
    for child in root_local.children.iter().rev() {
        let child_local = parsed.local_tr.get(child).ok_or_else(|| {
            VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!("ルートの子Transformがない（{}）", crate::escape_diag(label)),
            )
        })?;
        let child_go = parsed.local_go.get(&child_local.go).ok_or_else(|| {
            VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!("ルートの子GOがない（{}）", crate::escape_diag(label)),
            )
        })?;
        stack.push(DirectFrame {
            tr: *child,
            parent: None,
            scene_path: vec![avatar_name.clone(), child_go.name.clone()],
        });
    }
    while let Some(frame) = stack.pop() {
        let node_index = nodes.len();
        let local = parsed.local_tr.get(&frame.tr).ok_or_else(|| {
            VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!("追加物のTransformがない（{}）", crate::escape_diag(label)),
            )
        })?;
        let go = parsed.local_go.get(&local.go).ok_or_else(|| {
            VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!(
                    "追加物のゲームオブジェクトがない（{}）",
                    crate::escape_diag(label)
                ),
            )
        })?;
        // FBX対応の決定。SMR所有者を先に見る。
        let mut matched: Option<(usize, usize)> = owner_match.get(&local.go).copied();
        // 親が対応済みなら、そのFBXの子を名前で探す。
        if matched.is_none()
            && let Some(parent) = frame.parent
            && let Some((view_index, model_index)) = node_fbx.get(&parent).copied()
        {
            let view = &views[view_index];
            for child_model in view.children_sorted[model_index].iter() {
                if view.models[*child_model].name == go.name {
                    matched = Some((view_index, *child_model));
                    break;
                }
            }
        }
        // 親が未対応（アバタールート直下を含む）なら、各FBXのルートを名前で探す。
        // 入力FBXを優先する。
        if matched.is_none() {
            for (view_index, view) in views.iter().enumerate() {
                let mut found = None;
                for root_model in view.roots_sorted.iter() {
                    if view.models[*root_model].name == go.name {
                        found = Some((view_index, *root_model));
                        break;
                    }
                }
                if found.is_some() {
                    matched = found;
                    break;
                }
            }
        }
        let (source, fbx_index) = match matched {
            Some((view_index, model_index)) => {
                let view = &views[view_index];
                // FBX階層パスを作る。
                let mut reversed = vec![view.models[model_index].name.clone()];
                let mut current = model_index;
                while let Some(parent) = view.models[current].parent {
                    reversed.push(view.models[parent].name.clone());
                    current = parent;
                }
                reversed.reverse();
                let path = reversed;
                // Transform fileIDは階層パスから求める。
                let local_id = transform_file_id(&path.join("/"));
                node_fbx.insert(node_index, (view_index, model_index));
                (
                    Some(TreeSource {
                        model_id: view.model_id.clone(),
                        path,
                        asset_guid: view.asset_guid.clone(),
                        local_id,
                    }),
                    view.is_input.then_some(model_index),
                )
            }
            None => {
                unmatched.push(go.name.clone());
                (None, None)
            }
        };
        nodes.push(TreeNode {
            name: go.name.clone(),
            parent: frame.parent,
            children: Vec::new(),
            scene_path: frame.scene_path.clone(),
            source,
            added_trs: Some((local.position, local.rotation, local.scale)),
            fbx_index,
            phone_model: None,
            active_self: go.active,
            local_go: Some(local.go),
            local_tr: Some(frame.tr),
        });
        if let Some(parent) = frame.parent {
            nodes[parent].children.push(node_index);
        }
        // 子は `m_Children` 順で積む。
        for child in local.children.iter().rev() {
            let child_local = parsed.local_tr.get(child).ok_or_else(|| {
                VrmError::new(
                    VrmErrorKind::InvalidPrefab,
                    format!("追加物の子Transformがない（{}）", crate::escape_diag(label)),
                )
            })?;
            let child_go = parsed.local_go.get(&child_local.go).ok_or_else(|| {
                VrmError::new(
                    VrmErrorKind::InvalidPrefab,
                    format!("追加物の子GOがない（{}）", crate::escape_diag(label)),
                )
            })?;
            let mut child_path = frame.scene_path.clone();
            child_path.push(child_go.name.clone());
            stack.push(DirectFrame {
                tr: *child,
                parent: Some(node_index),
                scene_path: child_path,
            });
        }
    }
    // 未配置のローカル文書があれば、親の解決漏れとして失敗させる。
    {
        let mut placed: HashSet<FileId> = HashSet::new();
        for node in &nodes {
            if let Some(tr) = node.local_tr {
                placed.insert(tr);
            }
        }
        // アバタールート自体は配置しない。
        placed.insert(root_tr);
        for tr_fid in parsed.local_tr.keys() {
            if !placed.contains(tr_fid) {
                return Err(VrmError::new(
                    VrmErrorKind::InvalidPrefab,
                    format!(
                        "配置できない追加物がある（文書{tr_fid}）（{}）",
                        crate::escape_diag(label),
                    ),
                ));
            }
        }
    }
    // COMコンポーネント等の無関係なローカルGOは、参照解決時に無視する。
    // ここでは配置の有無だけを確かめる。
    unmatched.sort();
    unmatched.dedup();
    Ok(DirectBuild {
        tree: nodes,
        unmatched,
    })
}

/// 直接記述ツリーの物理参照を検査する。対応が取れていないノードを
/// 物理（PhysBone・非平面collider・制約）が参照していれば失敗させる。
/// 平面colliderだけの参照は平面経路が直接扱うため除く。
/// 対応が取れていないノード自体はTransformのみで出力し、件数を報告する。
#[cfg(not(target_arch = "wasm32"))]
fn check_direct_physics_refs(
    tree: &[TreeNode],
    collector: &Collected,
    label: &str,
) -> Result<(), VrmError> {
    let mut needed: HashSet<usize> = HashSet::new();
    for bone in &collector.phys_bones {
        needed.insert(bone.component_node);
        needed.insert(bone.root_node);
        needed.extend(bone.ignore_nodes.iter().copied());
    }
    for collider in &collector.colliders {
        // 平面は基準ノードを直接使うため、対応が無くてもよい。
        if collider.shape == "Plane" {
            continue;
        }
        needed.insert(collider.component_node);
        needed.insert(collider.root_node);
    }
    for constraint in &collector.constraints {
        needed.insert(constraint.component_node);
        needed.insert(constraint.target_node);
        for (node, _) in &constraint.sources {
            if *node != usize::MAX {
                needed.insert(*node);
            }
        }
    }
    // FBX対応の無いノード（Prefabで追加した当たり判定用オブジェクト等）は、
    // そのまま `source: null` で出し、Rust側が補助ノードとして作る。
    // ここで失敗させると、追加オブジェクトにPhysBoneやColliderを付けた
    // 一般的なアバターが変換できなくなる。ノードが木に無い場合だけ失敗する。
    for node in needed {
        if tree.get(node).is_none() {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!(
                    "物理が参照するノードが木に無い: {node}（{}）",
                    crate::escape_diag(label),
                ),
            ));
        }
    }
    let _ = label;
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
fn check_file_ids_generation(meta: &AssetMeta, label: &str) -> Result<(), VrmError> {
    let settings = meta.importer().settings.as_mapping().ok_or_else(|| {
        VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!(
                "importerの設定がマッピングでない（{}）",
                crate::escape_diag(label),
            ),
        )
    })?;
    if let Some(value) = settings.get("fileIdsGeneration") {
        let generation = value.as_i64().unwrap_or(-1);
        // 2が2019.3以降の新方式である。それ以外は旧方式として失敗する。
        if generation != 2 {
            return Err(VrmError::new(
                VrmErrorKind::UnsupportedPhysicsFeature,
                format!(
                    "2019.3より前のFBX取り込み方式は未対応である（fileIdsGeneration={generation}）（{}）",
                    crate::escape_diag(label),
                ),
            ));
        }
    }
    Ok(())
}

/// 主インスタンスの改変内容のうち、参照姿勢に効く上書きの集計。
#[cfg(not(target_arch = "wasm32"))]
struct MainInstanceOverlays {
    /// FBXのTransform fileIDごとの姿勢上書き。
    /// 位置・回転・拡大率の成分だけを持ち、無い成分は `None` である。
    trs: HashMap<FileId, TrsOverride>,
    /// ゲームオブジェクトfileIDごとの有効状態の上書き。
    active: HashMap<FileId, bool>,
    /// ゲームオブジェクトfileIDごとの改名。
    names: HashMap<FileId, String>,
    /// 取り込み根の改名。アバタールート名に使う。無い場合は `None` である。
    root_name: Option<String>,
}

/// 姿勢1件の成分別上書き。`None` の成分はFBXの値をそのまま使う。
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone, Default)]
struct TrsOverride {
    position: [Option<f64>; 3],
    rotation: [Option<f64>; 4],
    scale: [Option<f64>; 3],
}

/// 主インスタンスの改変内容を集める。姿勢の上書きは
/// 対象ノードのローカル変換へ適用し、構造変更は明示的に失敗させる。
#[cfg(not(target_arch = "wasm32"))]
fn collect_main_instance_overlays(
    parsed: &ParsedPrefab,
    hier: &FbxHierarchy,
    phone: &Option<PhoneSubtree>,
    main_fbx_guid: &str,
    label: &str,
) -> Result<MainInstanceOverlays, VrmError> {
    let import_tr = transform_file_id("");
    let import_go = crate::unity::game_object_file_id("");
    // 取り込みfileIDからFBXモデル索引への逆引き表を作る。
    let mut tr_to_fbx: HashMap<FileId, usize> = HashMap::new();
    for (index, fid) in hier.tr_fid_at.iter().enumerate() {
        tr_to_fbx.insert(*fid, index);
    }
    let mut go_to_fbx: HashMap<FileId, usize> = HashMap::new();
    for (index, path) in hier.paths.iter().enumerate() {
        go_to_fbx.insert(crate::unity::game_object_file_id(path), index);
    }
    go_to_fbx.insert(import_go, usize::MAX);
    // 描画設定の取り込みfileID表。`m_IsActive` の対象が
    // 成分かゲームオブジェクトかの区別に使う。
    let renderer_fids: HashSet<FileId> = hier
        .paths
        .iter()
        .map(|path| crate::unity::skinned_mesh_renderer_file_id(path))
        .collect();
    let tables = MainModTables {
        parsed,
        tr_to_fbx: &tr_to_fbx,
        go_to_fbx: &go_to_fbx,
        renderer_fids: &renderer_fids,
        import_tr,
        import_go,
    };
    let mut overlays = MainInstanceOverlays {
        trs: HashMap::new(),
        active: HashMap::new(),
        names: HashMap::new(),
        root_name: None,
    };
    for instance in &parsed.instances {
        if !instance.source_guid.eq_ignore_ascii_case(main_fbx_guid) {
            continue;
        }
        for m in &instance.modifications {
            let guid_match = m
                .target_guid
                .as_ref()
                .map(|g| g.eq_ignore_ascii_case(main_fbx_guid))
                .unwrap_or(false);
            if !guid_match {
                // 空参照の解除は効果が無いため無視する。
                if m.target_guid.is_none() && m.target_fid == 0 {
                    continue;
                }
                // 副階層（graft）の改変は graft 側で扱うため無視する。
                // 副Prefabと副FBXのGUIDが対象である。
                if let Some(phone) = phone.as_ref()
                    && let Some(target) = m.target_guid.as_ref()
                    && (target.eq_ignore_ascii_case(&phone.guid)
                        || target.eq_ignore_ascii_case(&phone.fbx_guid))
                {
                    continue;
                }
                // 主FBX以外への改変（他資産・文書内参照）は解釈できないため失敗させる。
                // 黙って落とさない。
                return Err(VrmError::new(
                    VrmErrorKind::InvalidPrefab,
                    format!(
                        "主FBX以外の資産への改変は未対応である: {}（{}）",
                        crate::escape_diag(&m.path),
                        crate::escape_diag(label),
                    ),
                ));
            }
            classify_main_mod(&tables, &mut overlays, m, label)?;
        }
    }
    Ok(overlays)
}

/// 主インスタンスの改変1件を振り分ける。姿勢・有効・改名は集計し、
/// 描画系の欄は無視し、構造変更と対象不明は明示的に失敗させる。
#[cfg(not(target_arch = "wasm32"))]
struct MainModTables<'a> {
    parsed: &'a ParsedPrefab,
    tr_to_fbx: &'a HashMap<FileId, usize>,
    go_to_fbx: &'a HashMap<FileId, usize>,
    renderer_fids: &'a HashSet<FileId>,
    import_tr: FileId,
    import_go: FileId,
}

#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::too_many_arguments)]
fn classify_main_mod(
    tables: &MainModTables<'_>,
    overlays: &mut MainInstanceOverlays,
    m: &PrefabMod,
    label: &str,
) -> Result<(), VrmError> {
    let fail_target = |why: &str| {
        VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!(
                "主FBXの{why}は未対応である: {}（対象{}）（{}）",
                crate::escape_diag(&m.path),
                m.target_fid,
                crate::escape_diag(label),
            ),
        )
    };
    let fail_structural = |why: &str| {
        VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!(
                "主FBXの{why}は未対応である: {}（{}）",
                crate::escape_diag(&m.path),
                crate::escape_diag(label),
            ),
        )
    };
    // 姿勢の成分上書き。`m_LocalEulerAnglesHint` は見かけの欄であり、姿勢に効かない。
    if m.path.starts_with("m_LocalEulerAnglesHint") {
        return Ok(());
    }
    if let Some(axis) = m.path.strip_prefix("m_LocalPosition.") {
        return apply_trs_component(overlays, tables, m, label, axis, 0);
    }
    if let Some(axis) = m.path.strip_prefix("m_LocalRotation.") {
        return apply_trs_component(overlays, tables, m, label, axis, 1);
    }
    if let Some(axis) = m.path.strip_prefix("m_LocalScale.") {
        return apply_trs_component(overlays, tables, m, label, axis, 2);
    }
    match m.path.as_str() {
        // 有効状態。ゲームオブジェクト対象だけを反映し、
        // 描画設定等の成分対象は無視する（階層の有効状態に効かない）。
        "m_IsActive" => {
            if m.target_fid == tables.import_go
                || tables.go_to_fbx.contains_key(&m.target_fid)
                || tables.parsed.local_go.contains_key(&m.target_fid)
            {
                overlays
                    .active
                    .insert(m.target_fid, mod_bool_value(m, label)?);
            } else if is_renderer_fid(tables.renderer_fids, m.target_fid) {
                // 描画設定の有効無効は階層に効かないため無視する。
            } else {
                return Err(fail_target("不明な対象への有効状態の上書き"));
            }
            Ok(())
        }
        // 改名。取り込み根はアバタールート名に使う。
        "m_Name" => {
            let name = mod_string_value(m, label)?;
            if m.target_fid == tables.import_go {
                overlays.root_name = Some(name);
            } else if tables.go_to_fbx.contains_key(&m.target_fid)
                || tables.parsed.local_go.contains_key(&m.target_fid)
            {
                overlays.names.insert(m.target_fid, name);
            } else {
                return Err(fail_target("不明な対象への改名"));
            }
            Ok(())
        }
        // 取り込み根の並び順は参照に効かないため無視する。
        // 骨への並び替えは構造変更であり、引き続き失敗させる。
        "m_RootOrder" => {
            if m.target_fid == tables.import_tr {
                Ok(())
            } else {
                Err(fail_structural("並び順の上書き"))
            }
        }
        // 描画系の欄は物理に効かないため無視する。
        // 対象がTransformやゲームオブジェクトの場合は破損として失敗させる。
        path if path.starts_with("m_AABB")
            || path.starts_with("m_Materials")
            || path.starts_with("m_ProbeAnchor")
            || path.starts_with("m_BlendShapeWeights")
            || path.starts_with("m_Controller")
            || path.starts_with("m_RootBone") =>
        {
            if tables.tr_to_fbx.contains_key(&m.target_fid)
                || tables.go_to_fbx.contains_key(&m.target_fid)
            {
                return Err(fail_structural("描画欄の想定外の対象"));
            }
            Ok(())
        }
        _ => {
            // 構造変更（`m_Children` 等）と成分値の上書きは未対応として失敗する。
            Err(fail_structural("想定外の改変"))
        }
    }
}

/// 姿勢の成分上書き1件を重ねる。`kind` は0=位置・1=回転・2=拡大率である。
/// 取り込み根の初期化は参照に効かないため無視する。
#[cfg(not(target_arch = "wasm32"))]
fn apply_trs_component(
    overlays: &mut MainInstanceOverlays,
    tables: &MainModTables<'_>,
    m: &PrefabMod,
    label: &str,
    axis: &str,
    kind: u8,
) -> Result<(), VrmError> {
    if m.target_fid == tables.import_tr {
        return Ok(());
    }
    if !tables.tr_to_fbx.contains_key(&m.target_fid) {
        return Err(VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!(
                "主FBXの不明な対象への姿勢上書きは未対応である: {}（対象{}）（{}）",
                crate::escape_diag(&m.path),
                m.target_fid,
                crate::escape_diag(label),
            ),
        ));
    }
    let value = mod_f64_value(m, label)?;
    let entry = overlays.trs.entry(m.target_fid).or_default();
    let slot: &mut Option<f64> = match (kind, axis) {
        (0, "x") => &mut entry.position[0],
        (0, "y") => &mut entry.position[1],
        (0, "z") => &mut entry.position[2],
        (1, "x") => &mut entry.rotation[0],
        (1, "y") => &mut entry.rotation[1],
        (1, "z") => &mut entry.rotation[2],
        (1, "w") => &mut entry.rotation[3],
        (2, "x") => &mut entry.scale[0],
        (2, "y") => &mut entry.scale[1],
        (2, "z") => &mut entry.scale[2],
        _ => {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!(
                    "主FBXの姿勢の軸が未対応である: {}（{}）",
                    crate::escape_diag(&m.path),
                    crate::escape_diag(label),
                ),
            ));
        }
    };
    *slot = Some(value);
    Ok(())
}

/// 描画設定の取り込みfileIDかどうかを判定する。
/// `m_IsActive` の対象が成分かゲームオブジェクトかの区別に使う。
#[cfg(not(target_arch = "wasm32"))]
fn is_renderer_fid(renderer_fids: &HashSet<FileId>, fid: FileId) -> bool {
    renderer_fids.contains(&fid)
}

/// 直接記述のSkinnedMeshRenderer1件。FBXとの対応付けに使う。
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone)]
struct ParsedSmr {
    /// 所属ゲームオブジェクトのfileID。
    go: FileId,
    /// 参照メッシュのfileID（`Type:Mesh->{モデル名}0`）。
    mesh_fid: FileId,
    /// 参照メッシュのGUID（小文字hex）。FBX資産の特定に使う。
    mesh_guid: Option<String>,
}

// Prefab文書の型付き読み取り結果。
#[cfg(not(target_arch = "wasm32"))]
struct ParsedPrefab {
    monos: Vec<ParsedMono>,
    /// 直接記述のSkinnedMeshRenderer列。FBX対応付けに使う。
    smrs: Vec<ParsedSmr>,
    /// Unityネイティブの回転制約（`RotationConstraint` 文書）。
    native_rotations: Vec<ParsedNativeRotation>,
    stripped_go: HashMap<FileId, StrippedRef>,
    stripped_tr: HashMap<FileId, StrippedRef>,
    local_go: HashMap<FileId, LocalGo>,
    local_tr: HashMap<FileId, LocalTr>,
    instances: Vec<ParsedInstance>,
}

/// Unityネイティブの回転制約1件。VRC制約ではなく `RotationConstraint`
/// 文書（classID `1818360609`）から読む。sidecarの `unity` 系制約になる。
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone)]
struct ParsedNativeRotation {
    /// 文書fileID。
    doc: FileId,
    /// 文書順（Prefabファイル内の出現順）。Unityの取得順に対応する。
    order: usize,
    /// 所属ゲームオブジェクトのfileID。
    go: FileId,
    /// 有効かどうか（`m_Enabled`）。
    enabled: bool,
    /// 制約の重み（`m_Weight`）。
    weight: f64,
    /// 回転に効かせる軸（`m_AffectRotationX/Y/Z`）。
    affects: [bool; 3],
    /// 稼働中かどうか（`m_IsContraintActive`）。
    active: bool,
    /// 参照先列（`m_Sources` の `sourceTransform` と重み）。
    /// 解決できない参照は呼び出し側で空ID・外部扱いにする。
    /// 重みはfloat32へ量子化済みである。
    sources: Vec<(FileId, f64)>,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone)]
struct StrippedRef {
    source_fid: FileId,
    source_guid: String,
    /// 所属するPrefabInstance文書。graft元の対応付けに使う。
    instance: FileId,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone)]
struct LocalGo {
    name: String,
    active: bool,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone)]
#[allow(dead_code)] // スマホ graft 保留中の保持項目であり、次単位で配線する。
struct LocalTr {
    go: FileId,
    father: FileId,
    children: Vec<FileId>,
    /// `m_RootOrder`（兄弟順）。Prefab直置き・追加物の順序決定に使う。
    root_order: i64,
    position: [f64; 3],
    rotation: [f64; 4],
    scale: [f64; 3],
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone)]
struct ParsedMono {
    doc: FileId,
    /// 文書順（Prefabファイル内の出現順）。Unityの取得順に対応する。
    /// 同一ノード上の順序決定に使い、fileIDの数値順ではない。
    order: usize,
    go: FileId,
    enabled: bool,
    script_guid: String,
    script_fid: i32,
    extra: yaml_serde::Mapping,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone)]
#[allow(dead_code)] // スマホ graft 保留中の保持項目であり、次単位で配線する。
struct ParsedInstance {
    doc: FileId,
    source_guid: String,
    parent: FileId,
    modifications: Vec<PrefabMod>,
    added_objects: Vec<AddedObject>,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone)]
#[allow(dead_code)] // スマホ graft 保留中の保持項目であり、次単位で配線する。
struct PrefabMod {
    target_fid: FileId,
    target_guid: Option<String>,
    path: String,
    value: Option<Value>,
    obj_ref: Option<ParsedRef>,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone)]
struct ParsedRef {
    fid: FileId,
    guid: Option<String>,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone)]
#[allow(dead_code)] // スマホ graft 保留中の保持項目であり、次単位で配線する。
struct AddedObject {
    target_fid: FileId,
    target_guid: String,
    object: FileId,
}

#[cfg(not(target_arch = "wasm32"))]
fn parse_ref_value(value: &Value) -> Result<ParsedRef, VrmError> {
    let mapping = value
        .as_mapping()
        .ok_or_else(|| VrmError::new(VrmErrorKind::InvalidPrefab, "参照がマッピングでない"))?;
    let fid = mapping
        .get("fileID")
        .and_then(|v| v.as_i64())
        .ok_or_else(|| VrmError::new(VrmErrorKind::InvalidPrefab, "参照にfileIDがない"))?;
    let guid = match mapping.get("guid") {
        None | Some(Value::Null) => None,
        Some(v) => v
            .as_str()
            .map(|s| s.to_ascii_lowercase())
            .or(Some(String::new())),
    };
    // GUIDの検証は呼び出し側で行う。空文字はGUIDなしとして扱う。
    let guid = match guid.as_deref() {
        None | Some("") => None,
        Some(s) => Some(s.to_owned()),
    };
    Ok(ParsedRef { fid, guid })
}

#[cfg(not(target_arch = "wasm32"))]
fn parse_prefab_docs(prefab: &SerializedFile, label: &str) -> Result<ParsedPrefab, VrmError> {
    parse_prefab_docs_inner(prefab, label)
}

#[cfg(not(target_arch = "wasm32"))]
fn parse_prefab_docs_from_bytes(bytes: &[u8], label: &str) -> Result<ParsedPrefab, VrmError> {
    use crate::unity::ParseOptions;
    let parsed = SerializedFile::parse(bytes, &ParseOptions::new()).map_err(|e| {
        VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!(
                "副Prefabの解析に失敗した（{}）: {e}",
                crate::escape_diag(label)
            ),
        )
    })?;
    parse_prefab_docs_inner(&parsed, label)
}

#[cfg(not(target_arch = "wasm32"))]
fn parse_prefab_docs_inner(prefab: &SerializedFile, label: &str) -> Result<ParsedPrefab, VrmError> {
    let mut monos = Vec::new();
    let mut smrs = Vec::new();
    let mut native_rotations = Vec::new();
    // 文書順の通し番号。Unityの取得順（`GetComponentsInChildren` 順）に相当し、
    // 同一ノード上の順序決定に使う。fileIDの数値順ではない。
    let mut mono_order: usize = 0;
    let mut native_order: usize = 0;
    let mut stripped_go = HashMap::new();
    let mut stripped_tr = HashMap::new();
    let mut local_go = HashMap::new();
    let mut local_tr = HashMap::new();
    let mut instances = Vec::new();
    for doc in prefab.documents() {
        if doc.stripped() {
            let raw = doc.raw().as_mapping().ok_or_else(|| {
                VrmError::new(VrmErrorKind::InvalidPrefab, "stripped文書が写像でない")
            })?;
            // strippedはGameObjectかTransformのいずれかである。
            if doc.class_name() == "GameObject" {
                let corr = raw.get("m_CorrespondingSourceObject").ok_or_else(|| {
                    VrmError::new(VrmErrorKind::InvalidPrefab, "strippedに対応元がない")
                })?;
                let r = parse_ref_value(corr)?;
                let instance = raw
                    .get("m_PrefabInstance")
                    .and_then(|v| v.as_mapping())
                    .and_then(|m| m.get("fileID"))
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                stripped_go.insert(
                    doc.file_id(),
                    StrippedRef {
                        source_fid: r.fid,
                        source_guid: r.guid.unwrap_or_default(),
                        instance,
                    },
                );
            } else if doc.class_name() == "Transform" {
                let corr = raw.get("m_CorrespondingSourceObject").ok_or_else(|| {
                    VrmError::new(VrmErrorKind::InvalidPrefab, "strippedに対応元がない")
                })?;
                let r = parse_ref_value(corr)?;
                let instance = raw
                    .get("m_PrefabInstance")
                    .and_then(|v| v.as_mapping())
                    .and_then(|m| m.get("fileID"))
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                stripped_tr.insert(
                    doc.file_id(),
                    StrippedRef {
                        source_fid: r.fid,
                        source_guid: r.guid.unwrap_or_default(),
                        instance,
                    },
                );
            }
            // m_PrefabInstanceは対応付けに使い、対応付けはfileIDで行う。
            continue;
        }
        // Unityネイティブの回転制約は型付き読み取りの対象外であり、
        // 原文書から直接読む。VRC制約と合わせて収集する。
        if doc.class_id() == 1818360609 {
            if doc.class_name() != "RotationConstraint" {
                return Err(VrmError::new(
                    VrmErrorKind::InvalidPrefab,
                    format!(
                        "classIDとクラス名が一致しない（文書{}）（{}）",
                        doc.file_id(),
                        crate::escape_diag(label),
                    ),
                ));
            }
            let mut native = parse_native_rotation(doc, label)?;
            native.order = native_order;
            native_order += 1;
            native_rotations.push(native);
            continue;
        }
        match doc.decode() {
            Ok(TypedObject::MonoBehaviour(mono)) => {
                let guid = mono
                    .script
                    .guid
                    .as_ref()
                    .map(|g| g.as_str().to_owned())
                    .unwrap_or_default();
                monos.push(ParsedMono {
                    doc: doc.file_id(),
                    order: mono_order,
                    go: mono.game_object.file_id,
                    enabled: mono.enabled,
                    script_guid: guid,
                    script_fid: mono.script.file_id as i32,
                    extra: mono.extra,
                });
                mono_order += 1;
            }
            Ok(TypedObject::GameObject(go)) => {
                local_go.insert(
                    doc.file_id(),
                    LocalGo {
                        name: go.name,
                        active: go.active,
                    },
                );
            }
            Ok(TypedObject::SkinnedMeshRenderer(renderer)) => {
                smrs.push(ParsedSmr {
                    go: renderer.game_object.file_id,
                    mesh_fid: renderer.mesh.file_id,
                    mesh_guid: renderer
                        .mesh
                        .guid
                        .as_ref()
                        .map(|g| g.as_str().to_ascii_lowercase()),
                });
            }
            Ok(TypedObject::Transform(tr)) => {
                // `m_RootOrder` は兄弟順であり、型付き読み取りに無いため
                // 原文書から読む。無いPrefab（旧形式）は0とみなす。
                // 汎用経路では `m_Children` 順が正であり、値は使わない。
                // 旧形式の追加物の順序は登録順（文書順）に定まる。
                let root_order = doc
                    .raw()
                    .as_mapping()
                    .and_then(|m| m.get("m_RootOrder"))
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                local_tr.insert(
                    doc.file_id(),
                    LocalTr {
                        go: tr.game_object.file_id,
                        father: tr.father.file_id,
                        children: tr.children.iter().map(|c| c.file_id).collect(),
                        root_order,
                        position: tr.position,
                        rotation: tr.rotation,
                        scale: tr.scale,
                    },
                );
            }
            Ok(TypedObject::PrefabInstance(instance)) => {
                let mods = instance.modifications().map_err(|e| {
                    VrmError::new(
                        VrmErrorKind::InvalidPrefab,
                        format!(
                            "Prefabの改変内容を読めない（{}）: {e}",
                            crate::escape_diag(label),
                        ),
                    )
                })?;
                let mut modifications = Vec::with_capacity(mods.len());
                for m in mods {
                    modifications.push(PrefabMod {
                        target_fid: m.target.file_id,
                        target_guid: m.target.guid.map(|g| g.as_str().to_owned()),
                        path: m.property_path,
                        value: m.value,
                        obj_ref: m.object_reference.map(|r| ParsedRef {
                            fid: r.file_id,
                            guid: r.guid.map(|g| g.as_str().to_owned()),
                        }),
                    });
                }
                // m_AddedGameObjectsはextraに残る。順序を保って読む。
                let mut added_objects = Vec::new();
                if let Some(Value::Mapping(extra)) = instance.extra.get("m_Modification") {
                    // extraはm_Modification写像である。
                    if let Some(Value::Sequence(seq)) = extra.get("m_AddedGameObjects") {
                        for item in seq {
                            let mapping = item.as_mapping().ok_or_else(|| {
                                VrmError::new(
                                    VrmErrorKind::InvalidPrefab,
                                    "m_AddedGameObjectsの項目が写像でない",
                                )
                            })?;
                            let target = mapping
                                .get("targetCorrespondingSourceObject")
                                .ok_or_else(|| {
                                    VrmError::new(
                                        VrmErrorKind::InvalidPrefab,
                                        "m_AddedGameObjectsに対象がない",
                                    )
                                })?;
                            let tr = parse_ref_value(target)?;
                            let object = mapping.get("addedObject").ok_or_else(|| {
                                VrmError::new(
                                    VrmErrorKind::InvalidPrefab,
                                    "m_AddedGameObjectsに追加物がない",
                                )
                            })?;
                            let ob = parse_ref_value(object)?;
                            added_objects.push(AddedObject {
                                target_fid: tr.fid,
                                target_guid: tr.guid.unwrap_or_default(),
                                object: ob.fid,
                            });
                        }
                    }
                }
                instances.push(ParsedInstance {
                    doc: doc.file_id(),
                    source_guid: instance
                        .source
                        .guid
                        .map(|g| g.as_str().to_owned())
                        .unwrap_or_default(),
                    // 親は `m_Modification` の内側にある。直下には無い。
                    parent: instance
                        .extra
                        .get("m_Modification")
                        .and_then(|v| v.as_mapping())
                        .and_then(|m| m.get("m_TransformParent"))
                        .and_then(|v| v.as_mapping())
                        .and_then(|m| m.get("fileID"))
                        .and_then(|v| v.as_i64())
                        .unwrap_or(0),
                    modifications,
                    added_objects,
                });
            }
            Ok(_) => {}
            Err(e) => {
                return Err(VrmError::new(
                    VrmErrorKind::InvalidPrefab,
                    format!(
                        "Prefabの文書を読めない（{}）: {e}",
                        crate::escape_diag(label),
                    ),
                ));
            }
        }
    }
    Ok(ParsedPrefab {
        monos,
        smrs,
        native_rotations,
        stripped_go,
        stripped_tr,
        local_go,
        local_tr,
        instances,
    })
}

/// Unityネイティブの回転制約1件を原文書から読む。
#[cfg(not(target_arch = "wasm32"))]
fn parse_native_rotation(
    doc: &crate::unity::Document,
    label: &str,
) -> Result<ParsedNativeRotation, VrmError> {
    let raw = doc.raw().as_mapping().ok_or_else(|| {
        VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!(
                "回転制約が写像でない（文書{}）（{}）",
                doc.file_id(),
                crate::escape_diag(label),
            ),
        )
    })?;
    let get = |key: &str| {
        raw.get(key).ok_or_else(|| {
            VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!(
                    "回転制約に{key}がない（文書{}）（{}）",
                    doc.file_id(),
                    crate::escape_diag(label),
                ),
            )
        })
    };
    let go_value = get("m_GameObject")?;
    let go = parse_ref_value(go_value)?.fid;
    let enabled = match get("m_Enabled")? {
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_i64().unwrap_or(0) != 0,
        _ => {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!(
                    "回転制約のm_Enabledが真偽値でない（文書{}）（{}）",
                    doc.file_id(),
                    crate::escape_diag(label),
                ),
            ));
        }
    };
    let weight = read_f64_value(get("m_Weight")?, "m_Weight", label)?;
    let mut affects = [true; 3];
    for (i, key) in [
        "m_AffectRotationX",
        "m_AffectRotationY",
        "m_AffectRotationZ",
    ]
    .iter()
    .enumerate()
    {
        affects[i] = match raw.get(*key) {
            None => true,
            Some(Value::Bool(b)) => *b,
            Some(Value::Number(n)) => n.as_i64().unwrap_or(0) != 0,
            _ => {
                return Err(VrmError::new(
                    VrmErrorKind::InvalidPrefab,
                    format!(
                        "回転制約の{key}が真偽値でない（文書{}）（{}）",
                        doc.file_id(),
                        crate::escape_diag(label),
                    ),
                ));
            }
        };
    }
    // 稼働中かどうかは `m_IsContraintActive`（Unity側の綴り）で読む。
    // `m_IsLocked` は編集時の錠であり、稼働には使わない。
    let active = match raw.get("m_IsContraintActive") {
        None => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_i64().unwrap_or(0) != 0,
        _ => {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!(
                    "回転制約のm_IsContraintActiveが真偽値でない（文書{}）（{}）",
                    doc.file_id(),
                    crate::escape_diag(label),
                ),
            ));
        }
    };
    let mut sources = Vec::new();
    if let Some(Value::Sequence(seq)) = raw.get("m_Sources") {
        for item in seq {
            let mapping = item.as_mapping().ok_or_else(|| {
                VrmError::new(
                    VrmErrorKind::InvalidPrefab,
                    format!(
                        "回転制約の参照が写像でない（文書{}）（{}）",
                        doc.file_id(),
                        crate::escape_diag(label),
                    ),
                )
            })?;
            let tr_value = mapping.get("sourceTransform").ok_or_else(|| {
                VrmError::new(
                    VrmErrorKind::InvalidPrefab,
                    format!(
                        "回転制約の参照先がない（文書{}）（{}）",
                        doc.file_id(),
                        crate::escape_diag(label),
                    ),
                )
            })?;
            let r = parse_ref_value(tr_value)?;
            let weight_raw = mapping
                .get("weight")
                .and_then(|v| v.as_f64().or_else(|| v.as_i64().map(|n| n as f64)))
                .unwrap_or(0.0);
            // 重みはfloat32へ量子化する。VRC制約の `Weight` と同じ扱いである。
            sources.push((r.fid, quantize_f32(weight_raw)));
        }
    }
    Ok(ParsedNativeRotation {
        doc: doc.file_id(),
        // 文書順は呼び出し側で付ける。
        order: 0,
        go,
        enabled,
        weight,
        affects,
        active,
        sources,
    })
}

#[cfg(not(target_arch = "wasm32"))]
fn find_instances<'p>(parsed: &'p ParsedPrefab, fbx_guid: &str) -> Vec<&'p ParsedInstance> {
    parsed
        .instances
        .iter()
        .filter(|i| i.source_guid.eq_ignore_ascii_case(fbx_guid))
        .collect()
}

// FBX階層（ソート付き）。
#[cfg(not(target_arch = "wasm32"))]
#[allow(dead_code)] // スマホ graft 保留中の保持項目であり、次単位で配線する。
struct FbxHierarchy {
    paths: Vec<String>,
    children_sorted: Vec<Vec<usize>>,
    go_fid_at: Vec<FileId>,
    tr_fid_at: Vec<FileId>,
    path_at: HashMap<FileId, String>,
}

#[cfg(not(target_arch = "wasm32"))]
fn build_fbx_hierarchy(file: &FbxFile<'_>, label: &str) -> Result<FbxHierarchy, VrmError> {
    use std::collections::HashMap;
    let models = file.models();
    let at: HashMap<i64, usize> = models
        .iter()
        .enumerate()
        .map(|(i, m)| (m.id.value(), i))
        .collect();
    let mut paths: Vec<Option<String>> = vec![None; models.len()];
    let mut visiting = vec![false; models.len()];
    fn visit(
        models: &[crate::fbx::Model],
        at: &HashMap<i64, usize>,
        paths: &mut [Option<String>],
        visiting: &mut [bool],
        index: usize,
        label: &str,
    ) -> Result<String, VrmError> {
        if let Some(p) = paths[index].clone() {
            return Ok(p);
        }
        if visiting[index] {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!("モデルの親子に循環がある（{}）", crate::escape_diag(label)),
            ));
        }
        if models[index].name.contains('/') {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!(
                    "モデル名に `/` を含むため階層パスが作れない: {}",
                    crate::escape_diag(&models[index].name),
                ),
            ));
        }
        visiting[index] = true;
        let path = match models[index].parent {
            None => models[index].name.clone(),
            Some(p) => {
                let pi = at.get(&p.value()).copied().ok_or_else(|| {
                    VrmError::new(VrmErrorKind::InvalidPrefab, "親モデルが一覧にない")
                })?;
                let parent = visit(models, at, paths, visiting, pi, label)?;
                format!("{parent}/{}", models[index].name)
            }
        };
        visiting[index] = false;
        paths[index] = Some(path.clone());
        Ok(path)
    }
    let mut out_paths = Vec::with_capacity(models.len());
    for i in 0..models.len() {
        out_paths.push(visit(models, &at, &mut paths, &mut visiting, i, label)?);
    }
    // 子を名前順に並べる（UnityのsortHierarchyByName相当）。
    let mut children_sorted: Vec<Vec<usize>> = vec![Vec::new(); models.len()];
    for (i, m) in models.iter().enumerate() {
        if let Some(p) = m.parent
            && let Some(&pi) = at.get(&p.value())
        {
            children_sorted[pi].push(i);
        }
    }
    for (i, children) in children_sorted.iter_mut().enumerate() {
        let _ = i;
        // Unityの取り込み順は大文字小文字を区別しない名前順である。
        // 実測（Milfyのメッシュルート）で `Bandage_*` が `BandAid_*` より先に来るため、
        // バイト順ではなく小文字化した名前で比べ、同点はバイト順で決定的にする。
        children.sort_by(|a, b| {
            models[*a]
                .name
                .to_ascii_lowercase()
                .cmp(&models[*b].name.to_ascii_lowercase())
                .then(models[*a].name.cmp(&models[*b].name))
        });
    }
    // fileIDの算出。
    let mut go_fid_at = Vec::with_capacity(models.len());
    let mut tr_fid_at = Vec::with_capacity(models.len());
    let mut path_at = HashMap::new();
    for p in &out_paths {
        let go = crate::unity::game_object_file_id(p);
        let tr = transform_file_id(p);
        path_at.insert(go, p.clone());
        path_at.insert(tr, p.clone());
        go_fid_at.push(go);
        tr_fid_at.push(tr);
    }
    // 取り込みルートも登録する。
    path_at.insert(crate::unity::game_object_file_id(""), String::new());
    path_at.insert(transform_file_id(""), String::new());
    Ok(FbxHierarchy {
        paths: out_paths,
        children_sorted,
        go_fid_at,
        tr_fid_at,
        path_at,
    })
}

/// Transformの取り込みfileIDを作る。`Type:Transform->//RootNode/root/{path}/Transform0` である。
#[cfg(not(target_arch = "wasm32"))]
fn transform_file_id(hierarchy_path: &str) -> FileId {
    let text = if hierarchy_path.is_empty() {
        "Type:Transform->//RootNode/root/Transform0".to_owned()
    } else {
        format!("Type:Transform->//RootNode/root/{hierarchy_path}/Transform0")
    };
    xxhash_rust::xxh64::xxh64(text.as_bytes(), 0) as FileId
}

// KumaPhone相当の副階層。Milfy.prefab内の2件目のインスタンス（副Prefab）と、
// その実体が指すSmartPhone.fbxを組み合わせた graft の材料である。
#[cfg(not(target_arch = "wasm32"))]
struct PhoneSubtree {
    /// 副PrefabのGUID（小文字）。Milfy側の解決と graft 対象の判定に使う。
    guid: String,
    /// 副Prefabの資産パス（`Assets/` 付き）。graft 根の既定名に使う。
    asset_path: String,
    /// 副Prefabの型付き読み取り。成分収集と追加物の配置に使う。
    parsed: ParsedPrefab,
    /// 副Prefab内のSmartPhone.fbxインスタンス文書。改変の適用元である。
    phone_instance_doc: FileId,
    /// SmartPhone.fbxのGUID（小文字）。
    fbx_guid: String,
    /// SmartPhone.fbxの資産パス（`Assets/` 付き）。sourceModelsに使う。
    fbx_asset_path: String,
    /// SmartPhone.fbxのSHA-256（小文字hex）。sourceModelsに使う。
    fbx_sha256: String,
    /// SmartPhone.fbxのモデル列。階層の組み立てに使う。
    models: Vec<PhoneModel>,
    /// モデル索引ごとの子索引列。名前順（小文字優先・同点はバイト順）である。
    children_sorted: Vec<Vec<usize>>,
    /// ルート索引列。同じ規則で名前順である。
    roots_sorted: Vec<usize>,
}

/// SmartPhone.fbxのモデル1件の所有側の写し。階層の組み立てに使う。
/// 継承方式は主階層と同じく素朴な親子積で扱い、区別しない。
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone)]
struct PhoneModel {
    name: String,
    parent: Option<usize>,
    transform: LocalTransform,
}

// ネストした副階層を解決する。Milfy.prefab内の主FBX以外のインスタンスが対象である。
// 主FBXへの参照と副Prefabへの参照がちょうど1件ずつあることを確かめ、
// それ以外（複数・FBX直置き等）は黙って選ばず明示的に失敗する。
#[cfg(not(target_arch = "wasm32"))]
fn resolve_phone_subtree(
    index: &crate::unity::AssetIndex,
    canonical_assets: &std::path::Path,
    parsed: &ParsedPrefab,
    main_fbx_guid: &str,
    label: &str,
) -> Result<Option<PhoneSubtree>, VrmError> {
    // 主FBX以外のインスタンス（副Prefab）を集める。
    let mut nested: Vec<&ParsedInstance> = Vec::new();
    for instance in &parsed.instances {
        if instance.source_guid.is_empty()
            || instance.source_guid.eq_ignore_ascii_case(main_fbx_guid)
        {
            continue;
        }
        nested.push(instance);
    }
    if nested.is_empty() {
        return Ok(None);
    }
    if nested.len() != 1 {
        return Err(VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!(
                "副Prefabの解決は1件に限る（{}件）（{}）",
                nested.len(),
                crate::escape_diag(label),
            ),
        ));
    }
    let nested = nested[0];
    let guid = nested.source_guid.clone();
    let Ok(guid_key) = crate::unity::Guid::parse(&guid) else {
        return Err(VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!(
                "PrefabInstanceのGUIDが不正である（{}）",
                crate::escape_diag(label),
            ),
        ));
    };
    let Some(record) = index.get(&guid_key) else {
        // 索引にないGUIDは解決できず失敗する。黙って落とさない。
        return Err(VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!(
                "PrefabInstanceの参照先が索引にない: {}（{}）",
                crate::escape_diag(&guid),
                crate::escape_diag(label),
            ),
        ));
    };
    // 副階層はprefabに限る。FBX直置き等は未対応として失敗する。
    if !record.path.ends_with(".prefab") {
        return Err(VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!(
                "副階層の参照がprefabでないため未対応である: {}（{}）",
                crate::escape_diag(&record.path),
                crate::escape_diag(label),
            ),
        ));
    }
    let full = canonical_assets.join(&record.path);
    let prefab_bytes = std::fs::read(&full).map_err(|e| {
        VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!("副Prefabを読めない: {e}"),
        )
    })?;
    // 副Prefab内のSmartPhone.fbx相当を探す。FBXインスタンス1件に限る。
    let phone_parsed = parse_prefab_docs_from_bytes(&prefab_bytes, label)?;
    let mut phone_instances: Vec<&ParsedInstance> = phone_parsed
        .instances
        .iter()
        .filter(|i| !i.source_guid.is_empty())
        .collect();
    if phone_instances.len() != 1 {
        return Err(VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!(
                "副Prefab内のFBX参照が{}件あるため未対応である（{}）",
                phone_instances.len(),
                crate::escape_diag(label),
            ),
        ));
    }
    let phone_instance = phone_instances.pop().expect("件数は確かめた");
    let phone_instance_doc = phone_instance.doc;
    let phone_fbx_guid = phone_instance.source_guid.clone();
    let Ok(phone_key) = crate::unity::Guid::parse(&phone_fbx_guid) else {
        return Err(VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!(
                "副PrefabのFBX参照GUIDが不正である（{}）",
                crate::escape_diag(label)
            ),
        ));
    };
    let Some(fbx_record) = index.get(&phone_key) else {
        return Err(VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!(
                "副PrefabのFBXが索引にない: {}（{}）",
                crate::escape_diag(&phone_fbx_guid),
                crate::escape_diag(label),
            ),
        ));
    };
    if !fbx_record.path.ends_with(".fbx") && !fbx_record.path.ends_with(".FBX") {
        return Err(VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!(
                "副Prefabの参照先がFBXでないため未対応である: {}（{}）",
                crate::escape_diag(&fbx_record.path),
                crate::escape_diag(label),
            ),
        ));
    }
    let fbx_full = canonical_assets.join(&fbx_record.path);
    let fbx_bytes = std::fs::read(&fbx_full)
        .map_err(|e| VrmError::new(VrmErrorKind::InvalidPrefab, format!("副FBXを読めない: {e}")))?;
    let fbx_asset_path = format!("Assets/{}", fbx_record.path);
    let fbx_sha256 = {
        use sha2::Digest;
        let digest = sha2::Sha256::digest(&fbx_bytes);
        let mut sha = String::with_capacity(64);
        for byte in digest {
            sha.push(char::from_digit(u32::from(byte >> 4), 16).expect("hexである"));
            sha.push(char::from_digit(u32::from(byte & 0x0f), 16).expect("hexである"));
        }
        sha
    };
    // 副FBXの階層を所有側へ写す。借用を残さない。
    let models = read_phone_models(&fbx_bytes, label)?;
    let (children_sorted, roots_sorted) = sort_phone_hierarchy(&models, label)?;
    Ok(Some(PhoneSubtree {
        guid,
        asset_path: format!("Assets/{}", record.path),
        parsed: phone_parsed,
        phone_instance_doc,
        fbx_guid: phone_fbx_guid,
        fbx_asset_path,
        fbx_sha256,
        models,
        children_sorted,
        roots_sorted,
    }))
}

/// 副FBXのモデル列を所有側へ写す。階層の組み立てだけに使う。
#[cfg(not(target_arch = "wasm32"))]
fn read_phone_models(bytes: &[u8], label: &str) -> Result<Vec<PhoneModel>, VrmError> {
    let file = FbxFile::parse(bytes, &FbxOptions::default()).map_err(|e| {
        VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!("副FBXを読めない（{}）: {e}", crate::escape_diag(label)),
        )
    })?;
    let models = file.models();
    let at: HashMap<i64, usize> = models
        .iter()
        .enumerate()
        .map(|(i, m)| (m.id.value(), i))
        .collect();
    let mut out = Vec::with_capacity(models.len());
    for model in models {
        let parent = match model.parent {
            None => None,
            Some(p) => Some(at.get(&p.value()).copied().ok_or_else(|| {
                VrmError::new(VrmErrorKind::InvalidPrefab, "副FBXの親モデルが一覧にない")
            })?),
        };
        out.push(PhoneModel {
            name: model.name.clone(),
            parent,
            transform: model.transform,
        });
    }
    Ok(out)
}

/// 副FBXの子列とルート列を名前順に並べる。主FBXの取り込み順と同じ規則
/// （小文字化した名前優先・同点はバイト順）であり、Unityの表示順と一致させる。
#[cfg(not(target_arch = "wasm32"))]
fn sort_phone_hierarchy(
    models: &[PhoneModel],
    label: &str,
) -> Result<(Vec<Vec<usize>>, Vec<usize>), VrmError> {
    for model in models {
        if model.name.contains('/') {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!(
                    "副FBXのモデル名に `/` を含むため階層パスが作れない: {}（{}）",
                    crate::escape_diag(&model.name),
                    crate::escape_diag(label),
                ),
            ));
        }
    }
    let mut children_sorted: Vec<Vec<usize>> = vec![Vec::new(); models.len()];
    let mut roots: Vec<usize> = Vec::new();
    for (i, model) in models.iter().enumerate() {
        match model.parent {
            Some(p) => children_sorted[p].push(i),
            None => roots.push(i),
        }
    }
    for children in children_sorted.iter_mut() {
        children.sort_by(|a, b| {
            models[*a]
                .name
                .to_ascii_lowercase()
                .cmp(&models[*b].name.to_ascii_lowercase())
                .then(models[*a].name.cmp(&models[*b].name))
        });
    }
    roots.sort_by(|a, b| {
        models[*a]
            .name
            .to_ascii_lowercase()
            .cmp(&models[*b].name.to_ascii_lowercase())
            .then(models[*a].name.cmp(&models[*b].name))
    });
    Ok((children_sorted, roots))
}

/// YAML値からf64を読む。整数も受け付ける。Unityのfloat32へ量子化して返す。
#[cfg(not(target_arch = "wasm32"))]
fn read_f64_value(value: &Value, what: &str, label: &str) -> Result<f64, VrmError> {
    match value {
        Value::Number(n) => n.as_f64().map(quantize_f32).ok_or_else(|| {
            VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!("{what}が数値でない（{}）", crate::escape_diag(label)),
            )
        }),
        _ => Err(VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!("{what}が数値でない（{}）", crate::escape_diag(label)),
        )),
    }
}

/// YAML値からboolを読む。0/1も受け付ける。
#[cfg(not(target_arch = "wasm32"))]
fn read_bool_value(value: &Value, what: &str, label: &str) -> Result<bool, VrmError> {
    match value {
        Value::Bool(b) => Ok(*b),
        Value::Number(n) => match n.as_i64() {
            Some(0) => Ok(false),
            Some(1) => Ok(true),
            _ => Err(VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!("{what}が0/1でない（{}）", crate::escape_diag(label)),
            )),
        },
        _ => Err(VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!("{what}が真偽値でない（{}）", crate::escape_diag(label)),
        )),
    }
}

/// `{x, y, z}` 形を読む。
#[cfg(not(target_arch = "wasm32"))]
fn read_xyz_value(value: &Value, what: &str, label: &str) -> Result<[f64; 3], VrmError> {
    let mapping = value.as_mapping().ok_or_else(|| {
        VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!("{what}が写像でない（{}）", crate::escape_diag(label)),
        )
    })?;
    let mut out = [0.0; 3];
    for (i, axis) in ["x", "y", "z"].iter().enumerate() {
        let v = mapping.get(*axis).ok_or_else(|| {
            VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!("{what}に{axis}がない（{}）", crate::escape_diag(label)),
            )
        })?;
        out[i] = read_f64_value(v, what, label)?;
    }
    Ok(out)
}

/// `{x, y, z, w}` 形を読む。
#[cfg(not(target_arch = "wasm32"))]
fn read_xyzw_value(value: &Value, what: &str, label: &str) -> Result<[f64; 4], VrmError> {
    let mapping = value.as_mapping().ok_or_else(|| {
        VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!("{what}が写像でない（{}）", crate::escape_diag(label)),
        )
    })?;
    let mut out = [0.0; 4];
    for (i, axis) in ["x", "y", "z", "w"].iter().enumerate() {
        let v = mapping.get(*axis).ok_or_else(|| {
            VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!("{what}に{axis}がない（{}）", crate::escape_diag(label)),
            )
        })?;
        out[i] = read_f64_value(v, what, label)?;
    }
    Ok(out)
}

// 最終ツリーの1節点。
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone)]
struct TreeNode {
    name: String,
    parent: Option<usize>,
    children: Vec<usize>,
    /// シーンルートからの名前列。先頭はアバタールート名である。
    scene_path: Vec<String>,
    // FBX由来の場合の出所。
    source: Option<TreeSource>,
    // シーン追加の場合のローカルTRS（prefab由来）。
    added_trs: Option<([f64; 3], [f64; 4], [f64; 3])>,
    // FBXモデル索引（Milfy.fbx由来の場合）。
    #[allow(dead_code)]
    fbx_index: Option<usize>,
    /// 副FBXモデル索引（SmartPhone.fbx由来の場合）。参照姿勢はscene扱いである。
    phone_model: Option<usize>,
    /// 自身の有効状態。prefabの `m_IsActive` を反映し、祖先と合わせて
    /// `activeInHierarchy` を判定する。主階層は常に真である。
    active_self: bool,
    /// ローカルGOのfileID（シーン追加の場合）。参照解決に使う。
    #[allow(dead_code)]
    local_go: Option<FileId>,
    /// ローカルTransformのfileID（シーン追加の場合）。参照解決に使う。
    local_tr: Option<FileId>,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone)]
struct TreeSource {
    model_id: String,
    path: Vec<String>,
    asset_guid: String,
    local_id: FileId,
}

/// ツリー組み立ての入力。引数の肥大化を避けるための束ねである。
#[cfg(not(target_arch = "wasm32"))]
struct TreeAssembly<'a> {
    fbx: &'a FbxFile<'a>,
    hier: &'a FbxHierarchy,
    parsed: &'a ParsedPrefab,
    phone: &'a Option<PhoneSubtree>,
    main_fbx_guid: &'a str,
    model_id_main: &'a str,
    overlays: &'a MainInstanceOverlays,
    label: &'a str,
}

#[cfg(not(target_arch = "wasm32"))]
fn assemble_tree(input: &TreeAssembly<'_>) -> Result<Vec<TreeNode>, VrmError> {
    // 副階層がある場合はMilfy相当の graft 方式、無い場合は汎用方式で組み立てる。
    // 汎用方式はFBX階層とPrefabの追加物を `m_Father`・`m_Children` で結び、
    // 姿勢の上書きを適用する。
    if input.phone.is_some() {
        return build_milfy_tree(
            input.fbx,
            input.hier,
            input.parsed,
            input.phone,
            input.main_fbx_guid,
            input.model_id_main,
            input.label,
        );
    }
    build_general_tree(
        input.fbx,
        input.hier,
        input.parsed,
        input.main_fbx_guid,
        input.model_id_main,
        input.overlays,
        input.label,
    )
}

/// 汎用のツリーを組み立てる。副階層（graft）が無いPrefab用である。
///
/// FBX階層を名前順DFSで出し、Prefabの追加物（ローカルGO/TR）を
/// `m_Father`・`m_Children` で結ぶ。FBX由来ノードの子はFBX子
/// （名前順）の後に追加物を `m_RootOrder` 順で足し、ローカル由来ノードの
/// 子は `m_Children` 順で出す。姿勢の上書きはFBX由来ノードに適用済みの
/// ものとして扱い（`compute_reference_locals` で反映する）、有効状態と
/// 改名はここで反映する。アバタールートは取り込み根であり、
/// sidecarには含めない（Unityヘルパと同順）。
#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::too_many_arguments)]
fn build_general_tree(
    fbx: &FbxFile<'_>,
    hier: &FbxHierarchy,
    parsed: &ParsedPrefab,
    main_fbx_guid: &str,
    model_id_main: &str,
    overlays: &MainInstanceOverlays,
    label: &str,
) -> Result<Vec<TreeNode>, VrmError> {
    let models = fbx.models();
    let import_tr = transform_file_id("");
    let import_go = crate::unity::game_object_file_id("");
    // 取り込み根の有効状態。ツリーの頂点に畳み込む。
    let import_active = overlays.active.get(&import_go).copied().unwrap_or(true);
    // strippedの対応付け。ローカル文書IDからFBX由来への写像である。
    // 取り込み根に対応するものはアバタールート（親なし）として扱う。
    let mut stripped_tr_to_fbx: HashMap<FileId, Option<usize>> = HashMap::new();
    for (local, stripped) in parsed.stripped_tr.iter() {
        if !stripped.source_guid.eq_ignore_ascii_case(main_fbx_guid) {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!(
                    "主FBX以外のstrippedがあるため未対応である（文書{local}）（{}）",
                    crate::escape_diag(label),
                ),
            ));
        }
        if stripped.source_fid == import_tr {
            stripped_tr_to_fbx.insert(*local, None);
        } else {
            let Some(fbx_index) = hier
                .tr_fid_at
                .iter()
                .position(|fid| *fid == stripped.source_fid)
            else {
                return Err(VrmError::new(
                    VrmErrorKind::InvalidPrefab,
                    format!(
                        "strippedの対応元がFBXにない（文書{local}）（{}）",
                        crate::escape_diag(label),
                    ),
                ));
            };
            stripped_tr_to_fbx.insert(*local, Some(fbx_index));
        }
    }
    // FBX由来ノードのTransform fileIDから、追加物の親引きに使う
    // stripped文書IDへの逆引き表を作る。主インスタンスの文書に限る。
    let mut fbx_tr_to_stripped: HashMap<FileId, Vec<FileId>> = HashMap::new();
    for instance in &parsed.instances {
        if !instance.source_guid.eq_ignore_ascii_case(main_fbx_guid) {
            continue;
        }
        for (local, stripped) in parsed.stripped_tr.iter() {
            if stripped.instance == instance.doc {
                fbx_tr_to_stripped
                    .entry(stripped.source_fid)
                    .or_default()
                    .push(*local);
            }
        }
    }
    let avatar_name = overlays
        .root_name
        .clone()
        .unwrap_or_else(|| avatar_name_from_label(label));
    // 追加物の親別索引。Transform fileID（FBX由来は取り込みfileID、
    // ローカルは文書ID）から追加物の文書IDへの表である。
    // FBX由来の親はstripped文書IDで引くため、ここでは取り込みfileIDで束ねる。
    let mut added_by_tr: HashMap<FileId, Vec<FileId>> = HashMap::new();
    for (tr_fid, local) in parsed.local_tr.iter() {
        let key = if local.father == 0 {
            // prefab直下。取り込み根と同じ扱いにする。
            import_tr
        } else if let Some(mapped) = stripped_tr_to_fbx.get(&local.father) {
            match mapped {
                // 取り込み根直下。取り込み根のfileIDで束ねる。
                None => import_tr,
                // FBX由来ノード直下。取り込みfileIDで束ねる。
                Some(_) => {
                    // stripped文書IDではなく取り込みfileIDで束ね、
                    // FBX由来ノードの処理時にまとめて取り出す。
                    // 対応する取り込みfileIDを求める。
                    let fbx_index = match stripped_tr_to_fbx.get(&local.father) {
                        Some(Some(index)) => *index,
                        _ => {
                            return Err(VrmError::new(
                                VrmErrorKind::InvalidPrefab,
                                format!(
                                    "追加物の親を解決できない（文書{tr_fid}）（{}）",
                                    crate::escape_diag(label),
                                ),
                            ));
                        }
                    };
                    hier.tr_fid_at[fbx_index]
                }
            }
        } else if parsed.local_tr.contains_key(&local.father) {
            // ローカル由来ノード直下。親の文書IDで束ねる。
            local.father
        } else {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!(
                    "追加物の親が無い（文書{tr_fid}）（{}）",
                    crate::escape_diag(label),
                ),
            ));
        };
        added_by_tr.entry(key).or_default().push(*tr_fid);
    }
    // 追加物の順序を確定する。FBX由来の親では `m_RootOrder` 順、
    // ローカルの親では `m_Children` 順（親文書の記載順）に並べる。
    // `m_RootOrder` が無い追加物は明示的に失敗させる。
    for (parent, list) in added_by_tr.iter_mut() {
        if parsed.local_tr.contains_key(parent) {
            let father = parsed.local_tr.get(parent).expect("存在は確かめた");
            let mut ordered = Vec::with_capacity(list.len());
            for child in &father.children {
                if list.contains(child) {
                    ordered.push(*child);
                }
            }
            if ordered.len() != list.len() {
                return Err(VrmError::new(
                    VrmErrorKind::InvalidPrefab,
                    format!(
                        "追加物の子順が親と一致しない（文書{parent}）（{}）",
                        crate::escape_diag(label),
                    ),
                ));
            }
            *list = ordered;
        } else {
            let mut with_order: Vec<(i64, FileId)> = Vec::with_capacity(list.len());
            for tr_fid in list.iter() {
                let local = parsed.local_tr.get(tr_fid).expect("存在は確かめた");
                with_order.push((local.root_order, *tr_fid));
            }
            with_order.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
            *list = with_order.into_iter().map(|(_, fid)| fid).collect();
        }
    }
    let mut nodes: Vec<TreeNode> = Vec::new();
    // 出所と有効状態の組み立てヘルパ。
    let fbx_go_fid =
        |fbx_index: usize| -> FileId { crate::unity::game_object_file_id(&hier.paths[fbx_index]) };
    // 反復DFS。子を逆順で積む。親は子より先に確定する。
    enum GeneralFrame {
        Fbx(usize),
        Added(FileId),
    }
    struct GeneralBuildFrame {
        kind: GeneralFrame,
        parent: Option<usize>,
        scene_path: Vec<String>,
    }
    // ルート列。FBXルート（名前順）の後にローカルルート（`m_RootOrder` 順）を足す。
    let mut fbx_roots: Vec<usize> = Vec::new();
    for (index, model) in models.iter().enumerate() {
        if model.parent.is_none() {
            fbx_roots.push(index);
        }
    }
    fbx_roots.sort_by(|a, b| {
        models[*a]
            .name
            .to_ascii_lowercase()
            .cmp(&models[*b].name.to_ascii_lowercase())
            .then(models[*a].name.cmp(&models[*b].name))
    });
    let local_roots = added_by_tr.get(&import_tr).cloned().unwrap_or_default();
    let mut stack: Vec<GeneralBuildFrame> = Vec::new();
    // ルート列はFBXルート（名前順）の後にローカルルート（`m_RootOrder` 順）を足す。
    // スタックに積むため逆順で足し、ローカルルートを先に積む。
    for tr_fid in local_roots.iter().rev() {
        let local = parsed.local_tr.get(tr_fid).expect("存在は確かめた");
        let go = parsed.local_go.get(&local.go).ok_or_else(|| {
            VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!(
                    "追加物のゲームオブジェクトがない（{}）",
                    crate::escape_diag(label)
                ),
            )
        })?;
        let name = overlays
            .names
            .get(&local.go)
            .cloned()
            .unwrap_or_else(|| go.name.clone());
        let scene_path = vec![avatar_name.clone(), name];
        stack.push(GeneralBuildFrame {
            kind: GeneralFrame::Added(*tr_fid),
            parent: None,
            scene_path,
        });
    }
    for fbx_index in fbx_roots.iter().rev() {
        let mut scene_path = vec![avatar_name.clone(), models[*fbx_index].name.clone()];
        if let Some(rename) = overlays.names.get(&fbx_go_fid(*fbx_index)) {
            scene_path[1] = rename.clone();
        }
        stack.push(GeneralBuildFrame {
            kind: GeneralFrame::Fbx(*fbx_index),
            parent: None,
            scene_path,
        });
    }
    while let Some(frame) = stack.pop() {
        match frame.kind {
            GeneralFrame::Fbx(fbx_index) => {
                let node_index = nodes.len();
                let model = &models[fbx_index];
                let go_fid = fbx_go_fid(fbx_index);
                let source = fbx_source_of(hier, fbx_index, model_id_main, main_fbx_guid, label)?;
                let name = overlays
                    .names
                    .get(&go_fid)
                    .cloned()
                    .unwrap_or_else(|| model.name.clone());
                let active = import_active && overlays.active.get(&go_fid).copied().unwrap_or(true);
                nodes.push(TreeNode {
                    name,
                    parent: frame.parent,
                    children: Vec::new(),
                    scene_path: frame.scene_path.clone(),
                    source: Some(source),
                    added_trs: None,
                    fbx_index: Some(fbx_index),
                    phone_model: None,
                    active_self: active,
                    local_go: None,
                    local_tr: None,
                });
                if let Some(parent) = frame.parent {
                    nodes[parent].children.push(node_index);
                }
                // 子の収集：FBX子（名前順）＋追加物（`m_RootOrder` 順）。
                let mut child_frames: Vec<GeneralBuildFrame> = Vec::new();
                for child_fbx in hier.children_sorted[fbx_index].iter() {
                    let mut child_path = frame.scene_path.clone();
                    let child_go = fbx_go_fid(*child_fbx);
                    let child_name = overlays
                        .names
                        .get(&child_go)
                        .cloned()
                        .unwrap_or_else(|| models[*child_fbx].name.clone());
                    child_path.push(child_name);
                    child_frames.push(GeneralBuildFrame {
                        kind: GeneralFrame::Fbx(*child_fbx),
                        parent: Some(node_index),
                        scene_path: child_path,
                    });
                }
                let tr_fid = hier.tr_fid_at[fbx_index];
                if let Some(added) = added_by_tr.get(&tr_fid) {
                    for object in added {
                        let local = parsed.local_tr.get(object).expect("存在は確かめた");
                        let go = parsed.local_go.get(&local.go).ok_or_else(|| {
                            VrmError::new(
                                VrmErrorKind::InvalidPrefab,
                                format!(
                                    "追加物のゲームオブジェクトがない（{}）",
                                    crate::escape_diag(label)
                                ),
                            )
                        })?;
                        let name = overlays
                            .names
                            .get(&local.go)
                            .cloned()
                            .unwrap_or_else(|| go.name.clone());
                        let mut child_path = frame.scene_path.clone();
                        child_path.push(name);
                        child_frames.push(GeneralBuildFrame {
                            kind: GeneralFrame::Added(*object),
                            parent: Some(node_index),
                            scene_path: child_path,
                        });
                    }
                }
                for child in child_frames.into_iter().rev() {
                    stack.push(child);
                }
            }
            GeneralFrame::Added(tr_fid) => {
                let node_index = nodes.len();
                let local = parsed.local_tr.get(&tr_fid).ok_or_else(|| {
                    VrmError::new(
                        VrmErrorKind::InvalidPrefab,
                        format!("追加物のTransformがない（{}）", crate::escape_diag(label)),
                    )
                })?;
                let go = parsed.local_go.get(&local.go).ok_or_else(|| {
                    VrmError::new(
                        VrmErrorKind::InvalidPrefab,
                        format!(
                            "追加物のゲームオブジェクトがない（{}）",
                            crate::escape_diag(label)
                        ),
                    )
                })?;
                let name = overlays
                    .names
                    .get(&local.go)
                    .cloned()
                    .unwrap_or_else(|| go.name.clone());
                let active = import_active
                    && go.active
                    && overlays.active.get(&local.go).copied().unwrap_or(true);
                nodes.push(TreeNode {
                    name,
                    parent: frame.parent,
                    children: Vec::new(),
                    scene_path: frame.scene_path.clone(),
                    source: None,
                    added_trs: Some((local.position, local.rotation, local.scale)),
                    fbx_index: None,
                    phone_model: None,
                    active_self: active,
                    local_go: Some(local.go),
                    local_tr: Some(tr_fid),
                });
                if let Some(parent) = frame.parent {
                    nodes[parent].children.push(node_index);
                }
                // 追加物の子（`m_Children` 順）。
                if let Some(added) = added_by_tr.get(&tr_fid) {
                    for object in added.iter().rev() {
                        let child_local = parsed.local_tr.get(object).ok_or_else(|| {
                            VrmError::new(
                                VrmErrorKind::InvalidPrefab,
                                format!(
                                    "追加物の子Transformがない（{}）",
                                    crate::escape_diag(label)
                                ),
                            )
                        })?;
                        let child_go = parsed.local_go.get(&child_local.go).ok_or_else(|| {
                            VrmError::new(
                                VrmErrorKind::InvalidPrefab,
                                format!("追加物の子GOがない（{}）", crate::escape_diag(label)),
                            )
                        })?;
                        let child_name = overlays
                            .names
                            .get(&child_local.go)
                            .cloned()
                            .unwrap_or_else(|| child_go.name.clone());
                        let mut child_path = frame.scene_path.clone();
                        child_path.push(child_name);
                        stack.push(GeneralBuildFrame {
                            kind: GeneralFrame::Added(*object),
                            parent: Some(node_index),
                            scene_path: child_path,
                        });
                    }
                }
            }
        }
    }
    // 未配置のローカル文書があれば、親の解決漏れとして失敗させる。
    {
        let mut placed: HashSet<FileId> = HashSet::new();
        for node in &nodes {
            if let Some(tr) = node.local_tr {
                placed.insert(tr);
            }
        }
        for tr_fid in parsed.local_tr.keys() {
            if !placed.contains(tr_fid) {
                return Err(VrmError::new(
                    VrmErrorKind::InvalidPrefab,
                    format!(
                        "配置できない追加物がある（文書{tr_fid}）（{}）",
                        crate::escape_diag(label),
                    ),
                ));
            }
        }
    }
    // COMコンポーネント等の無関係なローカルGOは、参照解決時に無視する。
    // ここでは配置の有無だけを確かめる。
    Ok(nodes)
}
#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::too_many_arguments)]
fn build_milfy_tree(
    fbx: &FbxFile<'_>,
    hier: &FbxHierarchy,
    parsed: &ParsedPrefab,
    phone: &Option<PhoneSubtree>,
    main_fbx_guid: &str,
    model_id_main: &str,
    label: &str,
) -> Result<Vec<TreeNode>, VrmError> {
    let models = fbx.models();
    // 追加物の親別索引。
    let mut added_by_parent: HashMap<FileId, Vec<FileId>> = HashMap::new();
    for instance in &parsed.instances {
        for added in &instance.added_objects {
            added_by_parent
                .entry(added.target_fid)
                .or_default()
                .push(added.object);
        }
    }
    let avatar_name = avatar_name_from_label(label);
    let mut nodes: Vec<TreeNode> = Vec::new();
    // Armatureを探す。
    let armature_idx = models
        .iter()
        .position(|m| m.name == "Armature" && m.parent.is_none())
        .ok_or_else(|| {
            VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!("Armatureが見つからない（{}）", crate::escape_diag(label)),
            )
        })?;
    // メッシュルート（Armature以外）を名前順で集める。
    let mut mesh_roots: Vec<usize> = Vec::new();
    for (i, m) in models.iter().enumerate() {
        if m.parent.is_none() && i != armature_idx {
            mesh_roots.push(i);
        }
    }
    // メッシュルート（Armature以外）をFBX順ではなくUnityの取り込み順（大文字小文字を区別しない名前順）で出す。
    // 実測で `Bandage_*` が `BandAid_*` より先に来るため、バイト順では合わない。
    mesh_roots.sort_by(|a, b| {
        models[*a]
            .name
            .to_ascii_lowercase()
            .cmp(&models[*b].name.to_ascii_lowercase())
            .then(models[*a].name.cmp(&models[*b].name))
    });
    // 追加物（Milfyルート直下のローカル）。stripped等の非ローカルは副階層のはずであり、
    // graft 対象でなければ明示的に失敗する。
    let root_tr = transform_file_id("");
    let mut root_added: Vec<FileId> = Vec::new();
    if let Some(list) = added_by_parent.get(&root_tr) {
        for object in list {
            if parsed.local_tr.contains_key(object) {
                root_added.push(*object);
            } else {
                return Err(VrmError::new(
                    VrmErrorKind::InvalidPrefab,
                    format!(
                        "ルート直下の非ローカル追加物は未対応である（文書{object}）（{}）",
                        crate::escape_diag(label),
                    ),
                ));
            }
        }
    }
    // 出所の組み立てヘルパ。
    let fbx_guid = main_fbx_guid;
    // Armature部分木のDFS。子はFBX名前順、追加物は後に付け、副階層は最後に graft する。
    // Head配下の追加物（Head_Contact等）もここで付ける。
    let mut stack: Vec<BuildFrame> = vec![BuildFrame {
        kind: FrameKind::Fbx(armature_idx),
        parent: None,
        scene_path: vec![avatar_name.clone(), models[armature_idx].name.clone()],
    }];
    // 反復DFS。子を逆順で積む。副階層の graft は全体で1回に限る。
    let mut grafted = false;
    while let Some(frame) = stack.pop() {
        match frame.kind {
            FrameKind::Fbx(fbx_index) => {
                let node_index = nodes.len();
                let model = &models[fbx_index];
                let source = fbx_source_of(hier, fbx_index, model_id_main, fbx_guid, label)?;
                nodes.push(TreeNode {
                    name: model.name.clone(),
                    parent: frame.parent,
                    children: Vec::new(),
                    scene_path: frame.scene_path.clone(),
                    source: Some(source),
                    added_trs: None,
                    fbx_index: Some(fbx_index),
                    phone_model: None,
                    active_self: true,
                    local_go: None,
                    local_tr: None,
                });
                if let Some(parent) = frame.parent {
                    nodes[parent].children.push(node_index);
                }
                // 子の収集：FBX子（名前順）＋追加物（登録順）＋副階層（最後）。
                let mut child_frames: Vec<BuildFrame> = Vec::new();
                for child_fbx in hier.children_sorted[fbx_index].iter() {
                    let mut child_path = frame.scene_path.clone();
                    child_path.push(models[*child_fbx].name.clone());
                    child_frames.push(BuildFrame {
                        kind: FrameKind::Fbx(*child_fbx),
                        parent: Some(node_index),
                        scene_path: child_path,
                    });
                }
                // 追加物（このFBXノードのTransform fileIDを親とする）。
                // 非ローカルは副階層の graft 対象であり、該当は1件に限る。
                let mut graft_here = false;
                let tr_fid = hier.tr_fid_at[fbx_index];
                if let Some(added) = added_by_parent.get(&tr_fid) {
                    for object in added {
                        if parsed.local_tr.contains_key(object) {
                            let local = parsed.local_tr.get(object).expect("存在は確かめた");
                            let go = parsed.local_go.get(&local.go).ok_or_else(|| {
                                VrmError::new(
                                    VrmErrorKind::InvalidPrefab,
                                    format!(
                                        "追加物のゲームオブジェクトがない（{}）",
                                        crate::escape_diag(label)
                                    ),
                                )
                            })?;
                            let mut child_path = frame.scene_path.clone();
                            child_path.push(go.name.clone());
                            child_frames.push(BuildFrame {
                                kind: FrameKind::Added(*object),
                                parent: Some(node_index),
                                scene_path: child_path,
                            });
                        } else if is_graft_object(parsed, phone, *object) {
                            // 副階層の graft 対象。該当は1件に限り、子列の最後に付ける。
                            // Unityはインスタンス根を既存子の後に足すため、ここで順序を確定する。
                            if graft_here {
                                return Err(VrmError::new(
                                    VrmErrorKind::InvalidPrefab,
                                    format!(
                                        "副階層の graft 対象が複数ある（{}）",
                                        crate::escape_diag(label),
                                    ),
                                ));
                            }
                            graft_here = true;
                        } else {
                            return Err(VrmError::new(
                                VrmErrorKind::InvalidPrefab,
                                format!(
                                    "非ローカルの追加物は副階層でなければ未対応である（文書{object}）（{}）",
                                    crate::escape_diag(label),
                                ),
                            ));
                        }
                    }
                }
                if graft_here {
                    child_frames.push(BuildFrame {
                        kind: FrameKind::Graft { parent: node_index },
                        parent: Some(node_index),
                        scene_path: frame.scene_path.clone(),
                    });
                }
                // 逆順で積む。
                for child in child_frames.into_iter().rev() {
                    stack.push(child);
                }
            }
            FrameKind::Added(object) => {
                let node_index = nodes.len();
                let local = parsed.local_tr.get(&object).ok_or_else(|| {
                    VrmError::new(
                        VrmErrorKind::InvalidPrefab,
                        format!("追加物のTransformがない（{}）", crate::escape_diag(label)),
                    )
                })?;
                let go = parsed.local_go.get(&local.go).ok_or_else(|| {
                    VrmError::new(
                        VrmErrorKind::InvalidPrefab,
                        format!(
                            "追加物のゲームオブジェクトがない（{}）",
                            crate::escape_diag(label)
                        ),
                    )
                })?;
                nodes.push(TreeNode {
                    name: go.name.clone(),
                    parent: frame.parent,
                    children: Vec::new(),
                    scene_path: frame.scene_path.clone(),
                    source: None,
                    added_trs: Some((local.position, local.rotation, local.scale)),
                    fbx_index: None,
                    phone_model: None,
                    active_self: true,
                    local_go: Some(local.go),
                    local_tr: Some(object),
                });
                if let Some(parent) = frame.parent {
                    nodes[parent].children.push(node_index);
                }
                // 追加物の子（m_Children順）。副階層の子は graft 側で扱う。
                for child_tr in local.children.iter().rev() {
                    let child_local = parsed.local_tr.get(child_tr).ok_or_else(|| {
                        VrmError::new(
                            VrmErrorKind::InvalidPrefab,
                            format!("追加物の子Transformがない（{}）", crate::escape_diag(label)),
                        )
                    })?;
                    let child_go = parsed.local_go.get(&child_local.go).ok_or_else(|| {
                        VrmError::new(
                            VrmErrorKind::InvalidPrefab,
                            format!("追加物の子GOがない（{}）", crate::escape_diag(label)),
                        )
                    })?;
                    let mut child_path = frame.scene_path.clone();
                    child_path.push(child_go.name.clone());
                    stack.push(BuildFrame {
                        kind: FrameKind::Added(*child_tr),
                        parent: Some(node_index),
                        scene_path: child_path,
                    });
                }
            }
            FrameKind::Graft { parent } => {
                if grafted {
                    return Err(VrmError::new(
                        VrmErrorKind::InvalidPrefab,
                        format!(
                            "副階層の graft が複数あるため未対応である（{}）",
                            crate::escape_diag(label)
                        ),
                    ));
                }
                grafted = true;
                let phone = phone.as_ref().ok_or_else(|| {
                    VrmError::new(
                        VrmErrorKind::InvalidPrefab,
                        format!(
                            "副階層なしに graft できない（{}）",
                            crate::escape_diag(label)
                        ),
                    )
                })?;
                build_graft_subtree(&mut nodes, parent, parsed, phone, label)?;
            }
        }
    }
    // メッシュルート（26件）を名前順で出す。子を持たないはずである。
    for fbx_index in mesh_roots {
        let model = &models[fbx_index];
        if !hier.children_sorted[fbx_index].is_empty() {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!(
                    "メッシュルートに子があるため未対応である: {}（{}）",
                    crate::escape_diag(&model.name),
                    crate::escape_diag(label),
                ),
            ));
        }
        let node_index = nodes.len();
        let source = fbx_source_of(hier, fbx_index, model_id_main, fbx_guid, label)?;
        let scene_path = vec![avatar_name.clone(), model.name.clone()];
        nodes.push(TreeNode {
            name: model.name.clone(),
            parent: None,
            children: Vec::new(),
            scene_path,
            source: Some(source),
            added_trs: None,
            fbx_index: Some(fbx_index),
            phone_model: None,
            active_self: true,
            local_go: None,
            local_tr: None,
        });
        let _ = node_index;
    }
    // ルート追加物（3件）を登録順で出す。
    for object in root_added {
        let local = parsed.local_tr.get(&object).expect("存在は確かめた");
        let go = parsed.local_go.get(&local.go).expect("存在は確かめた");
        let node_index = nodes.len();
        let scene_path = vec![avatar_name.clone(), go.name.clone()];
        nodes.push(TreeNode {
            name: go.name.clone(),
            parent: None,
            children: Vec::new(),
            scene_path,
            source: None,
            added_trs: Some((local.position, local.rotation, local.scale)),
            fbx_index: None,
            phone_model: None,
            active_self: true,
            local_go: Some(local.go),
            local_tr: Some(object),
        });
        let _ = node_index;
    }
    Ok(nodes)
}

/// DFS展開フレームの種別。副階層の graft は親が確定してから行う。
#[cfg(not(target_arch = "wasm32"))]
enum FrameKind {
    Fbx(usize),
    Added(FileId),
    Graft { parent: usize },
}

#[cfg(not(target_arch = "wasm32"))]
struct BuildFrame {
    kind: FrameKind,
    parent: Option<usize>,
    scene_path: Vec<String>,
}

/// 追加物が副階層の graft 起点かどうかを判定する。strippedで副Prefab由来であり、
/// 副Prefab根のTransformに対応するものだけを graft 対象にする。
#[cfg(not(target_arch = "wasm32"))]
fn is_graft_object(parsed: &ParsedPrefab, phone: &Option<PhoneSubtree>, object: FileId) -> bool {
    let Some(phone) = phone.as_ref() else {
        return false;
    };
    if parsed.local_tr.contains_key(&object) {
        return false;
    }
    let Some(stripped) = parsed.stripped_tr.get(&object) else {
        return false;
    };
    stripped.source_guid.eq_ignore_ascii_case(&phone.guid)
}

/// 資産パス（`Assets/` 付き）からモデルIDを求める。stemの小文字である。
/// 主FBXの既存規則（`.fbx` のみ剥がす）と同一にする。
#[cfg(not(target_arch = "wasm32"))]
fn model_id_from_asset_path(asset_path: &str) -> String {
    asset_path
        .rsplit('/')
        .next()
        .unwrap_or("model")
        .strip_suffix(".fbx")
        .or_else(|| asset_path.rsplit('/').next())
        .unwrap_or("model")
        .to_ascii_lowercase()
}

/// 副Prefab根の基準点。graft根の改変対象と追加物の親先の特定に使う。
#[cfg(not(target_arch = "wasm32"))]
struct PhoneAnchors {
    /// 副Prefab内の取り込み根Transform（KumaPhone側のstripped文書）。
    kuma_root_tr: FileId,
    /// 副Prefab内の取り込み根GameObject（KumaPhone側のstripped文書）。
    kuma_root_go: FileId,
}

/// 副Prefab根の基準点を求める。取り込み根に対応するstrippedが1件ずつあるはずであり、
/// それ以外（複数・欠落）は明示的に失敗する。
#[cfg(not(target_arch = "wasm32"))]
fn phone_anchors(phone: &PhoneSubtree, label: &str) -> Result<PhoneAnchors, VrmError> {
    let import_tr = transform_file_id("");
    let import_go = crate::unity::game_object_file_id("");
    let mut root_tr: Vec<FileId> = Vec::new();
    let mut root_go: Vec<FileId> = Vec::new();
    for (local, stripped) in phone.parsed.stripped_tr.iter() {
        if stripped.source_fid == import_tr
            && stripped.source_guid.eq_ignore_ascii_case(&phone.fbx_guid)
        {
            root_tr.push(*local);
        }
    }
    for (local, stripped) in phone.parsed.stripped_go.iter() {
        if stripped.source_fid == import_go
            && stripped.source_guid.eq_ignore_ascii_case(&phone.fbx_guid)
        {
            root_go.push(*local);
        }
    }
    if root_tr.len() != 1 || root_go.len() != 1 {
        return Err(VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!(
                "副Prefab根の対応が{}件・{}件あるため未対応である（{}）",
                root_tr.len(),
                root_go.len(),
                crate::escape_diag(label),
            ),
        ));
    }
    Ok(PhoneAnchors {
        kuma_root_tr: root_tr[0],
        kuma_root_go: root_go[0],
    })
}

/// Milfy側の副Prefabインスタンスを1件に定める。
#[cfg(not(target_arch = "wasm32"))]
fn find_kuma_instance<'p>(
    parsed: &'p ParsedPrefab,
    phone: &PhoneSubtree,
    label: &str,
) -> Result<&'p ParsedInstance, VrmError> {
    let mut found: Vec<&ParsedInstance> = parsed
        .instances
        .iter()
        .filter(|i| i.source_guid.eq_ignore_ascii_case(&phone.guid))
        .collect();
    if found.len() != 1 {
        return Err(VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!(
                "副Prefabのインスタンスが{}件あるため未対応である（{}）",
                found.len(),
                crate::escape_diag(label),
            ),
        ));
    }
    Ok(found.pop().expect("件数は確かめた"))
}

/// Milfy側の graft 起点を1件に定める。副Prefab根に対応するstrippedである。
#[cfg(not(target_arch = "wasm32"))]
fn find_graft_object(
    parsed: &ParsedPrefab,
    phone: &PhoneSubtree,
    kuma_doc: FileId,
    anchors: &PhoneAnchors,
    label: &str,
) -> Result<FileId, VrmError> {
    let mut found: Vec<FileId> = Vec::new();
    for (local, stripped) in parsed.stripped_tr.iter() {
        if stripped.source_guid.eq_ignore_ascii_case(&phone.guid) {
            found.push(*local);
        }
    }
    if found.len() != 1 {
        return Err(VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!(
                "副階層の graft 起点が{}件あるため未対応である（{}）",
                found.len(),
                crate::escape_diag(label),
            ),
        ));
    }
    let object = found[0];
    let stripped = parsed.stripped_tr.get(&object).expect("存在は確かめた");
    if stripped.source_fid != anchors.kuma_root_tr || stripped.instance != kuma_doc {
        return Err(VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!(
                "副階層の graft 起点が副Prefab根に届かない（{}）",
                crate::escape_diag(label),
            ),
        ));
    }
    Ok(object)
}

/// 副階層全体を graft する。親ノードの下に graft 根・副FBX部・副Prefab追加物を足す。
/// IDは追加順（DFS順）に `n:N` で決まり、呼び出し側の順序が正解と一致する。
#[cfg(not(target_arch = "wasm32"))]
fn build_graft_subtree(
    nodes: &mut Vec<TreeNode>,
    parent: usize,
    parsed: &ParsedPrefab,
    phone: &PhoneSubtree,
    label: &str,
) -> Result<(), VrmError> {
    let kuma = find_kuma_instance(parsed, phone, label)?;
    // 親の対応を確かめる。副Prefabインスタンスの親は親ノードの出所を指す。
    let parent_source = nodes[parent].source.as_ref().ok_or_else(|| {
        VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!("副階層の親に出所がない（{}）", crate::escape_diag(label)),
        )
    })?;
    let parent_stripped = parsed.stripped_tr.get(&kuma.parent).ok_or_else(|| {
        VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!(
                "副Prefabインスタンスの親を解決できない（{}）",
                crate::escape_diag(label)
            ),
        )
    })?;
    if parent_stripped.source_fid != parent_source.local_id {
        return Err(VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!(
                "副Prefabインスタンスの親が graft 先と一致しない（{}）",
                crate::escape_diag(label),
            ),
        ));
    }
    let anchors = phone_anchors(phone, label)?;
    let _ = find_graft_object(parsed, phone, kuma.doc, &anchors, label)?;
    let (root_name, root_trs, root_active) = graft_root_state(phone, kuma, &anchors, label)?;
    let model_id_phone = model_id_from_asset_path(&phone.fbx_asset_path);
    // graft根。取り込み根の例外として `path` は自身名だけを持つ。
    let mut scene_path = nodes[parent].scene_path.clone();
    scene_path.push(root_name.clone());
    let root_index = nodes.len();
    nodes.push(TreeNode {
        name: root_name.clone(),
        parent: Some(parent),
        children: Vec::new(),
        scene_path,
        source: Some(TreeSource {
            model_id: model_id_phone.clone(),
            path: vec![root_name.clone()],
            asset_guid: phone.fbx_guid.clone(),
            local_id: transform_file_id(""),
        }),
        added_trs: Some(root_trs),
        fbx_index: None,
        phone_model: None,
        active_self: root_active,
        local_go: None,
        local_tr: None,
    });
    nodes[parent].children.push(root_index);
    // 副Prefabの追加物の親別索引。対象は副FBXのTransformであり、
    // 副Prefab文書の登録順を保つ。対象GUIDの検証は組み立て時に行う。
    let phone_instance = phone
        .parsed
        .instances
        .iter()
        .find(|i| i.doc == phone.phone_instance_doc)
        .ok_or_else(|| {
            VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!(
                    "副Prefab内のFBXインスタンスがない（{}）",
                    crate::escape_diag(label)
                ),
            )
        })?;
    let mut phone_added_by_target: HashMap<FileId, Vec<FileId>> = HashMap::new();
    for added in &phone_instance.added_objects {
        if !added.target_guid.eq_ignore_ascii_case(&phone.fbx_guid) {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!(
                    "副Prefabの追加先が副FBXでないため未対応である（{}）",
                    crate::escape_diag(label)
                ),
            ));
        }
        phone_added_by_target
            .entry(added.target_fid)
            .or_default()
            .push(added.object);
    }
    // 副FBXのルート以下をDFSで出す。子順は名前順であり、Unityの取り込み順と一致する。
    // 各節点の追加物（副Prefab由来）はFBX子の後に付ける。主階層と同じ規則である。
    let mut full_paths: Vec<Vec<String>> = vec![Vec::new(); phone.models.len()];
    for (i, model) in phone.models.iter().enumerate() {
        let mut path = match model.parent {
            Some(p) => full_paths[p].clone(),
            None => Vec::new(),
        };
        path.push(model.name.clone());
        full_paths[i] = path;
    }
    let mut stack: Vec<GraftFrame> = Vec::new();
    // graft根直下の追加物（取り込み根対象）を先に積み、FBXルートを後に積む。
    // 後に積んだFBXルートから先に訪ねるため、子順はFBX・追加物の順になる。
    if let Some(added) = phone_added_by_target.get(&transform_file_id("")) {
        for object in added.iter().rev() {
            stack.push(GraftFrame::Added {
                parent: root_index,
                object: *object,
            });
        }
    }
    for root in phone.roots_sorted.iter().rev() {
        let full = vec![phone.models[*root].name.clone()];
        stack.push(GraftFrame::Fbx {
            model: *root,
            parent: root_index,
            full,
        });
    }
    while let Some(frame) = stack.pop() {
        match frame {
            GraftFrame::Fbx {
                model: model_idx,
                parent: tree_parent,
                full,
            } => {
                let mut scene_path = nodes[tree_parent].scene_path.clone();
                scene_path.push(phone.models[model_idx].name.clone());
                let node_index = nodes.len();
                nodes.push(TreeNode {
                    name: phone.models[model_idx].name.clone(),
                    parent: Some(tree_parent),
                    children: Vec::new(),
                    scene_path,
                    source: Some(TreeSource {
                        model_id: model_id_phone.clone(),
                        path: full.clone(),
                        asset_guid: phone.fbx_guid.clone(),
                        local_id: transform_file_id(&full.join("/")),
                    }),
                    added_trs: None,
                    fbx_index: None,
                    phone_model: Some(model_idx),
                    active_self: true,
                    local_go: None,
                    local_tr: None,
                });
                nodes[tree_parent].children.push(node_index);
                // 子の収集：FBX子（名前順）＋追加物（登録順）。
                let tr_fid = transform_file_id(&full.join("/"));
                if let Some(added) = phone_added_by_target.get(&tr_fid) {
                    for object in added.iter().rev() {
                        stack.push(GraftFrame::Added {
                            parent: node_index,
                            object: *object,
                        });
                    }
                }
                for child in phone.children_sorted[model_idx].iter().rev() {
                    let mut child_full = full.clone();
                    child_full.push(phone.models[*child].name.clone());
                    stack.push(GraftFrame::Fbx {
                        model: *child,
                        parent: node_index,
                        full: child_full,
                    });
                }
            }
            GraftFrame::Added { parent, object } => {
                graft_added_node(nodes, parent, object, phone, &mut stack, label)?;
            }
        }
    }
    Ok(())
}

/// graftのDFSフレーム。副FBX節点と副Prefab追加物を統一的に辿る。
#[cfg(not(target_arch = "wasm32"))]
enum GraftFrame {
    Fbx {
        model: usize,
        parent: usize,
        /// 副FBXルートからの名前列。出所とfileIDに使う。
        full: Vec<String>,
    },
    Added {
        parent: usize,
        /// 副Prefab内のローカルTransform文書。
        object: FileId,
    },
}

/// 副Prefabの追加物1件をツリーに出し、子（`m_Children` 順）を積む。
#[cfg(not(target_arch = "wasm32"))]
fn graft_added_node(
    nodes: &mut Vec<TreeNode>,
    parent: usize,
    object: FileId,
    phone: &PhoneSubtree,
    stack: &mut Vec<GraftFrame>,
    label: &str,
) -> Result<(), VrmError> {
    let local = phone.parsed.local_tr.get(&object).ok_or_else(|| {
        VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!(
                "副Prefabの追加物のTransformがない（文書{object}）（{}）",
                crate::escape_diag(label)
            ),
        )
    })?;
    let go = phone.parsed.local_go.get(&local.go).ok_or_else(|| {
        VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!(
                "副Prefabの追加物のゲームオブジェクトがない（文書{object}）（{}）",
                crate::escape_diag(label)
            ),
        )
    })?;
    let mut scene_path = nodes[parent].scene_path.clone();
    scene_path.push(go.name.clone());
    let node_index = nodes.len();
    nodes.push(TreeNode {
        name: go.name.clone(),
        parent: Some(parent),
        children: Vec::new(),
        scene_path,
        source: None,
        added_trs: Some((local.position, local.rotation, local.scale)),
        fbx_index: None,
        phone_model: None,
        active_self: go.active,
        local_go: Some(local.go),
        local_tr: None,
    });
    nodes[parent].children.push(node_index);
    for child_tr in local.children.iter().rev() {
        stack.push(GraftFrame::Added {
            parent: node_index,
            object: *child_tr,
        });
    }
    Ok(())
}

/// graft根の名前・ローカルTRS・有効状態を層化して求める。
/// 基準値（副Prefab名・単位TRS・有効）の上に副Prefab内改変・Milfy側改変を重ねる。
/// 物理に効く欄の想定外の改変は明示的に失敗し、描画系は無視する。
#[cfg(not(target_arch = "wasm32"))]
fn graft_root_state(
    phone: &PhoneSubtree,
    kuma: &ParsedInstance,
    anchors: &PhoneAnchors,
    label: &str,
) -> Result<(String, TrsF64, bool), VrmError> {
    let import_tr = transform_file_id("");
    let import_go = crate::unity::game_object_file_id("");
    let base_name = phone
        .asset_path
        .rsplit('/')
        .next()
        .unwrap_or("phone")
        .strip_suffix(".prefab")
        .or_else(|| phone.asset_path.rsplit('/').next())
        .unwrap_or("phone")
        .to_owned();
    let mut name = base_name;
    let mut pos = [0.0; 3];
    let mut rot = [0.0, 0.0, 0.0, 1.0];
    let mut scale = [1.0; 3];
    let mut active = true;
    // 副Prefab内の改変（取り込み根のTRS初期化・改名）。
    let phone_instance = phone
        .parsed
        .instances
        .iter()
        .find(|i| i.doc == phone.phone_instance_doc)
        .ok_or_else(|| {
            VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!(
                    "副Prefab内のFBXインスタンスがない（{}）",
                    crate::escape_diag(label)
                ),
            )
        })?;
    for m in &phone_instance.modifications {
        let guid_match = m
            .target_guid
            .as_ref()
            .map(|g| g.eq_ignore_ascii_case(&phone.fbx_guid))
            .unwrap_or(false);
        if !guid_match {
            continue;
        }
        if m.target_fid == import_tr {
            apply_root_trs_mod(&mut pos, &mut rot, &mut scale, m, label)?;
        } else if m.target_fid == import_go {
            if m.path == "m_Name" {
                name = mod_string_value(m, label)?;
            } else if m.path == "m_IsActive" {
                active = mod_bool_value(m, label)?;
            } else if m.path.starts_with("m_LocalEulerAnglesHint") {
                // 見かけの欄であり、姿勢に効かない。
            } else {
                return Err(unexpected_mod(label, m, "副Prefab内の取り込み根"));
            }
        } else if is_physics_path(&m.path) {
            // 取り込み根以外の物理に効く改変は未対応として失敗する。
            return Err(VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!(
                    "副Prefab内の想定外の改変である: {}（{}）",
                    crate::escape_diag(&m.path),
                    crate::escape_diag(label),
                ),
            ));
        }
        // 描画系（`m_RootBone`・`m_AABB`・`m_Materials` 等）は物理に効かないため無視する。
    }
    // Milfy側の改変（Hand.L相当への配置・改名・無効化）。
    for m in &kuma.modifications {
        let guid_match = m
            .target_guid
            .as_ref()
            .map(|g| g.eq_ignore_ascii_case(&phone.guid))
            .unwrap_or(false);
        if !guid_match {
            continue;
        }
        if m.target_fid == anchors.kuma_root_tr {
            apply_root_trs_mod(&mut pos, &mut rot, &mut scale, m, label)?;
        } else if m.target_fid == anchors.kuma_root_go {
            if m.path == "m_Name" {
                name = mod_string_value(m, label)?;
            } else if m.path == "m_IsActive" {
                active = mod_bool_value(m, label)?;
            } else if m.path.starts_with("m_LocalEulerAnglesHint") {
                // 見かけの欄であり、姿勢に効かない。
            } else {
                return Err(unexpected_mod(label, m, "Milfy側の副Prefab根"));
            }
        } else if m.path.starts_with("Sources.") {
            // 制約の参照先であり、成分収集で重ねる。ここでは扱わない。
        } else if is_physics_path(&m.path) {
            // 副Prefab根以外の物理に効く改変は未対応として失敗する。
            return Err(unexpected_mod(label, m, "Milfy側の副Prefab"));
        } else {
            // 描画系は物理に効かないため無視する。
        }
    }
    if name.is_empty() {
        return Err(VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!(
                "副階層の graft 根名が空である（{}）",
                crate::escape_diag(label)
            ),
        ));
    }
    Ok((name, (pos, rot, scale), active))
}

/// 物理に効く改変欄かどうかを判定する。姿勢・名前・有効・参照が該当し、
/// 見かけの欄（`m_LocalEulerAnglesHint`）と描画系は含まない。
/// 想定外の対象への改変は明示的に失敗させるために使う。
#[cfg(not(target_arch = "wasm32"))]
fn is_physics_path(path: &str) -> bool {
    path.starts_with("m_LocalPosition.")
        || path.starts_with("m_LocalRotation.")
        || path.starts_with("m_LocalScale.")
        || path == "m_Name"
        || path == "m_IsActive"
        || path == "m_Enabled"
        || path.starts_with("Sources.")
}

/// graft根のTRS改変1件を重ねる。`m_Local*` 以外は呼び出し側で振り分ける。
#[cfg(not(target_arch = "wasm32"))]
fn apply_root_trs_mod(
    pos: &mut [f64; 3],
    rot: &mut [f64; 4],
    scale: &mut [f64; 3],
    m: &PrefabMod,
    label: &str,
) -> Result<(), VrmError> {
    let axis = |path: &str, prefix: &str| -> Option<usize> {
        path.strip_prefix(prefix).and_then(|s| match s {
            "x" => Some(0),
            "y" => Some(1),
            "z" => Some(2),
            "w" => Some(3),
            _ => None,
        })
    };
    if let Some(i) = axis(&m.path, "m_LocalPosition.") {
        if i < 3 {
            pos[i] = mod_f64_value(m, label)?;
            return Ok(());
        }
    } else if let Some(i) = axis(&m.path, "m_LocalRotation.") {
        rot[i] = mod_f64_value(m, label)?;
        return Ok(());
    } else if let Some(i) = axis(&m.path, "m_LocalScale.") {
        if i < 3 {
            scale[i] = mod_f64_value(m, label)?;
            return Ok(());
        }
    } else if m.path.starts_with("m_LocalEulerAnglesHint") {
        // 見かけの欄であり、姿勢に効かない。
        return Ok(());
    }
    Err(unexpected_mod(label, m, "graft根のTRS"))
}

/// 想定外の改変の失敗を作る。黙って無視しない。
#[cfg(not(target_arch = "wasm32"))]
fn unexpected_mod(label: &str, m: &PrefabMod, what: &str) -> VrmError {
    VrmError::new(
        VrmErrorKind::InvalidPrefab,
        format!(
            "{what}の想定外の改変である: {}（{}）",
            crate::escape_diag(&m.path),
            crate::escape_diag(label),
        ),
    )
}

/// 改変の数値を読む。Prefab値はUnityのfloat32へ量子化する。
#[cfg(not(target_arch = "wasm32"))]
fn mod_f64_value(m: &PrefabMod, label: &str) -> Result<f64, VrmError> {
    let Some(value) = m.value.as_ref() else {
        return Err(VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!(
                "改変に値がない: {}（{}）",
                crate::escape_diag(&m.path),
                crate::escape_diag(label),
            ),
        ));
    };
    read_f64_value(value, &m.path, label)
}

/// 改変の文字列を読む。
#[cfg(not(target_arch = "wasm32"))]
fn mod_string_value(m: &PrefabMod, label: &str) -> Result<String, VrmError> {
    match m.value.as_ref() {
        Some(Value::String(s)) => Ok(s.clone()),
        _ => Err(VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!(
                "改変が文字列でない: {}（{}）",
                crate::escape_diag(&m.path),
                crate::escape_diag(label),
            ),
        )),
    }
}

/// 改変の真偽値を読む。0/1も受け付ける。
#[cfg(not(target_arch = "wasm32"))]
fn mod_bool_value(m: &PrefabMod, label: &str) -> Result<bool, VrmError> {
    let Some(value) = m.value.as_ref() else {
        return Err(VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!(
                "改変に値がない: {}（{}）",
                crate::escape_diag(&m.path),
                crate::escape_diag(label),
            ),
        ));
    };
    read_bool_value(value, &m.path, label)
}

/// Milfy側の副Prefab改変のうち、成分文書を対象にする重ね合わせ表を作る。
/// 制約の参照先（`Sources.*`）だけを扱う。成分文書への他の改変や、
/// 副Prefabのローカル節点への物理に効く改変は未対応として失敗する。
#[cfg(not(target_arch = "wasm32"))]
fn phone_source_overlays(
    parsed: &ParsedPrefab,
    phone: &Option<PhoneSubtree>,
    label: &str,
) -> Result<HashMap<FileId, Vec<PrefabMod>>, VrmError> {
    let mut overlays: HashMap<FileId, Vec<PrefabMod>> = HashMap::new();
    let Some(phone) = phone.as_ref() else {
        return Ok(overlays);
    };
    let kuma = find_kuma_instance(parsed, phone, label)?;
    let anchors = phone_anchors(phone, label)?;
    let mono_docs: HashSet<FileId> = phone.parsed.monos.iter().map(|m| m.doc).collect();
    for m in &kuma.modifications {
        let guid_match = m
            .target_guid
            .as_ref()
            .map(|g| g.eq_ignore_ascii_case(&phone.guid))
            .unwrap_or(false);
        if !guid_match {
            continue;
        }
        // graft根の欄はツリー組み立てで扱う。
        if m.target_fid == anchors.kuma_root_tr || m.target_fid == anchors.kuma_root_go {
            continue;
        }
        if mono_docs.contains(&m.target_fid) {
            // 成分文書への改変は制約の参照先に限る。
            if !m.path.starts_with("Sources.") {
                return Err(VrmError::new(
                    VrmErrorKind::InvalidPrefab,
                    format!(
                        "副Prefabの成分への想定外の改変である: {}（文書{}）（{}）",
                        crate::escape_diag(&m.path),
                        m.target_fid,
                        crate::escape_diag(label),
                    ),
                ));
            }
            overlays.entry(m.target_fid).or_default().push(m.clone());
        } else if is_physics_path(&m.path) {
            // 副Prefabのローカル節点への物理に効く改変は未対応として失敗する。
            return Err(VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!(
                    "副Prefabの節点への想定外の改変である: {}（文書{}）（{}）",
                    crate::escape_diag(&m.path),
                    m.target_fid,
                    crate::escape_diag(label),
                ),
            ));
        }
        // 描画系は物理に効かないため無視する。
    }
    Ok(overlays)
}

/// 制約の参照先の重ね合わせを適用する。`Sources.source{i}.SourceTransform` と
/// `Sources.source{i}.Weight` だけを扱い、他は明示的に失敗する。
/// 対象の入れ物がなければ失敗し、黙って足さない。
#[cfg(not(target_arch = "wasm32"))]
fn overlay_constraint_sources(
    extra: &yaml_serde::Mapping,
    mods: &[PrefabMod],
    label: &str,
) -> Result<yaml_serde::Mapping, VrmError> {
    let mut out = extra.clone();
    let Some(Value::Mapping(sources)) = out.get_mut(Value::String("Sources".to_owned())) else {
        return Err(VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!("制約に参照入れ物がない（{}）", crate::escape_diag(label)),
        ));
    };
    for m in mods {
        let Some(rest) = m.path.strip_prefix("Sources.") else {
            return Err(unexpected_mod(label, m, "制約の参照先"));
        };
        let Some((slot, field)) = rest.split_once('.') else {
            return Err(unexpected_mod(label, m, "制約の参照先"));
        };
        if !slot.starts_with("source") {
            return Err(unexpected_mod(label, m, "制約の参照先"));
        }
        let slot_key = Value::String(slot.to_owned());
        let Some(Value::Mapping(slot_map)) = sources.get_mut(&slot_key) else {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!(
                    "制約の参照入れ物がない: {slot}（{}）",
                    crate::escape_diag(label)
                ),
            ));
        };
        if field == "SourceTransform" {
            let fid = m.obj_ref.as_ref().map(|r| r.fid).unwrap_or(0);
            let mut mapping = yaml_serde::Mapping::new();
            mapping.insert(
                Value::String("fileID".to_owned()),
                Value::Number(yaml_serde::Number::from(fid)),
            );
            slot_map.insert(
                Value::String("SourceTransform".to_owned()),
                Value::Mapping(mapping),
            );
        } else if field == "Weight" {
            let Some(value) = m.value.as_ref() else {
                return Err(VrmError::new(
                    VrmErrorKind::InvalidPrefab,
                    format!(
                        "制約の重みに値がない: {slot}（{}）",
                        crate::escape_diag(label)
                    ),
                ));
            };
            let weight = value
                .as_f64()
                .or_else(|| value.as_i64().map(|n| n as f64))
                .ok_or_else(|| {
                    VrmError::new(
                        VrmErrorKind::InvalidPrefab,
                        format!(
                            "制約の重みが数値でない: {slot}（{}）",
                            crate::escape_diag(label)
                        ),
                    )
                })?;
            slot_map.insert(
                Value::String("Weight".to_owned()),
                Value::Number(yaml_serde::Number::from(weight)),
            );
        } else {
            return Err(unexpected_mod(label, m, "制約の参照先"));
        }
    }
    Ok(out)
}

/// FBXノードの出所を組み立てる。
#[cfg(not(target_arch = "wasm32"))]
fn fbx_source_of(
    hier: &FbxHierarchy,
    fbx_index: usize,
    model_id: &str,
    fbx_guid: &str,
    label: &str,
) -> Result<TreeSource, VrmError> {
    let full = hier.paths[fbx_index].clone();
    let _ = label;
    let path: Vec<String> = if full.is_empty() {
        Vec::new()
    } else {
        full.split('/').map(|s| s.to_owned()).collect()
    };
    Ok(TreeSource {
        model_id: model_id.to_owned(),
        path,
        asset_guid: fbx_guid.to_owned(),
        local_id: hier.tr_fid_at[fbx_index],
    })
}

/// Prefab表示名からアバタールート名を求める。`Milfy.prefab` → `Milfy` である。
#[cfg(not(target_arch = "wasm32"))]
fn avatar_name_from_label(label: &str) -> String {
    // `path/to/Milfy.prefab` または `Milfy.prefab` のstemを使う。
    // 区切りは `/` と `\` の両方に対応する。
    // CLIは表示名を `format!("{path:?}")` で作るため、両端の `"` を落とす。
    let unquoted = label.trim_matches('"');
    let base = unquoted.rsplit(['/', '\\']).next().unwrap_or(unquoted);
    base.strip_suffix(".prefab")
        .or_else(|| base.strip_suffix(".PREFAB"))
        .unwrap_or(base)
        .to_owned()
}

#[cfg(not(target_arch = "wasm32"))]
#[allow(dead_code)] // 所有モデルの索引は検証用に保持し、現状は名前だけを使う。
struct BindSelection {
    /// bind対象のFBXモデル索引集合。参照元メッシュのスキン関節である。
    joint_models: HashSet<usize>,
    /// 参照元メッシュのモデル索引。bindPoseSourceに使う。
    source_mesh_model: usize,
    /// 参照元メッシュ名（`Body` 等）。bindPoseSourceに使う。
    source_mesh_name: String,
}

#[cfg(not(target_arch = "wasm32"))]
fn select_bind(
    fbx: &FbxFile<'_>,
    hier: &FbxHierarchy,
    label: &str,
) -> Result<BindSelection, VrmError> {
    // アバター配下でボーン数が最も多いスキン（メッシュ）を選ぶ。
    // MilfyではBodyである。ボーン数が同点の場合はBodyという名前を優先し、
    // 次に頂点数（corner数）が最も多いものを選ぶ。同点ならモデル順で決定する。
    let models = fbx.models();
    let at: HashMap<i64, usize> = models
        .iter()
        .enumerate()
        .map(|(i, m)| (m.id.value(), i))
        .collect();
    // 所有モデル別にスキンを引く。所有モデルがないスキンは対象外である。
    struct Candidate {
        model: usize,
        bones: usize,
        is_body: bool,
        corners: usize,
    }
    let mut candidates: Vec<Candidate> = Vec::new();
    for skin in fbx.skins() {
        // ジオメトリの所有モデルを探す。
        let mut owner: Option<usize> = None;
        for (i, m) in models.iter().enumerate() {
            if m.mesh == Some(skin.geometry) {
                if owner.is_some() {
                    return Err(VrmError::new(
                        VrmErrorKind::InvalidPrefab,
                        format!(
                            "1つのメッシュに複数の所有モデルがある（{}）",
                            crate::escape_diag(label)
                        ),
                    ));
                }
                owner = Some(i);
            }
        }
        let Some(owner) = owner else {
            continue;
        };
        let bones = skin.clusters.len();
        let is_body = models[owner].name.eq_ignore_ascii_case("Body");
        let corners = fbx
            .mesh(skin.geometry)
            .map(|mesh| mesh.corner_count)
            .unwrap_or(0);
        candidates.push(Candidate {
            model: owner,
            bones,
            is_body,
            corners,
        });
    }
    if candidates.is_empty() {
        return Err(VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!(
                "bind-pose用のスキンがアバター配下にない（{}）",
                crate::escape_diag(label)
            ),
        ));
    }
    candidates.sort_by(|a, b| {
        b.bones
            .cmp(&a.bones)
            .then(b.is_body.cmp(&a.is_body))
            .then(b.corners.cmp(&a.corners))
            .then(a.model.cmp(&b.model))
    });
    let best = &candidates[0];
    // 関節集合を求める。所有モデルのスキンのクラスタ順を保つ。
    // 同一ジオメトリに複数スキンがある場合は先勝ちにせず失敗する。
    let mut joint_models = HashSet::new();
    let mut seen_geom: HashSet<i64> = HashSet::new();
    for skin in fbx.skins() {
        let owner_matches = models
            .iter()
            .enumerate()
            .any(|(i, m)| m.mesh == Some(skin.geometry) && i == best.model);
        if !owner_matches {
            continue;
        }
        if !seen_geom.insert(skin.geometry.value()) {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!(
                    "1つのメッシュに複数のスキンがある（{}）",
                    crate::escape_diag(label)
                ),
            ));
        }
        for cluster in &skin.clusters {
            let Some(&joint) = at.get(&cluster.joint.value()) else {
                return Err(VrmError::new(
                    VrmErrorKind::InvalidPrefab,
                    format!("結合モデルが一覧にない（{}）", crate::escape_diag(label)),
                ));
            };
            joint_models.insert(joint);
        }
    }
    let _ = hier;
    Ok(BindSelection {
        joint_models,
        source_mesh_model: best.model,
        source_mesh_name: models[best.model].name.clone(),
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::too_many_arguments)]
fn compute_reference_locals(
    fbx: &FbxFile<'_>,
    hier: &FbxHierarchy,
    tree: &[TreeNode],
    bind: &BindSelection,
    phone: &Option<PhoneSubtree>,
    overlays: &MainInstanceOverlays,
    label: &str,
) -> Result<Vec<ReferenceLocal>, VrmError> {
    // Unityの `Matrix4x4`（float32）と同順で計算し、sidecarと一致させる。
    // bind対象は親のbind世界またはscene世界の逆行列とbind世界の積を分解する。
    // 非対象はsceneのローカルTRSをそのまま使う（float32に丸める）。
    let models = fbx.models();
    // FBXの単位をメートルに直す係数。cmのモデルでは0.01になる。
    // Prefab直置きの追加物はUnity空間（メートル）のため換算しない。
    let factor = fbx.unit_scale_factor();
    if !factor.is_finite() || factor <= 0.0 {
        return Err(VrmError::new(
            VrmErrorKind::UnsupportedUnitScale,
            "単位換算係数が正の有限値でない",
        ));
    }
    let unit_to_meter = (factor / 100.0) as f32;
    // bind世界行列（FBX BindPose）をUnity空間へ写してfloat32で持つ。
    // Unityは `meshWorld * bindpose.inverse` をbind世界にするため、
    // 実データ（Milfy）ではFBXのbind行列の鏡像と一致する。
    let bind_nodes = fbx.bind_pose_nodes().map_err(|e| {
        VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!("バインド行列を読めない: {e}"),
        )
    })?;
    let mut bind_world_f32: HashMap<usize, [f32; 16]> = HashMap::new();
    let at: HashMap<i64, usize> = models
        .iter()
        .enumerate()
        .map(|(i, m)| (m.id.value(), i))
        .collect();
    for node in bind_nodes {
        if let Some(&idx) = at.get(&node.model.value()) {
            let mut m = [0f32; 16];
            for (i, v) in node.matrix.iter().enumerate() {
                m[i] = *v as f32;
            }
            let mut world = mirror_matrix_f32(&m);
            world[12] *= unit_to_meter;
            world[13] *= unit_to_meter;
            world[14] *= unit_to_meter;
            bind_world_f32.insert(idx, world);
        }
    }
    // sceneローカルTRS（float32・Unity空間・メートル）。
    // FBX由来はlocal_matrixを分解してから写し、副FBX由来も同じ手順でscene値を求める。
    // FBXの単位（cm等）はここでメートルに直す。
    // prefab由来の追加物はUnity空間（メートル）のためそのまま使う。混合しない。
    let mut scene_trs: Vec<([f32; 3], [f32; 4], [f32; 3])> = Vec::with_capacity(tree.len());
    for node in tree {
        if let Some(fbx_index) = node.fbx_index {
            let model = &models[fbx_index];
            let (p, r, s) = scene_trs_from_local_transform(&model.transform, &node.name, label)?;
            let (mut pos, mut rot, mut scale) = fbx_trs_to_unity(p, r, s);
            // 主インスタンスの姿勢上書きを適用する。成分ごとに置き換える。
            // Prefabの値はUnity空間（メートル）であり、量子化済みの値をそのまま使う。
            // graft経路（Milfy）では上書きは無い。
            let tr_fid = hier.tr_fid_at[fbx_index];
            if let Some(over) = overlays.trs.get(&tr_fid) {
                for (slot, value) in over.position.iter().enumerate() {
                    if let Some(v) = value {
                        pos[slot] = *v;
                    }
                }
                for (slot, value) in over.rotation.iter().enumerate() {
                    if let Some(v) = value {
                        rot[slot] = *v;
                    }
                }
                for (slot, value) in over.scale.iter().enumerate() {
                    if let Some(v) = value {
                        scale[slot] = *v;
                    }
                }
            }
            scene_trs.push((
                [
                    pos[0] as f32 * unit_to_meter,
                    pos[1] as f32 * unit_to_meter,
                    pos[2] as f32 * unit_to_meter,
                ],
                [rot[0] as f32, rot[1] as f32, rot[2] as f32, rot[3] as f32],
                [scale[0] as f32, scale[1] as f32, scale[2] as f32],
            ));
        } else if let Some(phone_model) = node.phone_model {
            let phone = phone.as_ref().ok_or_else(|| {
                VrmError::new(
                    VrmErrorKind::InvalidPrefab,
                    format!(
                        "副階層なしに副FBXノードを解決できない: {}",
                        crate::escape_diag(&node.name)
                    ),
                )
            })?;
            let model = phone.models.get(phone_model).ok_or_else(|| {
                VrmError::new(
                    VrmErrorKind::InvalidPrefab,
                    format!(
                        "副FBXのモデル索引が範囲外である: {}",
                        crate::escape_diag(&node.name)
                    ),
                )
            })?;
            let (p, r, s) = scene_trs_from_local_transform(&model.transform, &node.name, label)?;
            let (pos, rot, scale) = fbx_trs_to_unity(p, r, s);
            scene_trs.push((
                [pos[0] as f32, pos[1] as f32, pos[2] as f32],
                [rot[0] as f32, rot[1] as f32, rot[2] as f32, rot[3] as f32],
                [scale[0] as f32, scale[1] as f32, scale[2] as f32],
            ));
        } else if let Some((p, r, s)) = node.added_trs {
            scene_trs.push((
                [p[0] as f32, p[1] as f32, p[2] as f32],
                [r[0] as f32, r[1] as f32, r[2] as f32, r[3] as f32],
                [s[0] as f32, s[1] as f32, s[2] as f32],
            ));
        } else {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!(
                    "出所も追加TRSもないノードがある: {}",
                    crate::escape_diag(&node.name)
                ),
            ));
        }
    }
    // scene世界行列（float32）。親から順に積む。ツリーはDFS順であり親が先に確定する。
    let mut scene_world: Vec<[f32; 16]> = vec![f32_identity(); tree.len()];
    for (i, node) in tree.iter().enumerate() {
        let local = compose_trs_f32(scene_trs[i].0, scene_trs[i].1, scene_trs[i].2);
        match node.parent {
            Some(parent) => scene_world[i] = mul_f32(&scene_world[parent], &local),
            None => scene_world[i] = local,
        }
    }
    // 参照ローカルの計算。
    let mut out: Vec<ReferenceLocal> = Vec::with_capacity(tree.len());
    for (i, node) in tree.iter().enumerate() {
        let is_bind = match node.fbx_index {
            Some(fbx_index) => {
                bind.joint_models.contains(&fbx_index) && bind_world_f32.contains_key(&fbx_index)
            }
            None => false,
        };
        if is_bind {
            let fbx_index = node.fbx_index.expect("bindはFBX由来である");
            let bind_world = bind_world_f32.get(&fbx_index).expect("存在は確かめた");
            // 親世界はbindがあればbind、なければsceneを使う。
            let parent_world: [f32; 16] = match node.parent {
                Some(parent) => {
                    let parent_node = &tree[parent];
                    match parent_node.fbx_index {
                        Some(parent_fbx)
                            if bind.joint_models.contains(&parent_fbx)
                                && bind_world_f32.contains_key(&parent_fbx) =>
                        {
                            *bind_world_f32.get(&parent_fbx).expect("存在は確かめた")
                        }
                        _ => scene_world[parent],
                    }
                }
                None => {
                    // アバタールート直下。親は単位行列（Milfyルート）である。
                    f32_identity()
                }
            };
            let inv = inverse_f32(&parent_world).ok_or_else(|| {
                VrmError::new(
                    VrmErrorKind::InvalidPrefab,
                    format!("親世界が特異である: {}", crate::escape_diag(&node.name)),
                )
            })?;
            // 差分はUnity空間で求まっているため、そのまま分解して使う。
            // FBX空間で分解してから写すと回転の符号が変わる。
            let local = mul_f32(&inv, bind_world);
            match decompose_bind_local_f32(&local) {
                Ok((p, r, s)) => {
                    out.push((
                        [p[0] as f64, p[1] as f64, p[2] as f64],
                        [r[0] as f64, r[1] as f64, r[2] as f64, r[3] as f64],
                        [s[0] as f64, s[1] as f64, s[2] as f64],
                        "bind",
                    ));
                }
                Err(_) => {
                    // せん断等でTRSに戻らない行列。申告して近似として出す。
                    // 正確な近似値は後段で求める。ここではscene値（Unity空間）を仮置きする。
                    let (p, r, s) = scene_trs[i];
                    out.push((
                        [p[0] as f64, p[1] as f64, p[2] as f64],
                        [r[0] as f64, r[1] as f64, r[2] as f64, r[3] as f64],
                        [s[0] as f64, s[1] as f64, s[2] as f64],
                        "bind-decomposed-approx",
                    ));
                }
            }
        } else {
            // scene値（Unity空間）をそのまま使う。
            let (p, r, s) = scene_trs[i];
            out.push((
                [p[0] as f64, p[1] as f64, p[2] as f64],
                [r[0] as f64, r[1] as f64, r[2] as f64, r[3] as f64],
                [s[0] as f64, s[1] as f64, s[2] as f64],
                "scene",
            ));
        }
    }
    Ok(out)
}

/// FBXのローカル変換からsceneローカルTRS（float32）を求める。
/// `local_matrix` の分解であり、Unityの取り込みTRSと同じ手順である。
#[cfg(not(target_arch = "wasm32"))]
fn scene_trs_from_local_transform(
    transform: &LocalTransform,
    node_name: &str,
    label: &str,
) -> Result<TrsF32, VrmError> {
    let local = transform.local_matrix().map_err(|e| {
        VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!("ローカル行列を作れない: {e}"),
        )
    })?;
    let mut m32 = [0f32; 16];
    for (i, v) in local.iter().enumerate() {
        m32[i] = *v as f32;
    }
    decompose_bind_local_f32(&m32).map_err(|_| {
        VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!(
                "sceneローカルを分解できない: {}（{}）",
                crate::escape_diag(node_name),
                crate::escape_diag(label)
            ),
        )
    })
}

/// float32行列をUnity空間へ写す。`Mx * M * Mx^-1`（`Mx = diag(-1,1,1,1)`）であり、
/// FBXの右手系からUnityの左手系への写像である。TRS行列の写像は
/// `fbx_trs_to_unity` と一致するが、一般行列の回転抽出は写像後に行う。
/// Unityはbind差分を行列で求めてから分解するため、こちらに合わせる。
#[cfg(not(target_arch = "wasm32"))]
fn mirror_matrix_f32(m: &[f32; 16]) -> [f32; 16] {
    let mut out = *m;
    out[1] = -out[1];
    out[2] = -out[2];
    out[4] = -out[4];
    out[8] = -out[8];
    out[12] = -out[12];
    out
}

/// FBXの右手系TRSをUnityの左手系TRSへ写す。実測（Milfy）で位置Xと回転Y/Zの符号が反転するため。
/// 位置はXだけを反転し、回転はY/Zだけを反転し、拡大率は変えない。恒等回転は保たれる。
#[cfg(not(target_arch = "wasm32"))]
fn fbx_trs_to_unity(p: [f32; 3], r: [f32; 4], s: [f32; 3]) -> ([f64; 3], [f64; 4], [f64; 3]) {
    (
        [-(p[0] as f64), p[1] as f64, p[2] as f64],
        [r[0] as f64, -(r[1] as f64), -(r[2] as f64), r[3] as f64],
        [s[0] as f64, s[1] as f64, s[2] as f64],
    )
}

/// PrefabのYAML数値をUnityのfloat32として読み直す。`0.7` は `0.699999988...` になる。
/// sidecarのJSON比較でUnity出力と一致させるための量子化である。
#[cfg(not(target_arch = "wasm32"))]
fn quantize_f32(value: f64) -> f64 {
    (value as f32) as f64
}

/// float32の単位行列。
#[cfg(not(target_arch = "wasm32"))]
fn f32_identity() -> [f32; 16] {
    [
        1.0, 0.0, 0.0, 0.0, //
        0.0, 1.0, 0.0, 0.0, //
        0.0, 0.0, 1.0, 0.0, //
        0.0, 0.0, 0.0, 1.0, //
    ]
}

/// float32の行列積。列ベクトル規約である。
#[cfg(not(target_arch = "wasm32"))]
fn mul_f32(a: &[f32; 16], b: &[f32; 16]) -> [f32; 16] {
    let mut out = [0f32; 16];
    for row in 0..4 {
        for col in 0..4 {
            let mut sum = 0f32;
            for k in 0..4 {
                sum += a[k * 4 + row] * b[col * 4 + k];
            }
            out[col * 4 + row] = sum;
        }
    }
    out
}

/// float32の逆行列。特異な場合は `None` を返す。
#[cfg(not(target_arch = "wasm32"))]
fn inverse_f32(m: &[f32; 16]) -> Option<[f32; 16]> {
    let mut a = [[0f32; 8]; 4];
    for row in 0..4 {
        for col in 0..4 {
            a[row][col] = m[col * 4 + row];
        }
        a[row][4 + row] = 1.0;
    }
    for col in 0..4 {
        let mut pivot = col;
        for row in col..4 {
            if a[row][col].abs() > a[pivot][col].abs() {
                pivot = row;
            }
        }
        if a[pivot][col] == 0.0 {
            return None;
        }
        a.swap(col, pivot);
        let diag = a[col][col];
        for value in a[col].iter_mut() {
            *value /= diag;
        }
        let pivot_row = a[col];
        for (row, target) in a.iter_mut().enumerate() {
            if row != col {
                let factor = target[col];
                for (value, pivot) in target.iter_mut().zip(pivot_row.iter()) {
                    *value -= factor * pivot;
                }
            }
        }
    }
    let mut out = [0f32; 16];
    for row in 0..4 {
        for col in 0..4 {
            out[col * 4 + row] = a[row][4 + col];
        }
    }
    Some(out)
}

/// float32のTRS合成。`T * R * S` の順である。
#[cfg(not(target_arch = "wasm32"))]
fn compose_trs_f32(p: [f32; 3], r: [f32; 4], s: [f32; 3]) -> [f32; 16] {
    let (x, y, z, w) = (r[0], r[1], r[2], r[3]);
    let (sx, sy, sz) = (s[0], s[1], s[2]);
    let xx = x * x;
    let yy = y * y;
    let zz = z * z;
    let xy = x * y;
    let xz = x * z;
    let yz = y * z;
    let wx = w * x;
    let wy = w * y;
    let wz = w * z;
    let r00 = 1.0 - 2.0 * (yy + zz);
    let r10 = 2.0 * (xy + wz);
    let r20 = 2.0 * (xz - wy);
    let r01 = 2.0 * (xy - wz);
    let r11 = 1.0 - 2.0 * (xx + zz);
    let r21 = 2.0 * (yz + wx);
    let r02 = 2.0 * (xz + wy);
    let r12 = 2.0 * (yz - wx);
    let r22 = 1.0 - 2.0 * (xx + yy);
    [
        r00 * sx,
        r10 * sx,
        r20 * sx,
        0.0, //
        r01 * sy,
        r11 * sy,
        r21 * sy,
        0.0, //
        r02 * sz,
        r12 * sz,
        r22 * sz,
        0.0, //
        p[0],
        p[1],
        p[2],
        1.0, //
    ]
}

/// Unityの `DecomposeBindLocal` と同順のfloat32分解。鏡像はX軸に寄せる。
#[cfg(not(target_arch = "wasm32"))]
fn decompose_bind_local_f32(m: &[f32; 16]) -> Result<TrsF32, ()> {
    let position = [m[12], m[13], m[14]];
    let x = [m[0], m[1], m[2]];
    let y = [m[4], m[5], m[6]];
    let z = [m[8], m[9], m[10]];
    const EPS: f32 = 1e-8;
    fn length(v: &[f32; 3]) -> f32 {
        (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
    }
    let mut sx = length(&x);
    let sy = length(&y);
    let sz = length(&z);
    let mut nx = if sx > EPS {
        [x[0] / sx, x[1] / sx, x[2] / sx]
    } else {
        [1.0, 0.0, 0.0]
    };
    let ny = if sy > EPS {
        [y[0] / sy, y[1] / sy, y[2] / sy]
    } else {
        [0.0, 1.0, 0.0]
    };
    let nz = if sz > EPS {
        [z[0] / sz, z[1] / sz, z[2] / sz]
    } else {
        [0.0, 0.0, 1.0]
    };
    // 鏡像の判定。det3 < 0 のときX軸に寄せる。
    let cross_xy = [
        x[1] * y[2] - x[2] * y[1],
        x[2] * y[0] - x[0] * y[2],
        x[0] * y[1] - x[1] * y[0],
    ];
    let det = cross_xy[0] * z[0] + cross_xy[1] * z[1] + cross_xy[2] * z[2];
    if det < 0.0 {
        nx = [-nx[0], -nx[1], -nx[2]];
        sx = -sx;
    }
    let _ = (sy, sz);
    let scale = [sx, sy, sz];
    let rotation = quat_from_rotation_matrix_f32([nx, ny, nz]);
    // TRSで元行列に戻るか検証する。戻らない場合は近似として失敗にする。
    let rebuilt = compose_trs_f32(position, rotation, scale);
    for i in 0..16 {
        if (rebuilt[i] - m[i]).abs() > 1e-4 {
            return Err(());
        }
    }
    Ok((position, rotation, scale))
}

/// 回転行列（3列）から四元数を求める。Unityの `Matrix4x4.rotation` と同じ分岐規則である。
///
/// 対角和が正の場合はそこから求め、そうでなければ最大の対角成分を持つ分岐を選ぶ。
/// 平方根を成分ごとに取る方式とは180度付近の符号が変わるため、こちらに合わせる。
/// 取り出し後の符号はUnityの行列と一致し、再合成で元に戻る。
#[cfg(not(target_arch = "wasm32"))]
fn quat_from_rotation_matrix_f32(cols: [[f32; 3]; 3]) -> [f32; 4] {
    // 列ベクトルを行列要素に直す。m[row][col] = cols[col][row] である。
    let m00 = cols[0][0];
    let m01 = cols[1][0];
    let m02 = cols[2][0];
    let m10 = cols[0][1];
    let m11 = cols[1][1];
    let m12 = cols[2][1];
    let m20 = cols[0][2];
    let m21 = cols[1][2];
    let m22 = cols[2][2];
    let trace = m00 + m11 + m22;
    if trace > 0.0 {
        let s = (trace + 1.0).sqrt() * 2.0;
        let w = 0.25 * s;
        let x = (m21 - m12) / s;
        let y = (m02 - m20) / s;
        let z = (m10 - m01) / s;
        [x, y, z, w]
    } else if m00 > m11 && m00 > m22 {
        let s = (1.0 + m00 - m11 - m22).sqrt() * 2.0;
        let w = (m21 - m12) / s;
        let x = 0.25 * s;
        let y = (m01 + m10) / s;
        let z = (m02 + m20) / s;
        [x, y, z, w]
    } else if m11 > m22 {
        let s = (1.0 + m11 - m00 - m22).sqrt() * 2.0;
        let w = (m02 - m20) / s;
        let x = (m01 + m10) / s;
        let y = 0.25 * s;
        let z = (m12 + m21) / s;
        [x, y, z, w]
    } else {
        let s = (1.0 + m22 - m00 - m11).sqrt() * 2.0;
        let w = (m10 - m01) / s;
        let x = (m02 + m20) / s;
        let y = (m12 + m21) / s;
        let z = 0.25 * s;
        [x, y, z, w]
    }
}

#[cfg(not(target_arch = "wasm32"))]
struct Collected {
    ignored: Vec<String>,
    phys_bones: Vec<CollectedBone>,
    colliders: Vec<CollectedCollider>,
    constraints: Vec<CollectedConstraint>,
    descriptor: Option<CollectedDescriptor>,
}

/// PhysBone1件の収集結果。sidecar DTOへの変換前の中間形である。
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone)]
struct CollectedBone {
    /// 文書順。同一ノード上の順序決定に使う。
    order: usize,
    component_node: usize,
    root_node: usize,
    enabled: bool,
    active_chain: bool,
    version: String,
    integration: String,
    ignore_nodes: Vec<usize>,
    ignore_other: bool,
    endpoint: [f64; 3],
    multi_child: String,
    pull: CollectedCurve,
    spring: CollectedCurve,
    stiffness: CollectedCurve,
    gravity: CollectedCurve,
    gravity_falloff: CollectedCurve,
    immobile_type: String,
    immobile: CollectedCurve,
    radius: CollectedCurve,
    allow_collision: bool,
    collider_docs: Vec<FileId>,
    limit_type: String,
    max_angle_x: CollectedCurve,
    max_angle_z: CollectedCurve,
    limit_rotation: [f64; 3],
    limit_rx: CollectedCurve,
    limit_ry: CollectedCurve,
    limit_rz: CollectedCurve,
    is_animated: bool,
    allow_grabbing: bool,
    allow_posing: bool,
    has_stretch: bool,
}

/// collider1件の収集結果。
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone)]
struct CollectedCollider {
    doc: FileId,
    /// 文書順。同一ノード上の順序決定に使う。
    order: usize,
    component_node: usize,
    root_node: usize,
    enabled: bool,
    active_chain: bool,
    shape: String,
    inside: bool,
    radius: f64,
    height: f64,
    position: [f64; 3],
    rotation: [f64; 4],
    bones_as_spheres: bool,
}

/// 制約1件の収集結果。
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone)]
struct CollectedConstraint {
    /// 文書順。同一ノード上の順序決定に使う。
    order: usize,
    component_node: usize,
    target_node: usize,
    constraint_type: String,
    family: String,
    weight: f64,
    enabled: bool,
    active: bool,
    freeze: bool,
    affects: [bool; 3],
    aim_axis: [f64; 3],
    sources: Vec<(usize, f64)>,
    scene_path: Vec<String>,
    order_index: usize,
}

/// AvatarDescriptorの収集結果。視線に使う。
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone)]
struct CollectedDescriptor {
    #[allow(dead_code)]
    doc: FileId,
    enable_eye_look: bool,
    left_eye: Option<usize>,
    right_eye: Option<usize>,
    eyes: EyePoses,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone)]
struct EyePoses {
    straight: ([bool; 1], [f64; 4], [f64; 4]),
    up: ([bool; 1], [f64; 4], [f64; 4]),
    down: ([bool; 1], [f64; 4], [f64; 4]),
    left: ([bool; 1], [f64; 4], [f64; 4]),
    right: ([bool; 1], [f64; 4], [f64; 4]),
}

/// AnimationCurveの中間形。sidecarのCurve形式に直結する。
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone)]
struct CollectedCurve {
    value: f64,
    keys: Vec<CollectedKey>,
    pre_wrap: u32,
    post_wrap: u32,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone)]
struct CollectedKey {
    time: f64,
    value: f64,
    in_slope: f64,
    out_slope: f64,
    in_weight: f64,
    out_weight: f64,
}

#[cfg(not(target_arch = "wasm32"))]
fn collect_components(
    parsed: &ParsedPrefab,
    tree: &[TreeNode],
    phone: &Option<PhoneSubtree>,
    _fbx_guid: &str,
    label: &str,
) -> Result<Collected, VrmError> {
    // ノード索引の構築。MonoBehaviourの参照解決に使う。
    // stripped TR fid → tree node、stripped GO fid → tree node、doc fid → tree node。
    let mut tr_to_node: HashMap<FileId, usize> = HashMap::new();
    let mut go_to_node: HashMap<FileId, usize> = HashMap::new();
    // FBX由来ノードは、Transform/GameObjectのfileIDから引けるよう表を作る。
    // treeのsource.local_idはTransform fileIDである。GameObject fileIDは別途求める。
    // 取り込み根の例外は graft 根（FBX由来でなくsourceを持つ節点）だけに使う。
    // 単一区画のFBXパス（例: `Armature`）は通常のGameObject fileIDで引く。
    for (i, node) in tree.iter().enumerate() {
        if let Some(source) = node.source.as_ref() {
            tr_to_node.insert(source.local_id, i);
            let go_fid = if node.fbx_index.is_none() && node.phone_model.is_none() {
                crate::unity::game_object_file_id("")
            } else {
                crate::unity::game_object_file_id(&source.path.join("/"))
            };
            go_to_node.insert(go_fid, i);
        }
    }
    // 副Prefab側のstripped対応付け。local fid → source fid → tree node である。
    // 副階層の graft 根・SmartPhone相当はここで結ぶ。
    if let Some(phone) = phone.as_ref() {
        for (local, stripped) in phone.parsed.stripped_tr.iter() {
            if let Some(&node) = tr_to_node.get(&stripped.source_fid) {
                tr_to_node.insert(*local, node);
            }
        }
        for (local, stripped) in phone.parsed.stripped_go.iter() {
            if let Some(&node) = go_to_node.get(&stripped.source_fid) {
                go_to_node.insert(*local, node);
            }
        }
    }
    // strippedの対応付け。local fid → source fid → tree node である。
    // 取り込み根由来（import root）は graft 根と混同するため結ばない。
    // 該当はAvatarDescriptor等の所属だけであり、参照解決時に失敗させる。
    let import_tr = transform_file_id("");
    let import_go = crate::unity::game_object_file_id("");
    for (local, stripped) in parsed.stripped_tr.iter() {
        if stripped.source_fid == import_tr {
            continue;
        }
        if let Some(&node) = tr_to_node.get(&stripped.source_fid) {
            tr_to_node.insert(*local, node);
        }
    }
    for (local, stripped) in parsed.stripped_go.iter() {
        if stripped.source_fid == import_go {
            continue;
        }
        // GameObjectのsource fidからtree nodeを引く。
        if let Some(&node) = go_to_node.get(&stripped.source_fid) {
            go_to_node.insert(*local, node);
        }
        // 解決できないstripped GOは参照解決時に失敗させる。
        let _ = local;
    }
    // MonoBehaviourのdoc fid → tree node（所属GO経由）。
    // ローカルGO（Ground等）もtreeのlocal_goから引く。
    let mut local_go_to_node: HashMap<FileId, usize> = HashMap::new();
    for (i, node) in tree.iter().enumerate() {
        if let Some(go) = node.local_go {
            local_go_to_node.insert(go, i);
        }
        // ローカルTransformの文書IDからも引けるようにする。
        // 追加物上のPhysBoneのrootTransform等がローカルTRを指す場合に使う。
        if let Some(tr) = node.local_tr {
            tr_to_node.insert(tr, i);
        }
    }
    // 主Prefabと副PrefabのMonoBehaviourを合わせる。副階層の成分も同じ規則で収集する。
    let mut all_monos: Vec<&ParsedMono> = parsed.monos.iter().collect();
    if let Some(phone) = phone.as_ref() {
        all_monos.extend(phone.parsed.monos.iter());
    }
    let mut doc_to_node: HashMap<FileId, usize> = HashMap::new();
    for mono in all_monos.iter().copied() {
        if let Some(&node) = go_to_node.get(&mono.go) {
            doc_to_node.insert(mono.doc, node);
        } else if let Some(&node) = local_go_to_node.get(&mono.go) {
            doc_to_node.insert(mono.doc, node);
        }
        // 解決できない所属（import root等のstripped GO）は、参照解決時に失敗させる。
    }
    // 副Prefab側の改変のうち、成分文書を対象にする重ね合わせ表を作る。
    // 制約の参照先（`Sources.*`）だけを扱い、他は呼び出し側で失敗させる。
    let mut source_overlays = phone_source_overlays(parsed, phone, label)?;
    // 成分の判別と収集。未知のDLL型は明示的に失敗し、Contact等は無視する。
    let mut ignored: Vec<String> = Vec::new();
    let mut ignored_set: HashSet<String> = HashSet::new();
    let mut bones: Vec<CollectedBone> = Vec::new();
    let mut colliders: Vec<CollectedCollider> = Vec::new();
    let mut constraints: Vec<CollectedConstraint> = Vec::new();
    let mut descriptor: Option<CollectedDescriptor> = None;
    for mono in all_monos.iter().copied() {
        if mono.script_guid.is_empty() {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!(
                    "スクリプト参照にGUIDがない（文書{}）（{}）",
                    mono.doc,
                    crate::escape_diag(label)
                ),
            ));
        }
        match judge_script(&mono.script_guid, mono.script_fid) {
            ScriptJudgment::External => {
                // VRChatのDLLでない外部参照は無視する。KumaPhone等のPrefab参照が該当する。
                continue;
            }
            ScriptJudgment::UnknownDll => {
                return Err(VrmError::new(
                    VrmErrorKind::InvalidPrefab,
                    format!(
                        "型判定できないコンポーネントがある: guid={} fileID={}（文書{}）（{}）",
                        crate::escape_diag(&mono.script_guid),
                        mono.script_fid,
                        mono.doc,
                        crate::escape_diag(label),
                    ),
                ));
            }
            ScriptJudgment::Known(kind) => match kind {
                KnownScript::ContactReceiver
                | KnownScript::HeadChop
                | KnownScript::PipelineManager => {
                    // 無関係な成分は無視し、型名を報告する。
                    let name = match kind {
                        KnownScript::ContactReceiver => "VRCContactReceiver",
                        KnownScript::HeadChop => "VRCHeadChop",
                        KnownScript::PipelineManager => "VRC_PipelineManager",
                        _ => "?",
                    };
                    if ignored_set.insert(name.to_owned()) {
                        ignored.push(name.to_owned());
                    }
                    continue;
                }
                KnownScript::PhysBone => {
                    let Some(&node) = doc_to_node.get(&mono.doc) else {
                        return Err(VrmError::new(
                            VrmErrorKind::InvalidPrefab,
                            format!(
                                "PhysBoneの参照ノードを解決できない（文書{}）（{}）",
                                mono.doc,
                                crate::escape_diag(label)
                            ),
                        ));
                    };
                    if source_overlays.contains_key(&mono.doc) {
                        return Err(VrmError::new(
                            VrmErrorKind::InvalidPrefab,
                            format!(
                                "PhysBoneへの参照先の重ね合わせは未対応である（文書{}）（{}）",
                                mono.doc,
                                crate::escape_diag(label)
                            ),
                        ));
                    }
                    // 有効 chain の計算を先に行い、無効なものは読まずに省く。
                    // Unityヘルパも有効な成分だけを読み取るため、
                    // 無効な旧版の欄構成でも失敗しない。
                    // 無効なものはsidecarに含めない（Unityヘルパと同順）。
                    // 副階層の graft 根は無効であり、子孫はすべて無効になる。
                    let active = node_active(tree, node, parsed);
                    if !(mono.enabled && active) {
                        continue;
                    }
                    let mut bone = parse_phys_bone(mono, node, &tr_to_node, label)?;
                    bone.active_chain = true;
                    bones.push(bone);
                }
                KnownScript::PhysBoneCollider => {
                    let Some(&node) = doc_to_node.get(&mono.doc) else {
                        return Err(VrmError::new(
                            VrmErrorKind::InvalidPrefab,
                            format!(
                                "colliderの参照ノードを解決できない（文書{}）（{}）",
                                mono.doc,
                                crate::escape_diag(label)
                            ),
                        ));
                    };
                    if source_overlays.contains_key(&mono.doc) {
                        return Err(VrmError::new(
                            VrmErrorKind::InvalidPrefab,
                            format!(
                                "colliderへの参照先の重ね合わせは未対応である（文書{}）（{}）",
                                mono.doc,
                                crate::escape_diag(label)
                            ),
                        ));
                    }
                    let active = node_active(tree, node, parsed);
                    if !(mono.enabled && active) {
                        continue;
                    }
                    let mut collider = parse_collider(mono, node, &tr_to_node, label)?;
                    collider.active_chain = true;
                    colliders.push(collider);
                }
                KnownScript::RotationConstraint
                | KnownScript::ParentConstraint
                | KnownScript::PositionConstraint
                | KnownScript::ScaleConstraint
                | KnownScript::AimConstraint
                | KnownScript::LookAtConstraint => {
                    let Some(&node) = doc_to_node.get(&mono.doc) else {
                        return Err(VrmError::new(
                            VrmErrorKind::InvalidPrefab,
                            format!(
                                "制約の参照ノードを解決できない（文書{}）（{}）",
                                mono.doc,
                                crate::escape_diag(label)
                            ),
                        ));
                    };
                    // Milfy側の改変で参照先を差し替える（副Prefabの制約）。
                    // 対象外の文書への重ね合わせはここで消費し、残りは後段で失敗させる。
                    let effective = match source_overlays.remove(&mono.doc) {
                        Some(mods) => {
                            let patched = overlay_constraint_sources(&mono.extra, &mods, label)?;
                            ParsedMono {
                                extra: patched,
                                ..mono.clone()
                            }
                        }
                        None => mono.clone(),
                    };
                    // 同一Transform上のVRC制約の順序を求める。
                    let order_index = constraints
                        .iter()
                        .filter(|c: &&CollectedConstraint| c.component_node == node)
                        .count();
                    let constraint = parse_vrc_constraint(
                        &effective,
                        kind,
                        node,
                        tree,
                        &tr_to_node,
                        order_index,
                        label,
                    )?;
                    constraints.push(constraint);
                }
                KnownScript::AvatarDescriptor => {
                    if descriptor.is_some() {
                        return Err(VrmError::new(
                            VrmErrorKind::InvalidPrefab,
                            format!(
                                "VRCAvatarDescriptorが複数ある（{}）",
                                crate::escape_diag(label)
                            ),
                        ));
                    }
                    if source_overlays.contains_key(&mono.doc) {
                        return Err(VrmError::new(
                            VrmErrorKind::InvalidPrefab,
                            format!(
                                "descriptorへの参照先の重ね合わせは未対応である（文書{}）（{}）",
                                mono.doc,
                                crate::escape_diag(label)
                            ),
                        ));
                    }
                    descriptor = Some(parse_descriptor(mono, &tr_to_node, label)?);
                }
            },
        }
    }
    // Unityネイティブの回転制約を収集する。VRC制約と同じ決定性規則
    // （ノードDFS順→文書順）で並べる。UnityヘルパはVRC制約の後に
    // Unity制約を集めるが、片方だけの実データでは順序は一致する。
    // 同一ノード上に両種別が混在する場合は順序がずれる可能性があり、
    // その場合は別途対応が必要である。
    for native in &parsed.native_rotations {
        let Some(&node) = go_to_node
            .get(&native.go)
            .or_else(|| local_go_to_node.get(&native.go))
        else {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!(
                    "制約の参照ノードを解決できない（文書{}）（{}）",
                    native.doc,
                    crate::escape_diag(label)
                ),
            ));
        };
        let active = node_active(tree, node, parsed);
        if !(native.enabled && active) {
            continue;
        }
        // 同一ノード上の制約順序を求める。VRC制約と通しで数える。
        // Unityは `GetComponents<IConstraint>` の順序であり、
        // 1ノード1制約の実データでは一致する。
        let order_index = constraints
            .iter()
            .filter(|c: &&CollectedConstraint| c.component_node == node)
            .count();
        let mut sources = Vec::with_capacity(native.sources.len());
        for (fid, weight) in &native.sources {
            if *fid == 0 {
                // 未設定の参照は空ID・外部扱いで残す。重みを保持する。
                // Unityは外部偽で出すが、参照なしの実データでは差が出ない。
                sources.push((usize::MAX, *weight));
                continue;
            }
            if let Some(&target) = tr_to_node.get(fid) {
                sources.push((target, *weight));
            } else {
                // アバター外の参照は空ID・外部扱いで残す。
                sources.push((usize::MAX, *weight));
            }
        }
        constraints.push(CollectedConstraint {
            order: native.order,
            component_node: node,
            target_node: node,
            constraint_type: "Rotation".to_owned(),
            family: "unity".to_owned(),
            weight: quantize_f32(native.weight),
            enabled: native.enabled,
            active: native.active,
            freeze: false,
            affects: native.affects,
            aim_axis: [0.0, 0.0, 1.0],
            sources,
            scene_path: tree[node].scene_path.clone(),
            order_index,
        });
    }
    // 消費されなかった重ね合わせは未対応として失敗する。
    if let Some((doc, _)) = source_overlays.iter().next() {
        return Err(VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!(
                "制約でない文書への参照先の重ね合わせは未対応である（文書{doc}）（{}）",
                crate::escape_diag(label)
            ),
        ));
    }
    // ノードDFS順→取得順（文書順）で並べ直す。Unityヘルパと同じ決定性規則である。
    // component_node（tree索引）がDFS順であり、文書順が取得順である。
    // 文書順はfileIDの数値順ではなく、Prefabファイル内の出現順である。
    bones.sort_by(|a, b| {
        a.component_node
            .cmp(&b.component_node)
            .then(a.order.cmp(&b.order))
    });
    colliders.sort_by(|a, b| {
        a.component_node
            .cmp(&b.component_node)
            .then(a.order.cmp(&b.order))
    });
    constraints.sort_by(|a, b| {
        a.component_node
            .cmp(&b.component_node)
            .then(a.order.cmp(&b.order))
    });
    ignored.sort();
    ignored.dedup();
    Ok(Collected {
        ignored,
        phys_bones: bones,
        colliders,
        constraints,
        descriptor,
    })
}

/// ノードのactiveInHierarchyを求める。FBX由来は真、追加物はprefabのactiveである。
/// 副階層の graft 根はMilfy側の改変で無効であり、子孫はすべて無効になる。
#[cfg(not(target_arch = "wasm32"))]
fn node_active(tree: &[TreeNode], node: usize, parsed: &ParsedPrefab) -> bool {
    let mut current = Some(node);
    while let Some(index) = current {
        let tree_node = &tree[index];
        if !tree_node.active_self {
            return false;
        }
        if let Some(go_fid) = tree_node.local_go {
            // 追加物。prefabのm_IsActiveである。
            let active = parsed
                .local_go
                .get(&go_fid)
                .map(|go| go.active)
                .unwrap_or(true);
            if !active {
                return false;
            }
        }
        // FBX由来は真（m_IsActiveの上書きがない場合）。
        // Milfyの実データでは上書きがない。
        current = tree_node.parent;
    }
    true
}

/// AvatarDescriptorを読む。視線に使う。
#[cfg(not(target_arch = "wasm32"))]
fn parse_descriptor(
    mono: &ParsedMono,
    tr_to_node: &HashMap<FileId, usize>,
    label: &str,
) -> Result<CollectedDescriptor, VrmError> {
    let extra = &mono.extra;
    let doc = mono.doc;
    let enable_eye_look = extra
        .get("enableEyeLook")
        .and_then(|v| v.as_i64().or_else(|| v.as_bool().map(|b| b as i64)))
        .map(|n| n != 0)
        .unwrap_or(false);
    // customEyeLookSettingsから左右の目と姿勢を読む。
    let settings = extra
        .get("customEyeLookSettings")
        .and_then(|v| v.as_mapping());
    let mut left_eye: Option<usize> = None;
    let mut right_eye: Option<usize> = None;
    // 既定の姿勢（未設定時は非linked恒等）。
    let mut straight = ([false; 1], [0.0, 0.0, 0.0, 1.0], [0.0, 0.0, 0.0, 1.0]);
    let mut up = straight;
    let mut down = straight;
    let mut left = straight;
    let mut right = straight;
    if let Some(settings) = settings {
        if let Some(v) = settings.get("leftEye") {
            let r = parse_ref_value(v)?;
            if r.fid != 0 {
                left_eye = tr_to_node.get(&r.fid).copied();
                if left_eye.is_none() {
                    // アバター外の目は空文字にする（Unityヘルパと同順）。
                    left_eye = None;
                }
            }
        }
        if let Some(v) = settings.get("rightEye") {
            let r = parse_ref_value(v)?;
            if r.fid != 0 {
                right_eye = tr_to_node.get(&r.fid).copied();
                if right_eye.is_none() {
                    right_eye = None;
                }
            }
        }
        // 5方向の姿勢を読む。欠落時は既定のままにする。
        if let Some(v) = settings.get("eyesLookingStraight") {
            straight = read_eye_pose(v, doc, label)?;
        }
        if let Some(v) = settings.get("eyesLookingUp") {
            up = read_eye_pose(v, doc, label)?;
        }
        if let Some(v) = settings.get("eyesLookingDown") {
            down = read_eye_pose(v, doc, label)?;
        }
        if let Some(v) = settings.get("eyesLookingLeft") {
            left = read_eye_pose(v, doc, label)?;
        }
        if let Some(v) = settings.get("eyesLookingRight") {
            right = read_eye_pose(v, doc, label)?;
        }
    }
    Ok(CollectedDescriptor {
        doc,
        enable_eye_look,
        left_eye,
        right_eye,
        eyes: EyePoses {
            straight,
            up,
            down,
            left,
            right,
        },
    })
}

/// 視線の1方向を読む。`{linked, left:{x,y,z,w}, right:{...}}` である。
#[cfg(not(target_arch = "wasm32"))]
fn read_eye_pose(value: &Value, doc: FileId, label: &str) -> Result<EyePose, VrmError> {
    let mapping = value.as_mapping().ok_or_else(|| {
        VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!(
                "視線の姿勢が写像でない（文書{doc}）（{}）",
                crate::escape_diag(label)
            ),
        )
    })?;
    let linked = mapping
        .get("linked")
        .and_then(|v| v.as_i64().or_else(|| v.as_bool().map(|b| b as i64)))
        .map(|n| n != 0)
        .unwrap_or(false);
    let left = match mapping.get("left") {
        Some(v) => read_xyzw_value(v, "eye.left", label)?,
        None => [0.0, 0.0, 0.0, 1.0],
    };
    let right = match mapping.get("right") {
        Some(v) => read_xyzw_value(v, "eye.right", label)?,
        None => [0.0, 0.0, 0.0, 1.0],
    };
    Ok(([linked], left, right))
}

/// collider1件をMonoBehaviourから読む。
#[cfg(not(target_arch = "wasm32"))]
fn parse_collider(
    mono: &ParsedMono,
    component_node: usize,
    tr_to_node: &HashMap<FileId, usize>,
    label: &str,
) -> Result<CollectedCollider, VrmError> {
    let extra = &mono.extra;
    let doc = mono.doc;
    let root_node = match extra.get("rootTransform") {
        None | Some(Value::Null) => component_node,
        Some(v) => {
            let r = parse_ref_value(v)?;
            if r.fid == 0 {
                component_node
            } else if let Some(&node) = tr_to_node.get(&r.fid) {
                node
            } else {
                component_node
            }
        }
    };
    let shape_raw = extra
        .get("shapeType")
        .and_then(|v| v.as_i64())
        .ok_or_else(|| {
            VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!("欄がない: shapeType（文書{doc}）"),
            )
        })?;
    let shape = match shape_raw {
        0 => "Sphere",
        1 => "Capsule",
        2 => "Plane",
        _ => {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!("collider形状が未対応である: {shape_raw}（文書{doc}）"),
            ));
        }
    }
    .to_owned();
    let inside = extra_bool(extra, "insideBounds", doc, label)?;
    let radius = extra_f64(extra, "radius", doc, label)?;
    let height = extra_f64(extra, "height", doc, label)?;
    let position = extra_vec3(extra, "position", doc, label)?;
    let rotation = match extra.get("rotation") {
        Some(v) => read_xyzw_value(v, "rotation", label)?,
        None => [0.0, 0.0, 0.0, 1.0],
    };
    let bones_as_spheres = extra_bool(extra, "bonesAsSpheres", doc, label)?;
    Ok(CollectedCollider {
        doc,
        order: mono.order,
        component_node,
        root_node,
        enabled: mono.enabled,
        active_chain: true,
        shape,
        inside,
        radius,
        height,
        position,
        rotation,
        bones_as_spheres,
    })
}

/// VRC制約1件を読む。Unity制約は対象外として失敗する。
#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::too_many_arguments)]
fn parse_vrc_constraint(
    mono: &ParsedMono,
    kind: KnownScript,
    component_node: usize,
    tree: &[TreeNode],
    tr_to_node: &HashMap<FileId, usize>,
    order_index: usize,
    _label: &str,
) -> Result<CollectedConstraint, VrmError> {
    let extra = &mono.extra;
    let doc = mono.doc;
    let (constraint_type, family) = match kind {
        KnownScript::RotationConstraint => ("Rotation", "vrchat"),
        KnownScript::ParentConstraint => ("Parent", "vrchat"),
        KnownScript::PositionConstraint => ("Position", "vrchat"),
        KnownScript::ScaleConstraint => ("Scale", "vrchat"),
        KnownScript::AimConstraint => ("Aim", "vrchat"),
        KnownScript::LookAtConstraint => ("LookAt", "vrchat"),
        _ => {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!("制約型が未対応である（文書{doc}）"),
            ));
        }
    };
    // 対象はTargetTransform、なければ自ノードである。
    let target_node = match extra.get("TargetTransform") {
        None | Some(Value::Null) => component_node,
        Some(v) => {
            let r = parse_ref_value(v)?;
            if r.fid == 0 {
                component_node
            } else if let Some(&node) = tr_to_node.get(&r.fid) {
                node
            } else {
                return Err(VrmError::new(
                    VrmErrorKind::InvalidPrefab,
                    format!("制約の対象を解決できない（文書{doc}）"),
                ));
            }
        }
    };
    let weight_raw = extra
        .get("GlobalWeight")
        .and_then(|v| v.as_f64().or_else(|| v.as_i64().map(|n| n as f64)))
        .ok_or_else(|| {
            VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!("欄がない: GlobalWeight（文書{doc}）"),
            )
        })?;
    // PrefabのYAML数値は10進表記のため、Unityのfloat32へ量子化してから出す。
    // `0.7` は `0.699999988...` になり、Unity出力と一致する。
    let weight = quantize_f32(weight_raw);
    let active = extra
        .get("IsActive")
        .and_then(|v| v.as_i64())
        .map(|n| n != 0)
        .unwrap_or(false);
    let freeze = extra
        .get("FreezeToWorld")
        .and_then(|v| v.as_i64())
        .map(|n| n != 0)
        .unwrap_or(false);
    // 回転に効く軸。Rotationは3軸、他は種別による。
    let affects = match kind {
        KnownScript::RotationConstraint => {
            let ax = extra
                .get("AffectsRotationX")
                .and_then(|v| v.as_i64())
                .unwrap_or(1)
                != 0;
            let ay = extra
                .get("AffectsRotationY")
                .and_then(|v| v.as_i64())
                .unwrap_or(1)
                != 0;
            let az = extra
                .get("AffectsRotationZ")
                .and_then(|v| v.as_i64())
                .unwrap_or(1)
                != 0;
            [ax, ay, az]
        }
        _ => [true, true, true],
    };
    // 注視軸。Rotation等は既定の+Zである。
    let aim_axis = [0.0, 0.0, 1.0];
    // 参照先。Prefabの `Sources:` 写像にある `source{i}` を番号順に読み、末尾のnullを落とす。
    // Unityの `VRCConstraintBase.Sources` は末尾の未設定を保持しないため、最後の非nullまでが有効である。
    // 中間のnullは空ID・外部偽で残し、重みを保持する（Unityヘルパと同順）。
    let mut sources = Vec::new();
    if let Some(Value::Mapping(sources_map)) = extra.get("Sources") {
        // `source0` から連番で集める。欠番で打ち切り、16件を超えたら失敗させる。
        let mut slots: Vec<(usize, &Value)> = Vec::new();
        for i in 0..32 {
            let key = format!("source{i}");
            let Some(slot) = sources_map.get(key.as_str()) else {
                break;
            };
            slots.push((i, slot));
        }
        if slots.len() > 16 {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!("制約の参照数が多すぎる: {}（文書{doc}）", slots.len()),
            ));
        }
        // 末尾のnull（SourceTransformのfileIDが0）を落とす。有効長の確定である。
        let mut last_live: Option<usize> = None;
        let mut parsed_slots: Vec<(FileId, f64)> = Vec::with_capacity(slots.len());
        for (i, slot) in &slots {
            let slot_map = slot.as_mapping().ok_or_else(|| {
                VrmError::new(
                    VrmErrorKind::InvalidPrefab,
                    format!("制約の参照が写像でない: source{i}（文書{doc}）"),
                )
            })?;
            let tr_value = slot_map.get("SourceTransform").ok_or_else(|| {
                VrmError::new(
                    VrmErrorKind::InvalidPrefab,
                    format!("制約の参照先がない: source{i}（文書{doc}）"),
                )
            })?;
            let r = parse_ref_value(tr_value)?;
            let weight_value_raw = slot_map
                .get("Weight")
                .and_then(|v| v.as_f64().or_else(|| v.as_i64().map(|n| n as f64)))
                .unwrap_or(0.0);
            let weight_value = quantize_f32(weight_value_raw);
            if r.fid != 0 {
                last_live = Some(parsed_slots.len());
            }
            parsed_slots.push((r.fid, weight_value));
        }
        // 最後の非nullまでが有効である。全てnullの場合は空のままにする。
        if let Some(last) = last_live {
            for (fid, w) in parsed_slots.into_iter().take(last + 1) {
                if fid == 0 {
                    // 中間のnullは空IDで残す。重みは保持する（Unityヘルパと同順）。
                    // 出力時の `external` は偽になる。Milfyの実データに中間nullは無い。
                    sources.push((usize::MAX, w));
                } else if let Some(&node) = tr_to_node.get(&fid) {
                    sources.push((node, w));
                } else {
                    // アバター外の参照は空ID・外部扱いで残す。
                    sources.push((usize::MAX, w));
                }
            }
        }
    }
    let scene_path = tree[component_node].scene_path.clone();
    Ok(CollectedConstraint {
        order: mono.order,
        component_node,
        target_node,
        constraint_type: constraint_type.to_owned(),
        family: family.to_owned(),
        weight,
        enabled: mono.enabled,
        active,
        freeze,
        affects,
        aim_axis,
        sources,
        scene_path,
        order_index,
    })
}

/// PhysBone1件をMonoBehaviourから読む。参照はtree索引で解決する。
#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::too_many_arguments)]
fn parse_phys_bone(
    mono: &ParsedMono,
    component_node: usize,
    tr_to_node: &HashMap<FileId, usize>,
    label: &str,
) -> Result<CollectedBone, VrmError> {
    let extra = &mono.extra;
    let doc = mono.doc;
    // 版が無い旧版の欄構成では既定値を使う。Unityの読み取りでは0になる。
    let version_raw = extra.get("version").and_then(|v| v.as_i64()).unwrap_or(0);
    let version = match version_raw {
        0 => "1.0",
        1 => "1.1",
        _ => {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!("PhysBoneの版が未対応である: {version_raw}（文書{doc}）"),
            ));
        }
    }
    .to_owned();
    let integration_raw = extra
        .get("integrationType")
        .and_then(|v| v.as_i64())
        .ok_or_else(|| {
            VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!(
                    "欄がない: integrationType（文書{doc}）（{}）",
                    crate::escape_diag(label)
                ),
            )
        })?;
    let integration = match integration_raw {
        0 => "Simplified",
        1 => "Advanced",
        _ => {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!("積分方式が未対応である: {integration_raw}（文書{doc}）"),
            ));
        }
    }
    .to_owned();
    // rootTransformの解決。0は自ノード、外部は自ノードに読み替えて記録する。
    let mut root_outside = false;
    let root_node = match extra.get("rootTransform") {
        None | Some(Value::Null) => component_node,
        Some(v) => {
            let r = parse_ref_value(v)?;
            if r.fid == 0 {
                component_node
            } else if let Some(&node) = tr_to_node.get(&r.fid) {
                node
            } else {
                root_outside = true;
                component_node
            }
        }
    };
    let _ = root_outside;
    // ignoreTransforms（Milfyでは空）。
    let mut ignore_nodes = Vec::new();
    if let Some(Value::Sequence(seq)) = extra.get("ignoreTransforms") {
        for item in seq {
            let r = parse_ref_value(item)?;
            if r.fid == 0 {
                continue;
            }
            let Some(&node) = tr_to_node.get(&r.fid) else {
                return Err(VrmError::new(
                    VrmErrorKind::InvalidPrefab,
                    format!("ignoreTransformsの参照を解決できない（文書{doc}）"),
                ));
            };
            ignore_nodes.push(node);
        }
    }
    let ignore_other = match extra.get("ignoreOtherPhysBones") {
        // 旧版のSDKには無い欄であり、Unityの読み取りでは偽になる。
        None | Some(Value::Null) => false,
        Some(v) => read_bool_value(v, "ignoreOtherPhysBones", label)?,
    };
    let endpoint = extra_vec3(extra, "endpointPosition", doc, label)?;
    let multi_raw = extra
        .get("multiChildType")
        .and_then(|v| v.as_i64())
        .ok_or_else(|| {
            VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!("欄がない: multiChildType（文書{doc}）"),
            )
        })?;
    let multi_child = match multi_raw {
        0 => "Ignore",
        1 => "First",
        2 => "Average",
        _ => {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!("複数子の扱いが未対応である: {multi_raw}（文書{doc}）"),
            ));
        }
    }
    .to_owned();
    let pull = read_curve(extra, "pull", "pullCurve", doc, label)?;
    let spring = read_curve(extra, "spring", "springCurve", doc, label)?;
    let stiffness = read_curve(extra, "stiffness", "stiffnessCurve", doc, label)?;
    let gravity = read_curve(extra, "gravity", "gravityCurve", doc, label)?;
    let gravity_falloff = read_curve(extra, "gravityFalloff", "gravityFalloffCurve", doc, label)?;
    let immobile_raw = extra
        .get("immobileType")
        .and_then(|v| v.as_i64())
        .ok_or_else(|| {
            VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!("欄がない: immobileType（文書{doc}）"),
            )
        })?;
    let immobile_type = match immobile_raw {
        0 => "AllMotion",
        1 => "World",
        _ => {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!("不動の種別が未対応である: {immobile_raw}（文書{doc}）"),
            ));
        }
    }
    .to_owned();
    let immobile = read_curve(extra, "immobile", "immobileCurve", doc, label)?;
    let radius = read_curve(extra, "radius", "radiusCurve", doc, label)?;
    let allow_raw = extra
        .get("allowCollision")
        .and_then(|v| v.as_i64())
        .ok_or_else(|| {
            VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!("欄がない: allowCollision（文書{doc}）"),
            )
        })?;
    let allow_collision = allow_raw != 0;
    // collidersは文書fileIDの列であり、後段でc:Nに写す。
    let mut collider_docs = Vec::new();
    if let Some(Value::Sequence(seq)) = extra.get("colliders") {
        for item in seq {
            let r = parse_ref_value(item)?;
            if r.fid == 0 {
                continue;
            }
            collider_docs.push(r.fid);
        }
    }
    let limit_raw = extra
        .get("limitType")
        .and_then(|v| v.as_i64())
        .ok_or_else(|| {
            VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!("欄がない: limitType（文書{doc}）"),
            )
        })?;
    let limit_type = match limit_raw {
        0 => "None",
        1 => "Angle",
        2 => "Hinge",
        3 => "Polar",
        _ => {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!("制限の種別が未対応である: {limit_raw}（文書{doc}）"),
            ));
        }
    }
    .to_owned();
    let max_angle_x = read_curve(extra, "maxAngleX", "maxAngleXCurve", doc, label)?;
    let max_angle_z = read_curve(extra, "maxAngleZ", "maxAngleZCurve", doc, label)?;
    let limit_rotation = extra_vec3(extra, "limitRotation", doc, label)?;
    let limit_rx = read_limit_curve(extra, "limitRotationXCurve", doc, label)?;
    let limit_ry = read_limit_curve(extra, "limitRotationYCurve", doc, label)?;
    let limit_rz = read_limit_curve(extra, "limitRotationZCurve", doc, label)?;
    let is_animated = extra_bool(extra, "isAnimated", doc, label)?;
    let grabbing_raw = extra
        .get("allowGrabbing")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    let posing_raw = extra
        .get("allowPosing")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    let allow_grabbing = grabbing_raw != 0;
    let allow_posing = posing_raw != 0;
    // stretchの有無。値またはカーブのキーがあれば記録対象である。
    let mut has_stretch = false;
    if let Some(v) = extra
        .get("stretchMotion")
        .and_then(|v| v.as_f64().or_else(|| v.as_i64().map(|n| n as f64)))
        && v != 0.0
    {
        has_stretch = true;
    }
    if let Some(Value::Mapping(m)) = extra.get("stretchMotionCurve")
        && let Some(Value::Sequence(seq)) = m.get("m_Curve")
        && !seq.is_empty()
    {
        has_stretch = true;
    }
    Ok(CollectedBone {
        order: mono.order,
        component_node,
        root_node,
        enabled: mono.enabled,
        active_chain: true,
        version,
        integration,
        ignore_nodes,
        ignore_other,
        endpoint,
        multi_child,
        pull,
        spring,
        stiffness,
        gravity,
        gravity_falloff,
        immobile_type,
        immobile,
        radius,
        allow_collision,
        collider_docs,
        limit_type,
        max_angle_x,
        max_angle_z,
        limit_rotation,
        limit_rx,
        limit_ry,
        limit_rz,
        is_animated,
        allow_grabbing,
        allow_posing,
        has_stretch,
    })
}

/// MonoBehaviourのextraからf64を読む。欠落時は失敗する。
#[cfg(not(target_arch = "wasm32"))]
fn extra_f64(
    extra: &yaml_serde::Mapping,
    key: &str,
    doc: FileId,
    label: &str,
) -> Result<f64, VrmError> {
    let value = extra.get(key).ok_or_else(|| {
        VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!(
                "欄がない: {key}（文書{doc}）（{}）",
                crate::escape_diag(label)
            ),
        )
    })?;
    read_f64_value(value, key, label)
}

/// extraからbool（0/1）を読む。
#[cfg(not(target_arch = "wasm32"))]
fn extra_bool(
    extra: &yaml_serde::Mapping,
    key: &str,
    doc: FileId,
    label: &str,
) -> Result<bool, VrmError> {
    let value = extra.get(key).ok_or_else(|| {
        VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!(
                "欄がない: {key}（文書{doc}）（{}）",
                crate::escape_diag(label)
            ),
        )
    })?;
    read_bool_value(value, key, label)
}

/// extraから `{x, y, z}` を読む。
#[cfg(not(target_arch = "wasm32"))]
fn extra_vec3(
    extra: &yaml_serde::Mapping,
    key: &str,
    doc: FileId,
    label: &str,
) -> Result<[f64; 3], VrmError> {
    let value = extra.get(key).ok_or_else(|| {
        VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!(
                "欄がない: {key}（文書{doc}）（{}）",
                crate::escape_diag(label)
            ),
        )
    })?;
    read_xyz_value(value, key, label)
}

/// AnimationCurveをsidecar形式で読む。
#[cfg(not(target_arch = "wasm32"))]
fn read_curve(
    extra: &yaml_serde::Mapping,
    value_key: &str,
    curve_key: &str,
    doc: FileId,
    label: &str,
) -> Result<CollectedCurve, VrmError> {
    let value = extra_f64(extra, value_key, doc, label)?;
    read_curve_body(extra, curve_key, value, doc, label)
}

/// 制限回転の軸別カーブを読む。値は既定の1である。
#[cfg(not(target_arch = "wasm32"))]
fn read_limit_curve(
    extra: &yaml_serde::Mapping,
    curve_key: &str,
    doc: FileId,
    label: &str,
) -> Result<CollectedCurve, VrmError> {
    read_curve_body(extra, curve_key, 1.0, doc, label)
}

/// AnimationCurve本体を読む。Prefabの2（Loop）は8（ClampForever）に読み替える。
#[cfg(not(target_arch = "wasm32"))]
fn read_curve_body(
    extra: &yaml_serde::Mapping,
    curve_key: &str,
    value: f64,
    doc: FileId,
    label: &str,
) -> Result<CollectedCurve, VrmError> {
    let curve_value = extra.get(curve_key).ok_or_else(|| {
        VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!(
                "欄がない: {curve_key}（文書{doc}）（{}）",
                crate::escape_diag(label)
            ),
        )
    })?;
    let mapping = curve_value.as_mapping().ok_or_else(|| {
        VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!(
                "{curve_key}が写像でない（文書{doc}）（{}）",
                crate::escape_diag(label)
            ),
        )
    })?;
    let pre_raw = mapping
        .get("m_PreInfinity")
        .and_then(|v| v.as_i64())
        .unwrap_or(8);
    let post_raw = mapping
        .get("m_PostInfinity")
        .and_then(|v| v.as_i64())
        .unwrap_or(8);
    fn map_wrap(raw: i64, doc: FileId, label: &str) -> Result<u32, VrmError> {
        let mapped = if raw == 2 { 8 } else { raw };
        match mapped {
            0 | 1 | 2 | 4 | 8 => Ok(mapped as u32),
            _ => Err(VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!(
                    "カーブのwrapが未対応である: {mapped}（文書{doc}）（{}）",
                    crate::escape_diag(label)
                ),
            )),
        }
    }
    let pre_wrap = map_wrap(pre_raw, doc, label)?;
    let post_wrap = map_wrap(post_raw, doc, label)?;
    let mut keys = Vec::new();
    if let Some(Value::Sequence(seq)) = mapping.get("m_Curve") {
        for (i, item) in seq.iter().enumerate() {
            let km = item.as_mapping().ok_or_else(|| {
                VrmError::new(
                    VrmErrorKind::InvalidPrefab,
                    format!("カーブキーが写像でない: {curve_key}[{i}]（文書{doc}）"),
                )
            })?;
            let get = |k: &str| {
                km.get(k)
                    .and_then(|v| v.as_f64().or_else(|| v.as_i64().map(|n| n as f64)))
                    .map(quantize_f32)
                    .ok_or_else(|| {
                        VrmError::new(
                            VrmErrorKind::InvalidPrefab,
                            format!("カーブキーに{k}がない: {curve_key}[{i}]（文書{doc}）"),
                        )
                    })
            };
            let weighted = km.get("weightedMode").and_then(|v| v.as_i64()).unwrap_or(0);
            if weighted != 0 {
                return Err(VrmError::new(
                    VrmErrorKind::InvalidPrefab,
                    format!("重み付きカーブは未対応である: {curve_key}[{i}]（文書{doc}）"),
                ));
            }
            keys.push(CollectedKey {
                time: get("time")?,
                value: get("value")?,
                in_slope: get("inSlope")?,
                out_slope: get("outSlope")?,
                in_weight: get("inWeight")?,
                out_weight: get("outWeight")?,
            });
        }
    }
    Ok(CollectedCurve {
        value,
        keys,
        pre_wrap,
        post_wrap,
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::too_many_arguments)]
fn emit_sidecar(
    input: &PrefabDeriveInput<'_>,
    tree: &[TreeNode],
    locals: &[ReferenceLocal],
    collector: &Collected,
    meta: &AssetMeta,
    sources: &[SourceModelEntry],
    model_id: &str,
    bind: &BindSelection,
    label: &str,
    unmatched: &[String],
) -> Result<DerivedSidecar, VrmError> {
    // ノードIDの付番。DFS順に `n:N` である。
    let node_id_of = |index: usize| format!("n:{index}");
    let avatar_name = avatar_name_from_label(label);
    // ノード列の組み立て。
    let mut nodes_json = Vec::with_capacity(tree.len());
    for (i, node) in tree.iter().enumerate() {
        let (pos, rot, scale, ref_source) = locals[i];
        let parent_id = node.parent.map(node_id_of);
        // siblingIndexは親の子列での位置である。ルートは全体の先頭からの位置である。
        let sibling_index = match node.parent {
            Some(parent) => tree[parent]
                .children
                .iter()
                .position(|c| *c == i)
                .unwrap_or(0) as u32,
            None => {
                // ルートの順序はtree順である。
                tree.iter()
                    .enumerate()
                    .filter(|(_, n)| n.parent.is_none())
                    .position(|(idx, _)| idx == i)
                    .unwrap_or(0) as u32
            }
        };
        let source_json = match node.source.as_ref() {
            Some(source) => {
                serde_json::json!({
                    "modelId": source.model_id,
                    "path": source.path,
                    "assetGuid": source.asset_guid,
                    "localId": source.local_id,
                })
            }
            None => serde_json::Value::Null,
        };
        nodes_json.push(serde_json::json!({
            "id": node_id_of(i),
            "parentId": parent_id,
            "name": node.name,
            "siblingIndex": sibling_index,
            "scenePath": node.scene_path,
            "source": source_json,
            "referenceSource": ref_source,
            "referenceLocal": {
                "position": pos,
                "rotation": rot,
                "scale": scale,
            },
        }));
    }
    // PhysBone列の組み立て。IDは `pb:N` 体系であり、収集順（DFS→文書順）である。
    // collider参照は文書fileIDから `c:N` に写す。
    let mut collider_doc_to_id: HashMap<FileId, String> = HashMap::new();
    for (i, collider) in collector.colliders.iter().enumerate() {
        collider_doc_to_id.insert(collider.doc, format!("c:{i}"));
    }
    let mut bones_json = Vec::with_capacity(collector.phys_bones.len());
    let mut unsupported: Vec<String> = Vec::new();
    // TRSに戻らなかった参照ローカルは申告する。Unityヘルパと同順である。
    for (i, (_, _, _, ref_source)) in locals.iter().enumerate() {
        if *ref_source == "bind-decomposed-approx" {
            unsupported.push(format!("trs-decomposition:n:{i}"));
        }
    }
    for (i, bone) in collector.phys_bones.iter().enumerate() {
        let id = format!("pb:{i}");
        let component_node_id = node_id_of(bone.component_node);
        let root_node_id = node_id_of(bone.root_node);
        let ignore_ids: Vec<String> = bone.ignore_nodes.iter().map(|n| node_id_of(*n)).collect();
        let collider_ids: Vec<String> = bone
            .collider_docs
            .iter()
            .filter_map(|doc| collider_doc_to_id.get(doc).cloned())
            .collect();
        // 外部colliderの参照は記録し、ID列には含めない（Unityヘルパと同順）。
        if collider_ids.len() != bone.collider_docs.len() {
            unsupported.push(format!("external-colliders:{id}"));
        }
        // rootがアバター外の場合は自ノードに読み替えて記録する。
        // （収集時に読み替え済みのため、ここでは検出しない。）
        let _ = &root_node_id;
        bones_json.push(serde_json::json!({
            "id": id,
            "componentNodeId": component_node_id,
            "rootNodeId": root_node_id,
            "enabled": bone.enabled,
            "version": bone.version,
            "integrationType": bone.integration,
            "ignoreNodeIds": ignore_ids,
            "ignoreOtherPhysBones": bone.ignore_other,
            "endpointPosition": bone.endpoint,
            "multiChildType": bone.multi_child,
            "pull": curve_json(&bone.pull),
            "spring": curve_json(&bone.spring),
            "stiffness": curve_json(&bone.stiffness),
            "gravity": curve_json(&bone.gravity),
            "gravityFalloff": curve_json(&bone.gravity_falloff),
            "immobileType": bone.immobile_type,
            "immobile": curve_json(&bone.immobile),
            "radius": curve_json(&bone.radius),
            "allowCollision": bone.allow_collision,
            "colliderIds": collider_ids,
            "limitType": bone.limit_type,
            "maxAngleX": curve_json(&bone.max_angle_x),
            "maxAngleZ": curve_json(&bone.max_angle_z),
            "limitRotation": bone.limit_rotation,
            "limitRotationXCurve": curve_json(&bone.limit_rx),
            "limitRotationYCurve": curve_json(&bone.limit_ry),
            "limitRotationZCurve": curve_json(&bone.limit_rz),
            "isAnimated": bone.is_animated,
        }));
        if bone.allow_grabbing {
            unsupported.push(format!("allow-grabbing:{id}:True"));
        }
        if bone.allow_posing {
            unsupported.push(format!("allow-posing:{id}:True"));
        }
        if bone.has_stretch {
            unsupported.push(format!("stretch-motion:{id}"));
        }
    }
    // collider列の組み立て。IDは `c:N` 体系である。
    let mut colliders_json = Vec::with_capacity(collector.colliders.len());
    for (i, collider) in collector.colliders.iter().enumerate() {
        let id = format!("c:{i}");
        colliders_json.push(serde_json::json!({
            "id": id,
            "componentNodeId": node_id_of(collider.component_node),
            "rootNodeId": node_id_of(collider.root_node),
            "enabled": collider.enabled,
            "shapeType": collider.shape,
            "insideBounds": collider.inside,
            "radius": collider.radius,
            "height": collider.height,
            "position": collider.position,
            "rotation": collider.rotation,
            "bonesAsSpheres": collider.bones_as_spheres,
        }));
        if collider.shape == "Plane" && !unsupported.iter().any(|s| s == "plane-colliders") {
            unsupported.push("plane-colliders".to_owned());
        }
    }
    // 制約列の組み立て。IDは `vc:`・`uc:` + 現場パス + `#` + 順序である。
    // 由来（VRC・Unity）で接頭辞を分ける。Unityヘルパと同順である。
    let mut constraints_json = Vec::with_capacity(collector.constraints.len());
    for constraint in &collector.constraints {
        let prefix = if constraint.family == "unity" {
            "uc"
        } else {
            "vc"
        };
        let id = format!(
            "{}:{}#{}",
            prefix,
            constraint.scene_path.join("/"),
            constraint.order_index
        );
        let sources_json: Vec<serde_json::Value> = constraint
            .sources
            .iter()
            .map(|(node, weight)| {
                if *node == usize::MAX {
                    serde_json::json!({
                        "nodeId": "",
                        "external": true,
                        "weight": *weight,
                    })
                } else {
                    serde_json::json!({
                        "nodeId": node_id_of(*node),
                        "external": false,
                        "weight": *weight,
                    })
                }
            })
            .collect();
        constraints_json.push(serde_json::json!({
            "id": id,
            "nodeId": node_id_of(constraint.component_node),
            "targetNodeId": node_id_of(constraint.target_node),
            "type": constraint.constraint_type,
            "family": constraint.family,
            "weight": constraint.weight,
            "enabled": constraint.enabled,
            "active": constraint.active,
            "freezeToWorld": constraint.freeze,
            "affectsRotation": constraint.affects,
            "aimAxis": constraint.aim_axis,
            "sources": sources_json,
        }));
    }
    // 視線の組み立て。descriptorがなければ既定（無効・恒等）である。
    let look_at_json = match collector.descriptor.as_ref() {
        Some(descriptor) => {
            let left_id = descriptor.left_eye.map(node_id_of).unwrap_or_default();
            let right_id = descriptor.right_eye.map(node_id_of).unwrap_or_default();
            let eye_json = |pose: &([bool; 1], [f64; 4], [f64; 4])| {
                serde_json::json!({
                    "linked": pose.0[0],
                    "left": pose.1,
                    "right": pose.2,
                })
            };
            serde_json::json!({
                "eyeLook": descriptor.enable_eye_look,
                "leftEyeNodeId": left_id,
                "rightEyeNodeId": right_id,
                "eyesLookingStraight": eye_json(&descriptor.eyes.straight),
                "eyesLookingUp": eye_json(&descriptor.eyes.up),
                "eyesLookingDown": eye_json(&descriptor.eyes.down),
                "eyesLookingLeft": eye_json(&descriptor.eyes.left),
                "eyesLookingRight": eye_json(&descriptor.eyes.right),
            })
        }
        None => {
            let identity = serde_json::json!({
                "linked": false,
                "left": [0.0, 0.0, 0.0, 1.0],
                "right": [0.0, 0.0, 0.0, 1.0],
            });
            serde_json::json!({
                "eyeLook": false,
                "leftEyeNodeId": "",
                "rightEyeNodeId": "",
                "eyesLookingStraight": identity,
                "eyesLookingUp": identity,
                "eyesLookingDown": identity,
                "eyesLookingLeft": identity,
                "eyesLookingRight": identity,
            })
        }
    };
    let _ = input;
    // humanoidは.metaから組み立てる。
    let humanoid_json = build_humanoid_json(meta, tree, model_id, label)?;
    // sourceModelsは資産パス順に並べる。
    // Unityの `SortedDictionary` と同じ決定性規則である。
    let mut source_models_sorted: Vec<&SourceModelEntry> = sources.iter().collect();
    source_models_sorted.sort_by(|a, b| a.asset_path.cmp(&b.asset_path));
    let source_models_json: Vec<serde_json::Value> = source_models_sorted
        .iter()
        .map(|source| {
            serde_json::json!({
                "id": source.id,
                "sha256": source.sha256,
                "assetGuid": source.asset_guid,
                "assetPath": source.asset_path,
            })
        })
        .collect();
    // captureはbind-pose相当である。
    let root_scale = locals
        .first()
        .map(|(_, _, s, _)| *s)
        .unwrap_or([1.0, 1.0, 1.0]);
    // bindPoseSourceはavatar名 + 参照メッシュ名である。
    let bind_pose_source = vec![avatar_name.clone(), bind.source_mesh_name.clone()];
    // unsupportedFeaturesは決定的順序（バイト順）で並べる。
    let mut unsupported_sorted = unsupported.clone();
    unsupported_sorted.sort();
    unsupported_sorted.dedup();
    // 最終JSONの組み立て。DTOのフィールド順に並べる。
    let mut sidecar_json = serde_json::json!({
        "format": "vrctom.physics",
        "schemaVersion": 1,
        "generator": {
            "name": "vrctom prefab-derive",
            "version": "1",
        },
        "capture": {
            "statePolicy": "bind-pose",
            "coordinateSpace": "unity-avatar-root-local-meters",
            "avatarRootId": "n:0",
            "rootScale": root_scale,
            "bindPoseSource": bind_pose_source,
        },
        "sourceModels": source_models_json,
        "nodes": nodes_json,
        "humanoid": humanoid_json,
        "physBones": bones_json,
        "colliders": colliders_json,
        "constraints": constraints_json,
        "unsupportedFeatures": unsupported_sorted,
        "lookAt": look_at_json,
    });
    // `-0.0` を `+0.0` に正規化する。Unityの出力に `-0.0` は現れない。
    normalize_neg_zero(&mut sidecar_json);
    // 検証を通してから返す。検証済みDTOの不変条件を確認する。
    let bytes = serde_json::to_vec_pretty(&sidecar_json).map_err(|e| {
        VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!("sidecarのJSON化に失敗した: {e}"),
        )
    })?;
    // 自分の出力を検証器に通す。不変条件を満たさなければここで失敗する。
    {
        use super::physics::{PhysicsLimits, PhysicsSidecar};
        PhysicsSidecar::parse(&bytes, &PhysicsLimits::new()).map_err(|e| {
            VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!("導出したsidecarが検証を通らない: {e}"),
            )
        })?;
    }
    let mut ignored_sorted = collector.ignored.clone();
    ignored_sorted.sort();
    ignored_sorted.dedup();
    let mut unmatched_sorted = unmatched.to_vec();
    unmatched_sorted.sort();
    unmatched_sorted.dedup();
    Ok(DerivedSidecar {
        json: bytes,
        ignored_types: ignored_sorted,
        unmatched_nodes: unmatched_sorted,
    })
}

/// humanoidのT-poseを.metaから組み立てる。
#[cfg(not(target_arch = "wasm32"))]
fn build_humanoid_json(
    meta: &AssetMeta,
    tree: &[TreeNode],
    model_id_main: &str,
    label: &str,
) -> Result<serde_json::Value, VrmError> {
    // humanDescriptionのhumanとskeletonを読む。
    let settings = meta.importer().settings.as_mapping().ok_or_else(|| {
        VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!(
                "importerの設定が写像でない（{}）",
                crate::escape_diag(label)
            ),
        )
    })?;
    let human_desc = settings
        .get("humanDescription")
        .and_then(|v| v.as_mapping());
    let mut human_map: HashMap<String, String> = HashMap::new();
    let mut skeleton_map: HashMap<String, TrsF64> = HashMap::new();
    if let Some(human_desc) = human_desc {
        if let Some(Value::Sequence(human)) = human_desc.get("human") {
            for entry in human {
                let Some(m) = entry.as_mapping() else {
                    continue;
                };
                let bone = m.get("boneName").and_then(|v| v.as_str()).unwrap_or("");
                let human_name = m.get("humanName").and_then(|v| v.as_str()).unwrap_or("");
                if !bone.is_empty() && !human_name.is_empty() {
                    // Unityの `HumanBodyBones` 名は空白を含まない（`LeftThumbProximal`）が、
                    // `.meta` の `humanName` は空白を含む（`Left Thumb Proximal`）。
                    // 空白除去で正規化して突き合わせる。重複は最初の項目を残す。
                    let normalized: String = human_name.chars().filter(|c| *c != ' ').collect();
                    human_map.entry(normalized).or_insert(bone.to_owned());
                }
            }
        }
        if let Some(Value::Sequence(skeleton)) = human_desc.get("skeleton") {
            for entry in skeleton {
                let Some(m) = entry.as_mapping() else {
                    continue;
                };
                let name = m.get("name").and_then(|v| v.as_str()).unwrap_or("");
                if name.is_empty() {
                    continue;
                }
                let position = m
                    .get("position")
                    .map(|v| read_xyz_value(v, "skeleton.position", label))
                    .transpose()?
                    .unwrap_or([0.0, 0.0, 0.0]);
                let rotation = m
                    .get("rotation")
                    .map(|v| read_xyzw_value(v, "skeleton.rotation", label))
                    .transpose()?
                    .unwrap_or([0.0, 0.0, 0.0, 1.0]);
                let scale = m
                    .get("scale")
                    .map(|v| read_xyz_value(v, "skeleton.scale", label))
                    .transpose()?
                    .unwrap_or([1.0, 1.0, 1.0]);
                skeleton_map
                    .entry(name.to_owned())
                    .or_insert((position, rotation, scale));
            }
        }
    }
    let is_human = !human_map.is_empty();
    // UnityのHumanBodyBones順に並べる。LastBoneは除く。
    const ORDER: &[&str] = &[
        "Hips",
        "LeftUpperLeg",
        "RightUpperLeg",
        "LeftLowerLeg",
        "RightLowerLeg",
        "LeftFoot",
        "RightFoot",
        "Spine",
        "Chest",
        "UpperChest",
        "Neck",
        "Head",
        "LeftShoulder",
        "RightShoulder",
        "LeftUpperArm",
        "RightUpperArm",
        "LeftLowerArm",
        "RightLowerArm",
        "LeftHand",
        "RightHand",
        "LeftToes",
        "RightToes",
        "LeftEye",
        "RightEye",
        "Jaw",
        "LeftThumbProximal",
        "LeftThumbIntermediate",
        "LeftThumbDistal",
        "LeftIndexProximal",
        "LeftIndexIntermediate",
        "LeftIndexDistal",
        "LeftMiddleProximal",
        "LeftMiddleIntermediate",
        "LeftMiddleDistal",
        "LeftRingProximal",
        "LeftRingIntermediate",
        "LeftRingDistal",
        "LeftLittleProximal",
        "LeftLittleIntermediate",
        "LeftLittleDistal",
        "RightThumbProximal",
        "RightThumbIntermediate",
        "RightThumbDistal",
        "RightIndexProximal",
        "RightIndexIntermediate",
        "RightIndexDistal",
        "RightMiddleProximal",
        "RightMiddleIntermediate",
        "RightMiddleDistal",
        "RightRingProximal",
        "RightRingIntermediate",
        "RightRingDistal",
        "RightLittleProximal",
        "RightLittleIntermediate",
        "RightLittleDistal",
    ];
    // Transform名からtree索引を引く。主FBX由来ノードだけが対象であり、
    // 副階層（同名があり得る）はAnimatorの骨解決に現れないため除く。
    let mut name_to_nodes: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, node) in tree.iter().enumerate() {
        if node
            .source
            .as_ref()
            .map(|s| s.model_id == model_id_main)
            .unwrap_or(false)
        {
            name_to_nodes.entry(node.name.as_str()).or_default().push(i);
        }
    }
    let mut bones_json = Vec::new();
    for bone in ORDER {
        let Some(transform_name) = human_map.get(*bone) else {
            continue;
        };
        let candidates = name_to_nodes.get(transform_name.as_str());
        let Some(candidates) = candidates else {
            continue;
        };
        if candidates.len() != 1 {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!("humanoidの対応が複数ある: {bone}"),
            ));
        }
        let node_index = candidates[0];
        // `.meta` の skeleton はUnity空間のため、そのまま使う。
        // 参照実装の capture と同じく Transform 名で引いた humanoid の T-pose を記録する。
        // 回転はavatar空間の姿勢であり、写像は行わない。
        let t_pose = match skeleton_map.get(transform_name.as_str()) {
            Some((p, r, s)) => serde_json::json!({
                "position": p,
                "rotation": r,
                "scale": s,
            }),
            None => serde_json::Value::Null,
        };
        bones_json.push(serde_json::json!({
            "bone": bone,
            "nodeId": format!("n:{node_index}"),
            "tPose": t_pose,
        }));
    }
    Ok(serde_json::json!({
        "isHuman": is_human,
        "bones": bones_json,
    }))
}

/// JSON内の `-0.0` を `+0.0` に正規化する。Unityの出力に `-0.0` は現れない。
/// Prefabの `-0` 表記や行列演算の符号付きゼロが対象である。
/// 呼び出し側の数値比較では区別できないが、バイト一致には効く。
#[cfg(not(target_arch = "wasm32"))]
fn normalize_neg_zero(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Number(n) => {
            if n.as_f64().is_some_and(|v| v == 0.0 && v.is_sign_negative()) {
                *value = serde_json::Value::Number(
                    serde_json::Number::from_f64(0.0).expect("0.0は表せる"),
                );
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                normalize_neg_zero(item);
            }
        }
        serde_json::Value::Object(map) => {
            for item in map.values_mut() {
                normalize_neg_zero(item);
            }
        }
        _ => {}
    }
}

/// カーブをsidecar JSON形式にする。
#[cfg(not(target_arch = "wasm32"))]
fn curve_json(curve: &CollectedCurve) -> serde_json::Value {
    let keys: Vec<serde_json::Value> = curve
        .keys
        .iter()
        .map(|k| {
            serde_json::json!({
                "time": k.time,
                "value": k.value,
                "inSlope": k.in_slope,
                "outSlope": k.out_slope,
                "weightedMode": 0,
                "inWeight": k.in_weight,
                "outWeight": k.out_weight,
            })
        })
        .collect();
    serde_json::json!({
        "value": curve.value,
        "keys": keys,
        "preWrapMode": curve.pre_wrap,
        "postWrapMode": curve.post_wrap,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn md4の既知ベクタと一致する() {
        assert_eq!(md4_hex(b""), MD4_EMPTY);
    }

    #[test]
    fn スクリプトfile_idの実測6組と一致する() {
        assert_eq!(
            script_file_id("VRC.SDK3.Dynamics.PhysBone.Components", "VRCPhysBone"),
            1661641543
        );
        assert_eq!(
            script_file_id(
                "VRC.SDK3.Dynamics.PhysBone.Components",
                "VRCPhysBoneCollider"
            ),
            -1631200402
        );
        assert_eq!(
            script_file_id(
                "VRC.SDK3.Dynamics.Constraint.Components",
                "VRCRotationConstraint"
            ),
            1788371120
        );
        assert_eq!(
            script_file_id("VRC.SDK3.Avatars.Components", "VRCAvatarDescriptor"),
            542108242
        );
        assert_eq!(
            script_file_id("VRC.SDK3.Avatars.Components", "VRCHeadChop"),
            -1888410255
        );
        assert_eq!(
            script_file_id("VRC.SDK3.Dynamics.Contact.Components", "VRCContactReceiver"),
            -1450912254
        );
    }

    #[test]
    fn 既知スクリプトを判別できる() {
        assert_eq!(
            judge_script("2a2c05204084d904aa4945ccff20d8e5", 1661641543),
            ScriptJudgment::Known(KnownScript::PhysBone)
        );
        assert_eq!(
            judge_script("2a2c05204084d904aa4945ccff20d8e5", -1631200402),
            ScriptJudgment::Known(KnownScript::PhysBoneCollider)
        );
        assert_eq!(
            judge_script("9ef94258554e06b4ab03fbe9fc914c5c", 123),
            ScriptJudgment::External
        );
        assert_eq!(
            judge_script("2a2c05204084d904aa4945ccff20d8e5", 1),
            ScriptJudgment::UnknownDll
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn カーブの再現は実データと一致する() {
        // Milfy.prefabのBreast用radiusCurve（2キー）を読み、sidecarのpb:8と突き合わせる。
        // 値・キー・傾き・重み・wrap（Prefabの2→8）を確かめる。
        use yaml_serde::Mapping;
        let mut curve = Mapping::new();
        curve.insert("m_PreInfinity".into(), yaml_serde::Value::Number(2.into()));
        curve.insert("m_PostInfinity".into(), yaml_serde::Value::Number(2.into()));
        let key = |time: f64,
                   value: f64,
                   in_slope: f64,
                   out_slope: f64,
                   in_weight: f64,
                   out_weight: f64| {
            let mut m = Mapping::new();
            m.insert(
                "time".into(),
                yaml_serde::Value::Number(yaml_serde::Number::from(time)),
            );
            m.insert(
                "value".into(),
                yaml_serde::Value::Number(yaml_serde::Number::from(value)),
            );
            m.insert(
                "inSlope".into(),
                yaml_serde::Value::Number(yaml_serde::Number::from(in_slope)),
            );
            m.insert(
                "outSlope".into(),
                yaml_serde::Value::Number(yaml_serde::Number::from(out_slope)),
            );
            m.insert("tangentMode".into(), yaml_serde::Value::Number(0.into()));
            m.insert("weightedMode".into(), yaml_serde::Value::Number(0.into()));
            m.insert(
                "inWeight".into(),
                yaml_serde::Value::Number(yaml_serde::Number::from(in_weight)),
            );
            m.insert(
                "outWeight".into(),
                yaml_serde::Value::Number(yaml_serde::Number::from(out_weight)),
            );
            yaml_serde::Value::Mapping(m)
        };
        curve.insert(
            "m_Curve".into(),
            yaml_serde::Value::Sequence(vec![
                key(0.0, 0.28066117, 0.50450784, 0.50450784, 0.0, 0.19247137),
                key(0.9958191, 1.0, 0.43670022, 0.43670022, 0.14643128, 0.0),
            ]),
        );
        let mut extra = Mapping::new();
        extra.insert(
            "radius".into(),
            yaml_serde::Value::Number(yaml_serde::Number::from(0.025)),
        );
        extra.insert("radiusCurve".into(), yaml_serde::Value::Mapping(curve));
        let curve = read_curve(&extra, "radius", "radiusCurve", 1, "test")
            .expect("実データのカーブは読める");
        // Prefab値はUnityのfloat32として量子化される。`0.025` はf32で誤差を持つ。
        let want_value = quantize_f32(0.025);
        assert!((curve.value - want_value).abs() < 1e-12);
        assert_eq!(curve.pre_wrap, 8);
        assert_eq!(curve.post_wrap, 8);
        assert_eq!(curve.keys.len(), 2);
        assert!((curve.keys[0].time - 0.0).abs() < 1e-12);
        assert!((curve.keys[0].value - quantize_f32(0.28066117)).abs() < 1e-12);
        assert!((curve.keys[0].out_weight - quantize_f32(0.19247137)).abs() < 1e-12);
        assert!((curve.keys[1].time - quantize_f32(0.9958191)).abs() < 1e-12);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn 行列の分解は単位と鏡像を保つ() {
        // 単位行列はそのまま戻る。
        let identity = f32_identity();
        let (p, r, s) = decompose_bind_local_f32(&identity).expect("単位行列は分解できる");
        assert_eq!(p, [0.0, 0.0, 0.0]);
        assert_eq!(s, [1.0, 1.0, 1.0]);
        assert!((r[3] - 1.0).abs() < 1e-6);
        // X鏡像はX軸に寄せ、回転部は固有回転に戻る。
        let mut mirrored = f32_identity();
        mirrored[0] = -2.0;
        let (p, r, s) = decompose_bind_local_f32(&mirrored).expect("鏡像は分解できる");
        assert_eq!(p, [0.0, 0.0, 0.0]);
        assert!((s[0] + 2.0).abs() < 1e-6);
        assert_eq!([s[1], s[2]], [1.0, 1.0]);
        let _ = r;
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn 未知の_dll型は明示的に失敗する() {
        // VRChatのDLL GUIDだが既知クラスと一致しないfileIDは、黙って無視せず失敗させる。
        // 呼び出し側の導出はこの判定で打ち切る。
        assert_eq!(
            judge_script("2a2c05204084d904aa4945ccff20d8e5", 999_999_999),
            ScriptJudgment::UnknownDll
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn 参照にfile_idが無ければ失敗する() {
        // Prefabの参照解決はfileID必須であり、欠落は参照切れとして失敗させる。
        let mut mapping = yaml_serde::Mapping::new();
        mapping.insert(
            "guid".into(),
            yaml_serde::Value::String("9ef94258554e06b4ab03fbe9fc914c5c".to_owned()),
        );
        let value = yaml_serde::Value::Mapping(mapping);
        assert!(parse_ref_value(&value).is_err());
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn 旧取り込み方式は明示的に失敗する() {
        // 2019.3より前のFBX取り込み（fileIdsGeneration=1）は旧fileID体系のため未対応として失敗させる。
        // 最小の.metaから判定できる。
        let meta_bytes = b"fileFormatVersion: 2\nguid: 9ef94258554e06b4ab03fbe9fc914c5c\nModelImporter:\n  fileIdsGeneration: 1\n";
        let meta = crate::unity::AssetMeta::parse(meta_bytes).expect("最小の.metaは読める");
        assert!(check_file_ids_generation(&meta, "test").is_err());
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn 回転の抽出は鏡像の適用順序で符号が定まる() {
        // 単位行列は恒等四元数に戻る。
        let q = quat_from_rotation_matrix_f32([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);
        assert!((q[3] - 1.0).abs() < 1e-7);
        assert_eq!([q[0], q[1], q[2]], [0.0, 0.0, 0.0]);
        // X軸90度回転は正符号の四元数に戻る。
        // 列は `[[1,0,0],[0,0,1],[0,-1,0]]` である。
        let q = quat_from_rotation_matrix_f32([[1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, -1.0, 0.0]]);
        assert!((q[0] - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6);
        assert!((q[3] - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6);
        // 実データ（Breast.R.001相当）では、FBX空間で分解してから写すと符号が反転する。
        // 行列を先にUnity空間へ写してから分解すると、Unityの符号と一致する。
        let truth = [0.512_978_85_f32, 0.145_400_26, 0.834_192_4, -0.140_835];
        let rebuilt = compose_trs_f32([0.0, 0.0, 0.0], truth, [1.0, 1.0, 1.0]);
        // FBX空間の行列に戻す（Unity空間の鏡像を外す）。
        let mirrored_back = mirror_matrix_f32(&rebuilt);
        let cols = [
            [mirrored_back[0], mirrored_back[1], mirrored_back[2]],
            [mirrored_back[4], mirrored_back[5], mirrored_back[6]],
            [mirrored_back[8], mirrored_back[9], mirrored_back[10]],
        ];
        let q = quat_from_rotation_matrix_f32(cols);
        // FBX空間の抽出を写すと、Unityの符号とは反対になる。
        let mapped = [q[0], -q[1], -q[2], q[3]];
        let dot_wrong = mapped[0] * truth[0]
            + mapped[1] * truth[1]
            + mapped[2] * truth[2]
            + mapped[3] * truth[3];
        assert!(dot_wrong < 0.0, "dot_wrong={dot_wrong}");
        // 行列を先に写してから抽出すると、Unityの符号と一致する。
        let cols = [
            [rebuilt[0], rebuilt[1], rebuilt[2]],
            [rebuilt[4], rebuilt[5], rebuilt[6]],
            [rebuilt[8], rebuilt[9], rebuilt[10]],
        ];
        let q = quat_from_rotation_matrix_f32(cols);
        let dot = q[0] * truth[0] + q[1] * truth[1] + q[2] * truth[2] + q[3] * truth[3];
        assert!(dot > 0.999_999_9, "dot={dot}");
        for i in 0..4 {
            assert!((q[i] - truth[i]).abs() < 1e-6, "q={q:?} truth={truth:?}");
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn 表示名の両端の引用符を落として幹を求める() {
        // CLIは表示名を `format!("{path:?}")` で作るため `"` が付く。
        assert_eq!(avatar_name_from_label("\"path/to/Milfy.prefab\""), "Milfy");
        assert_eq!(avatar_name_from_label("Milfy.prefab"), "Milfy");
        assert_eq!(avatar_name_from_label("Milfy"), "Milfy");
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn 物理に効く改変欄を判定できる() {
        assert!(is_physics_path("m_LocalPosition.x"));
        assert!(is_physics_path("m_LocalRotation.w"));
        assert!(is_physics_path("m_LocalScale.y"));
        assert!(is_physics_path("m_Name"));
        assert!(is_physics_path("m_IsActive"));
        assert!(is_physics_path("m_Enabled"));
        assert!(is_physics_path("Sources.source0.SourceTransform"));
        assert!(!is_physics_path("m_LocalEulerAnglesHint.x"));
        assert!(!is_physics_path("m_RootBone"));
        assert!(!is_physics_path("m_AABB.m_Center.x"));
        assert_eq!(
            model_id_from_asset_path("Assets/PLUSONE/Milfy/FBX/Milfy.fbx"),
            "milfy"
        );
        assert_eq!(
            model_id_from_asset_path("Assets/PLUSONE/Milfy/FBX/SmartPhone.fbx"),
            "smartphone"
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn 主階層の姿勢上書きは集計し対象不明は失敗する() {
        // 取り込み根の初期化は想定内であり通す。
        // 骨Transformへの `m_LocalPosition` 上書きは集計し、対象不明は失敗させる。
        let main_guid = "9ef94258554e06b4ab03fbe9fc914c5c";
        let bone_fid = transform_file_id("Armature/Hips");
        let hier = FbxHierarchy {
            paths: vec!["Armature".to_owned(), "Armature/Hips".to_owned()],
            children_sorted: vec![vec![1], vec![]],
            go_fid_at: vec![
                crate::unity::game_object_file_id("Armature"),
                crate::unity::game_object_file_id("Armature/Hips"),
            ],
            tr_fid_at: vec![transform_file_id("Armature"), bone_fid],
            path_at: HashMap::new(),
        };
        let bone_mod = PrefabMod {
            target_fid: bone_fid,
            target_guid: Some(main_guid.to_owned()),
            path: "m_LocalPosition.x".to_owned(),
            value: Some(yaml_serde::Value::Number(1.into())),
            obj_ref: None,
        };
        let root_mod = PrefabMod {
            target_fid: transform_file_id(""),
            target_guid: Some(main_guid.to_owned()),
            path: "m_LocalPosition.x".to_owned(),
            value: Some(yaml_serde::Value::Number(0.into())),
            obj_ref: None,
        };
        let main = ParsedInstance {
            doc: 1,
            source_guid: main_guid.to_owned(),
            parent: 0,
            modifications: vec![root_mod, bone_mod],
            added_objects: vec![],
        };
        let parsed = ParsedPrefab {
            monos: vec![],
            smrs: vec![],
            native_rotations: vec![],
            stripped_go: HashMap::new(),
            stripped_tr: HashMap::new(),
            local_go: HashMap::new(),
            local_tr: HashMap::new(),
            instances: vec![main],
        };
        let overlays = collect_main_instance_overlays(&parsed, &hier, &None, main_guid, "test")
            .expect("取り込み根と既知の骨は通る");
        let entry = overlays.trs.get(&bone_fid).expect("骨の上書きを集計する");
        assert_eq!(entry.position[0], Some(1.0));
        assert_eq!(entry.position[1], None);
        // 対象不明のfileIDへの上書きは失敗させる。
        let unknown_mod = PrefabMod {
            target_fid: 123,
            target_guid: Some(main_guid.to_owned()),
            path: "m_LocalPosition.x".to_owned(),
            value: Some(yaml_serde::Value::Number(1.into())),
            obj_ref: None,
        };
        let bad = ParsedInstance {
            doc: 1,
            source_guid: main_guid.to_owned(),
            parent: 0,
            modifications: vec![unknown_mod],
            added_objects: vec![],
        };
        let parsed = ParsedPrefab {
            monos: vec![],
            smrs: vec![],
            native_rotations: vec![],
            stripped_go: HashMap::new(),
            stripped_tr: HashMap::new(),
            local_go: HashMap::new(),
            local_tr: HashMap::new(),
            instances: vec![bad],
        };
        assert!(
            collect_main_instance_overlays(&parsed, &hier, &None, main_guid, "test").is_err(),
            "対象不明の上書きは失敗する"
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn 制約の参照先の重ね合わせは入れ物の不足で失敗する() {
        // 存在しない `source9` への差し替えは黙って足さず失敗させる。
        let extra = yaml_serde::Mapping::new();
        let mods = vec![PrefabMod {
            target_fid: 7,
            target_guid: None,
            path: "Sources.source9.SourceTransform".to_owned(),
            value: None,
            obj_ref: Some(ParsedRef {
                fid: 437313472444593835,
                guid: None,
            }),
        }];
        assert!(overlay_constraint_sources(&extra, &mods, "test").is_err());
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn 制約の参照先の重ね合わせは変換と重みを差し替える() {
        // `source0` の参照と重みを差し替え、他は保つ。
        let mut slot = yaml_serde::Mapping::new();
        slot.insert(
            yaml_serde::Value::String("SourceTransform".to_owned()),
            yaml_serde::Value::Mapping({
                let mut m = yaml_serde::Mapping::new();
                m.insert(
                    yaml_serde::Value::String("fileID".to_owned()),
                    yaml_serde::Value::Number(0.into()),
                );
                m
            }),
        );
        slot.insert(
            yaml_serde::Value::String("Weight".to_owned()),
            yaml_serde::Value::Number(yaml_serde::Number::from(1.0)),
        );
        let mut extra = yaml_serde::Mapping::new();
        extra.insert(
            yaml_serde::Value::String("Sources".to_owned()),
            yaml_serde::Value::Mapping({
                let mut m = yaml_serde::Mapping::new();
                m.insert(
                    yaml_serde::Value::String("source0".to_owned()),
                    yaml_serde::Value::Mapping(slot),
                );
                m
            }),
        );
        let mods = vec![
            PrefabMod {
                target_fid: 7,
                target_guid: None,
                path: "Sources.source0.SourceTransform".to_owned(),
                value: None,
                obj_ref: Some(ParsedRef {
                    fid: 437313472444593835,
                    guid: None,
                }),
            },
            PrefabMod {
                target_fid: 7,
                target_guid: None,
                path: "Sources.source0.Weight".to_owned(),
                value: Some(yaml_serde::Value::Number(yaml_serde::Number::from(0.5))),
                obj_ref: None,
            },
        ];
        let patched = overlay_constraint_sources(&extra, &mods, "test").expect("差し替えできる");
        let sources = patched
            .get("Sources")
            .and_then(|v| v.as_mapping())
            .expect("Sourcesは残る");
        let slot = sources
            .get("source0")
            .and_then(|v| v.as_mapping())
            .expect("source0は残る");
        let fid = slot
            .get("SourceTransform")
            .and_then(|v| v.as_mapping())
            .and_then(|m| m.get("fileID"))
            .and_then(|v| v.as_i64())
            .expect("参照が残る");
        assert_eq!(fid, 437313472444593835);
        let weight = slot
            .get("Weight")
            .and_then(|v| v.as_f64().or_else(|| v.as_i64().map(|n| n as f64)))
            .expect("重みが残る");
        assert!((weight - 0.5).abs() < 1e-12);
        // 欄名の誤りは失敗させる。
        let bad = vec![PrefabMod {
            target_fid: 7,
            target_guid: None,
            path: "Sources.source0.Foo".to_owned(),
            value: None,
            obj_ref: None,
        }];
        assert!(overlay_constraint_sources(&extra, &bad, "test").is_err());
    }
}
