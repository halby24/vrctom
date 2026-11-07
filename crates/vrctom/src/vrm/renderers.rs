//! Unityヘルパが出力するレンダラsidecarの読み取りと材質割当。
//!
//! 入口は [`RendererSidecar::parse`]（バイト列の検証）、
//! [`plan_renderer_assignment`]（中間モデルへの対応付けと除外計画）、
//! [`apply_renderer_plan`]（除外と割当の適用）である。
//! 材質プールの読み取りは [`load_renderer_materials`]（Native限定）が担い、
//! `.mat` の解釈は [`crate::vrm::VrmMaterials`] の既存の写像・ベイク・
//! 取り込み解像度合わせを使う。新しいシェーダ変換器は作らない。
//!
//! ```mermaid
//! flowchart LR
//!     sidecar[RendererSidecar<br/>レンダラ→材質] --> plan[plan_renderer_assignment<br/>Model名で対応付け]
//!     fbx[FBXバイト列<br/>SHA-256照合] --> plan
//!     plan --> pool[load_renderer_materials<br/>材質プール]
//!     plan --> apply[apply_renderer_plan<br/>除外と割当]
//!     pool --> apply
//! ```
//!
//! 対応付けはUnityのメッシュサブアセット名（sidecarの `mesh.name`）と
//! FBXのModel名で行う。当方の出力メッシュ名はジオメトリ名のため一致しない
//! （`BandAid_Chest` に対する `Plane.001` など）。`model.nodePath` は
//! Prefab直置きのレンダラでは `null` のため使わない。
//! 有効なレンダラは `supported && enabled && activeInHierarchy` であり、
//! 無効なものは出力しない。材質プールは `(guid, fileID)` で同一性を判定し、
//! レンダラ順→サブメッシュ順の初出順に並べる。

use std::collections::{HashMap, HashSet};

use serde::Deserialize;

use super::error::{VrmError, VrmErrorKind};
use super::material::{MaterialReport, VrmMaterials};

/// レンダラsidecar JSONの書式名。
const SIDECAR_FORMAT: &str = "vrctom.renderers";
/// 対応するsidecarの版。
const SIDECAR_SCHEMA_VERSION: u32 = 1;

/// sidecar全体の資源上限。
///
/// フィールドは非公開で、既定値は [`Default`] が設定する。
/// 上限の変更は `with_` で始まるメソッドで行う。
#[derive(Debug, Clone)]
pub struct RendererLimits {
    max_json_bytes: usize,
    max_renderers: usize,
    max_models: usize,
    max_materials_per_renderer: usize,
}

impl RendererLimits {
    /// 既定の上限を持つ設定を生成する。
    pub fn new() -> Self {
        Self {
            // 実データの数十KBが収まり、破損由来の膨張を抑える大きさである。
            max_json_bytes: 16 << 20,
            // 実データの数十件が収まる大きさである。
            max_renderers: 100_000,
            // 実データの数件が収まる大きさである。
            max_models: 64,
            // 実データの数件が収まる大きさである。
            max_materials_per_renderer: 1024,
        }
    }

    /// JSONバイト数の上限を変更する。
    pub fn with_max_json_bytes(mut self, value: usize) -> Self {
        self.max_json_bytes = value;
        self
    }

    /// レンダラ数の上限を変更する。
    pub fn with_max_renderers(mut self, value: usize) -> Self {
        self.max_renderers = value;
        self
    }

    /// 参照モデル数の上限を変更する。
    pub fn with_max_models(mut self, value: usize) -> Self {
        self.max_models = value;
        self
    }

    /// 1レンダラの材質数の上限を変更する。
    pub fn with_max_materials_per_renderer(mut self, value: usize) -> Self {
        self.max_materials_per_renderer = value;
        self
    }
}

impl Default for RendererLimits {
    fn default() -> Self {
        Self::new()
    }
}

/// レンダラsidecar全体。検証済みのDTOである。
#[derive(Debug, Clone, Deserialize)]
pub struct RendererSidecar {
    /// 書式名。`vrctom.renderers` である。
    pub format: String,
    /// 版。`1` である。
    #[serde(rename = "schemaVersion")]
    pub schema_version: u32,
    /// 参照元のFBX列。
    pub models: Vec<RendererSourceModel>,
    /// レンダラ列。シーン走査順である。
    pub renderers: Vec<RendererEntry>,
}

/// 参照元のFBX1件。
#[derive(Debug, Clone, Deserialize)]
pub struct RendererSourceModel {
    /// 資産パス。
    pub path: String,
    /// 資産GUID。
    pub guid: String,
    /// FBXバイト列のSHA-256（小文字hex）。
    pub sha256: String,
}

