//! Unityヘルパが出力する物理sidecarの読み取りと対応解決。
//!
//! 入口は [`PhysicsSidecar::parse`]（バイト列の検証）と
//! [`VrmModel::resolve_physics_sidecar`](crate::vrm::VrmModel::resolve_physics_sidecar)
//! （`VrmModel` のノードへの対応付け）である。スプリングチェーンの
//! コンパイルやVRM出力は次の作業単位であり、ここでは行わない。

use std::collections::{HashMap, HashSet};

use serde::Deserialize;

use super::VrmModel;
use super::error::{VrmError, VrmErrorKind};
use super::matrix;

/// sidecar JSONの書式名。
const SIDECAR_FORMAT: &str = "vrctom.physics";
/// 対応するsidecarの版。
const SIDECAR_SCHEMA_VERSION: u32 = 1;
/// 参照姿勢の方針。今回は編集時だけ受け付ける。
const STATE_POLICY_EDIT_MODE: &str = "edit-mode";
/// 座標空間の宣言。Unityアバタールート基準・メートルである。
const COORDINATE_SPACE: &str = "unity-avatar-root-local-meters";

/// sidecar全体の資源上限。
///
/// フィールドは非公開で、既定値は [`Default`] が設定する。
/// 上限の変更は `with_` で始まるメソッドで行う。
#[derive(Debug, Clone)]
pub struct PhysicsLimits {
    max_json_bytes: usize,
    max_nodes: usize,
    max_phys_bones: usize,
    max_colliders: usize,
    max_curve_keys: usize,
    max_source_models: usize,
}

impl PhysicsLimits {
    /// 既定の上限を持つ設定を生成する。
    pub fn new() -> Self {
        Self {
            // 実データの数百KBが収まり、破損由来の膨張を抑える大きさである。
            max_json_bytes: 16 << 20,
            // 実データの数千ノードが収まる大きさである。
            max_nodes: 100_000,
            // 実データの数十件が収まる大きさである。
            max_phys_bones: 10_000,
            // 実データの十数件が収まる大きさである。
            max_colliders: 10_000,
            // 全カーブのキー総数の上限である。
            max_curve_keys: 1_000_000,
            // 実データの1件が収まる大きさである。
            max_source_models: 64,
        }
    }

    /// JSONバイト数の上限を変更する。
    pub fn with_max_json_bytes(mut self, value: usize) -> Self {
        self.max_json_bytes = value;
        self
    }

    /// ノード数の上限を変更する。
    pub fn with_max_nodes(mut self, value: usize) -> Self {
        self.max_nodes = value;
        self
    }

    /// PhysBone数の上限を変更する。
    pub fn with_max_phys_bones(mut self, value: usize) -> Self {
        self.max_phys_bones = value;
        self
    }

    /// collider数の上限を変更する。
    pub fn with_max_colliders(mut self, value: usize) -> Self {
        self.max_colliders = value;
        self
    }

    /// カーブキー総数の上限を変更する。
    pub fn with_max_curve_keys(mut self, value: usize) -> Self {
        self.max_curve_keys = value;
        self
    }
}

impl Default for PhysicsLimits {
    fn default() -> Self {
        Self::new()
    }
}

/// VRMのスプリングボーン全体。今回は空のまま置き、出力しない。
#[derive(Debug, Clone, Default)]
pub struct VrmSpringBones {
    /// スプリング列。今回は空である。
    pub springs: Vec<VrmSpring>,
    /// collider列。今回は空である。
    pub colliders: Vec<VrmSpringCollider>,
    /// colliderグループ列。今回は空である。
    pub collider_groups: Vec<VrmSpringColliderGroup>,
}

/// スプリング1件。今回は作らず、型だけ用意する。
#[derive(Debug, Clone, Default)]
pub struct VrmSpring {
    /// スプリング名。
    pub name: String,
    /// 関節列。
    pub joints: Vec<VrmSpringJoint>,
    /// 中心ノードの索引。無い場合は `None` である。
    pub center: Option<usize>,
    /// 参照するcolliderグループの索引列。
    pub collider_groups: Vec<usize>,
}

/// スプリング関節1件。
#[derive(Debug, Clone)]
pub struct VrmSpringJoint {
    /// 対象ノードの索引。
    pub node: usize,
    /// 当たり半径。
    pub hit_radius: f32,
    /// 硬さ。
    pub stiffness: f32,
    /// 重力の強さ。
    pub gravity_power: f32,
    /// 重力の向き。
    pub gravity_dir: [f32; 3],
    /// 抵抗。
    pub drag_force: f32,
}

/// スプリングcollider1件。
#[derive(Debug, Clone)]
pub struct VrmSpringCollider {
    /// 対象ノードの索引。
    pub node: usize,
    /// 形状。
    pub shape: VrmSpringColliderShape,
}

/// スプリングcolliderの形状。
#[derive(Debug, Clone)]
pub enum VrmSpringColliderShape {
    /// 球。中心ずれと半径を持つ。
    Sphere {
        /// 中心ずれ。
        offset: [f32; 3],
        /// 半径。
        radius: f32,
    },
    /// カプセル。中心ずれ・半径・末端のずれを持つ。
    Capsule {
        /// 中心ずれ。
        offset: [f32; 3],
        /// 半径。
        radius: f32,
        /// 末端のずれ。
        tail: [f32; 3],
    },
}

/// スプリングcolliderグループ1件。
#[derive(Debug, Clone, Default)]
pub struct VrmSpringColliderGroup {
    /// グループ名。
    pub name: String,
    /// 参照するcolliderの索引列。
    pub colliders: Vec<usize>,
}

/// 物理sidecar全体。検証済みのDTOである。
#[derive(Debug, Clone, Deserialize)]
pub struct PhysicsSidecar {
    /// 書式名。`vrctom.physics` である。
    pub format: String,
    /// 版。`1` である。
    #[serde(rename = "schemaVersion")]
    pub schema_version: u32,
    /// 出力したヘルパの情報。
    pub generator: SidecarGenerator,
    /// 取得時の条件。
    pub capture: SidecarCapture,
    /// 参照元のFBX列。
    #[serde(rename = "sourceModels")]
    pub source_models: Vec<SidecarSourceModel>,
    /// ノード列。
    pub nodes: Vec<SidecarNode>,
    /// PhysBone列。
    #[serde(rename = "physBones")]
    pub phys_bones: Vec<SidecarPhysBone>,
    /// collider列。
    pub colliders: Vec<SidecarCollider>,
    /// 未対応機能の名残。今回は検証せず保持する。
    #[serde(rename = "unsupportedFeatures", default)]
    pub unsupported_features: Vec<String>,
}

/// ヘルパの情報。
#[derive(Debug, Clone, Deserialize)]
pub struct SidecarGenerator {
    /// ヘルパ名。
    pub name: String,
    /// ヘルパの版。
    pub version: String,
    /// Unityの版。無い場合は `None` である。
    #[serde(rename = "unityVersion", default)]
    pub unity_version: Option<String>,
    /// SDKの版。無い場合は `None` である。
    #[serde(rename = "sdkVersion", default)]
    pub sdk_version: Option<String>,
}

