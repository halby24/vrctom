//! Prefabを入口にした変換の共有本体。
//!
//! Web（wasm core）とCLIの両方から使う純粋な経路であり、
//! ファイルシステムに触れない。索引と読み手だけで組み立てる。
//! 順序は既存CLIの `--prefab` 経路と同じである。
//!
//! ```mermaid
//! flowchart LR
//!     prefab[Prefab YAML] --> target[対象解決]
//!     meta[.fbx.meta] --> target
//!     target --> fbx[FBX解析]
//!     fbx --> desc[表情・基準姿勢]
//!     desc --> model[中間モデル]
//!     model --> humanoid[humanoid確定]
//!     humanoid --> physics[物理導出・適用]
//!     physics --> assign[材質割当]
//!     assign --> export[VRM書き出し]
//! ```

use crate::fbx::{FbxError, FbxFile, FbxOptions};
use crate::unity::{
    AssetIndex, AssetMeta, AssetSource, ExternalObjectEntry, ParseOptions, SerializedFile,
};
use crate::vrm::{
    GlbExportOptions, HumanoidBones, MaterialError, MaterialErrorKind, MaterialOptions,
    NestedPrefabInput, NestedPrefabLimits, PhysicsLimits, PhysicsSidecar, PrefabMeshView,
    PrefabMode, PrefabTargetState, SpringCompileOptions, VrmExportError, VrmExportOptions,
    VrmMaterials, VrmModel, VrmOptions, apply_nested_prefab_meshes, apply_physics_sidecar,
    apply_physics_sidecar_without_humanoid, apply_prefab_plan, derive_physics_sidecar_with,
    expressions_from_avatar_descriptor, load_renderer_materials_from, plan_prefab_assignment,
    prefab_base_materials, prefab_baseline_weights, prefab_hierarchy_paths,
    prepare_nested_prefab_meshes, resolve_prefab_pool_paths, resolve_prefab_target,
};

/// 1件のPrefab本文の既定上限（64MiB）。
///
/// 実体の数百KBに2桁の余裕を持たせた大きさであり、
/// [`crate::vrm::PrefabTargetLimits`] の既定値と同じである。
const DEFAULT_MAX_PREFAB_BYTES: u64 = 64 * 1024 * 1024;
/// FBX1件の既定上限（128MiB）。
///
/// Webの単一FBX予算と同じ大きさである。
const DEFAULT_MAX_FBX_BYTES: u64 = 128 * 1024 * 1024;
/// 追加・副FBX1件の既定上限（128MiB）。
///
/// 物理導出の副FBX予算と同じ大きさである。
const DEFAULT_MAX_EXTRA_FBX_BYTES: u64 = 128 * 1024 * 1024;
/// 物理導出の依存合計の既定上限（512MiB）。
const DEFAULT_MAX_TOTAL_BYTES: u64 = 512 * 1024 * 1024;
/// 出力バイト列の既定上限（512MiB）。
const DEFAULT_MAX_OUTPUT_BYTES: usize = 512 * 1024 * 1024;

/// Prefab変換の資源上限。
///
/// フィールドは非公開で、既定値は [`Default`] が設定する。
/// 上限の変更は `with_` で始まるメソッドで行う。
#[derive(Debug, Clone)]
pub struct PrefabConversionLimits {
    max_prefab_bytes: u64,
    max_fbx_bytes: u64,
    max_extra_fbx_bytes: u64,
    max_total_bytes: u64,
    max_output_bytes: usize,
}

impl PrefabConversionLimits {
    /// 既定の上限を持つ設定を生成する。
    pub fn new() -> Self {
        Self {
            max_prefab_bytes: DEFAULT_MAX_PREFAB_BYTES,
            max_fbx_bytes: DEFAULT_MAX_FBX_BYTES,
            max_extra_fbx_bytes: DEFAULT_MAX_EXTRA_FBX_BYTES,
            max_total_bytes: DEFAULT_MAX_TOTAL_BYTES,
            max_output_bytes: DEFAULT_MAX_OUTPUT_BYTES,
        }
    }

    /// 1件のPrefab本文の上限（バイト数）を変更する。
    pub fn with_max_prefab_bytes(mut self, value: u64) -> Self {
        self.max_prefab_bytes = value;
        self
    }

    /// FBXバイト列の上限（バイト数）を変更する。
    pub fn with_max_fbx_bytes(mut self, value: u64) -> Self {
        self.max_fbx_bytes = value;
        self
    }

    /// 追加・副FBX1件の上限（バイト数）を変更する。
    pub fn with_max_extra_fbx_bytes(mut self, value: u64) -> Self {
        self.max_extra_fbx_bytes = value;
        self
    }

    /// 物理導出の依存合計の上限（バイト数）を変更する。
    pub fn with_max_total_bytes(mut self, value: u64) -> Self {
        self.max_total_bytes = value;
        self
    }

    /// 出力バイト列の上限（バイト数）を変更する。
    pub fn with_max_output_bytes(mut self, value: usize) -> Self {
        self.max_output_bytes = value;
        self
    }
}

impl Default for PrefabConversionLimits {
    fn default() -> Self {
        Self::new()
    }
}

/// 表情の選択指定。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PrefabExpressions {
    /// 表情を作らない。
    #[default]
    None,
    /// Prefabの `VRCAvatarDescriptor` から既定生成する。
    Descriptor,
}

/// 物理の選択指定。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PrefabPhysics {
    /// 物理を載せない。
    #[default]
    None,
    /// 同じPrefabから物理を導出して適用する。
    Derive,
}

/// Prefab変換の選択肢。
#[derive(Debug, Clone)]
pub struct PrefabConversionOptions {
    /// 表情の選択指定。
    pub expressions: PrefabExpressions,
    /// 基準姿勢の焼き込みを行うかどうか。
    pub baseline: bool,
    /// 物理の選択指定。
    pub physics: PrefabPhysics,
    /// 材質読み取りの資源上限。
    pub material_options: MaterialOptions,
    /// 資源上限。
    pub limits: PrefabConversionLimits,
}

impl Default for PrefabConversionOptions {
    fn default() -> Self {
        Self {
            expressions: PrefabExpressions::None,
            baseline: false,
            physics: PrefabPhysics::None,
            material_options: MaterialOptions::new(),
            limits: PrefabConversionLimits::new(),
        }
    }
}

/// Prefab汎用 GLB 変換の選択肢。
///
/// 衣装やアクセサリなど Humanoid・表情を持たないモデルを書き出す際に使う。
#[derive(Debug, Clone)]
pub struct PrefabGlbConversionOptions {
    /// 基準姿勢の焼き込みを行うかどうか。
    pub baseline: bool,
    /// 物理の選択指定。
    pub physics: PrefabPhysics,
    /// 材質読み取りの資源上限。
    pub material_options: MaterialOptions,
    /// 資源上限。
    pub limits: PrefabConversionLimits,
}

impl Default for PrefabGlbConversionOptions {
    fn default() -> Self {
        Self {
            baseline: false,
            physics: PrefabPhysics::None,
            material_options: MaterialOptions::new(),
            limits: PrefabConversionLimits::new(),
        }
    }
}

/// 物理適用の構造化報告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrefabPhysicsReport {
    /// 物理を使ったかどうか。
    pub used: bool,
    /// スプリング数。
    pub springs: usize,
    /// 関節数。
    pub joints: usize,
    /// collider数。
    pub colliders: usize,
    /// グループ数。
    pub groups: usize,
    /// 適用したT-ポーズ数。
    pub tpose_applied: usize,
    /// 跳ばしたT-ポーズ数。
    pub tpose_skipped: usize,
    /// 視線種別。無い場合は `None` である。
    pub look_at: Option<String>,
}

/// Prefab変換の構造化報告。
#[derive(Debug, Clone)]
pub struct PrefabConversionReport {
    /// 解決したPrefabの資産パス（`Assets/...`）。
    pub prefab: String,
    /// 変換対象のFBXの資産パス（`Assets/...`）。
    pub fbx: String,
    /// 変換対象のFBXのGUID（小文字hex）。
    pub fbx_guid: String,
    /// ネストPrefabの資産パス列（昇順・重複なし）。
    pub nested_prefabs: Vec<String>,
    /// 解決に使った記述方式。
    pub assignment_mode: Option<PrefabMode>,
    /// 有効レンダラ数。
    pub active_meshes: usize,
    /// 除外したメッシュ数。
    pub excluded_meshes: usize,
    /// 除外したメッシュのModel名（中間モデル順）。
    pub excluded_names: Vec<String>,
    /// 材質プールの件数。
    pub material_pool: usize,
    /// 焼き込んだメッシュ数。
    pub baseline_meshes: usize,
    /// 焼き込んだ非ゼロチャンネル数。
    pub baseline_shapes: usize,
    /// プリセット式数。
    pub expressions: usize,
    /// 全メッシュ合計のモーフターゲット数。
    pub morph_targets: usize,
    /// 物理適用の報告。
    pub physics: PrefabPhysicsReport,
    /// 診断の警告文。本文だけを持ち、見出しは呼び出し側が付ける。
    /// 並びは対象解決・表情・中間モデル・humanoid・物理・材質の順である。
    pub warnings: Vec<String>,
    /// 出力ノード数（物理・割当の適用後）。
    pub nodes: usize,
    /// 出力メッシュ数（割当の適用後）。
    pub meshes: usize,
    /// 出力スキン数。
    pub skins: usize,
    /// 出力材質数。
    pub materials: usize,
    /// 出力画像数。
    pub images: usize,
    /// 出力バイト数。
    pub bytes: usize,
}

/// 失敗した段階。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrefabConversionStage {
    /// Prefab対象の解決。
    Target,
    /// Prefab本文の読み取り・解析。
    Prefab,
    /// 基準姿勢の解決。
    Baseline,
    /// 表情の解決。
    Expressions,
    /// FBX解析・中間モデルの生成。
    Model,
    /// 物理の導出・適用。
    Physics,
    /// 材質の読み取り。
    Material,
    /// Prefab割当の解決・適用。
    Assignment,
    /// VRMの書き出し。
    Export,
}

