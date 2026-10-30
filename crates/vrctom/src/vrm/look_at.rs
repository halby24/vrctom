//! VRMC_vrm lookAt（bone型の視線）の測定と書き出し。
//!
//! 入口は [`resolve_look_at`]（sidecarの視線記録とhumanoidからの測定）と
//! [`look_at_json`]（`VRMC_vrm.lookAt` のJSON化）である。
//! 角度の測定はUnity空間（左手系）で行う。[`VrmModel`](super::VrmModel) の
//! ワールド回転はVRM空間なので、既存の鏡像規則（四元数の `(x,-y,-z,w)`）で
//! Unity空間へ戻してから参照実装の式をそのまま適用する。鏡像は群同型のため、
//! 往復で厳密に戻る。`offsetFromHeadBone` はVRM空間のまま
//! `inverse(head世界) * 眼の中点` で求め、そのまま書く。

use serde::Deserialize;

use super::VrmModel;
use super::error::{VrmError, VrmErrorKind};
use super::export::{VrmExportError, VrmExportErrorKind};
use super::humanoid::{HumanoidBone, HumanoidBones};
use super::matrix;
use super::physics::{PhysicsResolution, PhysicsSidecar, mirror_quat};

/// 既定の入力最大角（度）。測定なし・未測定の方向に使う。
const DEFAULT_INPUT_DEGREES: f32 = 90.0;
/// 既定の出力角（度）。参照実装の `RigLookAtRange::DefaultDegrees` である。
const DEFAULT_OUTPUT_DEGREES: f32 = 10.0;
/// 範囲の上限（度）。参照実装の `RigLookAtRange::MaxDegrees` である。
const MAX_DEGREES: f32 = 90.0;
/// 測定値の入力最大角の下限（度）。参照実装の `MinMeasuredInputDegrees` である。
/// 0度の測定でも有効な範囲写像になるための下限である。
const MIN_MEASURED_INPUT_DEGREES: f32 = 1.0;
/// 逆向きとみなす許容（度）。参照実装の `WrongWayToleranceDegrees` である。
/// これより逆に振れた方向は測定不能として既定値に戻す。
const WRONG_WAY_TOLERANCE_DEGREES: f64 = 0.5;
/// 表情型の出力重み。参照実装の既定（入力90度で重み1.0）である。
const EXPRESSION_OUTPUT: f32 = 1.0;
/// 表情型の頭基準ずれ。参照実装の `DefaultOffsetFromHead`（頭上6cm）である。
const EXPRESSION_OFFSET_FROM_HEAD: [f32; 3] = [0.0, 0.06, 0.0];

/// sidecarの視線記録。Unityヘルパがdescriptorの生値を写したものである。
///
/// 旧形式のsidecarには無いため、[`PhysicsSidecar`](super::physics::PhysicsSidecar) 側では
/// `Option` で受け、無い場合は後方互換で視線を扱わない。
#[derive(Debug, Clone, Deserialize)]
pub struct SidecarLookAt {
    /// Eye Lookの有効可否。descriptorの `enableEyeLook` の生値である。
    #[serde(rename = "eyeLook", default)]
    pub eye_look: bool,
    /// 左目のdescriptor骨のsidecarノードID。未設定は空文字である。
    #[serde(rename = "leftEyeNodeId", default, deserialize_with = "de_node_id")]
    pub left_eye_node_id: String,
    /// 右目のdescriptor骨のsidecarノードID。未設定は空文字である。
    #[serde(rename = "rightEyeNodeId", default, deserialize_with = "de_node_id")]
    pub right_eye_node_id: String,
    /// 正面視の左右の回転。descriptorの生値（正規化しない）である。
    #[serde(rename = "eyesLookingStraight", default)]
    pub eyes_looking_straight: SidecarEyePose,
    /// 上視の左右の回転。
    #[serde(rename = "eyesLookingUp", default)]
    pub eyes_looking_up: SidecarEyePose,
    /// 下視の左右の回転。
    #[serde(rename = "eyesLookingDown", default)]
    pub eyes_looking_down: SidecarEyePose,
    /// 左視の左右の回転。
    #[serde(rename = "eyesLookingLeft", default)]
    pub eyes_looking_left: SidecarEyePose,
    /// 右視の左右の回転。
    #[serde(rename = "eyesLookingRight", default)]
    pub eyes_looking_right: SidecarEyePose,
}