/// 取得時の条件。
#[derive(Debug, Clone, Deserialize)]
pub struct SidecarCapture {
    /// 参照姿勢の方針。今回は `edit-mode` だけ受け付ける。
    #[serde(rename = "statePolicy")]
    pub state_policy: String,
    /// 座標空間の宣言。
    #[serde(rename = "coordinateSpace")]
    pub coordinate_space: String,
    /// アバタールートのノードID。
    #[serde(rename = "avatarRootId")]
    pub avatar_root_id: String,
    /// ルートの拡大率。
    #[serde(rename = "rootScale")]
    pub root_scale: [f64; 3],
}

/// 参照元のFBX1件。
#[derive(Debug, Clone, Deserialize)]
pub struct SidecarSourceModel {
    /// 参照ID。
    pub id: String,
    /// FBXバイト列のSHA-256（小文字hex）。
    pub sha256: String,
    /// 資産GUID。
    #[serde(rename = "assetGuid", default)]
    pub asset_guid: Option<String>,
    /// 資産パス。
    #[serde(rename = "assetPath", default)]
    pub asset_path: Option<String>,
}

/// sidecarノード1件。
#[derive(Debug, Clone, Deserialize)]
pub struct SidecarNode {
    /// ノードID（`n:0` など）。
    pub id: String,
    /// 親ノードのID。ルートは `None` である。
    #[serde(rename = "parentId", default)]
    pub parent_id: Option<String>,
    /// ノード名。
    pub name: String,
    /// 兄弟順。
    #[serde(rename = "siblingIndex", default = "default_sibling_index")]
    pub sibling_index: u32,
    /// シーンルートからの名前列。
    #[serde(rename = "scenePath", default)]
    pub scene_path: Vec<String>,
    /// FBX由来の出所。シーン追加ノードは `None` である。
    pub source: Option<SidecarNodeSource>,
    /// 参照姿勢のローカルTRS。
    #[serde(rename = "referenceLocal")]
    pub reference_local: SidecarTrs,
}

/// FBX由来の出所。
#[derive(Debug, Clone, Deserialize)]
pub struct SidecarNodeSource {
    /// 参照モデルのID。
    #[serde(rename = "modelId")]
    pub model_id: String,
    /// FBXルートからの名前列。
    pub path: Vec<String>,
    /// 資産GUID。無い場合は `None` である。
    #[serde(rename = "assetGuid", default)]
    pub asset_guid: Option<String>,
    /// ローカルID。
    #[serde(rename = "localId", default)]
    pub local_id: Option<i64>,
}

/// 参照姿勢のローカルTRS。
#[derive(Debug, Clone, Deserialize)]
pub struct SidecarTrs {
    /// 平行移動。
    pub position: [f64; 3],
    /// 回転（四元数 `[x, y, z, w]`）。
    pub rotation: [f64; 4],
    /// 拡大率。
    pub scale: [f64; 3],
}

/// PhysBone1件。曲線や参照IDは検証済みで保持する。
#[derive(Debug, Clone, Deserialize)]
pub struct SidecarPhysBone {
    /// PhysBoneのID（`pb:0` など）。
    pub id: String,
    /// 付属するノードのID。
    #[serde(rename = "componentNodeId")]
    pub component_node_id: String,
    /// 連鎖の起点ノードのID。
    #[serde(rename = "rootNodeId")]
    pub root_node_id: String,
    /// 有効かどうか。
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// 版（`1.1` など）。
    #[serde(default)]
    pub version: Option<String>,
    /// 積分方式。
    #[serde(rename = "integrationType", default = "default_integration_type")]
    pub integration_type: SidecarIntegrationType,
    /// 無視するノードのID列。
    #[serde(rename = "ignoreNodeIds", default)]
    pub ignore_node_ids: Vec<String>,
    /// 他のPhysBoneを無視するかどうか。
    #[serde(rename = "ignoreOtherPhysBones", default)]
    pub ignore_other_phys_bones: bool,
    /// 末端位置。
    #[serde(rename = "endpointPosition", default = "default_vec3")]
    pub endpoint_position: [f64; 3],
    /// 複数子の扱い。
    #[serde(rename = "multiChildType", default = "default_multi_child_type")]
    pub multi_child_type: SidecarMultiChildType,
    /// 引っ張り。
    #[serde(default = "default_curve")]
    pub pull: SidecarCurve,
    /// ばね。
    #[serde(default = "default_curve")]
    pub spring: SidecarCurve,
    /// 硬さ。
    #[serde(default = "default_curve")]
    pub stiffness: SidecarCurve,
    /// 重力。
    #[serde(default = "default_curve")]
    pub gravity: SidecarCurve,
    /// 重力の減衰。
    #[serde(rename = "gravityFalloff", default = "default_curve")]
    pub gravity_falloff: SidecarCurve,
    /// 不動の種別。
    #[serde(rename = "immobileType", default = "default_immobile_type")]
    pub immobile_type: SidecarImmobileType,
    /// 不動の強さ。
    #[serde(default = "default_curve")]
    pub immobile: SidecarCurve,
    /// 半径。
    #[serde(default = "default_curve")]
    pub radius: SidecarCurve,
    /// 衝突を許すかどうか。
    #[serde(rename = "allowCollision", default = "default_true")]
    pub allow_collision: bool,
    /// 参照するcolliderのID列。
    #[serde(rename = "colliderIds", default)]
    pub collider_ids: Vec<String>,
    /// 制限の種別。
    #[serde(rename = "limitType", default = "default_limit_type")]
    pub limit_type: SidecarLimitType,
    /// X軸の最大角。
    #[serde(rename = "maxAngleX", default = "default_curve")]
    pub max_angle_x: SidecarCurve,
    /// Z軸の最大角。
    #[serde(rename = "maxAngleZ", default = "default_curve")]
    pub max_angle_z: SidecarCurve,
    /// 制限回転。
    #[serde(rename = "limitRotation", default = "default_vec3")]
    pub limit_rotation: [f64; 3],
    /// 動画化されているかどうか。
    #[serde(rename = "isAnimated", default)]
    pub is_animated: bool,
}

