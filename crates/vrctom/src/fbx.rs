//! バイナリFBXからメッシュ幾何・モデル階層・スキン・バインドポーズ・
//! ブレンドシェイプを読み取る層。
//!
//! 担当するのは、展開済みバイト列の借用による一度の走査と、メッシュ・スキン・形状単位の
//! オンデマンド展開までである。VRM生成・三角形化・座標系変換・スキニングの
//! 解決は行わない。
//!
//! 入口は [`FbxFile::parse`](crate::fbx::FbxFile::parse)（走査）、
//! [`FbxFile::mesh_data`](crate::fbx::FbxFile::mesh_data)（対象配列の展開）、
//! [`FbxFile::skin_data`](crate::fbx::FbxFile::skin_data)（対象配列の展開）、
//! [`FbxFile::shape_data`](crate::fbx::FbxFile::shape_data)（対象配列の展開）、
//! [`FbxFile::bind_pose_nodes`](crate::fbx::FbxFile::bind_pose_nodes)
//! （BindPoseの統合）である。座標値と変換値はファイルの数値のまま返し、軸や単位は値として返す。
//!
//! ```mermaid
//! flowchart LR
//!     bytes[入力バイト列<br/>借用] --> header[ヘッダ検証<br/>magicとversion]
//!     header --> walk[ノード走査<br/>深さ・件数・境界の検査]
//!     walk --> objects[Objects<br/>ModelとGeometry MeshとShape<br/>DeformerとPose]
//!     walk --> conn[Connections<br/>OOの子→親]
//!     objects --> file[FbxFile<br/>階層・スキンとブレンドシェイプ要約・配列位置]
//!     conn --> file
//!     file --> decode[mesh_data / skin_data / shape_data<br/>対象配列だけzlib展開]
//!     file --> pose[bind_pose_nodes<br/>BindPoseの統合]
//!     decode --> data[MeshData / SkinData / ShapeData<br/>座標・面・法線・UV・材質番号・重み・差分]
//!     pose --> matrices[モデルごとのバインド行列]
//! ```
//!
//! 配列は解析時には展開せず、絶対位置・要素数・要素型・圧縮方式だけを保持する。
//! `mesh_data` と `skin_data` と `shape_data` は要求された対象の配列だけを境界検査付きで展開するため、
//! 確保するメモリは対象の大きさに比例し、ファイル全体には比例しない。

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
/// ピボット・オフセット・事前事後回転・幾何変換は、FBXの標準的な合成規則で
/// [`LocalTransform::local_matrix`]（階層側）と
/// [`LocalTransform::geometric_matrix`]（メッシュ側）が1つの行列にまとめる。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LocalTransform {
    /// 平行移動。
    pub translation: [f64; 3],
    /// 回転（XYZ順の度）。
    pub rotation: [f64; 3],
    /// 拡大縮小。
    pub scale: [f64; 3],
    /// 回転オフセット。
    pub rotation_offset: [f64; 3],
    /// 回転ピボット。
    pub rotation_pivot: [f64; 3],
    /// 事前回転（XYZ順の度）。
    pub pre_rotation: [f64; 3],
    /// 事後回転（XYZ順の度）。
    pub post_rotation: [f64; 3],
    /// 拡大縮小オフセット。
    pub scaling_offset: [f64; 3],
    /// 拡大縮小ピボット。
    pub scaling_pivot: [f64; 3],
    /// 幾何平行移動（メッシュ側）。
    pub geometric_translation: [f64; 3],
    /// 幾何回転（XYZ順の度、メッシュ側）。
    pub geometric_rotation: [f64; 3],
    /// 幾何拡大縮小（メッシュ側）。
    pub geometric_scaling: [f64; 3],
    /// 回転が有効かどうか。偽の場合は回転系（回転・事前事後回転）を無視する。
    pub rotation_active: bool,
}

impl LocalTransform {
    /// 既定のローカル変換（移動0・回転0・倍率1・補正なし・回転有効）を作る。
    fn identity() -> Self {
        Self {
            translation: [0.0, 0.0, 0.0],
            rotation: [0.0, 0.0, 0.0],
            scale: [1.0, 1.0, 1.0],
            rotation_offset: [0.0, 0.0, 0.0],
            rotation_pivot: [0.0, 0.0, 0.0],
            pre_rotation: [0.0, 0.0, 0.0],
            post_rotation: [0.0, 0.0, 0.0],
            scaling_offset: [0.0, 0.0, 0.0],
            scaling_pivot: [0.0, 0.0, 0.0],
            geometric_translation: [0.0, 0.0, 0.0],
            geometric_rotation: [0.0, 0.0, 0.0],
            geometric_scaling: [1.0, 1.0, 1.0],
            rotation_active: true,
        }
    }

    /// 全成分が有限かどうかを確かめる。非有限の場合は構造破損として失敗する。
    fn ensure_finite(&self) -> Result<(), FbxError> {
        let all = [
            &self.translation,
            &self.rotation,
            &self.scale,
            &self.rotation_offset,
            &self.rotation_pivot,
            &self.pre_rotation,
            &self.post_rotation,
            &self.scaling_offset,
            &self.scaling_pivot,
            &self.geometric_translation,
            &self.geometric_rotation,
            &self.geometric_scaling,
        ];
        if all.iter().all(|v| v.iter().all(|v| v.is_finite())) {
            Ok(())
        } else {
            Err(FbxError::new(
                FbxErrorKind::InvalidStructure,
                "有限でない変換値がある",
            ))
        }
    }

    /// 標準的なFBXのローカル変換合成で4x4行列を作る。
    ///
    /// 合成は `T × Roff × Rp × Rpre × R × Rpost × Rp⁻¹ × Soff × Sp × S × Sp⁻¹`
    /// （列ベクトル規約。左が後から適用される）であり、幾何変換は含まない。
    /// 回転の三つ組（事前・通常・事後）はいずれもFBXのXYZ順に従い、
    /// 行ベクトル由来の転置として角度を反転させて合成する。詳細は
    /// 行列欄の `fbx_euler_xyz_matrix` を参照。幾何変換はメッシュ側で
    /// [`LocalTransform::geometric_matrix`] を掛ける。
    /// `rotation_active` が偽の場合は、回転・事前事後回転を無視する。
    /// 補正成分が既定値の場合は `T × R × S` と1ビットも変わらない。
    ///
    /// # Errors
    ///
    /// いずれかの成分が非有限、または合成結果が非有限の場合に返す。
    pub fn local_matrix(&self) -> Result<[f64; 16], FbxError> {
        use crate::vrm::matrix;
        self.ensure_finite()?;
        let zero = [0.0, 0.0, 0.0];
        let (rotation, pre_rotation, post_rotation) = if self.rotation_active {
            (self.rotation, self.pre_rotation, self.post_rotation)
        } else {
            (zero, zero, zero)
        };
        let mut out = matrix::translation_matrix(self.translation);
        out = matrix::mul(&out, &matrix::translation_matrix(self.rotation_offset));
        out = matrix::mul(&out, &matrix::translation_matrix(self.rotation_pivot));
        out = matrix::mul(&out, &matrix::fbx_euler_xyz_matrix(pre_rotation));
        out = matrix::mul(&out, &matrix::fbx_euler_xyz_matrix(rotation));
        out = matrix::mul(&out, &matrix::fbx_euler_xyz_matrix(post_rotation));
        out = matrix::mul(
            &out,
            &matrix::translation_matrix([
                -self.rotation_pivot[0],
                -self.rotation_pivot[1],
                -self.rotation_pivot[2],
            ]),
        );
        out = matrix::mul(&out, &matrix::translation_matrix(self.scaling_offset));
        out = matrix::mul(&out, &matrix::translation_matrix(self.scaling_pivot));
        out = matrix::mul(&out, &matrix::non_uniform_scale_matrix(self.scale));
        out = matrix::mul(
            &out,
            &matrix::translation_matrix([
                -self.scaling_pivot[0],
                -self.scaling_pivot[1],
                -self.scaling_pivot[2],
            ]),
        );
        if matrix::is_finite_matrix(&out) {
            Ok(out)
        } else {
            Err(FbxError::new(
                FbxErrorKind::InvalidStructure,
                "合成したローカル行列が有限でない",
            ))
        }
    }

    /// 幾何変換を4x4行列にまとめる。`G = Tgeo × Rgeo × Sgeo` である。
    ///
    /// 階層の合成には含めず、メッシュ側で所有モデルの世界行列に掛ける
    /// （`World × G`）。成分が既定値の場合は単位行列になる。
    ///
    /// # Errors
    ///
    /// いずれかの成分が非有限、または合成結果が非有限の場合に返す。
    pub fn geometric_matrix(&self) -> Result<[f64; 16], FbxError> {
        use crate::vrm::matrix;
        self.ensure_finite()?;
        let mut out = matrix::translation_matrix(self.geometric_translation);
        out = matrix::mul(&out, &matrix::fbx_euler_xyz_matrix(self.geometric_rotation));
        out = matrix::mul(
            &out,
            &matrix::non_uniform_scale_matrix(self.geometric_scaling),
        );
        if matrix::is_finite_matrix(&out) {
            Ok(out)
        } else {
            Err(FbxError::new(
                FbxErrorKind::InvalidStructure,
                "合成した幾何行列が有限でない",
            ))
        }
    }
}

