//! PSD version 1の保存済み合成画像（composite）をRGBA8へ復号する層。
//!
//! 対象は8bit・RGB/Grayscale・Raw/RLEだけであり、PSB・16/32bit・
//! CMYKなどの追加の色形式・追加チャンネル・ZIP圧縮は扱わない。
//! レイヤー合成は行わず、ファイル末尾の保存済みcompositeだけを使う。
//! Image Resource 1057（Version Info）の `hasRealMergedData` が偽の
//! 場合は白画像でも使わず、真または情報なしで構造が正常な場合だけ使う。
//!
//! ```mermaid
//! flowchart TB
//!     head["ヘッダ<br/>版・寸法・深度・形式"] --> res["画像リソース<br/>1057の有無と値"]
//!     res --> skip1["層・マスク区間を長さで飛ばす"]
//!     skip1 --> comp["composite<br/>Raw・RLE"]
//!     comp --> rgba["RGBA8<br/>行単位で直接書く"]
//! ```
//!
//! 対象外・破損・composite不在は [`PsdError::Skip`] で返し、呼び出し側が
//! 警告してテクスチャを省く。画素上限の超過だけ [`PsdError::Limit`] で返し、
//! 呼び出し側が変換失敗させる。入力バイト列の読み込み失敗は呼び出し側の
//! 読み取りが既に分類しているため、ここでは扱わない。
//! 全チャンネル平面を別々に保持せず、最終RGBAへ行単位で書く。
//! レイヤー・サムネイル・ICC本体は複製しない。

use super::material::{MaterialError, MaterialErrorKind};

/// PSDの署名。
const PSD_MAGIC: [u8; 4] = *b"8BPS";
/// 対応するPSDの版（PSBは2であり対象外）。
const PSD_VERSION: u16 = 1;
/// Image Resourceの署名2種（`8BIM`・`PSIR`）。
const RESOURCE_SIGNS: [[u8; 4]; 2] = [*b"8BIM", *b"PSIR"];
/// Version Info（`hasRealMergedData` を持つ資源）の資源ID。
const VERSION_INFO_ID: u16 = 1057;
/// Version Info内で `hasRealMergedData` を読む最小長（版4＋真偽1）。
const VERSION_INFO_MIN: usize = 5;
/// Grayscaleの色形式番号。
const MODE_GRAYSCALE: u16 = 1;
/// RGBの色形式番号。
const MODE_RGB: u16 = 3;
/// Raw圧縮の方式番号。
const COMPRESSION_RAW: u16 = 0;
/// RLE圧縮の方式番号。
const COMPRESSION_RLE: u16 = 1;

/// PSD復号の結果。画素は行優先のRGBA8であり、アルファは常に不透明である。
///
/// compositeに透明情報を持たない（追加チャンネルを扱わない）ため、
/// 4チャンネルを無条件にRGBAとして扱わず出力アルファは255に固定する。
#[derive(Debug, Clone)]
pub(crate) struct PsdImage {
    /// 幅（テクセル数）。0ではない。
    width: u32,
    /// 高さ（テクセル数）。0ではない。
    height: u32,
    /// 行優先のRGBA8。長さは `width * height * 4` である。
    rgba: Vec<u8>,
}

impl PsdImage {
    /// 幅を返す。
    pub(crate) fn width(&self) -> u32 {
        self.width
    }

    /// 高さを返す。
    pub(crate) fn height(&self) -> u32 {
        self.height
    }

    /// 行優先のRGBA8を取り出す。
    pub(crate) fn into_pixels(self) -> Vec<u8> {
        self.rgba
    }
}

/// PSD復号の失敗。
///
/// 対象外・破損・composite不在は警告して省く側が落とすため
/// [`PsdError::Skip`] で返し、画素上限の超過だけ [`PsdError::Limit`] で返す。
#[derive(Debug)]
pub(crate) enum PsdError {
    /// 警告してテクスチャを省く。呼び出し側が枠と対象を付けて文面にする。
    Skip {
        /// 省く理由の文面（`のため省く` まで含む）。
        reason: &'static str,
        /// 補足（方式名・再保存の案内など）。
        detail: String,
    },
    /// 変換失敗として返す（画素上限の超過）。
    Limit(MaterialError),
}

/// バイト列がPSDかどうかを署名だけで判定する。
pub(crate) fn is_psd(bytes: &[u8]) -> bool {
    bytes.starts_with(&PSD_MAGIC)
}

/// PSDの寸法だけをヘッダから読む。
///
/// 画素データの確保は行わないため、巨大な寸法の申告でも確保が起きない。
/// 版・深度・形式・チャンネル数の対象外もここで判定し、呼び出し側が
/// 警告して省けるようにする。画素上限の超過だけ失敗として返す。
///
/// # Errors
///
/// 対象外・破損の場合は [`PsdError::Skip`]、画素上限の超過の場合は
/// [`PsdError::Limit`] を返す。
pub(crate) fn psd_dimensions(bytes: &[u8], max_pixels: u64) -> Result<(u32, u32), PsdError> {
    let header = PsdHeader::parse(bytes)?;
    header.check_pixels(max_pixels)?;
    Ok((header.width, header.height))
}

