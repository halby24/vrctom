//! VRM 1.0のGLBバイト列への書き出し。
//!
//! 入口は [`VrmModel::to_vrm`](crate::vrm::VrmModel::to_vrm) であり、
//! [`VrmMeta`] と [`HumanoidBones`](super::humanoid::HumanoidBones) の検証から
//! glTF JSON（`VRMC_vrm` 拡張付き）と単一バッファを組み立て、GLBの容器は自前で書く。
//! `gltf` クレートのimport機能は使わない（画像デコーダを依存に入れないため）。

use std::collections::{BTreeMap, HashMap};

use super::VrmModel;
use super::humanoid::{HumanoidBone, HumanoidBones, allowed_parents_for};
use super::matrix;
use crate::escape_diag;

/// VRM 1.0の既定ライセンスURL。
const DEFAULT_LICENSE_URL: &str = "https://vrm.dev/licenses/1.0/";

/// 出力バイト数の既定上限（2GiB）。
///
/// 単一GLBとして配布できる大きさの目安であり、破損由来の膨張を抑える。
/// 実データの数百MBが収まる大きさにし、上限の変更は `with_` で始まるメソッドで行う。
const DEFAULT_MAX_OUTPUT_BYTES: usize = 2 * 1024 * 1024 * 1024;

/// VRMメタ情報。
#[derive(Debug, Clone)]
pub struct VrmMeta {
    /// 作品名。空は受け付けない。
    pub name: String,
    /// バージョン。`Some` の場合だけ書く。
    pub version: Option<String>,
    /// 作者列。空でない要素が1件以上必要である。
    pub authors: Vec<String>,
    /// ライセンスURL。空は受け付けない。
    pub license_url: String,
    /// 著作権表示。`Some` の場合だけ書く。
    pub copyright_information: Option<String>,
    /// 連絡先。`Some` の場合だけ書く。
    pub contact_information: Option<String>,
    /// 参照。空でない場合だけ書く。
    pub references: Vec<String>,
    /// 第三者ライセンス。`Some` の場合だけ書く。
    pub third_party_licenses: Option<String>,
}

impl VrmMeta {
    /// メタ情報を作る。ライセンスURLはVRM 1.0の既定値になる。
    pub fn new(name: impl Into<String>, authors: Vec<String>) -> Self {
        Self {
            name: name.into(),
            version: None,
            authors,
            license_url: DEFAULT_LICENSE_URL.to_string(),
            copyright_information: None,
            contact_information: None,
            references: Vec::new(),
            third_party_licenses: None,
        }
    }

    /// 必須項目（空でないname・authors・licenseUrl）を確認する。
    fn validate(&self) -> Result<(), VrmExportError> {
        if self.name.is_empty() {
            return Err(VrmExportError::new(
                VrmExportErrorKind::InvalidMeta,
                "metaのnameが空である",
            ));
        }
        if !self.authors.iter().any(|author| !author.is_empty()) {
            return Err(VrmExportError::new(
                VrmExportErrorKind::InvalidMeta,
                "metaのauthorsに空でない要素が無い",
            ));
        }
        if self.license_url.is_empty() {
            return Err(VrmExportError::new(
                VrmExportErrorKind::InvalidMeta,
                "metaのlicenseUrlが空である",
            ));
        }
        Ok(())
    }
}

/// VRM書き出しの設定。
#[derive(Debug, Clone)]
pub struct VrmExportOptions {
    /// メタ情報。
    pub meta: VrmMeta,
    /// humanoidの対応付け。`None` の場合は名前検出を使う。
    pub humanoid: Option<HumanoidBones>,
    max_output_bytes: usize,
}

impl VrmExportOptions {
    /// 設定を作る。humanoidは名前検出になり、出力上限は既定値になる。
    pub fn new(meta: VrmMeta) -> Self {
        Self {
            meta,
            humanoid: None,
            max_output_bytes: DEFAULT_MAX_OUTPUT_BYTES,
        }
    }

    /// 出力バイト数の上限を変更する。
    pub fn with_max_output_bytes(mut self, bytes: usize) -> Self {
        self.max_output_bytes = bytes;
        self
    }
}

/// VRM書き出しの失敗エラー。
///
/// 対象ノード・骨が判明している範囲で索引を保持し、表示だけが逃がす。
/// 詳細部はヒープに置き、`Result` の `Err` 側を小さく保つ。
#[derive(Debug)]
pub struct VrmExportError {
    kind: VrmExportErrorKind,
    message: Box<str>,
    node: Option<usize>,
    bone: Option<HumanoidBone>,
}