/// 親の世界行列とローカル行列から子の世界行列を作る。
///
/// `inherit_type` は `InheritType` の値であり、0（RrSs）は親の行列をそのまま継承し、
/// 1（RSrs）は親の拡大縮小が子の回転方向を歪めないよう子の回転を親の拡大縮小の
/// 左に置き、2（Rrs）は親の拡大縮小を無視する。子の平行移動と拡大縮小は
/// 1でも親の拡大縮小を受け、2では受けない。親の拡大縮小が単位の場合は
/// いずれも `親 × ローカル` と一致する。
/// せん断を含む親行列の分解は行わないため、分解に失敗した場合は不正として失敗する。
///
/// # Errors
///
/// 入力または結果が非有限、継承方式が0〜2以外、RSrs・Rrsで親の分解に
/// 失敗した場合に返す。
pub fn inherit_world_matrix(
    parent_world: &[f64; 16],
    local: &[f64; 16],
    inherit_type: i32,
) -> Result<[f64; 16], FbxError> {
    use crate::vrm::matrix;
    if !matrix::is_finite_matrix(parent_world) || !matrix::is_finite_matrix(local) {
        return Err(FbxError::new(
            FbxErrorKind::InvalidStructure,
            "有限でない行列がある",
        ));
    }
    let out = match inherit_type {
        // RrSs: 親の行列をそのまま継承する。
        0 => matrix::mul(parent_world, local),
        // RSrs: 親の拡大縮小が子の回転方向を歪めないよう、子の回転を左に置く。
        // `Tp × Rp × Rl × Sp × Tl × Sl` であり、子の平行移動と拡大縮小は
        // 親の拡大縮小を受ける。FBX SDKの説明の解釈であり、詳細は設計文書を参照。
        1 => {
            let (parent_t, parent_r, parent_s) =
                matrix::decompose_trs(parent_world).ok_or_else(|| {
                    FbxError::new(FbxErrorKind::InvalidStructure, "親の世界行列を分解できない")
                })?;
            let (local_t, local_r, local_s) = matrix::decompose_trs(local).ok_or_else(|| {
                FbxError::new(FbxErrorKind::InvalidStructure, "ローカル行列を分解できない")
            })?;
            // `Rl` を `Sp` の左に置き直すため、分解した回転とそれ以外を組み替える。
            let left = matrix::mul(
                &matrix::translation_matrix(parent_t),
                &matrix::mul(&parent_r, &local_r),
            );
            let right = matrix::mul(
                &matrix::non_uniform_scale_matrix(parent_s),
                &matrix::mul(
                    &matrix::translation_matrix(local_t),
                    &matrix::non_uniform_scale_matrix(local_s),
                ),
            );
            matrix::mul(&left, &right)
        }
        // Rrs: 親の拡大縮小を無視する。
        2 => {
            let (parent_t, parent_r, _) = matrix::decompose_trs(parent_world).ok_or_else(|| {
                FbxError::new(FbxErrorKind::InvalidStructure, "親の世界行列を分解できない")
            })?;
            let left = matrix::mul(&matrix::translation_matrix(parent_t), &parent_r);
            matrix::mul(&left, local)
        }
        _ => {
            return Err(FbxError::new(
                FbxErrorKind::UnsupportedFeature,
                "非対応の継承方式である",
            ));
        }
    };
    if matrix::is_finite_matrix(&out) {
        Ok(out)
    } else {
        Err(FbxError::new(
            FbxErrorKind::InvalidStructure,
            "合成した世界行列が有限でない",
        ))
    }
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
    /// 材質スロット。接続順を保つ。
    pub materials: Vec<ObjectId>,
    /// ローカル変換。未指定の成分は既定値（移動0・回転0・倍率1）である。
    pub transform: LocalTransform,
    /// 継承方式。未指定の場合は1である。合成規則の解釈は生成側に残す。
    pub inherit_type: i32,
}

/// 材質1件の識別子。
///
/// 色・テクスチャ・シェーダは読まず、IDと名前だけを保持する。
/// 面の材質番号とスロット順の対応付けに使う。
#[derive(Debug, Clone)]
pub struct Material {
    /// オブジェクトID。
    pub id: ObjectId,
    /// 材質名。`Name\x00Class` 形式の場合は最初の `\x00` で切る。
    pub name: String,
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

/// スキン1件の要約。
///
/// 変形対象のメッシュとそれを所有するモデル、結合モデルの一覧を持つ。
/// 索引と重みの配列は持たず、[`skin_data`](FbxFile::skin_data) が対象分だけ展開する。
#[derive(Debug, Clone)]
pub struct Skin {
    /// オブジェクトID。
    pub id: ObjectId,
    /// スキン名。`Name\x00Class` 形式の場合は最初の `\x00` で切る。
    pub name: String,
    /// 変形対象のメッシュGeometry。
    pub geometry: ObjectId,
    /// そのGeometryを所有するModel。
    pub model: ObjectId,
    /// 結合モデルの一覧。`Connections` の出現順を保つ。
    pub clusters: Vec<Cluster>,
}

/// クラスタ1件の要約。
///
/// 結合モデルと重み付き頂点数を持ち、索引と重みの配列位置は非公開で保持する。
/// 配列の中身は [`skin_data`](FbxFile::skin_data) が対象分だけ展開する。
#[derive(Debug, Clone)]
pub struct Cluster {
    /// オブジェクトID。
    pub id: ObjectId,
    /// 結合モデル。
    pub joint: ObjectId,
    /// 重み付き頂点数。`Indexes`/`Weights` を持たない場合は0である。
    pub influence_count: usize,
    /// 頂点索引の配列位置。無い場合は `None` である。
    pub(crate) indexes: Option<ArrayDesc>,
    /// 重みの配列位置。無い場合は `None` である。
    pub(crate) weights: Option<ArrayDesc>,
    /// `Transform`（変形対象のバインド時世界行列）の位置。無い場合は `None` である。
    pub(crate) transform: Option<ArrayDesc>,
    /// `TransformLink`（結合モデルのバインド時世界行列）の位置。無い場合は `None` である。
    pub(crate) transform_link: Option<ArrayDesc>,
}

/// スキン1件の重みの展開結果。
///
/// 重みは書かれた値をそのまま返し、正規化しない。実データには総和が1でない
/// 頂点や微小な負値が含まれ、これらは破損ではなく入力の性質であるため、
/// 正規化と影響数の制限は生成側が担う。
#[derive(Debug, Clone, PartialEq)]
pub struct SkinData {
    /// クラスタごとの重み。`Skin::clusters` と同じ順序である。
    pub clusters: Vec<ClusterData>,
}

/// クラスタ1件の重みの展開結果。
#[derive(Debug, Clone, PartialEq)]
pub struct ClusterData {
    /// 結合モデル。
    pub joint: ObjectId,
    /// 変形対象の制御点番号。重みなしの場合は空である。
    pub vertex_indices: Vec<u32>,
    /// 頂点ごとの重み。`vertex_indices` と同じ長さである。
    pub weights: Vec<f64>,
}

/// ブレンドシェイプ1件の要約。
///
/// 変形対象のメッシュとチャンネルの一覧を持ち、形状のデルタは持たない。
/// デルタの中身は [`shape_data`](FbxFile::shape_data) が対象分だけ展開する。
#[derive(Debug, Clone)]
pub struct BlendShape {
    /// オブジェクトID。
    pub id: ObjectId,
    /// ブレンドシェイプ名。`Name\x00Class` 形式の場合は最初の `\x00` で切る。
    pub name: String,
    /// 変形対象のメッシュGeometry。
    pub geometry: ObjectId,
    /// チャンネルの一覧。`Connections` の出現順を保つ。
    pub channels: Vec<ObjectId>,
}

/// ブレンドシェイプチャンネル1件の要約。
///
/// 接続する形状の一覧を持ち、デルタと重み率の中身は持たない。
/// デルタは [`shape_data`](FbxFile::shape_data)、重み率は
/// [`blend_channel_weights`](FbxFile::blend_channel_weights) が対象分だけ展開する。
#[derive(Debug, Clone)]
pub struct BlendShapeChannel {
    /// オブジェクトID。
    pub id: ObjectId,
    /// チャンネル名。`Name\x00Class` 形式の場合は最初の `\x00` で切る。
    pub name: String,
    /// `DeformPercent` の値。無い場合は `None` である。
    pub deform_percent: Option<f64>,
    /// `FullWeights` の有無。ある場合は [`blend_channel_weights`](FbxFile::blend_channel_weights) で展開する。
    pub has_full_weights: bool,
    /// 接続する形状。`Connections` の出現順を保つ。
    pub shapes: Vec<BlendShapeFrame>,
}

/// チャンネルに接続する形状1件。
#[derive(Debug, Clone)]
pub struct BlendShapeFrame {
    /// 形状のオブジェクトID。
    pub shape: ObjectId,
    /// 形状名。`Name\x00Class` 形式の場合は最初の `\x00` で切る。
    pub name: String,
}

/// 形状1件のデルタの展開結果。
///
/// 位置の差分だけを返し、法線の差分は読まない。
#[derive(Debug, Clone, PartialEq)]
pub struct ShapeData {
    /// 差分の対象の制御点番号。
    pub control_point_indices: Vec<u32>,
    /// 制御点ごとの位置の差分。`control_point_indices` と同じ長さである。
    pub position_deltas: Vec<[f64; 3]>,
}

/// バインドポーズ1件の統合結果。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BindPoseNode {
    /// 対象モデル。
    pub model: ObjectId,
    /// 16値の行優先。平行移動は12〜14番である。
    pub matrix: [f64; 16],
}

