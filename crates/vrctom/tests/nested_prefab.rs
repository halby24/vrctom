//! ネストしたPrefabに対するメッシュ書き出しの契約テスト（公開 API のみを使う）。
//!
//! 非スキン/スキン・多段ネスト・複数配置・上書き（材質/TRS/有効状態）・
//! 直接記述・ローカル中間ノード・材質重複排除・欠損参照警告・循環/上限・旧方式IDを検証する。

#![cfg(not(target_arch = "wasm32"))]

use std::collections::HashMap;

use vrctom::conversion::prefab::{
    PrefabConversionLimits, PrefabConversionOptions, PrefabConversionStage, PrefabExpressions,
    PrefabPhysics, convert_prefab,
};
use vrctom::fbx::FbxOptions;
use vrctom::unity::{
    AssetIndex, AssetMeta, AssetSource, game_object_file_id, mesh_file_id, mesh_renderer_file_id,
    skinned_mesh_renderer_file_id, transform_file_id,
};
use vrctom::vrm::{MaterialOptions, VrmExportOptions, VrmMeta, VrmOptions};

// テスト名は日本語で書く（テストは仕様・契約であるため）。

/// メモリ内の読み手。
struct 記憶読み手 {
    表: HashMap<String, Vec<u8>>,
}

impl AssetSource for 記憶読み手 {
    fn read_limited(&self, path: &str, limit: u64) -> std::io::Result<Vec<u8>> {
        let Some(bytes) = self.表.get(path) else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("持たない: {path}"),
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

const MAIN_FBX_GUID: &str = "fb00000000000000000000000000000b";
const PROP_FBX_GUID: &str = "fb00000000000000000000000000000c";
const SKIN_PROP_FBX_GUID: &str = "fb00000000000000000000000000000e";
const AVATAR_PREFAB_GUID: &str = "a0000000000000000000000000000007";
const NEST_PREFAB_GUID: &str = "0e00000000000000000000000000000d";
const OUTER_PREFAB_GUID: &str = "0e00000000000000000000000000000f";
const MAT_MAIN_GUID: &str = "b0d71111111111111111111111111111";
const MAT_PROP_GUID: &str = "b0d72222222222222222222222222222";
const MAT_ALT_GUID: &str = "b0d73333333333333333333333333333";
const SHADER_OPAQUE: &str = "df12117ecd77c31469c224178886498e";

#[derive(Default)]
struct Props {
    buf: Vec<u8>,
    count: u32,
}

impl Props {
    fn prop(&mut self, code: u8, body: &[u8]) {
        self.buf.push(code);
        self.buf.extend_from_slice(body);
        self.count += 1;
    }

    fn l(&mut self, v: i64) {
        self.prop(b'L', &v.to_le_bytes());
    }

    fn d(&mut self, v: f64) {
        self.prop(b'D', &v.to_le_bytes());
    }

    fn i(&mut self, v: i32) {
        self.prop(b'I', &v.to_le_bytes());
    }

    fn s(&mut self, v: &[u8]) {
        let mut body = (v.len() as u32).to_le_bytes().to_vec();
        body.extend_from_slice(v);
        self.prop(b'S', &body);
    }

    fn array(&mut self, code: u8, count: u32, payload: &[u8]) {
        let mut body = count.to_le_bytes().to_vec();
        body.extend_from_slice(&0u32.to_le_bytes());
        body.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        body.extend_from_slice(payload);
        self.prop(code, &body);
    }

    fn d_raw(&mut self, values: &[f64]) {
        let mut payload = Vec::with_capacity(values.len() * 8);
        for v in values {
            payload.extend_from_slice(&v.to_le_bytes());
        }
        self.array(b'd', values.len() as u32, &payload);
    }

    fn i_raw(&mut self, values: &[i32]) {
        let mut payload = Vec::with_capacity(values.len() * 4);
        for v in values {
            payload.extend_from_slice(&v.to_le_bytes());
        }
        self.array(b'i', values.len() as u32, &payload);
    }
}

struct TNode {
    name: Vec<u8>,
    props: Props,
    children: Vec<TNode>,
}

impl TNode {
    fn leaf(name: &[u8], props: Props) -> Self {
        Self {
            name: name.to_vec(),
            props,
            children: Vec::new(),
        }
    }

    fn branch(name: &[u8], props: Props, children: Vec<TNode>) -> Self {
        Self {
            name: name.to_vec(),
            props,
            children,
        }
    }
}

fn ser(node: &TNode, buf: &mut Vec<u8>) {
    let start = buf.len();
    buf.extend_from_slice(&[0u8; 13]);
    buf.extend_from_slice(&node.name);
    buf.extend_from_slice(&node.props.buf);
    for child in &node.children {
        ser(child, buf);
    }
    if !node.children.is_empty() {
        buf.extend_from_slice(&[0u8; 13]);
    }
    let end = buf.len() as u32;
    buf[start..start + 4].copy_from_slice(&end.to_le_bytes());
    buf[start + 4..start + 8].copy_from_slice(&node.props.count.to_le_bytes());
    buf[start + 8..start + 12].copy_from_slice(&(node.props.buf.len() as u32).to_le_bytes());
    buf[start + 12] = node.name.len() as u8;
}

fn file_fbx(top: Vec<TNode>) -> Vec<u8> {
    let mut buf = b"Kaydara FBX Binary  \x00\x1a\x00".to_vec();
    buf.extend_from_slice(&7400u32.to_le_bytes());
    for node in &top {
        ser(node, &mut buf);
    }
    buf.extend_from_slice(&[0u8; 13]);
    buf
}

fn fbx_p_unit_scale(unit_scale: f64) -> TNode {
    let mut prop = Props::default();
    prop.s(b"UnitScaleFactor");
    prop.s(b"double");
    prop.s(b"Number");
    prop.s(b"");
    prop.d(unit_scale);
    let p70 = TNode::branch(
        b"Properties70",
        Props::default(),
        vec![TNode::leaf(b"P", prop)],
    );
    TNode::branch(b"GlobalSettings", Props::default(), vec![p70])
}

fn fbx_p_vec3(name: &[u8], xyz: [f64; 3]) -> TNode {
    let mut prop = Props::default();
    prop.s(name);
    prop.s(b"Lcl Translation");
    prop.s(b"");
    prop.s(b"A");
    prop.d(xyz[0]);
    prop.d(xyz[1]);
    prop.d(xyz[2]);
    TNode::leaf(b"P", prop)
}

fn model_with_translation(id: i64, name: &str, subtype: &str, pos: [f64; 3]) -> TNode {
    let mut props = Props::default();
    props.l(id);
    props.s(format!("{name}\x00\x01Model").as_bytes());
    props.s(subtype.as_bytes());
    let children = if pos == [0.0, 0.0, 0.0] {
        Vec::new()
    } else {
        vec![TNode::branch(
            b"Properties70",
            Props::default(),
            vec![fbx_p_vec3(b"Lcl Translation", pos)],
        )]
    };
    TNode::branch(b"Model", props, children)
}

fn material_node(id: i64, name: &str) -> TNode {
    let mut props = Props::default();
    props.l(id);
    props.s(format!("{name}\x00\x01Material").as_bytes());
    TNode::leaf(b"Material", props)
}

fn geometry(id: i64, name: &str, verts: &[f64], pvi: &[i32]) -> TNode {
    let mut props = Props::default();
    props.l(id);
    props.s(format!("{name}\x00\x01Geometry").as_bytes());
    props.s(b"Mesh");
    let mut vp = Props::default();
    vp.d_raw(verts);
    let mut pp = Props::default();
    pp.i_raw(pvi);
    TNode::branch(
        b"Geometry",
        props,
        vec![
            TNode::leaf(b"Vertices", vp),
            TNode::leaf(b"PolygonVertexIndex", pp),
        ],
    )
}

fn conn(child: i64, parent: i64) -> TNode {
    let mut props = Props::default();
    props.s(b"OO");
    props.l(child);
    props.l(parent);
    TNode::leaf(b"C", props)
}

fn pose_node(model: i64, matrix: &[f64; 16]) -> TNode {
    let mut mp = Props::default();
    mp.d_raw(matrix);
    let mut np = Props::default();
    np.l(model);
    TNode::branch(
        b"PoseNode",
        Props::default(),
        vec![TNode::leaf(b"Node", np), TNode::leaf(b"Matrix", mp)],
    )
}

fn bind_matrix(x: f64, y: f64, z: f64) -> [f64; 16] {
    [
        1.0, 0.0, 0.0, 0.0, //
        0.0, 1.0, 0.0, 0.0, //
        0.0, 0.0, 1.0, 0.0, //
        x, y, z, 1.0, //
    ]
}

fn 必須骨モデル列() -> Vec<(i64, &'static str, Option<i64>, [f64; 3])> {
    vec![
        (1, "Hips", None, [0.0, 0.0, 0.0]),
        (2, "Spine", Some(1), [0.0, 0.0, 0.0]),
        (3, "Head", Some(2), [0.0, 0.0, 0.0]),
        (4, "LeftUpperArm", Some(2), [0.0, 0.0, 0.0]),
        (5, "LeftLowerArm", Some(4), [0.0, 0.0, 0.0]),
        // LeftHand は Y=+0.5m（UnitScaleFactor=100のときは0.5, 1のときは50）にして配置差分を確かめやすくする。
        (6, "LeftHand", Some(5), [0.0, 0.5, 0.0]),
        (7, "RightUpperArm", Some(2), [0.0, 0.0, 0.0]),
        (8, "RightLowerArm", Some(7), [0.0, 0.0, 0.0]),
        (9, "RightHand", Some(8), [0.0, 0.5, 0.0]),
        (10, "LeftUpperLeg", Some(1), [0.0, 0.0, 0.0]),
        (11, "LeftLowerLeg", Some(10), [0.0, 0.0, 0.0]),
        (12, "LeftFoot", Some(11), [0.0, 0.0, 0.0]),
        (13, "RightUpperLeg", Some(1), [0.0, 0.0, 0.0]),
        (14, "RightLowerLeg", Some(13), [0.0, 0.0, 0.0]),
        (15, "RightFoot", Some(14), [0.0, 0.0, 0.0]),
        (16, "Owner", Some(1), [0.0, 0.0, 0.0]),
    ]
}

fn 主fbxを組み立てる(unit_scale: f64) -> Vec<u8> {
    let 骨 = 必須骨モデル列();
    let mut objs: Vec<TNode> = 骨
        .iter()
        .map(|(id, name, _, pos)| {
            let subtype = if *name == "Owner" { "Mesh" } else { "LimbNode" };
            model_with_translation(*id, name, subtype, *pos)
        })
        .collect();
    objs.push(material_node(300, "MatMain"));
    objs.push(geometry(
        100,
        "BodyTri",
        &[0.0, 0.0, 0.0, 0.1, 0.0, 0.0, 0.0, 0.1, 0.0],
        &[0, 1, -3],
    ));
    let mut pose_props = Props::default();
    pose_props.l(200);
    pose_props.s(b"Bind\x00\x01Pose");
    pose_props.s(b"BindPose");
    let binds: Vec<TNode> = 骨
        .iter()
        .map(|(id, _, _, pos)| pose_node(*id, &bind_matrix(pos[0], pos[1], pos[2])))
        .collect();
    objs.push(TNode::branch(b"Pose", pose_props, binds));
    let mut conns: Vec<TNode> = 骨
        .iter()
        .filter_map(|(id, _, parent, _)| parent.map(|p| conn(*id, p)))
        .collect();
    conns.push(conn(300, 16));
    conns.push(conn(100, 16));
    conns.push(conn(1, 0));
    file_fbx(vec![
        fbx_p_unit_scale(unit_scale),
        TNode::branch(b"Objects", Props::default(), objs),
        TNode::branch(b"Connections", Props::default(), conns),
    ])
}

/// BindPose を持たない非スキン小物FBXを組み立てる。
fn 非スキン小物fbxを組み立てる(
    unit_scale: f64,
    model_name: &str,
    verts: &[f64],
) -> Vec<u8> {
    let objs = vec![
        model_with_translation(10, model_name, "Mesh", [0.0, 0.0, 0.0]),
        material_node(20, "PropMat"),
        geometry(30, "PropGeo", verts, &[0, 1, -3]),
    ];
    let conns = vec![conn(10, 0), conn(20, 10), conn(30, 10)];
    file_fbx(vec![
        fbx_p_unit_scale(unit_scale),
        TNode::branch(b"Objects", Props::default(), objs),
        TNode::branch(b"Connections", Props::default(), conns),
    ])
}

/// スキン付きの小物FBXを組み立てる（BindPose + Skin + Cluster付き）。
fn スキン付き小物fbxを組み立てる(unit_scale: f64) -> Vec<u8> {
    let mut deformer_skin_props = Props::default();
    deformer_skin_props.l(40);
    deformer_skin_props.s(b"Skin\x00\x01Deformer");
    deformer_skin_props.s(b"Skin");
    let skin_node = TNode::branch(b"Deformer", deformer_skin_props, Vec::new());

    let mut cluster_props = Props::default();
    cluster_props.l(41);
    cluster_props.s(b"Cluster\x00\x01SubDeformer");
    cluster_props.s(b"Cluster");
    let mut idx_p = Props::default();
    idx_p.i_raw(&[0, 1, 2]);
    let mut w_p = Props::default();
    w_p.d_raw(&[1.0, 1.0, 1.0]);
    let mut tm_p = Props::default();
    tm_p.d_raw(&bind_matrix(0.0, 0.0, 0.0));
    let mut tl_p = Props::default();
    tl_p.d_raw(&bind_matrix(0.0, 2.0, 0.0));
    let cluster_node = TNode::branch(
        b"Deformer",
        cluster_props,
        vec![
            TNode::leaf(b"Indexes", idx_p),
            TNode::leaf(b"Weights", w_p),
            TNode::leaf(b"Transform", tm_p),
            TNode::leaf(b"TransformLink", tl_p),
        ],
    );

    let mut pose_props = Props::default();
    pose_props.l(50);
    pose_props.s(b"Bind\x00\x01Pose");
    pose_props.s(b"BindPose");
    let pose = TNode::branch(
        b"Pose",
        pose_props,
        vec![
            pose_node(10, &bind_matrix(0.0, 2.0, 0.0)),
            pose_node(11, &bind_matrix(0.0, 0.0, 0.0)),
        ],
    );

    let objs = vec![
        model_with_translation(10, "RibbonBone", "LimbNode", [0.0, 2.0, 0.0]),
        model_with_translation(11, "RibbonMesh", "Mesh", [0.0, 0.0, 0.0]),
        material_node(20, "RibbonMat"),
        geometry(
            30,
            "RibbonGeo",
            &[0.0, 2.0, 0.0, 1.0, 2.0, 0.0, 0.0, 3.0, 0.0],
            &[0, 1, -3],
        ),
        skin_node,
        cluster_node,
        pose,
    ];
    let conns = vec![
        conn(10, 0),
        conn(11, 0),
        conn(20, 11),
        conn(30, 11),
        conn(40, 30),
        conn(41, 40),
        conn(10, 41),
    ];
    let _ = Props::i;
    file_fbx(vec![
        fbx_p_unit_scale(unit_scale),
        TNode::branch(b"Objects", Props::default(), objs),
        TNode::branch(b"Connections", Props::default(), conns),
    ])
}

fn メタを作る(guid: &str) -> AssetMeta {
    AssetMeta::parse(
        format!("fileFormatVersion: 2\nguid: {guid}\nModelImporter:\n  externalObjects: []\n")
            .as_bytes(),
    )
    .expect("検査metaは読める")
}

fn 置換表付きfbxメタ(guid: &str, slot: &str, mat_guid: &str) -> AssetMeta {
    AssetMeta::parse(
        format!(
            "fileFormatVersion: 2\nguid: {guid}\nModelImporter:\n  externalObjects:\n  - first:\n      type: UnityEngine:Material\n      name: {slot}\n    second: {{fileID: 2100000, guid: {mat_guid}, type: 2}}\n"
        )
        .as_bytes(),
    )
    .expect("置換表metaは読める")
}

fn 旧方式id付きfbxメタ(guid: &str, legacy_lines: &str) -> AssetMeta {
    AssetMeta::parse(
        format!(
            "fileFormatVersion: 2\nguid: {guid}\nModelImporter:\n  serializedVersion: 19\n  fileIDToRecycleName:\n{legacy_lines}  externalObjects: []\n"
        )
        .as_bytes(),
    )
    .expect("旧方式metaは読める")
}

fn prefabメタを作る(guid: &str) -> AssetMeta {
    AssetMeta::parse(
        format!("fileFormatVersion: 2\nguid: {guid}\nPrefabImporter:\n  externalObjects: []\n")
            .as_bytes(),
    )
    .expect("prefab metaは読める")
}

fn matメタを作る(guid: &str) -> AssetMeta {
    AssetMeta::parse(
        format!(
            "fileFormatVersion: 2\nguid: {guid}\nNativeFormatImporter:\n  externalObjects: {{}}\n  userData: \n"
        )
        .as_bytes(),
    )
    .expect("mat metaは読める")
}

fn 検査材質を作る(名前: &str, r: f64, g: f64, b: f64) -> Vec<u8> {
    format!(
        "%YAML 1.1\n%TAG !u! tag:unity3d.com,2011:\n--- !u!21 &2100000\nMaterial:\n  m_Name: {名前}\n  m_Shader: {{fileID: 4800000, guid: {SHADER_OPAQUE}, type: 3}}\n  m_SavedProperties:\n    m_TexEnvs: []\n    m_Floats:\n    - _Cutoff: 0.5\n    - _Cull: 2.0\n    - _ZWrite: 1.0\n    - _UseShadow: 1.0\n    - _ShadowBorder: 0.5\n    - _ShadowBlur: 0.1\n    - _lilToonVersion: 45.0\n    m_Ints: []\n    m_Colors:\n    - _Color: {{r: {r}, g: {g}, b: {b}, a: 1}}\n    - _ShadowColor: {{r: 0.82, g: 0.76, b: 0.85, a: 1}}\n"
    )
    .into_bytes()
}

fn 索引と読み手を作る(
    件: &[(&str, AssetMeta, Option<Vec<u8>>)],
) -> (AssetIndex, 記憶読み手) {
    let mut index = AssetIndex::new();
    let mut 表 = HashMap::new();
    for (path, meta, body) in 件 {
        index.insert(path, meta.clone()).expect("索引に入る");
        if let Some(body) = body {
            表.insert((*path).to_owned(), body.clone());
        }
    }
    (index, 記憶読み手 { 表 })
}

fn prefab結合(文書列: &[String]) -> Vec<u8> {
    const DIRECTIVE: &str = "%YAML 1.1\n%TAG !u! tag:unity3d.com,2011:\n";
    let mut out = String::new();
    for (i, d) in 文書列.iter().enumerate() {
        if i == 0 {
            out.push_str(d);
        } else {
            out.push_str(d.strip_prefix(DIRECTIVE).expect("directive"));
        }
    }
    out.into_bytes()
}

fn 実体文書(anchor: i64, parent_fid: i64, source_guid: &str, mods: &str) -> String {
    format!(
        "%YAML 1.1\n%TAG !u! tag:unity3d.com,2011:\n--- !u!1001 &{anchor}\nPrefabInstance:\n  m_ObjectHideFlags: 0\n  serializedVersion: 2\n  m_Modification:\n    serializedVersion: 3\n    m_TransformParent: {{fileID: {parent_fid}}}\n    m_Modifications:\n{mods}  m_SourcePrefab: {{fileID: 100100000, guid: {source_guid}, type: 3}}\n"
    )
}

fn stripped_tr文書(anchor: i64, source_fid: i64, source_guid: &str, inst_fid: i64) -> String {
    format!(
        "%YAML 1.1\n%TAG !u! tag:unity3d.com,2011:\n--- !u!4 &{anchor} stripped\nTransform:\n  m_CorrespondingSourceObject: {{fileID: {source_fid}, guid: {source_guid}, type: 3}}\n  m_PrefabInstance: {{fileID: {inst_fid}}}\n"
    )
}

fn 材質上書き行(target_fid: i64, target_guid: &str, slot: usize, mat_guid: &str) -> String {
    format!(
        "    - target: {{fileID: {target_fid}, guid: {target_guid}, type: 3}}\n      propertyPath: m_Materials.Array.data[{slot}]\n      value: \n      objectReference: {{fileID: 2100000, guid: {mat_guid}, type: 2}}\n"
    )
}

fn 数値上書き行(target_fid: i64, target_guid: &str, path: &str, val: f64) -> String {
    format!(
        "    - target: {{fileID: {target_fid}, guid: {target_guid}, type: 3}}\n      propertyPath: {path}\n      value: {val}\n      objectReference: {{fileID: 0}}\n"
    )
}

fn 整数上書き行(target_fid: i64, target_guid: &str, path: &str, val: i64) -> String {
    format!(
        "    - target: {{fileID: {target_fid}, guid: {target_guid}, type: 3}}\n      propertyPath: {path}\n      value: {val}\n      objectReference: {{fileID: 0}}\n"
    )
}

fn glbからjsonとbinを取り出す(glb: &[u8]) -> (serde_json::Value, Vec<u8>) {
    assert_eq!(&glb[0..4], b"glTF");
    let json_len = u32::from_le_bytes(glb[12..16].try_into().unwrap()) as usize;
    let json_val: serde_json::Value =
        serde_json::from_slice(&glb[20..20 + json_len]).expect("JSONが読める");
    let bin_offset = 20 + json_len;
    let bin_len = u32::from_le_bytes(glb[bin_offset..bin_offset + 4].try_into().unwrap()) as usize;
    let bin = glb[bin_offset + 8..bin_offset + 8 + bin_len].to_vec();
    (json_val, bin)
}

fn accessor_vec3(json: &serde_json::Value, bin: &[u8], accessor_idx: usize) -> Vec<[f32; 3]> {
    let acc = &json["accessors"][accessor_idx];
    let bv = &json["bufferViews"][acc["bufferView"].as_u64().unwrap() as usize];
    let offset = bv["byteOffset"].as_u64().unwrap_or(0) as usize
        + acc["byteOffset"].as_u64().unwrap_or(0) as usize;
    let count = acc["count"].as_u64().unwrap() as usize;
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let base = offset + i * 12;
        out.push([
            f32::from_le_bytes(bin[base..base + 4].try_into().unwrap()),
            f32::from_le_bytes(bin[base + 4..base + 8].try_into().unwrap()),
            f32::from_le_bytes(bin[base + 8..base + 12].try_into().unwrap()),
        ]);
    }
    out
}

fn accessor_mat4(json: &serde_json::Value, bin: &[u8], accessor_idx: usize) -> Vec<[f32; 16]> {
    let acc = &json["accessors"][accessor_idx];
    let bv = &json["bufferViews"][acc["bufferView"].as_u64().unwrap() as usize];
    let offset = bv["byteOffset"].as_u64().unwrap_or(0) as usize
        + acc["byteOffset"].as_u64().unwrap_or(0) as usize;
    let count = acc["count"].as_u64().unwrap() as usize;
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let mut m = [0.0f32; 16];
        for (j, slot) in m.iter_mut().enumerate() {
            let base = offset + i * 64 + j * 4;
            *slot = f32::from_le_bytes(bin[base..base + 4].try_into().unwrap());
        }
        out.push(m);
    }
    out
}