/// 視線1方向の左右の回転。descriptorの生値（正規化しない）である。
#[derive(Debug, Clone, Deserialize)]
pub struct SidecarEyePose {
    /// 左右同値かどうか。真の場合は左の値を両目に使う。
    #[serde(default)]
    pub linked: bool,
    /// 左の回転 `[x, y, z, w]`。
    #[serde(default = "default_identity_quat")]
    pub left: [f64; 4],
    /// 右の回転 `[x, y, z, w]`。
    #[serde(default = "default_identity_quat")]
    pub right: [f64; 4],
}

impl Default for SidecarEyePose {
    /// 未設定の姿勢。非linkedの恒等回転であり、参照実装の未初期化時と同じである。
    fn default() -> Self {
        Self {
            linked: false,
            left: default_identity_quat(),
            right: default_identity_quat(),
        }
    }
}

/// 空の単位四元数。姿勢の欠落時は参照実装と同様に恒等回転として扱う。
fn default_identity_quat() -> [f64; 4] {
    [0.0, 0.0, 0.0, 1.0]
}

/// ノードIDの読み取り。欠落・`null` は未設定の空文字として扱う。
fn de_node_id<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer).map(|value| value.unwrap_or_default())
}

/// VRMの視線設定。[`VrmModel`](super::VrmModel) が持ち、書き出し層が出力する。
#[derive(Debug, Clone)]
pub enum VrmLookAt {
    /// 骨視線。両目と頭があり、頭基準のずれと4方向の範囲を持つ。
    Bone {
        /// 左目のノード索引。
        left_eye: usize,
        /// 右目のノード索引。
        right_eye: usize,
        /// 頭基準の注視原点。VRM空間の `inverse(head世界) * 眼の中点` である。
        offset_from_head: [f32; 3],
        /// 4方向の出力角（度）。入力最大角は測定時は同値（下限1度）、
        /// 未測定時は90度である。
        range: VrmLookAtRange,
        /// 測定値を使ったかどうか。偽の場合は既定の範囲である。
        measured: bool,
    },
    /// 表情視線。目が無い場合に使い、頭上6cmを原点に重み1.0で見る。
    Expression {
        /// 頭基準の注視原点。頭上6cmの既定値である。
        offset_from_head: [f32; 3],
    },
}

/// 視線の4方向の範囲。いずれも出力角（度）である。
#[derive(Debug, Clone, Copy)]
pub struct VrmLookAtRange {
    /// 内向きの最大角。
    pub horizontal_inner: f32,
    /// 外向きの最大角。
    pub horizontal_outer: f32,
    /// 下向きの最大角。
    pub vertical_down: f32,
    /// 上向きの最大角。
    pub vertical_up: f32,
}

/// 視線解決の報告。CLIの記録と試験の観測に使う。
#[derive(Debug, Clone)]
pub struct LookAtReport {
    /// 視線の種別。`"bone"` または `"expression"` である。
    pub look_type: &'static str,
    /// 測定値を使ったかどうか。偽の場合は既定の範囲である。
    pub measured: bool,
    /// 4方向の出力角（度）。表情型では重み1.0である。
    pub range: VrmLookAtRange,
    /// 頭基準の注視原点。
    pub offset_from_head: [f32; 3],
}

