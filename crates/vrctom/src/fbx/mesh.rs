//! メッシュ配列の位置情報と展開。
//!
//! 公開契約の入口は [`crate::fbx`] を参照。
//!
//! 解析時には配列の中身を展開せず、 [`crate::fbx::reader`] が残した位置だけを
//! ここで保持する。展開は [`expand`] が対象メッシュ分だけ行い、確保量を
//! 対象の大きさに比例させる。

use super::reader::{ArrayDesc, ArrayKind};
use super::{FbxError, FbxErrorKind, ObjectId};

/// 層要素の対応方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mapping {
    /// 頂点ごと。綴りはFBXの表記どおりである。
    ByVertice,
    /// corner（多角形頂点）ごと。
    ByPolygonVertex,
    /// 面ごと。
    ByPolygon,
    /// 全要素同一。
    AllSame,
}

impl Mapping {
    /// FBXの表記から対応方式を求める。非対応の表記は拒否する。
    pub(crate) fn parse(text: &[u8]) -> Result<Self, FbxError> {
        match text {
            b"ByVertice" => Ok(Self::ByVertice),
            b"ByPolygonVertex" => Ok(Self::ByPolygonVertex),
            b"ByPolygon" => Ok(Self::ByPolygon),
            b"AllSame" => Ok(Self::AllSame),
            _ => Err(FbxError::new(
                FbxErrorKind::UnsupportedFeature,
                format!(
                    "非対応のMappingである: {}",
                    crate::escape_diag(&String::from_utf8_lossy(text))
                ),
            )),
        }
    }
}

/// 層要素の参照方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Reference {
    /// データ配列をそのまま索引に使う。
    Direct,
    /// 索引配列をデータ配列への索引として使う。
    IndexToDirect,
}

impl Reference {
    /// FBXの表記から参照方式を求める。非対応の表記は拒否する。
    pub(crate) fn parse(text: &[u8]) -> Result<Self, FbxError> {
        match text {
            b"Direct" => Ok(Self::Direct),
            b"IndexToDirect" => Ok(Self::IndexToDirect),
            _ => Err(FbxError::new(
                FbxErrorKind::UnsupportedFeature,
                format!(
                    "非対応のReferenceである: {}",
                    crate::escape_diag(&String::from_utf8_lossy(text))
                ),
            )),
        }
    }
}

/// 属性層1件の位置情報。
#[derive(Debug, Clone)]
pub(crate) struct LayerDesc {
    /// 対応方式。
    pub(crate) mapping: Mapping,
    /// 参照方式。
    pub(crate) reference: Reference,
    /// 値の配列。
    pub(crate) data: ArrayDesc,
    /// 索引の配列。`Direct` の場合は `None` になり得る。
    pub(crate) index: Option<ArrayDesc>,
    /// UVセット名。法線・材質では使わない。
    pub(crate) name: Box<str>,
}

/// 配列プロパティを展開してバイト列で返す。
///
/// 圧縮時はzlib展開し、展開後の長さが要素数と一致することを確かめる。
/// 境界外の位置情報は呼び出し側の不変条件違反ではなく入力破損として扱う。
pub(crate) fn expand(bytes: &[u8], desc: ArrayDesc, object: ObjectId) -> Result<Vec<u8>, FbxError> {
    let plain_len = desc.count.saturating_mul(desc.kind.elem_size());
    let payload = bytes
        .get(desc.data_offset..desc.data_offset.saturating_add(desc.compressed_len))
        .ok_or_else(|| {
            FbxError::new(FbxErrorKind::InvalidStructure, "配列位置が入力を超える")
                .with_object(object)
        })?;
    if desc.encoding == 0 {
        if payload.len() != plain_len {
            return Err(FbxError::new(
                FbxErrorKind::InvalidStructure,
                "非圧縮配列の長さが要素数と一致しない",
            )
            .with_object(object));
        }
        Ok(payload.to_vec())
    } else if desc.encoding == 1 {
        use std::io::Read as _;
        let decoder = flate2::read::ZlibDecoder::new(payload);
        // 宣言長を超える展開（zlibボム）を抑えるため、読み取りを宣言長+1で打ち切る。
        // 打ち切り後に長さが一致しない場合は破損として失敗する。
        let mut limited = decoder.take(plain_len as u64 + 1);
        let mut plain = Vec::with_capacity(plain_len.min(8 << 20));
        limited.read_to_end(&mut plain).map_err(|error| {
            FbxError::new(FbxErrorKind::InvalidStructure, "圧縮配列の展開に失敗した")
                .with_object(object)
                .with_source(error)
        })?;
        if plain.len() != plain_len {
            return Err(FbxError::new(
                FbxErrorKind::InvalidStructure,
                "展開後の長さが要素数と一致しない",
            )
            .with_object(object));
        }
        Ok(plain)
    } else {
        Err(FbxError::new(
            FbxErrorKind::UnsupportedFeature,
            "非対応の配列圧縮方式である",
        )
        .with_object(object))
    }
}

