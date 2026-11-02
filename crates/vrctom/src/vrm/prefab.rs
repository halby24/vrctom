//! Prefabからの材質割当と出力対象レンダラの解決。
//!
//! 入口は [`plan_prefab_assignment`]（対応付けと除外計画）、
//! [`apply_prefab_plan`]（除外と割当の適用）である。
//! 材質プールの読み取りは [`crate::vrm::load_renderer_materials`] を使い、
//! `.mat` の解釈は [`crate::vrm::VrmMaterials`] の既存の写像・ベイク・
//! 取り込み解像度合わせを使う。新しいシェーダ変換器は作らない。
//!
//! ```mermaid
//! flowchart LR
//!     prefab[Prefab YAML<br/>PrefabInstance改変か直接記述] --> plan[plan_prefab_assignment<br/>Model名で対応付け]
//!     fbx[FBXモデル階層<br/>fileID算出] --> plan
//!     plan --> pool[材質プール<br/>guidとfileIDで同一性判定]
//!     plan --> apply[apply_prefab_plan<br/>除外と割当]
//!     pool --> apply
//! ```
//!
//! 対応付けはUnityの取り込み fileID（XXH64・seed 0）で行う。
//! 取り込み方式は2019.3以降の新方式だけを扱い、旧方式
//! （`.meta` の `fileIDToRecycleName`）のPrefabは解決できず失敗する。
//! 材質プールと適用手順はレンダラsidecar経路と同じ正規化に合流し、
//! 材質スロットの並びは中間メッシュ順→サブメッシュ順の初出順で決定的にする。

use std::collections::{HashMap, HashSet};

use yaml_serde::Value;

use super::error::{VrmError, VrmErrorKind};
use super::renderers::{
    RendererApplyReport, RendererMeshAssignment, RendererPoolEntry, apply_assignment_core,
};
use crate::fbx::FbxFile;
use crate::unity::{
    AssetIndex, FileId, GameObject, SerializedFile, SkinnedMeshRenderer, Transform, TypedObject,
    game_object_file_id, mesh_file_id, skinned_mesh_renderer_file_id,
};

/// 対応付け用の中間メッシュ1件の要約。
///
/// FBX由来の所有モデル名・階層パス・材質スロット名だけを持ち、読み取り層に
/// 依存しない。呼び出し側は中間モデルのメッシュ順に並べる。
/// 階層パスはFBXのModel階層を `/` で繋いだものであり、各モデル名に `/` を
/// 含まないことが前提である（含む場合は [`prefab_hierarchy_paths`] が失敗する）。
#[derive(Debug, Clone)]
pub struct PrefabMeshView<'a> {
    /// 所有モデルの名前。メッシュIDの算出に使う。
    pub model_name: &'a str,
    /// 所有モデルの階層パス。描画設定・ゲームオブジェクトIDの算出に使う。
    pub hierarchy_path: &'a str,
    /// FBXの材質スロット名。サブメッシュ順である。
    pub slot_names: &'a [String],
}

/// `.fbx.meta` の置換表由来の基礎材質1件。
///
/// 改変で上書きされないスロットの解決に使う。置換先がnullやGUIDなしの項目は
/// 含めない。スロット名の重複は最初の項目を残す。
#[derive(Debug, Clone)]
pub struct PrefabBaseMaterial {
    /// FBXの材質スロット名。
    pub slot: String,
    /// 置換先の `.mat` のGUID（小文字hex）。
    pub guid: String,
    /// 置換先のfileID。
    pub file_id: FileId,
}

/// FBXのモデル階層1件の要約。
///
/// [`prefab_hierarchy_paths`] が `models()` 順に作る。呼び出し側は
/// 所有モデルの索引引きに使う。
#[derive(Debug, Clone)]
pub struct PrefabHierarchyNode {
    /// モデル名。
    pub name: String,
    /// 根元からの `/` 区切りの階層パス。
    pub path: String,
    /// 親モデルの索引。親なしの場合は `None` である。
    pub parent: Option<usize>,
    /// メッシュを持つかどうか。
    pub has_mesh: bool,
}

/// Prefabの記述方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrefabMode {
    /// 取り込みへの `PrefabInstance` と改変内容で書く方式。
    Instance,
    /// 展開済みの文書列で直接書く方式。
    Direct,
}

/// Prefab由来の対応付けと除外の計画。
///
/// 材質プールと対応付けの形はレンダラsidecar経路と同じ正規化であり、
/// 適用は [`apply_prefab_plan`] が同じ本体を通す。
#[derive(Debug, Clone)]
pub struct PrefabPlan {
    /// 診断用のPrefab表示名（資産パスなど）。
    pub prefab_path: String,
    /// 入力FBXのGUID（小文字hex）。対象の一致判定に使った値である。
    pub fbx_guid: String,
    /// 解決に使った記述方式。
    pub mode: PrefabMode,
    /// 有効レンダラの対応付け。中間メッシュ順である。
    pub assignments: Vec<RendererMeshAssignment>,
    /// 出力しない中間メッシュの索引（中間モデル順）。
    pub excluded_meshes: Vec<usize>,
    /// 除外した中間メッシュのModel名（中間モデル順）。
    pub excluded_names: Vec<String>,
    /// 材質プール。初出順である。
    pub pool: Vec<RendererPoolEntry>,
}

impl PrefabPlan {
    /// 有効レンダラ数を返す。
    pub fn active_count(&self) -> usize {
        self.assignments.len()
    }
}

/// FBXのモデル階層から階層パス列を作る。
///
/// `models()` 順にノードを返し、親子は索引で指す。モデル名に `/` を含む
/// 場合は階層パスが一意に分解できないため失敗する。親子の循環・親欠落も
/// 失敗にする。
///
/// # Errors
///
/// 名前に `/` を含む場合・親子に循環や欠落がある場合に返す。
pub fn prefab_hierarchy_paths(file: &FbxFile<'_>) -> Result<Vec<PrefabHierarchyNode>, VrmError> {
    let models = file.models();
    let at: HashMap<i64, usize> = models
        .iter()
        .enumerate()
        .map(|(index, model)| (model.id.value(), index))
        .collect();
    let mut paths: Vec<Option<String>> = vec![None; models.len()];
    let mut visiting = vec![false; models.len()];
    fn visit(
        models: &[crate::fbx::Model],
        at: &HashMap<i64, usize>,
        paths: &mut [Option<String>],
        visiting: &mut [bool],
        index: usize,
    ) -> Result<String, VrmError> {
        if let Some(path) = paths[index].clone() {
            return Ok(path);
        }
        if visiting[index] {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!(
                    "モデルの親子に循環がある: {}",
                    crate::escape_diag(&models[index].name)
                ),
            ));
        }
        if models[index].name.contains('/') {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!(
                    "モデル名に `/` を含むため階層パスが作れない: {}",
                    crate::escape_diag(&models[index].name)
                ),
            ));
        }
        visiting[index] = true;
        let path = match models[index].parent {
            None => models[index].name.clone(),
            Some(parent) => {
                let parent_index = at.get(&parent.value()).copied().ok_or_else(|| {
                    VrmError::new(
                        VrmErrorKind::InvalidPrefab,
                        format!(
                            "親モデルが一覧にない: {}",
                            crate::escape_diag(&models[index].name)
                        ),
                    )
                })?;
                let parent_path = visit(models, at, paths, visiting, parent_index)?;
                format!("{parent_path}/{}", models[index].name)
            }
        };
        visiting[index] = false;
        paths[index] = Some(path.clone());
        Ok(path)
    }
    let mut nodes = Vec::with_capacity(models.len());
    for (index, model) in models.iter().enumerate() {
        let path = visit(models, &at, &mut paths, &mut visiting, index)?;
        nodes.push(PrefabHierarchyNode {
            name: model.name.clone(),
            path,
            parent: model.parent.and_then(|id| at.get(&id.value()).copied()),
            has_mesh: model.mesh.is_some(),
        });
    }
    Ok(nodes)
}

