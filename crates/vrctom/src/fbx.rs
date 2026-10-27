//! バイナリFBXからメッシュ幾何とモデル階層を読み取る層。
//!
//! 担当するのは、展開済みバイト列の借用による一度の走査と、メッシュ単位の
//! オンデマンド展開までである。VRM生成・三角形化・座標系変換・スキニングの
//! 解決は行わない。
//!
//! 入口は [`FbxFile::parse`](crate::fbx::FbxFile::parse)（走査）、
//! [`FbxFile::mesh_data`](crate::fbx::FbxFile::mesh_data)（対象配列の展開）
//! である。座標値と変換値はファイルの数値のまま返し、軸や単位は値として返す。
//!
//! ```mermaid
//! flowchart LR
//!     bytes[入力バイト列<br/>借用] --> header[ヘッダ検証<br/>magicとversion]
//!     header --> walk[ノード走査<br/>深さ・件数・境界の検査]
//!     walk --> objects[Objects<br/>ModelとGeometry Mesh]
//!     walk --> conn[Connections<br/>OOの子→親]
//!     objects --> file[FbxFile<br/>階層・メッシュ要約・配列位置]
//!     conn --> file
//!     file --> decode[mesh_data<br/>対象配列だけzlib展開]
//!     decode --> data[MeshData<br/>座標・面・法線・UV・材質番号]
//! ```
//!
//! 配列は解析時には展開せず、絶対位置・要素数・要素型・圧縮方式だけを保持する。
//! `mesh_data` は要求されたメッシュの配列だけを境界検査付きで展開するため、
//! 確保するメモリは対象メッシュの大きさに比例し、ファイル全体には比例しない。

mod error;
mod mesh;
mod reader;

pub use error::{FbxError, FbxErrorKind};
use mesh::{LayerDesc, Mapping, Reference, expand};
use reader::{ArrayDesc, ArrayKind, RawNode, RawProp};
use std::collections::{HashMap, HashSet};

/// ファイルサイズの既定上限（1GiB）。
///
/// メモリ常駐の入力全体を抑えるための歯止めである。通常のFBXより十分大きい。
const DEFAULT_MAX_FILE_BYTES: usize = 1 << 30;
/// ノード数の既定上限（100万）。
///
/// 子孫をすべて保持する走査の計算量を抑えるための歯止めである。
/// 実データの数千節点は十分通る。
const DEFAULT_MAX_NODES: usize = 1_000_000;
/// ネスト深さの既定上限（256）。
///
/// 再帰走査の深さを抑えるための歯止めである。通常の階層より十分深い。
const DEFAULT_MAX_DEPTH: usize = 256;
/// 文字列長の既定上限（1MiB）。
///
/// 名前や対応方式の表記は短いため、長い文字列は破損として抑える。
const DEFAULT_MAX_STRING_BYTES: usize = 1 << 20;
/// 配列1件の要素数の既定上限（64Mi）。
///
/// 1メッシュの頂点・索引が収まる大きさにし、破損した要素数の暴走を抑える。
const DEFAULT_MAX_ARRAY_ELEMENTS: usize = 64 << 20;
/// 配列要素数の合計の既定上限（512Mi）。
///
/// 全メッシュの幾何が収まる大きさにし、多数の配列の積み上げを抑える。
const DEFAULT_MAX_TOTAL_ARRAY_ELEMENTS: usize = 512 << 20;

/// FBX内のオブジェクトID。
///
/// `Model` と `Geometry` の `Connections` 対応に使う。値はファイル由来の
/// 64bit整数であり、0は親なし（root直下）を表す接続専用の番兵である。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ObjectId(i64);

impl ObjectId {
    /// ID値から生成する。
    ///
    /// 照会（[`FbxFile::model`] など）の鍵として使うため、公開する。
    pub fn new(id: i64) -> Self {
        Self(id)
    }

    /// ID値を返す。
    pub fn value(self) -> i64 {
        self.0
    }
}

/// FBX読み取りの資源上限。
///
/// フィールドは非公開で、既定値は [`Default`] が設定する。
/// 上限の変更は `with_` で始まるメソッドで行う。
#[derive(Debug, Clone)]
pub struct FbxOptions {
    max_file_bytes: usize,
    max_nodes: usize,
    max_depth: usize,
    max_string_bytes: usize,
    max_array_elements: usize,
    max_total_array_elements: usize,
}

impl FbxOptions {
    /// 既定の上限を持つ設定を生成する。
    pub fn new() -> Self {
        Self {
            max_file_bytes: DEFAULT_MAX_FILE_BYTES,
            max_nodes: DEFAULT_MAX_NODES,
            max_depth: DEFAULT_MAX_DEPTH,
            max_string_bytes: DEFAULT_MAX_STRING_BYTES,
            max_array_elements: DEFAULT_MAX_ARRAY_ELEMENTS,
            max_total_array_elements: DEFAULT_MAX_TOTAL_ARRAY_ELEMENTS,
        }
    }

    /// ファイルサイズの上限（バイト数）を変更する。
    pub fn with_max_file_bytes(mut self, max_file_bytes: usize) -> Self {
        self.max_file_bytes = max_file_bytes;
        self
    }

    /// ノード数の上限を変更する。
    pub fn with_max_nodes(mut self, max_nodes: usize) -> Self {
        self.max_nodes = max_nodes;
        self
    }

    /// ネスト深さの上限を変更する。
    pub fn with_max_depth(mut self, max_depth: usize) -> Self {
        self.max_depth = max_depth;
        self
    }

    /// 文字列長の上限（バイト数）を変更する。
    pub fn with_max_string_bytes(mut self, max_string_bytes: usize) -> Self {
        self.max_string_bytes = max_string_bytes;
        self
    }

    /// 配列1件の要素数の上限を変更する。
    pub fn with_max_array_elements(mut self, max_array_elements: usize) -> Self {
        self.max_array_elements = max_array_elements;
        self
    }

    /// 配列要素数の合計の上限を変更する。
    pub fn with_max_total_array_elements(mut self, max_total_array_elements: usize) -> Self {
        self.max_total_array_elements = max_total_array_elements;
        self
    }
}

impl Default for FbxOptions {
    fn default() -> Self {
        Self::new()
    }
}

/// 座標軸の向き。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    /// X軸。
    X,
    /// Y軸。
    Y,
    /// Z軸。
    Z,
}