/// sidecarの視線記録を [`VrmModel`](super::VrmModel) の視線設定へ解決する。
///
/// sidecarに視線記録が無い旧形式では `None` を返し、呼び出し側は何もしない
/// （後方互換）。ある場合は骨型か表情型のいずれかを返す。目はhumanoidの
/// 左右の目と頭がすべてある場合だけ骨型になり、それ以外は表情型になる。
/// 骨型の範囲は、Eye Lookが有効かつdescriptorの目がhumanoidの目と一致し、
/// 5姿勢が有効な場合だけ測定値を使い、それ以外は既定（入力90度・出力10度）に
/// 戻す。方向ごとの無効値（逆向き・上限超え）はその方向だけ既定値に戻す。
/// モデルは変更しないため、失敗時に部分更新は起きない。
///
/// # Errors
///
/// ノード索引や親子が範囲外・循環・ワールド行列の特異など、
/// 構造として成立しない場合に返す。
pub fn resolve_look_at(
    sidecar: &PhysicsSidecar,
    model: &VrmModel,
    resolution: &PhysicsResolution,
    humanoid: &HumanoidBones,
) -> Result<Option<(VrmLookAt, LookAtReport)>, VrmError> {
    let Some(look) = sidecar.look_at.as_ref() else {
        return Ok(None);
    };
    let (head, left, right) = (
        humanoid.get(HumanoidBone::Head),
        humanoid.get(HumanoidBone::LeftEye),
        humanoid.get(HumanoidBone::RightEye),
    );
    let (Some(head), Some(left_eye), Some(right_eye)) = (head, left, right) else {
        let report = LookAtReport {
            look_type: "expression",
            measured: false,
            range: VrmLookAtRange {
                horizontal_inner: EXPRESSION_OUTPUT,
                horizontal_outer: EXPRESSION_OUTPUT,
                vertical_down: EXPRESSION_OUTPUT,
                vertical_up: EXPRESSION_OUTPUT,
            },
            offset_from_head: EXPRESSION_OFFSET_FROM_HEAD,
        };
        let look_at = VrmLookAt::Expression {
            offset_from_head: EXPRESSION_OFFSET_FROM_HEAD,
        };
        return Ok(Some((look_at, report)));
    };
    for node in [head, left_eye, right_eye] {
        if node >= model.nodes.len() {
            return Err(VrmError::new(
                VrmErrorKind::InvalidStructure,
                "視線のhumanoid骨のノード索引が範囲外である",
            ));
        }
    }
    let worlds = vrm_world_matrices(model)?;
    let offset = head_offset(&worlds, head, left_eye, right_eye)?;
    // 測定の可否は参照実装の順序（Eye Look・解決・一致・姿勢の有効）に従う。
    // いずれかで測定できない場合は既定の範囲に戻す。
    let measured = if look.eye_look {
        measure_range(look, model, &worlds, resolution, left_eye, right_eye)
    } else {
        None
    };
    let (range, measured) = match measured {
        Some(range) => (range, true),
        None => (
            VrmLookAtRange {
                horizontal_inner: DEFAULT_OUTPUT_DEGREES,
                horizontal_outer: DEFAULT_OUTPUT_DEGREES,
                vertical_down: DEFAULT_OUTPUT_DEGREES,
                vertical_up: DEFAULT_OUTPUT_DEGREES,
            },
            false,
        ),
    };
    let report = LookAtReport {
        look_type: "bone",
        measured,
        range,
        offset_from_head: offset,
    };
    let look_at = VrmLookAt::Bone {
        left_eye,
        right_eye,
        offset_from_head: offset,
        range,
        measured,
    };
    Ok(Some((look_at, report)))
}

