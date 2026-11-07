//! 物理sidecarの解決からスプリング生成までを通す共通適用経路。
//!
//! CLIと将来のWebが同じ順序・同じ集計で適用するための薄い束ねである。
//! 解決・T-pose・視線・スプリング生成の順に進め、成功時にモデルを更新して
//! 構造化報告を返す。標準出力・標準エラーへの書き込みは行わない。
//! 診断文面が必要な場合は報告のメソッドが文字列を作り、
//! 呼び出し側が表示の順序と可視エスケープを受け持つ。
//! 失敗時はモデルを部分更新せず、呼び出し側が診断の見出しを付けて返す。

use serde::Serialize;

use super::VrmModel;
use super::error::VrmError;
use super::humanoid::HumanoidBones;
use super::look_at::{LookAtReport, resolve_look_at};
use super::physics::{
    CenterKind, PhysicsSidecar, SpringCompileOptions, apply_humanoid_t_pose, compile_springs,
};

/// T-pose適用で飛ばした骨1件の記録。
#[derive(Debug, Clone, Serialize)]
pub struct TposeSkippedEntry {
    /// 骨名。Unityの `HumanBodyBones` 名である。
    pub bone: String,
    /// 対応するsidecarノードのID。
    pub node_id: String,
    /// 飛ばした理由の表示（`T-poseが無い` など）。
    pub reason: String,
}

/// 視線の4方向と頭基準ずれの記録。
#[derive(Debug, Clone, Serialize)]
pub struct LookAtRangeSummary {
    /// 内向きの最大角（度）。
    pub horizontal_inner: f32,
    /// 外向きの最大角（度）。
    pub horizontal_outer: f32,
    /// 下向きの最大角（度）。
    pub vertical_down: f32,
    /// 上向きの最大角（度）。
    pub vertical_up: f32,
}

/// 視線の報告。種別・測定の有無・範囲・頭基準ずれを持つ。
#[derive(Debug, Clone, Serialize)]
pub struct LookAtAppliedSummary {
    /// 視線の種別。`"bone"` または `"expression"` である。
    pub look_type: String,
    /// 測定値を使ったかどうか。
    pub measured: bool,
    /// 4方向の出力角（度）。
    pub range: LookAtRangeSummary,
    /// 頭基準の注視原点。
    pub offset_from_head: [f32; 3],
}

/// 所有権を奪われたノード1件の記録。
#[derive(Debug, Clone, Serialize)]
pub struct LostNodeSummary {
    /// 奪われた側のPhysBone ID。
    pub bone: String,
    /// 奪われたsidecarノードID。
    pub node: String,
    /// 所有するPhysBoneのID。
    pub owner: String,
}

/// springにならなかったPhysBone1件の記録。
#[derive(Debug, Clone, Serialize)]
pub struct SkippedBoneSummary {
    /// PhysBoneのID。
    pub bone: String,
    /// 理由のデバッグ表示。閉じた集合の文字起こしである。
    pub reason: String,
}