/// collider1件。
#[derive(Debug, Clone, Deserialize)]
pub struct SidecarCollider {
    /// colliderのID（`c:0` など）。
    pub id: String,
    /// 付属するノードのID。
    #[serde(rename = "componentNodeId")]
    pub component_node_id: String,
    /// 基準ノードのID。
    #[serde(rename = "rootNodeId")]
    pub root_node_id: String,
    /// 有効かどうか。
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// 形状の種別。
    #[serde(rename = "shapeType")]
    pub shape_type: SidecarShapeType,
    /// 内側判定かどうか。
    #[serde(rename = "insideBounds", default)]
    pub inside_bounds: bool,
    /// 半径。
    #[serde(default)]
    pub radius: f64,
    /// 高さ。
    #[serde(default)]
    pub height: f64,
    /// 位置。
    #[serde(default = "default_vec3")]
    pub position: [f64; 3],
    /// 回転（四元数 `[x, y, z, w]`）。
    #[serde(default = "default_quat")]
    pub rotation: [f64; 4],
    /// 骨を球として扱うかどうか。
    #[serde(rename = "bonesAsSpheres", default)]
    pub bones_as_spheres: bool,
}

/// アニメーションカーブ。
#[derive(Debug, Clone, Deserialize)]
pub struct SidecarCurve {
    /// 既定値。キーが空の場合はこの値を使う。
    #[serde(default)]
    pub value: f64,
    /// キー列。空の場合は既定値を使う。
    #[serde(default)]
    pub keys: Vec<SidecarCurveKey>,
    /// 前側の wrap。
    #[serde(rename = "preWrapMode", default = "default_clamp_wrap")]
    pub pre_wrap_mode: u32,
    /// 後側の wrap。
    #[serde(rename = "postWrapMode", default = "default_clamp_wrap")]
    pub post_wrap_mode: u32,
}

/// カーブのキー1件。
#[derive(Debug, Clone, Deserialize)]
pub struct SidecarCurveKey {
    /// 時刻。
    #[serde(default)]
    pub time: f64,
    /// 値。
    #[serde(default)]
    pub value: f64,
    /// 入り傾き。
    #[serde(rename = "inSlope", default)]
    pub in_slope: f64,
    /// 出傾き。
    #[serde(rename = "outSlope", default)]
    pub out_slope: f64,
    /// 重み付きの扱い。0以外は今回未対応として失敗する。
    #[serde(rename = "weightedMode", default)]
    pub weighted_mode: u32,
    /// 入り重み。
    #[serde(rename = "inWeight", default)]
    pub in_weight: f64,
    /// 出重み。
    #[serde(rename = "outWeight", default)]
    pub out_weight: f64,
}

/// 積分方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum SidecarIntegrationType {
    /// 単純化積分。
    Simplified,
    /// 高精度積分。
    Advanced,
}

/// 複数子の扱い。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum SidecarMultiChildType {
    /// 無視する。
    Ignore,
    /// 先頭だけ使う。
    First,
    /// 平均する。
    Average,
}

/// 不動の種別。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum SidecarImmobileType {
    /// 全動作を止める。
    AllMotion,
    /// 世界固定で止める。
    World,
}

/// 制限の種別。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum SidecarLimitType {
    /// 制限なし。
    None,
    /// 角度制限。
    Angle,
    /// ヒンジ制限。
    Hinge,
    /// 極制限。
    Polar,
}

/// collider形状の種別。平面は今回未対応として失敗する。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum SidecarShapeType {
    /// 球。
    Sphere,
    /// カプセル。
    Capsule,
    /// 平面。参照がある場合は未対応として失敗する。
    Plane,
}

fn default_true() -> bool {
    true
}

fn default_vec3() -> [f64; 3] {
    [0.0, 0.0, 0.0]
}

fn default_quat() -> [f64; 4] {
    [0.0, 0.0, 0.0, 1.0]
}

fn default_curve() -> SidecarCurve {
    SidecarCurve {
        value: 0.0,
        keys: Vec::new(),
        pre_wrap_mode: 8,
        post_wrap_mode: 8,
    }
}

fn default_clamp_wrap() -> u32 {
    8
}

fn default_sibling_index() -> u32 {
    0
}

fn default_integration_type() -> SidecarIntegrationType {
    SidecarIntegrationType::Simplified
}

fn default_multi_child_type() -> SidecarMultiChildType {
    SidecarMultiChildType::Ignore
}

fn default_immobile_type() -> SidecarImmobileType {
    SidecarImmobileType::AllMotion
}

fn default_limit_type() -> SidecarLimitType {
    SidecarLimitType::None
}

