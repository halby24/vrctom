//! GUID・パス・種別・metaの索引。
//!
//! 公開契約の入口は [`crate::unity`] を参照。
//!
//! 本体の一括読み込み・自動キャッシュ・再帰ロードはしない。
//! 参照解決は呼び出し側が本文を読む段階で行い、この層は所在だけを答える。

use std::collections::HashMap;

use super::{AssetKind, AssetMeta, FileId, Guid, ObjectRef, UnityError, UnityErrorKind, classify};

/// meta 1件の既定上限（バイト）。
const DEFAULT_MAX_META_BYTES: u64 = 4 * 1024 * 1024;
/// meta合計の既定上限（バイト）。
const DEFAULT_MAX_META_TOTAL_BYTES: u64 = 64 * 1024 * 1024;
/// アセット数の既定上限。
const DEFAULT_MAX_ASSETS: usize = 100_000;

/// 索引構築の資源上限。
///
/// フィールドは非公開で、既定値は [`Default`] が設定する。
/// 上限の変更は `with_` で始まるメソッドで行い、CLIは既定値をそのまま使う。
#[derive(Debug, Clone)]
pub struct IndexOptions {
    pub(crate) max_meta_bytes: u64,
    pub(crate) max_meta_total_bytes: u64,
    pub(crate) max_assets: usize,
}

impl IndexOptions {
    /// 既定の上限を持つ設定を生成する。
    pub fn new() -> Self {
        Self {
            max_meta_bytes: DEFAULT_MAX_META_BYTES,
            max_meta_total_bytes: DEFAULT_MAX_META_TOTAL_BYTES,
            max_assets: DEFAULT_MAX_ASSETS,
        }
    }

    /// meta 1件の上限（バイト）を変更する。
    pub fn with_max_meta_bytes(mut self, max_meta_bytes: u64) -> Self {
        self.max_meta_bytes = max_meta_bytes;
        self
    }

    /// meta合計の上限（バイト）を変更する。
    pub fn with_max_meta_total_bytes(mut self, max_meta_total_bytes: u64) -> Self {
        self.max_meta_total_bytes = max_meta_total_bytes;
        self
    }

    /// アセット数の上限を変更する。
    pub fn with_max_assets(mut self, max_assets: usize) -> Self {
        self.max_assets = max_assets;
        self
    }
}

impl Default for IndexOptions {
    fn default() -> Self {
        Self::new()
    }
}

/// 索引の1件。
#[derive(Debug, Clone)]
pub struct AssetRecord {
    /// アセットのGUID。
    pub guid: Guid,
    /// 展開先からの相対パス。区切りは `/`。
    pub path: String,
    /// 拡張子とimporterから分類した種別。
    pub kind: AssetKind,
    /// metaの読み取り結果。
    pub meta: AssetMeta,
}

/// GUID・パス・種別・metaの索引。
///
/// 正は `.meta` であり、本体の読み込みは行わない。
/// 重複（GUID・パス・大小文字のみ異なるパス）は後勝ちにせず拒否する。
#[derive(Debug, Default, Clone)]
pub struct AssetIndex {
    records: Vec<AssetRecord>,
    by_guid: HashMap<Guid, usize>,
    by_path: HashMap<String, usize>,
    by_path_lower: HashMap<String, usize>,
}

impl AssetIndex {
    /// 空の索引を生成する。
    pub fn new() -> Self {
        Self::default()
    }