/// 物理sidecar適用の構造化報告。CLIの要約と診断行のもとになる。
#[derive(Debug, Clone, Serialize)]
pub struct PhysicsApplyReport {
    /// 追加した補助ノードの名前列。sidecar順である。
    pub auxiliary_node_names: Vec<String>,
    /// T-poseで上書きした骨数。
    pub tpose_applied: usize,
    /// T-poseで飛ばした骨列。sidecarの骨順である。
    pub tpose_skipped: Vec<TposeSkippedEntry>,
    /// 視線記録の解決結果。sidecarに視線が無い旧形式では `None` である。
    pub look_at: Option<LookAtAppliedSummary>,
    /// 出力したspring数。
    pub spring_count: usize,
    /// 出力したjoint総数。
    pub joint_count: usize,
    /// 出力したcollider数。
    pub collider_count: usize,
    /// 出力したcolliderグループ数。
    pub group_count: usize,
    /// 分岐の分割件数。
    pub branch_split_count: usize,
    /// centerを付けたspring件数。
    pub center_total: usize,
    /// World由来のcenter件数。
    pub center_world: usize,
    /// AllMotion由来のcenter件数。
    pub center_all_motion: usize,
    /// centerを付けなかったPhysBone ID列。
    pub center_missing: Vec<String>,
    /// endpointを生成しなかったPhysBone ID列。今回は全件である。
    pub no_endpoint: Vec<String>,
    /// `gravity_power` が0の関節数。
    pub gravity_zero_joints: usize,
    /// 最初の子に畳んだPhysBone ID列。
    pub first_child_multichild: Vec<String>,
    /// 平均近似したPhysBone ID列。
    pub average_approximated: Vec<String>,
    /// 平面のため落とした参照を持つPhysBone ID列。現在は常に空である。
    pub plane_colliders_dropped: Vec<String>,
    /// 先行Planに所有権を奪われたノード列。
    pub lost_nodes: Vec<LostNodeSummary>,
    /// springにならなかったPhysBone列。
    pub skipped_bones: Vec<SkippedBoneSummary>,
    /// 出力したノード制約数。
    pub constraint_count: usize,
    /// 出力しなかった制約のID列。
    pub skipped_constraints: Vec<String>,
    /// jointの硬さ平均。
    pub stiffness_mean: f64,
    /// 重力ありの関節数。
    pub gravity_joints: usize,
}

impl PhysicsApplyReport {
    /// 補助ノードの件数を返す。
    pub fn auxiliary_count(&self) -> usize {
        self.auxiliary_node_names.len()
    }

    /// T-poseのスキップ件数を返す。
    pub fn tpose_skipped_count(&self) -> usize {
        self.tpose_skipped.len()
    }

    /// CLIの補助ノード行と同じ文面を返す。
    pub fn auxiliary_message(&self) -> String {
        if self.auxiliary_node_names.is_empty() {
            "物理: 補助ノードの追加は無い".to_string()
        } else {
            format!(
                "物理: 補助ノード{}件を追加（{}）",
                self.auxiliary_node_names.len(),
                self.auxiliary_node_names.join(", "),
            )
        }
    }

    /// CLIのT-ポーズ件数行と同じ文面を返す。無い場合は `None` である。
    pub fn tpose_count_message(&self) -> Option<String> {
        if self.tpose_skipped.is_empty() {
            None
        } else {
            Some(format!(
                "T-ポーズのスキップ: {}件",
                self.tpose_skipped.len()
            ))
        }
    }

    /// CLIの視線行と同じ文面を返す。視線が無い場合は `None` である。
    pub fn look_at_message(&self) -> Option<String> {
        let look = self.look_at.as_ref()?;
        Some(format!(
            "視線: lookAt={} {} 内{:.3}度 外{:.3}度 下{:.3}度 上{:.3}度 頭基準ずれ[{:.6}, {:.6}, {:.6}]",
            look.look_type,
            if look.measured {
                "測定値"
            } else {
                "既定値"
            },
            look.range.horizontal_inner,
            look.range.horizontal_outer,
            look.range.vertical_down,
            look.range.vertical_up,
            look.offset_from_head[0],
            look.offset_from_head[1],
            look.offset_from_head[2],
        ))
    }

    /// CLIの物理要約行と同じ文面を返す。
    ///
    /// 内容・順序・書式は従来の `apply_physics_sidecar` の `summary` と完全一致する。
    /// 数値の丸め（硬さ平均の小数第3位など）も変えない。
    pub fn summary(&self) -> String {
        format!(
            "スプリング{}件 関節{}件 collider{}件 グループ{}件 分岐{}件 center{}件（world{} / all-motion{} / なし{}） 近似(endpoint未生成{} gravity0関節{} first-child{} 平均近似{} 平面落下{}) 関節値(硬さ平均{:.3} 重力あり{}件) 共有{}件 スキップ{}件 制約{}件 制約スキップ{}件 T-ポーズ{}件",
            self.spring_count,
            self.joint_count,
            self.collider_count,
            self.group_count,
            self.branch_split_count,
            self.center_total,
            self.center_world,
            self.center_all_motion,
            self.center_missing.len(),
            self.no_endpoint.len(),
            self.gravity_zero_joints,
            self.first_child_multichild.len(),
            self.average_approximated.len(),
            self.plane_colliders_dropped.len(),
            self.stiffness_mean,
            self.gravity_joints,
            self.lost_nodes.len(),
            self.skipped_bones.len(),
            self.constraint_count,
            self.skipped_constraints.len(),
            self.tpose_applied,
        )
    }
}