/// レンダラ1件。
#[derive(Debug, Clone, Deserialize)]
pub struct RendererEntry {
    /// シーン走査順の番号。
    pub index: u32,
    /// Prefabインスタンスのルートからのパス。
    pub path: String,
    /// 対応する種類のとき真（SkinnedMeshRenderer / MeshRenderer）。
    pub supported: bool,
    /// 有効かどうか。
    pub enabled: bool,
    /// 階層全体で有効かどうか。
    #[serde(rename = "activeInHierarchy")]
    pub active_in_hierarchy: bool,
    /// 参照するメッシュ。未対応種類では `None` である。
    pub mesh: Option<RendererMesh>,
    /// サブメッシュ順の材質列。null要素は未割り当てである。
    #[serde(default)]
    pub materials: Vec<Option<RendererMaterial>>,
}

/// 参照するメッシュ1件。名前はUnityのメッシュサブアセット名であり、
/// FBXのModel名と一致する（当方のジオメトリ名とは限らない）。
#[derive(Debug, Clone, Deserialize)]
pub struct RendererMesh {
    /// メッシュ名。FBXのModel名である。
    pub name: String,
    /// サブメッシュ数。材質列の長さと一致する。
    #[serde(rename = "subMeshCount")]
    pub sub_mesh_count: u32,
}

/// 材質1件。
#[derive(Debug, Clone, Deserialize)]
pub struct RendererMaterial {
    /// 材質名。
    pub name: String,
    /// 材質のGUID。
    pub guid: String,
    /// 材質のfileID。
    #[serde(rename = "fileID")]
    pub file_id: i64,
    /// `.mat` の資産パス。
    pub path: String,
}

impl RendererSidecar {
    /// sidecarのJSONバイト列を読み、厳密に検証する。
    ///
    /// 書式名・版・参照モデル・レンダラ列の形状を確認する。
    /// SHA-256の照合と中間モデルへの対応付けは
    /// [`plan_renderer_assignment`] で行うため、ここでは扱わない。
    ///
    /// # Errors
    ///
    /// JSONの構文破損・書式名や版の不一致・参照モデルの欠落や書式不良・
    /// レンダラ列の形状不良・上限超過の場合に返す。
    pub fn parse(bytes: &[u8], limits: &RendererLimits) -> Result<Self, VrmError> {
        if bytes.len() > limits.max_json_bytes {
            return Err(VrmError::new(
                VrmErrorKind::InvalidRendererSidecar,
                "レンダラsidecarのバイト数が上限を超えた",
            ));
        }
        let sidecar: Self = serde_json::from_slice(bytes).map_err(|error| {
            VrmError::new(
                VrmErrorKind::InvalidRendererSidecar,
                format!("レンダラsidecarのJSONが読めない: {error}"),
            )
        })?;
        sidecar.validate(limits)?;
        Ok(sidecar)
    }

    /// JSON文字列から読む。[`Self::parse`] の文字列版である。
    ///
    /// # Errors
    ///
    /// [`Self::parse`] と同じ条件で返す。
    pub fn from_json(text: &str, limits: &RendererLimits) -> Result<Self, VrmError> {
        Self::parse(text.as_bytes(), limits)
    }

    /// 有効なレンダラ（出力対象）だけを `index` 順に返す。
    ///
    /// 有効とは `supported && enabled && activeInHierarchy` である。
    pub fn active_renderers(&self) -> Vec<&RendererEntry> {
        let mut active: Vec<&RendererEntry> = self
            .renderers
            .iter()
            .filter(|entry| entry.is_active())
            .collect();
        active.sort_by_key(|entry| entry.index);
        active
    }