/// 測定を試みる。測定できない条件では `None` を返す。
///
/// Eye Lookが有効なことは呼び出し側で確認済みである。sidecarの目の解決と
/// humanoidとの一致、5姿勢の有効を確認し、すべて通った場合だけ4方向の平均を
/// 返す。方向ごとの無効値（逆向き・上限超え）はその方向だけ既定値に戻す。
fn measure_range(
    look: &SidecarLookAt,
    model: &VrmModel,
    worlds: &[[f64; 16]],
    resolution: &PhysicsResolution,
    left_eye: usize,
    right_eye: usize,
) -> Option<VrmLookAtRange> {
    let left_id = resolution
        .node_map
        .iter()
        .find_map(|(id, &node)| if node == left_eye { Some(id) } else { None });
    let right_id =
        resolution.node_map.iter().find_map(
            |(id, &node)| {
                if node == right_eye { Some(id) } else { None }
            },
        );
    // descriptorの目がhumanoidの目と一致する場合だけ測定値を使う。
    // 未設定・未解決・不一致は測定しない。
    if left_id.is_none_or(|id| *id != look.left_eye_node_id)
        || right_id.is_none_or(|id| *id != look.right_eye_node_id)
    {
        return None;
    }
    let left_parent = unity_parent_quat(model, worlds, left_eye).ok()?;
    let right_parent = unity_parent_quat(model, worlds, right_eye).ok()?;
    let left_poses = eye_poses(look, true)?;
    let right_poses = eye_poses(look, false)?;
    let left = measure_eye(true, left_parent, &left_poses);
    let right = measure_eye(false, right_parent, &right_poses);
    Some(combine_ranges(left, right))
}

/// 5姿勢をUnity空間の単位四元数で取り出す。1つでも無効なら `None` である。
fn eye_poses(look: &SidecarLookAt, left_eye: bool) -> Option<EyePoses> {
    Some(EyePoses {
        straight: try_pose(&look.eyes_looking_straight, left_eye)?,
        up: try_pose(&look.eyes_looking_up, left_eye)?,
        down: try_pose(&look.eyes_looking_down, left_eye)?,
        left: try_pose(&look.eyes_looking_left, left_eye)?,
        right: try_pose(&look.eyes_looking_right, left_eye)?,
    })
}

/// 1姿勢を取り出す。`linked` の場合は左の値を両目に使う。
fn try_pose(pose: &SidecarEyePose, left_eye: bool) -> Option<[f64; 4]> {
    let values = if left_eye || pose.linked {
        pose.left
    } else {
        pose.right
    };
    normalize_quat(values)
}

/// 目の親のワールド回転をUnity空間で求める。親が無い場合は恒等回転である。
fn unity_parent_quat(
    model: &VrmModel,
    worlds: &[[f64; 16]],
    eye: usize,
) -> Result<[f64; 4], VrmError> {
    let Some(parent) = model.nodes.get(eye).and_then(|node| node.parent) else {
        return Ok([0.0, 0.0, 0.0, 1.0]);
    };
    let world = worlds.get(parent).ok_or_else(|| {
        VrmError::new(
            VrmErrorKind::InvalidStructure,
            "視線の目の親ノード索引が範囲外である",
        )
    })?;
    let rotation = decompose_rotation(world).ok_or_else(|| {
        VrmError::new(
            VrmErrorKind::InvalidStructure,
            "視線の目の親のワールド行列から回転を取り出せない",
        )
    })?;
    // VRM空間からUnity空間へ戻す。鏡像は群同型のため式はそのまま使える。
    Ok(mirror_quat(rotation))
}

/// 頭基準のずれをVRM空間のまま求める。
///
/// `inverse(head世界) * 眼の中点` の同次変換であり、鏡像済みの座標をそのまま書く。
fn head_offset(
    worlds: &[[f64; 16]],
    head: usize,
    left_eye: usize,
    right_eye: usize,
) -> Result<[f32; 3], VrmError> {
    let midpoint = [
        (worlds[left_eye][12] + worlds[right_eye][12]) / 2.0,
        (worlds[left_eye][13] + worlds[right_eye][13]) / 2.0,
        (worlds[left_eye][14] + worlds[right_eye][14]) / 2.0,
    ];
    if !midpoint.iter().all(|v| v.is_finite()) {
        return Err(VrmError::new(
            VrmErrorKind::InvalidStructure,
            "視線の眼の中点が有限値でない",
        ));
    }
    let head_world = worlds.get(head).ok_or_else(|| {
        VrmError::new(
            VrmErrorKind::InvalidStructure,
            "視線の頭ノード索引が範囲外である",
        )
    })?;
    let inverse = matrix::inverse(head_world).ok_or_else(|| {
        VrmError::new(
            VrmErrorKind::InvalidStructure,
            "視線の頭のワールド行列が特異である",
        )
    })?;
    let offset = matrix::transform_point(&inverse, midpoint).ok_or_else(|| {
        VrmError::new(
            VrmErrorKind::InvalidStructure,
            "視線の頭基準ずれを変換できない",
        )
    })?;
    if !offset.iter().all(|v| v.is_finite()) {
        return Err(VrmError::new(
            VrmErrorKind::InvalidStructure,
            "視線の頭基準ずれが有限値でない",
        ));
    }
    Ok([offset[0] as f32, offset[1] as f32, offset[2] as f32])
}

