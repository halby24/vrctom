//! Unityのバイナリ保存形式（SerializedFile）の読み取り。
//!
//! 公開契約の入口は [`crate::unity`] を参照。
//!
//! Unity 2018以前の資産やAsset Serializationを `Force Binary` にした
//! プロジェクトでは、`.mat`・`.prefab`・`.unity` がYAMLではなく
//! 型木（type tree）付きのバイナリで保存される。この層は型木を手掛かりに
//! オブジェクトを汎用値 [`BinaryValue`] へ読み、代表型（現在は材質）へ写す。
//!
//! 対応する版は型木付きの17〜22（Unity 5.5〜2022系）である。
//! 型木を持たない（ビルド済みの）ファイルは読まない。
//! 汎用値の総数と配列長は残りバイト数と [`BinaryOptions`] の上限で抑える。

use super::{Guid, Material, NamedProperty, ObjectRef, TextureEnvironment};
use super::{UnityError, UnityErrorKind};

/// 対応する最小の版。
const MIN_VERSION: u32 = 17;
/// 対応する最大の版。
const MAX_VERSION: u32 = 22;
/// 材質のclassID。
const MATERIAL_CLASS_ID: i32 = 21;
/// 揃え（4バイト境界）を求めるmetaFlagのビット。
const ALIGN_FLAG: u32 = 0x4000;
/// 共通文字列表を指すオフセットの最上位ビット。
const COMMON_STRING_FLAG: u32 = 0x8000_0000;

/// Unityの共通文字列表。オフセットは各文字列をNUL区切りで並べた位置である。
///
/// Unity 2019以降の表を写したもので、古い版は先頭側の部分集合を使う。
const COMMON_STRINGS: &[&str] = &[
    "AABB",
    "AnimationClip",
    "AnimationCurve",
    "AnimationState",
    "Array",
    "Base",
    "BitField",
    "bitset",
    "bool",
    "char",
    "ColorRGBA",
    "Component",
    "data",
    "deque",
    "double",
    "dynamic_array",
    "FastPropertyName",
    "first",
    "float",
    "Font",
    "GameObject",
    "Generic Mono",
    "GradientNEW",
    "GUID",
    "GUIStyle",
    "int",
    "list",
    "long long",
    "map",
    "Matrix4x4f",
    "MdFour",
    "MonoBehaviour",
    "MonoScript",
    "m_ByteSize",
    "m_Curve",
    "m_EditorClassIdentifier",
    "m_EditorHideFlags",
    "m_Enabled",
    "m_ExtensionPtr",
    "m_GameObject",
    "m_Index",
    "m_IsArray",
    "m_IsStatic",
    "m_MetaFlag",
    "m_Name",
    "m_ObjectHideFlags",
    "m_PrefabInternal",
    "m_PrefabParentObject",
    "m_Script",
    "m_StaticEditorFlags",
    "m_Type",
    "m_Version",
    "Object",
    "pair",
    "PPtr<Component>",
    "PPtr<GameObject>",
    "PPtr<Material>",
    "PPtr<MonoBehaviour>",
    "PPtr<MonoScript>",
    "PPtr<Object>",
    "PPtr<Prefab>",
    "PPtr<Sprite>",
    "PPtr<TextAsset>",
    "PPtr<Texture>",
    "PPtr<Texture2D>",
    "PPtr<Transform>",
    "Prefab",
    "Quaternionf",
    "Rectf",
    "RectInt",
    "RectOffset",
    "second",
    "set",
    "short",
    "size",
    "SInt16",
    "SInt32",
    "SInt64",
    "SInt8",
    "staticvector",
    "string",
    "TextAsset",
    "TextMesh",
    "Texture",
    "Texture2D",
    "Transform",
    "TypelessData",
    "UInt16",
    "UInt32",
    "UInt64",
    "UInt8",
    "unsigned int",
    "unsigned long long",
    "unsigned short",
    "vector",
    "Vector2f",
    "Vector3f",
    "Vector4f",
    "m_ScriptingClassIdentifier",
    "Gradient",
    "Type*",
    "int2_storage",
    "int3_storage",
    "BoundsInt",
    "m_CorrespondingSourceObject",
    "m_PrefabInstance",
    "m_PrefabAsset",
    "FileSize",
    "Hash128",
];

/// 共通文字列表のオフセットから文字列を引く。
fn common_string(offset: u32) -> Option<&'static str> {
    let mut at = 0u32;
    for text in COMMON_STRINGS {
        if at == offset {
            return Some(text);
        }
        at += text.len() as u32 + 1;
    }
    None
}

/// バイナリ読み取りの上限。
#[derive(Debug, Clone)]
pub struct BinaryOptions {
    /// 型の件数の上限。
    pub max_types: usize,
    /// 1型の型木の節点数の上限。
    pub max_type_nodes: usize,
    /// オブジェクト件数の上限。
    pub max_objects: usize,
    /// 1オブジェクトから作る汎用値の総数の上限。
    pub max_values: usize,
}

impl BinaryOptions {
    /// 既定の上限を返す。
    pub fn new() -> Self {
        Self {
            max_types: 4096,
            max_type_nodes: 65_536,
            max_objects: 1_000_000,
            max_values: 4_000_000,
        }
    }
}

impl Default for BinaryOptions {
    fn default() -> Self {
        Self::new()
    }
}

/// 型木の節点1件。
#[derive(Debug, Clone)]
struct TypeNode {
    type_name: String,
    name: String,
    byte_size: i32,
    level: u8,
    is_array: bool,
    meta_flag: u32,
}

/// 型1件。classIDと型木を持つ。
#[derive(Debug, Clone)]
struct SerializedType {
    class_id: i32,
    stripped: bool,
    nodes: Vec<TypeNode>,
}

/// オブジェクト1件の所在。
#[derive(Debug, Clone, Copy)]
pub struct BinaryObject {
    /// ファイル内のオブジェクトID（YAMLのfileIDに当たる）。
    pub path_id: i64,
    /// classID。
    pub class_id: i32,
    /// 型が `stripped`（Prefabインスタンスの対応先だけを持つ）かどうか。
    pub stripped: bool,
    type_index: usize,
    start: usize,
    size: usize,
}