fn gltf_node_world_matrices(json: &serde_json::Value) -> Vec<[f64; 16]> {
    let nodes = json["nodes"].as_array().unwrap();
    let mut locals = Vec::with_capacity(nodes.len());
    let mut parents = vec![None; nodes.len()];
    for (i, n) in nodes.iter().enumerate() {
        let m = if let Some(arr) = n.get("matrix").and_then(|v| v.as_array()) {
            let mut out = [0.0f64; 16];
            for (k, val) in arr.iter().enumerate() {
                out[k] = val.as_f64().unwrap();
            }
            out
        } else {
            [
                1.0, 0.0, 0.0, 0.0, //
                0.0, 1.0, 0.0, 0.0, //
                0.0, 0.0, 1.0, 0.0, //
                0.0, 0.0, 0.0, 1.0, //
            ]
        };
        locals.push(m);
        if let Some(children) = n.get("children").and_then(|v| v.as_array()) {
            for c in children {
                parents[c.as_u64().unwrap() as usize] = Some(i);
            }
        }
    }
    fn mul(a: &[f64; 16], b: &[f64; 16]) -> [f64; 16] {
        let mut out = [0.0; 16];
        for r in 0..4 {
            for c in 0..4 {
                let mut s = 0.0;
                for k in 0..4 {
                    s += a[k * 4 + r] * b[c * 4 + k];
                }
                out[c * 4 + r] = s;
            }
        }
        out
    }
    let mut worlds = Vec::with_capacity(nodes.len());
    for i in 0..nodes.len() {
        let mut chain = vec![i];
        let mut cur = i;
        while let Some(p) = parents[cur] {
            chain.push(p);
            cur = p;
        }
        let mut w = locals[chain.pop().unwrap()];
        for idx in chain.into_iter().rev() {
            w = mul(&w, &locals[idx]);
        }
        worlds.push(w);
    }
    worlds
}

