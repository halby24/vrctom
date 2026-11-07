//! Prefabを入口に選んだときの変換対象FBXの解決。
//!
//! 入口は [`resolve_prefab_target`]（1件のPrefabの解決）と
//! [`list_prefab_targets`]（全Prefabの走査）である。
//! どちらも索引と読み手だけで組み立て、ファイルシステムに触れない。
//! 判定は型付き参照（`PrefabInstance.m_SourcePrefab` と
//! `SkinnedMeshRenderer.m_Mesh` のGUID）だけを使い、GUID文字列の
//! 部分一致検索はしない。`MonoBehaviour.m_Script` の外部GUID
//! （VRCSDK/Unity内蔵）は解決に使わず、未解決としても数えない。
//!
//! ```mermaid
//! flowchart LR
//!     prefab[Prefab YAML<br/>実体参照か直接記述] --> resolve[resolve_prefab_target<br/>直接FBXを1種に定める]
//!     index[AssetIndex<br/>GUIDと種別] --> resolve
//!     resolve --> list[list_prefab_targets<br/>Prefab種別をパス順に走査]
//! ```
//!
//! 直接参照されたFBXだけを対象にし、ネストPrefabの持つジオメトリは
//! 今回の対象外である。捨てずに [`PrefabTargetResolution::nested_prefabs`] に
//! 残し、呼び出し側が要解決として報告できる形にする。

use std::collections::{BTreeSet, HashMap};

use super::error::{VrmError, VrmErrorKind};
use crate::unity::{
    AssetIndex, AssetKind, AssetSource, Guid, ParseOptions, SerializedFile, TypedObject,
};

/// 1件のPrefab本文の既定上限（バイト）。
///
/// 実体の数百KBに2桁の余裕を持たせた大きさである。
const DEFAULT_MAX_PREFAB_BYTES: u64 = 64 * 1024 * 1024;
/// 走査の合計読み取り量の既定上限（バイト）。
const DEFAULT_MAX_TOTAL_BYTES: u64 = 512 * 1024 * 1024;

/// 解決時の読み取り上限。
///
/// フィールドは非公開で、既定値は [`Default`] が設定する。
/// 上限の変更は `with_` で始まるメソッドで行う。
#[derive(Debug, Clone)]
pub struct PrefabTargetLimits {
    max_prefab_bytes: u64,
    max_total_bytes: u64,
}

impl PrefabTargetLimits {
    /// 既定の上限を持つ設定を生成する。
    pub fn new() -> Self {
        Self {
            max_prefab_bytes: DEFAULT_MAX_PREFAB_BYTES,
            max_total_bytes: DEFAULT_MAX_TOTAL_BYTES,
        }
    }

    /// 1件のPrefab本文の上限（バイト数）を変更する。
    pub fn with_max_prefab_bytes(mut self, max_prefab_bytes: u64) -> Self {
        self.max_prefab_bytes = max_prefab_bytes;
        self
    }

    /// 走査の合計読み取り量の上限（バイト数）を変更する。
    pub fn with_max_total_bytes(mut self, max_total_bytes: u64) -> Self {
        self.max_total_bytes = max_total_bytes;
        self
    }
}

impl Default for PrefabTargetLimits {
    fn default() -> Self {
        Self::new()
    }
}

/// 解決結果の状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrefabTargetState {
    /// 直接FBXがちょうど1種あり、変換対象にできる場合。
    Resolved,
    /// 直接FBXもネストPrefabも無い場合。
    NoModel,
    /// 直接FBXが2種以上あり、先勝ちにできない場合。
    UnsupportedMultipleFbx,
    /// 直接FBXが無く、ネストPrefabの解決が必要な場合。
    RequiresPrefabResolution,
    /// 直接FBXが無く、参照先が索引に無いなど未解決の参照がある場合。
    Unresolved,
}

/// 変換対象のFBX1件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrefabTargetFbx {
    /// 資産パス（`Assets/...`）。
    pub path: String,
    /// 資産GUID（小文字hex）。
    pub guid: String,
}

/// 1件のPrefabの解決結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrefabTargetResolution {
    /// 解決したPrefabの資産パス（`Assets/...`）。
    pub prefab: String,
    /// 解決結果の状態。
    pub state: PrefabTargetState,
    /// 変換対象のFBX。`Resolved` の場合だけ `Some` である。
    pub fbx: Option<PrefabTargetFbx>,
    /// ネストPrefabの資産パス列（`Assets/...`、昇順・重複なし）。
    pub nested_prefabs: Vec<String>,
    /// 非FBXモデルや種別不明の解決済み参照の資産パス列（表示用、昇順・重複なし）。
    pub other_references: Vec<String>,
    /// GUID欠落・索引不在・不正GUIDの参照の記録列（表示用、昇順・重複なし）。
    pub unresolved_references: Vec<String>,
}

