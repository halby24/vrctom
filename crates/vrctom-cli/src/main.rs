//! UnityPackageの展開やVRMの書き出しを行う補助CLI。
//!
//! 標準出力には何も出さず、結果の要約や失敗の診断は標準エラーに出す。
//! 終了コードは成功で0、実行失敗で1、引数不正で2とする。
//! 引数不正の2は[`clap`]のエラーを可視エスケープして返す。

use std::fs::File;
use std::io::Read as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand, ValueEnum};
use vrctom::conversion::{ConversionError, build_model, export_model};
use vrctom::fbx::{FbxFile, FbxOptions};
use vrctom::unity::{AssetMeta, HumanBoneMapping, MAX_META_BYTES, ParseOptions, SerializedFile};
use vrctom::unitypackage::{UnpackOptions, unpack_to};
#[cfg(not(target_arch = "wasm32"))]
use vrctom::vrm::load_renderer_materials;
use vrctom::vrm::{
    HumanoidBone, HumanoidBones, MaterialError, MaterialOptions, MaterialReport, PhysicsLimits,
    PhysicsSidecar, RendererApplyReport, RendererLimits, RendererMeshView, RendererSidecar,
    SpringCompileOptions, VrmAvatarPermission, VrmCommercialUsage, VrmCreditNotation,
    VrmExportOptions, VrmExpressionProfile, VrmMaterials, VrmMeta, VrmModel, VrmModification,
    VrmOptions, apply_humanoid_t_pose, apply_renderer_plan, compile_springs,
    expressions_from_avatar_descriptor, plan_renderer_assignment, prefab_baseline_weights,
    resolve_look_at,
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
        /// 表情の選択指定。`vrc` はVRChat向けの8種類の対応から見つかったものを生成する。
        /// `descriptor` はPrefabの `VRCAvatarDescriptor` から既定生成する。
        /// `--prefab` 指定時は `--no-expressions` が無い限り `descriptor` と同じになる。
        #[arg(long, value_name = "PROFILE", requires_if("descriptor", "prefab"))]
        expressions: Option<ExpressionProfile>,
        /// 検証用: Unity採取の物理sidecar（JSON）。比較のための入力であり、通常は使わない。
        /// 既定では `--prefab` から物理を導出する。
        #[arg(long, value_name = "FILE")]
        physics: Option<PathBuf>,
        /// レンダラsidecar（JSON）のパス。指定時はPrefabの割当と有効状態を使う。
        #[arg(long, value_name = "FILE", conflicts_with = "prefab")]
        renderers: Option<PathBuf>,
        /// Prefabのパス。指定時はPrefabの改変内容または直接記述から割当と有効状態を使う。
        #[arg(long, value_name = "FILE")]
        prefab: Option<PathBuf>,
        /// 物理sidecarを `--prefab` から導出する。現在は既定であり、互換のため受け付ける。
        #[arg(long, requires = "prefab")]
        physics_from_prefab: bool,
        /// 物理を載せない。`--prefab` 指定時の導出を止める。
        #[arg(long, conflicts_with_all = ["physics", "physics_from_prefab"])]
        no_physics: bool,
        /// 表情を作らない。`--prefab` 指定時のdescriptor由来の既定生成を止める。
        /// `--expressions` とは同時に指定できない。
        #[arg(long, conflicts_with = "expressions")]
        no_expressions: bool,
        /// 基準姿勢の焼き込みを行わない。`--prefab` 必須である。
        /// 既定ではPrefabの `SkinnedMeshRenderer.m_BlendShapeWeights` を
        /// ベース形状へ焼き込む。`--no-expressions` でも焼く。
        #[arg(long, requires = "prefab")]
        no_baseline: bool,
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

/// 表情の選択指定。`vrc` は固定対応、`descriptor` はPrefab由来の既定生成である。
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum ExpressionProfile {
    /// VRChat向けの8種類の対応から見つかったものを生成する。
    Vrc,
    /// Prefabの `VRCAvatarDescriptor` から既定生成する。`--prefab` が必須である。
    Descriptor,
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
            prefab,
            physics_from_prefab: _,
            no_physics,
            no_expressions,
            no_baseline,
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
            prefab,
            no_physics,
            no_expressions,
            no_baseline,
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
    prefab: Option<PathBuf>,
    no_physics: bool,
    no_expressions: bool,
    no_baseline: bool,
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
    let mut vrm_options = VrmOptions::new();
    if expressions == Some(ExpressionProfile::Vrc) {
        vrm_options = vrm_options.with_expressions(VrmExpressionProfile::vrchat());
    }
    // descriptor由来の表情は `--expressions vrc` より弱い。`vrc` 指定時は
    // `--prefab` があっても従来どおり固定対応を優先する。
    let use_descriptor_expressions = expressions == Some(ExpressionProfile::Descriptor)
        || (expressions.is_none() && prefab.is_some() && !no_expressions);
    // 材質配置の解決は物理導出（Prefabからのsidecar生成）でも使うため、物理より先に確定させる。
    // 指定時はその配置、省略時はFBXの祖先の `Assets` から求める。
    // いずれも出力を持たない純粋な算出であり、診断順序に影響しない。
    let explicit_assets = assets.is_some();
    let assets_root = match assets {
        Some(dir) => Some(assets_root_of(&dir)),
        None => assets_root_from_ancestors(&input),
    };
    // 物理sidecar・Prefab・レンダラsidecarのいずれも使わない通常経路は、
    // 共有の変換入口に寄せる。読み取りは `convert_fbx` に一度だけ渡す。
    // 特殊経路はモデルを物理・割当のために保持する必要があるため、既存APIのまま残す。
    if physics.is_none() && renderers.is_none() && prefab.is_none() {
        return export_simple(
            &input,
            destination,
            meta_name,
            meta_authors,
            assets_root,
            explicit_assets,
            &bytes,
            &vrm_options,
            meta_args,
        );
    }
    let file = match FbxFile::parse(&bytes, &FbxOptions::default()) {
        Ok(file) => file,
        Err(error) => {
            diagnose_stage("FBXの解析に失敗した", &error);
            return 1;
        }
    };
    // descriptor由来の表情と基準姿勢の焼き込みはFBX解析後・中間モデル生成前に解決する。
    // Prefabの読み込みと `.fbx.meta`（GUID・humanoid対応表）の読み込みは、
    // descriptorまたは基準姿勢のどちらかが必要な場合に一度だけ行う。
    // `.fbx.meta` の対応表は後段のhumanoid検出と共有し、二重に読まない。
    // 材質割当が既にメタを要求するため、`--prefab` でメタが必要なのは退行ではないが、
    // `--prefab --no-expressions` でも基準姿勢のためにメタを読む点で診断順が早まる。
    let use_baseline = prefab.is_some() && !no_baseline;
    let mut early_mappings: Option<Vec<HumanBoneMapping>> = None;
    if use_descriptor_expressions || use_baseline {
        let Some(prefab_path) = prefab.as_ref() else {
            eprintln!("--expressions descriptor には --prefab が必須である");
            return 2;
        };
        let prefab_label = format!("{prefab_path:?}");
        let prefab_bytes = match std::fs::read(prefab_path) {
            Ok(prefab_bytes) => prefab_bytes,
            Err(error) => {
                eprintln!(
                    "Prefabを読み込めない: {}: {}",
                    escape_diag(&prefab_label),
                    escape_diag(&error.to_string())
                );
                return 1;
            }
        };
        let prefab_file = match SerializedFile::parse(&prefab_bytes, &ParseOptions::new()) {
            Ok(prefab_file) => prefab_file,
            Err(error) => {
                diagnose_stage("Prefabの解析に失敗した", &error);
                return 1;
            }
        };
        let fbx_meta = match load_fbx_meta(&input) {
            Ok(fbx_meta) => fbx_meta,
            Err(code) => return code,
        };
        // Prefabの解決にはFBXのGUIDが必要である。無い場合は後段でも失敗するため、
        // ここで明確に止める。全ゼロは未設定とみなす。用途が分かる文面にする。
        if fbx_meta.guid.chars().all(|c| c == '0') {
            let mut meta_os = input.as_os_str().to_owned();
            meta_os.push(".meta");
            let purpose = match (use_descriptor_expressions, use_baseline) {
                (true, true) => "Prefab表情と基準姿勢にはFBXのGUIDが必要である",
                (true, false) => "Prefab表情にはFBXのGUIDが必要である",
                _ => "Prefab基準姿勢にはFBXのGUIDが必要である",
            };
            eprintln!(
                "{purpose}: {}",
                escape_diag(&format!("{:?}", PathBuf::from(meta_os)))
            );
            return 1;
        }
        let mappings = fbx_meta.mappings;
        let fbx_guid = fbx_meta.guid;
        if use_descriptor_expressions {
            // 目条件はdescriptorの左右Eyeではなく、humanoidとして左右Eyeが
            // 解決できることで判定する。モデル構築前に名前列で確かめる。
            // 検出自体に失敗した場合は偽にし、後段の通常検出がエラーを出す。
            let names: Vec<&str> = file
                .models()
                .iter()
                .map(|model| model.name.as_str())
                .collect();
            let humanoid_eyes = match HumanoidBones::detect_with_mapping_names(&names, &mappings) {
                Ok((bones, _)) => {
                    bones.get(HumanoidBone::LeftEye).is_some()
                        && bones.get(HumanoidBone::RightEye).is_some()
                }
                Err(_) => false,
            };
            match expressions_from_avatar_descriptor(
                &prefab_file,
                &prefab_label,
                &file,
                &fbx_guid,
                humanoid_eyes,
            ) {
                Ok(plan) => {
                    for warning in &plan.warnings {
                        eprintln!("表情の警告: {}", escape_diag(warning));
                    }
                    if !plan.profile.bindings.is_empty() {
                        let count = plan.profile.bindings.len();
                        vrm_options = vrm_options.with_expressions(plan.profile);
                        if let Some(mesh) = plan.mesh {
                            vrm_options = vrm_options.with_expression_mesh(mesh);
                        }
                        eprintln!("表情: VRCAvatarDescriptor から{count}件");
                    }
                }
                Err(error) => {
                    diagnose_stage("表情の解決に失敗した", &error);
                    return 1;
                }
            }
        }
        // 基準姿勢の焼き込みは表情の有無に関わらず行う。`--no-expressions` でも焼く。
        // `--prefab` が無い場合は重みの出所が無いため焼かない。
        // `--no-baseline` で明示的に止められる。空の解決結果は何も焼かない。
        // Prefabとメタは上で一度だけ読んだものを使い、二重に読まない。
        if use_baseline {
            match prefab_baseline_weights(&prefab_file, &prefab_label, &file, &fbx_guid) {
                Ok(weights) => {
                    if !weights.meshes.is_empty() {
                        vrm_options = vrm_options.with_baseline_weights(weights);
                    }
                }
                Err(error) => {
                    diagnose_stage("基準姿勢の解決に失敗した", &error);
                    return 1;
                }
            }
        }
        early_mappings = Some(mappings);
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
    emit_model_warnings(&report);
    // 基準姿勢を焼き込んだ場合は、成功要約の前に診断を1行出す。
    // 見出しは `emit_model_warnings` の後、成功要約より前である。
    if report.baseline_shapes > 0 {
        eprintln!(
            "ベースライン: Prefabの基準姿勢から{}件を焼き込んだ（{}メッシュ）",
            report.baseline_shapes, report.baseline_meshes
        );
    }
    // 物理適用時はhumanoid検出を解決より先に行い、明示指定で引き継ぐ。
    // 解決で追加する補助ノードはFBXの骨と同名のノードを含むことがあり、
    // 書き出し時の名前基準の検出が重複で失敗するためである。
    // 入力FBXの隣の `.fbx.meta` があれば対応表を優先し、無ければ名前推測になる。
    // 物理・Prefabを使わない `--renderers` だけの経路でも書き出しで同じ対応を使う。
    // 検出の失敗は握り潰さず、そのまま終了コード1で返す。
    // descriptor経路では対応表を既に読んでいるため再利用する。
    let mappings = match early_mappings {
        Some(mappings) => mappings,
        None => match load_humanoid_mappings(&input) {
            Ok(mappings) => mappings,
            Err(code) => return code,
        },
    };
    let (detected, humanoid_warnings) = match HumanoidBones::detect_with_mapping(&model, &mappings)
    {
        Ok(resolved) => resolved,
        Err(error) => {
            diagnose_stage("humanoidの対応付けに失敗した", &error);
            return 1;
        }
    };
    for warning in &humanoid_warnings {
        eprintln!("humanoidの警告: {}", escape_diag(warning));
    }
    let explicit_humanoid = detected;
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
        match apply_physics_sidecar(&mut model, &sidecar, &bytes, &explicit_humanoid) {
            Ok(summary) => {
                spring_summary = Some(summary);
            }
            Err(code) => return code,
        }
    }
    // `--physics` の指定が無く `--no-physics` でもなければ、Prefabから物理sidecarを導出して
    // 同じく適用する（既定）。採取メタ以外のフィールド単位の一致を目標にし、
    // レポートに導出元を出す。失敗時は型判定不能・参照解決不能・旧FBX方式として明示的に失敗する。
    else if !no_physics && let Some(prefab_path) = prefab.as_ref() {
        match derive_physics_from_prefab(
            &mut model,
            &file,
            &bytes,
            prefab_path,
            &input,
            assets_root.as_deref(),
            &explicit_humanoid,
        ) {
            Ok(summary) => {
                spring_summary = Some(summary);
                eprintln!("物理: Prefab から導出");
            }
            Err(code) => return code,
        }
    }
    let mut meta = VrmMeta::new(meta_name, meta_authors);
    apply_meta_args(&mut meta, meta_args);
    // 材質の読み取りは上で確定した `assets_root` を使う。明示指定の有無は警告の扱いに使う。
    let use_assignment = renderers.is_some() || prefab.is_some();
    // レンダラsidecar・Prefabの指定時は、対応付けと除外が通ってから材質プールを読む。
    // 失敗時はモデルを部分更新しない。wasmでは未対応として終了する。
    #[cfg(target_arch = "wasm32")]
    let assignment_out: Option<(VrmMaterials, RendererApplyReport)> = {
        if use_assignment {
            eprintln!("レンダラ・Prefab割当は wasm では未対応である");
            return 1;
        }
        None
    };
    #[cfg(not(target_arch = "wasm32"))]
    let assignment_out: Option<(VrmMaterials, RendererApplyReport)> = match (renderers, prefab) {
        (Some(sidecar_path), None) => {
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
        (None, Some(prefab_path)) => {
            match apply_prefab_assignment(
                &mut model,
                &file,
                &prefab_path,
                &input,
                assets_root.as_deref(),
            ) {
                Ok(out) => Some(out),
                Err(code) => return code,
            }
        }
        (None, None) => None,
        (Some(_), Some(_)) => {
            eprintln!("--renderers と --prefab は同時に指定できない");
            return 2;
        }
    };
    let mut export_options = VrmExportOptions::new(meta);
    let mut material_count = 0usize;
    // 物理適用時に検出したhumanoidは明示指定で引き継ぐ。
    // 補助ノードの同名ノードを避け、解決前の対応付けを保つ。
    export_options = export_options.with_humanoid(explicit_humanoid);
    if let Some((assigned_materials, _)) = assignment_out {
        material_count = assigned_materials.entries().len();
        export_options = export_options.with_materials(assigned_materials);
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
    // 最終書き出しは共有入口を使い、通常経路と診断を共通化する。
    let vrm_bytes = match export_model(&model, &export_options) {
        Ok(vrm_bytes) => vrm_bytes,
        Err(error) => {
            diagnose_conversion(&error);
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
        "書き出しが完了した: ノード{}件 メッシュ{}件 スキン{}件 材質{}件 表情{}件 {}バイト: {}",
        report.node_count,
        model.meshes.len(),
        report.skin_count,
        material_count,
        model.expressions.preset.len(),
        vrm_bytes.len(),
        escape_diag(&format!("{destination:?}"))
    );
    if !use_assignment {
        eprintln!("材質の解決: FBXの置換表経路であり、Prefab の割当・有効状態は未反映である");
    }
    if let Some(summary) = spring_summary {
        eprintln!("物理: {summary}");
    }
    0
}

/// 通常経路（物理sidecar・Prefab・レンダラsidecarのいずれも使わない場合）の書き出し。
///
/// 中間モデルを先に作ってバインド代替の報告を残し、材質は所有権ごと
/// 書き出し設定へ移してから書き出す。表示は診断順序（FBX・中間モデルの
/// 失敗を先に返し、成功時はバインド代替・humanoid警告・材質警告の順に出す）を保つ。
/// 明示の材質失敗は書き出し失敗より先に返す。
/// 終了コード・要約・警告の意味は既存どおりである。
// 引数はCLI配線の束ねであり、分割より呼び出し側の見通しを優先するため許す。
#[allow(clippy::too_many_arguments)]
fn export_simple(
    input: &Path,
    destination: PathBuf,
    meta_name: String,
    meta_authors: Vec<String>,
    assets_root: Option<PathBuf>,
    explicit_assets: bool,
    bytes: &[u8],
    vrm_options: &VrmOptions,
    meta_args: ExportMetaArgs,
) -> i32 {
    let mut meta = VrmMeta::new(meta_name, meta_authors);
    apply_meta_args(&mut meta, meta_args);
    // 材質の読み取りだけ先に済ませ、表示は変換後に回す。
    // 読み取り自体に副作用は無く、変換に渡す設定へ反映するだけである。
    let loaded = assets_root
        .as_deref()
        .map(|root| VrmMaterials::load(root, input, &MaterialOptions::new()));
    // 中間モデルを先に作り、バインド代替の報告を残す。
    // FBX・中間モデルの失敗は材質表示より先に返す。
    // 従来は材質読み取りの前に返すためである。
    let built = match build_model(bytes, &FbxOptions::default(), vrm_options) {
        Ok(built) => built,
        Err(error) => {
            diagnose_conversion(&error);
            return 1;
        }
    };
    // バインド行列の代替を使った場合は、どの経路で何件補ったか残す。
    // 書き出しの成否に関わらず出すため、書き出しより先に表示する。
    if built.report.bind_cluster_models > 0 || built.report.bind_rest_models > 0 {
        eprintln!(
            "バインド行列の代替: Poseに無いモデルを補った（関節{}件・その他{}件）",
            built.report.bind_cluster_models, built.report.bind_rest_models
        );
    }
    emit_model_warnings(&built.report);
    // 明示の材質失敗は書き出し失敗より先に返す。
    // 従来は材質読み取りが書き出しより先のためである。
    if let Some(Err(load_error)) = &loaded
        && explicit_assets
    {
        diagnose_stage("材質の読み取りに失敗した", load_error);
        return 1;
    }
    // 材質と報告を分離し、材質は所有権ごと書き出し設定へ移す。
    // `VrmMaterials` は画像データ全体を所有するため、複製は画像総量に
    // 比例した複製になる。警告表示は報告側を使う。
    let mut export_options = VrmExportOptions::new(meta);
    let mut material_count = 0usize;
    let material_outcome: Option<Result<MaterialReport, MaterialError>> = match loaded {
        Some(Ok((materials, report))) => {
            if let Some(materials) = materials {
                material_count = materials.entries().len();
                export_options = export_options.with_materials(materials);
            }
            Some(Ok(report))
        }
        Some(Err(error)) => Some(Err(error)),
        None => None,
    };
    // 入力FBXの隣の `.fbx.meta` があれば対応表を優先し、無ければ名前推測になる。
    // 空の対応表では `detect` と同じ結果になる。`with_humanoid` は完全な明示指定で
    // 自動補完しないため、ここで解決した対応をそのまま渡す。
    let mappings = match load_humanoid_mappings(input) {
        Ok(mappings) => mappings,
        Err(code) => {
            emit_material_outcome_ref(&material_outcome, explicit_assets);
            return code;
        }
    };
    match HumanoidBones::detect_with_mapping(&built.model, &mappings) {
        Ok((humanoid, warnings)) => {
            for warning in &warnings {
                eprintln!("humanoidの警告: {}", escape_diag(warning));
            }
            export_options = export_options.with_humanoid(humanoid);
        }
        Err(error) => {
            emit_material_outcome_ref(&material_outcome, explicit_assets);
            diagnose_stage("humanoidの対応付けに失敗した", &error);
            return 1;
        }
    }
    let vrm_bytes = match export_model(&built.model, &export_options) {
        Ok(vrm_bytes) => vrm_bytes,
        Err(error) => {
            // 自動検出の失敗・警告は残し、書き出し失敗を返す。
            emit_material_outcome_ref(&material_outcome, explicit_assets);
            diagnose_conversion(&error);
            return 1;
        }
    };
    emit_material_outcome_ref(&material_outcome, explicit_assets);
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
        "書き出しが完了した: ノード{}件 メッシュ{}件 スキン{}件 材質{}件 表情{}件 {}バイト: {}",
        built.report.node_count,
        built.report.mesh_count,
        built.report.skin_count,
        material_count,
        built.model.expressions.preset.len(),
        vrm_bytes.len(),
        escape_diag(&format!("{destination:?}"))
    );
    eprintln!("材質の解決: FBXの置換表経路であり、Prefab の割当・有効状態は未反映である");
    0
}

/// 中間モデル生成の警告表示。
///
/// 通常経路と特殊経路の両方から呼び出す。書き出しより前に出し、
/// 後段で失敗してもモデル警告は残る。
fn emit_model_warnings(report: &vrctom::vrm::VrmReport) {
    for warning in &report.warnings {
        eprintln!("中間モデルの警告: {}", escape_diag(warning));
    }
}

/// 材質読み取り結果の警告表示。
///
/// 成功時の警告と自動検出の失敗警告を出す。明示指定の失敗は呼び出し側が
/// 段階診断で返すため、ここでは何も出さない。
fn emit_material_outcome_ref(
    outcome: &Option<Result<MaterialReport, MaterialError>>,
    explicit_assets: bool,
) {
    match outcome {
        Some(Ok(report)) => {
            for warning in report.warnings() {
                eprintln!("材質の警告: {}", escape_diag(warning));
            }
        }
        Some(Err(error)) if !explicit_assets => {
            eprintln!(
                "材質の警告: {}",
                escape_diag(&format!("自動検出の配置では材質を読まずに続ける: {error}"))
            );
        }
        Some(Err(_)) | None => {}
    }
}

/// 入力FBXの隣の `.fbx.meta` から読んだ情報。
///
/// GUIDはPrefab内の参照の出所検証に、対応表はhumanoid解決に使う。
/// いずれも同じ読み取りで求め、二重に読まない。
struct FbxMetaInfo {
    /// FBXアセットのGUID（小文字hex）。
    guid: String,
    /// humanoid対応表。メタが無い場合は空である。
    mappings: Vec<HumanBoneMapping>,
}

/// 入力FBXの隣の `.fbx.meta` からGUIDとhumanoid対応表を読む。
///
/// メタのパスは入力パスへ文字列 `.meta` を追加して求める。`with_extension` は
/// 使わない（`.fbx` の拡張子を保ったまま隣のファイルを指定するためである）。
/// `display()` 経由の文字列組み立ては区切りや非UTF-8の扱いを変え得るため、
/// OS文字列へ直接追加する。メタが無い場合だけGUIDを空・対応表を空として
/// 名前推測に任せ、権限不足などの読み取り失敗・上限超過・形状不良は
/// 診断を出して終了コード1で返す。
fn load_fbx_meta(input: &Path) -> Result<FbxMetaInfo, i32> {
    let mut meta_os = input.as_os_str().to_owned();
    meta_os.push(".meta");
    let meta_path = PathBuf::from(meta_os);
    let meta_label = escape_diag(&format!("{meta_path:?}"));
    // 上限+1バイトまでの制限付き読み取りにし、超過は読み取り層の上限エラーへつなぐ。
    // 全体を読み切ってから拒否しない。`NotFound` だけ空にして名前推測に任せる。
    let file = match File::open(&meta_path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(FbxMetaInfo {
                guid: String::new(),
                mappings: Vec::new(),
            });
        }
        Err(error) => {
            eprintln!(
                "humanoidの対応表を読めない: {meta_label}: {}",
                escape_diag(&error.to_string())
            );
            return Err(1);
        }
    };
    let mut bytes = Vec::new();
    if let Err(error) = file.take(MAX_META_BYTES + 1).read_to_end(&mut bytes) {
        eprintln!(
            "humanoidの対応表を読めない: {meta_label}: {}",
            escape_diag(&error.to_string())
        );
        return Err(1);
    }
    // 超過時は切り詰めで上限+1バイトになり、読み取り層が上限超過で拒否する。
    let meta = match AssetMeta::parse(&bytes) {
        Ok(meta) => meta,
        Err(error) => {
            diagnose_stage(&format!("humanoidの対応表を読めない: {meta_label}"), &error);
            return Err(1);
        }
    };
    let guid = meta.guid().as_str().to_owned();
    match meta.human_bone_mappings() {
        Ok(mappings) => Ok(FbxMetaInfo { guid, mappings }),
        Err(error) => {
            diagnose_stage(&format!("humanoidの対応表を読めない: {meta_label}"), &error);
            Err(1)
        }
    }
}

/// 入力FBXの隣の `.fbx.meta` からhumanoid対応表を読む。
///
/// GUIDが要らない経路のための取り出し口であり、読み取りは [`load_fbx_meta`] と同じである。
/// メタが無い場合だけ空の対応表として名前推測に任せる。
fn load_humanoid_mappings(input: &Path) -> Result<Vec<HumanBoneMapping>, i32> {
    load_fbx_meta(input).map(|info| info.mappings)
}

/// 共有変換入口の失敗を既存の段階文面で診断に出す。
///
/// 見出しは従来の各段階（`FbxFile::parse`・`VrmModel::from_fbx`・`to_vrm`）と
/// 同じにし、本体と `source` の表示は各エラーの既存表示を保つ。
fn diagnose_conversion(error: &ConversionError) {
    match error {
        ConversionError::Fbx(inner) => diagnose_stage("FBXの解析に失敗した", inner),
        ConversionError::Model(inner) => diagnose_stage("中間モデルの生成に失敗した", inner),
        ConversionError::Export(inner) => diagnose_stage("VRMの書き出しに失敗した", inner),
    }
}

/// レンダラsidecarを中間モデルへ適用し、材質プールと適用報告を返す。
///
/// sidecarの読み取り・FBXのSHA-256照合・Model名での対応付け・材質プールの
/// 読み取り・除外と割当の適用を順に行う。いずれかの段階で失敗した場合は
/// 診断を標準エラーに出し、終了コード1を返す。成功時はモデルを更新する。
/// 材質配置が無い場合は `.mat` を読めないため失敗する。
/// レンダラsidecarを中間モデルへ適用し、材質プールと適用報告を返す。
///
/// sidecarの読み取り・FBXのSHA-256照合・Model名での対応付け・材質プールの
/// 読み取り・除外と割当の適用を順に行う。いずれかの段階で失敗した場合は
/// 診断を標準エラーに出し、終了コード1を返す。成功時はモデルを更新する。
/// 材質配置が無い場合は `.mat` を読めないため失敗する。
#[cfg(not(target_arch = "wasm32"))]
fn derive_physics_from_prefab(
    model: &mut VrmModel,
    file: &FbxFile<'_>,
    fbx_bytes: &[u8],
    prefab_path: &std::path::Path,
    input: &std::path::Path,
    assets_root: Option<&std::path::Path>,
    humanoid: &HumanoidBones,
) -> Result<String, i32> {
    use vrctom::unity::{ParseOptions, SerializedFile};
    use vrctom::vrm::derive_physics_sidecar;
    let prefab_label = format!("{prefab_path:?}");
    let prefab_bytes = match std::fs::read(prefab_path) {
        Ok(prefab_bytes) => prefab_bytes,
        Err(error) => {
            eprintln!(
                "Prefabを読み込めない: {}: {}",
                escape_diag(&prefab_label),
                escape_diag(&error.to_string())
            );
            return Err(1);
        }
    };
    let prefab = match SerializedFile::parse(&prefab_bytes, &ParseOptions::new()) {
        Ok(prefab) => prefab,
        Err(error) => {
            diagnose_stage("Prefabの解析に失敗した", &error);
            return Err(1);
        }
    };
    let Some(root) = assets_root else {
        eprintln!(
            "Prefabからの物理導出には材質配置が必要である: {}",
            escape_diag(&prefab_label)
        );
        return Err(1);
    };
    // 索引・正規化は材質経路と同じ手順を使う。FBXの資産相対パスを求める。
    let canonical_root = match std::fs::canonicalize(root) {
        Ok(canonical_root) => canonical_root,
        Err(error) => {
            eprintln!(
                "材質の走査元を解決できない: {}: {}",
                escape_diag(&format!("{root:?}")),
                escape_diag(&error.to_string())
            );
            return Err(1);
        }
    };
    let canonical_assets = match std::fs::canonicalize(canonical_root.join("Assets")) {
        Ok(canonical_assets) => canonical_assets,
        Err(error) => {
            eprintln!(
                "材質のAssetsを解決できない: {}: {}",
                escape_diag(&format!("{:?}", canonical_root.join("Assets"))),
                escape_diag(&error.to_string())
            );
            return Err(1);
        }
    };
    let canonical_fbx = match std::fs::canonicalize(input) {
        Ok(canonical_fbx) => canonical_fbx,
        Err(error) => {
            eprintln!(
                "FBXを解決できない: {}: {}",
                escape_diag(&format!("{input:?}")),
                escape_diag(&error.to_string())
            );
            return Err(1);
        }
    };
    let relative = canonical_fbx
        .strip_prefix(&canonical_assets)
        .ok()
        .and_then(|path| {
            path.components()
                .map(|component| component.as_os_str().to_str().map(str::to_owned).ok_or(()))
                .collect::<Result<Vec<_>, _>>()
                .ok()
        })
        .map(|parts| parts.join("/").replace('\\', "/"));
    let Some(relative) = relative else {
        eprintln!(
            "Prefab割当のFBXがAssets配下にない: {}",
            escape_diag(&format!("{input:?}"))
        );
        return Err(1);
    };
    let fbx_asset_path = format!("Assets/{relative}");
    let fbx_asset_owned = fbx_asset_path.clone();
    let input_ref = vrctom::vrm::PrefabDeriveInput {
        fbx_bytes,
        fbx: file,
        assets_root: root,
        prefab: &prefab,
        prefab_label: &prefab_label,
        fbx_asset_path: &fbx_asset_owned,
    };
    let derived = match derive_physics_sidecar(&input_ref) {
        Ok(derived) => derived,
        Err(error) => {
            diagnose_stage("Prefabからの物理導出に失敗した", &error);
            return Err(1);
        }
    };
    for ignored in &derived.ignored_types {
        eprintln!("物理導出の無視: {}", escape_diag(ignored));
    }
    if !derived.unmatched_nodes.is_empty() {
        eprintln!(
            "物理導出の未対応{}件: {}",
            derived.unmatched_nodes.len(),
            derived
                .unmatched_nodes
                .iter()
                .map(|name| escape_diag(name))
                .collect::<Vec<_>>()
                .join(", "),
        );
    }
    // 検証用に導出sidecarを保存する。空でなければ書き出す。
    if let Ok(dump_path) = std::env::var("VRCTOM_PHYSICS_DUMP")
        && !dump_path.is_empty()
        && let Err(error) = std::fs::write(&dump_path, &derived.json)
    {
        eprintln!(
            "導出sidecarを保存できない: {}: {}",
            escape_diag(&dump_path),
            escape_diag(&error.to_string())
        );
        return Err(1);
    }
    let sidecar = match PhysicsSidecar::parse(&derived.json, &PhysicsLimits::new()) {
        Ok(sidecar) => sidecar,
        Err(error) => {
            diagnose_stage("導出した物理sidecarの解析に失敗した", &error);
            return Err(1);
        }
    };
    apply_physics_sidecar(model, &sidecar, fbx_bytes, humanoid)
}

/// 解決済みの物理sidecarを中間モデルへ適用し、要約文を返す。
///
/// 解決・T-pose・視線・スプリング生成が通ってからモデルを更新する。
/// 失敗時はモデルを部分更新せず、診断を標準エラーに出して終了コードを返す。
#[allow(clippy::too_many_lines)]
fn apply_physics_sidecar(
    model: &mut VrmModel,
    sidecar: &PhysicsSidecar,
    fbx_bytes: &[u8],
    humanoid: &HumanoidBones,
) -> Result<String, i32> {
    let resolution = match model.resolve_physics_sidecar(sidecar, fbx_bytes) {
        Ok(resolution) => {
            if resolution.auxiliary_nodes.is_empty() {
                eprintln!("物理: 補助ノードの追加は無い");
            } else {
                let names: Vec<&str> = resolution
                    .auxiliary_nodes
                    .iter()
                    .map(|aux| aux.name.as_str())
                    .collect();
                eprintln!(
                    "物理: 補助ノード{}件を追加（{}）",
                    resolution.auxiliary_nodes.len(),
                    names.join(", "),
                );
            }
            resolution
        }
        Err(error) => {
            diagnose_stage("物理sidecarの解決に失敗した", &error);
            return Err(1);
        }
    };
    // humanoidのrest姿勢はsidecarのT-poseを優先する。無い骨は
    // ノードの姿勢のまま残す。参照実装もリグ構築直後にT-poseを当ててから
    // 物理を解決するため、視線の測定とスプリングの生成より前に適用する。
    // 書き出しに使う解決済み対応を渡し、不一致は適用前に失敗させる。
    // 失敗時はモデルを部分更新せず打ち切る。
    let (tpose_applied, tpose_skipped) =
        match apply_humanoid_t_pose(sidecar, model, &resolution, humanoid) {
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
                return Err(1);
            }
        };
    // 視線の解決。sidecarに視線記録が無い旧形式では何もしない。
    // 測定も既定もここで確定する。モデルは変更しないため、
    // 失敗時は部分更新なく打ち切る。humanoidは解決より前に検出したものを
    // 使い、補助ノード（同名ノードを含む）の影響を受けない。
    let look_at = match sidecar.look_at.as_ref() {
        Some(_) => match resolve_look_at(sidecar, model, &resolution, humanoid) {
            Ok(look_at) => look_at,
            Err(error) => {
                diagnose_stage("視線の解決に失敗した", &error);
                return Err(1);
            }
        },
        None => None,
    };
    match compile_springs(sidecar, model, &resolution, &SpringCompileOptions::new()) {
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
            Ok(summary)
        }
        Err(error) => {
            diagnose_stage("スプリングの生成に失敗した", &error);
            Err(1)
        }
    }
}

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