/// 物理適用の失敗。段階の見出しと原因の `VrmError`、途中までの診断を持つ。
///
/// CLIの従来の4段階の見出し（解決・T-pose・視線・スプリング生成）を保つための形であり、
/// Webは [`PhysicsApplyError::into_inner`] や `From` で `VrmError` に戻して扱える。
/// 失敗時はモデルを部分更新しないが、失敗前に確定した補助ノード名と
/// T-poseスキップは呼び出し側の表示順を保つために残す。
#[derive(Debug)]
pub struct PhysicsApplyError {
    /// 失敗した段階の見出し。CLIの `diagnose_stage` の第1引数と同じ文面である。
    stage: &'static str,
    /// 失敗の原因。従来の各段階が返した `VrmError` そのものである。
    error: VrmError,
    /// 失敗前に確定した補助ノード名列。解決前は空である。
    auxiliary_node_names: Vec<String>,
    /// 失敗前に確定したT-poseスキップ列。T-pose成功前は空である。
    tpose_skipped: Vec<TposeSkippedEntry>,
}

impl PhysicsApplyError {
    /// 失敗した段階の見出しを返す。
    pub fn stage(&self) -> &'static str {
        self.stage
    }

    /// 失敗の原因を返す。
    pub fn error(&self) -> &VrmError {
        &self.error
    }

    /// 原因の `VrmError` を取り出す。
    pub fn into_inner(self) -> VrmError {
        self.error
    }

    /// 失敗前に確定した補助ノード名列を返す。
    pub fn auxiliary_node_names(&self) -> &[String] {
        &self.auxiliary_node_names
    }

    /// 失敗前に確定したT-poseスキップ列を返す。
    pub fn tpose_skipped(&self) -> &[TposeSkippedEntry] {
        &self.tpose_skipped
    }

    /// 補助ノード行の文面を作る。解決前は `None` である。
    pub fn auxiliary_message(&self) -> Option<String> {
        // 解決の失敗時は補助ノードの確定が無いため何も出さない。
        // 従来も解決失敗時は補助行を出さず診断だけを出していた。
        if self.stage == RESOLVE_STAGE {
            return None;
        }
        Some(if self.auxiliary_node_names.is_empty() {
            "物理: 補助ノードの追加は無い".to_string()
        } else {
            format!(
                "物理: 補助ノード{}件を追加（{}）",
                self.auxiliary_node_names.len(),
                self.auxiliary_node_names.join(", "),
            )
        })
    }
}

impl std::fmt::Display for PhysicsApplyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.stage, self.error)
    }
}

impl std::error::Error for PhysicsApplyError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.error)
    }
}

impl From<PhysicsApplyError> for VrmError {
    fn from(error: PhysicsApplyError) -> Self {
        error.error
    }
}

/// 解決段階の見出し。CLIの従来文面と同じである。
const RESOLVE_STAGE: &str = "物理sidecarの解決に失敗した";
/// T-pose段階の見出し。CLIの従来文面と同じである。
const TPOSE_STAGE: &str = "T-ポーズの適用に失敗した";
/// 視線段階の見出し。CLIの従来文面と同じである。
const LOOK_AT_STAGE: &str = "視線の解決に失敗した";
/// スプリング段階の見出し。CLIの従来文面と同じである。
const SPRING_STAGE: &str = "スプリングの生成に失敗した";