/// 展開済みバイト列を `f64` 列として読む。
///
/// `f` と `d` のどちらも受け付ける。頂点・法線・UVはどちらで書かれても
/// 意味が変わらないため、型の違いで拒否しない。
pub(crate) fn as_f64s(
    plain: &[u8],
    kind: ArrayKind,
    object: ObjectId,
) -> Result<Vec<f64>, FbxError> {
    match kind {
        ArrayKind::F64 => {
            let mut out = Vec::with_capacity(plain.len() / 8);
            for chunk in plain.chunks_exact(8) {
                out.push(f64::from_le_bytes([
                    chunk[0], chunk[1], chunk[2], chunk[3], chunk[4], chunk[5], chunk[6], chunk[7],
                ]));
            }
            Ok(out)
        }
        ArrayKind::F32 => {
            let mut out = Vec::with_capacity(plain.len() / 4);
            for chunk in plain.chunks_exact(4) {
                out.push(f64::from(f32::from_le_bytes([
                    chunk[0], chunk[1], chunk[2], chunk[3],
                ])));
            }
            Ok(out)
        }
        _ => {
            Err(FbxError::new(FbxErrorKind::InvalidStructure, "実数配列でない").with_object(object))
        }
    }
}

/// 展開済みバイト列を `i64` 列として読む。
///
/// `i` と `l` のどちらも受け付ける。索引はどちらで書かれても
/// 意味が変わらないため、型の違いで拒否しない。
pub(crate) fn as_i64s(
    plain: &[u8],
    kind: ArrayKind,
    object: ObjectId,
) -> Result<Vec<i64>, FbxError> {
    match kind {
        ArrayKind::I32 => {
            let mut out = Vec::with_capacity(plain.len() / 4);
            for chunk in plain.chunks_exact(4) {
                out.push(i64::from(i32::from_le_bytes([
                    chunk[0], chunk[1], chunk[2], chunk[3],
                ])));
            }
            Ok(out)
        }
        ArrayKind::I64 => {
            let mut out = Vec::with_capacity(plain.len() / 8);
            for chunk in plain.chunks_exact(8) {
                out.push(i64::from_le_bytes([
                    chunk[0], chunk[1], chunk[2], chunk[3], chunk[4], chunk[5], chunk[6], chunk[7],
                ]));
            }
            Ok(out)
        }
        _ => {
            Err(FbxError::new(FbxErrorKind::InvalidStructure, "整数配列でない").with_object(object))
        }
    }
}

/// 有限性を確かめる。NaNや無限大は不正として失敗する。
pub(crate) fn ensure_finite(values: &[f64], object: ObjectId) -> Result<(), FbxError> {
    if values.iter().all(|v| v.is_finite()) {
        Ok(())
    } else {
        Err(FbxError::new(
            FbxErrorKind::InvalidStructure,
            "有限でない座標・属性値がある",
        )
        .with_object(object))
    }
}