    /// metaを索引に加える。
    ///
    /// `relative_path` は展開先からの相対パス（区切りは `/`）で、
    /// 対応する `.meta` の `.meta` 接尾辞を除いた形である。
    /// 種別は `relative_path` の拡張子とmetaのimporterから分類する。
    ///
    /// # Errors
    ///
    /// パス契約違反・GUID重複・パス重複（大小文字のみの違いを含む）の場合に
    /// エラーを返す。後勝ちにはしない。
    pub fn insert(&mut self, relative_path: &str, meta: AssetMeta) -> Result<(), UnityError> {
        // `workspace` の検証を内部で再利用し、Unity用に包み直す。公開はしない。
        if let Err(workspace) = crate::workspace::validate_path(relative_path, false) {
            return Err(UnityError::new(
                UnityErrorKind::InvalidPath,
                format!("索引パスが契約に違反する: {workspace}"),
            )
            .with_path(relative_path.to_owned()));
        }
        if self.by_guid.contains_key(meta.guid()) {
            return Err(UnityError::new(UnityErrorKind::Conflict, "GUIDが重複する")
                .with_path(relative_path.to_owned()));
        }
        if self.by_path.contains_key(relative_path) {
            return Err(UnityError::new(UnityErrorKind::Conflict, "パスが重複する")
                .with_path(relative_path.to_owned()));
        }
        // 大小文字の違いはUnicodeの小文字化で判定する。ASCII限定では
        // `Ä` と `ä` のような衝突を見逃す。過剰に衝突させる側に倒し、
        // 安全側（拒否）に寄せる。
        let lower = relative_path.to_lowercase();
        if self.by_path_lower.contains_key(&lower) {
            return Err(UnityError::new(
                UnityErrorKind::Conflict,
                "大小文字のみ異なるパスが重複する",
            )
            .with_path(relative_path.to_owned()));
        }
        let kind = classify(relative_path, &meta);
        let index = self.records.len();
        self.by_guid.insert(meta.guid().clone(), index);
        self.by_path.insert(relative_path.to_owned(), index);
        self.by_path_lower.insert(lower, index);
        self.records.push(AssetRecord {
            guid: meta.guid().clone(),
            path: relative_path.to_owned(),
            kind,
            meta,
        });
        Ok(())
    }

    /// GUIDで件を探す。ない場合は `None` を返す。
    pub fn get(&self, guid: &Guid) -> Option<&AssetRecord> {
        self.by_guid.get(guid).map(|index| &self.records[*index])
    }

    /// 相対パスで件を探す。完全一致のみ受け付ける。
    pub fn get_by_path(&self, relative_path: &str) -> Option<&AssetRecord> {
        self.by_path
            .get(relative_path)
            .map(|index| &self.records[*index])
    }

    /// 件の一覧を返す。順序は挿入順である。
    pub fn entries(&self) -> &[AssetRecord] {
        &self.records
    }

    /// 参照の所在を特定する。
    ///
    /// `source` は参照元文書を含むアセットのGUIDである。
    /// `fileID == 0` はnullとして優先し、GUIDがなくても解決済みとはしない。
    /// GUIDの省略は同一ファイル内を指し、索引にある参照元自身として返す。
    /// 索引にないGUIDは欠落と組み込み資産を区別せず外部として返す。
    /// `type` は解決に使わない。
    ///
    /// # Errors
    ///
    /// 参照元自身が索引にない場合に [`UnityErrorKind::MissingObject`] を返す。
    pub fn locate(
        &self,
        source: &Guid,
        reference: &ObjectRef,
    ) -> Result<ReferenceLocation<'_>, UnityError> {
        if reference.is_null() {
            return Ok(ReferenceLocation::Null);
        }
        match &reference.guid {
            None => {
                let asset = self.get(source).ok_or_else(|| {
                    UnityError::new(UnityErrorKind::MissingObject, "参照元アセットが索引にない")
                })?;
                Ok(ReferenceLocation::Indexed {
                    asset,
                    file_id: reference.file_id,
                })
            }
            Some(guid) => match self.get(guid) {
                Some(asset) => Ok(ReferenceLocation::Indexed {
                    asset,
                    file_id: reference.file_id,
                }),
                None => Ok(ReferenceLocation::External {
                    guid: guid.clone(),
                    file_id: reference.file_id,
                }),
            },
        }
    }
}

/// 参照の所在。
#[derive(Debug, Clone)]
pub enum ReferenceLocation<'a> {
    /// null参照（`fileID: 0`）。
    Null,
    /// 索引にあるアセット内のオブジェクト。
    Indexed {
        /// 所在アセットの索引件。
        asset: &'a AssetRecord,
        /// 所在アセット内のfileID。
        file_id: FileId,
    },
    /// 索引にないGUIDへの参照。欠落と組み込み資産を区別しない。
    External {
        /// 参照先のGUID。
        guid: Guid,
        /// 参照先のfileID。
        file_id: FileId,
    },
}
