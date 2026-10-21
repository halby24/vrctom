//! Unity YAMLのファイル枠の読み取り。
//!
//! 公開契約の入口は [`crate::unity`] を参照。
//!
//! YAMLの一般構文は `yaml_serde` に任せ、この層ではUnity独自のファイル枠だけを
//! 扱う。BOM・改行・先頭directiveの検証と、列0の `--- !u!` 行での文書分割までを
//! 行い、本文は変更せず文書単位で `yaml_serde` へ渡す。独自のインデント解析や
//! flow mappingパーサは持たない。
//!
//! 区切り判定は行全体の `trim()` では行わず、列0の `---` から始まる完全な
//! ヘッダ形式だけを区切りとする。block scalar内の `---` 風文字列との誤認を避ける。

use std::collections::HashMap;

use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use std::fmt;
use yaml_serde::value::{Tag, TaggedValue};
use yaml_serde::{Mapping, Value};

use super::{FileId, UnityError, UnityErrorKind};

/// SerializedFile 1件の既定上限（バイト）。
///
/// 通常のprefab・material（数MB以下）に十分な余裕を持たせた仕様値。
const DEFAULT_MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
/// 1ファイル内の文書数の既定上限。
const DEFAULT_MAX_DOCUMENTS: usize = 100_000;

/// 数値として解決された巨大整数である印（crate内部の実装詳細）。
///
/// `Value` は引用符の有無などの出自を残さないため、64bitに収まらない整数を
/// 10進文字へ直すだけでは文字列スカラーと区別できない。
/// GUIDのように文字列解決だけを受け付ける読み取りが誤受理しないよう、
/// タグ付き値として保持する。桁自体は10進で正確に残る。
/// タグ名に利用者が依存してはならない。公開表現としては保証しない。
pub(crate) const BIGNUM_TAG: &str = "vrctom:bignum";

/// Unity YAML先頭のdirective。
const DIRECTIVE_YAML: &str = "%YAML 1.1";
/// Unity YAML先頭のタグdirective。
const DIRECTIVE_TAG: &str = "%TAG !u! tag:unity3d.com,2011:";

/// `SerializedFile::parse` の資源上限。
///
/// フィールドは非公開で、既定値は [`Default`] が設定する。
/// 上限の変更は `with_` で始まるメソッドで行う。
#[derive(Debug, Clone)]
pub struct ParseOptions {
    max_file_bytes: u64,
    max_documents: usize,
}

impl ParseOptions {
    /// 既定の上限を持つ設定を生成する。
    pub fn new() -> Self {
        Self {
            max_file_bytes: DEFAULT_MAX_FILE_BYTES,
            max_documents: DEFAULT_MAX_DOCUMENTS,
        }
    }

    /// 1件の上限（バイト）を変更する。
    pub fn with_max_file_bytes(mut self, max_file_bytes: u64) -> Self {
        self.max_file_bytes = max_file_bytes;
        self
    }

    /// 1ファイル内の文書数の上限を変更する。
    pub fn with_max_documents(mut self, max_documents: usize) -> Self {
        self.max_documents = max_documents;
        self
    }
}

impl Default for ParseOptions {
    fn default() -> Self {
        Self::new()
    }
}

/// UnityのSerializedFile（prefab・`.mat` などのYAML文書列）。
///
/// 文書の一覧を持ち、型付き読み取りは [`Document::decode`] で行う。
/// `PrefabInstance` の合成やメッシュの解決はしない。
#[derive(Debug, Clone)]
pub struct SerializedFile {
    documents: Vec<Document>,
}

