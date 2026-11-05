//! Prefabの基準姿勢（`SkinnedMeshRenderer.m_BlendShapeWeights`）の解決。
//!
//! 入口は [`prefab_baseline_weights`] であり、Unityを回さずPrefabの直接記述または
//! `PrefabInstance` の改変からUnity重み（0..=100）を読み、FBXチャンネル順を
//! Unityのブレンドシェイプ索引と同一視してチャンネルへ写す。
//! 出所の検証はdescriptor経路と同じ規則を使い、検証できない文書は使わない。
//! ベースへ焼き込む適用値そのものを返し、ヒントではない。

use std::collections::{HashMap, HashSet};

use yaml_serde::Value;

use super::descriptor::{fbx_renderer_ids, find_fbx_instance, renderer_docs};
use super::error::{VrmError, VrmErrorKind};
use super::{BaselineChannel, BaselineWeights};
use crate::fbx::{FbxFile, ObjectId};
use crate::unity::{FileId, SerializedFile, TypedObject};

/// 重み率と初期値の判定許容誤差。
/// descriptor・中間モデル側の `FULL_WEIGHT_TOLERANCE` と同じ値である。
const FULL_WEIGHT_TOLERANCE: f64 = 1e-6;

/// 基準姿勢の重み配列の上限（スロット数）。
///
/// 正常なブレンドシェイプ数は数千件以下であり、100万件は実データを十分に上回る。
/// 巨大な `.Array.size` や `.data[i]` による過大確保・加算オーバーフローを抑えるための上限であり、
/// 超えた値は破損として失敗させる。ゼロ埋めの実体化はしない。
const MAX_BASELINE_SHAPE_SLOTS: usize = 1_000_000;

/// 改変内容から復元した重み配列。巨大なゼロ列を実体化しない疎な保持である。
///
/// `.Array.size` の宣言と `.data[i]` の明示成分だけを持ち、未設定の索引は0として読む。
/// 巨大索引のゼロ値は保持しない（寄与しないため）。非ゼロの巨大索引は上限超過で失敗させる。
#[derive(Debug, Clone, Default, PartialEq)]
struct RestoredWeights {
    /// `.Array.size` の宣言値。無い場合は `None` である。
    size: Option<usize>,
    /// 明示成分（索引→重み）。値域・有限の検査済みである。
    values: std::collections::BTreeMap<usize, f64>,
}

impl RestoredWeights {
    /// 索引の実効値を返す。範囲外・未設定は0である。
    fn get(&self, index: usize) -> f64 {
        self.values.get(&index).copied().unwrap_or(0.0)
    }
}