/// Prefab変換の失敗エラー。
///
/// 段階ごとにバリアントを分け、表示は各層の既存エラーの表示をそのまま保つ。
/// FBX解析の失敗は [`PrefabConversionStage::Model`] に含め、
/// 物理適用の `PhysicsApplyError` は段階を保ったまま内側の表示に委ねる。
#[derive(Debug)]
pub enum PrefabConversionError {
    /// Prefab対象の解決の失敗。
    Target(crate::vrm::VrmError),
    /// Prefab本文の読み取り・解析の失敗。
    Prefab(crate::vrm::VrmError),
    /// 基準姿勢の解決の失敗。
    Baseline(crate::vrm::VrmError),
    /// 表情の解決の失敗。
    Expressions(crate::vrm::VrmError),
    /// FBX解析の失敗。
    Fbx(FbxError),
    /// 中間モデル生成の失敗。
    Model(crate::vrm::VrmError),
    /// 物理導出・sidecar読み取りの失敗。
    Physics(crate::vrm::VrmError),
    /// 物理適用の失敗。
    PhysicsApply(crate::vrm::PhysicsApplyError),
    /// 材質の読み取りの失敗。
    Material(MaterialError),
    /// Prefab割当の解決・適用の失敗。
    Assignment(crate::vrm::VrmError),
    /// VRM書き出しの失敗。
    Export(VrmExportError),
}

impl PrefabConversionError {
    /// 失敗した段階を返す。
    pub fn stage(&self) -> PrefabConversionStage {
        match self {
            Self::Target(_) => PrefabConversionStage::Target,
            Self::Prefab(_) => PrefabConversionStage::Prefab,
            Self::Baseline(_) => PrefabConversionStage::Baseline,
            Self::Expressions(_) => PrefabConversionStage::Expressions,
            Self::Fbx(_) | Self::Model(_) => PrefabConversionStage::Model,
            Self::Physics(_) | Self::PhysicsApply(_) => PrefabConversionStage::Physics,
            Self::Material(_) => PrefabConversionStage::Material,
            Self::Assignment(_) => PrefabConversionStage::Assignment,
            Self::Export(_) => PrefabConversionStage::Export,
        }
    }
}

impl std::fmt::Display for PrefabConversionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 各層の既存表示を壊さないため、内側の表示にそのまま委ねる。
        match self {
            Self::Target(error) => write!(f, "{error}"),
            Self::Prefab(error) => write!(f, "{error}"),
            Self::Baseline(error) => write!(f, "{error}"),
            Self::Expressions(error) => write!(f, "{error}"),
            Self::Fbx(error) => write!(f, "{error}"),
            Self::Model(error) => write!(f, "{error}"),
            Self::Physics(error) => write!(f, "{error}"),
            Self::PhysicsApply(error) => write!(f, "{error}"),
            Self::Material(error) => write!(f, "{error}"),
            Self::Assignment(error) => write!(f, "{error}"),
            Self::Export(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for PrefabConversionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Target(error) => Some(error),
            Self::Prefab(error) => Some(error),
            Self::Baseline(error) => Some(error),
            Self::Expressions(error) => Some(error),
            Self::Fbx(error) => Some(error),
            Self::Model(error) => Some(error),
            Self::Physics(error) => Some(error),
            Self::PhysicsApply(error) => Some(error),
            Self::Material(error) => Some(error),
            Self::Assignment(error) => Some(error),
            Self::Export(error) => Some(error),
        }
    }
}

/// Prefab変換の前処理済みコンテキスト。
struct PreparedPrefabContext<'a, 'idx> {
    fbx_asset_path: String,
    fbx_guid: String,
    nested_prefabs: Vec<String>,
    fbx_meta: &'idx AssetMeta,
    remaps: Vec<ExternalObjectEntry>,
    prefab_file: SerializedFile,
    file: FbxFile<'a>,
    warnings: Vec<String>,
}

/// 割当・材質読み取り・ネスト統合の結果。
struct AppliedAssignments {
    assignment_mode: Option<PrefabMode>,
    active_meshes: usize,
    excluded_meshes: usize,
    excluded_names: Vec<String>,
    material_pool: usize,
    materials: VrmMaterials,
    material_count: usize,
    image_count: usize,
}

/// Prefabを入口にVRMバイト列と構造化報告を作る。
///
/// 順序は既存CLIの `--prefab` 経路と同じである。`fbx_bytes` は呼び出し側が
/// 上限内で読み込んだものを渡し、関数内で一度だけ解析する。
/// `export_options` のmeta・権限は呼び出し側の設定を保ち、humanoidと材質だけを
/// 関数内で設定する。`vrm_options` の上限などは受け継ぎ、descriptor表情と
/// baselineだけを関数内で設定する。
/// ファイルシステムに触れず、`source` と `index` だけで組み立てる。
///
/// # Errors
///
/// 対象解決・Prefab解析・表情解決・基準姿勢解決・FBX解析・中間モデル生成・
/// 物理導出や適用・材質読み取り・割当解決や適用・書き出しの失敗を
/// 段階ごとのバリアントで返す。割当の適用に失敗した場合は
/// 関数内のローカルモデルで完結し、呼び出し側の状態を変えない。
#[allow(clippy::too_many_arguments)]
pub fn convert_prefab(
    source: &dyn AssetSource,
    index: &AssetIndex,
    prefab_path: &str,
    fbx_bytes: &[u8],
    fbx_options: &FbxOptions,
    vrm_options: &VrmOptions,
    export_options: VrmExportOptions,
    options: &PrefabConversionOptions,
) -> Result<(Vec<u8>, PrefabConversionReport), PrefabConversionError> {
    let limits = &options.limits;
    let mut ctx =
        prepare_prefab_context(source, index, prefab_path, fbx_bytes, fbx_options, limits)?;
    // Humanoid対応表はPrefabのAnimator参照を優先し、無ければ対象FBX自身の.fbx.metaを使う。
    let humanoid_meta =
        crate::vrm::resolve_animator_humanoid_meta(&ctx.prefab_file, index, ctx.fbx_meta);
    let mappings = humanoid_meta.human_bone_mappings().map_err(|error| {
        PrefabConversionError::Prefab(crate::vrm::VrmError::new(
            crate::vrm::VrmErrorKind::InvalidPrefab,
            format!("humanoidの対応表を読めない: {error}"),
        ))
    })?;

    // 表情・基準姿勢を中間モデル生成前に解決して `VrmOptions` へ反映する。
    let mut owned_vrm_options = vrm_options.clone();
    if options.expressions == PrefabExpressions::Descriptor {
        // 目条件はdescriptorの左右Eyeではなく、humanoidとして左右Eyeが
        // 解決できることで判定する。検出自体に失敗した場合は偽にする。
        let names: Vec<&str> = ctx
            .file
            .models()
            .iter()
            .map(|model| model.name.as_str())
            .collect();
        let humanoid_eyes = match HumanoidBones::detect_with_mapping_names(&names, &mappings) {
            Ok((bones, _)) => {
                bones.get(crate::vrm::HumanoidBone::LeftEye).is_some()
                    && bones.get(crate::vrm::HumanoidBone::RightEye).is_some()
            }
            Err(_) => false,
        };
        let plan = expressions_from_avatar_descriptor(
            &ctx.prefab_file,
            prefab_path,
            &ctx.file,
            &ctx.fbx_guid,
            humanoid_eyes,
        )
        .map_err(PrefabConversionError::Expressions)?;
        ctx.warnings.extend(plan.warnings.iter().cloned());
        if !plan.profile.bindings.is_empty() {
            owned_vrm_options = owned_vrm_options.with_expressions(plan.profile);
            if let Some(mesh) = plan.mesh {
                owned_vrm_options = owned_vrm_options.with_expression_mesh(mesh);
            }
        }
    }
    if options.baseline {
        let weights =
            prefab_baseline_weights(&ctx.prefab_file, prefab_path, &ctx.file, &ctx.fbx_guid)
                .map_err(PrefabConversionError::Baseline)?;
        if !weights.meshes.is_empty() {
            owned_vrm_options = owned_vrm_options.with_baseline_weights(weights);
        }
    }

    // 中間モデルを生成する。
    let (mut model, report) =
        VrmModel::from_fbx(&ctx.file, &owned_vrm_options).map_err(PrefabConversionError::Model)?;
    ctx.warnings.extend(report.warnings.iter().cloned());

    // humanoidを確定する。物理の補助ノード追加前に確定する。
    let (humanoid, humanoid_warnings) = HumanoidBones::detect_with_mapping(&model, &mappings)
        .map_err(PrefabConversionError::Export)?;
    ctx.warnings.extend(humanoid_warnings.iter().cloned());

    // 物理を適用する。
    let physics_report = derive_and_apply_prefab_physics(
        source,
        index,
        prefab_path,
        fbx_bytes,
        &ctx.file,
        &ctx.prefab_file,
        &ctx.fbx_asset_path,
        options.physics,
        Some(&humanoid),
        limits,
        &mut model,
        &mut ctx.warnings,
    )?;

    // 材質割当・ネスト統合を適用する。
    let applied = apply_prefab_assignments_and_materials(
        source,
        index,
        prefab_path,
        &ctx.prefab_file,
        &ctx.file,
        &ctx.fbx_guid,
        &ctx.remaps,
        fbx_options,
        &owned_vrm_options,
        &options.material_options,
        limits,
        &mut model,
        &mut ctx.warnings,
    )?;

    // VRMバイト列を作る。呼び出し側のmeta・権限は保つ。
    let export_options = export_options
        .with_humanoid(humanoid)
        .with_materials(applied.materials)
        .with_max_output_bytes(limits.max_output_bytes);
    let vrm_bytes = model
        .to_vrm(&export_options)
        .map_err(PrefabConversionError::Export)?;
    let report = build_prefab_report(
        prefab_path,
        ctx,
        &model,
        report.baseline_meshes,
        report.baseline_shapes,
        physics_report,
        applied.assignment_mode,
        applied.active_meshes,
        applied.excluded_meshes,
        applied.excluded_names,
        applied.material_pool,
        applied.material_count,
        applied.image_count,
        vrm_bytes.len(),
    );
    Ok((vrm_bytes, report))
}