    /// 検証済みDTOの不変条件を確認する。
    fn validate(&self, limits: &RendererLimits) -> Result<(), VrmError> {
        if self.format != SIDECAR_FORMAT {
            return Err(VrmError::new(
                VrmErrorKind::InvalidRendererSidecar,
                format!("レンダラsidecarの書式名が一致しない: {}", self.format),
            ));
        }
        if self.schema_version != SIDECAR_SCHEMA_VERSION {
            return Err(VrmError::new(
                VrmErrorKind::InvalidRendererSidecar,
                format!("レンダラsidecarの版が未対応である: {}", self.schema_version),
            ));
        }
        if self.models.is_empty() || self.models.len() > limits.max_models {
            return Err(VrmError::new(
                VrmErrorKind::InvalidRendererSidecar,
                "レンダラsidecarの参照モデル件数が範囲外である",
            ));
        }
        {
            let mut seen = HashSet::new();
            for model in &self.models {
                if model.path.is_empty() {
                    return Err(VrmError::new(
                        VrmErrorKind::InvalidRendererSidecar,
                        "参照モデルのパスが空である",
                    ));
                }
                if !seen.insert(model.path.as_str()) {
                    return Err(VrmError::new(
                        VrmErrorKind::InvalidRendererSidecar,
                        format!("参照モデルのパスが重複する: {}", model.path),
                    ));
                }
                if crate::unity::Guid::parse(&model.guid).is_err() {
                    return Err(VrmError::new(
                        VrmErrorKind::InvalidRendererSidecar,
                        format!("参照モデルのGUIDが32桁hexでない: {}", model.path),
                    ));
                }
                check_sha256_hex(&model.sha256).map_err(|reason| {
                    VrmError::new(
                        VrmErrorKind::InvalidRendererSidecar,
                        format!(
                            "参照モデルのSHA-256が64桁hexでない: {}: {reason}",
                            model.path
                        ),
                    )
                })?;
            }
        }
        if self.renderers.len() > limits.max_renderers {
            return Err(VrmError::new(
                VrmErrorKind::LimitExceeded,
                "レンダラsidecarのレンダラ数が上限を超えた",
            ));
        }
        {
            let mut seen = HashSet::new();
            for entry in &self.renderers {
                if !seen.insert(entry.index) {
                    return Err(VrmError::new(
                        VrmErrorKind::InvalidRendererSidecar,
                        format!("レンダラの番号が重複する: {}", entry.index),
                    ));
                }
                if entry.path.is_empty() {
                    return Err(VrmError::new(
                        VrmErrorKind::InvalidRendererSidecar,
                        format!("レンダラのパスが空である: {}", entry.index),
                    ));
                }
                if entry.materials.len() > limits.max_materials_per_renderer {
                    return Err(VrmError::new(
                        VrmErrorKind::LimitExceeded,
                        format!("レンダラの材質数が上限を超えた: {}", entry.path),
                    ));
                }
                if !entry.supported {
                    continue;
                }
                let Some(mesh) = entry.mesh.as_ref() else {
                    return Err(VrmError::new(
                        VrmErrorKind::InvalidRendererSidecar,
                        format!("対応種類のレンダラにメッシュが無い: {}", entry.path),
                    ));
                };
                if mesh.name.is_empty() {
                    return Err(VrmError::new(
                        VrmErrorKind::InvalidRendererSidecar,
                        format!("レンダラのメッシュ名が空である: {}", entry.path),
                    ));
                }
                if mesh.sub_mesh_count as usize != entry.materials.len() {
                    return Err(VrmError::new(
                        VrmErrorKind::InvalidRendererSidecar,
                        format!(
                            "レンダラのサブメッシュ数と材質数が一致しない: {}（{}に対して{}）",
                            entry.path,
                            mesh.sub_mesh_count,
                            entry.materials.len()
                        ),
                    ));
                }
            }
        }
        Ok(())
    }
}

impl RendererEntry {
    /// 出力対象の有効なレンダラかどうかを返す。
    ///
    /// `supported && enabled && activeInHierarchy` の場合だけ真である。
    /// 無効なものは材質数にもメッシュ数にも効かない。
    pub fn is_active(&self) -> bool {
        self.supported && self.enabled && self.active_in_hierarchy
    }
}

/// 対応付け用の中間メッシュ1件の要約。
///
/// FBX由来の所有モデル名と材質スロット数だけを持ち、読み取り層に依存しない。
/// 呼び出し側は中間モデルのメッシュ順に並べる。
#[derive(Debug, Clone)]
pub struct RendererMeshView<'a> {
    /// 所有モデルの名前。sidecarの `mesh.name` と突き合わせる。
    pub model_name: &'a str,
    /// 材質スロット数。レンダラの材質配列長と一致する。
    pub slot_count: usize,
}

/// 材質プール1件の識別子と読み取り元。
///
/// 同一性は `(guid, fileID)` で判定し、読み取りは `.mat` の資産パスから行う。
/// 初出のレンダラ順→サブメッシュ順に並べる。
#[derive(Debug, Clone)]
pub struct RendererPoolEntry {
    /// 材質のGUID（小文字hex）。
    pub guid: String,
    /// 材質のfileID。
    pub file_id: i64,
    /// `.mat` の資産パス。初出の記録を使う。
    pub path: String,
    /// 初出のレンダラパス。診断に使う。
    pub first_renderer_path: String,
}

/// 有効レンダラ1件と中間メッシュの対応付け。
#[derive(Debug, Clone)]
pub struct RendererMeshAssignment {
    /// 中間メッシュ列の索引（適用前の番号）。
    pub mesh_index: usize,
    /// 対応付けたModel名（sidecarの `mesh.name`）。
    pub model_name: String,
    /// レンダラ番号。
    pub renderer_index: u32,
    /// レンダラパス。診断に使う。
    pub renderer_path: String,
    /// ローカルスロット順の材質プール索引。
    pub pool_indices: Vec<usize>,
}

