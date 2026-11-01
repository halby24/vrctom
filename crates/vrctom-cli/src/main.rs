//! UnityPackageの展開やVRMの書き出しを行う補助CLI。
//!
//! 標準出力には何も出さず、結果の要約や失敗の診断は標準エラーに出す。
//! 終了コードは成功で0、実行失敗で1、引数不正で2とする。
//! 引数不正の2は[`clap`]のエラーを可視エスケープして返す。

use std::fs::File;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand, ValueEnum};
use vrctom::fbx::{FbxFile, FbxOptions};
use vrctom::unitypackage::{UnpackOptions, unpack_to};
#[cfg(not(target_arch = "wasm32"))]
use vrctom::vrm::load_renderer_materials;
use vrctom::vrm::{
    HumanoidBones, MaterialOptions, PhysicsLimits, PhysicsSidecar, RendererApplyReport,
    RendererLimits, RendererMeshView, RendererSidecar, SpringCompileOptions, VrmAvatarPermission,
    VrmCommercialUsage, VrmCreditNotation, VrmExportOptions, VrmExpressionProfile, VrmMaterials,
    VrmMeta, VrmModel, VrmModification, VrmOptions, apply_humanoid_t_pose, apply_renderer_plan,
    compile_springs, plan_renderer_assignment, resolve_look_at,
};

#[derive(Debug, Parser)]
#[command(
    name = "vrctom",
    version,
    about = "UnityPackageの展開やVRMの書き出しを行う補助CLI"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// UnityPackageを展開する。
    Unpack {
        /// 展開する `.unitypackage` ファイル。
        package: PathBuf,
        /// 出力先ディレクトリ。既存の場所は指定できない。
        #[arg(short, long, value_name = "DIR")]
        output: Option<PathBuf>,
    },
    /// FBXからVRM 1.0を書き出す。
    Export {
        /// 読み取る `.fbx` ファイル。
        input: PathBuf,
        /// 出力先。既定は入力の拡張子を `vrm` に変えたパス。
        #[arg(short, long, value_name = "OUT")]
        output: Option<PathBuf>,
        /// VRMのmeta.name。既定は入力ファイルのstem。空は受け付けない。
        #[arg(long, value_name = "NAME")]
        name: Option<String>,
        /// VRMのmeta.authors。繰り返し指定可。既定は `unknown` 1件。
        #[arg(long, value_name = "AUTHOR")]
        author: Vec<String>,
        /// `Assets` を含むUnityプロジェクトのルート。省略時はFBXの祖先から探す。
        #[arg(long, value_name = "DIR")]
        assets: Option<PathBuf>,
        /// 表情の選択指定。`vrc` はVRChat向けの8件（あいうえお・瞬き・視線上下）を使う。
        #[arg(long, value_name = "PROFILE")]
        expressions: Option<ExpressionProfile>,
        /// 物理sidecar（JSON）のパス。指定時はスプリングをコンパイルして載せる。
        #[arg(long, value_name = "FILE")]
        physics: Option<PathBuf>,
        /// レンダラsidecar（JSON）のパス。指定時はPrefabの割当と有効状態を使う。
        #[arg(long, value_name = "FILE")]
        renderers: Option<PathBuf>,
        /// アバターの人格権の扱い。既定は `only-author`。
        #[arg(long, value_enum, value_name = "PERMISSION")]
        avatar_permission: Option<AvatarPermissionArg>,
        /// 商用利用の範囲。既定は `personal-non-profit`。
        #[arg(long, value_enum, value_name = "USAGE")]
        commercial_usage: Option<CommercialUsageArg>,
        /// クレジット表記の要否。既定は `required`。
        #[arg(long, value_enum, value_name = "NOTATION")]
        credit_notation: Option<CreditNotationArg>,
        /// 改変の許可範囲。既定は `prohibited`。
        #[arg(long, value_enum, value_name = "MODIFICATION")]
        modification: Option<ModificationArg>,
        /// 再配布を許可する。既定は不許可。
        #[arg(long)]
        allow_redistribution: bool,
        /// 過度な暴力表現での利用を許可する。既定は不許可。
        #[arg(long)]
        allow_excessively_violent_usage: bool,
        /// 過度な性的表現での利用を許可する。既定は不許可。
        #[arg(long)]
        allow_excessively_sexual_usage: bool,
        /// 政治・宗教目的の利用を許可する。既定は不許可。
        #[arg(long)]
        allow_political_or_religious_usage: bool,
        /// 反社会的・憎悪目的の利用を許可する。既定は不許可。
        #[arg(long)]
        allow_antisocial_or_hate_usage: bool,
    },
}