/// PSDの保存済みcompositeをRGBA8へ復号する。
///
/// `max_pixels` は1枚の画素数の上限であり、画素バッファの確保前に
/// 寸法だけを見て判定するため、巨大な寸法の申告では確保が起きない。
/// RLEは行ごとに宣言長と展開幅を厳密に検査し、不足のゼロ埋めも
/// 過剰出力もしない。出力アルファは常に255である。
///
/// # Errors
///
/// 対象外・破損・composite不在の場合は [`PsdError::Skip`]、画素上限の
/// 超過の場合は [`PsdError::Limit`] を返す。
pub(crate) fn decode_psd(bytes: &[u8], max_pixels: u64) -> Result<PsdImage, PsdError> {
    let header = PsdHeader::parse(bytes)?;
    header.check_pixels(max_pixels)?;
    let mut cursor = PsdCursor::new(bytes, PsdHeader::LEN);
    // 色形式データは使わず長さで飛ばす。
    let color_len = cursor.read_u32("色形式データ")? as usize;
    cursor.skip(color_len, "色形式データ")?;
    let merged = cursor.scan_version_info()?;
    // 層・マスク区間は使わず長さで飛ばす。レイヤー合成へ落ちない。
    let layers_len = cursor.read_u32("層・マスク区間")? as usize;
    cursor.skip(layers_len, "層・マスク区間")?;
    if merged == Some(false) {
        return Err(PsdError::Skip {
            reason: "PSDに有効な合成画像がないため省く",
            detail:
                "Version Infoに実結合データなしと記録されている。Maximize Compatibilityを有効にして再保存してください"
                    .to_string(),
        });
    }
    let compression = cursor.read_u16("compositeの圧縮方式")?;
    let channels = header.channels as usize;
    let (width, height) = (header.width as usize, header.height as usize);
    let pixel_count = header.pixel_count();
    // 圧縮方式の対象外は確保前に判定し、文面は従来どおりにする。
    if compression != COMPRESSION_RAW && compression != COMPRESSION_RLE {
        return Err(PsdError::Skip {
            reason: "PSDの圧縮方式が未対応のため省く",
            detail: format!("方式{compression}({})", compression_name(compression)),
        });
    }
    // 本体の不足も確保前に判定する。残り入力が足りない場合は
    // composite不在・不足として省き、RGBAの確保は行わない。
    if compression == COMPRESSION_RAW {
        let need = (channels as u64)
            .checked_mul(pixel_count)
            .ok_or_else(|| missing("compositeのRaw平面"))?;
        if (cursor.rest() as u64) < need {
            return Err(missing("compositeのRaw平面"));
        }
    } else {
        let need = (height as u64)
            .checked_mul(channels as u64)
            .and_then(|rows| rows.checked_mul(2))
            .ok_or_else(|| missing("compositeのRLE行長表"))?;
        if (cursor.rest() as u64) < need {
            return Err(missing("compositeのRLE行長表"));
        }
    }
    // 最終RGBAの確保前に寸法を再確認する。確保量は画素上限以下に収まる。
    // `Vec` の確保可能範囲（`isize::MAX`）も検査し、巨大な申告では
    //  capacity overflow でパニックさせず上限超過として返す。
    let rgba_len = pixel_count
        .checked_mul(4)
        .and_then(|len| usize::try_from(len).ok())
        .filter(|len| *len <= isize::MAX as usize)
        .ok_or_else(|| {
            PsdError::Limit(MaterialError::new(
                MaterialErrorKind::LimitExceeded,
                format!("PSDの画素数が上限を超えた: {width}x{height}（上限{max_pixels}画素）"),
            ))
        })?;
    // 確保の失敗も上限超過として返し、パニックや異常終了させない。
    let mut rgba = Vec::new();
    rgba.try_reserve_exact(rgba_len).map_err(|_| {
        PsdError::Limit(MaterialError::new(
            MaterialErrorKind::LimitExceeded,
            format!("PSDの画素数が上限を超えた: {width}x{height}（上限{max_pixels}画素）"),
        ))
    })?;
    rgba.resize(rgba_len, 0);
    match compression {
        COMPRESSION_RAW => decode_raw(&mut cursor, &header, channels, width, height, &mut rgba)?,
        // 方式は上で判定済みであり、ここではRLEだけが残る。
        _ => decode_rle(&mut cursor, &header, channels, width, height, &mut rgba)?,
    }
    Ok(PsdImage {
        width: header.width,
        height: header.height,
        rgba,
    })
}

/// PSDヘッダ（署名から色形式までの26バイト）の読み取り結果。
struct PsdHeader {
    /// チャンネル数。RGBは3・Grayscaleは1だけを受け付ける。
    channels: u16,
    /// 幅。0ではない。
    width: u32,
    /// 高さ。0ではない。
    height: u32,
    /// 色形式番号。RGB・Grayscaleだけを受け付ける。
    mode: u16,
}

impl PsdHeader {
    /// ヘッダ長。
    const LEN: usize = 26;

    /// ヘッダを読み、版・寸法・深度・形式・チャンネル数を検査する。
    fn parse(bytes: &[u8]) -> Result<Self, PsdError> {
        let missing = || PsdError::Skip {
            reason: "PSDに有効な合成画像がないため省く",
            detail: "ヘッダが短い。Maximize Compatibilityを有効にして再保存してください"
                .to_string(),
        };
        let header = bytes.get(..Self::LEN).ok_or_else(missing)?;
        if header[..4] != PSD_MAGIC {
            return Err(PsdError::Skip {
                reason: "PSDに有効な合成画像がないため省く",
                detail: "署名がPSDでない".to_string(),
            });
        }
        let version = u16::from_be_bytes([header[4], header[5]]);
        if version != PSD_VERSION {
            return Err(PsdError::Skip {
                reason: "PSDの版が未対応のため省く",
                detail: if version == 2 {
                    "PSB".to_string()
                } else {
                    format!("版{version}")
                },
            });
        }
        let channels = u16::from_be_bytes([header[12], header[13]]);
        let height = u32::from_be_bytes([header[14], header[15], header[16], header[17]]);
        let width = u32::from_be_bytes([header[18], header[19], header[20], header[21]]);
        let depth = u16::from_be_bytes([header[22], header[23]]);
        let mode = u16::from_be_bytes([header[24], header[25]]);
        if width == 0 || height == 0 {
            return Err(PsdError::Skip {
                reason: "PSDに有効な合成画像がないため省く",
                detail: "寸法が0である".to_string(),
            });
        }
        if depth != 8 {
            return Err(PsdError::Skip {
                reason: "PSDの色深度が未対応のため省く",
                detail: format!("{depth}bit"),
            });
        }
        let expect = match mode {
            MODE_RGB => 3,
            MODE_GRAYSCALE => 1,
            _ => {
                return Err(PsdError::Skip {
                    reason: "PSDの色形式が未対応のため省く",
                    detail: format!("形式{mode}({})", mode_name(mode)),
                });
            }
        };
        if channels != expect {
            return Err(PsdError::Skip {
                reason: "PSDのチャンネル数が未対応のため省く",
                detail: format!("{}で{channels}チャンネル", mode_name(mode)),
            });
        }
        Ok(Self {
            channels,
            width,
            height,
            mode,
        })
    }

    /// 画素数を返す。寸法は32bitのため64bitでは溢れない。
    fn pixel_count(&self) -> u64 {
        u64::from(self.width).saturating_mul(u64::from(self.height))
    }

