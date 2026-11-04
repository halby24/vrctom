//! テクスチャのベイク。
//!
//! Unityの `.mat` と参照実装（`SoulFlameBake.shader` の各パス・
//! `BakeJobBuilder` の組み方・`BakeExecutor.Bind` の渡し方）に沿い、
//! 基準色・陰影・発光・リム乗算の合成とチャンネル詰め替え・法線合成をCPUで行う。
//! Matcap合成・デカールは扱わない。
//!
//! ```mermaid
//! flowchart LR
//!     mat[".mat"] --> decide["焼く/載せるの判断"]
//!     tex["画像バイト列"] --> load["復号<br/>RGBA8＋sRGB旗"]
//!     meta[".meta<br/>maxTextureSize・NPOT"] --> shrink["取り込み上限へ縮小<br/>box平均"]
//!     load --> shrink
//!     shrink --> sample["双線形＋wrap＋_ST"]
//!     decide --> pass["各パスの合成<br/>線形f32"]
//!     sample --> pass
//!     pass --> enc["PNG符号化<br/>色はsRGB・データは線形"]
//!     enc --> dedup["内容ハッシュで重複排除"]
//! ```
//!
//! 画素値は線形の `f32` で持ち、sRGB復号は入力の `.meta`
//! （`TextureImporter.mipmaps.sRGBTexture`）に従う。色出力はsRGB符号化し、
//! マスク・法線などのデータ出力は線形のまま書く。GPUの `tex2Dlod` と同じく
//! テクスル中心で標本化し、入力ごとの繰り返し・フィルタ・`_ST`
//! （`uv * scale + offset`）を適用する。

use std::rc::Rc;

use super::material::{
    AlbedoTint, MaterialError, MaterialErrorKind, VrmFilter, VrmImageMime, VrmSampler, VrmWrap,
};

/// 焼きが1辺に持てる最大テクセル数。
///
/// 参照実装の `BakeExecutor` が失敗させる上限（8192）より内側で、
/// `BakeJobBuilder.ResampleSizeLimit`（4096）に合わせる。
const MAX_BAKE_EDGE: u32 = 4096;
/// リサンプル時に詳細を保てる繰り返しの上限（面あたり）。
///
/// 参照実装の `MToonMapping.MaxResampleRepeats` と対応する。
const MAX_RESAMPLE_REPEATS: f64 = 8.0;
/// MatCap平均色の標本グリッドの1辺。
///
/// 参照実装の `BakeJobBuilder.MatcapFoldSamples` と対応する。
const MATCAP_FOLD_SAMPLES: u32 = 128;
/// 陰影層の重み付けの標本数。
///
/// 参照実装の `ShadeLayers.Samples` と対応する。
const SHADE_LAYER_SAMPLES: u32 = 1024;

/// 焼きの入力画像1枚。
///
/// 画素は復号直後のRGBA8（sRGB復号前）のまま持ち、標本化時に
/// [`BAKED_SRGB_LUT`] で線形へ直す。復号を遅延させず標本時に行うのは、
/// 同じ画像を色とデータの両方に使う場合の二重保持を避けるためである。
/// 4096x4096で67MiBであり、`f32` 保持（256MiB）より小さく収める。
#[derive(Debug, Clone)]
pub struct BakeImage {
    /// 幅（テクセル数）。0ではない。
    width: u32,
    /// 高さ（テクセル数）。0ではない。
    height: u32,
    /// 行優先のRGBA8。長さは `width * height * 4` である。
    rgba: Vec<u8>,
    /// 標本化時にsRGB復号するかどうか。`.meta` の `sRGBTexture` である。
    srgb: bool,
    /// 繰り返し・フィルタ。`.meta` から作ったサンプラである。
    sampler: VrmSampler,
}

impl BakeImage {
    /// 幅を返す。
    pub fn width(&self) -> u32 {
        self.width
    }

    /// 高さを返す。
    pub fn height(&self) -> u32 {
        self.height
    }

    /// 行優先のRGBA8を返す。
    pub fn pixels(&self) -> &[u8] {
        &self.rgba
    }

    /// 行優先のRGBA8から焼き入力画像を作る。
    ///
    /// PSDの保存済みcompositeなど、既に復号済みの画素を焼きや縮小の
    /// 共通処理へ載せる入口である。中間PNGを作らず直接載せる。
    /// 寸法と画素長の対応だけを確かめ、画素上限の判定は呼び出し側で行う。
    ///
    /// # Errors
    ///
    /// 寸法が0・画素長が寸法と合わない場合に [`MaterialErrorKind::InvalidMaterial`] で返す。
    pub(crate) fn from_rgba8(
        width: u32,
        height: u32,
        rgba: Vec<u8>,
        srgb: bool,
        sampler: VrmSampler,
    ) -> Result<Self, MaterialError> {
        let fail = |reason: &str| {
            MaterialError::new(
                MaterialErrorKind::InvalidMaterial,
                format!("RGBA8から焼き画像を作れない: {reason}"),
            )
        };
        if width == 0 || height == 0 {
            return Err(fail("寸法が0である"));
        }
        let expect = (u64::from(width))
            .checked_mul(u64::from(height))
            .and_then(|pixels| pixels.checked_mul(4))
            .and_then(|len| usize::try_from(len).ok())
            .ok_or_else(|| fail("寸法が大きすぎる"))?;
        if rgba.len() != expect {
            return Err(fail("画素数が寸法と合わない"));
        }
        Ok(Self {
            width,
            height,
            rgba,
            srgb,
            sampler,
        })
    }
}

/// sRGB復号の参照表。`LUT[v] = linear(v / 255)` である。
static BAKED_SRGB_LUT: std::sync::LazyLock<[f32; 256]> = std::sync::LazyLock::new(|| {
    let mut lut = [0.0f32; 256];
    for (value, slot) in lut.iter_mut().enumerate() {
        *slot = srgb_to_linear_f32(f64::from(value as u8) / 255.0);
    }
    lut
});

/// sRGB符号化値を線形に直す（IEC 61966-2-1の区分式）。
///
/// UnityのsRGBレンダーターゲットと同じ式であり、GPUの近似とは
/// 量子化以内の差に収まる。
fn srgb_to_linear_f32(value: f64) -> f32 {
    if value <= 0.04045 {
        (value / 12.92) as f32
    } else {
        (((value + 0.055) / 1.055).powf(2.4)) as f32
    }
}

/// 線形値をsRGB符号化する（IEC 61966-2-1の区分式）。
fn linear_to_srgb_f32(value: f32) -> f32 {
    let value = f64::from(value);
    if value <= 0.0031308 {
        (value * 12.92) as f32
    } else {
        (1.055 * value.powf(1.0 / 2.4) - 0.055) as f32
    }
}

/// バイト値を線形に直す。sRGB旗が真の場合は復号し、偽の場合はそのまま割る。
fn decode_byte(value: u8, srgb: bool) -> f32 {
    if srgb {
        BAKED_SRGB_LUT[value as usize]
    } else {
        f32::from(value) / 255.0
    }
}

/// 画像バイト列を焼きの入力に復号する。
///
/// PNGは8bit RGBA/RGB・グレースケール（1/2/4/8bit）・16bit・パレットを、
/// JPEGはグレースケールとRGBを受ける。CMYKなどは読み替えず失敗させる。
/// `srgb` は `.meta` の `sRGBTexture` であり、標本化時の復号に使う。
/// `max_pixels` は1枚の画素数の上限であり、画素バッファの確保前に
/// 寸法だけを見て判定するため、巨大な寸法の申告では確保が起きない。
///
/// # Errors
///
/// 対応外の形式・壊れた画像・空の寸法・画素上限の超過の場合に返す。
/// 上限超過は [`MaterialErrorKind::LimitExceeded`] であり、文面に
/// 寸法と上限を含む。
pub fn load_bake_image(
    bytes: &[u8],
    mime: VrmImageMime,
    srgb: bool,
    sampler: VrmSampler,
    max_pixels: u64,
) -> Result<BakeImage, MaterialError> {
    match mime {
        VrmImageMime::Png => decode_png(bytes, srgb, sampler, max_pixels),
        VrmImageMime::Jpeg => decode_jpeg(bytes, srgb, sampler, max_pixels),
    }
}

/// 画素数が1枚の上限に収まることを確かめる。
///
/// 画素バッファの確保前に呼ぶ。超過は確保を行わず失敗させる。
fn check_image_pixels(
    width: u32,
    height: u32,
    max_pixels: u64,
    format: &str,
) -> Result<(), MaterialError> {
    let pixels = u64::from(width).saturating_mul(u64::from(height));
    if pixels > max_pixels {
        return Err(MaterialError::new(
            MaterialErrorKind::LimitExceeded,
            format!("{format}の画素数が上限を超えた: {width}x{height}（上限{max_pixels}画素）"),
        ));
    }
    Ok(())
}

/// 画像の寸法だけをヘッダから読む。
///
/// 画素データの確保は行わないため、巨大な寸法の申告でも確保が起きない。
/// 焼き・縮小の要否にかかわらず埋め込む画像の寸法検査に使い、
/// 未検査の画像を埋め込まない。
///
/// # Errors
///
/// ヘッダを読めない・寸法が0の場合に [`MaterialErrorKind::InvalidMaterial`] で返す。
pub fn image_dimensions(bytes: &[u8], mime: VrmImageMime) -> Result<(u32, u32), MaterialError> {
    match mime {
        VrmImageMime::Png => {
            let fail = |reason: &str| {
                MaterialError::new(
                    MaterialErrorKind::InvalidMaterial,
                    format!("PNGの寸法を読めない: {reason}"),
                )
            };
            // `output_buffer_size()` や `next_frame()` は呼ばず、
            // ヘッダの寸法だけを見る。
            let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
            let reader = decoder
                .read_info()
                .map_err(|error| fail(&error.to_string()))?;
            let (width, height) = {
                let info = reader.info();
                (info.width, info.height)
            };
            if width == 0 || height == 0 {
                return Err(fail("寸法が0である"));
            }
            Ok((width, height))
        }
        VrmImageMime::Jpeg => {
            let fail = |reason: String| {
                MaterialError::new(
                    MaterialErrorKind::InvalidMaterial,
                    format!("JPEGの寸法を読めない: {reason}"),
                )
            };
            // 画素の復号は行わず、ヘッダの寸法だけを見る。
            let mut decoder = zune_jpeg::JpegDecoder::new(std::io::Cursor::new(bytes));
            decoder
                .decode_headers()
                .map_err(|error| fail(error.to_string()))?;
            let (width, height) = match decoder.info() {
                Some(info) => (u32::from(info.width), u32::from(info.height)),
                None => return Err(fail("寸法を読めない".to_string())),
            };
            if width == 0 || height == 0 {
                return Err(fail("寸法が0である".to_string()));
            }
            Ok((width, height))
        }
    }
}

/// PNGをRGBA8へ復号する。
///
/// `png` クレートは展開（パレット・低ビット・16bit）を呼び出し側で行う。
/// 16bitは上位バイトではなく `(v * 255 + 32895) >> 16` で丸める。
fn decode_png(
    bytes: &[u8],
    srgb: bool,
    sampler: VrmSampler,
    max_pixels: u64,
) -> Result<BakeImage, MaterialError> {
    let fail = |reason: &str| {
        MaterialError::new(
            MaterialErrorKind::InvalidMaterial,
            format!("PNGを焼き用に復号できない: {reason}"),
        )
    };
    let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    let mut reader = decoder
        .read_info()
        .map_err(|error| fail(&error.to_string()))?;
    // 画素バッファの確保前に寸法だけを見て上限を判定する。
    // 申告寸法が巨大な入力では `output_buffer_size` の確保が起きない。
    let (width, height) = {
        let info = reader.info();
        (info.width, info.height)
    };
    if width == 0 || height == 0 {
        return Err(fail("寸法が0である"));
    }
    check_image_pixels(width, height, max_pixels, "PNG").map_err(|error| {
        // 寸法と上限を含む文面はそのまま残し、接頭辞だけを揃える。
        MaterialError::new(
            MaterialErrorKind::LimitExceeded,
            format!("PNGを焼き用に復号できない: {error}"),
        )
    })?;
    let mut raw = vec![
        0u8;
        reader
            .output_buffer_size()
            .ok_or_else(|| fail("寸法を読めない"))?
    ];
    let info = reader
        .next_frame(&mut raw)
        .map_err(|error| fail(&error.to_string()))?;
    let width = info.width;
    let height = info.height;
    if width == 0 || height == 0 {
        return Err(fail("寸法が0である"));
    }
    let rgba = match (info.color_type, info.bit_depth) {
        (png::ColorType::Rgba, png::BitDepth::Eight) => raw,
        (png::ColorType::Rgb, png::BitDepth::Eight) => expand_rgb_to_rgba(&raw),
        (png::ColorType::Grayscale, png::BitDepth::Eight) => expand_gray_to_rgba(&raw, None),
        (png::ColorType::GrayscaleAlpha, png::BitDepth::Eight) => {
            expand_gray_to_rgba(&raw[..], None)
                .chunks_exact(4)
                .zip(raw.chunks_exact(2).map(|pair| pair[1]))
                .flat_map(|(pixel, alpha)| [pixel[0], pixel[1], pixel[2], alpha])
                .collect()
        }
        (png::ColorType::Grayscale, png::BitDepth::One)
        | (png::ColorType::Grayscale, png::BitDepth::Two)
        | (png::ColorType::Grayscale, png::BitDepth::Four) => {
            expand_packed_gray(&raw, width, height, info.bit_depth)?
        }
        (png::ColorType::Indexed, _) => {
            let palette = reader
                .info()
                .palette
                .as_deref()
                .ok_or_else(|| fail("パレットがない"))?;
            let trns = reader.info().trns.as_deref().unwrap_or(&[]).to_vec();
            expand_palette(&raw, palette, &trns)?
        }
        (color, png::BitDepth::Sixteen) => expand_16bit(&raw, color)?,
        (color, depth) => {
            return Err(fail(&format!(
                "対応外の色型とビット深度である: {color:?}/{depth:?}"
            )));
        }
    };
    let expect = width as usize * height as usize * 4;
    if rgba.len() != expect {
        return Err(fail("画素数が寸法と合わない"));
    }
    Ok(BakeImage {
        width,
        height,
        rgba,
        srgb,
        sampler,
    })
}