/// 表情の選択指定。現在はVRChat向けの8件だけを受け付ける。
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum ExpressionProfile {
    /// VRChat向けの8件（あいうえお・瞬き・視線上下）。
    Vrc,
}

/// アバター権限のCLI指定。VRM 1.0の3値をそのまま表す。
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum AvatarPermissionArg {
    /// 本人のみ利用可（Unityの既定値）。
    OnlyAuthor,
    /// 別途許諾者のみ利用可。
    OnlySeparatelyLicensedPerson,
    /// 誰でも利用可。
    Everyone,
}

impl AvatarPermissionArg {
    /// 書き出し側の列挙へ写す。
    fn into_vrm(self) -> VrmAvatarPermission {
        match self {
            Self::OnlyAuthor => VrmAvatarPermission::OnlyAuthor,
            Self::OnlySeparatelyLicensedPerson => VrmAvatarPermission::OnlySeparatelyLicensedPerson,
            Self::Everyone => VrmAvatarPermission::Everyone,
        }
    }
}

/// 商用利用のCLI指定。VRM 1.0の3値をそのまま表す。
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum CommercialUsageArg {
    /// 非営利の個人利用のみ（Unityの既定値）。
    PersonalNonProfit,
    /// 営利の個人利用まで許す。
    PersonalProfit,
    /// 法人利用まで許す。
    Corporation,
}

impl CommercialUsageArg {
    /// 書き出し側の列挙へ写す。
    fn into_vrm(self) -> VrmCommercialUsage {
        match self {
            Self::PersonalNonProfit => VrmCommercialUsage::PersonalNonProfit,
            Self::PersonalProfit => VrmCommercialUsage::PersonalProfit,
            Self::Corporation => VrmCommercialUsage::Corporation,
        }
    }
}

/// クレジット表記のCLI指定。VRM 1.0の2値をそのまま表す。
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum CreditNotationArg {
    /// 表記が必要（Unityの既定値）。
    Required,
    /// 表記が不要。
    Unnecessary,
}

impl CreditNotationArg {
    /// 書き出し側の列挙へ写す。
    fn into_vrm(self) -> VrmCreditNotation {
        match self {
            Self::Required => VrmCreditNotation::Required,
            Self::Unnecessary => VrmCreditNotation::Unnecessary,
        }
    }
}

/// 改変許可のCLI指定。VRM 1.0の3値をそのまま表す。
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum ModificationArg {
    /// 改変を禁止する（Unityの既定値）。
    Prohibited,
    /// 改変は許すが再配布は許さない。
    AllowModification,
    /// 改変と再配布を許す。
    AllowModificationRedistribution,
}

impl ModificationArg {
    /// 書き出し側の列挙へ写す。
    fn into_vrm(self) -> VrmModification {
        match self {
            Self::Prohibited => VrmModification::Prohibited,
            Self::AllowModification => VrmModification::AllowModification,
            Self::AllowModificationRedistribution => {
                VrmModification::AllowModificationRedistribution
            }
        }
    }
}

fn main() {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            use clap::error::ErrorKind;
            match error.kind() {
                // `--help`/`--version` は利用者入力を含まない定型文のため、
                // 従来どおり表示して終了コード0で終える。
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => error.exit(),
                // それ以外の引数エラーは文面全体を可視エスケープしてから
                // 標準エラーに出し、終了コード2で終える。改行も逃がすため
                // 出力は1行にまとまり、行偽装を作らない。
                _ => {
                    eprintln!("{}", escape_diag(&error.to_string()));
                    std::process::exit(2);
                }
            }
        }
    };
    let code = match cli.command {
        Command::Unpack { package, output } => unpack(package, output),
        Command::Export {
            input,
            output,
            name,
            author,
            assets,
            expressions,
            physics,
            renderers,
            avatar_permission,
            commercial_usage,
            credit_notation,
            modification,
            allow_redistribution,
            allow_excessively_violent_usage,
            allow_excessively_sexual_usage,
            allow_political_or_religious_usage,
            allow_antisocial_or_hate_usage,
        } => export(
            input,
            output,
            name,
            author,
            assets,
            expressions,
            physics,
            renderers,
            ExportMetaArgs {
                avatar_permission,
                commercial_usage,
                credit_notation,
                modification,
                allow_redistribution,
                allow_excessively_violent_usage,
                allow_excessively_sexual_usage,
                allow_political_or_religious_usage,
                allow_antisocial_or_hate_usage,
            },
        ),
    };
    std::process::exit(code);
}