/// ファイルの軸系と符号。
///
/// 変換は行わず、値として返す。軸の解釈と変換は生成側が担う。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AxisSystem {
    /// 上方向の軸。
    pub up: Axis,
    /// 上方向の符号。
    pub up_sign: i8,
    /// 前方向の軸。
    pub front: Axis,
    /// 前方向の符号。
    pub front_sign: i8,
    /// 横方向の軸。
    pub coord: Axis,
    /// 横方向の符号。
    pub coord_sign: i8,
}

/// モデルの種別。
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelKind {
    /// 幾何を持つ節点。
    Mesh,
    /// 骨節点。
    LimbNode,
    /// 空節点。
    Null,
    /// カメラ節点。
    Camera,
    /// 光源節点。
    Light,
    /// 四肢節点。
    Limb,
    /// 上記以外の種別。内部の文字列を保つ。
    Other(Box<str>),
}

/// モデルのローカル変換。
///
/// 回転はXYZ順の度数法のまま保持し、合成や変換はしない。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LocalTransform {
    /// 平行移動。
    pub translation: [f64; 3],
    /// 回転（XYZ順の度）。
    pub rotation: [f64; 3],
    /// 拡大縮小。
    pub scale: [f64; 3],
}

/// モデル1件。
#[derive(Debug, Clone)]
pub struct Model {
    /// オブジェクトID。
    pub id: ObjectId,
    /// モデル名。`Model\x00Class` 形式の場合は最初の `\x00` で切る。
    pub name: String,
    /// 種別。
    pub kind: ModelKind,
    /// 親モデル。親なしの場合は `None` である。
    pub parent: Option<ObjectId>,
    /// 子モデル。`Connections` の出現順を保つ。
    pub children: Vec<ObjectId>,
    /// 所属するメッシュ（`Geometry`）。持たない場合は `None` である。
    pub mesh: Option<ObjectId>,
    /// ローカル変換。未指定の成分は既定値（移動0・回転0・倍率1）である。
    pub transform: LocalTransform,
    /// 継承方式。未指定の場合は1である。合成規則の解釈は生成側に残す。
    pub inherit_type: i32,
}

/// メッシュ1件の要約。
///
/// 頂点数・面数などの件数と属性層の有無を持ち、配列の中身は持たない。
/// 配列の位置情報は非公開で保持し、[`FbxFile::mesh_data`] が展開に使う。
#[derive(Debug, Clone)]
pub struct Mesh {
    /// オブジェクトID。
    pub id: ObjectId,
    /// メッシュ名。`Name\x00Class` 形式の場合は最初の `\x00` で切る。
    pub name: String,
    /// 頂点数。
    pub vertex_count: usize,
    /// 多角形数。
    pub polygon_count: usize,
    /// corner（多角形頂点）数。
    pub corner_count: usize,
    /// UVセット数。`LayerElementUV` の出現順に数える。
    pub uv_set_count: usize,
    /// 法線層の有無。
    pub has_normals: bool,
    /// 材質層の有無。
    pub has_materials: bool,
    /// 頂点配列の位置。
    pub(crate) vertices: ArrayDesc,
    /// 多角形索引配列の位置。
    pub(crate) polygon_vertex_index: ArrayDesc,
    /// 先頭の法線層。無い場合は `None` である。
    pub(crate) normals: Option<LayerDesc>,
    /// UV層。出現順を保つ。
    pub(crate) uv_layers: Vec<LayerDesc>,
    /// 先頭の材質層。無い場合は `None` である。
    pub(crate) materials: Option<LayerDesc>,
}

/// 法線層の展開結果。
#[derive(Debug, Clone, PartialEq)]
pub struct NormalLayer {
    /// 法線値。3値で1件である。
    pub values: Vec<[f64; 3]>,
    /// cornerごとの値の索引。長さはcorner数と一致する。
    pub corner_indices: Vec<u32>,
}

/// UV層の展開結果。
#[derive(Debug, Clone, PartialEq)]
pub struct UvSet {
    /// UVセット名。
    pub name: String,
    /// UV値。2値で1件である。
    pub values: Vec<[f64; 2]>,
    /// cornerごとの値の索引。長さはcorner数と一致する。
    pub corner_indices: Vec<u32>,
}

/// メッシュ1件の幾何の展開結果。
///
/// 多角形のまま返し、三角形化は行わない。fan三角形化は凹多角形で誤った面を
/// 作り、耳切り法は読み取り層に収まらない判断を増やすためである。
#[derive(Debug, Clone, PartialEq)]
pub struct MeshData {
    /// 頂点座標。3値で1頂点である。
    pub positions: Vec<[f64; 3]>,
    /// 面ごとのcorner数。長さは多角形数と一致する。
    pub polygon_corner_counts: Vec<u32>,
    /// 多角形を連結したcorner列。長さはcorner数と一致する。
    pub corner_positions: Vec<u32>,
    /// 法線層。無い場合は `None` である。
    pub normals: Option<NormalLayer>,
    /// UV層。出現順を保つ。
    pub uv_sets: Vec<UvSet>,
    /// 面ごとの材質番号。長さは多角形数と一致する。無い場合は空である。
    pub material_indices: Vec<u32>,
}

/// 読み取り済みのFBX。
///
/// 入力バイト列を借用し、階層・要約・配列位置だけを保持する。
/// 幾何の中身は [`mesh_data`](Self::mesh_data) が対象分だけ展開する。
#[derive(Debug)]
pub struct FbxFile<'a> {
    bytes: &'a [u8],
    version: u32,
    axis_system: AxisSystem,
    unit_scale_factor: f64,
    models: Vec<Model>,
    model_at: HashMap<i64, usize>,
    meshes: Vec<Mesh>,
    mesh_at: HashMap<i64, usize>,
}

