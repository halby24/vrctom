//! 展開済みディレクトリの走査（Native限定）。
//!
//! 公開契約の入口は [`crate::unity`] を参照。
//!
//! `root` は [`crate::unitypackage::unpack_to`] の出力先をそのまま指す。
//! 再展開はせず、索引の正は `.meta` とする。本文の読み込みは行わない。
//! 読み取り中の入力変更に対するスナップショット保証はしない。

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use super::{
    AssetIndex, AssetMeta, AssetSource, IndexOptions, UnityError, UnityErrorKind,
    error::storage_failure_with_path,
};

/// 展開済みディレクトリを走査して索引を作る。
///
/// `root` は `unpack_to` の出力先（`Assets` を含む配置）を指す。
/// `.meta` のある通常ファイルとフォルダmetaを索引し、対応関係・実体有無を検証する。
/// symlinkは追従せず拒否する。本体の読み込みは行わない。
///
/// # Errors
///
/// パス契約違反・meta不良・対応不一致・重複・上限超過・入出力失敗の場合に
/// エラーを返す。
#[cfg(not(target_arch = "wasm32"))]
pub fn scan_directory(root: &Path, options: &IndexOptions) -> Result<AssetIndex, UnityError> {
    let staged = collect_staged(root, options)?;
    build_index(root, staged)
}

/// 展開済みディレクトリの `Assets` 直下を基準とする読み手。
///
/// `root` は [`scan_directory`] に渡す `Assets` そのものを指す。
/// 上限付き読み取りの意味は材質層の旧 `read_limited` と同じであり、
/// `limit` を超える内容は切り詰めず `InvalidData` で失敗する。
pub(crate) struct FsAssetSource<'a> {
    root: &'a Path,
}

impl<'a> FsAssetSource<'a> {
    pub(crate) fn new(root: &'a Path) -> Self {
        Self { root }
    }
}

impl AssetSource for FsAssetSource<'_> {
    fn read_limited(&self, path: &str, limit: u64) -> std::io::Result<Vec<u8>> {
        use std::io::Read as _;
        let file = std::fs::File::open(self.root.join(path))?;
        let mut limited = file.take(limit.saturating_add(1));
        let mut bytes = Vec::new();
        limited.read_to_end(&mut bytes)?;
        if bytes.len() as u64 > limit {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("読み取り上限を超えた: {limit}バイト"),
            ));
        }
        Ok(bytes)
    }
}

/// 走査で集めた中間情報。
struct Staged {
    /// metaの読み取り結果。キーは `.meta` を除いた相対パス。
    metas: HashMap<String, AssetMeta>,
    /// meta合計バイト数。
    meta_total_bytes: u64,
    /// 実体の相対パス一覧（通常ファイルのみ）。
    assets: Vec<String>,
    /// 実体のディレクトリ相対パス一覧。
    dirs: Vec<String>,
}

