//! Unityの保存データを参照関係を壊さず読み取る層。
//!
//! 担当するのは、展開済み配置の索引付けと、Unity YAML文書の汎用・型付き読み取り、
//! 単一ファイル内の階層の組み立てまでである。VRM生成・prefab合成・FBX解析・
//! 画像デコード・スクリプト実行は行わない。
//!
//! 入口は [`crate::unity::AssetIndex`]（索引）、[`crate::unity::SerializedFile`]（文書列）、
//! [`crate::unity::AssetMeta`]（meta読み取り）である。本文の読み込みや再帰ロードはしない。
//!
//! ```mermaid
//! flowchart LR
//!     meta[AssetMeta::parse<br/>.meta] --> index[AssetIndex<br/>索引]
//!     dir[scan_directory<br/>Native] --> index
//!     yaml[SerializedFile::parse<br/>Unity YAML] --> doc[Document<br/>汎用]
//!     doc --> typed[decode<br/>代表型]
//!     yaml --> hier[local_hierarchy<br/>単一内階層]
//!     index --> loc[locate<br/>所在特定]
//! ```
//!
//! 型・索引・YAML・階層はNative/Web共通で、同期の `&[u8]` 入力APIとする。
//! ディレクトリ走査だけがNative限定である。

mod error;
mod file_id;
mod hierarchy;
mod index;
mod meta;
mod objects;
mod reference;
mod source;
mod yaml;

#[cfg(not(target_arch = "wasm32"))]
mod native;

pub use error::{UnityError, UnityErrorKind};
pub use file_id::{game_object_file_id, mesh_file_id, skinned_mesh_renderer_file_id};
pub use hierarchy::{HierarchyOptions, LocalHierarchy, LocalNode};
pub use index::{AssetIndex, AssetRecord, IndexOptions, ReferenceLocation};
pub use meta::{
    AssetKind, AssetMeta, ExternalObjectEntry, HumanBoneMapping, Importer, MAX_META_BYTES,
};
// 内部分割の詳細は公開しない。使う型だけをこの層で再公開する。
pub(crate) use meta::classify;
pub use objects::{
    GameObject, Material, MonoBehaviour, NamedProperty, PrefabInstance, PrefabLink,
    PrefabModification, SkinnedMeshRenderer, TextureEnvironment, Transform, TypedObject,
};
pub(crate) use reference::read_guid;
pub use reference::{FileId, Guid, ObjectRef};
pub use source::AssetSource;
pub(crate) use yaml::bignum_digits;
pub use yaml::{Document, ParseOptions, SerializedFile};

#[cfg(not(target_arch = "wasm32"))]
pub(crate) use native::FsAssetSource;
#[cfg(not(target_arch = "wasm32"))]
pub use native::scan_directory;

impl SerializedFile {
    /// 単一ファイル内の階層を組み立てる。
    ///
    /// `stripped` や `PrefabInstance` を含む場合は、部分木を完全な階層として
    /// 返さず [`UnityErrorKind::UnsupportedPrefabComposition`] で失敗する。
    /// 汎用読み取りや代表型の取得は可能なままである。
    /// 深さの上限は既定値を使い、変える場合は
    /// [`local_hierarchy_with_options`](Self::local_hierarchy_with_options) を使う。
    ///
    /// # Errors
    ///
    /// 非対応構成・親子不整合・循環・参照欠落・深さ上限超過などの場合にエラーを返す。
    pub fn local_hierarchy(&self) -> Result<LocalHierarchy, UnityError> {
        hierarchy::build_hierarchy(self, &HierarchyOptions::default())
    }

    /// 深さの上限を指定して、単一ファイル内の階層を組み立てる。
    ///
    /// 判定の条件は [`local_hierarchy`](Self::local_hierarchy) と同じであり、
    /// 深さの上限だけが変わる。超過は [`UnityErrorKind::LimitExceeded`] になる。
    ///
    /// # Errors
    ///
    /// 非対応構成・親子不整合・循環・参照欠落・深さ上限超過などの場合にエラーを返す。
    pub fn local_hierarchy_with_options(
        &self,
        options: &HierarchyOptions,
    ) -> Result<LocalHierarchy, UnityError> {
        hierarchy::build_hierarchy(self, options)
    }
}