/// 診断表示のための可視エスケープ。
///
/// ライブラリ側と同等に、改行・復帰・タブ・NUL・ESC・その他の制御文字（Cc）と
/// 行偽装に使える不可視・書式文字（soft hyphen・双方向マーク・ゼロ幅・行区切り・
/// タグ文字など）を可視化する。対象に `char::is_control()` が偽になる不可視文字を
/// 含むのは、制御文字検査だけでは素通りするためである。
/// `\` 自体は変えないため、逃がし済みの文面への再適用は変えない。
fn escape_diag(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\0' => out.push_str("\\0"),
            c if c.is_control() => {
                let n = c as u32;
                if n <= 0xff {
                    out.push_str(&format!("\\x{n:02x}"));
                } else {
                    out.push_str(&format!("\\u{{{n:x}}}"));
                }
            }
            // 双方向制御・ゼロ幅・不可視マーク・行区切り・タグ文字は表示順序や行を
            // 偽装し得るため逃がす。`is_control()` が偽になるものを含む。
            '\u{ad}'
            | '\u{61c}'
            | '\u{180e}'
            | '\u{200b}'..='\u{200f}'
            | '\u{2028}'..='\u{202e}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{206f}'
            | '\u{feff}'
            | '\u{fff9}'..='\u{fffb}'
            | '\u{e0001}'
            | '\u{e0020}'..='\u{e007f}' => {
                out.push_str(&format!("\\u{{{:x}}}", c as u32));
            }
            c => out.push(c),
        }
    }
    out
}

/// 終了コードは成功で0、実行失敗で1とする。引数不正の2は[`clap`]のエラーを
/// 可視エスケープして返す（[`main`] が扱う）。
fn unpack(package: PathBuf, output: Option<PathBuf>) -> i32 {
    // 既定出力先はカレントの `<入力stem>-unpacked` とする。
    let destination = match output {
        Some(dir) => dir,
        None => {
            let current = match std::env::current_dir() {
                Ok(current) => current,
                Err(error) => {
                    eprintln!(
                        "カレントディレクトリを解決できない: {}",
                        escape_diag(&error.to_string())
                    );
                    return 1;
                }
            };
            let stem = package
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
                .unwrap_or_else(|| "package".to_owned());
            current.join(format!("{stem}-unpacked"))
        }
    };
    // 展開処理やパス検証はライブラリに任せ、ここでは開いて渡すだけにする。
    let input = match File::open(&package) {
        Ok(input) => input,
        Err(error) => {
            eprintln!(
                "入力を開けない: {}: {}",
                escape_diag(&format!("{package:?}")),
                escape_diag(&error.to_string())
            );
            return 1;
        }
    };
    let report = match unpack_to(input, &destination, &UnpackOptions::default()) {
        Ok(report) => report,
        Err(error) => {
            // 診断文面は可視エスケープしてから出す。端末制御や行偽装を防ぐ。
            // 逃がし済みの文面への再適用は変えない。
            eprintln!(
                "展開に失敗した: {}: {}",
                escape_diag(&format!("{error:?}")),
                escape_diag(&format!("{error}"))
            );
            if let Some(guid) = error.guid() {
                eprintln!("guid: {}", escape_diag(&format!("{guid:?}")));
            }
            if let Some(pathname) = error.pathname() {
                eprintln!("pathname: {}", escape_diag(&format!("{pathname:?}")));
            }
            if let Some(partial) = error.partial_output() {
                eprintln!("残存し得る出力先: {}", escape_diag(&format!("{partial:?}")));
            }
            if let Some(staging) = error.partial_temp_dir() {
                eprintln!(
                    "残存し得る一時領域: {}",
                    escape_diag(&format!("{staging:?}"))
                );
            }
            if let Some(source) = std::error::Error::source(&error) {
                eprintln!("原因: {}", escape_diag(&source.to_string()));
            }
            return 1;
        }
    };
    eprintln!(
        "展開が完了した: ファイル{}件 フォルダ{}件 {}バイト: {}",
        report.file_count(),
        report.directory_count(),
        report.output_bytes(),
        escape_diag(&format!("{destination:?}"))
    );
    0
}