impl VrmExportError {
    pub(crate) fn new(kind: VrmExportErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into().into_boxed_str(),
            node: None,
            bone: None,
        }
    }

    pub(crate) fn with_node(mut self, node: usize) -> Self {
        self.node = Some(node);
        self
    }

    pub(crate) fn with_bone(mut self, bone: HumanoidBone) -> Self {
        self.bone = Some(bone);
        self
    }

    pub(crate) fn missing_bone(bone: HumanoidBone, message: impl Into<String>) -> Self {
        Self::new(VrmExportErrorKind::MissingHumanoidBone, message).with_bone(bone)
    }

    pub(crate) fn duplicate_bone(
        bone: HumanoidBone,
        node: usize,
        message: impl Into<String>,
    ) -> Self {
        Self::new(VrmExportErrorKind::DuplicateHumanoidBone, message)
            .with_bone(bone)
            .with_node(node)
    }

    /// エラーの種別を返す。
    pub fn kind(&self) -> VrmExportErrorKind {
        self.kind
    }

    /// 判明している範囲で、対象ノードの索引を返す。
    pub fn node(&self) -> Option<usize> {
        self.node
    }

    /// 判明している範囲で、対象の骨を返す。
    pub fn bone(&self) -> Option<HumanoidBone> {
        self.bone
    }
}

impl std::fmt::Display for VrmExportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 文面は生成時に逃がし済みだが、表示時にも逃がして未逃がしの混入を防ぐ。
        // 逃がし済みの文面への再適用は変えない。
        write!(f, "{:?}: {}", self.kind, escape_diag(&self.message))?;
        if let Some(node) = self.node {
            write!(f, " [node: {node}]")?;
        }
        if let Some(bone) = self.bone {
            write!(f, " [bone: {}]", bone.name())?;
        }
        Ok(())
    }
}

impl std::error::Error for VrmExportError {}

/// VRM書き出しの失敗エラーの種別。
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VrmExportErrorKind {
    /// metaの必須項目が空の場合。
    InvalidMeta,
    /// 必須のhumanoid骨が無い場合。
    MissingHumanoidBone,
    /// 同じ骨への複数対応付け・同じノードへの複数割当の場合。
    DuplicateHumanoidBone,
    /// 骨の最も近いhumanoid祖先が親子関係に一致しない場合。
    InvalidHumanoidParent,
    /// humanoid骨の拡大率が正でない・鏡像の場合。
    NonPositiveHumanoidScale,
    /// ノード・メッシュ・スキン・材質・アクセサの索引範囲外や
    /// 頂点属性の長さ不一致など構造として成立しない場合。
    InvalidStructure,
    /// 出力バイト数が上限を超える場合。
    LimitExceeded,
}

impl VrmModel {
    /// VRM 1.0のGLBバイト列を作る。
    ///
    /// humanoidの指定が無い場合は名前検出を使い、meta・humanoid・構造を
    /// 検証してから単一バッファのGLBを組み立てる。
    ///
    /// # Errors
    ///
    /// metaの必須項目の欠落・humanoidの欠落や重複・親子不一致・非正の拡大率・
    /// 構造の不整合・出力上限の超過の場合に返す。
    pub fn to_vrm(&self, options: &VrmExportOptions) -> Result<Vec<u8>, VrmExportError> {
        options.meta.validate()?;
        let humanoid = match &options.humanoid {
            Some(bones) => bones.clone(),
            None => HumanoidBones::detect(self)?,
        };
        let bones = validate_humanoid(self, &humanoid)?;
        let mut writer = GlbWriter::new(self, options, &bones);
        writer.build()
    }
}

/// 検証済みのhumanoid対応付け（骨の列挙順）。
type ResolvedBones = Vec<(HumanoidBone, usize)>;

/// 中間ノードから求めた (glTFメッシュ, glTFスキン) の対応表。
type NodeMeshMap = HashMap<
    usize,
    (
        Option<gltf::json::Index<gltf::json::Mesh>>,
        Option<gltf::json::Index<gltf::json::Skin>>,
    ),
>;

/// humanoidを検証し、骨の列挙順の対応付けを返す。
///
/// 明示指定でも名前検出と同じ検証を行い、必須の存在・索引範囲・重複割当・
/// 最も近いhumanoid祖先・ローカル行列の拡大率を確かめる。
/// 任意骨が無い場合はその検証を飛ばす。
fn validate_humanoid(
    model: &VrmModel,
    humanoid: &HumanoidBones,
) -> Result<ResolvedBones, VrmExportError> {
    let mut bones: ResolvedBones = Vec::new();
    let mut node_to_bone: HashMap<usize, HumanoidBone> = HashMap::new();
    for bone in HumanoidBone::all() {
        let Some(node) = humanoid.get(bone) else {
            if bone.is_required() {
                return Err(VrmExportError::missing_bone(bone, "必須のhumanoid骨が無い"));
            }
            continue;
        };
        if node >= model.nodes.len() {
            return Err(VrmExportError::new(
                VrmExportErrorKind::InvalidStructure,
                "humanoid骨のノード索引が範囲外である",
            )
            .with_bone(bone)
            .with_node(node));
        }
        if let Some(_previous) = node_to_bone.insert(node, bone) {
            return Err(VrmExportError::new(
                VrmExportErrorKind::DuplicateHumanoidBone,
                "同じノードに複数の骨が割り当てられた",
            )
            .with_bone(bone)
            .with_node(node));
        }
        bones.push((bone, node));
    }
    for (bone, node) in &bones {
        let ancestor = nearest_humanoid_ancestor(model, &node_to_bone, *node)?;
        let allowed = allowed_parents_for(*bone);
        let matched = match ancestor {
            None => allowed.is_empty(),
            Some(parent) => allowed.contains(&parent),
        };
        if !matched {
            return Err(VrmExportError::new(
                VrmExportErrorKind::InvalidHumanoidParent,
                "humanoid骨の親子関係が一致しない",
            )
            .with_bone(*bone)
            .with_node(*node));
        }
        check_positive_scale(model, *bone, *node)?;
    }
    Ok(bones)
}