    /// 1枚の画素数が上限に収まることを確かめる。確保前に呼ぶ。
    fn check_pixels(&self, max_pixels: u64) -> Result<(), PsdError> {
        if self.pixel_count() > max_pixels {
            return Err(PsdError::Limit(MaterialError::new(
                MaterialErrorKind::LimitExceeded,
                format!(
                    "PSDの画素数が上限を超えた: {}x{}（上限{max_pixels}画素）",
                    self.width, self.height
                ),
            )));
        }
        Ok(())
    }

    /// Grayscaleかどうかを返す。
    fn is_gray(&self) -> bool {
        self.mode == MODE_GRAYSCALE
    }
}

/// 色形式番号の表示名。対象外の警告補足だけに使う。
fn mode_name(mode: u16) -> &'static str {
    match mode {
        0 => "Bitmap",
        MODE_GRAYSCALE => "Grayscale",
        2 => "Indexed",
        MODE_RGB => "RGB",
        4 => "CMYK",
        7 => "Multichannel",
        8 => "Duotone",
        9 => "Lab",
        _ => "不明",
    }
}

/// 圧縮方式番号の表示名。対象外の警告補足だけに使う。
fn compression_name(compression: u16) -> &'static str {
    match compression {
        COMPRESSION_RAW => "Raw",
        COMPRESSION_RLE => "RLE",
        2 => "ZIP",
        3 => "ZIP（予測あり）",
        _ => "不明",
    }
}

/// 検査付きで前に進む読み取り位置。複製せず借用だけを持つ。
struct PsdCursor<'a> {
    /// 入力全体。
    bytes: &'a [u8],
    /// 次に読む位置。
    pos: usize,
}

impl<'a> PsdCursor<'a> {
    /// 読み取り位置を作る。
    fn new(bytes: &'a [u8], pos: usize) -> Self {
        Self { bytes, pos }
    }

    /// 残り長さを返す。
    fn rest(&self) -> usize {
        self.bytes.len().saturating_sub(self.pos)
    }

    /// 2バイトの big-endian を読む。不足はcomposite不在として扱う。
    fn read_u16(&mut self, target: &str) -> Result<u16, PsdError> {
        let next = self.pos.checked_add(2).ok_or_else(|| missing(target))?;
        let pair: [u8; 2] = self
            .bytes
            .get(self.pos..next)
            .and_then(|slice| slice.try_into().ok())
            .ok_or_else(|| missing(target))?;
        self.pos = next;
        Ok(u16::from_be_bytes(pair))
    }

    /// 4バイトの big-endian を読む。不足はcomposite不在として扱う。
    fn read_u32(&mut self, target: &str) -> Result<u32, PsdError> {
        let next = self.pos.checked_add(4).ok_or_else(|| missing(target))?;
        let quad: [u8; 4] = self
            .bytes
            .get(self.pos..next)
            .and_then(|slice| slice.try_into().ok())
            .ok_or_else(|| missing(target))?;
        self.pos = next;
        Ok(u32::from_be_bytes(quad))
    }

    /// 指定長だけ飛ばす。不足はcomposite不在として扱う。
    fn skip(&mut self, len: usize, target: &str) -> Result<(), PsdError> {
        let next = self.pos.checked_add(len).ok_or_else(|| missing(target))?;
        self.bytes
            .get(self.pos..next)
            .ok_or_else(|| missing(target))?;
        self.pos = next;
        Ok(())
    }

    /// 指定長を借りる。不足はcomposite不在として扱う。
    fn take(&mut self, len: usize, target: &str) -> Result<&'a [u8], PsdError> {
        let next = self.pos.checked_add(len).ok_or_else(|| missing(target))?;
        let slice = self
            .bytes
            .get(self.pos..next)
            .ok_or_else(|| missing(target))?;
        self.pos = next;
        Ok(slice)
    }

    /// 指定長を切り出して部分カーソルを作る。元の位置は末尾へ進む。
    ///
    /// 切り出した範囲だけを読むカーソルを返すため、区間長の宣言を
    /// 越えた読み取りは起きない。不足はcomposite不在として扱う。
    fn split(&mut self, len: usize, target: &str) -> Result<PsdCursor<'a>, PsdError> {
        let slice = self.take(len, target)?;
        Ok(PsdCursor {
            bytes: slice,
            pos: 0,
        })
    }

    /// 画像リソース区間を走査し、Version Infoの有無と値を返す。
    ///
    /// 区間自体は複製せず、区間に限定した部分カーソルで資源ごとに
    /// 署名・番号・名前・長さだけを読む。区間の境界を跨ぐ読み取りは
    /// 起きない。`None` は情報なし（正常に復号できれば使う）、
    /// `Some` は記録値である。
    fn scan_version_info(&mut self) -> Result<Option<bool>, PsdError> {
        let total = self.read_u32("画像リソース区間")? as usize;
        // 宣言長だけを切り出し、走査は区間内に限定する。
        let mut inner = self.split(total, "画像リソース区間")?;
        let mut found = None;
        while inner.rest() > 0 {
            let sign: [u8; 4] = inner
                .take(4, "画像リソースの署名")?
                .try_into()
                .map_err(|_| missing("画像リソースの署名"))?;
            if !RESOURCE_SIGNS.contains(&sign) {
                return Err(missing("画像リソースの署名"));
            }
            let id = inner.read_u16("画像リソースの番号")?;
            // 名前はPascal文字列であり、全体（長さ1＋本文）を偶数長へ詰める。
            let name_len = inner
                .take(1, "画像リソースの名前")?
                .first()
                .copied()
                .ok_or_else(|| missing("画像リソースの名前"))? as usize;
            let padded = name_len
                .checked_add(1)
                .ok_or_else(|| missing("画像リソースの名前"))?;
            let aligned = padded
                .checked_add(padded % 2)
                .ok_or_else(|| missing("画像リソースの名前"))?;
            inner.skip(aligned.saturating_sub(1), "画像リソースの名前")?;
            let data_len = inner.read_u32("画像リソースの本文")? as usize;
            let data = inner.take(data_len, "画像リソースの本文")?;
            // 本文も偶数長へ詰める。
            inner.skip(data_len % 2, "画像リソースの本文")?;
            if id == VERSION_INFO_ID && found.is_none() {
                // 版4バイトの後の1バイトが `hasRealMergedData` である。
                // 本文が短い場合は情報なしとして扱う。
                found = data
                    .get(4..VERSION_INFO_MIN)
                    .and_then(|flag| flag.first())
                    .map(|flag| *flag != 0);
            }
        }
        Ok(found)
    }
}