/// FBXからVRM 1.0を書き出す。
///
/// 終了コードは成功で0、実行失敗で1とする。引数不正の2は[`clap`]のエラーを
/// 可視エスケープして返す（[`main`] が扱う）。
/// 標準出力には何も出さず、要約と診断は標準エラーに出す。
/// 既存の出力先は上書きせず、作成の競合も失敗として1を返す。
// 引数はCLI配線の束ねであり、分割より呼び出し側の見通しを優先するため許す。
#[allow(clippy::too_many_arguments)]
fn export(
    input: PathBuf,
    output: Option<PathBuf>,
    name: Option<String>,
    authors: Vec<String>,
    assets: Option<PathBuf>,
    expressions: Option<ExpressionProfile>,
    physics: Option<PathBuf>,
    renderers: Option<PathBuf>,
    meta_args: ExportMetaArgs,
) -> i32 {
    // 既定出力先は入力の拡張子を `vrm` に変えたパスとする。
    let destination = match output {
        Some(out) => out,
        None => input.with_extension("vrm"),
    };
    // 既定の作品名は入力ファイルのstemとする。空は受け付けない。
    let meta_name = match name {
        Some(name) => name,
        None => input
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default(),
    };
    if meta_name.is_empty() {
        eprintln!("作品名が空である: {}", escape_diag(&format!("{input:?}")));
        return 1;
    }
    // 作者の既定は `unknown` 1件とする。空文字だけの指定は書き出し層の
    // 検証に任せ、ここでは素通しする。
    let meta_authors = if authors.is_empty() {
        vec!["unknown".to_owned()]
    } else {
        authors
    };
    // 大きなバイト列を保持するが、上限の判断は読み取り層に任せる。
    let bytes = match std::fs::read(&input) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!(
                "入力を読み込めない: {}: {}",
                escape_diag(&format!("{input:?}")),
                escape_diag(&error.to_string())
            );
            return 1;
        }
    };
    let file = match FbxFile::parse(&bytes, &FbxOptions::default()) {
        Ok(file) => file,
        Err(error) => {
            diagnose_stage("FBXの解析に失敗した", &error);
            return 1;
        }
    };
    let mut vrm_options = VrmOptions::new();
    if expressions == Some(ExpressionProfile::Vrc) {
        vrm_options = vrm_options.with_expressions(VrmExpressionProfile::vrchat());
    }
    let (mut model, report) = match VrmModel::from_fbx(&file, &vrm_options) {
        Ok(built) => built,
        Err(error) => {
            diagnose_stage("中間モデルの生成に失敗した", &error);
            return 1;
        }
    };
    // バインド行列の代替を使った場合は、どの経路で何件補ったか残す。
    if report.bind_cluster_models > 0 || report.bind_rest_models > 0 {
        eprintln!(
            "バインド行列の代替: Poseに無いモデルを補った（関節{}件・その他{}件）",
            report.bind_cluster_models, report.bind_rest_models
        );
    }
    // 物理sidecarの指定時は、解決とコンパイルが通ってから代入する。
    // 失敗時はモデルを部分更新しない。
    let mut spring_summary: Option<String> = None;
    if let Some(sidecar_path) = physics {
        let sidecar_bytes = match std::fs::read(&sidecar_path) {
            Ok(sidecar_bytes) => sidecar_bytes,
            Err(error) => {
                eprintln!(
                    "物理sidecarを読み込めない: {}: {}",
                    escape_diag(&format!("{sidecar_path:?}")),
                    escape_diag(&error.to_string())
                );
                return 1;
            }
        };
        let sidecar = match PhysicsSidecar::parse(&sidecar_bytes, &PhysicsLimits::new()) {
            Ok(sidecar) => sidecar,
            Err(error) => {
                diagnose_stage("物理sidecarの解析に失敗した", &error);
                return 1;
            }
        };
        let resolution = match model.resolve_physics_sidecar(&sidecar, &bytes) {
            Ok(resolution) => resolution,
            Err(error) => {
                diagnose_stage("物理sidecarの解決に失敗した", &error);
                return 1;
            }
        };
        // humanoidのrest姿勢はsidecarのT-poseを優先する。無い骨は
        // ノードの姿勢のまま残す。参照実装もリグ構築直後にT-poseを当ててから
        // 物理を解決するため、視線の測定とスプリングの生成より前に適用する。
        // 失敗時はモデルを部分更新せず打ち切る。
        let (tpose_applied, tpose_skipped) = match apply_humanoid_t_pose(
            &sidecar,
            &mut model,
            &resolution,
            sidecar.humanoid.as_ref(),
        ) {
            Ok(report) => {
                for skipped in &report.skipped {
                    eprintln!(
                        "T-ポーズのスキップ: {} {}: {}",
                        escape_diag(&skipped.bone),
                        escape_diag(&skipped.node_id),
                        skipped.reason,
                    );
                }
                (report.applied.len(), report.skipped.len())
            }
            Err(error) => {
                diagnose_stage("T-ポーズの適用に失敗した", &error);
                return 1;
            }
        };
        // 視線の解決。sidecarに視線記録が無い旧形式では何もしない。
        // 測定も既定もここで確定する。モデルは変更しないため、
        // 失敗時は部分更新なく打ち切る。humanoidの検出は鎖の生成より前に行い、
        // 平面用の補助ノードの影響を受けない。
        let look_at = match sidecar.look_at.as_ref() {
            Some(_) => {
                let humanoid = match HumanoidBones::detect(&model) {
                    Ok(humanoid) => humanoid,
                    Err(error) => {
                        diagnose_stage("視線の解決に失敗した", &error);
                        return 1;
                    }
                };
                match resolve_look_at(&sidecar, &model, &resolution, &humanoid) {
                    Ok(look_at) => look_at,
                    Err(error) => {
                        diagnose_stage("視線の解決に失敗した", &error);
                        return 1;
                    }
                }
            }
            None => None,
        };
        match compile_springs(
            &sidecar,
            &mut model,
            &resolution,
            &SpringCompileOptions::new(),
        ) {
            Ok(compiled) => {
                // 主要な近似件数と関節値の目安も出し、文書の制限と突き合わせられるようにする。
                let world = compiled
                    .report
                    .center_applied
                    .iter()
                    .filter(|entry| entry.kind == vrctom::vrm::CenterKind::World)
                    .count();
                let all_motion = compiled
                    .report
                    .center_applied
                    .iter()
                    .filter(|entry| entry.kind == vrctom::vrm::CenterKind::AllMotion)
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
                let summary = format!(
                    "スプリング{}件 関節{}件 collider{}件 グループ{}件 分岐{}件 center{}件（world{} / all-motion{} / なし{}） 近似(endpoint未生成{} gravity0関節{} first-child{} 平均近似{} 平面落下{}) 関節値(硬さ平均{:.3} 重力あり{}件) 共有{}件 スキップ{}件 制約{}件 制約スキップ{}件 T-ポーズ{}件",
                    compiled.report.spring_count,
                    compiled.report.joint_count,
                    compiled.report.collider_count,
                    compiled.report.group_count,
                    compiled.report.branch_splits.len(),
                    compiled.report.center_applied.len(),
                    world,
                    all_motion,
                    compiled.report.center_missing.len(),
                    compiled.report.no_endpoint.len(),
                    compiled.report.gravity_zero_joints,
                    compiled.report.first_child_multichild.len(),
                    compiled.report.average_approximated.len(),
                    compiled.report.plane_colliders_dropped.len(),
                    stiff_mean,
                    grav_count,
                    compiled.report.lost_nodes.len(),
                    compiled.report.skipped_bones.len(),
                    model.node_constraints.len(),
                    compiled.report.skipped_constraints.len(),
                    tpose_applied,
                );
                if tpose_skipped > 0 {
                    eprintln!("T-ポーズのスキップ: {tpose_skipped}件");
                }
                model.spring_bones = compiled.bones;
                spring_summary = Some(summary);
                // 視線は鎖の生成が通ってから代入する。失敗時はモデルを更新しない。
                if let Some((look, report)) = look_at {
                    model.look_at = Some(look);
                    eprintln!(
                        "視線: lookAt={} {} 内{:.3}度 外{:.3}度 下{:.3}度 上{:.3}度 頭基準ずれ[{:.6}, {:.6}, {:.6}]",
                        report.look_type,
                        if report.measured {
                            "測定値"
                        } else {
                            "既定値"
                        },
                        report.range.horizontal_inner,
                        report.range.horizontal_outer,
                        report.range.vertical_down,
                        report.range.vertical_up,
                        report.offset_from_head[0],
                        report.offset_from_head[1],
                        report.offset_from_head[2],
                    );
                }
            }
            Err(error) => {
                diagnose_stage("スプリングの生成に失敗した", &error);
                return 1;
            }
        }
    }
    let mut meta = VrmMeta::new(meta_name, meta_authors);
    apply_meta_args(&mut meta, meta_args);
    // 材質の読み取り。指定時はその配置、省略時はFBXの祖先の `Assets` から求める。
    let explicit_assets = assets.is_some();
    let assets_root = match assets {
        Some(dir) => Some(assets_root_of(&dir)),
        None => assets_root_from_ancestors(&input),
    };
    let use_renderers = renderers.is_some();
    // レンダラsidecarの指定時は、対応付けと除外が通ってから材質プールを読む。
    // 失敗時はモデルを部分更新しない。wasmでは未対応として終了する。
    #[cfg(target_arch = "wasm32")]
    let renderer_out: Option<(VrmMaterials, RendererApplyReport)> = {
        if use_renderers {
            eprintln!("レンダラ割当は wasm では未対応である");
            return 1;
        }
        None
    };
    #[cfg(not(target_arch = "wasm32"))]
    let renderer_out: Option<(VrmMaterials, RendererApplyReport)> = match renderers {
        Some(sidecar_path) => {
            match apply_renderer_sidecar(
                &mut model,
                &file,
                &bytes,
                &sidecar_path,
                assets_root.as_deref(),
            ) {
                Ok(out) => Some(out),
                Err(code) => return code,
            }
        }
        None => None,
    };
    let mut export_options = VrmExportOptions::new(meta);
    let mut material_count = 0usize;
    if let Some((renderer_materials, _)) = renderer_out {
        material_count = renderer_materials.entries().len();
        export_options = export_options.with_materials(renderer_materials);
    } else if let Some(root) = assets_root {
        match VrmMaterials::load(&root, &input, &MaterialOptions::new()) {
            Ok((Some(materials), report)) => {
                for warning in report.warnings() {
                    eprintln!("材質の警告: {}", escape_diag(warning));
                }
                material_count = materials.entries().len();
                export_options = export_options.with_materials(materials);
            }
            Ok((None, report)) => {
                for warning in report.warnings() {
                    eprintln!("材質の警告: {}", escape_diag(warning));
                }
            }
            Err(error) => {
                if explicit_assets {
                    diagnose_stage("材質の読み取りに失敗した", &error);
                    return 1;
                }
                eprintln!(
                    "材質の警告: {}",
                    escape_diag(&format!("自動検出の配置では材質を読まずに続ける: {error}"))
                );
            }
        }
    }
    let vrm_bytes = match model.to_vrm(&export_options) {
        Ok(vrm_bytes) => vrm_bytes,
        Err(error) => {
            diagnose_stage("VRMの書き出しに失敗した", &error);
            return 1;
        }
    };
    // 既存ファイルは上書きしない。競合も失敗として1を返す。
    let mut out = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&destination)
    {
        Ok(out) => out,
        Err(error) => {
            eprintln!(
                "出力先を作成できない: {}: {}",
                escape_diag(&format!("{destination:?}")),
                escape_diag(&error.to_string())
            );
            return 1;
        }
    };
    if let Err(error) = out.write_all(&vrm_bytes) {
        eprintln!(
            "書き込みに失敗した: {}: {}",
            escape_diag(&format!("{destination:?}")),
            escape_diag(&error.to_string())
        );
        return 1;
    }
    eprintln!(
        "書き出しが完了した: ノード{}件 メッシュ{}件 スキン{}件 材質{}件 {}バイト: {}",
        report.node_count,
        model.meshes.len(),
        report.skin_count,
        material_count,
        vrm_bytes.len(),
        escape_diag(&format!("{destination:?}"))
    );
    if !use_renderers {
        eprintln!("材質の解決: FBXの置換表経路であり、Prefab の割当・有効状態は未反映である");
    }
    if let Some(summary) = spring_summary {
        eprintln!("物理: {summary}");
    }
    0
}