/// ノードから最も近いhumanoid祖先を求める。無い場合は `None` である。
fn nearest_humanoid_ancestor(
    model: &VrmModel,
    node_to_bone: &HashMap<usize, HumanoidBone>,
    node: usize,
) -> Result<Option<HumanoidBone>, VrmExportError> {
    // 呼び出し側で範囲を確認しているため、開始ノード自体は範囲内である。
    let mut current = model.nodes[node].parent;
    while let Some(index) = current {
        if let Some(bone) = node_to_bone.get(&index) {
            return Ok(Some(*bone));
        }
        let parent = model.nodes.get(index).ok_or_else(|| {
            VrmExportError::new(
                VrmExportErrorKind::InvalidStructure,
                "親ノードの索引が範囲外である",
            )
            .with_node(index)
        })?;
        current = parent.parent;
    }
    Ok(None)
}

/// humanoidノードのローカル行列が正の拡大率で鏡像でないことを確かめる。
///
/// 各列ベクトルの長さが正で、3x3部分の行列式が正の場合だけ受け付ける。
fn check_positive_scale(
    model: &VrmModel,
    bone: HumanoidBone,
    node: usize,
) -> Result<(), VrmExportError> {
    let local = &model.nodes[node].local;
    for column in [0, 4, 8] {
        let length = (local[column] * local[column]
            + local[column + 1] * local[column + 1]
            + local[column + 2] * local[column + 2])
            .sqrt();
        if !length.is_finite() || length <= 0.0 {
            return Err(VrmExportError::new(
                VrmExportErrorKind::NonPositiveHumanoidScale,
                "humanoid骨の拡大率が正でない",
            )
            .with_bone(bone)
            .with_node(node));
        }
    }
    let det = matrix::det3(local);
    if !det.is_finite() || det <= 0.0 {
        return Err(VrmExportError::new(
            VrmExportErrorKind::NonPositiveHumanoidScale,
            "humanoid骨の行列式が正でない",
        )
        .with_bone(bone)
        .with_node(node));
    }
    Ok(())
}

/// GLB組み立ての作業領域。バッファ・JSON・対応表を持つ。
struct GlbWriter<'a> {
    model: &'a VrmModel,
    options: &'a VrmExportOptions,
    bones: &'a ResolvedBones,
    root: gltf::json::Root,
    bin: Vec<u8>,
    mesh_indices: Vec<usize>,
    skin_indices: Vec<usize>,
    material_of_slot: HashMap<String, usize>,
}

impl<'a> GlbWriter<'a> {
    fn new(model: &'a VrmModel, options: &'a VrmExportOptions, bones: &'a ResolvedBones) -> Self {
        Self {
            model,
            options,
            bones,
            root: gltf::json::Root::default(),
            bin: Vec::new(),
            mesh_indices: Vec::new(),
            skin_indices: Vec::new(),
            material_of_slot: HashMap::new(),
        }
    }

    /// GLBバイト列を組み立てる。
    fn build(&mut self) -> Result<Vec<u8>, VrmExportError> {
        self.build_materials()?;
        self.build_meshes()?;
        self.build_skins()?;
        self.build_nodes()?;
        self.build_scene()?;
        self.build_vrm_extension();
        self.build_buffer();
        let json = gltf::json::serialize::to_string(&self.root).map_err(|error| {
            VrmExportError::new(
                VrmExportErrorKind::InvalidStructure,
                format!("glTF JSONのシリアライズに失敗した: {error}"),
            )
        })?;
        self.assemble_glb(json.as_bytes())
    }

    /// スロット名ごとに一意な材質を作る。
    fn build_materials(&mut self) -> Result<(), VrmExportError> {
        for mesh in &self.model.meshes {
            for slot in &mesh.material_slots {
                if self.material_of_slot.contains_key(slot) {
                    continue;
                }
                let name = if slot.is_empty() {
                    "Material".to_string()
                } else {
                    slot.clone()
                };
                let material = gltf::json::Material {
                    alpha_mode: gltf::json::validation::Checked::Valid(
                        gltf::json::material::AlphaMode::Opaque,
                    ),
                    double_sided: false,
                    name: Some(name),
                    pbr_metallic_roughness: gltf::json::material::PbrMetallicRoughness {
                        base_color_factor: gltf::json::material::PbrBaseColorFactor([
                            0.8, 0.8, 0.8, 1.0,
                        ]),
                        metallic_factor: gltf::json::material::StrengthFactor(0.0),
                        roughness_factor: gltf::json::material::StrengthFactor(1.0),
                        ..Default::default()
                    },
                    ..Default::default()
                };
                let index = self.root.materials.len();
                self.root.materials.push(material);
                self.material_of_slot.insert(slot.clone(), index);
            }
        }
        Ok(())
    }