fn 変換設定(expressions: PrefabExpressions, physics: PrefabPhysics) -> PrefabConversionOptions {
    PrefabConversionOptions {
        expressions,
        baseline: false,
        physics,
        tpose: true,
        material_options: MaterialOptions::new(),
        limits: PrefabConversionLimits::new(),
    }
}

fn 書き出し設定() -> VrmExportOptions {
    VrmExportOptions::new(VrmMeta::new("試験", vec!["作者".to_owned()]))
}

#[test]
fn 非スキンネストprefabが主ボーン配下の正しい世界位置と材質で書き出される() {
    // 主FBXはcm単位（UnitScaleFactor=1、1/100=0.01m、LeftHandのY=50 → 0.5m）。
    // 副FBX（BindPose無し）はm単位（UnitScaleFactor=100、100/100=1.0m）で、
    // 頂点 (1, 0, 0) を持つ MeshRenderer オブジェクト。
    // Nest.prefab は Prop.fbx を置き、Avatar.prefab は Nest.prefab を LeftHand 配下に
    // Unity座標 (0.1, 0.2, 0.3)m で配置する。
    let main_fbx = 主fbxを組み立てる(1.0);
    let prop_fbx = 非スキン小物fbxを組み立てる(
        100.0,
        "Wand",
        &[1.0, 0.0, 0.0, 2.0, 0.0, 0.0, 1.0, 1.0, 0.0],
    );
    let left_hand_tr = transform_file_id("Hips/Spine/LeftUpperArm/LeftLowerArm/LeftHand");
    let main_mr = skinned_mesh_renderer_file_id("Hips/Owner");
    let prop_mr = mesh_renderer_file_id("Wand");

    let nest_body = 実体文書(
        200,
        0,
        PROP_FBX_GUID,
        &材質上書き行(prop_mr, PROP_FBX_GUID, 0, MAT_PROP_GUID),
    );
    let avatar_body = prefab結合(&[
        実体文書(
            100,
            0,
            MAIN_FBX_GUID,
            &材質上書き行(main_mr, MAIN_FBX_GUID, 0, MAT_MAIN_GUID),
        ),
        stripped_tr文書(150, left_hand_tr, MAIN_FBX_GUID, 100),
        実体文書(
            300,
            150,
            NEST_PREFAB_GUID,
            &format!(
                "{}{}{}",
                数値上書き行(
                    transform_file_id(""),
                    PROP_FBX_GUID,
                    "m_LocalPosition.x",
                    0.1
                ),
                数値上書き行(
                    transform_file_id(""),
                    PROP_FBX_GUID,
                    "m_LocalPosition.y",
                    0.2
                ),
                数値上書き行(
                    transform_file_id(""),
                    PROP_FBX_GUID,
                    "m_LocalPosition.z",
                    0.3
                ),
            ),
        ),
    ]);

    let (index, 読み手) = 索引と読み手を作る(&[
        ("Main.fbx", メタを作る(MAIN_FBX_GUID), None),
        ("Prop.fbx", メタを作る(PROP_FBX_GUID), Some(prop_fbx)),
        (
            "Nest.prefab",
            prefabメタを作る(NEST_PREFAB_GUID),
            Some(nest_body.into_bytes()),
        ),
        (
            "Avatar.prefab",
            prefabメタを作る(AVATAR_PREFAB_GUID),
            Some(avatar_body),
        ),
        (
            "MatMain.mat",
            matメタを作る(MAT_MAIN_GUID),
            Some(検査材質を作る("主材質", 1.0, 1.0, 1.0)),
        ),
        (
            "MatProp.mat",
            matメタを作る(MAT_PROP_GUID),
            Some(検査材質を作る("小物材質", 0.0, 1.0, 0.0)),
        ),
    ]);

    let (glb, 報告) = convert_prefab(
        &読み手,
        &index,
        "Assets/Avatar.prefab",
        &main_fbx,
        &FbxOptions::new(),
        &VrmOptions::new(),
        書き出し設定(),
        &変換設定(PrefabExpressions::None, PrefabPhysics::None),
    )
    .expect("非スキンネストPrefabを変換できる");

    assert_eq!(報告.active_meshes, 2);
    assert_eq!(報告.meshes, 2);
    assert_eq!(報告.materials, 2);
    assert!(報告.warnings.is_empty(), "警告なし: {:?}", 報告.warnings);

    let (json, bin) = glbからjsonとbinを取り出す(&glb);
    let worlds = gltf_node_world_matrices(&json);
    let nodes = json["nodes"].as_array().unwrap();
    let wand_node_idx = nodes
        .iter()
        .position(|n| n["name"] == "Wand" && n.get("mesh").is_some())
        .expect("Wandメッシュノードが出力されている");
    let wand_mesh_idx = nodes[wand_node_idx]["mesh"].as_u64().unwrap() as usize;
    let pos_acc = json["meshes"][wand_mesh_idx]["primitives"][0]["attributes"]["POSITION"]
        .as_u64()
        .unwrap() as usize;
    let verts = accessor_vec3(&json, &bin, pos_acc);
    let w = &worlds[wand_node_idx];
    // 頂点0のローカル座標を世界行列で変換した位置を検証する:
    // LeftHand (0, 0.005, 0) + offset (-0.1, 0.2, 0.3) + local (1, 0, 0) = (0.9, 0.205, 0.3)
    // ※ main_fbxのLeftHandはLcl Translation (0, 0.5, 0) * 0.01 = 0.005m。
    let v0 = verts[0];
    let wx = w[0] * f64::from(v0[0]) + w[4] * f64::from(v0[1]) + w[8] * f64::from(v0[2]) + w[12];
    let wy = w[1] * f64::from(v0[0]) + w[5] * f64::from(v0[1]) + w[9] * f64::from(v0[2]) + w[13];
    let wz = w[2] * f64::from(v0[0]) + w[6] * f64::from(v0[1]) + w[10] * f64::from(v0[2]) + w[14];
    assert!((wx - 0.9).abs() < 1e-4, "wx={wx}");
    assert!((wy - 0.205).abs() < 1e-4, "wy={wy}");
    assert!((wz - 0.3).abs() < 1e-4, "wz={wz}");
    // 小物材質がGLBマテリアルに含まれている。
    let mat_names: Vec<&str> = json["materials"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["name"].as_str().unwrap())
        .collect();
    assert!(mat_names.contains(&"小物材質"), "mat_names={mat_names:?}");
}