/// 走査で読めなかった1件の記録。
///
/// 解析の失敗を握りつぶして0件扱いにしないため、パスと理由を残す。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrefabTargetIssue {
    /// 出力パス（`Assets/...`）。
    pub path: String,
    /// 読めなかった理由の日本語診断。
    pub reason: String,
}

/// 全Prefabの走査結果。
///
/// `candidates` は解決できたPrefabの列（パス昇順）であり、
/// `unreadable` は読めなかったPrefabの記録列（パス昇順）である。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrefabTargetList {
    /// 解決できたPrefabの列（パス昇順）。
    pub candidates: Vec<PrefabTargetResolution>,
    /// 読めなかったPrefabの記録列（パス昇順）。
    pub unreadable: Vec<PrefabTargetIssue>,
}

/// 1件のPrefabから変換対象のFBXを決める。
///
/// `prefab_path` は `Assets/...` 形であり、索引の相対パスへ正規化して読む。
/// 判定は型付き参照だけを使い、`MonoBehaviour.m_Script` の外部GUIDは
/// 使わない。同一GUID・同一パスの重複は数えず、並びはパス昇順にする。
///
/// # Errors
///
/// `Assets/` 配下でないパス・読み取り失敗・上限超過・YAML不良・
/// 文書の型付き読み取りの失敗の場合に返す。
pub fn resolve_prefab_target(
    source: &dyn AssetSource,
    index: &AssetIndex,
    prefab_path: &str,
    limits: &PrefabTargetLimits,
) -> Result<PrefabTargetResolution, VrmError> {
    let Some(relative) = prefab_relative_path(prefab_path) else {
        return Err(VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!(
                "PrefabのパスがAssets配下でない: {}",
                crate::escape_diag(prefab_path),
            ),
        ));
    };
    let bytes = source
        .read_limited(relative, limits.max_prefab_bytes)
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!(
                    "Prefabを読めない: {}: {error}",
                    crate::escape_diag(prefab_path)
                ),
            ),
            std::io::ErrorKind::InvalidData => VrmError::new(
                VrmErrorKind::LimitExceeded,
                format!(
                    "Prefabが上限を超えた: {}（上限{}バイト）",
                    crate::escape_diag(prefab_path),
                    limits.max_prefab_bytes,
                ),
            ),
            _ => VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!(
                    "Prefabを読めない: {}: {error}",
                    crate::escape_diag(prefab_path)
                ),
            ),
        })?;
    let options = ParseOptions::new().with_max_file_bytes(limits.max_prefab_bytes);
    let file = SerializedFile::parse(&bytes, &options).map_err(|error| {
        VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!(
                "PrefabのYAMLを読めない（{}）: {error}",
                crate::escape_diag(prefab_path),
            ),
        )
    })?;
    resolve_parsed(prefab_path, &file, index)
}

/// 全Prefabをパス昇順に走査し、変換対象のFBXを決める。
///
/// 走査対象は種別がPrefabの記録だけであり、本文が読めない・解析できない
/// Prefabは `unreadable` に記録して全体は成功させる。ただし走査の合計
/// 読み取り量が上限を超えた場合は全体を失敗させ、途中結果で成功扱いにしない。
/// 個別Prefabの上限超過は `unreadable` に記録して次へ進み、合計予算は消費しない。
/// 確保前に残予算で読む。読み取り上限は `min(個別上限, 残予算)` とする。
///
/// # Errors
///
/// 走査の合計読み取り量が上限を超えた場合に返す。
pub fn list_prefab_targets(
    source: &dyn AssetSource,
    index: &AssetIndex,
    limits: &PrefabTargetLimits,
) -> Result<PrefabTargetList, VrmError> {
    let mut prefabs: Vec<&crate::unity::AssetRecord> = index
        .entries()
        .iter()
        .filter(|entry| entry.kind == AssetKind::Prefab)
        .collect();
    prefabs.sort_by(|a, b| a.path.cmp(&b.path));
    let parse_options = ParseOptions::new().with_max_file_bytes(limits.max_prefab_bytes);
    let mut used: u64 = 0;
    let mut candidates = Vec::new();
    let mut unreadable = Vec::new();
    for entry in prefabs {
        let output = format!("Assets/{}", entry.path);
        // 確保前に残予算で読む。読み取り上限は残予算以下に絞る。
        let remaining = limits.max_total_bytes.saturating_sub(used);
        if remaining == 0 {
            return Err(VrmError::new(
                VrmErrorKind::LimitExceeded,
                format!(
                    "Prefab走査の合計が上限を超えた: {}バイト（上限{}バイト）（{}）",
                    used,
                    limits.max_total_bytes,
                    crate::escape_diag(&output),
                ),
            ));
        }
        let read_limit = limits.max_prefab_bytes.min(remaining);
        let bytes = match source.read_limited(&entry.path, read_limit) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::InvalidData => {
                if read_limit < limits.max_prefab_bytes {
                    // 残予算より本文が大きい場合は全体を失敗させる。
                    return Err(VrmError::new(
                        VrmErrorKind::LimitExceeded,
                        format!(
                            "Prefab走査の合計が上限を超えた: {}バイト超（上限{}バイト）（{}）",
                            used.saturating_add(read_limit),
                            limits.max_total_bytes,
                            crate::escape_diag(&output),
                        ),
                    ));
                }
                unreadable.push(PrefabTargetIssue {
                    path: output,
                    reason: format!("Prefabが上限を超えた: {error}"),
                });
                continue;
            }
            Err(error) => {
                unreadable.push(PrefabTargetIssue {
                    path: output,
                    reason: format!("Prefabを読めない: {error}"),
                });
                continue;
            }
        };
        // 読み取り上限は残予算以下のため、加算で上限を超えない。
        used += bytes.len() as u64;
        let parsed = match SerializedFile::parse(&bytes, &parse_options) {
            Ok(parsed) => parsed,
            Err(error) => {
                unreadable.push(PrefabTargetIssue {
                    path: output,
                    reason: format!("PrefabのYAMLを読めない: {error}"),
                });
                continue;
            }
        };
        match resolve_parsed(&output, &parsed, index) {
            Ok(resolution) => candidates.push(resolution),
            Err(error) => unreadable.push(PrefabTargetIssue {
                path: output,
                reason: format!("Prefabの文書を読めない: {error}"),
            }),
        }
    }
    candidates.sort_by(|a, b| a.prefab.cmp(&b.prefab));
    unreadable.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(PrefabTargetList {
        candidates,
        unreadable,
    })
}

