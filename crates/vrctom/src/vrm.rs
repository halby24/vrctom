//! FBX読み取り結果からVRM 1.0へ書き出す前の中間モデル。
//!
//! 入口は [`VrmModel::from_fbx`](crate::vrm::VrmModel::from_fbx) であり、ノード階層・スキン・メッシュ属性・
//! 材質スロット・件数報告を持つ所有データを作る。座標変換や三角形化は行わず、
//! 受け取ったバインド姿勢と幾何を決定的に組み立てるだけである。
//!
//! ```mermaid
//! flowchart LR
//!     fbx[FbxFile<br/>Model・Mesh・Skin・BindPose] --> build[VrmModel::from_fbx]
//!     build --> nodes[ノード階層<br/>ローカル行列]
//!     build --> skins[スキン<br/>関節列と逆バインド行列]
//!     build --> meshes[メッシュ<br/>頂点属性とプリミティブ]
//!     build --> report[VrmReport<br/>件数・切り捨て・非三角形]
//!     nodes --> out[VrmModel]
//!     skins --> out
//!     meshes --> out
//! ```

mod error;
mod export;
mod humanoid;
mod material;
mod matrix;
mod mesh;

pub use error::{VrmError, VrmErrorKind};
pub use export::{VrmExportError, VrmExportErrorKind, VrmExportOptions, VrmMeta};
pub use humanoid::{HumanoidBone, HumanoidBones};
pub use material::{
    MaterialError, MaterialErrorKind, MaterialOptions, MaterialReport, VrmAlphaMode, VrmFilter,
    VrmImage, VrmImageMime, VrmMaterial, VrmMaterialEntry, VrmMaterials, VrmMtoonParameters,
    VrmOutlineWidthMode, VrmSampler, VrmWrap,
};
use matrix::IDENTITY;
use std::collections::{HashMap, HashSet};

use crate::fbx::{Axis, FbxErrorKind, FbxFile, ObjectId};

/// メッシュ数の既定上限（10万）。
///
/// 実データの数千件より十分大きく、破損由来の大量生成の暴走を抑える。
const DEFAULT_MAX_MESHES: usize = 100_000;
/// 頂点数の既定上限（1000万）。
///
/// 実データの数百万頂点が収まる大きさにし、破損由来の膨張を抑える。
const DEFAULT_MAX_VERTICES: usize = 10_000_000;
/// 三角形数の既定上限（5000万）。
///
/// 実データの数百万面が収まる大きさにし、破損由来の膨張を抑える。
const DEFAULT_MAX_TRIANGLES: usize = 50_000_000;

/// VRM書き出し前の中間モデル。
///
/// 読み取り済みの値から作る所有データであり、入力バイト列は保持しない。
#[derive(Debug, Clone)]
pub struct VrmModel {
    /// シーンルートに掛ける行列。単位換算だけを含む。
    pub root: [f64; 16],
    /// ノード列。[`crate::fbx::FbxFile::models`] と同じ順序である。
    pub nodes: Vec<VrmNode>,
    /// スキン列。[`crate::fbx::FbxFile::skins`] と同じ順序である。
    pub skins: Vec<VrmSkin>,
    /// メッシュ列。所有モデルがあるメッシュだけを含み、[`crate::fbx::FbxFile::meshes`] の出現順である。
    pub meshes: Vec<VrmMesh>,
}

/// ノード1件。
///
/// メッシュを持つモデルは親なし・子なしのルートであり、非メッシュの親の
/// `children` にも含まれない。頂点がベイク済みのため親変換の二重適用を
/// 避ける対応であり、`children` の子の `parent` は自分・`parent` ありの
/// ノードは親の `children` に含まれる不変条件を保つ。
#[derive(Debug, Clone)]
pub struct VrmNode {
    /// ノード名。対応するモデルの名前である。
    pub name: String,
    /// 親ノードの索引。親なしの場合は `None` である。
    pub parent: Option<usize>,
    /// 子ノードの索引列。
    pub children: Vec<usize>,
    /// 親から見たローカル行列。
    pub local: [f64; 16],
}

/// スキン1件。
#[derive(Debug, Clone)]
pub struct VrmSkin {
    /// 関節ノードの索引列。読み取り層のクラスタ順を保つ。
    pub joints: Vec<usize>,
    /// 関節のバインド世界行列の逆行列。`joints` と同じ長さである。
    pub inverse_bind_matrices: Vec<[f64; 16]>,
    /// 対応するFBXのモデルID。`joints` と同じ長さである。
    pub joint_models: Vec<ObjectId>,
}