/// `.fbx.meta` の置換表から基礎材質列を作る。
///
/// `UnityEngine:Material` だけを残し、置換先がnullやGUIDなしの項目は
/// 含めない。スロット名の重複は最初を残す。順序は置換表のまま保つ。
pub fn prefab_base_materials(
    remaps: &[crate::unity::ExternalObjectEntry],
) -> Vec<PrefabBaseMaterial> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for entry in remaps {
        if entry.original_type != "UnityEngine:Material" {
            continue;
        }
        if entry.replacement.is_null() {
            continue;
        }
        let Some(guid) = entry.replacement.guid.clone() else {
            continue;
        };
        if !seen.insert(entry.original_name.clone()) {
            continue;
        }
        out.push(PrefabBaseMaterial {
            slot: entry.original_name.clone(),
            guid: guid.as_str().to_owned(),
            file_id: entry.replacement.file_id,
        });
    }
    out
}

/// 材質プールの `.mat` 資産パスを索引で埋める。
///
/// [`plan_prefab_assignment`] の計画直後は空であり、材質読み取りの前に
/// この関数で埋める。索引に無いGUIDは空のまま残し、読み取り側の
/// 灰色材質への落としに委ねる。索引と食い違う警告を出さないよう、
/// 見つかった場合は索引のパスを使う。
pub fn resolve_prefab_pool_paths(pool: &mut [RendererPoolEntry], index: &AssetIndex) {
    for entry in pool {
        if !entry.path.is_empty() {
            continue;
        }
        let Ok(guid) = crate::unity::Guid::parse(&entry.guid) else {
            continue;
        };
        if let Some(record) = index.get(&guid) {
            entry.path = record.path.clone();
        }
    }
}

/// Prefabを中間モデルへ対応付け、除外計画を作る。
///
/// `fbx_guid` は入力FBXの `.meta` のGUID（小文字hex）であり、
/// `PrefabInstance` の `m_SourcePrefab` や直接記述の `m_Mesh` と突き合わせる。
/// `base` は置換表由来の基礎材質であり、改変で上書きされないスロットに使う。
/// `meshes` は中間モデルのメッシュ順の要約である。
/// 記述方式は自動で選ぶ。入力FBXへの `PrefabInstance` がちょうど1件あれば
/// 改変内容方式、無ければ直接記述方式である。複数ある場合は先勝ちにせず
/// 失敗する。直接記述方式で入力FBXを指す描画設定が1件も無い場合も失敗する。
///
/// 材質の同一性は `(guid, fileID)` で判定し、中間メッシュ順→サブメッシュ順の
/// 初出順にプールへ並べる。材質プールの資産パスは空のまま残し、呼び出し側は
/// [`resolve_prefab_pool_paths`] で埋めてから読み取る。
/// 有効とは描画設定が有効かつ自階層がすべて有効な場合だけであり、
/// 祖先の無効も子に効く。無効なものは出力しない。
/// 先勝ちにせず、見つからない・重複する・数が合わない・材質参照が無い場合は
/// 失敗し、対象の名前や件数を文面に含める。
/// 旧方式（2019.3より前の取り込みfileID）のPrefabは新方式のIDと一致しない
/// ため解決できず失敗する。理由は文面に残す。
///
/// # Errors
///
/// 対象の不一致・重複・材質数の不一致・材質参照の欠落・記述の混在や破損の
/// 場合に返す。いずれも対象の名前や件数を文面に含める。
pub fn plan_prefab_assignment(
    prefab: &SerializedFile,
    prefab_label: &str,
    fbx_guid: &str,
    base: &[PrefabBaseMaterial],
    meshes: &[PrefabMeshView<'_>],
) -> Result<PrefabPlan, VrmError> {
    let decoded: Vec<(FileId, TypedObject)> = prefab
        .documents()
        .iter()
        .map(|doc| {
            doc.decode()
                .map(|typed| (doc.file_id(), typed))
                .map_err(|error| {
                    VrmError::new(
                        VrmErrorKind::InvalidPrefab,
                        format!(
                            "Prefabの文書を読めない: {}: {error}",
                            crate::escape_diag(prefab_label),
                        ),
                    )
                })
        })
        .collect::<Result<_, _>>()?;
    let instances: Vec<(FileId, &crate::unity::PrefabInstance)> = decoded
        .iter()
        .filter_map(|(file_id, typed)| match typed {
            TypedObject::PrefabInstance(instance)
                if !instance.source.is_null()
                    && instance
                        .source
                        .guid
                        .as_ref()
                        .is_some_and(|guid| guid.as_str() == fbx_guid) =>
            {
                Some((*file_id, instance))
            }
            _ => None,
        })
        .collect();
    if instances.len() > 1 {
        return Err(VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!(
                "入力FBXへのPrefabInstanceが複数あるため先勝ちにしない: {}（{}件）",
                crate::escape_diag(prefab_label),
                instances.len(),
            ),
        ));
    }
    let id_tables = build_id_tables(meshes, prefab_label)?;
    if let [(instance_id, instance)] = instances.as_slice() {
        plan_instance_mode(
            &decoded,
            *instance_id,
            instance,
            prefab_label,
            fbx_guid,
            base,
            meshes,
            &id_tables,
        )
    } else {
        plan_direct_mode(&decoded, prefab_label, fbx_guid, meshes, &id_tables)
    }
}

/// 対応付け計画を中間モデルへ適用する。
///
/// 除外と割当の手順はレンダラsidecar経路と同じ本体を通す。詳しくは
/// [`crate::vrm::apply_renderer_plan`] を参照。失敗の種別だけ
/// [`VrmErrorKind::InvalidPrefab`] になる。
///
/// # Errors
///
/// 表情の参照先が除外された場合や、計画とモデルの不整合の場合に返す。
pub fn apply_prefab_plan(
    model: &mut super::VrmModel,
    plan: &PrefabPlan,
    materials: &super::material::VrmMaterials,
) -> Result<RendererApplyReport, VrmError> {
    apply_assignment_core(
        model,
        &plan.assignments,
        &plan.excluded_meshes,
        &plan.excluded_names,
        &plan.pool,
        materials,
        VrmErrorKind::InvalidPrefab,
    )
}

/// ID算出表。中間メッシュ順に並ぶ。
struct ModelIdTables {
    /// 中間メッシュごとの階層パス。
    paths: Vec<String>,
    /// メッシュIDから中間メッシュ索引への表。
    mesh_at: HashMap<FileId, usize>,
    /// 描画設定IDから中間メッシュ索引への表。
    renderer_at: HashMap<FileId, usize>,
    /// ゲームオブジェクトIDから階層パスへの表。
    go_at: HashMap<FileId, String>,
}

/// 中間メッシュ列から取り込みIDの算出表を作る。
///
/// 階層パスは呼び出し側がFBXのModel階層から作ったものを使う。
/// メッシュIDの重複は先勝ちにせず失敗する。描画設定IDの重複は
/// 階層パスが違う場合に失敗する（同じ階層パスは共有祖先であり重複でない）。
fn build_id_tables(
    meshes: &[PrefabMeshView<'_>],
    prefab_label: &str,
) -> Result<ModelIdTables, VrmError> {
    let mut paths = Vec::with_capacity(meshes.len());
    let mut mesh_at = HashMap::new();
    let mut renderer_at: HashMap<FileId, usize> = HashMap::new();
    let mut go_at: HashMap<FileId, String> = HashMap::new();
    for (index, view) in meshes.iter().enumerate() {
        if view.model_name.contains('/') {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!(
                    "モデル名に `/` を含むため階層パスが作れない: {}",
                    crate::escape_diag(view.model_name),
                ),
            ));
        }
        let mesh_id = mesh_file_id(view.model_name);
        if mesh_at.contains_key(&mesh_id) {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!(
                    "メッシュIDが重複するため先勝ちにしない: {}（{}）",
                    crate::escape_diag(view.model_name),
                    crate::escape_diag(prefab_label),
                ),
            ));
        }
        mesh_at.insert(mesh_id, index);
        let renderer_id = skinned_mesh_renderer_file_id(view.hierarchy_path);
        match renderer_at.get(&renderer_id) {
            Some(&first) if meshes[first].hierarchy_path != view.hierarchy_path => {
                return Err(VrmError::new(
                    VrmErrorKind::InvalidPrefab,
                    format!(
                        "描画設定IDが重複するため先勝ちにしない: {}（{}）",
                        crate::escape_diag(view.hierarchy_path),
                        crate::escape_diag(prefab_label),
                    ),
                ));
            }
            Some(_) => {}
            None => {
                renderer_at.insert(renderer_id, index);
            }
        }
        // 自分を含む祖先のゲームオブジェクトIDを登録する。階層パスは
        // 呼び出し側が `/` を含まないモデル名から作るため、末尾からの
        // 分割で親パスが一意に求まる。共有祖先の再登録は重複でない。
        let mut rest = view.hierarchy_path;
        loop {
            let go_id = game_object_file_id(rest);
            if let Some(first) = go_at.get(&go_id)
                && first != rest
            {
                return Err(VrmError::new(
                    VrmErrorKind::InvalidPrefab,
                    format!(
                        "ゲームオブジェクトIDが重複するため先勝ちにしない: {}（{}）",
                        crate::escape_diag(rest),
                        crate::escape_diag(prefab_label),
                    ),
                ));
            }
            go_at.entry(go_id).or_insert_with(|| rest.to_owned());
            let Some((parent, _)) = rest.rsplit_once('/') else {
                break;
            };
            rest = parent;
        }
        paths.push(view.hierarchy_path.to_owned());
    }
    // 取り込みルートのゲームオブジェクトも登録する。階層パスは空であり、
    // `Type:GameObject->//RootNode/root0` から算出する。資産に依らず同値であり、
    // 実データ（マヌカ・シフォンの `m_IsActive` の対象）で裏付けた。
    // 無効の場合は配下の全メッシュが除外される。
    go_at.entry(game_object_file_id("")).or_default();
    Ok(ModelIdTables {
        paths,
        mesh_at,
        renderer_at,
        go_at,
    })
}

