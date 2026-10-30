//! Unityヘルパが出力する物理sidecarの読み取りと対応解決、スプリングの最小出力。
//!
//! 入口は [`PhysicsSidecar::parse`]（バイト列の検証）と
//! [`VrmModel::resolve_physics_sidecar`](crate::vrm::VrmModel::resolve_physics_sidecar)
//! （`VrmModel` のノードへの対応付け）、[`compile_springs`]（`VrmSpringBones` の
//! 生成）である。完全な参照実装一致（endpoint・limit・center・曲線の再現）は
//! 次の作業単位であり、ここでは分岐分割した鎖と第一近似の値を出すだけである。

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

/// VRMのスプリングボーン全体。[`compile_springs`] が作り、書き出し層が出力する。
#[derive(Debug, Clone, Default)]
pub struct VrmSpringBones {
    /// スプリング列。PhysBoneごとの木を分岐分割した線形鎖である。
    pub springs: Vec<VrmSpring>,
    /// collider列。参照されるsphere/capsuleだけを含む。
    pub colliders: Vec<VrmSpringCollider>,
    /// colliderグループ列。PhysBoneの参照集合ごとに1件である。
    pub collider_groups: Vec<VrmSpringColliderGroup>,
}

/// スプリング1件。PhysBoneの木を分岐分割した線形鎖の1本である。
#[derive(Debug, Clone, Default)]
pub struct VrmSpring {
    /// スプリング名。先頭関節のノード名である。
    pub name: String,
    /// 関節列。親から子への順である。
    pub joints: Vec<VrmSpringJoint>,
    /// 中心ノードの索引。無い場合は `None`（ワールド空間に従う）である。
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

/// スプリング出力の上限。
///
/// フィールドは非公開で、既定値は [`Default`] が設定する。
/// 上限の変更は `with_` で始まるメソッドで行う。
#[derive(Debug, Clone)]
pub struct SpringCompileOptions {
    max_joints: usize,
}

/// 1springのjoint数の既定上限（64）。
///
/// 実データの十数件が収まる大きさにし、破損由来の鎖の暴走を抑える。
const DEFAULT_MAX_SPRING_JOINTS: usize = 64;

impl SpringCompileOptions {
    /// 既定の上限を持つ設定を生成する。
    pub fn new() -> Self {
        Self {
            max_joints: DEFAULT_MAX_SPRING_JOINTS,
        }
    }

    /// 1springのjoint数の上限を変更する。1以上である。
    pub fn with_max_joints(mut self, max_joints: usize) -> Self {
        self.max_joints = max_joints;
        self
    }
}

impl Default for SpringCompileOptions {
    fn default() -> Self {
        Self::new()
    }
}

/// コンパイル結果。次段の書き出しが使う骨格と、近似の診断である。
#[derive(Debug, Clone)]
pub struct CompiledSprings {
    /// 出力する骨格。
    pub bones: VrmSpringBones,
    /// 近似と切り捨ての記録。
    pub report: SpringCompileReport,
}

/// スプリング出力の診断。近似・切り捨て・未出力の内訳である。
#[derive(Debug, Clone, Default)]
pub struct SpringCompileReport {
    /// 出力したspring数。
    pub spring_count: usize,
    /// 出力したjoint総数。
    pub joint_count: usize,
    /// 出力したcollider数。
    pub collider_count: usize,
    /// 出力したcolliderグループ数。
    pub group_count: usize,
    /// 分岐の分割記録。Aimした子とheadになり得る子を持つ。
    pub branch_splits: Vec<BranchSplit>,
    /// 他のPhysBoneのrootで切断した記録。
    pub cut_at_other_roots: Vec<RootCut>,
    /// joint上限で切断したspring名（先頭関節のノード名）。
    pub truncated_chains: Vec<String>,
    /// 長さがほぼ0の区間。分割せずそのまま残した記録である。
    pub zero_length_joints: Vec<ZeroLengthJoint>,
    /// 曲線のbase値を使った記録。キー列は評価しない。
    pub curve_base_used: Vec<CurveBaseUsed>,
    /// `1 - spring` が範囲外で丸めたPhysBoneのID。
    pub drag_clamped: Vec<String>,
    /// 無効化されていたが出力したPhysBoneのID。VRMに有効無効は無い。
    pub disabled_bones: Vec<String>,
    /// 無効化されていたが出力したcolliderのID。
    pub disabled_colliders: Vec<String>,
    /// 参照されず出力しなかったcolliderのID。
    pub unused_colliders: Vec<String>,
    /// 平面のため落とした参照を持つPhysBoneのID。
    pub plane_colliders_dropped: Vec<String>,
    /// VRMに写せない指定（`insideBounds`・`bonesAsSpheres`）を持つcolliderの記録。
    pub ignored_collider_options: Vec<String>,
    /// `center` を付けなかったspringのPhysBone ID。springごとに1件である。
    pub center_missing: Vec<String>,
    /// endpointを生成しなかったspringのPhysBone ID。今回は全件である。
    pub no_endpoint: Vec<String>,
    /// `multiChildType` が `Ignore` 以外で最初の子に畳んだspringのPhysBone ID。
    pub first_child_multichild: Vec<String>,
    /// `gravity_power` に0を置いた関節数。今回は全関節である。
    pub gravity_zero_joints: usize,
    /// springにならなかったPhysBoneの記録。
    pub skipped_bones: Vec<SkippedBone>,
    /// 出力したspringごとの所有PhysBoneと関節数。
    pub owned_springs: Vec<OwnedSpring>,
    /// 先行Planに所有権を奪われたノードの記録。
    pub lost_nodes: Vec<LostNode>,
    /// 複数Planが奪い合ったノードの集約。
    pub shared_nodes: Vec<SharedNode>,
    /// `Average` を最初の子で近似したPhysBoneのID。平均tail生成は次段階である。
    pub average_approximated: Vec<String>,
    /// centerを付けたspringの一覧。
    pub center_applied: Vec<CenterAssigned>,
    /// HoldsImmobileのためcenterを付けなかったPhysBoneのID。
    pub held_immobile: Vec<String>,
    /// Worldでroot値が0.5未満のためcenterを付けなかったPhysBoneのID。
    pub weak_world: Vec<String>,
    /// 合法なcenterが無く付けなかったspringの一覧。
    pub no_legal_center: Vec<NoCenter>,
}

/// centerを付けたspring1件。
#[derive(Debug, Clone)]
pub struct CenterAssigned {
    /// PhysBoneのID。
    pub bone: String,
    /// spring名。
    pub spring: String,
    /// centerのVrmノード索引。
    pub node: usize,
    /// centerの種別。
    pub kind: CenterKind,
}

/// centerの種別。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CenterKind {
    /// AllMotion由来。chain rootの親を起点に探す。
    AllMotion,
    /// World由来。avatar root直下の最上位祖先を起点に探す。
    World,
}

