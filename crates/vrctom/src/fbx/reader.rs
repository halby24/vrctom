//! バイナリFBXの低水準ノード走査。
//!
//! 公開契約の入口は [`crate::fbx`] を参照。
//!
//! 版によりノードレコードの幅を読み分ける。7000〜7400は32bit幅
//! （EndOffset u32・NumProperties u32・PropertyListLen u32・NameLen u8の13バイト）、
//! 7500〜7700は64bit幅（EndOffset u64・NumProperties u64・PropertyListLen u64・
//! NameLen u8の25バイト）である。プロパティ本体の形式（文字列・生バイトの長さ前置き、
//! 配列の要素数・encoding・圧縮長前置き）はいずれも32bit幅のままであり、
//! FBX SDK 2020.3.x再書き出しの7700版3件（ミルクRe・シフォン・カリン）で確認した。
//! ノード名・プロパティ・親子の境界を検証し、配列の中身は展開せず、
//! [`ArrayDesc`] として絶対位置だけを残す。値の意味付け（ModelやGeometryの解釈）は上位で行う。

use super::{FbxError, FbxErrorKind, FbxOptions};

/// 配列プロパティの要素型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ArrayKind {
    /// `f`。4バイト単精度実数。
    F32,
    /// `d`。8バイト倍精度実数。
    F64,
    /// `i`。4バイト符号付き整数。
    I32,
    /// `l`。8バイト符号付き整数。
    I64,
    /// `b`。1バイト。
    U8,
}

impl ArrayKind {
    /// 型コードから要素型を求める。未知の型コードの場合は `None` を返す。
    fn from_code(code: u8) -> Option<Self> {
        match code {
            b'f' => Some(Self::F32),
            b'd' => Some(Self::F64),
            b'i' => Some(Self::I32),
            b'l' => Some(Self::I64),
            b'b' => Some(Self::U8),
            _ => None,
        }
    }

    /// 1要素のバイト数を返す。
    pub(crate) fn elem_size(self) -> usize {
        match self {
            Self::F32 | Self::I32 => 4,
            Self::F64 | Self::I64 => 8,
            Self::U8 => 1,
        }
    }
}

/// 配列プロパティの位置情報。
///
/// 中身は [`super::FbxFile::mesh_data`] が要求されたときに境界検査付きで展開する。
/// 展開前の保持量がファイル全体に比例しないようにするためである。
#[derive(Debug, Clone, Copy)]
pub(crate) struct ArrayDesc {
    /// 要素型。
    pub(crate) kind: ArrayKind,
    /// 要素数。
    pub(crate) count: usize,
    /// 圧縮方式（0=raw、1=zlib）。
    pub(crate) encoding: u32,
    /// ペイロード先頭の絶対オフセット。
    pub(crate) data_offset: usize,
    /// ペイロードのバイト長（圧縮時は圧縮後）。
    pub(crate) compressed_len: usize,
}

/// ノードのプロパティ1件。
#[derive(Debug, Clone)]
pub(crate) enum RawProp {
    /// `Y`。2バイト符号付き整数。
    Y(i16),
    /// `C`。1バイト。0以外を真として扱う。
    C(bool),
    /// `I`。4バイト符号付き整数。
    I(i32),
    /// `F`。4バイト単精度実数。
    F(f32),
    /// `D`。8バイト倍精度実数。
    D(f64),
    /// `L`。8バイト符号付き整数。
    L(i64),
    /// `S`。長さ付きバイト列。文字列長の上限で抑える。
    S(Box<[u8]>),
    /// `R`。長さ付き生バイト。複写せず位置だけを残す。
    // 走査中の境界の読み飛ばしに使う。保持した位置は現在どこからも読まないため、
    // 未使用として許容する。
    #[allow(dead_code)]
    R {
        /// ペイロード先頭の絶対オフセット。
        offset: usize,
        /// ペイロードのバイト長。
        len: usize,
    },
    /// `f`・`d`・`l`・`i`・`b`。中身は展開せず位置だけを残す。
    Arr(ArrayDesc),
}