/// 改変内容方式で計画を作る。
#[allow(clippy::too_many_arguments)]
fn plan_instance_mode(
    decoded: &[(FileId, TypedObject)],
    instance_id: FileId,
    instance: &crate::unity::PrefabInstance,
    prefab_label: &str,
    fbx_guid: &str,
    base: &[PrefabBaseMaterial],
    meshes: &[PrefabMeshView<'_>],
    id_tables: &ModelIdTables,
) -> Result<PrefabPlan, VrmError> {
    // 直接記述との混在は真の割当が定まらないため失敗する。
    // `stripped` 文書は型付き読み取りで区別済みであり、ここには来ない。
    for (file_id, typed) in decoded {
        if let TypedObject::SkinnedMeshRenderer(renderer) = typed
            && !renderer.mesh.is_null()
            && renderer
                .mesh
                .guid
                .as_ref()
                .is_some_and(|guid| guid.as_str() == fbx_guid)
        {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!(
                    "PrefabInstanceと直接記述が混在するため先勝ちにしない: {}（描画設定{}）",
                    crate::escape_diag(prefab_label),
                    file_id,
                ),
            ));
        }
    }
    let modifications = instance.modifications().map_err(|error| {
        VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!(
                "Prefabの改変内容を読めない: {}（実体{}）: {error}",
                crate::escape_diag(prefab_label),
                instance_id,
            ),
        )
    })?;
    // 上書きの集計。同じ対象・同じ欄パスへの重複は先勝ちにせず失敗する。
    let mut material_overrides: HashMap<(usize, usize), (String, FileId)> = HashMap::new();
    let mut go_active: HashMap<String, bool> = HashMap::new();
    let mut renderer_enabled: HashMap<usize, bool> = HashMap::new();
    let mut seen_paths: HashSet<(FileId, String)> = HashSet::new();
    for modification in &modifications {
        // 取り込み由来以外（別資産の対象）は扱わない。
        if modification
            .target
            .guid
            .as_ref()
            .is_none_or(|guid| guid.as_str() != fbx_guid)
        {
            continue;
        }
        let target = modification.target.file_id;
        if let Some(slot) = material_slot_index(&modification.property_path) {
            if !seen_paths.insert((target, modification.property_path.clone())) {
                return Err(duplicate_override(
                    prefab_label,
                    &modification.property_path,
                    target,
                ));
            }
            let Some(&mesh_index) = id_tables.renderer_at.get(&target) else {
                return Err(unknown_target(
                    prefab_label,
                    &modification.property_path,
                    target,
                ));
            };
            let reference = modification.object_reference.as_ref().ok_or_else(|| {
                VrmError::new(
                    VrmErrorKind::InvalidPrefab,
                    format!(
                        "材質の参照欄が無い: {} スロット{slot}（{}）",
                        crate::escape_diag(&id_tables.paths[mesh_index]),
                        crate::escape_diag(prefab_label),
                    ),
                )
            })?;
            if reference.is_null() || reference.guid.is_none() {
                return Err(VrmError::new(
                    VrmErrorKind::InvalidPrefab,
                    format!(
                        "材質の参照がnullまたは材質GUID不在: {} スロット{slot}（{}）",
                        crate::escape_diag(&id_tables.paths[mesh_index]),
                        crate::escape_diag(prefab_label),
                    ),
                ));
            }
            let guid = reference.guid.as_ref().expect("GUIDの存在は確かめた");
            // 同じ欄パスへの重複は `seen_paths` で弾くが、添字の書式違い
            // （`data[0]` と `data[00]` など）は別文字列で同スロットになるため、
            // ここでも重複を先勝ちにせず失敗させる。
            if material_overrides
                .insert(
                    (mesh_index, slot),
                    (guid.as_str().to_owned(), reference.file_id),
                )
                .is_some()
            {
                return Err(duplicate_override(
                    prefab_label,
                    &modification.property_path,
                    target,
                ));
            }
            continue;
        }
        if modification.property_path == "m_IsActive" {
            if !seen_paths.insert((target, modification.property_path.clone())) {
                return Err(duplicate_override(
                    prefab_label,
                    &modification.property_path,
                    target,
                ));
            }
            let Some(path) = id_tables.go_at.get(&target) else {
                return Err(unknown_target(
                    prefab_label,
                    &modification.property_path,
                    target,
                ));
            };
            let active = read_flag_value(&modification.value, prefab_label, path, target)?;
            go_active.insert(path.clone(), active);
            continue;
        }
        if modification.property_path == "m_Enabled" {
            if !seen_paths.insert((target, modification.property_path.clone())) {
                return Err(duplicate_override(
                    prefab_label,
                    &modification.property_path,
                    target,
                ));
            }
            let Some(&mesh_index) = id_tables.renderer_at.get(&target) else {
                return Err(unknown_target(
                    prefab_label,
                    &modification.property_path,
                    target,
                ));
            };
            let path = &id_tables.paths[mesh_index];
            let enabled = read_flag_value(&modification.value, prefab_label, path, target)?;
            renderer_enabled.insert(mesh_index, enabled);
            continue;
        }
        // `m_Materials` の大きさなど、材質割当と有効状態に関わらない
        // 改変は無視する。対象の解決も求めない。
    }
    let base_at: HashMap<&str, &PrefabBaseMaterial> = base
        .iter()
        .map(|entry| (entry.slot.as_str(), entry))
        .collect();
    let mut assignments = Vec::new();
    let mut excluded_meshes = Vec::new();
    let mut excluded_names = Vec::new();
    let mut pool: Vec<RendererPoolEntry> = Vec::new();
    let mut pool_at: HashMap<(String, FileId), usize> = HashMap::new();
    for (mesh_index, view) in meshes.iter().enumerate() {
        let mut effective = Vec::with_capacity(view.slot_names.len());
        for (slot, slot_name) in view.slot_names.iter().enumerate() {
            if let Some(found) = material_overrides.get(&(mesh_index, slot)) {
                effective.push((found.0.clone(), found.1));
            } else if let Some(found) = base_at.get(slot_name.as_str()) {
                effective.push((found.guid.clone(), found.file_id));
            } else {
                return Err(VrmError::new(
                    VrmErrorKind::InvalidPrefab,
                    format!(
                        "上書きも置換表も無いため材質GUID不在: {} スロット{slot}（{slot_name}）（{}）",
                        crate::escape_diag(view.hierarchy_path),
                        crate::escape_diag(prefab_label),
                    ),
                ));
            }
        }
        // 残った上書きは対象スロットが存在しない。黙って捨てず失敗する。
        let overflow = material_overrides
            .keys()
            .filter(|(mesh, _)| *mesh == mesh_index)
            .map(|(_, slot)| *slot)
            .filter(|slot| *slot >= view.slot_names.len())
            .min();
        if let Some(slot) = overflow {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!(
                    "上書きのスロットがメッシュの材質数を超える: {} スロット{slot}（スロット{}件）（{}）",
                    crate::escape_diag(view.hierarchy_path),
                    view.slot_names.len(),
                    crate::escape_diag(prefab_label),
                ),
            ));
        }
        // 除外したメッシュの材質はプールに入れない。有効分だけが初出順になる。
        if !renderer_active(
            mesh_index,
            view.hierarchy_path,
            &renderer_enabled,
            &go_active,
        ) {
            excluded_meshes.push(mesh_index);
            excluded_names.push(view.model_name.to_owned());
            continue;
        }
        let mut pool_indices = Vec::with_capacity(effective.len());
        for (guid, file_id) in effective {
            let key = (guid.clone(), file_id);
            let pool_index = match pool_at.get(&key) {
                Some(&index) => index,
                None => {
                    let index = pool.len();
                    pool.push(RendererPoolEntry {
                        guid,
                        file_id,
                        path: String::new(),
                        first_renderer_path: view.hierarchy_path.to_owned(),
                    });
                    pool_at.insert(key, index);
                    index
                }
            };
            pool_indices.push(pool_index);
        }
        assignments.push(RendererMeshAssignment {
            mesh_index,
            model_name: view.model_name.to_owned(),
            renderer_index: mesh_index as u32,
            renderer_path: view.hierarchy_path.to_owned(),
            pool_indices,
        });
    }
    Ok(PrefabPlan {
        prefab_path: prefab_label.to_owned(),
        fbx_guid: fbx_guid.to_owned(),
        mode: PrefabMode::Instance,
        assignments,
        excluded_meshes,
        excluded_names,
        pool,
    })
}