/// RGB8をRGBA8（不透明）へ広げる。
fn expand_rgb_to_rgba(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len() / 3 * 4);
    for pixel in raw.chunks_exact(3) {
        out.extend_from_slice(&[pixel[0], pixel[1], pixel[2], 0xFF]);
    }
    out
}

/// グレースケールをRGBA8へ広げる。アルファ列がある場合は別途載せる。
fn expand_gray_to_rgba(raw: &[u8], alpha: Option<&[u8]>) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len() * 4);
    for (index, value) in raw.iter().enumerate() {
        let channel = match alpha {
            Some(list) => list[index],
            None => 0xFF,
        };
        out.extend_from_slice(&[*value, *value, *value, channel]);
    }
    out
}

/// 1/2/4bitグレースケールをRGBA8へ広げる。
fn expand_packed_gray(
    raw: &[u8],
    width: u32,
    height: u32,
    depth: png::BitDepth,
) -> Result<Vec<u8>, MaterialError> {
    let bits = match depth {
        png::BitDepth::One => 1,
        png::BitDepth::Two => 2,
        png::BitDepth::Four => 4,
        _ => {
            return Err(MaterialError::new(
                MaterialErrorKind::InvalidMaterial,
                "PNGを焼き用に復号できない: 対応外のビット深度である",
            ));
        }
    };
    let max = (1u16 << bits) - 1;
    let stride = (width as usize * bits as usize).div_ceil(8);
    if raw.len() < stride * height as usize {
        return Err(MaterialError::new(
            MaterialErrorKind::InvalidMaterial,
            "PNGを焼き用に復号できない: 画素が足りない",
        ));
    }
    let mut out = Vec::with_capacity(width as usize * height as usize * 4);
    for row in 0..height as usize {
        let line = &raw[row * stride..(row + 1) * stride];
        for column in 0..width as usize {
            let at = column * bits as usize;
            let byte = line[at / 8];
            let shift = 8 - (at % 8) - bits as usize;
            let value = (u16::from(byte) >> shift) & max;
            let scaled = ((u32::from(value) * 255 + u32::from(max) / 2) / u32::from(max)) as u8;
            out.extend_from_slice(&[scaled, scaled, scaled, 0xFF]);
        }
    }
    Ok(out)
}

/// パレット画像をRGBA8へ広げる。透明色表が短い場合は不透明にする。
fn expand_palette(raw: &[u8], palette: &[u8], trns: &[u8]) -> Result<Vec<u8>, MaterialError> {
    if !palette.len().is_multiple_of(3) {
        return Err(MaterialError::new(
            MaterialErrorKind::InvalidMaterial,
            "PNGを焼き用に復号できない: パレットが3の倍数でない",
        ));
    }
    let mut out = Vec::with_capacity(raw.len() * 4);
    for index in raw.iter().map(|value| usize::from(*value)) {
        let entry = palette.chunks_exact(3).nth(index).ok_or_else(|| {
            MaterialError::new(
                MaterialErrorKind::InvalidMaterial,
                "PNGを焼き用に復号できない: パレット範囲外の参照である",
            )
        })?;
        out.extend_from_slice(&[
            entry[0],
            entry[1],
            entry[2],
            trns.get(index).copied().unwrap_or(0xFF),
        ]);
    }
    Ok(out)
}

/// 16bit画像をRGBA8へ丸める。対応外の色型は失敗させる。
fn expand_16bit(raw: &[u8], color: png::ColorType) -> Result<Vec<u8>, MaterialError> {
    let fail = || {
        MaterialError::new(
            MaterialErrorKind::InvalidMaterial,
            "PNGを焼き用に復号できない: 対応外の16bit色型である",
        )
    };
    let words: Vec<u16> = raw
        .chunks_exact(2)
        .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
        .collect();
    let narrow = |value: u16| ((u32::from(value) * 255 + 32895) >> 16) as u8;
    let channels = match color {
        png::ColorType::Rgba => 4,
        png::ColorType::Rgb => 3,
        png::ColorType::Grayscale => 1,
        png::ColorType::GrayscaleAlpha => 2,
        _ => return Err(fail()),
    };
    let mut out = Vec::with_capacity(words.len() / channels * 4);
    for pixel in words.chunks_exact(channels) {
        let (red, green, blue, alpha) = match channels {
            4 => (pixel[0], pixel[1], pixel[2], pixel[3]),
            3 => (pixel[0], pixel[1], pixel[2], 0xFFFF),
            2 => (pixel[0], pixel[0], pixel[0], pixel[1]),
            _ => (pixel[0], pixel[0], pixel[0], 0xFFFF),
        };
        out.extend_from_slice(&[narrow(red), narrow(green), narrow(blue), narrow(alpha)]);
    }
    Ok(out)
}

/// JPEGをRGBA8へ復号する。グレースケールとRGBだけを受け、CMYKは失敗させる。
///
/// 先に `decode_headers` で寸法だけを読み、画素上限を判定してから
/// `decode` するため、巨大な寸法の申告では画素バッファの確保が起きない。
fn decode_jpeg(
    bytes: &[u8],
    srgb: bool,
    sampler: VrmSampler,
    max_pixels: u64,
) -> Result<BakeImage, MaterialError> {
    let fail = |reason: String| {
        MaterialError::new(
            MaterialErrorKind::InvalidMaterial,
            format!("JPEGを焼き用に復号できない: {reason}"),
        )
    };
    let mut decoder = zune_jpeg::JpegDecoder::new(std::io::Cursor::new(bytes));
    decoder
        .decode_headers()
        .map_err(|error| fail(error.to_string()))?;
    let (width, height) = match decoder.info() {
        Some(info) => (u32::from(info.width), u32::from(info.height)),
        None => return Err(fail("寸法を読めない".to_string())),
    };
    if width == 0 || height == 0 {
        return Err(fail("寸法が0である".to_string()));
    }
    if let Err(error) = check_image_pixels(width, height, max_pixels, "JPEG") {
        return Err(MaterialError::new(
            MaterialErrorKind::LimitExceeded,
            format!("JPEGを焼き用に復号できない: {error}"),
        ));
    }
    let pixels = decoder.decode().map_err(|error| fail(error.to_string()))?;
    let info = decoder
        .info()
        .ok_or_else(|| fail("寸法を読めない".to_string()))?;
    let (width, height) = (u32::from(info.width), u32::from(info.height));
    if width == 0 || height == 0 {
        return Err(fail("寸法が0である".to_string()));
    }
    let rgba = match info.components {
        1 => expand_gray_to_rgba(&pixels, None),
        3 => expand_rgb_to_rgba(&pixels),
        _ => {
            return Err(fail(format!("対応外の成分数である: {}", info.components)));
        }
    };
    if rgba.len() != width as usize * height as usize * 4 {
        return Err(fail("画素数が寸法と合わない".to_string()));
    }
    Ok(BakeImage {
        width,
        height,
        rgba,
        srgb,
        sampler,
    })
}

/// 焼き入力のUV変換（`_ST` 規約：`uv * scale + offset`）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BakeSt {
    /// 横・縦の繰り返し量。
    pub scale: [f64; 2],
    /// 横・縦のずらし量。
    pub offset: [f64; 2],
}

impl BakeSt {
    /// 単位変換を作る。
    pub fn identity() -> Self {
        Self {
            scale: [1.0, 1.0],
            offset: [0.0, 0.0],
        }
    }

    /// 単位変換かどうかを返す。
    pub fn is_identity(&self) -> bool {
        self.scale == [1.0, 1.0] && self.offset == [0.0, 0.0]
    }
}

/// 焼き入力が読むチャンネル。
///
/// 参照実装の `TextureChannel` と対応する。`Rgb` は色として保ち、
/// スカラー文脈ではRを読む（`BindInput.Selector` と対応）。
/// G・Bは将来のスロット（陰影ずれBなど）のために残す。
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BakeChannel {
    /// 色として保つ。
    Rgb,
    /// 赤。
    R,
    /// 緑。
    G,
    /// 青。
    B,
    /// アルファ。
    A,
}

/// 焼きが読むテクスチャ1件。
///
/// 参照実装の `BakeInput` と対応する。`image` が読み先、
/// `st` がUV0に対する `_ST`、`channel` が読むチャンネル、
/// `invert` がスカラーの `1 - value` である。
/// 参照実装の `Alpha`（リム係数のアルファ掛け）は
/// リム乗算合成でしか使わず、今回の対象外のため持たない。
#[derive(Debug, Clone)]
pub struct BakeInput {
    /// 読み先の画像。
    pub image: Rc<BakeImage>,
    /// UV0に対する `_ST`。
    pub st: BakeSt,
    /// 読むチャンネル。
    pub channel: BakeChannel,
    /// スカラーの反転。
    pub invert: bool,
}

/// 出力UVをメッシュUV0へ戻す（`_Target` の逆変換）。
///
/// 参照実装の `MeshUv` と対応する。今回は単位変換だけを通すが、
/// 式は写しておく。
pub fn mesh_uv(out_uv: [f64; 2], target: [f64; 4]) -> [f64; 2] {
    [
        (out_uv[0] - target[2]) / target[0],
        (out_uv[1] - target[3]) / target[1],
    ]
}

/// 単位UVへwrapを適用する。
fn wrap_unit(value: f32, wrap: VrmWrap) -> f32 {
    match wrap {
        VrmWrap::Repeat => value - value.floor(),
        VrmWrap::ClampToEdge => value.clamp(0.0, 1.0),
        VrmWrap::MirroredRepeat => {
            let folded = value - 2.0 * (value / 2.0).floor();
            if folded <= 1.0 { folded } else { 2.0 - folded }
        }
    }
}

/// テクスル番号を範囲へ収める。繰り返しは回し、他は端で止める。
///
/// GPUの繰り返し標本化と同じく、繰り返しは番号を回す
/// （UV折り畳み後の端点丸めでは tile 境界の混合が変わるため）。
fn wrap_index(index: i64, length: u32, wrap: VrmWrap) -> u32 {
    let length = i64::from(length);
    match wrap {
        VrmWrap::Repeat => (((index % length) + length) % length) as u32,
        _ => index.clamp(0, length - 1) as u32,
    }
}

/// 1テクセルの生バイト列を読む。
fn texel(image: &BakeImage, x: u32, y: u32) -> [u8; 4] {
    let at = (y as usize * image.width as usize + x as usize) * 4;
    [
        image.rgba[at],
        image.rgba[at + 1],
        image.rgba[at + 2],
        image.rgba[at + 3],
    ]
}

/// 1テクセルを線形 `f32` で読む。アルファはsRGB旗に関わらず生のままである。
fn texel_linear(image: &BakeImage, x: u32, y: u32) -> [f32; 4] {
    let raw = texel(image, x, y);
    [
        decode_byte(raw[0], image.srgb),
        decode_byte(raw[1], image.srgb),
        decode_byte(raw[2], image.srgb),
        f32::from(raw[3]) / 255.0,
    ]
}