    /// メッシュと頂点アクセサを作る。
    fn build_meshes(&mut self) -> Result<(), VrmExportError> {
        for mesh in &self.model.meshes {
            let node = mesh.node;
            if node >= self.model.nodes.len() {
                return Err(VrmExportError::new(
                    VrmExportErrorKind::InvalidStructure,
                    "メッシュのノード索引が範囲外である",
                )
                .with_node(node));
            }
            let count = mesh.vertices.positions.len();
            if count == 0 {
                return Err(VrmExportError::new(
                    VrmExportErrorKind::InvalidStructure,
                    "頂点が無いメッシュがある",
                ));
            }
            check_vertex_lengths(mesh, count)?;
            let mut attributes = BTreeMap::new();
            attributes.insert(
                gltf::json::validation::Checked::Valid(gltf::json::mesh::Semantic::Positions),
                self.push_float_accessor(
                    &mesh.vertices.positions,
                    gltf::json::accessor::Type::Vec3,
                    Some(position_min_max(&mesh.vertices.positions)),
                    gltf::json::buffer::Target::ArrayBuffer,
                )?,
            );
            if let Some(normals) = &mesh.vertices.normals {
                attributes.insert(
                    gltf::json::validation::Checked::Valid(gltf::json::mesh::Semantic::Normals),
                    self.push_float_accessor(
                        normals,
                        gltf::json::accessor::Type::Vec3,
                        None,
                        gltf::json::buffer::Target::ArrayBuffer,
                    )?,
                );
            }
            for (set, uvs) in mesh.vertices.uv_sets.iter().enumerate() {
                let semantic = u32::try_from(set)
                    .map(gltf::json::mesh::Semantic::TexCoords)
                    .map_err(|_| {
                        VrmExportError::new(
                            VrmExportErrorKind::InvalidStructure,
                            "UVセット数がu32の範囲を超えた",
                        )
                    })?;
                attributes.insert(
                    gltf::json::validation::Checked::Valid(semantic),
                    self.push_float_accessor(
                        uvs,
                        gltf::json::accessor::Type::Vec2,
                        None,
                        gltf::json::buffer::Target::ArrayBuffer,
                    )?,
                );
            }
            match (&mesh.vertices.joints, &mesh.vertices.weights) {
                (Some(joints), Some(weights)) => {
                    attributes.insert(
                        gltf::json::validation::Checked::Valid(gltf::json::mesh::Semantic::Joints(
                            0,
                        )),
                        self.push_joints_accessor(joints)?,
                    );
                    attributes.insert(
                        gltf::json::validation::Checked::Valid(
                            gltf::json::mesh::Semantic::Weights(0),
                        ),
                        self.push_float_accessor(
                            weights,
                            gltf::json::accessor::Type::Vec4,
                            None,
                            gltf::json::buffer::Target::ArrayBuffer,
                        )?,
                    );
                }
                (None, None) => {}
                _ => {
                    return Err(VrmExportError::new(
                        VrmExportErrorKind::InvalidStructure,
                        "関節と重みの有無が一致しない",
                    ));
                }
            }
            let mut primitives = Vec::with_capacity(mesh.primitives.len());
            for primitive in &mesh.primitives {
                let material = if mesh.material_slots.is_empty() {
                    None
                } else {
                    let slot = mesh.material_slots.get(primitive.material).ok_or_else(|| {
                        VrmExportError::new(
                            VrmExportErrorKind::InvalidStructure,
                            "材質番号がスロット範囲外である",
                        )
                    })?;
                    Some(gltf::json::Index::new(
                        u32::try_from(*self.material_of_slot.get(slot).ok_or_else(|| {
                            VrmExportError::new(
                                VrmExportErrorKind::InvalidStructure,
                                "材質スロットに対応する材質が無い",
                            )
                        })?)
                        .map_err(|_| {
                            VrmExportError::new(
                                VrmExportErrorKind::InvalidStructure,
                                "材質数がu32の範囲を超えた",
                            )
                        })?,
                    ))
                };
                let indices = self.push_indices_accessor(&primitive.indices)?;
                primitives.push(gltf::json::mesh::Primitive {
                    attributes: attributes.clone(),
                    indices: Some(indices),
                    material,
                    mode: gltf::json::validation::Checked::Valid(gltf::json::mesh::Mode::Triangles),
                    targets: None,
                    extensions: None,
                    extras: Default::default(),
                });
            }
            let index = self.root.meshes.len();
            self.root.meshes.push(gltf::json::Mesh {
                primitives,
                name: Some(mesh.name.clone()),
                weights: None,
                extensions: None,
                extras: Default::default(),
            });
            self.mesh_indices.push(index);
        }
        Ok(())
    }