impl<'a> FbxFile<'a> {
    /// 入力バイト列を読み取る。
    ///
    /// 走査は一度だけ行い、配列の中身は展開しない。ファイル全体の幾何データを
    /// 常駐させないためである。
    ///
    /// # Errors
    ///
    /// ヘッダ不良・非対応版・境界破損・上限超過・階層不整合などの場合に返す。
    pub fn parse(bytes: &'a [u8], options: &FbxOptions) -> Result<Self, FbxError> {
        let (version, top) = reader::parse_top(bytes, options)?;
        check_encryption(&top)?;
        let axis_system = read_axis_system(&top)?;
        let unit_scale_factor = read_unit_scale(&top)?;
        let mut models = read_models(&top)?;
        let mut meshes = read_meshes(bytes, &top)?;
        // FBXのオブジェクトIDは全体で一意であり、ModelとGeometryの衝突は
        // `apply_connections` の分類を曖昧にするため、接続解決の前に破損として拒否する。
        let model_ids: HashSet<i64> = models.iter().map(|model| model.id.value()).collect();
        for mesh in &meshes {
            if model_ids.contains(&mesh.id.value()) {
                return Err(FbxError::new(
                    FbxErrorKind::InvalidStructure,
                    "ModelとGeometryのIDが重複している",
                )
                .with_object(mesh.id));
            }
        }
        apply_connections(&top, &mut models, &mut meshes)?;
        let model_at = models
            .iter()
            .enumerate()
            .map(|(index, model)| (model.id.value(), index))
            .collect();
        let mesh_at = meshes
            .iter()
            .enumerate()
            .map(|(index, mesh)| (mesh.id.value(), index))
            .collect();
        Ok(Self {
            bytes,
            version,
            axis_system,
            unit_scale_factor,
            models,
            model_at,
            meshes,
            mesh_at,
        })
    }

    /// FBXの版を返す。
    pub fn version(&self) -> u32 {
        self.version
    }

    /// ファイルの軸系を返す。
    pub fn axis_system(&self) -> AxisSystem {
        self.axis_system
    }

    /// 単位換算係数を返す。
    pub fn unit_scale_factor(&self) -> f64 {
        self.unit_scale_factor
    }

    /// モデルの一覧を返す。順序は `Objects` の出現順である。
    pub fn models(&self) -> &[Model] {
        &self.models
    }

    /// 指定IDのモデルを返す。
    pub fn model(&self, id: ObjectId) -> Option<&Model> {
        self.model_at
            .get(&id.value())
            .map(|&index| &self.models[index])
    }

    /// 親なしのモデルを `Objects` の出現順に返す。
    pub fn root_models(&self) -> impl Iterator<Item = &Model> {
        self.models.iter().filter(|model| model.parent.is_none())
    }

    /// メッシュの一覧を返す。順序は `Objects` の出現順である。
    pub fn meshes(&self) -> &[Mesh] {
        &self.meshes
    }

    /// 指定IDのメッシュを返す。
    pub fn mesh(&self, id: ObjectId) -> Option<&Mesh> {
        self.mesh_at
            .get(&id.value())
            .map(|&index| &self.meshes[index])
    }

    /// 指定IDのメッシュ幾何を展開する。
    ///
    /// 対象メッシュの配列だけを境界検査付きで展開するため、確保するメモリは
    /// 対象メッシュの大きさに比例する。
    ///
    /// # Errors
    ///
    /// 未知ID・索引範囲外・属性長不一致・非有限値などの場合に返す。
    pub fn mesh_data(&self, id: ObjectId) -> Result<MeshData, FbxError> {
        let mesh = self.mesh(id).ok_or_else(|| {
            FbxError::new(FbxErrorKind::NotFound, "メッシュがない").with_object(id)
        })?;
        let positions = read_positions(self.bytes, mesh)?;
        let (polygon_corner_counts, corner_positions) =
            read_polygons(self.bytes, mesh, positions.len())?;
        let polygon_count = polygon_corner_counts.len();
        let shape = AttrShape {
            corners: &corner_positions,
            face_counts: &polygon_corner_counts,
            vertex_count: positions.len(),
        };
        let normals = match &mesh.normals {
            Some(layer) => Some(read_normals(self.bytes, mesh.id, layer, &shape)?),
            None => None,
        };
        let mut uv_sets = Vec::with_capacity(mesh.uv_layers.len());
        for layer in &mesh.uv_layers {
            uv_sets.push(read_uvs(self.bytes, mesh.id, layer, &shape)?);
        }
        let material_indices = match &mesh.materials {
            Some(layer) => read_materials(self.bytes, mesh, layer, polygon_count)?,
            None => Vec::new(),
        };
        Ok(MeshData {
            positions,
            polygon_corner_counts,
            corner_positions,
            normals,
            uv_sets,
            material_indices,
        })
    }
}

/// `FBXHeaderExtension/EncryptionType` が0以外の場合に拒否する。
///
/// 0以外は暗号化されており、復号はこの層の責務に含めない。
fn check_encryption(top: &[RawNode]) -> Result<(), FbxError> {
    let Some(extension) = top
        .iter()
        .find(|node| node.name.as_ref() == "FBXHeaderExtension")
    else {
        return Ok(());
    };
    let Some(node) = extension.child("EncryptionType") else {
        return Ok(());
    };
    let encrypted = node
        .props
        .first()
        .and_then(RawProp::as_int)
        .ok_or_else(|| {
            FbxError::new(
                FbxErrorKind::InvalidStructure,
                "EncryptionTypeの形状が壊れている",
            )
        })?;
    if encrypted != 0 {
        return Err(FbxError::new(
            FbxErrorKind::UnsupportedFeature,
            "暗号化されたFBXである",
        ));
    }
    Ok(())
}

/// `GlobalSettings/Properties70` から軸系を読む。無い場合はY-upを返す。
fn read_axis_system(top: &[RawNode]) -> Result<AxisSystem, FbxError> {
    // 既定はY-up（X=0・Y=1・Z=2、符号はすべて1）である。
    let mut axis = AxisSystem {
        up: Axis::Y,
        up_sign: 1,
        front: Axis::Z,
        front_sign: 1,
        coord: Axis::X,
        coord_sign: 1,
    };
    let Some(settings) = top
        .iter()
        .find(|node| node.name.as_ref() == "GlobalSettings")
    else {
        return Ok(axis);
    };
    let Some(props) = settings.child("Properties70") else {
        return Ok(axis);
    };
    for node in props.children_by("P") {
        let name = node.props.first().and_then(RawProp::as_bytes);
        let value = node.props.get(4).and_then(RawProp::as_int);
        let (Some(name), Some(value)) = (name, value) else {
            continue;
        };
        match name {
            b"UpAxis" => axis.up = parse_axis(value)?,
            b"UpAxisSign" => axis.up_sign = parse_sign(value)?,
            b"FrontAxis" => axis.front = parse_axis(value)?,
            b"FrontAxisSign" => axis.front_sign = parse_sign(value)?,
            b"CoordAxis" => axis.coord = parse_axis(value)?,
            b"CoordAxisSign" => axis.coord_sign = parse_sign(value)?,
            _ => {}
        }
    }
    Ok(axis)
}