#[test]
fn スキン付きネストprefabがスキンと逆バインド行列を保って書き出される() {
    let main_fbx = 主fbxを組み立てる(100.0);
    let skin_prop_fbx = スキン付き小物fbxを組み立てる(100.0);
    let left_hand_tr = transform_file_id("Hips/Spine/LeftUpperArm/LeftLowerArm/LeftHand");
    let main_mr = skinned_mesh_renderer_file_id("Hips/Owner");
    let ribbon_mr = skinned_mesh_renderer_file_id("RibbonMesh");

    let nest_body = 実体文書(
        200,
        0,
        SKIN_PROP_FBX_GUID,
        &材質上書き行(ribbon_mr, SKIN_PROP_FBX_GUID, 0, MAT_PROP_GUID),
    );
    let avatar_body = prefab結合(&[
        実体文書(
            100,
            0,
            MAIN_FBX_GUID,
            &材質上書き行(main_mr, MAIN_FBX_GUID, 0, MAT_MAIN_GUID),
        ),
        stripped_tr文書(150, left_hand_tr, MAIN_FBX_GUID, 100),
        実体文書(
            300,
            150,
            NEST_PREFAB_GUID,
            &数値上書き行(
                transform_file_id(""),
                SKIN_PROP_FBX_GUID,
                "m_LocalPosition.x",
                0.2,
            ),
        ),
    ]);

    let (index, 読み手) = 索引と読み手を作る(&[
        ("Main.fbx", メタを作る(MAIN_FBX_GUID), None),
        (
            "Ribbon.fbx",
            メタを作る(SKIN_PROP_FBX_GUID),
            Some(skin_prop_fbx),
        ),
        (
            "Nest.prefab",
            prefabメタを作る(NEST_PREFAB_GUID),
            Some(nest_body.into_bytes()),
        ),
        (
            "Avatar.prefab",
            prefabメタを作る(AVATAR_PREFAB_GUID),
            Some(avatar_body),
        ),
        (
            "MatMain.mat",
            matメタを作る(MAT_MAIN_GUID),
            Some(検査材質を作る("主材質", 1.0, 1.0, 1.0)),
        ),
        (
            "MatProp.mat",
            matメタを作る(MAT_PROP_GUID),
            Some(検査材質を作る("リボン材質", 1.0, 0.0, 0.5)),
        ),
    ]);

    let (glb, 報告) = convert_prefab(
        &読み手,
        &index,
        "Assets/Avatar.prefab",
        &main_fbx,
        &FbxOptions::new(),
        &VrmOptions::new(),
        書き出し設定(),
        &変換設定(PrefabExpressions::None, PrefabPhysics::None),
    )
    .expect("スキン付きネストPrefabを変換できる");

    assert_eq!(報告.active_meshes, 2);
    assert_eq!(報告.skins, 1);

    let (json, bin) = glbからjsonとbinを取り出す(&glb);
    let worlds = gltf_node_world_matrices(&json);
    let nodes = json["nodes"].as_array().unwrap();
    let ribbon_node_idx = nodes
        .iter()
        .position(|n| n["name"] == "RibbonMesh" && n.get("skin").is_some())
        .expect("RibbonMeshにskinが付いている");
    let skin_idx = nodes[ribbon_node_idx]["skin"].as_u64().unwrap() as usize;
    let joint_node_idx = json["skins"][skin_idx]["joints"][0].as_u64().unwrap() as usize;
    let ibm_acc = json["skins"][skin_idx]["inverseBindMatrices"]
        .as_u64()
        .unwrap() as usize;
    let ibms = accessor_mat4(&json, &bin, ibm_acc);
    let mesh_idx = nodes[ribbon_node_idx]["mesh"].as_u64().unwrap() as usize;
    let pos_acc = json["meshes"][mesh_idx]["primitives"][0]["attributes"]["POSITION"]
        .as_u64()
        .unwrap() as usize;
    let verts = accessor_vec3(&json, &bin, pos_acc);
    // バインド時のスキニング変換 `W_gltf(joint) * IBM * v0` が、
    // LeftHand(0, 0.5, 0) + offset(-0.2, 0, 0) + v0(0, 2, 0) = (-0.2, 2.5, 0) に一致する。
    let v0 = verts[0];
    let ibm = &ibms[0];
    let lx = ibm[0] * v0[0] + ibm[4] * v0[1] + ibm[8] * v0[2] + ibm[12];
    let ly = ibm[1] * v0[0] + ibm[5] * v0[1] + ibm[9] * v0[2] + ibm[13];
    let lz = ibm[2] * v0[0] + ibm[6] * v0[1] + ibm[10] * v0[2] + ibm[14];
    let jw = &worlds[joint_node_idx];
    let wx = jw[0] * f64::from(lx) + jw[4] * f64::from(ly) + jw[8] * f64::from(lz) + jw[12];
    let wy = jw[1] * f64::from(lx) + jw[5] * f64::from(ly) + jw[9] * f64::from(lz) + jw[13];
    let wz = jw[2] * f64::from(lx) + jw[6] * f64::from(ly) + jw[10] * f64::from(lz) + jw[14];
    assert!((wx - (-0.2)).abs() < 1e-4, "wx={wx}");
    assert!((wy - 2.5).abs() < 1e-4, "wy={wy}");
    assert!(wz.abs() < 1e-4, "wz={wz}");
}

