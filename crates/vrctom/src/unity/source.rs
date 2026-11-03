//! 資産の読み手（ターゲット非依存）。
//!
//! 公開契約の入口は [`crate::unity`] を参照。
//!
//! 材質などの読み取り層は、ファイルシステムに直接触れずこの読み手越しに読む。
//! nativeは展開済みディレクトリから、webはメモリ内パッケージから読む。

/// `Assets` 配下の資産を読む能力。
///
/// `path` は `Assets` 直下を基準とする `/` 区切りの相対パスであり、
/// `Assets/` 接頭辞は付けない。`limit` を超える内容は切り詰めず
/// [`std::io::ErrorKind::InvalidData`] で失敗する。
/// 存在しない場合は [`std::io::ErrorKind::NotFound`] で失敗し、
/// それ以外の入出力失敗はそのまま返す。
pub trait AssetSource {
    /// `path` の内容を上限付きで読む。
    fn read_limited(&self, path: &str, limit: u64) -> std::io::Result<Vec<u8>>;
}