/// 軸番号（0=X・1=Y・2=Z）を求める。範囲外は破損として失敗する。
fn parse_axis(value: i64) -> Result<Axis, FbxError> {
    match value {
        0 => Ok(Axis::X),
        1 => Ok(Axis::Y),
        2 => Ok(Axis::Z),
        _ => Err(FbxError::new(
            FbxErrorKind::InvalidStructure,
            "軸番号が範囲外である",
        )),
    }
}

/// 軸符号を `i8` で求める。範囲外は破損として失敗する。
fn parse_sign(value: i64) -> Result<i8, FbxError> {
    i8::try_from(value)
        .map_err(|_| FbxError::new(FbxErrorKind::InvalidStructure, "軸符号が範囲外である"))
}

/// `GlobalSettings/Properties70` から単位換算係数を読む。無い場合は1を返す。
fn read_unit_scale(top: &[RawNode]) -> Result<f64, FbxError> {
    let Some(settings) = top
        .iter()
        .find(|node| node.name.as_ref() == "GlobalSettings")
    else {
        return Ok(1.0);
    };
    let Some(props) = settings.child("Properties70") else {
        return Ok(1.0);
    };
    for node in props.children_by("P") {
        if node.props.first().and_then(RawProp::as_bytes) != Some(b"UnitScaleFactor".as_slice()) {
            continue;
        }
        let value = node
            .props
            .get(4)
            .and_then(RawProp::as_number)
            .ok_or_else(|| {
                FbxError::new(
                    FbxErrorKind::InvalidStructure,
                    "UnitScaleFactorの形状が壊れている",
                )
            })?;
        if !value.is_finite() {
            return Err(FbxError::new(
                FbxErrorKind::InvalidStructure,
                "UnitScaleFactorが有限でない",
            ));
        }
        return Ok(value);
    }
    Ok(1.0)
}

/// `Objects` 直下の `Model` を読む。
///
/// IDが0以下や重複の場合は破損として失敗する。後勝ちで黙って上書きすると
/// 階層の対応付けがずれるためである。
fn read_models(top: &[RawNode]) -> Result<Vec<Model>, FbxError> {
    let mut models = Vec::new();
    let mut seen: HashSet<i64> = HashSet::new();
    let Some(objects) = top.iter().find(|node| node.name.as_ref() == "Objects") else {
        return Ok(models);
    };
    for node in objects.children_by("Model") {
        let id = node
            .props
            .first()
            .and_then(RawProp::as_int)
            .ok_or_else(|| {
                FbxError::new(FbxErrorKind::InvalidStructure, "ModelのID形状が壊れている")
            })?;
        if id <= 0 {
            return Err(
                FbxError::new(FbxErrorKind::InvalidStructure, "ModelのIDが0以下である")
                    .with_object(ObjectId(id)),
            );
        }
        if !seen.insert(id) {
            return Err(
                FbxError::new(FbxErrorKind::InvalidStructure, "ModelのIDが重複している")
                    .with_object(ObjectId(id)),
            );
        }
        let object = ObjectId(id);
        let name = split_class_name(node.props.get(1).and_then(RawProp::as_bytes), object)?;
        let subtype = split_class_name(node.props.get(2).and_then(RawProp::as_bytes), object)?;
        let kind = match subtype.as_str() {
            "Mesh" => ModelKind::Mesh,
            "LimbNode" => ModelKind::LimbNode,
            "Null" => ModelKind::Null,
            "Camera" => ModelKind::Camera,
            "Light" => ModelKind::Light,
            "Limb" => ModelKind::Limb,
            other => ModelKind::Other(other.into()),
        };
        let (transform, inherit_type) = read_local(node)?;
        models.push(Model {
            id: ObjectId(id),
            name,
            kind,
            parent: None,
            children: Vec::new(),
            mesh: None,
            transform,
            inherit_type,
        });
    }
    Ok(models)
}

/// `Model` の子 `Properties70` からローカル変換を読む。
///
/// `Lcl Translation`・`Lcl Rotation`・`Lcl Scaling` と `InheritType` だけを
/// 受け付ける。TRS以外の変換成分は黙って落とすと誤った姿勢を返すため、
/// 明示的な非対応として失敗する。
fn read_local(node: &RawNode) -> Result<(LocalTransform, i32), FbxError> {
    let id = node.props.first().and_then(RawProp::as_int).unwrap_or(0);
    let object = ObjectId(id);
    let mut transform = LocalTransform {
        translation: [0.0, 0.0, 0.0],
        rotation: [0.0, 0.0, 0.0],
        scale: [1.0, 1.0, 1.0],
    };
    let mut inherit_type = 1;
    let Some(props) = node.child("Properties70") else {
        return Ok((transform, inherit_type));
    };
    // 値の黙殺を避けるため、非対応成分は列挙で拒否する。
    // `RotationOrder` だけは0（XYZ順）の場合に限り、既定の解釈と一致するため通す。
    const UNSUPPORTED: &[&[u8]] = &[
        b"PreRotation",
        b"RotationPivot",
        b"PostRotation",
        b"ScalingPivot",
        b"ScalingOffset",
        b"GeometricTranslation",
        b"GeometricRotation",
        b"GeometricScaling",
    ];
    for node in props.children_by("P") {
        let Some(name) = node.props.first().and_then(RawProp::as_bytes) else {
            continue;
        };
        if UNSUPPORTED.contains(&name) {
            return Err(
                FbxError::new(FbxErrorKind::UnsupportedFeature, "非対応の変換成分がある")
                    .with_object(object),
            );
        }
        match name {
            b"Lcl Translation" => transform.translation = read_triplet(node, object)?,
            b"Lcl Rotation" => transform.rotation = read_triplet(node, object)?,
            b"Lcl Scaling" => transform.scale = read_triplet(node, object)?,
            b"InheritType" => {
                inherit_type = node
                    .props
                    .get(4)
                    .and_then(RawProp::as_int)
                    .and_then(|v| i32::try_from(v).ok())
                    .ok_or_else(|| {
                        FbxError::new(
                            FbxErrorKind::InvalidStructure,
                            "InheritTypeの形状が壊れている",
                        )
                        .with_object(object)
                    })?;
            }
            b"RotationOrder" => {
                let order = node.props.get(4).and_then(RawProp::as_int).ok_or_else(|| {
                    FbxError::new(
                        FbxErrorKind::InvalidStructure,
                        "RotationOrderの形状が壊れている",
                    )
                    .with_object(object)
                })?;
                if order != 0 {
                    return Err(FbxError::new(
                        FbxErrorKind::UnsupportedFeature,
                        "非対応の回転順序である",
                    )
                    .with_object(object));
                }
            }
            _ => {}
        }
    }
    if !transform
        .translation
        .iter()
        .chain(&transform.rotation)
        .chain(&transform.scale)
        .all(|v| v.is_finite())
    {
        return Err(
            FbxError::new(FbxErrorKind::InvalidStructure, "有限でない変換値がある")
                .with_object(object),
        );
    }
    Ok((transform, inherit_type))
}