/// テクスチャを `_ST` 込みで色として標本化する（`Tap` と対応）。
///
/// 双線形はGPUと同じテクセル中心（`uv * size - 0.5`）で、
/// sRGB復号後の線形値で混合する。点補間は `floor` で拾う。
pub fn tap_color(input: &BakeInput, uv0: [f64; 2]) -> [f32; 4] {
    let image = input.image.as_ref();
    let width = image.width;
    let height = image.height;
    let sampler = image.sampler;
    let u = wrap_unit(
        (uv0[0] * input.st.scale[0] + input.st.offset[0]) as f32,
        sampler.wrap_u,
    );
    let v = wrap_unit(
        (uv0[1] * input.st.scale[1] + input.st.offset[1]) as f32,
        sampler.wrap_v,
    );
    if sampler.filter == VrmFilter::Nearest {
        let x = wrap_index((u * width as f32).floor() as i64, width, sampler.wrap_u);
        let y = wrap_index((v * height as f32).floor() as i64, height, sampler.wrap_v);
        return texel_linear(image, x, y);
    }
    let x = u * width as f32 - 0.5;
    let y = v * height as f32 - 0.5;
    let x0 = x.floor() as i64;
    let y0 = y.floor() as i64;
    let fx = x - x0 as f32;
    let fy = y - y0 as f32;
    let corners = [
        texel_linear(
            image,
            wrap_index(x0, width, sampler.wrap_u),
            wrap_index(y0, height, sampler.wrap_v),
        ),
        texel_linear(
            image,
            wrap_index(x0 + 1, width, sampler.wrap_u),
            wrap_index(y0, height, sampler.wrap_v),
        ),
        texel_linear(
            image,
            wrap_index(x0, width, sampler.wrap_u),
            wrap_index(y0 + 1, height, sampler.wrap_v),
        ),
        texel_linear(
            image,
            wrap_index(x0 + 1, width, sampler.wrap_u),
            wrap_index(y0 + 1, height, sampler.wrap_v),
        ),
    ];
    let mut out = [0.0f32; 4];
    for channel in 0..4 {
        let top = corners[0][channel] + (corners[1][channel] - corners[0][channel]) * fx;
        let bottom = corners[2][channel] + (corners[3][channel] - corners[2][channel]) * fx;
        out[channel] = top + (bottom - top) * fy;
    }
    out
}

/// チャンネル選択の重みを返す（`BindInput.Selector` と対応）。
fn channel_select(channel: BakeChannel) -> [f32; 4] {
    match channel {
        BakeChannel::G => [0.0, 1.0, 0.0, 0.0],
        BakeChannel::B => [0.0, 0.0, 1.0, 0.0],
        BakeChannel::A => [0.0, 0.0, 0.0, 1.0],
        // `Rgb` のスカラー読みはRを読む。`R` も同じである。
        BakeChannel::Rgb | BakeChannel::R => [1.0, 0.0, 0.0, 0.0],
    }
}

/// スカラー入力を読む（`Scalar` と対応）。
pub fn tap_scalar(input: &BakeInput, uv0: [f64; 2]) -> f32 {
    let color = tap_color(input, uv0);
    let select = channel_select(input.channel);
    let value =
        color[0] * select[0] + color[1] * select[1] + color[2] * select[2] + color[3] * select[3];
    if input.invert { 1.0 - value } else { value }
}

/// RGBまたはスカラーのマスクを読む（`Masked` と対応）。
///
/// チャンネルが `Rgb` の場合は色をそのまま返し、
/// それ以外はスカラーを3成分へ広げる。
pub fn tap_masked(input: &BakeInput, uv0: [f64; 2]) -> [f32; 3] {
    if input.channel == BakeChannel::Rgb {
        let color = tap_color(input, uv0);
        [color[0], color[1], color[2]]
    } else {
        let value = tap_scalar(input, uv0);
        [value, value, value]
    }
}

/// lilToonの色調補正（ガンマ・HSV移動）。
///
/// 参照実装の `ToneCorrection`（lilToon由来）の写しである。
/// 著作権表示は `THIRD_PARTY_NOTICES.md` を参照する。
pub fn tone_correction(color: [f32; 3], hsvg: [f32; 4]) -> [f32; 3] {
    let corrected = [
        color[0].abs().powf(hsvg[3]),
        color[1].abs().powf(hsvg[3]),
        color[2].abs().powf(hsvg[3]),
    ];
    let mixed = if corrected[2] > corrected[1] {
        [corrected[2], corrected[1], -1.0, 2.0 / 3.0]
    } else {
        [corrected[1], corrected[2], 0.0, -1.0 / 3.0]
    };
    let ordered = if mixed[0] > corrected[0] {
        [mixed[0], mixed[1], mixed[3], corrected[0]]
    } else {
        [corrected[0], mixed[1], mixed[2], mixed[0]]
    };
    let delta = ordered[0] - ordered[3].min(ordered[1]);
    let hue = (ordered[2] + (ordered[3] - ordered[1]) / (6.0 * delta + 1e-10)).abs();
    let saturation = delta / (ordered[0] + 1e-10);
    let value = ordered[0];
    let shifted = [
        hue + hsvg[0],
        (saturation * hsvg[1]).clamp(0.0, 1.0),
        (value * hsvg[2]).clamp(0.0, 1.0),
    ];
    let wave = |phase: f32| (phase * 6.0 - 3.0).abs() - 1.0;
    let shape = [
        wave((shifted[0] + 1.0).fract()).clamp(0.0, 1.0),
        wave((shifted[0] + 2.0 / 3.0).fract()).clamp(0.0, 1.0),
        wave((shifted[0] + 1.0 / 3.0).fract()).clamp(0.0, 1.0),
    ];
    [
        shifted[2] - shifted[2] * shifted[1] + shifted[2] * shifted[1] * shape[0],
        shifted[2] - shifted[2] * shifted[1] + shifted[2] * shifted[1] * shape[1],
        shifted[2] - shifted[2] * shifted[1] + shifted[2] * shifted[1] * shape[2],
    ]
}

/// 法線マップをUnityの `UnpackScaleNormal` と同じく読む（`UnpackScaled` と対応）。
pub fn unpack_scaled(packed: [f32; 4], scale: f32) -> [f32; 3] {
    let x = (packed[0] * 2.0 - 1.0) * scale;
    let y = (packed[1] * 2.0 - 1.0) * scale;
    let z = (1.0 - (x * x + y * y).clamp(0.0, 1.0)).sqrt();
    [x, y, z]
}

/// 陰影層の光項（`ShadeLayers.Edge` と対応）。
fn shade_edge(x: f64, border: f32, blur: f32) -> f64 {
    let low = f64::from(border - blur * 0.5).clamp(0.0, 1.0);
    let high = f64::from(border + blur * 0.5).clamp(0.0, 1.0);
    if high <= low {
        return if x > low { 1.0 } else { 0.0 };
    }
    ((x - low) / (high - low)).clamp(0.0, 1.0)
}

/// 陰影色の層の重み（`ShadeLayers.Weights` と対応）。
///
/// 先頭が1層目の重みで、合計は1になる。各 `x` の寄与は
/// 第1境界の影の出方 `(1 - edge)^2` で重み付ける。
/// 2層目以降の色は呼び出し側が `Color.a` に引き込み量を持つ。
pub fn shade_layer_weights(border: f32, blur: f32, layers: &[(f32, f32, f32)]) -> Vec<f64> {
    let mut weights = vec![0.0f64; layers.len() + 1];
    let mut shares = vec![0.0f64; layers.len() + 1];
    let mut total = 0.0f64;
    for index in 0..SHADE_LAYER_SAMPLES {
        let x = (f64::from(index) + 0.5) / f64::from(SHADE_LAYER_SAMPLES);
        let shown = 1.0 - shade_edge(x, border, blur);
        let weight = shown * shown;
        if weight <= 0.0 {
            continue;
        }
        shares[0] = 1.0;
        for (slot, layer) in layers.iter().enumerate() {
            let pull = f64::from(layer.2).clamp(0.0, 1.0) * (1.0 - shade_edge(x, layer.0, layer.1));
            for share in shares.iter_mut().take(slot + 1) {
                *share *= 1.0 - pull;
            }
            shares[slot + 1] = pull;
        }
        for (weight_slot, share) in weights.iter_mut().zip(shares.iter()) {
            *weight_slot += weight * share;
        }
        total += weight;
    }
    if total <= 0.0 {
        weights.fill(0.0);
        weights[0] = 1.0;
        return weights;
    }
    for weight in weights.iter_mut() {
        *weight /= total;
    }
    weights
}

/// テクスチャを生値（復号前）のまま標本化する。
///
/// `MatcapDiscAverage` が `GetPixelBilinear` の生値に復号を掛ける順序と
/// 対応する。sRGB画像は双線形後に復号するため、GPU標本化とは量子化以内で
/// 異なる場合がある（平均化で均される）。
fn tap_raw(input: &BakeInput, uv0: [f64; 2]) -> [f32; 4] {
    let image = input.image.as_ref();
    let width = image.width;
    let height = image.height;
    let sampler = image.sampler;
    let u = wrap_unit(
        (uv0[0] * input.st.scale[0] + input.st.offset[0]) as f32,
        sampler.wrap_u,
    );
    let v = wrap_unit(
        (uv0[1] * input.st.scale[1] + input.st.offset[1]) as f32,
        sampler.wrap_v,
    );
    let pick = |x: u32, y: u32| {
        let raw = texel(image, x, y);
        [
            f32::from(raw[0]) / 255.0,
            f32::from(raw[1]) / 255.0,
            f32::from(raw[2]) / 255.0,
            f32::from(raw[3]) / 255.0,
        ]
    };
    if sampler.filter == VrmFilter::Nearest {
        let x = wrap_index((u * width as f32).floor() as i64, width, sampler.wrap_u);
        let y = wrap_index((v * height as f32).floor() as i64, height, sampler.wrap_v);
        return pick(x, y);
    }
    let x = u * width as f32 - 0.5;
    let y = v * height as f32 - 0.5;
    let x0 = x.floor() as i64;
    let y0 = y.floor() as i64;
    let fx = x - x0 as f32;
    let fy = y - y0 as f32;
    let corners = [
        pick(
            wrap_index(x0, width, sampler.wrap_u),
            wrap_index(y0, height, sampler.wrap_v),
        ),
        pick(
            wrap_index(x0 + 1, width, sampler.wrap_u),
            wrap_index(y0, height, sampler.wrap_v),
        ),
        pick(
            wrap_index(x0, width, sampler.wrap_u),
            wrap_index(y0 + 1, height, sampler.wrap_v),
        ),
        pick(
            wrap_index(x0 + 1, width, sampler.wrap_u),
            wrap_index(y0 + 1, height, sampler.wrap_v),
        ),
    ];
    let mut out = [0.0f32; 4];
    for channel in 0..4 {
        let top = corners[0][channel] + (corners[1][channel] - corners[0][channel]) * fx;
        let bottom = corners[2][channel] + (corners[3][channel] - corners[2][channel]) * fx;
        out[channel] = top + (bottom - top) * fy;
    }
    out
}

/// MatCapの平均色を内接円で求める（`MatcapDiscAverage` と対応）。
///
/// 球がMatCapに見せる範囲（内接円）の平均であり、生値の双線形の後に
/// sRGB旗に従い復号してアルファを掛ける。
pub fn matcap_disc_average(image: &Rc<BakeImage>, st: &BakeSt) -> [f32; 3] {
    // `Rc` を分け合うため、画素の複製は起きない。
    let input = BakeInput {
        image: Rc::clone(image),
        st: BakeSt::identity(),
        channel: BakeChannel::Rgb,
        invert: false,
    };
    let mut sum = [0.0f64; 3];
    let mut count = 0u32;
    for y in 0..MATCAP_FOLD_SAMPLES {
        for x in 0..MATCAP_FOLD_SAMPLES {
            let nx = (f64::from(x) + 0.5) / f64::from(MATCAP_FOLD_SAMPLES) * 2.0 - 1.0;
            let ny = (f64::from(y) + 0.5) / f64::from(MATCAP_FOLD_SAMPLES) * 2.0 - 1.0;
            if nx * nx + ny * ny > 1.0 {
                continue;
            }
            let u = 0.5 + 0.5 * (nx * st.scale[0] + st.offset[0]);
            let v = 0.5 + 0.5 * (ny * st.scale[1] + st.offset[1]);
            let folded = [u - (u / 1.0).floor(), v - (v / 1.0).floor()];
            let raw = tap_raw(&input, folded);
            let decoded = [
                decode_byte((raw[0] * 255.0).round().clamp(0.0, 255.0) as u8, image.srgb),
                decode_byte((raw[1] * 255.0).round().clamp(0.0, 255.0) as u8, image.srgb),
                decode_byte((raw[2] * 255.0).round().clamp(0.0, 255.0) as u8, image.srgb),
            ];
            sum[0] += f64::from(decoded[0]) * f64::from(raw[3]);
            sum[1] += f64::from(decoded[1]) * f64::from(raw[3]);
            sum[2] += f64::from(decoded[2]) * f64::from(raw[3]);
            count += 1;
        }
    }
    if count == 0 {
        return [0.0, 0.0, 0.0];
    }
    [
        (sum[0] / f64::from(count)) as f32,
        (sum[1] / f64::from(count)) as f32,
        (sum[2] / f64::from(count)) as f32,
    ]
}