/// メッシュ1件。
#[derive(Debug, Clone)]
pub struct VrmMesh {
    /// 所有モデルのノード索引。頂点はシーン空間へ移してあるためローカル行列は単位行列である。
    pub node: usize,
    /// 変形対象のメッシュID。
    pub geometry: ObjectId,
    /// 所有モデルのID。
    pub model: ObjectId,
    /// メッシュ名。対応するジオメトリの名前である。
    pub name: String,
    /// 対応するスキンの索引。無い場合は `None` である。
    pub skin: Option<usize>,
    /// FBXの材質スロット名。スロットが無い場合は空である。
    pub material_slots: Vec<String>,
    /// メッシュで共有する頂点属性。
    pub vertices: VrmVertices,
    /// 材質番号ごとのプリミティブ。材質番号の昇順である。
    pub primitives: Vec<VrmPrimitive>,
}

/// 頂点属性列。いずれも頂点数と同じ長さである。
#[derive(Debug, Clone, PartialEq)]
pub struct VrmVertices {
    /// 位置。所有モデルのバインド世界行列でシーン空間へ移してある。
    pub positions: Vec<[f32; 3]>,
    /// 法線。全cornerにある場合だけ `Some` である。
    pub normals: Option<Vec<[f32; 3]>>,
    /// UVセット。全セットを出現順に保つ。
    pub uv_sets: Vec<Vec<[f32; 2]>>,
    /// 関節索引。スキンがある場合だけ `Some` である。
    pub joints: Option<Vec<[u16; 4]>>,
    /// 正規化した重み。スキンがある場合だけ `Some` である。
    pub weights: Option<Vec<[f32; 4]>>,
}

/// プリミティブ1件。1つの材質番号に対応する三角形列である。
#[derive(Debug, Clone, PartialEq)]
pub struct VrmPrimitive {
    /// [`VrmMesh::material_slots`] の索引。スロットが無いメッシュは0である。
    pub material: usize,
    /// 三角形の頂点索引列。3件で1三角形である。
    pub indices: Vec<u32>,
}

/// 中間モデル生成の件数報告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VrmReport {
    /// ノード数。
    pub node_count: usize,
    /// スキン数。
    pub skin_count: usize,
    /// 出力したメッシュ数。
    pub mesh_count: usize,
    /// 所有モデルが無く飛ばしたメッシュ数。
    pub skipped_meshes: usize,
    /// 全メッシュのプリミティブ数。
    pub primitive_count: usize,
    /// 統合後の全頂点数。
    pub vertex_count: usize,
    /// 全メッシュの三角形数。
    pub triangle_count: usize,
    /// 4件を超える影響を落とした頂点数。
    pub truncated_influence_vertices: usize,
    /// 重みが無く既定値を与えた頂点数。
    pub unweighted_vertices: usize,
    /// 三角形でない多角形の数。
    pub non_triangle_polygons: usize,
    /// 鏡像で巻き方向を反転したメッシュ数。
    pub mirrored_meshes: usize,
}

/// 中間モデル生成の資源上限。
///
/// フィールドは非公開で、既定値は [`Default`] が設定する。
/// 上限の変更は `with_` で始まるメソッドで行う。
#[derive(Debug, Clone)]
pub struct VrmOptions {
    max_meshes: usize,
    max_vertices: usize,
    max_triangles: usize,
}

impl VrmOptions {
    /// 既定の上限を持つ設定を生成する。
    pub fn new() -> Self {
        Self {
            max_meshes: DEFAULT_MAX_MESHES,
            max_vertices: DEFAULT_MAX_VERTICES,
            max_triangles: DEFAULT_MAX_TRIANGLES,
        }
    }

    /// メッシュ数の上限を変更する。
    pub fn with_max_meshes(mut self, max_meshes: usize) -> Self {
        self.max_meshes = max_meshes;
        self
    }

    /// 頂点数の上限を変更する。
    pub fn with_max_vertices(mut self, max_vertices: usize) -> Self {
        self.max_vertices = max_vertices;
        self
    }