    /// スキンと逆バインド行列アクセサを作る。
    fn build_skins(&mut self) -> Result<(), VrmExportError> {
        let hips = self
            .bones
            .iter()
            .find(|(bone, _)| *bone == HumanoidBone::Hips)
            .map(|(_, node)| *node);
        for skin in &self.model.skins {
            if skin.joints.len() != skin.inverse_bind_matrices.len() {
                return Err(VrmExportError::new(
                    VrmExportErrorKind::InvalidStructure,
                    "関節列と逆バインド行列の長さが一致しない",
                ));
            }
            for joint in &skin.joints {
                if *joint >= self.model.nodes.len() {
                    return Err(VrmExportError::new(
                        VrmExportErrorKind::InvalidStructure,
                        "関節のノード索引が範囲外である",
                    )
                    .with_node(*joint));
                }
            }
            let joints = skin.joints.to_vec();
            let matrices = self.push_matrices_accessor(&skin.inverse_bind_matrices)?;
            let index = self.root.skins.len();
            self.root.skins.push(gltf::json::Skin {
                inverse_bind_matrices: Some(matrices),
                joints: joints
                    .into_iter()
                    .map(|joint| {
                        u32::try_from(joint)
                            .map(gltf::json::Index::new)
                            .map_err(|_| {
                                VrmExportError::new(
                                    VrmExportErrorKind::InvalidStructure,
                                    "ノード数がu32の範囲を超えた",
                                )
                            })
                    })
                    .collect::<Result<Vec<_>, _>>()?,
                skeleton: hips
                    .map(|node| {
                        u32::try_from(node)
                            .map(gltf::json::Index::new)
                            .map_err(|_| {
                                VrmExportError::new(
                                    VrmExportErrorKind::InvalidStructure,
                                    "ノード数がu32の範囲を超えた",
                                )
                            })
                    })
                    .transpose()?,
                name: None,
                extensions: None,
                extras: Default::default(),
            });
            self.skin_indices.push(index);
        }
        Ok(())
    }

    /// 全ノードを書く。ルート（親なし）の行列は `root * local`、他は `local` である。
    ///
    /// 単位行列の箇所は `matrix` を省略する（glTFの既定値が単位行列のため）。
    fn build_nodes(&mut self) -> Result<(), VrmExportError> {
        let mesh_of_node = self.node_mesh_map()?;
        let mut nodes = Vec::with_capacity(self.model.nodes.len());
        for (index, node) in self.model.nodes.iter().enumerate() {
            if let Some(parent) = node.parent
                && parent >= self.model.nodes.len()
            {
                return Err(VrmExportError::new(
                    VrmExportErrorKind::InvalidStructure,
                    "親ノードの索引が範囲外である",
                )
                .with_node(index));
            }
            let mut children = Vec::with_capacity(node.children.len());
            for child in &node.children {
                if *child >= self.model.nodes.len() {
                    return Err(VrmExportError::new(
                        VrmExportErrorKind::InvalidStructure,
                        "子ノードの索引が範囲外である",
                    )
                    .with_node(index));
                }
                children.push(gltf::json::Index::new(u32::try_from(*child).map_err(
                    |_| {
                        VrmExportError::new(
                            VrmExportErrorKind::InvalidStructure,
                            "ノード数がu32の範囲を超えた",
                        )
                    },
                )?));
            }
            let (mesh, skin) = mesh_of_node.get(&index).copied().unwrap_or((None, None));
            // 合成ルートは置かず、親なしノードに `root * local` を直接持たせる
            // （`root` は列優先・列ベクトル規約のため左から掛ける）。
            // 単位行列はglTFの既定値のため省略する。
            let effective = if node.parent.is_none() {
                matrix::mul(&self.model.root, &node.local)
            } else {
                node.local
            };
            nodes.push(gltf::json::Node {
                matrix: if effective == matrix::IDENTITY {
                    None
                } else {
                    Some(to_f32_matrix(&effective))
                },
                children: if children.is_empty() {
                    None
                } else {
                    Some(children)
                },
                mesh,
                skin,
                name: Some(node.name.clone()),
                ..Default::default()
            });
        }
        self.root.nodes = nodes;
        Ok(())
    }