/// Prefabから基準姿勢の重みを解決する。
///
/// 戻り値は適用値そのものであり、呼び出し側はメッシュ変換へそのまま渡す。
/// 非ゼロ重みの無いジオメトリは含めない。重みはUnityの0..=100である。
/// `DeformPercent` は使わない。既存の非ゼロ検査はkeptチャンネル側に残す。
///
/// # Errors
///
/// 取り込みID表を作れない場合・PrefabInstanceが複数ある場合・
/// 重みが非有限・負値・100超・上限超えの場合・非ゼロ索引が範囲外の場合・
/// 非ゼロチャンネルが単一フレームでない場合・`FullWeights` が全100でない場合・
/// 同じジオメトリの実効重みが一致しない場合・FBXを指す非ゼロ重みが
/// メッシュへ解決できない場合・出所の矛盾する描画設定への非ゼロ改変がある場合に返す。
/// 対応元GUID・instance・classIDの不一致などで出所が検証できない文書自体は使わないが、
/// 除外したstripped文書の対応元（tainted）への非ゼロ改変は検証の迂回になるため失敗させる。
/// 文書が存在しない対応元への改変は矛盾が無いため受け入れて突き合わせる。
pub fn prefab_baseline_weights(
    prefab: &SerializedFile,
    prefab_label: &str,
    fbx: &FbxFile<'_>,
    fbx_guid: &str,
) -> Result<BaselineWeights, VrmError> {
    // 取り込みID表とPrefabInstanceの特定はdescriptor経路と共有する。
    // 二重実装にせず、検証規則の差異を作らない。
    let (mesh_ids, renderer_ids) = fbx_renderer_ids(fbx, prefab_label)?;
    let instance = find_fbx_instance(prefab, prefab_label, fbx_guid)?;
    let doc_set = renderer_docs(
        prefab,
        prefab_label,
        &mesh_ids,
        &renderer_ids,
        fbx_guid,
        instance,
    )?;
    let docs = &doc_set.docs;
    // 直接記述の重みは文書IDから引く。検証済み文書だけを使う。
    let mut direct_by_doc: HashMap<FileId, Vec<f64>> = HashMap::new();
    for doc in docs {
        if doc.source_id.is_some() {
            continue;
        }
        let Some(document) = prefab.document(doc.doc_id) else {
            continue;
        };
        let typed = document.decode().map_err(|error| {
            VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!(
                    "Prefabの文書を読めない（{}）: {error}",
                    crate::escape_diag(prefab_label),
                ),
            )
        })?;
        let TypedObject::SkinnedMeshRenderer(renderer) = typed else {
            continue;
        };
        let weights = renderer.blend_shape_weights.clone().unwrap_or_default();
        for (index, weight) in weights.iter().enumerate() {
            if !weight.is_finite() || !(0.0..=100.0).contains(weight) {
                return Err(VrmError::new(
                    VrmErrorKind::InvalidPrefab,
                    format!(
                        "基準姿勢の重みが範囲外である: 索引{index}（{weight}） [object: {}]（{}）",
                        doc.geometry.value(),
                        crate::escape_diag(prefab_label),
                    ),
                ));
            }
        }
        // 同一文書IDの重複は通常到達しないが、先勝ちにせず失敗する。
        if direct_by_doc.insert(doc.doc_id, weights).is_some() {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!(
                    "基準姿勢の文書が重複するため先勝ちにしない: fileID {}（{}）",
                    doc.doc_id,
                    crate::escape_diag(prefab_label),
                ),
            ));
        }
    }
    // 改変内容方式の重みは特定したinstanceの改変から復元する。
    // 直接記述方式では改変を使わない。戻り値は疎であり巨大なゼロ列は無い。
    let mut stripped_by_source: HashMap<FileId, RestoredWeights> = HashMap::new();
    if let Some(instance_id) = instance {
        let modifications = instance_modifications(prefab, prefab_label, instance_id)?;
        stripped_by_source =
            restore_stripped_weights(&modifications, &renderer_ids, fbx_guid, prefab_label)?;
    }
    // ジオメトリごとの候補重み列を集める。検証済み文書に対応するsourceと、
    // 文書が無いが矛盾も無いsourceの改変を受け入れる。矛盾するsource（tainted）の
    // 非ゼロ改変は `reject_unresolved_nonzero` で失敗させる。
    // 直接記述と改変内容の混在も同じジオメトリに載る場合は突き合わせ、
    // 受理した全rendererの実効重みを比較し、一致しない場合は失敗する。
    // 候補が無いジオメトリは基準姿勢を持たない。
    let mut candidates: HashMap<ObjectId, Vec<RestoredWeights>> = HashMap::new();
    for doc in docs {
        let weights = match doc.source_id {
            None => direct_weights(&direct_by_doc, doc.doc_id),
            Some(source) => stripped_by_source.get(&source).cloned().unwrap_or_default(),
        };
        candidates.entry(doc.geometry).or_default().push(weights);
    }
    // 検証済み文書に対応するsourceの集合を作る。
    let verified_sources: HashSet<FileId> = docs.iter().filter_map(|doc| doc.source_id).collect();
    // 検証済み文書を持たないsourceの改変は、矛盾が無ければ受け入れて突き合わせる
    // （候補の有無で打ち切らない）。ゼロのみ・大きさのみの改変も候補に入れ、
    // 受理した全rendererの実効値を比較する。統合後に全ゼロのgeometryは省く。
    for (source, weights) in &stripped_by_source {
        if verified_sources.contains(source) {
            continue;
        }
        if doc_set.tainted_sources.contains(source) {
            continue;
        }
        let Some(&geometry) = renderer_ids.get(source) else {
            // 復元は既知の取り込みIDだけを集めるため、到達しない。
            continue;
        };
        candidates
            .entry(geometry)
            .or_default()
            .push(weights.clone());
    }
    // FBXを指すがメッシュへ解決できない非ゼロ重みは欠落させず失敗する。
    // 出所の矛盾するsourceへの非ゼロ改変もここで失敗させる。
    reject_unresolved_nonzero(
        prefab,
        prefab_label,
        &mesh_ids,
        &renderer_ids,
        fbx_guid,
        &doc_set.tainted_sources,
    )?;
    // 候補を統合し、チャンネルへ写す。空のジオメトリは省く。
    let mut meshes: HashMap<ObjectId, Vec<BaselineChannel>> = HashMap::new();
    // 決定的な診断のため、ジオメトリID順に処理する。
    let mut geometries: Vec<ObjectId> = candidates.keys().copied().collect();
    geometries.sort_by_key(|id| id.value());
    for geometry in geometries {
        let list = candidates.remove(&geometry).unwrap_or_default();
        let effective = unify_geometry_weights(&list, geometry, prefab_label)?;
        let channels = fbx.blend_shape_channels(geometry);
        // チャンネル数を超える非ゼロの明示成分は範囲外として失敗させる。
        // チャンネル数未満の索引だけが焼き込み対象である。
        for (index, weight) in &effective.values {
            if *weight != 0.0 && *index >= channels.len() {
                return Err(VrmError::new(
                    VrmErrorKind::InvalidPrefab,
                    format!(
                        "基準姿勢の索引が範囲外である: 索引{index}（チャンネル{}件） [object: {}]（{}）",
                        channels.len(),
                        geometry.value(),
                        crate::escape_diag(prefab_label),
                    ),
                ));
            }
        }
        let mut kept = Vec::new();
        for (index, channel) in channels.iter().enumerate() {
            let weight = effective.get(index);
            if weight == 0.0 {
                continue;
            }
            if channel.shapes.len() != 1 {
                return Err(VrmError::new(
                    VrmErrorKind::UnsupportedBlendShape,
                    format!(
                        "単一フレームでないチャンネルがある: {}（形状数{}）",
                        channel.name,
                        channel.shapes.len()
                    ),
                )
                .with_object(channel.id));
            }
            if channel.has_full_weights {
                let weights = fbx.blend_channel_weights(channel.id).map_err(|error| {
                    VrmError::new(VrmErrorKind::InvalidStructure, error.to_string())
                        .with_object(channel.id)
                })?;
                let all_full = weights.iter().all(|value| {
                    value.is_finite() && (*value - 100.0).abs() <= FULL_WEIGHT_TOLERANCE
                });
                if !all_full {
                    return Err(VrmError::new(
                        VrmErrorKind::UnsupportedBlendShape,
                        format!("重み率が全100でないチャンネルがある: {}", channel.name),
                    )
                    .with_object(channel.id));
                }
            }
            kept.push(BaselineChannel {
                channel: channel.id,
                weight,
            });
        }
        if !kept.is_empty() {
            meshes.insert(geometry, kept);
        }
    }
    Ok(BaselineWeights { meshes })
}

