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
use vrctom::vrm::{
    MaterialOptions, PhysicsLimits, PhysicsSidecar, SpringCompileOptions, VrmExportOptions,
    VrmExpressionProfile, VrmMaterials, VrmMeta, VrmModel, VrmOptions, compile_springs,
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
    },
}

/// 表情の選択指定。現在はVRChat向けの8件だけを受け付ける。
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum ExpressionProfile {
    /// VRChat向けの8件（あいうえお・瞬き・視線上下）。
    Vrc,
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
        } => export(input, output, name, author, assets, expressions, physics),
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
fn export(
    input: PathBuf,
    output: Option<PathBuf>,
    name: Option<String>,
    authors: Vec<String>,
    assets: Option<PathBuf>,
    expressions: Option<ExpressionProfile>,
    physics: Option<PathBuf>,
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
                    "スプリング{}件 関節{}件 collider{}件 グループ{}件 分岐{}件 center{}件（world{} / all-motion{} / なし{}） 近似(endpoint未生成{} gravity0関節{} first-child{} 平均近似{} 平面落下{}) 関節値(硬さ平均{:.3} 重力あり{}件) 共有{}件 スキップ{}件",
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
                );
                model.spring_bones = compiled.bones;
                spring_summary = Some(summary);
            }
            Err(error) => {
                diagnose_stage("スプリングの生成に失敗した", &error);
                return 1;
            }
        }
    }
    let meta = VrmMeta::new(meta_name, meta_authors);
    // 材質の読み取り。指定時はその配置、省略時はFBXの祖先の `Assets` から求める。
    let explicit_assets = assets.is_some();
    let assets_root = match assets {
        Some(dir) => Some(assets_root_of(&dir)),
        None => assets_root_from_ancestors(&input),
    };
    let mut export_options = VrmExportOptions::new(meta);
    let mut material_count = 0usize;
    if let Some(root) = assets_root {
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
        report.mesh_count,
        report.skin_count,
        material_count,
        vrm_bytes.len(),
        escape_diag(&format!("{destination:?}"))
    );
    if let Some(summary) = spring_summary {
        eprintln!("物理: {summary}");
    }
    0
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