/// composite不在・不足・破損の失敗を作る。
fn missing(target: &str) -> PsdError {
    PsdError::Skip {
        reason: "PSDに有効な合成画像がないため省く",
        detail: format!(
            "{target}が不足している。Maximize Compatibilityを有効にして再保存してください"
        ),
    }
}

/// Rawのcompositeを最終RGBAへ直接書く。
///
/// 平面はR・G・B（Grayscaleは1平面）の順に高さ行ずつ並ぶ。
/// 全平面を別々に保持せず、行単位で最終位置へ書く。
fn decode_raw(
    cursor: &mut PsdCursor<'_>,
    header: &PsdHeader,
    channels: usize,
    width: usize,
    height: usize,
    rgba: &mut [u8],
) -> Result<(), PsdError> {
    let plane = width
        .checked_mul(height)
        .ok_or_else(|| missing("compositeのRaw平面"))?;
    let total = plane
        .checked_mul(channels)
        .ok_or_else(|| missing("compositeのRaw平面"))?;
    if cursor.rest() < total {
        return Err(missing("compositeのRaw平面"));
    }
    if header.is_gray() {
        let gray = cursor.take(plane, "compositeのRaw平面")?;
        for (row, line) in gray.chunks_exact(width).enumerate() {
            let at = row
                .checked_mul(width)
                .and_then(|row| row.checked_mul(4))
                .ok_or_else(|| missing("compositeのRaw平面"))?;
            for (column, value) in line.iter().enumerate() {
                let pixel = at
                    .checked_add(
                        column
                            .checked_mul(4)
                            .ok_or_else(|| missing("compositeのRaw平面"))?,
                    )
                    .ok_or_else(|| missing("compositeのRaw平面"))?;
                let slot = rgba
                    .get_mut(
                        pixel
                            ..pixel
                                .checked_add(4)
                                .ok_or_else(|| missing("compositeのRaw平面"))?,
                    )
                    .ok_or_else(|| missing("compositeのRaw平面"))?;
                slot[0] = *value;
                slot[1] = *value;
                slot[2] = *value;
                slot[3] = 0xFF;
            }
        }
        return Ok(());
    }
    // RGBはR・G・Bの順に平面が並ぶ。行ごとに3平面の同じ行を読む。
    for channel in 0..channels {
        let flat = cursor.take(plane, "compositeのRaw平面")?;
        for (row, line) in flat.chunks_exact(width).enumerate() {
            let at = row
                .checked_mul(width)
                .and_then(|row| row.checked_mul(4))
                .ok_or_else(|| missing("compositeのRaw平面"))?;
            for (column, value) in line.iter().enumerate() {
                let pixel = at
                    .checked_add(
                        column
                            .checked_mul(4)
                            .ok_or_else(|| missing("compositeのRaw平面"))?,
                    )
                    .ok_or_else(|| missing("compositeのRaw平面"))?;
                let slot = rgba
                    .get_mut(
                        pixel
                            ..pixel
                                .checked_add(4)
                                .ok_or_else(|| missing("compositeのRaw平面"))?,
                    )
                    .ok_or_else(|| missing("compositeのRaw平面"))?;
                slot[channel] = *value;
            }
        }
    }
    // アルファは不透明に固定する。追加チャンネルは扱わない。
    for pixel in rgba.chunks_exact_mut(4) {
        pixel[3] = 0xFF;
    }
    Ok(())
}

/// RLEのcompositeを最終RGBAへ直接書く。
///
/// 行長表（高さ×チャンネル数のu16）を先に読み、各行は宣言長ちょうどの
/// 入力から幅ちょうどへ展開する。不足のゼロ埋めも過剰出力もしない。
/// 全平面を別々に保持せず、行単位で最終位置へ書く。
fn decode_rle(
    cursor: &mut PsdCursor<'_>,
    header: &PsdHeader,
    channels: usize,
    width: usize,
    height: usize,
    rgba: &mut [u8],
) -> Result<(), PsdError> {
    let rows = height
        .checked_mul(channels)
        .ok_or_else(|| missing("compositeのRLE行長表"))?;
    let table_len = rows
        .checked_mul(2)
        .ok_or_else(|| missing("compositeのRLE行長表"))?;
    let table = cursor.take(table_len, "compositeのRLE行長表")?;
    let mut lengths = Vec::with_capacity(rows);
    for pair in table.chunks_exact(2) {
        lengths.push(usize::from(u16::from_be_bytes([pair[0], pair[1]])));
    }
    let gray = header.is_gray();
    for (plane, plane_lengths) in lengths.chunks_exact(height).enumerate() {
        for (row, length) in plane_lengths.iter().enumerate() {
            let packed = cursor.take(*length, "compositeのRLE行")?;
            let line = unpack_packbits(packed, width)?;
            let at = row
                .checked_mul(width)
                .and_then(|row| row.checked_mul(4))
                .ok_or_else(|| missing("compositeのRLE行"))?;
            if gray {
                for (column, value) in line.iter().enumerate() {
                    let pixel = at
                        .checked_add(
                            column
                                .checked_mul(4)
                                .ok_or_else(|| missing("compositeのRLE行"))?,
                        )
                        .ok_or_else(|| missing("compositeのRLE行"))?;
                    let slot = rgba
                        .get_mut(
                            pixel
                                ..pixel
                                    .checked_add(4)
                                    .ok_or_else(|| missing("compositeのRLE行"))?,
                        )
                        .ok_or_else(|| missing("compositeのRLE行"))?;
                    slot[0] = *value;
                    slot[1] = *value;
                    slot[2] = *value;
                    slot[3] = 0xFF;
                }
            } else {
                for (column, value) in line.iter().enumerate() {
                    let pixel = at
                        .checked_add(
                            column
                                .checked_mul(4)
                                .ok_or_else(|| missing("compositeのRLE行"))?,
                        )
                        .ok_or_else(|| missing("compositeのRLE行"))?;
                    let slot = rgba
                        .get_mut(
                            pixel
                                ..pixel
                                    .checked_add(4)
                                    .ok_or_else(|| missing("compositeのRLE行"))?,
                        )
                        .ok_or_else(|| missing("compositeのRLE行"))?;
                    slot[plane] = *value;
                }
            }
        }
    }
    // アルファは不透明に固定する。追加チャンネルは扱わない。
    for pixel in rgba.chunks_exact_mut(4) {
        pixel[3] = 0xFF;
    }
    Ok(())
}