/// `P` の `[4..]` から3数値を読む。個数違いは破損として失敗する。
fn read_triplet(node: &RawNode, object: ObjectId) -> Result<[f64; 3], FbxError> {
    let values: Vec<Option<f64>> = node
        .props
        .get(4..)
        .map(|props| props.iter().map(RawProp::as_number).collect())
        .unwrap_or_default();
    match values.as_slice() {
        [Some(x), Some(y), Some(z)] => Ok([*x, *y, *z]),
        _ => Err(
            FbxError::new(FbxErrorKind::InvalidStructure, "変換値の形状が壊れている")
                .with_object(object),
        ),
    }
}

/// `Name\x00Class` 形式の先頭部分を取り出す。
///
/// FBXの名前は `\x00` 以降にクラス名が付くため、表示名は手前だけを使う。
/// バイト列がUTF-8でない場合は破損として失敗する。損失置換すると
/// 別名の衝突や表示の不一致を招くためである。
fn split_class_name(raw: Option<&[u8]>, object: ObjectId) -> Result<String, FbxError> {
    let bytes = raw.unwrap_or_default();
    let head = bytes.split(|b| *b == 0).next().unwrap_or_default();
    std::str::from_utf8(head).map(str::to_owned).map_err(|_| {
        FbxError::new(FbxErrorKind::InvalidStructure, "名前がUTF-8でない").with_object(object)
    })
}

/// `Objects` 直下の `Geometry`（`Mesh`）を読む。
///
/// IDが0以下や重複の場合は破損として失敗する。後勝ちで黙って上書きすると
/// 所属の対応付けがずれるためである。
fn read_meshes(bytes: &[u8], top: &[RawNode]) -> Result<Vec<Mesh>, FbxError> {
    let mut meshes = Vec::new();
    let mut seen: HashSet<i64> = HashSet::new();
    let Some(objects) = top.iter().find(|node| node.name.as_ref() == "Objects") else {
        return Ok(meshes);
    };
    for node in objects.children_by("Geometry") {
        // `Mesh` 以外は読まない。比較はバイト列で行い、名前のUTF-8検証は後段で行う。
        let is_mesh = node
            .props
            .get(2)
            .and_then(RawProp::as_bytes)
            .map(|bytes| bytes.split(|b| *b == 0).next() == Some(b"Mesh".as_slice()))
            .unwrap_or(false);
        if !is_mesh {
            continue;
        }
        let id = node
            .props
            .first()
            .and_then(RawProp::as_int)
            .ok_or_else(|| {
                FbxError::new(
                    FbxErrorKind::InvalidStructure,
                    "GeometryのID形状が壊れている",
                )
            })?;
        if id <= 0 {
            return Err(
                FbxError::new(FbxErrorKind::InvalidStructure, "GeometryのIDが0以下である")
                    .with_object(ObjectId(id)),
            );
        }
        if !seen.insert(id) {
            return Err(FbxError::new(
                FbxErrorKind::InvalidStructure,
                "GeometryのIDが重複している",
            )
            .with_object(ObjectId(id)));
        }
        let object = ObjectId(id);
        let name = split_class_name(node.props.get(1).and_then(RawProp::as_bytes), object)?;
        let vertices = child_array(node, "Vertices", object)?;
        let polygon_vertex_index = child_array(node, "PolygonVertexIndex", object)?;
        if vertices.count % 3 != 0 {
            return Err(
                FbxError::new(FbxErrorKind::InvalidStructure, "頂点配列が3の倍数でない")
                    .with_object(object),
            );
        }
        let vertex_count = vertices.count / 3;
        // 面数は要約に必要なため、索引だけはここで展開して数える。
        // 中身は残さず、位置情報から求め直すため常駐量は増えない。
        let (polygon_count, corner_count) = count_polygons(bytes, polygon_vertex_index, object)?;
        let normals = node
            .children_by("LayerElementNormal")
            .next()
            .map(|layer| read_normal_layer(layer, object))
            .transpose()?;
        let mut uv_layers = Vec::new();
        for layer in node.children_by("LayerElementUV") {
            uv_layers.push(read_uv_layer(layer, object)?);
        }
        let materials = node
            .children_by("LayerElementMaterial")
            .next()
            .map(|layer| read_material_layer(layer, object))
            .transpose()?;
        meshes.push(Mesh {
            id: object,
            name,
            vertex_count,
            polygon_count,
            corner_count,
            uv_set_count: uv_layers.len(),
            has_normals: normals.is_some(),
            has_materials: materials.is_some(),
            vertices,
            polygon_vertex_index,
            normals,
            uv_layers,
            materials,
        });
    }
    return Ok(meshes);

    /// 指定名の子の先頭プロパティから配列位置を取り出す。
    fn child_array(node: &RawNode, name: &str, object: ObjectId) -> Result<ArrayDesc, FbxError> {
        node.child(name)
            .and_then(|child| child.props.first())
            .and_then(RawProp::as_array)
            .ok_or_else(|| {
                FbxError::new(FbxErrorKind::InvalidStructure, "幾何配列がない").with_object(object)
            })
    }
}