/// 焼き出力の寸法。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BakeSize {
    /// 幅。
    pub width: u32,
    /// 高さ。
    pub height: u32,
}

/// 焼き出力の寸法を求める（`BakeJobBuilder.SizeFrom` の対象部分と対応）。
///
/// 各入力の `(幅, 高さ, 横の繰り返し, 縦の繰り返し)` から、
/// 繰り返しの多い入力は `ResampledSize` で嵩増しし、最大を取る。
/// 繰り返しは `|scale| / |target|` であり、今回は単位変換同士のため
/// 常に1以下で入力の素の寸法が採用される。寸法が無い場合は4x4、
/// 1辺が上限超えの場合は失敗させる。
pub fn bake_output_size(
    inputs: &[(u32, u32, f64, f64)],
    output_id: &str,
) -> Result<BakeSize, MaterialError> {
    let mut width = 0u32;
    let mut height = 0u32;
    for (texels_w, texels_h, scale_x, scale_y) in inputs {
        width = width.max(resampled_size(*texels_w, *scale_x));
        height = height.max(resampled_size(*texels_h, *scale_y));
    }
    let (width, height) = if width == 0 || height == 0 {
        (4, 4)
    } else {
        (width, height)
    };
    if width > MAX_BAKE_EDGE || height > MAX_BAKE_EDGE {
        return Err(MaterialError::new(
            MaterialErrorKind::LimitExceeded,
            format!("焼き出力が上限を超えた: {output_id}: {width}x{height}"),
        ));
    }
    Ok(BakeSize { width, height })
}

/// リサンプル後の1辺を求める（`ResampledSize` と対応）。
///
/// 繰り返しが1以下・上限超えの場合は素のテクセル数を保ち、
/// それ以外は繰り返し倍して上限（4096）で止める。
fn resampled_size(texels: u32, scale: f64) -> u32 {
    let repeats = scale.abs();
    if !repeats.is_finite() || repeats <= 1.0 || repeats > MAX_RESAMPLE_REPEATS {
        return texels;
    }
    (f64::from(texels) * repeats)
        .ceil()
        .clamp(0.0, f64::from(MAX_BAKE_EDGE)) as u32
}

/// Unityの `TextureImporter.nPOTScale` に対応する非2のべき乗の扱い。
///
/// 値はUnityのenum（0=None・1=ToNearest・2=ToLarger・3=ToSmaller）と対応する。
/// 不明な値は呼び出し側が [`NpotScale::None`] として扱い、警告する。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NpotScale {
    /// そのままの寸法にする。
    None,
    /// 直近の2のべき乗にする。
    ToNearest,
    /// 切り上げの2のべき乗にする。
    ToLarger,
    /// 切り捨ての2のべき乗にする。
    ToSmaller,
}

/// 取り込み上限（`maxTextureSize`）とNPOTを適用した寸法を求める。
///
/// 長辺が上限を超える場合だけ縦横比を保って縮め、上限以下では拡大しない。
/// 上限が `None`・0の場合は縮めない。その後にNPOTを各辺へ適用する
/// （`None` はそのまま）。戻り値は少なくとも1x1である。
/// 参照実装の書き出し側の半分ずつ（`CappedTextureSerializer.Fitted`）ではなく、
/// Unityの取り込み（`TextureImporter`）と同じく縦横比を保つ。
/// プラットフォーム上書きは使わず、呼び出し側が `DefaultTexturePlatform` か
/// グローバル値のどちらかへ倒してから渡す。
pub fn import_fitted_size(
    width: u32,
    height: u32,
    max_size: Option<u32>,
    npot: NpotScale,
) -> (u32, u32) {
    let (mut fitted_w, mut fitted_h) = (width.max(1), height.max(1));
    if let Some(max) = max_size.filter(|max| *max > 0) {
        let longest = fitted_w.max(fitted_h);
        if longest > max {
            // 整数除算の切り捨てで縦横比を保ち、上限以内に収める。
            // `u64` へ広げるため、32bitの寸法では溢れない。
            fitted_w = (u64::from(width) * u64::from(max) / u64::from(longest)).max(1) as u32;
            fitted_h = (u64::from(height) * u64::from(max) / u64::from(longest)).max(1) as u32;
        }
    }
    match npot {
        NpotScale::None => {}
        NpotScale::ToNearest => {
            fitted_w = nearest_pot(fitted_w);
            fitted_h = nearest_pot(fitted_h);
        }
        NpotScale::ToLarger => {
            fitted_w = ceil_pot(fitted_w);
            fitted_h = ceil_pot(fitted_h);
        }
        NpotScale::ToSmaller => {
            fitted_w = floor_pot(fitted_w);
            fitted_h = floor_pot(fitted_h);
        }
    }
    (fitted_w.max(1), fitted_h.max(1))
}

/// 切り上げの2のべき乗を求める。1以下は1である。
fn ceil_pot(value: u32) -> u32 {
    if value <= 1 {
        return 1;
    }
    // `value - 1` の最上位bitのひとつ上であり、`u64` で求めて飽和させる。
    // 取り込みの寸法（最大でも数万）では飽和に届かない。
    (1u64 << (64 - u64::from(value - 1).leading_zeros())).min(1u64 << 31) as u32
}

/// 切り捨ての2のべき乗を求める。1以下は1である。
fn floor_pot(value: u32) -> u32 {
    if value <= 1 {
        return 1;
    }
    1u32 << (31 - value.leading_zeros())
}

/// 直近の2のべき乗を求める。等距離の場合は大きい方を選ぶ。
fn nearest_pot(value: u32) -> u32 {
    let low = floor_pot(value);
    let high = ceil_pot(value);
    if value - low < high - value {
        low
    } else {
        high
    }
}

/// 面積平均（box）で画素を縮小する。
///
/// `linear` が真の場合は線形空間で平均し（sRGB標識なら復号して平均して
/// 符号化し直す）、偽の場合は符号化済み（sRGB）のまま平均する。
/// アルファはどちらの場合も線形で扱う（符号化済み平均と一致する）。
/// Unityの取り込みはsRGB標識の画像を符号化済み空間で、
/// リニア標識（法線など）を線形空間で縮小するため、この切り分けで近づける。
/// sRGB標識のマスクは色と同じ扱いになるが、標識に従う点は取り込みと同じである。
/// 書き出し側の半分ずつの平均（`TextureHalving`、線形で平均）とは異なる。
///
/// # Panics
///
/// `src` の長さが `width * height * 4` と合わない場合にパニックする。
pub fn box_downscale_rgba(
    src: &[u8],
    width: u32,
    height: u32,
    dst_width: u32,
    dst_height: u32,
    linear: bool,
    srgb: bool,
) -> Vec<u8> {
    assert_eq!(
        src.len(),
        width as usize * height as usize * 4,
        "画素数が寸法と合わない"
    );
    let dst_width = dst_width.max(1);
    let dst_height = dst_height.max(1);
    if (dst_width, dst_height) == (width.max(1), height.max(1)) {
        return src.to_vec();
    }
    let width_f = f64::from(width);
    let height_f = f64::from(height);
    let scale_x = width_f / f64::from(dst_width);
    let scale_y = height_f / f64::from(dst_height);
    // sRGB標識の復号は標本化と同じ参照表を使う。
    let decode = |byte: u8| -> f64 {
        if srgb {
            f64::from(BAKED_SRGB_LUT[usize::from(byte)])
        } else {
            f64::from(byte) / 255.0
        }
    };
    let mut out = vec![0u8; dst_width as usize * dst_height as usize * 4];
    for dy in 0..dst_height {
        let y0 = f64::from(dy) * scale_y;
        let y1 = (y0 + scale_y).min(height_f);
        let ya = y0.floor() as u32;
        let yb = y1.ceil() as u32;
        for dx in 0..dst_width {
            let x0 = f64::from(dx) * scale_x;
            let x1 = (x0 + scale_x).min(width_f);
            let xa = x0.floor() as u32;
            let xb = x1.ceil() as u32;
            let mut sum = [0.0f64; 4];
            let mut area = 0.0f64;
            for y in ya..yb {
                let overlap_y = (f64::from(y) + 1.0).min(y1) - f64::from(y).max(y0);
                if overlap_y <= 0.0 {
                    continue;
                }
                for x in xa..xb {
                    let overlap_x = (f64::from(x) + 1.0).min(x1) - f64::from(x).max(x0);
                    if overlap_x <= 0.0 {
                        continue;
                    }
                    let weight = overlap_x * overlap_y;
                    let at = (y as usize * width as usize + x as usize) * 4;
                    if linear {
                        sum[0] += decode(src[at]) * weight;
                        sum[1] += decode(src[at + 1]) * weight;
                        sum[2] += decode(src[at + 2]) * weight;
                        sum[3] += f64::from(src[at + 3]) / 255.0 * weight;
                    } else {
                        sum[0] += f64::from(src[at]) * weight;
                        sum[1] += f64::from(src[at + 1]) * weight;
                        sum[2] += f64::from(src[at + 2]) * weight;
                        sum[3] += f64::from(src[at + 3]) * weight;
                    }
                    area += weight;
                }
            }
            // 共有範囲は正であり、0除算は起きない。
            debug_assert!(area > 0.0, "共有範囲が正でない");
            let at = (dy as usize * dst_width as usize + dx as usize) * 4;
            if linear {
                // r155+g155 のような中間値は符号化し直すため、量子化は最後の1回だけである。
                let encode = |value: f64| -> u8 {
                    let mean = (value / area).clamp(0.0, 1.0);
                    if srgb {
                        (linear_to_srgb_f32(mean as f32) * 255.0)
                            .round()
                            .clamp(0.0, 255.0) as u8
                    } else {
                        (mean * 255.0).round().clamp(0.0, 255.0) as u8
                    }
                };
                out[at] = encode(sum[0]);
                out[at + 1] = encode(sum[1]);
                out[at + 2] = encode(sum[2]);
                out[at + 3] = ((sum[3] / area).clamp(0.0, 1.0) * 255.0)
                    .round()
                    .clamp(0.0, 255.0) as u8;
            } else {
                for (channel, slot) in out[at..at + 4].iter_mut().enumerate() {
                    *slot = (sum[channel] / area).round().clamp(0.0, 255.0) as u8;
                }
            }
        }
    }
    out
}

/// 焼き入力を取り込み後の寸法へ縮小する。
///
/// `linear` と `srgb` の扱いは [`box_downscale_rgba`] と同じであり、
/// 標本化のsRGB旗とサンプラは保つ。同じ寸法の場合は画素を複製せず返す。
pub fn downscale_bake_image(image: &BakeImage, width: u32, height: u32, linear: bool) -> BakeImage {
    if (width, height) == (image.width, image.height) {
        return BakeImage {
            width: image.width,
            height: image.height,
            rgba: image.rgba.clone(),
            srgb: image.srgb,
            sampler: image.sampler,
        };
    }
    let rgba = box_downscale_rgba(
        &image.rgba,
        image.width,
        image.height,
        width,
        height,
        linear,
        image.srgb,
    );
    BakeImage {
        width: width.max(1),
        height: height.max(1),
        rgba,
        srgb: image.srgb,
        sampler: image.sampler,
    }
}

/// 焼き入力を取り込み後の寸法へ縮小する（所有権版）。
///
/// 同じ寸法の場合は複製せず所有権を移して返し、違う寸法の場合だけ
/// [`downscale_bake_image`] と同じ縮小を行う。直載せ・焼きの両方で
/// 等倍時の全体複製を避けるための入口である。公開の借用版は保つ。
pub(crate) fn downscale_bake_image_owned(
    image: BakeImage,
    width: u32,
    height: u32,
    linear: bool,
) -> BakeImage {
    if (width, height) == (image.width, image.height) {
        return image;
    }
    downscale_bake_image(&image, width, height, linear)
}

/// RGBA8をPNGへ符号化する。
///
/// 取り込み縮小後の直載せ用であり、圧縮は既定の決定論的設定である。
/// 時刻などの非決定要素は含めない。
pub fn encode_rgba_png(raw: &[u8], size: BakeSize) -> Vec<u8> {
    encode_png(raw, size, png::ColorType::Rgba)
}