/// 対応付けと除外の計画。
#[derive(Debug, Clone)]
pub struct RendererPlan {
    /// SHA-256が一致した参照モデルの資産パス。
    pub matched_model_path: String,
    /// 一致したSHA-256（小文字hex）。
    pub matched_model_sha256: String,
    /// 有効レンダラの対応付け。レンダラ番号順である。
    pub assignments: Vec<RendererMeshAssignment>,
    /// 出力しない中間メッシュの索引（中間モデル順）。
    pub excluded_meshes: Vec<usize>,
    /// 除外した中間メッシュのModel名（中間モデル順）。
    pub excluded_names: Vec<String>,
    /// 材質プール。初出順である。
    pub pool: Vec<RendererPoolEntry>,
}

impl RendererPlan {
    /// 有効レンダラ数を返す。
    pub fn active_count(&self) -> usize {
        self.assignments.len()
    }
}

/// 適用結果の報告。
#[derive(Debug, Clone)]
pub struct RendererApplyReport {
    /// 出力に残したメッシュ数。
    pub included_count: usize,
    /// 除外したメッシュ数。
    pub excluded_count: usize,
    /// 除外したメッシュのModel名（中間モデル順）。
    pub excluded_names: Vec<String>,
    /// 材質プールの件数。
    pub pool_count: usize,
}

/// 材質プール1件に対応する材質スロット名を作る。
///
/// 同じ `(guid, fileID)` は同じスロット名になり、書き出し側の
/// 重複排除で1つのglTF材質にまとまる。決定的であり、列挙順に依存しない。
pub(crate) fn renderer_slot_key(guid: &str, file_id: i64) -> String {
    format!("renderer:{guid}:{file_id}")
}

