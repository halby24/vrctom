//! メッシュ幾何のベイク・三角形化・頂点統合・重み解決。
//!
//! 所有モデルのバインド世界行列で頂点をシーン空間へ移し、多角形をfan三角形化し、
//! 材質番号ごとにプリミティブへ分ける。頂点は位置・法線・UVの組で統合し、
//! スキンがある場合は制御点ごとに最大4件の重みを正規化して載せる。

use std::collections::{BTreeMap, HashMap};

use super::error::{VrmError, VrmErrorKind};
use super::matrix;
use super::{VrmMesh, VrmMorphTarget, VrmPrimitive, VrmVertices};
use crate::fbx::{FbxFile, Mesh as FbxMesh, MeshData, Model as FbxModel, ObjectId, SkinData};

/// メッシュ1件の変換結果と集計。
///
/// 件数は [`super::VrmReport`] へ積むための内訳であり、呼び出し側で合計する。
pub(crate) struct MeshOutcome {
    /// 変換後のメッシュ。
    pub mesh: VrmMesh,
    /// 出力した三角形数。
    pub triangles: usize,
    /// 4件を超える影響を落とした頂点数。
    pub truncated: usize,
    /// 重みが無く既定値を与えた頂点数。
    pub unweighted: usize,
    /// 三角形でない多角形の数。
    pub non_triangles: usize,
    /// 長さ0の法線を面法線で補った頂点数。
    pub degenerate_normals: usize,
    /// 鏡像で巻き方向を反転したかどうか。
    pub mirrored: bool,
    /// 基準姿勢として焼き込んだ非ゼロチャンネル数。
    pub baseline_shapes: usize,
}

/// 頂点統合の鍵。同じ組のcornerは1頂点にまとめる。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct VertexKey {
    /// 位置索引。
    pos: u32,
    /// 法線索引。法線層が無い場合は `None` である。
    normal: Option<u32>,
    /// UVセットごとの索引。セットが無い場合は空である。
    uvs: Vec<u32>,
}

/// 選択済みのモーフチャンネル。対象メッシュにだけ渡す。
pub(crate) struct SelectedMorph<'a> {
    /// 選択順のチャンネル。名前と形状デルタを持つ。
    pub channels: &'a [SelectedMorphChannel<'a>],
    /// モーフ全体のバイト数上限。確保前に検査する。
    pub max_morph_bytes: usize,
    /// 1メッシュのターゲット数上限。確保前に検査する。
    pub max_morph_targets: usize,
}

/// 選択済みのモーフチャンネル1件。
pub(crate) struct SelectedMorphChannel<'a> {
    /// チャンネル名。ターゲット名になる。
    pub name: &'a str,
    /// チャンネルのObjectId。基準姿勢の倍率引きに使う。
    pub id: ObjectId,
    /// 形状デルタ。
    pub data: &'a crate::fbx::ShapeData,
}