/// `Assets/...` 形の資産パスを索引用の相対パスに直す。
///
/// `Assets/`・`Assets\` の接頭辞だけを落とす。空や接頭辞だけの場合は
/// `None` を返し、呼び出し側が `Assets` 配下でないとして失敗する。
fn prefab_relative_path(prefab_path: &str) -> Option<&str> {
    let relative = prefab_path
        .strip_prefix("Assets/")
        .or_else(|| prefab_path.strip_prefix("Assets\\"))
        .unwrap_or("");
    if relative.is_empty() || prefab_path == relative {
        None
    } else {
        Some(relative)
    }
}

/// 参照の集計。重複排除と並び順の確定は呼び出し側で行う。
#[derive(Debug, Default)]
struct CollectedRefs {
    /// 直接FBX。キーはGUID（小文字hex）、値は（出力パス、GUID）である。
    direct: HashMap<String, (String, String)>,
    /// ネストPrefabの出力パス集合。
    nested: BTreeSet<String>,
    /// 解決済みだが対象外の参照の出力パス集合。
    other: BTreeSet<String>,
    /// 未解決の参照の表示用記録集合。
    unresolved: BTreeSet<String>,
}

/// 読み取り済みPrefab文書から解決結果を組み立てる。
fn resolve_parsed(
    prefab_path: &str,
    file: &SerializedFile,
    index: &AssetIndex,
) -> Result<PrefabTargetResolution, VrmError> {
    let mut collected = CollectedRefs::default();
    for document in file.documents() {
        let typed = document.decode().map_err(|error| {
            VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!(
                    "Prefabの文書を読めない（{}）: {error}",
                    crate::escape_diag(prefab_path),
                ),
            )
        })?;
        match typed {
            TypedObject::PrefabInstance(instance) => {
                if instance.source.is_null() {
                    collected
                        .unresolved
                        .insert("PrefabInstance.m_SourcePrefab: null参照".to_owned());
                } else {
                    collect_guid_ref(
                        &mut collected,
                        "PrefabInstance.m_SourcePrefab",
                        instance.source.guid.as_ref(),
                        index,
                    );
                }
            }
            TypedObject::SkinnedMeshRenderer(renderer) => {
                if renderer.mesh.is_null() {
                    collected
                        .unresolved
                        .insert("SkinnedMeshRenderer.m_Mesh: null参照".to_owned());
                } else {
                    collect_guid_ref(
                        &mut collected,
                        "SkinnedMeshRenderer.m_Mesh",
                        renderer.mesh.guid.as_ref(),
                        index,
                    );
                }
            }
            _ => {}
        }
    }
    let mut direct: Vec<(String, String)> = collected.direct.into_values().collect();
    direct.sort();
    let (state, fbx) = if direct.len() == 1 {
        // 件数は直前で確かめたため、先頭の取り出しは範囲内である。
        let (path, guid) = direct.swap_remove(0);
        (
            PrefabTargetState::Resolved,
            Some(PrefabTargetFbx { path, guid }),
        )
    } else if direct.len() >= 2 {
        (PrefabTargetState::UnsupportedMultipleFbx, None)
    } else if !collected.nested.is_empty() {
        (PrefabTargetState::RequiresPrefabResolution, None)
    } else if !collected.unresolved.is_empty() {
        (PrefabTargetState::Unresolved, None)
    } else {
        (PrefabTargetState::NoModel, None)
    };
    Ok(PrefabTargetResolution {
        prefab: prefab_path.to_owned(),
        state,
        fbx,
        nested_prefabs: collected.nested.into_iter().collect(),
        other_references: collected.other.into_iter().collect(),
        unresolved_references: collected.unresolved.into_iter().collect(),
    })
}