/// VRM空間のワールド行列列。親なしノードには `root` を掛ける。
fn vrm_world_matrices(model: &VrmModel) -> Result<Vec<[f64; 16]>, VrmError> {
    let mut worlds = vec![matrix::IDENTITY; model.nodes.len()];
    let mut done = vec![false; model.nodes.len()];
    for start in 0..model.nodes.len() {
        let mut chain = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut current = start;
        loop {
            if done[current] {
                break;
            }
            if !seen.insert(current) {
                return Err(VrmError::new(
                    VrmErrorKind::InvalidStructure,
                    "視線の解決でノードの親子に循環がある",
                ));
            }
            chain.push(current);
            match model.nodes[current].parent {
                Some(parent) if parent < model.nodes.len() => current = parent,
                Some(_) => {
                    return Err(VrmError::new(
                        VrmErrorKind::InvalidStructure,
                        "視線の解決で親ノードの索引が範囲外である",
                    ));
                }
                None => break,
            }
        }
        for &node in chain.iter().rev() {
            let local = &model.nodes[node].local;
            worlds[node] = match model.nodes[node].parent {
                Some(parent) => matrix::mul(&worlds[parent], local),
                None => matrix::mul(&model.root, local),
            };
        }
        for &node in &chain {
            done[node] = true;
        }
    }
    Ok(worlds)
}

/// ワールド行列から回転の単位四元数を取り出す。
///
/// 列ベクトルを正規化して回転行列に戻す。TRS合成が前提であり、ゼロ長の列や
/// 反射・非有限の場合は `None` である。
fn decompose_rotation(world: &[f64; 16]) -> Option<[f64; 4]> {
    let mut mat = [[0.0; 3]; 3];
    for column in 0..3 {
        let length = (world[column * 4] * world[column * 4]
            + world[column * 4 + 1] * world[column * 4 + 1]
            + world[column * 4 + 2] * world[column * 4 + 2])
            .sqrt();
        if !length.is_finite() || length <= 0.0 {
            return None;
        }
        for row in 0..3 {
            mat[row][column] = world[column * 4 + row] / length;
        }
    }
    // 反射を含む行列は回転でないため受け付けない。
    let det = mat[0][0] * (mat[1][1] * mat[2][2] - mat[1][2] * mat[2][1])
        - mat[0][1] * (mat[1][0] * mat[2][2] - mat[1][2] * mat[2][0])
        + mat[0][2] * (mat[1][0] * mat[2][1] - mat[1][1] * mat[2][0]);
    if !det.is_finite() || det <= 0.0 {
        return None;
    }
    let quat = quat_from_mat3(mat);
    if quat.iter().all(|v| v.is_finite()) {
        Some(quat)
    } else {
        None
    }
}