impl RawProp {
    /// 文字列プロパティの場合に中身を返す。
    pub(crate) fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Self::S(bytes) => Some(bytes),
            _ => None,
        }
    }

    /// 整数として読める場合に値を返す。
    ///
    /// 整数型（`Y`・`C`・`I`・`L`）だけを受け付け、実数型は受け付けない。
    /// 型の取り違えを黙って丸めないためである。
    pub(crate) fn as_int(&self) -> Option<i64> {
        match *self {
            Self::Y(v) => Some(i64::from(v)),
            Self::C(v) => Some(i64::from(v)),
            Self::I(v) => Some(i64::from(v)),
            Self::L(v) => Some(v),
            _ => None,
        }
    }

    /// 数値として読める場合に値を返す。
    ///
    /// 整数型と実数型のどちらも受け付け、`f64` に寄せる。
    pub(crate) fn as_number(&self) -> Option<f64> {
        match *self {
            Self::Y(v) => Some(f64::from(v)),
            Self::C(v) => Some(f64::from(v)),
            Self::I(v) => Some(f64::from(v)),
            Self::L(v) => Some(v as f64),
            Self::F(v) => Some(f64::from(v)),
            Self::D(v) => Some(v),
            _ => None,
        }
    }

    /// 配列プロパティの場合に位置情報を返す。
    pub(crate) fn as_array(&self) -> Option<ArrayDesc> {
        match *self {
            Self::Arr(desc) => Some(desc),
            _ => None,
        }
    }
}

/// 検証済みのノード1件。
#[derive(Debug, Clone)]
pub(crate) struct RawNode {
    /// ノード名。UTF-8として検証済みのもの。
    pub(crate) name: Box<str>,
    /// プロパティ列。
    pub(crate) props: Vec<RawProp>,
    /// 子ノード列。出現順を保つ。
    pub(crate) children: Vec<RawNode>,
}

impl RawNode {
    /// 指定名の子を出現順に集める。
    pub(crate) fn children_by(&self, name: &str) -> impl Iterator<Item = &RawNode> {
        self.children
            .iter()
            .filter(move |child| &*child.name == name)
    }

    /// 指定名の先頭の子を返す。
    pub(crate) fn child(&self, name: &str) -> Option<&RawNode> {
        self.children_by(name).next()
    }
}

/// 走査中の上限計数。
struct Scanner<'a> {
    bytes: &'a [u8],
    options: &'a FbxOptions,
    /// 真のとき7500以上の64bit幅レコードで読む。偽のとき32bit幅で読む。
    wide: bool,
    /// 読み終えたノード数。
    nodes: usize,
    /// 読み終えた配列の要素数の合計。
    total_array_elements: u64,
}