/// 合法なcenterが無く付けなかったspring1件。
#[derive(Debug, Clone)]
pub struct NoCenter {
    /// PhysBoneのID。
    pub bone: String,
    /// spring名。
    pub name: String,
    /// centerが無い理由。
    pub reason: NoCenterReason,
}

/// centerが無い理由。閉じた集合である。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoCenterReason {
    /// すべての候補が他のspringの内側かavatar rootで尽きた。
    NoLegalNode,
}

/// 分岐の分割記録1件。ノードはsidecar IDで指す。
#[derive(Debug, Clone)]
pub struct BranchSplit {
    /// PhysBoneのID。
    pub bone: String,
    /// 分岐元のsidecarノードID。
    pub node: String,
    /// Aimした子。`Ignore` 分岐では `None` である。
    pub aimed: Option<String>,
    /// 新しいspringのheadになり得る子。Aimした子は含まない。
    pub heads: Vec<String>,
}

/// springにならなかったPhysBone1件。
#[derive(Debug, Clone)]
pub struct SkippedBone {
    /// PhysBoneのID。
    pub bone: String,
    /// springにしなかった理由。
    pub reason: SkippedBoneReason,
}

/// springにしない理由。閉じた集合である。
#[derive(Debug, Clone)]
pub enum SkippedBoneReason {
    /// rootが無視指定に含まれる。
    RootIgnored,
    /// rootの所有権を先行Planが持つ。
    RootShared {
        /// 先に所有したPhysBoneのID。
        owner: String,
    },
    /// Aim先が所有外で区間を持たない。
    AimTargetUnowned,
    /// 所有したAim骨が無い。
    NoHeads,
    /// 関節にできないsidecarノードを含む。飛ばして接続はしない。
    UnmappedJoint {
        /// sidecarノードID。
        node: String,
    },
}

/// 出力したspring1件の所有記録。
#[derive(Debug, Clone)]
pub struct OwnedSpring {
    /// 所有するPhysBoneのID。
    pub bone: String,
    /// spring名。先頭関節のノード名である。
    pub name: String,
    /// 関節数。
    pub joints: usize,
}

/// 所有権を奪われたノード1件。ノードはsidecar IDで指す。
#[derive(Debug, Clone)]
pub struct LostNode {
    /// 奪われたPlanのPhysBone ID。
    pub bone: String,
    /// 奪われたsidecarノードID。
    pub node: String,
    /// 所有するPhysBoneのID。
    pub owner: String,
}

/// 奪い合ったノードの集約1件。ノードはsidecar IDで指す。
#[derive(Debug, Clone)]
pub struct SharedNode {
    /// 奪い合ったsidecarノードID。
    pub node: String,
    /// 所有するPhysBoneのID。
    pub owner: String,
    /// 奪われたPlanのPhysBone ID列。
    pub lost_by: Vec<String>,
}

/// 他のPhysBoneのrootでの切断1件。
#[derive(Debug, Clone)]
pub struct RootCut {
    /// 切断した側のPhysBone ID。
    pub bone: String,
    /// 切断したsidecarノードID。他のPhysBoneのrootである。
    pub node: String,
    /// 他のPhysBoneのID。
    pub other_bone: String,
}

/// 長さがほぼ0の区間1件。
#[derive(Debug, Clone)]
pub struct ZeroLengthJoint {
    /// PhysBoneのID。
    pub bone: String,
    /// 親のsidecarノードID。
    pub parent: String,
    /// 子のsidecarノードID。
    pub child: String,
}

/// 曲線のbase値使用1件。
#[derive(Debug, Clone)]
pub struct CurveBaseUsed {
    /// PhysBoneのID。
    pub bone: String,
    /// 曲線名（`radius`・`spring`・`stiffness`）。
    pub field: &'static str,
    /// 使わなかったキー数。
    pub keys: usize,
}

/// ゼロ長とみなす区間の長さ（メートル）。
///
/// 出力世界行列はメートル単位であり、丸め由来の微小なずれを吸収する。
/// この値自体は仮であり、実データでの観測に基づく見直しの対象である。
const ZERO_LENGTH_TOLERANCE: f64 = 1e-9;

/// Unity左手系からVRM右手系への点変換。Xを反転する。
fn mirror_point(point: [f64; 3]) -> [f64; 3] {
    [-point[0], point[1], point[2]]
}

/// Unity左手系からVRM右手系への四元数変換。YとZを反転する。
fn mirror_quat(rotation: [f64; 4]) -> [f64; 4] {
    [rotation[0], -rotation[1], -rotation[2], rotation[3]]
}

/// f64の有限値をf32へ寄せる。溢れて非有限になる場合は失敗する。
fn to_f32_finite(value: f64, what: &str) -> Result<f32, VrmError> {
    let out = value as f32;
    if out.is_finite() {
        Ok(out)
    } else {
        Err(VrmError::new(
            VrmErrorKind::InvalidPhysicsSidecar,
            format!("{what}がf32に収まらない"),
        ))
    }
}

/// 3要素をf32へ寄せる。いずれかが非有限になる場合は失敗する。
fn to_f32x3(point: [f64; 3], what: &str) -> Result<[f32; 3], VrmError> {
    Ok([
        to_f32_finite(point[0], what)?,
        to_f32_finite(point[1], what)?,
        to_f32_finite(point[2], what)?,
    ])
}

