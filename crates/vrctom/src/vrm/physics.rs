//! Unityヘルパが出力する物理sidecarの読み取りと対応解決、スプリングの最小出力。
//!
//! 入口は [`PhysicsSidecar::parse`]（バイト列の検証）と
//! [`VrmModel::resolve_physics_sidecar`](crate::vrm::VrmModel::resolve_physics_sidecar)
//! （`VrmModel` のノードへの対応付け）、[`compile_springs`]（`VrmSpringBones` の
//! 生成）である。endpoint・平均tail・limit拡張の出力は次の作業単位であり、
//! ここでは分岐分割した鎖と参照実装の式による関節値を出す。

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
    max_constraints: usize,
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
            // 実データの十数件が収まる大きさである。
            max_constraints: 10_000,
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

    /// 制約数の上限を変更する。
    pub fn with_max_constraints(mut self, value: usize) -> Self {
        self.max_constraints = value;
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
    /// collider列。参照されるsphere/capsuleと、平面（拡張付きの球）を含む。
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
    /// 角度制限。末端や制限なし・rest不明の場合は `None` である。
    /// 参照実装の `SpringJointLimit` に相当し、`VRMC_springBone_limit` として出す。
    pub limit: Option<VrmSpringJointLimit>,
}

/// スプリング関節の角度制限。角度はラジアン、回転はVRM空間の単位四元数である。
/// 参照実装の `SpringJointLimit`（`SpringLimitKind`）に相当する。
/// `Cone` はVRChatのAngle、`Hinge` はHinge、`Spherical` はPolar制限である。
#[derive(Debug, Clone)]
pub enum VrmSpringJointLimit {
    /// 円錐制限。`angle` は最大角（`maxAngleX` を180度で止めた値）である。
    Cone {
        /// 最大角（ラジアン）。
        angle: f32,
        /// 制限の枠組み（VRM空間）。
        rotation: [f32; 4],
    },
    /// 蝶番制限。`angle` は最大角（`maxAngleX` を180度で止めた値）である。
    Hinge {
        /// 最大角（ラジアン）。
        angle: f32,
        /// 制限の枠組み（VRM空間）。
        rotation: [f32; 4],
    },
    /// 球面制限。`pitch` はX軸、`yaw` はZ軸の最大角である。
    Spherical {
        /// X軸の最大角（ラジアン。180度で止める）。
        pitch: f32,
        /// Z軸の最大角（ラジアン。90度で止める）。
        yaw: f32,
        /// 制限の枠組み（VRM空間）。
        rotation: [f32; 4],
    },
}

/// スプリングcollider1件。
#[derive(Debug, Clone)]
pub struct VrmSpringCollider {
    /// 対象ノードの索引。
    pub node: usize,
    /// 形状。
    pub shape: VrmSpringColliderShape,
    /// 拡張collider。平面の場合に `Some` になる。
    /// 参照実装の拡張colliderに相当し、`VRMC_springBone_extended_collider` として出す。
    pub extended: Option<VrmSpringColliderExtension>,
}

/// ノード制約1件。対象ノードの回転を参照先から写す。
/// 参照実装の `CompiledConstraint` に相当し、
/// `VRMC_node_constraint` として出す。
#[derive(Debug, Clone)]
pub struct VrmNodeConstraint {
    /// 対象ノードの索引。
    pub node: usize,
    /// 参照先ノードの索引。
    pub source: usize,
    /// 制約の重み。0〜1である。
    pub weight: f32,
    /// 制約の種別。
    pub kind: VrmNodeConstraintKind,
}

/// ノード制約の種別。参照実装の `NodeConstraintKind` に相当する。
#[derive(Debug, Clone)]
pub enum VrmNodeConstraintKind {
    /// 回転の複写。
    Rotation,
    /// 1軸のひねり。Unity軸で表す。
    Roll {
        /// ひねりの軸。
        axis: VrmRollAxis,
    },
    /// 主軸の注視。Unity軸で表し、書き出し時にX鏡像する。
    Aim {
        /// 注視の軸。
        axis: VrmAimAxis,
    },
}

/// ひねりの軸。参照実装の `ConstraintRollAxis` に相当する。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VrmRollAxis {
    /// X軸。
    X,
    /// Y軸。
    Y,
    /// Z軸。
    Z,
}

/// 注視の軸（Unity軸）。参照実装の `ConstraintAimAxis` に相当する。
/// 書き出し時にXだけ鏡像する（`PositiveX` と `NegativeX` を入れ替える）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VrmAimAxis {
    /// +X。
    PositiveX,
    /// -X。
    NegativeX,
    /// +Y。
    PositiveY,
    /// -Y。
    NegativeY,
    /// +Z。
    PositiveZ,
    /// -Z。
    NegativeZ,
}

/// スプリングcolliderの拡張形状。平面のみを扱う。
/// 参照実装の拡張colliderの `plane` に相当する。
#[derive(Debug, Clone)]
pub enum VrmSpringColliderExtension {
    /// 平面。法線は単位ベクトル、ずれは平面上の点である。
    /// いずれもcollider基準ノードのローカル・VRM右手系・メートルである。
    Plane {
        /// 法線（単位ベクトル）。
        normal: [f32; 3],
        /// 平面上の点（ずれ）。
        offset: [f32; 3],
    },
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
    /// 制約列。無い場合は空として扱い、後方互換を保つ。
    #[serde(default)]
    pub constraints: Vec<SidecarConstraint>,
    /// 視線記録。無い旧形式の場合は `None` として扱い、後方互換を保つ。
    #[serde(rename = "lookAt", default)]
    pub look_at: Option<super::look_at::SidecarLookAt>,
    /// humanoidのT-pose記録。無い旧形式の場合は `None` として扱い、
    /// 後方互換を保つ。ある場合は [`apply_humanoid_t_pose`] で
    /// 各骨のローカル回転だけを上書きする。
    #[serde(default)]
    pub humanoid: Option<SidecarHumanoid>,
    /// 未対応機能の名残。今回は検証せず保持する。
    #[serde(rename = "unsupportedFeatures", default)]
    pub unsupported_features: Vec<String>,
}

/// humanoidのT-pose記録。Unityヘルパが `avatar.humanDescription.skeleton` と
/// `Animator.GetBoneTransform` の突き合わせから作ったものである。
///
/// 参照実装の `RigCompiler.Humanoid` と同じく、capture時の姿勢ではなく
/// humanoidのT-pose（`skeleton` のローカルTRS）を各骨の `tPose` に持つ。
/// `skeleton` に無い骨は `tPose` を持たず、Rust側はその骨を上書きしない。
#[derive(Debug, Clone, Deserialize)]
pub struct SidecarHumanoid {
    /// humanoidとして有効かどうか。`avatar.isHuman` の写しである。
    #[serde(rename = "isHuman", default)]
    pub is_human: bool,
    /// 骨列。`Animator.GetBoneTransform` の列挙順である。
    #[serde(default)]
    pub bones: Vec<SidecarHumanBone>,
}

/// humanoidの骨1件。
#[derive(Debug, Clone, Deserialize)]
pub struct SidecarHumanBone {
    /// 骨名。Unityの `HumanBodyBones` 名である。
    pub bone: String,
    /// 対応するsidecarノードのID（`n:N` 体系）である。
    #[serde(rename = "nodeId")]
    pub node_id: String,
    /// T-poseのローカルTRS。`skeleton` に無い骨は `None` である。
    #[serde(rename = "tPose", default)]
    pub t_pose: Option<SidecarHumanTransform>,
}

/// T-poseのローカルTRS。Unity空間（左手系）の生値である。
#[derive(Debug, Clone, Deserialize)]
pub struct SidecarHumanTransform {
    /// 平行移動。
    pub position: [f64; 3],
    /// 回転（四元数 `[x, y, z, w]`）。
    pub rotation: [f64; 4],
    /// 拡大率。
    pub scale: [f64; 3],
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
    /// 制限回転X軸のカーブ。キーが無い場合は1倍である。
    #[serde(rename = "limitRotationXCurve", default = "default_unit_curve")]
    pub limit_rotation_x_curve: SidecarCurve,
    /// 制限回転Y軸のカーブ。キーが無い場合は1倍である。
    #[serde(rename = "limitRotationYCurve", default = "default_unit_curve")]
    pub limit_rotation_y_curve: SidecarCurve,
    /// 制限回転Z軸のカーブ。キーが無い場合は1倍である。
    #[serde(rename = "limitRotationZCurve", default = "default_unit_curve")]
    pub limit_rotation_z_curve: SidecarCurve,
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

/// 制約の種別。未知値は検証で失敗する。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum SidecarConstraintType {
    /// 回転の複写。
    Rotation,
    /// 位置の複写（今回は出力対象外として落とす）。
    Position,
    /// 拡大率の複写（今回は出力対象外として落とす）。
    Scale,
    /// 親付け（今回は出力対象外として落とす）。
    Parent,
    /// 主軸への注視。
    Aim,
    /// +Z軸への注視。
    LookAt,
}

/// 制約の参照先1件。
#[derive(Debug, Clone, Deserialize)]
pub struct SidecarConstraintSource {
    /// 参照先ノードのID。外部参照の場合は空である。
    #[serde(rename = "nodeId", default)]
    pub node_id: String,
    /// アバター外の参照かどうか。
    #[serde(default)]
    pub external: bool,
    /// 参照の重み。
    #[serde(default)]
    pub weight: f64,
}

/// ノード制約1件。Unity・VRChat由来の制約のうち、
/// 回転・1軸ロール・主軸Aim・注視だけを `VRMC_node_constraint` へ写す。
#[derive(Debug, Clone, Deserialize)]
pub struct SidecarConstraint {
    /// 制約のID。
    pub id: String,
    /// 付属するノードのID。
    #[serde(rename = "nodeId", default)]
    pub node_id: String,
    /// 対象ノードのID。空の場合は `nodeId` を使う。
    #[serde(rename = "targetNodeId", default)]
    pub target_node_id: String,
    /// 制約の種別。
    #[serde(rename = "type")]
    pub constraint_type: SidecarConstraintType,
    /// 由来（`vrchat` など）。診断用であり、判定には使わない。
    #[serde(default)]
    pub family: String,
    /// 制約全体の重み。
    #[serde(default)]
    pub weight: f64,
    /// 有効かどうか。
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// 稼働中かどうか。
    #[serde(default = "default_true")]
    pub active: bool,
    /// ワールド固定かどうか。真の場合は出力しない。
    #[serde(rename = "freezeToWorld", default)]
    pub freeze_to_world: bool,
    /// 回転に効かせる軸列。真の軸数を数えて種別を決める。
    #[serde(rename = "affectsRotation", default)]
    pub affects_rotation: [bool; 3],
    /// 注視の軸（Unity軸）。主軸の場合だけ使う。
    #[serde(rename = "aimAxis", default)]
    pub aim_axis: [f64; 3],
    /// 参照先列。
    #[serde(default)]
    pub sources: Vec<SidecarConstraintSource>,
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

/// 係数1のカーブ。キーが無い場合の倍率として使う。
fn default_unit_curve() -> SidecarCurve {
    SidecarCurve {
        value: 1.0,
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
        if self.constraints.len() > limits.max_constraints {
            return Err(VrmError::new(
                VrmErrorKind::LimitExceeded,
                "sidecarの制約数が上限を超えた",
            ));
        }
        // 制約のID・対象・値を確認する。種別の可否は変換時に判断する。
        {
            let mut seen = HashSet::new();
            for constraint in &self.constraints {
                if constraint.id.is_empty() {
                    return Err(VrmError::new(
                        VrmErrorKind::InvalidPhysicsSidecar,
                        "制約のIDが空である",
                    ));
                }
                if !seen.insert(constraint.id.as_str()) {
                    return Err(VrmError::new(
                        VrmErrorKind::InvalidPhysicsSidecar,
                        format!("制約のIDが重複する: {}", constraint.id),
                    ));
                }
                let target = if constraint.target_node_id.is_empty() {
                    constraint.node_id.as_str()
                } else {
                    constraint.target_node_id.as_str()
                };
                if target.is_empty() {
                    return Err(VrmError::new(
                        VrmErrorKind::InvalidPhysicsSidecar,
                        format!("制約の対象ノードが空である: {}", constraint.id),
                    ));
                }
                if !constraint.weight.is_finite() {
                    return Err(VrmError::new(
                        VrmErrorKind::InvalidPhysicsSidecar,
                        format!("制約の重みが非有限である: {}", constraint.id),
                    ));
                }
                for component in constraint.aim_axis {
                    if !component.is_finite() {
                        return Err(VrmError::new(
                            VrmErrorKind::InvalidPhysicsSidecar,
                            format!("制約の注視軸が非有限である: {}", constraint.id),
                        ));
                    }
                }
                for source in &constraint.sources {
                    if !source.weight.is_finite() {
                        return Err(VrmError::new(
                            VrmErrorKind::InvalidPhysicsSidecar,
                            format!("制約の参照重みが非有限である: {}", constraint.id),
                        ));
                    }
                }
            }
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
        // 視線記録は任意であり、無い旧形式は検証しない。ある場合は目の参照と
        // 姿勢の有限値を確認する。正規化は測定時に行うためここでは求めない。
        if let Some(look) = self.look_at.as_ref() {
            super::look_at::validate_sidecar_look_at(look, &node_at)?;
        }
        // humanoid記録は任意であり、無い旧形式は検証しない。ある場合は
        // 骨名とノード参照、T-poseの有限値と回転の非ゼロを確認する。
        // 正規化は適用時に行うためここでは求めない。重複ノードの検出は
        // 適用時（[`apply_humanoid_t_pose`]）に行う。
        if let Some(humanoid) = self.humanoid.as_ref() {
            validate_sidecar_humanoid(humanoid, &node_at)?;
        }
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
                ("limitRotationXCurve", &bone.limit_rotation_x_curve),
                ("limitRotationYCurve", &bone.limit_rotation_y_curve),
                ("limitRotationZCurve", &bone.limit_rotation_z_curve),
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
fn check_trs(trs: &SidecarTrs, _id: &str) -> Result<(), VrmError> {
    check_vec3_finite(&trs.position, "参照位置")?;
    check_quat(&trs.rotation, "参照回転")?;
    check_vec3_finite(&trs.scale, "参照拡大率")?;
    // 拡大率0はデータとして成立する（非表示にするため0を入れるPrefabがある）。
    // 物理が参照するノードだけ、解決時に別途拒否する。
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

/// humanoid記録を検証する。[`PhysicsSidecar::validate`] から呼び出す。
///
/// 骨名とノード参照の存在、T-poseの位置・拡大率の有限値、回転の有限値と
/// 非ゼロを確認する。正規化と重複ノードの検出は適用時に行うため、
/// ここでは求めない。`tPose` が無い骨は `skeleton` に無い骨であり、
/// 適用時に飛ばすため検証では通す。
///
/// # Errors
///
/// 骨名の空・ノード参照の欠落・T-poseの非有限値やゼロ回転の場合に返す。
fn validate_sidecar_humanoid(
    humanoid: &SidecarHumanoid,
    node_at: &HashMap<&str, usize>,
) -> Result<(), VrmError> {
    for bone in &humanoid.bones {
        if bone.bone.is_empty() {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPhysicsSidecar,
                "humanoidの骨名が空である",
            ));
        }
        check_node_ref(
            node_at,
            &bone.node_id,
            bone.bone.as_str(),
            "humanoidの参照ノード",
        )?;
        let Some(t_pose) = bone.t_pose.as_ref() else {
            continue;
        };
        check_vec3_finite(&t_pose.position, "humanoidのT-pose位置")?;
        check_vec3_finite(&t_pose.scale, "humanoidのT-pose拡大率")?;
        if !t_pose.rotation.iter().all(|v| v.is_finite()) {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPhysicsSidecar,
                format!("humanoidのT-pose回転に非有限値がある: {}", bone.bone),
            ));
        }
        let squared = t_pose.rotation[0] * t_pose.rotation[0]
            + t_pose.rotation[1] * t_pose.rotation[1]
            + t_pose.rotation[2] * t_pose.rotation[2]
            + t_pose.rotation[3] * t_pose.rotation[3];
        if !squared.is_finite() || squared < 1e-8 {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPhysicsSidecar,
                format!("humanoidのT-pose回転がゼロである: {}", bone.bone),
            ));
        }
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
    /// sidecarノードIDからVrmノード索引への対応表。
    /// sidecarにあってVrmに無かったノードは補助ノードとして追加済みであり、
    /// 追加分も含む。
    pub node_map: HashMap<String, usize>,
    /// sidecarにあってVrmに無かったため、Transformのみの補助ノードとして
    /// 追加した記録。sidecar順である。meshを持たず、skin・humanoidの写像は変えない。
    pub auxiliary_nodes: Vec<AuxiliaryNode>,
    /// PhysBoneの参照解決。
    pub phys_bones: Vec<ResolvedPhysBone>,
    /// colliderの参照解決。平面colliderは含まない（平面はsidecarから直接出力する）。
    pub colliders: Vec<ResolvedCollider>,
    /// 解決時点では出力対象外として除外したcollider ID（平面）。
    /// コンパイル側ではsidecarから直接出力するため、出力物には含まれる。
    pub skipped_colliders: Vec<String>,
}