/// クラスタ1件のバインド時行列の展開結果。
///
/// `Transform` は変形対象（メッシュ側）のバインド時世界行列、
/// `TransformLink` は結合モデル（関節側）のバインド時世界行列である。
/// いずれも16値の行優先（平行移動は12〜14番）で返す。
/// 配列を持たないクラスタは `None` になる。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClusterBindMatrix {
    /// クラスタのオブジェクトID。
    pub cluster: ObjectId,
    /// 結合モデル。
    pub joint: ObjectId,
    /// `Transform` の行列。無い場合は `None` である。
    pub transform: Option<[f64; 16]>,
    /// `TransformLink` の行列。無い場合は `None` である。
    pub transform_link: Option<[f64; 16]>,
}
/// 読み取り済みのFBX。
///
/// 入力バイト列を借用し、階層・要約・配列位置だけを保持する。
/// 幾何と重みの中身は [`mesh_data`](Self::mesh_data) と
/// [`skin_data`](Self::skin_data) が対象分だけ展開する。
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
    materials: Vec<Material>,
    material_at: HashMap<i64, usize>,
    /// メッシュIDから所有モデルIDへの対応。スロット範囲検査に使う。
    mesh_owner: HashMap<i64, i64>,
    skins: Vec<Skin>,
    skin_at: HashMap<i64, usize>,
    /// ブレンドシェイプ1件の要約。順序は `Objects` の出現順である。
    blend_shapes: Vec<BlendShape>,
    /// ブレンドシェイプIDから `blend_shapes` への位置。
    blend_shape_at: HashMap<i64, usize>,
    /// チャンネルの要約。順序は `Objects` の出現順である。
    blend_channels: Vec<BlendChannelRec>,
    /// チャンネルIDから `blend_channels` への位置。
    blend_channel_at: HashMap<i64, usize>,
    /// チャンネルIDから接続する形状ID列への対応。`Connections` の出現順を保つ。
    channel_shapes: HashMap<i64, Vec<ObjectId>>,
    /// 形状IDから形状名への対応。
    shape_names: HashMap<i64, String>,
    /// 形状IDから `Indexes` 配列位置への対応。
    shape_indexes: HashMap<i64, ArrayDesc>,
    /// 形状IDから `Vertices` 配列位置への対応。
    shape_vertices: HashMap<i64, ArrayDesc>,
    /// 形状IDから対象メッシュIDへの対応。
    shape_mesh: HashMap<i64, ObjectId>,
    pose_nodes: Vec<StoredPoseNode>,
}

/// `BindPose` の `PoseNode` 1件の位置情報。
///
/// モデルIDと行列配列の位置だけを保持し、行列の中身は
/// [`bind_pose_nodes`](FbxFile::bind_pose_nodes) が対象分だけ展開する。
#[derive(Debug, Clone, Copy)]
struct StoredPoseNode {
    /// `PoseNode/Node` が指すモデルID。
    model: i64,
    /// `PoseNode/Matrix` の配列位置。
    matrix: ArrayDesc,
}

/// `Deformer` の種別。`Skin`・`Cluster`・`BlendShape`・`BlendShapeChannel` を読む。
///
/// ブレンドシェイプの重み合成は別の作業単位であり、この層では単一フレームの
/// 接続とデルタの読み取りまでを扱う。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeformerKind {
    /// 変形対象と結合モデルの対応付け。
    Skin,
    /// 結合モデルごとの頂点索引と重み。
    Cluster,
    /// ブレンドシェイプの束ね。チャンネルと対象メッシュを結ぶ。
    BlendShape,
    /// ブレンドシェイプの1チャンネル。形状と重み率を持つ。
    BlendShapeChannel,
}

/// `Deformer`（`Skin`/`Cluster`/`BlendShape`/`BlendShapeChannel`）1件の読み取り結果。
///
/// 索引と重みの配列は展開せず位置だけを残し、
/// [`skin_data`](FbxFile::skin_data) と [`shape_data`](FbxFile::shape_data) が
/// 対象分だけ展開する。ブレンドシェイプ系は形状の接続だけを保持し、
/// 重み率の解釈は呼び出し側に残す。
#[derive(Debug, Clone)]
struct DeformerRec {
    /// オブジェクトID。
    id: ObjectId,
    /// 名前。`Name\x00Class` 形式の場合は最初の `\x00` で切る。
    name: Box<str>,
    /// 種別。
    kind: DeformerKind,
    /// `Cluster/Indexes` の位置。無い場合は `None` である。
    indexes: Option<ArrayDesc>,
    /// `Cluster/Weights` の位置。無い場合は `None` である。
    weights: Option<ArrayDesc>,
    /// `Cluster/Transform` の位置。無い場合は `None` である。
    transform: Option<ArrayDesc>,
    /// `Cluster/TransformLink` の位置。無い場合は `None` である。
    transform_link: Option<ArrayDesc>,
    /// `BlendShapeChannel/DeformPercent`。チャンネル以外では `None` である。
    deform_percent: Option<f64>,
    /// `BlendShapeChannel/FullWeights` の位置。無い場合は `None` である。
    full_weights: Option<ArrayDesc>,
}