/// レンダラsidecarを中間モデルへ適用し、材質プールと適用報告を返す。
///
/// sidecarの読み取り・FBXのSHA-256照合・Model名での対応付け・材質プールの
/// 読み取り・除外と割当の適用を順に行う。いずれかの段階で失敗した場合は
/// 診断を標準エラーに出し、終了コード1を返す。成功時はモデルを更新する。
/// 材質配置が無い場合は `.mat` を読めないため失敗する。
#[cfg(not(target_arch = "wasm32"))]
fn apply_renderer_sidecar(
    model: &mut VrmModel,
    file: &FbxFile<'_>,
    fbx_bytes: &[u8],
    sidecar_path: &std::path::Path,
    assets_root: Option<&std::path::Path>,
) -> Result<(VrmMaterials, RendererApplyReport), i32> {
    let sidecar_bytes = match std::fs::read(sidecar_path) {
        Ok(sidecar_bytes) => sidecar_bytes,
        Err(error) => {
            eprintln!(
                "レンダラsidecarを読み込めない: {}: {}",
                escape_diag(&format!("{sidecar_path:?}")),
                escape_diag(&error.to_string())
            );
            return Err(1);
        }
    };
    let sidecar = match RendererSidecar::parse(&sidecar_bytes, &RendererLimits::new()) {
        Ok(sidecar) => sidecar,
        Err(error) => {
            diagnose_stage("レンダラsidecarの解析に失敗した", &error);
            return Err(1);
        }
    };
    let mut views = Vec::with_capacity(model.meshes.len());
    for mesh in &model.meshes {
        let Some(owner) = file.model(mesh.model) else {
            eprintln!(
                "所有モデルがFBXに無い: {}",
                escape_diag(&format!("{:?}", mesh.model.value()))
            );
            return Err(1);
        };
        views.push(RendererMeshView {
            model_name: owner.name.as_str(),
            slot_count: mesh.material_slots.len(),
        });
    }
    let plan = match plan_renderer_assignment(&sidecar, fbx_bytes, &views) {
        Ok(plan) => plan,
        Err(error) => {
            diagnose_stage("レンダラ割当の解決に失敗した", &error);
            return Err(1);
        }
    };
    eprintln!(
        "レンダラ割当: FBX一致 {} sha256:{}",
        escape_diag(&plan.matched_model_path),
        plan.matched_model_sha256
    );
    let Some(root) = assets_root else {
        eprintln!(
            "レンダラ割当には材質配置が必要である: {}",
            escape_diag(&format!("{sidecar_path:?}"))
        );
        return Err(1);
    };
    let (materials, material_report) =
        match load_renderer_materials(root, &plan.pool, &MaterialOptions::new()) {
            Ok(loaded) => loaded,
            Err(error) => {
                diagnose_stage("レンダラ材質の読み取りに失敗した", &error);
                return Err(1);
            }
        };
    for warning in material_report.warnings() {
        eprintln!("材質の警告: {}", escape_diag(warning));
    }
    let report = match apply_renderer_plan(model, &plan, &materials) {
        Ok(report) => report,
        Err(error) => {
            diagnose_stage("レンダラ割当の適用に失敗した", &error);
            return Err(1);
        }
    };
    eprintln!(
        "レンダラ割当: 有効{}件 除外{}件（{}） 材質プール{}件",
        plan.active_count(),
        report.excluded_count,
        escape_diag(&report.excluded_names.join(", ")),
        report.pool_count
    );
    Ok((materials, report))
}