    /// 三角形数の上限を変更する。
    pub fn with_max_triangles(mut self, max_triangles: usize) -> Self {
        self.max_triangles = max_triangles;
        self
    }
}

impl Default for VrmOptions {
    fn default() -> Self {
        Self::new()
    }
}

impl VrmModel {
    /// 読み取り済みFBXから中間モデルを作る。
    ///
    /// 軸系と単位換算を確認し、全モデルのバインド行列からノード・スキン・
    /// メッシュを組み立てる。幾何と重みは対象分だけ展開するため、確保する
    /// メモリは対象の大きさに比例する。
    ///
    /// # Errors
    ///
    /// 標準軸以外の軸系・0以下または非有限の単位換算・バインド行列が無いモデル・
    /// 複数スキンの対応・子を持つメッシュモデル・破損した幾何や重み・
    /// 上限の超過などの場合に返す。
    pub fn from_fbx(
        file: &FbxFile<'_>,
        options: &VrmOptions,
    ) -> Result<(Self, VrmReport), VrmError> {
        check_axis_system(file)?;
        let root = check_unit_scale(file)?;
        let bind = bind_world_matrices(file)?;
        // 関節の集合を先に求め、非関節ルートの正規化判定に使う。
        let joints = joint_model_ids(file);
        let nodes = build_nodes(file, &bind, &joints)?;
        let skins = build_skins(file, &bind)?;
        let (meshes, report) = build_meshes(file, options, &bind)?;
        let model = Self {
            root,
            nodes,
            skins,
            meshes,
        };
        Ok((model, report))
    }
}

/// 軸系がY-up・+Z正面・+X右手であることを確かめる。
///
/// それ以外の軸系は正面や上下の解釈を誤るため、非対応として失敗する。
fn check_axis_system(file: &FbxFile<'_>) -> Result<(), VrmError> {
    let axis = file.axis_system();
    let standard = axis.up == Axis::Y
        && axis.up_sign == 1
        && axis.front == Axis::Z
        && axis.front_sign == 1
        && axis.coord == Axis::X
        && axis.coord_sign == 1;
    if standard {
        Ok(())
    } else {
        Err(VrmError::new(
            VrmErrorKind::UnsupportedAxis,
            "軸系がY-up・+Z正面・+X右手でない",
        ))
    }
}

/// 単位換算係数からシーンルートの拡大行列を作る。
///
/// 係数は `UnitScaleFactor / 100` を拡大率にする。0以下または非有限の
/// 場合は大きさを決められないため、非対応として失敗する。
fn check_unit_scale(file: &FbxFile<'_>) -> Result<[f64; 16], VrmError> {
    let factor = file.unit_scale_factor();
    if factor.is_finite() && factor > 0.0 {
        Ok(matrix::scale_matrix(factor / 100.0))
    } else {
        Err(VrmError::new(
            VrmErrorKind::UnsupportedUnitScale,
            "単位換算係数が正の有限値でない",
        ))
    }
}

/// 全モデルのバインド世界行列をモデルID順ではなく `models()` 順に返す。
///
/// 全モデルにバインド行列が必要であり、無いモデルがある場合は失敗する。
fn bind_world_matrices(file: &FbxFile<'_>) -> Result<Vec<[f64; 16]>, VrmError> {
    let nodes = file.bind_pose_nodes().map_err(|error| {
        // 読み取り層の検査結果を引き継ぎ、構造破損として扱う。
        // 資源上限の超過だけは種別を保つ。
        let message = error.to_string();
        let object = error.object_id();
        let mut out = VrmError::new(
            match error.kind() {
                FbxErrorKind::LimitExceeded => VrmErrorKind::LimitExceeded,
                _ => VrmErrorKind::InvalidStructure,
            },
            message,
        );
        if let Some(id) = object {
            out = out.with_object(id);
        }
        out
    })?;
    let by_model: HashMap<i64, [f64; 16]> = nodes
        .into_iter()
        .map(|node| (node.model.value(), node.matrix))
        .collect();
    file.models()
        .iter()
        .map(|model| {
            by_model.get(&model.id.value()).copied().ok_or_else(|| {
                VrmError::new(
                    VrmErrorKind::MissingBindPose,
                    "バインド行列が無いモデルがある",
                )
                .with_object(model.id)
            })
        })
        .collect()
}