/// メッシュ1件を変換する。
///
/// `bind` は所有モデルのバインド世界行列に幾何変換を掛けた行列であり、頂点のベイクに使う。
/// `skin` は対応するスキンの索引とその展開結果であり、無い場合は重みを載せない。
/// `node` は所有モデルのノード索引である。
/// `morph` は対象メッシュの選択済みチャンネルであり、ある場合は位置の差分を
/// 出力頂点へ配ってターゲットを作る。対象外のメッシュには `None` を渡す。
/// `baseline` はそのメッシュ分の基準姿勢の重みであり、空の場合は焼き込まない。
/// 重みはUnityの0..=100であり、呼び出し側が解決した適用値そのものを受け取る。
// 入力の構成要素が多く、構造体へまとめると呼び出し側で同じ値を組み直すだけになる。
#[allow(clippy::too_many_arguments)]
pub(crate) fn convert_mesh(
    file: &FbxFile<'_>,
    mesh: &FbxMesh,
    data: &MeshData,
    owner: &FbxModel,
    node: usize,
    bind: &[f64; 16],
    skin: Option<(usize, &SkinData)>,
    morph: Option<SelectedMorph<'_>>,
    baseline: &[super::BaselineChannel],
) -> Result<MeshOutcome, VrmError> {
    if matrix::det3(bind) == 0.0 {
        return Err(VrmError::new(
            VrmErrorKind::InvalidStructure,
            "バインド行列の3x3部分が特異である",
        )
        .with_object(mesh.id));
    }
    // 拡大縮小に鏡像が含まれると面の表裏が逆になるため、巻き方向を反転する。
    let mirrored = matrix::det3(bind) < 0.0;
    let influences = skin
        .map(|(_, skin_data)| collect_influences(mesh, skin_data))
        .transpose()?;

    let mut key_to_vertex: HashMap<VertexKey, u32> = HashMap::new();
    let mut positions: Vec<[f32; 3]> = Vec::new();
    // モーフ対象または基準姿勢の焼き込み対象のメッシュだけ、
    // 出力頂点ごとの制御点番号を残す。差分は制御点単位で書かれるため、
    // UVや法線の継ぎ目で複製された頂点へ配る写像として使う。対象外では確保しない。
    let keep_control_points = morph.is_some() || !baseline.is_empty();
    let mut control_points: Option<Vec<u32>> = keep_control_points.then(Vec::new);
    let mut normals: Option<Vec<[f32; 3]>> = data.normals.as_ref().map(|_| Vec::new());
    let mut uv_sets: Vec<Vec<[f32; 2]>> = data.uv_sets.iter().map(|_| Vec::new()).collect();
    let mut joints_out: Option<Vec<[u16; 4]>> = influences.as_ref().map(|_| Vec::new());
    let mut weights_out: Option<Vec<[f32; 4]>> = influences.as_ref().map(|_| Vec::new());
    let mut truncated = 0usize;
    let mut unweighted = 0usize;
    let mut non_triangles = 0usize;
    let mut degenerate_normals = 0usize;
    // 材質番号ごとの索引列。番号の昇順でプリミティブにする。
    let mut by_material: BTreeMap<u32, Vec<u32>> = BTreeMap::new();

    let mut corner_base = 0usize;
    let slot_count = owner.materials.len();
    for (face, &count) in data.polygon_corner_counts.iter().enumerate() {
        let count = count as usize;
        // 読み取り層でも3未満は拒否するが、直接渡された幾何に備えてここでも検査する。
        if count < 3 {
            return Err(
                VrmError::new(VrmErrorKind::InvalidStructure, "3未満の多角形がある")
                    .with_object(mesh.id),
            );
        }
        let material = data.material_indices.get(face).copied().unwrap_or(0);
        // スロットが無い場合は材質番号0だけを受け付け、ある場合は範囲内を要求する。
        let out_of_range = if slot_count == 0 {
            material != 0
        } else {
            (material as usize) >= slot_count
        };
        if out_of_range {
            return Err(VrmError::new(
                VrmErrorKind::InvalidStructure,
                "材質番号がスロット範囲外である",
            )
            .with_object(mesh.id));
        }
        if count != 3 {
            non_triangles += 1;
        }
        // 面法線は長さ0の法線の代替に使う。面が退化している場合は `None` になる。
        let face_normal = face_normal_of(data, corner_base, count)
            .and_then(|n| matrix::transform_normal(bind, n));
        let mut face_vertices = Vec::with_capacity(count);
        for offset in 0..count {
            let corner = corner_base + offset;
            let key = corner_key(data, corner)?;
            let vertex = match key_to_vertex.get(&key) {
                Some(&vertex) => vertex,
                None => {
                    let vertex = push_vertex(
                        data,
                        mesh.id,
                        bind,
                        corner,
                        &mut positions,
                        normals.as_mut(),
                        &mut uv_sets,
                        &mut joints_out,
                        &mut weights_out,
                        &mut truncated,
                        &mut unweighted,
                        &mut degenerate_normals,
                        face_normal,
                        influences.as_ref(),
                        control_points.as_mut(),
                    )?;
                    key_to_vertex.insert(key, vertex);
                    vertex
                }
            };
            face_vertices.push(vertex);
        }
        // fan三角形化であり、元の多角形の頂点順を保つ。
        let indices = by_material.entry(material).or_default();
        for i in 1..count.saturating_sub(1) {
            if mirrored {
                indices.extend_from_slice(&[
                    face_vertices[0],
                    face_vertices[i + 1],
                    face_vertices[i],
                ]);
            } else {
                indices.extend_from_slice(&[
                    face_vertices[0],
                    face_vertices[i],
                    face_vertices[i + 1],
                ]);
            }
        }
        corner_base += count;
    }

    let primitives = by_material
        .into_iter()
        .map(|(material, indices)| VrmPrimitive {
            material: material as usize,
            indices,
        })
        .collect::<Vec<_>>();
    let triangles = primitives.iter().map(|p| p.indices.len() / 3).sum();
    // 基準姿勢は位置を組み終えた後に焼き込む。ベースには非ゼロ重みの全チャンネル分を
    // 累積し、各ターゲットからは自身の分だけを引くための倍率表を作る。
    // 位置の焼き込みはf64で累積し、最後にf32へ寄せて非有限を検査する。
    let (baseline_scales, baseline_shapes) = match control_points.as_ref() {
        Some(control_points) if !baseline.is_empty() => apply_baseline(
            file,
            mesh.id,
            &mut positions,
            control_points,
            bind,
            baseline,
        )?,
        _ => (HashMap::new(), 0),
    };
    let targets = match (morph, control_points.as_ref()) {
        (Some(selected), Some(control_points)) => build_morph_targets(
            mesh.id,
            &positions,
            control_points,
            bind,
            &selected,
            &baseline_scales,
        )?,
        _ => Vec::new(),
    };
    let material_slots = owner
        .materials
        .iter()
        .map(|id| {
            file.material(*id)
                .map(|material| material.name.clone())
                .ok_or_else(|| {
                    VrmError::new(VrmErrorKind::InvalidStructure, "材質スロットの材質がない")
                        .with_object(owner.id)
                })
        })
        .collect::<Result<Vec<_>, _>>()?;

    Ok(MeshOutcome {
        mesh: VrmMesh {
            node,
            geometry: mesh.id,
            model: owner.id,
            name: mesh.name.clone(),
            skin: skin.map(|(index, _)| index),
            material_slots,
            vertices: VrmVertices {
                positions,
                normals,
                uv_sets,
                joints: joints_out,
                weights: weights_out,
            },
            primitives,
            targets,
        },
        triangles,
        truncated,
        unweighted,
        non_triangles,
        degenerate_normals,
        mirrored,
        baseline_shapes,
    })
}