/// meta権限9項目のCLI上書き指定。指定が無い列挙は既定値のままにする。
struct ExportMetaArgs {
    avatar_permission: Option<AvatarPermissionArg>,
    commercial_usage: Option<CommercialUsageArg>,
    credit_notation: Option<CreditNotationArg>,
    modification: Option<ModificationArg>,
    allow_redistribution: bool,
    allow_excessively_violent_usage: bool,
    allow_excessively_sexual_usage: bool,
    allow_political_or_religious_usage: bool,
    allow_antisocial_or_hate_usage: bool,
}

/// `VrmMeta` にCLIの権限上書きを反映する。指定が無い項目は既定値のままにする。
fn apply_meta_args(meta: &mut VrmMeta, args: ExportMetaArgs) {
    if let Some(value) = args.avatar_permission {
        meta.avatar_permission = value.into_vrm();
    }
    if let Some(value) = args.commercial_usage {
        meta.commercial_usage = value.into_vrm();
    }
    if let Some(value) = args.credit_notation {
        meta.credit_notation = value.into_vrm();
    }
    if let Some(value) = args.modification {
        meta.modification = value.into_vrm();
    }
    meta.allow_redistribution = args.allow_redistribution;
    meta.allow_excessively_violent_usage = args.allow_excessively_violent_usage;
    meta.allow_excessively_sexual_usage = args.allow_excessively_sexual_usage;
    meta.allow_political_or_religious_usage = args.allow_political_or_religious_usage;
    meta.allow_antisocial_or_hate_usage = args.allow_antisocial_or_hate_usage;
}

