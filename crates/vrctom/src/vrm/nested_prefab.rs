//! ネストしたPrefabが持つメッシュ・ノード階層・スキン・材質の解決と統合。
//!
//! 入口は [`prepare_nested_prefab_meshes`]（材質プールへの追記と追加データの準備）と
//! [`apply_nested_prefab_meshes`]（[`VrmModel`] への反映）の2段階である。
//! 材質の読み取りより前に準備を完了させ、失敗時は中間モデルを変更しない。

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

use yaml_serde::Value;

use super::error::{VrmError, VrmErrorKind};
use super::prefab::{
    PrefabBaseMaterial, material_slot_index, prefab_base_materials, prefab_hierarchy_paths,
    read_flag_value, renderer_active,
};
use super::renderers::{RendererPoolEntry, renderer_slot_key};
use super::{
    VrmMesh, VrmModel, VrmNode, VrmOptions, VrmSkin, axis_transform,
    bind_world_matrices_for_nested, matrix, mesh, model_vrm_worlds, wrap_fbx_error,
};
use crate::fbx::{FbxErrorKind, FbxFile, FbxOptions};
use crate::unity::{
    AssetIndex, AssetKind, AssetSource, FileId, GameObject, Guid, MeshFilter, MeshRenderer,
    ObjectRef, ParseOptions, PrefabInstance, SerializedFile, SkinnedMeshRenderer, Transform,
    TypedObject, game_object_file_id, mesh_file_id, mesh_filter_file_id, mesh_renderer_file_id,
    skinned_mesh_renderer_file_id, transform_file_id,
};

/// ネストPrefab1件の既定バイト数上限（64MiB）。
const DEFAULT_MAX_PREFAB_BYTES: u64 = 64 * 1024 * 1024;
/// 副FBX1件の既定バイト数上限（128MiB）。
const DEFAULT_MAX_FBX_BYTES: u64 = 128 * 1024 * 1024;
/// ネスト解決の合計読み取り量の既定上限（512MiB）。
const DEFAULT_MAX_TOTAL_BYTES: u64 = 512 * 1024 * 1024;
/// ネストPrefabの再帰深さの既定上限。
const DEFAULT_MAX_DEPTH: usize = 16;
/// 展開するインスタンス総数の既定上限。
const DEFAULT_MAX_INSTANCES: usize = 256;

/// ネストPrefab解決の資源上限。
///
/// フィールドは非公開で、既定値は [`Default`] が設定する。
/// 上限の変更は `with_` で始まるメソッドで行う。
#[derive(Debug, Clone)]
pub struct NestedPrefabLimits {
    max_prefab_bytes: u64,
    max_fbx_bytes: u64,
    max_total_bytes: u64,
    max_depth: usize,
    max_instances: usize,
}

impl NestedPrefabLimits {
    /// 既定の上限を持つ設定を生成する。
    pub fn new() -> Self {
        Self {
            max_prefab_bytes: DEFAULT_MAX_PREFAB_BYTES,
            max_fbx_bytes: DEFAULT_MAX_FBX_BYTES,
            max_total_bytes: DEFAULT_MAX_TOTAL_BYTES,
            max_depth: DEFAULT_MAX_DEPTH,
            max_instances: DEFAULT_MAX_INSTANCES,
        }
    }

    /// 1件のPrefab本文の上限（バイト数）を変更する。
    pub fn with_max_prefab_bytes(mut self, value: u64) -> Self {
        self.max_prefab_bytes = value;
        self
    }

    /// 副FBX1件の上限（バイト数）を変更する。
    pub fn with_max_fbx_bytes(mut self, value: u64) -> Self {
        self.max_fbx_bytes = value;
        self
    }

    /// 依存資産の合計読み取り量の上限（バイト数）を変更する。
    pub fn with_max_total_bytes(mut self, value: u64) -> Self {
        self.max_total_bytes = value;
        self
    }

    /// 再帰深さの上限を変更する。
    pub fn with_max_depth(mut self, value: usize) -> Self {
        self.max_depth = value;
        self
    }

    /// 展開するインスタンス総数の上限を変更する。
    pub fn with_max_instances(mut self, value: usize) -> Self {
        self.max_instances = value;
        self
    }
}

impl Default for NestedPrefabLimits {
    fn default() -> Self {
        Self::new()
    }
}

/// ネストPrefab解決の入力一式。
pub struct NestedPrefabInput<'a> {
    /// `Assets` 直下基準で読む読み手。
    pub source: &'a dyn AssetSource,
    /// 資産索引。
    pub index: &'a AssetIndex,
    /// 変換対象のルートPrefab（またはシーン）。
    pub root_prefab: &'a SerializedFile,
    /// ルートPrefabの表示用パス（`Assets/...`）。
    pub root_prefab_path: &'a str,
    /// 主FBXの解析結果。
    pub main_fbx: &'a FbxFile<'a>,
    /// 主FBXのGUID（小文字hex）。
    pub main_fbx_guid: &'a str,
    /// 副FBXの解析オプション。
    pub fbx_options: &'a FbxOptions,
    /// メッシュ・頂点・三角形の上限を含む中間モデルオプション。
    pub vrm_options: &'a VrmOptions,
    /// ネスト解決の資源上限。
    pub limits: &'a NestedPrefabLimits,
}

/// ネストPrefab解決の構造化報告。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NestedPrefabReport {
    /// 出力に追加した有効メッシュ数。
    pub active_meshes: usize,
    /// 無効状態で除外したメッシュ数。
    pub excluded_meshes: usize,
    /// 除外したメッシュのModel名（出現順）。
    pub excluded_names: Vec<String>,
    /// 追加したノード数。
    pub added_nodes: usize,
    /// 追加したスキン数。
    pub added_skins: usize,
    /// 非FBX参照や欠損参照を飛ばした際の警告文。
    pub warnings: Vec<String>,
}

/// [`prepare_nested_prefab_meshes`] が組み立てた適用前の追加データ。
///
/// [`apply_nested_prefab_meshes`] に渡すことでモデルへ反映する。
#[derive(Debug, Clone)]
pub struct PreparedNestedPrefabs {
    base_node_count: usize,
    base_skin_count: usize,
    nodes: Vec<VrmNode>,
    parent_children: Vec<(usize, usize)>,
    skins: Vec<VrmSkin>,
    meshes: Vec<VrmMesh>,
    report: NestedPrefabReport,
}

impl PreparedNestedPrefabs {
    /// 準備された内容の報告を返す。
    pub fn report(&self) -> &NestedPrefabReport {
        &self.report
    }
}

/// 上位Prefabから下位インスタンスへ引き渡す改変1件。
#[derive(Debug, Clone)]
struct ScopedOverride {
    /// 対象資産のGUID（小文字hex）。
    target_guid: String,
    /// 対象オブジェクトの `FileId`。
    target_file_id: FileId,
    /// 上書きする欄パス。
    property_path: String,
    /// 上書き値。
    value: Option<Value>,
    /// 上書き参照。
    object_reference: Option<ObjectRef>,
}

type InstMountEntry = (Option<usize>, bool, HashMap<FileId, usize>);
type InstMountCache = HashMap<FileId, InstMountEntry>;
type LocalBuildResult = (HashMap<FileId, MountPoint>, HashMap<FileId, usize>);

/// 展開先ノードの親情報。
#[derive(Debug, Clone, Copy)]
struct MountPoint {
    /// 仮想ノード領域上の親ノード索引（`None` はアバタールート直下）。
    parent_node: Option<usize>,
    /// 親階層が有効かどうか。
    parent_active: bool,
}

/// stripped文書1件の解決先。
#[derive(Debug, Clone)]
struct StrippedTarget {
    source_guid: String,
    source_fid: FileId,
    instance_fid: FileId,
}

/// `local_tr` に紐づく追加先情報。
#[derive(Debug, Clone)]
struct AddedParentLink {
    owner_instance: FileId,
    target_guid: String,
    target_fid: FileId,
}

