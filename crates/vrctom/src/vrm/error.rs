//! VRM中間モデル生成の失敗エラー。
//!
//! 公開契約の入口は [`crate::vrm`] を参照。
//!
//! 原因の診断情報は保持するが、メッセージ文面や `source` の具体型は契約にしない。
//! 外部由来の文字列を含む診断は表示時に可視エスケープする。逃がし済みの文面への
//! 再適用は変えない。

/// VRM中間モデル生成の失敗エラー。
///
/// 対象オブジェクトが判明している範囲でIDを保持し、表示だけが逃がす。
/// 詳細部はヒープに置き、`Result` の `Err` 側を小さく保つ。
#[derive(Debug)]
pub struct VrmError {
    kind: VrmErrorKind,
    message: Box<str>,
    object_id: Option<i64>,
}

impl VrmError {
    pub(crate) fn new(kind: VrmErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into().into_boxed_str(),
            object_id: None,
        }
    }

    pub(crate) fn with_object(mut self, id: crate::fbx::ObjectId) -> Self {
        self.object_id = Some(id.value());
        self
    }

    /// エラーの種別を返す。
    pub fn kind(&self) -> VrmErrorKind {
        self.kind
    }

    /// 判明している範囲で、対象のオブジェクトIDを返す。
    pub fn object_id(&self) -> Option<crate::fbx::ObjectId> {
        self.object_id.map(crate::fbx::ObjectId::new)
    }
}

impl std::fmt::Display for VrmError {
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

impl std::error::Error for VrmError {}

/// VRM中間モデル生成の失敗エラーの種別。
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VrmErrorKind {
    /// 軸系がY-up・+Z正面・+X右手でない場合。
    UnsupportedAxis,
    /// 単位換算係数が0以下または非有限の場合。
    UnsupportedUnitScale,
    /// バインド姿勢の行列が無いモデルがある場合。
    MissingBindPose,
    /// 1つのメッシュに複数のスキンが対応する場合。
    MultipleSkins,
    /// メッシュを持つモデルに子モデルがある場合。
    MeshModelHasChildren,
    /// 座標・索引・材質番号などの破損や、行列の特異など構造として成立しない場合。
    InvalidStructure,
    /// メッシュ・頂点・三角形の件数が上限を超える場合。
    LimitExceeded,
}