/// ブレンドシェイプチャンネル1件の内部要約。
///
/// 重み率の配列は展開せず位置だけを残す。
#[derive(Debug, Clone)]
struct BlendChannelRec {
    /// オブジェクトID。
    id: ObjectId,
    /// チャンネル名。
    name: String,
    /// `DeformPercent` の値。無い場合は `None` である。
    deform_percent: Option<f64>,
    /// `FullWeights` の位置。無い場合は `None` である。
    full_weights: Option<ArrayDesc>,
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
        let materials = read_material_objects(&top)?;
        let deformers = read_deformers(&top)?;
        let (pose_nodes, pose_ids) = read_pose_nodes(&top)?;
        // FBXのオブジェクトIDは全体で一意であり、衝突は
        // `apply_connections` の分類を曖昧にするため、接続解決の前に破損として拒否する。
        let mut used_ids: HashSet<i64> = models.iter().map(|model| model.id.value()).collect();
        for mesh in &meshes {
            if !used_ids.insert(mesh.id.value()) {
                return Err(FbxError::new(
                    FbxErrorKind::InvalidStructure,
                    "ModelとGeometryのIDが重複している",
                )
                .with_object(mesh.id));
            }
        }
        for material in &materials {
            if !used_ids.insert(material.id.value()) {
                return Err(FbxError::new(
                    FbxErrorKind::InvalidStructure,
                    "MaterialのIDが重複している",
                )
                .with_object(material.id));
            }
        }
        for rec in &deformers {
            if !used_ids.insert(rec.id.value()) {
                return Err(FbxError::new(
                    FbxErrorKind::InvalidStructure,
                    "DeformerのIDが重複している",
                )
                .with_object(rec.id));
            }
        }
        // PoseのIDも全体で一意であり、重複と他オブジェクトとの衝突を拒否する。
        for &pose_id in &pose_ids {
            if !used_ids.insert(pose_id) {
                return Err(FbxError::new(
                    FbxErrorKind::InvalidStructure,
                    "PoseのIDが重複している",
                )
                .with_object(ObjectId(pose_id)));
            }
        }
        apply_connections(&top, &mut models, &mut meshes, &materials)?;
        let skins = wire_skins(&top, &models, &meshes, &deformers)?;
        let (blend_shapes, blend_channels, channel_shapes) =
            wire_blend_shapes(&top, &meshes, &deformers)?;
        let shapes = read_shapes(bytes, &top)?;
        // 形状IDも全体で一意であり、重複と他オブジェクトとの衝突を拒否する。
        for shape in &shapes {
            if !used_ids.insert(shape.id.value()) {
                return Err(FbxError::new(
                    FbxErrorKind::InvalidStructure,
                    "GeometryのIDが重複している",
                )
                .with_object(shape.id));
            }
        }
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
        let skin_at = skins
            .iter()
            .enumerate()
            .map(|(index, skin)| (skin.id.value(), index))
            .collect();
        let material_at = materials
            .iter()
            .enumerate()
            .map(|(index, material)| (material.id.value(), index))
            .collect();
        let mesh_owner: HashMap<i64, i64> = models
            .iter()
            .filter_map(|model| model.mesh.map(|mesh| (mesh.value(), model.id.value())))
            .collect();
        let blend_shape_at = blend_shapes
            .iter()
            .enumerate()
            .map(|(index, shape)| (shape.id.value(), index))
            .collect();
        let blend_channel_at = blend_channels
            .iter()
            .enumerate()
            .map(|(index, channel)| (channel.id.value(), index))
            .collect();
        let mut shape_names = HashMap::with_capacity(shapes.len());
        let mut shape_indexes = HashMap::with_capacity(shapes.len());
        let mut shape_vertices = HashMap::with_capacity(shapes.len());
        for shape in &shapes {
            shape_names.insert(shape.id.value(), shape.name.clone());
            shape_indexes.insert(shape.id.value(), shape.indexes);
            shape_vertices.insert(shape.id.value(), shape.vertices);
        }
        let shape_mesh = build_shape_meshes(&channel_shapes, &blend_shapes)?;
        Ok(Self {
            bytes,
            version,
            axis_system,
            unit_scale_factor,
            models,
            model_at,
            meshes,
            mesh_at,
            materials,
            material_at,
            mesh_owner,
            skins,
            skin_at,
            blend_shapes,
            blend_shape_at,
            blend_channels,
            blend_channel_at,
            channel_shapes,
            shape_names,
            shape_indexes,
            shape_vertices,
            shape_mesh,
            pose_nodes,
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

    /// 材質の一覧を返す。順序は `Objects` の出現順である。
    pub fn materials(&self) -> &[Material] {
        &self.materials
    }

    /// 指定IDの材質を返す。
    pub fn material(&self, id: ObjectId) -> Option<&Material> {
        self.material_at
            .get(&id.value())
            .map(|&index| &self.materials[index])
    }

    /// 指定IDのメッシュ幾何を展開する。
    ///
    /// 対象メッシュの配列だけを境界検査付きで展開するため、確保するメモリは
    /// 対象メッシュの大きさに比例する。
    /// 面の材質番号は所有モデルのスロット数に対して範囲検査する。
    ///
    /// # Errors
    ///
    /// 未知ID・索引範囲外・属性長不一致・非有限値・スロット範囲外の材質番号などの場合に返す。
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
        // 面の材質番号は所有モデルのスロット数に対して範囲検査する。
        // 所有モデルが無い場合は材質スロットが決まらないため検査しない。
        // 材質層が無い場合は `material_indices` が空であり検査対象がない。
        if !material_indices.is_empty()
            && let Some(&owner) = self.mesh_owner.get(&id.value())
            && let Some(model) = self.model(ObjectId::new(owner))
        {
            let slots = model.materials.len();
            for &index in &material_indices {
                let out_of_range = if slots == 0 {
                    index != 0
                } else {
                    (index as usize) >= slots
                };
                if out_of_range {
                    return Err(FbxError::new(
                        FbxErrorKind::InvalidStructure,
                        "材質番号がスロット範囲外である",
                    )
                    .with_object(mesh.id));
                }
            }
        }
        Ok(MeshData {
            positions,
            polygon_corner_counts,
            corner_positions,
            normals,
            uv_sets,
            material_indices,
        })
    }

    /// ブレンドシェイプの一覧を返す。順序は `Objects` の出現順である。
    pub fn blend_shapes(&self) -> &[BlendShape] {
        &self.blend_shapes
    }

    /// 指定IDのブレンドシェイプを返す。
    pub fn blend_shape(&self, id: ObjectId) -> Option<&BlendShape> {
        self.blend_shape_at
            .get(&id.value())
            .map(|&index| &self.blend_shapes[index])
    }

    /// 指定メッシュのブレンドシェイプチャンネルを返す。
    ///
    /// 順序は `Connections` の出現順であり、チャンネル名の重複は許す。
    /// 選択と単一フレーム条件の検証は呼び出し側が行い、ここでは接続の列挙に留める。
    /// 対応する `BlendShape` が無いチャンネルは無視する。
    pub fn blend_shape_channels(&self, geometry: ObjectId) -> Vec<BlendShapeChannel> {
        let mut out = Vec::new();
        for shape in &self.blend_shapes {
            if shape.geometry != geometry {
                continue;
            }
            for &channel_id in &shape.channels {
                let Some(&index) = self.blend_channel_at.get(&channel_id.value()) else {
                    continue;
                };
                let Some(channel) = self.blend_channels.get(index) else {
                    continue;
                };
                let mut shapes = Vec::new();
                if let Some(shape_ids) = self.channel_shapes.get(&channel_id.value()) {
                    for &shape_id in shape_ids {
                        let name = self
                            .shape_names
                            .get(&shape_id.value())
                            .cloned()
                            .unwrap_or_default();
                        shapes.push(BlendShapeFrame {
                            shape: shape_id,
                            name,
                        });
                    }
                }
                out.push(BlendShapeChannel {
                    id: channel.id,
                    name: channel.name.clone(),
                    deform_percent: channel.deform_percent,
                    has_full_weights: channel.full_weights.is_some(),
                    shapes,
                });
            }
        }
        out
    }

    /// 指定IDの形状（ブレンドシェイプの1フレーム）のデルタを展開する。
    ///
    /// 対象形状の配列だけを境界検査付きで展開するため、確保するメモリは
    /// 対象形状の大きさに比例する。デルタは制御点番号と3成分の組で返し、
    /// 法線の差分は読まない。
    ///
    /// # Errors
    ///
    /// 未知ID・配列長不整合・範囲外の制御点番号・重複する制御点番号・
    /// 有限でないデルタなどの場合に返す。
    pub fn shape_data(&self, id: ObjectId) -> Result<ShapeData, FbxError> {
        let indexes_desc = self
            .shape_indexes
            .get(&id.value())
            .copied()
            .ok_or_else(|| FbxError::new(FbxErrorKind::NotFound, "形状がない").with_object(id))?;
        let vertices_desc = self
            .shape_vertices
            .get(&id.value())
            .copied()
            .ok_or_else(|| FbxError::new(FbxErrorKind::NotFound, "形状がない").with_object(id))?;
        let mesh_id = self.shape_mesh.get(&id.value()).copied().ok_or_else(|| {
            FbxError::new(
                FbxErrorKind::InvalidStructure,
                "形状の対象メッシュが決まらない",
            )
            .with_object(id)
        })?;
        let mesh = self.mesh(mesh_id).ok_or_else(|| {
            FbxError::new(FbxErrorKind::InvalidStructure, "形状の対象メッシュがない")
                .with_object(id)
        })?;
        let raw_indexes = expand_integers(self.bytes, indexes_desc, id)?;
        let plain = expand(self.bytes, vertices_desc, id)?;
        let raw_deltas = mesh::as_f64s(plain.as_slice(), vertices_desc.kind, id)?;
        mesh::ensure_finite(&raw_deltas, id)?;
        if raw_deltas.len() % 3 != 0 {
            return Err(FbxError::new(
                FbxErrorKind::InvalidStructure,
                "形状のデルタ配列が3の倍数でない",
            )
            .with_object(id));
        }
        if raw_deltas.len() / 3 != raw_indexes.len() {
            return Err(FbxError::new(
                FbxErrorKind::InvalidStructure,
                "形状の索引数とデルタ数が一致しない",
            )
            .with_object(id));
        }
        let mut seen: HashSet<u32> = HashSet::with_capacity(raw_indexes.len());
        let mut control_point_indices = Vec::with_capacity(raw_indexes.len());
        let mut position_deltas = Vec::with_capacity(raw_indexes.len());
        for (index, delta) in raw_indexes.iter().zip(raw_deltas.chunks_exact(3)) {
            if *index < 0 || *index as usize >= mesh.vertex_count {
                return Err(FbxError::new(
                    FbxErrorKind::InvalidStructure,
                    "形状の制御点番号が範囲外である",
                )
                .with_object(id));
            }
            let index = *index as u32;
            if !seen.insert(index) {
                return Err(FbxError::new(
                    FbxErrorKind::InvalidStructure,
                    "形状内で制御点番号が重複する",
                )
                .with_object(id));
            }
            control_point_indices.push(index);
            position_deltas.push([delta[0], delta[1], delta[2]]);
        }
        Ok(ShapeData {
            control_point_indices,
            position_deltas,
        })
    }

    /// 指定チャンネルの `FullWeights` を展開して返す。
    ///
    /// 対象チャンネルの配列だけを境界検査付きで展開するため、確保するメモリは
    /// 対象の大きさに比例する。値の解釈（単一フレーム条件の判定）は呼び出し側が行う。
    ///
    /// # Errors
    ///
    /// 未知ID・有限でない重みの場合に返す。
    pub fn blend_channel_weights(&self, id: ObjectId) -> Result<Vec<f64>, FbxError> {
        let desc = self
            .blend_channel_at
            .get(&id.value())
            .and_then(|&index| self.blend_channels.get(index))
            .and_then(|channel| channel.full_weights)
            .ok_or_else(|| {
                FbxError::new(FbxErrorKind::NotFound, "チャンネルがない").with_object(id)
            })?;
        let plain = expand(self.bytes, desc, id)?;
        let values = mesh::as_f64s(plain.as_slice(), desc.kind, id)?;
        mesh::ensure_finite(&values, id)?;
        Ok(values)
    }

    /// スキンの一覧を返す。順序は `Objects` の出現順である。
    pub fn skins(&self) -> &[Skin] {
        &self.skins
    }

    /// 指定IDのスキンを返す。
    pub fn skin(&self, id: ObjectId) -> Option<&Skin> {
        self.skin_at
            .get(&id.value())
            .map(|&index| &self.skins[index])
    }

    /// 指定IDのスキンの重みを展開する。
    ///
    /// 対象スキンの配列だけを境界検査付きで展開するため、確保するメモリは
    /// 対象スキンの大きさに比例する。重みは書かれた値のまま返し、
    /// 正規化は行わない。
    ///
    /// # Errors
    ///
    /// 未知ID・索引と重みの長さ不一致・範囲外の索引・重複する索引・
    /// 片方だけある索引と重み・有限でない重みなどの場合に返す。
    pub fn skin_data(&self, id: ObjectId) -> Result<SkinData, FbxError> {
        let skin = self
            .skin(id)
            .ok_or_else(|| FbxError::new(FbxErrorKind::NotFound, "スキンがない").with_object(id))?;
        let mesh = self.mesh(skin.geometry).ok_or_else(|| {
            FbxError::new(FbxErrorKind::InvalidStructure, "変形対象のメッシュがない")
                .with_object(skin.id)
        })?;
        let vertex_count = mesh.vertex_count;
        let mut clusters = Vec::with_capacity(skin.clusters.len());
        for cluster in &skin.clusters {
            let (vertex_indices, weights) = match (cluster.indexes, cluster.weights) {
                (None, None) => (Vec::new(), Vec::new()),
                (Some(indexes), Some(weights)) => {
                    read_cluster_weights(self.bytes, cluster.id, vertex_count, indexes, weights)?
                }
                _ => {
                    return Err(FbxError::new(
                        FbxErrorKind::InvalidStructure,
                        "クラスタのIndexesとWeightsが片方だけある",
                    )
                    .with_object(cluster.id));
                }
            };
            clusters.push(ClusterData {
                joint: cluster.joint,
                vertex_indices,
                weights,
            });
        }
        Ok(SkinData { clusters })
    }

    /// `BindPose` をモデルごとに統合して返す。
    ///
    /// 同じモデルが複数の `BindPose` に現れる場合は、行列が一致することを
    /// 確かめて1つにまとめる。`BindPose` が無い場合は空を返す。
    /// 順序はモデルIDの昇順である。未知のモデルを指す `PoseNode` は無視する。
    ///
    /// # Errors
    ///
    /// 行列の要素数が16でない・有限でない・同じモデルで一致しない場合に返す。
    pub fn bind_pose_nodes(&self) -> Result<Vec<BindPoseNode>, FbxError> {
        let mut by_model: HashMap<i64, [f64; 16]> = HashMap::with_capacity(self.pose_nodes.len());
        for node in &self.pose_nodes {
            if !self.model_at.contains_key(&node.model) {
                continue;
            }
            let object = ObjectId(node.model);
            let plain = expand(self.bytes, node.matrix, object)?;
            let values = mesh::as_f64s(plain.as_slice(), node.matrix.kind, object)?;
            if values.len() != 16 {
                return Err(FbxError::new(
                    FbxErrorKind::InvalidStructure,
                    "バインド行列が16要素でない",
                )
                .with_object(object));
            }
            if !values.iter().all(|v| v.is_finite()) {
                return Err(FbxError::new(
                    FbxErrorKind::InvalidStructure,
                    "有限でないバインド行列がある",
                )
                .with_object(object));
            }
            let matrix: [f64; 16] = values.try_into().map_err(|_| {
                FbxError::new(FbxErrorKind::InvalidStructure, "バインド行列が16要素でない")
                    .with_object(object)
            })?;
            match by_model.get(&node.model) {
                Some(&prev) if prev != matrix => {
                    return Err(FbxError::new(
                        FbxErrorKind::InvalidStructure,
                        "同じモデルのバインド行列が一致しない",
                    )
                    .with_object(object));
                }
                Some(_) => {}
                None => {
                    by_model.insert(node.model, matrix);
                }
            }
        }
        let mut nodes: Vec<BindPoseNode> = by_model
            .into_iter()
            .map(|(model, matrix)| BindPoseNode {
                model: ObjectId(model),
                matrix,
            })
            .collect();
        // モデルIDの昇順で返す。
        nodes.sort_by_key(|node| node.model.value());
        Ok(nodes)
    }

    /// 指定IDのスキンのクラスタごとにバインド時行列を展開する。
    ///
    /// 順序は [`Skin::clusters`] と同じである。`Transform`・`TransformLink` を
    /// 持たないクラスタは `None` として返す。行列の中身は対象分だけ展開する。
    ///
    /// # Errors
    ///
    /// 未知ID・行列の要素数が16でない・有限でない場合に返す。
    pub fn cluster_bind_matrices(&self, id: ObjectId) -> Result<Vec<ClusterBindMatrix>, FbxError> {
        let skin = self
            .skin(id)
            .ok_or_else(|| FbxError::new(FbxErrorKind::NotFound, "スキンがない").with_object(id))?;
        skin.clusters
            .iter()
            .map(|cluster| {
                Ok(ClusterBindMatrix {
                    cluster: cluster.id,
                    joint: cluster.joint,
                    transform: expand_bind_matrix(self.bytes, cluster.transform, cluster.id)?,
                    transform_link: expand_bind_matrix(
                        self.bytes,
                        cluster.transform_link,
                        cluster.id,
                    )?,
                })
            })
            .collect()
    }
}

/// バインド時行列の配列位置を展開する。位置が無い場合は `None` を返す。
///
/// 要素数が16でない場合や非有限値の場合は構造破損として失敗する。
fn expand_bind_matrix(
    bytes: &[u8],
    desc: Option<ArrayDesc>,
    object: ObjectId,
) -> Result<Option<[f64; 16]>, FbxError> {
    let Some(desc) = desc else {
        return Ok(None);
    };
    let plain = expand(bytes, desc, object)?;
    let values = mesh::as_f64s(plain.as_slice(), desc.kind, object)?;
    if values.len() != 16 {
        return Err(FbxError::new(
            FbxErrorKind::InvalidStructure,
            "バインド時行列が16要素でない",
        )
        .with_object(object));
    }
    if !values.iter().all(|v| v.is_finite()) {
        return Err(FbxError::new(
            FbxErrorKind::InvalidStructure,
            "有限でないバインド時行列がある",
        )
        .with_object(object));
    }
    let matrix: [f64; 16] = values.try_into().map_err(|_| {
        FbxError::new(
            FbxErrorKind::InvalidStructure,
            "バインド時行列が16要素でない",
        )
        .with_object(object)
    })?;
    Ok(Some(matrix))
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
            materials: Vec::new(),
            transform,
            inherit_type,
        });
    }
    Ok(models)
}