/// 索引配列を展開し、多角形数とcorner数を数える。
///
/// 負値が多角形の終端を表し、絶対値は `!v` で復元する。
/// 終端のない連続や3未満の多角形は不正として失敗する。
fn count_polygons(
    bytes: &[u8],
    desc: ArrayDesc,
    object: ObjectId,
) -> Result<(usize, usize), FbxError> {
    let indices = expand_integers(bytes, desc, object)?;
    let mut polygons: usize = 0;
    let mut corners: usize = 0;
    let mut open: usize = 0;
    for &v in &indices {
        if v >= 0 {
            open = open.saturating_add(1);
        } else {
            open = open.saturating_add(1);
            if open < 3 {
                return Err(
                    FbxError::new(FbxErrorKind::InvalidStructure, "3未満の多角形がある")
                        .with_object(object),
                );
            }
            polygons = polygons.saturating_add(1);
            corners = corners.saturating_add(open);
            open = 0;
        }
    }
    if open != 0 {
        return Err(
            FbxError::new(FbxErrorKind::InvalidStructure, "多角形の終端がない").with_object(object),
        );
    }
    Ok((polygons, corners))
}

/// 先頭の `LayerElementNormal` を読む。
fn read_normal_layer(layer: &RawNode, object: ObjectId) -> Result<LayerDesc, FbxError> {
    read_layer(layer, object, "Normals", Some("NormalsIndex"), None)
}

/// `LayerElementUV` を読む。`Name` の子からセット名を取る。
/// セット名がUTF-8でない場合は破損として失敗する。
fn read_uv_layer(layer: &RawNode, object: ObjectId) -> Result<LayerDesc, FbxError> {
    let name = match layer
        .child("Name")
        .and_then(|child| child.props.first())
        .and_then(RawProp::as_bytes)
    {
        Some(bytes) => std::str::from_utf8(bytes)
            .map_err(|_| {
                FbxError::new(FbxErrorKind::InvalidStructure, "UVセット名がUTF-8でない")
                    .with_object(object)
            })?
            .to_owned()
            .into_boxed_str(),
        None => Box::default(),
    };
    read_layer(layer, object, "UV", Some("UVIndex"), Some(name))
}

/// 先頭の `LayerElementMaterial` を読む。
///
/// 材質の `Materials` は値そのものであり、独立した索引ノードを持たない。
/// 参照方式によらず直接読む。
fn read_material_layer(layer: &RawNode, object: ObjectId) -> Result<LayerDesc, FbxError> {
    read_layer(layer, object, "Materials", None, None)
}

/// 属性層の対応方式・参照方式・配列位置を読む。
///
/// 索引名を渡さない層（材質）は索引を持たず、参照方式によらず値から読む。
fn read_layer(
    layer: &RawNode,
    object: ObjectId,
    data_name: &str,
    index_name: Option<&str>,
    name: Option<Box<str>>,
) -> Result<LayerDesc, FbxError> {
    let mapping = layer
        .child("MappingInformationType")
        .and_then(|child| child.props.first())
        .and_then(RawProp::as_bytes)
        .ok_or_else(|| {
            FbxError::new(FbxErrorKind::InvalidStructure, "Mappingがない").with_object(object)
        })
        .and_then(Mapping::parse)
        .map_err(|error| error.with_object(object))?;
    let reference = layer
        .child("ReferenceInformationType")
        .and_then(|child| child.props.first())
        .and_then(RawProp::as_bytes)
        .ok_or_else(|| {
            FbxError::new(FbxErrorKind::InvalidStructure, "Referenceがない").with_object(object)
        })
        .and_then(Reference::parse)
        .map_err(|error| error.with_object(object))?;
    let data = layer
        .child(data_name)
        .and_then(|child| child.props.first())
        .and_then(RawProp::as_array)
        .ok_or_else(|| {
            FbxError::new(FbxErrorKind::InvalidStructure, "属性配列がない").with_object(object)
        })?;
    let index = index_name
        .and_then(|index_name| layer.child(index_name))
        .and_then(|child| child.props.first())
        .and_then(RawProp::as_array);
    if reference == Reference::IndexToDirect && index_name.is_some() && index.is_none() {
        return Err(
            FbxError::new(FbxErrorKind::InvalidStructure, "索引配列がない").with_object(object),
        );
    }
    Ok(LayerDesc {
        mapping,
        reference,
        data,
        index,
        name: name.unwrap_or_default(),
    })
}

/// `Connections` の `OO`（子→親）から親子と所属を組み立てる。
///
/// 先頭プロパティが `OO` の接続だけを読み、`OP` などは読まない。
/// 未知IDへの接続は無視する。Model同士の多重親と循環、
/// 1つの `Geometry` の多重所有は不正として失敗する。
fn apply_connections(
    top: &[RawNode],
    models: &mut [Model],
    meshes: &mut [Mesh],
) -> Result<(), FbxError> {
    let Some(connections) = top.iter().find(|node| node.name.as_ref() == "Connections") else {
        return Ok(());
    };
    let known_models: HashSet<i64> = models.iter().map(|model| model.id.value()).collect();
    let known_meshes: HashSet<i64> = meshes.iter().map(|mesh| mesh.id.value()).collect();
    // 位置は先に確定し、走査中の借用と更新の競合を避ける。
    let at_model: HashMap<i64, usize> = models
        .iter()
        .enumerate()
        .map(|(index, model)| (model.id.value(), index))
        .collect();
    // メッシュIDから所有モデルIDへの対応。重複する同一対応の無視と多重所有の検出に使う。
    let mut mesh_owner: HashMap<i64, i64> = HashMap::new();
    // 親子と所属は接続順に積み、重複する同一対応は無視する。
    for node in &connections.children {
        if node.props.first().and_then(RawProp::as_bytes) != Some(b"OO".as_slice()) {
            continue;
        }
        let Some(child) = node.props.get(1).and_then(RawProp::as_int) else {
            continue;
        };
        let Some(parent) = node.props.get(2).and_then(RawProp::as_int) else {
            continue;
        };
        if known_models.contains(&child) && known_models.contains(&parent) {
            let child_at = at_model[&child];
            if models[child_at].parent == Some(ObjectId(parent)) {
                continue;
            }
            if models[child_at].parent.is_some() {
                return Err(
                    FbxError::new(FbxErrorKind::InvalidStructure, "親モデルが複数ある")
                        .with_object(ObjectId(child)),
                );
            }
            models[child_at].parent = Some(ObjectId(parent));
            let parent_at = at_model[&parent];
            models[parent_at].children.push(ObjectId(child));
        } else if known_meshes.contains(&child) && known_models.contains(&parent) {
            if let Some(&owner) = mesh_owner.get(&child) {
                if owner != parent {
                    return Err(FbxError::new(
                        FbxErrorKind::InvalidStructure,
                        "1つのGeometryを複数Modelが所有する",
                    )
                    .with_object(ObjectId(child)));
                }
                continue;
            }
            let parent_at = at_model[&parent];
            if models[parent_at].mesh.is_some() {
                // 同一Modelへの複数Geometryの接続は、後勝ちにせず先勝ちで無視しない。
                // 生成側の対応付けが曖昧になるため、明示的な非対応として失敗する。
                // ただし実データでは1対1のため、ここに到達するのは破損入力である。
                return Err(FbxError::new(
                    FbxErrorKind::InvalidStructure,
                    "1つのModelが複数Geometryを持つ",
                )
                .with_object(ObjectId(parent)));
            }
            models[parent_at].mesh = Some(ObjectId(child));
            mesh_owner.insert(child, parent);
        }
    }
    // 親を辿って循環を検出する。各節点の親は高々1つのため、
    // 未訪問・訪問中・完了の3状態で各辺を一度だけ辿る。
    let mut state: HashMap<i64, u8> = HashMap::with_capacity(models.len());
    for index in 0..models.len() {
        let start = models[index].id.value();
        if state.get(&start) == Some(&2) {
            continue;
        }
        let mut current = start;
        let mut path = Vec::new();
        loop {
            match state.get(&current).copied().unwrap_or(0) {
                // 完了済みに到達した経路に循環はない。
                2 => break,
                // 訪問中の節点に戻った場合は循環である。
                1 => {
                    return Err(
                        FbxError::new(FbxErrorKind::InvalidStructure, "親子に循環がある")
                            .with_object(ObjectId(current)),
                    );
                }
                _ => {
                    state.insert(current, 1);
                    path.push(current);
                    match models[at_model[&current]].parent {
                        Some(parent) => current = parent.value(),
                        None => break,
                    }
                }
            }
        }
        for id in path {
            state.insert(id, 2);
        }
    }
    Ok(())
}