/// 明示指定の材質配置から走査元を求める。末尾が `Assets` ならその親を使う。
fn assets_root_of(dir: &Path) -> PathBuf {
    if dir.file_name().is_some_and(|name| name == "Assets") {
        match dir.parent() {
            // `Path::new("Assets").parent()` は空パスを返すため、そのまま返すと
            // 後段の正規化に失敗する。カレントディレクトリを指すものとして扱う。
            Some(parent) if parent.as_os_str().is_empty() => PathBuf::from("."),
            Some(parent) => parent.to_path_buf(),
            None => dir.to_path_buf(),
        }
    } else {
        dir.to_path_buf()
    }
}

/// FBXの祖先で最初に見つかった `Assets` の親を求める。無ければ `None` である。
fn assets_root_from_ancestors(input: &Path) -> Option<PathBuf> {
    input.ancestors().find_map(|ancestor| {
        // 相対パスの末端の祖先は空パスになり、そのまま返すと材質読み取りの
        // 正規化に失敗するため、カレントディレクトリを基準にする。
        if ancestor.as_os_str().is_empty() {
            return std::env::current_dir()
                .ok()
                .filter(|current| current.join("Assets").is_dir());
        }
        if ancestor.join("Assets").is_dir() {
            Some(ancestor.to_path_buf())
        } else {
            None
        }
    })
}