/// Prefabを入口に `VRMC_vrm` を含まない汎用 GLB バイト列と構造化報告を作る。
///
/// 衣装やアクセサリなど Humanoid 骨や表情を持たないモデルの変換に使う。
/// 非スキン FBX かつ `BindPose` が無いモデルも rest 姿勢で読み込み、
/// `options.physics` が [`PrefabPhysics::Derive`] の場合は警告を残して物理を飛ばす。
///
/// # Errors
///
/// 対象解決・Prefab解析・基準姿勢解決・FBX解析・中間モデル生成・
/// 物理導出や適用・材質読み取り・割当解決や適用・書き出しの失敗を
/// 段階ごとのバリアントで返す。
#[allow(clippy::too_many_arguments)]
pub fn convert_prefab_glb(
    source: &dyn AssetSource,
    index: &AssetIndex,
    prefab_path: &str,
    fbx_bytes: &[u8],
    fbx_options: &FbxOptions,
    vrm_options: &VrmOptions,
    export_options: GlbExportOptions,
    options: &PrefabGlbConversionOptions,
) -> Result<(Vec<u8>, PrefabConversionReport), PrefabConversionError> {
    let limits = &options.limits;
    let mut ctx =
        prepare_prefab_context(source, index, prefab_path, fbx_bytes, fbx_options, limits)?;
    let mut owned_vrm_options = vrm_options.clone().with_allow_rest_without_bind_pose(true);
    if options.baseline {
        let weights =
            prefab_baseline_weights(&ctx.prefab_file, prefab_path, &ctx.file, &ctx.fbx_guid)
                .map_err(PrefabConversionError::Baseline)?;
        if !weights.meshes.is_empty() {
            owned_vrm_options = owned_vrm_options.with_baseline_weights(weights);
        }
    }
    let (mut model, report) =
        VrmModel::from_fbx(&ctx.file, &owned_vrm_options).map_err(PrefabConversionError::Model)?;
    ctx.warnings.extend(report.warnings.iter().cloned());

    let physics_report = derive_and_apply_prefab_physics(
        source,
        index,
        prefab_path,
        fbx_bytes,
        &ctx.file,
        &ctx.prefab_file,
        &ctx.fbx_asset_path,
        options.physics,
        None,
        limits,
        &mut model,
        &mut ctx.warnings,
    )?;

    let applied = apply_prefab_assignments_and_materials(
        source,
        index,
        prefab_path,
        &ctx.prefab_file,
        &ctx.file,
        &ctx.fbx_guid,
        &ctx.remaps,
        fbx_options,
        &owned_vrm_options,
        &options.material_options,
        limits,
        &mut model,
        &mut ctx.warnings,
    )?;

    let export_options = export_options
        .with_materials(applied.materials)
        .with_max_output_bytes(limits.max_output_bytes);
    let glb_bytes = model
        .to_glb(&export_options)
        .map_err(PrefabConversionError::Export)?;
    let report = build_prefab_report(
        prefab_path,
        ctx,
        &model,
        report.baseline_meshes,
        report.baseline_shapes,
        physics_report,
        applied.assignment_mode,
        applied.active_meshes,
        applied.excluded_meshes,
        applied.excluded_names,
        applied.material_pool,
        applied.material_count,
        applied.image_count,
        glb_bytes.len(),
    );
    Ok((glb_bytes, report))
}

/// 対象解決・`.fbx.meta` 取得・Prefab 解析・FBX 解析・旧 fileID 読み替えを行う。
fn prepare_prefab_context<'a, 'idx>(
    source: &dyn AssetSource,
    index: &'idx AssetIndex,
    prefab_path: &str,
    fbx_bytes: &'a [u8],
    fbx_options: &FbxOptions,
    limits: &PrefabConversionLimits,
) -> Result<PreparedPrefabContext<'a, 'idx>, PrefabConversionError> {
    use crate::vrm::PrefabTargetLimits;
    use crate::vrm::VrmErrorKind;

    // 入力済みバイト列の大きさで上限を確かめる。上限内の場合だけ後段へ進む。
    if fbx_bytes.len() as u64 > limits.max_fbx_bytes {
        return Err(PrefabConversionError::Model(crate::vrm::VrmError::new(
            VrmErrorKind::LimitExceeded,
            format!(
                "FBXが上限を超えた: {}バイト（上限{}バイト）",
                fbx_bytes.len(),
                limits.max_fbx_bytes,
            ),
        )));
    }
    // 1. Prefabから直接FBXを解決する。`Resolved` 以外は失敗にする。
    let target_limits = PrefabTargetLimits::new()
        .with_max_prefab_bytes(limits.max_prefab_bytes)
        .with_max_total_bytes(limits.max_total_bytes);
    let resolution = resolve_prefab_target(source, index, prefab_path, &target_limits)
        .map_err(PrefabConversionError::Target)?;
    let (fbx_asset_path, _expected_guid, nested_prefabs) = match resolution.state {
        PrefabTargetState::Resolved => {
            let fbx = resolution.fbx.clone().ok_or_else(|| {
                PrefabConversionError::Target(crate::vrm::VrmError::new(
                    VrmErrorKind::InvalidPrefab,
                    format!("Prefabから直接FBXを解決できない: {prefab_path}"),
                ))
            })?;
            (fbx.path, fbx.guid, resolution.nested_prefabs.clone())
        }
        PrefabTargetState::NoModel => {
            return Err(PrefabConversionError::Target(crate::vrm::VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!("Prefabから直接FBXを解決できない: モデル参照が無い ({prefab_path})"),
            )));
        }
        PrefabTargetState::UnsupportedMultipleFbx => {
            return Err(PrefabConversionError::Target(crate::vrm::VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!("Prefabから直接FBXを解決できない: 直接FBXが複数ある ({prefab_path})"),
            )));
        }
        PrefabTargetState::RequiresPrefabResolution => {
            return Err(PrefabConversionError::Target(crate::vrm::VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!(
                    "Prefabから直接FBXを解決できない: ネストPrefabの解決が必要である ({prefab_path})"
                ),
            )));
        }
        PrefabTargetState::Unresolved => {
            return Err(PrefabConversionError::Target(crate::vrm::VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!(
                    "Prefabから直接FBXを解決できない: 未解決の参照がある ({prefab_path}: {})",
                    resolution.unresolved_references.join(", "),
                ),
            )));
        }
    };
    let mut warnings: Vec<String> = Vec::new();
    // 2. `.fbx.meta` からGUID・置換表を索引経由で読む。
    let Some(indexed_fbx) = strip_assets_prefix(&fbx_asset_path) else {
        return Err(PrefabConversionError::Prefab(crate::vrm::VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!("Prefab割当のFBXがAssets配下にない: {fbx_asset_path}"),
        )));
    };
    let Some(record) = index.get_by_path(indexed_fbx) else {
        return Err(PrefabConversionError::Prefab(crate::vrm::VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!("Prefab割当のFBXが索引にない: {fbx_asset_path}"),
        )));
    };
    let fbx_guid = record.meta.guid().as_str().to_owned();
    // 全ゼロは未設定とみなし、用途が分かる文面で明確に失敗する。
    if fbx_guid.chars().all(|c| c == '0') {
        return Err(PrefabConversionError::Prefab(crate::vrm::VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!("Prefab変換にはFBXのGUIDが必要である: {fbx_asset_path}（GUIDが全ゼロである）"),
        )));
    }
    let remaps = record.meta.external_object_remaps().map_err(|error| {
        PrefabConversionError::Material(MaterialError::new(
            MaterialErrorKind::Unity,
            format!("FBXの置換表を読めない: {error}"),
        ))
    })?;
    let Some(prefab_relative) = strip_assets_prefix(prefab_path) else {
        return Err(PrefabConversionError::Prefab(crate::vrm::VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!("PrefabのパスがAssets配下でない: {prefab_path}"),
        )));
    };
    let prefab_bytes = source
        .read_limited(prefab_relative, limits.max_prefab_bytes)
        .map_err(|error| {
            PrefabConversionError::Prefab(crate::vrm::VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!("Prefabを読めない: {prefab_path}: {error}"),
            ))
        })?;
    let mut prefab_file = SerializedFile::parse(
        &prefab_bytes,
        &ParseOptions::new().with_max_file_bytes(limits.max_prefab_bytes),
    )
    .map_err(|error| {
        PrefabConversionError::Prefab(crate::vrm::VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!("PrefabのYAMLを読めない（{prefab_path}）: {error}"),
        ))
    })?;
    let file = FbxFile::parse(fbx_bytes, fbx_options).map_err(PrefabConversionError::Fbx)?;
    // 2019.3より前の取り込みfileIDで書かれたPrefab・シーンは、以降の段が扱う
    // 新方式のfileIDへ一度だけ読み替える。
    let legacy = crate::vrm::modernize_legacy_import_ids(
        &mut prefab_file,
        &fbx_guid,
        &record.meta.legacy_recycle_names(),
        &file,
    )
    .map_err(PrefabConversionError::Prefab)?;
    if legacy.rewritten > 0 {
        warnings.push(format!(
            "旧方式の取り込みfileIDを新方式へ{}件読み替えた",
            legacy.rewritten
        ));
    }
    if !legacy.unresolved.is_empty() {
        warnings.push(format!(
            "旧方式の取り込みfileIDを新方式へ写せない対象{}件: {}",
            legacy.unresolved.len(),
            legacy.unresolved.join(", ")
        ));
    }
    Ok(PreparedPrefabContext {
        fbx_asset_path,
        fbx_guid,
        nested_prefabs,
        fbx_meta: &record.meta,
        remaps,
        prefab_file,
        file,
        warnings,
    })
}

