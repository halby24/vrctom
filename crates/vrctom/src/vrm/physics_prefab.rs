//! Prefab・FBX・`.fbx.meta` から物理sidecarをUnityなしで導出する。
//!
//! 入口は [`derive_physics_sidecar`] であり、FBXバイト列・Assets配置・
//! Prefab文書からUnityヘルパと同一内容のsidecar JSONを組み立てる。
//! 型判定はスクリプトfileIDのMD4計算で行い、`(guid, fileID)` の表を持たない。
//! 旧方式（2019.3より前の取り込みfileID）・参照切れ・未知型は黙って落とさず失敗する。

use std::collections::{HashMap, HashSet};

use yaml_serde::Value;

use super::error::{VrmError, VrmErrorKind};
use crate::fbx::FbxFile;
use crate::unity::{AssetMeta, FileId, SerializedFile, TypedObject};

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

/// MD4の既知ベクタ（空文字）。依存の取り違えを検出する。単体テストで使う。
#[allow(dead_code)]
const MD4_EMPTY: &str = "31d6cfe0d16ae931b73c59d7e0c089c0";

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

/// スクリプトfileIDをMD4で計算する。
fn script_file_id(namespace: &str, class_name: &str) -> i32 {
    let mut input = Vec::with_capacity(4 + namespace.len() + class_name.len());
    input.extend_from_slice(&[b's', 0, 0, 0]);
    input.extend_from_slice(namespace.as_bytes());
    input.extend_from_slice(class_name.as_bytes());
    let digest = md4(&input);
    i32::from_le_bytes([digest[0], digest[1], digest[2], digest[3]])
}

/// VRChatのDLLとみなすGUID集合。
const VRCHAT_DLL_GUIDS: &[&str] = &[
    "2a2c05204084d904aa4945ccff20d8e5",
    "58e2f01a24261a14cb82e6d3399e8b16",
    "67cc4cb7839cd3741b63733d5adf0442",
    "80f1b8067b0760e4bb45023bc2e9de66",
    "4ecd63eff847044b68db9453ce219299",
];

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScriptJudgment {
    Known(KnownScript),
    UnknownDll,
    External,
}

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
    let instances = find_instances(&parsed, &fbx_guid);
    if instances.len() != 1 {
        return Err(VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!(
                "入力FBXへのPrefabInstanceが{}件あるため先勝ちにしない（{}）",
                instances.len(),
                crate::escape_diag(label),
            ),
        ));
    }
    // ネストしたKumaPhoneの解決。2件目のインスタンス（KumaPhone prefab）の
    // 実体をAssetsから読み、SmartPhone.fbxと合わせて副階層を作る。
    let phone = resolve_phone_subtree(&index, &canonical_assets, &parsed, label)?;
    // 1930年代方式ではなく、2019.3以降のXXH64方式であることを確かめる。
    // fileIDの一致で解決できない対象があれば旧方式として失敗する。
    // ノード階層の組み立て。
    let tree = assemble_tree(input.fbx, &milfy_hier, &parsed, &phone, label)?;
    // bind選択（Body優先）。
    let bind = select_bind(input.fbx, &milfy_hier, label)?;
    // 参照ローカルの計算（f32でUnityと同順）。
    let locals = compute_reference_locals(input.fbx, &tree, &bind)?;
    // 成分の判別と収集。
    let collector = collect_components(&parsed, &tree, &phone, &fbx_guid, label)?;
    // humanoid・lookAtのための.meta読み取り。
    // 索引と同じくAssets相対で引く。`Assets/` 接頭辞は落とす。
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
    // sidecar JSONの組み立て。
    emit_sidecar(
        input, &tree, &locals, &collector, &meta, &phone, &fbx_guid, &bind, label,
    )
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

// Prefab文書の型付き読み取り結果。
#[cfg(not(target_arch = "wasm32"))]
struct ParsedPrefab {
    monos: Vec<ParsedMono>,
    stripped_go: HashMap<FileId, StrippedRef>,
    stripped_tr: HashMap<FileId, StrippedRef>,
    local_go: HashMap<FileId, LocalGo>,
    local_tr: HashMap<FileId, LocalTr>,
    instances: Vec<ParsedInstance>,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone)]