/// 解決済みの物理sidecarを中間モデルへ適用し、構造化報告を返す。
///
/// 適用順序は解決・T-pose・視線・スプリング生成であり、CLIと将来のWebで
/// 同じ順序を使う。成功時はモデルの補助ノード・T-pose・スプリング・視線を
/// 更新して報告を返す。標準出力・標準エラーへは書かない。
/// 診断の表示は呼び出し側が報告のメソッドとフィールドから行う。
///
/// 失敗時はモデルを部分更新せず、段階の見出しと原因・途中までの診断を
/// [`PhysicsApplyError`] で返す。呼び出し側は見出しと原因で `diagnose_stage`
/// 相当の表示を行い、Webは `into_inner` で `VrmError` に戻せる。
/// springと視線の代入は成功時のみ行うため、失敗時に残らない。
///
/// # Errors
///
/// 対応解決・T-pose適用・視線解決・スプリング生成のいずれかが失敗した場合に返す。
/// この場合モデルのノード列と制約列は呼び出し前の状態に戻す。
pub fn apply_physics_sidecar(
    model: &mut VrmModel,
    sidecar: &PhysicsSidecar,
    fbx_bytes: &[u8],
    humanoid: &HumanoidBones,
    options: &SpringCompileOptions,
) -> Result<PhysicsApplyReport, PhysicsApplyError> {
    // 失敗時の部分更新を戻すため、変更し得る列だけを退避する。
    // メッシュ・スキン・表情は触らないため複製しない。
    let backup_nodes = model.nodes.clone();
    let backup_constraints = model.node_constraints.clone();
    let result = apply_inner(model, sidecar, fbx_bytes, humanoid, options);
    if result.is_err() {
        model.nodes = backup_nodes;
        model.node_constraints = backup_constraints;
    }
    result
}