/// 型木から読んだ汎用値。
#[derive(Debug, Clone, PartialEq)]
pub enum BinaryValue {
    /// 真偽値。
    Bool(bool),
    /// 符号付き整数。
    Int(i64),
    /// 符号なし整数。
    UInt(u64),
    /// 浮動小数点数。
    Float(f64),
    /// 文字列。UTF-8でない場合は置換文字で補う。
    String(String),
    /// 型なしのバイト列。
    Bytes(Vec<u8>),
    /// 配列。`vector`・`map`・`set` の要素列である。
    Array(Vec<BinaryValue>),
    /// 構造体。型名を持ち、欄の順序を保つ。
    Struct {
        /// 型木の型名（`PPtr<Texture>`・`Vector3f` など）。
        type_name: String,
        /// 欄の列。
        fields: Vec<(String, BinaryValue)>,
    },
}

impl BinaryValue {
    /// 構造体の欄を名前で引く。
    pub fn field(&self, name: &str) -> Option<&BinaryValue> {
        match self {
            BinaryValue::Struct { fields, .. } => fields
                .iter()
                .find(|(field, _)| field == name)
                .map(|(_, value)| value),
            _ => None,
        }
    }

    /// 整数として読む。
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            BinaryValue::Int(value) => Some(*value),
            BinaryValue::UInt(value) => i64::try_from(*value).ok(),
            BinaryValue::Bool(value) => Some(i64::from(*value)),
            _ => None,
        }
    }

    /// 数値として読む。
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            BinaryValue::Float(value) => Some(*value),
            BinaryValue::Int(value) => Some(*value as f64),
            BinaryValue::UInt(value) => Some(*value as f64),
            _ => None,
        }
    }

    /// 文字列として読む。
    pub fn as_str(&self) -> Option<&str> {
        match self {
            BinaryValue::String(value) => Some(value),
            _ => None,
        }
    }

    /// 配列として読む。
    pub fn as_array(&self) -> Option<&[BinaryValue]> {
        match self {
            BinaryValue::Array(values) => Some(values),
            _ => None,
        }
    }
}

/// 型木付きバイナリの保存ファイル。
#[derive(Debug, Clone)]
pub struct BinaryFile<'a> {
    bytes: &'a [u8],
    version: u32,
    unity_version: String,
    big_endian: bool,
    types: Vec<SerializedType>,
    objects: Vec<BinaryObject>,
    externals: Vec<Option<Guid>>,
    max_values: usize,
}

/// 組み込み資産のパスから、Unity YAMLが書くGUIDを返す。
///
/// バイナリ形式の外部参照表では組み込み資産のGUIDが全ゼロで、パスだけを持つ。
fn builtin_guid(path: &str) -> Option<Guid> {
    let text = match path {
        "library/unity default resources" => "0000000000000000e000000000000000",
        "resources/unity_builtin_extra" | "library/unity_builtin_extra" => {
            "0000000000000000f000000000000000"
        }
        _ => return None,
    };
    Guid::parse(text).ok()
}

/// 先頭がバイナリ保存形式の見出しらしいかを返す。
///
/// 版と寸法の整合だけを見る軽い判定であり、正しさは
/// [`BinaryFile::parse`] が確かめる。
pub fn is_binary_serialized(bytes: &[u8]) -> bool {
    if bytes.len() < 20 || bytes.starts_with(b"%YAML") {
        return false;
    }
    let version = be_u32(bytes, 8);
    let file_size = be_u32(bytes, 4);
    // 非対応版も「非対応」と報告できるよう、版の範囲は広めに見る。
    if !(9..=30).contains(&version) {
        return false;
    }
    // 22以降は旧欄の寸法が0になりうるため、版だけで判定する。
    version >= 22 || file_size as usize == bytes.len()
}

fn be_u32(bytes: &[u8], at: usize) -> u32 {
    u32::from_be_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

/// 読み取り位置付きの読み手。
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
    big_endian: bool,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8], at: usize, big_endian: bool) -> Self {
        Self {
            bytes,
            at,
            big_endian,
        }
    }

    fn take(&mut self, count: usize) -> Result<&'a [u8], UnityError> {
        let end = self
            .at
            .checked_add(count)
            .filter(|end| *end <= self.bytes.len())
            .ok_or_else(|| broken("データが途中で切れている"))?;
        let slice = &self.bytes[self.at..end];
        self.at = end;
        Ok(slice)
    }

    fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.at)
    }

    fn align(&mut self, base: usize) -> Result<(), UnityError> {
        let offset = (self.at - base) % 4;
        if offset != 0 {
            self.take(4 - offset)?;
        }
        Ok(())
    }

    fn u8(&mut self) -> Result<u8, UnityError> {
        Ok(self.take(1)?[0])
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], UnityError> {
        let mut out = [0u8; N];
        out.copy_from_slice(self.take(N)?);
        if self.big_endian {
            out.reverse();
        }
        Ok(out)
    }

    fn u16(&mut self) -> Result<u16, UnityError> {
        Ok(u16::from_le_bytes(self.array()?))
    }

    fn i16(&mut self) -> Result<i16, UnityError> {
        Ok(i16::from_le_bytes(self.array()?))
    }

    fn u32(&mut self) -> Result<u32, UnityError> {
        Ok(u32::from_le_bytes(self.array()?))
    }

    fn i32(&mut self) -> Result<i32, UnityError> {
        Ok(i32::from_le_bytes(self.array()?))
    }

    fn u64(&mut self) -> Result<u64, UnityError> {
        Ok(u64::from_le_bytes(self.array()?))
    }

    fn i64(&mut self) -> Result<i64, UnityError> {
        Ok(i64::from_le_bytes(self.array()?))
    }

    fn f32(&mut self) -> Result<f32, UnityError> {
        Ok(f32::from_le_bytes(self.array()?))
    }

    fn f64(&mut self) -> Result<f64, UnityError> {
        Ok(f64::from_le_bytes(self.array()?))
    }

    fn cstring(&mut self) -> Result<String, UnityError> {
        let rest = &self.bytes[self.at..];
        let end = rest
            .iter()
            .position(|byte| *byte == 0)
            .ok_or_else(|| broken("文字列の終端がない"))?;
        let text = String::from_utf8_lossy(&rest[..end]).into_owned();
        self.at += end + 1;
        Ok(text)
    }

    /// 件数を読み、残りバイト数を超えないことを確かめる。
    fn count(&mut self, what: &str, limit: usize) -> Result<usize, UnityError> {
        let count = self.i32()?;
        let count = usize::try_from(count).map_err(|_| broken(format!("{what}の件数が負")))?;
        if count > limit {
            return Err(UnityError::new(
                UnityErrorKind::LimitExceeded,
                format!("{what}の件数が上限を超えた: {count}"),
            ));
        }
        if count > self.remaining() {
            return Err(broken(format!("{what}の件数が残りの長さを超える: {count}")));
        }
        Ok(count)
    }
}