    /// 中間ノードから (glTFメッシュ, glTFスキン) を求める。
    fn node_mesh_map(&self) -> Result<NodeMeshMap, VrmExportError> {
        let mut map = HashMap::new();
        for (mesh, mesh_index) in self.model.meshes.iter().zip(self.mesh_indices.iter()) {
            if map.contains_key(&mesh.node) {
                return Err(VrmExportError::new(
                    VrmExportErrorKind::InvalidStructure,
                    "1つのノードに複数のメッシュが対応する",
                )
                .with_node(mesh.node));
            }
            let skin = match mesh.skin {
                Some(skin) => {
                    let gltf_skin = *self.skin_indices.get(skin).ok_or_else(|| {
                        VrmExportError::new(
                            VrmExportErrorKind::InvalidStructure,
                            "スキンの索引が範囲外である",
                        )
                    })?;
                    Some(gltf::json::Index::new(u32::try_from(gltf_skin).map_err(
                        |_| {
                            VrmExportError::new(
                                VrmExportErrorKind::InvalidStructure,
                                "スキン数がu32の範囲を超えた",
                            )
                        },
                    )?))
                }
                None => None,
            };
            map.insert(
                mesh.node,
                (
                    Some(gltf::json::Index::new(u32::try_from(*mesh_index).map_err(
                        |_| {
                            VrmExportError::new(
                                VrmExportErrorKind::InvalidStructure,
                                "メッシュ数がu32の範囲を超えた",
                            )
                        },
                    )?)),
                    skin,
                ),
            );
        }
        Ok(map)
    }

    /// シーンは1つだけ作り、中間モデルのルート（親なし）を指す。
    fn build_scene(&mut self) -> Result<(), VrmExportError> {
        let mut roots = Vec::new();
        for (index, node) in self.model.nodes.iter().enumerate() {
            if node.parent.is_none() {
                roots.push(gltf::json::Index::new(u32::try_from(index).map_err(
                    |_| {
                        VrmExportError::new(
                            VrmExportErrorKind::InvalidStructure,
                            "ノード数がu32の範囲を超えた",
                        )
                    },
                )?));
            }
        }
        self.root.scenes.push(gltf::json::Scene {
            nodes: roots,
            name: None,
            extensions: None,
            extras: Default::default(),
        });
        self.root.scene = Some(gltf::json::Index::new(0));
        Ok(())
    }

    /// `VRMC_vrm` 拡張（meta・humanoid）を書く。
    fn build_vrm_extension(&mut self) {
        let meta = &self.options.meta;
        let mut meta_value = serde_json::Map::new();
        meta_value.insert(
            "name".to_string(),
            serde_json::Value::String(meta.name.clone()),
        );
        meta_value.insert(
            "authors".to_string(),
            serde_json::Value::Array(
                meta.authors
                    .iter()
                    .map(|author| serde_json::Value::String(author.clone()))
                    .collect(),
            ),
        );
        meta_value.insert(
            "licenseUrl".to_string(),
            serde_json::Value::String(meta.license_url.clone()),
        );
        if let Some(version) = &meta.version {
            meta_value.insert(
                "version".to_string(),
                serde_json::Value::String(version.clone()),
            );
        }
        if let Some(copyright) = &meta.copyright_information {
            meta_value.insert(
                "copyrightInformation".to_string(),
                serde_json::Value::String(copyright.clone()),
            );
        }
        if let Some(contact) = &meta.contact_information {
            meta_value.insert(
                "contactInformation".to_string(),
                serde_json::Value::String(contact.clone()),
            );
        }
        if !meta.references.is_empty() {
            meta_value.insert(
                "references".to_string(),
                serde_json::Value::Array(
                    meta.references
                        .iter()
                        .map(|reference| serde_json::Value::String(reference.clone()))
                        .collect(),
                ),
            );
        }
        if let Some(licenses) = &meta.third_party_licenses {
            meta_value.insert(
                "thirdPartyLicenses".to_string(),
                serde_json::Value::String(licenses.clone()),
            );
        }
        let mut human_bones = serde_json::Map::new();
        for (bone, node) in self.bones {
            human_bones.insert(bone.name().to_string(), serde_json::json!({ "node": node }));
        }
        let mut vrmc = serde_json::Map::new();
        vrmc.insert(
            "specVersion".to_string(),
            serde_json::Value::String("1.0".to_string()),
        );
        vrmc.insert("meta".to_string(), serde_json::Value::Object(meta_value));
        vrmc.insert(
            "humanoid".to_string(),
            serde_json::json!({ "humanBones": human_bones }),
        );
        let mut others = serde_json::Map::new();
        others.insert("VRMC_vrm".to_string(), serde_json::Value::Object(vrmc));
        self.root.extensions = Some(gltf::json::extensions::root::Root { others });
        self.root.extensions_used = vec!["VRMC_vrm".to_string()];
    }

    /// バッファ定義を作る。BINが空の場合は定義を置かない。
    fn build_buffer(&mut self) {
        if self.bin.is_empty() {
            return;
        }
        self.root.buffers.push(gltf::json::Buffer {
            byte_length: gltf::json::validation::USize64(self.bin.len() as u64),
            name: None,
            uri: None,
            extensions: None,
            extras: Default::default(),
        });
    }