/// 単一Prefabファイルの型付き索引。
struct ParsedLayer<'a> {
    label: &'a str,
    local_go: HashMap<FileId, &'a GameObject>,
    local_tr: HashMap<FileId, &'a Transform>,
    tr_of_go: HashMap<FileId, FileId>,
    stripped_tr: HashMap<FileId, StrippedTarget>,
    stripped_go: HashMap<FileId, StrippedTarget>,
    stripped_other: HashMap<FileId, StrippedTarget>,
    instances: Vec<(FileId, &'a PrefabInstance)>,
    instance_at: HashMap<FileId, usize>,
    added_parent_of: HashMap<FileId, AddedParentLink>,
    smrs: Vec<(FileId, &'a SkinnedMeshRenderer)>,
    mesh_renderers: Vec<(FileId, &'a MeshRenderer)>,
    mesh_filters_by_go: HashMap<FileId, Vec<(FileId, &'a MeshFilter)>>,
}

/// 仮想ノード・スキン・メッシュ・材質プールの構築作業領域。
struct NestedContext<'a> {
    input: &'a NestedPrefabInput<'a>,
    main_root_scale: f64,
    base_node_count: usize,
    base_node_worlds: Vec<[f64; 16]>,
    base_skin_count: usize,
    base_mesh_count: usize,
    base_vertex_count: usize,
    base_triangle_count: usize,
    nodes: Vec<VrmNode>,
    node_worlds: Vec<[f64; 16]>,
    parent_children: Vec<(usize, usize)>,
    skins: Vec<VrmSkin>,
    meshes: Vec<VrmMesh>,
    pool: &'a mut Vec<RendererPoolEntry>,
    pool_at: HashMap<(String, FileId), usize>,
    used_bytes: u64,
    instance_count: usize,
    fbx_cache: HashMap<String, Vec<u8>>,
    prefab_cache: HashMap<String, SerializedFile>,
    report: NestedPrefabReport,
}

impl<'a> NestedContext<'a> {
    fn new(
        model: &VrmModel,
        pool: &'a mut Vec<RendererPoolEntry>,
        input: &'a NestedPrefabInput<'a>,
    ) -> Result<Self, VrmError> {
        let main_root_scale = model.root[0];
        if !main_root_scale.is_finite()
            || main_root_scale <= 0.0
            || model.root[5] != main_root_scale
            || model.root[10] != main_root_scale
            || model.root[1] != 0.0
            || model.root[2] != 0.0
            || model.root[4] != 0.0
            || model.root[6] != 0.0
            || model.root[8] != 0.0
            || model.root[9] != 0.0
        {
            return Err(VrmError::new(
                VrmErrorKind::UnsupportedUnitScale,
                "モデルルートが一様な単位換算でない",
            ));
        }
        let base_node_worlds = model_vrm_worlds(model)?;
        let base_vertex_count = model
            .meshes
            .iter()
            .map(|m| m.vertices.positions.len())
            .sum();
        let base_triangle_count = model
            .meshes
            .iter()
            .flat_map(|m| m.primitives.iter())
            .map(|p| p.indices.len() / 3)
            .sum();
        let mut pool_at = HashMap::with_capacity(pool.len());
        for (idx, entry) in pool.iter().enumerate() {
            pool_at.insert((entry.guid.to_ascii_lowercase(), entry.file_id), idx);
        }
        Ok(Self {
            input,
            main_root_scale,
            base_node_count: model.nodes.len(),
            base_node_worlds,
            base_skin_count: model.skins.len(),
            base_mesh_count: model.meshes.len(),
            base_vertex_count,
            base_triangle_count,
            nodes: Vec::new(),
            node_worlds: Vec::new(),
            parent_children: Vec::new(),
            skins: Vec::new(),
            meshes: Vec::new(),
            pool,
            pool_at,
            used_bytes: 0,
            instance_count: 0,
            fbx_cache: HashMap::new(),
            prefab_cache: HashMap::new(),
            report: NestedPrefabReport::default(),
        })
    }

    fn total_node_count(&self) -> usize {
        self.base_node_count + self.nodes.len()
    }

    fn total_skin_count(&self) -> usize {
        self.base_skin_count + self.skins.len()
    }

    fn world_of(&self, node: usize) -> [f64; 16] {
        if node < self.base_node_count {
            self.base_node_worlds[node]
        } else {
            self.node_worlds[node - self.base_node_count]
        }
    }

    fn push_node(&mut self, node: VrmNode) -> Result<usize, VrmError> {
        let index = self.total_node_count();
        let world = match node.parent {
            Some(parent) => {
                if parent >= index {
                    return Err(VrmError::new(
                        VrmErrorKind::InvalidStructure,
                        "追加ノードの親索引が範囲外である",
                    ));
                }
                let parent_world = self.world_of(parent);
                if parent < self.base_node_count {
                    self.parent_children.push((parent, index));
                } else {
                    self.nodes[parent - self.base_node_count]
                        .children
                        .push(index);
                }
                matrix::mul(&parent_world, &node.local)
            }
            None => node.local,
        };
        self.nodes.push(node);
        self.node_worlds.push(world);
        Ok(index)
    }

    /// 既存・追加済みを含め、指定ノードが既にメッシュを所有していないか確認し、
    /// 空いていればそのノードを、既に使われていれば `IDENTITY` の子ノードを作って返す。
    fn alloc_mesh_node(
        &mut self,
        node: usize,
        name: &str,
        claimed_nodes: &mut HashSet<usize>,
        model: &VrmModel,
    ) -> Result<usize, VrmError> {
        let already_used = claimed_nodes.contains(&node)
            || (node < self.base_node_count && model.meshes.iter().any(|m| m.node == node));
        if !already_used {
            claimed_nodes.insert(node);
            return Ok(node);
        }
        let child = self.push_node(VrmNode {
            name: name.to_owned(),
            parent: Some(node),
            children: Vec::new(),
            local: matrix::IDENTITY,
        })?;
        claimed_nodes.insert(child);
        Ok(child)
    }

    fn push_mesh(&mut self, mesh: VrmMesh) -> Result<(), VrmError> {
        let total_meshes = self.base_mesh_count + self.meshes.len() + 1;
        if total_meshes > self.input.vrm_options.max_meshes {
            return Err(VrmError::new(
                VrmErrorKind::LimitExceeded,
                "メッシュ数が上限を超えた",
            ));
        }
        self.base_vertex_count += mesh.vertices.positions.len();
        if self.base_vertex_count > self.input.vrm_options.max_vertices {
            return Err(VrmError::new(
                VrmErrorKind::LimitExceeded,
                "頂点数が上限を超えた",
            ));
        }
        let added_tris: usize = mesh.primitives.iter().map(|p| p.indices.len() / 3).sum();
        self.base_triangle_count += added_tris;
        if self.base_triangle_count > self.input.vrm_options.max_triangles {
            return Err(VrmError::new(
                VrmErrorKind::LimitExceeded,
                "三角形数が上限を超えた",
            ));
        }
        self.meshes.push(mesh);
        self.report.active_meshes += 1;
        Ok(())
    }

    fn register_material(&mut self, guid: &str, file_id: FileId, renderer_path: &str) -> String {
        let guid_lower = guid.to_ascii_lowercase();
        let key = (guid_lower.clone(), file_id);
        if !self.pool_at.contains_key(&key) {
            let idx = self.pool.len();
            self.pool.push(RendererPoolEntry {
                guid: guid_lower.clone(),
                file_id,
                path: String::new(),
                first_renderer_path: renderer_path.to_owned(),
            });
            self.pool_at.insert(key, idx);
        }
        renderer_slot_key(&guid_lower, file_id)
    }

    fn consume_bytes(&mut self, len: usize, what: &str, label: &str) -> Result<(), VrmError> {
        self.used_bytes = self.used_bytes.saturating_add(len as u64);
        if self.used_bytes > self.input.limits.max_total_bytes {
            return Err(VrmError::new(
                VrmErrorKind::LimitExceeded,
                format!(
                    "{what}の合計読み取り量が上限を超えた: {}バイト（上限{}バイト）（{}）",
                    self.used_bytes,
                    self.input.limits.max_total_bytes,
                    crate::escape_diag(label),
                ),
            ));
        }
        Ok(())
    }

    fn read_limited_checked(
        &mut self,
        relative_path: &str,
        single_limit: u64,
        what: &str,
        label: &str,
    ) -> Result<Vec<u8>, VrmError> {
        let remaining = self
            .input
            .limits
            .max_total_bytes
            .saturating_sub(self.used_bytes);
        let limit = single_limit.min(remaining);
        let bytes = self
            .input
            .source
            .read_limited(relative_path, limit)
            .map_err(|error| match error.kind() {
                std::io::ErrorKind::NotFound => VrmError::new(
                    VrmErrorKind::InvalidPrefab,
                    format!("{what}を読めない: {}: {error}", crate::escape_diag(label)),
                ),
                std::io::ErrorKind::InvalidData => VrmError::new(
                    VrmErrorKind::LimitExceeded,
                    format!(
                        "{what}が上限を超えた: {}（上限{limit}バイト）",
                        crate::escape_diag(label),
                    ),
                ),
                _ => VrmError::new(
                    VrmErrorKind::InvalidPrefab,
                    format!("{what}を読めない: {}: {error}", crate::escape_diag(label)),
                ),
            })?;
        self.consume_bytes(bytes.len(), what, label)?;
        Ok(bytes)
    }

    fn load_prefab_cached(&mut self, relative_path: &str) -> Result<SerializedFile, VrmError> {
        if let Some(cached) = self.prefab_cache.get(relative_path) {
            return Ok(cached.clone());
        }
        let asset_label = format!("Assets/{relative_path}");
        let bytes = self.read_limited_checked(
            relative_path,
            self.input.limits.max_prefab_bytes,
            "ネストPrefab",
            &asset_label,
        )?;
        let parsed = SerializedFile::parse(
            &bytes,
            &ParseOptions::new().with_max_file_bytes(self.input.limits.max_prefab_bytes),
        )
        .map_err(|error| {
            VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!(
                    "ネストPrefabのYAMLを読めない（{}）: {error}",
                    crate::escape_diag(&asset_label),
                ),
            )
        })?;
        self.prefab_cache
            .insert(relative_path.to_owned(), parsed.clone());
        Ok(parsed)
    }

    fn load_fbx_bytes_cached(&mut self, relative_path: &str) -> Result<Vec<u8>, VrmError> {
        if let Some(cached) = self.fbx_cache.get(relative_path) {
            return Ok(cached.clone());
        }
        let asset_label = format!("Assets/{relative_path}");
        let bytes = self.read_limited_checked(
            relative_path,
            self.input.limits.max_fbx_bytes,
            "ネストFBX",
            &asset_label,
        )?;
        self.fbx_cache
            .insert(relative_path.to_owned(), bytes.clone());
        Ok(bytes)
    }
}

/// ネストPrefabが持つメッシュ・ノード・スキン・材質を準備する。
///
/// `pool` へは解決した材質（重複排除済み）を追記し、呼び出し側がその後の
/// 一括材質読み込みで `.mat` を読めるようにする。`model` 自体は変更しない。
///
/// # Errors
///
/// 循環参照・深さやインスタンス数・読み取り量の上限超過・YAMLやFBXの解析不良・
/// 非対応の改変や材質欠落の場合に返す。
pub fn prepare_nested_prefab_meshes(
    model: &VrmModel,
    pool: &mut Vec<RendererPoolEntry>,
    input: &NestedPrefabInput<'_>,
) -> Result<PreparedNestedPrefabs, VrmError> {
    let mut ctx = NestedContext::new(model, pool, input)?;
    let decoded = decode_all(input.root_prefab, input.root_prefab_path)?;
    let layer = build_parsed_layer(&decoded, input.root_prefab_path)?;

    // 主FBXの階層からTransform fileID・パス・ノード索引の対応表を作る。
    let main_nodes = prefab_hierarchy_paths(input.main_fbx)?;
    let main_path_to_fbx_idx: HashMap<&str, usize> = main_nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.path.as_str(), i))
        .collect();
    let main_tr_to_fbx_idx: HashMap<FileId, usize> = main_nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (transform_file_id(&n.path), i))
        .collect();

    // 主FBXのPrefabInstanceがあれば、その有効状態（m_IsActive）を読む。
    let main_instances: Vec<(FileId, &PrefabInstance)> = layer
        .instances
        .iter()
        .copied()
        .filter(|(_, inst)| guid_matches(&inst.source, input.main_fbx_guid))
        .collect();
    let mut main_go_active: HashMap<String, bool> = HashMap::new();
    let mut main_go_id_to_path: HashMap<FileId, String> = HashMap::new();
    main_go_id_to_path.insert(game_object_file_id(""), String::new());
    for node in &main_nodes {
        main_go_id_to_path.insert(game_object_file_id(&node.path), node.path.clone());
    }
    if main_instances.len() == 1 {
        let (_, main_inst) = main_instances[0];
        for m in main_inst.modifications().map_err(|e| {
            VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!(
                    "Prefabの改変内容を読めない（{}）: {e}",
                    crate::escape_diag(input.root_prefab_path)
                ),
            )
        })? {
            if !guid_matches(&m.target, input.main_fbx_guid) {
                continue;
            }
            if m.property_path == "m_IsActive"
                && let Some(path) = main_go_id_to_path.get(&m.target.file_id)
                && let Ok(active) =
                    read_flag_value(&m.value, input.root_prefab_path, path, m.target.file_id)
            {
                main_go_active.insert(path.clone(), active);
            }
        }
    }

    // ルートPrefab内で主FBX以外のPrefabInstanceのうち、解決対象となるものを特定する。
    let mut root_nested_inst_ids: HashSet<FileId> = HashSet::new();
    for (inst_id, inst) in &layer.instances {
        if guid_matches(&inst.source, input.main_fbx_guid) {
            continue;
        }
        if let Some(AssetKind::Prefab) = resolve_source_kind(&inst.source, input.index) {
            root_nested_inst_ids.insert(*inst_id);
        }
    }
    if root_nested_inst_ids.is_empty() {
        return Ok(PreparedNestedPrefabs {
            base_node_count: ctx.base_node_count,
            base_skin_count: ctx.base_skin_count,
            nodes: ctx.nodes,
            parent_children: ctx.parent_children,
            skins: ctx.skins,
            meshes: ctx.meshes,
            report: ctx.report,
        });
    }

    let mut local_cache: HashMap<FileId, MountPoint> = HashMap::new();
    let mut local_visiting: HashSet<FileId> = HashSet::new();
    let mut inst_mount_cache: InstMountCache = HashMap::new();
    let mut inst_visiting: HashSet<FileId> = HashSet::new();
    let mut claimed_nodes: HashSet<usize> = HashSet::new();

    for (inst_id, inst) in &layer.instances {
        if !root_nested_inst_ids.contains(inst_id) {
            continue;
        }
        if inst_mount_cache.contains_key(inst_id) {
            continue;
        }
        expand_root_nested_instance(
            *inst_id,
            inst,
            &layer,
            &main_nodes,
            &main_path_to_fbx_idx,
            &main_tr_to_fbx_idx,
            &main_go_active,
            &mut local_cache,
            &mut local_visiting,
            &mut inst_mount_cache,
            &mut inst_visiting,
            &mut claimed_nodes,
            model,
            &mut ctx,
        )?;
    }

    ctx.report.added_nodes = ctx.nodes.len();
    ctx.report.added_skins = ctx.skins.len();
    Ok(PreparedNestedPrefabs {
        base_node_count: ctx.base_node_count,
        base_skin_count: ctx.base_skin_count,
        nodes: ctx.nodes,
        parent_children: ctx.parent_children,
        skins: ctx.skins,
        meshes: ctx.meshes,
        report: ctx.report,
    })
}

/// 準備済みのネストPrefabデータを [`VrmModel`] へ反映する。
///
/// # Errors
///
/// 準備時点とモデルのノード数・スキン数が食い違う場合に返す。
pub fn apply_nested_prefab_meshes(
    model: &mut VrmModel,
    prepared: PreparedNestedPrefabs,
) -> Result<NestedPrefabReport, VrmError> {
    if model.nodes.len() != prepared.base_node_count {
        return Err(VrmError::new(
            VrmErrorKind::InvalidStructure,
            format!(
                "ネストPrefab適用前にノード数が変化した: {} に対して {}",
                prepared.base_node_count,
                model.nodes.len(),
            ),
        ));
    }
    if model.skins.len() != prepared.base_skin_count {
        return Err(VrmError::new(
            VrmErrorKind::InvalidStructure,
            format!(
                "ネストPrefab適用前にスキン数が変化した: {} に対して {}",
                prepared.base_skin_count,
                model.skins.len(),
            ),
        ));
    }
    model.nodes.extend(prepared.nodes);
    for (parent, child) in prepared.parent_children {
        model.nodes[parent].children.push(child);
    }
    model.skins.extend(prepared.skins);
    model.meshes.extend(prepared.meshes);
    Ok(prepared.report)
}

/// 全文書を型付きでデコードする（`classID` も保持する）。
fn decode_all(
    file: &SerializedFile,
    label: &str,
) -> Result<Vec<(FileId, i32, TypedObject)>, VrmError> {
    file.documents()
        .iter()
        .map(|doc| {
            doc.decode()
                .map(|typed| (doc.file_id(), doc.class_id(), typed))
                .map_err(|error| {
                    VrmError::new(
                        VrmErrorKind::InvalidPrefab,
                        format!(
                            "Prefabの文書を読めない（{}）: {error}",
                            crate::escape_diag(label),
                        ),
                    )
                })
        })
        .collect()
}