fn broken(message: impl Into<String>) -> UnityError {
    UnityError::new(UnityErrorKind::InvalidBinary, message)
}

/// Unityのバイナリ表現のGUID（16バイト）を文字列形式へ直す。
///
/// 各バイトの下位4ビット・上位4ビットの順に16進で書く。
fn guid_of(bytes: &[u8]) -> Option<Guid> {
    if bytes.iter().all(|byte| *byte == 0) {
        return None;
    }
    let mut text = String::with_capacity(32);
    for byte in bytes {
        text.push(char::from_digit(u32::from(byte & 0x0f), 16)?);
        text.push(char::from_digit(u32::from(byte >> 4), 16)?);
    }
    Guid::parse(&text).ok()
}

impl<'a> BinaryFile<'a> {
    /// バイト列を読み取る。
    ///
    /// # Errors
    ///
    /// 見出しの不整合・非対応版・型木なし・境界破損・上限超過の場合に返す。
    pub fn parse(bytes: &'a [u8], options: &BinaryOptions) -> Result<Self, UnityError> {
        if bytes.len() < 20 {
            return Err(UnityError::new(
                UnityErrorKind::UnsupportedFormat,
                "バイナリ保存形式の見出しが短い",
            ));
        }
        let version = be_u32(bytes, 8);
        if !(MIN_VERSION..=MAX_VERSION).contains(&version) {
            return Err(UnityError::new(
                UnityErrorKind::UnsupportedFormat,
                format!("非対応のバイナリ保存形式の版: {version}"),
            ));
        }
        let mut header = Reader::new(bytes, 16, true);
        let big_endian = header.u8()? != 0;
        header.take(3)?;
        let data_offset = if version >= 22 {
            let _metadata_size = header.u32()?;
            let _file_size = header.i64()?;
            let data_offset = header.i64()?;
            let _unknown = header.i64()?;
            usize::try_from(data_offset).map_err(|_| broken("データ位置が負"))?
        } else {
            be_u32(bytes, 12) as usize
        };
        if data_offset > bytes.len() {
            return Err(broken("データ位置がファイル外を指す"));
        }
        let mut reader = Reader::new(bytes, header.at, big_endian);
        let unity_version = reader.cstring()?;
        let _platform = reader.i32()?;
        let type_tree = reader.u8()? != 0;
        if !type_tree {
            return Err(UnityError::new(
                UnityErrorKind::UnsupportedFormat,
                "型木を持たないバイナリ保存形式は読まない",
            ));
        }
        let type_count = reader.count("型", options.max_types)?;
        let mut types = Vec::with_capacity(type_count);
        for _ in 0..type_count {
            types.push(read_type(&mut reader, version, options)?);
        }
        let object_count = reader.count("オブジェクト", options.max_objects)?;
        let mut objects = Vec::with_capacity(object_count);
        for _ in 0..object_count {
            reader.align(0)?;
            let path_id = reader.i64()?;
            let start = if version >= 22 {
                usize::try_from(reader.i64()?).map_err(|_| broken("オブジェクト位置が負"))?
            } else {
                reader.u32()? as usize
            };
            let size = reader.u32()? as usize;
            let type_index = usize::try_from(reader.i32()?)
                .ok()
                .filter(|index| *index < types.len())
                .ok_or_else(|| broken("オブジェクトの型番号が範囲外"))?;
            let start = data_offset
                .checked_add(start)
                .filter(|start| {
                    start
                        .checked_add(size)
                        .is_some_and(|end| end <= bytes.len())
                })
                .ok_or_else(|| broken("オブジェクトの範囲がファイル外を指す"))?;
            objects.push(BinaryObject {
                path_id,
                class_id: types[type_index].class_id,
                stripped: types[type_index].stripped,
                type_index,
                start,
                size,
            });
        }
        let script_count = reader.count("スクリプト型", options.max_objects)?;
        for _ in 0..script_count {
            let _file_index = reader.i32()?;
            reader.align(0)?;
            let _identifier = reader.i64()?;
        }
        let external_count = reader.count("外部参照", options.max_objects)?;
        let mut externals = Vec::with_capacity(external_count);
        for _ in 0..external_count {
            let _empty = reader.cstring()?;
            let guid = guid_of(reader.take(16)?);
            let _kind = reader.i32()?;
            let path = reader.cstring()?;
            externals.push(guid.or_else(|| builtin_guid(&path)));
        }
        Ok(Self {
            bytes,
            version,
            unity_version,
            big_endian,
            types,
            objects,
            externals,
            max_values: options.max_values,
        })
    }

    /// 保存形式の版を返す。
    pub fn version(&self) -> u32 {
        self.version
    }

    /// 書き出したUnityの版を返す。
    pub fn unity_version(&self) -> &str {
        &self.unity_version
    }

    /// オブジェクトの一覧を返す。ファイル内の順序を保つ。
    pub fn objects(&self) -> &[BinaryObject] {
        &self.objects
    }

    /// オブジェクト1件を型木に従って汎用値へ読む。
    ///
    /// # Errors
    ///
    /// 境界破損・未知の基本型・上限超過の場合に返す。
    pub fn read_value(&self, object: &BinaryObject) -> Result<BinaryValue, UnityError> {
        let nodes = &self.types[object.type_index].nodes;
        if nodes.is_empty() {
            return Err(broken("型木が空である").with_class_id(object.class_id));
        }
        let data = &self.bytes[..object.start + object.size];
        let mut walker = Walker {
            reader: Reader::new(data, object.start, self.big_endian),
            base: object.start,
            nodes,
            budget: self.max_values,
        };
        let (value, _) = walker
            .node(0)
            .map_err(|error| error.with_file_id(object.path_id))?;
        Ok(value)
    }

    /// `PPtr` の値（`m_FileID`・`m_PathID`）を参照へ直す。
    ///
    /// `m_FileID` が0なら同一ファイル内、正なら外部参照表の位置（1始まり）である。
    /// 外部参照のGUIDが分からない場合は、同一ファイル内と取り違えないよう
    /// null参照にする。
    pub fn object_ref(&self, value: &BinaryValue) -> ObjectRef {
        let file_index = value
            .field("m_FileID")
            .and_then(BinaryValue::as_i64)
            .unwrap_or(0);
        let path_id = value
            .field("m_PathID")
            .and_then(BinaryValue::as_i64)
            .unwrap_or(0);
        let null = ObjectRef {
            file_id: 0,
            guid: None,
            type_hint: None,
        };
        if file_index == 0 {
            return ObjectRef {
                file_id: path_id,
                guid: None,
                type_hint: None,
            };
        }
        let guid = usize::try_from(file_index - 1)
            .ok()
            .and_then(|at| self.externals.get(at).cloned().flatten());
        match guid {
            Some(guid) => ObjectRef {
                file_id: path_id,
                guid: Some(guid),
                type_hint: None,
            },
            None => null,
        }
    }