    /// GLBの容器（ヘッダ・JSONチャンク・BINチャンク）を組み立てる。
    fn assemble_glb(&self, json: &[u8]) -> Result<Vec<u8>, VrmExportError> {
        let json_padded = align4(json.len());
        let bin_padded = align4(self.bin.len());
        let mut total: u64 = 12 + 8 + json_padded as u64;
        if !self.bin.is_empty() {
            total += 8 + bin_padded as u64;
        }
        if total > u32::MAX as u64 || total > self.options.max_output_bytes as u64 {
            return Err(VrmExportError::new(
                VrmExportErrorKind::LimitExceeded,
                "出力バイト数が上限を超えた",
            ));
        }
        let mut out = Vec::with_capacity(total as usize);
        out.extend_from_slice(b"glTF");
        out.extend_from_slice(&2u32.to_le_bytes());
        out.extend_from_slice(&(total as u32).to_le_bytes());
        out.extend_from_slice(&(json_padded as u32).to_le_bytes());
        out.extend_from_slice(&0x4E4F534Au32.to_le_bytes());
        out.extend_from_slice(json);
        out.extend(std::iter::repeat_n(b' ', json_padded - json.len()));
        if !self.bin.is_empty() {
            out.extend_from_slice(&(bin_padded as u32).to_le_bytes());
            out.extend_from_slice(&0x004E4942u32.to_le_bytes());
            out.extend_from_slice(&self.bin);
            out.extend(std::iter::repeat_n(0, bin_padded - self.bin.len()));
        }
        Ok(out)
    }

    /// バッファへ4バイト境界で詰めて書き、アクセサとビューを作る。
    ///
    /// BINだけで上限を超える場合は追加前に打ち切り、巨大な `bin` を確保してから
    /// 失敗する事態を避ける。JSON分の超過は最終段の検査で扱う。
    fn push_accessor(
        &mut self,
        data: &[u8],
        spec: AccessorSpec,
    ) -> Result<gltf::json::Index<gltf::json::Accessor>, VrmExportError> {
        self.check_bin_capacity(data.len())?;
        let offset = align4(self.bin.len());
        self.bin
            .extend(std::iter::repeat_n(0, offset - self.bin.len()));
        self.bin.extend_from_slice(data);
        let view = gltf::json::buffer::View {
            buffer: gltf::json::Index::new(0),
            byte_length: gltf::json::validation::USize64(data.len() as u64),
            byte_offset: if offset == 0 {
                None
            } else {
                Some(gltf::json::validation::USize64(offset as u64))
            },
            byte_stride: None,
            name: None,
            target: spec.target.map(gltf::json::validation::Checked::Valid),
            extensions: None,
            extras: Default::default(),
        };
        let view_index = self.root.buffer_views.len() as u32;
        self.root.buffer_views.push(view);
        // 後続のアクセサが4バイト境界に来るよう詰める。
        let padded = align4(self.bin.len());
        self.bin
            .extend(std::iter::repeat_n(0, padded - self.bin.len()));
        let accessor = gltf::json::Accessor {
            buffer_view: Some(gltf::json::Index::new(view_index)),
            byte_offset: None,
            count: gltf::json::validation::USize64(spec.count as u64),
            component_type: gltf::json::validation::Checked::Valid(
                gltf::json::accessor::GenericComponentType(spec.component_type),
            ),
            type_: gltf::json::validation::Checked::Valid(spec.accessor_type),
            min: spec.min,
            max: spec.max,
            name: None,
            normalized: false,
            sparse: None,
            extensions: None,
            extras: Default::default(),
        };
        let accessor_index = self.root.accessors.len() as u32;
        self.root.accessors.push(accessor);
        Ok(gltf::json::Index::new(accessor_index))
    }

    /// 次に追加するデータでBINが上限を超える場合に、追加前に打ち切る。
    ///
    /// 比較対象はBIN単体であり、JSONと容器分の超過は最終段の検査に残す。
    /// BIN単体で上限を超える場合は全体も必ず超えるため、誤って拒まない。
    fn check_bin_capacity(&self, data_len: usize) -> Result<(), VrmExportError> {
        let offset = align4(self.bin.len());
        let Some(end) = offset.checked_add(data_len) else {
            return Err(VrmExportError::new(
                VrmExportErrorKind::LimitExceeded,
                "出力バイト数が上限を超えた",
            ));
        };
        if align4(end) > self.options.max_output_bytes {
            return Err(VrmExportError::new(
                VrmExportErrorKind::LimitExceeded,
                "出力バイト数が上限を超えた",
            ));
        }
        Ok(())
    }

    /// f32配列のアクセサを作る。min/maxは位置だけに付ける。
    fn push_float_accessor<const N: usize>(
        &mut self,
        values: &[[f32; N]],
        accessor_type: gltf::json::accessor::Type,
        min_max: Option<([f32; 3], [f32; 3])>,
        target: gltf::json::buffer::Target,
    ) -> Result<gltf::json::Index<gltf::json::Accessor>, VrmExportError> {
        let mut data = Vec::with_capacity(values.len() * N * 4);
        for value in values {
            for component in value {
                data.extend_from_slice(&component.to_le_bytes());
            }
        }
        let (min, max) = min_max
            .map(|(min, max)| {
                (
                    Some(serde_json::json!([min[0], min[1], min[2]])),
                    Some(serde_json::json!([max[0], max[1], max[2]])),
                )
            })
            .unwrap_or((None, None));
        self.push_accessor(
            &data,
            AccessorSpec {
                component_type: gltf::json::accessor::ComponentType::F32,
                accessor_type,
                count: values.len(),
                min,
                max,
                target: Some(target),
            },
        )
    }