/// 単一Prefab層の索引を構築する。
fn build_parsed_layer<'a>(
    decoded: &'a [(FileId, i32, TypedObject)],
    label: &'a str,
) -> Result<ParsedLayer<'a>, VrmError> {
    let mut local_go = HashMap::new();
    let mut local_tr = HashMap::new();
    let mut tr_of_go = HashMap::new();
    let mut stripped_tr = HashMap::new();
    let mut stripped_go = HashMap::new();
    let mut stripped_other = HashMap::new();
    let mut instances = Vec::new();
    let mut instance_at = HashMap::new();
    let mut added_parent_of = HashMap::new();
    let mut smrs = Vec::new();
    let mut mesh_renderers = Vec::new();
    let mut mesh_filters_by_go: HashMap<FileId, Vec<(FileId, &'a MeshFilter)>> = HashMap::new();

    for (file_id, class_id, typed) in decoded {
        match typed {
            TypedObject::GameObject(go) => {
                local_go.insert(*file_id, go);
            }
            TypedObject::Transform(tr) => {
                local_tr.insert(*file_id, tr);
                tr_of_go.insert(tr.game_object.file_id, *file_id);
            }
            TypedObject::PrefabInstance(inst) => {
                let idx = instances.len();
                instances.push((*file_id, inst));
                instance_at.insert(*file_id, idx);
                for added in inst.added_game_objects().map_err(|e| {
                    VrmError::new(
                        VrmErrorKind::InvalidPrefab,
                        format!(
                            "Prefabのm_AddedGameObjectsを読めない（{}）: {e}",
                            crate::escape_diag(label),
                        ),
                    )
                })? {
                    if let Some(guid) = added
                        .target_corresponding_source
                        .guid
                        .as_ref()
                        .map(|g| g.as_str().to_ascii_lowercase())
                    {
                        added_parent_of.insert(
                            added.added_object.file_id,
                            AddedParentLink {
                                owner_instance: *file_id,
                                target_guid: guid,
                                target_fid: added.target_corresponding_source.file_id,
                            },
                        );
                    }
                }
            }
            TypedObject::Stripped(link) => {
                let inst_ref = link.prefab_instance().map_err(|e| {
                    VrmError::new(
                        VrmErrorKind::InvalidPrefab,
                        format!(
                            "strippedのm_PrefabInstanceを読めない（{}）: {e}",
                            crate::escape_diag(label),
                        ),
                    )
                })?;
                if let Some(guid) = link
                    .corresponding_source
                    .guid
                    .as_ref()
                    .map(|g| g.as_str().to_ascii_lowercase())
                {
                    let target = StrippedTarget {
                        source_guid: guid,
                        source_fid: link.corresponding_source.file_id,
                        instance_fid: inst_ref.file_id,
                    };
                    match *class_id {
                        4 => {
                            stripped_tr.insert(*file_id, target);
                        }
                        1 => {
                            stripped_go.insert(*file_id, target);
                        }
                        _ => {
                            stripped_other.insert(*file_id, target);
                        }
                    }
                }
            }
            TypedObject::SkinnedMeshRenderer(smr) => smrs.push((*file_id, smr)),
            TypedObject::MeshRenderer(mr) => mesh_renderers.push((*file_id, mr)),
            TypedObject::MeshFilter(mf) => {
                mesh_filters_by_go
                    .entry(mf.game_object.file_id)
                    .or_default()
                    .push((*file_id, mf));
            }
            _ => {}
        }
    }

    Ok(ParsedLayer {
        label,
        local_go,
        local_tr,
        tr_of_go,
        stripped_tr,
        stripped_go,
        stripped_other,
        instances,
        instance_at,
        added_parent_of,
        smrs,
        mesh_renderers,
        mesh_filters_by_go,
    })
}

impl<'a> ParsedLayer<'a> {
    fn find_stripped(&self, file_id: FileId) -> Option<&StrippedTarget> {
        self.stripped_tr
            .get(&file_id)
            .or_else(|| self.stripped_go.get(&file_id))
            .or_else(|| self.stripped_other.get(&file_id))
    }
}

fn guid_matches(reference: &ObjectRef, target_guid: &str) -> bool {
    !reference.is_null()
        && reference
            .guid
            .as_ref()
            .is_some_and(|g| g.as_str().eq_ignore_ascii_case(target_guid))
}

fn resolve_source_kind(reference: &ObjectRef, index: &AssetIndex) -> Option<AssetKind> {
    if reference.is_null() {
        return None;
    }
    let guid = reference.guid.as_ref()?;
    let key = Guid::parse(guid.as_str()).ok()?;
    index.get(&key).map(|record| record.kind)
}

fn fbx_extension(relative_path: &str) -> bool {
    let file = relative_path.rsplit('/').next().unwrap_or(relative_path);
    match file.rsplit_once('.') {
        Some((stem, ext)) => !stem.is_empty() && ext.eq_ignore_ascii_case("fbx"),
        None => false,
    }
}

fn main_fbx_node_active(path: &str, main_go_active: &HashMap<String, bool>) -> bool {
    let mut rest = path;
    loop {
        if main_go_active.get(rest).is_some_and(|active| !active) {
            return false;
        }
        let Some((parent, _)) = rest.rsplit_once('/') else {
            return main_go_active.get("").is_none_or(|active| *active);
        };
        rest = parent;
    }
}

/// ルートPrefab内のネストPrefabインスタンスを親依存順に展開する。
#[allow(clippy::too_many_arguments)]
fn expand_root_nested_instance(
    inst_id: FileId,
    inst: &PrefabInstance,
    layer: &ParsedLayer<'_>,
    main_nodes: &[super::PrefabHierarchyNode],
    main_path_to_fbx_idx: &HashMap<&str, usize>,
    main_tr_to_fbx_idx: &HashMap<FileId, usize>,
    main_go_active: &HashMap<String, bool>,
    local_cache: &mut HashMap<FileId, MountPoint>,
    local_visiting: &mut HashSet<FileId>,
    inst_mount_cache: &mut InstMountCache,
    inst_visiting: &mut HashSet<FileId>,
    claimed_nodes: &mut HashSet<usize>,
    model: &VrmModel,
    ctx: &mut NestedContext<'_>,
) -> Result<(), VrmError> {
    if !inst_visiting.insert(inst_id) {
        return Err(VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!(
                "ネストPrefabの配置親子に循環がある（{}）",
                crate::escape_diag(layer.label)
            ),
        ));
    }
    let mount = resolve_root_instance_mount(
        inst_id,
        inst,
        layer,
        main_nodes,
        main_path_to_fbx_idx,
        main_tr_to_fbx_idx,
        main_go_active,
        local_cache,
        local_visiting,
        inst_mount_cache,
        inst_visiting,
        claimed_nodes,
        model,
        ctx,
    )?;

    let guid_text = inst
        .source
        .guid
        .as_ref()
        .expect("Prefab種別確認済み")
        .as_str()
        .to_ascii_lowercase();
    let key = Guid::parse(&guid_text).expect("GUID形式確認済み");
    let record = ctx.input.index.get(&key).expect("索引確認済み");
    let rel_path = record.path.clone();

    let overrides = collect_instance_overrides(inst_id, inst, layer.label)?;
    let mut visiting_prefabs = HashSet::new();
    let mut created_source_nodes = HashMap::new();
    expand_prefab_instance(
        &guid_text,
        &rel_path,
        mount,
        &overrides,
        1,
        &mut visiting_prefabs,
        &mut created_source_nodes,
        claimed_nodes,
        model,
        ctx,
    )?;

    inst_visiting.remove(&inst_id);
    inst_mount_cache.insert(
        inst_id,
        (mount.parent_node, mount.parent_active, created_source_nodes),
    );
    Ok(())
}

/// ルートPrefab内でのネストインスタンスのマウント先を解決する。
#[allow(clippy::too_many_arguments)]
fn resolve_root_instance_mount(
    inst_id: FileId,
    inst: &PrefabInstance,
    layer: &ParsedLayer<'_>,
    main_nodes: &[super::PrefabHierarchyNode],
    main_path_to_fbx_idx: &HashMap<&str, usize>,
    main_tr_to_fbx_idx: &HashMap<FileId, usize>,
    main_go_active: &HashMap<String, bool>,
    local_cache: &mut HashMap<FileId, MountPoint>,
    local_visiting: &mut HashSet<FileId>,
    inst_mount_cache: &mut InstMountCache,
    inst_visiting: &mut HashSet<FileId>,
    claimed_nodes: &mut HashSet<usize>,
    model: &VrmModel,
    ctx: &mut NestedContext<'_>,
) -> Result<MountPoint, VrmError> {
    let parent_ref = inst.transform_parent().map_err(|e| {
        VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!(
                "PrefabInstanceのm_TransformParentを読めない（{}）: {e}",
                crate::escape_diag(layer.label)
            ),
        )
    })?;
    if !parent_ref.is_null() {
        return resolve_root_transform_mount(
            parent_ref.file_id,
            layer,
            main_nodes,
            main_path_to_fbx_idx,
            main_tr_to_fbx_idx,
            main_go_active,
            local_cache,
            local_visiting,
            inst_mount_cache,
            inst_visiting,
            claimed_nodes,
            model,
            ctx,
        );
    }
    // `m_TransformParent` が0の場合は、自身のstripped Transformが別のPrefabInstanceの
    // `m_AddedGameObjects` に入っているか確認する。
    for (&stripped_fid, target) in &layer.stripped_tr {
        if target.instance_fid == inst_id
            && let Some(link) = layer.added_parent_of.get(&stripped_fid)
        {
            return resolve_added_link_mount(
                link,
                layer,
                main_nodes,
                main_path_to_fbx_idx,
                main_tr_to_fbx_idx,
                main_go_active,
                local_cache,
                local_visiting,
                inst_mount_cache,
                inst_visiting,
                claimed_nodes,
                model,
                ctx,
            );
        }
    }
    Ok(MountPoint {
        parent_node: None,
        parent_active: main_go_active.get("").copied().unwrap_or(true),
    })
}