/// GUID付き参照1件を集計する。
///
/// `MonoBehaviour.m_Script` は呼び出し側で文書ごと見ないため、
/// ここに来るのは実体参照とメッシュ参照だけである。
/// 解決先の種別で直接FBX・ネストPrefab・対象外に振り分け、
/// GUIDなし・書式不良・索引不在は未解決の記録に残す。
fn collect_guid_ref(
    collected: &mut CollectedRefs,
    field: &str,
    guid: Option<&Guid>,
    index: &AssetIndex,
) {
    let Some(guid) = guid else {
        collected
            .unresolved
            .insert(format!("{field}: GUIDなし参照"));
        return;
    };
    let text = guid.as_str();
    let Ok(key) = Guid::parse(text) else {
        collected
            .unresolved
            .insert(format!("{field}: GUIDの書式が不正: {text}"));
        return;
    };
    let Some(record) = index.get(&key) else {
        collected
            .unresolved
            .insert(format!("{field}: GUID {text} の参照先が索引にない"));
        return;
    };
    let output = format!("Assets/{}", record.path);
    if fbx拡張子か(&record.path) {
        collected
            .direct
            .entry(text.to_owned())
            .or_insert((output, text.to_owned()));
    } else if record.kind == AssetKind::Prefab {
        collected.nested.insert(output);
    } else {
        collected.other.insert(output);
    }
}