impl SerializedFile {
    /// バイト列をUnity YAMLとして読み取る。
    ///
    /// BOM・LF/CRLFを扱い、先頭directiveと文書ヘッダを検証してから
    /// 本文を `yaml_serde` で読む。トップレベルは単一クラス名への
    /// マッピングであることを確かめる。
    ///
    /// # Errors
    ///
    /// 上限超過・directive不一致・ヘッダ不良・YAML構文不良・
    /// クラス形状違い・fileID重複などの場合にエラーを返す。
    pub fn parse(bytes: &[u8], options: &ParseOptions) -> Result<Self, UnityError> {
        if bytes.len() as u64 > options.max_file_bytes {
            return Err(UnityError::new(
                UnityErrorKind::LimitExceeded,
                format!("SerializedFileが上限を超えた: {}バイト", bytes.len()),
            ));
        }
        let text = std::str::from_utf8(bytes).map_err(|error| {
            UnityError::new(
                UnityErrorKind::UnsupportedFormat,
                format!(
                    "Unity YAMLがUTF-8でない: {}",
                    crate::escape_diag(&error.to_string())
                ),
            )
            .with_source(error)
        })?;
        // UTF-8のBOMだけを取り除く。本文のBOMはYAMLとして扱う。
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        // 行境界のバイト位置を記録する。本文は行の再結合ではなく
        // 元バイト列の範囲で切り出す。block scalar末尾の改行の有無が
        // 途中文書と最終文書で変わらないようにし、改行符号（LF/CRLF）も保つ。
        // `split('\n')` と同じ区切りであり、行内容に `\n` は含まない。
        let mut line_starts: Vec<usize> = Vec::new();
        let mut lines: Vec<&str> = Vec::new();
        {
            let mut start = 0;
            for (index, byte) in text.bytes().enumerate() {
                if byte == b'\n' {
                    line_starts.push(start);
                    lines.push(&text[start..index]);
                    start = index + 1;
                }
            }
            line_starts.push(start);
            lines.push(&text[start..]);
        }
        let mut cursor = 0;
        for expected in [DIRECTIVE_YAML, DIRECTIVE_TAG] {
            let line = lines.get(cursor).ok_or_else(|| {
                UnityError::new(UnityErrorKind::InvalidYaml, "Unity YAMLのdirectiveがない")
            })?;
            // 行末の `\r`（CRLF）だけを除いて比べる。本文側の扱いとは分ける。
            if line.strip_suffix('\r').unwrap_or(line) != expected {
                return Err(UnityError::new(
                    UnityErrorKind::InvalidYaml,
                    format!("Unity YAMLのdirectiveが不正: {line:?}"),
                )
                .with_line(cursor as u64 + 1));
            }
            cursor += 1;
        }
        let mut documents = Vec::new();
        let mut seen: HashMap<FileId, u64> = HashMap::new();
        // 先頭directiveの後から、列0のヘッダ行で文書を区切る。
        // `trim()` した行全体では判定しない。
        while cursor < lines.len() {
            // 行末の `\r`（CRLF）だけを除いて判定する。本文側の扱いとは分ける。
            let header = lines[cursor].strip_suffix('\r').unwrap_or(lines[cursor]);
            // directive後の空行は読み飛ばす。内容のある行はヘッダでなければならない。
            if !is_document_header(header) {
                if header.trim().is_empty() {
                    cursor += 1;
                    continue;
                }
                return Err(UnityError::new(
                    UnityErrorKind::InvalidYaml,
                    "文書ヘッダでない行がある",
                )
                .with_line(cursor as u64 + 1));
            }
            let header_line = cursor as u64 + 1;
            let (class_id, file_id, stripped) = parse_header(header)
                .ok_or_else(|| {
                    UnityError::new(UnityErrorKind::InvalidYaml, "文書ヘッダが不正")
                        .with_line(header_line)
                })
                .and_then(|parsed| {
                    // classIDは `i32`、fileIDは `i64` の範囲に収める。
                    // 浮動小数点経由では読まない。
                    let (class_text, file_text, stripped) = parsed;
                    let class_id: i32 = class_text.parse().map_err(|_| {
                        UnityError::new(UnityErrorKind::InvalidYaml, "classIDが範囲外")
                            .with_line(header_line)
                    })?;
                    let file_id: i64 = file_text.parse().map_err(|_| {
                        UnityError::new(UnityErrorKind::InvalidYaml, "fileIDが範囲外")
                            .with_line(header_line)
                    })?;
                    Ok((class_id, file_id, stripped))
                })?;
            cursor += 1;
            let body_start = cursor;
            while cursor < lines.len() && !is_document_header(lines[cursor]) {
                cursor += 1;
            }
            // 最終ヘッダ直後で本文が無い（EOF）場合は空スライスとして扱う。
            // `line_starts[body_start]` の範囲外参照でpanicせず、
            // 空本文として `parse_body` の `InvalidYaml` に委ねる。
            let body = if body_start >= line_starts.len() {
                ""
            } else {
                // 本文は元のバイト列の範囲で切り出す。行を集めて再結合すると
                // 末尾改行が落ち、block scalarの値が文書位置で変わる。
                let body_end = if cursor < line_starts.len() {
                    line_starts[cursor]
                } else {
                    text.len()
                };
                &text[line_starts[body_start]..body_end]
            };
            if documents.len() >= options.max_documents {
                return Err(UnityError::new(
                    UnityErrorKind::LimitExceeded,
                    format!("文書数が上限を超えた: {}", documents.len() + 1),
                )
                .with_line(header_line));
            }
            if let Some(first) = seen.get(&file_id) {
                return Err(UnityError::new(
                    UnityErrorKind::Conflict,
                    format!("fileIDが重複する: {file_id}"),
                )
                .with_file_id(file_id)
                .with_line(header_line)
                .with_field(format!("first seen at line {first}")));
            }
            seen.insert(file_id, header_line);
            let document = parse_body(body, class_id, file_id, stripped, body_start as u64 + 1)?;
            documents.push(document);
        }
        Ok(Self { documents })
    }

