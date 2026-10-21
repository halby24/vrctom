//! UnityPackageの展開などを行う補助CLI。
//!
//! 標準出力には何も出さず、結果の要約や失敗の診断は標準エラーに出す。
//! 終了コードは成功で0、実行失敗で1、引数不正で2とする。
//! 引数不正の2は[`clap`]のエラーを可視エスケープして返す。

use std::fs::File;
use std::path::PathBuf;

use clap::{Parser, Subcommand};
use vrctom::unitypackage::{UnpackOptions, unpack_to};

#[derive(Debug, Parser)]
#[command(
    name = "vrctom",
    version,
    about = "UnityPackageの展開などを行う補助CLI"
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
            // 診断文面は可視エスケープしてから出し、端末制御や行偽装を防ぐ。
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