/// 書き出し各段階の失敗を1行にまとめて診断に出す。
///
/// 本体は可視エスケープし、`source` がある場合は原因も併記する。
fn diagnose_stage<E>(stage: &str, error: &E)
where
    E: std::error::Error,
{
    eprintln!(
        "{stage}: {}: {}",
        escape_diag(&format!("{error:?}")),
        escape_diag(&format!("{error}"))
    );
    if let Some(source) = std::error::Error::source(error) {
        eprintln!("原因: {}", escape_diag(&source.to_string()));
    }
}

#[cfg(test)]
mod tests {
    /// テスト名は日本語で書く（テストは仕様・契約であるため）。

    #[test]
    fn 存在しない入力は実行失敗になる() {
        let code = super::unpack(
            std::path::PathBuf::from("存在しない入力.unitypackage"),
            Some(std::env::temp_dir().join("vrctom-cli-存在しない出力")),
        );
        assert_eq!(code, 1);
    }

    #[test]
    fn 診断の可視エスケープは生の制御文字を残さない() {
        // 改行・ESC・NULが可視化され、日本語は保つ。再適用は変えない。
        let escaped = super::escape_diag("a\nb\x1bc\0dあ");
        assert_eq!(escaped, "a\\nb\\x1bc\\0dあ");
        assert_eq!(super::escape_diag(&escaped), escaped);
    }

    #[test]
    fn 診断の可視エスケープは不可視書式文字も逃がす() {
        // U+061C・U+2060・U+2028・U+2029 などは `is_control()` が偽になるため、
        // 制御文字だけの検査では素通りして CLI の stderr に生のUTF-8が残る。
        // 回帰として残す。
        for raw in ['\u{61c}', '\u{2060}', '\u{2028}', '\u{2029}'] {
            assert!(
                !raw.is_control(),
                "U+{:04X} は制御文字検査では捉えられない",
                raw as u32
            );
        }
        // stderr 経路はこの関数で逃がしてから出すため、ここでの可視化が残留防止になる。
        let escaped = super::escape_diag("a\u{61c}b\u{2060}c\u{2028}d\u{2029}eあ");
        assert_eq!(escaped, "a\\u{61c}b\\u{2060}c\\u{2028}d\\u{2029}eあ");
        for raw in ['\u{61c}', '\u{2060}', '\u{2028}', '\u{2029}'] {
            assert!(!escaped.contains(raw), "生の不可視文字が残らない");
        }
        assert_eq!(super::escape_diag(&escaped), escaped);
    }
}