/// 重み率と初期値の判定許容誤差。
/// 中間モデル側の `FULL_WEIGHT_TOLERANCE` と同じ値であり、実データの丸めを吸収する。
const FULL_WEIGHT_TOLERANCE: f64 = 1e-6;

/// 制御点ごとの位置差分を線形変換して写像にまとめる。
/// ベース焼き込みとターゲット生成で共用し、二重実装にしない。
///
/// 差分の変換は線形部分だけを使い、平行移動は掛けない。
/// 入力・結果の非有限は破損として失敗する。
fn transformed_deltas(
    mesh_id: ObjectId,
    bind: &[f64; 16],
    data: &crate::fbx::ShapeData,
) -> Result<HashMap<u32, [f64; 3]>, VrmError> {
    let mut deltas: HashMap<u32, [f64; 3]> =
        HashMap::with_capacity(data.control_point_indices.len());
    for (control_point, delta) in data
        .control_point_indices
        .iter()
        .zip(data.position_deltas.iter())
    {
        let moved = matrix::transform_vector(bind, *delta).ok_or_else(|| {
            VrmError::new(VrmErrorKind::InvalidStructure, "差分の変換に失敗した")
                .with_object(mesh_id)
        })?;
        // `transform_vector` は非有限の入出力を `None` にするため、
        // ここに来る値は有限である。念のため不変条件として確かめる。
        if !moved.iter().all(|v| v.is_finite()) {
            return Err(
                VrmError::new(VrmErrorKind::InvalidStructure, "有限でない差分がある")
                    .with_object(mesh_id),
            );
        }
        deltas.insert(*control_point, moved);
    }
    Ok(deltas)
}

/// 単一フレーム条件と重み率をkeptチャンネルと同じ規則で確かめる。
/// 基準姿勢の適用値の検証に使い、違反は `UnsupportedBlendShape` で返す。
/// 初期値（`DeformPercent`）は見ない。ベース焼き込みでは初期値を使わないためである。
fn check_baseline_single_frame(
    channel: &crate::fbx::BlendShapeChannel,
    file: &FbxFile<'_>,
) -> Result<(), VrmError> {
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
        let weights = file.blend_channel_weights(channel.id).map_err(|error| {
            VrmError::new(VrmErrorKind::InvalidStructure, error.to_string()).with_object(channel.id)
        })?;
        let all_full = weights
            .iter()
            .all(|value| value.is_finite() && (*value - 100.0).abs() <= FULL_WEIGHT_TOLERANCE);
        if !all_full {
            return Err(VrmError::new(
                VrmErrorKind::UnsupportedBlendShape,
                format!("重み率が全100でないチャンネルがある: {}", channel.name),
            )
            .with_object(channel.id));
        }
    }
    Ok(())
}