    /// オブジェクトの型名（型木の根の型名）を返す。
    pub fn class_name(&self, object: &BinaryObject) -> &str {
        self.types[object.type_index]
            .nodes
            .first()
            .map_or("", |node| node.type_name.as_str())
    }

    /// 汎用値をUnity YAMLと同じ形の汎用値へ写す。
    ///
    /// 構造体はマッピング、配列はシーケンス、`PPtr` は `{fileID, guid}`、
    /// `map` は鍵が文字列なら `- 鍵: 値`、そうでなければ `- first: / second:` の
    /// シーケンスになる。真偽値はUnity YAMLと同じく0/1の整数で書く。
    /// 型なしデータは16進文字列にする。
    pub fn to_yaml(&self, value: &BinaryValue) -> yaml_serde::Value {
        use yaml_serde::{Mapping, Number, Value};
        match value {
            BinaryValue::Bool(flag) => Value::Number(Number::from(i64::from(*flag))),
            BinaryValue::Int(number) => Value::Number(Number::from(*number)),
            BinaryValue::UInt(number) => Value::Number(Number::from(*number)),
            BinaryValue::Float(number) => Value::Number(Number::from(*number)),
            BinaryValue::String(text) => Value::String(text.clone()),
            BinaryValue::Bytes(bytes) => {
                Value::String(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
            }
            BinaryValue::Array(values) => {
                let is_map = !values.is_empty()
                    && values.iter().all(|value| {
                        matches!(value, BinaryValue::Struct { type_name, .. } if type_name == "pair")
                    });
                let items = values
                    .iter()
                    .map(|item| {
                        if is_map
                            && let (Some(first), Some(second)) =
                                (item.field("first"), item.field("second"))
                        {
                            let key = match first {
                                BinaryValue::String(text) => Some(text.clone()),
                                BinaryValue::Struct { type_name, .. }
                                    if type_name == "FastPropertyName" =>
                                {
                                    first
                                        .field("name")
                                        .and_then(BinaryValue::as_str)
                                        .map(str::to_owned)
                                }
                                _ => None,
                            };
                            if let Some(key) = key {
                                let mut entry = Mapping::new();
                                entry.insert(Value::String(key), self.to_yaml(second));
                                return Value::Mapping(entry);
                            }
                        }
                        self.to_yaml(item)
                    })
                    .collect();
                Value::Sequence(items)
            }
            BinaryValue::Struct { type_name, fields } => {
                if type_name.starts_with("PPtr<") {
                    let reference = self.object_ref(value);
                    let mut out = Mapping::new();
                    out.insert(
                        Value::String("fileID".to_owned()),
                        Value::Number(Number::from(reference.file_id)),
                    );
                    if reference.file_id != 0
                        && let Some(guid) = reference.guid
                    {
                        out.insert(
                            Value::String("guid".to_owned()),
                            Value::String(guid.as_str().to_owned()),
                        );
                    }
                    return Value::Mapping(out);
                }
                let mut out = Mapping::new();
                for (name, field) in fields {
                    out.insert(Value::String(name.clone()), self.to_yaml(field));
                }
                Value::Mapping(out)
            }
        }
    }

    /// 最初の材質オブジェクトを [`Material`] として読む。材質が無ければ `None` である。
    ///
    /// # Errors
    ///
    /// 型木の読み取り失敗や、材質の必須欄の形状違いの場合に返す。
    pub fn first_material(&self) -> Result<Option<Material>, UnityError> {
        let Some(object) = self
            .objects
            .iter()
            .find(|object| object.class_id == MATERIAL_CLASS_ID)
        else {
            return Ok(None);
        };
        let value = self.read_value(object)?;
        self.decode_material(&value)
            .map(Some)
            .map_err(|error| error.with_file_id(object.path_id))
    }

    /// 汎用値を [`Material`] へ写す。
    fn decode_material(&self, value: &BinaryValue) -> Result<Material, UnityError> {
        let mismatch = |field: &str| {
            UnityError::new(UnityErrorKind::TypeMismatch, "材質の欄の形状が違う")
                .with_class_id(MATERIAL_CLASS_ID)
                .with_field(field.to_owned())
        };
        let name = value
            .field("m_Name")
            .and_then(BinaryValue::as_str)
            .ok_or_else(|| mismatch("m_Name"))?
            .to_owned();
        let shader = self.object_ref(
            value
                .field("m_Shader")
                .ok_or_else(|| mismatch("m_Shader"))?,
        );
        let custom_render_queue = match value.field("m_CustomRenderQueue") {
            None => -1,
            Some(queue) => queue
                .as_i64()
                .and_then(|queue| i32::try_from(queue).ok())
                .ok_or_else(|| mismatch("m_CustomRenderQueue"))?,
        };
        let saved = value
            .field("m_SavedProperties")
            .ok_or_else(|| mismatch("m_SavedProperties"))?;
        let tex_envs = named_list(saved, "m_TexEnvs", |second| {
            let scale = vector2(second.field("m_Scale")?)?;
            let offset = vector2(second.field("m_Offset")?)?;
            Some(TextureEnvironment {
                texture: self.object_ref(second.field("m_Texture")?),
                scale,
                offset,
            })
        })
        .ok_or_else(|| mismatch("m_TexEnvs"))?;
        let floats = named_list(saved, "m_Floats", BinaryValue::as_f64)
            .ok_or_else(|| mismatch("m_Floats"))?;
        let ints =
            named_list(saved, "m_Ints", BinaryValue::as_i64).ok_or_else(|| mismatch("m_Ints"))?;
        let colors = named_list(saved, "m_Colors", |second| {
            Some([
                second.field("r")?.as_f64()?,
                second.field("g")?.as_f64()?,
                second.field("b")?.as_f64()?,
                second.field("a")?.as_f64()?,
            ])
        })
        .ok_or_else(|| mismatch("m_Colors"))?;
        Ok(Material {
            name,
            shader,
            custom_render_queue,
            tex_envs,
            floats,
            ints,
            colors,
            shader_keywords: shader_keywords(value),
        })
    }
}

/// シェーダキーワードを読む。規則はYAML側の [`Material::shader_keywords`] と同じである。
fn shader_keywords(value: &BinaryValue) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |keyword: &str| {
        if !keyword.is_empty() && !out.iter().any(|known| known == keyword) {
            out.push(keyword.to_owned());
        }
    };
    if let Some(text) = value
        .field("m_ShaderKeywords")
        .and_then(BinaryValue::as_str)
    {
        text.split_whitespace().for_each(&mut push);
    }
    for key in ["m_ValidKeywords", "m_InvalidKeywords"] {
        if let Some(list) = value.field(key).and_then(BinaryValue::as_array) {
            for item in list {
                if let Some(keyword) = item.as_str() {
                    push(keyword.trim());
                }
            }
        }
    }
    out
}