impl Scanner<'_> {
    /// ノード先頭のバイト長を返す。32bit幅で13、64bit幅で25である。
    fn record_header_len(&self) -> usize {
        if self.wide { 25 } else { 13 }
    }

    /// 指定範囲の u32 LE を読む。範囲外の場合は切り詰めとして失敗する。
    fn u32_at(&self, offset: usize) -> Result<u32, FbxError> {
        let bytes = self
            .bytes
            .get(offset..offset.saturating_add(4))
            .ok_or_else(|| {
                FbxError::new(
                    FbxErrorKind::InvalidStructure,
                    "ノード境界の読み取りで切り詰めた",
                )
            })?;
        let Some(chunk) = bytes.get(..4) else {
            return Err(FbxError::new(
                FbxErrorKind::InvalidStructure,
                "ノード境界の読み取りで切り詰めた",
            ));
        };
        Ok(u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
    }

    /// 指定範囲の u64 LE を読む。範囲外の場合は切り詰めとして失敗する。
    fn u64_at(&self, offset: usize) -> Result<u64, FbxError> {
        let bytes = self
            .bytes
            .get(offset..offset.saturating_add(8))
            .ok_or_else(|| {
                FbxError::new(
                    FbxErrorKind::InvalidStructure,
                    "ノード境界の読み取りで切り詰めた",
                )
            })?;
        let Some(chunk) = bytes.get(..8) else {
            return Err(FbxError::new(
                FbxErrorKind::InvalidStructure,
                "ノード境界の読み取りで切り詰めた",
            ));
        };
        Ok(u64::from_le_bytes([
            chunk[0], chunk[1], chunk[2], chunk[3], chunk[4], chunk[5], chunk[6], chunk[7],
        ]))
    }

    /// u64値を添え字に使う。収まらない場合は上限超過として失敗する。
    ///
    /// 32bit標的（wasm32を含む）ではu64が `usize` に収まらないため、
    /// 黙って丸めず `LimitExceeded` で止める。64bit標的では通るが、
    /// その後の範囲検査（親範囲・入力長との比較）が破損を弾く。
    fn offset_in_range(value: u64, what: &'static str) -> Result<usize, FbxError> {
        usize::try_from(value).map_err(|_| {
            FbxError::new(FbxErrorKind::LimitExceeded, format!("{what}が上限を超えた"))
        })
    }

    /// ノード1件を `[pos, parent_end)` の範囲で読む。
    ///
    /// null終端の場合は `Ok(None)` を返し、呼び出し側が子列の終わりとして扱う。
    /// 終端の有無の判定自体が境界検査であり、黙ってEOFを終端とみなさない。
    fn node_at(
        &mut self,
        mut pos: usize,
        parent_end: usize,
        depth: usize,
    ) -> Result<Option<(RawNode, usize)>, FbxError> {
        let header = self.record_header_len();
        if pos.saturating_add(header) > parent_end || pos.saturating_add(header) > self.bytes.len()
        {
            return Err(FbxError::new(
                FbxErrorKind::InvalidStructure,
                "ノード先頭の読み取りで切り詰めた",
            ));
        }
        // 先頭3項目の幅だけが版で変わる。u64に寄せて読み、32bit版は上位が0になる。
        // 名前長は両版ともu8のままである。
        let (end_raw, props_raw, list_raw, name_len) = if self.wide {
            (
                self.u64_at(pos)?,
                self.u64_at(pos + 8)?,
                self.u64_at(pos + 16)?,
                self.bytes[pos + 24] as usize,
            )
        } else {
            (
                self.u32_at(pos)? as u64,
                self.u32_at(pos + 4)? as u64,
                self.u32_at(pos + 8)? as u64,
                self.bytes[pos + 12] as usize,
            )
        };
        if end_raw == 0 && props_raw == 0 && list_raw == 0 && name_len == 0 {
            return Ok(None);
        }
        if depth > self.options.max_depth {
            return Err(FbxError::new(
                FbxErrorKind::LimitExceeded,
                "ノードの深さが上限を超えた",
            ));
        }
        self.nodes = self.nodes.saturating_add(1);
        if self.nodes > self.options.max_nodes {
            return Err(FbxError::new(
                FbxErrorKind::LimitExceeded,
                "ノード数が上限を超えた",
            ));
        }
        if end_raw <= pos as u64 {
            return Err(FbxError::new(
                FbxErrorKind::InvalidStructure,
                "ノードの終了位置が現在位置以前である",
            ));
        }
        if end_raw > parent_end as u64 {
            return Err(FbxError::new(
                FbxErrorKind::InvalidStructure,
                "ノードの終了位置が親の範囲外である",
            ));
        }
        if end_raw > self.bytes.len() as u64 {
            return Err(FbxError::new(
                FbxErrorKind::InvalidStructure,
                "ノードの終了位置が入力を超える",
            ));
        }
        // 添え字化の前にu64の値を上限検査する。溢れは破損ではなく上限超過とする。
        let end_offset = Self::offset_in_range(end_raw, "ノードの終了位置")?;
        let num_props = Self::offset_in_range(props_raw, "プロパティ数")?;
        let prop_list_len = Self::offset_in_range(list_raw, "プロパティ列長")?;
        pos += header;
        let name_end = pos.saturating_add(name_len);
        if name_end > end_offset {
            return Err(FbxError::new(
                FbxErrorKind::InvalidStructure,
                "ノード名がノード範囲を超える",
            ));
        }
        let name = std::str::from_utf8(&self.bytes[pos..name_end])
            .map_err(|_| FbxError::new(FbxErrorKind::InvalidStructure, "ノード名がUTF-8でない"))?
            .to_owned()
            .into_boxed_str();
        pos = name_end;
        // プロパティ列長は名前を含まない。加算の溢れも上限超過として扱う。
        let prop_end = pos.checked_add(prop_list_len).ok_or_else(|| {
            FbxError::new(FbxErrorKind::LimitExceeded, "プロパティ列長が上限を超えた")
        })?;
        if prop_end > end_offset {
            return Err(FbxError::new(
                FbxErrorKind::InvalidStructure,
                "プロパティ列がノード範囲を超える",
            ));
        }
        let mut props = Vec::with_capacity(num_props.min(16));
        for _ in 0..num_props {
            let (prop, next) = self.prop_at(pos, prop_end)?;
            props.push(prop);
            pos = next;
        }
        if pos != prop_end {
            return Err(FbxError::new(
                FbxErrorKind::InvalidStructure,
                "プロパティ列長と読み取り量が一致しない",
            ));
        }
        let mut children = Vec::new();
        if pos < end_offset {
            loop {
                match self.node_at(pos, end_offset, depth + 1)? {
                    Some((child, next)) => {
                        pos = next;
                        children.push(child);
                    }
                    None => {
                        // null終端は子列の一部であり、親範囲の末端でなければならない。
                        pos += header;
                        break;
                    }
                }
            }
            if pos != end_offset {
                return Err(FbxError::new(
                    FbxErrorKind::InvalidStructure,
                    "子列の終端がノード範囲の末端でない",
                ));
            }
        }
        Ok(Some((
            RawNode {
                name,
                props,
                children,
            },
            pos,
        )))
    }

    /// プロパティ1件を `[pos, prop_end)` の範囲で読む。
    fn prop_at(&mut self, mut pos: usize, prop_end: usize) -> Result<(RawProp, usize), FbxError> {
        let code = *self.bytes.get(pos).ok_or_else(|| {
            FbxError::new(
                FbxErrorKind::InvalidStructure,
                "プロパティ型の読み取りで切り詰めた",
            )
        })?;
        pos += 1;
        let remaining = prop_end.saturating_sub(pos);
        match code {
            b'Y' => {
                if remaining < 2 {
                    return Err(truncated());
                }
                let v = i16::from_le_bytes([self.bytes[pos], self.bytes[pos + 1]]);
                Ok((RawProp::Y(v), pos + 2))
            }
            b'C' => {
                if remaining < 1 {
                    return Err(truncated());
                }
                Ok((RawProp::C(self.bytes[pos] != 0), pos + 1))
            }
            b'I' => {
                if remaining < 4 {
                    return Err(truncated());
                }
                let v = i32::from_le_bytes([
                    self.bytes[pos],
                    self.bytes[pos + 1],
                    self.bytes[pos + 2],
                    self.bytes[pos + 3],
                ]);
                Ok((RawProp::I(v), pos + 4))
            }
            b'F' => {
                if remaining < 4 {
                    return Err(truncated());
                }
                let v = f32::from_le_bytes([
                    self.bytes[pos],
                    self.bytes[pos + 1],
                    self.bytes[pos + 2],
                    self.bytes[pos + 3],
                ]);
                Ok((RawProp::F(v), pos + 4))
            }
            b'D' => {
                if remaining < 8 {
                    return Err(truncated());
                }
                let v = f64::from_le_bytes([
                    self.bytes[pos],
                    self.bytes[pos + 1],
                    self.bytes[pos + 2],
                    self.bytes[pos + 3],
                    self.bytes[pos + 4],
                    self.bytes[pos + 5],
                    self.bytes[pos + 6],
                    self.bytes[pos + 7],
                ]);
                Ok((RawProp::D(v), pos + 8))
            }
            b'L' => {
                if remaining < 8 {
                    return Err(truncated());
                }
                let v = i64::from_le_bytes([
                    self.bytes[pos],
                    self.bytes[pos + 1],
                    self.bytes[pos + 2],
                    self.bytes[pos + 3],
                    self.bytes[pos + 4],
                    self.bytes[pos + 5],
                    self.bytes[pos + 6],
                    self.bytes[pos + 7],
                ]);
                Ok((RawProp::L(v), pos + 8))
            }
            b'S' => {
                if remaining < 4 {
                    return Err(truncated());
                }
                let len = self.u32_at(pos)? as usize;
                if len > self.options.max_string_bytes {
                    return Err(FbxError::new(
                        FbxErrorKind::LimitExceeded,
                        "文字列長が上限を超えた",
                    ));
                }
                if len > prop_end.saturating_sub(pos + 4) {
                    return Err(truncated());
                }
                let bytes: Box<[u8]> = self.bytes[pos + 4..pos + 4 + len].into();
                Ok((RawProp::S(bytes), pos + 4 + len))
            }
            b'R' => {
                if remaining < 4 {
                    return Err(truncated());
                }
                let len = self.u32_at(pos)? as usize;
                if len > prop_end.saturating_sub(pos + 4) {
                    return Err(truncated());
                }
                let offset = pos + 4;
                Ok((RawProp::R { offset, len }, offset + len))
            }
            b'f' | b'd' | b'l' | b'i' | b'b' => {
                let Some(kind) = ArrayKind::from_code(code) else {
                    return Err(FbxError::new(
                        FbxErrorKind::UnsupportedFeature,
                        "非対応のプロパティ型である",
                    ));
                };
                if remaining < 12 {
                    return Err(truncated());
                }
                let count = self.u32_at(pos)? as usize;
                let encoding = self.u32_at(pos + 4)?;
                let compressed_len = self.u32_at(pos + 8)? as usize;
                if count > self.options.max_array_elements {
                    return Err(FbxError::new(
                        FbxErrorKind::LimitExceeded,
                        "配列要素数が上限を超えた",
                    ));
                }
                self.total_array_elements = self.total_array_elements.saturating_add(count as u64);
                if self.total_array_elements > self.options.max_total_array_elements as u64 {
                    return Err(FbxError::new(
                        FbxErrorKind::LimitExceeded,
                        "配列要素数の合計が上限を超えた",
                    ));
                }
                if encoding != 0 && encoding != 1 {
                    return Err(FbxError::new(
                        FbxErrorKind::UnsupportedFeature,
                        "非対応の配列圧縮方式である",
                    ));
                }
                if compressed_len > prop_end.saturating_sub(pos + 12) {
                    return Err(truncated());
                }
                if encoding == 0 && compressed_len != count.saturating_mul(kind.elem_size()) {
                    return Err(FbxError::new(
                        FbxErrorKind::InvalidStructure,
                        "非圧縮配列の長さが要素数と一致しない",
                    ));
                }
                let desc = ArrayDesc {
                    kind,
                    count,
                    encoding,
                    data_offset: pos + 12,
                    compressed_len,
                };
                Ok((RawProp::Arr(desc), pos + 12 + compressed_len))
            }
            _ => Err(FbxError::new(
                FbxErrorKind::UnsupportedFeature,
                "非対応のプロパティ型である",
            )),
        }
    }
}