/// 基準姿勢の重みを検証する。非有限・負値・100超は構造破損として失敗する。
fn check_baseline_weight(weight: f64, channel_id: ObjectId) -> Result<(), VrmError> {
    if !weight.is_finite() || !(0.0..=100.0).contains(&weight) {
        return Err(VrmError::new(
            VrmErrorKind::InvalidStructure,
            "基準姿勢の重みが範囲外である",
        )
        .with_object(channel_id));
    }
    Ok(())
}

/// ベース形状へ基準姿勢を焼き込み、ターゲット用の倍率表を返す。
///
/// ベースには非ゼロ重みの全チャンネル分を `(重み/100) * 差分` で累積する。
/// 戻り値の写像はチャンネルIDから `1 - 重み/100` であり、
/// `build_morph_targets` が該当ターゲットの各成分へ掛ける。
/// 重み0の項目は検証も展開もせず、件数にも数えない。
/// 非ゼロ重みのチャンネルだけ `shape_data` を読み、全チャンネルを展開しない。
/// 焼き込み後の位置はf32へ寄せ、非有限は破損として失敗する。
fn apply_baseline(
    file: &FbxFile<'_>,
    mesh_id: ObjectId,
    positions: &mut [[f32; 3]],
    control_points: &[u32],
    bind: &[f64; 16],
    baseline: &[super::BaselineChannel],
) -> Result<(HashMap<ObjectId, f64>, usize), VrmError> {
    let mut scales: HashMap<ObjectId, f64> = HashMap::new();
    // f64で累積し、丸めは最後に1度だけ行う。
    let mut accumulated = vec![[0.0; 3]; positions.len()];
    let mut applied = 0usize;
    // 決定的な診断のため、入力順を保って処理する。
    for entry in baseline {
        if entry.weight == 0.0 {
            continue;
        }
        check_baseline_weight(entry.weight, entry.channel)?;
        let channels = file.blend_shape_channels(mesh_id);
        let Some(channel) = channels.iter().find(|channel| channel.id == entry.channel) else {
            return Err(VrmError::new(
                VrmErrorKind::InvalidStructure,
                "基準姿勢のチャンネルがメッシュに無い",
            )
            .with_object(entry.channel));
        };
        check_baseline_single_frame(channel, file)?;
        let data = file.shape_data(channel.shapes[0].shape).map_err(|error| {
            VrmError::new(VrmErrorKind::InvalidStructure, error.to_string())
                .with_object(channel.shapes[0].shape)
        })?;
        let deltas = transformed_deltas(mesh_id, bind, &data)?;
        let factor = entry.weight / 100.0;
        for (vertex, control_point) in control_points.iter().enumerate() {
            if let Some(delta) = deltas.get(control_point) {
                accumulated[vertex][0] += factor * delta[0];
                accumulated[vertex][1] += factor * delta[1];
                accumulated[vertex][2] += factor * delta[2];
            }
        }
        scales.insert(entry.channel, 1.0 - factor);
        applied += 1;
    }
    for (vertex, position) in positions.iter_mut().enumerate() {
        let baked = [
            f64::from(position[0]) + accumulated[vertex][0],
            f64::from(position[1]) + accumulated[vertex][1],
            f64::from(position[2]) + accumulated[vertex][2],
        ];
        let baked32 = [baked[0] as f32, baked[1] as f32, baked[2] as f32];
        if !baked32.iter().all(|v| v.is_finite()) {
            return Err(
                VrmError::new(VrmErrorKind::InvalidStructure, "有限でない位置がある")
                    .with_object(mesh_id),
            );
        }
        *position = baked32;
    }
    Ok((scales, applied))
}