/// ルートPrefab上の任意のTransform `FileId` をマウント先へ解決する。
#[allow(clippy::too_many_arguments)]
fn resolve_root_transform_mount(
    tr_fid: FileId,
    layer: &ParsedLayer<'_>,
    main_nodes: &[super::PrefabHierarchyNode],
    main_path_to_fbx_idx: &HashMap<&str, usize>,
    main_tr_to_fbx_idx: &HashMap<FileId, usize>,
    main_go_active: &HashMap<String, bool>,
    local_cache: &mut HashMap<FileId, MountPoint>,
    local_visiting: &mut HashSet<FileId>,
    inst_mount_cache: &mut InstMountCache,
    inst_visiting: &mut HashSet<FileId>,
    claimed_nodes: &mut HashSet<usize>,
    model: &VrmModel,
    ctx: &mut NestedContext<'_>,
) -> Result<MountPoint, VrmError> {
    if let Some(cached) = local_cache.get(&tr_fid) {
        return Ok(*cached);
    }
    // 1. stripped Transform の場合。
    if let Some(stripped) = layer.find_stripped(tr_fid) {
        if stripped.source_guid == ctx.input.main_fbx_guid {
            return resolve_main_fbx_mount(
                stripped.source_fid,
                main_nodes,
                main_tr_to_fbx_idx,
                main_go_active,
            );
        }
        // 別のネストPrefabインスタンス配下のstripped Transformなら先にその親を展開する。
        if let Some(&parent_inst_idx) = layer.instance_at.get(&stripped.instance_fid) {
            let (parent_inst_id, parent_inst) = layer.instances[parent_inst_idx];
            if !inst_mount_cache.contains_key(&parent_inst_id) {
                expand_root_nested_instance(
                    parent_inst_id,
                    parent_inst,
                    layer,
                    main_nodes,
                    main_path_to_fbx_idx,
                    main_tr_to_fbx_idx,
                    main_go_active,
                    local_cache,
                    local_visiting,
                    inst_mount_cache,
                    inst_visiting,
                    claimed_nodes,
                    model,
                    ctx,
                )?;
            }
            if let Some((_, active, map)) = inst_mount_cache.get(&parent_inst_id)
                && let Some(&node_idx) = map.get(&stripped.source_fid)
            {
                return Ok(MountPoint {
                    parent_node: Some(node_idx),
                    parent_active: *active,
                });
            }
        }
        return Err(VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!(
                "ネストPrefabの親Transform（文書{tr_fid}）を解決できない（{}）",
                crate::escape_diag(layer.label),
            ),
        ));
    }

    // 2. ルートPrefab内のローカル Transform の場合。
    let Some(tr) = layer.local_tr.get(&tr_fid) else {
        return Err(VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!(
                "ネストPrefabの親Transform（文書{tr_fid}）が見つからない（{}）",
                crate::escape_diag(layer.label),
            ),
        ));
    };
    let Some(go) = layer.local_go.get(&tr.game_object.file_id) else {
        return Err(VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!(
                "ローカルTransform（文書{tr_fid}）のGameObjectが見つからない（{}）",
                crate::escape_diag(layer.label),
            ),
        ));
    };
    if !local_visiting.insert(tr_fid) {
        return Err(VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!(
                "Prefab内のTransform親子に循環がある（{}）",
                crate::escape_diag(layer.label),
            ),
        ));
    }

    // Direct modeで主FBXのノード階層に直接一致するか確認する。
    if let Some(direct_mount) =
        try_match_direct_main_node(tr_fid, layer, main_nodes, main_path_to_fbx_idx)
    {
        local_visiting.remove(&tr_fid);
        local_cache.insert(tr_fid, direct_mount);
        return Ok(direct_mount);
    }

    let parent_mount = if !tr.father.is_null() {
        resolve_root_transform_mount(
            tr.father.file_id,
            layer,
            main_nodes,
            main_path_to_fbx_idx,
            main_tr_to_fbx_idx,
            main_go_active,
            local_cache,
            local_visiting,
            inst_mount_cache,
            inst_visiting,
            claimed_nodes,
            model,
            ctx,
        )?
    } else if let Some(link) = layer.added_parent_of.get(&tr_fid) {
        resolve_added_link_mount(
            link,
            layer,
            main_nodes,
            main_path_to_fbx_idx,
            main_tr_to_fbx_idx,
            main_go_active,
            local_cache,
            local_visiting,
            inst_mount_cache,
            inst_visiting,
            claimed_nodes,
            model,
            ctx,
        )?
    } else {
        MountPoint {
            parent_node: None,
            parent_active: main_go_active.get("").copied().unwrap_or(true),
        }
    };

    let local_mat =
        matrix::unity_local_to_vrm_matrix(tr.position, tr.rotation, tr.scale, ctx.main_root_scale)
            .ok_or_else(|| {
                VrmError::new(
                    VrmErrorKind::InvalidPrefab,
                    format!(
                        "ローカルTransform（文書{tr_fid}）の姿勢が不正である（{}）",
                        crate::escape_diag(layer.label),
                    ),
                )
            })?;
    let node_idx = ctx.push_node(VrmNode {
        name: go.name.clone(),
        parent: parent_mount.parent_node,
        children: Vec::new(),
        local: local_mat,
    })?;
    let mount = MountPoint {
        parent_node: Some(node_idx),
        parent_active: parent_mount.parent_active && go.active,
    };
    local_visiting.remove(&tr_fid);
    local_cache.insert(tr_fid, mount);
    Ok(mount)
}

#[allow(clippy::too_many_arguments)]
fn resolve_added_link_mount(
    link: &AddedParentLink,
    layer: &ParsedLayer<'_>,
    main_nodes: &[super::PrefabHierarchyNode],
    main_path_to_fbx_idx: &HashMap<&str, usize>,
    main_tr_to_fbx_idx: &HashMap<FileId, usize>,
    main_go_active: &HashMap<String, bool>,
    local_cache: &mut HashMap<FileId, MountPoint>,
    local_visiting: &mut HashSet<FileId>,
    inst_mount_cache: &mut InstMountCache,
    inst_visiting: &mut HashSet<FileId>,
    claimed_nodes: &mut HashSet<usize>,
    model: &VrmModel,
    ctx: &mut NestedContext<'_>,
) -> Result<MountPoint, VrmError> {
    if link.target_guid == ctx.input.main_fbx_guid {
        return resolve_main_fbx_mount(
            link.target_fid,
            main_nodes,
            main_tr_to_fbx_idx,
            main_go_active,
        );
    }
    if !inst_mount_cache.contains_key(&link.owner_instance)
        && let Some(&parent_inst_idx) = layer.instance_at.get(&link.owner_instance)
    {
        let (parent_inst_id, parent_inst) = layer.instances[parent_inst_idx];
        expand_root_nested_instance(
            parent_inst_id,
            parent_inst,
            layer,
            main_nodes,
            main_path_to_fbx_idx,
            main_tr_to_fbx_idx,
            main_go_active,
            local_cache,
            local_visiting,
            inst_mount_cache,
            inst_visiting,
            claimed_nodes,
            model,
            ctx,
        )?;
    }
    if let Some((_, active, map)) = inst_mount_cache.get(&link.owner_instance)
        && let Some(&node_idx) = map.get(&link.target_fid)
    {
        return Ok(MountPoint {
            parent_node: Some(node_idx),
            parent_active: *active,
        });
    }
    Err(VrmError::new(
        VrmErrorKind::InvalidPrefab,
        format!(
            "追加先の親Transform（{}）を解決できない（{}）",
            link.target_fid,
            crate::escape_diag(layer.label),
        ),
    ))
}

fn resolve_main_fbx_mount(
    target_fid: FileId,
    main_nodes: &[super::PrefabHierarchyNode],
    main_tr_to_fbx_idx: &HashMap<FileId, usize>,
    main_go_active: &HashMap<String, bool>,
) -> Result<MountPoint, VrmError> {
    if target_fid == transform_file_id("") {
        return Ok(MountPoint {
            parent_node: None,
            parent_active: main_go_active.get("").copied().unwrap_or(true),
        });
    }
    let Some(&fbx_idx) = main_tr_to_fbx_idx.get(&target_fid) else {
        return Err(VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!("主FBXの親Transform（fileID {target_fid}）が階層に無い"),
        ));
    };
    let path = &main_nodes[fbx_idx].path;
    Ok(MountPoint {
        parent_node: Some(fbx_idx),
        parent_active: main_fbx_node_active(path, main_go_active),
    })
}

/// Direct ModeのルートPrefabで、ローカルTransform鎖が主FBXノードパスと一致する場合に解決する。
fn try_match_direct_main_node(
    tr_fid: FileId,
    layer: &ParsedLayer<'_>,
    main_nodes: &[super::PrefabHierarchyNode],
    main_path_to_fbx_idx: &HashMap<&str, usize>,
) -> Option<MountPoint> {
    let mut names = Vec::new();
    let mut actives = Vec::new();
    let mut current = tr_fid;
    let mut visited = HashSet::new();
    loop {
        if !visited.insert(current) {
            return None;
        }
        let tr = layer.local_tr.get(&current)?;
        let go = layer.local_go.get(&tr.game_object.file_id)?;
        names.push(go.name.as_str());
        actives.push(go.active);
        if tr.father.is_null() {
            break;
        }
        current = tr.father.file_id;
    }
    names.reverse();
    let full = names.join("/");
    let without_root = (names.len() >= 2).then(|| names[1..].join("/"));
    let fbx_idx = main_path_to_fbx_idx
        .get(full.as_str())
        .copied()
        .or_else(|| {
            without_root
                .as_deref()
                .and_then(|p| main_path_to_fbx_idx.get(p).copied())
        })?;
    let all_active = actives.into_iter().all(|a| a);
    let _ = main_nodes[fbx_idx];
    Some(MountPoint {
        parent_node: Some(fbx_idx),
        parent_active: all_active,
    })
}

/// `PrefabInstance` 自身の `m_Modifications` を `ScopedOverride` 列に読む。
fn collect_instance_overrides(
    inst_id: FileId,
    inst: &PrefabInstance,
    label: &str,
) -> Result<Vec<ScopedOverride>, VrmError> {
    let modifications = inst.modifications().map_err(|e| {
        VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!(
                "Prefabの改変内容を読めない（{}・実体{inst_id}）: {e}",
                crate::escape_diag(label),
            ),
        )
    })?;
    let mut seen: HashSet<(String, FileId, String)> = HashSet::new();
    let mut out = Vec::with_capacity(modifications.len());
    for m in modifications {
        let Some(guid) = m
            .target
            .guid
            .as_ref()
            .map(|g| g.as_str().to_ascii_lowercase())
        else {
            continue;
        };
        if !seen.insert((guid.clone(), m.target.file_id, m.property_path.clone())) {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!(
                    "同じ対象への上書きが重複するため先勝ちにしない: {}（対象{}）（{}）",
                    crate::escape_diag(&m.property_path),
                    m.target.file_id,
                    crate::escape_diag(label),
                ),
            ));
        }
        out.push(ScopedOverride {
            target_guid: guid,
            target_file_id: m.target.file_id,
            property_path: m.property_path,
            value: m.value,
            object_reference: m.object_reference,
        });
    }
    Ok(out)
}

/// ネストされた `.prefab` インスタンスを再帰的に展開する。
#[allow(clippy::too_many_arguments)]
fn expand_prefab_instance(
    prefab_guid: &str,
    prefab_rel_path: &str,
    mount: MountPoint,
    incoming_overrides: &[ScopedOverride],
    depth: usize,
    visiting_prefabs: &mut HashSet<String>,
    created_source_nodes: &mut HashMap<FileId, usize>,
    claimed_nodes: &mut HashSet<usize>,
    model: &VrmModel,
    ctx: &mut NestedContext<'_>,
) -> Result<(), VrmError> {
    if depth > ctx.input.limits.max_depth {
        return Err(VrmError::new(
            VrmErrorKind::LimitExceeded,
            format!(
                "ネストPrefabの深さが上限を超えた: 深さ{depth}（上限{}）（Assets/{prefab_rel_path}）",
                ctx.input.limits.max_depth,
            ),
        ));
    }
    if ctx.instance_count >= ctx.input.limits.max_instances {
        return Err(VrmError::new(
            VrmErrorKind::LimitExceeded,
            format!(
                "ネストPrefabの展開数が上限を超えた: {}（上限{}）",
                ctx.instance_count, ctx.input.limits.max_instances,
            ),
        ));
    }
    ctx.instance_count += 1;

    if !visiting_prefabs.insert(prefab_guid.to_owned()) {
        return Err(VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!("ネストPrefabに循環参照がある: Assets/{prefab_rel_path}"),
        ));
    }

    let asset_label = format!("Assets/{prefab_rel_path}");
    let prefab_file = ctx.load_prefab_cached(prefab_rel_path)?;
    let decoded = decode_all(&prefab_file, &asset_label)?;
    let layer = build_parsed_layer(&decoded, &asset_label)?;

    // 上位からのオーバーライドを、このPrefab宛（stripped書き換え・ローカル適用）と
    // 下位へのパススルーに振り分ける。
    let mut local_overrides: Vec<ScopedOverride> = Vec::new();
    let mut per_child_overrides: HashMap<FileId, Vec<ScopedOverride>> = HashMap::new();
    let mut passthrough_overrides: Vec<ScopedOverride> = Vec::new();
    for over in incoming_overrides {
        if over.target_guid == prefab_guid {
            if let Some(stripped) = layer.find_stripped(over.target_file_id) {
                let rewritten = ScopedOverride {
                    target_guid: stripped.source_guid.clone(),
                    target_file_id: stripped.source_fid,
                    property_path: over.property_path.clone(),
                    value: over.value.clone(),
                    object_reference: over.object_reference.clone(),
                };
                per_child_overrides
                    .entry(stripped.instance_fid)
                    .or_default()
                    .push(rewritten);
            } else {
                local_overrides.push(over.clone());
            }
        } else {
            passthrough_overrides.push(over.clone());
        }
    }

    // このPrefab内のローカルGameObject・Transform・Rendererを仮想ノードへ構築する。
    let (local_mounts, local_source_nodes) = build_local_hierarchy_and_renderers(
        &layer,
        mount,
        &local_overrides,
        claimed_nodes,
        model,
        ctx,
    )?;
    for (fid, node_idx) in local_source_nodes {
        created_source_nodes.insert(fid, node_idx);
    }

    // このPrefab内の子PrefabInstanceを展開する（親の依存順に展開するためキャッシュ付き）。
    let mut child_inst_cache: HashMap<FileId, (MountPoint, HashMap<FileId, usize>)> =
        HashMap::new();
    let mut child_visiting: HashSet<FileId> = HashSet::new();

    for (child_id, child_inst) in &layer.instances {
        if child_inst_cache.contains_key(child_id) {
            continue;
        }
        expand_child_instance_in_prefab(
            *child_id,
            child_inst,
            &layer,
            mount,
            &local_mounts,
            &per_child_overrides,
            &passthrough_overrides,
            depth,
            visiting_prefabs,
            &mut child_inst_cache,
            &mut child_visiting,
            claimed_nodes,
            model,
            ctx,
        )?;
    }
    // stripped参照先を `created_source_nodes` にマップして親からの参照解決を可能にする。
    for (&stripped_fid, target) in &layer.stripped_tr {
        if let Some((_, child_nodes)) = child_inst_cache.get(&target.instance_fid)
            && let Some(&node_idx) = child_nodes.get(&target.source_fid)
        {
            created_source_nodes.insert(stripped_fid, node_idx);
        }
    }

    visiting_prefabs.remove(prefab_guid);
    Ok(())
}