/// ディレクトリを再帰走査し、metaと実体の一覧を集める。
fn collect_staged(root: &Path, options: &IndexOptions) -> Result<Staged, UnityError> {
    let metadata = fs::symlink_metadata(root).map_err(|error| {
        storage_failure_with_path(error, format!("走査元を開けない: {root:?}"), root)
    })?;
    if metadata.is_symlink() {
        return Err(UnityError::new(
            UnityErrorKind::InvalidPath,
            format!("走査元がシンボリックリンクである: {root:?}"),
        ));
    }
    if !metadata.is_dir() {
        return Err(UnityError::new(
            UnityErrorKind::InvalidPath,
            format!("走査元がディレクトリでない: {root:?}"),
        ));
    }
    let mut staged = Staged {
        metas: HashMap::new(),
        meta_total_bytes: 0,
        assets: Vec::new(),
        dirs: Vec::new(),
    };
    let mut stack = vec![PathBuf::new()];
    while let Some(relative) = stack.pop() {
        let full = root.join(&relative);
        let mut children: Vec<fs::DirEntry> = fs::read_dir(&full)
            .map_err(|error| {
                storage_failure_with_path(error, format!("一覧を読めない: {full:?}"), &full)
            })?
            .collect::<Result<_, _>>()
            .map_err(|error| {
                storage_failure_with_path(error, format!("項目を読めない: {full:?}"), &full)
            })?;
        // 挿入順を決定的にするため、名前順に辿る。
        children.sort_by_key(|entry| entry.file_name());
        for entry in children {
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                return Err(
                    UnityError::new(UnityErrorKind::InvalidPath, "UTF-8でない名前がある")
                        .with_path(full.to_string_lossy().into_owned()),
                );
            };
            let child_relative = if relative.as_os_str().is_empty() {
                PathBuf::from(name)
            } else {
                relative.join(name)
            };
            let child_full = root.join(&child_relative);
            let child_metadata = fs::symlink_metadata(&child_full).map_err(|error| {
                storage_failure_with_path(
                    error,
                    format!("項目を開けない: {child_full:?}"),
                    &child_full,
                )
            })?;
            // symlinkは追従せず拒否する。ぶら下がりもここで落ちる。
            if child_metadata.is_symlink() {
                return Err(UnityError::new(
                    UnityErrorKind::InvalidPath,
                    format!("シンボリックリンクを追従しない: {child_full:?}"),
                ));
            }
            // 区切りは `/` に統一する。
            let relative_posix = child_relative
                .iter()
                .map(|component| {
                    component.to_str().ok_or_else(|| {
                        UnityError::new(UnityErrorKind::InvalidPath, "UTF-8でない名前がある")
                            .with_path(child_full.to_string_lossy().into_owned())
                    })
                })
                .collect::<Result<Vec<_>, _>>()?
                .join("/");
            if child_metadata.is_dir() {
                staged.dirs.push(relative_posix.clone());
                stack.push(child_relative);
            } else if child_metadata.is_file() {
                // `.meta` 自体は実体ではなく索引の正である。
                if let Some(asset_path) = relative_posix.strip_suffix(".meta") {
                    read_staged_meta(&child_full, asset_path, options, &mut staged)?;
                } else {
                    staged.assets.push(relative_posix);
                }
            } else {
                return Err(UnityError::new(
                    UnityErrorKind::InvalidPath,
                    format!("通常ファイルでもディレクトリでもない: {child_full:?}"),
                ));
            }
        }
    }
    Ok(staged)
}

/// `.meta` を上限付きで読み、作業域に置く。
fn read_staged_meta(
    full: &Path,
    asset_path: &str,
    options: &IndexOptions,
    staged: &mut Staged,
) -> Result<(), UnityError> {
    if staged.metas.len() >= options.max_assets {
        return Err(UnityError::new(
            UnityErrorKind::LimitExceeded,
            format!("アセット数が上限を超えた: {}", staged.metas.len() + 1),
        )
        .with_path(asset_path.to_owned()));
    }
    let size = full
        .metadata()
        .map_err(|error| {
            storage_failure_with_path(error, format!("metaを開けない: {full:?}"), full)
        })?
        .len();
    if size > options.max_meta_bytes {
        return Err(UnityError::new(
            UnityErrorKind::LimitExceeded,
            format!("metaが1件の上限を超えた: {size}バイト"),
        )
        .with_path(asset_path.to_owned()));
    }
    staged.meta_total_bytes = staged.meta_total_bytes.saturating_add(size);
    if staged.meta_total_bytes > options.max_meta_total_bytes {
        return Err(UnityError::new(
            UnityErrorKind::LimitExceeded,
            format!("metaの合計が上限を超えた: {}", staged.meta_total_bytes),
        )
        .with_path(asset_path.to_owned()));
    }
    let bytes = fs::read(full).map_err(|error| {
        storage_failure_with_path(error, format!("metaを読めない: {full:?}"), full)
    })?;
    let meta = AssetMeta::parse_with_limit(&bytes, options.max_meta_bytes).map_err(|error| {
        if error.path().is_none() {
            error.with_path(asset_path.to_owned())
        } else {
            error
        }
    })?;
    staged.metas.insert(asset_path.to_owned(), meta);
    Ok(())
}

/// 対応関係と実体有無を検証し、決定的な順序で索引を作る。
///
/// 実体の有無と種別はファイルシステムに問い合わせる。
/// 読み取り中の入力変更に対するスナップショット保証はしない。
fn build_index(root: &Path, staged: Staged) -> Result<AssetIndex, UnityError> {
    AssetIndex::build_from_parts(staged.metas, &staged.assets, &staged.dirs, |path| {
        fs::symlink_metadata(root.join(path))
            .ok()
            .map(|entity| entity.is_dir())
    })
}