/// PackBits（PhotoshopのRLE変種）の1行を展開する。
///
/// 長さバイトが128は無操作、0〜127は直後の `n + 1` バイトの複写、
// 129〜255は直後の1バイトを `257 - n` 回繰り返す。
/// 宣言長ちょうどを消費し幅ちょうどへ展開しない場合は破損として扱う。
fn unpack_packbits(packed: &[u8], width: usize) -> Result<Vec<u8>, PsdError> {
    let mut out = Vec::with_capacity(width);
    let mut pos = 0;
    while out.len() < width {
        let head = packed
            .get(pos)
            .copied()
            .ok_or_else(|| missing("compositeのRLE行"))?;
        pos = pos
            .checked_add(1)
            .ok_or_else(|| missing("compositeのRLE行"))?;
        if head == 128 {
            continue;
        } else if head < 128 {
            let count = usize::from(head)
                .checked_add(1)
                .ok_or_else(|| missing("compositeのRLE行"))?;
            let end = pos
                .checked_add(count)
                .ok_or_else(|| missing("compositeのRLE行"))?;
            let literal = packed
                .get(pos..end)
                .ok_or_else(|| missing("compositeのRLE行"))?;
            if out.len().checked_add(count).is_none_or(|len| len > width) {
                return Err(missing("compositeのRLE行"));
            }
            out.extend_from_slice(literal);
            pos = end;
        } else {
            let count = 257usize.saturating_sub(usize::from(head));
            let value = packed
                .get(pos)
                .copied()
                .ok_or_else(|| missing("compositeのRLE行"))?;
            pos = pos
                .checked_add(1)
                .ok_or_else(|| missing("compositeのRLE行"))?;
            if out.len().checked_add(count).is_none_or(|len| len > width) {
                return Err(missing("compositeのRLE行"));
            }
            out.extend(std::iter::repeat_n(value, count));
        }
    }
    if pos != packed.len() {
        return Err(missing("compositeのRLE行"));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    // テスト名は日本語で書く（テストは仕様・契約であるため）。

    /// 小さいPSDを組み立てる部品。規定値は2x2のRGB・Rawである。
    struct 組立 {
        版: u16,
        チャンネル数: u16,
        幅: u32,
        高さ: u32,
        深度: u16,
        形式: u16,
        資源: Vec<(u16, Vec<u8>)>,
        層区間: Vec<u8>,
        圧縮: u16,
        本体: Vec<u8>,
    }

    impl 組立 {
        /// 規定値を作る。
        fn 新規() -> Self {
            Self {
                版: 1,
                チャンネル数: 3,
                幅: 2,
                高さ: 2,
                深度: 8,
                形式: MODE_RGB,
                資源: Vec::new(),
                層区間: Vec::new(),
                圧縮: COMPRESSION_RAW,
                本体: vec![0; 2 * 2 * 3],
            }
        }

        /// バイト列にする。
        fn 組み立てる(&self) -> Vec<u8> {
            let mut out = Vec::new();
            out.extend_from_slice(&PSD_MAGIC);
            out.extend_from_slice(&self.版.to_be_bytes());
            out.extend_from_slice(&[0; 6]);
            out.extend_from_slice(&self.チャンネル数.to_be_bytes());
            out.extend_from_slice(&self.高さ.to_be_bytes());
            out.extend_from_slice(&self.幅.to_be_bytes());
            out.extend_from_slice(&self.深度.to_be_bytes());
            out.extend_from_slice(&self.形式.to_be_bytes());
            out.extend_from_slice(&(0u32.to_be_bytes()));
            let mut resources = Vec::new();
            for (id, data) in &self.資源 {
                resources.extend_from_slice(b"8BIM");
                resources.extend_from_slice(&id.to_be_bytes());
                resources.push(0);
                resources.push(0);
                resources.extend_from_slice(&(data.len() as u32).to_be_bytes());
                resources.extend_from_slice(data);
                if data.len() % 2 == 1 {
                    resources.push(0);
                }
            }
            out.extend_from_slice(&(resources.len() as u32).to_be_bytes());
            out.extend_from_slice(&resources);
            out.extend_from_slice(&(self.層区間.len() as u32).to_be_bytes());
            out.extend_from_slice(&self.層区間);
            out.extend_from_slice(&self.圧縮.to_be_bytes());
            out.extend_from_slice(&self.本体);
            out
        }
    }

    /// Version Info資源の本文を作る。真偽値の前は版4バイトである。
    fn 版情報(結合あり: bool) -> (u16, Vec<u8>) {
        let mut data = vec![0, 0, 0, 1];
        data.push(u8::from(結合あり));
        data.extend_from_slice(b"reader");
        data.push(0);
        (VERSION_INFO_ID, data)
    }

    /// RLEの行長表と本体を組み合わせる。
    fn rle本体(行長: &[usize], 行群: &[Vec<u8>]) -> Vec<u8> {
        let mut out = Vec::new();
        for (len, row) in 行長.iter().zip(行群.iter()) {
            assert_eq!(*len, row.len(), "行長表は本体と一致させる");
            out.extend_from_slice(&(*len as u16).to_be_bytes());
        }
        for row in 行群 {
            out.extend_from_slice(row);
        }
        out
    }

    /// 失敗の理由と補足を取り出す。警告扱いであることを確かめる。
    fn 省く理由(結果: Result<PsdImage, PsdError>) -> (&'static str, String) {
        match 結果 {
            Ok(_) => panic!("警告して省くはずが復号できた"),
            Err(PsdError::Skip { reason, detail }) => (reason, detail),
            Err(PsdError::Limit(error)) => panic!("上限超過ではなく警告のはず: {error}"),
        }
    }

    /// テスト用に画素列を比較する。不透明（アルファ255）も確かめる。
    fn 画素が等しい(画像: PsdImage, gray: &[u8]) -> bool {
        if 画像.width() as usize * 画像.height() as usize != gray.len() {
            return false;
        }
        画像
            .into_pixels()
            .chunks_exact(4)
            .zip(gray.iter())
            .all(|(pixel, &v)| pixel == [v, v, v, 0xFF])
    }

    #[test]
    fn rgbのrawは平面順に読めて行列と非正方形を保つ() {
        // 3x2。R平面・G平面・B平面の順に並べ、値は平面と位置で変える。
        let mut 組立 = 組立::新規();
        組立.幅 = 3;
        組立.高さ = 2;
        let r = [10, 20, 30, 40, 50, 60];
        let g = [70, 80, 90, 100, 110, 120];
        let b = [130, 140, 150, 160, 170, 180];
        組立.本体 = [r.as_slice(), g.as_slice(), b.as_slice()].concat();
        let 画像 = decode_psd(&組立.組み立てる(), u64::MAX).expect("正常なRawは読める");
        assert_eq!((画像.width(), 画像.height()), (3, 2));
        let 画素 = 画像.into_pixels();
        let 期待 = [
            [10, 70, 130, 255],
            [20, 80, 140, 255],
            [30, 90, 150, 255],
            [40, 100, 160, 255],
            [50, 110, 170, 255],
            [60, 120, 180, 255],
        ];
        assert_eq!(画素, 期待.concat());
    }

    #[test]
    fn grayscaleのrawは複写して不透明にする() {
        let mut 組立 = 組立::新規();
        組立.チャンネル数 = 1;
        組立.形式 = MODE_GRAYSCALE;
        組立.本体 = vec![0, 128, 255, 64];
        let 画像 = decode_psd(&組立.組み立てる(), u64::MAX).expect("正常なGrayは読める");
        assert_eq!(
            画像.into_pixels(),
            vec![
                0, 0, 0, 255, 128, 128, 128, 255, 255, 255, 255, 255, 64, 64, 64, 255
            ]
        );
    }

    #[test]
    fn rleは直写と反復と無操作を混ぜて読める() {
        // 4x1のRGB。R行は直写・G行は反復・B行は無操作混じりである。
        let mut 組立 = 組立::新規();
        組立.幅 = 4;
        組立.高さ = 1;
        組立.圧縮 = COMPRESSION_RLE;
        let r = vec![3, 11, 22, 33, 44];
        let g = vec![253, 77];
        let b = vec![128, 1, 55, 66, 255, 88];
        組立.本体 = rle本体(&[r.len(), g.len(), b.len()], &[r, g, b]);
        let 画像 = decode_psd(&組立.組み立てる(), u64::MAX).expect("正常なRLEは読める");
        assert_eq!(
            画像.into_pixels(),
            vec![
                11, 77, 55, 255, 22, 77, 66, 255, 33, 77, 88, 255, 44, 77, 88, 255
            ]
        );
    }

    #[test]
    fn rleの境界長は正しく展開する() {
        // 直写の最大（128バイト）と反復の最大（128回）を読む。
        let mut 組立 = 組立::新規();
        組立.幅 = 128;
        組立.高さ = 1;
        組立.チャンネル数 = 1;
        組立.形式 = MODE_GRAYSCALE;
        組立.圧縮 = COMPRESSION_RLE;
        let mut literal = vec![127];
        literal.extend((0..128u8).map(|v| v.wrapping_mul(2)));
        let repeat = vec![129, 9];
        // Gray1chなので行は高さ1行だけである。
        組立.本体 = {
            let mut out = Vec::new();
            out.extend_from_slice(&(literal.len() as u16).to_be_bytes());
            out.extend_from_slice(&literal);
            out
        };
        let 画像 = decode_psd(&組立.組み立てる(), u64::MAX).expect("境界長は読める");
        let 画素 = 画像.into_pixels();
        assert_eq!(画素.len(), 128 * 4);
        for (i, pixel) in 画素.chunks_exact(4).enumerate() {
            let 期待 = (i as u8).wrapping_mul(2);
            assert_eq!(pixel, &[期待, 期待, 期待, 255]);
        }
        // 反復の最大は別画像で確かめる。
        let mut 反復 = 組立::新規();
        反復.幅 = 128;
        反復.高さ = 1;
        反復.チャンネル数 = 1;
        反復.形式 = MODE_GRAYSCALE;
        反復.圧縮 = COMPRESSION_RLE;
        let mut 本体 = Vec::new();
        本体.extend_from_slice(&2u16.to_be_bytes());
        本体.extend_from_slice(&repeat);
        反復.本体 = 本体;
        let 画像 = decode_psd(&反復.組み立てる(), u64::MAX).expect("反復の最大は読める");
        assert!(画素が等しい(画像, &[9u8; 128]));
    }

    #[test]
    fn 行の不足と過剰と余りは破損として省く() {
        // 不足：宣言より短い入力。
        let mut 不足 = 組立::新規();
        不足.圧縮 = COMPRESSION_RLE;
        不足.本体 = rle本体(&[2, 2, 2], &[vec![0, 1], vec![0, 2], vec![0, 3]]);
        // R行は幅2に1画素しかない。
        let (理由, _) = 省く理由(decode_psd(&不足.組み立てる(), u64::MAX));
        assert_eq!(理由, "PSDに有効な合成画像がないため省く");
        // 過剰：幅を超える展開。
        let mut 過剰 = 組立::新規();
        過剰.圧縮 = COMPRESSION_RLE;
        過剰.本体 = rle本体(&[4, 2, 2], &[vec![2, 1, 2, 3], vec![0, 2], vec![0, 3]]);
        let (理由, _) = 省く理由(decode_psd(&過剰.組み立てる(), u64::MAX));
        assert_eq!(理由, "PSDに有効な合成画像がないため省く");
        // 余り：行長ちょうどを超えて消費が残る。
        let mut 余り = 組立::新規();
        余り.圧縮 = COMPRESSION_RLE;
        余り.本体 = rle本体(&[4, 2, 2], &[vec![1, 1, 2, 128], vec![0, 2], vec![0, 3]]);
        let (理由, _) = 省く理由(decode_psd(&余り.組み立てる(), u64::MAX));
        assert_eq!(理由, "PSDに有効な合成画像がないため省く");
        // Rawの不足。
        let mut 原不足 = 組立::新規();
        原不足.本体 = vec![0; 2 * 2 * 3 - 1];
        let (理由, _) = 省く理由(decode_psd(&原不足.組み立てる(), u64::MAX));
        assert_eq!(理由, "PSDに有効な合成画像がないため省く");
    }

    #[test]
    fn 白一色の正当な画像は使えて版情報なしでも警告しない() {
        let mut 組立 = 組立::新規();
        組立.本体 = vec![0xFF; 2 * 2 * 3];
        let 画像 = decode_psd(&組立.組み立てる(), u64::MAX).expect("白一色も使える");
        assert!(画像.into_pixels().iter().all(|&v| v == 0xFF));
    }

    #[test]
    fn 版情報の偽は白でも省き真と不在は使う() {
        // 偽：本文は正しくても省く。
        let mut 偽 = 組立::新規();
        偽.資源 = vec![版情報(false)];
        偽.本体 = vec![0xFF; 2 * 2 * 3];
        let (理由, 補足) = 省く理由(decode_psd(&偽.組み立てる(), u64::MAX));
        assert_eq!(理由, "PSDに有効な合成画像がないため省く");
        assert!(補足.contains("Maximize Compatibility"), "{補足}");
        // 真：使う。
        let mut 真 = 組立::新規();
        真.資源 = vec![版情報(true)];
        真.本体 = vec![1; 2 * 2 * 3];
        let 画像 = decode_psd(&真.組み立てる(), u64::MAX).expect("結合ありは使える");
        assert_eq!(画像.into_pixels()[0], 1);
        // 本文が短い版情報は情報なしとして使う。
        let mut 短 = 組立::新規();
        短.資源 = vec![(VERSION_INFO_ID, vec![0, 0])];
        短.本体 = vec![2; 2 * 2 * 3];
        let 画像 = decode_psd(&短.組み立てる(), u64::MAX).expect("短い版情報は無視する");
        assert_eq!(画像.into_pixels()[0], 2);
    }

    #[test]
    fn 対象外の版と深度と形式と圧縮は警告して省く() {
        let mut 版 = 組立::新規();
        版.版 = 2;
        let (理由, 補足) = 省く理由(decode_psd(&版.組み立てる(), u64::MAX));
        assert_eq!(理由, "PSDの版が未対応のため省く");
        assert!(補足.contains("PSB"), "{補足}");

        for (深度, 期待) in [(1, "1bit"), (16, "16bit"), (32, "32bit")] {
            let mut 画像 = 組立::新規();
            画像.深度 = 深度;
            let (理由, 補足) = 省く理由(decode_psd(&画像.組み立てる(), u64::MAX));
            assert_eq!(理由, "PSDの色深度が未対応のため省く");
            assert!(補足.contains(期待), "{補足}");
        }

        for (形式, 期待) in [(0, "Bitmap"), (2, "Indexed"), (4, "CMYK"), (9, "Lab")] {
            let mut 画像 = 組立::新規();
            画像.形式 = 形式;
            画像.チャンネル数 = 3;
            let (理由, 補足) = 省く理由(decode_psd(&画像.組み立てる(), u64::MAX));
            assert_eq!(理由, "PSDの色形式が未対応のため省く");
            assert!(補足.contains(期待), "{補足}");
        }

        // 追加チャンネルは4チャンネルをRGBAとして扱わず省く。
        let mut 追加 = 組立::新規();
        追加.チャンネル数 = 4;
        追加.本体 = vec![0; 2 * 2 * 4];
        let (理由, 補足) = 省く理由(decode_psd(&追加.組み立てる(), u64::MAX));
        assert_eq!(理由, "PSDのチャンネル数が未対応のため省く");
        assert!(補足.contains("4チャンネル"), "{補足}");

        let mut 灰追加 = 組立::新規();
        灰追加.形式 = MODE_GRAYSCALE;
        灰追加.チャンネル数 = 2;
        灰追加.本体 = vec![0; 2 * 2 * 2];
        let (理由, _) = 省く理由(decode_psd(&灰追加.組み立てる(), u64::MAX));
        assert_eq!(理由, "PSDのチャンネル数が未対応のため省く");

        for (圧縮, 期待) in [(2, "ZIP"), (3, "ZIP"), (99, "方式99")] {
            let mut 画像 = 組立::新規();
            画像.圧縮 = 圧縮;
            let (理由, 補足) = 省く理由(decode_psd(&画像.組み立てる(), u64::MAX));
            assert_eq!(理由, "PSDの圧縮方式が未対応のため省く");
            assert!(補足.contains(期待), "{補足}");
        }
    }

    #[test]
    fn 層区間と資源区間の不足と巨大長は省いてパニックしない() {
        // 層区間が足りない。
        let mut 層不足 = 組立::新規();
        層不足.層区間 = vec![0; 4];
        let mut バイト列 = 層不足.組み立てる();
        バイト列.truncate(バイト列.len() - 2);
        let (理由, _) = 省く理由(decode_psd(&バイト列, u64::MAX));
        assert_eq!(理由, "PSDに有効な合成画像がないため省く");

        // 巨大な色形式長。
        let mut 巨大 = 組立::新規().組み立てる();
        巨大[26..30].copy_from_slice(&u32::MAX.to_be_bytes());
        let (理由, _) = 省く理由(decode_psd(&巨大, u64::MAX));
        assert_eq!(理由, "PSDに有効な合成画像がないため省く");

        // 壊れた署名は省く。
        let mut 署名 = 組立::新規().組み立てる();
        署名[0] = b'X';
        let (理由, _) = 省く理由(decode_psd(&署名, u64::MAX));
        assert_eq!(理由, "PSDに有効な合成画像がないため省く");
        assert!(!is_psd(&署名));
        assert!(is_psd(&組立::新規().組み立てる()));

        // 寸法0は省く。
        let mut 零 = 組立::新規();
        零.幅 = 0;
        let (理由, _) = 省く理由(decode_psd(&零.組み立てる(), u64::MAX));
        assert_eq!(理由, "PSDに有効な合成画像がないため省く");
    }

    #[test]
    fn どの位置の切断でも復号できずパニックしない() {
        let 正常 = 組立::新規().組み立てる();
        for 切断 in 0..正常.len() {
            let 結果 = decode_psd(&正常[..切断], u64::MAX);
            assert!(結果.is_err(), "切断{切断}では復号できない");
        }
        // RLE版も全位置で確かめる。
        let mut rle = 組立::新規();
        rle.圧縮 = COMPRESSION_RLE;
        rle.高さ = 1;
        // あえて不足行にせず正常なRLEで切断走査する。
        rle.本体 = rle本体(&[3, 3, 3], &[vec![1, 1, 2], vec![1, 3, 4], vec![1, 5, 6]]);
        let 正常 = rle.組み立てる();
        decode_psd(&正常, u64::MAX).expect("走査対象のRLEは正常である");
        for 切断 in 0..正常.len() {
            let 結果 = decode_psd(&正常[..切断], u64::MAX);
            assert!(結果.is_err(), "RLE切断{切断}では復号できない");
        }
    }

    #[test]
    fn 画素上限の一致は通り1超過は失敗する() {
        let 正常 = 組立::新規().組み立てる();
        assert!(psd_dimensions(&正常, 4).is_ok());
        assert!(decode_psd(&正常, 4).is_ok());
        let 失敗 = decode_psd(&正常, 3).expect_err("1超過は失敗する");
        assert!(matches!(失敗, PsdError::Limit(_)));
        let 表 = psd_dimensions(&正常, 3).expect_err("寸法検査も失敗する");
        assert!(matches!(表, PsdError::Limit(_)));
    }

    /// 方式の対象外と本体の不足はRGBA確保より前に省く。
    ///
    /// 40バイト・4096²のZIP／Raw本体なし／RLE行長表なし・不足は
    /// いずれも警告扱い（Skip）で失敗し、64MiBの確保は起きない。
    /// 確保量の直接測定はしない代わり、確保前の判定構造と
    /// エラー種別・文面を固定する。
    #[test]
    fn 確保前に方式と不足を判定して省く() {
        // 40バイト・4096²・ZIP → 方式の対象外で省く。
        let mut zip = 組立::新規();
        zip.幅 = 4096;
        zip.高さ = 4096;
        zip.圧縮 = 2;
        zip.本体 = Vec::new();
        let バイト列 = zip.組み立てる();
        assert_eq!(バイト列.len(), 40, "入力は40バイトである");
        let (理由, 補足) = 省く理由(decode_psd(&バイト列, u64::MAX));
        assert_eq!(理由, "PSDの圧縮方式が未対応のため省く");
        assert!(補足.contains("ZIP"), "{補足}");
        // 40バイト・4096²・Raw本体なし → 不足で省く。
        let mut 空 = 組立::新規();
        空.幅 = 4096;
        空.高さ = 4096;
        空.圧縮 = COMPRESSION_RAW;
        空.本体 = Vec::new();
        let バイト列 = 空.組み立てる();
        assert_eq!(バイト列.len(), 40, "入力は40バイトである");
        let (理由, 補足) = 省く理由(decode_psd(&バイト列, u64::MAX));
        assert_eq!(理由, "PSDに有効な合成画像がないため省く");
        assert!(補足.contains("Raw平面"), "{補足}");
        // 40バイト・4096²・RLE行長表なし → 不足で省く。
        let mut 表なし = 組立::新規();
        表なし.幅 = 4096;
        表なし.高さ = 4096;
        表なし.圧縮 = COMPRESSION_RLE;
        表なし.本体 = Vec::new();
        let バイト列 = 表なし.組み立てる();
        assert_eq!(バイト列.len(), 40, "入力は40バイトである");
        let (理由, 補足) = 省く理由(decode_psd(&バイト列, u64::MAX));
        assert_eq!(理由, "PSDに有効な合成画像がないため省く");
        assert!(補足.contains("RLE行長表"), "{補足}");
        // RLE行長表が1バイトだけ → 不足で省く。
        let mut 表不足 = 組立::新規();
        表不足.幅 = 4096;
        表不足.高さ = 4096;
        表不足.圧縮 = COMPRESSION_RLE;
        表不足.本体 = vec![0];
        let (理由, 補足) = 省く理由(decode_psd(&表不足.組み立てる(), u64::MAX));
        assert_eq!(理由, "PSDに有効な合成画像がないため省く");
        assert!(補足.contains("RLE行長表"), "{補足}");
    }

    /// 画像リソース区間の宣言長を縮めても境界を越えて読まない。
    ///
    /// 1057=true を持つ正常PSDに対し、総長を小さくした各ケースは
    /// すべて警告扱い（Skip）で失敗し、パニックしない。
    /// 走査は区間に限定した部分カーソルで行い、区間外は読まない。
    #[test]
    fn 資源区間の宣言長不足は境界内で失敗する() {
        let mut 正常 = 組立::新規();
        正常.資源 = vec![版情報(true)];
        let 組立済 = 正常.組み立てる();
        decode_psd(&組立済, u64::MAX).expect("走査対象は正常である");
        // 資源総長の位置はヘッダ26＋色形式長4の直後である。
        let 総長位置 = 30;
        let 宣言 = u32::from_be_bytes(
            組立済[総長位置..総長位置 + 4]
                .try_into()
                .expect("総長は読める"),
        ) as usize;
        assert!(宣言 >= 24, "1057資源1件の総長がある: {宣言}");
        for 縮小 in [1, 4, 10, 17, 23, 宣言 - 1] {
            let mut 切詰 = 組立済.clone();
            切詰[総長位置..総長位置 + 4].copy_from_slice(&(縮小 as u32).to_be_bytes());
            let (理由, _) = 省く理由(decode_psd(&切詰, u64::MAX));
            assert_eq!(
                理由, "PSDに有効な合成画像がないため省く",
                "総長{縮小}では省く"
            );
        }
    }

    /// 巨大寸法の申告でも確保せず失敗し、パニックしない。
    ///
    /// 幅2^31・高さ2^30のRaw 40バイト入力は、本体不足の判定が
    /// RGBA確保より先に働くため、警告扱い（Skip）で省く。
    /// 確保も capacity overflow も起きない。
    /// 画素上限を設けた場合はヘッダ検査で上限超過を返す。
    #[test]
    fn 巨大寸法は確保せず失敗しパニックしない() {
        let mut 巨大 = 組立::新規();
        巨大.幅 = 1 << 31;
        巨大.高さ = 1 << 30;
        巨大.本体 = Vec::new();
        let バイト列 = 巨大.組み立てる();
        assert_eq!(バイト列.len(), 40, "入力は40バイトである");
        let (理由, 補足) = 省く理由(decode_psd(&バイト列, u64::MAX));
        assert_eq!(理由, "PSDに有効な合成画像がないため省く");
        assert!(補足.contains("Raw平面"), "{補足}");
        let 失敗 = decode_psd(&バイト列, 4).expect_err("画素上限では失敗する");
        assert!(
            matches!(失敗, PsdError::Limit(_)),
            "上限超過で返す: {失敗:?}"
        );
    }
}