/// 選択済みチャンネルからモーフターゲット列を作る。
///
/// 差分の確保の前に、ターゲット数と総バイト数を上限検査する。
/// 差分は制御点番号から出力頂点への写像で配り、継ぎ目の複製すべてに同じ差分を載せる。
/// 差分の変換は線形部分だけを使い、平行移動は掛けない。
fn build_morph_targets(
    mesh_id: ObjectId,
    positions: &[[f32; 3]],
    control_points: &[u32],
    bind: &[f64; 16],
    selected: &SelectedMorph<'_>,
    baseline_scales: &HashMap<ObjectId, f64>,
) -> Result<Vec<VrmMorphTarget>, VrmError> {
    if selected.channels.len() > selected.max_morph_targets {
        return Err(VrmError::new(
            VrmErrorKind::LimitExceeded,
            "モーフターゲット数が上限を超えた",
        )
        .with_object(mesh_id));
    }
    let bytes = selected
        .channels
        .len()
        .checked_mul(positions.len())
        .and_then(|count| count.checked_mul(12))
        .unwrap_or(usize::MAX);
    if bytes > selected.max_morph_bytes {
        return Err(VrmError::new(
            VrmErrorKind::LimitExceeded,
            "モーフターゲットの量が上限を超えた",
        )
        .with_object(mesh_id));
    }
    let mut targets = Vec::with_capacity(selected.channels.len());
    for channel in selected.channels {
        // 差分の変換は [`transformed_deltas`] と共用する。掛け算はf64で行い、
        // 出力前にf32へ寄せ、非有限は破損として失敗する。
        let source = transformed_deltas(mesh_id, bind, channel.data)?;
        // 基準姿勢で焼き込んだ分だけターゲットを縮める。該当しない場合は1.0である。
        let scale = baseline_scales.get(&channel.id).copied().unwrap_or(1.0);
        let mut deltas: HashMap<u32, [f32; 3]> =
            HashMap::with_capacity(channel.data.control_point_indices.len());
        for (control_point, moved) in &source {
            let scaled = [moved[0] * scale, moved[1] * scale, moved[2] * scale];
            let moved32 = [scaled[0] as f32, scaled[1] as f32, scaled[2] as f32];
            if !moved32.iter().all(|v| v.is_finite()) {
                return Err(
                    VrmError::new(VrmErrorKind::InvalidStructure, "有限でない差分がある")
                        .with_object(mesh_id),
                );
            }
            deltas.insert(*control_point, moved32);
        }
        let mut out = vec![[0.0f32; 3]; positions.len()];
        for (vertex, control_point) in control_points.iter().enumerate() {
            if let Some(delta) = deltas.get(control_point) {
                out[vertex] = *delta;
            }
        }
        targets.push(VrmMorphTarget {
            name: channel.name.to_owned(),
            positions: out,
        });
    }
    Ok(targets)
}

/// cornerから頂点統合の鍵を作る。
fn corner_key(data: &MeshData, corner: usize) -> Result<VertexKey, VrmError> {
    let pos = *data.corner_positions.get(corner).ok_or_else(|| {
        VrmError::new(
            VrmErrorKind::InvalidStructure,
            "corner列が多角形数と一致しない",
        )
    })?;
    let normal = data
        .normals
        .as_ref()
        .map(|normals| {
            normals.corner_indices.get(corner).copied().ok_or_else(|| {
                VrmError::new(
                    VrmErrorKind::InvalidStructure,
                    "法線索引がcorner数と一致しない",
                )
            })
        })
        .transpose()?;
    let mut uvs = Vec::with_capacity(data.uv_sets.len());
    for set in &data.uv_sets {
        uvs.push(set.corner_indices.get(corner).copied().ok_or_else(|| {
            VrmError::new(
                VrmErrorKind::InvalidStructure,
                "UV索引がcorner数と一致しない",
            )
        })?);
    }
    Ok(VertexKey { pos, normal, uvs })
}

/// 面の法線をNewell法で求める。ファイル空間の位置から計算する。
///
/// 呼び出し側が所有モデルの行列で変換するため、ここでは正規化しない。
/// 面が退化している（面積が0）場合や非有限の場合は `None` を返す。
fn face_normal_of(data: &MeshData, corner_base: usize, count: usize) -> Option<[f64; 3]> {
    let mut normal = [0.0; 3];
    for offset in 0..count {
        let current = data
            .positions
            .get(*data.corner_positions.get(corner_base + offset)? as usize)?;
        let next = data.positions.get(
            *data
                .corner_positions
                .get(corner_base + (offset + 1) % count)? as usize,
        )?;
        if !current.iter().chain(next.iter()).all(|v| v.is_finite()) {
            return None;
        }
        normal[0] += (current[1] - next[1]) * (current[2] + next[2]);
        normal[1] += (current[2] - next[2]) * (current[0] + next[0]);
        normal[2] += (current[0] - next[0]) * (current[1] + next[1]);
    }
    let length = (normal[0] * normal[0] + normal[1] * normal[1] + normal[2] * normal[2]).sqrt();
    if length == 0.0 || !length.is_finite() {
        return None;
    }
    Some(normal)
}