/// RGBA8を残り合計予算の範囲でPNGへ符号化する。
///
/// PSD由来の直載せ用であり、出力が残り予算を超える場合は確保を続けず
/// [`MaterialErrorKind::LimitExceeded`] で返す。種別は上限超過に統一するが、
/// 文面は原因を誤認させない形にする。`CappedVec` の予算超過は上限超過と
/// 分かる文面にし、符号化器自体の失敗は失敗として伝播する
/// （`expect` に流さない）。時刻などの非決定要素は含めない.
///
/// 行単位で流して符号化するため、全画像分の内部複製を作らない。
/// 追加の確保は行バッファ3行分と出力塊（[`PNG_STREAM_CHUNK`]）だけであり、
/// 既定圧縮経路の圧縮器状態と flate2 の固定バッファ（約32KiB）も含めて
/// 有界に収まる。残り予算の超過は行の途中で打ち切る。圧縮設定は既定の
/// 決定論的な設定のまま保ち、`write_image_data` の一括符号化とは使わない。
///
/// # Errors
///
/// 画素長が寸法と合わない場合は [`MaterialErrorKind::InvalidMaterial`]、
/// 出力が `remaining` を超える場合は [`MaterialErrorKind::LimitExceeded`] を返す。
/// 符号化に失敗する場合も [`MaterialErrorKind::LimitExceeded`] として返す。
pub(crate) fn encode_rgba_png_limited(
    raw: &[u8],
    size: BakeSize,
    remaining: u64,
    target: &str,
) -> Result<Vec<u8>, MaterialError> {
    // 呼び出し側は正確な画素列を渡す。不足・過剰は内部矛盾として受け付けない。
    let stride = (u64::from(size.width))
        .checked_mul(4)
        .and_then(|row| usize::try_from(row).ok())
        .ok_or_else(|| {
            MaterialError::new(
                MaterialErrorKind::InvalidMaterial,
                format!("{target}のPNG符号化に渡す寸法が大きすぎる"),
            )
        })?;
    let expect = (stride as u64)
        .checked_mul(u64::from(size.height))
        .and_then(|len| usize::try_from(len).ok())
        .ok_or_else(|| {
            MaterialError::new(
                MaterialErrorKind::InvalidMaterial,
                format!("{target}のPNG符号化に渡す寸法が大きすぎる"),
            )
        })?;
    if raw.len() != expect {
        return Err(MaterialError::new(
            MaterialErrorKind::InvalidMaterial,
            format!("{target}のPNG符号化に渡す画素数が寸法と合わない"),
        ));
    }
    // 種別は上限超過に統一する。文面は `CappedVec` の予算超過か
    // 符号化器自体の失敗かが分かる形にし、原因を誤認させない。
    let limit = |error: png::EncodingError| {
        MaterialError::new(
            MaterialErrorKind::LimitExceeded,
            format!("{target}のPNG符号化に失敗した（上限超過として扱う）: {error}"),
        )
    };
    let io_limit = |error: std::io::Error| {
        // `CappedVec` の書き込み失敗は予算超過だけであり、
        // その他の入出力失敗は起きない。
        MaterialError::new(
            MaterialErrorKind::LimitExceeded,
            format!("{target}のPNG符号化が上限を超えた: {error}"),
        )
    };
    let mut capped = CappedVec::new(remaining);
    let mut encoder = png::Encoder::new(&mut capped, size.width, size.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(limit)?;
    {
        let mut stream = writer
            .stream_writer_with_size(PNG_STREAM_CHUNK)
            .map_err(limit)?;
        for row in raw.chunks_exact(stride) {
            std::io::Write::write_all(&mut stream, row).map_err(io_limit)?;
        }
        // データ不足や圧縮失敗をここで伝播する。Dropに任せると握り潰す。
        stream.finish().map_err(limit)?;
    }
    // IENDを明示的に書き、終端の失敗も握り潰さない。
    writer.finish().map_err(limit)?;
    Ok(capped.into_inner())
}

/// 行単位の符号化に使う出力塊の大きさ。
///
/// `StreamWriter` の内部塊であり、行バッファ（3行分）とは別に有界に保つ。
/// 全画像分の内部複製を作らないための上限であり、出力の内容には影響しない。
const PNG_STREAM_CHUNK: usize = 4096;

/// 残り予算で打ち切る書き込み先。超過分は書かず失敗させる。
struct CappedVec {
    /// 書き込んだバイト列。
    inner: Vec<u8>,
    /// あと書けるバイト数。
    remaining: u64,
}

impl CappedVec {
    /// 書き込み先を作る。
    fn new(remaining: u64) -> Self {
        Self {
            inner: Vec::new(),
            remaining,
        }
    }

    /// 書き込んだバイト列を取り出す。
    fn into_inner(self) -> Vec<u8> {
        self.inner
    }
}

impl std::io::Write for CappedVec {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let len = buf.len() as u64;
        if len > self.remaining {
            return Err(std::io::Error::new(
                std::io::ErrorKind::QuotaExceeded,
                "PNG符号化の出力が残り合計予算を超えた",
            ));
        }
        self.remaining -= len;
        self.inner.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// 基準色合成の入力（`BaseColorComposite` パスと対応）。
///
/// 色調補正・マスクは任意であり、カラーレイヤーとデカールは
/// 今回の対象外のため持たない（ある場合は焼かずに直載せする）。
#[derive(Debug, Clone)]
pub struct BaseColorJob {
    /// 主入力。無い場合は白から始める。
    pub source: Option<BakeInput>,
    /// `BaseTint`（線形・アルファ付き）。今回は白である。
    pub base_tint: [f32; 4],
    /// `_MainTexHSVG`（色相・彩度・明度・ガンマ）。
    pub hsvg: Option<[f32; 4]>,
    /// 色調補正の適用マスク（スカラー）。
    pub color_adjust_mask: Option<BakeInput>,
    /// アルファマスク（`_AlphaMaskMode` 1置換・2乗算・3加算・4減算）。
    pub alpha_mask: Option<AlphaMaskInput>,
    /// 出力の `_Target`（今回は単位）。
    pub target: [f64; 4],
    /// 出力寸法。
    pub size: BakeSize,
}

/// アルファマスクの適用方法。
#[derive(Debug, Clone)]
pub struct AlphaMaskInput {
    /// マスク画像（Rチャンネル・スカラー）。
    pub mask: BakeInput,
    /// 適用方式（1置換・2乗算・3加算・4減算）。
    pub mode: i64,
    /// `saturate(r * scale + value)` の係数。
    pub scale: f32,
    /// 同上の切片。
    pub value: f32,
}

/// 基準色を合成する（`Frag` の `BaseColorComposite` 節と対応）。
///
/// 主入力へ色調補正・基準色掛け・アルファマスクを順に適用する。
/// カラーレイヤーとデカールは対象外のため描かない。
pub fn bake_base_color(job: &BaseColorJob) -> Vec<[f32; 4]> {
    let mut out = Vec::with_capacity(job.size.width as usize * job.size.height as usize);
    let hsvg_on = job.hsvg.is_some();
    let hsvg = job.hsvg.unwrap_or([0.0, 1.0, 1.0, 1.0]);
    for oy in 0..job.size.height {
        for ox in 0..job.size.width {
            let uv = [
                (f64::from(ox) + 0.5) / f64::from(job.size.width),
                (f64::from(oy) + 0.5) / f64::from(job.size.height),
            ];
            let uv0 = mesh_uv(uv, job.target);
            let mut color = match &job.source {
                Some(source) => tap_color(source, uv0),
                None => [1.0, 1.0, 1.0, 1.0],
            };
            if hsvg_on {
                let adjust = match &job.color_adjust_mask {
                    Some(mask) => tap_scalar(mask, uv0),
                    None => 1.0,
                };
                let toned = tone_correction([color[0], color[1], color[2]], hsvg);
                color[0] += (toned[0] - color[0]) * adjust;
                color[1] += (toned[1] - color[1]) * adjust;
                color[2] += (toned[2] - color[2]) * adjust;
            }
            color[0] *= job.base_tint[0];
            color[1] *= job.base_tint[1];
            color[2] *= job.base_tint[2];
            color[3] *= job.base_tint[3];
            if let Some(alpha) = &job.alpha_mask {
                let mut mask =
                    tap_scalar(&alpha.mask, uv0).clamp(0.0, 1.0) * alpha.scale + alpha.value;
                mask = mask.clamp(0.0, 1.0);
                match alpha.mode {
                    1 => color[3] = mask,
                    2 => color[3] *= mask,
                    3 => color[3] = (color[3] + mask).min(1.0),
                    _ => color[3] = (color[3] - mask).max(0.0),
                }
            }
            out.push(color);
        }
    }
    out
}

/// チャンネル詰め替えの入力（`ChannelRepack` パスと対応）。
#[derive(Debug, Clone)]
pub struct RepackJob {
    /// 読み先。
    pub source: BakeInput,
    /// 出力の `_Target`（今回は単位）。
    pub target: [f64; 4],
    /// 出力寸法。
    pub size: BakeSize,
}

/// 1チャンネルを3成分へ広げる（`FragRepack` と対応）。
///
/// MToonが読むG（アウトライン幅）などへ載せ替える。
/// 出力は線形データであり、sRGB符号化しない。
pub fn bake_channel_repack(job: &RepackJob) -> Vec<[f32; 4]> {
    let mut out = Vec::with_capacity(job.size.width as usize * job.size.height as usize);
    for oy in 0..job.size.height {
        for ox in 0..job.size.width {
            let uv = [
                (f64::from(ox) + 0.5) / f64::from(job.size.width),
                (f64::from(oy) + 0.5) / f64::from(job.size.height),
            ];
            let value = tap_scalar(&job.source, mesh_uv(uv, job.target));
            out.push([value, value, value, 1.0]);
        }
    }
    out
}

/// 陰影合成の入力（`ShadeComposite` パスと対応）。
#[derive(Debug, Clone)]
pub struct ShadeJob {
    /// 1層目の陰影色テクスチャ。無い場合は透明黒定数である。
    pub source: Option<BakeInput>,
    /// アルベド（焼いた基準色か主テクスチャ）。無い場合は白である。
    pub albedo: Option<BakeInput>,
    /// アルベドに掛ける色（線形・アルファ付き）。
    pub albedo_color: [f32; 4],
    /// `_ShadowColor`（線形・アルファ付き）。
    pub shadow_color: [f32; 4],
    /// `_Strength`。
    pub strength: f32,
    /// 1層目の重み（`_ShadowWeight`）。
    pub shadow_weight: f32,
    /// 2層目以降の持ち分（`_ShadowExtra`・アルベド掛け前）。
    pub shadow_extra: [f32; 3],
    /// `_ShadowMainStrength`（アルベド掛けの強さ）。
    pub main_strength: f32,
    /// 強さマスク（スカラー）。
    pub strength_mask: Option<BakeInput>,
    /// 出力の `_Target`（今回は単位）。
    pub target: [f64; 4],
    /// 出力寸法。
    pub size: BakeSize,
}

/// 陰影色を合成する（`Frag` の `ShadeComposite` 節と対応）。
pub fn bake_shade(job: &ShadeJob) -> Vec<[f32; 4]> {
    let mut out = Vec::with_capacity(job.size.width as usize * job.size.height as usize);
    for oy in 0..job.size.height {
        for ox in 0..job.size.width {
            let uv = [
                (f64::from(ox) + 0.5) / f64::from(job.size.width),
                (f64::from(oy) + 0.5) / f64::from(job.size.height),
            ];
            let uv0 = mesh_uv(uv, job.target);
            let albedo = match &job.albedo {
                Some(input) => tap_color(input, uv0),
                None => [1.0, 1.0, 1.0, 1.0],
            };
            let albedo = [
                albedo[0] * job.albedo_color[0],
                albedo[1] * job.albedo_color[1],
                albedo[2] * job.albedo_color[2],
                albedo[3] * job.albedo_color[3],
            ];
            let shadow_tex = match &job.source {
                Some(source) => tap_color(source, uv0),
                None => [0.0, 0.0, 0.0, 0.0],
            };
            let mut indirect = [
                (albedo[0] + (shadow_tex[0] - albedo[0]) * shadow_tex[3])
                    * job.shadow_color[0]
                    * job.shadow_weight
                    + albedo[0] * job.shadow_extra[0],
                (albedo[1] + (shadow_tex[1] - albedo[1]) * shadow_tex[3])
                    * job.shadow_color[1]
                    * job.shadow_weight
                    + albedo[1] * job.shadow_extra[1],
                (albedo[2] + (shadow_tex[2] - albedo[2]) * shadow_tex[3])
                    * job.shadow_color[2]
                    * job.shadow_weight
                    + albedo[2] * job.shadow_extra[2],
            ];
            // `lerp(indirect, indirect * albedo, _MainStrength)` の写しである。
            indirect[0] += (indirect[0] * albedo[0] - indirect[0]) * job.main_strength;
            indirect[1] += (indirect[1] * albedo[1] - indirect[1]) * job.main_strength;
            indirect[2] += (indirect[2] * albedo[2] - indirect[2]) * job.main_strength;
            indirect[0] = indirect[0].min(albedo[0]);
            indirect[1] = indirect[1].min(albedo[1]);
            indirect[2] = indirect[2].min(albedo[2]);
            let mask = match &job.strength_mask {
                Some(input) => tap_scalar(input, uv0),
                None => 1.0,
            };
            let strength = job.strength * mask;
            out.push([
                albedo[0] + (indirect[0] - albedo[0]) * strength,
                albedo[1] + (indirect[1] - albedo[1]) * strength,
                albedo[2] + (indirect[2] - albedo[2]) * strength,
                1.0,
            ]);
        }
    }
    out
}

/// 発光合成のマスク（RGB保持かスカラーか）。
#[derive(Debug, Clone)]
pub struct EmissionMask {
    /// 読み先。
    pub input: BakeInput,
    /// 真の場合は1成分として掛け、偽の場合はRGBで掛ける。
    pub scalar: bool,
}

/// 発光合成の入力（`EmissionComposite` パスと対応）。
#[derive(Debug, Clone)]
pub struct EmissionJob {
    /// 発光マップ。無い場合は白から始める。
    pub source: Option<BakeInput>,
    /// 発光色（`_EmissionTint`・線形RGB）。
    pub emission_tint: [f32; 3],
    /// アルベド（無い場合は白）。
    pub albedo: Option<BakeInput>,
    /// アルベドに掛ける色（線形・アルファ付き）。
    pub albedo_color: [f32; 4],
    /// `_MainStrength`（アルベド掛けの強さ）。
    pub main_strength: f32,
    /// 発光マスク。
    pub mask: Option<EmissionMask>,
    /// さらに掛けるスカラー（Poiyomiの発光マップのアルファ）。
    pub scale: Option<BakeInput>,
    /// 折り込むMatCapの平均色（線形・重み込み）。無い場合は足さない。
    pub fold_color: Option<[f32; 3]>,
    /// 平均色に掛けるマスク。
    pub fold_mask: Option<EmissionMask>,
    /// 出力の `_Target`（今回は単位）。
    pub target: [f64; 4],
    /// 出力寸法。
    pub size: BakeSize,
}

/// マスクを発光に掛ける（`Masked` の発光側の写し）。
fn apply_emission_mask(emission: [f32; 3], mask: &EmissionMask, uv0: [f64; 2]) -> [f32; 3] {
    if mask.scalar {
        let value = tap_scalar(&mask.input, uv0);
        [
            emission[0] * value,
            emission[1] * value,
            emission[2] * value,
        ]
    } else {
        let factor = tap_masked(&mask.input, uv0);
        [
            emission[0] * factor[0],
            emission[1] * factor[1],
            emission[2] * factor[2],
        ]
    }
}

/// 発光を合成する（`Frag` の `EmissionComposite` 節と対応）。
pub fn bake_emission(job: &EmissionJob) -> Vec<[f32; 4]> {
    let mut out = Vec::with_capacity(job.size.width as usize * job.size.height as usize);
    for oy in 0..job.size.height {
        for ox in 0..job.size.width {
            let uv = [
                (f64::from(ox) + 0.5) / f64::from(job.size.width),
                (f64::from(oy) + 0.5) / f64::from(job.size.height),
            ];
            let uv0 = mesh_uv(uv, job.target);
            let base = match &job.source {
                Some(source) => tap_color(source, uv0),
                None => [1.0, 1.0, 1.0, 1.0],
            };
            let mut emission = [
                base[0] * job.emission_tint[0],
                base[1] * job.emission_tint[1],
                base[2] * job.emission_tint[2],
            ];
            let albedo = match &job.albedo {
                Some(input) => tap_color(input, uv0),
                None => [1.0, 1.0, 1.0, 1.0],
            };
            let albedo = [
                albedo[0] * job.albedo_color[0],
                albedo[1] * job.albedo_color[1],
                albedo[2] * job.albedo_color[2],
            ];
            emission[0] += (emission[0] * albedo[0] - emission[0]) * job.main_strength;
            emission[1] += (emission[1] * albedo[1] - emission[1]) * job.main_strength;
            emission[2] += (emission[2] * albedo[2] - emission[2]) * job.main_strength;
            if let Some(mask) = &job.mask {
                emission = apply_emission_mask(emission, mask, uv0);
            }
            if let Some(scale) = &job.scale {
                let value = tap_scalar(scale, uv0);
                emission[0] *= value;
                emission[1] *= value;
                emission[2] *= value;
            }
            if let Some(fold) = &job.fold_color {
                let addition = match &job.fold_mask {
                    Some(mask) => apply_emission_mask(*fold, mask, uv0),
                    None => *fold,
                };
                emission[0] += addition[0];
                emission[1] += addition[1];
                emission[2] += addition[2];
            }
            out.push([emission[0], emission[1], emission[2], 1.0]);
        }
    }
    out
}

/// 法線合成の入力（`NormalComposite` パスと対応）。
#[derive(Debug, Clone)]
pub struct NormalJob {
    /// 1枚目の法線マップ。無い場合は平面である。
    pub first: Option<BakeInput>,
    /// 1枚目の強さ。
    pub first_scale: f32,
    /// 2枚目の法線マップ。無い場合は平面である。
    pub second: Option<BakeInput>,
    /// 2枚目の強さ（マスク掛け前）。
    pub second_scale: f32,
    /// 2枚目の強さマスク（スカラー）。
    pub scale_mask: Option<BakeInput>,
    /// 出力の `_Target`（今回は単位）。
    pub target: [f64; 4],
    /// 出力寸法。
    pub size: BakeSize,
}

/// 法線を合成する（`Frag` の `NormalComposite` 節と対応）。
///
/// 2枚を `UnpackScaled` で戻して `(xyの和, zの積)` で混ぜて正規化し、
/// 強さ1用の `n * 0.5 + 0.5` で書く。出力後の法線 scale は1である。
pub fn bake_normal(job: &NormalJob) -> Vec<[f32; 4]> {
    let mut out = Vec::with_capacity(job.size.width as usize * job.size.height as usize);
    for oy in 0..job.size.height {
        for ox in 0..job.size.width {
            let uv = [
                (f64::from(ox) + 0.5) / f64::from(job.size.width),
                (f64::from(oy) + 0.5) / f64::from(job.size.height),
            ];
            let uv0 = mesh_uv(uv, job.target);
            let first = match &job.first {
                Some(input) => unpack_scaled(tap_color(input, uv0), job.first_scale),
                None => [0.0, 0.0, 1.0],
            };
            let mask = match &job.scale_mask {
                Some(input) => tap_scalar(input, uv0),
                None => 1.0,
            };
            let second = match &job.second {
                Some(input) => unpack_scaled(tap_color(input, uv0), job.second_scale * mask),
                None => [0.0, 0.0, 1.0],
            };
            let mut normal = [
                first[0] + second[0],
                first[1] + second[1],
                first[2] * second[2],
            ];
            let length =
                (normal[0] * normal[0] + normal[1] * normal[1] + normal[2] * normal[2]).sqrt();
            if length > 1e-6 {
                normal[0] /= length;
                normal[1] /= length;
                normal[2] /= length;
            } else {
                normal = [0.0, 0.0, 1.0];
            }
            out.push([
                normal[0] * 0.5 + 0.5,
                normal[1] * 0.5 + 0.5,
                normal[2] * 0.5 + 0.5,
                1.0,
            ]);
        }
    }
    out
}

/// リム乗算合成の入力（`RimMultiplyComposite` パスと対応）。
#[derive(Debug, Clone)]
pub struct RimMultiplyJob {
    /// 1枚目の倍率テクスチャ。無い場合は1から始める。
    pub source: Option<BakeInput>,
    /// 2枚目の倍率テクスチャ。無い場合は掛けない。
    pub mask: Option<BakeInput>,
    /// 3枚目の倍率テクスチャ。無い場合は掛けない。
    pub scale: Option<BakeInput>,
    /// アルベド（焼いた基準色か主テクスチャ）。無い場合は白である。
    pub albedo: Option<BakeInput>,
    /// アルベドに掛ける色（線形・アルファ付き）。
    pub albedo_color: [f32; 4],
    /// アルベドtint。無い場合は掛けない。
    pub tint: Option<AlbedoTint>,
    /// 0〜1へ丸める前に掛ける倍率。色係数に入らない明るさを運ぶ。
    pub gain: f32,
    /// 出力の `_Target`（今回は単位）。
    pub target: [f64; 4],
    /// 出力寸法。
    pub size: BakeSize,
}

/// リム乗算テクスチャを合成する（`Frag` の `RimMultiplyComposite` 節と対応）。
///
/// 各入力を [`tap_masked`] でRGBへ直し（スカラーは3成分へ広げる）、
/// 積にアルベドtint（`lerp(1, albedo, mix) * (constant + albedo係数 * albedo)`）を掛け、
/// `gain` を掛けて0〜1へ丸める。`AlbedoTint::None` は参照実装の呼び出し側で
/// 渡さない（tint無しと対応）のと同じく、ここでも `None` で扱う。
pub fn bake_rim_multiply(job: &RimMultiplyJob) -> Vec<[f32; 4]> {
    let mut out = Vec::with_capacity(job.size.width as usize * job.size.height as usize);
    for oy in 0..job.size.height {
        for ox in 0..job.size.width {
            let uv = [
                (f64::from(ox) + 0.5) / f64::from(job.size.width),
                (f64::from(oy) + 0.5) / f64::from(job.size.height),
            ];
            let uv0 = mesh_uv(uv, job.target);
            let mut tinted = [1.0f32; 3];
            for input in [&job.source, &job.mask, &job.scale].into_iter().flatten() {
                let factor = tap_masked(input, uv0);
                tinted[0] *= factor[0];
                tinted[1] *= factor[1];
                tinted[2] *= factor[2];
            }
            if let Some(tint) = &job.tint {
                let albedo = match &job.albedo {
                    Some(input) => tap_color(input, uv0),
                    None => [1.0, 1.0, 1.0, 1.0],
                };
                let albedo = [
                    albedo[0] * job.albedo_color[0],
                    albedo[1] * job.albedo_color[1],
                    albedo[2] * job.albedo_color[2],
                ];
                let mixed = tint.at_color(albedo);
                tinted[0] *= mixed[0];
                tinted[1] *= mixed[1];
                tinted[2] *= mixed[2];
            }
            out.push([
                (tinted[0] * job.gain).clamp(0.0, 1.0),
                (tinted[1] * job.gain).clamp(0.0, 1.0),
                (tinted[2] * job.gain).clamp(0.0, 1.0),
                1.0,
            ]);
        }
    }
    out
}

/// 色出力をPNGへ符号化する。RGBはsRGB符号化し、アルファはそのまま書く。
///
/// 参照実装のsRGBレンダーターゲットと同じく、範囲外は0〜1へ丸める。
/// 符号化に時刻などの非決定要素は含めない。
pub fn encode_color_png(pixels: &[[f32; 4]], size: BakeSize) -> Vec<u8> {
    let mut raw = Vec::with_capacity(pixels.len() * 4);
    for pixel in pixels {
        raw.push(
            (linear_to_srgb_f32(pixel[0].clamp(0.0, 1.0)) * 255.0)
                .round()
                .clamp(0.0, 255.0) as u8,
        );
        raw.push(
            (linear_to_srgb_f32(pixel[1].clamp(0.0, 1.0)) * 255.0)
                .round()
                .clamp(0.0, 255.0) as u8,
        );
        raw.push(
            (linear_to_srgb_f32(pixel[2].clamp(0.0, 1.0)) * 255.0)
                .round()
                .clamp(0.0, 255.0) as u8,
        );
        raw.push((pixel[3].clamp(0.0, 1.0) * 255.0).round().clamp(0.0, 255.0) as u8);
    }
    encode_png(&raw, size, png::ColorType::Rgba)
}

/// データ出力をPNGへ符号化する。線形のままRGBで書く（アルファは落とす）。
///
/// 参照実装の線形レンダーターゲットと同じく、範囲外は0〜1へ丸める。
/// 参照実装の焼き画像（陰影・発光詰め替え・法線）もRGBである。
pub fn encode_data_png(pixels: &[[f32; 4]], size: BakeSize) -> Vec<u8> {
    let mut raw = Vec::with_capacity(pixels.len() * 3);
    for pixel in pixels {
        raw.push((pixel[0].clamp(0.0, 1.0) * 255.0).round().clamp(0.0, 255.0) as u8);
        raw.push((pixel[1].clamp(0.0, 1.0) * 255.0).round().clamp(0.0, 255.0) as u8);
        raw.push((pixel[2].clamp(0.0, 1.0) * 255.0).round().clamp(0.0, 255.0) as u8);
    }
    encode_png(&raw, size, png::ColorType::Rgb)
}

/// PNGへ符号化する。圧縮は既定の決定論的な設定である。
fn encode_png(raw: &[u8], size: BakeSize, color: png::ColorType) -> Vec<u8> {
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, size.width, size.height);
    encoder.set_color(color);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder
        .write_header()
        .expect("メモリへのPNG符号化は失敗しない");
    writer
        .write_image_data(raw)
        .expect("メモリへのPNG符号化は失敗しない");
    drop(writer);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 1x1のRGBA8画像を作る。
    fn solid_rgba(pixel: [u8; 4], srgb: bool) -> BakeImage {
        BakeImage {
            width: 1,
            height: 1,
            rgba: pixel.to_vec(),
            srgb,
            sampler: VrmSampler::default(),
        }
    }

    fn input_of(image: BakeImage, channel: BakeChannel) -> BakeInput {
        BakeInput {
            image: Rc::new(image),
            st: BakeSt::identity(),
            channel,
            invert: false,
        }
    }

    /// PNG符号化して復号する。
    fn roundtrip_png(pixels: &[[f32; 4]], size: BakeSize, color: bool) -> BakeImage {
        let bytes = if color {
            encode_color_png(pixels, size)
        } else {
            encode_data_png(pixels, size)
        };
        load_bake_image(
            &bytes,
            VrmImageMime::Png,
            color,
            VrmSampler::default(),
            u64::MAX,
        )
        .expect("自作のPNGは復号できる")
    }

    #[test]
    fn 画素の往復は量子化以内に収まる() {
        let pixels = [[0.2, 0.5, 0.8, 0.35]];
        let size = BakeSize {
            width: 1,
            height: 1,
        };
        let back = roundtrip_png(&pixels, size, true);
        let got = tap_color(&input_of(back, BakeChannel::Rgb), [0.5, 0.5]);
        for (channel, want) in got.iter().zip(pixels[0].iter()) {
            // sRGB符号化の量子化（1/255）と復号の誤差だけ許す。
            assert!((channel - want).abs() < 0.006, "{got:?}");
        }
    }

    #[test]
    fn データの往復は量子化以内に収まる() {
        let pixels = [[0.0, 0.5019608, 1.0, 1.0]];
        let size = BakeSize {
            width: 1,
            height: 1,
        };
        let back = roundtrip_png(&pixels, size, false);
        let got = tap_color(&input_of(back, BakeChannel::Rgb), [0.5, 0.5]);
        assert!((got[0] - 0.0).abs() < 0.002, "{got:?}");
        assert!((got[1] - 0.5019608).abs() < 0.003, "{got:?}");
        assert!((got[2] - 1.0).abs() < 0.002, "{got:?}");
    }

    #[test]
    fn グレースケールと16bitとパレットを読める() {
        // 8bitグレースケール2x1。
        let gray = {
            let mut out = Vec::new();
            let mut encoder = png::Encoder::new(&mut out, 2, 1);
            encoder.set_color(png::ColorType::Grayscale);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(&[0x00, 0xFF]).unwrap();
            drop(writer);
            out
        };
        let image = load_bake_image(
            &gray,
            VrmImageMime::Png,
            false,
            VrmSampler::default(),
            u64::MAX,
        )
        .unwrap();
        assert_eq!((image.width(), image.height()), (2, 1));
        let left = tap_color(&input_of(image, BakeChannel::Rgb), [0.25, 0.5]);
        assert_eq!(left[0], 0.0);
        assert_eq!(left[3], 1.0);

        // 16bit RGB 1x1。
        let wide = {
            let mut out = Vec::new();
            let mut encoder = png::Encoder::new(&mut out, 1, 1);
            encoder.set_color(png::ColorType::Rgb);
            encoder.set_depth(png::BitDepth::Sixteen);
            let mut writer = encoder.write_header().unwrap();
            writer
                .write_image_data(&[0x80u8, 0x00, 0x80, 0x00, 0x80, 0x00])
                .unwrap();
            drop(writer);
            out
        };
        let image = load_bake_image(
            &wide,
            VrmImageMime::Png,
            false,
            VrmSampler::default(),
            u64::MAX,
        )
        .unwrap();
        let got = tap_color(&input_of(image, BakeChannel::Rgb), [0.5, 0.5]);
        // 0x8000/0xFFFF = 0.5000076… → 丸め後は128/255付近である。
        assert!((got[0] - 0.5).abs() < 0.004, "{got:?}");

        // パレット1x1（赤）。
        let paletted = {
            let mut out = Vec::new();
            let mut encoder = png::Encoder::new(&mut out, 1, 1);
            encoder.set_color(png::ColorType::Indexed);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.set_palette(vec![0xFF, 0x00, 0x00]);
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(&[0x00]).unwrap();
            drop(writer);
            out
        };
        let image = load_bake_image(
            &paletted,
            VrmImageMime::Png,
            true,
            VrmSampler::default(),
            u64::MAX,
        )
        .unwrap();
        let got = tap_color(&input_of(image, BakeChannel::Rgb), [0.5, 0.5]);
        assert!((got[0] - 1.0).abs() < 0.002, "{got:?}");
        assert!(got[1] < 0.002 && got[2] < 0.002, "{got:?}");
    }

    #[test]
    fn 壊れた画像は失敗する() {
        assert!(
            load_bake_image(
                &[0x89, 0x50],
                VrmImageMime::Png,
                true,
                VrmSampler::default(),
                u64::MAX,
            )
            .is_err()
        );
        assert!(
            load_bake_image(
                &[0xFF, 0xD8, 0x00],
                VrmImageMime::Jpeg,
                true,
                VrmSampler::default(),
                u64::MAX,
            )
            .is_err()
        );
    }

    #[test]
    fn 双線形は中心で平均になる() {
        // 2x2: 左上=黒・右上=赤・左下=緑・右下=白（線形）。
        let image = BakeImage {
            width: 2,
            height: 2,
            rgba: vec![
                0, 0, 0, 255, 255, 0, 0, 255, 0, 255, 0, 255, 255, 255, 255, 255,
            ],
            srgb: false,
            sampler: VrmSampler::default(),
        };
        let got = tap_color(&input_of(image, BakeChannel::Rgb), [0.5, 0.5]);
        assert!((got[0] - 0.5).abs() < 1e-6, "{got:?}");
        assert!((got[1] - 0.5).abs() < 1e-6, "{got:?}");
        assert!((got[2] - 0.25).abs() < 1e-6, "{got:?}");
    }

    #[test]
    fn 繰り返しと端止めと鏡像の境界が正しい() {
        // 2x1: 左=黒・右=白（線形）。
        let striped = |wrap| BakeImage {
            width: 2,
            height: 1,
            rgba: vec![0, 0, 0, 255, 255, 255, 255, 255],
            srgb: false,
            sampler: VrmSampler {
                wrap_u: wrap,
                wrap_v: VrmWrap::Repeat,
                ..VrmSampler::default()
            },
        };
        // 繰り返し：-0.25は右端（白側）の中央である。
        let got = tap_color(
            &input_of(striped(VrmWrap::Repeat), BakeChannel::Rgb),
            [-0.25, 0.5],
        );
        assert!((got[0] - 1.0).abs() < 1e-6, "{got:?}");
        // 端止め：範囲外は端の値である。
        let got = tap_color(
            &input_of(striped(VrmWrap::ClampToEdge), BakeChannel::Rgb),
            [-1.0, 0.5],
        );
        assert!((got[0] - 0.0).abs() < 1e-6, "{got:?}");
        // 鏡像：-0.375は0.375と同じ（両端の中間）である。
        let got = tap_color(
            &input_of(striped(VrmWrap::MirroredRepeat), BakeChannel::Rgb),
            [-0.375, 0.5],
        );
        assert!((got[0] - 0.25).abs() < 1e-6, "{got:?}");
    }

    #[test]
    fn 点補間は最近傍を拾う() {
        let image = BakeImage {
            width: 2,
            height: 1,
            rgba: vec![0, 0, 0, 255, 255, 255, 255, 255],
            srgb: false,
            sampler: VrmSampler {
                filter: VrmFilter::Nearest,
                ..VrmSampler::default()
            },
        };
        let got = tap_color(&input_of(image, BakeChannel::Rgb), [0.7, 0.5]);
        assert!((got[0] - 1.0).abs() < 1e-6, "{got:?}");
    }

    #[test]
    fn 変換とチャンネル選択と反転が適用される() {
        // 1x1赤（sRGB旗あり：復号後は線形1である）。
        let red = solid_rgba([255, 0, 0, 255], true);
        let scaled = BakeInput {
            image: Rc::new(red),
            st: BakeSt {
                scale: [2.0, 2.0],
                offset: [0.5, 0.5],
            },
            channel: BakeChannel::Rgb,
            invert: false,
        };
        // uv0=(0,0) → (0.5,0.5) → 赤のままである。
        let got = tap_color(&scaled, [0.0, 0.0]);
        assert!((got[0] - 1.0).abs() < 1e-6, "{got:?}");

        // G選択は0、反転は1である（選択がRgbの場合はRを読む）。
        let green_read = BakeInput {
            image: Rc::clone(&scaled.image),
            channel: BakeChannel::G,
            ..scaled
        };
        assert_eq!(tap_scalar(&green_read, [0.0, 0.0]), 0.0);
        let rgb_read = BakeInput {
            image: Rc::clone(&scaled.image),
            channel: BakeChannel::Rgb,
            ..scaled
        };
        assert!((tap_scalar(&rgb_read, [0.0, 0.0]) - 1.0).abs() < 1e-6);
        let inverted = BakeInput {
            image: Rc::clone(&scaled.image),
            channel: BakeChannel::R,
            invert: true,
            ..scaled
        };
        assert!((tap_scalar(&inverted, [0.0, 0.0]) - 0.0).abs() < 1e-6);

        // マスク：Rgbは色、スカラーは広げる。
        let masked = tap_masked(&rgb_read, [0.0, 0.0]);
        assert!(
            (masked[0] - 1.0).abs() < 1e-6 && masked[1] == 0.0,
            "{masked:?}"
        );
        let lone = tap_masked(&inverted, [0.0, 0.0]);
        assert_eq!(lone, [0.0, 0.0, 0.0]);
    }

    #[test]
    fn 同じ焼きは同じバイト列になる() {
        let image = solid_rgba([10, 128, 200, 255], true);
        let job = RepackJob {
            source: input_of(image, BakeChannel::R),
            target: [1.0, 1.0, 0.0, 0.0],
            size: BakeSize {
                width: 4,
                height: 4,
            },
        };
        let first = encode_data_png(&bake_channel_repack(&job), job.size);
        let second = encode_data_png(&bake_channel_repack(&job), job.size);
        assert_eq!(first, second);
    }

    #[test]
    fn 陰影の1画素が手計算と合う() {
        // アルベド0.5・影テクスチャ0.25（a=1）・影色白・強さ1・マスク無し。
        let albedo = solid_rgba([188, 188, 188, 255], true);
        let shadow = solid_rgba([128, 128, 128, 255], true);
        let linear = |byte: u8| decode_byte(byte, true);
        let albedo_lin = linear(188);
        let shadow_lin = linear(128);
        let job = ShadeJob {
            source: Some(input_of(shadow, BakeChannel::Rgb)),
            albedo: Some(input_of(albedo, BakeChannel::Rgb)),
            albedo_color: [1.0, 1.0, 1.0, 1.0],
            shadow_color: [1.0, 1.0, 1.0, 1.0],
            strength: 1.0,
            shadow_weight: 1.0,
            shadow_extra: [0.0, 0.0, 0.0],
            main_strength: 0.0,
            strength_mask: None,
            target: [1.0, 1.0, 0.0, 0.0],
            size: BakeSize {
                width: 1,
                height: 1,
            },
        };
        let got = bake_shade(&job);
        // indirect = shadow（a=1でlerp）→ minで抑え → strength=1でindirect。
        let indirect = shadow_lin.min(albedo_lin);
        assert!((got[0][0] - indirect).abs() < 1e-6, "{got:?}");
        assert_eq!(got[0][3], 1.0);
    }

    #[test]
    fn 発光の折り込みは加算になる() {
        let job = EmissionJob {
            source: None,
            emission_tint: [0.0, 0.0, 0.0],
            albedo: None,
            albedo_color: [1.0, 1.0, 1.0, 1.0],
            main_strength: 0.0,
            mask: None,
            scale: None,
            fold_color: Some([0.1, 0.2, 0.3]),
            fold_mask: None,
            target: [1.0, 1.0, 0.0, 0.0],
            size: BakeSize {
                width: 1,
                height: 1,
            },
        };
        let got = bake_emission(&job);
        assert!((got[0][0] - 0.1).abs() < 1e-6, "{got:?}");
        assert!((got[0][1] - 0.2).abs() < 1e-6, "{got:?}");
        assert!((got[0][2] - 0.3).abs() < 1e-6, "{got:?}");
    }

    #[test]
    fn 法線の平面は上向きになる() {
        let job = NormalJob {
            first: None,
            first_scale: 1.0,
            second: None,
            second_scale: 1.0,
            scale_mask: None,
            target: [1.0, 1.0, 0.0, 0.0],
            size: BakeSize {
                width: 1,
                height: 1,
            },
        };
        let got = bake_normal(&job);
        assert_eq!(got[0], [0.5, 0.5, 1.0, 1.0]);
    }

    #[test]
    fn 平均色は一様な色になる() {
        // 一様な赤（sRGB）・不透明。
        let image = Rc::new(BakeImage {
            width: 8,
            height: 8,
            rgba: [255, 0, 0, 255].repeat(64),
            srgb: true,
            sampler: VrmSampler::default(),
        });
        let got = matcap_disc_average(&image, &BakeSt::identity());
        assert!((got[0] - 1.0).abs() < 0.01, "{got:?}");
        assert!(got[1] < 0.01 && got[2] < 0.01, "{got:?}");
        // sRGB旗の有無を直接確認する。
        assert!(image.srgb);
        assert_eq!(image.sampler.wrap_u, VrmWrap::Repeat);
    }

    #[test]
    fn 層の重みは合計が1になる() {
        let weights = shade_layer_weights(0.5, 0.1, &[(0.15, 0.1, 0.8)]);
        let total: f64 = weights.iter().sum();
        assert!((total - 1.0).abs() < 1e-9, "{weights:?}");
        assert_eq!(weights.len(), 2);
        let single = shade_layer_weights(0.5, 0.1, &[]);
        assert_eq!(single, vec![1.0]);
    }

    #[test]
    fn 色調補正の単位元は不変である() {
        let color = [0.3, 0.5, 0.7];
        let got = tone_correction(color, [0.0, 1.0, 1.0, 1.0]);
        for (channel, want) in got.iter().zip(color.iter()) {
            assert!((channel - want).abs() < 1e-5, "{got:?}");
        }
    }

    #[test]
    fn 取り込み上限は長辺だけを縦横比どおりに縮める() {
        // 上限なし・0はそのままにし、上限以下では拡大しない。
        assert_eq!(
            import_fitted_size(4096, 4096, None, NpotScale::None),
            (4096, 4096)
        );
        assert_eq!(
            import_fitted_size(512, 512, Some(0), NpotScale::None),
            (512, 512)
        );
        assert_eq!(
            import_fitted_size(512, 512, Some(2048), NpotScale::None),
            (512, 512)
        );
        // 上限を超える場合だけ長辺を上限へ寄せ、縦横比を保つ。
        assert_eq!(
            import_fitted_size(4096, 4096, Some(2048), NpotScale::None),
            (2048, 2048)
        );
        assert_eq!(
            import_fitted_size(4096, 2048, Some(2048), NpotScale::None),
            (2048, 1024)
        );
        assert_eq!(
            import_fitted_size(2048, 4096, Some(1024), NpotScale::None),
            (512, 1024)
        );
        // 整数除算の切り捨てのため、上限以内に収まる。
        let (width, height) = import_fitted_size(1000, 700, Some(512), NpotScale::None);
        assert!(width.max(height) <= 512, "{width}x{height}");
        assert_eq!((width, height), (512, 358));
    }

    #[test]
    fn 非2のべき乗の各値を解釈できる() {
        // 等距離の120は大きい方の128を選び、96は96に最も近い128ではなく64である。
        // 100に最も近い2のべき乗は128、60に最も近いのは64である。
        assert_eq!(
            import_fitted_size(100, 60, None, NpotScale::None),
            (100, 60)
        );
        assert_eq!(
            import_fitted_size(100, 60, None, NpotScale::ToLarger),
            (128, 64)
        );
        assert_eq!(
            import_fitted_size(100, 60, None, NpotScale::ToSmaller),
            (64, 32)
        );
        assert_eq!(
            import_fitted_size(100, 60, None, NpotScale::ToNearest),
            (128, 64)
        );
        // 365に最も近いのは256（差109）であり、512（差147）ではない。
        assert_eq!(
            import_fitted_size(365, 365, None, NpotScale::ToNearest),
            (256, 256)
        );
        // 2のべき乗は変えない。
        assert_eq!(
            import_fitted_size(512, 1024, None, NpotScale::ToNearest),
            (512, 1024)
        );
        // 上限の後にNPOTを適用する。4096を512へ縮めた後は512のままである。
        assert_eq!(
            import_fitted_size(4096, 4096, Some(512), NpotScale::ToNearest),
            (512, 512)
        );
    }

    #[test]
    fn 面積平均は手計算と合う() {
        // 2x2（R=0・64・128・255、他は固定）を1x1へ符号化済み平均する。
        // (0+64+128+255)/4=111.75 → 112である。
        let src = vec![
            0, 10, 20, 255, //
            64, 10, 20, 255, //
            128, 10, 20, 255, //
            255, 10, 20, 255, //
        ];
        let got = box_downscale_rgba(&src, 2, 2, 1, 1, false, true);
        assert_eq!(got, vec![112, 10, 20, 255], "{got:?}");
        // 同じ寸法の場合は複製して返す。
        let same = box_downscale_rgba(&src, 2, 2, 2, 2, false, true);
        assert_eq!(same, src);
    }

    #[test]
    fn 線形平均と符号化済み平均は黒白で分かれる() {
        // 2x1の黒白（sRGB・不透明）を1x1へ縮める。
        // 符号化済み平均は (0+255)/2=127.5 → 128、
        // 線形平均は 0.5 のsRGB符号化 → 188 である。
        let src = vec![0, 0, 0, 255, 255, 255, 255, 255];
        let encoded = box_downscale_rgba(&src, 2, 1, 1, 1, false, true);
        assert_eq!(encoded, vec![128, 128, 128, 255], "{encoded:?}");
        let linear = box_downscale_rgba(&src, 2, 1, 1, 1, true, true);
        assert_eq!(linear, vec![188, 188, 188, 255], "{linear:?}");
        // リニア標識の線形平均は素の値を割って平均するため、同じく128になる。
        let raw = box_downscale_rgba(&src, 2, 1, 1, 1, true, false);
        assert_eq!(raw, vec![128, 128, 128, 255], "{raw:?}");
    }

    #[test]
    fn 縮小結果は決定的で標本化の旗を保つ() {
        // 4x4の格子を2x2へ縮め、2回が一致し、旗が残ることを確かめる。
        let mut src = Vec::with_capacity(4 * 4 * 4);
        for y in 0..4 {
            for x in 0..4 {
                src.extend_from_slice(&[(x * 64) as u8, (y * 64) as u8, 128, 255]);
            }
        }
        let first = box_downscale_rgba(&src, 4, 4, 2, 2, false, true);
        let second = box_downscale_rgba(&src, 4, 4, 2, 2, false, true);
        assert_eq!(first, second);
        // 左上2x2（R=0・64・0・64）の平均は32、右上は160である。
        assert_eq!(first[0], 32, "{first:?}");
        assert_eq!(first[4], 160, "{first:?}");
        let image = BakeImage {
            width: 4,
            height: 4,
            rgba: src,
            srgb: false,
            sampler: VrmSampler::default(),
        };
        let small = downscale_bake_image(&image, 2, 2, true);
        assert_eq!((small.width(), small.height()), (2, 2));
        assert!(!small.srgb);
        assert_eq!(small.pixels().len(), 2 * 2 * 4);
    }

    /// 制限付きPNG符号化の正常出力長を求める。2x2のべた塗りである。
    fn 制限付き出力長() -> (Vec<u8>, BakeSize, usize) {
        let raw = vec![
            10, 20, 30, 255, 40, 50, 60, 255, 70, 80, 90, 255, 100, 110, 120, 255,
        ];
        let size = BakeSize {
            width: 2,
            height: 2,
        };
        let 正常 =
            encode_rgba_png_limited(&raw, size, u64::MAX, "検査").expect("十分な予算では書ける");
        let 長さ = 正常.len();
        (raw, size, 長さ)
    }

    /// 終端書き込みの失敗を握り潰さない。
    ///
    /// 正常出力長Nでは成功し、N−1〜N−12ではすべて上限超過で失敗する。
    /// 予算不足でIEND不完全なPNGが成功扱いにならないことを確かめる。
    #[test]
    fn 終端不足の予算ではすべて失敗し正常出力はiendまで完全である() {
        let (raw, size, 長さ) = 制限付き出力長();
        assert!(長さ > 12, "IEND検査に足る長さがある: {長さ}");
        let 正常 =
            encode_rgba_png_limited(&raw, size, 長さ as u64, "検査").expect("上限一致は通る");
        assert_eq!(正常.len(), 長さ);
        // 終端12バイトは空のIENDチャンク（長さ0・型IEND・CRC）である。
        assert_eq!(
            &正常[長さ - 12..],
            &[0, 0, 0, 0, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82],
            "IENDまで完全である"
        );
        // 復号して画素が一致し、壊れた成功扱いがないことを確かめる。
        let 復号 = load_bake_image(
            &正常,
            VrmImageMime::Png,
            true,
            VrmSampler::default(),
            u64::MAX,
        )
        .expect("自作のPNGは復号できる");
        assert_eq!(復号.pixels(), raw.as_slice());
        for 不足 in 1..=12u64 {
            let 失敗 = encode_rgba_png_limited(&raw, size, 長さ as u64 - 不足, "検査")
                .expect_err(&format!("{不足}不足では失敗する"));
            assert_eq!(失敗.kind(), MaterialErrorKind::LimitExceeded, "{失敗}");
        }
    }

    /// 低予算かつ非圧縮性の巨大画像でも有界のまま即失敗する。
    ///
    /// 行単位で流して符号化するため、全画像分の内部複製を作らない。
    /// 4096²の乱数列（67MiB・ほぼ非圧縮性）に対し、残り予算1024では
    /// 先頭行の途中で打ち切り、上限超過で失敗する。
    #[test]
    fn 巨大な非圧縮性画像は低予算で即失敗する() {
        let 幅 = 4096usize;
        let 高 = 4096usize;
        // 決定論的なxorshiftで非圧縮性の画素列を作る。
        let mut 状態 = 0x1234_5678_9ABC_DEF1u64;
        let mut raw = vec![0u8; 幅 * 高 * 4];
        for slot in raw.chunks_exact_mut(8) {
            状態 ^= 状態 << 13;
            状態 ^= 状態 >> 7;
            状態 ^= 状態 << 17;
            slot.copy_from_slice(&状態.to_le_bytes());
        }
        let size = BakeSize {
            width: 幅 as u32,
            height: 高 as u32,
        };
        let 失敗 =
            encode_rgba_png_limited(&raw, size, 1024, "検査").expect_err("低予算では失敗する");
        assert_eq!(失敗.kind(), MaterialErrorKind::LimitExceeded, "{失敗}");
    }

    /// 所有権版の縮小は等倍で複製せず画素を保つ。
    ///
    /// 等倍分岐は所有権を移して返すだけであり、符号化前の全体複製は
    /// 起きない（コード上の担保）。縮小時は借用版と同じ結果になる。
    #[test]
    fn 所有権版の縮小は等倍で画素を保つ() {
        let image = BakeImage {
            width: 2,
            height: 2,
            rgba: vec![1, 2, 3, 255, 4, 5, 6, 255, 7, 8, 9, 255, 10, 11, 12, 255],
            srgb: true,
            sampler: VrmSampler::default(),
        };
        let 等倍 = downscale_bake_image_owned(image, 2, 2, false);
        assert_eq!((等倍.width(), 等倍.height()), (2, 2));
        assert_eq!(
            等倍.pixels(),
            &[1, 2, 3, 255, 4, 5, 6, 255, 7, 8, 9, 255, 10, 11, 12, 255]
        );
        assert!(等倍.srgb);
        let image = BakeImage {
            width: 2,
            height: 2,
            rgba: vec![
                0, 0, 0, 255, 255, 255, 255, 255, 0, 0, 0, 255, 255, 255, 255, 255,
            ],
            srgb: false,
            sampler: VrmSampler::default(),
        };
        let 縮小 = downscale_bake_image_owned(image, 1, 1, false);
        let 借用 = BakeImage {
            width: 2,
            height: 2,
            rgba: vec![
                0, 0, 0, 255, 255, 255, 255, 255, 0, 0, 0, 255, 255, 255, 255, 255,
            ],
            srgb: false,
            sampler: VrmSampler::default(),
        };
        let 期待 = downscale_bake_image(&借用, 1, 1, false);
        assert_eq!(縮小.pixels(), 期待.pixels());
    }
}