/// 全スキンのクラスタが指す関節モデルのID集合を返す。
fn joint_model_ids(file: &FbxFile<'_>) -> HashSet<i64> {
    file.skins()
        .iter()
        .flat_map(|skin| skin.clusters.iter().map(|cluster| cluster.joint.value()))
        .collect()
}

/// ノード列を作る。`bind` は `models()` 順のバインド世界行列、
/// `joints` は関節モデルのID集合である。
///
/// メッシュを持つモデルは元の親子を使わず、親なし・子なし・単位行列の
/// ルートとして出す。頂点は所有モデルのバインド世界行列でベイク済みの
/// ため、親の `children` に残すと親変換が二重適用される。そこで非メッシュ
/// モデルの `children` からはメッシュを持つモデルを除外し、双方向整合
/// （子の `parent` が自分・親ありの `parent` の `children` に自分）を保つ。
/// 子モデルを持つメッシュモデルは前提が崩れるため失敗する。
///
/// 一般的なインポータはスケルトンルートに単位行列を想定するため、
/// 関節でないルート（FBXの `Armature` など）はバインド世界行列を使わず
/// 単位行列に正規化する。関節のワールド行列はバインド世界行列のままにし、
/// 逆バインド行列との積が単位行列になる関係を保つ。
fn build_nodes(
    file: &FbxFile<'_>,
    bind: &[[f64; 16]],
    joints: &HashSet<i64>,
) -> Result<Vec<VrmNode>, VrmError> {
    let at: HashMap<i64, usize> = file
        .models()
        .iter()
        .enumerate()
        .map(|(index, model)| (model.id.value(), index))
        .collect();
    // メッシュ所有の有無を先に確定させる。親の `children` から除外するためである。
    let has_mesh: Vec<bool> = file
        .models()
        .iter()
        .map(|model| model.mesh.is_some())
        .collect();
    let is_joint: Vec<bool> = file
        .models()
        .iter()
        .map(|model| joints.contains(&model.id.value()))
        .collect();
    let mut parents: Vec<Option<usize>> = Vec::with_capacity(file.models().len());
    let mut children_list: Vec<Vec<usize>> = Vec::with_capacity(file.models().len());
    for model in file.models() {
        let children = model
            .children
            .iter()
            .map(|id| {
                at.get(&id.value()).copied().ok_or_else(|| {
                    VrmError::new(VrmErrorKind::InvalidStructure, "子モデルが一覧にない")
                        .with_object(model.id)
                })
            })
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            // 頂点はベイク済みのため、メッシュ所有モデルは親階層から外す。
            .filter(|child| !has_mesh[*child])
            .collect::<Vec<_>>();
        if model.mesh.is_some() {
            if !model.children.is_empty() {
                return Err(VrmError::new(
                    VrmErrorKind::MeshModelHasChildren,
                    "メッシュを持つモデルに子モデルがある",
                )
                .with_object(model.id));
            }
            parents.push(None);
            children_list.push(Vec::new());
            continue;
        }
        let parent = model
            .parent
            .map(|id| {
                at.get(&id.value()).copied().ok_or_else(|| {
                    VrmError::new(VrmErrorKind::InvalidStructure, "親モデルが一覧にない")
                        .with_object(model.id)
                })
            })
            .transpose()?;
        parents.push(parent);
        children_list.push(children);
    }
    // 出力側の世界行列を根から順に確定させ、ローカル行列を逆算する。
    // 親より子が先に並ぶ場合もあるため、根へ遡る鎖を作ってから根向きに処理する。
    let mut locals: Vec<[f64; 16]> = vec![IDENTITY; file.models().len()];
    let mut worlds: Vec<[f64; 16]> = vec![IDENTITY; file.models().len()];
    let mut done = vec![false; file.models().len()];
    for start in 0..file.models().len() {
        let mut chain = Vec::new();
        // 同一鎖内の多重走査を避けるため、所属判定を O(1) にする。
        // `done` は確定済み節での打ち切り、`seen` は鎖内の循環検出であり、
        // 循環は `InvalidStructure`、完了済み到達は正常打ち切りのまま保つ。
        let mut seen = HashSet::new();
        let mut current = start;
        loop {
            if done[current] {
                break;
            }
            if !seen.insert(current) {
                return Err(VrmError::new(
                    VrmErrorKind::InvalidStructure,
                    "モデルの親子に循環がある",
                )
                .with_object(file.models()[current].id));
            }
            chain.push(current);
            match parents[current] {
                Some(parent) => current = parent,
                None => break,
            }
        }
        for &node in chain.iter().rev() {
            if file.models()[node].mesh.is_some() {
                locals[node] = IDENTITY;
                worlds[node] = IDENTITY;
            } else if let Some(parent) = parents[node] {
                let inverse = matrix::inverse(&worlds[parent]).ok_or_else(|| {
                    VrmError::new(
                        VrmErrorKind::InvalidStructure,
                        "親の出力世界行列が特異である",
                    )
                    .with_object(file.models()[parent].id)
                })?;
                locals[node] = matrix::mul(&inverse, &bind[node]);
                worlds[node] = matrix::mul(&worlds[parent], &locals[node]);
            } else if is_joint[node] {
                locals[node] = bind[node];
                worlds[node] = bind[node];
            } else {
                // 関節でないルートは単位行列に正規化する。
                // 子の世界行列は `単位行列 * バインド世界行列` で保たれる。
                locals[node] = IDENTITY;
                worlds[node] = IDENTITY;
            }
            done[node] = true;
        }
    }
    Ok(file
        .models()
        .iter()
        .enumerate()
        .map(|(index, model)| VrmNode {
            name: model.name.clone(),
            parent: parents[index],
            children: children_list[index].clone(),
            local: locals[index],
        })
        .collect())
}