impl PhysicsSidecar {
    /// sidecarのJSONバイト列を読み、厳密に検証する。
    ///
    /// 書式名・版・参照の存在・重複ID・親子・有限値・曲線の扱い・
    /// 資源上限を確認する。SHA-256の照合は
    /// [`VrmModel::resolve_physics_sidecar`](crate::vrm::VrmModel::resolve_physics_sidecar)
    /// で行うため、ここでは扱わない。
    ///
    /// # Errors
    ///
    /// JSONの構文破損・書式名や版の不一致・未知の列挙値やwrap・
    /// 欠落IDや重複ID・親子の不整合・非有限値や範囲外・上限超過の場合に返す。
    pub fn parse(bytes: &[u8], limits: &PhysicsLimits) -> Result<Self, VrmError> {
        if bytes.len() > limits.max_json_bytes {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPhysicsSidecar,
                "sidecarのバイト数が上限を超えた",
            ));
        }
        let sidecar: Self = serde_json::from_slice(bytes).map_err(|error| {
            VrmError::new(
                VrmErrorKind::InvalidPhysicsSidecar,
                format!("sidecarのJSONが読めない: {error}"),
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
    pub fn from_json(text: &str, limits: &PhysicsLimits) -> Result<Self, VrmError> {
        Self::parse(text.as_bytes(), limits)
    }

    /// 検証済みDTOの不変条件を確認する。
    fn validate(&self, limits: &PhysicsLimits) -> Result<(), VrmError> {
        if self.format != SIDECAR_FORMAT {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPhysicsSidecar,
                format!("sidecarの書式名が一致しない: {}", self.format),
            ));
        }
        if self.schema_version != SIDECAR_SCHEMA_VERSION {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPhysicsSidecar,
                format!("sidecarの版が未対応である: {}", self.schema_version),
            ));
        }
        if self.generator.name.is_empty() {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPhysicsSidecar,
                "sidecarの生成者名が空である",
            ));
        }
        if self.capture.coordinate_space != COORDINATE_SPACE {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPhysicsSidecar,
                format!(
                    "sidecarの座標空間が未対応である: {}",
                    self.capture.coordinate_space
                ),
            ));
        }
        if self.capture.avatar_root_id.is_empty() {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPhysicsSidecar,
                "sidecarのアバタールートIDが空である",
            ));
        }
        check_vec3_finite(&self.capture.root_scale, "ルート拡大率")?;
        if self
            .capture
            .root_scale
            .iter()
            .any(|v| !v.is_finite() || *v <= 0.0)
        {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPhysicsSidecar,
                "ルート拡大率が正の有限値でない",
            ));
        }
        if self.source_models.is_empty() || self.source_models.len() > limits.max_source_models {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPhysicsSidecar,
                "参照モデルの件数が範囲外である",
            ));
        }
        if self.nodes.is_empty() {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPhysicsSidecar,
                "sidecarのノードが無い",
            ));
        }
        if self.nodes.len() > limits.max_nodes {
            return Err(VrmError::new(
                VrmErrorKind::LimitExceeded,
                "sidecarのノード数が上限を超えた",
            ));
        }
        if self.phys_bones.len() > limits.max_phys_bones {
            return Err(VrmError::new(
                VrmErrorKind::LimitExceeded,
                "sidecarのPhysBone数が上限を超えた",
            ));
        }
        if self.colliders.len() > limits.max_colliders {
            return Err(VrmError::new(
                VrmErrorKind::LimitExceeded,
                "sidecarのcollider数が上限を超えた",
            ));
        }
        // 参照モデルのIDとSHA-256の書式を確認する。照合自体は解決時に行う。
        {
            let mut seen = HashSet::new();
            for model in &self.source_models {
                if model.id.is_empty() {
                    return Err(VrmError::new(
                        VrmErrorKind::InvalidPhysicsSidecar,
                        "参照モデルのIDが空である",
                    ));
                }
                if !seen.insert(model.id.as_str()) {
                    return Err(VrmError::new(
                        VrmErrorKind::InvalidPhysicsSidecar,
                        format!("参照モデルのIDが重複する: {}", model.id),
                    ));
                }
                check_sha256_hex(&model.sha256)?;
            }
        }
        // ノード・PhysBone・colliderのIDは全体で一意である。
        // 接頭辞（`n:`・`pb:`・`c:`）が違っても混同しないためである。
        {
            let mut seen = HashSet::new();
            for id in self
                .nodes
                .iter()
                .map(|node| node.id.as_str())
                .chain(self.phys_bones.iter().map(|bone| bone.id.as_str()))
                .chain(self.colliders.iter().map(|collider| collider.id.as_str()))
            {
                if id.is_empty() {
                    return Err(VrmError::new(
                        VrmErrorKind::InvalidPhysicsSidecar,
                        "IDが空である",
                    ));
                }
                if id.len() > 256 {
                    return Err(VrmError::new(
                        VrmErrorKind::InvalidPhysicsSidecar,
                        "IDが長すぎる",
                    ));
                }
                if !seen.insert(id) {
                    return Err(VrmError::new(
                        VrmErrorKind::InvalidPhysicsSidecar,
                        format!("IDが重複する: {id}"),
                    ));
                }
            }
        }
        let node_at: HashMap<&str, usize> = self
            .nodes
            .iter()
            .enumerate()
            .map(|(index, node)| (node.id.as_str(), index))
            .collect();
        for node in &self.nodes {
            if node.name.is_empty() {
                return Err(VrmError::new(
                    VrmErrorKind::InvalidPhysicsSidecar,
                    format!("ノード名が空である: {}", node.id),
                ));
            }
            if let Some(parent) = node.parent_id.as_deref()
                && !node_at.contains_key(parent)
            {
                return Err(VrmError::new(
                    VrmErrorKind::InvalidPhysicsSidecar,
                    format!("親ノードが無い: {} -> {parent}", node.id),
                ));
            }
            if let Some(source) = node.source.as_ref() {
                if source.model_id.is_empty() {
                    return Err(VrmError::new(
                        VrmErrorKind::InvalidPhysicsSidecar,
                        format!("出所のモデルIDが空である: {}", node.id),
                    ));
                }
                if !self
                    .source_models
                    .iter()
                    .any(|model| model.id == source.model_id)
                {
                    return Err(VrmError::new(
                        VrmErrorKind::InvalidPhysicsSidecar,
                        format!("出所のモデルが無い: {} -> {}", node.id, source.model_id),
                    ));
                }
                if source.path.is_empty() || source.path.len() > 256 {
                    return Err(VrmError::new(
                        VrmErrorKind::InvalidPhysicsSidecar,
                        format!("出所のパスが範囲外である: {}", node.id),
                    ));
                }
                for segment in &source.path {
                    if segment.is_empty() || segment.len() > 256 {
                        return Err(VrmError::new(
                            VrmErrorKind::InvalidPhysicsSidecar,
                            format!("出所のパス要素が空か長すぎる: {}", node.id),
                        ));
                    }
                }
            }
            check_trs(&node.reference_local, &node.id)?;
        }
        check_parent_cycles(&self.nodes, &node_at)?;
        let mut key_total = 0usize;
        for bone in &self.phys_bones {
            check_node_ref(&node_at, &bone.component_node_id, &bone.id, "付属ノード")?;
            check_node_ref(&node_at, &bone.root_node_id, &bone.id, "起点ノード")?;
            for ignore in &bone.ignore_node_ids {
                check_node_ref(&node_at, ignore, &bone.id, "無視ノード")?;
            }
            check_vec3_finite(&bone.endpoint_position, "末端位置")?;
            check_vec3_finite(&bone.limit_rotation, "制限回転")?;
            for (name, curve) in [
                ("pull", &bone.pull),
                ("spring", &bone.spring),
                ("stiffness", &bone.stiffness),
                ("gravity", &bone.gravity),
                ("gravityFalloff", &bone.gravity_falloff),
                ("immobile", &bone.immobile),
                ("radius", &bone.radius),
                ("maxAngleX", &bone.max_angle_x),
                ("maxAngleZ", &bone.max_angle_z),
            ] {
                key_total += curve.keys.len();
                check_curve(curve, &format!("{}:{}", bone.id, name))?;
            }
            if !bone.radius.value.is_finite() || bone.radius.value < 0.0 {
                return Err(VrmError::new(
                    VrmErrorKind::InvalidPhysicsSidecar,
                    format!("半径が0以上の有限値でない: {}", bone.id),
                ));
            }
            for collider in &bone.collider_ids {
                if collider.is_empty() {
                    return Err(VrmError::new(
                        VrmErrorKind::InvalidPhysicsSidecar,
                        format!("collider参照が空である: {}", bone.id),
                    ));
                }
            }
        }
        for collider in &self.colliders {
            check_node_ref(
                &node_at,
                &collider.component_node_id,
                &collider.id,
                "付属ノード",
            )?;
            check_node_ref(&node_at, &collider.root_node_id, &collider.id, "基準ノード")?;
            if !collider.radius.is_finite() || collider.radius < 0.0 {
                return Err(VrmError::new(
                    VrmErrorKind::InvalidPhysicsSidecar,
                    format!("colliderの半径が0以上の有限値でない: {}", collider.id),
                ));
            }
            if !collider.height.is_finite() || collider.height < 0.0 {
                return Err(VrmError::new(
                    VrmErrorKind::InvalidPhysicsSidecar,
                    format!("colliderの高さが0以上の有限値でない: {}", collider.id),
                ));
            }
            check_vec3_finite(&collider.position, "collider位置")?;
            check_quat(&collider.rotation, "collider回転")?;
            for bone in &self.phys_bones {
                let _ = bone;
            }
        }
        // collider参照の存在はPhysBoneの検査とは別にまとめて確認する。
        // どちらに欠落があるか分かるよう、collider側の索引を作ってから照合する。
        {
            let collider_at: HashSet<&str> = self
                .colliders
                .iter()
                .map(|collider| collider.id.as_str())
                .collect();
            for bone in &self.phys_bones {
                for collider in &bone.collider_ids {
                    if !collider_at.contains(collider.as_str()) {
                        return Err(VrmError::new(
                            VrmErrorKind::InvalidPhysicsSidecar,
                            format!("collider参照が無い: {} -> {collider}", bone.id),
                        ));
                    }
                }
            }
        }
        if key_total > limits.max_curve_keys {
            return Err(VrmError::new(
                VrmErrorKind::LimitExceeded,
                "sidecarのカーブキー総数が上限を超えた",
            ));
        }
        Ok(())
    }
}