/// sidecarを中間モデルへ対応付け、除外計画を作る。
///
/// `fbx_bytes` は入力FBXの生バイト列であり、SHA-256を `models[]` と照合する。
/// `meshes` は中間モデルのメッシュ順の要約であり、Model名で一意に解決する。
/// 先勝ちにせず、見つからない・重複する場合は失敗する。
/// 表情の参照先の検査は [`apply_renderer_plan`] で行うため、ここでは扱わない。
///
/// # Errors
///
/// SHA-256の不一致・未知や重複のメッシュ名・材質数の不一致・null材質・
/// 1メッシュへの複数レンダラの対応などの場合に返す。
/// いずれも対象の名前や件数を文面に含める。
pub fn plan_renderer_assignment(
    sidecar: &RendererSidecar,
    fbx_bytes: &[u8],
    meshes: &[RendererMeshView<'_>],
) -> Result<RendererPlan, VrmError> {
    let digest = sha256_hex(fbx_bytes);
    let matched: Vec<&RendererSourceModel> = sidecar
        .models
        .iter()
        .filter(|model| model.sha256.to_ascii_lowercase() == digest)
        .collect();
    let [single] = matched.as_slice() else {
        let candidates: Vec<String> = sidecar
            .models
            .iter()
            .map(|model| format!("{}:{}", model.path, model.sha256))
            .collect();
        return Err(VrmError::new(
            VrmErrorKind::InvalidRendererSidecar,
            format!(
                "FBXのSHA-256に一致する参照モデルが1件でない: {}件（入力{} 候補{}）",
                matched.len(),
                digest,
                candidates.join(", ")
            ),
        ));
    };
    // Model名から中間メッシュ索引への表。重複の検出は解決時に行う。
    let mut by_model_name: HashMap<&str, Vec<usize>> = HashMap::new();
    for (index, view) in meshes.iter().enumerate() {
        by_model_name
            .entry(view.model_name)
            .or_default()
            .push(index);
    }
    let mut assignments = Vec::new();
    let mut assigned_meshes = HashSet::new();
    let mut pool: Vec<RendererPoolEntry> = Vec::new();
    let mut pool_at: HashMap<(String, i64), usize> = HashMap::new();
    for entry in sidecar.active_renderers() {
        let Some(mesh) = entry.mesh.as_ref() else {
            return Err(VrmError::new(
                VrmErrorKind::InvalidRendererSidecar,
                format!("有効レンダラにメッシュが無い: {}", entry.path),
            ));
        };
        // null材質は灰色で続行せず失敗する。スロット番号も添える。
        for (slot, material) in entry.materials.iter().enumerate() {
            if material.is_none() {
                return Err(VrmError::new(
                    VrmErrorKind::InvalidRendererSidecar,
                    format!(
                        "有効レンダラにnull材質がある: {} スロット{slot}",
                        entry.path
                    ),
                ));
            }
        }
        let candidates = by_model_name.get(mesh.name.as_str());
        let mesh_index = match candidates.map(Vec::as_slice) {
            None | Some([]) => {
                return Err(VrmError::new(
                    VrmErrorKind::InvalidRendererSidecar,
                    format!(
                        "sidecarにある有効レンダラがFBXに無い: {}（レンダラ{}）",
                        mesh.name, entry.path
                    ),
                ));
            }
            Some(&[single]) => single,
            Some(many) => {
                return Err(VrmError::new(
                    VrmErrorKind::InvalidRendererSidecar,
                    format!(
                        "Model名の解決が一意でない: {}（候補{}件 レンダラ{}）",
                        mesh.name,
                        many.len(),
                        entry.path
                    ),
                ));
            }
        };
        if !assigned_meshes.insert(mesh_index) {
            return Err(VrmError::new(
                VrmErrorKind::InvalidRendererSidecar,
                format!(
                    "1つのメッシュに複数の有効レンダラが対応する: {}（レンダラ{}）",
                    mesh.name, entry.path
                ),
            ));
        }
        let slot_count = meshes[mesh_index].slot_count;
        if entry.materials.len() != slot_count {
            return Err(VrmError::new(
                VrmErrorKind::InvalidRendererSidecar,
                format!(
                    "レンダラの材質数とメッシュの材質スロット数が一致しない: {}（レンダラ{}件に対してスロット{}件）",
                    entry.path,
                    entry.materials.len(),
                    slot_count
                ),
            ));
        }
        let mut pool_indices = Vec::with_capacity(entry.materials.len());
        for material in entry.materials.iter().flatten() {
            let guid = material.guid.to_ascii_lowercase();
            let key = (guid.clone(), material.file_id);
            let pool_index = match pool_at.get(&key) {
                Some(&index) => index,
                None => {
                    let index = pool.len();
                    pool.push(RendererPoolEntry {
                        guid,
                        file_id: material.file_id,
                        path: material.path.clone(),
                        first_renderer_path: entry.path.clone(),
                    });
                    pool_at.insert(key, index);
                    index
                }
            };
            pool_indices.push(pool_index);
        }
        assignments.push(RendererMeshAssignment {
            mesh_index,
            model_name: mesh.name.clone(),
            renderer_index: entry.index,
            renderer_path: entry.path.clone(),
            pool_indices,
        });
    }
    let mut excluded_meshes = Vec::new();
    let mut excluded_names = Vec::new();
    for (index, view) in meshes.iter().enumerate() {
        if !assigned_meshes.contains(&index) {
            excluded_meshes.push(index);
            excluded_names.push(view.model_name.to_owned());
        }
    }
    Ok(RendererPlan {
        matched_model_path: single.path.clone(),
        matched_model_sha256: single.sha256.to_ascii_lowercase(),
        assignments,
        excluded_meshes,
        excluded_names,
        pool,
    })
}

/// 対応付け計画を中間モデルへ適用する。
///
/// 除外したメッシュは出力しないが、ノードやスキンの関節は消さない。
/// 表情の参照先が除外された場合は黙って消さず失敗する。
/// 残したメッシュの材質スロットは材質プールの共有名へ置き換え、
/// `primitive.material` の引き方は保つ。`materials` の順序は
/// レンダラ順→サブメッシュ順の初出順であり、決定的である。
/// 表情の束ねのメッシュ索引は残した列へ写し替える。
///
/// # Errors
///
/// 表情の参照先が除外された場合や、計画とモデルの不整合の場合に返す。
pub fn apply_renderer_plan(
    model: &mut super::VrmModel,
    plan: &RendererPlan,
    materials: &VrmMaterials,
) -> Result<RendererApplyReport, VrmError> {
    apply_assignment_core(
        model,
        &plan.assignments,
        &plan.excluded_meshes,
        &plan.excluded_names,
        &plan.pool,
        materials,
        VrmErrorKind::InvalidRendererSidecar,
    )
}

/// 対応付けの適用本体。sidecar由来とPrefab由来で共有する。
///
/// 計画の由来（sidecarかPrefabか）で失敗の種別だけを変え、
/// 適用の手順は同じにする。呼び出し側は由来に応じた適用口
/// （[`apply_renderer_plan`] など）を使う。
pub(crate) fn apply_assignment_core(
    model: &mut super::VrmModel,
    assignments: &[RendererMeshAssignment],
    excluded_meshes: &[usize],
    excluded_names: &[String],
    pool: &[RendererPoolEntry],
    materials: &VrmMaterials,
    kind: VrmErrorKind,
) -> Result<RendererApplyReport, VrmError> {
    let excluded: HashSet<usize> = excluded_meshes.iter().copied().collect();
    for preset in &model.expressions.preset {
        for bind in &preset.morph_target_binds {
            if excluded.contains(&bind.mesh) {
                return Err(VrmError::new(
                    kind,
                    format!(
                        "表情の参照先が除外された: {}（メッシュ{}）",
                        preset.name, bind.mesh
                    ),
                ));
            }
        }
    }
    if materials.entries().len() != pool.len() {
        return Err(VrmError::new(
            kind,
            format!(
                "材質プールの件数が計画と一致しない: {}件に対して{}件",
                materials.entries().len(),
                pool.len()
            ),
        ));
    }
    let slot_of = |pool_index: usize| {
        let entry = &pool[pool_index];
        renderer_slot_key(&entry.guid, entry.file_id)
    };
    let assignment_at: HashMap<usize, &RendererMeshAssignment> = assignments
        .iter()
        .map(|assignment| (assignment.mesh_index, assignment))
        .collect();
    let mut meshes = Vec::with_capacity(assignments.len());
    let mut remap = HashMap::new();
    for (old_index, mesh) in model.meshes.iter().enumerate() {
        let Some(assignment) = assignment_at.get(&old_index) else {
            continue;
        };
        if mesh.material_slots.len() != assignment.pool_indices.len() {
            return Err(VrmError::new(
                kind,
                format!(
                    "メッシュの材質スロット数が計画と一致しない: {}（{}に対して{}）",
                    assignment.model_name,
                    mesh.material_slots.len(),
                    assignment.pool_indices.len()
                ),
            ));
        }
        let mut mesh = mesh.clone();
        mesh.material_slots = assignment
            .pool_indices
            .iter()
            .map(|&pool_index| slot_of(pool_index))
            .collect();
        remap.insert(old_index, meshes.len());
        meshes.push(mesh);
    }
    if meshes.len() != assignments.len() {
        return Err(VrmError::new(
            kind,
            "対応付けたメッシュ数が出力に一致しない",
        ));
    }
    for preset in &mut model.expressions.preset {
        for bind in &mut preset.morph_target_binds {
            let Some(&next) = remap.get(&bind.mesh) else {
                return Err(VrmError::new(
                    kind,
                    format!(
                        "表情の参照先が除外された: {}（メッシュ{}）",
                        preset.name, bind.mesh
                    ),
                ));
            };
            bind.mesh = next;
        }
    }
    model.meshes = meshes;
    Ok(RendererApplyReport {
        included_count: assignments.len(),
        excluded_count: excluded_meshes.len(),
        excluded_names: excluded_names.to_owned(),
        pool_count: pool.len(),
    })
}

/// 材質プールを `.mat` から読み取る（Native限定）。
///
/// `pool` は [`plan_renderer_assignment`] の初出順の列であり、
/// 同じ `(guid, fileID)` は1つの材質にまとまる。
/// 材質名は `.mat` の `m_Name` を使い、写像・ベイク・取り込み解像度合わせは
/// [`VrmMaterials`] の既存の処理を使う。null材質は計画時に弾くため、
/// ここでは扱わない。
///
/// # Errors
///
/// [`VrmMaterials::load`](super::material::VrmMaterials::load) と同じ条件で返す。
#[cfg(not(target_arch = "wasm32"))]
pub fn load_renderer_materials(
    root: &std::path::Path,
    pool: &[RendererPoolEntry],
    options: &super::material::MaterialOptions,
) -> Result<(VrmMaterials, MaterialReport), super::material::MaterialError> {
    use crate::unity::{IndexOptions, scan_directory};

    let canonical_root = std::fs::canonicalize(root).map_err(|error| {
        super::material::MaterialError::new(
            super::material::MaterialErrorKind::Io,
            format!(
                "材質の走査元を解決できない: {}",
                crate::escape_diag(&error.to_string())
            ),
        )
        .with_path(root.to_string_lossy().into_owned())
        .with_source(error)
    })?;
    let canonical_assets =
        std::fs::canonicalize(canonical_root.join("Assets")).map_err(|error| {
            super::material::MaterialError::new(
                super::material::MaterialErrorKind::Io,
                format!(
                    "材質のAssetsを解決できない: {}",
                    crate::escape_diag(&error.to_string())
                ),
            )
            .with_path(canonical_root.join("Assets").to_string_lossy().into_owned())
            .with_source(error)
        })?;
    let index = scan_directory(&canonical_assets, &IndexOptions::default()).map_err(|error| {
        let path = error.path().map(str::to_owned);
        let mut failure = super::material::MaterialError::new(
            super::material::MaterialErrorKind::Unity,
            format!("材質の索引化に失敗した: {error}"),
        )
        .with_source(error);
        if let Some(path) = path {
            failure = failure.with_path(path);
        }
        failure
    })?;
    load_renderer_materials_with_index(&canonical_assets, &index, pool, options)
}

/// 索引済みの配置から材質プールを `.mat` から読み取る（Native限定）。
///
/// [`load_renderer_materials`] の走査済み版であり、Prefab由来の計画など
/// 索引を先に使う場合に二重走査を避ける。`assets` は `Assets`
/// そのものを指し、`index` はその走査結果である。
/// 材質プールの扱いは [`load_renderer_materials`] と同じである。
///
/// # Errors
///
/// [`VrmMaterials::load`](super::material::VrmMaterials::load) と同じ条件で返す。
/// ただし走査は行わないため、索引化の失敗は呼び出し側の責任である。
#[cfg(not(target_arch = "wasm32"))]
pub fn load_renderer_materials_with_index(
    assets: &std::path::Path,
    index: &crate::unity::AssetIndex,
    pool: &[RendererPoolEntry],
    options: &super::material::MaterialOptions,
) -> Result<(VrmMaterials, MaterialReport), super::material::MaterialError> {
    let source = crate::unity::FsAssetSource::new(assets);
    load_renderer_materials_from(&source, index, pool, options)
}

/// 材質プールを `.mat` から読み取る（ターゲット非依存）。
///
/// `source` は `Assets` 直下を基準とする読み手、`index` はその索引であり、
/// Nativeの展開済み配置とWebのメモリ内パッケージのどちらでも使う。
/// `pool` は [`plan_renderer_assignment`] の初出順の列であり、
/// 同じ `(guid, fileID)` は1つの材質にまとまる。
/// 材質名は `.mat` の `m_Name` を使い、写像・ベイク・取り込み解像度合わせは
/// [`VrmMaterials`] の既存の処理を使う。null材質は計画時に弾くため、
/// ここでは扱わない。
///
/// # Errors
///
/// [`VrmMaterials::load`](super::material::VrmMaterials::load) と同じ条件で返す。
pub fn load_renderer_materials_from(
    source: &dyn crate::unity::AssetSource,
    index: &crate::unity::AssetIndex,
    pool: &[RendererPoolEntry],
    options: &super::material::MaterialOptions,
) -> Result<(VrmMaterials, MaterialReport), super::material::MaterialError> {
    use super::material::LoadContext;

    let mut context = LoadContext::new(source, index, options);
    for entry in pool {
        context.load_renderer_material(&entry.guid, &entry.path, &slot_key_of(entry))?;
    }
    context.assign_render_queue_offsets();
    Ok(context.finish())
}

/// 材質プール1件に対応する材質スロット名を作る.
///
/// sidecar由来とPrefab由来で同じ写像を使い、書き出し側の共有に合流させる。
fn slot_key_of(entry: &RendererPoolEntry) -> String {
    renderer_slot_key(&entry.guid, entry.file_id)
}

/// FBXバイト列のSHA-256を小文字hexで返す。
fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest as _;
    let digest = sha2::Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// SHA-256の書式（64桁hex）を確認する。
fn check_sha256_hex(text: &str) -> Result<(), &'static str> {
    if text.len() == 64 && text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err("64桁hexでない")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    use crate::unity::{AssetIndex, AssetMeta, AssetSource};
    use crate::vrm::MaterialOptions;

    // テスト名は日本語で書く（テストは仕様・契約であるため）。

    /// メモリ内の読み手。パスと内容の対だけを持ち、ファイルシステムに触れない。
    struct MapSource {
        files: HashMap<String, Vec<u8>>,
    }

    impl AssetSource for MapSource {
        fn read_limited(&self, path: &str, limit: u64) -> std::io::Result<Vec<u8>> {
            let Some(bytes) = self.files.get(path) else {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "持たない",
                ));
            };
            if bytes.len() as u64 > limit {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "上限を超えた",
                ));
            }
            Ok(bytes.clone())
        }
    }

    /// 検査用の材質GUID（32桁hex）。引用符なしでも数値化されないよう英字を含む。
    const TEST_MAT_GUID: &str = "b0d71111111111111111111111111111";
    /// 不透明lilToonシェーダのGUID。`.shader` が無くても系列を判定できる。
    const TEST_SHADER_OPAQUE: &str = "df12117ecd77c31469c224178886498e";

    /// 検査用の最小 `.mat` を組み立てる。テクスチャを持たない。
    fn 検査材質を作る(名前: &str) -> Vec<u8> {
        format!(
            "%YAML 1.1\n%TAG !u! tag:unity3d.com,2011:\n--- !u!21 &2100000\nMaterial:\n  m_Name: {名前}\n  m_Shader: {{fileID: 4800000, guid: {TEST_SHADER_OPAQUE}, type: 3}}\n  m_SavedProperties:\n    m_TexEnvs: []\n    m_Floats:\n    - _Cutoff: 0.5\n    - _Cull: 2.0\n    - _ZWrite: 1.0\n    - _UseShadow: 1.0\n    - _ShadowBorder: 0.5\n    - _ShadowBlur: 0.1\n    - _lilToonVersion: 45.0\n    m_Ints: []\n    m_Colors:\n    - _Color: {{r: 1, g: 1, b: 1, a: 1}}\n    - _ShadowColor: {{r: 0.82, g: 0.76, b: 0.85, a: 1}}\n"
        )
        .into_bytes()
    }

    /// 検査材質だけを持つ索引を作る。
    fn 検査索引を作る() -> AssetIndex {
        let mut index = AssetIndex::new();
        let meta = AssetMeta::parse(
            format!(
                "fileFormatVersion: 2\nguid: {TEST_MAT_GUID}\nNativeFormatImporter:\n  externalObjects: {{}}\n  userData: \n"
            )
            .as_bytes(),
        )
        .expect("検査metaは読める");
        index
            .insert("Materials/Body.mat", meta)
            .expect("索引に入る");
        index
    }

    /// 検査材質1件の材質プールを作る。
    fn 検査プールを作る() -> Vec<RendererPoolEntry> {
        vec![RendererPoolEntry {
            guid: TEST_MAT_GUID.to_owned(),
            file_id: 2100000,
            path: "Assets/Materials/Body.mat".to_owned(),
            first_renderer_path: "Body".to_owned(),
        }]
    }

    #[test]
    fn メモリ内読み手から材質プールを読める() {
        // `MemoryPackage` などwasm側の読み手と同じく、ファイルシステムに触れない。
        let source = MapSource {
            files: HashMap::from([("Materials/Body.mat".to_owned(), 検査材質を作る("肌"))]),
        };
        let index = 検査索引を作る();
        let options = MaterialOptions::new();
        let (materials, _) =
            load_renderer_materials_from(&source, &index, &検査プールを作る(), &options)
                .expect("材質プールは読める");
        assert_eq!(materials.entries().len(), 1);
        let entry = &materials.entries()[0];
        // スロット名は `(guid, fileID)` の共有名であり、読み手によらない。
        assert_eq!(entry.slot, format!("renderer:{TEST_MAT_GUID}:2100000"));
        assert_eq!(entry.material.name, "肌");
    }

    /// fs版とメモリ版で同じ材質プールが同じ材質名・スロットになる。
    ///
    /// どちらも同じ `.mat` バイト列・同じ索引内容から読む。fs版は
    /// `FsAssetSource` を作る委譲先であり、読み手の違いで結果が変わらない。
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn fs版とメモリ版で同じ材質プールが同じ結果になる() {
        let mat = 検査材質を作る("肌");
        let meta = format!(
            "fileFormatVersion: 2\nguid: {TEST_MAT_GUID}\nNativeFormatImporter:\n  externalObjects: {{}}\n  userData: \n"
        );
        let guard = tempfile::TempDir::new().expect("作業域は作れる");
        let assets = guard.path().join("Assets");
        std::fs::create_dir_all(assets.join("Materials")).expect("親ディレクトリは作れる");
        std::fs::write(assets.join("Materials/Body.mat"), &mat).expect("資産は書ける");
        std::fs::write(assets.join("Materials/Body.mat.meta"), meta.as_bytes())
            .expect("metaは書ける");
        let mut fs_index = AssetIndex::new();
        let fs_meta = AssetMeta::parse(meta.as_bytes()).expect("検査metaは読める");
        fs_index
            .insert("Materials/Body.mat", fs_meta)
            .expect("索引に入る");

        let options = MaterialOptions::new();
        let pool = 検査プールを作る();
        let (from_fs, _) = load_renderer_materials_with_index(&assets, &fs_index, &pool, &options)
            .expect("fs版は読める");
        let memory = MapSource {
            files: HashMap::from([("Materials/Body.mat".to_owned(), mat)]),
        };
        let (from_memory, _) =
            load_renderer_materials_from(&memory, &検査索引を作る(), &pool, &options)
                .expect("メモリ版は読める");
        assert_eq!(from_fs.entries().len(), 1);
        assert_eq!(from_memory.entries().len(), 1);
        assert_eq!(from_fs.entries()[0].slot, from_memory.entries()[0].slot);
        assert_eq!(
            from_fs.entries()[0].material.name,
            from_memory.entries()[0].material.name
        );
        assert_eq!(from_fs.entries()[0].material.name, "肌");
    }
}
