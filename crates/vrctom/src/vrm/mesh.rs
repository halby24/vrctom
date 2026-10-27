//! メッシュ幾何のベイク・三角形化・頂点統合・重み解決。
//!
//! 所有モデルのバインド世界行列で頂点をシーン空間へ移し、多角形をfan三角形化し、
//! 材質番号ごとにプリミティブへ分ける。頂点は位置・法線・UVの組で統合し、
//! スキンがある場合は制御点ごとに最大4件の重みを正規化して載せる。

use std::collections::{BTreeMap, HashMap};

use super::error::{VrmError, VrmErrorKind};
use super::matrix;
use super::{VrmMesh, VrmPrimitive, VrmVertices};
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
    /// 鏡像で巻き方向を反転したかどうか。
    pub mirrored: bool,
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

/// メッシュ1件を変換する。
///
/// `bind` は所有モデルのバインド世界行列であり、頂点のベイクに使う。
/// `skin` は対応するスキンの索引とその展開結果であり、無い場合は重みを載せない。
/// `node` は所有モデルのノード索引である。
pub(crate) fn convert_mesh(
    file: &FbxFile<'_>,
    mesh: &FbxMesh,
    data: &MeshData,
    owner: &FbxModel,
    node: usize,
    bind: &[f64; 16],
    skin: Option<(usize, &SkinData)>,
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
    let mut normals: Option<Vec<[f32; 3]>> = data.normals.as_ref().map(|_| Vec::new());
    let mut uv_sets: Vec<Vec<[f32; 2]>> = data.uv_sets.iter().map(|_| Vec::new()).collect();
    let mut joints_out: Option<Vec<[u16; 4]>> = influences.as_ref().map(|_| Vec::new());
    let mut weights_out: Option<Vec<[f32; 4]>> = influences.as_ref().map(|_| Vec::new());
    let mut truncated = 0usize;
    let mut unweighted = 0usize;
    let mut non_triangles = 0usize;
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
                        influences.as_ref(),
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
        },
        triangles,
        truncated,
        unweighted,
        non_triangles,
        mirrored,
    })
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
    influences: Option<&PerPoint>,
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

    if let (Some(layer), Some(out)) = (data.normals.as_ref(), normals) {
        let normal_index = layer.corner_indices[corner] as usize;
        let raw = layer.values.get(normal_index).ok_or_else(|| {
            VrmError::new(VrmErrorKind::InvalidStructure, "法線索引が範囲外である")
                .with_object(mesh_id)
        })?;
        let baked = matrix::transform_normal(bind, *raw).ok_or_else(|| {
            VrmError::new(VrmErrorKind::InvalidStructure, "法線のベイクに失敗した")
                .with_object(mesh_id)
        })?;
        out.push([baked[0] as f32, baked[1] as f32, baked[2] as f32]);
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