#[allow(dead_code)] // スマホ graft 保留中の保持項目であり、次単位で配線する。
struct StrippedRef {
    source_fid: FileId,
    source_guid: String,
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
    position: [f64; 3],
    rotation: [f64; 4],
    scale: [f64; 3],
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone)]
struct ParsedMono {
    doc: FileId,
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
                stripped_go.insert(
                    doc.file_id(),
                    StrippedRef {
                        source_fid: r.fid,
                        source_guid: r.guid.unwrap_or_default(),
                        instance: 0,
                    },
                );
            } else if doc.class_name() == "Transform" {
                let corr = raw.get("m_CorrespondingSourceObject").ok_or_else(|| {
                    VrmError::new(VrmErrorKind::InvalidPrefab, "strippedに対応元がない")
                })?;
                let r = parse_ref_value(corr)?;
                stripped_tr.insert(
                    doc.file_id(),
                    StrippedRef {
                        source_fid: r.fid,
                        source_guid: r.guid.unwrap_or_default(),
                        instance: 0,
                    },
                );
            }
            // m_PrefabInstanceは使わない（対応付けはfileIDで行う）。
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
                    go: mono.game_object.file_id,
                    enabled: mono.enabled,
                    script_guid: guid,
                    script_fid: mono.script.file_id as i32,
                    extra: mono.extra,
                });
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
            Ok(TypedObject::Transform(tr)) => {
                local_tr.insert(
                    doc.file_id(),
                    LocalTr {
                        go: tr.game_object.file_id,
                        father: tr.father.file_id,
                        children: tr.children.iter().map(|c| c.file_id).collect(),
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
                    parent: instance
                        .extra
                        .get("m_TransformParent")
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
        stripped_go,
        stripped_tr,
        local_go,
        local_tr,
        instances,
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

// KumaPhone副階層。
#[cfg(not(target_arch = "wasm32"))]
#[allow(dead_code)] // スマホ graft 保留中の保持項目であり、次単位で配線する。
struct PhoneSubtree {
    guid: String,
    // KumaPhone.prefabの文書列。所有権のためバイト列も保持する。
    prefab_bytes: Vec<u8>,
    // SmartPhone.fbxのバイト列と階層。SHA-256とsourceModelsに使う。
    fbx_bytes: Vec<u8>,
    #[allow(dead_code)]
    fbx_paths: Vec<String>,
}

// ネストしたKumaPhoneを解決する。Milfy.prefab内の2件目のインスタンスが対象である。
#[cfg(not(target_arch = "wasm32"))]
fn resolve_phone_subtree(
    index: &crate::unity::AssetIndex,
    canonical_assets: &std::path::Path,
    parsed: &ParsedPrefab,
    label: &str,
) -> Result<Option<PhoneSubtree>, VrmError> {
    // Milfy.fbx以外のインスタンス（KumaPhone）を探す。
    // guidがFBXでなくprefabであるものを副階層とみなす。
    // KumaPhoneでないprefabインスタンスがあれば明示的に失敗する。
    for instance in &parsed.instances {
        let guid = instance.source_guid.clone();
        if guid.is_empty() {
            continue;
        }
        let Ok(g) = crate::unity::Guid::parse(&guid) else {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!(
                    "PrefabInstanceのGUIDが不正である（{}）",
                    crate::escape_diag(label),
                ),
            ));
        };
        let Some(record) = index.get(&g) else {
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
        // FBXでなくprefabの場合だけ副階層として読む。
        if record.path.ends_with(".prefab") {
            let full = canonical_assets.join(&record.path);
            let prefab_bytes = std::fs::read(&full).map_err(|e| {
                VrmError::new(
                    VrmErrorKind::InvalidPrefab,
                    format!("副Prefabを読めない: {e}"),
                )
            })?;
            // 副Prefab内のSmartPhone.fbxを探す。KumaPhoneの実体参照が対象である。
            let phone_parsed = parse_prefab_docs_from_bytes(&prefab_bytes, label)?;
            let mut phone_fbx_guid = String::new();
            for instance in &phone_parsed.instances {
                if !instance.source_guid.is_empty() {
                    phone_fbx_guid = instance.source_guid.clone();
                    break;
                }
            }
            if phone_fbx_guid.is_empty() {
                return Err(VrmError::new(
                    VrmErrorKind::InvalidPrefab,
                    format!(
                        "副Prefab内に入力FBXへの参照がない（{}）",
                        crate::escape_diag(label),
                    ),
                ));
            }
            let Ok(g2) = crate::unity::Guid::parse(&phone_fbx_guid) else {
                return Err(VrmError::new(
                    VrmErrorKind::InvalidPrefab,
                    format!(
                        "副PrefabのFBX参照GUIDが不正である（{}）",
                        crate::escape_diag(label)
                    ),
                ));
            };
            let Some(fbx_record) = index.get(&g2) else {
                return Err(VrmError::new(
                    VrmErrorKind::InvalidPrefab,
                    format!(
                        "副PrefabのFBXが索引にない: {}（{}）",
                        crate::escape_diag(&phone_fbx_guid),
                        crate::escape_diag(label),
                    ),
                ));
            };
            let fbx_full = canonical_assets.join(&fbx_record.path);
            let fbx_bytes = std::fs::read(&fbx_full).map_err(|e| {
                VrmError::new(VrmErrorKind::InvalidPrefab, format!("副FBXを読めない: {e}"))
            })?;
            return Ok(Some(PhoneSubtree {
                guid,
                prefab_bytes,
                fbx_bytes,
                fbx_paths: Vec::new(),
            }));
        }
    }
    Ok(None)
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
    /// ローカルGOのfileID（シーン追加の場合）。参照解決に使う。
    #[allow(dead_code)]
    local_go: Option<FileId>,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone)]
#[allow(dead_code)] // スマホ graft 保留中の保持項目であり、次単位で配線する。
struct TreeSource {
    model_id: String,
    path: Vec<String>,
    asset_guid: String,
    local_id: FileId,
}

#[cfg(not(target_arch = "wasm32"))]
fn assemble_tree(
    fbx: &FbxFile<'_>,
    hier: &FbxHierarchy,
    parsed: &ParsedPrefab,
    phone: &Option<PhoneSubtree>,
    label: &str,
) -> Result<Vec<TreeNode>, VrmError> {
    // 実装は段階的に行う。まずMilfy.fbx単体の階層をDFSで出し、
    // 追加物と副階層を graft する。
    // 現段階ではMilfyの実データ構造に対応し、それ以外は明示的に失敗する。
    let _ = phone;
    build_milfy_tree(fbx, hier, parsed, label)
}

/// Milfy単体のツリーを組み立てる。副階層の graft は呼び出し側で行う。
#[cfg(not(target_arch = "wasm32"))]
fn build_milfy_tree(
    fbx: &FbxFile<'_>,
    hier: &FbxHierarchy,
    parsed: &ParsedPrefab,
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
    // 追加物（Milfyルート直下のローカル）。
    let root_tr = transform_file_id("");
    let mut root_added: Vec<FileId> = Vec::new();
    if let Some(list) = added_by_parent.get(&root_tr) {
        for object in list {
            if parsed.local_tr.contains_key(object) {
                root_added.push(*object);
            }
        }
    }
    // 出所の組み立てヘルパ。
    let fbx_guid = find_fbx_guid(parsed, label)?;
    // Armature部分木のDFS。子はFBX名前順、追加物は後に付ける。
    // Head配下の追加物（Head_Contact等）もここで付ける。
    // Hand.L配下のスマホ graft は別途行う（副階層が必要）。
    let mut stack: Vec<BuildFrame> = vec![BuildFrame {
        kind: FrameKind::Fbx(armature_idx),
        parent: None,
        scene_path: vec![avatar_name.clone(), models[armature_idx].name.clone()],
    }];
    // 反復DFS。子を逆順で積む。
    while let Some(frame) = stack.pop() {
        match frame.kind {
            FrameKind::Fbx(fbx_index) => {
                let node_index = nodes.len();
                let model = &models[fbx_index];
                let source = fbx_source_of(hier, fbx_index, &fbx_guid, label)?;
                nodes.push(TreeNode {
                    name: model.name.clone(),
                    parent: frame.parent,
                    children: Vec::new(),
                    scene_path: frame.scene_path.clone(),
                    source: Some(source),
                    added_trs: None,
                    fbx_index: Some(fbx_index),
                    local_go: None,
                });
                if let Some(parent) = frame.parent {
                    nodes[parent].children.push(node_index);
                }
                // 子の収集：FBX子（名前順）＋追加物（登録順）。
                // Hand.Lの場合はスマホ graft を最後に付ける（呼び出し側で解決）。
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
                let tr_fid = hier.tr_fid_at[fbx_index];
                if let Some(added) = added_by_parent.get(&tr_fid) {
                    for object in added {
                        // KumaPhone由来（stripped）はスマホ graft として扱い、ここでは出さない。
                        // Hand.L配下の633...が該当する。
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
                        }
                    }
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
                    local_go: Some(local.go),
                });
                if let Some(parent) = frame.parent {
                    nodes[parent].children.push(node_index);
                }
                // 追加物の子（m_Children順）。Colliders等が該当するが、
                // Milfy直下の追加物は子を持たない。副階層の子は別途 graft する。
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
        let source = fbx_source_of(hier, fbx_index, &fbx_guid, label)?;
        let scene_path = vec![avatar_name.clone(), model.name.clone()];
        nodes.push(TreeNode {
            name: model.name.clone(),
            parent: None,
            children: Vec::new(),
            scene_path,
            source: Some(source),
            added_trs: None,
            fbx_index: Some(fbx_index),
            local_go: None,
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
            local_go: Some(local.go),
        });
        let _ = node_index;
    }
    // スマホ graft は呼び出し側（assemble_tree）で行う。ここでは出さない。
    Ok(nodes)
}

/// DFS展開フレームの種別。
#[cfg(not(target_arch = "wasm32"))]
enum FrameKind {
    Fbx(usize),
    Added(FileId),
}

#[cfg(not(target_arch = "wasm32"))]
struct BuildFrame {
    kind: FrameKind,
    parent: Option<usize>,
    scene_path: Vec<String>,
}

/// 入力FBXのGUIDをPrefabInstanceから求める。
#[cfg(not(target_arch = "wasm32"))]
fn find_fbx_guid(parsed: &ParsedPrefab, label: &str) -> Result<String, VrmError> {
    // Milfy.fbxを指すインスタンスを探す。副Prefab（KumaPhone）でないものを選ぶ。
    // 判定は後段の副階層解決に任せ、ここでは最初のFBXらしいGUIDを返す。
    // 複数ある場合は呼び出し側で絞る。
    for instance in &parsed.instances {
        if !instance.source_guid.is_empty() {
            return Ok(instance.source_guid.clone());
        }
    }
    Err(VrmError::new(
        VrmErrorKind::InvalidPrefab,
        format!(
            "入力FBXへのPrefabInstanceがない（{}）",
            crate::escape_diag(label)
        ),
    ))
}

/// FBXノードの出所を組み立てる。
#[cfg(not(target_arch = "wasm32"))]
fn fbx_source_of(
    hier: &FbxHierarchy,
    fbx_index: usize,
    fbx_guid: &str,
    label: &str,
) -> Result<TreeSource, VrmError> {
    let full = hier.paths[fbx_index].clone();
    // モデルIDは資産パス由来であり、呼び出し側で上書きする。
    // ここでは仮に空にし、emit時に確定させる。
    let _ = label;
    let path: Vec<String> = if full.is_empty() {
        Vec::new()
    } else {
        full.split('/').map(|s| s.to_owned()).collect()
    };
    Ok(TreeSource {
        model_id: String::new(),
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
    let base = label.rsplit(['/', '\\']).next().unwrap_or(label);
    base.strip_suffix(".prefab")
        .or_else(|| base.strip_suffix(".PREFAB"))
        .unwrap_or(base)
        .to_owned()
}

/// Hand.LのTransform fileIDを求める。KumaPhoneの graft 位置の特定に使う。
#[cfg(not(target_arch = "wasm32"))]
#[allow(dead_code)] // スマホ graft 保留中の保持項目であり、次単位で配線する。
fn find_hand_transform(
    models: &[crate::fbx::Model],
    hier: &FbxHierarchy,
) -> Result<FileId, VrmError> {
    for (i, m) in models.iter().enumerate() {
        if m.name == "Hand.L" {
            return Ok(hier.tr_fid_at[i]);
        }
    }
    Err(VrmError::new(
        VrmErrorKind::InvalidPrefab,
        "Hand.Lが見つからない",
    ))
}

/// KumaPhone由来の追加物（SmartPhoneルート）かどうかを判定する。
#[cfg(not(target_arch = "wasm32"))]
#[allow(dead_code)] // スマホ graft 保留中の保持項目であり、次単位で配線する。
fn is_phone_root_object(parsed: &ParsedPrefab, object: FileId) -> bool {
    // 633...はstripped（KumaPhone由来）であり、local_trにない。
    !parsed.local_tr.contains_key(&object)
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
fn compute_reference_locals(
    fbx: &FbxFile<'_>,
    tree: &[TreeNode],
    bind: &BindSelection,
) -> Result<Vec<ReferenceLocal>, VrmError> {
    // Unityの `Matrix4x4`（float32）と同順で計算し、sidecarと一致させる。
    // bind対象は親のbind世界またはscene世界の逆行列とbind世界の積を分解する。
    // 非対象はsceneのローカルTRSをそのまま使う（float32に丸める）。
    let models = fbx.models();
    // bind世界行列（FBX BindPose）をfloat32で持つ。
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
            bind_world_f32.insert(idx, m);
        }
    }
    // sceneローカルTRS（float32）。FBX由来はlocal_matrixの分解、追加物はprefab値である。
    let mut scene_trs: Vec<([f32; 3], [f32; 4], [f32; 3])> = Vec::with_capacity(tree.len());
    for node in tree {
        if let Some(fbx_index) = node.fbx_index {
            let model = &models[fbx_index];
            let local = model.transform.local_matrix().map_err(|e| {
                VrmError::new(
                    VrmErrorKind::InvalidPrefab,
                    format!("ローカル行列を作れない: {e}"),
                )
            })?;
            let mut m32 = [0f32; 16];
            for (i, v) in local.iter().enumerate() {
                m32[i] = *v as f32;
            }
            let (p, r, s) = decompose_bind_local_f32(&m32).map_err(|_| {
                VrmError::new(
                    VrmErrorKind::InvalidPrefab,
                    format!(
                        "sceneローカルを分解できない: {}",
                        crate::escape_diag(&node.name)
                    ),
                )
            })?;
            scene_trs.push((p, r, s));
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
            let local = mul_f32(&inv, bind_world);
            match decompose_bind_local_f32(&local) {
                Ok((p, r, s)) => {
                    let (pos, rot, scale) = fbx_trs_to_unity(p, r, s);
                    out.push((pos, rot, scale, "bind"));
                }
                Err(_) => {
                    // せん断等でTRSに戻らない行列。申告して近似として出す。
                    // 正確な近似値は後段で求める。ここではscene値を仮置きする。
                    let (p, r, s) = scene_trs[i];
                    let (pos, rot, scale) = fbx_trs_to_unity(p, r, s);
                    out.push((pos, rot, scale, "bind-decomposed-approx"));
                }
            }
        } else {
            let (p, r, s) = scene_trs[i];
            let (pos, rot, scale) = fbx_trs_to_unity(p, r, s);
            out.push((pos, rot, scale, "scene"));
        }
    }
    Ok(out)
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

/// 回転行列（3列）から四元数を求める。Unityの `Matrix4x4.rotation` 相当である。
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
    doc: FileId,
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
    doc: FileId,
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
    _phone: &Option<PhoneSubtree>,
    _fbx_guid: &str,
    label: &str,
) -> Result<Collected, VrmError> {
    // ノード索引の構築。MonoBehaviourの参照解決に使う。
    // stripped TR fid → tree node、stripped GO fid → tree node、doc fid → tree node。
    let mut tr_to_node: HashMap<FileId, usize> = HashMap::new();
    let mut go_to_node: HashMap<FileId, usize> = HashMap::new();
    // FBX由来ノードは、Transform/GameObjectのfileIDから引けるよう表を作る。
    // treeのsource.local_idはTransform fileIDである。GameObject fileIDは別途求める。
    for (i, node) in tree.iter().enumerate() {
        if let Some(source) = node.source.as_ref() {
            tr_to_node.insert(source.local_id, i);
            // GameObject fileIDはpathから求める。import root例外に注意する。
            let go_fid = if source.path.len() == 1 && is_import_root_path(&source.path, &node.name)
            {
                crate::unity::game_object_file_id("")
            } else {
                crate::unity::game_object_file_id(&source.path.join("/"))
            };
            go_to_node.insert(go_fid, i);
        }
    }
    // strippedの対応付け。local fid → source fid → tree node である。
    for (local, stripped) in parsed.stripped_tr.iter() {
        if let Some(&node) = tr_to_node.get(&stripped.source_fid) {
            tr_to_node.insert(*local, node);
        }
    }
    for (local, stripped) in parsed.stripped_go.iter() {
        // GameObjectのsource fidからtree nodeを引く。
        if let Some(&node) = go_to_node.get(&stripped.source_fid) {
            go_to_node.insert(*local, node);
        }
        // 解決できないstripped GO（import root等）は参照解決時に失敗させる。
        let _ = local;
    }
    // MonoBehaviourのdoc fid → tree node（所属GO経由）。
    // ローカルGO（Ground等）もtreeのlocal_goから引く。
    let mut local_go_to_node: HashMap<FileId, usize> = HashMap::new();
    for (i, node) in tree.iter().enumerate() {
        if let Some(go) = node.local_go {
            local_go_to_node.insert(go, i);
        }
    }
    let mut doc_to_node: HashMap<FileId, usize> = HashMap::new();
    for mono in &parsed.monos {
        if let Some(&node) = go_to_node.get(&mono.go) {
            doc_to_node.insert(mono.doc, node);
        } else if let Some(&node) = local_go_to_node.get(&mono.go) {
            doc_to_node.insert(mono.doc, node);
        }
        // 解決できない所属（import root等のstripped GO）は、参照解決時に失敗させる。
    }
    // 成分の判別と収集。未知のDLL型は明示的に失敗し、Contact等は無視する。
    let mut ignored: Vec<String> = Vec::new();
    let mut ignored_set: HashSet<String> = HashSet::new();
    let mut bones: Vec<CollectedBone> = Vec::new();
    let mut colliders: Vec<CollectedCollider> = Vec::new();
    let mut constraints: Vec<CollectedConstraint> = Vec::new();
    let mut descriptor: Option<CollectedDescriptor> = None;
    for mono in &parsed.monos {
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
                    let mut bone = parse_phys_bone(mono, node, &tr_to_node, label)?;
                    // 有効 chain の計算。FBX由来は真、追加物はprefabのactiveである。
                    // 祖先はすべて有効である（Milfyの実データ）。
                    // 無効なものはsidecarに含めない（Unityヘルパと同順）。
                    let active = node_active(tree, node, parsed);
                    bone.active_chain = active;
                    if bone.enabled && active {
                        bones.push(bone);
                    }
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
                    let mut collider = parse_collider(mono, node, &tr_to_node, label)?;
                    let active = node_active(tree, node, parsed);
                    collider.active_chain = active;
                    if collider.enabled && active {
                        colliders.push(collider);
                    }
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
                    // 同一Transform上のVRC制約の順序を求める。
                    let order_index = constraints
                        .iter()
                        .filter(|c: &&CollectedConstraint| c.component_node == node)
                        .count();
                    let constraint = parse_vrc_constraint(
                        mono,
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
                    descriptor = Some(parse_descriptor(mono, &tr_to_node, label)?);
                }
            },
        }
    }
    // ノードDFS順→取得順（文書順）で並べ直す。Unityヘルパと同じ決定性規則である。
    // component_node（tree索引）がDFS順であり、docが取得順である。
    bones.sort_by(|a, b| {
        a.component_node
            .cmp(&b.component_node)
            .then(a.doc.cmp(&b.doc))
    });
    colliders.sort_by(|a, b| {
        a.component_node
            .cmp(&b.component_node)
            .then(a.doc.cmp(&b.doc))
    });
    constraints.sort_by(|a, b| {
        a.component_node
            .cmp(&b.component_node)
            .then(a.doc.cmp(&b.doc))
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
#[cfg(not(target_arch = "wasm32"))]
fn node_active(tree: &[TreeNode], node: usize, parsed: &ParsedPrefab) -> bool {
    let mut current = Some(node);
    while let Some(index) = current {
        let tree_node = &tree[index];
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

/// import rootのpath例外かどうかを判定する。`["SmartPhone"]` 等が該当する。
#[cfg(not(target_arch = "wasm32"))]
fn is_import_root_path(path: &[String], node_name: &str) -> bool {
    path.len() == 1 && path[0] == node_name
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
        doc,
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
    let version_raw = extra
        .get("version")
        .and_then(|v| v.as_i64())
        .ok_or_else(|| {
            VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!(
                    "欄がない: version（文書{doc}）（{}）",
                    crate::escape_diag(label)
                ),
            )
        })?;
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
    let ignore_other = extra_bool(extra, "ignoreOtherPhysBones", doc, label)?;
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
        doc,
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
    _phone: &Option<PhoneSubtree>,
    fbx_guid: &str,
    bind: &BindSelection,
    label: &str,
) -> Result<DerivedSidecar, VrmError> {
    use sha2::Digest;
    // ノードIDの付番。DFS順に `n:N` である。
    let node_id_of = |index: usize| format!("n:{index}");
    // sourceModels。Milfyのみ（副階層は後段で追加する）。
    let digest = sha2::Sha256::digest(input.fbx_bytes);
    let mut sha = String::with_capacity(64);
    for byte in digest {
        sha.push(char::from_digit(u32::from(byte >> 4), 16).expect("hexである"));
        sha.push(char::from_digit(u32::from(byte & 0x0f), 16).expect("hexである"));
    }
    let avatar_name = avatar_name_from_label(label);
    // モデルIDは資産パスのstemを小文字化する。
    let model_id = input
        .fbx_asset_path
        .rsplit('/')
        .next()
        .unwrap_or("model")
        .strip_suffix(".fbx")
        .or_else(|| input.fbx_asset_path.rsplit('/').next())
        .unwrap_or("model")
        .to_ascii_lowercase();
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
                let path = source.path.clone();
                // import root例外は呼び出し側で解決済みである。
                let _ = &path;
                serde_json::json!({
                    "modelId": model_id,
                    "path": source.path,
                    "assetGuid": fbx_guid,
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
    // 制約列の組み立て。IDは `vc:` + 現場パス + `#` + 順序である。
    let mut constraints_json = Vec::with_capacity(collector.constraints.len());
    for constraint in &collector.constraints {
        let id = format!(
            "vc:{}#{}",
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
    let _ = (meta, &avatar_name, &model_id, &sha);
    let _ = nodes_json;
    let _ = bones_json;
    let _ = unsupported;
    let _ = colliders_json;
    let _ = constraints_json;
    let _ = look_at_json;
    // humanoidは.metaから組み立てる。
    let humanoid_json = build_humanoid_json(meta, tree, label)?;
    // sourceModelsはMilfyのみ（副階層は後段で追加する）。
    let source_models_json = vec![serde_json::json!({
        "id": model_id,
        "sha256": sha,
        "assetGuid": fbx_guid,
        "assetPath": input.fbx_asset_path,
    })];
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
    let sidecar_json = serde_json::json!({
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
    Ok(DerivedSidecar {
        json: bytes,
        ignored_types: ignored_sorted,
    })
}

/// humanoidのT-poseを.metaから組み立てる。
#[cfg(not(target_arch = "wasm32"))]
fn build_humanoid_json(
    meta: &AssetMeta,
    tree: &[TreeNode],
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
    // Transform名からtree索引を引く。FBX由来ノードが対象である。
    let mut name_to_nodes: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, node) in tree.iter().enumerate() {
        if node.source.is_some() {
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
        // `.meta` の skeleton はFBX空間（右手系）のため、Unity空間（左手系）へ写す。
        // 参照実装の capture と同じく位置Xと回転Y/Zの符号を反転させる。
        let t_pose = match skeleton_map.get(transform_name.as_str()) {
            Some((p, r, s)) => serde_json::json!({
                "position": [-p[0], p[1], p[2]],
                "rotation": [r[0], -r[1], -r[2], r[3]],
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
}
