//! VRM 1.0のGLBバイト列への書き出し。
//!
//! 入口は [`VrmModel::to_vrm`](crate::vrm::VrmModel::to_vrm) であり、
//! [`VrmMeta`] と [`HumanoidBones`](super::humanoid::HumanoidBones) の検証から
//! glTF JSON（`VRMC_vrm` 拡張付き）と単一バッファを組み立て、GLBの容器は自前で書く。
//! `gltf` クレートのimport機能は使わない（画像デコーダを依存に入れないため）。

use std::collections::{BTreeMap, HashMap, HashSet};

use super::VrmModel;
use super::humanoid::{HumanoidBone, HumanoidBones, allowed_parents_for};
use super::material::{
    VrmAlphaMode, VrmFilter, VrmMaterial, VrmMaterials, VrmMtoonParameters, VrmSampler, VrmWrap,
    placeholder,
};
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
    /// Unityの `.mat` から作った材質。`None` の場合は灰色PBR材質を書く。
    pub materials: Option<VrmMaterials>,
    max_output_bytes: usize,
}

impl VrmExportOptions {
    /// 設定を作る。humanoidは名前検出、材質は灰色PBR、出力上限は既定値になる。
    pub fn new(meta: VrmMeta) -> Self {
        Self {
            meta,
            humanoid: None,
            materials: None,
            max_output_bytes: DEFAULT_MAX_OUTPUT_BYTES,
        }
    }

    /// Unityから読み取った材質を使う。
    pub fn with_materials(mut self, materials: VrmMaterials) -> Self {
        self.materials = Some(materials);
        self
    }