/// SHA-256のhex書式を確認する。
fn check_sha256_hex(text: &str) -> Result<(), VrmError> {
    if text.len() != 64 || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(VrmError::new(
            VrmErrorKind::InvalidPhysicsSidecar,
            "SHA-256が64桁のhexでない",
        ));
    }
    Ok(())
}

/// 3要素の有限値を確認する。
fn check_vec3_finite(value: &[f64; 3], what: &str) -> Result<(), VrmError> {
    if !value.iter().all(|v| v.is_finite()) {
        return Err(VrmError::new(
            VrmErrorKind::InvalidPhysicsSidecar,
            format!("{what}に非有限値がある"),
        ));
    }
    Ok(())
}

/// 四元数の有限値と正規化を確認する。
fn check_quat(value: &[f64; 4], what: &str) -> Result<(), VrmError> {
    if !value.iter().all(|v| v.is_finite()) {
        return Err(VrmError::new(
            VrmErrorKind::InvalidPhysicsSidecar,
            format!("{what}に非有限値がある"),
        ));
    }
    let length =
        (value[0] * value[0] + value[1] * value[1] + value[2] * value[2] + value[3] * value[3])
            .sqrt();
    if !length.is_finite() || (length - 1.0).abs() > 1e-3 {
        return Err(VrmError::new(
            VrmErrorKind::InvalidPhysicsSidecar,
            format!("{what}が正規化されていない"),
        ));
    }
    Ok(())
}

/// TRSの有限値・正規化・拡大率を確認する。
fn check_trs(trs: &SidecarTrs, id: &str) -> Result<(), VrmError> {
    check_vec3_finite(&trs.position, "参照位置")?;
    check_quat(&trs.rotation, "参照回転")?;
    check_vec3_finite(&trs.scale, "参照拡大率")?;
    if trs.scale.contains(&0.0) {
        return Err(VrmError::new(
            VrmErrorKind::InvalidPhysicsSidecar,
            format!("参照拡大率に0がある: {id}"),
        ));
    }
    Ok(())
}

/// ノード参照の存在を確認する。
fn check_node_ref(
    node_at: &HashMap<&str, usize>,
    target: &str,
    owner: &str,
    what: &str,
) -> Result<(), VrmError> {
    if !node_at.contains_key(target) {
        return Err(VrmError::new(
            VrmErrorKind::InvalidPhysicsSidecar,
            format!("{what}の参照が無い: {owner} -> {target}"),
        ));
    }
    Ok(())
}

/// Unityの既知wrap値かどうか。`ClampForever=8` が既定である。
fn is_known_wrap(mode: u32) -> bool {
    matches!(mode, 0 | 1 | 2 | 4 | 8)
}

/// カーブの有限値・wrap・重み付きを確認する。
fn check_curve(curve: &SidecarCurve, owner: &str) -> Result<(), VrmError> {
    if !curve.value.is_finite() {
        return Err(VrmError::new(
            VrmErrorKind::InvalidPhysicsSidecar,
            format!("カーブ既定値が有限値でない: {owner}"),
        ));
    }
    if !is_known_wrap(curve.pre_wrap_mode) || !is_known_wrap(curve.post_wrap_mode) {
        return Err(VrmError::new(
            VrmErrorKind::InvalidPhysicsSidecar,
            format!("カーブのwrapが未対応である: {owner}"),
        ));
    }
    for key in &curve.keys {
        if ![
            key.time,
            key.value,
            key.in_slope,
            key.out_slope,
            key.in_weight,
            key.out_weight,
        ]
        .iter()
        .all(|v| v.is_finite())
        {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPhysicsSidecar,
                format!("カーブキーに非有限値がある: {owner}"),
            ));
        }
        if key.weighted_mode != 0 {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPhysicsSidecar,
                format!("重み付きカーブは未対応である: {owner}"),
            ));
        }
    }
    Ok(())
}

/// 親子の循環を確認する。
fn check_parent_cycles(
    nodes: &[SidecarNode],
    node_at: &HashMap<&str, usize>,
) -> Result<(), VrmError> {
    // 深さ優先ではなく訪問状態で判定し、鎖の長さに比例する作業量に抑える。
    // `done` は検査済み、`visiting` は現在の鎖に含まれる節である。
    let mut done = vec![false; nodes.len()];
    let mut visiting = vec![false; nodes.len()];
    for start in 0..nodes.len() {
        let mut current = start;
        let mut chain = Vec::new();
        loop {
            if done[current] {
                break;
            }
            if visiting[current] {
                return Err(VrmError::new(
                    VrmErrorKind::InvalidPhysicsSidecar,
                    format!("ノードの親子に循環がある: {}", nodes[current].id),
                ));
            }
            visiting[current] = true;
            chain.push(current);
            match nodes[current].parent_id.as_deref() {
                Some(parent) => match node_at.get(parent) {
                    Some(&index) => current = index,
                    None => break,
                },
                None => break,
            }
        }
        for index in chain {
            visiting[index] = false;
            done[index] = true;
        }
    }
    Ok(())
}

/// 対応解決の結果。次単位の物理コンパイルが使う。
#[derive(Debug, Clone)]
pub struct PhysicsResolution {
    /// 一致した参照モデルのID。
    pub matched_source_model: String,
    /// 照合したFBXバイト列のSHA-256（小文字hex）。
    pub fbx_sha256: String,
    /// sidecar参照世界からモデル出力世界への変換行列。
    pub coordinate_transform: [f64; 16],
    /// sidecarノードIDからVrmノード索引への対応表。
    pub node_map: HashMap<String, usize>,
    /// 参照されずに残ったシーン追加ノード。
    pub unmapped_nodes: Vec<UnmappedPhysicsNode>,
    /// PhysBoneの参照解決。
    pub phys_bones: Vec<ResolvedPhysBone>,
    /// colliderの参照解決。今回は平面colliderを含まない。
    pub colliders: Vec<ResolvedCollider>,
    /// 今回の出力対象外として除外したcollider ID（平面）。
    pub skipped_colliders: Vec<String>,
}