/// スキン列を作る。`bind` は `models()` 順のバインド世界行列である。
///
/// 関節列は読み取り層のクラスタ順を保ち、逆バインド行列は関節のバインド
/// 世界行列の逆行列である。ルートの単位換算は関節より上に置くため含めない。
fn build_skins(file: &FbxFile<'_>, bind: &[[f64; 16]]) -> Result<Vec<VrmSkin>, VrmError> {
    // 同じジオメトリに複数のスキンが対応すると、どちらを使うか決められない。
    let mut skin_count: HashMap<i64, usize> = HashMap::new();
    for skin in file.skins() {
        *skin_count.entry(skin.geometry.value()).or_default() += 1;
    }
    let mut skins = Vec::with_capacity(file.skins().len());
    for skin in file.skins() {
        if skin_count[&skin.geometry.value()] > 1 {
            return Err(VrmError::new(
                VrmErrorKind::MultipleSkins,
                "1つのメッシュに複数のスキンが対応する",
            )
            .with_object(skin.geometry));
        }
        if file.mesh(skin.geometry).is_none() {
            return Err(VrmError::new(
                VrmErrorKind::InvalidStructure,
                "スキンが参照するメッシュが無い",
            )
            .with_object(skin.id));
        }
        let at: HashMap<i64, usize> = file
            .models()
            .iter()
            .enumerate()
            .map(|(index, model)| (model.id.value(), index))
            .collect();
        let mut joints = Vec::with_capacity(skin.clusters.len());
        let mut inverse_bind_matrices = Vec::with_capacity(skin.clusters.len());
        let mut joint_models = Vec::with_capacity(skin.clusters.len());
        for cluster in &skin.clusters {
            let joint = at.get(&cluster.joint.value()).copied().ok_or_else(|| {
                VrmError::new(VrmErrorKind::InvalidStructure, "結合モデルが一覧にない")
                    .with_object(cluster.id)
            })?;
            // 全モデルにバインド行列があることは呼び出し前に確認済みであり、
            // ここでの欠落は到達しない。逆行列の失敗は特異として扱う。
            let Some(inverse) = matrix::inverse(&bind[joint]) else {
                return Err(VrmError::new(
                    VrmErrorKind::InvalidStructure,
                    "関節のバインド行列が特異である",
                )
                .with_object(cluster.joint));
            };
            joints.push(joint);
            inverse_bind_matrices.push(inverse);
            joint_models.push(cluster.joint);
        }
        skins.push(VrmSkin {
            joints,
            inverse_bind_matrices,
            joint_models,
        });
    }
    Ok(skins)
}

