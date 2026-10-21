//! VRChat向けアバター(Unity Setup)をVRMへ変換するライブラリ。

/// 変換用の一時作業領域。
pub mod workspace;

/// UnityPackageの展開。
#[cfg(not(target_arch = "wasm32"))]
pub mod unitypackage;