#[test]
fn 多段ネストと同一prefab複数配置と上書きおよび再有効化が効く() {
    // Avatar.prefab -> Outer.prefab -> Nest.prefab -> Prop.fbx（多段ネスト）。
    // Nest.prefab 内では m_IsActive: 0 にし、Outer.prefab で m_IsActive: 1 に再有効化し、
    // Avatar.prefab で材質を上書きする（stripped経由）。
    // さらに Avatar.prefab は Nest.prefab をもう1つ別ノードへ置き、そちらは m_Enabled: 0 で無効化する。
    let main_fbx = 主fbxを組み立てる(100.0);
    let prop_fbx = 非スキン小物fbxを組み立てる(
        100.0,
        "Wand",
        &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
    );
    let main_mr = skinned_mesh_renderer_file_id("Hips/Owner");
    let prop_mr = mesh_renderer_file_id("Wand");
    let root_go = game_object_file_id("");

    let nest_body = prefab結合(&[
        実体文書(
            200,
            0,
            PROP_FBX_GUID,
            &format!(
                "{}{}",
                材質上書き行(prop_mr, PROP_FBX_GUID, 0, MAT_PROP_GUID),
                整数上書き行(root_go, PROP_FBX_GUID, "m_IsActive", 0),
            ),
        ),
        format!(
            "%YAML 1.1\n%TAG !u! tag:unity3d.com,2011:\n--- !u!1 &250 stripped\nGameObject:\n  m_CorrespondingSourceObject: {{fileID: {root_go}, guid: {PROP_FBX_GUID}, type: 3}}\n  m_PrefabInstance: {{fileID: 200}}\n"
        ),
    ]);
    // Outer.prefab は Nest.prefab を置き、stripped経由で m_IsActive: 1 に戻す。
    let outer_body = prefab結合(&[
        実体文書(
            210,
            0,
            NEST_PREFAB_GUID,
            &整数上書き行(250, NEST_PREFAB_GUID, "m_IsActive", 1),
        ),
        format!(
            "%YAML 1.1\n%TAG !u! tag:unity3d.com,2011:\n--- !u!23 &251 stripped\nMeshRenderer:\n  m_CorrespondingSourceObject: {{fileID: {prop_mr}, guid: {PROP_FBX_GUID}, type: 3}}\n  m_PrefabInstance: {{fileID: 210}}\n"
        ),
    ]);
    // Avatar.prefab は Outer.prefab を置き（stripped &251 経由で材質を MAT_ALT へ上書き）、
    // さらに Nest.prefab を直接もう1個置いて m_Enabled: 0 で無効化する。
    let avatar_body = prefab結合(&[
        実体文書(
            100,
            0,
            MAIN_FBX_GUID,
            &材質上書き行(main_mr, MAIN_FBX_GUID, 0, MAT_MAIN_GUID),
        ),
        実体文書(
            300,
            0,
            OUTER_PREFAB_GUID,
            &材質上書き行(251, OUTER_PREFAB_GUID, 0, MAT_ALT_GUID),
        ),
        実体文書(
            301,
            0,
            NEST_PREFAB_GUID,
            &format!(
                "{}{}",
                整数上書き行(root_go, PROP_FBX_GUID, "m_IsActive", 1),
                整数上書き行(prop_mr, PROP_FBX_GUID, "m_Enabled", 0),
            ),
        ),
    ]);

    let (index, 読み手) = 索引と読み手を作る(&[
        ("Main.fbx", メタを作る(MAIN_FBX_GUID), None),
        ("Prop.fbx", メタを作る(PROP_FBX_GUID), Some(prop_fbx)),
        (
            "Nest.prefab",
            prefabメタを作る(NEST_PREFAB_GUID),
            Some(nest_body),
        ),
        (
            "Outer.prefab",
            prefabメタを作る(OUTER_PREFAB_GUID),
            Some(outer_body),
        ),
        (
            "Avatar.prefab",
            prefabメタを作る(AVATAR_PREFAB_GUID),
            Some(avatar_body),
        ),
        (
            "MatMain.mat",
            matメタを作る(MAT_MAIN_GUID),
            Some(検査材質を作る("主材質", 1.0, 1.0, 1.0)),
        ),
        (
            "MatProp.mat",
            matメタを作る(MAT_PROP_GUID),
            Some(検査材質を作る("小物材質", 0.0, 1.0, 0.0)),
        ),
        (
            "MatAlt.mat",
            matメタを作る(MAT_ALT_GUID),
            Some(検査材質を作る("上書き材質", 0.0, 0.0, 1.0)),
        ),
    ]);

    let (glb, 報告) = convert_prefab(
        &読み手,
        &index,
        "Assets/Avatar.prefab",
        &main_fbx,
        &FbxOptions::new(),
        &VrmOptions::new(),
        書き出し設定(),
        &変換設定(PrefabExpressions::None, PrefabPhysics::None),
    )
    .expect("多段ネストと複数配置を変換できる");

    // 主1 + 有効なOuter配下1 = 2メッシュ、無効な2個目Nest = 除外1。
    assert_eq!(報告.active_meshes, 2);
    assert_eq!(報告.excluded_meshes, 1);
    assert_eq!(報告.excluded_names, vec!["Wand".to_owned()]);
    let (json, _) = glbからjsonとbinを取り出す(&glb);
    let mat_names: Vec<&str> = json["materials"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["name"].as_str().unwrap())
        .collect();
    assert_eq!(mat_names, vec!["主材質", "上書き材質"]);
}