/// UnityのAnimationCurve相当の評価。未検証のwrapや重み付きは検証済みのはずであり、
/// ここでは解釈だけを行う。キーは時刻順に並べ替えてから使う。
/// 1キーならその値を返し、範囲外はwrapに従う。
fn eval_unity_curve(curve: &SidecarCurve, mut t: f64) -> f64 {
    if curve.keys.is_empty() {
        return 1.0;
    }
    let mut keys: Vec<&SidecarCurveKey> = curve.keys.iter().collect();
    keys.sort_by(|a, b| {
        a.time
            .partial_cmp(&b.time)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let first = keys[0];
    let last = keys[keys.len() - 1];
    if keys.len() == 1 {
        return first.value;
    }
    let span = last.time - first.time;
    // 範囲外のwrap。範囲内はそのまま評価する。
    if t < first.time || t > last.time {
        // 既知のwrap値だけが検証を通る。範囲外の扱いはUnityに従う。
        // 0:既定・1:Onceは端値、2:Loopは折り返し、4:PingPongは往復、8:ClampForeverは端値。
        match (t < first.time, curve.pre_wrap_mode, curve.post_wrap_mode) {
            (true, 2, _) | (false, _, 2) if span > 0.0 => {
                let mut u = (t - first.time) / span;
                u -= u.floor();
                t = first.time + u * span;
            }
            (true, 4, _) | (false, _, 4) if span > 0.0 => {
                let mut u = (t - first.time) / span;
                let n = u.floor();
                u -= n;
                if (n as i64) & 1 == 1 {
                    u = 1.0 - u;
                }
                t = first.time + u * span;
            }
            _ => {
                t = t.clamp(first.time, last.time);
            }
        }
    }
    // 区間を探す。時刻が等しいキーは0除算を避けて先の値を返す。
    for pair in keys.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        if t < a.time || t > b.time {
            continue;
        }
        let dt = b.time - a.time;
        if dt <= 0.0 {
            return a.value;
        }
        // Hermite補間。Unityは傾きに区間幅を掛けて使う。
        let u = (t - a.time) / dt;
        let u2 = u * u;
        let u3 = u2 * u;
        let m0 = a.out_slope * dt;
        let m1 = b.in_slope * dt;
        return (2.0 * u3 - 3.0 * u2 + 1.0) * a.value
            + (u3 - 2.0 * u2 + u) * m0
            + (-2.0 * u3 + 3.0 * u2) * b.value
            + (u3 - u2) * m1;
    }
    // 丸めで区間に載らない場合は近い方の端値を返す。
    if (t - first.time).abs() <= (t - last.time).abs() {
        first.value
    } else {
        last.value
    }
}

/// カーブ付き設定値の評価。`value × curve(t)` であり、キーが無い場合は `value` である。
fn eval_curved(curve: &SidecarCurve, t: f64) -> f64 {
    if curve.keys.is_empty() {
        curve.value
    } else {
        curve.value * eval_unity_curve(curve, t)
    }
}

/// sidecarのcolliderをVRMの形状へ換える。
///
/// `position`・`rotation` はrootローカルの値であり、componentには依存しない。
/// componentは追従対象の識別にだけ使い、値の座標換算には使わない。
/// Unity左手系からVRM右手系への鏡像（X反転・四元数のY/Z反転）だけを適用する。
/// `Sphere` は高さを無視して必ず球にする。`Capsule` は回転した+Y軸の
/// 両端から `height/2 - radius` を詰め、縮退した場合は球にする。
fn convert_collider_shape(
    id: &str,
    source: &SidecarCollider,
) -> Result<VrmSpringColliderShape, VrmError> {
    let local = trs_to_matrix(&SidecarTrs {
        position: mirror_point(source.position),
        rotation: mirror_quat(source.rotation),
        scale: [1.0, 1.0, 1.0],
    });
    let center = to_f32x3([local[12], local[13], local[14]], "collider位置")?;
    let radius = to_f32_finite(source.radius, "collider半径")?;
    if radius < 0.0 {
        return Err(VrmError::new(
            VrmErrorKind::InvalidPhysicsSidecar,
            format!("colliderの半径が0以上の有限値でない: {id}"),
        ));
    }
    match source.shape_type {
        SidecarShapeType::Sphere => Ok(VrmSpringColliderShape::Sphere {
            offset: center,
            radius,
        }),
        SidecarShapeType::Capsule => {
            let height = to_f32_finite(source.height, "collider高さ")?;
            if height < 0.0 {
                return Err(VrmError::new(
                    VrmErrorKind::InvalidPhysicsSidecar,
                    format!("colliderの高さが0以上の有限値でない: {id}"),
                ));
            }
            // 長さと半径の差が0以下の場合は球とみなす。
            let half = (f64::from(height) / 2.0 - f64::from(radius)).max(0.0);
            if half <= 0.0 {
                return Ok(VrmSpringColliderShape::Sphere {
                    offset: center,
                    radius,
                });
            }
            // 軸は姿勢のY列である。拡大率は1のため正規化済みのはずだが、
            // 丸めに備えて確かめる。縮退した姿勢は検証で弾く。
            let axis = [local[4], local[5], local[6]];
            let length = (axis[0] * axis[0] + axis[1] * axis[1] + axis[2] * axis[2]).sqrt();
            if !length.is_finite() || length == 0.0 {
                return Err(VrmError::new(
                    VrmErrorKind::InvalidPhysicsSidecar,
                    format!("colliderの軸が縮退している: {id}"),
                ));
            }
            let unit = [axis[0] / length, axis[1] / length, axis[2] / length];
            let half_f32 = to_f32_finite(half, "collider半長")?;
            let axis_f32 = to_f32x3(unit, "collider軸")?;
            let offset = [
                center[0] - axis_f32[0] * half_f32,
                center[1] - axis_f32[1] * half_f32,
                center[2] - axis_f32[2] * half_f32,
            ];
            let tail = [
                center[0] + axis_f32[0] * half_f32,
                center[1] + axis_f32[1] * half_f32,
                center[2] + axis_f32[2] * half_f32,
            ];
            Ok(VrmSpringColliderShape::Capsule {
                offset,
                radius,
                tail,
            })
        }
        SidecarShapeType::Plane => Err(VrmError::new(
            VrmErrorKind::InvalidPhysicsSidecar,
            // 解決側で除外済みのため到達しない。黙って球にしない。
            format!("平面colliderは換算できない: {id}"),
        )),
    }
}

/// `PhysicsResolution` と `PhysicsSidecar` から `VrmSpringBones` を作る。
///
/// PhysBoneごとにsidecarの親子と `siblingIndex` 順の木を作り、
/// `ignore_nodes` の部分木と（`ignoreOtherPhysBones` が真の場合だけ）
/// 他のPhysBoneのrootの部分木を除く。分岐のAimは `multiChildType` で決め、
/// 所有権は解決順の先勝ちで確定する。springは所有しAimする長さのある骨のうち、
/// 親がAimしていないものをheadとして始め、所有するAim先を辿る。
/// 長さの無い骨はheadにならず、前springの端末として終わる。
/// colliderはroot基準で換算する。`position`・`rotation` はrootローカルの値であり、
/// 鏡像だけを適用する。componentは追従対象の識別にだけ使う。
/// endpoint生成・平均tail生成・ゼロ長分割・limit・center・曲線の再現はしない。
/// パラメータは第一近似であり、詳細は [`SpringCompileReport`] に残る。
/// 参照実装の完全移植ではなく、近似であることを診断と文書に明記する。
///
/// 失敗時は `VrmModel` を変更しない。呼び出し側が結果を受け取ってから代入する。
///
/// # Errors
///
/// 上限の0指定・f32に収まらない値・負の半径や硬さ・対応表とsidecarの不整合の場合に返す。
pub fn compile_springs(
    sidecar: &PhysicsSidecar,
    model: &VrmModel,
    resolution: &PhysicsResolution,
    options: &SpringCompileOptions,
) -> Result<CompiledSprings, VrmError> {
    if options.max_joints == 0 {
        return Err(VrmError::new(
            VrmErrorKind::LimitExceeded,
            "1springのjoint上限が0である",
        ));
    }
    let bone_at: HashMap<&str, &SidecarPhysBone> = sidecar
        .phys_bones
        .iter()
        .map(|bone| (bone.id.as_str(), bone))
        .collect();
    let collider_at: HashMap<&str, &SidecarCollider> = sidecar
        .colliders
        .iter()
        .map(|collider| (collider.id.as_str(), collider))
        .collect();
    // sidecarノードの索引付け。木・所有権・Aimはsidecarの親子と
    // `siblingIndex` 順で行い、VrmModel（FBX）の子順は使わない。
    let sidecar_at: HashMap<&str, usize> = sidecar
        .nodes
        .iter()
        .enumerate()
        .map(|(index, node)| (node.id.as_str(), index))
        .collect();
    let mut children_at: HashMap<usize, Vec<usize>> = HashMap::new();
    for (index, node) in sidecar.nodes.iter().enumerate() {
        if let Some(parent) = node.parent_id.as_deref()
            && let Some(&parent_index) = sidecar_at.get(parent)
        {
            children_at.entry(parent_index).or_default().push(index);
        }
    }
    for children in children_at.values_mut() {
        children.sort_by_key(|&index| {
            (
                sidecar.nodes[index].sibling_index,
                sidecar.nodes[index].id.as_str(),
            )
        });
    }
    // 解決表を逆引きし、Vrm索引からsidecar索引を求める。
    // 対応は完全一致のため、写像先が複数になることはない。
    let vrm_to_sidecar: HashMap<usize, usize> = resolution
        .node_map
        .iter()
        .map(|(id, &vrm)| (vrm, sidecar_at[id.as_str()]))
        .collect();
    let sidecar_worlds = sidecar_world_matrices(sidecar)?;
    // avatar rootは所有しない。rootの子から開始・所有する。
    let avatar_root = sidecar_at.get(sidecar.capture.avatar_root_id.as_str());
    // 他のPhysBoneのroot集合（sidecar索引）。`ignoreOtherPhysBones` が真の
    // 場合だけ木の構築時に部分木ごと切る。対応表は切断の診断に使う。
    let mut root_bone: HashMap<usize, &str> = HashMap::new();
    for bone in &resolution.phys_bones {
        if let Some(&sidecar_index) = vrm_to_sidecar.get(&bone.root_node) {
            root_bone.insert(sidecar_index, bone.id.as_str());
        }
    }
    let mut report = SpringCompileReport::default();
    // 解決順（sidecarのcapture順）にPlanを作り、ノードを最初のPlanが所有する。
    // 所有権争いはここで確定し、後のspringは所有骨だけを使う。
    let mut plans: Vec<ChainPlan<'_>> = Vec::with_capacity(resolution.phys_bones.len());
    let mut owner_of: HashMap<usize, &str> = HashMap::new();
    for bone in &resolution.phys_bones {
        let Some(source) = bone_at.get(bone.id.as_str()) else {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPhysicsSidecar,
                format!("解決済みPhysBoneに対応するsidecarが無い: {}", bone.id),
            ));
        };
        if !source.enabled {
            report.disabled_bones.push(bone.id.clone());
        }
        let Some(&root_sidecar) = vrm_to_sidecar.get(&bone.root_node) else {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPhysicsSidecar,
                format!("解決済みPhysBoneのrootに対応するsidecarが無い: {}", bone.id),
            ));
        };
        let tree = build_plan_tree(
            sidecar,
            bone,
            source,
            root_sidecar,
            &children_at,
            &vrm_to_sidecar,
            &root_bone,
            &mut report,
        )?;
        if tree.is_empty() {
            // rootが無視指定に含まれる場合だけ空になる。黙って落とさず記録する。
            report.skipped_bones.push(SkippedBone {
                bone: bone.id.clone(),
                reason: SkippedBoneReason::RootIgnored,
            });
            continue;
        }
        let mut owned: Vec<usize> = Vec::new();
        for node in tree.order() {
            // avatar rootは所有しない。rootの子から開始・所有する。
            if Some(&node) == avatar_root {
                continue;
            }
            if let Some(&owner) = owner_of.get(&node) {
                let id = sidecar.nodes[node].id.clone();
                let shared = report
                    .shared_nodes
                    .iter_mut()
                    .find(|entry| entry.node == id);
                match shared {
                    Some(entry) => {
                        if !entry.lost_by.contains(&bone.id) {
                            entry.lost_by.push(bone.id.clone());
                        }
                    }
                    None => report.shared_nodes.push(SharedNode {
                        node: id,
                        owner: owner.to_owned(),
                        lost_by: vec![bone.id.clone()],
                    }),
                }
                report.lost_nodes.push(LostNode {
                    bone: bone.id.clone(),
                    node: sidecar.nodes[node].id.clone(),
                    owner: owner.to_owned(),
                });
            } else {
                owner_of.insert(node, bone.id.as_str());
                owned.push(node);
            }
        }
        plans.push(ChainPlan {
            bone,
            source,
            root: root_sidecar,
            tree,
            owned,
        });
    }
    report.shared_nodes.sort_by(|a, b| a.node.cmp(&b.node));
    // springと対応する解決済みPhysBoneの組。
    // Planごとのspring範囲も記録し、center割当で使う。
    let mut spring_entries: Vec<(&ResolvedPhysBone, VrmSpring)> = Vec::new();
    let mut plan_ranges: Vec<std::ops::Range<usize>> = vec![0..0; plans.len()];
    for (plan_index, plan) in plans.iter().enumerate() {
        let spring_start = spring_entries.len();
        let bone = plan.bone;
        let source = plan.source;
        let owned: HashSet<usize> = plan.owned.iter().copied().collect();
        let head_list = plan.heads(&owned, &sidecar_worlds, sidecar, &mut report);
        if head_list.is_empty() {
            // 所有したAim骨が無い場合。root共有などで所有を奪われた場合に起こる。
            let reason = match owner_of.get(&plan.root) {
                Some(&owner) if owner != bone.id.as_str() => SkippedBoneReason::RootShared {
                    owner: owner.to_owned(),
                },
                _ => SkippedBoneReason::NoHeads,
            };
            report.skipped_bones.push(SkippedBone {
                bone: bone.id.clone(),
                reason,
            });
            continue;
        }
        let (hit_radius, stiffness, drag_force) =
            approx_joint_params(&bone.id, source, &mut report)?;
        for node in head_list {
            let (joints, truncated) = trace_spring(
                plan,
                sidecar,
                node,
                &owned,
                &sidecar_worlds,
                options.max_joints,
                &mut report,
            );
            // 関節をVrm索引へ写す。未解決ノードが混ざる鎖は落とし、
            // 飛ばして接続はしない。
            let mut vrm_joints = Vec::with_capacity(joints.len());
            let mut unmapped: Option<String> = None;
            for joint in &joints {
                match resolution.node_map.get(&sidecar.nodes[*joint].id) {
                    Some(&vrm) => vrm_joints.push(vrm),
                    None => {
                        unmapped = Some(sidecar.nodes[*joint].id.clone());
                        break;
                    }
                }
            }
            if let Some(node) = unmapped {
                report.skipped_bones.push(SkippedBone {
                    bone: bone.id.clone(),
                    reason: SkippedBoneReason::UnmappedJoint { node },
                });
                continue;
            }
            let name = {
                let head_name = model.nodes[vrm_joints[0]].name.clone();
                if head_name.is_empty() {
                    bone.id.clone()
                } else {
                    head_name
                }
            };
            if truncated {
                report.truncated_chains.push(name.clone());
            }
            if vrm_joints.len() < 2 {
                // Aim先が所有外で1関節だけになったspringは作らない。
                // シミュレーションの区間を持たないためである。
                // 切り詰めで1件になった場合は切り詰めの記録が残る。
                if !truncated {
                    report.skipped_bones.push(SkippedBone {
                        bone: bone.id.clone(),
                        reason: SkippedBoneReason::AimTargetUnowned,
                    });
                }
                continue;
            }
            let joints = vrm_joints
                .into_iter()
                .map(|node| VrmSpringJoint {
                    node,
                    hit_radius,
                    stiffness,
                    gravity_power: 0.0,
                    gravity_dir: [0.0, -1.0, 0.0],
                    drag_force,
                })
                .collect::<Vec<_>>();
            report.joint_count += joints.len();
            report.gravity_zero_joints += joints.len();
            // endpointは今回は付けない。近似として記録する。
            report.no_endpoint.push(bone.id.clone());
            if source.multi_child_type != SidecarMultiChildType::Ignore
                && !report.first_child_multichild.contains(&bone.id)
            {
                report.first_child_multichild.push(bone.id.clone());
            }
            // 平面参照の落下は骨ごとに記録する。全体の一覧は解決側が持つ。
            if source
                .collider_ids
                .iter()
                .any(|id| resolution.skipped_colliders.iter().any(|skip| skip == id))
                && !report.plane_colliders_dropped.contains(&bone.id)
            {
                report.plane_colliders_dropped.push(bone.id.clone());
            }
            report.owned_springs.push(OwnedSpring {
                bone: bone.id.clone(),
                name: name.clone(),
                joints: joints.len(),
            });
            spring_entries.push((
                bone,
                VrmSpring {
                    name,
                    joints,
                    center: None,
                    collider_groups: Vec::new(),
                },
            ));
        }
        plan_ranges[plan_index] = spring_start..spring_entries.len();
    }
    report.spring_count = spring_entries.len();
    assign_centers(
        sidecar,
        &plans,
        &plan_ranges,
        &sidecar_at,
        &vrm_to_sidecar,
        &resolution.node_map,
        &mut spring_entries,
        &mut report,
    );
    // centerが付かなかったspringは記録に残す。
    for (bone, spring) in spring_entries.iter() {
        if spring.center.is_none() {
            report.center_missing.push(bone.id.clone());
        }
    }
    // colliderは作ったspringが参照するものだけを、解決順に並べて出力する。
    let created: HashSet<&str> = spring_entries
        .iter()
        .map(|(bone, _)| bone.id.as_str())
        .collect();
    let mut referenced_colliders: HashSet<&str> = HashSet::new();
    for bone in &resolution.phys_bones {
        if !created.contains(bone.id.as_str()) {
            continue;
        }
        let Some(source) = bone_at.get(bone.id.as_str()) else {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPhysicsSidecar,
                format!("解決済みPhysBoneに対応するsidecarが無い: {}", bone.id),
            ));
        };
        for collider in &source.collider_ids {
            if resolution.skipped_colliders.iter().any(|id| id == collider) {
                continue;
            }
            if collider_at.contains_key(collider.as_str()) {
                referenced_colliders.insert(collider.as_str());
            }
        }
    }
    // colliderは参照されるものだけを、解決順に並べて出力する。
    let mut collider_index: HashMap<&str, usize> = HashMap::new();
    let mut colliders = Vec::new();
    for resolved in &resolution.colliders {
        if !referenced_colliders.contains(resolved.id.as_str()) {
            report.unused_colliders.push(resolved.id.clone());
            continue;
        }
        let Some(source) = collider_at.get(resolved.id.as_str()) else {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPhysicsSidecar,
                format!("解決済みcolliderに対応するsidecarが無い: {}", resolved.id),
            ));
        };
        if !source.enabled {
            report.disabled_colliders.push(resolved.id.clone());
        }
        if source.inside_bounds {
            report
                .ignored_collider_options
                .push(format!("{}.insideBounds", resolved.id));
        }
        if source.bones_as_spheres {
            report
                .ignored_collider_options
                .push(format!("{}.bonesAsSpheres", resolved.id));
        }
        collider_index.insert(resolved.id.as_str(), colliders.len());
        // 出力ノードはrootであり、姿勢はrootローカルのまま鏡像だけ換算する。
        colliders.push(VrmSpringCollider {
            node: resolved.root_node,
            shape: convert_collider_shape(&resolved.id, source)?,
        });
    }
    report.collider_count = colliders.len();
    // グループはPhysBoneの参照集合ごとに作り、同じ集合は順序や重複によらず共有する。
    // 順序に依存しないよう重複を除いて並べ替え、鍵と出力の両方に使う。
    let mut group_at: HashMap<Vec<usize>, usize> = HashMap::new();
    let mut collider_groups = Vec::new();
    for (bone, spring) in &mut spring_entries {
        let Some(source) = bone_at.get(bone.id.as_str()) else {
            continue;
        };
        let mut members = Vec::new();
        for collider in &source.collider_ids {
            if let Some(&index) = collider_index.get(collider.as_str()) {
                members.push(index);
            }
        }
        if members.is_empty() {
            continue;
        }
        members.sort_unstable();
        members.dedup();
        let group = match group_at.get(&members) {
            Some(&index) => index,
            None => {
                let index = collider_groups.len();
                collider_groups.push(VrmSpringColliderGroup {
                    name: format!("{}-colliders", bone.id),
                    colliders: members.clone(),
                });
                group_at.insert(members, index);
                index
            }
        };
        spring.collider_groups.push(group);
    }
    report.group_count = collider_groups.len();
    let springs = spring_entries
        .into_iter()
        .map(|(_, spring)| spring)
        .collect::<Vec<_>>();
    Ok(CompiledSprings {
        bones: VrmSpringBones {
            springs,
            colliders,
            collider_groups,
        },
        report,
    })
}