    /// humanoidの対応付けを明示指定する。指定時は名前検出を使わない。
    pub fn with_humanoid(mut self, humanoid: HumanoidBones) -> Self {
        self.humanoid = Some(humanoid);
        self
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
    /// 構造の不整合・材質値の不正・出力上限の超過の場合に返す。
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
    /// 画像索引からテクスチャ索引への対応表。画像順に詰める。
    image_textures: Vec<gltf::json::Index<gltf::json::texture::Texture>>,
    /// MToon材質を1件でも書いたかどうか。`extensionsUsed` に反映する。
    wrote_mtoon: bool,
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
            image_textures: Vec::new(),
            wrote_mtoon: false,
        }
    }

    /// GLBバイト列を組み立てる。
    fn build(&mut self) -> Result<Vec<u8>, VrmExportError> {
        self.build_images()?;
        self.build_materials()?;
        self.build_meshes()?;
        self.build_skins()?;
        self.build_nodes()?;
        self.build_scene()?;
        self.build_vrm_extension()?;
        self.build_buffer();
        let json = gltf::json::serialize::to_string(&self.root).map_err(|error| {
            VrmExportError::new(
                VrmExportErrorKind::InvalidStructure,
                format!("glTF JSONのシリアライズに失敗した: {error}"),
            )
        })?;
        self.assemble_glb(json.as_bytes())
    }

    /// 材質の画像・サンプラ・テクスチャを作る。
    ///
    /// 画像は単一バッファへ4バイト境界で詰め、サンプラは値の組で重複排除し、
    /// テクスチャは画像ごとに1件作る。材質が無い場合は何もしない。
    fn build_images(&mut self) -> Result<(), VrmExportError> {
        // `options` は `&'a` の共有参照のため、参照だけを複写して借用を切り離し、
        // 後続の `bin`・`root` への可変借用と競合させない。
        let options: &'a VrmExportOptions = self.options;
        let Some(materials) = options.materials.as_ref() else {
            return Ok(());
        };
        let images = materials.images();
        let max_output_bytes = options.max_output_bytes;
        // フィールド単位で借用し、`self` 全体の可変借用を作らない。
        let bin = &mut self.bin;
        let root = &mut self.root;
        let image_textures = &mut self.image_textures;
        // サンプラ値とサンプラ索引の対応表。件数が少ないため線形探索で足る。
        let mut sampler_of: Vec<(VrmSampler, usize)> = Vec::new();
        for source in images {
            let view = push_view_into(bin, root, max_output_bytes, &source.data, None)?;
            let mime_type = match source.mime {
                super::material::VrmImageMime::Png => "image/png",
                super::material::VrmImageMime::Jpeg => "image/jpeg",
            };
            let image = gltf::json::image::Image {
                buffer_view: Some(view),
                mime_type: Some(gltf::json::image::MimeType(mime_type.to_string())),
                name: Some(source.name.clone()),
                uri: None,
                extensions: None,
                extras: Default::default(),
            };
            let image_at = u32::try_from(root.images.len()).map_err(|_| {
                VrmExportError::new(
                    VrmExportErrorKind::InvalidStructure,
                    "画像数がu32の範囲を超えた",
                )
            })?;
            root.images.push(image);
            let sampler_at = match sampler_of
                .iter()
                .find(|(known, _)| *known == source.sampler)
            {
                Some((_, index)) => *index,
                None => {
                    let index = root.samplers.len();
                    root.samplers.push(sampler_json(&source.sampler));
                    sampler_of.push((source.sampler, index));
                    index
                }
            };
            let texture = gltf::json::texture::Texture {
                name: None,
                sampler: Some(gltf::json::Index::new(u32::try_from(sampler_at).map_err(
                    |_| {
                        VrmExportError::new(
                            VrmExportErrorKind::InvalidStructure,
                            "サンプラ数がu32の範囲を超えた",
                        )
                    },
                )?)),
                source: gltf::json::Index::new(image_at),
                extensions: None,
                extras: Default::default(),
            };
            let texture_at = u32::try_from(root.textures.len()).map_err(|_| {
                VrmExportError::new(
                    VrmExportErrorKind::InvalidStructure,
                    "テクスチャ数がu32の範囲を超えた",
                )
            })?;
            root.textures.push(texture);
            image_textures.push(gltf::json::Index::new(texture_at));
        }
        Ok(())
    }

    /// スロット名ごとに一意な材質を作る。
    ///
    /// 材質があるスロットは読み取り結果から、無いスロットは従来どおり
    /// スロット名だけを持つ灰色のPBR材質を書く。
    fn build_materials(&mut self) -> Result<(), VrmExportError> {
        // スロットの初出順を保ちつつ、存在判定は `HashSet` でO(1)にする。
        let mut seen = HashSet::new();
        let mut slots = Vec::new();
        for mesh in &self.model.meshes {
            for slot in &mesh.material_slots {
                if !seen.contains(slot) {
                    seen.insert(slot.clone());
                    slots.push(slot.clone());
                }
            }
        }
        for slot in slots {
            let resolved = self
                .options
                .materials
                .as_ref()
                .and_then(|materials| materials.get(&slot))
                .cloned();
            let material = match resolved {
                Some(vrm) => self.build_vrm_material(&vrm)?,
                None => gray_material(&slot),
            };
            let index = self.root.materials.len();
            self.root.materials.push(material);
            self.material_of_slot.insert(slot, index);
        }
        Ok(())
    }

    /// 読み取り済み材質からglTF材質を作る。
    fn build_vrm_material(
        &mut self,
        vrm: &VrmMaterial,
    ) -> Result<gltf::json::Material, VrmExportError> {
        if let Err(field) = super::material::validate_material(vrm) {
            return Err(VrmExportError::new(
                VrmExportErrorKind::InvalidStructure,
                format!("材質値が不正である: {field}"),
            ));
        }
        let alpha_mode = match vrm.alpha_mode {
            VrmAlphaMode::Opaque => gltf::json::material::AlphaMode::Opaque,
            VrmAlphaMode::Mask => gltf::json::material::AlphaMode::Mask,
            VrmAlphaMode::Blend => gltf::json::material::AlphaMode::Blend,
        };
        // `alphaCutoff` はカットアウトのときだけ書く。
        let alpha_cutoff = match vrm.alpha_mode {
            VrmAlphaMode::Mask => Some(gltf::json::material::AlphaCutoff(vrm.alpha_cutoff)),
            VrmAlphaMode::Opaque | VrmAlphaMode::Blend => None,
        };
        let base_color_texture = vrm
            .base_color_image
            .map(|image| self.texture_info(image))
            .transpose()?;
        let emissive_texture = vrm
            .emissive_image
            .map(|image| self.texture_info(image))
            .transpose()?;
        let extensions = match &vrm.mtoon {
            Some(parameters) => {
                self.wrote_mtoon = true;
                Some(mtoon_extension(parameters, &self.image_textures)?)
            }
            None => None,
        };
        Ok(gltf::json::Material {
            alpha_mode: gltf::json::validation::Checked::Valid(alpha_mode),
            alpha_cutoff,
            double_sided: vrm.double_sided,
            name: Some(vrm.name.clone()),
            pbr_metallic_roughness: gltf::json::material::PbrMetallicRoughness {
                base_color_factor: gltf::json::material::PbrBaseColorFactor(vrm.base_color_factor),
                base_color_texture,
                metallic_factor: gltf::json::material::StrengthFactor(0.0),
                roughness_factor: gltf::json::material::StrengthFactor(1.0),
                ..Default::default()
            },
            emissive_factor: gltf::json::material::EmissiveFactor(vrm.emissive_factor),
            emissive_texture,
            extensions,
            ..Default::default()
        })
    }

    /// 画像索引からテクスチャ参照を作る。
    fn texture_info(&self, image: usize) -> Result<gltf::json::texture::Info, VrmExportError> {
        let texture = self.image_textures.get(image).ok_or_else(|| {
            VrmExportError::new(
                VrmExportErrorKind::InvalidStructure,
                "材質の画像索引が範囲外である",
            )
        })?;
        Ok(gltf::json::texture::Info {
            index: *texture,
            tex_coord: 0,
            extensions: None,
            extras: Default::default(),
        })
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
                    self.push_uv_accessor(uvs)?,
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
            // モーフターゲットは位置の差分だけを持ち、全プリミティブが同じ順序で参照する。
            // 材質分割とターゲット参照は直交するため、アクセサはメッシュ単位で1回だけ作る。
            let mut morph_accessors = Vec::with_capacity(mesh.targets.len());
            for target in &mesh.targets {
                morph_accessors.push(self.push_float_accessor(
                    &target.positions,
                    gltf::json::accessor::Type::Vec3,
                    Some(position_min_max(&target.positions)),
                    gltf::json::buffer::Target::ArrayBuffer,
                )?);
            }
            let morph_targets = if morph_accessors.is_empty() {
                None
            } else {
                Some(
                    morph_accessors
                        .into_iter()
                        .map(|accessor| gltf::json::mesh::MorphTarget {
                            positions: Some(accessor),
                            normals: None,
                            tangents: None,
                        })
                        .collect::<Vec<_>>(),
                )
            };
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
                    targets: morph_targets.clone(),
                    extensions: None,
                    extras: Default::default(),
                });
            }
            let target_names: Vec<String> = mesh
                .targets
                .iter()
                .map(|target| target.name.clone())
                .collect();
            let extras = if target_names.is_empty() {
                Default::default()
            } else {
                let raw = serde_json::value::to_raw_value(&serde_json::json!({
                    "targetNames": target_names,
                }))
                .map_err(|error| {
                    VrmExportError::new(
                        VrmExportErrorKind::InvalidStructure,
                        format!("targetNamesのシリアライズに失敗した: {error}"),
                    )
                })?;
                Some(raw)
            };
            let weights = if target_names.is_empty() {
                None
            } else {
                Some(vec![0.0f32; target_names.len()])
            };
            let index = self.root.meshes.len();
            self.root.meshes.push(gltf::json::Mesh {
                primitives,
                name: Some(mesh.name.clone()),
                weights,
                extensions: None,
                extras,
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

    /// `VRMC_vrm` 拡張（meta・humanoid・expressions）を書く。
    fn build_vrm_extension(&mut self) -> Result<(), VrmExportError> {
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
        if !self.model.expressions.preset.is_empty() {
            let preset = self.build_expressions_json()?;
            vrmc.insert("expressions".to_string(), serde_json::Value::Object(preset));
        }
        let mut others = serde_json::Map::new();
        others.insert("VRMC_vrm".to_string(), serde_json::Value::Object(vrmc));
        let mut wrote_spring_bone = false;
        let mut wrote_spring_bone_limit = false;
        if !self.model.spring_bones.springs.is_empty()
            || !self.model.spring_bones.colliders.is_empty()
            || !self.model.spring_bones.collider_groups.is_empty()
        {
            let spring_bone = self.build_spring_bone_json()?;
            // 1件でも角度制限があれば、読み手向けに拡張使用を宣言する。
            wrote_spring_bone_limit = self
                .model
                .spring_bones
                .springs
                .iter()
                .flat_map(|spring| spring.joints.iter())
                .any(|joint| joint.limit.is_some());
            others.insert("VRMC_springBone".to_string(), spring_bone);
            wrote_spring_bone = true;
        }
        self.root.extensions = Some(gltf::json::extensions::root::Root { others });
        let mut used = vec!["VRMC_vrm".to_string()];
        if self.wrote_mtoon {
            used.push("VRMC_materials_mtoon".to_string());
        }
        if wrote_spring_bone {
            used.push("VRMC_springBone".to_string());
        }
        if wrote_spring_bone_limit {
            used.push("VRMC_springBone_limit".to_string());
        }
        self.root.extensions_used = used;
        Ok(())
    }

    /// VRM 1.0 のプリセット表情名かどうか。
    fn is_preset_expression_name(name: &str) -> bool {
        matches!(
            name,
            "happy"
                | "angry"
                | "sad"
                | "relaxed"
                | "surprised"
                | "aa"
                | "ih"
                | "ou"
                | "ee"
                | "oh"
                | "blink"
                | "blinkLeft"
                | "blinkRight"
                | "lookUp"
                | "lookDown"
                | "lookLeft"
                | "lookRight"
                | "neutral"
        )
    }

    /// 表情（プリセット式）のJSONを作る。
    ///
    /// 束ねのメッシュ・ターゲット・名前・重みを検証し、中間メッシュ索引から
    /// ノード索引へ写して `morphTargetBinds` を作る。`node.weights` は書かず、
    /// 初期値は `mesh.weights` のゼロ列に任せる。
    fn build_expressions_json(
        &self,
    ) -> Result<serde_json::Map<String, serde_json::Value>, VrmExportError> {
        let mut seen: HashSet<&str> = HashSet::new();
        let mut preset = serde_json::Map::new();
        for expression in &self.model.expressions.preset {
            if expression.name.is_empty() {
                return Err(VrmExportError::new(
                    VrmExportErrorKind::InvalidStructure,
                    "表情の名前が空である",
                ));
            }
            if !seen.insert(expression.name.as_str()) {
                return Err(VrmExportError::new(
                    VrmExportErrorKind::InvalidStructure,
                    "表情の名前が重複する",
                ));
            }
            if !Self::is_preset_expression_name(&expression.name) {
                return Err(VrmExportError::new(
                    VrmExportErrorKind::InvalidStructure,
                    format!("表情名がVRM 1.0のプリセット名でない: {}", expression.name),
                ));
            }
            let mut binds = Vec::with_capacity(expression.morph_target_binds.len());
            for bind in &expression.morph_target_binds {
                let mesh = self.model.meshes.get(bind.mesh).ok_or_else(|| {
                    VrmExportError::new(
                        VrmExportErrorKind::InvalidStructure,
                        "表情束ねのメッシュ索引が範囲外である",
                    )
                })?;
                let target = mesh.targets.get(bind.target).ok_or_else(|| {
                    VrmExportError::new(
                        VrmExportErrorKind::InvalidStructure,
                        "表情束ねのターゲット索引が範囲外である",
                    )
                })?;
                if !bind.weight.is_finite() || !(0.0..=1.0).contains(&bind.weight) {
                    return Err(VrmExportError::new(
                        VrmExportErrorKind::InvalidStructure,
                        "表情束ねの重みが0〜1の有限値でない",
                    ));
                }
                let _ = target;
                binds.push(serde_json::json!({
                    "node": mesh.node,
                    "index": bind.target,
                    "weight": bind.weight,
                }));
            }
            preset.insert(
                expression.name.clone(),
                serde_json::json!({
                    "morphTargetBinds": binds,
                    "isBinary": expression.is_binary,
                    "overrideBlink": override_name(expression.override_blink),
                    "overrideLookAt": override_name(expression.override_look_at),
                    "overrideMouth": override_name(expression.override_mouth),
                }),
            );
        }
        let mut out = serde_json::Map::new();
        out.insert("preset".to_string(), serde_json::Value::Object(preset));
        Ok(out)
    }

    /// `VRMC_springBone` 拡張のJSONを作る。
    ///
    /// ノード索引・関節の親子連鎖と一意性・colliderとグループの索引・
    /// 値の有限値と範囲を検証する。`gravityDir` は正規化して書く。
    /// 中心（`center`）の解決は今回の対象外であり、ある場合は索引だけ確かめる。
    fn build_spring_bone_json(&self) -> Result<serde_json::Value, VrmExportError> {
        let node_count = self.model.nodes.len();
        let check_node = |node: usize, where_: &str| -> Result<(), VrmExportError> {
            if node >= node_count {
                return Err(VrmExportError::new(
                    VrmExportErrorKind::InvalidStructure,
                    format!("{where_}のノード索引が範囲外である"),
                )
                .with_node(node));
            }
            Ok(())
        };
        let bones = &self.model.spring_bones;
        let mut colliders = Vec::with_capacity(bones.colliders.len());
        for collider in &bones.colliders {
            check_node(collider.node, "springBone collider")?;
            let shape = match &collider.shape {
                super::VrmSpringColliderShape::Sphere { offset, radius } => {
                    check_shape_point(offset, "springBone collider sphere offset")?;
                    check_radius(*radius, "springBone collider sphere")?;
                    serde_json::json!({
                        "sphere": {
                            "offset": [offset[0], offset[1], offset[2]],
                            "radius": radius,
                        }
                    })
                }
                super::VrmSpringColliderShape::Capsule {
                    offset,
                    radius,
                    tail,
                } => {
                    check_shape_point(offset, "springBone collider capsule offset")?;
                    check_shape_point(tail, "springBone collider capsule tail")?;
                    check_radius(*radius, "springBone collider capsule")?;
                    serde_json::json!({
                        "capsule": {
                            "offset": [offset[0], offset[1], offset[2]],
                            "radius": radius,
                            "tail": [tail[0], tail[1], tail[2]],
                        }
                    })
                }
            };
            colliders.push(serde_json::json!({
                "node": collider.node,
                "shape": shape,
            }));
        }
        let mut groups = Vec::with_capacity(bones.collider_groups.len());
        for group in &bones.collider_groups {
            for collider in &group.colliders {
                if *collider >= colliders.len() {
                    return Err(VrmExportError::new(
                        VrmExportErrorKind::InvalidStructure,
                        "springBone colliderグループの参照が範囲外である",
                    ));
                }
            }
            groups.push(serde_json::json!({
                "name": group.name,
                "colliders": group.colliders,
            }));
        }
        // 関節ノードの二重所有は書式違反のため、springをまたいで確かめる。
        let mut joint_owner: HashMap<usize, &str> = HashMap::new();
        let mut springs = Vec::with_capacity(bones.springs.len());
        for spring in &bones.springs {
            if spring.joints.is_empty() {
                return Err(VrmExportError::new(
                    VrmExportErrorKind::InvalidStructure,
                    format!("関節が無いspringがある: {}", spring.name),
                ));
            }
            let mut joints = Vec::with_capacity(spring.joints.len());
            let mut previous: Option<usize> = None;
            for joint in &spring.joints {
                check_node(joint.node, "springBone joint")?;
                if let Some(owner) = joint_owner.insert(joint.node, spring.name.as_str()) {
                    return Err(VrmExportError::new(
                        VrmExportErrorKind::InvalidStructure,
                        format!(
                            "関節ノードが複数のspringに属する: {}（{}と重なる）",
                            joint.node, owner
                        ),
                    )
                    .with_node(joint.node));
                }
                if let Some(before) = previous
                    && !self.model.nodes[before].children.contains(&joint.node)
                {
                    return Err(VrmExportError::new(
                        VrmExportErrorKind::InvalidStructure,
                        format!(
                            "springの関節が親子でつながらない: {} -> {}",
                            before, joint.node
                        ),
                    )
                    .with_node(joint.node));
                }
                previous = Some(joint.node);
                check_joint_value(joint.hit_radius, "hitRadius", 0.0, f32::INFINITY)?;
                check_joint_value(joint.stiffness, "stiffness", 0.0, f32::INFINITY)?;
                check_joint_value(joint.gravity_power, "gravityPower", 0.0, f32::INFINITY)?;
                check_joint_value(joint.drag_force, "dragForce", 0.0, 1.0)?;
                let gravity_dir = normalize_unit(joint.gravity_dir, "gravityDir")?;
                let mut joint_value = serde_json::json!({
                    "node": joint.node,
                    "hitRadius": joint.hit_radius,
                    "stiffness": joint.stiffness,
                    "gravityPower": joint.gravity_power,
                    "gravityDir": [gravity_dir[0], gravity_dir[1], gravity_dir[2]],
                    "dragForce": joint.drag_force,
                });
                if let Some(limit_value) = build_joint_limit_json(joint.limit.as_ref())? {
                    joint_value
                        .as_object_mut()
                        .expect("関節JSONはオブジェクトである")
                        .insert("extensions".to_string(), limit_value);
                }
                joints.push(joint_value);
            }
            if let Some(center) = spring.center {
                check_node(center, "springBone center")?;
            }
            for group in &spring.collider_groups {
                if *group >= groups.len() {
                    return Err(VrmExportError::new(
                        VrmExportErrorKind::InvalidStructure,
                        format!("springのグループ参照が範囲外である: {}", spring.name),
                    ));
                }
            }
            let mut entry = serde_json::Map::new();
            entry.insert(
                "name".to_string(),
                serde_json::Value::String(spring.name.clone()),
            );
            if let Some(center) = spring.center {
                entry.insert("center".to_string(), serde_json::json!(center));
            }
            entry.insert("joints".to_string(), serde_json::Value::Array(joints));
            // 空の `colliderGroups` は書式違反（`minItems: 1`）のため、無い場合は書かない。
            if !spring.collider_groups.is_empty() {
                entry.insert(
                    "colliderGroups".to_string(),
                    serde_json::json!(spring.collider_groups),
                );
            }
            springs.push(serde_json::Value::Object(entry));
        }
        // 空配列は書式違反（`minItems: 1`）のため、ある場合だけ書く。
        let mut root = serde_json::Map::new();
        root.insert(
            "specVersion".to_string(),
            serde_json::Value::String("1.0".to_string()),
        );
        if !colliders.is_empty() {
            root.insert("colliders".to_string(), serde_json::Value::Array(colliders));
        }
        if !groups.is_empty() {
            root.insert(
                "colliderGroups".to_string(),
                serde_json::Value::Array(groups),
            );
        }
        if !springs.is_empty() {
            root.insert("springs".to_string(), serde_json::Value::Array(springs));
        }
        Ok(serde_json::Value::Object(root))
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
        let view = self.push_view(data, spec.target)?;
        let accessor = gltf::json::Accessor {
            buffer_view: Some(view),
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

    /// バッファへ4バイト境界で詰めて書き、ビューを作る。
    ///
    /// 画像などアクセサを持たない用途で使う。容量検査はアクセサと同じく
    /// BIN単体で行い、超過は追加前に打ち切る。
    fn push_view(
        &mut self,
        data: &[u8],
        target: Option<gltf::json::buffer::Target>,
    ) -> Result<gltf::json::Index<gltf::json::buffer::View>, VrmExportError> {
        let max_output_bytes = self.options.max_output_bytes;
        push_view_into(
            &mut self.bin,
            &mut self.root,
            max_output_bytes,
            data,
            target,
        )
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

    /// UVのアクセサを作る。FBXとUnityの左下原点からglTFの左上原点へ合わせる。
    ///
    /// 中間モデルはUVをそのまま持ち、書き出し時に全UVセットのVを `1 - v` にする。
    /// 画像自体は上下反転しない。位置・法線は変えない。
    fn push_uv_accessor(
        &mut self,
        values: &[[f32; 2]],
    ) -> Result<gltf::json::Index<gltf::json::Accessor>, VrmExportError> {
        let flipped: Vec<[f32; 2]> = values.iter().map(|[u, v]| [*u, 1.0 - *v]).collect();
        self.push_float_accessor(
            &flipped,
            gltf::json::accessor::Type::Vec2,
            None,
            gltf::json::buffer::Target::ArrayBuffer,
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

/// springBoneの形状点が3つの有限値か確かめる。
fn check_shape_point(point: &[f32; 3], where_: &str) -> Result<(), VrmExportError> {
    if !point.iter().all(|v| v.is_finite()) {
        return Err(VrmExportError::new(
            VrmExportErrorKind::InvalidStructure,
            format!("{where_}に非有限値がある"),
        ));
    }
    Ok(())
}

/// springBoneのcollider半径が0以上の有限値か確かめる。
fn check_radius(radius: f32, where_: &str) -> Result<(), VrmExportError> {
    if !radius.is_finite() || radius < 0.0 {
        return Err(VrmExportError::new(
            VrmExportErrorKind::InvalidStructure,
            format!("{where_}が0以上の有限値でない"),
        ));
    }
    Ok(())
}

/// springBoneの関節値が範囲内の有限値か確かめる。
fn check_joint_value(value: f32, field: &str, min: f32, max: f32) -> Result<(), VrmExportError> {
    if !value.is_finite() || value < min || value > max {
        return Err(VrmExportError::new(
            VrmExportErrorKind::InvalidStructure,
            format!("springBone jointの{field}が範囲外である"),
        ));
    }
    Ok(())
}

/// 単位ベクトルを正規化して返す。ゼロや非有限の場合は失敗する。
fn normalize_unit(vector: [f32; 3], field: &str) -> Result<[f32; 3], VrmExportError> {
    let length = (vector[0] * vector[0] + vector[1] * vector[1] + vector[2] * vector[2]).sqrt();
    if !length.is_finite() || length <= 0.0 {
        return Err(VrmExportError::new(
            VrmExportErrorKind::InvalidStructure,
            format!("springBone jointの{field}が正規化できない"),
        ));
    }
    Ok([vector[0] / length, vector[1] / length, vector[2] / length])
}

/// 関節の角度制限が単位四元数か確かめる。非有限・ゼロ長の場合は失敗する。
fn check_quat(rotation: [f32; 4], field: &str) -> Result<[f32; 4], VrmExportError> {
    let length2 = rotation[0] * rotation[0]
        + rotation[1] * rotation[1]
        + rotation[2] * rotation[2]
        + rotation[3] * rotation[3];
    if !length2.is_finite() || length2 <= 0.0 {
        return Err(VrmExportError::new(
            VrmExportErrorKind::InvalidStructure,
            format!("springBone jointの{field}が正規化できない"),
        ));
    }
    let length = length2.sqrt();
    // 既に単位のはずであり、ずれは丸めの範囲に収まる。
    if (length - 1.0).abs() > 1e-3 {
        return Err(VrmExportError::new(
            VrmExportErrorKind::InvalidStructure,
            format!("springBone jointの{field}が単位四元数でない"),
        ));
    }
    Ok([
        rotation[0] / length,
        rotation[1] / length,
        rotation[2] / length,
        rotation[3] / length,
    ])
}

/// 関節の角度制限を `VRMC_springBone_limit` の拡張JSONにする。
/// 制限なしの場合は `None` を返す。角度は有限値で、円錐・蝶番は0〜π、
/// 球面のpitchは0〜π・yawは0〜π/2の範囲を検証する。
/// 回転は単位四元数を検証する。仕様版は `1.0-draft` である。
fn build_joint_limit_json(
    limit: Option<&super::VrmSpringJointLimit>,
) -> Result<Option<serde_json::Value>, VrmExportError> {
    use std::f32::consts::{FRAC_PI_2, PI};
    let Some(limit) = limit else {
        return Ok(None);
    };
    let limit_value = match limit {
        super::VrmSpringJointLimit::Cone { angle, rotation } => {
            check_joint_value(*angle, "limit angle", 0.0, PI)?;
            let rotation = check_quat(*rotation, "limit rotation")?;
            serde_json::json!({
                "cone": {
                    "angle": angle,
                    "rotation": [rotation[0], rotation[1], rotation[2], rotation[3]],
                }
            })
        }
        super::VrmSpringJointLimit::Hinge { angle, rotation } => {
            check_joint_value(*angle, "limit angle", 0.0, PI)?;
            let rotation = check_quat(*rotation, "limit rotation")?;
            serde_json::json!({
                "hinge": {
                    "angle": angle,
                    "rotation": [rotation[0], rotation[1], rotation[2], rotation[3]],
                }
            })
        }
        super::VrmSpringJointLimit::Spherical {
            pitch,
            yaw,
            rotation,
        } => {
            check_joint_value(*pitch, "limit pitch", 0.0, PI)?;
            check_joint_value(*yaw, "limit yaw", 0.0, FRAC_PI_2)?;
            let rotation = check_quat(*rotation, "limit rotation")?;
            serde_json::json!({
                "spherical": {
                    "pitch": pitch,
                    "yaw": yaw,
                    "rotation": [rotation[0], rotation[1], rotation[2], rotation[3]],
                }
            })
        }
    };
    Ok(Some(serde_json::json!({
        "VRMC_springBone_limit": {
            "specVersion": "1.0-draft",
            "limit": limit_value,
        }
    })))
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

/// バッファへ4バイト境界で詰めて書き、ビューを作る。
///
/// `GlbWriter::push_view` の本体であり、`build_images` では画像バイト列の複製を
/// 避けるため、`options` からの借用を保ったまま `bin` と `root` だけを渡して呼ぶ。
/// 容量検査はBIN単体で行い、超過は追加前に打ち切る。4バイト境界の詰め方と
/// 上限検査の挙動は従来どおりである。
fn push_view_into(
    bin: &mut Vec<u8>,
    root: &mut gltf::json::Root,
    max_output_bytes: usize,
    data: &[u8],
    target: Option<gltf::json::buffer::Target>,
) -> Result<gltf::json::Index<gltf::json::buffer::View>, VrmExportError> {
    check_bin_capacity(bin.len(), max_output_bytes, data.len())?;
    let offset = align4(bin.len());
    bin.extend(std::iter::repeat_n(0, offset - bin.len()));
    bin.extend_from_slice(data);
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
        target: target.map(gltf::json::validation::Checked::Valid),
        extensions: None,
        extras: Default::default(),
    };
    let view_index = root.buffer_views.len() as u32;
    root.buffer_views.push(view);
    // 後続のデータが4バイト境界に来るよう詰める。
    let padded = align4(bin.len());
    bin.extend(std::iter::repeat_n(0, padded - bin.len()));
    Ok(gltf::json::Index::new(view_index))
}

/// 次に追加するデータでBINが上限を超える場合に、追加前に打ち切る。
///
/// 比較対象はBIN単体であり、JSONと容器分の超過は最終段の検査に残す。
/// BIN単体で上限を超える場合は全体も必ず超えるため、誤って拒まない。
fn check_bin_capacity(
    bin_len: usize,
    max_output_bytes: usize,
    data_len: usize,
) -> Result<(), VrmExportError> {
    let offset = align4(bin_len);
    let Some(end) = offset.checked_add(data_len) else {
        return Err(VrmExportError::new(
            VrmExportErrorKind::LimitExceeded,
            "出力バイト数が上限を超えた",
        ));
    };
    if align4(end) > max_output_bytes {
        return Err(VrmExportError::new(
            VrmExportErrorKind::LimitExceeded,
            "出力バイト数が上限を超えた",
        ));
    }
    Ok(())
}

/// 材質が無いスロットの灰色PBR材質を作る。
///
/// 材質名はスロット名（空なら `Material`）、不透明、片面、
/// 金属度0・粗さ1の中間調の灰色である。
fn gray_material(slot: &str) -> gltf::json::Material {
    gltf::json::Material {
        alpha_mode: gltf::json::validation::Checked::Valid(gltf::json::material::AlphaMode::Opaque),
        double_sided: false,
        name: Some(placeholder(slot).name),
        pbr_metallic_roughness: gltf::json::material::PbrMetallicRoughness {
            base_color_factor: gltf::json::material::PbrBaseColorFactor([0.8, 0.8, 0.8, 1.0]),
            metallic_factor: gltf::json::material::StrengthFactor(0.0),
            roughness_factor: gltf::json::material::StrengthFactor(1.0),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// サンプラ値からglTFサンプラを作る。
///
/// 拡大は点補間かそれ以外かだけを区別し、縮小はmipmapの有無と組み合わせる。
fn sampler_json(sampler: &VrmSampler) -> gltf::json::texture::Sampler {
    let wrap = |wrap: VrmWrap| match wrap {
        VrmWrap::Repeat => gltf::json::texture::WrappingMode::Repeat,
        VrmWrap::ClampToEdge => gltf::json::texture::WrappingMode::ClampToEdge,
        VrmWrap::MirroredRepeat => gltf::json::texture::WrappingMode::MirroredRepeat,
    };
    let mag = match sampler.filter {
        VrmFilter::Nearest => gltf::json::texture::MagFilter::Nearest,
        VrmFilter::Linear | VrmFilter::Trilinear => gltf::json::texture::MagFilter::Linear,
    };
    let min = match (sampler.filter, sampler.mipmaps) {
        (VrmFilter::Nearest, true) => gltf::json::texture::MinFilter::NearestMipmapNearest,
        (VrmFilter::Nearest, false) => gltf::json::texture::MinFilter::Nearest,
        (VrmFilter::Linear, true) => gltf::json::texture::MinFilter::LinearMipmapNearest,
        (VrmFilter::Linear, false) => gltf::json::texture::MinFilter::Linear,
        (VrmFilter::Trilinear, true) => gltf::json::texture::MinFilter::LinearMipmapLinear,
        (VrmFilter::Trilinear, false) => gltf::json::texture::MinFilter::Linear,
    };
    gltf::json::texture::Sampler {
        mag_filter: Some(gltf::json::validation::Checked::Valid(mag)),
        min_filter: Some(gltf::json::validation::Checked::Valid(min)),
        name: None,
        wrap_s: gltf::json::validation::Checked::Valid(wrap(sampler.wrap_u)),
        wrap_t: gltf::json::validation::Checked::Valid(wrap(sampler.wrap_v)),
        extensions: None,
        extras: Default::default(),
    }
}

/// MToon値から `VRMC_materials_mtoon` 1.0の拡張を作る。
///
/// テクスチャは `None` の項目を省き、`outlineWidthMode` は
/// `none` か `worldCoordinates` を書く。
fn mtoon_extension(
    parameters: &VrmMtoonParameters,
    textures: &[gltf::json::Index<gltf::json::texture::Texture>],
) -> Result<gltf::json::extensions::material::Material, VrmExportError> {
    let texture_of = |image: Option<usize>| -> Result<Option<serde_json::Value>, VrmExportError> {
        image
            .map(|index| {
                textures
                    .get(index)
                    .map(|texture| serde_json::json!({ "index": texture.value() }))
                    .ok_or_else(|| {
                        VrmExportError::new(
                            VrmExportErrorKind::InvalidStructure,
                            "MToonの画像索引が範囲外である",
                        )
                    })
            })
            .transpose()
    };
    let mut mtoon = serde_json::Map::new();
    mtoon.insert("specVersion".to_string(), serde_json::json!("1.0"));
    mtoon.insert(
        "transparentWithZWrite".to_string(),
        serde_json::json!(parameters.transparent_with_z_write),
    );
    mtoon.insert(
        "renderQueueOffsetNumber".to_string(),
        serde_json::json!(parameters.render_queue_offset),
    );
    mtoon.insert(
        "shadeColorFactor".to_string(),
        serde_json::json!(parameters.shade_color_factor),
    );
    if let Some(texture) = texture_of(parameters.shade_multiply_image)? {
        mtoon.insert("shadeMultiplyTexture".to_string(), texture);
    }
    mtoon.insert(
        "shadingShiftFactor".to_string(),
        serde_json::json!(parameters.shading_shift_factor),
    );
    mtoon.insert(
        "shadingToonyFactor".to_string(),
        serde_json::json!(parameters.shading_toony_factor),
    );
    mtoon.insert(
        "giEqualizationFactor".to_string(),
        serde_json::json!(parameters.gi_equalization_factor),
    );
    mtoon.insert(
        "matcapFactor".to_string(),
        serde_json::json!(parameters.matcap_factor),
    );
    if let Some(texture) = texture_of(parameters.matcap_image)? {
        mtoon.insert("matcapTexture".to_string(), texture);
    }
    let outline_width_mode = match parameters.outline_width_mode {
        super::material::VrmOutlineWidthMode::None => "none",
        super::material::VrmOutlineWidthMode::WorldCoordinates => "worldCoordinates",
    };
    mtoon.insert(
        "outlineWidthMode".to_string(),
        serde_json::json!(outline_width_mode),
    );
    mtoon.insert(
        "outlineWidthFactor".to_string(),
        serde_json::json!(parameters.outline_width_factor),
    );
    if let Some(texture) = texture_of(parameters.outline_width_multiply_image)? {
        mtoon.insert("outlineWidthMultiplyTexture".to_string(), texture);
    }
    mtoon.insert(
        "outlineColorFactor".to_string(),
        serde_json::json!(parameters.outline_color_factor),
    );
    mtoon.insert(
        "outlineLightingMixFactor".to_string(),
        serde_json::json!(parameters.outline_lighting_mix_factor),
    );
    let mut others = serde_json::Map::new();
    others.insert(
        "VRMC_materials_mtoon".to_string(),
        serde_json::Value::Object(mtoon),
    );
    Ok(gltf::json::extensions::material::Material { others })
}
/// 式の上書き指定をJSON名へ写す。
fn override_name(value: super::VrmExpressionOverride) -> &'static str {
    match value {
        super::VrmExpressionOverride::None => "none",
        super::VrmExpressionOverride::Block => "block",
        super::VrmExpressionOverride::Blend => "blend",
    }
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
    for target in &mesh.targets {
        if target.name.is_empty() {
            return Err(VrmExportError::new(
                VrmExportErrorKind::InvalidStructure,
                "モーフターゲットの名前が空である",
            ));
        }
        if target.positions.len() != count {
            return Err(VrmExportError::new(
                VrmExportErrorKind::InvalidStructure,
                "モーフターゲットの長さが位置と一致しない",
            ));
        }
        if target
            .positions
            .iter()
            .any(|position| position.iter().any(|value| !value.is_finite()))
        {
            return Err(VrmExportError::new(
                VrmExportErrorKind::InvalidStructure,
                "モーフターゲットに有限でない値がある",
            ));
        }
    }
    Ok(())
}