    /// 文書の一覧を返す。順序はファイル内の出現順である。
    pub fn documents(&self) -> &[Document] {
        &self.documents
    }

    /// fileIDで文書を探す。ない場合は `None` を返す。
    pub fn document(&self, file_id: FileId) -> Option<&Document> {
        self.documents.iter().find(|doc| doc.file_id() == file_id)
    }
}

/// 列0の `---` から始まる行が文書ヘッダの完全形かどうかを返す。
///
/// 行末の `\r` だけを除いて判定する。本文のblock scalar内に現れる
/// `---` 風文字列は、完全形でなければ区切りにしない。
fn is_document_header(line: &str) -> bool {
    let line = line.strip_suffix('\r').unwrap_or(line);
    line.starts_with("---") && parse_header(line).is_some()
}

/// ヘッダ行を `(classID原文, fileID原文, stripped)` に分解する。
///
/// 形式は `--- !u!<classID> &<fileID>[ stripped]` のみ受け付ける。
/// classIDは0以上の10進のみ受け付ける。契約は `i32` だがUnityのclassIDは
/// 非負であり、負値は枠不良として拒否する。数値の解釈は呼び出し側で行う。
fn parse_header(line: &str) -> Option<(&str, &str, bool)> {
    let rest = line.strip_prefix("--- !u!")?;
    let (class_text, rest) = rest.split_once(' ')?;
    if class_text.is_empty() || !class_text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let rest = rest.strip_prefix('&')?;
    // fileIDは符号付き10進のみ受け付ける。浮動小数点・16進は拒否する。
    let (file_text, stripped) = match rest.split_once(' ') {
        Some((file, "stripped")) => (file, true),
        Some(_) => return None,
        None => (rest, false),
    };
    let digits = file_text.strip_prefix('-').unwrap_or(file_text);
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    Some((class_text, file_text, stripped))
}