/// Prefabを中間モデルへ適用し、材質プールと適用報告を返す。
///
/// Prefabの読み取り・入力FBXのGUID照合・取り込みIDでの対応付け・材質プールの
/// 読み取り・除外と割当の適用を順に行う。いずれかの段階で失敗した場合は
/// 診断を標準エラーに出し、終了コード1を返す。成功時はモデルを更新する。
/// 材質配置が無い場合は `.fbx.meta` と `.mat` を読めないため失敗する。
/// 索引は一度だけ走査し、計画と読み取りで共有する。
#[cfg(not(target_arch = "wasm32"))]
fn apply_prefab_assignment(
    model: &mut VrmModel,
    file: &FbxFile<'_>,
    prefab_path: &std::path::Path,
    input: &std::path::Path,
    assets_root: Option<&std::path::Path>,
) -> Result<(VrmMaterials, RendererApplyReport), i32> {
    use vrctom::unity::{IndexOptions, ParseOptions, SerializedFile, scan_directory};
    use vrctom::vrm::{
        PrefabMeshView, apply_prefab_plan, load_renderer_materials_with_index,
        plan_prefab_assignment, prefab_base_materials, prefab_hierarchy_paths,
        resolve_prefab_pool_paths,
    };

    let prefab_label = format!("{prefab_path:?}");
    let prefab_bytes = match std::fs::read(prefab_path) {
        Ok(prefab_bytes) => prefab_bytes,
        Err(error) => {
            eprintln!(
                "Prefabを読み込めない: {}: {}",
                escape_diag(&prefab_label),
                escape_diag(&error.to_string())
            );
            return Err(1);
        }
    };
    let prefab = match SerializedFile::parse(&prefab_bytes, &ParseOptions::new()) {
        Ok(prefab) => prefab,
        Err(error) => {
            diagnose_stage("Prefabの解析に失敗した", &error);
            return Err(1);
        }
    };
    let Some(root) = assets_root else {
        eprintln!(
            "Prefab割当には材質配置が必要である: {}",
            escape_diag(&prefab_label)
        );
        return Err(1);
    };
    // 以降の索引・正規化は `.fbx.meta` の読み取りと同じ手順であり、
    // 材質経路の `VrmMaterials::load` と対応する。
    let canonical_root = match std::fs::canonicalize(root) {
        Ok(canonical_root) => canonical_root,
        Err(error) => {
            eprintln!(
                "材質の走査元を解決できない: {}: {}",
                escape_diag(&format!("{root:?}")),
                escape_diag(&error.to_string())
            );
            return Err(1);
        }
    };
    let canonical_assets = match std::fs::canonicalize(canonical_root.join("Assets")) {
        Ok(canonical_assets) => canonical_assets,
        Err(error) => {
            eprintln!(
                "材質のAssetsを解決できない: {}: {}",
                escape_diag(&format!("{:?}", canonical_root.join("Assets"))),
                escape_diag(&error.to_string())
            );
            return Err(1);
        }
    };
    let canonical_fbx = match std::fs::canonicalize(input) {
        Ok(canonical_fbx) => canonical_fbx,
        Err(error) => {
            eprintln!(
                "FBXを解決できない: {}: {}",
                escape_diag(&format!("{input:?}")),
                escape_diag(&error.to_string())
            );
            return Err(1);
        }
    };
    let relative = canonical_fbx
        .strip_prefix(&canonical_assets)
        .ok()
        .and_then(|path| {
            path.components()
                .map(|component| component.as_os_str().to_str().map(str::to_owned).ok_or(()))
                .collect::<Result<Vec<_>, _>>()
                .ok()
        })
        .map(|parts| parts.join("/").replace('\\', "/"));
    let Some(relative) = relative else {
        eprintln!(
            "Prefab割当のFBXがAssets配下にない: {}",
            escape_diag(&format!("{input:?}"))
        );
        return Err(1);
    };
    let index = match scan_directory(&canonical_assets, &IndexOptions::default()) {
        Ok(index) => index,
        Err(error) => {
            diagnose_stage("材質の索引化に失敗した", &error);
            return Err(1);
        }
    };
    let Some(record) = index.get_by_path(&relative) else {
        eprintln!("Prefab割当のFBXが索引にない: {}", escape_diag(&relative));
        return Err(1);
    };
    let fbx_guid = record.meta.guid().as_str().to_owned();
    let remaps = match record.meta.external_object_remaps() {
        Ok(remaps) => remaps,
        Err(error) => {
            diagnose_stage("FBXの置換表を読めない", &error);
            return Err(1);
        }
    };
    let base = prefab_base_materials(&remaps);
    let nodes = match prefab_hierarchy_paths(file) {
        Ok(nodes) => nodes,
        Err(error) => {
            diagnose_stage("Prefab割当の階層パスを作れない", &error);
            return Err(1);
        }
    };
    let at: std::collections::HashMap<i64, usize> = file
        .models()
        .iter()
        .enumerate()
        .map(|(index, model)| (model.id.value(), index))
        .collect();
    let mut views = Vec::with_capacity(model.meshes.len());
    for mesh in &model.meshes {
        let Some(owner) = file.model(mesh.model) else {
            eprintln!(
                "所有モデルがFBXに無い: {}",
                escape_diag(&format!("{:?}", mesh.model.value()))
            );
            return Err(1);
        };
        let Some(&node_index) = at.get(&owner.id.value()) else {
            eprintln!(
                "所有モデルが階層に無い: {}",
                escape_diag(&format!("{:?}", owner.id.value()))
            );
            return Err(1);
        };
        views.push(PrefabMeshView {
            model_name: owner.name.as_str(),
            hierarchy_path: nodes[node_index].path.as_str(),
            slot_names: mesh.material_slots.as_slice(),
        });
    }
    let mut plan = match plan_prefab_assignment(&prefab, &prefab_label, &fbx_guid, &base, &views) {
        Ok(plan) => plan,
        Err(error) => {
            diagnose_stage("Prefab割当の解決に失敗した", &error);
            return Err(1);
        }
    };
    let mode = match plan.mode {
        vrctom::vrm::PrefabMode::Instance => "改変内容",
        vrctom::vrm::PrefabMode::Direct => "直接記述",
    };
    eprintln!(
        "Prefab割当: FBX一致 {} guid:{}",
        escape_diag(&relative),
        plan.fbx_guid
    );
    resolve_prefab_pool_paths(&mut plan.pool, &index);
    let (materials, material_report) = match load_renderer_materials_with_index(
        &canonical_assets,
        &index,
        &plan.pool,
        &MaterialOptions::new(),
    ) {
        Ok(loaded) => loaded,
        Err(error) => {
            diagnose_stage("Prefab材質の読み取りに失敗した", &error);
            return Err(1);
        }
    };
    for warning in material_report.warnings() {
        eprintln!("材質の警告: {}", escape_diag(warning));
    }
    let report = match apply_prefab_plan(model, &plan, &materials) {
        Ok(report) => report,
        Err(error) => {
            diagnose_stage("Prefab割当の適用に失敗した", &error);
            return Err(1);
        }
    };
    eprintln!(
        "Prefab割当: {}方式 有効{}件 除外{}件（{}） 材質プール{}件",
        mode,
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

    /// 指定バイト数の `.fbx.meta` を作る。YAML注釈で埋めて大きさを調整する。
    ///
    /// 上限ちょうど・上限+1の境界を確かめるために使う。
    /// 内容は空の対応表（`human: []`）であり、読み取り自体は成功する。
    fn 上限サイズのmetaを作る(作業: &std::path::Path, 全体: usize) {
        let 頭 = b"fileFormatVersion: 2\nguid: 0123456789abcdef0123456789abcdef\nModelImporter:\n  animationType: 3\n  humanDescription:\n    human: []\n";
        assert!(全体 > 頭.len() + 4, "注釈を足せる大きさである");
        let mut 本文 = 頭.to_vec();
        本文.extend_from_slice(b"# ");
        本文.extend(std::iter::repeat_n(b'x', 全体 - 本文.len() - 1));
        本文.push(b'\n');
        assert_eq!(本文.len(), 全体, "指定の大きさになる");
        std::fs::write(作業.join("入力.fbx.meta"), 本文).expect("metaは書ける");
        std::fs::write(作業.join("入力.fbx"), b"fake-fbx").expect("fbxは書ける");
    }

    #[test]
    fn 上限ちょうどのmetaは読み取れる() {
        // 上限ちょうどは制限付き読み取りで通り、空の対応表になる。
        let 作業 = tempfile::TempDir::new().expect("作業場所は作れる");
        上限サイズのmetaを作る(作業.path(), super::MAX_META_BYTES as usize);
        let 対応 = super::load_humanoid_mappings(&作業.path().join("入力.fbx"))
            .expect("上限ちょうどは通る");
        assert!(対応.is_empty(), "空の対応表である");
    }

    #[test]
    fn 上限を超えたmetaは読み取れない() {
        // 上限+1は読み取り層の上限超過で失敗する。出力は作らない。
        let 作業 = tempfile::TempDir::new().expect("作業場所は作れる");
        上限サイズのmetaを作る(作業.path(), super::MAX_META_BYTES as usize + 1);
        let 結果 = super::load_humanoid_mappings(&作業.path().join("入力.fbx"));
        assert!(結果.is_err(), "上限超過は失敗する");
    }

    #[test]
    fn 存在しないmetaは空の対応表になる() {
        // `.fbx.meta` が無い場合は名前推測に任せるため空になる。
        let 作業 = tempfile::TempDir::new().expect("作業場所は作れる");
        let 入力 = 作業.path().join("入力.fbx");
        std::fs::write(&入力, b"fake-fbx").expect("fbxは書ける");
        let 対応 = super::load_humanoid_mappings(&入力).expect("meta無しは空になる");
        assert!(対応.is_empty(), "空の対応表である");
    }

    #[test]
    fn 読み取れないmetaは空に読み替えない() {
        // `NotFound` 以外の読み取り失敗は空対応表への読み替えをせず失敗する。
        // ディレクトリを開いた読み取りは失敗するため、権限に依存せず確かめられる。
        let 作業 = tempfile::TempDir::new().expect("作業場所は作れる");
        let 入力 = 作業.path().join("入力.fbx");
        std::fs::write(&入力, b"fake-fbx").expect("fbxは書ける");
        std::fs::create_dir(作業.path().join("入力.fbx.meta")).expect("meta位置に場所を作る");
        let 結果 = super::load_humanoid_mappings(&入力);
        assert!(結果.is_err(), "読み取り失敗は空にしない");
    }
}