/// 3x3回転行列（行優先）から単位四元数 `[x, y, z, w]` を求める。
fn quat_from_mat3(mat: [[f64; 3]; 3]) -> [f64; 4] {
    let trace = mat[0][0] + mat[1][1] + mat[2][2];
    if trace > 0.0 {
        let s = (trace + 1.0).sqrt() * 2.0;
        [
            (mat[2][1] - mat[1][2]) / s,
            (mat[0][2] - mat[2][0]) / s,
            (mat[1][0] - mat[0][1]) / s,
            0.25 * s,
        ]
    } else if mat[0][0] > mat[1][1] && mat[0][0] > mat[2][2] {
        let s = (1.0 + mat[0][0] - mat[1][1] - mat[2][2]).sqrt() * 2.0;
        [
            0.25 * s,
            (mat[0][1] + mat[1][0]) / s,
            (mat[0][2] + mat[2][0]) / s,
            (mat[2][1] - mat[1][2]) / s,
        ]
    } else if mat[1][1] > mat[2][2] {
        let s = (1.0 + mat[1][1] - mat[0][0] - mat[2][2]).sqrt() * 2.0;
        [
            (mat[0][1] + mat[1][0]) / s,
            0.25 * s,
            (mat[1][2] + mat[2][1]) / s,
            (mat[0][2] - mat[2][0]) / s,
        ]
    } else {
        let s = (1.0 + mat[2][2] - mat[0][0] - mat[1][1]).sqrt() * 2.0;
        [
            (mat[0][2] + mat[2][0]) / s,
            (mat[1][2] + mat[2][1]) / s,
            0.25 * s,
            (mat[1][0] - mat[0][1]) / s,
        ]
    }
}

/// 四元数の積。適用順は右から左（`b` を先に適用）である。
fn quat_mul(a: [f64; 4], b: [f64; 4]) -> [f64; 4] {
    [
        a[3] * b[0] + a[0] * b[3] + a[1] * b[2] - a[2] * b[1],
        a[3] * b[1] - a[0] * b[2] + a[1] * b[3] + a[2] * b[0],
        a[3] * b[2] + a[0] * b[1] - a[1] * b[0] + a[2] * b[3],
        a[3] * b[3] - a[0] * b[0] - a[1] * b[1] - a[2] * b[2],
    ]
}

/// 単位四元数の逆元。共役である。
fn quat_inverse(q: [f64; 4]) -> [f64; 4] {
    [-q[0], -q[1], -q[2], q[3]]
}

/// 有限かつ非ゼロ長の四元数を正規化する。無効な場合は `None` である。
///
/// 参照実装の `TryRotation` と同じ規則（有限・二乗和 `1e-8` 以上）である。
fn normalize_quat(q: [f64; 4]) -> Option<[f64; 4]> {
    if !q.iter().all(|v| v.is_finite()) {
        return None;
    }
    let squared = q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3];
    if squared < 1e-8 {
        return None;
    }
    let length = squared.sqrt();
    if !length.is_finite() {
        return None;
    }
    Some([q[0] / length, q[1] / length, q[2] / length, q[3] / length])
}

/// 四元数でベクトルを回転する。
fn rotate_vector(q: [f64; 4], v: [f64; 3]) -> [f64; 3] {
    let t = quat_mul(q, [v[0], v[1], v[2], 0.0]);
    let r = quat_mul(t, quat_inverse(q));
    [r[0], r[1], r[2]]
}

/// Unity空間の5姿勢。いずれも正規化済みのローカル回転である。
struct EyePoses {
    straight: [f64; 4],
    up: [f64; 4],
    down: [f64; 4],
    left: [f64; 4],
    right: [f64; 4],
}

/// 視線のヨー（+X向きが正・度）とピッチ（上が正・度）を求める。
///
/// 参照実装の `EyeLookRange.Gaze` と同じ式であり、姿勢の差分を眼の親の枠で
/// 前方（+Z）に適用する。
fn gaze(parent: [f64; 4], straight: [f64; 4], pose: [f64; 4]) -> [f64; 2] {
    let delta = quat_mul(
        parent,
        quat_mul(pose, quat_inverse(quat_mul(parent, straight))),
    );
    let direction = rotate_vector(delta, [0.0, 0.0, 1.0]);
    let yaw = direction[0].atan2(direction[2]).to_degrees();
    let pitch = direction[1].clamp(-1.0, 1.0).asin().to_degrees();
    [yaw, pitch]
}