/// `f32` をYAMLの最短表記と同じ `f64` へ広げる。
fn widen(value: f32) -> f64 {
    if value.is_finite() {
        value.to_string().parse().unwrap_or(f64::from(value))
    } else {
        f64::from(value)
    }
}

/// `Vector2f` を読む。
fn vector2(value: &BinaryValue) -> Option<[f64; 2]> {
    Some([value.field("x")?.as_f64()?, value.field("y")?.as_f64()?])
}

/// `map<FastPropertyName, T>` を順序付きの一覧へ読む。欄が無ければ空である。
///
/// 鍵は版により `FastPropertyName { string name }` か `string` で書かれる。
/// 形状が合わない場合は `None` を返す。
fn named_list<T>(
    saved: &BinaryValue,
    key: &str,
    read: impl Fn(&BinaryValue) -> Option<T>,
) -> Option<Vec<NamedProperty<T>>> {
    let Some(map) = saved.field(key) else {
        return Some(Vec::new());
    };
    let mut out = Vec::new();
    for pair in map.as_array()? {
        let first = pair.field("first")?;
        let name = match first {
            BinaryValue::String(name) => name.clone(),
            other => other.field("name")?.as_str()?.to_owned(),
        };
        let value = read(pair.field("second")?)?;
        out.push(NamedProperty { name, value });
    }
    Some(out)
}

/// 型1件を読む。
fn read_type(
    reader: &mut Reader<'_>,
    version: u32,
    options: &BinaryOptions,
) -> Result<SerializedType, UnityError> {
    let class_id = reader.i32()?;
    let stripped = reader.u8()? != 0;
    let _script_type_index = reader.i16()?;
    if class_id == 114 {
        reader.take(16)?;
    }
    reader.take(16)?;
    let node_count = reader.count("型木の節点", options.max_type_nodes)?;
    let string_size = reader.i32()?;
    let string_size =
        usize::try_from(string_size).map_err(|_| broken("型木の文字列表の長さが負"))?;
    let node_bytes = if version >= 19 { 32 } else { 24 };
    let raw_nodes = reader.take(
        node_count
            .checked_mul(node_bytes)
            .ok_or_else(|| broken("型木の節点が多すぎる"))?,
    )?;
    let strings = reader.take(string_size)?;
    let mut nodes = Vec::with_capacity(node_count);
    let mut node_reader = Reader::new(raw_nodes, 0, reader.big_endian);
    for _ in 0..node_count {
        let _version = node_reader.u16()?;
        let level = node_reader.u8()?;
        let flags = node_reader.u8()?;
        let type_offset = node_reader.u32()?;
        let name_offset = node_reader.u32()?;
        let byte_size = node_reader.i32()?;
        let _index = node_reader.i32()?;
        let meta_flag = node_reader.u32()?;
        if version >= 19 {
            let _ref_type_hash = node_reader.u64()?;
        }
        nodes.push(TypeNode {
            type_name: type_string(strings, type_offset)?,
            name: type_string(strings, name_offset)?,
            byte_size,
            level,
            is_array: flags & 1 != 0,
            meta_flag,
        });
    }
    if version >= 21 {
        let dependency_count = reader.count("型の依存", options.max_types)?;
        reader.take(dependency_count.saturating_mul(4))?;
    }
    Ok(SerializedType {
        class_id,
        stripped,
        nodes,
    })
}

/// 型木の文字列を引く。最上位ビットが立っていれば共通文字列表である。
fn type_string(strings: &[u8], offset: u32) -> Result<String, UnityError> {
    if offset & COMMON_STRING_FLAG != 0 {
        return common_string(offset & !COMMON_STRING_FLAG)
            .map(str::to_owned)
            .ok_or_else(|| {
                broken(format!(
                    "共通文字列表に無い位置: {}",
                    offset & !COMMON_STRING_FLAG
                ))
            });
    }
    let start = offset as usize;
    let rest = strings
        .get(start..)
        .ok_or_else(|| broken("型木の文字列位置が範囲外"))?;
    let end = rest
        .iter()
        .position(|byte| *byte == 0)
        .ok_or_else(|| broken("型木の文字列に終端がない"))?;
    Ok(String::from_utf8_lossy(&rest[..end]).into_owned())
}

/// 型木に従う読み手。
struct Walker<'a, 'b> {
    reader: Reader<'a>,
    base: usize,
    nodes: &'b [TypeNode],
    budget: usize,
}

impl Walker<'_, '_> {
    /// 汎用値を1つ数え、上限を超えたら失敗する。
    fn spend(&mut self) -> Result<(), UnityError> {
        self.budget = self.budget.checked_sub(1).ok_or_else(|| {
            UnityError::new(
                UnityErrorKind::LimitExceeded,
                "バイナリ保存形式の値の総数が上限を超えた",
            )
        })?;
        Ok(())
    }

    /// `index` の直下の子の位置を返す。
    fn children(&self, index: usize) -> Vec<usize> {
        let level = self.nodes[index].level;
        let mut out = Vec::new();
        let mut at = index + 1;
        while at < self.nodes.len() && self.nodes[at].level > level {
            if self.nodes[at].level == level + 1 {
                out.push(at);
            }
            at += 1;
        }
        out
    }

    /// `index` の部分木の次の位置を返す。
    fn subtree_end(&self, index: usize) -> usize {
        let level = self.nodes[index].level;
        let mut at = index + 1;
        while at < self.nodes.len() && self.nodes[at].level > level {
            at += 1;
        }
        at
    }