/// `Model` の子 `Properties70` からローカル変換を読む。
///
/// `Lcl Translation`・`Lcl Rotation`・`Lcl Scaling`・回転と拡大縮小の
/// オフセットとピボット・事前事後回転・幾何変換・`InheritType`・`RotationActive`
/// を受け付け、1つの行列への合成は [`LocalTransform::local_matrix`] が行う。
/// `RotationOrder` だけは0（XYZ順）の場合に限り、既定の解釈と一致するため通す。
/// それ以外の値は、黙って落とすと誤った姿勢を返すため明示的に失敗する。
fn read_local(node: &RawNode) -> Result<(LocalTransform, i32), FbxError> {
    let id = node.props.first().and_then(RawProp::as_int).unwrap_or(0);
    let object = ObjectId(id);
    let mut transform = LocalTransform::identity();
    let mut inherit_type = 1;
    let Some(props) = node.child("Properties70") else {
        return Ok((transform, inherit_type));
    };
    for node in props.children_by("P") {
        let Some(name) = node.props.first().and_then(RawProp::as_bytes) else {
            continue;
        };
        match name {
            b"Lcl Translation" => transform.translation = read_triplet(node, object)?,
            b"Lcl Rotation" => transform.rotation = read_triplet(node, object)?,
            b"Lcl Scaling" => transform.scale = read_triplet(node, object)?,
            b"RotationOffset" => transform.rotation_offset = read_triplet(node, object)?,
            b"RotationPivot" => transform.rotation_pivot = read_triplet(node, object)?,
            b"PreRotation" => transform.pre_rotation = read_triplet(node, object)?,
            b"PostRotation" => transform.post_rotation = read_triplet(node, object)?,
            b"ScalingOffset" => transform.scaling_offset = read_triplet(node, object)?,
            b"ScalingPivot" => transform.scaling_pivot = read_triplet(node, object)?,
            b"GeometricTranslation" => {
                transform.geometric_translation = read_triplet(node, object)?;
            }
            b"GeometricRotation" => {
                transform.geometric_rotation = read_triplet(node, object)?;
            }
            b"GeometricScaling" => {
                transform.geometric_scaling = read_triplet(node, object)?;
            }
            b"RotationActive" => {
                transform.rotation_active = read_active_flag(node, object)?;
            }
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
                if !(0..=2).contains(&inherit_type) {
                    return Err(FbxError::new(
                        FbxErrorKind::UnsupportedFeature,
                        "非対応の継承方式である",
                    )
                    .with_object(object));
                }
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
    transform.ensure_finite().map_err(|_| {
        // 対象モデルを添えるため、ここで作り直す。
        FbxError::new(FbxErrorKind::InvalidStructure, "有限でない変換値がある").with_object(object)
    })?;
    Ok((transform, inherit_type))
}

/// `P` の `[4]` から真偽値を読む。真偽値・整数のどちらでも受け付ける。
fn read_active_flag(node: &RawNode, object: ObjectId) -> Result<bool, FbxError> {
    let values = node.props.get(4..).unwrap_or_default();
    match values {
        [prop] => match prop.as_int() {
            Some(0) => Ok(false),
            Some(_) => Ok(true),
            None => Err(
                FbxError::new(FbxErrorKind::InvalidStructure, "真偽値の形状が壊れている")
                    .with_object(object),
            ),
        },
        _ => Err(
            FbxError::new(FbxErrorKind::InvalidStructure, "真偽値の形状が壊れている")
                .with_object(object),
        ),
    }
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

/// `Objects` 直下の `Material` を読む。
///
/// IDと名前だけを保持し、色・テクスチャ・シェーダは読まない。
/// IDが0以下や重複の場合は破損として失敗する。
fn read_material_objects(top: &[RawNode]) -> Result<Vec<Material>, FbxError> {
    let mut materials = Vec::new();
    let mut seen: HashSet<i64> = HashSet::new();
    let Some(objects) = top.iter().find(|node| node.name.as_ref() == "Objects") else {
        return Ok(materials);
    };
    for node in objects.children_by("Material") {
        let id = node
            .props
            .first()
            .and_then(RawProp::as_int)
            .ok_or_else(|| {
                FbxError::new(
                    FbxErrorKind::InvalidStructure,
                    "MaterialのID形状が壊れている",
                )
            })?;
        if id <= 0 {
            return Err(
                FbxError::new(FbxErrorKind::InvalidStructure, "MaterialのIDが0以下である")
                    .with_object(ObjectId(id)),
            );
        }
        if !seen.insert(id) {
            return Err(FbxError::new(
                FbxErrorKind::InvalidStructure,
                "MaterialのIDが重複している",
            )
            .with_object(ObjectId(id)));
        }
        let object = ObjectId(id);
        let name = split_class_name(node.props.get(1).and_then(RawProp::as_bytes), object)?;
        materials.push(Material { id: object, name });
    }
    Ok(materials)
}

/// `Objects` 直下の `Deformer`（`Skin`/`Cluster`/`BlendShape`/`BlendShapeChannel`）を読む。
///
/// IDが0以下や重複の場合は破損として失敗する。後勝ちで黙って上書きすると
/// 接続の対応付けがずれるためである。上記4種以外はこの層で読まないため読み飛ばす。
fn read_deformers(top: &[RawNode]) -> Result<Vec<DeformerRec>, FbxError> {
    let mut deformers = Vec::new();
    let mut seen: HashSet<i64> = HashSet::new();
    let Some(objects) = top.iter().find(|node| node.name.as_ref() == "Objects") else {
        return Ok(deformers);
    };
    for node in objects.children_by("Deformer") {
        let id = node
            .props
            .first()
            .and_then(RawProp::as_int)
            .ok_or_else(|| {
                FbxError::new(
                    FbxErrorKind::InvalidStructure,
                    "DeformerのID形状が壊れている",
                )
            })?;
        if id <= 0 {
            return Err(
                FbxError::new(FbxErrorKind::InvalidStructure, "DeformerのIDが0以下である")
                    .with_object(ObjectId(id)),
            );
        }
        if !seen.insert(id) {
            return Err(FbxError::new(
                FbxErrorKind::InvalidStructure,
                "DeformerのIDが重複している",
            )
            .with_object(ObjectId(id)));
        }
        let head = node
            .props
            .get(2)
            .and_then(RawProp::as_bytes)
            .map(|bytes| bytes.split(|b| *b == 0).next().unwrap_or_default());
        let kind = if head == Some(b"Skin".as_slice()) {
            DeformerKind::Skin
        } else if head == Some(b"Cluster".as_slice()) {
            DeformerKind::Cluster
        } else if head == Some(b"BlendShape".as_slice()) {
            DeformerKind::BlendShape
        } else if head == Some(b"BlendShapeChannel".as_slice()) {
            DeformerKind::BlendShapeChannel
        } else {
            continue;
        };
        let object = ObjectId(id);
        let name = split_class_name(node.props.get(1).and_then(RawProp::as_bytes), object)?
            .into_boxed_str();
        let indexes = node
            .child("Indexes")
            .and_then(|child| child.props.first())
            .and_then(RawProp::as_array);
        let weights = node
            .child("Weights")
            .and_then(|child| child.props.first())
            .and_then(RawProp::as_array);
        // クラスタのバインド時行列（変形対象の世界行列と結合モデルの世界行列）は、
        // `BindPose` に無い結合モデルを補うために位置だけ残す。
        // 値の解釈（どちらを正とするか）は呼び出し側に残す。
        let (transform, transform_link) = if kind == DeformerKind::Cluster {
            (
                node.child("Transform")
                    .and_then(|child| child.props.first())
                    .and_then(RawProp::as_array),
                node.child("TransformLink")
                    .and_then(|child| child.props.first())
                    .and_then(RawProp::as_array),
            )
        } else {
            (None, None)
        };
        // ブレンドシェイプ系は `Skin`/`Cluster` の配列を持たず、
        // チャンネルの `DeformPercent` と `FullWeights` だけを保持する。
        // 値の解釈（単一フレーム条件）は呼び出し側に残す。
        // `DeformPercent` は `D` 型の数値だけを受け付け、型違いは黙って落とさず
        // 存在しないものとして扱う。非有限の値はここでは拒否し、
        // 選択時に理由を明示して失敗させる。
        let deform_percent = if kind == DeformerKind::BlendShapeChannel {
            node.child("DeformPercent")
                .and_then(|child| child.props.first())
                .and_then(RawProp::as_number)
        } else {
            None
        };
        let full_weights = if kind == DeformerKind::BlendShapeChannel {
            node.child("FullWeights")
                .and_then(|child| child.props.first())
                .and_then(RawProp::as_array)
        } else {
            None
        };
        deformers.push(DeformerRec {
            id: object,
            name,
            kind,
            indexes,
            weights,
            transform,
            transform_link,
            deform_percent,
            full_weights,
        });
    }
    Ok(deformers)
}

/// `Geometry`（`Shape`）1件の読み取り結果。
///
/// 形状のデルタ配列は展開せず位置だけを残し、
/// [`shape_data`](FbxFile::shape_data) が対象分だけ展開する。
/// 法線の差分は読まない。
#[derive(Debug, Clone)]
struct ShapeRec {
    /// オブジェクトID。
    id: ObjectId,
    /// 形状名。
    name: String,
    /// `Indexes` の位置。
    indexes: ArrayDesc,
    /// `Vertices` の位置。
    vertices: ArrayDesc,
}

/// `Objects` 直下の `Geometry`（`Shape`）を読む。
///
/// `Vertices`（差分）と `Indexes`（制御点番号）を持ち、両方が無い形状は
/// 破損として失敗する。IDが0以下や重複の場合も破損として失敗する。
fn read_shapes(bytes: &[u8], top: &[RawNode]) -> Result<Vec<ShapeRec>, FbxError> {
    let _ = bytes;
    let mut shapes = Vec::new();
    let mut seen: HashSet<i64> = HashSet::new();
    let Some(objects) = top.iter().find(|node| node.name.as_ref() == "Objects") else {
        return Ok(shapes);
    };
    for node in objects.children_by("Geometry") {
        // `Shape` 以外は読まない。比較はバイト列で行い、名前のUTF-8検証は後段で行う。
        let is_shape = node
            .props
            .get(2)
            .and_then(RawProp::as_bytes)
            .map(|bytes| bytes.split(|b| *b == 0).next() == Some(b"Shape".as_slice()))
            .unwrap_or(false);
        if !is_shape {
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
        let indexes = node
            .child("Indexes")
            .and_then(|child| child.props.first())
            .and_then(RawProp::as_array)
            .ok_or_else(|| {
                FbxError::new(FbxErrorKind::InvalidStructure, "形状の索引配列がない")
                    .with_object(object)
            })?;
        let vertices = node
            .child("Vertices")
            .and_then(|child| child.props.first())
            .and_then(RawProp::as_array)
            .ok_or_else(|| {
                FbxError::new(FbxErrorKind::InvalidStructure, "形状の差分配列がない")
                    .with_object(object)
            })?;
        shapes.push(ShapeRec {
            id: object,
            name,
            indexes,
            vertices,
        });
    }
    Ok(shapes)
}

/// `Connections` の `OO`（子→親）からブレンドシェイプの対応を組み立てる。
///
/// 読むのは `Shape`→`BlendShapeChannel`・`BlendShapeChannel`→`BlendShape`・
/// `BlendShape`→`Geometry(Mesh)` だけであり、出現順を保つ。
/// 未知IDへの接続は無視する。1つの `BlendShape` が複数のメッシュに接続する場合は
/// 変形対象が決まらないため不正として失敗する。
/// 1つのチャンネルへの複数形状の接続は許し、単一フレーム条件の検証は
/// 形状の選択時に呼び出し側が行う。
/// ブレンドシェイプの接続解決結果。`BlendShape` 一覧、チャンネル一覧、
/// チャンネルごとの形状ID列（出現順）を返す。
type WiredBlendShapes = (
    Vec<BlendShape>,
    Vec<BlendChannelRec>,
    HashMap<i64, Vec<ObjectId>>,
);

/// 形状IDから変形対象メッシュへの対応を作る。
///
/// 1つの形状は1つのチャンネルにだけ接続できる。同じ形状が複数チャンネルへ
/// 接続する場合や、1つのチャンネルが異なるメッシュの `BlendShape` に属する場合は、
/// 対象メッシュが一意に決まらないため破損として失敗する。
fn build_shape_meshes(
    channel_shapes: &HashMap<i64, Vec<ObjectId>>,
    blend_shapes: &[BlendShape],
) -> Result<HashMap<i64, ObjectId>, FbxError> {
    let mut channel_geometry: HashMap<i64, ObjectId> = HashMap::new();
    for blend in blend_shapes {
        for channel in &blend.channels {
            match channel_geometry.get(&channel.value()) {
                Some(previous) if *previous != blend.geometry => {
                    return Err(FbxError::new(
                        FbxErrorKind::InvalidStructure,
                        "チャンネルが複数のメッシュに接続する",
                    )
                    .with_object(*channel));
                }
                Some(_) => {}
                None => {
                    channel_geometry.insert(channel.value(), blend.geometry);
                }
            }
        }
    }
    let mut shape_channel: HashMap<i64, i64> = HashMap::new();
    for (channel, shapes) in channel_shapes {
        for shape in shapes {
            match shape_channel.get(&shape.value()) {
                Some(previous) if *previous != *channel => {
                    return Err(FbxError::new(
                        FbxErrorKind::InvalidStructure,
                        "形状が複数のチャンネルに接続する",
                    )
                    .with_object(*shape));
                }
                Some(_) => {}
                None => {
                    shape_channel.insert(shape.value(), *channel);
                }
            }
        }
    }
    let mut shape_mesh = HashMap::with_capacity(shape_channel.len());
    for (shape, channel) in shape_channel {
        if let Some(geometry) = channel_geometry.get(&channel) {
            shape_mesh.insert(shape, *geometry);
        }
    }
    Ok(shape_mesh)
}

fn wire_blend_shapes(
    top: &[RawNode],
    meshes: &[Mesh],
    deformers: &[DeformerRec],
) -> Result<WiredBlendShapes, FbxError> {
    let known_meshes: HashSet<i64> = meshes.iter().map(|mesh| mesh.id.value()).collect();
    let mut blend_ids: HashSet<i64> = HashSet::new();
    let mut channel_of: HashMap<i64, &DeformerRec> = HashMap::new();
    for rec in deformers {
        match rec.kind {
            DeformerKind::BlendShape => {
                blend_ids.insert(rec.id.value());
            }
            DeformerKind::BlendShapeChannel => {
                channel_of.insert(rec.id.value(), rec);
            }
            DeformerKind::Skin | DeformerKind::Cluster => {}
        }
    }
    // 対応付けと順序記録を一度の走査で行う。同一対応の重複は無視し、
    // 異なる対象が複数ある場合だけ後段で拒否する。
    let mut channel_shapes: HashMap<i64, Vec<i64>> = HashMap::new();
    let mut blend_channels: HashMap<i64, Vec<i64>> = HashMap::new();
    let mut blend_geometry: HashMap<i64, Vec<i64>> = HashMap::new();
    if let Some(connections) = top.iter().find(|node| node.name.as_ref() == "Connections") {
        for node in &connections.children {
            if node.props.first().and_then(RawProp::as_bytes) != Some(b"OO".as_slice()) {
                continue;
            }
            let (Some(child), Some(parent)) = (
                node.props.get(1).and_then(RawProp::as_int),
                node.props.get(2).and_then(RawProp::as_int),
            ) else {
                continue;
            };
            if channel_of.contains_key(&parent) {
                // 子は形状のはずだが、未知IDへの接続は無視するため種類を問わず記録し、
                // 実在の形状かは後段の `read_shapes` との突合せで決める。
                let frames = channel_shapes.entry(parent).or_default();
                if !frames.contains(&child) {
                    frames.push(child);
                }
            } else if blend_ids.contains(&parent) && channel_of.contains_key(&child) {
                let channels = blend_channels.entry(parent).or_default();
                if !channels.contains(&child) {
                    channels.push(child);
                }
            } else if known_meshes.contains(&parent) && blend_ids.contains(&child) {
                let targets = blend_geometry.entry(child).or_default();
                if !targets.contains(&parent) {
                    targets.push(parent);
                }
            }
        }
    }
    let mut summaries = Vec::new();
    for rec in deformers {
        if rec.kind != DeformerKind::BlendShape {
            continue;
        }
        let geometry = match blend_geometry.get(&rec.id.value()).map(Vec::as_slice) {
            Some(&[geometry]) => geometry,
            _ => {
                return Err(FbxError::new(
                    FbxErrorKind::InvalidStructure,
                    "ブレンドシェイプの変形対象が1つでない",
                )
                .with_object(rec.id));
            }
        };
        let mut channels = Vec::new();
        if let Some(ordered) = blend_channels.get(&rec.id.value()) {
            for &channel_id in ordered {
                channels.push(ObjectId(channel_id));
            }
        }
        summaries.push(BlendShape {
            id: rec.id,
            name: rec.name.to_string(),
            geometry: ObjectId(geometry),
            channels,
        });
    }
    let mut channels_out = Vec::new();
    for rec in deformers {
        if rec.kind != DeformerKind::BlendShapeChannel {
            continue;
        }
        channels_out.push(BlendChannelRec {
            id: rec.id,
            name: rec.name.to_string(),
            deform_percent: rec.deform_percent,
            full_weights: rec.full_weights,
        });
    }
    let mut frames_out: HashMap<i64, Vec<ObjectId>> = HashMap::new();
    for (channel, frames) in channel_shapes {
        frames_out.insert(channel, frames.into_iter().map(ObjectId).collect());
    }
    Ok((summaries, channels_out, frames_out))
}

/// `Objects` 直下の `Pose`（`BindPose`）から `PoseNode` を読む。
///
/// `BindPose` 以外はこの層で読まないため読み飛ばす。行列は展開せず位置だけを残す。
/// `Node` が指すモデルIDの実在は展開時に確かめ、未知のモデルは無視する。
fn read_pose_nodes(top: &[RawNode]) -> Result<(Vec<StoredPoseNode>, Vec<i64>), FbxError> {
    let mut out = Vec::new();
    let mut ids = Vec::new();
    // 同一IDの `Pose` が二重に現れた場合は後勝ちにせず破損とする。
    let mut seen: HashSet<i64> = HashSet::new();
    let Some(objects) = top.iter().find(|node| node.name.as_ref() == "Objects") else {
        return Ok((out, ids));
    };
    for node in objects.children_by("Pose") {
        let id = node
            .props
            .first()
            .and_then(RawProp::as_int)
            .ok_or_else(|| {
                FbxError::new(FbxErrorKind::InvalidStructure, "PoseのID形状が壊れている")
            })?;
        if id <= 0 {
            return Err(
                FbxError::new(FbxErrorKind::InvalidStructure, "PoseのIDが0以下である")
                    .with_object(ObjectId(id)),
            );
        }
        if !seen.insert(id) {
            return Err(
                FbxError::new(FbxErrorKind::InvalidStructure, "PoseのIDが重複している")
                    .with_object(ObjectId(id)),
            );
        }
        ids.push(id);
        let object = ObjectId(id);
        let is_bind = node
            .props
            .get(2)
            .and_then(RawProp::as_bytes)
            .map(|bytes| bytes.split(|b| *b == 0).next() == Some(b"BindPose".as_slice()))
            .unwrap_or(false);
        if !is_bind {
            continue;
        }
        for pose_node in node.children_by("PoseNode") {
            let model = pose_node
                .child("Node")
                .and_then(|child| child.props.first())
                .and_then(RawProp::as_int)
                .ok_or_else(|| {
                    FbxError::new(
                        FbxErrorKind::InvalidStructure,
                        "PoseNodeの対象形状が壊れている",
                    )
                    .with_object(object)
                })?;
            let matrix = pose_node
                .child("Matrix")
                .and_then(|child| child.props.first())
                .and_then(RawProp::as_array)
                .ok_or_else(|| {
                    FbxError::new(FbxErrorKind::InvalidStructure, "PoseNodeの行列がない")
                        .with_object(object)
                })?;
            out.push(StoredPoseNode { model, matrix });
        }
    }
    Ok((out, ids))
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
/// 子=`Material`・親=`Model` の接続は材質スロットとして接続順に保持し、
/// 同一接続の重複は無視する。材質は複数Modelに共有されてよい。
fn apply_connections(
    top: &[RawNode],
    models: &mut [Model],
    meshes: &mut [Mesh],
    materials: &[Material],
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
    let known_materials: HashSet<i64> = materials
        .iter()
        .map(|material| material.id.value())
        .collect();
    // モデルIDと材質IDの対応をO(1)で重複判定するための集合。順序は `Model::materials` が保つ。
    // `Model::materials` の線形走査では接続数に対してO(n^2)になるためである。
    let mut material_seen: HashSet<(i64, i64)> = HashSet::new();
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
        } else if known_materials.contains(&child) && known_models.contains(&parent) {
            // 材質スロットは接続順に積み、同一接続の重複は無視する。
            // 判定は対応集合で行い、順序は `Model::materials` が保つ。
            // 材質は複数Modelに共有されてよく、多重所有として扱わない。
            if material_seen.insert((parent, child)) {
                let parent_at = at_model[&parent];
                models[parent_at].materials.push(ObjectId(child));
            }
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

/// `Connections` の `OO`（子→親）からスキンとクラスタの対応を組み立てる。
///
/// 読むのは `Skin`→`Geometry`・`Cluster`→`Skin`・`Model`→`Cluster` だけである。
/// 未知IDへの接続は無視する。スキンに属さないクラスタは無視する。
/// クラスタの順序は接続順を保つ。変形対象が0個・複数のスキン、
/// 所有者のない変形対象、結合モデルが0個・複数のクラスタ、
/// 結合モデルが重複するスキンは不正として失敗する。
fn wire_skins(
    top: &[RawNode],
    models: &[Model],
    meshes: &[Mesh],
    deformers: &[DeformerRec],
) -> Result<Vec<Skin>, FbxError> {
    let known_models: HashSet<i64> = models.iter().map(|model| model.id.value()).collect();
    let known_meshes: HashSet<i64> = meshes.iter().map(|mesh| mesh.id.value()).collect();
    let mut skin_ids: HashSet<i64> = HashSet::new();
    let mut cluster_of: HashMap<i64, &DeformerRec> = HashMap::new();
    for rec in deformers {
        match rec.kind {
            DeformerKind::Skin => {
                skin_ids.insert(rec.id.value());
            }
            DeformerKind::Cluster => {
                cluster_of.insert(rec.id.value(), rec);
            }
            // ブレンドシェイプ系はスキンの配線では扱わない。
            DeformerKind::BlendShape | DeformerKind::BlendShapeChannel => {}
        }
    }
    // Geometryから所有モデルへの対応。複数所有は `apply_connections` で先に拒否するため、
    // ここでは1対1である。
    let mut geometry_owner: HashMap<i64, i64> = HashMap::new();
    for model in models {
        if let Some(mesh) = model.mesh {
            geometry_owner.insert(mesh.value(), model.id.value());
        }
    }
    // 対応付けと順序記録を一度の走査で行い、O(n^2)を避ける。
    let mut skin_geometry: HashMap<i64, Vec<i64>> = HashMap::new();
    let mut cluster_skin: HashMap<i64, i64> = HashMap::new();
    let mut cluster_joints: HashMap<i64, Vec<i64>> = HashMap::new();
    let mut skin_clusters: HashMap<i64, Vec<i64>> = HashMap::new();
    if let Some(connections) = top.iter().find(|node| node.name.as_ref() == "Connections") {
        for node in &connections.children {
            if node.props.first().and_then(RawProp::as_bytes) != Some(b"OO".as_slice()) {
                continue;
            }
            let (Some(child), Some(parent)) = (
                node.props.get(1).and_then(RawProp::as_int),
                node.props.get(2).and_then(RawProp::as_int),
            ) else {
                continue;
            };
            if skin_ids.contains(&child) && known_meshes.contains(&parent) {
                // 同一対応の重複は無視し、異なる対象だけを残して後段で拒否する。
                let targets = skin_geometry.entry(child).or_default();
                if !targets.contains(&parent) {
                    targets.push(parent);
                }
            } else if cluster_of.contains_key(&child) && skin_ids.contains(&parent) {
                match cluster_skin.get(&child) {
                    Some(&prev) if prev != parent => {
                        return Err(FbxError::new(
                            FbxErrorKind::InvalidStructure,
                            "クラスタが複数のスキンに接続する",
                        )
                        .with_object(ObjectId(child)));
                    }
                    Some(_) => {}
                    None => {
                        cluster_skin.insert(child, parent);
                        skin_clusters.entry(parent).or_default().push(child);
                    }
                }
            } else if known_models.contains(&child) && cluster_of.contains_key(&parent) {
                // 同一対応の重複は無視し、異なる結合だけを残して後段で拒否する。
                // `Cluster→Skin` と同じ方針である。
                let joints = cluster_joints.entry(parent).or_default();
                if !joints.contains(&child) {
                    joints.push(child);
                }
            }
        }
    }
    let mut skins = Vec::new();
    for rec in deformers {
        if rec.kind != DeformerKind::Skin {
            continue;
        }
        let geometry = match skin_geometry.get(&rec.id.value()).map(Vec::as_slice) {
            Some(&[geometry]) => geometry,
            _ => {
                return Err(FbxError::new(
                    FbxErrorKind::InvalidStructure,
                    "スキンの変形対象が1つでない",
                )
                .with_object(rec.id));
            }
        };
        let Some(&owner) = geometry_owner.get(&geometry) else {
            return Err(FbxError::new(
                FbxErrorKind::InvalidStructure,
                "変形対象を所有するモデルがない",
            )
            .with_object(rec.id));
        };
        let mut seen_joints: HashSet<i64> = HashSet::new();
        let mut clusters = Vec::new();
        // クラスタの順序は接続順を保つ。
        if let Some(ordered) = skin_clusters.get(&rec.id.value()) {
            for &cluster_id in ordered {
                let cluster = cluster_of[&cluster_id];
                let joint = match cluster_joints.get(&cluster_id).map(Vec::as_slice) {
                    Some(&[joint]) => joint,
                    _ => {
                        return Err(FbxError::new(
                            FbxErrorKind::InvalidStructure,
                            "クラスタの結合モデルが1つでない",
                        )
                        .with_object(cluster.id));
                    }
                };
                if !seen_joints.insert(joint) {
                    return Err(FbxError::new(
                        FbxErrorKind::InvalidStructure,
                        "1つのスキン内で結合モデルが重複する",
                    )
                    .with_object(cluster.id));
                }
                let influence_count = match (cluster.indexes, cluster.weights) {
                    (Some(indexes), Some(_)) => indexes.count,
                    _ => 0,
                };
                clusters.push(Cluster {
                    id: cluster.id,
                    joint: ObjectId(joint),
                    influence_count,
                    indexes: cluster.indexes,
                    weights: cluster.weights,
                    transform: cluster.transform,
                    transform_link: cluster.transform_link,
                });
            }
        }
        skins.push(Skin {
            id: rec.id,
            name: rec.name.to_string(),
            geometry: ObjectId(geometry),
            model: ObjectId(owner),
            clusters,
        });
    }
    Ok(skins)
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

/// クラスタの索引と重みを展開し、範囲・重複・有限性を確かめる。
///
/// 索引は変形対象Geometryの制御点番号であり、頂点数未満であることを確かめる。
/// 重みの範囲と総和は検査しない。実データに負値や総和が1でない頂点が含まれ、
/// これらは破損ではなく入力の性質であるため、生成側が正規化する。
fn read_cluster_weights(
    bytes: &[u8],
    id: ObjectId,
    vertex_count: usize,
    indexes: ArrayDesc,
    weights: ArrayDesc,
) -> Result<(Vec<u32>, Vec<f64>), FbxError> {
    let raw_indexes = expand_integers(bytes, indexes, id)?;
    let plain = expand(bytes, weights, id)?;
    let raw_weights = mesh::as_f64s(plain.as_slice(), weights.kind, id)?;
    if raw_indexes.len() != raw_weights.len() {
        return Err(FbxError::new(
            FbxErrorKind::InvalidStructure,
            "IndexesとWeightsの長さが一致しない",
        )
        .with_object(id));
    }
    if !raw_weights.iter().all(|v| v.is_finite()) {
        return Err(
            FbxError::new(FbxErrorKind::InvalidStructure, "有限でない重みがある").with_object(id),
        );
    }
    let mut seen: HashSet<u32> = HashSet::with_capacity(raw_indexes.len());
    let mut vertex_indices = Vec::with_capacity(raw_indexes.len());
    for &index in &raw_indexes {
        if index < 0 || index as usize >= vertex_count {
            return Err(
                FbxError::new(FbxErrorKind::InvalidStructure, "頂点番号が範囲外である")
                    .with_object(id),
            );
        }
        let index = index as u32;
        if !seen.insert(index) {
            return Err(FbxError::new(
                FbxErrorKind::InvalidStructure,
                "クラスタ内で頂点索引が重複する",
            )
            .with_object(id));
        }
        vertex_indices.push(index);
    }
    Ok((vertex_indices, raw_weights))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn 束ね(id: i64, geometry: i64, channels: &[i64]) -> BlendShape {
        BlendShape {
            id: ObjectId::new(id),
            name: format!("b{id}"),
            geometry: ObjectId::new(geometry),
            channels: channels
                .iter()
                .map(|channel| ObjectId::new(*channel))
                .collect(),
        }
    }

    #[test]
    fn 形状が複数チャンネルに接続すると失敗する() {
        let mut channel_shapes = HashMap::new();
        channel_shapes.insert(30, vec![ObjectId::new(20)]);
        channel_shapes.insert(31, vec![ObjectId::new(20)]);
        let blend_shapes = vec![束ね(40, 10, &[30]), 束ね(41, 11, &[31])];
        assert!(build_shape_meshes(&channel_shapes, &blend_shapes).is_err());
    }

    #[test]
    fn チャンネルが複数メッシュに接続すると失敗する() {
        let mut channel_shapes = HashMap::new();
        channel_shapes.insert(30, vec![ObjectId::new(20)]);
        let blend_shapes = vec![束ね(40, 10, &[30]), 束ね(41, 11, &[30])];
        assert!(build_shape_meshes(&channel_shapes, &blend_shapes).is_err());
    }
}