    /// 関節索引（VEC4/U16）のアクセサを作る。
    fn push_joints_accessor(
        &mut self,
        values: &[[u16; 4]],
    ) -> Result<gltf::json::Index<gltf::json::Accessor>, VrmExportError> {
        let mut data = Vec::with_capacity(values.len() * 8);
        for value in values {
            for component in value {
                data.extend_from_slice(&component.to_le_bytes());
            }
        }
        self.push_accessor(
            &data,
            AccessorSpec {
                component_type: gltf::json::accessor::ComponentType::U16,
                accessor_type: gltf::json::accessor::Type::Vec4,
                count: values.len(),
                min: None,
                max: None,
                target: Some(gltf::json::buffer::Target::ArrayBuffer),
            },
        )
    }

    /// 索引（SCALAR/U32）のアクセサを作る。
    fn push_indices_accessor(
        &mut self,
        values: &[u32],
    ) -> Result<gltf::json::Index<gltf::json::Accessor>, VrmExportError> {
        let mut data = Vec::with_capacity(values.len() * 4);
        for value in values {
            data.extend_from_slice(&value.to_le_bytes());
        }
        self.push_accessor(
            &data,
            AccessorSpec {
                component_type: gltf::json::accessor::ComponentType::U32,
                accessor_type: gltf::json::accessor::Type::Scalar,
                count: values.len(),
                min: None,
                max: None,
                target: Some(gltf::json::buffer::Target::ElementArrayBuffer),
            },
        )
    }

    /// 逆バインド行列（MAT4/F32）のアクセサを作る。
    ///
    /// 頂点属性ではないため、bufferViewに `target` を付けない。
    fn push_matrices_accessor(
        &mut self,
        values: &[[f64; 16]],
    ) -> Result<gltf::json::Index<gltf::json::Accessor>, VrmExportError> {
        let mut data = Vec::with_capacity(values.len() * 64);
        for matrix in values {
            for component in matrix {
                data.extend_from_slice(&(*component as f32).to_le_bytes());
            }
        }
        self.push_accessor(
            &data,
            AccessorSpec {
                component_type: gltf::json::accessor::ComponentType::F32,
                accessor_type: gltf::json::accessor::Type::Mat4,
                count: values.len(),
                min: None,
                max: None,
                target: None,
            },
        )
    }
}

/// アクセサ定義。型情報と用途をまとめて引数の膨張を避ける。
struct AccessorSpec {
    component_type: gltf::json::accessor::ComponentType,
    accessor_type: gltf::json::accessor::Type,
    count: usize,
    min: Option<serde_json::Value>,
    max: Option<serde_json::Value>,
    target: Option<gltf::json::buffer::Target>,
}
/// 4バイト境界へ切り上げる。
fn align4(length: usize) -> usize {
    length.next_multiple_of(4)
}

/// 列優先のままf64行列をf32へ変換する。
fn to_f32_matrix(matrix: &[f64; 16]) -> [f32; 16] {
    let mut out = [0.0f32; 16];
    for (slot, value) in out.iter_mut().zip(matrix.iter()) {
        *slot = *value as f32;
    }
    out
}

/// 位置のmin/maxを求める。呼び出し側で空でないことを保証する。
fn position_min_max(positions: &[[f32; 3]]) -> ([f32; 3], [f32; 3]) {
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for position in positions {
        for (index, value) in position.iter().enumerate() {
            min[index] = min[index].min(*value);
            max[index] = max[index].max(*value);
        }
    }
    (min, max)
}

/// 頂点属性の長さが位置と一致することを確かめる。
fn check_vertex_lengths(mesh: &super::VrmMesh, count: usize) -> Result<(), VrmExportError> {
    if let Some(normals) = &mesh.vertices.normals
        && normals.len() != count
    {
        return Err(VrmExportError::new(
            VrmExportErrorKind::InvalidStructure,
            "法線の長さが位置と一致しない",
        ));
    }
    for (set, uvs) in mesh.vertices.uv_sets.iter().enumerate() {
        if uvs.len() != count {
            return Err(VrmExportError::new(
                VrmExportErrorKind::InvalidStructure,
                format!("UVセット{set}の長さが位置と一致しない"),
            ));
        }
    }
    if let Some(joints) = &mesh.vertices.joints
        && joints.len() != count
    {
        return Err(VrmExportError::new(
            VrmExportErrorKind::InvalidStructure,
            "関節索引の長さが位置と一致しない",
        ));
    }
    if let Some(weights) = &mesh.vertices.weights
        && weights.len() != count
    {
        return Err(VrmExportError::new(
            VrmExportErrorKind::InvalidStructure,
            "重みの長さが位置と一致しない",
        ));
    }
    Ok(())
}