/// 直接記述方式で計画を作る。
fn plan_direct_mode(
    decoded: &[(FileId, TypedObject)],
    prefab_label: &str,
    fbx_guid: &str,
    meshes: &[PrefabMeshView<'_>],
    id_tables: &ModelIdTables,
) -> Result<PrefabPlan, VrmError> {
    let mut game_objects: HashMap<FileId, &GameObject> = HashMap::new();
    let mut transforms: HashMap<FileId, &Transform> = HashMap::new();
    let mut transform_of_go: HashMap<FileId, FileId> = HashMap::new();
    for (file_id, typed) in decoded {
        match typed {
            TypedObject::GameObject(game_object) => {
                if game_objects.insert(*file_id, game_object).is_some() {
                    return Err(duplicate_document(prefab_label, *file_id));
                }
            }
            TypedObject::Transform(transform) => {
                if transforms.insert(*file_id, transform).is_some() {
                    return Err(duplicate_document(prefab_label, *file_id));
                }
                if transform_of_go
                    .insert(transform.game_object.file_id, *file_id)
                    .is_some()
                {
                    return Err(VrmError::new(
                        VrmErrorKind::InvalidPrefab,
                        format!(
                            "1つのゲームオブジェクトにTransformが複数ある: {}（{}）",
                            transform.game_object.file_id,
                            crate::escape_diag(prefab_label),
                        ),
                    ));
                }
            }
            _ => {}
        }
    }
    // 入力FBXを指す描画設定だけを集める。別FBXのものは扱わない。
    struct Candidate<'a> {
        renderer: &'a SkinnedMeshRenderer,
        mesh_index: usize,
        materials: Vec<(String, FileId)>,
        renderer_path: String,
    }
    let mut candidates: Vec<Candidate<'_>> = Vec::new();
    for (file_id, typed) in decoded {
        let TypedObject::SkinnedMeshRenderer(renderer) = typed else {
            continue;
        };
        if renderer.mesh.is_null()
            || renderer
                .mesh
                .guid
                .as_ref()
                .is_none_or(|guid| guid.as_str() != fbx_guid)
        {
            continue;
        };
        let Some(&mesh_index) = id_tables.mesh_at.get(&renderer.mesh.file_id) else {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!(
                    "メッシュIDを解決できない: 描画設定{}のfileID {}（{}。旧方式のPrefabは新方式のXXH64と一致しない）",
                    file_id,
                    renderer.mesh.file_id,
                    crate::escape_diag(prefab_label),
                ),
            ));
        };
        let view = &meshes[mesh_index];
        if renderer.materials.len() != view.slot_names.len() {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!(
                    "レンダラの材質数とメッシュの材質スロット数が一致しない: {}（{}件に対してスロット{}件）（{}）",
                    crate::escape_diag(view.hierarchy_path),
                    renderer.materials.len(),
                    view.slot_names.len(),
                    crate::escape_diag(prefab_label),
                ),
            ));
        }
        let mut materials = Vec::with_capacity(renderer.materials.len());
        for (slot, reference) in renderer.materials.iter().enumerate() {
            if reference.is_null() || reference.guid.is_none() {
                return Err(VrmError::new(
                    VrmErrorKind::InvalidPrefab,
                    format!(
                        "材質の参照がnullまたは材質GUID不在: {} スロット{slot}（{}）",
                        crate::escape_diag(view.hierarchy_path),
                        crate::escape_diag(prefab_label),
                    ),
                ));
            }
            let guid = reference.guid.as_ref().expect("GUIDの存在は確かめた");
            materials.push((guid.as_str().to_owned(), reference.file_id));
        }
        let renderer_path = hierarchy_names(
            renderer.game_object.file_id,
            &game_objects,
            &transforms,
            &transform_of_go,
            prefab_label,
        )?;
        candidates.push(Candidate {
            renderer,
            mesh_index,
            materials,
            renderer_path,
        });
    }
    if candidates.is_empty() {
        return Err(VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!(
                "Prefabが入力FBXを含まない: 入力FBXを指す描画設定が無い（{}）",
                crate::escape_diag(prefab_label),
            ),
        ));
    }
    let mut active_of: HashMap<usize, &Candidate<'_>> = HashMap::new();
    for candidate in &candidates {
        let active = candidate.renderer.enabled
            && hierarchy_active(
                candidate.renderer.game_object.file_id,
                &game_objects,
                &transforms,
                &transform_of_go,
                prefab_label,
            )?;
        if !active {
            continue;
        }
        if active_of.insert(candidate.mesh_index, candidate).is_some() {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!(
                    "1つのメッシュに複数の有効レンダラが対応するため先勝ちにしない: {}（{}）",
                    crate::escape_diag(meshes[candidate.mesh_index].model_name),
                    crate::escape_diag(prefab_label),
                ),
            ));
        }
    }
    let mut assignments = Vec::new();
    let mut excluded_meshes = Vec::new();
    let mut excluded_names = Vec::new();
    let mut pool: Vec<RendererPoolEntry> = Vec::new();
    let mut pool_at: HashMap<(String, FileId), usize> = HashMap::new();
    for (mesh_index, view) in meshes.iter().enumerate() {
        let Some(candidate) = active_of.get(&mesh_index) else {
            excluded_meshes.push(mesh_index);
            excluded_names.push(view.model_name.to_owned());
            continue;
        };
        let mut pool_indices = Vec::with_capacity(candidate.materials.len());
        for (guid, file_id) in &candidate.materials {
            let key = (guid.clone(), *file_id);
            let pool_index = match pool_at.get(&key) {
                Some(&index) => index,
                None => {
                    let index = pool.len();
                    pool.push(RendererPoolEntry {
                        guid: guid.clone(),
                        file_id: *file_id,
                        path: String::new(),
                        first_renderer_path: candidate.renderer_path.clone(),
                    });
                    pool_at.insert(key, index);
                    index
                }
            };
            pool_indices.push(pool_index);
        }
        assignments.push(RendererMeshAssignment {
            mesh_index,
            model_name: view.model_name.to_owned(),
            renderer_index: mesh_index as u32,
            renderer_path: candidate.renderer_path.clone(),
            pool_indices,
        });
    }
    Ok(PrefabPlan {
        prefab_path: prefab_label.to_owned(),
        fbx_guid: fbx_guid.to_owned(),
        mode: PrefabMode::Direct,
        assignments,
        excluded_meshes,
        excluded_names,
        pool,
    })
}