#[test]
fn 直接記述のネストprefabとローカル中間ノードおよび置換表と材質プール重複排除が動く() {
    // Nest.prefab は PrefabInstance を持たない直接記述（GameObject + MeshFilter + MeshRenderer）。
    // Prop.fbx の .meta の置換表が MAT_MAIN_GUID（主FBXと同じ材質）を指し、材質プールで重複排除される。
    // Avatar.prefab はローカル中間 GameObject (Holder, active=1) 配下に Nest.prefab を置く。
    let main_fbx = 主fbxを組み立てる(100.0);
    let prop_fbx = 非スキン小物fbxを組み立てる(
        100.0,
        "Wand",
        &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
    );
    let main_mr = skinned_mesh_renderer_file_id("Hips/Owner");
    let wand_mesh_fid = mesh_file_id("Wand");

    let nest_body = format!(
        "%YAML 1.1\n%TAG !u! tag:unity3d.com,2011:\n\
--- !u!1 &10\nGameObject:\n  m_ObjectHideFlags: 0\n  m_Name: DirectWand\n  m_IsActive: 1\n  m_Component: []\n\
--- !u!4 &11\nTransform:\n  m_GameObject: {{fileID: 10}}\n  m_LocalRotation: {{x: 0, y: 0, z: 0, w: 1}}\n  m_LocalPosition: {{x: 0.5, y: 0, z: 0}}\n  m_LocalScale: {{x: 1, y: 1, z: 1}}\n  m_Children: []\n  m_Father: {{fileID: 0}}\n\
--- !u!33 &12\nMeshFilter:\n  m_GameObject: {{fileID: 10}}\n  m_Mesh: {{fileID: {wand_mesh_fid}, guid: {PROP_FBX_GUID}, type: 3}}\n\
--- !u!23 &13\nMeshRenderer:\n  m_GameObject: {{fileID: 10}}\n  m_Enabled: 1\n  m_Materials:\n  - {{fileID: 2100000, guid: {MAT_MAIN_GUID}, type: 2}}\n"
    );

    let avatar_body = prefab結合(&[
        実体文書(
            100,
            0,
            MAIN_FBX_GUID,
            &材質上書き行(main_mr, MAIN_FBX_GUID, 0, MAT_MAIN_GUID),
        ),
        "%YAML 1.1\n%TAG !u! tag:unity3d.com,2011:\n--- !u!1 &500\nGameObject:\n  m_ObjectHideFlags: 0\n  m_Name: Holder\n  m_IsActive: 1\n  m_Component: []\n--- !u!4 &501\nTransform:\n  m_GameObject: {fileID: 500}\n  m_LocalRotation: {x: 0, y: 0, z: 0, w: 1}\n  m_LocalPosition: {x: 0, y: 1, z: 0}\n  m_LocalScale: {x: 1, y: 1, z: 1}\n  m_Children: []\n  m_Father: {fileID: 0}\n".to_owned(),
        実体文書(300, 501, NEST_PREFAB_GUID, ""),
    ]);

    let (index, 読み手) = 索引と読み手を作る(&[
        ("Main.fbx", メタを作る(MAIN_FBX_GUID), None),
        (
            "Prop.fbx",
            置換表付きfbxメタ(PROP_FBX_GUID, "PropMat", MAT_MAIN_GUID),
            Some(prop_fbx),
        ),
        (
            "Nest.prefab",
            prefabメタを作る(NEST_PREFAB_GUID),
            Some(nest_body.into_bytes()),
        ),
        (
            "Avatar.prefab",
            prefabメタを作る(AVATAR_PREFAB_GUID),
            Some(avatar_body),
        ),
        (
            "MatMain.mat",
            matメタを作る(MAT_MAIN_GUID),
            Some(検査材質を作る("共有材質", 1.0, 1.0, 1.0)),
        ),
    ]);

    let (glb, 報告) = convert_prefab(
        &読み手,
        &index,
        "Assets/Avatar.prefab",
        &main_fbx,
        &FbxOptions::new(),
        &VrmOptions::new(),
        書き出し設定(),
        &変換設定(PrefabExpressions::None, PrefabPhysics::None),
    )
    .expect("直接記述ネストPrefabを変換できる");

    assert_eq!(報告.active_meshes, 2);
    // 主FBXとネストPrefabで同じ材質を使うため、材質数は1に重複排除される。
    assert_eq!(報告.materials, 1);
    let (json, _) = glbからjsonとbinを取り出す(&glb);
    assert_eq!(json["materials"].as_array().unwrap().len(), 1);
}