/// 新しい頂点を追加し、属性をベイクして索引を返す。
#[allow(clippy::too_many_arguments)]
fn push_vertex(
    data: &MeshData,
    mesh_id: ObjectId,
    bind: &[f64; 16],
    corner: usize,
    positions: &mut Vec<[f32; 3]>,
    normals: Option<&mut Vec<[f32; 3]>>,
    uv_sets: &mut [Vec<[f32; 2]>],
    joints_out: &mut Option<Vec<[u16; 4]>>,
    weights_out: &mut Option<Vec<[f32; 4]>>,
    truncated: &mut usize,
    unweighted: &mut usize,
    degenerate_normals: &mut usize,
    face_normal: Option<[f64; 3]>,
    influences: Option<&PerPoint>,
    control_points: Option<&mut Vec<u32>>,
) -> Result<u32, VrmError> {
    let pos_index = data.corner_positions[corner] as usize;
    let raw = data.positions.get(pos_index).ok_or_else(|| {
        VrmError::new(VrmErrorKind::InvalidStructure, "位置索引が範囲外である").with_object(mesh_id)
    })?;
    if !raw.iter().all(|v| v.is_finite()) {
        return Err(
            VrmError::new(VrmErrorKind::InvalidStructure, "有限でない位置がある")
                .with_object(mesh_id),
        );
    }
    let baked = matrix::transform_point(bind, *raw).ok_or_else(|| {
        VrmError::new(VrmErrorKind::InvalidStructure, "位置のベイクに失敗した").with_object(mesh_id)
    })?;
    let baked32 = [baked[0] as f32, baked[1] as f32, baked[2] as f32];
    if !baked32.iter().all(|v| v.is_finite()) {
        return Err(
            VrmError::new(VrmErrorKind::InvalidStructure, "有限でない位置がある")
                .with_object(mesh_id),
        );
    }
    positions.push(baked32);
    if let Some(control_points) = control_points {
        let index = u32::try_from(pos_index).map_err(|_| {
            VrmError::new(
                VrmErrorKind::InvalidStructure,
                "制御点番号がu32の範囲を超えた",
            )
            .with_object(mesh_id)
        })?;
        control_points.push(index);
    }

    if let (Some(layer), Some(out)) = (data.normals.as_ref(), normals) {
        let normal_index = layer.corner_indices[corner] as usize;
        let raw = layer.values.get(normal_index).ok_or_else(|| {
            VrmError::new(VrmErrorKind::InvalidStructure, "法線索引が範囲外である")
                .with_object(mesh_id)
        })?;
        // 長さ0の法線は方向を持たないため、属する面の法線で補う。
        // 実データ（購入アバター）に含まれる退化であり、捏造を避ける最終手段である。
        // 件数はレポートに残す。面自体が退化している場合と非ゼロの変換失敗は
        // 方向を決められないため従来どおり失敗とする。
        if raw == &[0.0, 0.0, 0.0] {
            let Some(fallback) = face_normal else {
                return Err(VrmError::new(
                    VrmErrorKind::InvalidStructure,
                    "面法線の算出に失敗した",
                )
                .with_object(mesh_id));
            };
            *degenerate_normals += 1;
            out.push([fallback[0] as f32, fallback[1] as f32, fallback[2] as f32]);
        } else {
            let baked = matrix::transform_normal(bind, *raw).ok_or_else(|| {
                VrmError::new(VrmErrorKind::InvalidStructure, "法線のベイクに失敗した")
                    .with_object(mesh_id)
            })?;
            out.push([baked[0] as f32, baked[1] as f32, baked[2] as f32]);
        }
    }
    for (set, out) in data.uv_sets.iter().zip(uv_sets.iter_mut()) {
        let uv_index = set.corner_indices[corner] as usize;
        let raw = set.values.get(uv_index).ok_or_else(|| {
            VrmError::new(VrmErrorKind::InvalidStructure, "UV索引が範囲外である")
                .with_object(mesh_id)
        })?;
        if !raw.iter().all(|v| v.is_finite()) {
            return Err(
                VrmError::new(VrmErrorKind::InvalidStructure, "有限でないUVがある")
                    .with_object(mesh_id),
            );
        }
        let baked = [raw[0] as f32, raw[1] as f32];
        if !baked.iter().all(|v| v.is_finite()) {
            return Err(
                VrmError::new(VrmErrorKind::InvalidStructure, "有限でないUVがある")
                    .with_object(mesh_id),
            );
        }
        out.push(baked);
    }

    if let (Some(per_point), Some(joints), Some(weights)) =
        (influences, joints_out.as_mut(), weights_out.as_mut())
    {
        let (joint, weight, was_truncated, was_unweighted) = resolve_weights(per_point, pos_index);
        if was_truncated {
            *truncated += 1;
        }
        if was_unweighted {
            *unweighted += 1;
        }
        joints.push(joint);
        weights.push(weight);
    }

    u32::try_from(positions.len() - 1).map_err(|_| {
        VrmError::new(VrmErrorKind::InvalidStructure, "頂点数が索引範囲を超えた")
            .with_object(mesh_id)
    })
}