/// プロパティ読み取り中の切り詰めを表す。
fn truncated() -> FbxError {
    FbxError::new(
        FbxErrorKind::InvalidStructure,
        "プロパティの読み取りで切り詰めた",
    )
}

/// バイナリFBXのヘッダを検証し、版と直下ノード列を返す。
///
/// トップレベルの終端後はフッタが続くため、EOFは要求しない。
/// 終端以降は読まず、無視する。
pub(crate) fn parse_top(
    bytes: &[u8],
    options: &FbxOptions,
) -> Result<(u32, Vec<RawNode>), FbxError> {
    if bytes.len() > options.max_file_bytes {
        return Err(FbxError::new(
            FbxErrorKind::LimitExceeded,
            "ファイルサイズが上限を超えた",
        ));
    }
    const MAGIC: &[u8; 21] = b"Kaydara FBX Binary  \x00";
    let header_ok =
        bytes.len() >= 27 && &bytes[..21] == MAGIC && bytes[21] == 0x1a && bytes[22] == 0x00;
    if !header_ok {
        return Err(FbxError::new(
            FbxErrorKind::InvalidHeader,
            "バイナリFBXのヘッダでない",
        ));
    }
    let version = u32::from_le_bytes([bytes[23], bytes[24], bytes[25], bytes[26]]);
    if !(7000..=7700).contains(&version) {
        return Err(FbxError::new(
            FbxErrorKind::UnsupportedVersion,
            "対応版（7000以上7700以下）でない",
        ));
    }
    let mut scanner = Scanner {
        bytes,
        options,
        // 7500（FBX 2016）以降はノード先頭が64bit幅になる。
        wide: version >= 7500,
        nodes: 0,
        total_array_elements: 0,
    };
    let mut pos = 27;
    let mut top = Vec::new();
    while let Some((node, next)) = scanner.node_at(pos, bytes.len(), 0)? {
        pos = next;
        top.push(node);
    }
    Ok((version, top))
}