/// 描画設定の階層パス（表示用）を作る。
///
/// ゲームオブジェクト名を根元から `/` で繋ぐ。sidecarの `path` と同じ形に
/// なる。文書の欠落や循環は失敗にする。
fn hierarchy_names(
    mut game_object: FileId,
    game_objects: &HashMap<FileId, &GameObject>,
    transforms: &HashMap<FileId, &Transform>,
    transform_of_go: &HashMap<FileId, FileId>,
    prefab_label: &str,
) -> Result<String, VrmError> {
    let mut names = Vec::new();
    let mut visited = HashSet::new();
    loop {
        if !visited.insert(game_object) {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!(
                    "Prefab内の親子に循環がある: {}（{}）",
                    game_object,
                    crate::escape_diag(prefab_label),
                ),
            ));
        }
        let Some(object) = game_objects.get(&game_object) else {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!(
                    "Prefab内にゲームオブジェクトが無い: {}（{}）",
                    game_object,
                    crate::escape_diag(prefab_label),
                ),
            ));
        };
        names.push(object.name.clone());
        let Some(&transform_id) = transform_of_go.get(&game_object) else {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!(
                    "Prefab内にTransformが無い: {}（{}）",
                    crate::escape_diag(&object.name),
                    crate::escape_diag(prefab_label),
                ),
            ));
        };
        let Some(transform) = transforms.get(&transform_id) else {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!(
                    "Prefab内にTransformが無い: {}（{}）",
                    transform_id,
                    crate::escape_diag(prefab_label),
                ),
            ));
        };
        if transform.father.is_null() {
            break;
        }
        let Some(father) = transforms.get(&transform.father.file_id) else {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!(
                    "Prefab内に親Transformが無い: {}（{}）",
                    transform.father.file_id,
                    crate::escape_diag(prefab_label),
                ),
            ));
        };
        game_object = father.game_object.file_id;
    }
    names.reverse();
    Ok(names.join("/"))
}

/// 描画設定の有効を階層込みで判定する。
///
/// 本体の `m_Enabled` ではなく、ゲームオブジェクト鎖の `m_IsActive` だけを
/// 見る。呼び出し側は本体の有効と合わせて判定する。文書の欠落や循環は
/// 失敗にする。祖先の無効は子にも効く。
fn hierarchy_active(
    mut game_object: FileId,
    game_objects: &HashMap<FileId, &GameObject>,
    transforms: &HashMap<FileId, &Transform>,
    transform_of_go: &HashMap<FileId, FileId>,
    prefab_label: &str,
) -> Result<bool, VrmError> {
    let mut visited = HashSet::new();
    loop {
        if !visited.insert(game_object) {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!(
                    "Prefab内の親子に循環がある: {}（{}）",
                    game_object,
                    crate::escape_diag(prefab_label),
                ),
            ));
        }
        let Some(object) = game_objects.get(&game_object) else {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!(
                    "Prefab内にゲームオブジェクトが無い: {}（{}）",
                    game_object,
                    crate::escape_diag(prefab_label),
                ),
            ));
        };
        if !object.active {
            return Ok(false);
        }
        let Some(&transform_id) = transform_of_go.get(&game_object) else {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!(
                    "Prefab内にTransformが無い: {}（{}）",
                    crate::escape_diag(&object.name),
                    crate::escape_diag(prefab_label),
                ),
            ));
        };
        let Some(transform) = transforms.get(&transform_id) else {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!(
                    "Prefab内にTransformが無い: {}（{}）",
                    transform_id,
                    crate::escape_diag(prefab_label),
                ),
            ));
        };
        if transform.father.is_null() {
            return Ok(true);
        }
        let Some(father) = transforms.get(&transform.father.file_id) else {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!(
                    "Prefab内に親Transformが無い: {}（{}）",
                    transform.father.file_id,
                    crate::escape_diag(prefab_label),
                ),
            ));
        };
        game_object = father.game_object.file_id;
    }
}

/// 改変内容方式の有効を階層込みで判定する。
///
/// 描画設定自体の有効と、自階層のゲームオブジェクト・祖先の有効を合わせる。
/// 祖先の無効は子にも効く。取り込みルート（階層パスが空）の無効も子に効く。
/// 登録の無い階層は既定で有効である。祖先パスは階層パスの末尾からの分割で求める。
fn renderer_active(
    mesh_index: usize,
    hierarchy_path: &str,
    renderer_enabled: &HashMap<usize, bool>,
    go_active: &HashMap<String, bool>,
) -> bool {
    if renderer_enabled
        .get(&mesh_index)
        .is_some_and(|enabled| !enabled)
    {
        return false;
    }
    let mut rest = hierarchy_path;
    loop {
        if go_active.get(rest).is_some_and(|active| !active) {
            return false;
        }
        let Some((parent, _)) = rest.rsplit_once('/') else {
            // 取り込みルートの判定。`build_id_tables` が空パスで登録する。
            return go_active.get("").is_none_or(|active| *active);
        };
        rest = parent;
    }
}

/// `m_Materials.Array.data[i]` の添字を読む。形違いの場合は `None` である。
///
/// `m_Materials` で始まる他の形（大きさなど）は材質割当と有効状態に
/// 関わらないため無視する。`data[` で始まるのに添字が読めない場合は
/// 呼び出し側で失敗させる。
fn material_slot_index(property_path: &str) -> Option<usize> {
    const PREFIX: &str = "m_Materials.Array.data[";
    let rest = property_path.strip_prefix(PREFIX)?;
    let digits = rest.strip_suffix(']')?;
    digits.parse().ok()
}

/// 0/1または真偽値の改変値を真偽値として読む。
fn read_flag_value(
    value: &Option<Value>,
    prefab_label: &str,
    path: &str,
    target: FileId,
) -> Result<bool, VrmError> {
    match value {
        Some(Value::Bool(flag)) => Ok(*flag),
        Some(Value::Number(number)) => match number.as_i64() {
            Some(0) => Ok(false),
            Some(1) => Ok(true),
            _ => Err(flag_value_error(prefab_label, path, target)),
        },
        _ => Err(flag_value_error(prefab_label, path, target)),
    }
}

/// 真偽値の改変値の読み取り失敗を作る。
fn flag_value_error(prefab_label: &str, path: &str, target: FileId) -> VrmError {
    VrmError::new(
        VrmErrorKind::InvalidPrefab,
        format!(
            "有効状態の値が0/1でない: {}（対象{}）（{}）",
            crate::escape_diag(path),
            target,
            crate::escape_diag(prefab_label),
        ),
    )
}

/// 同じ対象・同じ欄パスへの重複上書きの失敗を作る。
fn duplicate_override(prefab_label: &str, property_path: &str, target: FileId) -> VrmError {
    VrmError::new(
        VrmErrorKind::InvalidPrefab,
        format!(
            "同じ対象への上書きが重複するため先勝ちにしない: {}（対象{}）（{}）",
            crate::escape_diag(property_path),
            target,
            crate::escape_diag(prefab_label),
        ),
    )
}

/// 新方式で解決できない対象の失敗を作る。
fn unknown_target(prefab_label: &str, property_path: &str, target: FileId) -> VrmError {
    VrmError::new(
        VrmErrorKind::InvalidPrefab,
        format!(
            "新方式のXXH64で対象を解決できない: {}（対象{}）（{}。旧方式のPrefabは未対応）",
            crate::escape_diag(property_path),
            target,
            crate::escape_diag(prefab_label),
        ),
    )
}