/// 子Prefab内の `PrefabInstance` を親依存順に展開する。
#[allow(clippy::too_many_arguments)]
fn expand_child_instance_in_prefab(
    child_id: FileId,
    child_inst: &PrefabInstance,
    layer: &ParsedLayer<'_>,
    prefab_mount: MountPoint,
    local_mounts: &HashMap<FileId, MountPoint>,
    per_child_overrides: &HashMap<FileId, Vec<ScopedOverride>>,
    passthrough_overrides: &[ScopedOverride],
    depth: usize,
    visiting_prefabs: &mut HashSet<String>,
    child_inst_cache: &mut HashMap<FileId, (MountPoint, HashMap<FileId, usize>)>,
    child_visiting: &mut HashSet<FileId>,
    claimed_nodes: &mut HashSet<usize>,
    model: &VrmModel,
    ctx: &mut NestedContext<'_>,
) -> Result<(), VrmError> {
    if !child_visiting.insert(child_id) {
        return Err(VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!(
                "ネストPrefab内の配置親子に循環がある（{}）",
                crate::escape_diag(layer.label),
            ),
        ));
    }

    let child_mount = resolve_child_instance_mount(
        child_id,
        child_inst,
        layer,
        prefab_mount,
        local_mounts,
        per_child_overrides,
        passthrough_overrides,
        depth,
        visiting_prefabs,
        child_inst_cache,
        child_visiting,
        claimed_nodes,
        model,
        ctx,
    )?;

    let Some(guid) = child_inst
        .source
        .guid
        .as_ref()
        .map(|g| g.as_str().to_ascii_lowercase())
    else {
        child_visiting.remove(&child_id);
        return Ok(());
    };
    let Ok(guid_key) = Guid::parse(&guid) else {
        ctx.report.warnings.push(format!(
            "ネストPrefab内の不正GUID参照をスキップした: {guid}（{}）",
            layer.label,
        ));
        child_visiting.remove(&child_id);
        return Ok(());
    };
    let Some(record) = ctx.input.index.get(&guid_key) else {
        ctx.report.warnings.push(format!(
            "ネストPrefab内の索引外参照をスキップした: {guid}（{}）",
            layer.label,
        ));
        child_visiting.remove(&child_id);
        return Ok(());
    };

    // 内側（このインスタンス自身）→ 外側（上位Prefabからの指定・パススルー）の順に並べる。
    let mut combined = collect_instance_overrides(child_id, child_inst, layer.label)?;
    if let Some(targeted) = per_child_overrides.get(&child_id) {
        combined.extend(targeted.iter().cloned());
    }
    combined.extend(passthrough_overrides.iter().cloned());

    let rel_path = record.path.clone();
    let kind = record.kind;
    let mut child_nodes = HashMap::new();
    if kind == AssetKind::Prefab {
        expand_prefab_instance(
            &guid,
            &rel_path,
            child_mount,
            &combined,
            depth + 1,
            visiting_prefabs,
            &mut child_nodes,
            claimed_nodes,
            model,
            ctx,
        )?;
    } else if fbx_extension(&rel_path) {
        let default_name = prefab_stem_from_label(layer.label, &rel_path);
        expand_fbx_instance(
            &guid,
            &rel_path,
            &default_name,
            child_mount,
            &combined,
            &mut child_nodes,
            claimed_nodes,
            model,
            ctx,
        )?;
    } else {
        ctx.report.warnings.push(format!(
            "ネストPrefab内の非FBX参照をスキップした: Assets/{rel_path}（{}）",
            layer.label,
        ));
    }

    child_visiting.remove(&child_id);
    child_inst_cache.insert(child_id, (child_mount, child_nodes));
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn resolve_child_instance_mount(
    child_id: FileId,
    child_inst: &PrefabInstance,
    layer: &ParsedLayer<'_>,
    prefab_mount: MountPoint,
    local_mounts: &HashMap<FileId, MountPoint>,
    per_child_overrides: &HashMap<FileId, Vec<ScopedOverride>>,
    passthrough_overrides: &[ScopedOverride],
    depth: usize,
    visiting_prefabs: &mut HashSet<String>,
    child_inst_cache: &mut HashMap<FileId, (MountPoint, HashMap<FileId, usize>)>,
    child_visiting: &mut HashSet<FileId>,
    claimed_nodes: &mut HashSet<usize>,
    model: &VrmModel,
    ctx: &mut NestedContext<'_>,
) -> Result<MountPoint, VrmError> {
    let parent_ref = child_inst.transform_parent().map_err(|e| {
        VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!(
                "ネストPrefab内のm_TransformParentを読めない（{}）: {e}",
                crate::escape_diag(layer.label)
            ),
        )
    })?;
    if !parent_ref.is_null() {
        let fid = parent_ref.file_id;
        if let Some(mount) = local_mounts.get(&fid) {
            return Ok(*mount);
        }
        if let Some(stripped) = layer.find_stripped(fid)
            && let Some(&parent_idx) = layer.instance_at.get(&stripped.instance_fid)
        {
            let (parent_id, parent_inst) = layer.instances[parent_idx];
            if !child_inst_cache.contains_key(&parent_id) {
                expand_child_instance_in_prefab(
                    parent_id,
                    parent_inst,
                    layer,
                    prefab_mount,
                    local_mounts,
                    per_child_overrides,
                    passthrough_overrides,
                    depth,
                    visiting_prefabs,
                    child_inst_cache,
                    child_visiting,
                    claimed_nodes,
                    model,
                    ctx,
                )?;
            }
            if let Some((parent_mount, nodes)) = child_inst_cache.get(&parent_id)
                && let Some(&node_idx) = nodes.get(&stripped.source_fid)
            {
                return Ok(MountPoint {
                    parent_node: Some(node_idx),
                    parent_active: parent_mount.parent_active,
                });
            }
        }
        return Err(VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!(
                "ネストPrefab内の親Transform（文書{fid}）を解決できない（{}）",
                crate::escape_diag(layer.label),
            ),
        ));
    }

    for (&stripped_fid, target) in &layer.stripped_tr {
        if target.instance_fid == child_id
            && let Some(link) = layer.added_parent_of.get(&stripped_fid)
            && let Some(&parent_idx) = layer.instance_at.get(&link.owner_instance)
        {
            let (parent_id, parent_inst) = layer.instances[parent_idx];
            if !child_inst_cache.contains_key(&parent_id) {
                expand_child_instance_in_prefab(
                    parent_id,
                    parent_inst,
                    layer,
                    prefab_mount,
                    local_mounts,
                    per_child_overrides,
                    passthrough_overrides,
                    depth,
                    visiting_prefabs,
                    child_inst_cache,
                    child_visiting,
                    claimed_nodes,
                    model,
                    ctx,
                )?;
            }
            if let Some((parent_mount, nodes)) = child_inst_cache.get(&parent_id)
                && let Some(&node_idx) = nodes.get(&link.target_fid)
            {
                return Ok(MountPoint {
                    parent_node: Some(node_idx),
                    parent_active: parent_mount.parent_active,
                });
            }
        }
    }

    Ok(prefab_mount)
}