/// 1本のPhysBoneの計画。木と所有骨を持つ。
/// ノード索引はすべてsidecar索引であり、Vrm索引ではない。
struct ChainPlan<'a> {
    /// 解決済みPhysBone。
    bone: &'a ResolvedPhysBone,
    /// 対応するsidecar PhysBone。
    source: &'a SidecarPhysBone,
    /// rootのsidecar索引。
    root: usize,
    /// rootからのDFS木。無視部分木と他のPhysBoneのrootを除く。
    tree: PlanTree,
    /// このPlanが所有するノード。木のDFS順である。
    owned: Vec<usize>,
}

/// Planの木。DFS順の骨列と索引を持つ。
#[derive(Debug, Default)]
struct PlanTree {
    /// DFS順の骨。
    bones: Vec<TreeBone>,
    /// ノード索引から骨位置への対応表。
    at: HashMap<usize, usize>,
}

/// 木の骨1件。索引と深さはsidecar基準である。
#[derive(Debug)]
struct TreeBone {
    /// sidecarノード索引。
    node: usize,
    /// 木の親ノード。rootは `None` である。
    parent: Option<usize>,
    /// rootからの深さ。rootは0である。
    depth: usize,
    /// Aimする子。無い場合は `None` である。
    aim: Option<usize>,
}