/// 参照されなかったシーン追加ノード。
#[derive(Debug, Clone)]
pub struct UnmappedPhysicsNode {
    /// sidecarノードID。
    pub id: String,
    /// ノード名。
    pub name: String,
}

/// PhysBone1件の参照解決。
#[derive(Debug, Clone)]
pub struct ResolvedPhysBone {
    /// sidecarのPhysBone ID。
    pub id: String,
    /// 付属ノードのVrm索引。
    pub component_node: usize,
    /// 起点ノードのVrm索引。
    pub root_node: usize,
    /// 無視ノードのVrm索引列。
    pub ignore_nodes: Vec<usize>,
}

/// collider1件の参照解決。
#[derive(Debug, Clone)]
pub struct ResolvedCollider {
    /// sidecarのcollider ID。
    pub id: String,
    /// 付属ノードのVrm索引。
    pub component_node: usize,
    /// 基準ノードのVrm索引。
    pub root_node: usize,
}

/// 座標ゲートの許容差。既存の行列演算の精度に合わせる。
// Unity sidecarのTRSはf32由来であり、bind姿勢の丸め差を許容する。
const COORDINATE_TOLERANCE: f64 = 1e-4;

impl VrmModel {
    /// 物理sidecarを `VrmModel` のノードへ対応付ける。
    ///
    /// FBXバイト列のSHA-256と `sourceModels` を照合し、一致がちょうど1件の
    /// 場合だけ進む。`VrmModel` のルートからの名前列を索引化し、sidecarの
    /// `source.path` と完全一致で対応付ける。接尾辞や一意名の自動補完はしない。
    /// 参照姿勢は `edit-mode` だけ受け付け、座標ゲートで単一の変換行列を
    /// 確かめる。スプリングのコンパイルや出力は行わない。
    ///
    /// # Errors
    ///
    /// SHA-256の不一致・参照姿勢の未対応・0件や複数件の対応・
    /// 参照されるシーン追加ノード・座標の不一致・上限超過の場合に返す。
    pub fn resolve_physics_sidecar(
        &self,
        sidecar: &PhysicsSidecar,
        fbx_bytes: &[u8],
    ) -> Result<PhysicsResolution, VrmError> {
        if sidecar.capture.state_policy != STATE_POLICY_EDIT_MODE
            && sidecar.capture.state_policy != "bind-pose"
        {
            return Err(VrmError::new(
                VrmErrorKind::UnsupportedPhysicsFeature,
                format!(
                    "参照姿勢がedit-modeまたはbind-poseでない: {}",
                    sidecar.capture.state_policy
                ),
            ));
        }
        let digest = sha256_hex(fbx_bytes);
        let matched: Vec<&SidecarSourceModel> = sidecar
            .source_models
            .iter()
            .filter(|model| model.sha256.to_ascii_lowercase() == digest)
            .collect();
        let [single] = matched.as_slice() else {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPhysicsSidecar,
                format!(
                    "FBXのSHA-256に一致する参照モデルが1件でない: {}件",
                    matched.len()
                ),
            ));
        };
        // Vrmノードのルートからの完全パスを索引化する。
        // メッシュ所有ノードは切り離されたルートになるため、そのままの鎖で扱う。
        let mut path_at: HashMap<Vec<String>, Vec<usize>> = HashMap::new();
        for (index, _) in self.nodes.iter().enumerate() {
            let path = vrm_full_path(self, index)?;
            path_at.entry(path).or_default().push(index);
        }
        let mut node_map: HashMap<String, usize> = HashMap::new();
        let mut missing: Vec<String> = Vec::new();
        for node in &sidecar.nodes {
            let Some(source) = node.source.as_ref() else {
                continue;
            };
            if source.model_id != single.id {
                continue;
            }
            match path_at.get(&source.path) {
                Some(indices) if indices.len() == 1 => {
                    node_map.insert(node.id.clone(), indices[0]);
                }
                Some(indices) => {
                    let names = indices
                        .iter()
                        .map(|index| self.nodes[*index].name.clone())
                        .collect::<Vec<_>>()
                        .join(", ");
                    return Err(VrmError::new(
                        VrmErrorKind::UnresolvedPhysicsNode,
                        format!(
                            "対応が複数ある: {} [{}] ({}件: {names})",
                            node.id,
                            source.path.join("/"),
                            indices.len()
                        ),
                    ));
                }
                None => {
                    missing.push(format!("{} [{}]", node.id, source.path.join("/")));
                }
            }
        }
        if !missing.is_empty() {
            // 診断用に候補を返すのではなく、未解決の一覧をそのまま返す。
            // 接尾辞での自動補完はしない。
            return Err(VrmError::new(
                VrmErrorKind::UnresolvedPhysicsNode,
                format!("対応が無い: {}", missing.join(", ")),
            ));
        }
        // 参照されるID集合を集め、シーン追加ノードの参照を検出する。
        let mut referenced: HashSet<&str> = HashSet::new();
        for bone in &sidecar.phys_bones {
            referenced.insert(bone.component_node_id.as_str());
            referenced.insert(bone.root_node_id.as_str());
            for ignore in &bone.ignore_node_ids {
                referenced.insert(ignore.as_str());
            }
        }
        for collider in &sidecar.colliders {
            // 平面colliderは今回の出力対象外であり、そのシーン追加ノードも
            // 参照済みとして扱わない。除外は後段で報告する。
            if collider.shape_type == SidecarShapeType::Plane {
                continue;
            }
            referenced.insert(collider.component_node_id.as_str());
            referenced.insert(collider.root_node_id.as_str());
        }
        let mut unmapped_nodes = Vec::new();
        for node in &sidecar.nodes {
            if node.source.is_some() {
                continue;
            }
            if referenced.contains(node.id.as_str()) {
                return Err(VrmError::new(
                    VrmErrorKind::UnresolvedPhysicsNode,
                    format!(
                        "シーン追加ノードが参照されている: {}（次単位の補助ノード生成が必要）",
                        node.id
                    ),
                ));
            }
            unmapped_nodes.push(UnmappedPhysicsNode {
                id: node.id.clone(),
                name: node.name.clone(),
            });
        }
        // 参照されるFBX由来ノードが対応表に無い場合は失敗する。
        // `source.path` が別モデル向きの場合は照合対象外のため、ここで検出する。
        for id in &referenced {
            let node = sidecar.nodes.iter().find(|node| node.id.as_str() == *id);
            let Some(node) = node else {
                return Err(VrmError::new(
                    VrmErrorKind::InvalidPhysicsSidecar,
                    format!("参照IDに対応するノードが無い: {id}"),
                ));
            };
            if node.source.is_none() {
                continue;
            }
            if !node_map.contains_key(*id) {
                return Err(VrmError::new(
                    VrmErrorKind::UnresolvedPhysicsNode,
                    format!("参照ノードの対応が無い: {id}"),
                ));
            }
        }
        let mut phys_bones = Vec::with_capacity(sidecar.phys_bones.len());
        for bone in &sidecar.phys_bones {
            let component_node = lookup_mapped(&node_map, &bone.component_node_id, &bone.id)?;
            let root_node = lookup_mapped(&node_map, &bone.root_node_id, &bone.id)?;
            let mut ignore_nodes = Vec::with_capacity(bone.ignore_node_ids.len());
            for ignore in &bone.ignore_node_ids {
                ignore_nodes.push(lookup_mapped(&node_map, ignore, &bone.id)?);
            }
            // 平面colliderの参照は今回未対応として失敗する。
            // 形状自体はDTOに残るが、使う経路が無いためである。
            let collider_at: HashMap<&str, &SidecarCollider> = sidecar
                .colliders
                .iter()
                .map(|collider| (collider.id.as_str(), collider))
                .collect();
            for collider in &bone.collider_ids {
                match collider_at.get(collider.as_str()) {
                    Some(resolved) if resolved.shape_type == SidecarShapeType::Plane => {
                        // 今回の出力対象外。参照は残さず、レポートにのみ残す。
                    }
                    Some(_) => {}
                    None => {
                        return Err(VrmError::new(
                            VrmErrorKind::InvalidPhysicsSidecar,
                            format!("collider参照が無い: {} -> {collider}", bone.id),
                        ));
                    }
                }
            }
            phys_bones.push(ResolvedPhysBone {
                id: bone.id.clone(),
                component_node,
                root_node,
                ignore_nodes,
            });
        }
        let mut colliders = Vec::with_capacity(sidecar.colliders.len());
        let skipped_colliders: Vec<String> = sidecar
            .colliders
            .iter()
            .filter(|collider| collider.shape_type == SidecarShapeType::Plane)
            .map(|collider| collider.id.clone())
            .collect();
        for collider in &sidecar.colliders {
            if collider.shape_type == SidecarShapeType::Plane {
                continue;
            }
            colliders.push(ResolvedCollider {
                id: collider.id.clone(),
                component_node: lookup_mapped(
                    &node_map,
                    &collider.component_node_id,
                    &collider.id,
                )?,
                root_node: lookup_mapped(&node_map, &collider.root_node_id, &collider.id)?,
            });
        }
        let coordinate_transform = check_coordinate_gate(self, sidecar, &node_map)?;
        Ok(PhysicsResolution {
            matched_source_model: single.id.clone(),
            fbx_sha256: digest,
            coordinate_transform,
            node_map,
            unmapped_nodes,
            phys_bones,
            colliders,
            skipped_colliders,
        })
    }
}

