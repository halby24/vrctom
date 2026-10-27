//! FBX読み取りの失敗エラー。
//!
//! 公開契約の入口は [`crate::fbx`] を参照。
//!
//! 原因の診断情報は保持するが、メッセージ文言や `source` の具体型は契約にしない。
//! 外部由来の文字列を含む診断は表示時に可視エスケープする。逃がし済みの文面への
//! 再適用は変えない。

/// FBX読み取りの失敗エラー。
///
/// 対象オブジェクトが判明している範囲でIDを保持し、表示だけが逃がす。
/// 詳細部はヒープに置き、`Result` の `Err` 側を小さく保つ。
#[derive(Debug)]
pub struct FbxError {
    kind: FbxErrorKind,
    message: Box<str>,
    object_id: Option<i64>,
    source: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl FbxError {
    pub(crate) fn new(kind: FbxErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into().into_boxed_str(),
            object_id: None,
            source: None,
        }
    }

    pub(crate) fn with_object(mut self, id: super::ObjectId) -> Self {
        self.object_id = Some(id.value());
        self
    }

    pub(crate) fn with_source(
        mut self,
        source: impl Into<Box<dyn std::error::Error + Send + Sync>>,
    ) -> Self {
        self.source = Some(source.into());
        self
    }

    /// エラーの種別を返す。
    pub fn kind(&self) -> FbxErrorKind {
        self.kind
    }

    /// 判明している範囲で、対象のオブジェクトIDを返す。
    pub fn object_id(&self) -> Option<super::ObjectId> {
        self.object_id.map(super::ObjectId)
    }
}

impl std::fmt::Display for FbxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 文面は生成時に逃がし済みだが、表示時にも逃がして未逃がしの混入を防ぐ。
        // 逃がし済みの文面への再適用は変えない。
        write!(f, "{:?}: {}", self.kind, crate::escape_diag(&self.message))?;
        if let Some(id) = self.object_id {
            write!(f, " [object: {id}]")?;
        }
        Ok(())
    }
}

impl std::error::Error for FbxError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_ref()
            .map(|source| source.as_ref() as &(dyn std::error::Error + 'static))
    }
}

/// FBX読み取りの失敗エラーの種別。
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FbxErrorKind {
    /// 先頭のマジックや版の手前で形式が合わない場合。
    InvalidHeader,
    /// 7.xの32bitレコードで扱える版（7000以上7500未満）でない場合。
    UnsupportedVersion,
    /// 暗号化・未知の型やencoding・非対応の対応方式・変換成分など、
    /// 黙って補正できない非対応要素がある場合。
    UnsupportedFeature,
    /// オフセット・長さ・索引・値の破損など、構造として成立しない場合。
    InvalidStructure,
    /// 資源上限を超える場合。
    LimitExceeded,
    /// 存在しないIDを要求された場合。
    NotFound,
}