/// 補助ノード1件。sidecarにあってVrmに無かったノードをTransformのみで追加した記録である。
#[derive(Debug, Clone)]
pub struct AuxiliaryNode {
    /// sidecarノードID。
    pub id: String,
    /// ノード名。
    pub name: String,
    /// 追加したVrmノード索引。
    pub node: usize,
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

/// 参照ローカルの許容差。世界行列の要素差で評価する。
/// 平行移動はメートル、線形部は無次元であり、同じ尺度で比べる。
/// sidecarはf32由来であり、bind分解と行列積の丸めを許容する。
/// 実測ではManukaの脚鎖で最大1.5e-3の偏差が出る。鎖の上流のわずかな
/// 回転差がレバーアームで増幅されたものであり、見た目では区別できない。
/// 誤ったFBXとの照合はcm単位以上のずれになるため、この値でも検出できる。
const REFERENCE_WORLD_TOLERANCE: f64 = 5e-3;

impl VrmModel {
    /// 物理sidecarを `VrmModel` のノードへ対応付ける。
    ///
    /// FBXバイト列のSHA-256と `sourceModels` を照合し、一致がちょうど1件の
    /// 場合だけ進む。`VrmModel` のルートからの名前列を索引化し、sidecarの
    /// `source.path` と完全一致で対応付ける。接尾辞や一意名の自動補完はしない。
    /// sidecarにあって `VrmModel` に無いノード（Prefabで追加された
    /// オブジェクトなど）は、Transformのみの補助ノードとして `VrmModel` の
    /// 末尾に追加し、対応表に含める。追加分は既存の索引をずらさない。
    /// 参照姿勢は `edit-mode` だけ受け付け、参照ローカルの一致で
    /// 対応表を検証する。スプリングのコンパイルや出力は行わない。
    ///
    /// # Errors
    ///
    /// SHA-256の不一致・参照姿勢の未対応・0件や複数件の対応・
    /// 補助ノードの親不在・参照ローカルの不一致・上限超過の場合に返す。
    pub fn resolve_physics_sidecar(
        &mut self,
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
        // 物理が参照するID集合を集める。補助ノード化の要否と、
        // 解決後の対応漏れ検査に使う。
        // 平面colliderだけに参照されるノードは、平面経路が基準ノードを
        // 直接合成するため、補助ノード化の対象から外す（重複を避ける）。
        let mut plane_only: HashSet<&str> = HashSet::new();
        for collider in &sidecar.colliders {
            if collider.shape_type != SidecarShapeType::Plane {
                continue;
            }
            plane_only.insert(collider.component_node_id.as_str());
            plane_only.insert(collider.root_node_id.as_str());
        }
        let mut referenced: HashSet<&str> = HashSet::new();
        for bone in &sidecar.phys_bones {
            referenced.insert(bone.component_node_id.as_str());
            referenced.insert(bone.root_node_id.as_str());
            for ignore in &bone.ignore_node_ids {
                referenced.insert(ignore.as_str());
            }
        }
        for collider in &sidecar.colliders {
            // 平面colliderの参照は補助ノード化の要否では数えない。
            // 解決後の検査では平面も含めて数える。
            if collider.shape_type == SidecarShapeType::Plane {
                continue;
            }
            referenced.insert(collider.component_node_id.as_str());
            referenced.insert(collider.root_node_id.as_str());
        }
        // 制約の対象・参照先も補助ノード化の要否に数える。
        // 解決できない制約は後段で落とすが、ノード自体は出す。
        for constraint in &sidecar.constraints {
            let target = if constraint.target_node_id.is_empty() {
                constraint.node_id.as_str()
            } else {
                constraint.target_node_id.as_str()
            };
            if !target.is_empty() {
                referenced.insert(target);
            }
            if !constraint.node_id.is_empty() {
                referenced.insert(constraint.node_id.as_str());
            }
            for source in &constraint.sources {
                if !source.node_id.is_empty() {
                    referenced.insert(source.node_id.as_str());
                }
            }
        }
        // humanoidと視線の参照先も補助ノード化の要否に数える。
        if let Some(humanoid) = sidecar.humanoid.as_ref() {
            for bone in &humanoid.bones {
                referenced.insert(bone.node_id.as_str());
            }
        }
        if let Some(look_at) = sidecar.look_at.as_ref() {
            for eye in [&look_at.left_eye_node_id, &look_at.right_eye_node_id] {
                if !eye.is_empty() {
                    referenced.insert(eye.as_str());
                }
            }
        }
        // sidecarにあってVrmに無いノードは、Transformのみの補助ノードとして
        // 追加する（sidecar順）。Prefabで追加されたオブジェクト（シーン追加ノード）が
        // スプリング・コライダー・制約から参照される場合に必要になる。
        // 親子・名前・ローカル変換（`referenceLocal` のVRM右手系換算）・兄弟順は
        // sidecarの値を用いる。meshを持たず、skin・humanoidの写像は変えない。
        // 決定性はsidecar順で保つ。重複IDは検証済みのため起きず、
        // 循環は検証で拒否済み、親不在はここで失敗する。
        let mut auxiliary_nodes = Vec::new();
        let mut aux_parents: HashSet<usize> = HashSet::new();
        // 補助ノードのsidecar ID集合。座標ゲートの対象から外す。
        // 補助ノードは親の鎖の比率をそのまま継承するため、
        // 骨鎖の単一変換行列とは一般に一致しない。
        // （例：Armature直下の基準ノードはArmatureの比率になる。）
        // 一貫性は参照ローカルの換算そのもので保つ。
        let mut aux_ids: HashSet<String> = HashSet::new();
        for node in &sidecar.nodes {
            if node_map.contains_key(node.id.as_str()) {
                continue;
            }
            // 平面colliderだけに参照されるノードは補助ノード化しない。
            // 平面経路が基準ノードを直接合成するため、重複を避ける。
            if plane_only.contains(node.id.as_str()) && !referenced.contains(node.id.as_str()) {
                continue;
            }
            let parent = match node.parent_id.as_deref() {
                None => None,
                Some(parent_id) => Some(*node_map.get(parent_id).ok_or_else(|| {
                    VrmError::new(
                        VrmErrorKind::UnresolvedPhysicsNode,
                        format!("補助ノードの親が無い: {} -> {parent_id}", node.id),
                    )
                })?),
            };
            // Unity左手系の参照ローカルをVRM右手系へ換算し、ローカル行列に戻す。
            // 座標ゲートと同じ換算であり、親が同じ鎖にあれば単一の変換行列に一致する。
            let converted = SidecarTrs {
                position: mirror_point(node.reference_local.position),
                rotation: mirror_quat(node.reference_local.rotation),
                scale: node.reference_local.scale,
            };
            let local = trs_to_matrix(&converted);
            let index = self.nodes.len();
            self.nodes.push(super::VrmNode {
                name: node.name.clone(),
                parent,
                children: Vec::new(),
                local,
            });
            if let Some(parent) = parent {
                self.nodes[parent].children.push(index);
                aux_parents.insert(parent);
            }
            node_map.insert(node.id.clone(), index);
            aux_ids.insert(node.id.clone());
            auxiliary_nodes.push(AuxiliaryNode {
                id: node.id.clone(),
                name: node.name.clone(),
                node: index,
            });
        }
        // 兄弟順の再整列。補助ノードを受けた親だけをsidecarの`siblingIndex`順に並べる。
        // sidecarに対応が無い子（mesh所有等）は末尾に残し、既存の相対順序を保つ。
        {
            let order_of: HashMap<usize, (u32, usize)> = sidecar
                .nodes
                .iter()
                .enumerate()
                .filter_map(|(sidecar_index, node)| {
                    node_map
                        .get(node.id.as_str())
                        .map(|&vrm| (vrm, (node.sibling_index, sidecar_index)))
                })
                .collect();
            for parent in aux_parents {
                let mut keyed: Vec<(u8, u32, usize, usize)> = self.nodes[parent]
                    .children
                    .iter()
                    .enumerate()
                    .map(|(position, &child)| match order_of.get(&child) {
                        Some(&(sibling, sidecar_index)) => (0, sibling, sidecar_index, child),
                        None => (1, 0, position, child),
                    })
                    .collect();
                keyed.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
                self.nodes[parent].children =
                    keyed.into_iter().map(|(_, _, _, child)| child).collect();
            }
        }
        // 参照されるノードが対応表に無い場合は失敗する。
        // 骨と平面以外のcolliderの参照は解決が必須であり、
        // ここに無い参照はsidecar自体にノードが無い破損である。
        // 検証で拒否済みのはずだが、念のため確かめる。
        // 制約・humanoid・視線の参照は後段で飛ばして記録するため、ここでは数えない。
        // 平面colliderの参照は平面経路が直接扱うため、ここでは数えない。
        {
            let mut required: HashSet<&str> = HashSet::new();
            for bone in &sidecar.phys_bones {
                required.insert(bone.component_node_id.as_str());
                required.insert(bone.root_node_id.as_str());
                for ignore in &bone.ignore_node_ids {
                    required.insert(ignore.as_str());
                }
            }
            for collider in &sidecar.colliders {
                if collider.shape_type == SidecarShapeType::Plane {
                    continue;
                }
                required.insert(collider.component_node_id.as_str());
                required.insert(collider.root_node_id.as_str());
            }
            for id in &required {
                if !node_map.contains_key(*id) {
                    return Err(VrmError::new(
                        VrmErrorKind::UnresolvedPhysicsNode,
                        format!("参照ノードの対応が無い: {id}"),
                    ));
                }
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
        check_reference_locals(self, sidecar, &node_map, &aux_ids)?;
        Ok(PhysicsResolution {
            matched_source_model: single.id.clone(),
            fbx_sha256: digest,
            node_map,
            auxiliary_nodes,
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
    compose_trs(trs.position, trs.rotation, trs.scale)
}

/// 位置・回転（四元数 `[x, y, z, w]`）・拡大率から列優先の4x4行列を作る。
/// `T * R * S` の順であり、回転は正規化済みを想定する（正規化は行わない）。
/// sidecarのTRSとVRMローカルの再合成で共有する。
fn compose_trs(position: [f64; 3], rotation: [f64; 4], scale: [f64; 3]) -> [f64; 16] {
    let (x, y, z, w) = (rotation[0], rotation[1], rotation[2], rotation[3]);
    let (sx, sy, sz) = (scale[0], scale[1], scale[2]);
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
        position[0],
        position[1],
        position[2],
        1.0, //
    ]
}

/// sidecarの参照世界行列を求める。
fn sidecar_world_matrices(sidecar: &PhysicsSidecar) -> Result<Vec<[f64; 16]>, VrmError> {
    world_matrices(sidecar, false).map(|(worlds, _)| worlds)
}

/// sidecarの参照世界行列と世界拡大率をUnity空間のまま求める。
/// 関節のrest形状と骨長の計算に使う。ルートは拡大率だけを残す、
/// という参照実装の約束に従う。
fn unity_worlds_and_scales(
    sidecar: &PhysicsSidecar,
) -> Result<(Vec<[f64; 16]>, Vec<f64>), VrmError> {
    world_matrices(sidecar, true)
}

/// 親の連鎖をルートから並べる。循環と欠落は失敗する。
fn upward_chain(
    sidecar: &PhysicsSidecar,
    at: &HashMap<&str, usize>,
    start: usize,
    done: &[bool],
) -> Result<Vec<usize>, VrmError> {
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
    chain.reverse();
    Ok(chain)
}

/// 参照世界行列を求める。`unity` の場合はUnity空間のままにし、
/// ルートは拡大率だけを残す。世界拡大率の最大成分も同時に返す。
/// 拡大率は参照姿勢の積であり、鏡像の影響を受けない。
fn world_matrices(
    sidecar: &PhysicsSidecar,
    unity: bool,
) -> Result<(Vec<[f64; 16]>, Vec<f64>), VrmError> {
    let at: HashMap<&str, usize> = sidecar
        .nodes
        .iter()
        .enumerate()
        .map(|(index, node)| (node.id.as_str(), index))
        .collect();
    // UnityのTransformは左手系、VRM/glTFは右手系である。sidecarは
    // Unity空間（`unity-avatar-root-local-meters`）なので、VRM比較用は
    // X反転と四元数の共役でVRM空間へ移す。Unity空間のままの行列は
    // 関節形状の計算に使い、ルートは拡大率だけを残す。
    let locals: Vec<[f64; 16]> = sidecar
        .nodes
        .iter()
        .map(|node| {
            let local = &node.reference_local;
            if unity {
                trs_to_matrix(local)
            } else {
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
            }
        })
        .collect();
    let mut worlds = vec![matrix::IDENTITY; sidecar.nodes.len()];
    let mut scales = vec![[1.0; 3]; sidecar.nodes.len()];
    let mut done = vec![false; sidecar.nodes.len()];
    for start in 0..sidecar.nodes.len() {
        for node in upward_chain(sidecar, &at, start, &done)? {
            match sidecar.nodes[node].parent_id.as_deref() {
                Some(parent) => match at.get(parent) {
                    Some(&index) => {
                        worlds[node] = matrix::mul(&worlds[index], &locals[node]);
                        let parent_scale = scales[index];
                        let local_scale = sidecar.nodes[node].reference_local.scale;
                        scales[node] = [
                            parent_scale[0] * local_scale[0],
                            parent_scale[1] * local_scale[1],
                            parent_scale[2] * local_scale[2],
                        ];
                    }
                    None => {
                        return Err(VrmError::new(
                            VrmErrorKind::InvalidPhysicsSidecar,
                            "sidecarの親ノードが無い",
                        ));
                    }
                },
                None => {
                    if unity {
                        let scale = sidecar.nodes[node].reference_local.scale;
                        worlds[node] = [
                            scale[0], 0.0, 0.0, 0.0, //
                            0.0, scale[1], 0.0, 0.0, //
                            0.0, 0.0, scale[2], 0.0, //
                            0.0, 0.0, 0.0, 1.0, //
                        ];
                        scales[node] = scale;
                    } else {
                        worlds[node] = locals[node];
                        scales[node] = sidecar.nodes[node].reference_local.scale;
                    }
                }
            }
            done[node] = true;
        }
    }
    let max_scales = scales
        .iter()
        .map(|scale| scale[0].abs().max(scale[1].abs()).max(scale[2].abs()))
        .collect();
    Ok((worlds, max_scales))
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

/// 参照ローカルの一致を確認する。
///
/// sidecarの参照世界行列と、`VrmModel` の出力世界行列をノードごとに比べる。
/// 対象は物理が参照する対応済みノードであり、mesh所有・補助ノードは除く。
/// 世界行列の比較は親鎖の比率選びに左右されないため、bind姿勢で作られて
/// いないアバター（bindとsceneが乖離）でも、鎖の上流の取り違えが無ければ通る。
/// 対応表の検証であり、スプリングの生成や出力は行わない。
///
/// # Errors
///
/// 世界行列の不一致・対象不在・循環や範囲外の不整合の場合に返す。
fn check_reference_locals(
    model: &VrmModel,
    sidecar: &PhysicsSidecar,
    node_map: &HashMap<String, usize>,
    aux_ids: &HashSet<String>,
) -> Result<(), VrmError> {
    let sidecar_worlds = sidecar_world_matrices(sidecar)?;
    let model_worlds = model_world_matrices(model)?;
    let sidecar_at: HashMap<&str, usize> = sidecar
        .nodes
        .iter()
        .enumerate()
        .map(|(index, node)| (node.id.as_str(), index))
        .collect();
    // モデルルートは一様な単位換算のはずである。`from_fbx` が作る行列であり、
    // 対角が等しい正の有限値、非対角と平行移動は0のはずである。
    // 世界行列の線形部はこの拡大を含むため、比較前に割って正規化する。
    // 平行移動は既にメートルであり、そのまま比べる。
    let root_scale = model.root[0];
    if !root_scale.is_finite()
        || root_scale <= 0.0
        || model.root[5] != root_scale
        || model.root[10] != root_scale
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
    let mesh_nodes: HashSet<usize> = model.meshes.iter().map(|mesh| mesh.node).collect();
    // 物理が実際に参照するノードだけを対象にする。
    // 全身の関節を対象にすると、指や表情用ボーンの編集時ポーズ差で
    // スプリングに無関係な不一致まで拾ってしまうためである。
    let mut physics_ids: HashSet<&str> = HashSet::new();
    for bone in &sidecar.phys_bones {
        physics_ids.insert(bone.component_node_id.as_str());
        physics_ids.insert(bone.root_node_id.as_str());
        for ignore in &bone.ignore_node_ids {
            physics_ids.insert(ignore.as_str());
        }
    }
    for collider in &sidecar.colliders {
        physics_ids.insert(collider.component_node_id.as_str());
        physics_ids.insert(collider.root_node_id.as_str());
    }
    // 対象ノードごとに世界行列を比べ、最大の要素差を集める。
    // 平行移動と線形部は別々に記録し、診断しやすくする。
    let mut worst: Vec<(String, f64, f64)> = Vec::new();
    let mut compared = 0usize;
    for (sidecar_id, &vrm_index) in node_map {
        if !physics_ids.contains(sidecar_id.as_str()) {
            continue;
        }
        // 補助ノードは対象外にする。親の鎖の比率を継承するため、
        // 骨鎖の世界とは一般に一致しない。
        // 一貫性は参照ローカルの換算そのもので保つ。
        if aux_ids.contains(sidecar_id.as_str()) {
            continue;
        }
        let Some(&sidecar_index) = sidecar_at.get(sidecar_id.as_str()) else {
            continue;
        };
        // メッシュ所有ノードは頂点ベイクで切り離されるため、常に対象外である。
        if mesh_nodes.contains(&vrm_index) {
            continue;
        }
        // 世界行列を比べる。線形部（3x3）はモデルルートの拡大で割り、
        // 平行移動はメートルのまま比べる。w行は両方 `[0, 0, 0, 1]` のため除く。
        let translation_dev = model_worlds[vrm_index][12..15]
            .iter()
            .zip(sidecar_worlds[sidecar_index][12..15].iter())
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f64, f64::max);
        let linear_dev = model_worlds[vrm_index][..12]
            .iter()
            .zip(sidecar_worlds[sidecar_index][..12].iter())
            .map(|(a, b)| (a / root_scale - b).abs())
            .fold(0.0f64, f64::max);
        let deviation = linear_dev.max(translation_dev);
        worst.push((sidecar_id.clone(), translation_dev, deviation));
        compared += 1;
    }
    if compared == 0 {
        return Err(VrmError::new(
            VrmErrorKind::PhysicsCoordinateMismatch,
            "座標ゲートの対象ノードが無い",
        ));
    }
    worst.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
    if worst
        .iter()
        .any(|(_, _, deviation)| *deviation > REFERENCE_WORLD_TOLERANCE)
    {
        let over: Vec<String> = worst
            .iter()
            .take(8)
            .map(|(id, translation, deviation)| format!("{id}:t{translation:.3e} m{deviation:.3e}"))
            .collect();
        let top = worst
            .iter()
            .max_by(|a, b| a.2.partial_cmp(&b.2).unwrap_or(std::cmp::Ordering::Equal))
            .expect("対象がある");
        return Err(VrmError::new(
            VrmErrorKind::PhysicsCoordinateMismatch,
            format!(
                "参照ローカルが一致しない: {} worst=[{}]",
                top.0,
                over.join(" ")
            ),
        ));
    }
    Ok(())
}

/// スプリング出力の上限。
///
/// フィールドは非公開で、既定値は [`Default`] が設定する。
/// 上限の変更は `with_` で始まるメソッドで行う。
#[derive(Debug, Clone)]
pub struct SpringCompileOptions {
    max_joints: usize,
    joint_trace: bool,
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
            joint_trace: false,
        }
    }

    /// 1springのjoint数の上限を変更する。1以上である。
    pub fn with_max_joints(mut self, max_joints: usize) -> Self {
        self.max_joints = max_joints;
        self
    }

    /// joint計算の追跡記録を残すかどうかを切り替える。
    /// 参照実装の JointTrace 相当の試験用出入口であり、既定では記録しない。
    /// 有効にすると `SpringCompileReport::joint_traces` が増えるが、
    /// springやjointの出力内容は変わらない。
    pub fn with_joint_trace(mut self, joint_trace: bool) -> Self {
        self.joint_trace = joint_trace;
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
    /// 無効化されていたが出力したPhysBoneのID。VRMに有効無効は無い。
    pub disabled_bones: Vec<String>,
    /// 無効化されていたが出力したcolliderのID。
    pub disabled_colliders: Vec<String>,
    /// 参照されず出力しなかったcolliderのID。
    pub unused_colliders: Vec<String>,
    /// 平面のため落とした参照を持つPhysBoneのID。
    /// 現在は平面も拡張付きで出力するため、常に空である。
    /// 互換のために残す。
    pub plane_colliders_dropped: Vec<String>,
    /// 出力しなかった制約のID。種別・軸数・参照の不備や重複による。
    pub skipped_constraints: Vec<String>,
    /// VRMに写せない指定（`insideBounds`・`bonesAsSpheres`）を持つcolliderの記録。
    pub ignored_collider_options: Vec<String>,
    /// `center` を付けなかったspringのPhysBone ID。springごとに1件である。
    pub center_missing: Vec<String>,
    /// endpointを生成しなかったspringのPhysBone ID。今回は全件である。
    pub no_endpoint: Vec<String>,
    /// `multiChildType` が `Ignore` 以外で最初の子に畳んだspringのPhysBone ID。
    pub first_child_multichild: Vec<String>,
    /// `gravity_power` が0の関節数。
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
    /// joint計算の追跡。`with_joint_trace(true)` の場合だけ記録する。
    pub joint_traces: Vec<JointTrace>,
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

/// joint計算の追跡1件。試験用の診断であり、通常の出力には使わない。
/// `with_joint_trace(true)` の場合だけ記録する。
/// 参照実装の `PhysicsJointTrace` に相当する。
/// 制限角・ループ統計・保持比は倍精度の本計算と同じ値であり、単精度化は行わない。
/// 一致方向の試行では倍精度でも角度が0に潰れる場合があり、その場合はUnity側の
/// 単精度丸めだけが有限の角度を返す。差分の分析は設計文書に残す。
#[derive(Debug, Clone)]
pub struct JointTrace {
    /// PhysBoneのID。
    pub bone: String,
    /// spring名。
    pub spring: String,
    /// spring内の関節索引。
    pub joint: usize,
    /// 関節のsidecarノードID。
    pub node: String,
    /// headの計算かどうか。末端は直前headの写しである。
    pub is_head: bool,
    /// head位置の曲線引数。
    pub head_t: f64,
    /// tail位置の半径引数。
    pub tail_t: f64,
    /// 制限の種別。
    pub limit_kind: SidecarLimitType,
    /// 最大角の評価値 `[X, Z]`（度）。
    pub max_angle: [f64; 2],
    /// 制限回転の評価値 `[X, Y, Z]`（度）。
    pub limit_rotation: [f64; 3],
    /// Aim先への自空間ベクトル（Unity軸・メートル）。
    pub bone_vector: [f64; 3],
    /// 骨のrest世界行列（Unity軸）。
    pub bone_to_world: [f64; 16],
    /// 有効制限角（度）。本計算と同じ値である。
    pub limit: f64,
    /// 36方向ループの角度の最小・最大・平均（度）。
    pub loop_min: f64,
    pub loop_max: f64,
    pub loop_mean: f64,
    /// 保持した引き比。本計算と同じ値である。
    pub held: f64,
    /// 硬さ。本計算と同じ値である。
    pub stiffness: f64,
    /// 抵抗。
    pub drag: f64,
    /// 重力の強さ。
    pub power: f64,
    /// 当たり半径。
    pub hit_radius: f64,
    /// 重力が上向きかどうか。
    pub gravity_up: bool,
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
///
/// 視線の測定でもVRM空間からUnity空間へ戻す際に使う。鏡像は群同型のため、
/// 往復で厳密に戻る。
pub(crate) fn mirror_quat(rotation: [f64; 4]) -> [f64; 4] {
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

/// 関節パラメータ式の定数。参照実装と同値である。
/// 較正記録のフレーム率は、PhysBoneの motion が骨長に依存しないのに対し、
/// VRMのstiffnessと重力が速度であるため、骨長で比例させる倍率である。
const CALIBRATION_FRAME_RATE: f64 = 60.0;
/// 較正記録の骨長。骨長が不明な場合の仮定値でもある。
const CALIBRATION_LENGTH: f64 = 0.1;
/// pull比の上限。1に近いpullは骨をrestに留める。
const MAX_PULL_RATIO: f64 = 60.0;
/// Simplifiedで式が使うspringの上限。
const MAX_SIMPLIFIED_SPRING: f64 = 0.9;
/// 版1.1の重力換算における `1 - |G|` の下限。
const MIN_GRAVITY_BALANCE: f64 = 0.05;
/// 制限角の基準。制限がこれを下回るとpull比を強める。
const LIMIT_REFERENCE_ANGLE: f64 = 45.0;
/// きつい制限の下限。0制限が有限に収まる。
const MIN_LIMIT_ANGLE: f64 = 1.0;
/// 緩いheld jointのpull下限係数。
const LOOSE_FLOOR_FACTOR: f64 = 9.0;
/// held jointの運動量がpullを上回るとみなすdragの境目。
const LOOSE_DRAG_GATE: f64 = 0.5;
/// 骨長が不明扱いになる上限。参照実装の `1e-5` と同値である。
const UNKNOWN_BONE_LENGTH: f64 = 1e-5;
/// 沈み角の一致判定。参照実装と同値である。
const SETTLED_ANGLE_TOLERANCE: f64 = 1e-4;
/// 有効制限角の応答方向数。参照実装と同値である。
const LIMIT_RESPONSE_STEPS: usize = 36;
/// 有効制限角の平均の冪。参照実装と同値である。
const LIMIT_RESPONSE_POWER: f64 = 0.5;

/// 3要素ベクトルの内積。
fn vec_dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// 3要素ベクトルの外積。
fn vec_cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// 3要素ベクトルの長さの二乗。
fn vec_len2(v: [f64; 3]) -> f64 {
    vec_dot(v, v)
}

/// 3要素ベクトルの長さ。
fn vec_len(v: [f64; 3]) -> f64 {
    vec_len2(v).sqrt()
}

/// 零ベクトルは零のまま返す正規化。Unityの `Vector3.Normalize` と同じ扱いである。
fn vec_normalized_or_zero(v: [f64; 3]) -> [f64; 3] {
    let len = vec_len(v);
    if len > 0.0 && len.is_finite() {
        [v[0] / len, v[1] / len, v[2] / len]
    } else {
        [0.0; 3]
    }
}

/// 2ベクトルの角度（度）。いずれかが零の場合は0を返す。
fn vec_angle_deg(a: [f64; 3], b: [f64; 3]) -> f64 {
    let denom = vec_len(a) * vec_len(b);
    if denom <= 0.0 || !denom.is_finite() {
        return 0.0;
    }
    (vec_dot(a, b) / denom).clamp(-1.0, 1.0).acos() * 180.0 / std::f64::consts::PI
}

/// 0〜1に丸める。非有限値は0にする。
fn clamp01(value: f64) -> f64 {
    if !value.is_finite() {
        return 0.0;
    }
    value.clamp(0.0, 1.0)
}

/// 非有限値を0にする。
fn finite_or_zero(value: f64) -> f64 {
    if value.is_finite() { value } else { 0.0 }
}

/// 四元数の積。Unityと同じHamilton積（列ベクトル規約）である。
fn quat_mul(a: [f64; 4], b: [f64; 4]) -> [f64; 4] {
    [
        a[3] * b[0] + a[0] * b[3] + a[1] * b[2] - a[2] * b[1],
        a[3] * b[1] - a[0] * b[2] + a[1] * b[3] + a[2] * b[0],
        a[3] * b[2] + a[0] * b[1] - a[1] * b[0] + a[2] * b[3],
        a[3] * b[3] - a[0] * b[0] - a[1] * b[1] - a[2] * b[2],
    ]
}

/// 軸回りの回転。軸が零の場合は恒等写像を返す。
fn quat_angle_axis(axis: [f64; 3], angle_deg: f64) -> [f64; 4] {
    let axis = vec_normalized_or_zero(axis);
    if vec_len2(axis) <= 0.0 {
        return [0.0, 0.0, 0.0, 1.0];
    }
    let half = angle_deg * std::f64::consts::PI / 360.0;
    [
        axis[0] * half.sin(),
        axis[1] * half.sin(),
        axis[2] * half.sin(),
        half.cos(),
    ]
}

/// 単位四元数によるベクトルの回転。
fn quat_rotate(rotation: [f64; 4], v: [f64; 3]) -> [f64; 3] {
    let u = [rotation[0], rotation[1], rotation[2]];
    let uv = vec_cross(u, v);
    let uuv = vec_cross(u, uv);
    [
        v[0] + 2.0 * (rotation[3] * uv[0] + uuv[0]),
        v[1] + 2.0 * (rotation[3] * uv[1] + uuv[1]),
        v[2] + 2.0 * (rotation[3] * uv[2] + uuv[2]),
    ]
}

/// `from` から `to` への回転。正反対の場合は直交軸の180度回転を返す。
fn quat_from_to_rotation(from: [f64; 3], to: [f64; 3]) -> [f64; 4] {
    let from = vec_normalized_or_zero(from);
    let to = vec_normalized_or_zero(to);
    let dot = vec_dot(from, to);
    if dot >= 1.0 - 1e-9 {
        return [0.0, 0.0, 0.0, 1.0];
    }
    if dot <= -1.0 + 1e-9 {
        let mut axis = vec_cross(from, [1.0, 0.0, 0.0]);
        if vec_len2(axis) < 1e-12 {
            axis = vec_cross(from, [0.0, 1.0, 0.0]);
        }
        return quat_angle_axis(axis, 180.0);
    }
    let cross = vec_cross(from, to);
    let len = (vec_len2(cross) + (1.0 + dot) * (1.0 + dot)).sqrt();
    if len > 0.0 && len.is_finite() {
        [
            cross[0] / len,
            cross[1] / len,
            cross[2] / len,
            (1.0 + dot) / len,
        ]
    } else {
        [0.0, 0.0, 0.0, 1.0]
    }
}

/// オイラー角（yawをY軸、pitchをX軸に掛ける）。Unityの `Quaternion.Euler` と同じ順序である。
fn quat_euler_yx(yaw_deg: f64, pitch_deg: f64) -> [f64; 4] {
    quat_mul(
        quat_angle_axis([0.0, 1.0, 0.0], yaw_deg),
        quat_angle_axis([1.0, 0.0, 0.0], pitch_deg),
    )
}

/// `current` を `target` へ `max_rad` だけ近づける。単位ベクトル同士で使う。
fn rotate_towards(current: [f64; 3], target: [f64; 3], max_rad: f64) -> [f64; 3] {
    let angle = vec_angle_deg(current, target) * std::f64::consts::PI / 180.0;
    if angle <= max_rad || angle < 1e-12 {
        return target;
    }
    let axis = vec_normalized_or_zero(vec_cross(current, target));
    let step = max_rad.min(angle);
    let (sin, cos) = step.sin_cos();
    let across = vec_cross(axis, current);
    [
        current[0] * cos + across[0] * sin,
        current[1] * cos + across[1] * sin,
        current[2] * cos + across[2] * sin,
    ]
}

/// 関節のrest形状。制限角と重力が働く基準であり、参照実装の `JointRest` に相当する。
#[derive(Debug, Clone, Copy)]
struct JointRest {
    /// Aim先への自空間ベクトル（Unity軸）。
    bone_vector: [f64; 3],
    /// 骨のrest世界行列（Unity軸）。ルートは拡大率だけを残す。
    bone_to_world: [f64; 16],
}

/// 計算済みの関節パラメータ。重さ・長さはメートル、角度は度である。
#[derive(Debug, Clone, Copy)]
struct JointParams {
    /// 硬さ。
    stiffness: f64,
    /// 抵抗。
    drag: f64,
    /// 重力の強さ。
    power: f64,
    /// 重力が上向きかどうか。下向きが既定である。
    gravity_up: bool,
    /// 当たり半径。
    hit_radius: f64,
    /// 保持した引き比。試験用の追跡記録のために残す。
    held: f64,
}

/// 1フレームのVRM pull比。PhysBoneの `share` だけ寄せ、`keep` だけ残す。
fn pull_ratio(share: f64, keep: f64) -> f64 {
    if share <= 0.0 {
        return 0.0;
    }
    let denominator = (1.0 - share) * keep;
    if denominator * MAX_PULL_RATIO <= share {
        MAX_PULL_RATIO
    } else {
        share / denominator
    }
}

/// 制限角で強めたpull比。制限が基準角を下回ると平方根で強める。
fn held_ratio(ratio: f64, immobile: f64, limit_angle: f64) -> f64 {
    if ratio <= 0.0 {
        return 0.0;
    }
    let scale = if limit_angle < LIMIT_REFERENCE_ANGLE {
        (LIMIT_REFERENCE_ANGLE / limit_angle.max(MIN_LIMIT_ANGLE)).sqrt()
    } else {
        1.0
    };
    let keep = 1.0 - immobile;
    let held = ratio * scale;
    if keep * MAX_PULL_RATIO <= held {
        MAX_PULL_RATIO
    } else {
        (held / keep).min(MAX_PULL_RATIO)
    }
}

/// 緩いheld jointのpull下限。運動量がpullを上回り制限が基準角を下回る場合に使う。
/// 制限角が非数値の場合は適用しない（参照実装の否定比較と同じ）。
fn loose_floor(held: f64, drag: f64, limit_angle: f64) -> f64 {
    if drag >= LOOSE_DRAG_GATE || limit_angle.is_nan() || limit_angle >= LIMIT_REFERENCE_ANGLE {
        return held;
    }
    let floor =
        LOOSE_FLOOR_FACTOR * (1.0 / limit_angle.max(MIN_LIMIT_ANGLE) - 1.0 / LIMIT_REFERENCE_ANGLE);
    held.max(floor).min(MAX_PULL_RATIO)
}

/// 制限角（度）。rest形状が不明な場合の `EffectiveLimitAngle` であり、
/// Angle/HingeはX軸、Polarは両軸の平均、制限なしは無限大である。
fn limit_angle(record: &SidecarPhysBone, t: f64) -> f64 {
    let axis = |curve: &SidecarCurve| finite_or_zero(eval_curved(curve, t)).abs();
    match record.limit_type {
        SidecarLimitType::Angle | SidecarLimitType::Hinge => axis(&record.max_angle_x),
        SidecarLimitType::Polar => (axis(&record.max_angle_x) + axis(&record.max_angle_z)) / 2.0,
        SidecarLimitType::None => f64::INFINITY,
    }
}

/// 制限回転（度）。各軸の設定値にカーブを掛ける。
/// カーブの `value` は使わず、キー列だけを評価する（参照実装の `CurveRecord` と同じ）。
fn limit_rotation(record: &SidecarPhysBone, t: f64) -> [f64; 3] {
    let axis =
        |setting: f64, curve: &SidecarCurve| finite_or_zero(setting) * eval_unity_curve(curve, t);
    [
        axis(record.limit_rotation[0], &record.limit_rotation_x_curve),
        axis(record.limit_rotation[1], &record.limit_rotation_y_curve),
        axis(record.limit_rotation[2], &record.limit_rotation_z_curve),
    ]
}

/// X/Z軸の最大角（度）。0未満は0にする。
fn max_angles(record: &SidecarPhysBone, t: f64) -> [f64; 2] {
    [
        finite_or_zero(eval_curved(&record.max_angle_x, t)).max(0.0),
        finite_or_zero(eval_curved(&record.max_angle_z, t)).max(0.0),
    ]
}

/// 制限の枠組み。骨ベクトルから上向きへの回転に制限回転（x→y→z）を掛ける。
fn limit_frame(bone_vector: [f64; 3], limit_rotation: [f64; 3]) -> [f64; 4] {
    quat_mul(
        quat_from_to_rotation([0.0, 1.0, 0.0], vec_normalized_or_zero(bone_vector)),
        quat_mul(
            quat_angle_axis([0.0, 0.0, 1.0], limit_rotation[2]),
            quat_mul(
                quat_angle_axis([0.0, 1.0, 0.0], limit_rotation[1]),
                quat_angle_axis([1.0, 0.0, 0.0], limit_rotation[0]),
            ),
        ),
    )
}

/// 方向を錐体に収める。角度内ならそのまま返す。
fn clamp_to_cone(axis: [f64; 3], direction: [f64; 3], max_angle: f64) -> [f64; 3] {
    if vec_angle_deg(axis, direction) <= max_angle {
        return direction;
    }
    quat_rotate(
        quat_angle_axis(
            vec_normalized_or_zero(vec_cross(axis, direction)),
            max_angle,
        ),
        axis,
    )
}

/// 極座標（度）。前方からの方位角と水平面からの仰角である。
fn cartesian_to_polar(normal: [f64; 3]) -> [f64; 2] {
    let flat = vec_normalized_or_zero([normal[0], 0.0, normal[2]]);
    let mut around = flat[2].clamp(-1.0, 1.0).acos() * 180.0 / std::f64::consts::PI;
    if flat[0] < 0.0 {
        around = -around;
    }
    let height = normal[1].clamp(-1.0, 1.0).acos() * 180.0 / std::f64::consts::PI - 90.0;
    [around, height]
}

/// VRChatの角度制限。SDKの適用と同形（Angle/Hinge/Polar）である。
fn apply_angle_limit(
    limit_type: &SidecarLimitType,
    bone_vector: [f64; 3],
    rotation: [f64; 3],
    max_angle: [f64; 2],
    direction: [f64; 3],
) -> [f64; 3] {
    let frame = limit_frame(bone_vector, rotation);
    let axis_x = quat_rotate(frame, [1.0, 0.0, 0.0]);
    let axis_y = quat_rotate(frame, [0.0, 1.0, 0.0]);
    match limit_type {
        SidecarLimitType::None => direction,
        SidecarLimitType::Angle => clamp_to_cone(axis_y, direction, max_angle[0]),
        SidecarLimitType::Hinge => {
            let dot = vec_dot(axis_x, direction);
            let flat = vec_normalized_or_zero([
                direction[0] - axis_x[0] * dot,
                direction[1] - axis_x[1] * dot,
                direction[2] - axis_x[2] * dot,
            ]);
            clamp_to_cone(axis_y, flat, max_angle[0])
        }
        SidecarLimitType::Polar => {
            let axis_z = vec_normalized_or_zero(vec_cross(axis_x, axis_y));
            let local = vec_normalized_or_zero([
                vec_dot(axis_z, direction),
                vec_dot(axis_x, direction),
                vec_dot(axis_y, direction),
            ]);
            let mut polar = cartesian_to_polar(local);
            polar[0] = polar[0].clamp(-max_angle[0], max_angle[0]);
            polar[1] = polar[1].clamp(-max_angle[1], max_angle[1]);
            let back = quat_rotate(quat_euler_yx(polar[0], polar[1]), [0.0, 0.0, 1.0]);
            vec_normalized_or_zero([
                axis_z[0] * back[0] + axis_x[0] * back[1] + axis_y[0] * back[2],
                axis_z[1] * back[0] + axis_x[1] * back[1] + axis_y[1] * back[2],
                axis_z[2] * back[0] + axis_x[2] * back[1] + axis_y[2] * back[2],
            ])
        }
    }
}

/// 制限下での方向。軸だけの場合は制限の上軸を返す。
fn limit_direction(
    record: &SidecarPhysBone,
    t: f64,
    bone_vector: [f64; 3],
    direction: [f64; 3],
    axis_only: bool,
) -> [f64; 3] {
    let rotation = limit_rotation(record, t);
    if axis_only {
        return quat_rotate(limit_frame(bone_vector, rotation), [0.0, 1.0, 0.0]);
    }
    apply_angle_limit(
        &record.limit_type,
        bone_vector,
        rotation,
        max_angles(record, t),
        direction,
    )
}

/// 制限の有無。Angle/Hinge/Polarの場合に真である。
fn has_angle_limit(record: &SidecarPhysBone) -> bool {
    !matches!(record.limit_type, SidecarLimitType::None)
}

/// 制限が許す最大の振り（度）。rest方向からの回転に最大角を足し、180で止める。
fn max_reach(record: &SidecarPhysBone, t: f64, bone_vector: [f64; 3]) -> f64 {
    let axis = limit_direction(record, t, bone_vector, [0.0; 3], true);
    let max_x = finite_or_zero(eval_curved(&record.max_angle_x, t)).max(0.0);
    let widest = match record.limit_type {
        SidecarLimitType::Polar => {
            max_x.max(finite_or_zero(eval_curved(&record.max_angle_z, t)).max(0.0))
        }
        _ => max_x,
    };
    (vec_angle_deg(bone_vector, axis) + widest).min(180.0)
}

/// 36方向ループの評価。参照実装と同じ構造で `(制限角, 最小角, 最大角, 平均角)` を返す。
/// 本計算と試験の追跡記録で共有する。単精度化は行わない。
/// 一致方向の試行では倍精度でも角度が0に潰れる場合があり、その場合はUnity側の
/// 単精度丸めだけが有限の角度を返す。差分の分析は設計文書に残す。
fn efflim(
    bone: [f64; 3],
    rot: [f64; 3],
    max: [f64; 2],
    limit_type: &SidecarLimitType,
) -> (f64, f64, f64, f64) {
    let b = vec_normalized_or_zero(bone);
    let mut across = vec_cross(b, [0.0, 0.0, 1.0]);
    if vec_dot(across, across) < 1e-6 {
        across = vec_cross(b, [1.0, 0.0, 0.0]);
    }
    let across = vec_normalized_or_zero(across);
    let other = vec_cross(b, across);
    let lrest = apply_angle_limit(limit_type, bone, rot, max, b);
    let tilt = LIMIT_REFERENCE_ANGLE * std::f64::consts::PI / 180.0;
    let mut sum: f64 = 0.0;
    let mut smallest = f64::INFINITY;
    let mut largest: f64 = 0.0;
    for i in 0..LIMIT_RESPONSE_STEPS {
        let around = 2.0 * std::f64::consts::PI * i as f64 / LIMIT_RESPONSE_STEPS as f64;
        let pushed = [
            tilt.cos() * b[0] + tilt.sin() * (around.cos() * across[0] + around.sin() * other[0]),
            tilt.cos() * b[1] + tilt.sin() * (around.cos() * across[1] + around.sin() * other[1]),
            tilt.cos() * b[2] + tilt.sin() * (around.cos() * across[2] + around.sin() * other[2]),
        ];
        let turned = apply_angle_limit(limit_type, bone, rot, max, pushed);
        let angle = vec_angle_deg(lrest, turned);
        smallest = smallest.min(angle);
        largest = largest.max(angle);
        sum += angle.powf(LIMIT_RESPONSE_POWER);
    }
    let mean = sum / LIMIT_RESPONSE_STEPS as f64;
    (
        mean.powf(1.0 / LIMIT_RESPONSE_POWER),
        smallest,
        largest,
        mean,
    )
}

/// 有効制限角（度）。36方向に押した応答の冪平均であり、塞ぐ方向を重く見る。
/// 倍精度で求めた数学的に正しい値をそのまま使い、単精度化は行わない。
/// 参照実装は単精度で計算するが、通常の試行では差は無視できる。
/// 一致方向の試行でだけ単精度側が有限の角度を返す場合があり、
/// Unity出力との突き合わせと残差の分析は試験と設計文書に残す。
/// 試験用の内訳が必要な場合は [`efflim`] を使う。
fn effective_limit_angle(record: &SidecarPhysBone, t: f64, rest: Option<&JointRest>) -> f64 {
    let Some(rest) = rest else {
        return limit_angle(record, t);
    };
    if !has_angle_limit(record) {
        return limit_angle(record, t);
    }
    let rotation = limit_rotation(record, t);
    let max_angle = max_angles(record, t);
    efflim(rest.bone_vector, rotation, max_angle, &record.limit_type).0
}

/// 関節の角度制限。`VRMC_springBone_limit` として出す値であり、
/// 参照実装の `JointLimit` に相当する。
/// rest不明・制限なしの場合は `None` を返す。末端の扱いは呼び出し側で行い、
/// ここでは末端かどうかを問わない。
/// 角度は有限値に丸めて180度（yawは90度）で止め、ラジアンで持つ。
/// 回転はUnity空間の `limit_frame` をVRM空間へ鏡像した単位四元数である。
fn joint_limit(
    record: &SidecarPhysBone,
    t: f64,
    rest: Option<&JointRest>,
) -> Option<VrmSpringJointLimit> {
    let rest = rest?;
    if !has_angle_limit(record) {
        return None;
    }
    let max = max_angles(record, t);
    let rotation = limit_rotation(record, t);
    let frame = limit_frame(rest.bone_vector, rotation);
    // 鏡像はノルムを保つため、正規化して有限性を確かめる。
    let mirrored = mirror_quat(frame);
    let len2 = mirrored[0] * mirrored[0]
        + mirrored[1] * mirrored[1]
        + mirrored[2] * mirrored[2]
        + mirrored[3] * mirrored[3];
    if !len2.is_finite() || len2 <= 0.0 {
        return None;
    }
    let len = len2.sqrt();
    let normalized = [
        (mirrored[0] / len) as f32,
        (mirrored[1] / len) as f32,
        (mirrored[2] / len) as f32,
        (mirrored[3] / len) as f32,
    ];
    if !normalized.iter().all(|v| v.is_finite()) {
        return None;
    }
    let to_radians = std::f64::consts::PI / 180.0;
    let pitch = (finite_or_zero(max[0]).min(180.0) * to_radians) as f32;
    let yaw = (finite_or_zero(max[1]).min(90.0) * to_radians) as f32;
    if !pitch.is_finite() || !yaw.is_finite() {
        return None;
    }
    match record.limit_type {
        SidecarLimitType::Angle => Some(VrmSpringJointLimit::Cone {
            angle: pitch,
            rotation: normalized,
        }),
        SidecarLimitType::Hinge => Some(VrmSpringJointLimit::Hinge {
            angle: pitch,
            rotation: normalized,
        }),
        SidecarLimitType::Polar => Some(VrmSpringJointLimit::Spherical {
            pitch,
            yaw,
            rotation: normalized,
        }),
        SidecarLimitType::None => None,
    }
}

/// 主軸の判定許容差。参照実装の `PrincipalTolerance` と同じ値である。
const PRINCIPAL_TOLERANCE: f64 = 1e-4;

/// 注視軸が6主軸のいずれかに沿うか確かめる。Unity軸で返す。
/// 参照実装の `TryPrincipalAxis` に相当する。
fn principal_axis(axis: [f64; 3]) -> Option<VrmAimAxis> {
    let length2 = axis[0] * axis[0] + axis[1] * axis[1] + axis[2] * axis[2];
    if !length2.is_finite() || length2 < 1e-12 {
        return None;
    }
    let length = length2.sqrt();
    for (index, component) in axis.iter().enumerate() {
        if ((component / length).abs() - 1.0).abs() > PRINCIPAL_TOLERANCE {
            continue;
        }
        let positive = *component > 0.0;
        return Some(match (index, positive) {
            (0, true) => VrmAimAxis::PositiveX,
            (0, false) => VrmAimAxis::NegativeX,
            (1, true) => VrmAimAxis::PositiveY,
            (1, false) => VrmAimAxis::NegativeY,
            (2, true) => VrmAimAxis::PositiveZ,
            _ => VrmAimAxis::NegativeZ,
        });
    }
    None
}

/// sidecar制約1件を `VrmNodeConstraint` へ写す。出力しない場合は `None` を返す。
/// 参照実装の `ConstraintCompiler.Map` と `WithSource` に相当する。
/// 回転3軸→回転、1軸→その軸のひねり、2軸→出力しない。
/// 注視は3軸かつ主軸の場合だけ写し、注視は+Z軸として写す。
/// 親付け・位置・拡大率・外部参照・負の重み・ワールド固定・
/// 無効な対象と参照は出力しない。
/// 重みは全体重みに注視の場合だけ参照重みを掛け、0〜1に丸める。
fn map_node_constraint(
    record: &SidecarConstraint,
    node_map: &HashMap<String, usize>,
) -> Option<VrmNodeConstraint> {
    let target_id = if record.target_node_id.is_empty() {
        record.node_id.as_str()
    } else {
        record.target_node_id.as_str()
    };
    let &target = node_map.get(target_id)?;
    // capture時に無効な制約と重み0は動かないため出力しない。
    // FX既定値の上書きは未対応であり、capture時の状態で判断する。
    if !(record.enabled && record.active) {
        return None;
    }
    if record.weight <= 0.0 {
        return None;
    }
    if record.freeze_to_world {
        return None;
    }
    let axes = record.affects_rotation.iter().filter(|axis| **axis).count();
    let kind = match record.constraint_type {
        SidecarConstraintType::Rotation => match axes {
            3 => VrmNodeConstraintKind::Rotation,
            1 => {
                let index = record
                    .affects_rotation
                    .iter()
                    .position(|axis| *axis)
                    .expect("1軸は位置を持つ");
                VrmNodeConstraintKind::Roll {
                    axis: match index {
                        0 => VrmRollAxis::X,
                        1 => VrmRollAxis::Y,
                        _ => VrmRollAxis::Z,
                    },
                }
            }
            // 2軸はVRMで表せず、0軸は動かないため出力しない。
            _ => return None,
        },
        SidecarConstraintType::Aim => {
            if axes != 3 {
                return None;
            }
            VrmNodeConstraintKind::Aim {
                axis: principal_axis(record.aim_axis)?,
            }
        }
        SidecarConstraintType::LookAt => VrmNodeConstraintKind::Aim {
            axis: VrmAimAxis::PositiveZ,
        },
        // 親付けはリペアレントであり、今回の出力対象外として落とす。
        // 位置・拡大率も回転を写さないため出力しない。
        SidecarConstraintType::Parent
        | SidecarConstraintType::Position
        | SidecarConstraintType::Scale => return None,
    };
    let is_aim = matches!(kind, VrmNodeConstraintKind::Aim { .. });
    // 有効な参照先だけを数える。負の重みがある場合は動きを変えるため落とす。
    if record
        .sources
        .iter()
        .any(|source| (source.external || !source.node_id.is_empty()) && source.weight < 0.0)
    {
        return None;
    }
    let mut weighted = record
        .sources
        .iter()
        .filter(|source| (source.external || !source.node_id.is_empty()) && source.weight > 0.0);
    let single = match (weighted.next(), weighted.next()) {
        (Some(only), None) => only,
        // 参照なし・複数参照（blend）は出力しない。
        _ => return None,
    };
    if single.external {
        return None;
    }
    let &source = node_map.get(single.node_id.as_str())?;
    if source == target {
        return None;
    }
    let weight = (record.weight * if is_aim { single.weight } else { 1.0 }).clamp(0.0, 1.0);
    if !weight.is_finite() {
        return None;
    }
    Some(VrmNodeConstraint {
        node: target,
        source,
        weight: weight as f32,
        kind,
    })
}

/// sidecar制約列を `VrmNodeConstraint` 列へ写す。sidecar順に処理し、
/// 同じ対象の2件目以降は最初だけを残す。戻り値は（制約列、落とした制約ID列）である。
fn compile_node_constraints(
    sidecar: &PhysicsSidecar,
    resolution: &PhysicsResolution,
) -> (Vec<VrmNodeConstraint>, Vec<String>) {
    let mut constraints = Vec::new();
    let mut skipped = Vec::new();
    let mut seen_targets: HashSet<usize> = HashSet::new();
    for record in &sidecar.constraints {
        let Some(mapped) = map_node_constraint(record, &resolution.node_map) else {
            skipped.push(record.id.clone());
            continue;
        };
        if !seen_targets.insert(mapped.node) {
            skipped.push(record.id.clone());
            continue;
        }
        constraints.push(mapped);
    }
    (constraints, skipped)
}

/// rest方向から重力への角度（ラジアン）。rest不明時は直角である。
fn rest_to_gravity(rest: Option<&JointRest>, gravity_up: bool) -> f64 {
    let Some(rest) = rest else {
        return std::f64::consts::PI / 2.0;
    };
    let Some(world) = matrix::transform_vector(&rest.bone_to_world, rest.bone_vector) else {
        return std::f64::consts::PI / 2.0;
    };
    let world = vec_normalized_or_zero(world);
    if vec_len2(world) <= 0.0 {
        return std::f64::consts::PI / 2.0;
    }
    let gravity = if gravity_up {
        [0.0, 1.0, 0.0]
    } else {
        [0.0, -1.0, 0.0]
    };
    vec_dot(world, gravity).clamp(-1.0, 1.0).acos()
}

/// 版1.0の重力の当てはめ。pullの後に足す力である。
fn gravity_at10(gravity: f64, falloff: f64, ratio: f64, scale: f64, closeness: f64) -> f64 {
    gravity * (1.0 - falloff * closeness) * (1.0 + ratio) * scale
}

/// 版1.0の接線。pullなしで力が残る場合は無限大、力なしは0である。
/// 0/0を無限とみなすとfalloff全除去が消えるため分ける。
fn gravity_tangent10(at: f64, ratio: f64) -> f64 {
    if ratio > 0.0 {
        at / ratio
    } else if at > 0.0 {
        f64::INFINITY
    } else {
        0.0
    }
}

/// 版1.0の重力比。falloffは沈み角で取り除く。
fn gravity_ratio10(gravity: f64, falloff: f64, ratio: f64, scale: f64, to_gravity: f64) -> f64 {
    if falloff <= 0.0 {
        return gravity_at10(gravity, falloff, ratio, scale, 1.0);
    }
    let mut angle = 0.0f64;
    for _ in 0..64 {
        let closeness = 0.0f64.max(angle.cos());
        let tangent = gravity_tangent10(
            gravity_at10(gravity, falloff, ratio, scale, closeness),
            ratio,
        );
        angle = if tangent.is_infinite() {
            to_gravity
        } else {
            (tangent * to_gravity.sin()).atan2(1.0 + tangent * to_gravity.cos())
        };
    }
    gravity_at10(gravity, falloff, ratio, scale, 0.0f64.max(angle.cos()))
}

/// 版1.1の重力比。rest姿勢と同じ倒れになる `G / (1 - G)` である。
fn gravity_share11(gravity: f64, falloff: f64, closeness: f64) -> f64 {
    (gravity * (1.0 - falloff * closeness)).min(1.0)
}

/// 版1.1の比 `G / (1 - G)`。分母の下限で発散を抑える。
fn gravity_balance11(share: f64) -> f64 {
    share / (1.0 - share).max(MIN_GRAVITY_BALANCE)
}

/// 版1.1の重力比。falloffは沈み角で取り除く。
fn gravity_ratio11(gravity: f64, falloff: f64, to_gravity: f64) -> f64 {
    if falloff <= 0.0 {
        return gravity_balance11(gravity_share11(gravity, falloff, 1.0));
    }
    let mut angle = 0.0f64;
    for _ in 0..64 {
        let closeness = 0.0f64.max(angle.cos());
        let ratio = gravity_balance11(gravity_share11(gravity, falloff, closeness));
        angle = (ratio * to_gravity.sin()).atan2(1.0 + ratio * to_gravity.cos());
    }
    gravity_balance11(gravity_share11(gravity, falloff, 0.0f64.max(angle.cos())))
}

/// 制限で抑えられる重力。制限が沈みを止める場合はそこに収める。
fn limited_gravity(
    record: &SidecarPhysBone,
    t: f64,
    rest: Option<&JointRest>,
    stiffness: f64,
    power: f64,
    gravity_up: bool,
) -> f64 {
    let Some(rest) = rest else {
        return power;
    };
    if power <= 0.0 || stiffness <= 0.0 || !has_angle_limit(record) {
        return power;
    }
    let mut power = power;
    let widest = max_reach(record, t, rest.bone_vector);
    if widest < 90.0 {
        power = power.min(stiffness * (widest * std::f64::consts::PI / 180.0).sin());
    }
    let Some(rest_world) = matrix::transform_vector(&rest.bone_to_world, rest.bone_vector) else {
        return power;
    };
    let rest_world = vec_normalized_or_zero(rest_world);
    if vec_len2(rest_world) <= 0.0 {
        return power;
    }
    let gravity = if gravity_up {
        [0.0, 1.0, 0.0]
    } else {
        [0.0, -1.0, 0.0]
    };
    let to_gravity = vec_dot(rest_world, gravity).clamp(-1.0, 1.0).acos();
    if to_gravity < SETTLED_ANGLE_TOLERANCE {
        return power;
    }
    let settled = (power * to_gravity.sin()).atan2(stiffness + power * to_gravity.cos());
    let settled_world = rotate_towards(rest_world, gravity, settled);
    let Some(inverse) = matrix::inverse(&rest.bone_to_world) else {
        return power;
    };
    let Some(local) = matrix::transform_vector(&inverse, settled_world) else {
        return power;
    };
    let limited_local = limit_direction(
        record,
        t,
        rest.bone_vector,
        vec_normalized_or_zero(local),
        false,
    );
    if vec_len2(limited_local) <= 0.0 {
        return power;
    }
    let Some(limited_world) =
        matrix::transform_vector(&rest.bone_to_world, vec_normalized_or_zero(limited_local))
    else {
        return power;
    };
    let reach = vec_dot(rest_world, vec_normalized_or_zero(limited_world))
        .clamp(-1.0, 1.0)
        .acos();
    if reach >= settled - SETTLED_ANGLE_TOLERANCE {
        return power;
    }
    power.min(stiffness * reach.sin() / (to_gravity - reach).sin())
}

/// 1関節のパラメータ。参照実装の `JointParameters` に相当する。
/// 版1.0とそれ以外で重力式が変わり、制限とheld immobileでpullを強める。
/// 端末は呼び出し側で直前headの値を写す。
#[allow(clippy::too_many_arguments)]
fn joint_parameters(
    source: &SidecarPhysBone,
    head_t: f64,
    tail_t: f64,
    tail_scale: f64,
    bone_length: f64,
    held_immobile: f64,
    rest: Option<&JointRest>,
) -> JointParams {
    let speed = CALIBRATION_FRAME_RATE
        * if bone_length.is_nan() || bone_length.is_infinite() || bone_length <= UNKNOWN_BONE_LENGTH
        {
            CALIBRATION_LENGTH
        } else {
            bone_length
        };
    let pull = clamp01(eval_curved(&source.pull, head_t));
    let mut spring = clamp01(eval_curved(&source.spring, head_t));
    let advanced = source.integration_type == SidecarIntegrationType::Advanced;
    // stiffnessはAdvancedでのみ効く。Simplifiedでは測定上無視された。
    let own_stiffness = if advanced {
        clamp01(eval_curved(&source.stiffness, head_t))
    } else {
        0.0
    };
    let (ratio, drag, gravity_scale) = if advanced {
        (
            pull_ratio(pull, 1.0 - spring * own_stiffness),
            1.0 - spring * (1.0 - own_stiffness),
            1.0,
        )
    } else {
        // spring 1の鎖も戻って静止するため、springは0.9で止める。
        spring = spring.min(MAX_SIMPLIFIED_SPRING);
        let pull_share = pull * (1.0 - spring);
        (
            pull_ratio(pull_share, 1.0),
            1.0 - spring / (1.0 - pull_share).max(1.0 / MAX_PULL_RATIO),
            1.0 - spring,
        )
    };
    let gravity = finite_or_zero(eval_curved(&source.gravity, head_t));
    let falloff = clamp01(eval_curved(&source.gravity_falloff, head_t));
    let gravity_up = gravity < 0.0;
    let to_gravity = rest_to_gravity(rest, gravity_up);
    let version10 = source.version.as_deref() == Some("1.0");
    let mut power = if version10 {
        speed * gravity_ratio10(gravity.abs(), falloff, ratio, gravity_scale, to_gravity)
    } else if pull > 0.0 {
        speed * ratio * gravity_ratio11(gravity.abs(), falloff, to_gravity)
    } else {
        0.0
    };
    let limit = effective_limit_angle(source, head_t, rest);
    let mut held = held_ratio(ratio, clamp01(held_immobile), limit);
    if ratio > 0.0 {
        held = loose_floor(held, drag, limit);
    }
    if ratio > 0.0 {
        power *= held / ratio;
    }
    let stiffness = speed * held;
    power = limited_gravity(source, head_t, rest, stiffness, power, gravity_up);
    let hit_radius = 0.0f64.max(eval_curved(&source.radius, tail_t) * tail_scale.abs());
    JointParams {
        stiffness: finite_or_zero(stiffness),
        drag: clamp01(drag),
        power: finite_or_zero(power),
        gravity_up,
        hit_radius: finite_or_zero(hit_radius),
        held,
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
            // 通常の形状換算では扱わない。平面は `convert_plane_collider` で
            // 拡張付きの球に換算する。黙って球にしない。
            format!("平面colliderは換算できない: {id}"),
        )),
    }
}

/// 平面colliderを拡張付きの球へ換算する。
///
/// `position`・`rotation` はrootローカルの値であり、componentには依存しない。
/// Unity左手系からVRM右手系への鏡像（X反転・四元数のY/Z反転）だけを適用する。
/// 法線は姿勢の+Y軸、平面ずれは位置であり、baseの球は
/// `offset - normal*1000` を中心とする半径1000の球になる
/// （参照実装の `DISTANCE` と同じ）。
/// 拡張の平面は同じ法線とずれを持つ。法線は単位ベクトルに正規化する。
fn convert_plane_collider(
    id: &str,
    source: &SidecarCollider,
) -> Result<(VrmSpringColliderShape, VrmSpringColliderExtension), VrmError> {
    if source.shape_type != SidecarShapeType::Plane {
        return Err(VrmError::new(
            VrmErrorKind::InvalidPhysicsSidecar,
            format!("平面colliderでない: {id}"),
        ));
    }
    // Unity空間で法線を求めてから鏡像する。鏡像はノルムを保つ。
    let normal_unity = quat_rotate(source.rotation, [0.0, 1.0, 0.0]);
    let normal_mirrored = mirror_point(normal_unity);
    let length2 = normal_mirrored[0] * normal_mirrored[0]
        + normal_mirrored[1] * normal_mirrored[1]
        + normal_mirrored[2] * normal_mirrored[2];
    if !length2.is_finite() || length2 <= 0.0 {
        return Err(VrmError::new(
            VrmErrorKind::InvalidPhysicsSidecar,
            format!("平面colliderの法線が縮退している: {id}"),
        ));
    }
    let length = length2.sqrt();
    let normal = [
        normal_mirrored[0] / length,
        normal_mirrored[1] / length,
        normal_mirrored[2] / length,
    ];
    let plane_offset = mirror_point(source.position);
    // baseの球と拡張の平面は有限値である。1000は参照実装の定数である。
    const PLANE_DISTANCE: f64 = 1000.0;
    let base = [
        plane_offset[0] - normal[0] * PLANE_DISTANCE,
        plane_offset[1] - normal[1] * PLANE_DISTANCE,
        plane_offset[2] - normal[2] * PLANE_DISTANCE,
    ];
    let base_f32 = to_f32x3(base, "平面collider基球")?;
    let normal_f32 = to_f32x3(normal, "平面collider法線")?;
    let offset_f32 = to_f32x3(plane_offset, "平面colliderずれ")?;
    Ok((
        VrmSpringColliderShape::Sphere {
            offset: base_f32,
            radius: PLANE_DISTANCE as f32,
        },
        VrmSpringColliderExtension::Plane {
            normal: normal_f32,
            offset: offset_f32,
        },
    ))
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
/// 平面colliderはシーン専用ノードの補助ノードを末尾に追加して出力する。
/// endpoint生成・平均tail生成・ゼロ長分割はしない。
/// centerの計算と制限の評価は行う。未対応は診断と文書に明記する。
///
/// 平面のための補助ノードは、すべての換算が通ってから末尾に追加する。
/// 既存ノードの索引は維持し、失敗時は `VrmModel` を変更しない。
///
/// # Errors
///
/// 上限の0指定・f32に収まらない値・負の半径や硬さ・対応表とsidecarの不整合・
/// 補助ノードの行列が求まらない場合に返す。
pub fn compile_springs(
    sidecar: &PhysicsSidecar,
    model: &mut VrmModel,
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
    // 関節形状の計算用にUnity空間の行列と拡大率も求める。
    let (unity_worlds, unity_max_scales) = unity_worlds_and_scales(sidecar)?;
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
        // 参照実装の `MaxDepth` に相当する。endpointは作らないため、
        // 末端なしのまま深さの最大値を使う。
        let max_depth = plan
            .tree
            .bones
            .iter()
            .map(|bone| bone.depth)
            .max()
            .unwrap_or(0);
        let holds_immobile = plan_immobile(plan, &owned, source).holds;
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
            // 関節ごとに参照実装の式で求める。末端は直前headの値を写す
            // （VRMは末端の値を無視するため）。
            // 角度制限はheadごとに求め、末端（springの末尾）には付けない。
            let mut params_list = Vec::with_capacity(joints.len());
            let mut limits: Vec<Option<VrmSpringJointLimit>> = Vec::with_capacity(joints.len());
            for (index, &sidecar_joint) in joints.iter().enumerate() {
                let is_tail = index + 1 == joints.len() && !truncated;
                if is_tail {
                    let tail = params_list
                        .last()
                        .copied()
                        .expect("末端の前にhead関節がある");
                    params_list.push(tail);
                    // 末端には角度制限を付けない。参照実装のTailはlimitを消す。
                    limits.push(None);
                    // 末端は直前headの写しとして記録する。
                    if options.joint_trace {
                        let mut traced = report
                            .joint_traces
                            .last()
                            .cloned()
                            .expect("末端の前にheadの追跡がある");
                        traced.joint = index;
                        traced.is_head = false;
                        report.joint_traces.push(traced);
                    }
                    continue;
                }
                let entry = plan
                    .tree
                    .bone(sidecar_joint)
                    .expect("鎖の関節は木の骨である");
                let head_t = chain_t(entry.depth, max_depth);
                let tail_t = radius_t(entry.depth + 1, max_depth);
                let held = if holds_immobile {
                    clamp01(eval_curved(&source.immobile, head_t))
                } else {
                    0.0
                };
                let bone_length =
                    matrix::transform_vector(&unity_worlds[sidecar_joint], entry.aim_local)
                        .map(vec_len)
                        .unwrap_or(f64::NAN);
                // 半径の基準はChildのAim先、Averageでは自身である。
                let radius_node = if entry.aim_average {
                    sidecar_joint
                } else {
                    entry.aim.unwrap_or(sidecar_joint)
                };
                let rest = if vec_len2(entry.aim_local) > 0.0
                    && matrix::det3(&unity_worlds[sidecar_joint]) != 0.0
                {
                    Some(JointRest {
                        bone_vector: entry.aim_local,
                        bone_to_world: unity_worlds[sidecar_joint],
                    })
                } else {
                    None
                };
                limits.push(joint_limit(source, head_t, rest.as_ref()));
                params_list.push(joint_parameters(
                    source,
                    head_t,
                    tail_t,
                    unity_max_scales[radius_node],
                    bone_length,
                    held,
                    rest.as_ref(),
                ));
                // 試験用の追跡記録。参照実装の JointTrace 相当にあたり、
                // 出力内容には影響しない。
                if options.joint_trace {
                    let params = params_list
                        .last()
                        .copied()
                        .expect("追跡対象の関節値は直前に求める");
                    let rotation = limit_rotation(source, head_t);
                    let max_angle = max_angles(source, head_t);
                    let (limit, loop_min, loop_max, loop_mean) =
                        if rest.is_some() && has_angle_limit(source) {
                            efflim(entry.aim_local, rotation, max_angle, &source.limit_type)
                        } else {
                            // ループを実行しない場合は制限角そのものを入れる。
                            let plain = limit_angle(source, head_t);
                            (plain, plain, plain, plain)
                        };
                    report.joint_traces.push(JointTrace {
                        bone: bone.id.clone(),
                        spring: name.clone(),
                        joint: index,
                        node: sidecar.nodes[sidecar_joint].id.clone(),
                        is_head: true,
                        head_t,
                        tail_t,
                        limit_kind: source.limit_type,
                        max_angle,
                        limit_rotation: rotation,
                        bone_vector: entry.aim_local,
                        bone_to_world: unity_worlds[sidecar_joint],
                        limit,
                        loop_min,
                        loop_max,
                        loop_mean,
                        held: params.held,
                        stiffness: params.stiffness,
                        drag: params.drag,
                        power: params.power,
                        hit_radius: params.hit_radius,
                        gravity_up: params.gravity_up,
                    });
                }
            }
            // springの末尾は末端として角度制限を付けない。
            // 切り詰めで末端写しが無い場合も、出力上の末尾は制限なしにする。
            if let Some(last) = limits.last_mut() {
                *last = None;
            }
            let joints = vrm_joints
                .into_iter()
                .zip(params_list)
                .zip(limits)
                .map(|((node, params), limit)| {
                    let gravity_dir = if params.gravity_up {
                        [0.0, 1.0, 0.0]
                    } else {
                        [0.0, -1.0, 0.0]
                    };
                    Ok(VrmSpringJoint {
                        node,
                        hit_radius: to_f32_finite(params.hit_radius, "半径")?,
                        stiffness: to_f32_finite(params.stiffness, "硬さ")?,
                        gravity_power: to_f32_finite(params.power, "重力")?,
                        gravity_dir,
                        drag_force: to_f32_finite(params.drag, "抵抗")?,
                        limit,
                    })
                })
                .collect::<Result<Vec<_>, VrmError>>()?;
            report.joint_count += joints.len();
            report.gravity_zero_joints += joints
                .iter()
                .filter(|joint| joint.gravity_power == 0.0)
                .count();
            // endpointは今回は付けない。近似として記録する。
            report.no_endpoint.push(bone.id.clone());
            if source.multi_child_type != SidecarMultiChildType::Ignore
                && !report.first_child_multichild.contains(&bone.id)
            {
                report.first_child_multichild.push(bone.id.clone());
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
            extended: None,
        });
    }
    // 平面colliderはsidecarから直接換算し、補助ノードを末尾に追加して出力する。
    // 参照の有無にかかわらず出力し、参照集合には他colliderと同じ規則で含める。
    // すべての換算が通ってからモデルを変更し、失敗時は追加しない。
    {
        let mut plane_work: Vec<(
            &SidecarCollider,
            VrmSpringColliderShape,
            VrmSpringColliderExtension,
        )> = Vec::new();
        for source in sidecar
            .colliders
            .iter()
            .filter(|collider| collider.shape_type == SidecarShapeType::Plane)
        {
            // 平面は参照の有無にかかわらず出力する。未使用の場合は
            // colliderだけが出てグループには入らない。
            if !source.enabled {
                report.disabled_colliders.push(source.id.clone());
            }
            if source.inside_bounds {
                report
                    .ignored_collider_options
                    .push(format!("{}.insideBounds", source.id));
            }
            if source.bones_as_spheres {
                report
                    .ignored_collider_options
                    .push(format!("{}.bonesAsSpheres", source.id));
            }
            let (shape, extended) = convert_plane_collider(&source.id, source)?;
            plane_work.push((source, shape, extended));
        }
        // 補助ノードの対応付け。同じsidecarノードは1件にまとめる。
        let mut aux_at: HashMap<&str, usize> = HashMap::new();
        for (source, _, _) in &plane_work {
            if aux_at.contains_key(source.root_node_id.as_str()) {
                continue;
            }
            let sidecar_index = sidecar_at
                .get(source.root_node_id.as_str())
                .copied()
                .ok_or_else(|| {
                    VrmError::new(
                        VrmErrorKind::InvalidPhysicsSidecar,
                        format!("平面colliderの基準ノードが無い: {}", source.id),
                    )
                })?;
            let sidecar_node = &sidecar.nodes[sidecar_index];
            // Unity空間の参照ローカルをVRM右手系の世界行列にし、
            // モデルルートの逆変換でローカル行列に戻す。
            // 親なしのrootとして追加するため、ワールド行列がそのまま基準になる。
            let world = trs_to_matrix(&SidecarTrs {
                position: mirror_point(sidecar_node.reference_local.position),
                rotation: mirror_quat(sidecar_node.reference_local.rotation),
                scale: sidecar_node.reference_local.scale,
            });
            let inverse_root = matrix::inverse(&model.root).ok_or_else(|| {
                VrmError::new(
                    VrmErrorKind::InvalidStructure,
                    "モデルのルート行列が正則でない",
                )
            })?;
            let local = matrix::mul(&inverse_root, &world);
            for component in local {
                if !component.is_finite() {
                    return Err(VrmError::new(
                        VrmErrorKind::InvalidPhysicsSidecar,
                        format!("平面colliderの補助ノード行列が非有限である: {}", source.id),
                    ));
                }
            }
            let index = model.nodes.len();
            model.nodes.push(super::VrmNode {
                name: sidecar_node.name.clone(),
                parent: None,
                children: Vec::new(),
                local,
            });
            aux_at.insert(source.root_node_id.as_str(), index);
        }
        for (source, shape, extended) in plane_work {
            let node = aux_at
                .get(source.root_node_id.as_str())
                .copied()
                .expect("平面の補助ノードがある");
            collider_index.insert(source.id.as_str(), colliders.len());
            colliders.push(VrmSpringCollider {
                node,
                shape,
                extended: Some(extended),
            });
        }
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
    // ノード制約はsidecar順に写し、モデルに設定する。写像自体は失敗しない。
    // 補助ノードの追加より後に行い、失敗時の部分更新を避ける配置にする。
    let (node_constraints, skipped_constraints) = compile_node_constraints(sidecar, resolution);
    report.skipped_constraints = skipped_constraints;
    model.node_constraints = node_constraints;
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
    /// Aim先の自空間位置。子1件または `First` では子の参照位置、
    /// `Average` では子位置の平均、Aimなしでは零である。
    /// 参照実装の `AimLocalPosition` に相当する。
    aim_local: [f64; 3],
    /// `Average` によるAimかどうか。半径の基準ノード判定に使う。
    aim_average: bool,
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
        let child_position = |child: usize| sidecar.nodes[child].reference_local.position;
        let (aim, aim_average) = match children.as_slice() {
            [] => (None, false),
            [single] => (Some(*single), false),
            [first, rest @ ..] => match source.multi_child_type {
                SidecarMultiChildType::First => {
                    report.branch_splits.push(BranchSplit {
                        bone: bone.id.clone(),
                        node: node_id(node),
                        aimed: Some(node_id(*first)),
                        heads: rest.iter().map(|&child| node_id(child)).collect(),
                    });
                    (Some(*first), false)
                }
                SidecarMultiChildType::Ignore => {
                    report.branch_splits.push(BranchSplit {
                        bone: bone.id.clone(),
                        node: node_id(node),
                        aimed: None,
                        heads: children.iter().map(|&child| node_id(child)).collect(),
                    });
                    (None, false)
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
                    (Some(*first), true)
                }
            },
        };
        // Aim先の自空間位置。参照実装の `AimLocalPosition` に相当する。
        let aim_local = match (aim, aim_average) {
            (Some(child), false) => child_position(child),
            (Some(_), true) => {
                let mut sum = [0.0; 3];
                for &child in &children {
                    let position = child_position(child);
                    sum[0] += position[0];
                    sum[1] += position[1];
                    sum[2] += position[2];
                }
                let count = children.len() as f64;
                [sum[0] / count, sum[1] / count, sum[2] / count]
            }
            (None, _) => [0.0; 3],
        };
        tree.at.insert(node, tree.bones.len());
        tree.bones.push(TreeBone {
            node,
            parent,
            depth,
            aim,
            aim_local,
            aim_average,
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

/// 半径の深さ位置。SDKが半径を1手先で測ることに対応する。
/// 端末を作らないため、末端なしのまま `depth / maxDepth` を使う。
fn radius_t(depth: usize, max_depth: usize) -> f64 {
    if max_depth == 0 {
        0.0
    } else {
        (depth as f64 / max_depth as f64).clamp(0.0, 1.0)
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

/// humanoidのT-pose適用で上書きした骨1件。
#[derive(Debug, Clone)]
pub struct AppliedHumanoidBone {
    /// 骨名。Unityの `HumanBodyBones` 名である。
    pub bone: String,
    /// 上書きしたVRMノードの索引。
    pub node: usize,
}

/// humanoidのT-pose適用で飛ばした骨1件。
#[derive(Debug, Clone)]
pub struct SkippedHumanoidBone {
    /// 骨名。Unityの `HumanBodyBones` 名である。
    pub bone: String,
    /// 対応するsidecarノードのID。
    pub node_id: String,
    /// 飛ばした理由。
    pub reason: SkippedHumanoidReason,
}

/// humanoidのT-pose適用で飛ばした理由。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkippedHumanoidReason {
    /// `skeleton` に無い骨であり `tPose` が無い場合。
    /// 参照実装が `node.reference` をT-poseに使う場合に相当し、
    /// こちらはノードの姿勢をそのまま残す。
    MissingTPose,
    /// sidecarノードIDが対応表やsidecar自体に無い場合。
    UnresolvedNode,
    /// 対応表の指すVRMノードがモデルに無い場合。
    MissingModelNode,
}

impl std::fmt::Display for SkippedHumanoidReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingTPose => write!(f, "T-poseが無い"),
            Self::UnresolvedNode => write!(f, "ノードが解決できない"),
            Self::MissingModelNode => write!(f, "モデルにノードが無い"),
        }
    }
}

/// humanoidのT-pose適用の報告。CLIの記録と試験の観測に使う。
#[derive(Debug, Clone, Default)]
pub struct HumanoidPoseReport {
    /// 上書きした骨列。sidecarの骨順である。
    pub applied: Vec<AppliedHumanoidBone>,
    /// 飛ばした骨列。sidecarの骨順である。
    pub skipped: Vec<SkippedHumanoidBone>,
    /// 差分δの角度が0.01度以上の骨列。sidecarの骨順である。
    /// 参照姿勢とT-poseが実質的に違う骨だけが残る。
    pub changed: Vec<ChangedHumanoidBone>,
}

/// humanoidのT-pose適用で差分が0.01度以上あった骨1件。
#[derive(Debug, Clone)]
pub struct ChangedHumanoidBone {
    /// 骨名。Unityの `HumanBodyBones` 名である。
    pub bone: String,
    /// 上書きしたVRMノードの索引。
    pub node: usize,
    /// 差分δの角度（度）。`inverse(reference) * tPose` の回転角である。
    pub delta_deg: f64,
}

/// 参照姿勢とT-poseの差分を「変化あり」とみなす角度（度）。
/// 参照実装の計測で0.01度以上違った8骨（肩・親指・中指）を捉える目安であり、
/// f32由来の丸め（1e-5度程度）を拾わない大きさである。
const HUMANOID_CHANGED_DEGREES: f64 = 0.01;

/// 適用する差分δが恒等とみなせる角度（ラジアン）。
/// これ未満の骨は行列を触らず、丸めを入れない。1e-12ラジアンは約1e-10度である。
const HUMANOID_IDENTITY_DELTA_RAD: f64 = 1e-12;

/// sidecarのhumanoid記録（T-pose）を [`VrmModel`] のローカル回転へ適用する。
///
/// 参照実装の `RigCompiler.Humanoid.PoseHumanoid` はリグのローカル回転を
/// `tPose` で置換する。こちらは参照実装とフレームが違うため（`Armature` の
/// -90度Xを根元に焼き込まない等）、置換ではなく参照姿勢からの差分δを
/// 右から掛ける。`δ = inverse(reference_local) * tPose`（Unity空間）を
/// 鏡像規則（四元数の `(x, -y, -z, w)`）でVRM空間へ変換し、正規化してから
/// `local' = local * R(δ)` とする。3x3の列ベクトルを回転させ、平行移動は
/// そのまま残す。分解・再構成はしないため、シアー（直交しない基底）を
/// 持つ行列も壊さない。こちらとUnityのワールド回転は鏡像 `M` で結ばれるため
/// （`W_our = M(W_u)`）、`W_u' = W_u * δ` に対して
/// `W_our' = M(W_u) * M(δ)` となり、同じ姿勢変化になる。
/// δが `HUMANOID_IDENTITY_DELTA_RAD` 未満の骨は行列を触らず、
/// `applied` にだけ記録して丸めを入れない。
/// `tPose` が無い骨・解決できないノード・モデルに無いノードは記録して飛ばし、
/// 失敗にはしない。`humanoid` が `None` の旧形式sidecarでは何もせず空の報告を
/// 返す。呼び出し側は [`compile_springs`] より前に呼ぶ（参照実装もリグ構築直後に
/// T-poseを当ててから物理を解決するため）。適用は全件の検証が通ってから
/// 一括で行い、部分更新はしない。同じVRMノードに2件の骨が対応する場合は失敗する。
///
/// # Errors
///
/// T-poseや参照姿勢の回転の非有限値やゼロ・
/// 同じノードへの重複の場合に返す。この場合モデルは変更しない。
pub fn apply_humanoid_t_pose(
    sidecar: &PhysicsSidecar,
    model: &mut VrmModel,
    resolution: &PhysicsResolution,
    humanoid: Option<&SidecarHumanoid>,
) -> Result<HumanoidPoseReport, VrmError> {
    let Some(humanoid) = humanoid else {
        return Ok(HumanoidPoseReport::default());
    };
    let sidecar_at: HashMap<&str, &SidecarNode> = sidecar
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect();
    // 先に全件を検証し、適用計画だけを作る。失敗時は部分更新しない。
    // 差分δはUnity空間の参照姿勢から求め、VRM空間へ鏡像して正規化する。
    let mut planned: Vec<(String, usize, [f64; 4], f64)> = Vec::with_capacity(humanoid.bones.len());
    let mut report = HumanoidPoseReport::default();
    let mut seen_nodes: HashSet<usize> = HashSet::new();
    for bone in &humanoid.bones {
        let Some(t_pose) = bone.t_pose.as_ref() else {
            report.skipped.push(SkippedHumanoidBone {
                bone: bone.bone.clone(),
                node_id: bone.node_id.clone(),
                reason: SkippedHumanoidReason::MissingTPose,
            });
            continue;
        };
        let (Some(&index), Some(sidecar_node)) = (
            resolution.node_map.get(bone.node_id.as_str()),
            sidecar_at.get(bone.node_id.as_str()),
        ) else {
            report.skipped.push(SkippedHumanoidBone {
                bone: bone.bone.clone(),
                node_id: bone.node_id.clone(),
                reason: SkippedHumanoidReason::UnresolvedNode,
            });
            continue;
        };
        if model.nodes.get(index).is_none() {
            report.skipped.push(SkippedHumanoidBone {
                bone: bone.bone.clone(),
                node_id: bone.node_id.clone(),
                reason: SkippedHumanoidReason::MissingModelNode,
            });
            continue;
        }
        if !seen_nodes.insert(index) {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPhysicsSidecar,
                format!("humanoidのT-poseが同じノードに重複する: {}", bone.node_id),
            ));
        }
        // 検証済みのDTOを直接受け取る場合もあるため、ここでも有限値と
        // 非ゼロを確認する。失敗時は適用前のため部分更新は起きない。
        let reference = normalize_humanoid_quat(
            &sidecar_node.reference_local.rotation,
            bone.bone.as_str(),
            "参照姿勢",
        )?;
        let t_pose = normalize_humanoid_quat(&t_pose.rotation, bone.bone.as_str(), "T-pose")?;
        let delta = quat_mul(quat_inverse(reference), t_pose);
        let delta_deg = quat_angle_deg(delta);
        // 差分をVRM空間へ鏡像して正規化する。鏡像は群同型のため、
        // 往復で厳密に戻り、角度も保つ。
        let mirrored = normalize_mirrored_quat(&mirror_quat(delta), bone.bone.as_str())?;
        planned.push((bone.bone.clone(), index, mirrored, delta_deg));
    }
    // 検証が全件通ってから一括で適用する。差分はローカル行列に右から掛け、
    // 平行移動は自動的に保つ（回転行列の平行移動成分がゼロのため）。
    // 恒等に近い差分の骨は行列を触らず、丸めを入れない。
    for (bone, index, delta, delta_deg) in &planned {
        if delta_deg.to_radians() >= HUMANOID_IDENTITY_DELTA_RAD {
            let local = model.nodes[*index].local;
            model.nodes[*index].local = matrix::mul(&local, &rotation_matrix(delta));
        }
        report.applied.push(AppliedHumanoidBone {
            bone: bone.clone(),
            node: *index,
        });
        if *delta_deg >= HUMANOID_CHANGED_DEGREES {
            report.changed.push(ChangedHumanoidBone {
                bone: bone.clone(),
                node: *index,
                delta_deg: *delta_deg,
            });
        }
    }
    Ok(report)
}

/// 単位四元数から回転のみの4x4行列（列優先）を作る。平行移動はゼロである。
fn rotation_matrix(rotation: &[f64; 4]) -> [f64; 16] {
    let (x, y, z, w) = (rotation[0], rotation[1], rotation[2], rotation[3]);
    let r00 = 1.0 - 2.0 * (y * y + z * z);
    let r10 = 2.0 * (x * y + z * w);
    let r20 = 2.0 * (x * z - y * w);
    let r01 = 2.0 * (x * y - z * w);
    let r11 = 1.0 - 2.0 * (x * x + z * z);
    let r21 = 2.0 * (y * z + x * w);
    let r02 = 2.0 * (x * z + y * w);
    let r12 = 2.0 * (y * z - x * w);
    let r22 = 1.0 - 2.0 * (x * x + y * y);
    [
        r00, r10, r20, 0.0, //
        r01, r11, r21, 0.0, //
        r02, r12, r22, 0.0, //
        0.0, 0.0, 0.0, 1.0, //
    ]
}

/// humanoidの姿勢四元数を正規化する。有限値と非ゼロを確認する。
///
/// # Errors
///
/// 非有限値やゼロ（ノルム2乗が `1e-8` 未満）の場合に返す。
fn normalize_humanoid_quat(
    rotation: &[f64; 4],
    bone: &str,
    what: &str,
) -> Result<[f64; 4], VrmError> {
    if !rotation.iter().all(|v| v.is_finite()) {
        return Err(VrmError::new(
            VrmErrorKind::InvalidPhysicsSidecar,
            format!("humanoidの{what}回転に非有限値がある: {bone}"),
        ));
    }
    normalize_mirrored_quat(rotation, bone).map_err(|_| {
        VrmError::new(
            VrmErrorKind::InvalidPhysicsSidecar,
            format!("humanoidの{what}回転がゼロである: {bone}"),
        )
    })
}

/// 鏡像済みの四元数を正規化する。有限値であることは呼び出し側で確認済みとする。
///
/// # Errors
///
/// ゼロ（ノルム2乗が `1e-8` 未満）の場合に返す。
fn normalize_mirrored_quat(rotation: &[f64; 4], bone: &str) -> Result<[f64; 4], VrmError> {
    let squared = rotation[0] * rotation[0]
        + rotation[1] * rotation[1]
        + rotation[2] * rotation[2]
        + rotation[3] * rotation[3];
    if !squared.is_finite() || squared < 1e-8 {
        return Err(VrmError::new(
            VrmErrorKind::InvalidPhysicsSidecar,
            format!("humanoidのT-pose回転がゼロである: {bone}"),
        ));
    }
    let length = squared.sqrt();
    Ok([
        rotation[0] / length,
        rotation[1] / length,
        rotation[2] / length,
        rotation[3] / length,
    ])
}

/// 単位四元数の逆元。共役である。
fn quat_inverse(q: [f64; 4]) -> [f64; 4] {
    [-q[0], -q[1], -q[2], q[3]]
}

/// 単位四元数の回転角（度）。符号は同一視し、非単位の入力は正規化しない。
/// 呼び出し側は正規化済みの値を渡す。
fn quat_angle_deg(q: [f64; 4]) -> f64 {
    2.0 * q[3].abs().clamp(0.0, 1.0).acos().to_degrees()
}

#[cfg(test)]
mod joint_tests {
    use super::*;

    /// キー無しのカーブ。
    fn plain(value: f64) -> SidecarCurve {
        SidecarCurve {
            value,
            keys: Vec::new(),
            pre_wrap_mode: 8,
            post_wrap_mode: 8,
        }
    }

    /// 1件のキー。
    fn key(time: f64, value: f64, in_slope: f64, out_slope: f64) -> SidecarCurveKey {
        SidecarCurveKey {
            time,
            value,
            in_slope,
            out_slope,
            weighted_mode: 0,
            in_weight: 0.0,
            out_weight: 0.0,
        }
    }

    /// 試験用のPhysBone。曲線は全てキー無しである。
    fn bone() -> SidecarPhysBone {
        SidecarPhysBone {
            id: "pb:test".to_string(),
            component_node_id: "n:0".to_string(),
            root_node_id: "n:0".to_string(),
            enabled: true,
            version: Some("1.1".to_string()),
            integration_type: SidecarIntegrationType::Simplified,
            ignore_node_ids: Vec::new(),
            ignore_other_phys_bones: false,
            endpoint_position: [0.0; 3],
            multi_child_type: SidecarMultiChildType::Ignore,
            pull: plain(0.0),
            spring: plain(0.0),
            stiffness: plain(0.0),
            gravity: plain(0.0),
            gravity_falloff: plain(0.0),
            immobile_type: SidecarImmobileType::AllMotion,
            immobile: plain(0.0),
            radius: plain(0.0),
            allow_collision: true,
            collider_ids: Vec::new(),
            limit_type: SidecarLimitType::None,
            max_angle_x: plain(0.0),
            max_angle_z: plain(0.0),
            limit_rotation: [0.0; 3],
            limit_rotation_x_curve: plain(1.0),
            limit_rotation_y_curve: plain(1.0),
            limit_rotation_z_curve: plain(1.0),
            is_animated: false,
        }
    }

    /// キー無しは既定値を返す。
    #[test]
    fn 曲線なしは既定値を返す() {
        assert_eq!(eval_curved(&plain(0.3), 0.7), 0.3);
        assert_eq!(eval_unity_curve(&plain(0.3), 0.7), 1.0);
    }

    /// 1キーは時刻によらず値を返す。
    #[test]
    fn 単一キーは値を返す() {
        let mut curve = plain(0.0);
        curve.keys.push(key(0.5, 2.0, 0.0, 0.0));
        assert_eq!(eval_unity_curve(&curve, 0.0), 2.0);
        assert_eq!(eval_unity_curve(&curve, 9.0), 2.0);
        assert_eq!(eval_curved(&curve, 0.25), 0.0);
    }

    /// 傾き0の2キーは中点で平均を返す。
    #[test]
    fn 直線補間は中点で平均になる() {
        let mut curve = plain(0.0);
        curve.keys.push(key(0.0, 10.0, 0.0, 0.0));
        curve.keys.push(key(1.0, 20.0, 0.0, 0.0));
        assert!((eval_unity_curve(&curve, 0.5) - 15.0).abs() < 1e-12);
    }

    /// Hermiteの傾きは区間幅を掛けて使う。
    #[test]
    fn 傾き付き補間は三次式に従う() {
        let mut curve = plain(0.0);
        curve.keys.push(key(0.0, 0.0, 0.0, 3.0));
        curve.keys.push(key(1.0, 1.0, 0.0, 0.0));
        // h00=0.5、h10=0.125、h01=0.5、h11=-0.125 のため0.5+0.375=0.875。
        assert!((eval_unity_curve(&curve, 0.5) - 0.875).abs() < 1e-12);
    }

    /// 時刻0の重複キーは0除算を避けて先の値を返す。
    #[test]
    fn 時刻重複は先の値を返す() {
        let mut curve = plain(0.0);
        curve.keys.push(key(0.0, 1.0, 0.0, 0.0));
        curve.keys.push(key(0.0, 2.0, 0.0, 0.0));
        assert_eq!(eval_unity_curve(&curve, 0.0), 1.0);
    }

    /// 範囲外は端値に丸める（Clamp相当）。
    #[test]
    fn 範囲外は端値になる() {
        let mut curve = plain(0.0);
        curve.keys.push(key(0.0, 10.0, 0.0, 0.0));
        curve.keys.push(key(1.0, 20.0, 0.0, 0.0));
        assert_eq!(eval_unity_curve(&curve, -2.0), 10.0);
        assert_eq!(eval_unity_curve(&curve, 5.0), 20.0);
    }

    /// Loop wrapは折り返す。
    #[test]
    fn 繰り返しは折り返す() {
        let mut curve = plain(0.0);
        curve.pre_wrap_mode = 2;
        curve.post_wrap_mode = 2;
        curve.keys.push(key(0.0, 10.0, 0.0, 0.0));
        curve.keys.push(key(1.0, 20.0, 0.0, 0.0));
        assert!((eval_unity_curve(&curve, 2.5) - 15.0).abs() < 1e-12);
        assert!((eval_unity_curve(&curve, -0.5) - 15.0).abs() < 1e-12);
    }

    /// PingPong wrapは往復する。
    #[test]
    fn 往復は折り返して戻る() {
        let mut curve = plain(0.0);
        curve.pre_wrap_mode = 4;
        curve.post_wrap_mode = 4;
        curve.keys.push(key(0.0, 10.0, 0.0, 0.0));
        curve.keys.push(key(1.0, 20.0, 0.0, 0.0));
        assert!((eval_unity_curve(&curve, 1.5) - 15.0).abs() < 1e-12);
        assert!((eval_unity_curve(&curve, 0.5) - 15.0).abs() < 1e-12);
    }

    /// pull比の基本。0は0、1は上限、0.5は等倍である。
    #[test]
    fn 引き比は分配に従う() {
        assert_eq!(pull_ratio(0.0, 1.0), 0.0);
        assert_eq!(pull_ratio(1.0, 1.0), MAX_PULL_RATIO);
        assert!((pull_ratio(0.5, 1.0) - 1.0).abs() < 1e-12);
    }

    /// 保持比の基本。0は0、制限なしは等倍、きつい制限は強める。
    #[test]
    fn 保持比は制限で強まる() {
        assert_eq!(held_ratio(0.0, 0.5, 10.0), 0.0);
        assert!((held_ratio(2.0, 0.0, f64::INFINITY) - 2.0).abs() < 1e-12);
        // 制限11.25度は基準の4分の1のため2倍になる。
        assert!((held_ratio(2.0, 0.0, 11.25) - 4.0).abs() < 1e-12);
        // 不動1は上限に張り付く。
        assert_eq!(held_ratio(2.0, 1.0, f64::INFINITY), MAX_PULL_RATIO);
    }

    /// 版1.1の重力比。0は0、半分は等倍である。
    #[test]
    fn 重力比11は比に従う() {
        assert_eq!(gravity_ratio11(0.0, 0.0, 1.0), 0.0);
        assert!((gravity_ratio11(0.5, 0.0, 1.0) - 1.0).abs() < 1e-12);
    }

    /// 版1.0の重力比。0は0である。
    #[test]
    fn 重力比10は0で0になる() {
        assert_eq!(gravity_ratio10(0.0, 0.0, 1.0, 1.0, 1.0), 0.0);
    }

    /// Advancedの基本。pull 1・spring 0・stiffness 1で上限の比になる。
    #[test]
    fn 高精度積分は上限の比になる() {
        let mut source = bone();
        source.integration_type = SidecarIntegrationType::Advanced;
        source.pull = plain(1.0);
        source.stiffness = plain(1.0);
        // 速度6・比60・保持60のため硬さ360、抵抗1、重力0である。
        let params = joint_parameters(&source, 0.0, 0.0, 1.0, 0.1, 0.0, None);
        assert!((params.stiffness - 360.0).abs() < 1e-9);
        assert_eq!(params.drag, 1.0);
        assert_eq!(params.power, 0.0);
        assert!(!params.gravity_up);
        assert_eq!(params.hit_radius, 0.0);
    }

    /// Simplifiedの基本。pull・spring半分で3分の1の比になる。
    #[test]
    fn 単純化積分は分配に従う() {
        let mut source = bone();
        source.pull = plain(0.5);
        source.spring = plain(0.5);
        // pull共有0.25・比1/3・抵抗1/3・速度6のため硬さ2である。
        let params = joint_parameters(&source, 0.0, 0.0, 1.0, 0.1, 0.0, None);
        assert!((params.stiffness - 2.0).abs() < 1e-9);
        assert!((params.drag - (1.0 / 3.0)).abs() < 1e-12);
        assert_eq!(params.power, 0.0);
    }

    /// 版1.0の重力。pull 1・falloffなしで61倍の速度になる。
    #[test]
    fn 版10の重力は速度の61倍になる() {
        let mut source = bone();
        source.version = Some("1.0".to_string());
        source.pull = plain(1.0);
        source.gravity = plain(1.0);
        // 速度6・比60・重力比61のため重力366、硬さ360である。
        let params = joint_parameters(&source, 0.0, 0.0, 1.0, 0.1, 0.0, None);
        assert!((params.power - 366.0).abs() < 1e-9);
        assert!((params.stiffness - 360.0).abs() < 1e-9);
        assert!(!params.gravity_up);
    }

    /// falloff最大はrest姿勢の重力を消す。
    #[test]
    fn 最大減衰は重力を消す() {
        let mut source = bone();
        source.version = Some("1.0".to_string());
        source.pull = plain(1.0);
        source.gravity = plain(1.0);
        source.gravity_falloff = plain(1.0);
        let params = joint_parameters(&source, 0.0, 0.0, 1.0, 0.1, 0.0, None);
        assert_eq!(params.power, 0.0);
    }

    /// 中間減衰は重力を弱める。
    #[test]
    fn 中間減衰は重力を弱める() {
        let mut full = bone();
        full.version = Some("1.0".to_string());
        full.pull = plain(1.0);
        full.gravity = plain(1.0);
        let mut half = full.clone();
        half.gravity_falloff = plain(0.5);
        let base_power = joint_parameters(&full, 0.0, 0.0, 1.0, 0.1, 0.0, None).power;
        let half_power = joint_parameters(&half, 0.0, 0.0, 1.0, 0.1, 0.0, None).power;
        assert!(half_power > 0.0 && half_power < base_power);
    }

    /// 負の重力は上向きになる。
    #[test]
    fn 負の重力は上向きになる() {
        let mut source = bone();
        source.version = Some("1.0".to_string());
        source.pull = plain(1.0);
        source.gravity = plain(-1.0);
        let params = joint_parameters(&source, 0.0, 0.0, 1.0, 0.1, 0.0, None);
        assert!(params.gravity_up);
        assert!(params.power > 0.0);
    }

    /// held immobileは引きを強める。
    #[test]
    fn 保持は引きを強める() {
        let mut source = bone();
        source.integration_type = SidecarIntegrationType::Advanced;
        source.pull = plain(0.5);
        // 比1・不動0.5のため保持2・硬さ12、不動なしなら硬さ6である。
        let held = joint_parameters(&source, 0.0, 0.0, 1.0, 0.1, 0.5, None);
        let free = joint_parameters(&source, 0.0, 0.0, 1.0, 0.1, 0.0, None);
        assert!((held.stiffness - 12.0).abs() < 1e-9);
        assert!((free.stiffness - 6.0).abs() < 1e-9);
    }

    /// 制限なしの制限角は無限大、Polarは平均である。
    #[test]
    fn 制限角は種別に従う() {
        let plain_bone = bone();
        assert_eq!(limit_angle(&plain_bone, 0.0), f64::INFINITY);
        let mut angle = bone();
        angle.limit_type = SidecarLimitType::Angle;
        angle.max_angle_x = plain(30.0);
        assert!((limit_angle(&angle, 0.0) - 30.0).abs() < 1e-12);
        let mut polar = bone();
        polar.limit_type = SidecarLimitType::Polar;
        polar.max_angle_x = plain(30.0);
        polar.max_angle_z = plain(60.0);
        assert!((limit_angle(&polar, 0.0) - 45.0).abs() < 1e-12);
    }

    /// rest不明時は制限角に落ちる。
    #[test]
    fn 不明形状は制限角に落ちる() {
        let mut angle = bone();
        angle.limit_type = SidecarLimitType::Angle;
        angle.max_angle_x = plain(30.0);
        assert!((effective_limit_angle(&angle, 0.0, None) - 30.0).abs() < 1e-12);
    }

    /// 広い制限は方向を変えない。
    #[test]
    fn 広い制限は方向を変えない() {
        let mut polar = bone();
        polar.limit_type = SidecarLimitType::Polar;
        polar.max_angle_x = plain(180.0);
        polar.max_angle_z = plain(90.0);
        let direction = [0.3, 0.5, 0.8];
        let limited = apply_angle_limit(
            &polar.limit_type,
            [0.0, 1.0, 0.0],
            [0.0; 3],
            [180.0, 90.0],
            direction,
        );
        let want = vec_normalized_or_zero(direction);
        for i in 0..3 {
            assert!((limited[i] - want[i]).abs() < 1e-12);
        }
    }

    /// 錐体制限は角度内に収める。
    #[test]
    fn 錐体制限は角度内に収める() {
        let tilted = [0.0, 0.9961946980917455, 0.08715574274765817];
        let kept = clamp_to_cone([0.0, 1.0, 0.0], tilted, 45.0);
        assert!((vec_angle_deg([0.0, 1.0, 0.0], kept) - 5.0).abs() < 1e-9);
        let cut = clamp_to_cone([0.0, 1.0, 0.0], tilted, 2.0);
        assert!((vec_angle_deg([0.0, 1.0, 0.0], cut) - 2.0).abs() < 1e-9);
    }

    /// 角度制限の出力形。種別ごとに円錐・蝶番・球面へ写す。
    #[test]
    fn 制限出力は種別に従う() {
        let rest = JointRest {
            bone_vector: [0.0, 1.0, 0.0],
            bone_to_world: [
                1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
            ],
        };
        let mut polar = bone();
        polar.limit_type = SidecarLimitType::Polar;
        polar.max_angle_x = plain(30.0);
        polar.max_angle_z = plain(60.0);
        let spherical = joint_limit(&polar, 0.0, Some(&rest)).expect("Polarは球面になる");
        match spherical {
            VrmSpringJointLimit::Spherical {
                pitch,
                yaw,
                rotation,
            } => {
                assert!((pitch - 30.0f32.to_radians()).abs() < 1e-6);
                assert!((yaw - 60.0f32.to_radians()).abs() < 1e-6);
                assert!((rotation[3] - 1.0).abs() < 1e-6);
            }
            _ => panic!("Polarは球面である"),
        }
        let mut angle = bone();
        angle.limit_type = SidecarLimitType::Angle;
        angle.max_angle_x = plain(30.0);
        let cone = joint_limit(&angle, 0.0, Some(&rest)).expect("Angleは円錐になる");
        match cone {
            VrmSpringJointLimit::Cone { angle, rotation } => {
                assert!((angle - 30.0f32.to_radians()).abs() < 1e-6);
                assert!((rotation[3] - 1.0).abs() < 1e-6);
            }
            _ => panic!("Angleは円錐である"),
        }
        let mut hinge = bone();
        hinge.limit_type = SidecarLimitType::Hinge;
        hinge.max_angle_x = plain(30.0);
        let hinge_limit = joint_limit(&hinge, 0.0, Some(&rest)).expect("Hingeは蝶番になる");
        match hinge_limit {
            VrmSpringJointLimit::Hinge { angle, rotation } => {
                assert!((angle - 30.0f32.to_radians()).abs() < 1e-6);
                assert!((rotation[3] - 1.0).abs() < 1e-6);
            }
            _ => panic!("Hingeは蝶番である"),
        }
    }

    /// 角度制限の上限。180度・90度で止める。
    #[test]
    fn 制限出力は上限で止まる() {
        let rest = JointRest {
            bone_vector: [0.0, 1.0, 0.0],
            bone_to_world: [
                1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
            ],
        };
        let mut polar = bone();
        polar.limit_type = SidecarLimitType::Polar;
        polar.max_angle_x = plain(200.0);
        polar.max_angle_z = plain(120.0);
        let limited = joint_limit(&polar, 0.0, Some(&rest)).expect("上限でも球面になる");
        match limited {
            VrmSpringJointLimit::Spherical { pitch, yaw, .. } => {
                assert!((pitch - std::f32::consts::PI).abs() < 1e-6);
                assert!((yaw - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
            }
            _ => panic!("Polarは球面である"),
        }
    }

    /// 制限なし・rest不明は制限なしになる。
    #[test]
    fn 制限なしは出力しない() {
        let rest = JointRest {
            bone_vector: [0.0, 1.0, 0.0],
            bone_to_world: [
                1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
            ],
        };
        let none = bone();
        assert!(joint_limit(&none, 0.0, Some(&rest)).is_none());
        let mut polar = bone();
        polar.limit_type = SidecarLimitType::Polar;
        polar.max_angle_x = plain(30.0);
        polar.max_angle_z = plain(60.0);
        assert!(joint_limit(&polar, 0.0, None).is_none());
    }
}