/// 単一Prefab層内のローカル `Transform`/`GameObject` と直接記述レンダラを解決する。
fn build_local_hierarchy_and_renderers(
    layer: &ParsedLayer<'_>,
    prefab_mount: MountPoint,
    local_overrides: &[ScopedOverride],
    claimed_nodes: &mut HashSet<usize>,
    model: &VrmModel,
    ctx: &mut NestedContext<'_>,
) -> Result<LocalBuildResult, VrmError> {
    // ローカルオブジェクト宛のオーバーライドを集計する。
    let mut go_active_over: HashMap<FileId, bool> = HashMap::new();
    let mut tr_over: HashMap<FileId, TrsPatch> = HashMap::new();
    let mut renderer_enabled_over: HashMap<FileId, bool> = HashMap::new();
    let mut material_over: HashMap<(FileId, usize), (String, FileId)> = HashMap::new();

    for over in local_overrides {
        let fid = over.target_file_id;
        if over.property_path == "m_IsActive" {
            let active = read_flag_value(&over.value, layer.label, &over.property_path, fid)?;
            go_active_over.insert(fid, active);
        } else if over.property_path == "m_Enabled" {
            let enabled = read_flag_value(&over.value, layer.label, &over.property_path, fid)?;
            renderer_enabled_over.insert(fid, enabled);
        } else if let Some(slot) = material_slot_index(&over.property_path) {
            let (guid, mat_fid) = read_material_override(over, layer.label, slot)?;
            material_over.insert((fid, slot), (guid, mat_fid));
        } else {
            apply_trs_patch_item(&mut tr_over, fid, over, layer.label)?;
        }
    }

    let mut local_mounts: HashMap<FileId, MountPoint> = HashMap::new();
    let mut local_source_nodes: HashMap<FileId, usize> = HashMap::new();
    let mut visiting = HashSet::new();

    // 決定的な順序で全ローカルTransformを構築する。
    let mut tr_ids: Vec<FileId> = layer.local_tr.keys().copied().collect();
    tr_ids.sort_unstable();
    for tr_fid in tr_ids {
        ensure_local_node(
            tr_fid,
            layer,
            prefab_mount,
            &go_active_over,
            &tr_over,
            &mut local_mounts,
            &mut local_source_nodes,
            &mut visiting,
            ctx,
        )?;
    }

    // ローカルの直接記述レンダラ（SkinnedMeshRenderer / MeshFilter + MeshRenderer）を変換する。
    let mut direct_renderers: Vec<(FileId, FileId, bool, &ObjectRef, &[ObjectRef])> = Vec::new();
    for &(fid, smr) in &layer.smrs {
        direct_renderers.push((
            fid,
            smr.game_object.file_id,
            smr.enabled,
            &smr.mesh,
            &smr.materials,
        ));
    }
    for &(fid, mr) in &layer.mesh_renderers {
        if let Some(filters) = layer.mesh_filters_by_go.get(&mr.game_object.file_id) {
            if filters.len() > 1 {
                return Err(VrmError::new(
                    VrmErrorKind::InvalidPrefab,
                    format!(
                        "1つのゲームオブジェクトに複数のMeshFilterがあるため先勝ちにしない: {}（{}）",
                        mr.game_object.file_id,
                        crate::escape_diag(layer.label),
                    ),
                ));
            }
            if let Some((_, mf)) = filters.first() {
                direct_renderers.push((
                    fid,
                    mr.game_object.file_id,
                    mr.enabled,
                    &mf.mesh,
                    &mr.materials,
                ));
            }
        }
    }

    for (renderer_fid, go_fid, base_enabled, mesh_ref, base_materials) in direct_renderers {
        if mesh_ref.is_null() {
            continue;
        }
        let Some(mesh_guid) = mesh_ref
            .guid
            .as_ref()
            .map(|g| g.as_str().to_ascii_lowercase())
        else {
            continue;
        };
        let Ok(guid_key) = Guid::parse(&mesh_guid) else {
            ctx.report.warnings.push(format!(
                "ネストPrefab内の不正メッシュGUIDをスキップした: {mesh_guid}（{}）",
                layer.label,
            ));
            continue;
        };
        let Some(fbx_record) = ctx.input.index.get(&guid_key) else {
            ctx.report.warnings.push(format!(
                "ネストPrefab内の索引外メッシュ参照をスキップした: {mesh_guid}（{}）",
                layer.label,
            ));
            continue;
        };
        let fbx_rel_path = fbx_record.path.clone();
        if !fbx_extension(&fbx_rel_path) {
            ctx.report.warnings.push(format!(
                "ネストPrefab内の非FBXメッシュ参照をスキップした: Assets/{fbx_rel_path}（{}）",
                layer.label,
            ));
            continue;
        }
        let fbx_legacy_names = fbx_record.meta.legacy_recycle_names();
        let fbx_bytes = ctx.load_fbx_bytes_cached(&fbx_rel_path)?;
        let sub_fbx = FbxFile::parse(&fbx_bytes, ctx.input.fbx_options).map_err(|error| {
            VrmError::new(
                match error.kind() {
                    FbxErrorKind::LimitExceeded => VrmErrorKind::LimitExceeded,
                    _ => VrmErrorKind::InvalidStructure,
                },
                format!("ネストFBXを解析できない（Assets/{fbx_rel_path}）: {error}"),
            )
        })?;

        let (mesh_idx, owner_idx) =
            resolve_direct_fbx_mesh(&sub_fbx, mesh_ref.file_id, &fbx_legacy_names, layer.label)?;
        let fbx_mesh = &sub_fbx.meshes()[mesh_idx];
        let owner = &sub_fbx.models()[owner_idx];

        let Some(&tr_fid) = layer.tr_of_go.get(&go_fid) else {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!(
                    "直接記述レンダラのGameObject（{go_fid}）にTransformがない（{}）",
                    crate::escape_diag(layer.label),
                ),
            ));
        };
        let node_mount = local_mounts[&tr_fid];
        let enabled = renderer_enabled_over
            .get(&renderer_fid)
            .copied()
            .unwrap_or(base_enabled);
        if !(node_mount.parent_active && enabled) {
            ctx.report.excluded_meshes += 1;
            ctx.report.excluded_names.push(owner.name.clone());
            continue;
        }
        let Some(mount_node) = node_mount.parent_node else {
            return Err(VrmError::new(
                VrmErrorKind::InvalidStructure,
                "ローカルレンダラのノードが作成されていない",
            ));
        };

        // 材質スロットを解決する。
        let slot_count = owner.materials.len();
        if base_materials.len() != slot_count {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!(
                    "レンダラの材質数とメッシュの材質スロット数が一致しない: {}（{}件に対してスロット{}件）（{}）",
                    crate::escape_diag(&owner.name),
                    base_materials.len(),
                    slot_count,
                    crate::escape_diag(layer.label),
                ),
            ));
        }
        let renderer_path = format!("{}/{}", layer.label, owner.name);
        let mut slot_keys = Vec::with_capacity(slot_count);
        for (slot, reference) in base_materials.iter().enumerate() {
            let (mat_guid, mat_fid) = if let Some(found) = material_over.get(&(renderer_fid, slot))
            {
                found.clone()
            } else {
                if reference.is_null() || reference.guid.is_none() {
                    return Err(VrmError::new(
                        VrmErrorKind::InvalidPrefab,
                        format!(
                            "材質の参照がnullまたは材質GUID不在: {} スロット{slot}（{}）",
                            crate::escape_diag(&owner.name),
                            crate::escape_diag(layer.label),
                        ),
                    ));
                }
                (
                    reference
                        .guid
                        .as_ref()
                        .expect("確認済み")
                        .as_str()
                        .to_owned(),
                    reference.file_id,
                )
            };
            slot_keys.push(ctx.register_material(&mat_guid, mat_fid, &renderer_path));
        }

        let sub_axis = axis_transform(&sub_fbx)?;
        let sub_unit_scale = sub_fbx.unit_scale_factor() / 100.0;
        if !sub_unit_scale.is_finite() || sub_unit_scale <= 0.0 {
            return Err(VrmError::new(
                VrmErrorKind::UnsupportedUnitScale,
                "副FBXの単位換算係数が正の有限値でない",
            ));
        }
        let alpha = sub_unit_scale / ctx.main_root_scale;
        let geometric = owner.transform.geometric_matrix().map_err(|error| {
            let mut out = wrap_fbx_error(error);
            if out.object_id().is_none() {
                out = out.with_object(owner.id);
            }
            out
        })?;
        let data = sub_fbx.mesh_data(fbx_mesh.id).map_err(wrap_fbx_error)?;

        let skin_idx_opt = sub_fbx
            .skins()
            .iter()
            .position(|skin| skin.geometry == fbx_mesh.id);
        let mut vrm_mesh = if let Some(skin_idx) = skin_idx_opt {
            let skin = &sub_fbx.skins()[skin_idx];
            let skin_data = sub_fbx.skin_data(skin.id).map_err(wrap_fbx_error)?;
            let sub_bind = bind_world_matrices_for_nested(&sub_fbx)?;
            let sub_node_worlds = apply_node_worlds(&sub_axis, &sub_bind);
            let sub_mesh_worlds = apply_mesh_worlds(&sub_axis, &sub_bind, &sub_node_worlds);
            let owner_world = matrix::scale_translation(&sub_node_worlds[owner_idx], alpha);
            let inv_owner_world = matrix::inverse(&owner_world).ok_or_else(|| {
                VrmError::new(
                    VrmErrorKind::InvalidStructure,
                    "副FBX所有モデルのバインド行列が特異である",
                )
            })?;
            let mount_world = ctx.world_of(mount_node);
            let t_attach = matrix::mul(&mount_world, &inv_owner_world);
            let mesh_bake = matrix::mul(
                &t_attach,
                &matrix::mul(
                    &matrix::scale_matrix(alpha),
                    &matrix::mul(&sub_mesh_worlds[owner_idx], &geometric),
                ),
            );
            let mut sub_model_idx_by_id: HashMap<i64, usize> = HashMap::new();
            for (idx, m) in sub_fbx.models().iter().enumerate() {
                sub_model_idx_by_id.insert(m.id.value(), idx);
            }
            let sub_hier = prefab_hierarchy_paths(&sub_fbx)?;
            let sub_tr_to_idx: HashMap<FileId, usize> = sub_hier
                .iter()
                .enumerate()
                .map(|(i, n)| (transform_file_id(&n.path), i))
                .collect();
            let smr_bones = layer
                .smrs
                .iter()
                .find(|(fid, _)| *fid == renderer_fid)
                .map(|(_, smr)| smr.bones.as_slice())
                .unwrap_or(&[]);
            let final_skin_idx = build_direct_skin(
                &sub_fbx,
                skin,
                &sub_model_idx_by_id,
                &sub_node_worlds,
                alpha,
                &t_attach,
                smr_bones,
                &sub_tr_to_idx,
                &local_mounts,
                ctx,
                layer.label,
            )?;
            let skin_mesh_node = ctx.push_node(VrmNode {
                name: owner.name.clone(),
                parent: None,
                children: Vec::new(),
                local: matrix::IDENTITY,
            })?;
            claimed_nodes.insert(skin_mesh_node);
            mesh::convert_mesh(
                &sub_fbx,
                fbx_mesh,
                &data,
                owner,
                skin_mesh_node,
                &mesh_bake,
                Some((final_skin_idx, &skin_data)),
                None,
                &[],
            )?
            .mesh
        } else {
            let owner_vrm_node =
                ctx.alloc_mesh_node(mount_node, &owner.name, claimed_nodes, model)?;
            let local_bake = if sub_axis.handedness_flipped() {
                matrix::mul(
                    &matrix::scale_matrix(alpha),
                    &matrix::mul_left_mirror_x(&geometric),
                )
            } else {
                matrix::mul(&matrix::scale_matrix(alpha), &geometric)
            };
            mesh::convert_mesh(
                &sub_fbx,
                fbx_mesh,
                &data,
                owner,
                owner_vrm_node,
                &local_bake,
                None,
                None,
                &[],
            )?
            .mesh
        };
        vrm_mesh.material_slots = slot_keys;
        ctx.push_mesh(vrm_mesh)?;
    }

    Ok((local_mounts, local_source_nodes))
}

#[allow(clippy::too_many_arguments)]
fn ensure_local_node(
    tr_fid: FileId,
    layer: &ParsedLayer<'_>,
    prefab_mount: MountPoint,
    go_active_over: &HashMap<FileId, bool>,
    tr_over: &HashMap<FileId, TrsPatch>,
    local_mounts: &mut HashMap<FileId, MountPoint>,
    local_source_nodes: &mut HashMap<FileId, usize>,
    visiting: &mut HashSet<FileId>,
    ctx: &mut NestedContext<'_>,
) -> Result<MountPoint, VrmError> {
    if let Some(cached) = local_mounts.get(&tr_fid) {
        return Ok(*cached);
    }
    if !visiting.insert(tr_fid) {
        return Err(VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!(
                "ネストPrefab内のTransform親子に循環がある（{}）",
                crate::escape_diag(layer.label),
            ),
        ));
    }
    let tr = layer.local_tr[&tr_fid];
    let go_fid = tr.game_object.file_id;
    let Some(go) = layer.local_go.get(&go_fid) else {
        return Err(VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!(
                "Transform（文書{tr_fid}）のGameObjectが見つからない（{}）",
                crate::escape_diag(layer.label),
            ),
        ));
    };
    let parent_mount = if tr.father.is_null() {
        prefab_mount
    } else if layer.local_tr.contains_key(&tr.father.file_id) {
        ensure_local_node(
            tr.father.file_id,
            layer,
            prefab_mount,
            go_active_over,
            tr_over,
            local_mounts,
            local_source_nodes,
            visiting,
            ctx,
        )?
    } else {
        // 別のPrefabInstanceのstripped Transform配下にある追加GameObjectの場合は、
        // ここでは親未解決のままとせず、子インスタンス展開時より前に扱えないため失敗にする。
        return Err(VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!(
                "ローカルTransform（文書{tr_fid}）の親Transform（{}）がPrefab内に無い（{}）",
                tr.father.file_id,
                crate::escape_diag(layer.label),
            ),
        ));
    };

    let mut pos = tr.position;
    let mut rot = tr.rotation;
    let mut scale = tr.scale;
    if let Some(patch) = tr_over.get(&tr_fid) {
        patch.apply(&mut pos, &mut rot, &mut scale);
    }
    let local_mat = matrix::unity_local_to_vrm_matrix(pos, rot, scale, ctx.main_root_scale)
        .ok_or_else(|| {
            VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!(
                    "ローカルTransform（文書{tr_fid}）の姿勢が不正である（{}）",
                    crate::escape_diag(layer.label),
                ),
            )
        })?;
    let node_idx = ctx.push_node(VrmNode {
        name: go.name.clone(),
        parent: parent_mount.parent_node,
        children: Vec::new(),
        local: local_mat,
    })?;
    let active = go_active_over.get(&go_fid).copied().unwrap_or(go.active);
    let mount = MountPoint {
        parent_node: Some(node_idx),
        parent_active: parent_mount.parent_active && active,
    };
    visiting.remove(&tr_fid);
    local_mounts.insert(tr_fid, mount);
    local_source_nodes.insert(tr_fid, node_idx);
    Ok(mount)
}

#[allow(clippy::too_many_arguments)]
fn build_direct_skin(
    sub_fbx: &FbxFile<'_>,
    skin: &crate::fbx::Skin,
    sub_model_idx_by_id: &HashMap<i64, usize>,
    sub_node_worlds: &[[f64; 16]],
    alpha: f64,
    t_attach: &[f64; 16],
    smr_bones: &[ObjectRef],
    sub_tr_to_idx: &HashMap<FileId, usize>,
    local_mounts: &HashMap<FileId, MountPoint>,
    ctx: &mut NestedContext<'_>,
    label: &str,
) -> Result<usize, VrmError> {
    if smr_bones.len() != skin.clusters.len() {
        return Err(VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!(
                "直接記述SkinnedMeshRendererのm_Bones数（{}）がFBXのクラスタ数（{}）と一致しない（{}）",
                smr_bones.len(),
                skin.clusters.len(),
                crate::escape_diag(label),
            ),
        ));
    }
    let mut joints = Vec::with_capacity(skin.clusters.len());
    let mut ibms = Vec::with_capacity(skin.clusters.len());
    let mut joint_models = Vec::with_capacity(skin.clusters.len());
    for (cluster, bone_ref) in skin.clusters.iter().zip(smr_bones.iter()) {
        let Some(&joint_model_idx) = sub_model_idx_by_id.get(&cluster.joint.value()) else {
            return Err(VrmError::new(
                VrmErrorKind::InvalidStructure,
                "副FBXスキンの関節モデルが一覧にない",
            ));
        };
        let Some(bone_mount) = local_mounts.get(&bone_ref.file_id) else {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!(
                    "SkinnedMeshRendererのボーン参照（{}）を解決できない（{}）",
                    bone_ref.file_id,
                    crate::escape_diag(label),
                ),
            ));
        };
        let Some(vrm_node) = bone_mount.parent_node else {
            return Err(VrmError::new(
                VrmErrorKind::InvalidStructure,
                "ボーン参照先のノードが存在しない",
            ));
        };
        let _ = (sub_fbx, sub_tr_to_idx);
        let joint_bind_world = matrix::mul(
            t_attach,
            &matrix::scale_translation(&sub_node_worlds[joint_model_idx], alpha),
        );
        let mut ibm = matrix::inverse(&joint_bind_world).ok_or_else(|| {
            VrmError::new(
                VrmErrorKind::InvalidStructure,
                "副FBX関節のバインド行列が特異である",
            )
            .with_object(cluster.joint)
        })?;
        matrix::snap_affine_row(&mut ibm);
        joints.push(vrm_node);
        ibms.push(ibm);
        joint_models.push(cluster.joint);
    }
    let final_idx = ctx.total_skin_count();
    ctx.skins.push(VrmSkin {
        joints,
        inverse_bind_matrices: ibms,
        joint_models,
    });
    Ok(final_idx)
}

