//! Workerから呼ぶwasmバインディングの薄い境界。
//!
//! 変換の中核はターゲット非依存の [`core`] に置き、ここでは
//! `#[wasm_bindgen]` の出入口とJSエラーへの写しだけを行う。

pub mod core;

use wasm_bindgen::prelude::*;

/// 検証済みパッケージの所有者。入力バイト列自体は保持しない。
#[wasm_bindgen]
pub struct WebPackage {
    package: core::Package,
}

#[wasm_bindgen]
impl WebPackage {
    /// 入力をWeb向け上限で展開し、索引まで作る。
    ///
    /// 検証に失敗したら開かない。成功後は元のバイト列を残さない。
    #[wasm_bindgen(constructor)]
    pub fn new(bytes: &[u8]) -> Result<WebPackage, JsValue> {
        core::Package::open(bytes)
            .map(|package| WebPackage { package })
            .map_err(to_js_error)
    }

    /// FBX候補のJSON文字列を返す。パス昇順である。
    pub fn fbx_candidates(&self) -> Result<String, JsValue> {
        Ok(self.package.candidates_json())
    }

    /// 選んだFBXを変換し、VRMバイト列とレポートJSONを持つ [`Converted`] を返す。
    pub fn convert(&self, fbx_path: &str, options_json: &str) -> Result<Converted, JsValue> {
        self.package
            .convert(fbx_path, options_json)
            .map(|converted| Converted {
                vrm: converted.vrm,
                report: converted.report_json,
            })
            .map_err(to_js_error)
    }
}

/// 変換結果。VRMバイト列とレポートJSONを持つ。
#[wasm_bindgen]
pub struct Converted {
    vrm: Vec<u8>,
    report: String,
}

#[wasm_bindgen]
impl Converted {
    /// VRM 1.0のGLBバイト列を返す。
    #[wasm_bindgen(getter)]
    pub fn vrm(&self) -> Vec<u8> {
        self.vrm.clone()
    }

    /// VRM 1.0のGLBバイト列を取り出す。
    ///
    /// 複製せず所有権ごと渡すため、大きなVRMではこちらを使う。
    /// 呼び出し後は空になる。Worker側の移行先であり、
    /// 従来の `vrm` 取得子は互換のため残す。
    pub fn take_vrm(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.vrm)
    }

    /// レポートのJSON文字列を返す。
    #[wasm_bindgen(getter)]
    pub fn report(&self) -> String {
        self.report.clone()
    }
}

/// 失敗をJSの `Error` へ写す。
///
/// `message` に日本語の診断を持ち、`stage` と対象がある場合の `path` を
/// 性質として付ける。内部の `Debug` 文字列は出さない。
fn to_js_error(error: core::CoreError) -> JsValue {
    let js_error = js_sys::Error::new(error.message());
    js_sys::Reflect::set(
        &js_error,
        &JsValue::from_str("stage"),
        &JsValue::from_str(error.stage().as_str()),
    )
    .unwrap_or_default();
    if let Some(path) = error.path() {
        js_sys::Reflect::set(
            &js_error,
            &JsValue::from_str("path"),
            &JsValue::from_str(path),
        )
        .unwrap_or_default();
    }
    JsValue::from(js_error)
}