/// 直接記述の重み列を疎な保持へ写す。
///
/// 直接記述の配列はYAML読み取り時に実体化済みであり、上限の検査対象にしない。
/// 検証済みの範囲検査（有限・0..=100）は呼び出し側で済ませている。
fn direct_weights(by_doc: &HashMap<FileId, Vec<f64>>, doc_id: FileId) -> RestoredWeights {
    let Some(weights) = by_doc.get(&doc_id) else {
        return RestoredWeights::default();
    };
    // 決定的な順序のため、索引順に並べてから載せる。
    let mut values = std::collections::BTreeMap::new();
    for (index, weight) in weights.iter().enumerate() {
        values.insert(index, *weight);
    }
    RestoredWeights {
        size: Some(weights.len()),
        values,
    }
}

/// 特定したPrefabInstanceの改変内容を読む。
fn instance_modifications(
    prefab: &SerializedFile,
    prefab_label: &str,
    instance_id: FileId,
) -> Result<Vec<crate::unity::PrefabModification>, VrmError> {
    let Some(document) = prefab.document(instance_id) else {
        return Err(VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!(
                "PrefabInstanceの文書が無い（{}）",
                crate::escape_diag(prefab_label),
            ),
        ));
    };
    let typed = document.decode().map_err(|error| {
        VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!(
                "Prefabの文書を読めない（{}）: {error}",
                crate::escape_diag(prefab_label),
            ),
        )
    })?;
    let TypedObject::PrefabInstance(instance) = typed else {
        return Err(VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!(
                "PrefabInstanceの文書が描画設定に変わっている（{}）",
                crate::escape_diag(prefab_label),
            ),
        ));
    };
    instance.modifications().map_err(|error| {
        VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!(
                "Prefabの改変内容を読めない: {}（実体{}）: {error}",
                crate::escape_diag(prefab_label),
                instance_id,
            ),
        )
    })
}

/// 改変内容から取り込み描画設定ごとの重み配列を復元する。
///
/// `.Array.size` で長さを決め、`.data[i]` を順に適用する。戻り値は疎であり、
/// 巨大なゼロ列を実体化しない。足りない索引は0として扱う。
/// 対象GUIDが入力FBXと一致し、既知の取り込み描画設定を指すものだけを集める。
/// GUID不一致・未知の対象は入れない。未知の対象への非ゼロ重みは呼び出し側で別途失敗させる。
/// 大きさ・索引が上限（[`MAX_BASELINE_SHAPE_SLOTS`]）を超えた場合は失敗させる。
/// ただし上限超えの索引でも値がゼロの場合は寄与しないため無視して成功させる。
fn restore_stripped_weights(
    modifications: &[crate::unity::PrefabModification],
    renderer_ids: &HashMap<FileId, ObjectId>,
    fbx_guid: &str,
    prefab_label: &str,
) -> Result<HashMap<FileId, RestoredWeights>, VrmError> {
    const SIZE_PATH: &str = "m_BlendShapeWeights.Array.size";
    const DATA_PREFIX: &str = "m_BlendShapeWeights.Array.data[";
    // 対象ごとの大きさ・成分・重複検査用の集合を持つ。
    struct Entry {
        size: Option<usize>,
        data: std::collections::BTreeMap<usize, f64>,
    }
    let mut entries: HashMap<FileId, Entry> = HashMap::new();
    let mut seen: HashSet<(FileId, String)> = HashSet::new();
    for modification in modifications {
        // 取り込み由来以外（別資産の対象）は扱わない。
        let guid_matches = modification
            .target
            .guid
            .as_ref()
            .is_some_and(|guid| guid.as_str().eq_ignore_ascii_case(fbx_guid));
        if !guid_matches {
            continue;
        }
        let path = modification.property_path.as_str();
        if path != SIZE_PATH && !path.starts_with(DATA_PREFIX) {
            continue;
        }
        if !renderer_ids.contains_key(&modification.target.file_id) {
            continue;
        }
        if !seen.insert((
            modification.target.file_id,
            modification.property_path.clone(),
        )) {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!(
                    "同じ対象への上書きが重複するため先勝ちにしない: {}（対象{}）（{}）",
                    crate::escape_diag(path),
                    modification.target.file_id,
                    crate::escape_diag(prefab_label),
                ),
            ));
        }
        let entry = entries.entry(modification.target.file_id).or_insert(Entry {
            size: None,
            data: std::collections::BTreeMap::new(),
        });
        if path == SIZE_PATH {
            let size = mod_size_value(&modification.value, prefab_label, path)?;
            if size > MAX_BASELINE_SHAPE_SLOTS {
                return Err(VrmError::new(
                    VrmErrorKind::InvalidPrefab,
                    format!(
                        "基準姿勢の大きさが上限を超える: {}（{}）（{}）",
                        size,
                        crate::escape_diag(path),
                        crate::escape_diag(prefab_label),
                    ),
                ));
            }
            if entry.size.is_some() {
                return Err(VrmError::new(
                    VrmErrorKind::InvalidPrefab,
                    format!(
                        "同じ対象への上書きが重複するため先勝ちにしない: {}（対象{}）（{}）",
                        crate::escape_diag(path),
                        modification.target.file_id,
                        crate::escape_diag(prefab_label),
                    ),
                ));
            }
            entry.size = Some(size);
        } else {
            let Some(index) = mod_data_index(path) else {
                return Err(VrmError::new(
                    VrmErrorKind::InvalidPrefab,
                    format!(
                        "基準姿勢の欄パスが壊れている: {}（対象{}）（{}）",
                        crate::escape_diag(path),
                        modification.target.file_id,
                        crate::escape_diag(prefab_label),
                    ),
                ));
            };
            let weight = mod_f64_value(&modification.value, prefab_label, path)?;
            if !weight.is_finite() || !(0.0..=100.0).contains(&weight) {
                return Err(VrmError::new(
                    VrmErrorKind::InvalidPrefab,
                    format!(
                        "基準姿勢の重みが範囲外である: {path}（{weight}）（{}）",
                        crate::escape_diag(prefab_label),
                    ),
                ));
            }
            // 上限超えの索引は、非ゼロなら資源を使い尽くすため失敗させる。
            // ゼロ値は寄与しないため保持せず無視して成功させる。
            if index > MAX_BASELINE_SHAPE_SLOTS {
                if weight != 0.0 {
                    return Err(VrmError::new(
                        VrmErrorKind::InvalidPrefab,
                        format!(
                            "基準姿勢の索引が上限を超える: {}（{}）（{}）",
                            index,
                            crate::escape_diag(path),
                            crate::escape_diag(prefab_label),
                        ),
                    ));
                }
                continue;
            }
            if entry.data.insert(index, weight).is_some() {
                return Err(VrmError::new(
                    VrmErrorKind::InvalidPrefab,
                    format!(
                        "同じ対象への上書きが重複するため先勝ちにしない: {}（対象{}）（{}）",
                        crate::escape_diag(path),
                        modification.target.file_id,
                        crate::escape_diag(prefab_label),
                    ),
                ));
            }
        }
    }
    // 疎のまま返し、巨大なゼロ列を実体化しない。
    // 決定的な順序のため、対象ID順に並べてから載せる。
    let mut targets: Vec<FileId> = entries.keys().copied().collect();
    targets.sort_unstable();
    let mut out: HashMap<FileId, RestoredWeights> = HashMap::new();
    for target in targets {
        let entry = entries
            .remove(&target)
            .map_or(RestoredWeights::default(), |entry| RestoredWeights {
                size: entry.size,
                values: entry.data,
            });
        out.insert(target, entry);
    }
    Ok(out)
}