/// 物理の導出と中間モデルへの適用を行う。
#[allow(clippy::too_many_arguments)]
fn derive_and_apply_prefab_physics(
    source: &dyn AssetSource,
    index: &AssetIndex,
    prefab_path: &str,
    fbx_bytes: &[u8],
    file: &FbxFile<'_>,
    prefab_file: &SerializedFile,
    fbx_asset_path: &str,
    physics: PrefabPhysics,
    humanoid: Option<&HumanoidBones>,
    limits: &PrefabConversionLimits,
    model: &mut VrmModel,
    warnings: &mut Vec<String>,
) -> Result<PrefabPhysicsReport, PrefabConversionError> {
    let empty = PrefabPhysicsReport {
        used: false,
        springs: 0,
        joints: 0,
        colliders: 0,
        groups: 0,
        tpose_applied: 0,
        tpose_skipped: 0,
        look_at: None,
    };
    if physics == PrefabPhysics::None {
        return Ok(empty);
    }
    // 非スキン小物などスキンを持たない GLB 変換では `select_bind` が失敗するため、
    // 物理導出をスキップして警告を残す。
    if humanoid.is_none() && file.skins().is_empty() {
        warnings.push("スキンが無いためPrefab由来の物理を適用しない".to_owned());
        return Ok(empty);
    }
    use crate::vrm::PhysicsDeriveLimits;
    use crate::vrm::PrefabDeriveSource;
    let derive_limits = PhysicsDeriveLimits::new()
        .with_max_main_fbx_bytes(limits.max_fbx_bytes)
        .with_max_extra_fbx_bytes(limits.max_extra_fbx_bytes)
        .with_max_prefab_bytes(limits.max_prefab_bytes)
        .with_max_total_bytes(limits.max_total_bytes);
    let derive_input = PrefabDeriveSource {
        fbx_bytes,
        fbx: file,
        source,
        index,
        prefab: prefab_file,
        prefab_label: prefab_path,
        fbx_asset_path,
        limits: derive_limits,
    };
    let derived =
        derive_physics_sidecar_with(&derive_input).map_err(PrefabConversionError::Physics)?;
    for ignored in &derived.ignored_types {
        warnings.push(format!("物理導出の無視: {ignored}"));
    }
    if !derived.unmatched_nodes.is_empty() {
        warnings.push(format!(
            "物理導出の未対応{}件: {}",
            derived.unmatched_nodes.len(),
            derived.unmatched_nodes.join(", "),
        ));
    }
    let sidecar = PhysicsSidecar::parse(&derived.json, &PhysicsLimits::new())
        .map_err(PrefabConversionError::Physics)?;
    let applied = match humanoid {
        Some(humanoid) => apply_physics_sidecar(
            model,
            &sidecar,
            fbx_bytes,
            humanoid,
            &SpringCompileOptions::new(),
        ),
        None => apply_physics_sidecar_without_humanoid(
            model,
            &sidecar,
            fbx_bytes,
            &SpringCompileOptions::new(),
        ),
    }
    .map_err(PrefabConversionError::PhysicsApply)?;
    for skipped in &applied.tpose_skipped {
        warnings.push(format!(
            "T-ポーズのスキップ: {} {}: {}",
            skipped.bone, skipped.node_id, skipped.reason,
        ));
    }
    Ok(PrefabPhysicsReport {
        used: true,
        springs: applied.spring_count,
        joints: applied.joint_count,
        colliders: applied.collider_count,
        groups: applied.group_count,
        tpose_applied: applied.tpose_applied,
        tpose_skipped: applied.tpose_skipped.len(),
        look_at: applied.look_at.as_ref().map(|look| look.look_type.clone()),
    })
}

/// 材質割当・ネスト統合・テクスチャ読み込み・中間モデル反映を行う。
#[allow(clippy::too_many_arguments)]
fn apply_prefab_assignments_and_materials(
    source: &dyn AssetSource,
    index: &AssetIndex,
    prefab_path: &str,
    prefab_file: &SerializedFile,
    file: &FbxFile<'_>,
    fbx_guid: &str,
    remaps: &[ExternalObjectEntry],
    fbx_options: &FbxOptions,
    vrm_options: &VrmOptions,
    material_options: &MaterialOptions,
    limits: &PrefabConversionLimits,
    model: &mut VrmModel,
    warnings: &mut Vec<String>,
) -> Result<AppliedAssignments, PrefabConversionError> {
    use crate::vrm::VrmErrorKind;

    let base = prefab_base_materials(remaps);
    let nodes = prefab_hierarchy_paths(file).map_err(PrefabConversionError::Assignment)?;
    let at: std::collections::HashMap<i64, usize> = file
        .models()
        .iter()
        .enumerate()
        .map(|(index, model)| (model.id.value(), index))
        .collect();
    let mut views = Vec::with_capacity(model.meshes.len());
    for mesh in &model.meshes {
        let Some(owner) = file.model(mesh.model) else {
            return Err(PrefabConversionError::Assignment(
                crate::vrm::VrmError::new(
                    VrmErrorKind::InvalidPrefab,
                    format!("所有モデルがFBXに無い: {:?}", mesh.model.value()),
                ),
            ));
        };
        let Some(node_index) = at.get(&owner.id.value()).copied() else {
            return Err(PrefabConversionError::Assignment(
                crate::vrm::VrmError::new(
                    VrmErrorKind::InvalidPrefab,
                    format!("所有モデルが階層に無い: {:?}", owner.id.value()),
                ),
            ));
        };
        views.push(PrefabMeshView {
            model_name: owner.name.as_str(),
            hierarchy_path: nodes[node_index].path.as_str(),
            slot_names: mesh.material_slots.as_slice(),
        });
    }
    let mut plan = plan_prefab_assignment(prefab_file, prefab_path, fbx_guid, &base, &views)
        .map_err(PrefabConversionError::Assignment)?;
    let assignment_mode = Some(plan.mode);
    let nested_limits = NestedPrefabLimits::new()
        .with_max_prefab_bytes(limits.max_prefab_bytes)
        .with_max_fbx_bytes(limits.max_extra_fbx_bytes)
        .with_max_total_bytes(limits.max_total_bytes);
    let nested_input = NestedPrefabInput {
        source,
        index,
        root_prefab: prefab_file,
        root_prefab_path: prefab_path,
        main_fbx: file,
        main_fbx_guid: fbx_guid,
        fbx_options,
        vrm_options,
        limits: &nested_limits,
    };
    let nested_prepared = prepare_nested_prefab_meshes(model, &mut plan.pool, &nested_input)
        .map_err(PrefabConversionError::Assignment)?;
    resolve_prefab_pool_paths(&mut plan.pool, index);
    let (materials, material_report) =
        load_renderer_materials_from(source, index, &plan.pool, material_options)
            .map_err(PrefabConversionError::Material)?;
    warnings.extend(material_report.warnings().iter().cloned());
    let material_count = materials.entries().len();
    let image_count = materials.images().len();
    let assignment_report =
        apply_prefab_plan(model, &plan, &materials).map_err(PrefabConversionError::Assignment)?;
    let nested_report = apply_nested_prefab_meshes(model, nested_prepared)
        .map_err(PrefabConversionError::Assignment)?;
    warnings.extend(nested_report.warnings);
    let mut excluded_names = assignment_report.excluded_names.clone();
    excluded_names.extend(nested_report.excluded_names);
    let active_meshes = plan.active_count() + nested_report.active_meshes;
    let excluded_meshes = assignment_report.excluded_count + nested_report.excluded_meshes;
    Ok(AppliedAssignments {
        assignment_mode,
        active_meshes,
        excluded_meshes,
        excluded_names,
        material_pool: assignment_report.pool_count,
        materials,
        material_count,
        image_count,
    })
}

/// 適用後のモデルと集計から構造化報告を組み立てる。
#[allow(clippy::too_many_arguments)]
fn build_prefab_report(
    prefab_path: &str,
    ctx: PreparedPrefabContext<'_, '_>,
    model: &VrmModel,
    baseline_meshes: usize,
    baseline_shapes: usize,
    physics: PrefabPhysicsReport,
    assignment_mode: Option<PrefabMode>,
    active_meshes: usize,
    excluded_meshes: usize,
    excluded_names: Vec<String>,
    material_pool: usize,
    materials: usize,
    images: usize,
    bytes: usize,
) -> PrefabConversionReport {
    let morph_targets = model.meshes.iter().map(|mesh| mesh.targets.len()).sum();
    PrefabConversionReport {
        prefab: prefab_path.to_owned(),
        fbx: ctx.fbx_asset_path,
        fbx_guid: ctx.fbx_guid,
        nested_prefabs: ctx.nested_prefabs,
        assignment_mode,
        active_meshes,
        excluded_meshes,
        excluded_names,
        material_pool,
        baseline_meshes,
        baseline_shapes,
        expressions: model.expressions.preset.len(),
        morph_targets,
        physics,
        warnings: ctx.warnings,
        nodes: model.nodes.len(),
        meshes: model.meshes.len(),
        skins: model.skins.len(),
        materials,
        images,
        bytes,
    }
}