/// 対応表からVrm索引を引く。
fn lookup_mapped(
    node_map: &HashMap<String, usize>,
    sidecar_id: &str,
    owner: &str,
) -> Result<usize, VrmError> {
    node_map.get(sidecar_id).copied().ok_or_else(|| {
        VrmError::new(
            VrmErrorKind::UnresolvedPhysicsNode,
            format!("参照ノードの対応が無い: {owner} -> {sidecar_id}"),
        )
    })
}

/// Vrmノードのルートからの名前列を求める。
fn vrm_full_path(model: &VrmModel, mut index: usize) -> Result<Vec<String>, VrmError> {
    let mut reversed = Vec::new();
    // 循環した親子は無限に遡らないよう、訪問済みで打ち切る。
    let mut seen = HashSet::new();
    loop {
        let node = model.nodes.get(index).ok_or_else(|| {
            VrmError::new(
                VrmErrorKind::InvalidPhysicsSidecar,
                "Vrmノードの索引が範囲外である",
            )
        })?;
        if !seen.insert(index) {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPhysicsSidecar,
                "Vrmノードの親子に循環がある",
            ));
        }
        reversed.push(node.name.clone());
        match node.parent {
            Some(parent) => index = parent,
            None => break,
        }
    }
    reversed.reverse();
    Ok(reversed)
}

/// FBXバイト列のSHA-256を小文字hexで返す。wasmでも動く純Rust実装である。
fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for byte in digest {
        out.push(hex_nibble(byte >> 4));
        out.push(hex_nibble(byte & 0x0f));
    }
    out
}

fn hex_nibble(value: u8) -> char {
    (match value {
        0..=9 => b'0' + value,
        _ => b'a' + (value - 10),
    }) as char
}

/// TRSから列優先の4x4行列を作る。`T * R * S` の順である。
fn trs_to_matrix(trs: &SidecarTrs) -> [f64; 16] {
    let (x, y, z, w) = (
        trs.rotation[0],
        trs.rotation[1],
        trs.rotation[2],
        trs.rotation[3],
    );
    let (sx, sy, sz) = (trs.scale[0], trs.scale[1], trs.scale[2]);
    // 正規化済みの四元数から回転行列を作り、列ごとに拡大率を掛ける。
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
        trs.position[0],
        trs.position[1],
        trs.position[2],
        1.0, //
    ]
}

/// sidecarの参照世界行列を求める。
fn sidecar_world_matrices(sidecar: &PhysicsSidecar) -> Result<Vec<[f64; 16]>, VrmError> {
    let at: HashMap<&str, usize> = sidecar
        .nodes
        .iter()
        .enumerate()
        .map(|(index, node)| (node.id.as_str(), index))
        .collect();
    // UnityのTransformは左手系、VRM/glTFは右手系である。sidecarは
    // Unity空間（`unity-avatar-root-local-meters`）なので、X反転と
    // 四元数の共役でVRM空間へ移してから比較する。
    let locals: Vec<[f64; 16]> = sidecar
        .nodes
        .iter()
        .map(|node| {
            let local = &node.reference_local;
            let converted = SidecarTrs {
                position: [-local.position[0], local.position[1], local.position[2]],
                rotation: [
                    local.rotation[0],
                    -local.rotation[1],
                    -local.rotation[2],
                    local.rotation[3],
                ],
                scale: local.scale,
            };
            trs_to_matrix(&converted)
        })
        .collect();
    let mut worlds = vec![matrix::IDENTITY; sidecar.nodes.len()];
    let mut done = vec![false; sidecar.nodes.len()];
    for start in 0..sidecar.nodes.len() {
        let mut chain = Vec::new();
        let mut seen = HashSet::new();
        let mut current = start;
        loop {
            if done[current] {
                break;
            }
            if !seen.insert(current) {
                return Err(VrmError::new(
                    VrmErrorKind::InvalidPhysicsSidecar,
                    "sidecarの親子に循環がある",
                ));
            }
            chain.push(current);
            match sidecar.nodes[current].parent_id.as_deref() {
                Some(parent) => match at.get(parent) {
                    Some(&index) => current = index,
                    None => {
                        return Err(VrmError::new(
                            VrmErrorKind::InvalidPhysicsSidecar,
                            "sidecarの親ノードが無い",
                        ));
                    }
                },
                None => break,
            }
        }
        for &node in chain.iter().rev() {
            match sidecar.nodes[node].parent_id.as_deref() {
                Some(parent) => match at.get(parent) {
                    Some(&index) => worlds[node] = matrix::mul(&worlds[index], &locals[node]),
                    None => {
                        return Err(VrmError::new(
                            VrmErrorKind::InvalidPhysicsSidecar,
                            "sidecarの親ノードが無い",
                        ));
                    }
                },
                None => worlds[node] = locals[node],
            }
            done[node] = true;
        }
    }
    Ok(worlds)
}