#[cfg(test)]
#[cfg(not(target_arch = "wasm32"))]
mod tests {
    use super::*;

    /// 実fixtureの直下走査で版と主要区画を読む。
    ///
    /// 意味層（`FbxFile::parse`）の可否ではなく、走査層が版の幅で
    /// 直下ノード列を壊さず読めることを確かめる。対象は環境変数
    /// `VRCTOM_FBX_FIXTURE` が指す1件であり、未設定なら跳ばす。
    #[test]
    fn 実fixtureの直下走査で版と区画を読む() {
        let Ok(path) = std::env::var("VRCTOM_FBX_FIXTURE") else {
            eprintln!("VRCTOM_FBX_FIXTUREが未設定のため跳ばす");
            return;
        };
        let bytes = std::fs::read(&path).expect("実fixtureは読める");
        let (version, top) =
            parse_top(&bytes, &FbxOptions::new()).expect("実fixtureの直下は読める");
        assert!(
            (7000..=7700).contains(&version),
            "実fixtureの版は対応範囲である: {version}"
        );
        let names: Vec<&str> = top.iter().map(|node| &*node.name).collect();
        for expected in [
            "FBXHeaderExtension",
            "GlobalSettings",
            "Objects",
            "Connections",
        ] {
            assert!(
                names.contains(&expected),
                "直下に{expected}がある: {names:?}"
            );
        }
        eprintln!(
            "fixture_version={version} top_nodes={} top={names:?}",
            top.len()
        );
    }
}