impl PlanTree {
    /// 空の木かどうか。
    fn is_empty(&self) -> bool {
        self.bones.is_empty()
    }

    /// 木のDFS順のノード列。
    fn order(&self) -> impl Iterator<Item = usize> + '_ {
        self.bones.iter().map(|bone| bone.node)
    }

    /// ノードの骨を引く。
    fn bone(&self, node: usize) -> Option<&TreeBone> {
        self.at.get(&node).and_then(|&index| self.bones.get(index))
    }
}

/// PhysBoneの木を作る。rootから子孫をsidecarの親子と `siblingIndex` 順で
/// DFSし、無視部分木を除く。他のPhysBoneのrootの部分木は
/// `ignoreOtherPhysBones` が真の場合だけ除く。分岐のAimは `multiChildType`
/// で決め、`Average` は今回は最初の子として扱い記録する（平均tail生成は次段階）。
/// 循環は訪問済み判定で止まる。索引はすべてsidecar由来のため範囲外にならない。
#[allow(clippy::too_many_arguments)]
fn build_plan_tree(
    sidecar: &PhysicsSidecar,
    bone: &ResolvedPhysBone,
    source: &SidecarPhysBone,
    root_sidecar: usize,
    children_at: &HashMap<usize, Vec<usize>>,
    vrm_to_sidecar: &HashMap<usize, usize>,
    root_bone: &HashMap<usize, &str>,
    report: &mut SpringCompileReport,
) -> Result<PlanTree, VrmError> {
    let node_id = |index: usize| sidecar.nodes[index].id.clone();
    // 無視部分木の閉包。無視ノード以下の子孫へは降りない。
    // 無視指定はVrm索引のため、sidecar索引へ写してから辿る。
    let mut ignored = HashSet::new();
    let mut stack: Vec<usize> = bone
        .ignore_nodes
        .iter()
        .filter_map(|vrm| vrm_to_sidecar.get(vrm).copied())
        .collect();
    while let Some(node) = stack.pop() {
        if !ignored.insert(node) {
            continue;
        }
        if let Some(children) = children_at.get(&node) {
            stack.extend(children.iter().copied());
        }
    }
    if ignored.contains(&root_sidecar) {
        return Ok(PlanTree::default());
    }
    // 他のPhysBoneのrootは `ignoreOtherPhysBones` が真の場合だけ切る。
    let cut_roots = source.ignore_other_phys_bones;
    let mut tree = PlanTree::default();
    // 未訪問だけを積むため、辺は必ず新規ノードへ向かう。
    // 挿入順が位相順になり、循環は起きない。
    let mut pending: Vec<(usize, Option<usize>, usize)> = vec![(root_sidecar, None, 0)];
    while let Some((node, parent, depth)) = pending.pop() {
        if tree.at.contains_key(&node) {
            continue;
        }
        let children: Vec<usize> = children_at
            .get(&node)
            .map(|siblings| {
                siblings
                    .iter()
                    .copied()
                    .filter(|child| !ignored.contains(child))
                    .filter(|child| {
                        if cut_roots && *child != root_sidecar && root_bone.contains_key(child) {
                            report.cut_at_other_roots.push(RootCut {
                                bone: bone.id.clone(),
                                node: node_id(*child),
                                other_bone: root_bone[child].to_owned(),
                            });
                            return false;
                        }
                        true
                    })
                    .filter(|child| !tree.at.contains_key(child))
                    .collect()
            })
            .unwrap_or_default();
        let aim = match children.as_slice() {
            [] => None,
            [single] => Some(*single),
            [first, rest @ ..] => match source.multi_child_type {
                SidecarMultiChildType::First => {
                    report.branch_splits.push(BranchSplit {
                        bone: bone.id.clone(),
                        node: node_id(node),
                        aimed: Some(node_id(*first)),
                        heads: rest.iter().map(|&child| node_id(child)).collect(),
                    });
                    Some(*first)
                }
                SidecarMultiChildType::Ignore => {
                    report.branch_splits.push(BranchSplit {
                        bone: bone.id.clone(),
                        node: node_id(node),
                        aimed: None,
                        heads: children.iter().map(|&child| node_id(child)).collect(),
                    });
                    None
                }
                SidecarMultiChildType::Average => {
                    // 平均tailは作らず、最初の子に畳む。近似として記録する。
                    report.branch_splits.push(BranchSplit {
                        bone: bone.id.clone(),
                        node: node_id(node),
                        aimed: Some(node_id(*first)),
                        heads: rest.iter().map(|&child| node_id(child)).collect(),
                    });
                    if !report.average_approximated.contains(&bone.id) {
                        report.average_approximated.push(bone.id.clone());
                    }
                    Some(*first)
                }
            },
        };
        tree.at.insert(node, tree.bones.len());
        tree.bones.push(TreeBone {
            node,
            parent,
            depth,
            aim,
        });
        for &child in children.iter().rev() {
            pending.push((child, Some(node), depth + 1));
        }
    }
    Ok(tree)
}

