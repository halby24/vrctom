//! GUID・fileID・オブジェクト参照。
//!
//! 公開契約の入口は [`crate::unity`] を参照。
//!
//! Unity YAML内の `{fileID, guid, type}` 形を、数値化で精度や桁数を失わずに読む。
//! GUIDは常に32桁hex文字列として検証し、小文字に統一して保持する。

use yaml_serde::Value;

use super::{UnityError, UnityErrorKind};

/// UnityのfileID。
///
/// ファイル内オブジェクトは非ゼロ値、null参照は `0` で表す。
/// YAMLの浮動小数点経由では読まず、整数スカラーのみ受け付ける。
pub type FileId = i64;

/// GUID文字列の長さ（32桁hex）。
const GUID_LEN: usize = 32;

/// Unityアセットを一意に指すGUID。
///
/// 32桁hexとして検証済みで、大文字小文字の違いは小文字に統一する。
/// 全ゼロも有効値として保持する（欠落とは区別する）。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Guid(Box<str>);

impl Guid {
    /// 文字列をGUIDとして検証し、小文字に統一して保持する。
    ///
    /// # Errors
    ///
    /// 32桁hexでない場合に [`UnityErrorKind::InvalidMetadata`] を返す。
    pub fn parse(text: &str) -> Result<Self, UnityError> {
        if text.len() == GUID_LEN && text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            Ok(Self(text.to_ascii_lowercase().into_boxed_str()))
        } else {
            Err(UnityError::new(
                UnityErrorKind::InvalidMetadata,
                format!("GUIDが32桁hexでない: {text:?}"),
            ))
        }
    }

    /// 小文字に統一した32桁hexを返す。
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Guid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// YAML値からGUIDを読む。
///
/// YAMLで文字列として解決されたスカラーだけを32hexとして検証する。
/// 数値スカラーは桁落ちや先行ゼロ・符号の欠落で復元できないため、黙って壊さず
/// [`UnityErrorKind::InvalidMetadata`] で拒否する。
/// 引用符なしの全数字32桁や `1e` 形など、YAMLの数値と解釈され得る字面は
/// 数値解決されて拒否する。GUIDは引用符付きの32桁hexで書く必要がある。
pub(crate) fn read_guid(value: &Value) -> Result<Guid, UnityError> {
    read_guid_impl(value, UnityErrorKind::InvalidMetadata)
}

/// 本文内のYAML値からGUIDを読む。
///
/// 検証の条件は [`read_guid`] と同じだが、不良の種別は
/// [`UnityErrorKind::InvalidYaml`] になる。本文の字面不良はYAMLの枠の問題であり、
/// `.meta` の内容不良とは区別するためである。
pub(crate) fn read_body_guid(value: &Value) -> Result<Guid, UnityError> {
    read_guid_impl(value, UnityErrorKind::InvalidYaml)
}

/// GUID検証の本体。種別だけを呼び出し側で選ぶ。
fn read_guid_impl(value: &Value, kind: UnityErrorKind) -> Result<Guid, UnityError> {
    match value {
        Value::String(text) => Guid::parse(text)
            .map_err(|_| UnityError::new(kind, format!("GUIDが32桁hexでない: {text:?}"))),
        // 数値解決された値は文字列化して受理しない。符号落ちや桁落ちを防ぐ。
        Value::Number(_) => Err(UnityError::new(kind, "GUIDが数値化されていて復元できない")),
        Value::Tagged(_) if super::yaml::is_bignum(value) => {
            Err(UnityError::new(kind, "GUIDが数値化されていて復元できない"))
        }
        _ => Err(UnityError::new(kind, "GUIDが文字列でない")),
    }
}

/// Unity YAML内のオブジェクト参照 `{fileID, guid, type}`。
///
/// `file_id` が `0` の場合はnull参照を表し、`guid` の有無に優先する。
/// `guid` がない参照は同一ファイル内、`guid` がある参照はGUIDで指す。
/// `type` は解決に使わない目安（ヒント）であり、欠けていてもよい。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectRef {
    /// 参照先のfileID。`0` はnull参照。
    pub file_id: FileId,
    /// 参照先アセットのGUID。ない場合は同一ファイル内を指す。
    pub guid: Option<Guid>,
    /// Unityの型番号の目安。解決には使わない。
    pub type_hint: Option<i32>,
}

impl ObjectRef {
    /// null参照（`fileID: 0`）かどうかを返す。
    pub fn is_null(&self) -> bool {
        self.file_id == 0
    }

    /// YAML値から参照を読み取る。
    ///
    /// `{"fileID": 0}` はnull参照になり、`guid` の有無に優先する。
    /// 本文内の参照は `ObjectRef::decode_body` を使い、GUID不良の種別を
    /// [`UnityErrorKind::InvalidYaml`] に寄せる。
    ///
    /// # Errors
    ///
    /// 参照の形状が壊れている場合に [`UnityErrorKind::InvalidReference`]、
    /// GUIDの値が不良の場合に [`UnityErrorKind::InvalidMetadata`] を返す。
    pub fn decode(value: &Value) -> Result<Self, UnityError> {
        Self::decode_impl(value, false)
    }

    /// 本文内のYAML値から参照を読み取る。
    ///
    /// 検証の条件は [`ObjectRef::decode`] と同じだが、GUID不良の種別だけが
    /// [`UnityErrorKind::InvalidYaml`] になる。
    ///
    /// # Errors
    ///
    /// 参照の形状が壊れている場合に [`UnityErrorKind::InvalidReference`]、
    /// GUIDの値が不良の場合に [`UnityErrorKind::InvalidYaml`] を返す。
    pub(crate) fn decode_body(value: &Value) -> Result<Self, UnityError> {
        Self::decode_impl(value, true)
    }

    /// 参照読み取りの本体。本文内かどうかでGUID不良の種別だけを変える。
    fn decode_impl(value: &Value, in_body: bool) -> Result<Self, UnityError> {
        let mapping = value.as_mapping().ok_or_else(|| {
            UnityError::new(
                UnityErrorKind::InvalidReference,
                "オブジェクト参照がマッピングでない",
            )
        })?;
        let file_value = mapping.get("fileID").ok_or_else(|| {
            UnityError::new(
                UnityErrorKind::InvalidReference,
                "オブジェクト参照にfileIDがない",
            )
        })?;
        // 整数スカラーのみ受け付け、浮動小数点経由では読まない。
        let file_id = match file_value {
            Value::Number(number) => number.as_i64().ok_or_else(|| {
                UnityError::new(
                    UnityErrorKind::InvalidReference,
                    "オブジェクト参照のfileIDがi64の範囲外",
                )
            })?,
            _ => {
                return Err(UnityError::new(
                    UnityErrorKind::InvalidReference,
                    "オブジェクト参照のfileIDが整数でない",
                ));
            }
        };
        // `type` はヒントであり、整数でなければ無視ではなく拒否する。
        // 欠けた場合だけ `None` とする。
        let type_hint = match mapping.get("type") {
            None | Some(Value::Null) => None,
            Some(value) => Some(
                value
                    .as_i64()
                    .and_then(|n| i32::try_from(n).ok())
                    .ok_or_else(|| {
                        UnityError::new(
                            UnityErrorKind::InvalidReference,
                            "オブジェクト参照のtypeが整数でない",
                        )
                    })?,
            ),
        };
        let guid = match mapping.get("guid") {
            None | Some(Value::Null) => None,
            Some(value) => Some(if in_body {
                read_body_guid(value)?
            } else {
                read_guid(value)?
            }),
        };
        Ok(Self {
            file_id,
            guid,
            type_hint,
        })
    }
}