/// 相対パスが `.fbx`（大小無視）かどうかを返す。
fn fbx拡張子か(relative_path: &str) -> bool {
    let file = relative_path.rsplit('/').next().unwrap_or(relative_path);
    match file.rsplit_once('.') {
        Some((stem, extension)) => !stem.is_empty() && extension.eq_ignore_ascii_case("fbx"),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    use crate::unity::{AssetMeta, AssetSource};

    // テスト名は日本語で書く（テストは仕様・契約であるため）。

    /// テスト用の直接FBXのGUID（数値化されないよう英字を含む32桁hex）。
    const FBX_A: &str = "fb00000000000000000000000000000b";
    /// テスト用の2件目の直接FBXのGUID。
    const FBX_B: &str = "fb00000000000000000000000000000c";
    /// テスト用のネストPrefabのGUID。
    const NEST_GUID: &str = "0e00000000000000000000000000000d";
    /// テスト用の対象外モデル（`.obj`）のGUID。
    const OBJ_GUID: &str = "face0000000000000000000000000001";
    /// テスト用の解決対象Prefab自体のGUID（他と重複しない32桁hex）。
    const AVATAR_GUID: &str = "a0000000000000000000000000000007";
    /// 索引に無いGUID。
    const MISSING_GUID: &str = "dead000000000000000000000000beef";

    /// メモリ内の読み手。パスと内容の対だけを持ち、ファイルシステムに触れない。
    struct 記憶読み手 {
        表: HashMap<String, Vec<u8>>,
    }

    impl AssetSource for 記憶読み手 {
        fn read_limited(&self, path: &str, limit: u64) -> std::io::Result<Vec<u8>> {
            let Some(bytes) = self.表.get(path) else {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "持たない",
                ));
            };
            if bytes.len() as u64 > limit {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "上限を超えた",
                ));
            }
            Ok(bytes.clone())
        }
    }

    /// 読み手へ渡された上限を記録する読み手。予算の事前絞り込みを確かめる。
    struct 記録読み手 {
        表: HashMap<String, Vec<u8>>,
        受けた上限: std::cell::RefCell<Vec<u64>>,
    }

    impl AssetSource for 記録読み手 {
        fn read_limited(&self, path: &str, limit: u64) -> std::io::Result<Vec<u8>> {
            self.受けた上限.borrow_mut().push(limit);
            let Some(bytes) = self.表.get(path) else {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "持たない",
                ));
            };
            if bytes.len() as u64 > limit {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "上限を超えた",
                ));
            }
            Ok(bytes.clone())
        }
    }

    /// 検査用の `.meta` を組み立てる。GUIDは呼び出し側が32桁hexで渡す。
    fn メタを作る(guid: &str, importer: &str) -> AssetMeta {
        AssetMeta::parse(
            format!("fileFormatVersion: 2\nguid: {guid}\n{importer}:\n  externalObjects: {{}}\n")
                .as_bytes(),
        )
        .expect("検査metaは読める")
    }

    /// 検査用の索引を作る。要素は（相対パス、GUID、importer名）である。
    fn 索引を作る(件: &[(&str, &str, &str)]) -> AssetIndex {
        let mut index = AssetIndex::new();
        for (path, guid, importer) in 件 {
            index
                .insert(path, メタを作る(guid, importer))
                .expect("索引に入る");
        }
        index
    }

    /// `PrefabInstance` 1件の文書を作る。結合時は先頭以外からdirectiveを落とす。
    fn 実体文書を作る(anchor: &str, guid: &str) -> String {
        format!(
            "%YAML 1.1\n%TAG !u! tag:unity3d.com,2011:\n--- !u!1001 &{anchor}\nPrefabInstance:\n  m_ObjectHideFlags: 0\n  m_SourcePrefab: {{fileID: 100100000, guid: {guid}, type: 2}}\n"
        )
    }

    /// `GameObject` と `SkinnedMeshRenderer` の直接記述2文書を作る。
    fn 直接文書を作る(go_anchor: &str, smr_anchor: &str, guid: &str) -> String {
        format!(
            "%YAML 1.1\n%TAG !u! tag:unity3d.com,2011:\n--- !u!1 &{go_anchor}\nGameObject:\n  m_ObjectHideFlags: 0\n  m_Name: Body\n  m_IsActive: 1\n  m_Component: []\n--- !u!137 &{smr_anchor}\nSkinnedMeshRenderer:\n  m_ObjectHideFlags: 0\n  m_GameObject: {{fileID: {go_anchor}}}\n  m_Enabled: 1\n  m_Mesh: {{fileID: 4300000, guid: {guid}, type: 2}}\n  m_Materials: []\n  m_Bones: []\n  m_RootBone: {{fileID: 0}}\n"
        )
    }

    /// 参照を持たない `GameObject` だけの文書を作る。
    fn 空文書を作る() -> String {
        "%YAML 1.1\n%TAG !u! tag:unity3d.com,2011:\n--- !u!1 &100000\nGameObject:\n  m_ObjectHideFlags: 0\n  m_Name: Root\n  m_IsActive: 1\n  m_Component: []\n"
            .to_owned()
    }

    /// 複数の文書列を1つのPrefab本文に結合する。先頭以外からdirectiveを落とす。
    fn 結合する(個々: &[String]) -> Vec<u8> {
        const DIRECTIVE: &str = "%YAML 1.1\n%TAG !u! tag:unity3d.com,2011:\n";
        let mut out = String::new();
        for (番号, 文書) in 個々.iter().enumerate() {
            if 番号 == 0 {
                out.push_str(文書);
            } else {
                out.push_str(文書.strip_prefix(DIRECTIVE).expect("directiveがある"));
            }
        }
        out.into_bytes()
    }

    /// 解決用の索引と読み手を作る。要素は（相対パス、GUID、importer名、本文）である。
    fn 解決材料を作る(件: &[(&str, &str, &str, Vec<u8>)]) -> (AssetIndex, 記憶読み手) {
        let mut index = AssetIndex::new();
        let mut 表 = HashMap::new();
        for (path, guid, importer, 本文) in 件 {
            index
                .insert(path, メタを作る(guid, importer))
                .expect("索引に入る");
            表.insert((*path).to_owned(), 本文.clone());
        }
        (index, 記憶読み手 { 表 })
    }

    /// 上限の記録付きで解決用の索引と読み手を作る。要素は解決材料と同じである。
    fn 記録材料を作る(件: &[(&str, &str, &str, Vec<u8>)]) -> (AssetIndex, 記録読み手) {
        let mut index = AssetIndex::new();
        let mut 表 = HashMap::new();
        for (path, guid, importer, 本文) in 件 {
            index
                .insert(path, メタを作る(guid, importer))
                .expect("索引に入る");
            表.insert((*path).to_owned(), 本文.clone());
        }
        (
            index,
            記録読み手 {
                表,
                受けた上限: std::cell::RefCell::new(Vec::new()),
            },
        )
    }

    /// 標準のFBX2件・ネストPrefab・対象外モデルを含む索引と読み手を作る。
    fn 標準材料を作る(avatar_body: Vec<u8>) -> (AssetIndex, 記憶読み手) {
        解決材料を作る(&[
            ("Model.fbx", FBX_A, "ModelImporter", b"dummy".to_vec()),
            ("Other.fbx", FBX_B, "ModelImporter", b"dummy".to_vec()),
            (
                "Nest.prefab",
                NEST_GUID,
                "PrefabImporter",
                空文書を作る().into_bytes(),
            ),
            ("Prop.obj", OBJ_GUID, "ModelImporter", b"dummy".to_vec()),
            ("Avatar.prefab", AVATAR_GUID, "PrefabImporter", avatar_body),
        ])
    }

    #[test]
    fn 実体参照の直接fbx1件を解決する() {
        // `m_SourcePrefab` がFBXを直接指す場合は変換対象にできる。
        let (index, 読み手) =
            標準材料を作る(実体文書を作る("100100000", FBX_A).into_bytes());
        let 解決 = resolve_prefab_target(
            &読み手,
            &index,
            "Assets/Avatar.prefab",
            &PrefabTargetLimits::new(),
        )
        .expect("直接FBX1件は解決できる");
        assert_eq!(解決.prefab, "Assets/Avatar.prefab");
        assert_eq!(解決.state, PrefabTargetState::Resolved);
        assert_eq!(
            解決.fbx,
            Some(PrefabTargetFbx {
                path: "Assets/Model.fbx".to_owned(),
                guid: FBX_A.to_owned(),
            })
        );
        assert!(解決.nested_prefabs.is_empty(), "ネストは無い");
        assert!(解決.unresolved_references.is_empty(), "未解決は無い");
    }

    #[test]
    fn 直接fbx1件とネストprefabを解決しつつ残す() {
        // 直接FBXが1種なら `Resolved` にし、ネストPrefabは捨てずに残す。
        let 本文 = 結合する(&[
            実体文書を作る("100100000", FBX_A),
            実体文書を作る("100100001", NEST_GUID),
        ]);
        let (index, 読み手) = 標準材料を作る(本文);
        let 解決 = resolve_prefab_target(
            &読み手,
            &index,
            "Assets/Avatar.prefab",
            &PrefabTargetLimits::new(),
        )
        .expect("直接FBX1件は解決できる");
        assert_eq!(解決.state, PrefabTargetState::Resolved);
        assert_eq!(
            解決.fbx,
            Some(PrefabTargetFbx {
                path: "Assets/Model.fbx".to_owned(),
                guid: FBX_A.to_owned(),
            })
        );
        assert_eq!(解決.nested_prefabs, vec!["Assets/Nest.prefab".to_owned()]);
    }

    #[test]
    fn 直接記述のメッシュ参照1件を解決する() {
        // `SkinnedMeshRenderer.m_Mesh` がFBXを指す場合も変換対象にできる。
        let (index, 読み手) =
            標準材料を作る(直接文書を作る("100000", "13700000", FBX_A).into_bytes());
        let 解決 = resolve_prefab_target(
            &読み手,
            &index,
            "Assets/Avatar.prefab",
            &PrefabTargetLimits::new(),
        )
        .expect("直接記述のFBX1件は解決できる");
        assert_eq!(解決.state, PrefabTargetState::Resolved);
        assert_eq!(
            解決.fbx.map(|fbx| fbx.path),
            Some("Assets/Model.fbx".to_owned())
        );
        assert!(解決.nested_prefabs.is_empty(), "ネストは無い");
    }

    #[test]
    fn 同一fbxの複数参照は1件に畳む() {
        // 実体参照と直接記述で同じFBXを指しても1種として解決する。
        let 本文 = 結合する(&[
            実体文書を作る("100100000", FBX_A),
            直接文書を作る("100000", "13700000", FBX_A),
            直接文書を作る("100001", "13700001", FBX_A),
        ]);
        let (index, 読み手) = 標準材料を作る(本文);
        let 解決 = resolve_prefab_target(
            &読み手,
            &index,
            "Assets/Avatar.prefab",
            &PrefabTargetLimits::new(),
        )
        .expect("同一FBXの重複は解決できる");
        assert_eq!(解決.state, PrefabTargetState::Resolved);
        assert_eq!(
            解決.fbx,
            Some(PrefabTargetFbx {
                path: "Assets/Model.fbx".to_owned(),
                guid: FBX_A.to_owned(),
            })
        );
    }

    #[test]
    fn 直接fbx2件は複数として報告する() {
        // 直接FBXが2種ある場合は先勝ちにせず `UnsupportedMultipleFbx` にする。
        let 本文 = 結合する(&[
            実体文書を作る("100100000", FBX_A),
            実体文書を作る("100100001", FBX_B),
        ]);
        let (index, 読み手) = 標準材料を作る(本文);
        let 解決 = resolve_prefab_target(
            &読み手,
            &index,
            "Assets/Avatar.prefab",
            &PrefabTargetLimits::new(),
        )
        .expect("走査自体は成功する");
        assert_eq!(解決.state, PrefabTargetState::UnsupportedMultipleFbx);
        assert_eq!(解決.fbx, None);
    }

    #[test]
    fn ネストprefabのみは要解決になる() {
        // 直接FBXが無くネストPrefabだけの場合は黙って捨てず要解決にする。
        let (index, 読み手) =
            標準材料を作る(実体文書を作る("100100000", NEST_GUID).into_bytes());
        let 解決 = resolve_prefab_target(
            &読み手,
            &index,
            "Assets/Avatar.prefab",
            &PrefabTargetLimits::new(),
        )
        .expect("走査自体は成功する");
        assert_eq!(解決.state, PrefabTargetState::RequiresPrefabResolution);
        assert_eq!(解決.fbx, None);
        assert_eq!(解決.nested_prefabs, vec!["Assets/Nest.prefab".to_owned()]);
    }

    #[test]
    fn 索引に無いguidは未解決になる() {
        // 参照先が索引に無い場合は `Unresolved` にし、GUIDが分かる記録を残す。
        let (index, 読み手) =
            標準材料を作る(実体文書を作る("100100000", MISSING_GUID).into_bytes());
        let 解決 = resolve_prefab_target(
            &読み手,
            &index,
            "Assets/Avatar.prefab",
            &PrefabTargetLimits::new(),
        )
        .expect("走査自体は成功する");
        assert_eq!(解決.state, PrefabTargetState::Unresolved);
        assert_eq!(解決.fbx, None);
        assert_eq!(解決.unresolved_references.len(), 1);
        assert!(
            解決.unresolved_references[0].contains(MISSING_GUID),
            "GUIDが分かる記録にする: {}",
            解決.unresolved_references[0]
        );
    }

    #[test]
    fn 参照なしprefabはモデルなしになる() {
        // 実体参照もメッシュ参照も無いPrefabは `NoModel` になる。
        let (index, 読み手) = 標準材料を作る(空文書を作る().into_bytes());
        let 解決 = resolve_prefab_target(
            &読み手,
            &index,
            "Assets/Avatar.prefab",
            &PrefabTargetLimits::new(),
        )
        .expect("走査自体は成功する");
        assert_eq!(解決.state, PrefabTargetState::NoModel);
        assert_eq!(解決.fbx, None);
        assert!(解決.nested_prefabs.is_empty(), "ネストは無い");
        assert!(解決.other_references.is_empty(), "対象外も無い");
        assert!(解決.unresolved_references.is_empty(), "未解決も無い");
    }

    #[test]
    fn 対象外モデルは表示用に残す() {
        // 非FBXモデル（`.obj`）への解決済み参照は対象外として表示に残す。
        let (index, 読み手) =
            標準材料を作る(実体文書を作る("100100000", OBJ_GUID).into_bytes());
        let 解決 = resolve_prefab_target(
            &読み手,
            &index,
            "Assets/Avatar.prefab",
            &PrefabTargetLimits::new(),
        )
        .expect("走査自体は成功する");
        assert_eq!(解決.state, PrefabTargetState::NoModel);
        assert_eq!(解決.other_references, vec!["Assets/Prop.obj".to_owned()]);
    }

    #[test]
    fn 一覧は昇順で解析不能を記録しつつ成功する() {
        // パス昇順に走査し、解析不能Prefabは記録して全体は成功させる。
        let (index, 読み手) = 解決材料を作る(&[
            ("Model.fbx", FBX_A, "ModelImporter", b"dummy".to_vec()),
            (
                "B.prefab",
                NEST_GUID,
                "PrefabImporter",
                実体文書を作る("100100000", FBX_A).into_bytes(),
            ),
            (
                "Broken.prefab",
                OBJ_GUID,
                "PrefabImporter",
                b"broken-body".to_vec(),
            ),
            (
                "A.prefab",
                FBX_B,
                "PrefabImporter",
                空文書を作る().into_bytes(),
            ),
        ]);
        let 一覧 = list_prefab_targets(&読み手, &index, &PrefabTargetLimits::new())
            .expect("解析不能があっても全体は成功する");
        assert_eq!(
            一覧
                .candidates
                .iter()
                .map(|解決| 解決.prefab.as_str())
                .collect::<Vec<_>>(),
            vec!["Assets/A.prefab", "Assets/B.prefab"]
        );
        assert_eq!(一覧.candidates[0].state, PrefabTargetState::NoModel);
        assert_eq!(一覧.candidates[1].state, PrefabTargetState::Resolved);
        assert_eq!(一覧.unreadable.len(), 1);
        assert_eq!(一覧.unreadable[0].path, "Assets/Broken.prefab");
        assert!(
            !一覧.unreadable[0].reason.is_empty(),
            "理由を残して黙って落とさない"
        );
    }

    #[test]
    fn 合計上限の超過は失敗する() {
        // 走査の合計が上限を超えた場合は途中結果で成功扱いにせず失敗する。
        // 読み手へ渡す上限が残予算に絞られていることも確かめる。
        let (index, 読み手) = 記録材料を作る(&[
            ("Model.fbx", FBX_A, "ModelImporter", b"dummy".to_vec()),
            (
                "B.prefab",
                NEST_GUID,
                "PrefabImporter",
                実体文書を作る("100100000", FBX_A).into_bytes(),
            ),
            (
                "A.prefab",
                FBX_B,
                "PrefabImporter",
                空文書を作る().into_bytes(),
            ),
        ]);
        let 制限 = PrefabTargetLimits::new().with_max_total_bytes(10);
        let 失敗 = list_prefab_targets(&読み手, &index, &制限).expect_err("合計超過は失敗する");
        assert_eq!(失敗.kind(), VrmErrorKind::LimitExceeded);
        assert_eq!(
            読み手.受けた上限.borrow().as_slice(),
            &[10],
            "残予算に絞った上限で読む",
        );
    }

    #[test]
    fn 残予算より大きい本文は絞った上限で読み全体が失敗する() {
        // 読み手へ渡す上限が `min(個別上限, 残予算)` であることと、
        // 残予算を超える本文で全体が `LimitExceeded` になることを確かめる。
        let 先頭 = 空文書を作る().into_bytes();
        let 残予算 = 先頭.len() as u64 + 10;
        let (index, 読み手) = 記録材料を作る(&[
            ("Model.fbx", FBX_A, "ModelImporter", b"dummy".to_vec()),
            ("A.prefab", FBX_B, "PrefabImporter", 先頭),
            (
                "B.prefab",
                NEST_GUID,
                "PrefabImporter",
                実体文書を作る("100100000", FBX_A).into_bytes(),
            ),
        ]);
        let 制限 = PrefabTargetLimits::new()
            .with_max_prefab_bytes(1000)
            .with_max_total_bytes(残予算);
        let 失敗 =
            list_prefab_targets(&読み手, &index, &制限).expect_err("残予算超過は全体の失敗になる");
        assert_eq!(失敗.kind(), VrmErrorKind::LimitExceeded);
        assert_eq!(
            読み手.受けた上限.borrow().as_slice(),
            &[残予算.min(1000), 10],
            "読み取り前に残予算で上限を絞る",
        );
    }

    #[test]
    fn 個別上限の超過は記録して成功し合計を消費しない() {
        // 残予算が十分でも個別上限を超える本文は `unreadable` にして次へ進み、
        // 全体は成功する。本文の合計より小さな合計上限でも通るため、
        // 個別上限の超過で合計予算を消費しないことが分かる。
        let (index, 読み手) = 記録材料を作る(&[
            ("Model.fbx", FBX_A, "ModelImporter", b"dummy".to_vec()),
            (
                "B.prefab",
                NEST_GUID,
                "PrefabImporter",
                実体文書を作る("100100000", FBX_A).into_bytes(),
            ),
            (
                "A.prefab",
                FBX_B,
                "PrefabImporter",
                空文書を作る().into_bytes(),
            ),
        ]);
        let 制限 = PrefabTargetLimits::new()
            .with_max_prefab_bytes(10)
            .with_max_total_bytes(15);
        let 一覧 =
            list_prefab_targets(&読み手, &index, &制限).expect("個別上限の超過は全体の成功になる");
        assert!(一覧.candidates.is_empty(), "読めた候補は無い");
        assert_eq!(一覧.unreadable.len(), 2);
        assert_eq!(
            読み手.受けた上限.borrow().as_slice(),
            &[10, 10],
            "個別上限の超過で合計予算を消費しない",
        );
    }

    #[test]
    fn 既定の上限は64メビバイトと512メビバイトであり変更できる() {
        // 既定値は実体の数百KBに余裕を持たせた大きさであり、`with_` で変える。
        let 制限 = PrefabTargetLimits::default();
        assert_eq!(制限.max_prefab_bytes, 64 * 1024 * 1024);
        assert_eq!(制限.max_total_bytes, 512 * 1024 * 1024);
        let 変更 = PrefabTargetLimits::new()
            .with_max_prefab_bytes(1)
            .with_max_total_bytes(2);
        assert_eq!(変更.max_prefab_bytes, 1);
        assert_eq!(変更.max_total_bytes, 2);
    }

    #[test]
    fn assets配下でないパスは失敗する() {
        // `Assets/` 接頭辞の無いパスは索引に引き当てず失敗する。
        let index = 索引を作る(&[("Avatar.prefab", NEST_GUID, "PrefabImporter")]);
        let 読み手 = 記憶読み手 {
            表: HashMap::new()
        };
        for パス in ["", "Avatar.prefab", "Other/Avatar.prefab", "Assets/"] {
            let 失敗 = resolve_prefab_target(&読み手, &index, パス, &PrefabTargetLimits::new())
                .expect_err("配下でないパスは失敗する");
            assert_eq!(失敗.kind(), VrmErrorKind::InvalidPrefab);
        }
    }
}