/// 改変値の大きさを非負整数として読む。
fn mod_size_value(
    value: &Option<Value>,
    prefab_label: &str,
    path: &str,
) -> Result<usize, VrmError> {
    let number = match value {
        Some(Value::Number(number)) => number,
        _ => {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!(
                    "基準姿勢の大きさが整数でない: {}（{}）",
                    crate::escape_diag(path),
                    crate::escape_diag(prefab_label),
                ),
            ));
        }
    };
    if let Some(int) = number.as_i64() {
        if int < 0 {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!(
                    "基準姿勢の大きさが負である: {}（{}）",
                    crate::escape_diag(path),
                    crate::escape_diag(prefab_label),
                ),
            ));
        }
        return usize::try_from(int).map_err(|_| {
            VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!(
                    "基準姿勢の大きさが範囲外である: {}（{}）",
                    crate::escape_diag(path),
                    crate::escape_diag(prefab_label),
                ),
            )
        });
    }
    if let Some(int) = number.as_u64() {
        return usize::try_from(int).map_err(|_| {
            VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!(
                    "基準姿勢の大きさが範囲外である: {}（{}）",
                    crate::escape_diag(path),
                    crate::escape_diag(prefab_label),
                ),
            )
        });
    }
    if let Some(float) = number.as_f64()
        && float.is_finite()
        && float.fract() == 0.0
        && float >= 0.0
        && float <= usize::MAX as f64
    {
        return Ok(float as usize);
    }
    Err(VrmError::new(
        VrmErrorKind::InvalidPrefab,
        format!(
            "基準姿勢の大きさが整数でない: {}（{}）",
            crate::escape_diag(path),
            crate::escape_diag(prefab_label),
        ),
    ))
}

/// 改変値の重みをf64として読む。数値以外は失敗する。
fn mod_f64_value(value: &Option<Value>, prefab_label: &str, path: &str) -> Result<f64, VrmError> {
    match value {
        Some(Value::Number(number)) => {
            if let Some(int) = number.as_i64() {
                return Ok(int as f64);
            }
            if let Some(int) = number.as_u64() {
                return Ok(int as f64);
            }
            if let Some(float) = number.as_f64() {
                return Ok(float);
            }
            Err(VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!(
                    "基準姿勢の重みが数値でない: {}（{}）",
                    crate::escape_diag(path),
                    crate::escape_diag(prefab_label),
                ),
            ))
        }
        _ => Err(VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!(
                "基準姿勢の重みが数値でない: {}（{}）",
                crate::escape_diag(path),
                crate::escape_diag(prefab_label),
            ),
        )),
    }
}