/// ネストされた `.fbx` の `PrefabInstance` を展開し、ノード・スキン・メッシュを追加する。
#[allow(clippy::too_many_arguments)]
fn expand_fbx_instance(
    fbx_guid: &str,
    fbx_rel_path: &str,
    default_root_name: &str,
    mount: MountPoint,
    overrides: &[ScopedOverride],
    created_source_nodes: &mut HashMap<FileId, usize>,
    claimed_nodes: &mut HashSet<usize>,
    model: &VrmModel,
    ctx: &mut NestedContext<'_>,
) -> Result<(), VrmError> {
    let fbx_asset_label = format!("Assets/{fbx_rel_path}");
    let fbx_key = Guid::parse(fbx_guid).expect("GUID確認済み");
    let fbx_record = ctx.input.index.get(&fbx_key).ok_or_else(|| {
        VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!("副FBXが索引にない: {fbx_asset_label}"),
        )
    })?;
    let remaps = fbx_record.meta.external_object_remaps().map_err(|e| {
        VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!("副FBXの置換表を読めない（{fbx_asset_label}）: {e}"),
        )
    })?;
    let base_materials = prefab_base_materials(&remaps);
    let base_at: HashMap<&str, &PrefabBaseMaterial> = base_materials
        .iter()
        .map(|entry| (entry.slot.as_str(), entry))
        .collect();
    let legacy_recycle = fbx_record.meta.legacy_recycle_names();

    let fbx_bytes = ctx.load_fbx_bytes_cached(fbx_rel_path)?;
    let sub_fbx = FbxFile::parse(&fbx_bytes, ctx.input.fbx_options).map_err(|error| {
        VrmError::new(
            match error.kind() {
                FbxErrorKind::LimitExceeded => VrmErrorKind::LimitExceeded,
                _ => VrmErrorKind::InvalidStructure,
            },
            format!("副FBXを解析できない（{fbx_asset_label}）: {error}"),
        )
    })?;

    let sub_nodes = prefab_hierarchy_paths(&sub_fbx)?;
    let legacy_map = build_legacy_fbx_id_map(&legacy_recycle, &sub_nodes);

    // この副FBX向けのオーバーライドを集約する。
    let root_tr_fid = transform_file_id("");
    let root_go_fid = game_object_file_id("");

    let mut tr_to_model: HashMap<FileId, usize> = HashMap::new();
    let mut go_to_path: HashMap<FileId, String> = HashMap::new();
    go_to_path.insert(root_go_fid, String::new());
    let mut renderer_to_model: HashMap<FileId, usize> = HashMap::new();
    for (idx, node) in sub_nodes.iter().enumerate() {
        tr_to_model.insert(transform_file_id(&node.path), idx);
        go_to_path.insert(game_object_file_id(&node.path), node.path.clone());
        if node.has_mesh {
            renderer_to_model.insert(skinned_mesh_renderer_file_id(&node.path), idx);
            renderer_to_model.insert(mesh_renderer_file_id(&node.path), idx);
        }
    }

    let mut root_name = default_root_name.to_owned();
    let mut root_active = true;
    let mut root_trs = TrsPatch::default();
    let mut node_trs_patches: HashMap<usize, TrsPatch> = HashMap::new();
    let mut go_active: HashMap<String, bool> = HashMap::new();
    let mut renderer_enabled: HashMap<usize, bool> = HashMap::new();
    let mut material_overrides: HashMap<(usize, usize), (String, FileId)> = HashMap::new();

    for over in overrides {
        if !over.target_guid.eq_ignore_ascii_case(fbx_guid) {
            continue;
        }
        let target_fid = match legacy_map.get(&over.target_file_id) {
            Some(Ok(modern)) => *modern,
            Some(Err(_)) | None => over.target_file_id,
        };

        if target_fid == root_go_fid {
            if over.property_path == "m_Name" {
                if let Some(Value::String(name)) = &over.value {
                    root_name = name.clone();
                }
            } else if over.property_path == "m_IsActive" {
                root_active =
                    read_flag_value(&over.value, &fbx_asset_label, &root_name, target_fid)?;
                go_active.insert(String::new(), root_active);
            }
            continue;
        }
        if target_fid == root_tr_fid {
            apply_trs_patch_single(&mut root_trs, over, &fbx_asset_label)?;
            continue;
        }
        if over.property_path == "m_IsActive" {
            if let Some(path) = go_to_path.get(&target_fid) {
                let active = read_flag_value(&over.value, &fbx_asset_label, path, target_fid)?;
                go_active.insert(path.clone(), active);
            }
            continue;
        }
        if over.property_path == "m_Enabled" {
            if let Some(&model_idx) = renderer_to_model.get(&target_fid) {
                let enabled = read_flag_value(
                    &over.value,
                    &fbx_asset_label,
                    &sub_nodes[model_idx].path,
                    target_fid,
                )?;
                renderer_enabled.insert(model_idx, enabled);
            }
            continue;
        }
        if let Some(slot) = material_slot_index(&over.property_path) {
            if let Some(&model_idx) = renderer_to_model.get(&target_fid) {
                let (mat_guid, mat_fid) = read_material_override(over, &fbx_asset_label, slot)?;
                material_overrides.insert((model_idx, slot), (mat_guid, mat_fid));
            }
            continue;
        }
        if let Some(&model_idx) = tr_to_model.get(&target_fid) {
            let entry = node_trs_patches.entry(model_idx).or_default();
            apply_trs_patch_single(entry, over, &fbx_asset_label)?;
        }
    }

    // 親階層またはこの副FBXのルートが無効なら、所有モデルを持つ全メッシュを除外として数える。
    let mut owners_by_mesh: HashMap<i64, usize> = HashMap::new();
    for (idx, m) in sub_fbx.models().iter().enumerate() {
        if let Some(mesh_id) = m.mesh
            && owners_by_mesh.insert(mesh_id.value(), idx).is_some()
        {
            return Err(VrmError::new(
                VrmErrorKind::InvalidStructure,
                "副FBXで1つのメッシュに複数の所有モデルがある",
            )
            .with_object(mesh_id));
        }
    }
    if !(mount.parent_active && root_active) {
        for fbx_mesh in sub_fbx.meshes() {
            if let Some(&owner_idx) = owners_by_mesh.get(&fbx_mesh.id.value()) {
                ctx.report.excluded_meshes += 1;
                ctx.report
                    .excluded_names
                    .push(sub_fbx.models()[owner_idx].name.clone());
            }
        }
        return Ok(());
    }

    let sub_axis = axis_transform(&sub_fbx)?;
    let sub_unit_scale = sub_fbx.unit_scale_factor() / 100.0;
    if !sub_unit_scale.is_finite() || sub_unit_scale <= 0.0 {
        return Err(VrmError::new(
            VrmErrorKind::UnsupportedUnitScale,
            "副FBXの単位換算係数が正の有限値でない",
        ));
    }
    let alpha = sub_unit_scale / ctx.main_root_scale;
    let sub_bind = bind_world_matrices_for_nested(&sub_fbx)?;
    let sub_node_worlds = apply_node_worlds(&sub_axis, &sub_bind);
    let sub_mesh_worlds = apply_mesh_worlds(&sub_axis, &sub_bind, &sub_node_worlds);

    // ルートノード R_sub を構築する。
    let mut r_pos = [0.0, 0.0, 0.0];
    let mut r_rot = [0.0, 0.0, 0.0, 1.0];
    let mut r_scale = [1.0, 1.0, 1.0];
    root_trs.apply(&mut r_pos, &mut r_rot, &mut r_scale);
    let root_local = matrix::unity_local_to_vrm_matrix(r_pos, r_rot, r_scale, ctx.main_root_scale)
        .ok_or_else(|| {
            VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!("副FBXルートの姿勢が不正である: {fbx_asset_label}"),
            )
        })?;
    let sub_root_node = ctx.push_node(VrmNode {
        name: root_name,
        parent: mount.parent_node,
        children: Vec::new(),
        local: root_local,
    })?;
    created_source_nodes.insert(root_tr_fid, sub_root_node);

    // 副FBXの全モデルノードを依存順（親が先）に作成する。
    let sub_models = sub_fbx.models();
    let mut sub_vrm_nodes: Vec<Option<usize>> = vec![None; sub_models.len()];
    let mut done = vec![false; sub_models.len()];
    for start in 0..sub_models.len() {
        let mut chain = Vec::new();
        let mut cur = start;
        let mut seen = HashSet::new();
        while !done[cur] {
            if !seen.insert(cur) {
                return Err(VrmError::new(
                    VrmErrorKind::InvalidStructure,
                    "副FBXモデルの親子に循環がある",
                ));
            }
            chain.push(cur);
            match sub_nodes[cur].parent {
                Some(p) => cur = p,
                None => break,
            }
        }
        for &m_idx in chain.iter().rev() {
            let parent_vrm = match sub_nodes[m_idx].parent {
                Some(p) => sub_vrm_nodes[p].expect("親は先に構築済み"),
                None => sub_root_node,
            };
            let self_w = matrix::scale_translation(&sub_node_worlds[m_idx], alpha);
            let mut local = match sub_nodes[m_idx].parent {
                Some(p) => {
                    let parent_w = matrix::scale_translation(&sub_node_worlds[p], alpha);
                    let inv_p = matrix::inverse(&parent_w).ok_or_else(|| {
                        VrmError::new(
                            VrmErrorKind::InvalidStructure,
                            "副FBX親モデルの行列が特異である",
                        )
                    })?;
                    matrix::mul(&inv_p, &self_w)
                }
                None => self_w,
            };
            matrix::snap_affine_row(&mut local);
            if let Some(patch) = node_trs_patches.get(&m_idx) {
                let (mut p, mut r, mut s) =
                    matrix::decompose_vrm_matrix_to_unity_trs(&local, ctx.main_root_scale)
                        .ok_or_else(|| {
                            VrmError::new(
                                VrmErrorKind::InvalidPrefab,
                                format!(
                                    "副FBXノードの姿勢を分解できない: {}",
                                    sub_nodes[m_idx].path
                                ),
                            )
                        })?;
                patch.apply(&mut p, &mut r, &mut s);
                local = matrix::unity_local_to_vrm_matrix(p, r, s, ctx.main_root_scale)
                    .ok_or_else(|| {
                        VrmError::new(
                            VrmErrorKind::InvalidPrefab,
                            format!(
                                "副FBXノードの改変後姿勢が不正である: {}",
                                sub_nodes[m_idx].path
                            ),
                        )
                    })?;
            }
            let vrm_node = ctx.push_node(VrmNode {
                name: sub_models[m_idx].name.clone(),
                parent: Some(parent_vrm),
                children: Vec::new(),
                local,
            })?;
            sub_vrm_nodes[m_idx] = Some(vrm_node);
            created_source_nodes.insert(transform_file_id(&sub_nodes[m_idx].path), vrm_node);
            done[m_idx] = true;
        }
    }

    let geometry_skin: HashMap<i64, usize> = sub_fbx
        .skins()
        .iter()
        .enumerate()
        .map(|(idx, skin)| (skin.geometry.value(), idx))
        .collect();
    let mut sub_model_idx_by_id: HashMap<i64, usize> = HashMap::new();
    for (idx, m) in sub_models.iter().enumerate() {
        sub_model_idx_by_id.insert(m.id.value(), idx);
    }

    let sub_root_w_vrm = ctx.world_of(sub_root_node);

    // 副FBXの各メッシュを変換する。
    for fbx_mesh in sub_fbx.meshes() {
        let Some(&owner_idx) = owners_by_mesh.get(&fbx_mesh.id.value()) else {
            continue;
        };
        let owner = &sub_models[owner_idx];
        let owner_path = &sub_nodes[owner_idx].path;

        if !renderer_active(owner_idx, owner_path, &renderer_enabled, &go_active) {
            ctx.report.excluded_meshes += 1;
            ctx.report.excluded_names.push(owner.name.clone());
            continue;
        }

        // 材質スロット解決。
        let slot_count = owner.materials.len();
        let mut slot_names = Vec::with_capacity(slot_count);
        for id in &owner.materials {
            let mat = sub_fbx.material(*id).ok_or_else(|| {
                VrmError::new(VrmErrorKind::InvalidStructure, "材質スロットの材質がない")
                    .with_object(owner.id)
            })?;
            slot_names.push(mat.name.as_str());
        }
        let overflow = material_overrides
            .keys()
            .filter(|(m, _)| *m == owner_idx)
            .map(|(_, slot)| *slot)
            .filter(|slot| *slot >= slot_count)
            .min();
        if let Some(slot) = overflow {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!(
                    "上書きのスロットがメッシュの材質数を超える: {} スロット{slot}（スロット{slot_count}件）（{fbx_asset_label}）",
                    crate::escape_diag(owner_path),
                ),
            ));
        }
        let renderer_label = format!("{fbx_asset_label}:{owner_path}");
        let mut slot_keys = Vec::with_capacity(slot_count);
        for (slot, slot_name) in slot_names.iter().enumerate() {
            let (mat_guid, mat_fid) = if let Some(found) =
                material_overrides.get(&(owner_idx, slot))
            {
                found.clone()
            } else if let Some(found) = base_at.get(slot_name) {
                (found.guid.clone(), found.file_id)
            } else {
                return Err(VrmError::new(
                    VrmErrorKind::InvalidPrefab,
                    format!(
                        "上書きも置換表も無いため材質GUID不在: {} スロット{slot}（{slot_name}）（{fbx_asset_label}）",
                        crate::escape_diag(owner_path),
                    ),
                ));
            };
            slot_keys.push(ctx.register_material(&mat_guid, mat_fid, &renderer_label));
        }

        let geometric = owner.transform.geometric_matrix().map_err(|error| {
            let mut out = wrap_fbx_error(error);
            if out.object_id().is_none() {
                out = out.with_object(owner.id);
            }
            out
        })?;
        let data = sub_fbx.mesh_data(fbx_mesh.id).map_err(wrap_fbx_error)?;

        let mut vrm_mesh = if let Some(&skin_idx) = geometry_skin.get(&fbx_mesh.id.value()) {
            let skin = &sub_fbx.skins()[skin_idx];
            let skin_data = sub_fbx.skin_data(skin.id).map_err(wrap_fbx_error)?;
            let mut joints = Vec::with_capacity(skin.clusters.len());
            let mut ibms = Vec::with_capacity(skin.clusters.len());
            let mut joint_models = Vec::with_capacity(skin.clusters.len());
            for cluster in &skin.clusters {
                let Some(&joint_model_idx) = sub_model_idx_by_id.get(&cluster.joint.value()) else {
                    return Err(VrmError::new(
                        VrmErrorKind::InvalidStructure,
                        "副FBXスキンの関節モデルが一覧にない",
                    ));
                };
                let vrm_node = sub_vrm_nodes[joint_model_idx].expect("構築済み");
                let joint_bind_world = matrix::mul(
                    &sub_root_w_vrm,
                    &matrix::scale_translation(&sub_node_worlds[joint_model_idx], alpha),
                );
                let mut ibm = matrix::inverse(&joint_bind_world).ok_or_else(|| {
                    VrmError::new(
                        VrmErrorKind::InvalidStructure,
                        "副FBX関節のバインド行列が特異である",
                    )
                    .with_object(cluster.joint)
                })?;
                matrix::snap_affine_row(&mut ibm);
                joints.push(vrm_node);
                ibms.push(ibm);
                joint_models.push(cluster.joint);
            }
            let final_skin_idx = ctx.total_skin_count();
            ctx.skins.push(VrmSkin {
                joints,
                inverse_bind_matrices: ibms,
                joint_models,
            });

            let mesh_bake = matrix::mul(
                &sub_root_w_vrm,
                &matrix::mul(
                    &matrix::scale_matrix(alpha),
                    &matrix::mul(&sub_mesh_worlds[owner_idx], &geometric),
                ),
            );
            let skin_mesh_node = ctx.push_node(VrmNode {
                name: owner.name.clone(),
                parent: None,
                children: Vec::new(),
                local: matrix::IDENTITY,
            })?;
            claimed_nodes.insert(skin_mesh_node);
            mesh::convert_mesh(
                &sub_fbx,
                fbx_mesh,
                &data,
                owner,
                skin_mesh_node,
                &mesh_bake,
                Some((final_skin_idx, &skin_data)),
                None,
                &[],
            )?
            .mesh
        } else {
            let base_vrm_node = sub_vrm_nodes[owner_idx].expect("構築済み");
            let owner_vrm_node =
                ctx.alloc_mesh_node(base_vrm_node, &owner.name, claimed_nodes, model)?;
            let local_bake = if sub_axis.handedness_flipped() {
                matrix::mul(
                    &matrix::scale_matrix(alpha),
                    &matrix::mul_left_mirror_x(&geometric),
                )
            } else {
                matrix::mul(&matrix::scale_matrix(alpha), &geometric)
            };
            mesh::convert_mesh(
                &sub_fbx,
                fbx_mesh,
                &data,
                owner,
                owner_vrm_node,
                &local_bake,
                None,
                None,
                &[],
            )?
            .mesh
        };

        vrm_mesh.material_slots = slot_keys;
        ctx.push_mesh(vrm_mesh)?;
    }

    Ok(())
}