/// `Assets/...` 形の資産パスを索引用の相対パスに直す。
fn strip_assets_prefix(path: &str) -> Option<&str> {
    path.strip_prefix("Assets/")
        .or_else(|| path.strip_prefix("Assets\\"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    use crate::fbx::FbxOptions;
    use crate::unity::{AssetIndex, AssetMeta, AssetSource};
    use crate::vrm::{VrmExportOptions, VrmMeta, VrmOptions};

    // テスト名は日本語で書く（テストは仕様・契約であるため）。

    /// メモリ内の読み手。パスと内容の対だけを持ち、ファイルシステムに触れない。
    struct 記憶読み手 {
        表: HashMap<String, Vec<u8>>,
    }

    impl AssetSource for 記憶読み手 {
        fn read_limited(&self, path: &str, limit: u64) -> std::io::Result<Vec<u8>> {
            let Some(bytes) = self.表.get(path) else {
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

    /// テスト用の直接FBXのGUID。
    const FBX_GUID: &str = "fb00000000000000000000000000000b";
    /// テスト用の2件目の直接FBXのGUID。
    const FBX_GUID_2: &str = "fb00000000000000000000000000000c";
    /// テスト用のネストPrefabのGUID。
    const NEST_GUID: &str = "0e00000000000000000000000000000d";
    /// テスト用の解決対象Prefab自体のGUID。
    const AVATAR_GUID: &str = "a0000000000000000000000000000007";
    /// テスト用の材質GUID。
    const MAT_GUID: &str = "b0d71111111111111111111111111111";
    /// 不透明lilToonシェーダのGUID。
    const SHADER_OPAQUE: &str = "df12117ecd77c31469c224178886498e";

    /// 検査用の `.meta` を組み立てる。
    fn メタを作る(guid: &str) -> AssetMeta {
        AssetMeta::parse(
            format!("fileFormatVersion: 2\nguid: {guid}\nModelImporter:\n  externalObjects: []\n")
                .as_bytes(),
        )
        .expect("検査metaは読める")
    }

    /// 検査用のPrefabの `.meta` を組み立てる。
    fn prefabメタを作る(guid: &str) -> AssetMeta {
        AssetMeta::parse(
            format!("fileFormatVersion: 2\nguid: {guid}\nPrefabImporter:\n  externalObjects: []\n")
                .as_bytes(),
        )
        .expect("検査metaは読める")
    }

    /// 検査用の `.mat` の `.meta` を組み立てる。
    fn matメタを作る(guid: &str) -> AssetMeta {
        AssetMeta::parse(
            format!(
                "fileFormatVersion: 2\nguid: {guid}\nNativeFormatImporter:\n  externalObjects: {{}}\n  userData: \n"
            )
            .as_bytes(),
        )
        .expect("検査metaは読める")
    }

    /// 検査用の最小 `.mat` を組み立てる。
    fn 検査材質を作る(名前: &str) -> Vec<u8> {
        format!(
            "%YAML 1.1\n%TAG !u! tag:unity3d.com,2011:\n--- !u!21 &2100000\nMaterial:\n  m_Name: {名前}\n  m_Shader: {{fileID: 4800000, guid: {SHADER_OPAQUE}, type: 3}}\n  m_SavedProperties:\n    m_TexEnvs: []\n    m_Floats:\n    - _Cutoff: 0.5\n    - _Cull: 2.0\n    - _ZWrite: 1.0\n    - _UseShadow: 1.0\n    - _ShadowBorder: 0.5\n    - _ShadowBlur: 0.1\n    - _lilToonVersion: 45.0\n    m_Ints: []\n    m_Colors:\n    - _Color: {{r: 1, g: 1, b: 1, a: 1}}\n    - _ShadowColor: {{r: 0.82, g: 0.76, b: 0.85, a: 1}}\n"
        )
        .into_bytes()
    }

    /// `PrefabInstance` 1件の文書を作る。
    fn 実体文書を作る(anchor: &str, guid: &str) -> String {
        format!(
            "%YAML 1.1\n%TAG !u! tag:unity3d.com,2011:\n--- !u!1001 &{anchor}\nPrefabInstance:\n  m_ObjectHideFlags: 0\n  m_SourcePrefab: {{fileID: 100100000, guid: {guid}, type: 2}}\n"
        )
    }

    /// 改変内容方式の最小Prefabを作る。改変列は呼び出し側が渡す。
    fn 改変文書を作る(fbx_guid: &str, modifications: &str) -> String {
        format!(
            "%YAML 1.1\n%TAG !u! tag:unity3d.com,2011:\n--- !u!1001 &100\nPrefabInstance:\n  m_ObjectHideFlags: 0\n  serializedVersion: 2\n  m_Modification:\n    serializedVersion: 3\n    m_TransformParent: {{fileID: 0}}\n    m_Modifications:\n{modifications}  m_SourcePrefab: {{fileID: 100100000, guid: {fbx_guid}, type: 3}}\n"
        )
    }

    /// `m_Materials.Array.data[i]` の上書き1件を作る。
    fn material上書きを作る(target: i64, slot: usize, guid: &str) -> String {
        format!(
            "    - target: {{fileID: {target}, guid: {FBX_GUID}, type: 3}}\n      propertyPath: m_Materials.Array.data[{slot}]\n      value: \n      objectReference: {{fileID: 2100000, guid: {guid}, type: 2}}\n"
        )
    }

    /// `m_IsActive` の上書き1件を作る。0で無効になる。
    fn active上書きを作る(target: i64, value: i64) -> String {
        format!(
            "    - target: {{fileID: {target}, guid: {FBX_GUID}, type: 3}}\n      propertyPath: m_IsActive\n      value: {value}\n      objectReference: {{fileID: 0}}\n"
        )
    }

    /// 複数の文書列を1つのPrefab本文に結合する。
    fn 結合する(個々: &[String]) -> Vec<u8> {
        const DIRECTIVE: &str = "%YAML 1.1\n%TAG !u! tag:unity3d.com,2011:\n";
        let mut out = String::new();
        for (番号, 文書) in 個々.iter().enumerate() {
            if 番号 == 0 {
                out.push_str(文書);
            } else {
                out.push_str(文書.strip_prefix(DIRECTIVE).expect("directiveがある"));
            }
        }
        out.into_bytes()
    }

    /// 論理プロパティ列。
    #[derive(Default)]
    struct Props {
        buf: Vec<u8>,
        count: u32,
    }

    impl Props {
        fn prop(&mut self, code: u8, body: &[u8]) {
            self.buf.push(code);
            self.buf.extend_from_slice(body);
            self.count += 1;
        }

        fn l(&mut self, v: i64) {
            self.prop(b'L', &v.to_le_bytes());
        }

        fn s(&mut self, v: &[u8]) {
            let mut body = (v.len() as u32).to_le_bytes().to_vec();
            body.extend_from_slice(v);
            self.prop(b'S', &body);
        }

        fn array(&mut self, code: u8, count: u32, payload: &[u8]) {
            let mut body = count.to_le_bytes().to_vec();
            body.extend_from_slice(&0u32.to_le_bytes());
            body.extend_from_slice(&(payload.len() as u32).to_le_bytes());
            body.extend_from_slice(payload);
            self.prop(code, &body);
        }

        fn d_raw(&mut self, values: &[f64]) {
            let mut payload = Vec::with_capacity(values.len() * 8);
            for v in values {
                payload.extend_from_slice(&v.to_le_bytes());
            }
            self.array(b'd', values.len() as u32, &payload);
        }

        fn i_raw(&mut self, values: &[i32]) {
            let mut payload = Vec::with_capacity(values.len() * 4);
            for v in values {
                payload.extend_from_slice(&v.to_le_bytes());
            }
            self.array(b'i', values.len() as u32, &payload);
        }
    }

    /// 論理ノード。
    struct TNode {
        name: Vec<u8>,
        props: Props,
        children: Vec<TNode>,
    }

    impl TNode {
        fn leaf(name: &[u8], props: Props) -> Self {
            Self {
                name: name.to_vec(),
                props,
                children: Vec::new(),
            }
        }

        fn branch(name: &[u8], props: Props, children: Vec<TNode>) -> Self {
            Self {
                name: name.to_vec(),
                props,
                children,
            }
        }
    }

    fn ser(node: &TNode, buf: &mut Vec<u8>) {
        let start = buf.len();
        buf.extend_from_slice(&[0u8; 13]);
        buf.extend_from_slice(&node.name);
        buf.extend_from_slice(&node.props.buf);
        for child in &node.children {
            ser(child, buf);
        }
        if !node.children.is_empty() {
            buf.extend_from_slice(&[0u8; 13]);
        }
        let end = buf.len() as u32;
        buf[start..start + 4].copy_from_slice(&end.to_le_bytes());
        buf[start + 4..start + 8].copy_from_slice(&node.props.count.to_le_bytes());
        buf[start + 8..start + 12].copy_from_slice(&(node.props.buf.len() as u32).to_le_bytes());
        buf[start + 12] = node.name.len() as u8;
    }

    fn file(top: Vec<TNode>) -> Vec<u8> {
        let mut buf = b"Kaydara FBX Binary  \x00\x1a\x00".to_vec();
        buf.extend_from_slice(&7400u32.to_le_bytes());
        for node in &top {
            ser(node, &mut buf);
        }
        buf.extend_from_slice(&[0u8; 13]);
        buf
    }

    fn model(id: i64, name: &str, subtype: &str) -> TNode {
        let mut props = Props::default();
        props.l(id);
        props.s(format!("{name}\x00\x01Model").as_bytes());
        props.s(subtype.as_bytes());
        TNode::branch(b"Model", props, Vec::new())
    }

    fn material_node(id: i64, name: &str) -> TNode {
        let mut props = Props::default();
        props.l(id);
        props.s(format!("{name}\x00\x01Material").as_bytes());
        TNode::leaf(b"Material", props)
    }

    fn geometry(id: i64, name: &str, children: Vec<TNode>) -> TNode {
        let mut props = Props::default();
        props.l(id);
        props.s(format!("{name}\x00\x01Geometry").as_bytes());
        props.s(b"Mesh");
        TNode::branch(b"Geometry", props, children)
    }

    fn vertices_node(values: &[f64]) -> TNode {
        let mut props = Props::default();
        props.d_raw(values);
        TNode::leaf(b"Vertices", props)
    }

    fn pvi_node(values: &[i32]) -> TNode {
        let mut props = Props::default();
        props.i_raw(values);
        TNode::leaf(b"PolygonVertexIndex", props)
    }

    fn conn(child: i64, parent: i64) -> TNode {
        let mut props = Props::default();
        props.s(b"OO");
        props.l(child);
        props.l(parent);
        TNode::leaf(b"C", props)
    }

    fn objects(children: Vec<TNode>) -> TNode {
        TNode::branch(b"Objects", Props::default(), children)
    }

    fn connections(children: Vec<TNode>) -> TNode {
        TNode::branch(b"Connections", Props::default(), children)
    }

    fn pose_node(model: i64, matrix: &[f64; 16]) -> TNode {
        let mut matrix_props = Props::default();
        matrix_props.d_raw(matrix);
        let mut node_props = Props::default();
        node_props.l(model);
        TNode::branch(
            b"PoseNode",
            Props::default(),
            vec![
                TNode::leaf(b"Node", node_props),
                TNode::leaf(b"Matrix", matrix_props),
            ],
        )
    }

    fn bind_matrix(x: f64, y: f64, z: f64) -> [f64; 16] {
        [
            1.0, 0.0, 0.0, 0.0, //
            0.0, 1.0, 0.0, 0.0, //
            0.0, 0.0, 1.0, 0.0, //
            x, y, z, 1.0, //
        ]
    }

    /// 必須15骨を持つモデル列。末尾はメッシュ所有モデルである。
    fn 必須骨モデル列() -> Vec<(i64, &'static str, Option<i64>)> {
        vec![
            (1, "Hips", None),
            (2, "Spine", Some(1)),
            (3, "Head", Some(2)),
            (4, "LeftUpperArm", Some(2)),
            (5, "LeftLowerArm", Some(4)),
            (6, "LeftHand", Some(5)),
            (7, "RightUpperArm", Some(2)),
            (8, "RightLowerArm", Some(7)),
            (9, "RightHand", Some(8)),
            (10, "LeftUpperLeg", Some(1)),
            (11, "LeftLowerLeg", Some(10)),
            (12, "LeftFoot", Some(11)),
            (13, "RightUpperLeg", Some(1)),
            (14, "RightLowerLeg", Some(13)),
            (15, "RightFoot", Some(14)),
            (16, "Owner", Some(1)),
        ]
    }

    /// 必須15骨と三角形1枚を持つFBXを組み立てる。材質スロットは持たない。
    fn 書き出し可能なfbxを組み立てる() -> Vec<u8> {
        材質付きfbxを組み立てる(&[])
    }

    /// 材質スロット付きのFBXを組み立てる。材質列は（ID、名前）である。
    fn 材質付きfbxを組み立てる(材質ら: &[(i64, &str)]) -> Vec<u8> {
        let mut objs: Vec<TNode> = 必須骨モデル列()
            .iter()
            .map(|(id, name, _)| {
                let subtype = if *name == "Owner" { "Mesh" } else { "LimbNode" };
                model(*id, name, subtype)
            })
            .collect();
        for (id, 名前) in 材質ら {
            objs.push(material_node(*id, 名前));
        }
        objs.push(geometry(
            100,
            "Tri",
            vec![
                vertices_node(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]),
                pvi_node(&[0, 1, -3]),
            ],
        ));
        let mut pose_props = Props::default();
        pose_props.l(200);
        pose_props.s(b"Bind\x00\x01Pose");
        pose_props.s(b"BindPose");
        let binds: Vec<TNode> = 必須骨モデル列()
            .iter()
            .map(|(id, _, _)| pose_node(*id, &bind_matrix(0.0, 0.0, 0.0)))
            .collect();
        objs.push(TNode::branch(b"Pose", pose_props, binds));
        let mut conns: Vec<TNode> = 必須骨モデル列()
            .iter()
            .filter_map(|(id, _, parent)| parent.map(|parent| conn(*id, parent)))
            .collect();
        for (id, _) in 材質ら {
            conns.push(conn(*id, 16));
        }
        conns.push(conn(100, 16));
        conns.push(conn(1, 0));
        file(vec![objects(objs), connections(conns)])
    }

    fn 書き出し設定を作る() -> VrmExportOptions {
        VrmExportOptions::new(VrmMeta::new("試験", vec!["作者".to_string()]))
    }

    fn 変換選択を作る(
        expressions: PrefabExpressions,
        baseline: bool,
        physics: PrefabPhysics,
    ) -> PrefabConversionOptions {
        PrefabConversionOptions {
            expressions,
            baseline,
            physics,
            material_options: MaterialOptions::new(),
            limits: PrefabConversionLimits::new(),
        }
    }

    /// 索引と読み手を作る。要素は（相対パス、meta、本文の有無）である。
    fn 索引と読み手を作る(
        件: &[(&str, AssetMeta, Option<Vec<u8>>)],
    ) -> (AssetIndex, 記憶読み手) {
        let mut index = AssetIndex::new();
        let mut 表 = HashMap::new();
        for (path, meta, 本文) in 件 {
            index.insert(path, meta.clone()).expect("索引に入る");
            if let Some(本文) = 本文 {
                表.insert((*path).to_owned(), 本文.clone());
            }
        }
        (index, 記憶読み手 { 表 })
    }

    #[test]
    fn 直接fbx1件とネストprefabで変換に成功する() {
        // 直接FBX1件に空のネストPrefabが付いても `Resolved` として変換でき、
        // ネストは報告に残る。「出力しない」警告は出さない。
        use crate::unity::skinned_mesh_renderer_file_id;
        let fbx_bytes = 書き出し可能なfbxを組み立てる();
        let renderer = skinned_mesh_renderer_file_id("Hips/Owner");
        let prefab_body = 結合する(&[
            改変文書を作る(FBX_GUID, ""),
            実体文書を作る("100100001", NEST_GUID),
        ]);
        // 上書きが無いがスロットも無いため、材質プールは空で通る。
        let _ = renderer;
        let (index, 読み手) = 索引と読み手を作る(&[
            ("Model.fbx", メタを作る(FBX_GUID), None),
            (
                "Nest.prefab",
                prefabメタを作る(NEST_GUID),
                Some(
                    "%YAML 1.1\n%TAG !u! tag:unity3d.com,2011:\n--- !u!1 &100000\nGameObject:\n  m_ObjectHideFlags: 0\n  m_Name: Root\n  m_IsActive: 1\n  m_Component: []\n"
                        .as_bytes()
                        .to_vec(),
                ),
            ),
            (
                "Avatar.prefab",
                prefabメタを作る(AVATAR_GUID),
                Some(prefab_body),
            ),
        ]);
        let 選択 = 変換選択を作る(PrefabExpressions::None, false, PrefabPhysics::None);
        let (vrm_bytes, 報告) = convert_prefab(
            &読み手,
            &index,
            "Assets/Avatar.prefab",
            &fbx_bytes,
            &FbxOptions::new(),
            &VrmOptions::new(),
            書き出し設定を作る(),
            &選択,
        )
        .expect("直接FBX1件とネストPrefabは変換できる");
        assert_eq!(&vrm_bytes[..4], b"glTF");
        assert_eq!(報告.fbx, "Assets/Model.fbx");
        assert_eq!(報告.fbx_guid, FBX_GUID);
        assert_eq!(報告.nested_prefabs, vec!["Assets/Nest.prefab".to_owned()]);
        assert!(
            !報告
                .warnings
                .iter()
                .any(|警告| 警告.contains("ネストPrefabのジオメトリは出力しない")),
            "旧仕様の警告は出さない: {:?}",
            報告.warnings
        );
        assert!(!報告.physics.used, "物理なしを指定したため使わない");
        assert_eq!(報告.active_meshes, 1);
        assert_eq!(報告.excluded_meshes, 0);
    }

    #[test]
    fn 物理導出の失敗を握り潰さない() {
        // 骨なしの最小FBXでは導出がbind-poseの欠落で失敗し、物理なしに落ちない。
        let fbx_bytes = 書き出し可能なfbxを組み立てる();
        let (index, 読み手) = 索引と読み手を作る(&[
            ("Model.fbx", メタを作る(FBX_GUID), None),
            (
                "Avatar.prefab",
                prefabメタを作る(AVATAR_GUID),
                Some(改変文書を作る(FBX_GUID, "").into_bytes()),
            ),
        ]);
        let 選択 = 変換選択を作る(PrefabExpressions::None, false, PrefabPhysics::Derive);
        let 失敗 = convert_prefab(
            &読み手,
            &index,
            "Assets/Avatar.prefab",
            &fbx_bytes,
            &FbxOptions::new(),
            &VrmOptions::new(),
            書き出し設定を作る(),
            &選択,
        )
        .expect_err("導出の失敗は全体の失敗になる");
        assert_eq!(失敗.stage(), PrefabConversionStage::Physics);
    }

    #[test]
    fn descriptorとbaselineの空解決でも変換に成功する() {
        // descriptorが無く基準姿勢の重みも無い場合は空のまま通り、
        // 表情と焼き込みの件数はゼロになる。物理なしでも解決自体は行う。
        let fbx_bytes = 書き出し可能なfbxを組み立てる();
        let (index, 読み手) = 索引と読み手を作る(&[
            ("Model.fbx", メタを作る(FBX_GUID), None),
            (
                "Avatar.prefab",
                prefabメタを作る(AVATAR_GUID),
                Some(改変文書を作る(FBX_GUID, "").into_bytes()),
            ),
        ]);
        let 選択 = 変換選択を作る(PrefabExpressions::Descriptor, true, PrefabPhysics::None);
        let (vrm_bytes, 報告) = convert_prefab(
            &読み手,
            &index,
            "Assets/Avatar.prefab",
            &fbx_bytes,
            &FbxOptions::new(),
            &VrmOptions::new(),
            書き出し設定を作る(),
            &選択,
        )
        .expect("空の表情・基準姿勢でも通る");
        assert_eq!(&vrm_bytes[..4], b"glTF");
        assert_eq!(報告.expressions, 0);
        assert_eq!(報告.baseline_meshes, 0);
        assert_eq!(報告.baseline_shapes, 0);
        assert!(!報告.physics.used, "物理なしを指定したため使わない");
    }

    #[test]
    fn 直接fbx複数で対象解決の段階で失敗する() {
        // 直接FBXが2種ある場合は先勝ちにせず対象解決で失敗する。
        let fbx_bytes = 書き出し可能なfbxを組み立てる();
        let prefab_body = 結合する(&[
            実体文書を作る("100100000", FBX_GUID),
            実体文書を作る("100100001", FBX_GUID_2),
        ]);
        let (index, 読み手) = 索引と読み手を作る(&[
            ("Model.fbx", メタを作る(FBX_GUID), None),
            ("Other.fbx", メタを作る(FBX_GUID_2), None),
            (
                "Avatar.prefab",
                prefabメタを作る(AVATAR_GUID),
                Some(prefab_body),
            ),
        ]);
        let 選択 = 変換選択を作る(PrefabExpressions::None, false, PrefabPhysics::None);
        let 失敗 = convert_prefab(
            &読み手,
            &index,
            "Assets/Avatar.prefab",
            &fbx_bytes,
            &FbxOptions::new(),
            &VrmOptions::new(),
            書き出し設定を作る(),
            &選択,
        )
        .expect_err("直接FBX複数は失敗する");
        assert_eq!(失敗.stage(), PrefabConversionStage::Target);
    }

    #[test]
    fn ネストprefabのみで対象解決の段階で失敗する() {
        // 直接FBXが無くネストだけの場合は解決が必要として失敗する。
        let fbx_bytes = 書き出し可能なfbxを組み立てる();
        let (index, 読み手) = 索引と読み手を作る(&[
            ("Model.fbx", メタを作る(FBX_GUID), None),
            (
                "Nest.prefab",
                prefabメタを作る(NEST_GUID),
                Some(
                    "%YAML 1.1\n%TAG !u! tag:unity3d.com,2011:\n--- !u!1 &100000\nGameObject:\n  m_ObjectHideFlags: 0\n  m_Name: Root\n  m_IsActive: 1\n  m_Component: []\n"
                        .as_bytes()
                        .to_vec(),
                ),
            ),
            (
                "Avatar.prefab",
                prefabメタを作る(AVATAR_GUID),
                Some(実体文書を作る("100100000", NEST_GUID).into_bytes()),
            ),
        ]);
        let 選択 = 変換選択を作る(PrefabExpressions::None, false, PrefabPhysics::None);
        let 失敗 = convert_prefab(
            &読み手,
            &index,
            "Assets/Avatar.prefab",
            &fbx_bytes,
            &FbxOptions::new(),
            &VrmOptions::new(),
            書き出し設定を作る(),
            &選択,
        )
        .expect_err("ネストのみは失敗する");
        assert_eq!(失敗.stage(), PrefabConversionStage::Target);
    }

    #[test]
    fn 物理なしでも材質割当が適用される() {
        // 物理を使わなくても割当・材質プールは通り、件数に残る。
        use crate::unity::skinned_mesh_renderer_file_id;
        let fbx_bytes = 材質付きfbxを組み立てる(&[(300, "Mat0")]);
        let renderer = skinned_mesh_renderer_file_id("Hips/Owner");
        let prefab_body =
            改変文書を作る(FBX_GUID, &material上書きを作る(renderer, 0, MAT_GUID)).into_bytes();
        let (index, 読み手) = 索引と読み手を作る(&[
            ("Model.fbx", メタを作る(FBX_GUID), None),
            (
                "Avatar.prefab",
                prefabメタを作る(AVATAR_GUID),
                Some(prefab_body),
            ),
            (
                "Materials/Body.mat",
                matメタを作る(MAT_GUID),
                Some(検査材質を作る("肌")),
            ),
        ]);
        let 選択 = 変換選択を作る(PrefabExpressions::None, false, PrefabPhysics::None);
        let (vrm_bytes, 報告) = convert_prefab(
            &読み手,
            &index,
            "Assets/Avatar.prefab",
            &fbx_bytes,
            &FbxOptions::new(),
            &VrmOptions::new(),
            書き出し設定を作る(),
            &選択,
        )
        .expect("物理なしでも割当は通る");
        assert_eq!(&vrm_bytes[..4], b"glTF");
        assert!(!報告.physics.used, "物理を使わない");
        assert_eq!(報告.active_meshes, 1);
        assert_eq!(報告.material_pool, 1);
        assert_eq!(報告.materials, 1);
        assert_eq!(報告.excluded_meshes, 0);
    }

    #[test]
    fn 無効なメッシュは共有入口で除外される() {
        // Prefabの `m_IsActive: 0` は材質割当の前に効き、除外件数と名前が
        // レポートに残る。共有入口が対応付けと適用を正しく通すことの回帰である。
        use crate::unity::{game_object_file_id, skinned_mesh_renderer_file_id};
        let fbx_bytes = 材質付きfbxを組み立てる(&[(300, "Mat0")]);
        let renderer = skinned_mesh_renderer_file_id("Hips/Owner");
        let owner = game_object_file_id("Hips/Owner");
        let prefab_body = 改変文書を作る(
            FBX_GUID,
            &(material上書きを作る(renderer, 0, MAT_GUID) + &active上書きを作る(owner, 0)),
        )
        .into_bytes();
        let (index, 読み手) = 索引と読み手を作る(&[
            ("Model.fbx", メタを作る(FBX_GUID), None),
            (
                "Avatar.prefab",
                prefabメタを作る(AVATAR_GUID),
                Some(prefab_body),
            ),
            (
                "Materials/Body.mat",
                matメタを作る(MAT_GUID),
                Some(検査材質を作る("肌")),
            ),
        ]);
        let 選択 = 変換選択を作る(PrefabExpressions::None, false, PrefabPhysics::None);
        let (vrm_bytes, 報告) = convert_prefab(
            &読み手,
            &index,
            "Assets/Avatar.prefab",
            &fbx_bytes,
            &FbxOptions::new(),
            &VrmOptions::new(),
            書き出し設定を作る(),
            &選択,
        )
        .expect("無効なメッシュの除外は成功する");
        assert_eq!(&vrm_bytes[..4], b"glTF");
        assert_eq!(報告.active_meshes, 0);
        assert_eq!(報告.excluded_meshes, 1);
        assert_eq!(報告.excluded_names, vec!["Owner".to_owned()]);
        assert_eq!(報告.meshes, 0);
        assert_eq!(報告.material_pool, 0);
        assert_eq!(報告.materials, 0);
    }

    #[test]
    fn guid全ゼロのfbxは明確に失敗する() {
        // `.fbx.meta` のGUIDが全ゼロの場合はGUID不足として失敗する。
        const ZERO: &str = "00000000000000000000000000000000";
        let fbx_bytes = 書き出し可能なfbxを組み立てる();
        let (index, 読み手) = 索引と読み手を作る(&[
            ("Model.fbx", メタを作る(ZERO), None),
            (
                "Avatar.prefab",
                prefabメタを作る(AVATAR_GUID),
                Some(実体文書を作る("100100000", ZERO).into_bytes()),
            ),
        ]);
        let 選択 = 変換選択を作る(PrefabExpressions::None, false, PrefabPhysics::None);
        let 失敗 = convert_prefab(
            &読み手,
            &index,
            "Assets/Avatar.prefab",
            &fbx_bytes,
            &FbxOptions::new(),
            &VrmOptions::new(),
            書き出し設定を作る(),
            &選択,
        )
        .expect_err("全ゼロGUIDは失敗する");
        assert_eq!(失敗.stage(), PrefabConversionStage::Prefab);
        assert!(
            format!("{失敗}").contains("GUID"),
            "GUID不足が分かる文面である: {失敗}"
        );
    }

    #[test]
    fn 上書きも置換表も無いスロットは割当で失敗する() {
        // 材質スロットがあるのに上書きも置換表も無い場合は割当で失敗する。
        let fbx_bytes = 材質付きfbxを組み立てる(&[(300, "Mat0")]);
        let (index, 読み手) = 索引と読み手を作る(&[
            ("Model.fbx", メタを作る(FBX_GUID), None),
            (
                "Avatar.prefab",
                prefabメタを作る(AVATAR_GUID),
                Some(改変文書を作る(FBX_GUID, "").into_bytes()),
            ),
        ]);
        let 選択 = 変換選択を作る(PrefabExpressions::None, false, PrefabPhysics::None);
        let 失敗 = convert_prefab(
            &読み手,
            &index,
            "Assets/Avatar.prefab",
            &fbx_bytes,
            &FbxOptions::new(),
            &VrmOptions::new(),
            書き出し設定を作る(),
            &選択,
        )
        .expect_err("割当の厳格失敗は通さない");
        assert!(
            matches!(
                失敗.stage(),
                PrefabConversionStage::Assignment | PrefabConversionStage::Material
            ),
            "段階は割当または材質である: {:?}",
            失敗.stage()
        );
    }

    #[test]
    fn convert_prefab_glbでhumanoid無しのスキン衣装prefabを物理と材質込みでglb変換できる() {
        use crate::unity::{game_object_file_id, skinned_mesh_renderer_file_id, transform_file_id};
        use crate::vrm::VrmMorphMode;

        // Humanoid 骨（Hips 等）を持たない衣装用スキン FBX（Armature -> SkirtRoot -> SkirtTip）。
        let mut objs = vec![
            model(1, "Armature", "Null"),
            model(2, "SkirtRoot", "LimbNode"),
            model(3, "SkirtTip", "LimbNode"),
            model(10, "SkirtMesh", "Mesh"),
            material_node(300, "SkirtMat"),
            geometry(
                100,
                "SkirtGeo",
                vec![
                    vertices_node(&[0.0, 0.0, 0.0, 0.2, 0.0, 0.0, 0.0, 0.2, 0.0]),
                    pvi_node(&[0, 1, -3]),
                ],
            ),
            {
                let mut sp = Props::default();
                sp.l(400);
                sp.s(b"Skin\x00\x01Deformer");
                sp.s(b"Skin");
                TNode::leaf(b"Deformer", sp)
            },
            {
                let mut c1 = Props::default();
                c1.l(401);
                c1.s(b"C1\x00\x01SubDeformer");
                c1.s(b"Cluster");
                let mut idx1 = Props::default();
                idx1.i_raw(&[0, 1, 2]);
                let mut wt1 = Props::default();
                wt1.d_raw(&[1.0, 1.0, 1.0]);
                let mut tr = Props::default();
                tr.d_raw(&bind_matrix(0.0, 0.0, 0.0));
                let mut trl = Props::default();
                trl.d_raw(&bind_matrix(0.0, 0.0, 0.0));
                TNode::branch(
                    b"Deformer",
                    c1,
                    vec![
                        TNode::leaf(b"Indexes", idx1),
                        TNode::leaf(b"Weights", wt1),
                        TNode::leaf(b"Transform", tr),
                        TNode::leaf(b"TransformLink", trl),
                    ],
                )
            },
            {
                let mut c2 = Props::default();
                c2.l(402);
                c2.s(b"C2\x00\x01SubDeformer");
                c2.s(b"Cluster");
                let mut tr = Props::default();
                tr.d_raw(&bind_matrix(0.0, 0.0, 0.0));
                let mut trl = Props::default();
                trl.d_raw(&bind_matrix(0.0, -0.2, 0.0));
                TNode::branch(
                    b"Deformer",
                    c2,
                    vec![
                        TNode::leaf(b"Transform", tr),
                        TNode::leaf(b"TransformLink", trl),
                    ],
                )
            },
            {
                let mut pose_props = Props::default();
                pose_props.l(200);
                pose_props.s(b"Bind\x00\x01Pose");
                pose_props.s(b"BindPose");
                TNode::branch(
                    b"Pose",
                    pose_props,
                    vec![
                        pose_node(1, &bind_matrix(0.0, 0.0, 0.0)),
                        pose_node(2, &bind_matrix(0.0, 0.0, 0.0)),
                        pose_node(3, &bind_matrix(0.0, -0.2, 0.0)),
                        pose_node(10, &bind_matrix(0.0, 0.0, 0.0)),
                    ],
                )
            },
        ];
        let conns = vec![
            conn(1, 0),
            conn(2, 1),
            conn(3, 2),
            conn(10, 0),
            conn(100, 10),
            conn(300, 10),
            conn(400, 100),
            conn(401, 400),
            conn(402, 400),
            conn(2, 401),
            conn(3, 402),
        ];
        let _ = &mut objs;
        let fbx_bytes = file(vec![objects(objs), connections(conns)]);

        let renderer = skinned_mesh_renderer_file_id("SkirtMesh");
        let skirt_root_go = game_object_file_id("Armature/SkirtRoot");
        let skirt_root_tr = transform_file_id("Armature/SkirtRoot");
        let phys_guid = "2a2c05204084d904aa4945ccff20d8e5";
        let empty_curve = "{m_Curve: [], m_PreInfinity: 2, m_PostInfinity: 2}";
        let prefab_body = format!(
            "%YAML 1.1\n%TAG !u! tag:unity3d.com,2011:\n\
--- !u!1001 &100\nPrefabInstance:\n  m_Modification:\n    m_TransformParent: {{fileID: 0}}\n    m_Modifications:\n    - target: {{fileID: {renderer}, guid: {FBX_GUID}, type: 3}}\n      propertyPath: m_Materials.Array.data[0]\n      value: \n      objectReference: {{fileID: 2100000, guid: {MAT_GUID}, type: 2}}\n    m_AddedComponents:\n    - targetCorrespondingSourceObject: {{fileID: {skirt_root_go}, guid: {FBX_GUID}, type: 3}}\n      insertIndex: -1\n      addedObject: {{fileID: 5001}}\n  m_SourcePrefab: {{fileID: 100100000, guid: {FBX_GUID}, type: 3}}\n\
--- !u!1 &1001 stripped\nGameObject:\n  m_CorrespondingSourceObject: {{fileID: {skirt_root_go}, guid: {FBX_GUID}, type: 3}}\n  m_PrefabInstance: {{fileID: 100}}\n  m_PrefabAsset: {{fileID: 0}}\n\
--- !u!4 &1002 stripped\nTransform:\n  m_CorrespondingSourceObject: {{fileID: {skirt_root_tr}, guid: {FBX_GUID}, type: 3}}\n  m_PrefabInstance: {{fileID: 100}}\n  m_PrefabAsset: {{fileID: 0}}\n\
--- !u!114 &5001\nMonoBehaviour:\n  m_ObjectHideFlags: 0\n  m_CorrespondingSourceObject: {{fileID: 0}}\n  m_PrefabInstance: {{fileID: 0}}\n  m_PrefabAsset: {{fileID: 0}}\n  m_GameObject: {{fileID: 1001}}\n  m_Enabled: 1\n  m_Name: \n  m_Script: {{fileID: 1661641543, guid: {phys_guid}, type: 3}}\n  version: 0\n  integrationType: 0\n  rootTransform: {{fileID: 1002}}\n  ignoreTransforms: []\n  endpointPosition: {{x: 0, y: 0, z: 0}}\n  multiChildType: 0\n  pull: 0.2\n  pullCurve: {empty_curve}\n  spring: 0.3\n  springCurve: {empty_curve}\n  stiffness: 0.1\n  stiffnessCurve: {empty_curve}\n  gravity: 0.0\n  gravityCurve: {empty_curve}\n  gravityFalloff: 0.0\n  gravityFalloffCurve: {empty_curve}\n  immobileType: 0\n  immobile: 0.0\n  immobileCurve: {empty_curve}\n  radius: 0.02\n  radiusCurve: {empty_curve}\n  allowCollision: 1\n  colliders: []\n  limitType: 0\n  maxAngleX: 0.0\n  maxAngleXCurve: {empty_curve}\n  maxAngleZ: 0.0\n  maxAngleZCurve: {empty_curve}\n  limitRotation: {{x: 0, y: 0, z: 0}}\n  limitRotationXCurve: {empty_curve}\n  limitRotationYCurve: {empty_curve}\n  limitRotationZCurve: {empty_curve}\n  isAnimated: 0\n"
        );
        let (index, 読み手) = 索引と読み手を作る(&[
            ("Skirt.fbx", メタを作る(FBX_GUID), None),
            (
                "Skirt.prefab",
                prefabメタを作る(AVATAR_GUID),
                Some(prefab_body.into_bytes()),
            ),
            (
                "Materials/Skirt.mat",
                matメタを作る(MAT_GUID),
                Some(検査材質を作る("スカート")),
            ),
        ]);
        let options = PrefabGlbConversionOptions {
            baseline: true,
            physics: PrefabPhysics::Derive,
            material_options: MaterialOptions::new(),
            limits: PrefabConversionLimits::new(),
        };
        let (glb, 報告) = convert_prefab_glb(
            &読み手,
            &index,
            "Assets/Skirt.prefab",
            &fbx_bytes,
            &FbxOptions::new(),
            &VrmOptions::new().with_morph_mode(VrmMorphMode::All),
            GlbExportOptions::new(),
            &options,
        )
        .expect("Humanoid無しの衣装PrefabもGLB変換できる");
        assert_eq!(&glb[..4], b"glTF");
        assert_eq!(報告.materials, 1);
        assert!(報告.physics.used, "PhysBoneが導出・適用される");
        assert_eq!(報告.physics.springs, 1);
        assert_eq!(報告.physics.tpose_applied, 0);
        assert!(報告.physics.look_at.is_none());
        let json_len = u32::from_le_bytes(glb[12..16].try_into().expect("長さ")) as usize;
        let json: serde_json::Value =
            serde_json::from_slice(&glb[20..20 + json_len]).expect("JSONは読める");
        assert!(json["extensions"].get("VRMC_vrm").is_none());
        assert!(json["extensions"].get("VRMC_springBone").is_some());
    }

    #[test]
    fn convert_prefab_glbで非スキン小物prefabは物理derive指定でも警告を残して成功する() {
        // BindPoseもスキンも持たない小物FBX。
        let objs = vec![
            model(1, "Hat", "Mesh"),
            geometry(
                100,
                "HatGeo",
                vec![
                    vertices_node(&[0.0, 0.0, 0.0, 0.2, 0.0, 0.0, 0.0, 0.2, 0.0]),
                    pvi_node(&[0, 1, -3]),
                ],
            ),
        ];
        let conns = vec![conn(1, 0), conn(100, 1)];
        let fbx_bytes = file(vec![objects(objs), connections(conns)]);
        let (index, 読み手) = 索引と読み手を作る(&[
            ("Hat.fbx", メタを作る(FBX_GUID), None),
            (
                "Hat.prefab",
                prefabメタを作る(AVATAR_GUID),
                Some(改変文書を作る(FBX_GUID, "").into_bytes()),
            ),
        ]);
        let options = PrefabGlbConversionOptions {
            baseline: true,
            physics: PrefabPhysics::Derive,
            material_options: MaterialOptions::new(),
            limits: PrefabConversionLimits::new(),
        };
        let (glb, 報告) = convert_prefab_glb(
            &読み手,
            &index,
            "Assets/Hat.prefab",
            &fbx_bytes,
            &FbxOptions::new(),
            &VrmOptions::new(),
            GlbExportOptions::new(),
            &options,
        )
        .expect("非スキン小物でもDerive指定で成功する");
        assert_eq!(&glb[..4], b"glTF");
        assert!(!報告.physics.used);
        assert!(
            報告.warnings.iter().any(|w| w.contains("スキンが無いため")),
            "警告が残る: {:?}",
            報告.warnings
        );
    }

    #[test]
    fn convert_prefabでmorph_mode_allを指定すると表情と全モーフが両方出力される() {
        use crate::vrm::VrmMorphMode;
        // 必須15骨 + Mesh + BlendShape 2件（vrc.v_aa と Shrink_Body）。
        let mut objs: Vec<TNode> = 必須骨モデル列()
            .iter()
            .map(|(id, name, _)| {
                let subtype = if *name == "Owner" { "Mesh" } else { "LimbNode" };
                model(*id, name, subtype)
            })
            .collect();
        objs.push(geometry(
            100,
            "Tri",
            vec![
                vertices_node(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]),
                pvi_node(&[0, 1, -3]),
            ],
        ));
        let mk_deformer = |id: i64, name: &[u8], subtype: &[u8], children: Vec<TNode>| {
            let mut props = Props::default();
            props.l(id);
            props.s(name);
            props.s(subtype);
            TNode::branch(b"Deformer", props, children)
        };
        objs.push(mk_deformer(
            500,
            b"Blend\x00\x01Deformer",
            b"BlendShape",
            vec![],
        ));
        for (idx, name) in ["Shrink_Body", "vrc.v_aa"].iter().enumerate() {
            let cid = 510 + idx as i64;
            let sid = 520 + idx as i64;
            let mut cname = name.as_bytes().to_vec();
            cname.extend_from_slice(b"\x00\x01SubDeformer");
            objs.push(mk_deformer(
                cid,
                &cname,
                b"BlendShapeChannel",
                vec![{
                    let mut dp = Props::default();
                    dp.prop(b'D', &0.0f64.to_le_bytes());
                    TNode::leaf(b"DeformPercent", dp)
                }],
            ));
            let mut sname = format!("shape{idx}").into_bytes();
            sname.extend_from_slice(b"\x00\x01Shape");
            let mut idx_p = Props::default();
            idx_p.i_raw(&[0]);
            let mut v_p = Props::default();
            v_p.d_raw(&[0.1, 0.0, 0.0]);
            objs.push(TNode::branch(
                {
                    let _ = 0;
                    b"Geometry"
                },
                {
                    let mut p = Props::default();
                    p.l(sid);
                    p.s(&sname);
                    p.s(b"Shape");
                    p
                },
                vec![
                    TNode::leaf(b"Indexes", idx_p),
                    TNode::leaf(b"Vertices", v_p),
                ],
            ));
        }
        let mut pose_props = Props::default();
        pose_props.l(200);
        pose_props.s(b"Bind\x00\x01Pose");
        pose_props.s(b"BindPose");
        let pose_nodes = 必須骨モデル列()
            .iter()
            .map(|(id, _, _)| pose_node(*id, &bind_matrix(0.0, 0.0, 0.0)))
            .collect();
        objs.push(TNode::branch(b"Pose", pose_props, pose_nodes));
        let mut conns: Vec<TNode> = 必須骨モデル列()
            .iter()
            .filter_map(|(id, _, parent)| parent.map(|parent| conn(*id, parent)))
            .collect();
        conns.push(conn(100, 16));
        conns.push(conn(1, 0));
        conns.push(conn(500, 100));
        conns.push(conn(510, 500));
        conns.push(conn(520, 510));
        conns.push(conn(511, 500));
        conns.push(conn(521, 511));
        let fbx_bytes = file(vec![objects(objs), connections(conns)]);
        let (index, 読み手) = 索引と読み手を作る(&[
            ("Model.fbx", メタを作る(FBX_GUID), None),
            (
                "Avatar.prefab",
                prefabメタを作る(AVATAR_GUID),
                Some(改変文書を作る(FBX_GUID, "").into_bytes()),
            ),
        ]);
        let 選択 = 変換選択を作る(PrefabExpressions::None, false, PrefabPhysics::None);
        let (vrm_bytes, 報告) = convert_prefab(
            &読み手,
            &index,
            "Assets/Avatar.prefab",
            &fbx_bytes,
            &FbxOptions::new(),
            &VrmOptions::new()
                .with_morph_mode(VrmMorphMode::All)
                .with_expressions(crate::vrm::VrmExpressionProfile::vrchat()),
            書き出し設定を作る(),
            &選択,
        )
        .expect("AllモーフでもVRM変換できる");
        assert_eq!(&vrm_bytes[..4], b"glTF");
        assert_eq!(報告.morph_targets, 2);
        assert_eq!(報告.expressions, 1);
        let json_len = u32::from_le_bytes(vrm_bytes[12..16].try_into().expect("長さ")) as usize;
        let json: serde_json::Value =
            serde_json::from_slice(&vrm_bytes[20..20 + json_len]).expect("JSONは読める");
        assert_eq!(
            json["meshes"][0]["extras"]["targetNames"],
            serde_json::json!(["Shrink_Body", "vrc.v_aa"])
        );
        assert_eq!(
            json["extensions"]["VRMC_vrm"]["expressions"]["preset"]["aa"]["morphTargetBinds"][0]["index"],
            serde_json::json!(1)
        );
    }
}