/// `m_BlendShapeWeights.Array.data[i]` の添字を読む。形違いの場合は `None` である。
fn mod_data_index(path: &str) -> Option<usize> {
    const PREFIX: &str = "m_BlendShapeWeights.Array.data[";
    let rest = path.strip_prefix(PREFIX)?;
    let digits = rest.strip_suffix(']')?;
    // 空・符号・空白は受け付けず、10進の添字だけを読む。
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// 同じジオメトリの候補重みを統合する。明示成分だけを比較し、一致を要求する。
///
/// 未設定の索引は両者とも0であり等しいため、明示索引の和集合だけを比べる。
/// 短い側を0詰めするのと同じ意味であり、巨大なゼロ列を実体化しない。
/// 実効値は最初の候補から作り、後段はチャンネル数で範囲を区切る。
fn unify_geometry_weights(
    list: &[RestoredWeights],
    geometry: ObjectId,
    prefab_label: &str,
) -> Result<RestoredWeights, VrmError> {
    let Some(first) = list.first() else {
        return Ok(RestoredWeights::default());
    };
    // 明示索引の和集合だけを比べる。範囲外は両者とも0であり等しい。
    let mut indices: std::collections::BTreeSet<usize> = std::collections::BTreeSet::new();
    for candidate in list {
        indices.extend(candidate.values.keys().copied());
    }
    for index in indices {
        let base = first.get(index);
        for other in &list[1..] {
            if other.get(index) != base {
                return Err(VrmError::new(
                    VrmErrorKind::InvalidPrefab,
                    format!(
                        "基準姿勢の重みが一致しないため先勝ちにしない: 索引{index}（{base}に対して{}） [object: {}]（{}）",
                        other.get(index),
                        geometry.value(),
                        crate::escape_diag(prefab_label),
                    ),
                ));
            }
        }
    }
    Ok(first.clone())
}

/// FBXを指すがメッシュへ解決できない非ゼロ重みを失敗させる。
///
/// 出所が検証できない文書自体は使わないが、対象が入力FBXであり重みが
/// 非ゼロなのに解決できない場合は見た目の欠落になるため失敗する。
/// ゼロ重みの解決不能は無視する。
/// 出所の矛盾するsource（検証で除外したstripped文書の対応元）への非ゼロ改変も、
/// 検証済み文書の除外を迂回するため失敗させる。ゼロ改変は寄与しないため無視する。
fn reject_unresolved_nonzero(
    prefab: &SerializedFile,
    prefab_label: &str,
    mesh_ids: &HashMap<FileId, ObjectId>,
    renderer_ids: &HashMap<FileId, ObjectId>,
    fbx_guid: &str,
    tainted_sources: &HashSet<FileId>,
) -> Result<(), VrmError> {
    // 直接記述の未解決は、メッシュ参照のGUIDが入力FBXであり、
    // 非ゼロ重みを持つ場合だけ失敗させる。null参照（`fileID: 0`）でも
    // GUIDが入力FBXなら対象FBXを指すため、解決不能として失敗させる。
    for doc in prefab.documents() {
        if doc.stripped() {
            continue;
        }
        let typed = doc.decode().map_err(|error| {
            VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!(
                    "Prefabの文書を読めない（{}）: {error}",
                    crate::escape_diag(prefab_label),
                ),
            )
        })?;
        let TypedObject::SkinnedMeshRenderer(renderer) = typed else {
            continue;
        };
        let guid_matches = renderer
            .mesh
            .guid
            .as_ref()
            .is_some_and(|guid| guid.as_str().eq_ignore_ascii_case(fbx_guid));
        if !guid_matches {
            continue;
        }
        // GUIDが無い・別資産のnull参照は対象外であり無視する。
        // 解決済み（取り込みID表にある参照）は使っているため通す。
        let resolved = !renderer.mesh.is_null() && mesh_ids.contains_key(&renderer.mesh.file_id);
        if resolved {
            continue;
        }
        let nonzero = renderer
            .blend_shape_weights
            .as_deref()
            .unwrap_or_default()
            .iter()
            .any(|weight| *weight != 0.0);
        if nonzero {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!(
                    "基準姿勢の重みがメッシュに解決できない: fileID {}（{}）",
                    renderer.mesh.file_id,
                    crate::escape_diag(prefab_label),
                ),
            ));
        }
    }
    // 改変内容の未解決は、対象GUIDが入力FBXであり、非ゼロの成分を
    // 持つ場合だけ失敗させる。大きさだけでは重みが無いため無視する。
    let instance = find_fbx_instance(prefab, prefab_label, fbx_guid)?;
    let Some(instance_id) = instance else {
        return Ok(());
    };
    let Ok(modifications) = instance_modifications(prefab, prefab_label, instance_id) else {
        return Ok(());
    };
    for modification in &modifications {
        let guid_matches = modification
            .target
            .guid
            .as_ref()
            .is_some_and(|guid| guid.as_str().eq_ignore_ascii_case(fbx_guid));
        if !guid_matches {
            continue;
        }
        if !modification
            .property_path
            .starts_with("m_BlendShapeWeights.Array.data[")
        {
            continue;
        }
        if renderer_ids.contains_key(&modification.target.file_id) {
            // 出所の矛盾するsourceへの改変は、検証済み文書の除外を迂回するため、
            // 非ゼロの場合だけ失敗させる。ゼロ改変は寄与しないため無視する。
            if tainted_sources.contains(&modification.target.file_id) {
                let weight = mod_f64_value(
                    &modification.value,
                    prefab_label,
                    &modification.property_path,
                )
                .unwrap_or(f64::NAN);
                if weight != 0.0 {
                    return Err(VrmError::new(
                        VrmErrorKind::InvalidPrefab,
                        format!(
                            "基準姿勢の重みが出所の検証できない描画設定を指す: fileID {}（{}）",
                            modification.target.file_id,
                            crate::escape_diag(prefab_label),
                        ),
                    ));
                }
            }
            continue;
        }
        let weight = mod_f64_value(
            &modification.value,
            prefab_label,
            &modification.property_path,
        )
        .unwrap_or(f64::NAN);
        if weight != 0.0 {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!(
                    "基準姿勢の重みがメッシュに解決できない: fileID {}（{}）",
                    modification.target.file_id,
                    crate::escape_diag(prefab_label),
                ),
            ));
        }
    }
    // 参照が壊れている場合の非有限も非ゼロとして扱い、上で失敗させる。
    // `unwrap_or(NaN)` で `NaN != 0.0` が真になるためである。
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // テスト名は日本語で書く（テストは仕様・契約であるため）。

    /// 最小のPrefab本文を作る。部品は呼び出し側が渡す。
    fn 試験表prefabを読む(部品ら: &[Vec<u8>]) -> SerializedFile {
        let mut 本文 = b"%YAML 1.1\n%TAG !u! tag:unity3d.com,2011:\n".to_vec();
        for 部品 in 部品ら {
            本文.extend_from_slice(部品);
        }
        SerializedFile::parse(&本文, &crate::unity::ParseOptions::new())
            .expect("試験prefabは読める")
    }

    const 試験GUID: &str = "9ef94258554e06b4ab03fbe9fc914c5c";

    /// 直接記述SMR文書の部品を作る。重み列はYAMLの数値列になる。
    fn 直接文書(文書id: i64, 網id: i64, 網guid: Option<&str>, 重み: &str) -> Vec<u8> {
        let 網 = match 網guid {
            Some(guid) => format!("{{fileID: {網id}, guid: {guid}, type: 3}}"),
            None => format!("{{fileID: {網id}}}"),
        };
        format!(
            "--- !u!137 &{文書id}\n\
             SkinnedMeshRenderer:\n\
             \x20 m_ObjectHideFlags: 0\n\
             \x20 m_CorrespondingSourceObject: {{fileID: 0}}\n\
             \x20 m_PrefabInstance: {{fileID: 0}}\n\
             \x20 m_PrefabAsset: {{fileID: 0}}\n\
             \x20 m_GameObject: {{fileID: 1}}\n\
             \x20 m_Enabled: 1\n\
             \x20 m_Mesh: {網}\n\
             \x20 m_BlendShapeWeights: [{重み}]\n\
             \x20 m_Materials: []\n\
             \x20 m_Bones: []\n\
             \x20 m_RootBone: {{fileID: 0}}\n"
        )
        .into_bytes()
    }

    #[test]
    fn 改変欄パスの添字を読む() {
        // 正常・形違い・空の扱いを確かめる。
        assert_eq!(mod_data_index("m_BlendShapeWeights.Array.data[5]"), Some(5));
        assert_eq!(mod_data_index("m_BlendShapeWeights.Array.size"), None);
        assert_eq!(mod_data_index("m_BlendShapeWeights.Array.data[]"), None);
        assert_eq!(mod_data_index("m_BlendShapeWeights.Array.data[-1]"), None);
    }

    #[test]
    fn 大きさの数値だけを受け付ける() {
        // 整数は通り、負値・非数値・欠落は失敗する。
        assert_eq!(
            mod_size_value(&Some(Value::Number(3.into())), "試験", "大きさ").expect("通る"),
            3
        );
        assert!(mod_size_value(&Some(Value::Number((-1).into())), "試験", "大きさ").is_err());
        assert!(mod_size_value(&None, "試験", "大きさ").is_err());
    }

    #[test]
    fn 重みの数値だけを受け付ける() {
        // 整数・浮動小数は通り、真偽値・欠落は失敗する。
        assert_eq!(
            mod_f64_value(&Some(Value::Number(50.into())), "試験", "重み").expect("通る"),
            50.0
        );
        assert!(mod_f64_value(&Some(Value::Bool(true)), "試験", "重み").is_err());
        assert!(mod_f64_value(&None, "試験", "重み").is_err());
    }

    /// 疎な重みを検査用に実体化する。呼び出し側は小さな長さだけを指定する。
    fn 実体化(重み: &RestoredWeights, 長さ: usize) -> Vec<f64> {
        (0..長さ).map(|index| 重み.get(index)).collect()
    }

    /// 密な列から疎な重みを作る。小さな試験値専用である。
    fn 疎(列: &[f64]) -> RestoredWeights {
        RestoredWeights {
            size: Some(列.len()),
            values: 列
                .iter()
                .enumerate()
                .map(|(index, weight)| (index, *weight))
                .collect(),
        }
    }

    #[test]
    fn 同じ重みの候補は統合できる() {
        // 0詰めで長さが違っても、実効値が同じなら通る。
        let 統合 =
            unify_geometry_weights(&[疎(&[50.0, 0.0]), 疎(&[50.0])], ObjectId::new(10), "試験")
                .expect("一致する");
        assert_eq!(実体化(&統合, 2), vec![50.0, 0.0]);
    }

    #[test]
    fn 異なる重みの候補は先勝ちにしない() {
        // 同じ索引の実効値が違えば失敗する。
        let 結果 = unify_geometry_weights(&[疎(&[50.0]), 疎(&[25.0])], ObjectId::new(10), "試験");
        assert!(matches!(
            結果,
            Err(ref error) if error.kind() == VrmErrorKind::InvalidPrefab
        ));
    }

    #[test]
    fn ゼロ対非ゼロは一致しない() {
        // 同じ索引の実効値がゼロと非ゼロで違えば失敗する。
        let 結果 = unify_geometry_weights(&[疎(&[0.0]), 疎(&[25.0])], ObjectId::new(10), "試験");
        assert!(matches!(
            結果,
            Err(ref error) if error.kind() == VrmErrorKind::InvalidPrefab
        ));
    }

    #[test]
    fn 直接記述の重み欄を読める() {
        // 直接文書の部品がYAMLとして読め、重み列が残る。
        let prefab = 試験表prefabを読む(&[直接文書(600, 700, Some(試験GUID), "50, 0")]);
        let 文書 = prefab.document(600).expect("文書がある");
        let TypedObject::SkinnedMeshRenderer(描画) = 文書.decode().expect("読める") else {
            panic!("描画設定である");
        };
        assert_eq!(
            描画.blend_shape_weights,
            Some(vec![50.0, 0.0]),
            "重み列が残る",
        );
    }
}