/// 文書本文をYAMLとして読み、単一クラス形状を検証する。
///
/// 64bitに収まらない整数スカラーは、桁を失わないようタグ付き値として保持する
/// 読み替えを使う。Unity送出に現れる巨大な10進整数（代表型の対象外である
/// 独自欄の値など）で文書全体の読み取りを失敗させないためである。
/// 本文自体は変更せず、`yaml_serde` の構文解釈に任せる。
fn parse_body(
    body: &str,
    class_id: i32,
    file_id: FileId,
    stripped: bool,
    body_start_line: u64,
) -> Result<Document, UnityError> {
    if body.trim().is_empty() {
        return Err(
            UnityError::new(UnityErrorKind::InvalidYaml, "文書本文がない")
                .with_file_id(file_id)
                .with_class_id(class_id),
        );
    }
    let value: Value = yaml_serde::from_str::<WideValue>(body)
        .map(|wide| wide.0)
        .map_err(|error| {
            let line = error
                .location()
                .map(|location| body_start_line + location.line() as u64 - 1);
            let mut failure = UnityError::new(
                UnityErrorKind::InvalidYaml,
                format!(
                    "文書本文のYAMLが読めない: {}",
                    crate::escape_diag(&error.to_string())
                ),
            )
            .with_file_id(file_id)
            .with_class_id(class_id)
            .with_source(error);
            if let Some(line) = line {
                failure = failure.with_line(line);
            }
            failure
        })?;
    let mapping = value.as_mapping().ok_or_else(|| {
        UnityError::new(
            UnityErrorKind::InvalidYaml,
            "文書のトップレベルがマッピングでない",
        )
        .with_file_id(file_id)
        .with_class_id(class_id)
        .with_line(body_start_line)
    })?;
    // 重複キーは `yaml_serde` の読み取り時点で失敗するため、ここでは件数を見る。
    // リスト内の同名プロパティは別物であり、ここでは扱わない。
    if mapping.len() != 1 {
        return Err(UnityError::new(
            UnityErrorKind::InvalidYaml,
            "文書のトップレベルが単一クラスでない",
        )
        .with_file_id(file_id)
        .with_class_id(class_id)
        .with_line(body_start_line));
    }
    let (key, content) = mapping.iter().next().expect("1件あることは確かめた");
    let class_name = key.as_str().ok_or_else(|| {
        UnityError::new(
            UnityErrorKind::InvalidYaml,
            "文書のトップレベルがクラス名でない",
        )
        .with_file_id(file_id)
        .with_class_id(class_id)
        .with_line(body_start_line)
    })?;
    let body_mapping = content.as_mapping().ok_or_else(|| {
        UnityError::new(
            UnityErrorKind::InvalidYaml,
            "文書のクラス本体がマッピングでない",
        )
        .with_file_id(file_id)
        .with_class_id(class_id)
        .with_line(body_start_line)
    })?;
    Ok(Document {
        file_id,
        class_id,
        class_name: class_name.to_owned(),
        stripped,
        raw: Value::Mapping(body_mapping.clone()),
    })
}

/// Unity YAML内の1文書。
///
/// ヘッダ情報とクラス本体の汎用データを保持する。
/// 型付き読み取りは [`Document::decode`] で行い、未知クラスでも
/// 元データはこの文書に残る。
#[derive(Debug, Clone)]
pub struct Document {
    file_id: FileId,
    class_id: i32,
    class_name: String,
    stripped: bool,
    raw: Value,
}

impl Document {
    /// fileIDを返す。
    pub fn file_id(&self) -> FileId {
        self.file_id
    }

    /// classIDを返す。
    pub fn class_id(&self) -> i32 {
        self.class_id
    }

    /// クラス名を返す。
    pub fn class_name(&self) -> &str {
        &self.class_name
    }

    /// `stripped` 文書かどうかを返す。
    pub fn stripped(&self) -> bool {
        self.stripped
    }

    /// クラス本体の汎用データを返す。
    ///
    /// 64bitに収まらない整数スカラーは `!vrctom:bignum` の印を付けた
    /// タグ付き値として現れることがある。これは引用符の有無などの出自を
    /// 残さない汎用値で桁を保つための実装詳細であり、安定した公開表現では
    /// ない。利用者がタグ名や値の形に依存してはならない。
    pub fn raw(&self) -> &Value {
        &self.raw
    }

    /// 文書を代表型に読み替える。
    ///
    /// `stripped` 文書は [`crate::unity::TypedObject`] の `Stripped`、未知クラスは
    /// `Unknown` になる。いずれも元データは残る。
    ///
    /// # Errors
    ///
    /// 既知クラスとclassIDの不一致や必須欄の欠落・形状違いの場合にエラーを返す。
    pub fn decode(&self) -> Result<super::TypedObject, UnityError> {
        super::objects::decode_document(self)
    }
}

/// YAML本文を汎用値として読む。
///
/// 64bitに収まらない整数スカラーは10進のタグ付き値として保持する。
/// 本文の構文解釈自体は `yaml_serde` に任せる。
pub(crate) fn parse_value(text: &str) -> Result<Value, yaml_serde::Error> {
    yaml_serde::from_str::<WideValue>(text).map(|wide| wide.0)
}

/// 数値として解決された巨大整数をタグ付き値として包む。
///
/// [`BIGNUM_TAG`] の印を付け、10進の桁は正確に残す。
/// 基数や区切り子の字面は保たない。
fn bignum_value(text: String) -> Value {
    Value::Tagged(Box::new(TaggedValue {
        tag: Tag::new(BIGNUM_TAG),
        value: Value::String(text),
    }))
}