fn apply_node_worlds<'b>(
    axis: &super::matrix::AxisTransform,
    bind: &'b [[f64; 16]],
) -> Cow<'b, [[f64; 16]]> {
    if axis.is_identity() {
        Cow::Borrowed(bind)
    } else {
        Cow::Owned(
            bind.iter()
                .map(|world| axis.apply_node_world(world))
                .collect(),
        )
    }
}

fn apply_mesh_worlds<'b>(
    axis: &super::matrix::AxisTransform,
    bind: &'b [[f64; 16]],
    node_worlds: &'b [[f64; 16]],
) -> Cow<'b, [[f64; 16]]> {
    if axis.is_identity() {
        Cow::Borrowed(bind)
    } else if !axis.handedness_flipped() {
        Cow::Borrowed(node_worlds)
    } else {
        Cow::Owned(
            bind.iter()
                .map(|world| axis.apply_mesh_world(world))
                .collect(),
        )
    }
}

fn prefab_stem_from_label(prefab_label: &str, fbx_rel_path: &str) -> String {
    let source = if prefab_label.ends_with(".prefab") {
        prefab_label
    } else {
        fbx_rel_path
    };
    let file = source.rsplit('/').next().unwrap_or(source);
    file.rsplit_once('.')
        .map(|(stem, _)| stem.to_owned())
        .unwrap_or_else(|| file.to_owned())
}

fn read_material_override(
    over: &ScopedOverride,
    label: &str,
    slot: usize,
) -> Result<(String, FileId), VrmError> {
    let reference = over.object_reference.as_ref().ok_or_else(|| {
        VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!(
                "材質の参照欄が無い: スロット{slot}（{}）",
                crate::escape_diag(label),
            ),
        )
    })?;
    if reference.is_null() || reference.guid.is_none() {
        return Err(VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!(
                "材質の参照がnullまたは材質GUID不在: スロット{slot}（{}）",
                crate::escape_diag(label),
            ),
        ));
    }
    let guid = reference
        .guid
        .as_ref()
        .expect("確認済み")
        .as_str()
        .to_ascii_lowercase();
    Ok((guid, reference.file_id))
}

#[derive(Debug, Default, Clone)]
struct TrsPatch {
    pos: [Option<f64>; 3],
    rot: [Option<f64>; 4],
    scale: [Option<f64>; 3],
}

impl TrsPatch {
    fn apply(&self, pos: &mut [f64; 3], rot: &mut [f64; 4], scale: &mut [f64; 3]) {
        for i in 0..3 {
            if let Some(v) = self.pos[i] {
                pos[i] = v;
            }
            if let Some(v) = self.scale[i] {
                scale[i] = v;
            }
        }
        for (slot, val) in rot.iter_mut().zip(self.rot.iter()) {
            if let Some(v) = val {
                *slot = *v;
            }
        }
    }
}

fn apply_trs_patch_item(
    map: &mut HashMap<FileId, TrsPatch>,
    fid: FileId,
    over: &ScopedOverride,
    label: &str,
) -> Result<(), VrmError> {
    if over.property_path.starts_with("m_LocalPosition.")
        || over.property_path.starts_with("m_LocalRotation.")
        || over.property_path.starts_with("m_LocalScale.")
    {
        let entry = map.entry(fid).or_default();
        apply_trs_patch_single(entry, over, label)?;
    }
    Ok(())
}

fn apply_trs_patch_single(
    patch: &mut TrsPatch,
    over: &ScopedOverride,
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
    if let Some(i) = axis(&over.property_path, "m_LocalPosition.")
        && i < 3
    {
        patch.pos[i] = Some(read_f64_override(over, label)?);
    } else if let Some(i) = axis(&over.property_path, "m_LocalRotation.") {
        patch.rot[i] = Some(read_f64_override(over, label)?);
    } else if let Some(i) = axis(&over.property_path, "m_LocalScale.")
        && i < 3
    {
        patch.scale[i] = Some(read_f64_override(over, label)?);
    }
    Ok(())
}

fn read_f64_override(over: &ScopedOverride, label: &str) -> Result<f64, VrmError> {
    match over.value.as_ref().and_then(Value::as_f64) {
        Some(v) if v.is_finite() => Ok(v),
        _ => Err(VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!(
                "姿勢の改変値が有限の数値でない: {}（{}）",
                crate::escape_diag(&over.property_path),
                crate::escape_diag(label),
            ),
        )),
    }
}

fn build_legacy_fbx_id_map(
    recycle_names: &[(FileId, String)],
    nodes: &[super::PrefabHierarchyNode],
) -> HashMap<FileId, Result<FileId, String>> {
    if recycle_names.is_empty() {
        return HashMap::new();
    }
    let unique = |name: &str| -> Option<&super::PrefabHierarchyNode> {
        let mut found = nodes.iter().filter(|node| node.name == name);
        let first = found.next()?;
        found.next().is_none().then_some(first)
    };
    let mut table = HashMap::new();
    for (legacy, name) in recycle_names {
        let class_id = legacy / 100_000;
        let modern = match class_id {
            1 if name == "//RootNode" => Some(game_object_file_id("")),
            1 => unique(name).map(|node| game_object_file_id(&node.path)),
            4 if name == "//RootNode" => Some(transform_file_id("")),
            4 => unique(name).map(|node| transform_file_id(&node.path)),
            23 => unique(name)
                .filter(|node| node.has_mesh)
                .map(|node| mesh_renderer_file_id(&node.path)),
            33 => unique(name)
                .filter(|node| node.has_mesh)
                .map(|node| mesh_filter_file_id(&node.path)),
            43 => unique(name)
                .filter(|node| node.has_mesh)
                .map(|node| mesh_file_id(&node.name)),
            137 => unique(name)
                .filter(|node| node.has_mesh)
                .map(|node| skinned_mesh_renderer_file_id(&node.path)),
            _ => continue,
        };
        table.insert(*legacy, modern.ok_or_else(|| format!("{class_id}:{name}")));
    }
    table
}

fn resolve_direct_fbx_mesh(
    fbx: &FbxFile<'_>,
    target_fid: FileId,
    legacy_names: &[(FileId, String)],
    label: &str,
) -> Result<(usize, usize), VrmError> {
    let nodes = prefab_hierarchy_paths(fbx)?;
    let legacy_map = build_legacy_fbx_id_map(legacy_names, &nodes);
    let modern_fid = match legacy_map.get(&target_fid) {
        Some(Ok(modern)) => *modern,
        _ => target_fid,
    };

    let mut mesh_idx_by_fid: HashMap<FileId, (usize, usize)> = HashMap::new();
    let mut owners_by_mesh: HashMap<i64, usize> = HashMap::new();
    for (idx, m) in fbx.models().iter().enumerate() {
        if let Some(mesh_id) = m.mesh {
            owners_by_mesh.insert(mesh_id.value(), idx);
        }
    }
    for (mesh_idx, fbx_mesh) in fbx.meshes().iter().enumerate() {
        if let Some(&owner_idx) = owners_by_mesh.get(&fbx_mesh.id.value()) {
            let owner = &fbx.models()[owner_idx];
            mesh_idx_by_fid.insert(mesh_file_id(&owner.name), (mesh_idx, owner_idx));
        }
    }
    mesh_idx_by_fid.get(&modern_fid).copied().ok_or_else(|| {
        VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!(
                "ネストPrefabのメッシュID（{target_fid}）を副FBXから解決できない（{}）",
                crate::escape_diag(label),
            ),
        )
    })
}