/// 頂点配列を展開して座標列で返す。
fn read_positions(bytes: &[u8], mesh: &Mesh) -> Result<Vec<[f64; 3]>, FbxError> {
    let plain = expand(bytes, mesh.vertices, mesh.id)?;
    let values = mesh::as_f64s(plain.as_slice(), mesh.vertices.kind, mesh.id)?;
    mesh::ensure_finite(&values, mesh.id)?;
    if values.len() % 3 != 0 {
        return Err(
            FbxError::new(FbxErrorKind::InvalidStructure, "頂点配列が3の倍数でない")
                .with_object(mesh.id),
        );
    }
    Ok(values.chunks_exact(3).map(|v| [v[0], v[1], v[2]]).collect())
}

/// 多角形索引を展開し、面ごとのcorner数とcorner列で返す。
fn read_polygons(
    bytes: &[u8],
    mesh: &Mesh,
    vertex_count: usize,
) -> Result<(Vec<u32>, Vec<u32>), FbxError> {
    let indices = expand_integers(bytes, mesh.polygon_vertex_index, mesh.id)?;
    let mut counts = Vec::new();
    let mut corners = Vec::with_capacity(mesh.corner_count);
    let mut open: Vec<u32> = Vec::new();
    for &v in &indices {
        // `!v` は `-v-1` と等しく、FBXの負値終端を絶対値に戻す。
        let index = if v >= 0 { v } else { !v };
        if index < 0 || index as usize >= vertex_count {
            return Err(
                FbxError::new(FbxErrorKind::InvalidStructure, "頂点番号が範囲外である")
                    .with_object(mesh.id),
            );
        }
        open.push(index as u32);
        if v < 0 {
            if open.len() < 3 {
                return Err(
                    FbxError::new(FbxErrorKind::InvalidStructure, "3未満の多角形がある")
                        .with_object(mesh.id),
                );
            }
            counts.push(open.len() as u32);
            corners.extend_from_slice(&open);
            open.clear();
        }
    }
    if !open.is_empty() {
        return Err(
            FbxError::new(FbxErrorKind::InvalidStructure, "多角形の終端がない")
                .with_object(mesh.id),
        );
    }
    Ok((counts, corners))
}

/// 属性層の解決に使う形状（corner列・面表・頂点数）。
///
/// 解決関数の引き回しをまとめるための束ねであり、意味の追加はしない。
struct AttrShape<'x> {
    corners: &'x [u32],
    face_counts: &'x [u32],
    vertex_count: usize,
}

/// 法線層を解決してcorner対応で返す。
///
/// 先頭の `LayerElementNormal` だけを使い、対応方式と参照方式の組み合わせを
/// 領域（頂点・corner・面・全体）の大きさで検証する。
fn read_normals(
    bytes: &[u8],
    object: ObjectId,
    layer: &LayerDesc,
    shape: &AttrShape<'_>,
) -> Result<NormalLayer, FbxError> {
    let (values, corner_indices) = resolve_double3(bytes, object, layer, shape, "法線")?;
    Ok(NormalLayer {
        values,
        corner_indices,
    })
}

/// UV層を解決してcorner対応で返す。
fn read_uvs(
    bytes: &[u8],
    object: ObjectId,
    layer: &LayerDesc,
    shape: &AttrShape<'_>,
) -> Result<UvSet, FbxError> {
    let plain = expand(bytes, layer.data, object)?;
    let values = mesh::as_f64s(plain.as_slice(), layer.data.kind, object)?;
    mesh::ensure_finite(&values, object)?;
    if values.len() % 2 != 0 {
        return Err(
            FbxError::new(FbxErrorKind::InvalidStructure, "UV配列が2の倍数でない")
                .with_object(object),
        );
    }
    let data: Vec<[f64; 2]> = values.chunks_exact(2).map(|v| [v[0], v[1]]).collect();
    let corner_indices = resolve_indices(bytes, object, layer, shape, data.len(), "UV")?;
    Ok(UvSet {
        name: layer.name.to_string(),
        values: data,
        corner_indices,
    })
}