/// 文書IDの重複の失敗を作る。本文の解析では重複を弾くため到達しないはずだが、
///
/// 念のため先勝ちにせず失敗させる。
fn duplicate_document(prefab_label: &str, file_id: FileId) -> VrmError {
    VrmError::new(
        VrmErrorKind::InvalidPrefab,
        format!(
            "Prefab内の文書IDが重複する: {}（{}）",
            file_id,
            crate::escape_diag(prefab_label),
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::unity::{ExternalObjectEntry, ObjectRef, ParseOptions};

    /// テスト用のFBXのGUID。小文字hexである。
    const FBX_GUID: &str = "fb00000000000000000000000000000b";
    /// テスト用の別資産のGUID。
    const OTHER_GUID: &str = "0e00000000000000000000000000000d";
    /// テスト用の材質GUID。
    const FACE_GUID: &str = "face0000000000000000000000000001";
    /// テスト用の材質GUID。
    const COSTUME_GUID: &str = "c0570000000000000000000000000002";
    /// テスト用の上書き材質GUID。先頭に数字があっても英字を含むため文字列になる。
    const OVERRIDE_GUID: &str = "0e9a0000000000000000000000000003";

    /// YAML文字列をPrefabとして読む。
    fn parse_prefab(yaml: &str) -> SerializedFile {
        SerializedFile::parse(yaml.as_bytes(), &ParseOptions::new()).expect("検査YAMLは読める")
    }

    /// 改変内容方式の最小Prefabを作る。改変列は呼び出し側が渡す。
    fn instance_prefab(fbx_guid: &str, modifications: &str) -> String {
        format!(
            "%YAML 1.1\n%TAG !u! tag:unity3d.com,2011:\n\
             --- !u!1001 &100\nPrefabInstance:\n  m_ObjectHideFlags: 0\n  serializedVersion: 2\n  \
             m_Modification:\n    serializedVersion: 3\n    m_TransformParent: {{fileID: 0}}\n    \
             m_Modifications:\n{modifications}  \
             m_SourcePrefab: {{fileID: 100100000, guid: {fbx_guid}, type: 3}}\n"
        )
    }

    /// `m_Materials.Array.data[i]` の上書き1件を作る。
    fn material_override(target: FileId, slot: usize, guid: &str) -> String {
        format!(
            "    - target: {{fileID: {target}, guid: {FBX_GUID}, type: 3}}\n      \
             propertyPath: m_Materials.Array.data[{slot}]\n      value: \n      \
             objectReference: {{fileID: 2100000, guid: {guid}, type: 2}}\n"
        )
    }

    /// 有効状態の上書き1件を作る。
    fn flag_override(target: FileId, path: &str, value: i64) -> String {
        format!(
            "    - target: {{fileID: {target}, guid: {FBX_GUID}, type: 3}}\n      \
             propertyPath: {path}\n      value: {value}\n      objectReference: {{fileID: 0}}\n"
        )
    }

    /// 置換表由来の基礎材質1件を作る。
    fn base_material(slot: &str, guid: &str) -> PrefabBaseMaterial {
        PrefabBaseMaterial {
            slot: slot.to_owned(),
            guid: guid.to_owned(),
            file_id: 2100000,
        }
    }

    /// 中間メッシュ2件（BodyとPants）の要約を作る。
    ///
    /// Bodyは2スロット、Pantsは `Clothes` 配下の1スロットであり、
    /// 祖先の無効の伝播を確かめる材料になる。
    fn two_meshes<'a>(
        body_slots: &'a [String],
        pants_slots: &'a [String],
    ) -> [PrefabMeshView<'a>; 2] {
        [
            PrefabMeshView {
                model_name: "Body",
                hierarchy_path: "Body",
                slot_names: body_slots,
            },
            PrefabMeshView {
                model_name: "Pants",
                hierarchy_path: "Clothes/Pants",
                slot_names: pants_slots,
            },
        ]
    }

    #[test]
    fn 改変内容方式で上書きと祖先の無効を解決する() {
        // Bodyの1番スロットだけ上書きし、0番は置換表から引く。
        // `Clothes` を無効にしてPantsが祖先越しに除外される。
        let body_id = skinned_mesh_renderer_file_id("Body");
        let clothes_id = game_object_file_id("Clothes");
        let modifications = material_override(body_id, 1, OVERRIDE_GUID)
            + &flag_override(clothes_id, "m_IsActive", 0);
        let prefab = parse_prefab(&instance_prefab(FBX_GUID, &modifications));
        let body_slots = ["Face".to_owned(), "Costume".to_owned()];
        let pants_slots = ["Costume".to_owned()];
        let meshes = two_meshes(&body_slots, &pants_slots);
        let base = [
            base_material("Face", FACE_GUID),
            base_material("Costume", COSTUME_GUID),
        ];
        let plan =
            plan_prefab_assignment(&prefab, "test", FBX_GUID, &base, &meshes).expect("解決できる");
        assert_eq!(plan.mode, PrefabMode::Instance);
        assert_eq!(plan.assignments.len(), 1);
        assert_eq!(plan.assignments[0].model_name, "Body");
        assert_eq!(plan.assignments[0].pool_indices, vec![0, 1]);
        assert_eq!(plan.excluded_names, vec!["Pants".to_owned()]);
        assert_eq!(plan.pool.len(), 2);
        assert_eq!(plan.pool[0].guid, FACE_GUID);
        assert_eq!(plan.pool[1].guid, OVERRIDE_GUID);
        assert_eq!(plan.pool[0].file_id, 2100000);
    }

    #[test]
    fn 取り込みルートの無効は全体を除外する() {
        // 空パスのゲームオブジェクト（`Type:GameObject->//RootNode/root0`）への
        // `m_IsActive: 0` は実データ（固定ID）で裏付けた動作である。
        let root_id = game_object_file_id("");
        assert_eq!(root_id, 919132149155446097);
        let modifications = flag_override(root_id, "m_IsActive", 0);
        let prefab = parse_prefab(&instance_prefab(FBX_GUID, &modifications));
        let body_slots = ["Face".to_owned()];
        let pants_slots = ["Costume".to_owned()];
        let meshes = two_meshes(&body_slots, &pants_slots);
        let base = [
            base_material("Face", FACE_GUID),
            base_material("Costume", COSTUME_GUID),
        ];
        let plan =
            plan_prefab_assignment(&prefab, "test", FBX_GUID, &base, &meshes).expect("解決できる");
        assert!(plan.assignments.is_empty());
        assert_eq!(plan.excluded_names.len(), 2);
    }

    #[test]
    fn 描画設定の無効はそのメッシュだけを除外する() {
        // `m_Enabled: 0` は対象メッシュだけに効き、祖先には波及しない。
        let body_id = skinned_mesh_renderer_file_id("Body");
        let modifications = flag_override(body_id, "m_Enabled", 0);
        let prefab = parse_prefab(&instance_prefab(FBX_GUID, &modifications));
        let body_slots = ["Face".to_owned()];
        let pants_slots = ["Costume".to_owned()];
        let meshes = two_meshes(&body_slots, &pants_slots);
        let base = [
            base_material("Face", FACE_GUID),
            base_material("Costume", COSTUME_GUID),
        ];
        let plan =
            plan_prefab_assignment(&prefab, "test", FBX_GUID, &base, &meshes).expect("解決できる");
        assert_eq!(plan.assignments.len(), 1);
        assert_eq!(plan.assignments[0].model_name, "Pants");
        assert_eq!(plan.excluded_names, vec!["Body".to_owned()]);
    }

    #[test]
    fn 範囲外スロットの上書きは失敗する() {
        // 1スロットのメッシュに `data[1]` を書いても黙って捨てない。
        let body_id = skinned_mesh_renderer_file_id("Body");
        let modifications = material_override(body_id, 1, OVERRIDE_GUID);
        let prefab = parse_prefab(&instance_prefab(FBX_GUID, &modifications));
        let body_slots = ["Face".to_owned()];
        let pants_slots = ["Costume".to_owned()];
        let meshes = two_meshes(&body_slots, &pants_slots);
        let base = [
            base_material("Face", FACE_GUID),
            base_material("Costume", COSTUME_GUID),
        ];
        let error = plan_prefab_assignment(&prefab, "test", FBX_GUID, &base, &meshes)
            .expect_err("失敗する");
        assert_eq!(error.kind(), VrmErrorKind::InvalidPrefab);
    }

    #[test]
    fn null材質の上書きは失敗する() {
        // 空参照の上書きは灰色材質へ落とさず失敗する。
        let body_id = skinned_mesh_renderer_file_id("Body");
        let modifications = format!(
            "    - target: {{fileID: {body_id}, guid: {FBX_GUID}, type: 3}}\n      \
             propertyPath: m_Materials.Array.data[0]\n      value: \n      \
             objectReference: {{fileID: 0}}\n"
        );
        let prefab = parse_prefab(&instance_prefab(FBX_GUID, &modifications));
        let body_slots = ["Face".to_owned()];
        let pants_slots = ["Costume".to_owned()];
        let meshes = two_meshes(&body_slots, &pants_slots);
        let base = [
            base_material("Face", FACE_GUID),
            base_material("Costume", COSTUME_GUID),
        ];
        let error = plan_prefab_assignment(&prefab, "test", FBX_GUID, &base, &meshes)
            .expect_err("失敗する");
        assert_eq!(error.kind(), VrmErrorKind::InvalidPrefab);
    }

    #[test]
    fn 上書きも置換表も無いスロットは失敗する() {
        // Pantsの `Costume` を置換表から外すと解決できず失敗する。
        let prefab = parse_prefab(&instance_prefab(FBX_GUID, ""));
        let body_slots = ["Face".to_owned()];
        let pants_slots = ["Costume".to_owned()];
        let meshes = two_meshes(&body_slots, &pants_slots);
        let base = [base_material("Face", FACE_GUID)];
        let error = plan_prefab_assignment(&prefab, "test", FBX_GUID, &base, &meshes)
            .expect_err("失敗する");
        assert_eq!(error.kind(), VrmErrorKind::InvalidPrefab);
    }

    #[test]
    fn 新方式で解決できない対象は失敗する() {
        // 実在しないfileIDへの上書きは旧方式の疑いとして失敗する。
        let modifications = material_override(123456789, 0, OVERRIDE_GUID);
        let prefab = parse_prefab(&instance_prefab(FBX_GUID, &modifications));
        let body_slots = ["Face".to_owned()];
        let pants_slots = ["Costume".to_owned()];
        let meshes = two_meshes(&body_slots, &pants_slots);
        let base = [
            base_material("Face", FACE_GUID),
            base_material("Costume", COSTUME_GUID),
        ];
        let error = plan_prefab_assignment(&prefab, "test", FBX_GUID, &base, &meshes)
            .expect_err("失敗する");
        assert_eq!(error.kind(), VrmErrorKind::InvalidPrefab);
    }

    #[test]
    fn メッシュ識別子の重複は失敗する() {
        // 同名モデルが2件ある場合は先勝ちにしない。
        let prefab = parse_prefab(&instance_prefab(FBX_GUID, ""));
        let slots = ["Face".to_owned()];
        let meshes = [
            PrefabMeshView {
                model_name: "Body",
                hierarchy_path: "Body",
                slot_names: &slots,
            },
            PrefabMeshView {
                model_name: "Body",
                hierarchy_path: "Clothes/Body",
                slot_names: &slots,
            },
        ];
        let base = [base_material("Face", FACE_GUID)];
        let error = plan_prefab_assignment(&prefab, "test", FBX_GUID, &base, &meshes)
            .expect_err("失敗する");
        assert_eq!(error.kind(), VrmErrorKind::InvalidPrefab);
    }

    #[test]
    fn 複数の実体は先勝ちにせず失敗する() {
        // 入力FBXへの `PrefabInstance` が2件ある場合は曖昧さで失敗する。
        let one = instance_prefab(FBX_GUID, "");
        let two = one.replace("&100\n", "&200\n");
        let both = one + &two["%YAML 1.1\n%TAG !u! tag:unity3d.com,2011:\n".len()..];
        let prefab = parse_prefab(&both);
        let body_slots = ["Face".to_owned()];
        let pants_slots = ["Costume".to_owned()];
        let meshes = two_meshes(&body_slots, &pants_slots);
        let base = [
            base_material("Face", FACE_GUID),
            base_material("Costume", COSTUME_GUID),
        ];
        let error = plan_prefab_assignment(&prefab, "test", FBX_GUID, &base, &meshes)
            .expect_err("失敗する");
        assert_eq!(error.kind(), VrmErrorKind::InvalidPrefab);
    }

    #[test]
    fn 実体と直接記述の混在は失敗する() {
        // 同じFBXを両方式で書く場合は真の割当が定まらないため失敗する。
        let mesh_id = mesh_file_id("Body");
        let direct = format!(
            "--- !u!137 &300\nSkinnedMeshRenderer:\n  m_ObjectHideFlags: 0\n  \
             m_GameObject: {{fileID: 11}}\n  m_Enabled: 1\n  m_Materials:\n  \
             - {{fileID: 2100000, guid: {FACE_GUID}, type: 2}}\n  m_Bones: []\n  \
             m_RootBone: {{fileID: 0}}\n  m_Mesh: {{fileID: {mesh_id}, guid: {FBX_GUID}, type: 3}}\n"
        );
        let both = instance_prefab(FBX_GUID, "") + &direct;
        let prefab = parse_prefab(&both);
        let body_slots = ["Face".to_owned()];
        let pants_slots = ["Costume".to_owned()];
        let meshes = two_meshes(&body_slots, &pants_slots);
        let base = [
            base_material("Face", FACE_GUID),
            base_material("Costume", COSTUME_GUID),
        ];
        let error = plan_prefab_assignment(&prefab, "test", FBX_GUID, &base, &meshes)
            .expect_err("失敗する");
        assert_eq!(error.kind(), VrmErrorKind::InvalidPrefab);
    }

    #[test]
    fn 別資産の実体だけでは入力を含まないとして失敗する() {
        // GUIDが違う実体は対象外であり、描画設定も無ければ失敗する。
        let prefab = parse_prefab(&instance_prefab(OTHER_GUID, ""));
        let body_slots = ["Face".to_owned()];
        let pants_slots = ["Costume".to_owned()];
        let meshes = two_meshes(&body_slots, &pants_slots);
        let base = [
            base_material("Face", FACE_GUID),
            base_material("Costume", COSTUME_GUID),
        ];
        let error = plan_prefab_assignment(&prefab, "test", FBX_GUID, &base, &meshes)
            .expect_err("失敗する");
        assert_eq!(error.kind(), VrmErrorKind::InvalidPrefab);
    }

    /// 直接記述方式の最小Prefabを作る。
    ///
    /// `Root`（指定の有効状態）の下に `Body`（有効）と `Pants`（指定の有効状態）
    /// を置き、各描画設定は入力FBXのメッシュを指す。材質列は呼び出し側が渡す。
    fn direct_prefab(
        root_active: i64,
        pants_active: i64,
        body_mats: &str,
        pants_mats: &str,
    ) -> String {
        let body_mesh = mesh_file_id("Body");
        let pants_mesh = mesh_file_id("Pants");
        format!(
            "%YAML 1.1\n%TAG !u! tag:unity3d.com,2011:\n\
             --- !u!1 &10\nGameObject:\n  m_ObjectHideFlags: 0\n  m_Name: Root\n  m_Component:\n  \
             - component: {{fileID: 20}}\n  m_Layer: 0\n  m_TagString: Untagged\n  m_Icon: {{fileID: 0}}\n  \
             m_NavMeshLayer: 0\n  m_StaticEditorFlags: 0\n  m_IsActive: {root_active}\n\
             --- !u!4 &20\nTransform:\n  m_ObjectHideFlags: 0\n  m_GameObject: {{fileID: 10}}\n  \
             m_LocalPosition: {{x: 0, y: 0, z: 0}}\n  m_LocalRotation: {{x: 0, y: 0, z: 0, w: 1}}\n  \
             m_LocalScale: {{x: 1, y: 1, z: 1}}\n  m_Children:\n  - {{fileID: 21}}\n  - {{fileID: 23}}\n  \
             m_Father: {{fileID: 0}}\n\
             --- !u!1 &11\nGameObject:\n  m_ObjectHideFlags: 0\n  m_Name: Body\n  m_Component:\n  \
             - component: {{fileID: 21}}\n  - component: {{fileID: 31}}\n  m_Layer: 0\n  \
             m_TagString: Untagged\n  m_Icon: {{fileID: 0}}\n  m_NavMeshLayer: 0\n  \
             m_StaticEditorFlags: 0\n  m_IsActive: 1\n\
             --- !u!4 &21\nTransform:\n  m_ObjectHideFlags: 0\n  m_GameObject: {{fileID: 11}}\n  \
             m_LocalPosition: {{x: 0, y: 0, z: 0}}\n  m_LocalRotation: {{x: 0, y: 0, z: 0, w: 1}}\n  \
             m_LocalScale: {{x: 1, y: 1, z: 1}}\n  m_Children: []\n  m_Father: {{fileID: 20}}\n\
             --- !u!137 &31\nSkinnedMeshRenderer:\n  m_ObjectHideFlags: 0\n  \
             m_GameObject: {{fileID: 11}}\n  m_Enabled: 1\n{body_mats}  m_Bones: []\n  \
             m_RootBone: {{fileID: 0}}\n  m_Mesh: {{fileID: {body_mesh}, guid: {FBX_GUID}, type: 3}}\n\
             --- !u!1 &12\nGameObject:\n  m_ObjectHideFlags: 0\n  m_Name: Pants\n  m_Component:\n  \
             - component: {{fileID: 23}}\n  - component: {{fileID: 32}}\n  m_Layer: 0\n  \
             m_TagString: Untagged\n  m_Icon: {{fileID: 0}}\n  m_NavMeshLayer: 0\n  \
             m_StaticEditorFlags: 0\n  m_IsActive: {pants_active}\n\
             --- !u!4 &23\nTransform:\n  m_ObjectHideFlags: 0\n  m_GameObject: {{fileID: 12}}\n  \
             m_LocalPosition: {{x: 0, y: 0, z: 0}}\n  m_LocalRotation: {{x: 0, y: 0, z: 0, w: 1}}\n  \
             m_LocalScale: {{x: 1, y: 1, z: 1}}\n  m_Children: []\n  m_Father: {{fileID: 20}}\n\
             --- !u!137 &32\nSkinnedMeshRenderer:\n  m_ObjectHideFlags: 0\n  \
             m_GameObject: {{fileID: 12}}\n  m_Enabled: 1\n{pants_mats}  m_Bones: []\n  \
             m_RootBone: {{fileID: 0}}\n  m_Mesh: {{fileID: {pants_mesh}, guid: {FBX_GUID}, type: 3}}\n"
        )
    }

    /// 描画設定の材質列1件分を作る。
    fn direct_materials(guid: &str) -> String {
        format!("  m_Materials:\n  - {{fileID: 2100000, guid: {guid}, type: 2}}\n")
    }

    #[test]
    fn 直接記述方式で階層の無効を反映する() {
        // `Pants` 自体を無効にすると `Pants` だけが除外される。
        // 階層パスは根元からの名前連結になる。
        let prefab = parse_prefab(&direct_prefab(
            1,
            0,
            &direct_materials(FACE_GUID),
            &direct_materials(COSTUME_GUID),
        ));
        let body_slots = ["Face".to_owned()];
        let pants_slots = ["Costume".to_owned()];
        let meshes = [
            PrefabMeshView {
                model_name: "Body",
                hierarchy_path: "Body",
                slot_names: &body_slots,
            },
            PrefabMeshView {
                model_name: "Pants",
                hierarchy_path: "Pants",
                slot_names: &pants_slots,
            },
        ];
        let plan =
            plan_prefab_assignment(&prefab, "test", FBX_GUID, &[], &meshes).expect("解決できる");
        assert_eq!(plan.mode, PrefabMode::Direct);
        assert_eq!(plan.assignments.len(), 1);
        assert_eq!(plan.assignments[0].model_name, "Body");
        assert_eq!(plan.assignments[0].renderer_path, "Root/Body");
        assert_eq!(plan.excluded_names, vec!["Pants".to_owned()]);
        assert_eq!(plan.pool.len(), 1);
        assert_eq!(plan.pool[0].guid, FACE_GUID);
    }

    #[test]
    fn 直接記述方式で祖先の無効は子にも効く() {
        // 根元を無効にすると両方が除外される。
        let prefab = parse_prefab(&direct_prefab(
            0,
            1,
            &direct_materials(FACE_GUID),
            &direct_materials(COSTUME_GUID),
        ));
        let body_slots = ["Face".to_owned()];
        let pants_slots = ["Costume".to_owned()];
        let meshes = [
            PrefabMeshView {
                model_name: "Body",
                hierarchy_path: "Body",
                slot_names: &body_slots,
            },
            PrefabMeshView {
                model_name: "Pants",
                hierarchy_path: "Pants",
                slot_names: &pants_slots,
            },
        ];
        let plan =
            plan_prefab_assignment(&prefab, "test", FBX_GUID, &[], &meshes).expect("解決できる");
        assert!(plan.assignments.is_empty());
        assert_eq!(plan.excluded_names.len(), 2);
    }

    #[test]
    fn 直接記述方式で材質数が合わない場合は失敗する() {
        // 2材質の描画設定を1スロットのメッシュに当てると失敗する。
        let body_mats = direct_materials(FACE_GUID)
            + "  - {fileID: 2100000, guid: "
            + COSTUME_GUID
            + ", type: 2}\n";
        let prefab = parse_prefab(&direct_prefab(
            1,
            1,
            &body_mats,
            &direct_materials(COSTUME_GUID),
        ));
        let body_slots = ["Face".to_owned()];
        let pants_slots = ["Costume".to_owned()];
        let meshes = [
            PrefabMeshView {
                model_name: "Body",
                hierarchy_path: "Body",
                slot_names: &body_slots,
            },
            PrefabMeshView {
                model_name: "Pants",
                hierarchy_path: "Pants",
                slot_names: &pants_slots,
            },
        ];
        let error =
            plan_prefab_assignment(&prefab, "test", FBX_GUID, &[], &meshes).expect_err("失敗する");
        assert_eq!(error.kind(), VrmErrorKind::InvalidPrefab);
    }

    #[test]
    fn 直接記述方式で未知メッシュは失敗する() {
        // 取り込みIDと一致しない `m_Mesh` は旧方式の疑いとして失敗する。
        let prefab = parse_prefab(
            &direct_prefab(
                1,
                1,
                &direct_materials(FACE_GUID),
                &direct_materials(COSTUME_GUID),
            )
            .replace(&mesh_file_id("Pants").to_string(), "123456789"),
        );
        let body_slots = ["Face".to_owned()];
        let pants_slots = ["Costume".to_owned()];
        let meshes = [
            PrefabMeshView {
                model_name: "Body",
                hierarchy_path: "Body",
                slot_names: &body_slots,
            },
            PrefabMeshView {
                model_name: "Pants",
                hierarchy_path: "Pants",
                slot_names: &pants_slots,
            },
        ];
        let error =
            plan_prefab_assignment(&prefab, "test", FBX_GUID, &[], &meshes).expect_err("失敗する");
        assert_eq!(error.kind(), VrmErrorKind::InvalidPrefab);
    }

    /// 置換表の項目を作る。
    fn remap(slot: &str, guid: Option<&str>) -> ExternalObjectEntry {
        ExternalObjectEntry {
            original_type: "UnityEngine:Material".to_owned(),
            original_assembly: "UnityEngine.CoreModule".to_owned(),
            original_name: slot.to_owned(),
            replacement: ObjectRef {
                file_id: 2100000,
                guid: guid.and_then(|text| crate::unity::Guid::parse(text).ok()),
                type_hint: Some(2),
            },
        }
    }

    #[test]
    fn 基礎材質は材質置換だけを残し重複は最初を使う() {
        // 型違い・null置換・GUIDなしは落とし、同名の2件目は捨てる。
        let mut entries = vec![
            remap("Face", Some(FACE_GUID)),
            remap("Face", Some(OVERRIDE_GUID)),
            remap("Costume", Some(COSTUME_GUID)),
        ];
        entries.push(ExternalObjectEntry {
            original_type: "UnityEngine:Mesh".to_owned(),
            original_assembly: "UnityEngine.CoreModule".to_owned(),
            original_name: "Body".to_owned(),
            replacement: ObjectRef {
                file_id: 123,
                guid: crate::unity::Guid::parse(FACE_GUID).ok(),
                type_hint: Some(3),
            },
        });
        entries.push(remap("Null", None));
        let base = prefab_base_materials(&entries);
        assert_eq!(base.len(), 2);
        assert_eq!(base[0].slot, "Face");
        assert_eq!(base[0].guid, FACE_GUID);
        assert_eq!(base[1].slot, "Costume");
    }

    #[test]
    fn プールパスは索引の資産パスで埋める() {
        // GUIDが索引にあればそのパスを使い、無ければ空のまま残す。
        use crate::unity::{AssetIndex, AssetMeta};
        let mut index = AssetIndex::new();
        let meta = AssetMeta::parse(
            format!("fileFormatVersion: 2\nguid: {FACE_GUID}\nNativeFormatImporter:\n  foo: 1\n")
                .as_bytes(),
        )
        .expect("検査metaは読める");
        index.insert("Mats/Face.mat", meta).expect("索引に入る");
        let mut pool = vec![
            RendererPoolEntry {
                guid: FACE_GUID.to_owned(),
                file_id: 2100000,
                path: String::new(),
                first_renderer_path: "Body".to_owned(),
            },
            RendererPoolEntry {
                guid: COSTUME_GUID.to_owned(),
                file_id: 2100000,
                path: String::new(),
                first_renderer_path: "Pants".to_owned(),
            },
        ];
        resolve_prefab_pool_paths(&mut pool, &index);
        assert_eq!(pool[0].path, "Mats/Face.mat");
        assert!(pool[1].path.is_empty());
    }
}