/// 制御点ごとの影響列。`(重み, 関節列への索引)` の降順である。
type PerPoint = Vec<Vec<(f64, usize)>>;

/// 制御点ごとに影響を集め、重みの降順に並べる。
///
/// 重み0は無視し、負値は0に丸める（実データの微小な負値を吸収するため）。
fn collect_influences(mesh: &FbxMesh, skin_data: &SkinData) -> Result<PerPoint, VrmError> {
    if skin_data.clusters.len() > u16::MAX as usize {
        return Err(
            VrmError::new(VrmErrorKind::InvalidStructure, "関節列がu16の範囲を超えた")
                .with_object(mesh.id),
        );
    }
    let vertex_count = mesh.vertex_count;
    let mut per_point: PerPoint = vec![Vec::new(); vertex_count];
    for (column, cluster) in skin_data.clusters.iter().enumerate() {
        for (vertex, weight) in cluster.vertex_indices.iter().zip(cluster.weights.iter()) {
            if !weight.is_finite() {
                return Err(
                    VrmError::new(VrmErrorKind::InvalidStructure, "有限でない重みがある")
                        .with_object(mesh.id),
                );
            }
            let mut weight = *weight;
            if weight == 0.0 {
                continue;
            }
            if weight < 0.0 {
                weight = 0.0;
            }
            if weight == 0.0 {
                continue;
            }
            let slot = per_point.get_mut(*vertex as usize).ok_or_else(|| {
                VrmError::new(
                    VrmErrorKind::InvalidStructure,
                    "重みの頂点番号が範囲外である",
                )
                .with_object(mesh.id)
            })?;
            slot.push((weight, column));
        }
    }
    for slot in per_point.iter_mut() {
        // 重みの降順であり、同値は関節列の順で決定的にする。
        slot.sort_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.1.cmp(&b.1))
        });
    }
    Ok(per_point)
}

/// 頂点1件の関節と重みを求める。
///
/// 最大4件を残し、合計が1になるよう正規化する。影響が無い場合は
/// 関節0・重み1の既定値を与える。
fn resolve_weights(per_point: &PerPoint, pos_index: usize) -> ([u16; 4], [f32; 4], bool, bool) {
    let slot = per_point.get(pos_index).map(Vec::as_slice).unwrap_or(&[]);
    let truncated = slot.len() > 4;
    let top = if slot.len() > 4 { &slot[..4] } else { slot };
    let sum: f64 = top.iter().map(|(weight, _)| weight).sum();
    if top.is_empty() || sum == 0.0 {
        return ([0, 0, 0, 0], [1.0, 0.0, 0.0, 0.0], truncated, true);
    }
    let mut joints = [0u16; 4];
    let mut weights = [0.0f32; 4];
    for (i, (weight, column)) in top.iter().enumerate() {
        // 列数は事前にu16範囲を確認しているため、変換は失敗しない。
        joints[i] = *column as u16;
        weights[i] = (weight / sum) as f32;
    }
    (joints, weights, truncated, false)
}