/// モデルの出力世界行列を求める。親なしには `root * local` を掛ける。
fn model_world_matrices(model: &VrmModel) -> Result<Vec<[f64; 16]>, VrmError> {
    let mut worlds = vec![matrix::IDENTITY; model.nodes.len()];
    let mut done = vec![false; model.nodes.len()];
    for start in 0..model.nodes.len() {
        let mut chain = Vec::new();
        let mut seen = HashSet::new();
        let mut current = start;
        loop {
            if done[current] {
                break;
            }
            if !seen.insert(current) {
                return Err(VrmError::new(
                    VrmErrorKind::InvalidPhysicsSidecar,
                    "Vrmノードの親子に循環がある",
                ));
            }
            chain.push(current);
            match model.nodes.get(current).and_then(|node| node.parent) {
                Some(parent) if parent < model.nodes.len() => current = parent,
                Some(_) => {
                    return Err(VrmError::new(
                        VrmErrorKind::InvalidPhysicsSidecar,
                        "Vrmノードの親索引が範囲外である",
                    ));
                }
                None => break,
            }
        }
        for &node in chain.iter().rev() {
            let local = &model.nodes[node].local;
            match model.nodes[node].parent {
                Some(parent) => worlds[node] = matrix::mul(&worlds[parent], local),
                None => worlds[node] = matrix::mul(&model.root, local),
            }
            done[node] = true;
        }
    }
    Ok(worlds)
}

/// 座標ゲートを確認し、単一の変換行列を返す。
///
/// 対応する関節ノードで `model_world * inverse(sidecar_world)` が
/// 単一の行列に一致するか確かめる。対象はスキンの関節に含まれる対応ノードであり、
/// 関節が無い場合はメッシュ所有を除く対応ノードを使う。どちらも無い場合は失敗する。
fn check_coordinate_gate(
    model: &VrmModel,
    sidecar: &PhysicsSidecar,
    node_map: &HashMap<String, usize>,
) -> Result<[f64; 16], VrmError> {
    let sidecar_worlds = sidecar_world_matrices(sidecar)?;
    let model_worlds = model_world_matrices(model)?;
    let sidecar_at: HashMap<&str, usize> = sidecar
        .nodes
        .iter()
        .enumerate()
        .map(|(index, node)| (node.id.as_str(), index))
        .collect();
    let mesh_nodes: HashSet<usize> = model.meshes.iter().map(|mesh| mesh.node).collect();
    // 物理が実際に参照するノードだけを gate の対象にする。
    // 全身の関節を対象にすると、指や表情用ボーンの編集時ポーズ差で
    // スプリングに無関係な不一致まで拾ってしまうためである。
    let mut physics_ids: HashSet<&str> = HashSet::new();
    for bone in &sidecar.phys_bones {
        physics_ids.insert(bone.root_node_id.as_str());
        for ignore in &bone.ignore_node_ids {
            physics_ids.insert(ignore.as_str());
        }
    }
    for collider in &sidecar.colliders {
        physics_ids.insert(collider.root_node_id.as_str());
    }
    let mut targets: Vec<(usize, usize)> = Vec::new();
    for (sidecar_id, &vrm_index) in node_map {
        if !physics_ids.contains(sidecar_id.as_str()) {
            continue;
        }
        let Some(&sidecar_index) = sidecar_at.get(sidecar_id.as_str()) else {
            continue;
        };
        // メッシュ所有ノードは頂点ベイクで切り離されるため、常に対象外である。
        if mesh_nodes.contains(&vrm_index) {
            continue;
        }
        targets.push((sidecar_index, vrm_index));
    }
    if targets.is_empty() {
        return Err(VrmError::new(
            VrmErrorKind::PhysicsCoordinateMismatch,
            "座標ゲートの対象ノードが無い",
        ));
    }
    // HashMapの反復順に依存せず、最初の基準ノードを安定させる。
    targets.sort_unstable();
    // 最初の対応から基準変換を求め、残りが同じ変換に一致するか確かめる。
    // 軸や単位を推測せず、行列の一致だけを見る。
    let mut baseline: Option<[f64; 16]> = None;
    let mut scale_hint = 1.0;
    for (sidecar_index, vrm_index) in &targets {
        let Some(inverse) = matrix::inverse(&sidecar_worlds[*sidecar_index]) else {
            return Err(VrmError::new(
                VrmErrorKind::PhysicsCoordinateMismatch,
                "sidecarの参照世界行列が特異である",
            ));
        };
        let candidate = matrix::mul(&model_worlds[*vrm_index], &inverse);
        match baseline {
            None => {
                scale_hint = candidate
                    .iter()
                    .map(|v| v.abs())
                    .fold(0.0f64, f64::max)
                    .max(1.0);
                baseline = Some(candidate);
            }
            Some(base) => {
                let tolerance = COORDINATE_TOLERANCE * scale_hint.max(1.0);
                let mismatch = candidate
                    .iter()
                    .zip(base.iter())
                    .any(|(a, b)| (a - b).abs() > tolerance || !a.is_finite() || !b.is_finite());
                if mismatch {
                    let name = model.nodes[*vrm_index].name.as_str();
                    let path = vrm_full_path(model, *vrm_index)
                        .map(|path| path.join("/"))
                        .unwrap_or_default();
                    let side = sidecar
                        .nodes
                        .get(*sidecar_index)
                        .map(|node| (node.name.as_str(), node.scene_path.join("/")))
                        .unwrap_or(("?", String::new()));
                    let model_pos = [
                        model_worlds[*vrm_index][12],
                        model_worlds[*vrm_index][13],
                        model_worlds[*vrm_index][14],
                    ];
                    let side_pos = [
                        sidecar_worlds[*sidecar_index][12],
                        sidecar_worlds[*sidecar_index][13],
                        sidecar_worlds[*sidecar_index][14],
                    ];
                    return Err(VrmError::new(
                        VrmErrorKind::PhysicsCoordinateMismatch,
                        format!(
                            "座標変換が単一の行列に一致しない: model={name} path={path} sidecar={} scene={} modelPos={model_pos:?} sidePos={side_pos:?} base={base:?} candidate={candidate:?}",
                            side.0, side.1
                        ),
                    ));
                }
            }
        }
    }
    baseline.ok_or_else(|| {
        VrmError::new(
            VrmErrorKind::PhysicsCoordinateMismatch,
            "座標ゲートの対象ノードが無い",
        )
    })
}