/// 実数3成分の属性層（法線）を解決する。
fn resolve_double3(
    bytes: &[u8],
    object: ObjectId,
    layer: &LayerDesc,
    shape: &AttrShape<'_>,
    what: &'static str,
) -> Result<(Vec<[f64; 3]>, Vec<u32>), FbxError> {
    let plain = expand(bytes, layer.data, object)?;
    let values = mesh::as_f64s(plain.as_slice(), layer.data.kind, object)?;
    mesh::ensure_finite(&values, object)?;
    if values.len() % 3 != 0 {
        return Err(
            FbxError::new(FbxErrorKind::InvalidStructure, "属性配列が3の倍数でない")
                .with_object(object),
        );
    }
    let data: Vec<[f64; 3]> = values.chunks_exact(3).map(|v| [v[0], v[1], v[2]]).collect();
    let corner_indices = resolve_indices(bytes, object, layer, shape, data.len(), what)?;
    Ok((data, corner_indices))
}

/// 属性層の索引をcorner列に解決する。
///
/// `Direct` は領域の大きさとデータ長が一致することを確かめ、
/// `IndexToDirect` は索引長が領域と一致し、値がデータ範囲内であることを確かめる。
/// 領域の大きさは対応方式で決まる（頂点・corner・面・全体）。
fn resolve_indices(
    bytes: &[u8],
    object: ObjectId,
    layer: &LayerDesc,
    shape: &AttrShape<'_>,
    data_len: usize,
    what: &'static str,
) -> Result<Vec<u32>, FbxError> {
    let face_of = face_starts(shape.face_counts);
    // 対応方式ごとの領域の大きさである。`AllSame` の領域は1である。
    let domain_len = match layer.mapping {
        Mapping::ByVertice => shape.vertex_count,
        Mapping::ByPolygonVertex => shape.corners.len(),
        Mapping::ByPolygon => shape.face_counts.len(),
        Mapping::AllSame => 1,
    };
    // 領域の要素からcornerへの写像を数える。`ByVertice` は頂点経由である。
    let domain_of_corner = |corner: usize| -> usize {
        match layer.mapping {
            Mapping::ByVertice => shape.corners[corner] as usize,
            Mapping::ByPolygonVertex => corner,
            Mapping::ByPolygon => face_of[corner],
            Mapping::AllSame => 0,
        }
    };
    match layer.reference {
        Reference::Direct => {
            if data_len != domain_len {
                return Err(FbxError::new(
                    FbxErrorKind::InvalidStructure,
                    "属性データ長が対応方式と一致しない",
                )
                .with_object(object));
            }
            // `Direct` は領域の番号をそのまま索引にする。
            // `u32` への変換は領域の大きさが `usize` に収まる範囲であり、
            // 巨大な入力は配列上限で先に抑える。
            let _ = what;
            Ok((0..shape.corners.len())
                .map(|corner| domain_of_corner(corner) as u32)
                .collect())
        }
        Reference::IndexToDirect => {
            let index_desc = layer.index.ok_or_else(|| {
                FbxError::new(FbxErrorKind::InvalidStructure, "索引配列がない").with_object(object)
            })?;
            let raw = expand_integers(bytes, index_desc, object)?;
            if raw.len() != domain_len {
                return Err(FbxError::new(
                    FbxErrorKind::InvalidStructure,
                    "索引長が対応方式と一致しない",
                )
                .with_object(object));
            }
            for &v in &raw {
                if v < 0 || v as usize >= data_len {
                    return Err(FbxError::new(
                        FbxErrorKind::InvalidStructure,
                        "属性番号が範囲外である",
                    )
                    .with_object(object));
                }
            }
            Ok((0..shape.corners.len())
                .map(|corner| raw[domain_of_corner(corner)] as u32)
                .collect())
        }
    }
}

/// 面番号ごとの先頭corner位置から、corner番号→面番号の表を返す。
fn face_starts(face_counts: &[u32]) -> Vec<usize> {
    let mut table = Vec::new();
    for (face, &count) in face_counts.iter().enumerate() {
        table.extend(std::iter::repeat_n(face, count as usize));
    }
    table
}

/// 材質層を面ごとに解決する。
///
/// 先頭の `LayerElementMaterial` だけを使い、`AllSame` と `ByPolygon` を
/// 受け付ける。番号の実在は保証せず、範囲検査（負値の拒否）までを行う。
fn read_materials(
    bytes: &[u8],
    mesh: &Mesh,
    layer: &LayerDesc,
    polygon_count: usize,
) -> Result<Vec<u32>, FbxError> {
    // 材質の `Materials` は値そのものであり、参照方式によらず直接読む。
    // 値から他の表への間接は無いためである。
    let plain = expand(bytes, layer.data, mesh.id)?;
    let values = mesh::as_i64s(plain.as_slice(), layer.data.kind, mesh.id)?;
    match (layer.mapping, layer.reference) {
        (Mapping::AllSame, _) => {
            if values.len() != 1 {
                return Err(FbxError::new(
                    FbxErrorKind::InvalidStructure,
                    "材質数が対応方式と一致しない",
                )
                .with_object(mesh.id));
            }
            let first = to_material_index(values[0], mesh.id)?;
            Ok(vec![first; polygon_count])
        }
        (Mapping::ByPolygon, _) => {
            if values.len() != polygon_count {
                return Err(FbxError::new(
                    FbxErrorKind::InvalidStructure,
                    "材質数が面数と一致しない",
                )
                .with_object(mesh.id));
            }
            values
                .into_iter()
                .map(|v| to_material_index(v, mesh.id))
                .collect()
        }
        _ => Err(FbxError::new(
            FbxErrorKind::UnsupportedFeature,
            "非対応の材質対応方式である",
        )
        .with_object(mesh.id)),
    }
}

/// 材質番号1件を `u32` で求める。負値は破損として失敗する。
fn to_material_index(value: i64, object: ObjectId) -> Result<u32, FbxError> {
    u32::try_from(value).map_err(|_| {
        FbxError::new(FbxErrorKind::InvalidStructure, "材質番号が範囲外である").with_object(object)
    })
}

/// 整数配列を展開して `i64` 列で返す。
fn expand_integers(bytes: &[u8], desc: ArrayDesc, object: ObjectId) -> Result<Vec<i64>, FbxError> {
    if desc.kind != ArrayKind::I32 && desc.kind != ArrayKind::I64 {
        return Err(
            FbxError::new(FbxErrorKind::InvalidStructure, "整数配列でない").with_object(object),
        );
    }
    let plain = expand(bytes, desc, object)?;
    mesh::as_i64s(plain.as_slice(), desc.kind, object)
}