/// 片目の4方向 `[上, 下, 内, 外]`（度）を測定する。
///
/// 上下はその方向の姿勢から、内外は左右の姿勢のうち実際にその目を振った向きの
/// 大きい方から取る。参照実装の `EyeLookRange.Measure` と同じ式である。
fn measure_eye(left_eye: bool, parent: [f64; 4], poses: &EyePoses) -> [f64; 4] {
    let left_yaw = gaze(parent, poses.straight, poses.left)[0];
    let right_yaw = gaze(parent, poses.straight, poses.right)[0];
    let toward_minus_x = (-left_yaw).max(-right_yaw);
    let toward_plus_x = left_yaw.max(right_yaw);
    let up = gaze(parent, poses.straight, poses.up)[1];
    let down = -gaze(parent, poses.straight, poses.down)[1];
    let (inner, outer) = if left_eye {
        (toward_plus_x, toward_minus_x)
    } else {
        (toward_minus_x, toward_plus_x)
    };
    [up, down, inner, outer]
}

/// 左右の測定値を1方向ずつ束ねる。無効な方向は既定値に戻す.
///
/// 参照実装の `EyeRangeDirection` と同じ規則であり、平均が非数・逆向きの許容
/// （-0.5度）未満・上限（90度）超えの場合は既定の10度にし、それ以外は0度で止める。
fn combine_ranges(left: [f64; 4], right: [f64; 4]) -> VrmLookAtRange {
    let direction = |l: f64, r: f64| {
        let value = (l + r) / 2.0;
        if value.is_nan() || value < -WRONG_WAY_TOLERANCE_DEGREES || value > f64::from(MAX_DEGREES)
        {
            f64::from(DEFAULT_OUTPUT_DEGREES)
        } else {
            value.max(0.0)
        }
    };
    VrmLookAtRange {
        horizontal_inner: direction(left[2], right[2]) as f32,
        horizontal_outer: direction(left[3], right[3]) as f32,
        vertical_down: direction(left[1], right[1]) as f32,
        vertical_up: direction(left[0], right[0]) as f32,
    }
}

/// `VRMC_vrm.lookAt` のJSON値を作る。
///
/// 骨型は索引の範囲内・左右の相違・ずれと範囲の有限値と値域を検証し、
/// 表情型はずれの有限値を検証する。範囲の値は0以上上限以下の有限値であり、
/// 入力最大値は正である。詳細は設計文書の出力仕様に従う。
///
/// # Errors
///
/// 索引の範囲外・左右の一致・非有限値や値域外の場合に返す。
pub(crate) fn look_at_json(
    look: &VrmLookAt,
    node_count: usize,
) -> Result<serde_json::Value, VrmExportError> {
    match look {
        VrmLookAt::Bone {
            left_eye,
            right_eye,
            offset_from_head,
            range,
            measured,
        } => {
            if *left_eye >= node_count || *right_eye >= node_count {
                return Err(VrmExportError::new(
                    VrmExportErrorKind::InvalidStructure,
                    "視線の目のノード索引が範囲外である",
                ));
            }
            if left_eye == right_eye {
                return Err(VrmExportError::new(
                    VrmExportErrorKind::InvalidStructure,
                    "視線の左右の目が同じノードである",
                ));
            }
            check_offset(offset_from_head)?;
            let input_of = |output: f32| {
                if *measured {
                    output.max(MIN_MEASURED_INPUT_DEGREES)
                } else {
                    DEFAULT_INPUT_DEGREES
                }
            };
            let directions = [
                ("HorizontalInner", range.horizontal_inner),
                ("HorizontalOuter", range.horizontal_outer),
                ("VerticalDown", range.vertical_down),
                ("VerticalUp", range.vertical_up),
            ];
            let mut maps = Vec::with_capacity(directions.len());
            for (name, output) in directions {
                check_range(name, output)?;
                let input = input_of(output);
                if !input.is_finite() || input <= 0.0 {
                    return Err(VrmExportError::new(
                        VrmExportErrorKind::InvalidStructure,
                        format!("視線の{name}の入力最大値が正の有限値でない"),
                    ));
                }
                maps.push((name, input, output));
            }
            let map_of = |input: f32, output: f32| serde_json::json!({"inputMaxValue": input, "outputScale": output});
            Ok(serde_json::json!({
                "offsetFromHeadBone": [offset_from_head[0], offset_from_head[1], offset_from_head[2]],
                "rangeMapHorizontalInner": map_of(maps[0].1, maps[0].2),
                "rangeMapHorizontalOuter": map_of(maps[1].1, maps[1].2),
                "rangeMapVerticalDown": map_of(maps[2].1, maps[2].2),
                "rangeMapVerticalUp": map_of(maps[3].1, maps[3].2),
                "type": "bone",
            }))
        }
        VrmLookAt::Expression { offset_from_head } => {
            check_offset(offset_from_head)?;
            let map = serde_json::json!({"inputMaxValue": DEFAULT_INPUT_DEGREES, "outputScale": EXPRESSION_OUTPUT});
            Ok(serde_json::json!({
                "offsetFromHeadBone": [offset_from_head[0], offset_from_head[1], offset_from_head[2]],
                "rangeMapHorizontalInner": map,
                "rangeMapHorizontalOuter": map,
                "rangeMapVerticalDown": map,
                "rangeMapVerticalUp": map,
                "type": "expression",
            }))
        }
    }
}