/// メッシュ列と件数報告を作る。
fn build_meshes(
    file: &FbxFile<'_>,
    options: &VrmOptions,
    bind: &[[f64; 16]],
) -> Result<(Vec<VrmMesh>, VrmReport), VrmError> {
    let geometry_skin: HashMap<i64, usize> = file
        .skins()
        .iter()
        .enumerate()
        .map(|(index, skin)| (skin.geometry.value(), index))
        .collect();
    let mut meshes = Vec::new();
    let mut skipped_meshes = 0usize;
    let mut primitive_count = 0usize;
    let mut vertex_count = 0usize;
    let mut triangle_count = 0usize;
    let mut truncated_influence_vertices = 0usize;
    let mut unweighted_vertices = 0usize;
    let mut non_triangle_polygons = 0usize;
    let mut mirrored_meshes = 0usize;
    // 所有モデル探索をメッシュごとの全走査にしないため、モデル列を一度だけ走査する。
    // 1メッシュに複数モデルが対応する破損はここで残し、後段で同じ挙動で失敗する。
    let mut owners_by_mesh: HashMap<i64, Vec<usize>> = HashMap::new();
    for (index, model) in file.models().iter().enumerate() {
        if let Some(mesh_id) = model.mesh {
            owners_by_mesh
                .entry(mesh_id.value())
                .or_default()
                .push(index);
        }
    }
    for mesh in file.meshes() {
        // 所有モデルは `model.mesh` から探す。所有者が無い場合は飛ばす。
        let owner_index = match owners_by_mesh.get(&mesh.id.value()).map(Vec::as_slice) {
            None | Some([]) => {
                skipped_meshes += 1;
                continue;
            }
            Some(&[single]) => single,
            Some(_) => {
                return Err(VrmError::new(
                    VrmErrorKind::InvalidStructure,
                    "1つのメッシュに複数の所有モデルがある",
                )
                .with_object(mesh.id));
            }
        };
        let owner = &file.models()[owner_index];
        if meshes.len() >= options.max_meshes {
            return Err(VrmError::new(
                VrmErrorKind::LimitExceeded,
                "メッシュ数が上限を超えた",
            ));
        }
        let data = file.mesh_data(mesh.id).map_err(|error| {
            let message = error.to_string();
            let object = error.object_id();
            let mut out = VrmError::new(
                match error.kind() {
                    FbxErrorKind::LimitExceeded => VrmErrorKind::LimitExceeded,
                    _ => VrmErrorKind::InvalidStructure,
                },
                message,
            );
            if let Some(id) = object {
                out = out.with_object(id);
            }
            out
        })?;
        let skin = match geometry_skin.get(&mesh.id.value()) {
            Some(&index) => {
                let skin = &file.skins()[index];
                let skin_data = file.skin_data(skin.id).map_err(|error| {
                    let message = error.to_string();
                    let object = error.object_id();
                    let mut out = VrmError::new(
                        match error.kind() {
                            FbxErrorKind::LimitExceeded => VrmErrorKind::LimitExceeded,
                            _ => VrmErrorKind::InvalidStructure,
                        },
                        message,
                    );
                    if let Some(id) = object {
                        out = out.with_object(id);
                    }
                    out
                })?;
                Some((index, skin_data))
            }
            None => None,
        };
        let skin_ref = skin.as_ref().map(|(index, data)| (*index, data));
        let outcome = mesh::convert_mesh(
            file,
            mesh,
            &data,
            owner,
            owner_index,
            &bind[owner_index],
            skin_ref,
        )?;
        vertex_count += outcome.mesh.vertices.positions.len();
        if vertex_count > options.max_vertices {
            return Err(VrmError::new(
                VrmErrorKind::LimitExceeded,
                "頂点数が上限を超えた",
            ));
        }
        triangle_count += outcome.triangles;
        if triangle_count > options.max_triangles {
            return Err(VrmError::new(
                VrmErrorKind::LimitExceeded,
                "三角形数が上限を超えた",
            ));
        }
        primitive_count += outcome.mesh.primitives.len();
        truncated_influence_vertices += outcome.truncated;
        unweighted_vertices += outcome.unweighted;
        non_triangle_polygons += outcome.non_triangles;
        if outcome.mirrored {
            mirrored_meshes += 1;
        }
        meshes.push(outcome.mesh);
    }
    let report = VrmReport {
        node_count: file.models().len(),
        skin_count: file.skins().len(),
        mesh_count: meshes.len(),
        skipped_meshes,
        primitive_count,
        vertex_count,
        triangle_count,
        truncated_influence_vertices,
        unweighted_vertices,
        non_triangle_polygons,
        mirrored_meshes,
    };
    Ok((meshes, report))
}