#[test]
fn 旧方式file_idの副fbxを参照するネストprefabも書き出せる() {
    let main_fbx = 主fbxを組み立てる(100.0);
    let prop_fbx = 非スキン小物fbxを組み立てる(
        100.0,
        "Wand",
        &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
    );
    let main_mr = skinned_mesh_renderer_file_id("Hips/Owner");
    let legacy_meta = 旧方式id付きfbxメタ(
        PROP_FBX_GUID,
        "    100000: Wand\n    400000: Wand\n    2300000: Wand\n    3300000: Wand\n    4300000: Wand\n",
    );
    let nest_body = 実体文書(
        200,
        0,
        PROP_FBX_GUID,
        &材質上書き行(2300000, PROP_FBX_GUID, 0, MAT_PROP_GUID),
    );
    let avatar_body = prefab結合(&[
        実体文書(
            100,
            0,
            MAIN_FBX_GUID,
            &材質上書き行(main_mr, MAIN_FBX_GUID, 0, MAT_MAIN_GUID),
        ),
        実体文書(300, 0, NEST_PREFAB_GUID, ""),
    ]);

    let (index, 読み手) = 索引と読み手を作る(&[
        ("Main.fbx", メタを作る(MAIN_FBX_GUID), None),
        ("Prop.fbx", legacy_meta, Some(prop_fbx)),
        (
            "Nest.prefab",
            prefabメタを作る(NEST_PREFAB_GUID),
            Some(nest_body.into_bytes()),
        ),
        (
            "Avatar.prefab",
            prefabメタを作る(AVATAR_PREFAB_GUID),
            Some(avatar_body),
        ),
        (
            "MatMain.mat",
            matメタを作る(MAT_MAIN_GUID),
            Some(検査材質を作る("主材質", 1.0, 1.0, 1.0)),
        ),
        (
            "MatProp.mat",
            matメタを作る(MAT_PROP_GUID),
            Some(検査材質を作る("旧小物材質", 0.5, 0.5, 0.5)),
        ),
    ]);

    let (_glb, 報告) = convert_prefab(
        &読み手,
        &index,
        "Assets/Avatar.prefab",
        &main_fbx,
        &FbxOptions::new(),
        &VrmOptions::new(),
        書き出し設定(),
        &変換設定(PrefabExpressions::None, PrefabPhysics::None),
    )
    .expect("旧方式IDの副FBXを含むネストPrefabを変換できる");

    assert_eq!(報告.active_meshes, 2);
    assert_eq!(報告.materials, 2);
}