/// 頭基準ずれが3つの有限値か確かめる。
fn check_offset(offset: &[f32; 3]) -> Result<(), VrmExportError> {
    if !offset.iter().all(|v| v.is_finite()) {
        return Err(VrmExportError::new(
            VrmExportErrorKind::InvalidStructure,
            "視線の頭基準ずれに非有限値がある",
        ));
    }
    Ok(())
}

/// 範囲の値が0以上上限以下の有限値か確かめる。
fn check_range(name: &str, value: f32) -> Result<(), VrmExportError> {
    if !value.is_finite() || !(0.0..=MAX_DEGREES).contains(&value) {
        return Err(VrmExportError::new(
            VrmExportErrorKind::InvalidStructure,
            format!("視線の{name}の範囲が0〜上限の有限値でない"),
        ));
    }
    Ok(())
}

/// sidecarの視線記録を検証する。[`PhysicsSidecar`](super::physics::PhysicsSidecar)
/// の検証から呼び出す。
///
/// 目の参照は空（未設定）か既存ノードであり、姿勢の成分は有限値である。
/// 配列長4は型で保証する。正規化は測定時に行うためここでは求めない。
///
/// # Errors
///
/// 目の参照の欠落・姿勢の非有限値の場合に返す。
pub(crate) fn validate_sidecar_look_at(
    look: &SidecarLookAt,
    node_at: &std::collections::HashMap<&str, usize>,
) -> Result<(), VrmError> {
    for id in [&look.left_eye_node_id, &look.right_eye_node_id] {
        if !id.is_empty() && !node_at.contains_key(id.as_str()) {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPhysicsSidecar,
                format!("視線の目の参照が無い: {id}"),
            ));
        }
    }
    let poses = [
        ("eyesLookingStraight", &look.eyes_looking_straight),
        ("eyesLookingUp", &look.eyes_looking_up),
        ("eyesLookingDown", &look.eyes_looking_down),
        ("eyesLookingLeft", &look.eyes_looking_left),
        ("eyesLookingRight", &look.eyes_looking_right),
    ];
    for (name, pose) in poses {
        for values in [&pose.left, &pose.right] {
            if !values.iter().all(|v| v.is_finite()) {
                return Err(VrmError::new(
                    VrmErrorKind::InvalidPhysicsSidecar,
                    format!("視線の{name}に非有限値がある"),
                ));
            }
        }
    }
    Ok(())
}