    /// 節点1つを読み、値と部分木の次の位置を返す。
    fn node(&mut self, index: usize) -> Result<(BinaryValue, usize), UnityError> {
        self.spend()?;
        let node = &self.nodes[index];
        let end = self.subtree_end(index);
        let children = self.children(index);
        let mut align = node.meta_flag & ALIGN_FLAG != 0;
        let value = match node.type_name.as_str() {
            "bool" => BinaryValue::Bool(self.reader.u8()? != 0),
            "char" | "SInt8" => BinaryValue::Int(i64::from(self.reader.u8()? as i8)),
            "UInt8" => BinaryValue::UInt(u64::from(self.reader.u8()?)),
            "short" | "SInt16" => BinaryValue::Int(i64::from(self.reader.i16()?)),
            "UInt16" | "unsigned short" => BinaryValue::UInt(u64::from(self.reader.u16()?)),
            "int" | "SInt32" => BinaryValue::Int(i64::from(self.reader.i32()?)),
            "UInt32" | "unsigned int" | "Type*" => BinaryValue::UInt(u64::from(self.reader.u32()?)),
            "long long" | "SInt64" => BinaryValue::Int(self.reader.i64()?),
            "UInt64" | "unsigned long long" | "FileSize" => BinaryValue::UInt(self.reader.u64()?),
            // YAMLの最短表記と同じ値にするため、`f32` の最短10進表記を経由して広げる。
            "float" => BinaryValue::Float(widen(self.reader.f32()?)),
            "double" => BinaryValue::Float(self.reader.f64()?),
            "string" => {
                let length = self.reader.count("文字列", usize::MAX)?;
                let text = String::from_utf8_lossy(self.reader.take(length)?).into_owned();
                if children
                    .first()
                    .is_some_and(|child| self.nodes[*child].meta_flag & ALIGN_FLAG != 0)
                {
                    align = true;
                }
                BinaryValue::String(text)
            }
            "TypelessData" => {
                let length = self.reader.count("型なしデータ", usize::MAX)?;
                BinaryValue::Bytes(self.reader.take(length)?.to_vec())
            }
            _ if children
                .first()
                .is_some_and(|child| self.nodes[*child].is_array) =>
            {
                let array = children[0];
                if self.nodes[array].meta_flag & ALIGN_FLAG != 0 {
                    align = true;
                }
                let value = self.array(array)?;
                if children.len() > 1 {
                    return Err(broken("配列の型木に余分な欄がある"));
                }
                value
            }
            _ if node.is_array => self.array(index)?,
            _ => {
                if children.is_empty() && node.byte_size > 0 {
                    return Err(UnityError::new(
                        UnityErrorKind::UnsupportedFormat,
                        format!("未知の基本型: {}", node.type_name),
                    ));
                }
                let mut fields = Vec::with_capacity(children.len());
                for child in children {
                    let (value, _) = self.node(child)?;
                    fields.push((self.nodes[child].name.clone(), value));
                }
                BinaryValue::Struct {
                    type_name: node.type_name.clone(),
                    fields,
                }
            }
        };
        if align {
            self.reader.align(self.base)?;
        }
        Ok((value, end))
    }