#[test]
fn 循環参照と深さ上限と未解決参照の扱いが正しい() {
    let main_fbx = 主fbxを組み立てる(100.0);
    let main_mr = skinned_mesh_renderer_file_id("Hips/Owner");

    // 1. 循環参照 (Nest -> Outer -> Nest) は失敗する。
    let nest_cycle = 実体文書(200, 0, OUTER_PREFAB_GUID, "");
    let outer_cycle = 実体文書(210, 0, NEST_PREFAB_GUID, "");
    let avatar_cycle = prefab結合(&[
        実体文書(
            100,
            0,
            MAIN_FBX_GUID,
            &材質上書き行(main_mr, MAIN_FBX_GUID, 0, MAT_MAIN_GUID),
        ),
        実体文書(300, 0, NEST_PREFAB_GUID, ""),
    ]);
    let (index, 読み手) = 索引と読み手を作る(&[
        ("Main.fbx", メタを作る(MAIN_FBX_GUID), None),
        (
            "Nest.prefab",
            prefabメタを作る(NEST_PREFAB_GUID),
            Some(nest_cycle.into_bytes()),
        ),
        (
            "Outer.prefab",
            prefabメタを作る(OUTER_PREFAB_GUID),
            Some(outer_cycle.into_bytes()),
        ),
        (
            "Avatar.prefab",
            prefabメタを作る(AVATAR_PREFAB_GUID),
            Some(avatar_cycle),
        ),
        (
            "MatMain.mat",
            matメタを作る(MAT_MAIN_GUID),
            Some(検査材質を作る("主材質", 1.0, 1.0, 1.0)),
        ),
    ]);
    let err = convert_prefab(
        &読み手,
        &index,
        "Assets/Avatar.prefab",
        &main_fbx,
        &FbxOptions::new(),
        &VrmOptions::new(),
        書き出し設定(),
        &変換設定(PrefabExpressions::None, PrefabPhysics::None),
    )
    .expect_err("循環参照は失敗する");
    assert_eq!(err.stage(), PrefabConversionStage::Assignment);
    assert!(format!("{err}").contains("循環"), "{err}");

    // 2. 索引外参照を含むネストPrefabは警告を出してスキップし、主FBXは変換成功する。
    const MISSING_GUID: &str = "dead000000000000000000000000beef";
    let nest_missing = 実体文書(200, 0, MISSING_GUID, "");
    let avatar_missing = prefab結合(&[
        実体文書(
            100,
            0,
            MAIN_FBX_GUID,
            &材質上書き行(main_mr, MAIN_FBX_GUID, 0, MAT_MAIN_GUID),
        ),
        実体文書(300, 0, NEST_PREFAB_GUID, ""),
    ]);
    let (index_missing, 読み手_missing) = 索引と読み手を作る(&[
        ("Main.fbx", メタを作る(MAIN_FBX_GUID), None),
        (
            "Nest.prefab",
            prefabメタを作る(NEST_PREFAB_GUID),
            Some(nest_missing.into_bytes()),
        ),
        (
            "Avatar.prefab",
            prefabメタを作る(AVATAR_PREFAB_GUID),
            Some(avatar_missing),
        ),
        (
            "MatMain.mat",
            matメタを作る(MAT_MAIN_GUID),
            Some(検査材質を作る("主材質", 1.0, 1.0, 1.0)),
        ),
    ]);
    let (_glb, 報告) = convert_prefab(
        &読み手_missing,
        &index_missing,
        "Assets/Avatar.prefab",
        &main_fbx,
        &FbxOptions::new(),
        &VrmOptions::new(),
        書き出し設定(),
        &変換設定(PrefabExpressions::None, PrefabPhysics::None),
    )
    .expect("索引外参照は警告でスキップされる");
    assert_eq!(報告.active_meshes, 1);
    assert!(
        報告.warnings.iter().any(|w| w.contains(MISSING_GUID)),
        "索引外参照の警告がある: {:?}",
        報告.warnings
    );

    // 3. 予算上限超過は失敗する。
    let mut 厳制限 = 変換設定(PrefabExpressions::None, PrefabPhysics::None);
    厳制限.limits = PrefabConversionLimits::new().with_max_total_bytes(10);
    let err_limit = convert_prefab(
        &読み手_missing,
        &index_missing,
        "Assets/Avatar.prefab",
        &main_fbx,
        &FbxOptions::new(),
        &VrmOptions::new(),
        書き出し設定(),
        &厳制限,
    )
    .expect_err("合計上限超過は失敗する");
    assert_eq!(err_limit.stage(), PrefabConversionStage::Assignment);
}