#[cfg(test)]
mod 復元試験 {
    use super::*;
    use crate::unity::ObjectRef;

    // テスト名は日本語で書く（テストは仕様・契約であるため）。

    /// 改変1件を作る。値は呼び出し側がYAML値で渡す。
    fn 改変(
        対象: FileId,
        対象guid: Option<&str>,
        欄: &str,
        値: Option<Value>,
    ) -> crate::unity::PrefabModification {
        crate::unity::PrefabModification {
            target: ObjectRef {
                file_id: 対象,
                guid: 対象guid
                    .map(|guid| crate::unity::Guid::parse(guid).expect("試験GUIDは読める")),
                type_hint: Some(3),
            },
            property_path: 欄.to_owned(),
            value: 値,
            object_reference: None,
        }
    }

    /// 数値の改変値を作る。非有限もYAML値として持てる。
    fn 数値(値: f64) -> Option<Value> {
        Some(Value::Number(値.into()))
    }

    /// 整数の改変値を作る。
    fn 整数(値: i64) -> Option<Value> {
        Some(Value::Number(値.into()))
    }

    const 試験GUID: &str = "9ef94258554e06b4ab03fbe9fc914c5c";

    /// 既知の取り込みID表を作る。描画設定900がメッシュ10に対応する。
    fn 試験表() -> HashMap<FileId, ObjectId> {
        HashMap::from([(900, ObjectId::new(10))])
    }

    /// 疎な重みを検査用に実体化する。呼び出し側は小さな長さだけを指定する。
    fn 実体化(重み: &RestoredWeights, 長さ: usize) -> Vec<f64> {
        (0..長さ).map(|index| 重み.get(index)).collect()
    }

    /// 大きな整数の改変値を作る。YAMLのu64として持てる。
    fn 大整数(値: u64) -> Option<Value> {
        Some(Value::Number(値.into()))
    }

    #[test]
    fn 大きさと成分から配列を復元する() {
        // 大きさ2・成分0=10・成分1=20 → [10,20]。足りない索引はゼロである。
        let 改変ら = vec![
            改変(
                900,
                Some(試験GUID),
                "m_BlendShapeWeights.Array.size",
                整数(2),
            ),
            改変(
                900,
                Some(試験GUID),
                "m_BlendShapeWeights.Array.data[0]",
                数値(10.0),
            ),
            改変(
                900,
                Some(試験GUID),
                "m_BlendShapeWeights.Array.data[1]",
                数値(20.0),
            ),
        ];
        let 表 =
            restore_stripped_weights(&改変ら, &試験表(), 試験GUID, "試験").expect("復元できる");
        assert_eq!(実体化(&表[&900], 2), vec![10.0, 20.0]);
    }

    #[test]
    fn 範囲外の成分は拡張してゼロ埋めする() {
        // 大きさ0・成分3=40 → [0,0,0,40]。大きさ無しの成分1=30 → [0,30]。
        let 改変ら = vec![
            改変(
                900,
                Some(試験GUID),
                "m_BlendShapeWeights.Array.size",
                整数(0),
            ),
            改変(
                900,
                Some(試験GUID),
                "m_BlendShapeWeights.Array.data[3]",
                数値(40.0),
            ),
        ];
        let 表 =
            restore_stripped_weights(&改変ら, &試験表(), 試験GUID, "試験").expect("復元できる");
        assert_eq!(実体化(&表[&900], 4), vec![0.0, 0.0, 0.0, 40.0]);
        let 成分だけ = vec![改変(
            900,
            Some(試験GUID),
            "m_BlendShapeWeights.Array.data[1]",
            数値(30.0),
        )];
        let 表 =
            restore_stripped_weights(&成分だけ, &試験表(), 試験GUID, "試験").expect("復元できる");
        assert_eq!(実体化(&表[&900], 2), vec![0.0, 30.0]);
    }

    #[test]
    fn 重複する上書きは先勝ちにしない() {
        // 同じ対象・同じ欄パスが2件あれば失敗する。
        let 改変ら = vec![
            改変(
                900,
                Some(試験GUID),
                "m_BlendShapeWeights.Array.data[0]",
                数値(10.0),
            ),
            改変(
                900,
                Some(試験GUID),
                "m_BlendShapeWeights.Array.data[0]",
                数値(20.0),
            ),
        ];
        let 結果 = restore_stripped_weights(&改変ら, &試験表(), 試験GUID, "試験");
        assert!(matches!(
            結果,
            Err(ref error) if error.kind() == VrmErrorKind::InvalidPrefab
        ));
    }

    #[test]
    fn 別資産と未知の対象は入れない() {
        // GUID不一致・未知のfileIDは集計に入れない（非ゼロの失敗判定は呼び出し側で行う）。
        let 改変ら = vec![
            改変(
                900,
                Some("7464d0b6ac9485e4c9b53c7a0f61a372"),
                "m_BlendShapeWeights.Array.data[0]",
                数値(50.0),
            ),
            改変(
                901,
                Some(試験GUID),
                "m_BlendShapeWeights.Array.data[0]",
                数値(50.0),
            ),
        ];
        let 表 =
            restore_stripped_weights(&改変ら, &試験表(), 試験GUID, "試験").expect("無視できる");
        assert!(表.is_empty(), "別資産と未知の対象は入れない");
    }

    #[test]
    fn 巨大な大きさは実体化せず失敗する() {
        // `.Array.size=18446744073709551615` は上限超過で失敗し、確保しない。
        let 改変ら = vec![改変(
            900,
            Some(試験GUID),
            "m_BlendShapeWeights.Array.size",
            大整数(u64::MAX),
        )];
        let 結果 = restore_stripped_weights(&改変ら, &試験表(), 試験GUID, "試験");
        assert!(matches!(
            結果,
            Err(ref error) if error.kind() == VrmErrorKind::InvalidPrefab
        ));
    }

    #[test]
    fn 巨大な索引の非ゼロは失敗しゼロは無視する() {
        // `.data[18446744073709551615]=50` は上限超過で失敗する。
        // 同じ索引でも値がゼロなら寄与しないため無視して成功する。
        // 仕様の判断: ゼロ値は焼き込み・倍率・比較のいずれにも効かないため、
        // 巨大でも捨ててよく、非ゼロだけを資源超過として失敗させる。
        let 非ゼロ = vec![改変(
            900,
            Some(試験GUID),
            "m_BlendShapeWeights.Array.data[18446744073709551615]",
            数値(50.0),
        )];
        let 結果 = restore_stripped_weights(&非ゼロ, &試験表(), 試験GUID, "試験");
        assert!(matches!(
            結果,
            Err(ref error) if error.kind() == VrmErrorKind::InvalidPrefab
        ));
        let ゼロ = vec![改変(
            900,
            Some(試験GUID),
            "m_BlendShapeWeights.Array.data[18446744073709551615]",
            数値(0.0),
        )];
        let 表 = restore_stripped_weights(&ゼロ, &試験表(), 試験GUID, "試験")
            .expect("巨大索引のゼロは無視して成功する");
        assert!(
            表.get(&900)
                .is_none_or(|重み| 重み.values.values().all(|値| *値 == 0.0)),
            "巨大ゼロは保持しない"
        );
    }

    #[test]
    fn 大きさ無しの大きな索引は上限で区切る() {
        // 上限ちょうど（1000000）は通り、上限超え（1000001）の非ゼロは失敗する。
        let 境界 = vec![改変(
            900,
            Some(試験GUID),
            "m_BlendShapeWeights.Array.data[1000000]",
            数値(50.0),
        )];
        restore_stripped_weights(&境界, &試験表(), 試験GUID, "試験").expect("上限ちょうどは通る");
        let 超過 = vec![改変(
            900,
            Some(試験GUID),
            "m_BlendShapeWeights.Array.data[1000001]",
            数値(50.0),
        )];
        let 結果 = restore_stripped_weights(&超過, &試験表(), 試験GUID, "試験");
        assert!(matches!(
            結果,
            Err(ref error) if error.kind() == VrmErrorKind::InvalidPrefab
        ));
    }

    #[test]
    fn 異常な重みと大きさは失敗する() {
        // 負値・100超・非有限・負の大きさ・非数値は失敗する。
        for 値 in [数値(-1.0), 数値(101.0), 数値(f64::NAN), 整数(-1), None] {
            let 改変ら = if 値.is_none() {
                vec![改変(
                    900,
                    Some(試験GUID),
                    "m_BlendShapeWeights.Array.data[0]",
                    None,
                )]
            } else {
                vec![改変(
                    900,
                    Some(試験GUID),
                    "m_BlendShapeWeights.Array.data[0]",
                    値,
                )]
            };
            let 結果 = restore_stripped_weights(&改変ら, &試験表(), 試験GUID, "試験");
            assert!(
                matches!(
                    結果,
                    Err(ref error) if error.kind() == VrmErrorKind::InvalidPrefab
                ),
                "異常な重みは失敗する"
            );
        }
        let 大きさ = vec![改変(
            900,
            Some(試験GUID),
            "m_BlendShapeWeights.Array.size",
            整数(-1),
        )];
        let 結果 = restore_stripped_weights(&大きさ, &試験表(), 試験GUID, "試験");
        assert!(matches!(
            結果,
            Err(ref error) if error.kind() == VrmErrorKind::InvalidPrefab
        ));
    }
}