impl ChainPlan<'_> {
    /// springのheadになる所有骨をDFS順で返す。
    ///
    /// headは所有しAimし、長さを持つ骨であり、親が無い・親が所有外・
    /// 親が自分をAimしていない・親との区間が長さ0のいずれかである。
    /// 長さの無い骨はheadにならず、前springの端末として終わる。
    /// 長さ0の親区間でheadになる場合はここで記録する。
    /// avatar rootは所有の段階で除外済みのため、ここには現れない。
    fn heads(
        &self,
        owned: &HashSet<usize>,
        worlds: &[[f64; 16]],
        sidecar: &PhysicsSidecar,
        report: &mut SpringCompileReport,
    ) -> Vec<usize> {
        let mut heads = Vec::new();
        for bone in &self.tree.bones {
            if !owned.contains(&bone.node) {
                continue;
            }
            let Some(aim) = bone.aim else {
                continue;
            };
            if is_zero_length(worlds, bone.node, aim) {
                continue;
            }
            let head = match bone.parent {
                None => true,
                Some(parent) if !owned.contains(&parent) => true,
                Some(parent) => {
                    let aims_at = self
                        .tree
                        .bone(parent)
                        .and_then(|entry| entry.aim)
                        .is_some_and(|aim| aim == bone.node);
                    if !aims_at {
                        true
                    } else if is_zero_length(worlds, parent, bone.node) {
                        // 長さ0の親区間でheadになる。trace側では重ねないため、
                        // ここで記録する。
                        report.zero_length_joints.push(ZeroLengthJoint {
                            bone: self.bone.id.clone(),
                            parent: sidecar.nodes[parent].id.clone(),
                            child: sidecar.nodes[bone.node].id.clone(),
                        });
                        true
                    } else {
                        false
                    }
                }
            };
            if head {
                heads.push(bone.node);
            }
        }
        heads
    }
}