    /// `Array` 節点（子は `size` と `data`）を読む。
    fn array(&mut self, index: usize) -> Result<BinaryValue, UnityError> {
        let children = self.children(index);
        let [_, element] = children[..] else {
            return Err(broken("配列の型木が size と data の2欄でない"));
        };
        let count = self.reader.count("配列", usize::MAX)?;
        let element_size = self.nodes[element].byte_size;
        if element_size > 0
            && count
                .checked_mul(element_size as usize)
                .is_none_or(|bytes| bytes > self.reader.remaining())
        {
            return Err(broken(format!("配列の件数が残りの長さを超える: {count}")));
        }
        if count > self.budget {
            return Err(UnityError::new(
                UnityErrorKind::LimitExceeded,
                "バイナリ保存形式の値の総数が上限を超えた",
            ));
        }
        let mut values = Vec::with_capacity(count.min(self.reader.remaining()));
        for _ in 0..count {
            let (value, _) = self.node(element)?;
            values.push(value);
        }
        Ok(BinaryValue::Array(values))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 共通文字列表の位置はunityの表と一致する() {
        assert_eq!(common_string(0), Some("AABB"));
        assert_eq!(common_string(49), Some("Array"));
        assert_eq!(common_string(427), Some("m_Name"));
        assert_eq!(common_string(688), Some("PPtr<Texture>"));
        assert_eq!(common_string(840), Some("string"));
        assert_eq!(common_string(1093), Some("m_CorrespondingSourceObject"));
        assert_eq!(common_string(1161), Some("Hash128"));
        assert_eq!(common_string(1), None);
    }

    /// 型木の節点（段・型名・欄名・寸法・配列か・metaFlag）。
    type Node = (u8, &'static str, &'static str, i32, bool, u32);

    /// `string` の部分木を `level` に積む。
    fn string_nodes(out: &mut Vec<Node>, level: u8, name: &'static str) {
        out.push((level, "string", name, -1, false, 0x8000));
        out.push((level + 1, "Array", "Array", -1, true, ALIGN_FLAG));
        out.push((level + 2, "int", "size", 4, false, 0));
        out.push((level + 2, "char", "data", 1, false, 0));
    }

    /// `PPtr` の部分木を `level` に積む。
    fn pptr_nodes(out: &mut Vec<Node>, level: u8, type_name: &'static str, name: &'static str) {
        out.push((level, type_name, name, 12, false, 0));
        out.push((level + 1, "int", "m_FileID", 4, false, 0));
        out.push((level + 1, "SInt64", "m_PathID", 8, false, 0));
    }

    /// `map<FastPropertyName, second>` の前半を `level` に積む。
    fn map_head(out: &mut Vec<Node>, level: u8, name: &'static str) {
        out.push((level, "map", name, -1, false, 0));
        out.push((level + 1, "Array", "Array", -1, true, ALIGN_FLAG));
        out.push((level + 2, "int", "size", 4, false, 0));
        out.push((level + 2, "pair", "data", -1, false, 0));
        out.push((level + 3, "FastPropertyName", "first", -1, false, 0));
        string_nodes(out, level + 4, "name");
    }

    /// Unity 5.6相当（版17）の材質の型木を作る。
    fn material_nodes() -> Vec<Node> {
        let mut out = vec![(0, "Material", "Base", -1, false, 0)];
        string_nodes(&mut out, 1, "m_Name");
        pptr_nodes(&mut out, 1, "PPtr<Shader>", "m_Shader");
        out.push((1, "int", "m_CustomRenderQueue", 4, false, 0));
        out.push((1, "UnityPropertySheet", "m_SavedProperties", -1, false, 0));
        map_head(&mut out, 2, "m_TexEnvs");
        out.push((5, "UnityTexEnv", "second", -1, false, 0));
        pptr_nodes(&mut out, 6, "PPtr<Texture>", "m_Texture");
        for name in ["m_Scale", "m_Offset"] {
            out.push((6, "Vector2f", name, 8, false, 0));
            out.push((7, "float", "x", 4, false, 0));
            out.push((7, "float", "y", 4, false, 0));
        }
        map_head(&mut out, 2, "m_Floats");
        out.push((5, "float", "second", 4, false, 0));
        map_head(&mut out, 2, "m_Colors");
        out.push((5, "ColorRGBA", "second", 16, false, 0));
        for name in ["r", "g", "b", "a"] {
            out.push((6, "float", name, 4, false, 0));
        }
        out
    }

    /// 長さ前置きの文字列を4バイト境界まで詰めて書く。
    fn put_string(out: &mut Vec<u8>, text: &str) {
        out.extend_from_slice(&(text.len() as i32).to_le_bytes());
        out.extend_from_slice(text.as_bytes());
        while !out.len().is_multiple_of(4) {
            out.push(0);
        }
    }

    fn put_pptr(out: &mut Vec<u8>, file: i32, path: i64) {
        out.extend_from_slice(&file.to_le_bytes());
        out.extend_from_slice(&path.to_le_bytes());
    }

    fn put_f32(out: &mut Vec<u8>, values: &[f32]) {
        for value in values {
            out.extend_from_slice(&value.to_le_bytes());
        }
    }

    /// 材質1件の本文を作る。
    fn material_body() -> Vec<u8> {
        let mut out = Vec::new();
        put_string(&mut out, "Body");
        put_pptr(&mut out, 1, 4_800_000);
        out.extend_from_slice(&(-1i32).to_le_bytes());
        out.extend_from_slice(&1i32.to_le_bytes());
        put_string(&mut out, "_MainTex");
        put_pptr(&mut out, 1, 2_800_000);
        put_f32(&mut out, &[1.0, 1.0, 0.0, 0.0]);
        out.extend_from_slice(&1i32.to_le_bytes());
        put_string(&mut out, "_Shadow");
        put_f32(&mut out, &[0.4]);
        out.extend_from_slice(&1i32.to_le_bytes());
        put_string(&mut out, "_Color");
        put_f32(&mut out, &[1.0, 0.5, 0.25, 1.0]);
        out
    }

    /// 型木付き版17の保存ファイルを組み立てる。外部参照は `externals` の順である。
    /// `m_Name` だけは共通文字列表を指し、残りはファイル内の文字列表に置く。
    fn binary_file(nodes: &[Node], body: &[u8], externals: &[[u8; 16]]) -> Vec<u8> {
        let mut strings: Vec<u8> = Vec::new();
        let mut offset_of = |text: &str| -> u32 {
            if text == "m_Name" {
                return COMMON_STRING_FLAG | 427;
            }
            let at = strings.len() as u32;
            strings.extend_from_slice(text.as_bytes());
            strings.push(0);
            at
        };
        let mut raw_nodes = Vec::new();
        for (index, (level, type_name, name, size, is_array, meta)) in nodes.iter().enumerate() {
            let type_offset = offset_of(type_name);
            let name_offset = offset_of(name);
            raw_nodes.extend_from_slice(&1u16.to_le_bytes());
            raw_nodes.push(*level);
            raw_nodes.push(u8::from(*is_array));
            raw_nodes.extend_from_slice(&type_offset.to_le_bytes());
            raw_nodes.extend_from_slice(&name_offset.to_le_bytes());
            raw_nodes.extend_from_slice(&size.to_le_bytes());
            raw_nodes.extend_from_slice(&(index as i32).to_le_bytes());
            raw_nodes.extend_from_slice(&meta.to_le_bytes());
        }
        let mut meta = Vec::new();
        meta.extend_from_slice(b"5.6.3p1\0");
        meta.extend_from_slice(&(-2i32).to_le_bytes());
        meta.push(1);
        meta.extend_from_slice(&1i32.to_le_bytes());
        meta.extend_from_slice(&MATERIAL_CLASS_ID.to_le_bytes());
        meta.push(0);
        meta.extend_from_slice(&(-1i16).to_le_bytes());
        meta.extend_from_slice(&[0u8; 16]);
        meta.extend_from_slice(&(nodes.len() as i32).to_le_bytes());
        meta.extend_from_slice(&(strings.len() as i32).to_le_bytes());
        meta.extend_from_slice(&raw_nodes);
        meta.extend_from_slice(&strings);
        meta.extend_from_slice(&1i32.to_le_bytes());
        while !(20 + meta.len()).is_multiple_of(4) {
            meta.push(0);
        }
        meta.extend_from_slice(&2_100_000i64.to_le_bytes());
        meta.extend_from_slice(&0u32.to_le_bytes());
        meta.extend_from_slice(&(body.len() as u32).to_le_bytes());
        meta.extend_from_slice(&0i32.to_le_bytes());
        meta.extend_from_slice(&0i32.to_le_bytes());
        meta.extend_from_slice(&(externals.len() as i32).to_le_bytes());
        for guid in externals {
            meta.push(0);
            meta.extend_from_slice(guid);
            meta.extend_from_slice(&0i32.to_le_bytes());
            meta.push(0);
        }
        meta.push(0);
        let mut data_offset = 20 + meta.len();
        data_offset = data_offset.div_ceil(16) * 16;
        let file_size = data_offset + body.len();
        let mut out = Vec::with_capacity(file_size);
        out.extend_from_slice(&(meta.len() as u32).to_be_bytes());
        out.extend_from_slice(&(file_size as u32).to_be_bytes());
        out.extend_from_slice(&17u32.to_be_bytes());
        out.extend_from_slice(&(data_offset as u32).to_be_bytes());
        out.extend_from_slice(&[0, 0, 0, 0]);
        out.extend_from_slice(&meta);
        out.resize(data_offset, 0);
        out.extend_from_slice(body);
        out
    }

    /// 文字列形式のGUIDをバイナリ表現（各バイトの下位上位の順）へ直す。
    fn guid_bytes(text: &str) -> [u8; 16] {
        let digits: Vec<u8> = text
            .chars()
            .map(|c| c.to_digit(16).expect("16進") as u8)
            .collect();
        let mut out = [0u8; 16];
        for (at, pair) in digits.chunks(2).enumerate() {
            out[at] = pair[0] | (pair[1] << 4);
        }
        out
    }

    const TEXTURE_GUID: &str = "f234daff8eb77e4478c48a2dc0071fb4";

    #[test]
    fn 型木付きの材質を読み参照を外部参照表で解決する() {
        // 外部参照は1始まりで、m_FileID 1 は表の先頭を指す。
        let bytes = binary_file(
            &material_nodes(),
            &material_body(),
            &[guid_bytes(TEXTURE_GUID)],
        );
        assert!(is_binary_serialized(&bytes));
        let file = BinaryFile::parse(&bytes, &BinaryOptions::new()).expect("読める");
        assert_eq!(file.version(), 17);
        assert_eq!(file.unity_version(), "5.6.3p1");
        assert_eq!(file.objects().len(), 1);
        assert_eq!(file.objects()[0].class_id, MATERIAL_CLASS_ID);
        assert_eq!(file.objects()[0].path_id, 2_100_000);
        let material = file
            .first_material()
            .expect("材質を読める")
            .expect("材質がある");
        assert_eq!(material.name, "Body");
        assert_eq!(material.shader.file_id, 4_800_000);
        assert_eq!(material.custom_render_queue, -1);
        assert_eq!(material.tex_envs.len(), 1);
        assert_eq!(material.tex_envs[0].name, "_MainTex");
        let texture = &material.tex_envs[0].value.texture;
        assert_eq!(texture.file_id, 2_800_000);
        assert_eq!(
            texture.guid.as_ref().expect("GUIDがある").as_str(),
            TEXTURE_GUID
        );
        assert_eq!(material.tex_envs[0].value.scale, [1.0, 1.0]);
        // `f32` の0.4はYAMLの最短表記と同じ0.4に広げる。
        assert_eq!(material.floats[0].name, "_Shadow");
        assert_eq!(material.floats[0].value, 0.4);
        assert_eq!(material.colors[0].name, "_Color");
        assert_eq!(material.colors[0].value, [1.0, 0.5, 0.25, 1.0]);
        assert!(material.ints.is_empty());
    }

    #[test]
    fn 解決できない外部参照は同一ファイル内と取り違えずnullにする() {
        let bytes = binary_file(&material_nodes(), &material_body(), &[]);
        let file = BinaryFile::parse(&bytes, &BinaryOptions::new()).expect("読める");
        let material = file.first_material().expect("読める").expect("ある");
        assert!(material.shader.is_null());
        assert!(material.tex_envs[0].value.texture.is_null());
    }

    #[test]
    fn バイナリはyamlと同じ形の文書として読める() {
        let bytes = binary_file(
            &material_nodes(),
            &material_body(),
            &[guid_bytes(TEXTURE_GUID)],
        );
        let file = crate::unity::SerializedFile::parse(&bytes, &crate::unity::ParseOptions::new())
            .expect("バイナリも読める");
        let document = &file.documents()[0];
        assert_eq!(document.file_id(), 2_100_000);
        assert_eq!(document.class_id(), MATERIAL_CLASS_ID);
        assert_eq!(document.class_name(), "Material");
        assert!(!document.stripped());
        // `map` は `- 鍵: 値` のシーケンス、`PPtr` は `{fileID, guid}` になる。
        let main_tex = document
            .raw()
            .get("m_SavedProperties")
            .and_then(|saved| saved.get("m_TexEnvs"))
            .and_then(|envs| envs.get(0))
            .and_then(|entry| entry.get("_MainTex"))
            .and_then(|env| env.get("m_Texture"))
            .expect("テクスチャ参照がある");
        assert_eq!(
            main_tex.get("fileID").and_then(|value| value.as_i64()),
            Some(2_800_000)
        );
        assert_eq!(
            main_tex.get("guid").and_then(|value| value.as_str()),
            Some(TEXTURE_GUID)
        );
        let crate::unity::TypedObject::Material(material) =
            document.decode().expect("代表型に読める")
        else {
            panic!("材質でない");
        };
        assert_eq!(material.name, "Body");
        assert_eq!(material.floats[0].value, 0.4);
    }

    #[test]
    fn 本文の途中切れと非対応版と型木なしを区別する() {
        let bytes = binary_file(&material_nodes(), &material_body(), &[]);
        // 本文を縮めるとオブジェクト範囲がファイル外を指す。
        let truncated = &bytes[..bytes.len() - 4];
        let error = BinaryFile::parse(truncated, &BinaryOptions::new()).expect_err("切れている");
        assert_eq!(error.kind(), UnityErrorKind::InvalidBinary);

        let mut old = bytes.clone();
        old[8..12].copy_from_slice(&16u32.to_be_bytes());
        let error = BinaryFile::parse(&old, &BinaryOptions::new()).expect_err("非対応版");
        assert_eq!(error.kind(), UnityErrorKind::UnsupportedFormat);

        let mut stripped = bytes.clone();
        // 版文字列(8)と対象環境(4)の直後が型木の有無である。
        stripped[20 + 12] = 0;
        let error = BinaryFile::parse(&stripped, &BinaryOptions::new()).expect_err("型木なし");
        assert_eq!(error.kind(), UnityErrorKind::UnsupportedFormat);

        assert!(!is_binary_serialized(
            b"%YAML 1.1\n%TAG !u! tag:unity3d.com,2011:\n"
        ));
    }

    #[test]
    fn 値の総数の上限を超えると失敗する() {
        let bytes = binary_file(&material_nodes(), &material_body(), &[]);
        let options = BinaryOptions {
            max_values: 10,
            ..BinaryOptions::new()
        };
        let file = BinaryFile::parse(&bytes, &options).expect("見出しは読める");
        let error = file.first_material().expect_err("値が多すぎる");
        assert_eq!(error.kind(), UnityErrorKind::LimitExceeded);
    }

    #[test]
    fn 本文の配列長が残りを超えると失敗する() {
        let mut body = material_body();
        // m_TexEnvs の件数（名前・シェーダ・描画順の直後）を壊す。
        let at = 8 + 12 + 4;
        body[at..at + 4].copy_from_slice(&1_000_000i32.to_le_bytes());
        let bytes = binary_file(&material_nodes(), &body, &[]);
        let file = BinaryFile::parse(&bytes, &BinaryOptions::new()).expect("見出しは読める");
        let error = file.first_material().expect_err("件数が壊れている");
        assert_eq!(error.kind(), UnityErrorKind::InvalidBinary);
    }

    #[test]
    fn guidは各バイトの下位上位の順に書く() {
        let bytes = [
            0x2f, 0x43, 0xad, 0xff, 0xe8, 0x7b, 0xe7, 0x44, 0x87, 0x4c, 0xa8, 0xd2, 0x0c, 0x70,
            0xf1, 0x4b,
        ];
        assert_eq!(
            guid_of(&bytes).expect("GUIDになる").as_str(),
            "f234daff8eb77e4478c48a2dc0071fb4"
        );
        assert!(guid_of(&[0; 16]).is_none());
    }
}
