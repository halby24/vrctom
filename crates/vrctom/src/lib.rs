//! VRChat向けアバター(Unity Setup)をVRMへ変換するライブラリ。

/// 診断表示のための可視エスケープ。
///
/// アーカイブ由来の文字列や依存エラーの文面など、外部入力を含む表示に使う。
/// 改行・復帰・タブ・NUL・ESC・その他の制御文字（Cc）と、行偽装に使える
/// 不可視・書式文字（soft hyphen・双方向マーク・ゼロ幅・行区切り・タグ文字など）を
/// `\\n`・`\\x1b`・`\\u{...}` 形に変える。
/// 対象に `char::is_control()` が偽になる不可視文字を含むのは、制御文字検査だけでは
/// 素通りするためである（`is_printable` は当該toolchainで未安定のため明示範囲で判定する）。
/// `\\` 自体は変えないため、逃がし済みの文面への再適用は変えない。
pub(crate) fn escape_diag(text: &str) -> String {
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

/// 変換用の一時作業領域.
pub mod workspace;

/// Unityの保存データを参照関係を壊さず読み取る層。
pub mod unity;

/// バイナリFBXからメッシュ幾何とモデル階層を読み取る層。
pub mod fbx;

/// FBX読み取り結果からVRM 1.0へ書き出す前の中間モデル。
pub mod vrm;

/// UnityPackageの展開。
#[cfg(not(target_arch = "wasm32"))]
pub mod unitypackage;