/// 数値として解決された巨大整数かどうかを返す。
///
/// GUIDのように文字列解決だけを受け付ける読み取りが、数値解決の値を
/// 文字列化して受理しないための判定である。
pub(crate) fn is_bignum(value: &Value) -> bool {
    match value {
        Value::Tagged(tagged) => tagged.tag == BIGNUM_TAG,
        _ => false,
    }
}

/// 数値として解決された巨大整数の10進桁を返す。
///
/// 名前欄のように文字列概念を受け付ける読み取りが、桁を保って受理するための
/// 取り出し口である。GUID読み取りには使わない。
pub(crate) fn bignum_digits(value: &Value) -> Option<&str> {
    match value {
        Value::Tagged(tagged) if tagged.tag == BIGNUM_TAG => tagged.value.as_str(),
        _ => None,
    }
}

/// `yaml_serde::Value` への読み替え。
///
/// 64bitに収まらない整数スカラーだけを10進のタグ付き値として保持する。
/// 桁落ちや文書全体の失敗を避けるための差分であり、構文解釈自体は
/// `yaml_serde` に任せる。写像の重複キーは `yaml_serde` と同じく拒否する。
/// タグ付き値は数値解決の印であり、YAML本来のタグ構文とは区別して扱う。
struct WideValue(Value);

impl<'de> Deserialize<'de> for WideValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(WideVisitor).map(WideValue)
    }
}

/// [`WideValue`] の読み取り本体。
struct WideVisitor;

impl<'de> Visitor<'de> for WideVisitor {
    type Value = Value;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("any YAML value")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Value, E>
    where
        E: de::Error,
    {
        Ok(Value::Bool(value))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Value, E>
    where
        E: de::Error,
    {
        Ok(Value::Number(value.into()))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Value, E>
    where
        E: de::Error,
    {
        Ok(Value::Number(value.into()))
    }

    fn visit_f64<E>(self, value: f64) -> Result<Value, E>
    where
        E: de::Error,
    {
        Ok(Value::Number(value.into()))
    }

    fn visit_str<E>(self, value: &str) -> Result<Value, E>
    where
        E: de::Error,
    {
        Ok(Value::String(value.to_owned()))
    }

    fn visit_string<E>(self, value: String) -> Result<Value, E>
    where
        E: de::Error,
    {
        Ok(Value::String(value))
    }

    fn visit_none<E>(self) -> Result<Value, E>
    where
        E: de::Error,
    {
        Ok(Value::Null)
    }

    fn visit_unit<E>(self) -> Result<Value, E>
    where
        E: de::Error,
    {
        Ok(Value::Null)
    }

    fn visit_u128<E>(self, value: u128) -> Result<Value, E>
    where
        E: de::Error,
    {
        match u64::try_from(value) {
            Ok(int) => Ok(Value::Number(int.into())),
            // 10進の桁は format で正確に残る。基数や区切り子の字面は保たない。
            // 文字列スカラーと区別するため印を付け、GUIDの誤受理を防ぐ。
            Err(_) => Ok(bignum_value(value.to_string())),
        }
    }

    fn visit_i128<E>(self, value: i128) -> Result<Value, E>
    where
        E: de::Error,
    {
        match i64::try_from(value) {
            Ok(int) => Ok(Value::Number(int.into())),
            // 10進の桁は format で正確に残る。基数や区切り子の字面は保たない。
            // 文字列スカラーと区別するため印を付け、GUIDの誤受理を防ぐ。
            Err(_) => Ok(bignum_value(value.to_string())),
        }
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut out = Vec::with_capacity(sequence.size_hint().unwrap_or(0));
        while let Some(entry) = sequence.next_element::<WideValue>()? {
            out.push(entry.0);
        }
        Ok(Value::Sequence(out))
    }

    fn visit_map<A>(self, mut mapping: A) -> Result<Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut out = Mapping::new();
        while let Some(key) = mapping.next_key::<WideValue>()? {
            // `yaml_serde` の写像と同じく重複キーを拒否する。
            // リスト内の同名プロパティは写像ではなく列であり、ここでは扱わない。
            if out.contains_key(&key.0) {
                return Err(de::Error::custom("YAMLの写像に重複キーがある"));
            }
            let value = mapping.next_value::<WideValue>()?;
            out.insert(key.0, value.0);
        }
        Ok(Value::Mapping(out))
    }
}