/// centerを割り当てる。springは作り終えた後で、全体を見て決める。
///
/// AllMotionはchain rootの親、Worldはavatar root直下の最上位祖先を起点にし、
/// 先頭jointの祖先であり、他のspringの内側でない最初のノードをcenterにする。
/// 未解決ノードは飛ばして親へ進む。
#[allow(clippy::too_many_arguments)]
fn assign_centers(
    sidecar: &PhysicsSidecar,
    plans: &[ChainPlan<'_>],
    plan_ranges: &[std::ops::Range<usize>],
    sidecar_at: &HashMap<&str, usize>,
    vrm_to_sidecar: &HashMap<usize, usize>,
    node_map: &HashMap<String, usize>,
    spring_entries: &mut [(&ResolvedPhysBone, VrmSpring)],
    report: &mut SpringCompileReport,
) {
    let parent_of = |index: usize| -> Option<usize> {
        sidecar.nodes[index]
            .parent_id
            .as_deref()
            .and_then(|id| sidecar_at.get(id))
            .copied()
    };
    let name_of = |index: usize| sidecar.nodes[index].id.clone();
    // headのsidecar索引。spring順に並べ、LegalCenterの内側判定に使う。
    // 落とされたspringは無いため、全springが1件ずつ対応する。
    let head_sidecars: Vec<Option<usize>> = spring_entries
        .iter()
        .map(|(_, spring)| {
            spring
                .joints
                .first()
                .and_then(|joint| vrm_to_sidecar.get(&joint.node).copied())
        })
        .collect();
    for (plan, range) in plans.iter().zip(plan_ranges.iter()) {
        let owned: HashSet<usize> = plan.owned.iter().copied().collect();
        let im = plan_immobile(plan, &owned, plan.source);
        if range.is_empty() {
            continue;
        }
        if !im.uses {
            continue;
        }
        if im.holds {
            report.held_immobile.push(plan.bone.id.clone());
            continue;
        }
        if im.is_world && im.root_value < WORLD_CENTER_THRESHOLD {
            report.weak_world.push(plan.bone.id.clone());
            continue;
        }
        // 候補：AllMotionはchain rootの親、Worldは鎖の最上位祖先。
        // 参照実装のTopmostBelowAvatarRootに相当し、実アバタールート
        // （Milfyのようにsidecarに含まれない）は辿れないため、
        // sidecarの最上位（Armatureなど）が候補になる。
        let candidate = if im.is_world {
            let mut topmost = None;
            let mut current = Some(plan.root);
            let mut seen = HashSet::new();
            while let Some(node) = current {
                if !seen.insert(node) {
                    break;
                }
                topmost = Some(node);
                current = parent_of(node);
            }
            topmost
        } else {
            parent_of(plan.root)
        };
        // LegalCenter相当：先頭jointの祖先であり、他のspringの内側でない
        // 最初のノードを探す。未解決は飛ばして親へ進む。
        for index in range.clone() {
            let Some((_, spring)) = spring_entries.get(index) else {
                continue;
            };
            let Some(head_joint) = spring.joints.first() else {
                continue;
            };
            let Some(&head) = vrm_to_sidecar.get(&head_joint.node) else {
                continue;
            };
            let mut current = candidate;
            let mut found = None;
            let mut seen = HashSet::new();
            while let Some(node) = current {
                // 除外するのは実アバタールートのみであり、sidecarには無い。
                // 循環だけを打ち切る。
                if !seen.insert(node) {
                    break;
                }
                if node_map.contains_key(name_of(node).as_str())
                    && is_ancestor_or_self(sidecar, sidecar_at, head, node)
                {
                    // 他のspringの内側か調べる。
                    let mut inside = false;
                    for (other, other_head) in head_sidecars.iter().enumerate() {
                        if other == index {
                            continue;
                        }
                        let Some(other_head) = *other_head else {
                            continue;
                        };
                        if is_ancestor_or_self(sidecar, sidecar_at, node, other_head) {
                            inside = true;
                            break;
                        }
                    }
                    if !inside {
                        found = Some(node);
                        break;
                    }
                }
                current = parent_of(node);
            }
            match found {
                Some(node) => {
                    let vrm = node_map[name_of(node).as_str()];
                    if let Some((_, spring)) = spring_entries.get_mut(index) {
                        spring.center = Some(vrm);
                        report.center_applied.push(CenterAssigned {
                            bone: plan.bone.id.clone(),
                            spring: spring.name.clone(),
                            node: vrm,
                            kind: if im.is_world {
                                CenterKind::World
                            } else {
                                CenterKind::AllMotion
                            },
                        });
                    }
                }
                None => {
                    if let Some((_, spring)) = spring_entries.get(index) {
                        report.no_legal_center.push(NoCenter {
                            bone: plan.bone.id.clone(),
                            name: spring.name.clone(),
                            reason: NoCenterReason::NoLegalNode,
                        });
                    }
                }
            }
        }
    }
}

/// headからAimを辿り、関節列を作る。所有骨だけを通り、Aimなし・所有外・
/// 上限・長さ0で止まる。端末の骨も関節に含み、親子連鎖を保つ。
/// ゼロ長区間の先がheadになる場合は重ねずに終わり、先が自身のspringを始める。
/// 辺は挿入順が位相順のため、循環で無限反復しない。
#[allow(clippy::too_many_arguments)]
fn trace_spring(
    plan: &ChainPlan<'_>,
    sidecar: &PhysicsSidecar,
    head: usize,
    owned: &HashSet<usize>,
    worlds: &[[f64; 16]],
    max_joints: usize,
    report: &mut SpringCompileReport,
) -> (Vec<usize>, bool) {
    let node_id = |index: usize| sidecar.nodes[index].id.clone();
    // 長さを持つかどうか。Aim先があり、区間が長さ0でない場合である。
    let has_length = |node: usize| {
        plan.tree
            .bone(node)
            .and_then(|bone| bone.aim)
            .is_some_and(|aim| !is_zero_length(worlds, node, aim))
    };
    let mut joints = vec![head];
    let mut truncated = false;
    let mut current = head;
    while let Some(next) = plan.tree.bone(current).and_then(|entry| entry.aim) {
        if !owned.contains(&next) {
            break;
        }
        if joints.len() >= max_joints {
            truncated = true;
            break;
        }
        let zero_segment = is_zero_length(worlds, current, next);
        if zero_segment && has_length(next) {
            // 長さの無い区間の先がheadになる場合は重ねない。
            // 記録はhead側の判定で行う。
            break;
        }
        if zero_segment {
            report.zero_length_joints.push(ZeroLengthJoint {
                bone: plan.bone.id.clone(),
                parent: node_id(current),
                child: node_id(next),
            });
        }
        joints.push(next);
        if !has_length(next) {
            break;
        }
        current = next;
    }
    (joints, truncated)
}

/// 関節パラメータの第一近似を求める。
///
/// `hit_radius` と `stiffness` はbase値を使い、曲線のキー列は評価しない。
/// `drag_force` は `1 - spring` を0〜1に丸める。`gravity` 系は使わない。
/// 近似した箇所は報告に残す。参照実装の式への置き換えが前提である。
fn approx_joint_params(
    bone: &str,
    source: &SidecarPhysBone,
    report: &mut SpringCompileReport,
) -> Result<(f32, f32, f32), VrmError> {
    for (field, curve) in [
        ("radius", &source.radius),
        ("spring", &source.spring),
        ("stiffness", &source.stiffness),
    ] {
        if !curve.keys.is_empty() {
            report.curve_base_used.push(CurveBaseUsed {
                bone: bone.to_owned(),
                field,
                keys: curve.keys.len(),
            });
        }
    }
    let hit_radius = to_f32_finite(source.radius.value, "半径")?;
    if hit_radius < 0.0 {
        return Err(VrmError::new(
            VrmErrorKind::InvalidPhysicsSidecar,
            format!("半径が0以上の有限値でない: {bone}"),
        ));
    }
    let stiffness = to_f32_finite(source.stiffness.value, "硬さ")?;
    if !stiffness.is_finite() || stiffness < 0.0 {
        return Err(VrmError::new(
            VrmErrorKind::InvalidPhysicsSidecar,
            format!("硬さが0以上の有限値でない: {bone}"),
        ));
    }
    let spring_base = to_f32_finite(source.spring.value, "ばね")?;
    let raw = 1.0 - spring_base;
    let drag_force = raw.clamp(0.0, 1.0);
    if drag_force != raw {
        report.drag_clamped.push(bone.to_owned());
    }
    Ok((hit_radius, stiffness, drag_force))
}

/// world位置の差がほぼ0かどうか。ゼロ長区間の検出に使う。
fn is_zero_length(worlds: &[[f64; 16]], parent: usize, child: usize) -> bool {
    let Some(from) = worlds.get(parent) else {
        return false;
    };
    let Some(to) = worlds.get(child) else {
        return false;
    };
    let dx = to[12] - from[12];
    let dy = to[13] - from[13];
    let dz = to[14] - from[14];
    (dx * dx + dy * dy + dz * dz).sqrt() < ZERO_LENGTH_TOLERANCE
}

/// 木の深さ位置。端末が無い鎖は0になる。
fn chain_t(depth: usize, max_depth: usize) -> f64 {
    if max_depth <= 1 {
        0.0
    } else {
        (depth as f64 / (max_depth - 1) as f64).clamp(0.0, 1.0)
    }
}

/// Planのimmobile状態。centerを付けるかどうかの判断材料である。
struct PlanImmobile {
    /// chain rootのimmobile値（t=0）。
    root_value: f64,
    /// いずれかの所有骨でimmobileが非0。
    uses: bool,
    /// AllMotionかつ部分的なimmobile。centerの代わりに強いpullで近似する（次単位）。
    holds: bool,
    /// World型かどうか。
    is_world: bool,
}

/// Planのimmobile状態を求める。Tは深さ位置、値は `value × curve(t)` である。
fn plan_immobile(
    plan: &ChainPlan<'_>,
    owned: &HashSet<usize>,
    source: &SidecarPhysBone,
) -> PlanImmobile {
    let max_depth = plan
        .tree
        .bones
        .iter()
        .map(|bone| bone.depth)
        .max()
        .unwrap_or(0);
    let root_value = eval_curved(&source.immobile, 0.0);
    let is_world = source.immobile_type == SidecarImmobileType::World;
    let mut uses = root_value != 0.0;
    let mut full = root_value == 1.0;
    for bone in &plan.tree.bones {
        if !owned.contains(&bone.node) {
            continue;
        }
        let value = eval_curved(&source.immobile, chain_t(bone.depth, max_depth));
        uses |= value != 0.0;
        full &= value == 1.0;
    }
    PlanImmobile {
        root_value,
        uses,
        holds: uses && !full && !is_world,
        is_world,
    }
}

/// World immobileがcenterを持つ下限。参照実装と同じ値である。
const WORLD_CENTER_THRESHOLD: f64 = 0.5;

/// sidecar階層で `node` が `ancestor` の子孫または自身かどうか。
/// 循環に備えて訪問済みで打ち切る。
fn is_ancestor_or_self(
    sidecar: &PhysicsSidecar,
    at: &HashMap<&str, usize>,
    mut node: usize,
    ancestor: usize,
) -> bool {
    let mut seen = HashSet::new();
    loop {
        if node == ancestor {
            return true;
        }
        if !seen.insert(node) {
            return false;
        }
        let Some(parent) = sidecar.nodes[node]
            .parent_id
            .as_deref()
            .and_then(|id| at.get(id))
            .copied()
        else {
            return false;
        };
        node = parent;
    }
}