/// 適用の本体。呼び出し側の退避と復元に包まれて使う。
fn apply_inner(
    model: &mut VrmModel,
    sidecar: &PhysicsSidecar,
    fbx_bytes: &[u8],
    humanoid: &HumanoidBones,
    options: &SpringCompileOptions,
) -> Result<PhysicsApplyReport, PhysicsApplyError> {
    let resolution = model
        .resolve_physics_sidecar(sidecar, fbx_bytes)
        .map_err(|error| PhysicsApplyError {
            stage: RESOLVE_STAGE,
            error,
            auxiliary_node_names: Vec::new(),
            tpose_skipped: Vec::new(),
        })?;
    let auxiliary_node_names: Vec<String> = resolution
        .auxiliary_nodes
        .iter()
        .map(|aux| aux.name.clone())
        .collect();
    // humanoidのrest姿勢はsidecarのT-poseを優先する。無い骨は
    // ノードの姿勢のまま残す。視線の測定とスプリングの生成より前に適用する。
    // 書き出しに使う解決済み対応を渡し、不一致は適用前に失敗させる。
    let pose = apply_humanoid_t_pose(sidecar, model, &resolution, humanoid).map_err(|error| {
        PhysicsApplyError {
            stage: TPOSE_STAGE,
            error,
            auxiliary_node_names: auxiliary_node_names.clone(),
            tpose_skipped: Vec::new(),
        }
    })?;
    let tpose_skipped: Vec<TposeSkippedEntry> = pose
        .skipped
        .iter()
        .map(|skipped| TposeSkippedEntry {
            bone: skipped.bone.clone(),
            node_id: skipped.node_id.clone(),
            reason: skipped.reason.to_string(),
        })
        .collect();
    // 視線の解決。sidecarに視線記録が無い旧形式では何もしない。
    // 測定も既定もここで確定する。モデルは変更しないため、
    // 失敗時に部分更新は起きない。humanoidは解決より前に検出したものを
    // 使い、補助ノードの影響を受けない。
    let look_at = match sidecar.look_at.as_ref() {
        Some(_) => resolve_look_at(sidecar, model, &resolution, humanoid).map_err(|error| {
            PhysicsApplyError {
                stage: LOOK_AT_STAGE,
                error,
                auxiliary_node_names: auxiliary_node_names.clone(),
                tpose_skipped: tpose_skipped.clone(),
            }
        })?,
        None => None,
    };
    let compiled = compile_springs(sidecar, model, &resolution, options).map_err(|error| {
        PhysicsApplyError {
            stage: SPRING_STAGE,
            error,
            auxiliary_node_names: auxiliary_node_names.clone(),
            tpose_skipped: tpose_skipped.clone(),
        }
    })?;
    // 主要な近似件数と関節値の目安も出し、文書の制限と突き合わせられるようにする。
    let world = compiled
        .report
        .center_applied
        .iter()
        .filter(|entry| entry.kind == CenterKind::World)
        .count();
    let all_motion = compiled
        .report
        .center_applied
        .iter()
        .filter(|entry| entry.kind == CenterKind::AllMotion)
        .count();
    let mut stiff_sum = 0.0f64;
    let mut grav_count = 0usize;
    let mut joint_total = 0usize;
    for spring in &compiled.bones.springs {
        for joint in &spring.joints {
            stiff_sum += f64::from(joint.stiffness);
            if joint.gravity_power > 0.0 {
                grav_count += 1;
            }
            joint_total += 1;
        }
    }
    let stiff_mean = if joint_total > 0 {
        stiff_sum / joint_total as f64
    } else {
        0.0
    };
    let report = PhysicsApplyReport {
        auxiliary_node_names,
        tpose_applied: pose.applied.len(),
        tpose_skipped,
        look_at: look_at.as_ref().map(|(_, report)| to_look_summary(report)),
        spring_count: compiled.report.spring_count,
        joint_count: compiled.report.joint_count,
        collider_count: compiled.report.collider_count,
        group_count: compiled.report.group_count,
        branch_split_count: compiled.report.branch_splits.len(),
        center_total: compiled.report.center_applied.len(),
        center_world: world,
        center_all_motion: all_motion,
        center_missing: compiled.report.center_missing.clone(),
        no_endpoint: compiled.report.no_endpoint.clone(),
        gravity_zero_joints: compiled.report.gravity_zero_joints,
        first_child_multichild: compiled.report.first_child_multichild.clone(),
        average_approximated: compiled.report.average_approximated.clone(),
        plane_colliders_dropped: compiled.report.plane_colliders_dropped.clone(),
        lost_nodes: compiled
            .report
            .lost_nodes
            .iter()
            .map(|entry| LostNodeSummary {
                bone: entry.bone.clone(),
                node: entry.node.clone(),
                owner: entry.owner.clone(),
            })
            .collect(),
        skipped_bones: compiled
            .report
            .skipped_bones
            .iter()
            .map(|entry| SkippedBoneSummary {
                bone: entry.bone.clone(),
                reason: format!("{:?}", entry.reason),
            })
            .collect(),
        constraint_count: model.node_constraints.len(),
        skipped_constraints: compiled.report.skipped_constraints.clone(),
        stiffness_mean: stiff_mean,
        gravity_joints: grav_count,
    };
    model.spring_bones = compiled.bones;
    // 視線は鎖の生成が通ってから代入する。失敗時はモデルを更新しない。
    if let Some((look, _)) = look_at {
        model.look_at = Some(look);
    }
    Ok(report)
}

/// 視線報告を構造化報告の形に写す。
fn to_look_summary(report: &LookAtReport) -> LookAtAppliedSummary {
    LookAtAppliedSummary {
        look_type: report.look_type.to_string(),
        measured: report.measured,
        range: LookAtRangeSummary {
            horizontal_inner: report.range.horizontal_inner,
            horizontal_outer: report.range.horizontal_outer,
            vertical_down: report.range.vertical_down,
            vertical_up: report.range.vertical_up,
        },
        offset_from_head: report.offset_from_head,
    }
}
