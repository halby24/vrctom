//! VRM中間モデル生成の契約テスト（公開 API のみを使う）。
//!
//! テスト内でバイナリFBXを組み立て、ノード・スキン・メッシュ・報告の変換と
//! 失敗条件を固定する。実fixtureを使う検証は `VRCTOM_FBX_FIXTURE` が指す
//! ファイルだけを読み、未設定なら跳ばす。

use std::collections::HashMap;

use vrctom::fbx::{FbxFile, FbxOptions, ObjectId};
use vrctom::unity::{ParseOptions, SerializedFile, mesh_file_id, skinned_mesh_renderer_file_id};
use vrctom::vrm::{
    BaselineChannel, BaselineWeights, VrmErrorKind, VrmExpressionProfile, VrmModel, VrmOptions,
    prefab_baseline_weights,
};

// テスト名は日本語で書く（テストは仕様・契約であるため）。

/// 論理プロパティ列。件数は組み立て時に数える。
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

    fn i(&mut self, v: i32) {
        self.prop(b'I', &v.to_le_bytes());
    }

    fn d(&mut self, v: f64) {
        self.prop(b'D', &v.to_le_bytes());
    }

    fn l(&mut self, v: i64) {
        self.prop(b'L', &v.to_le_bytes());
    }

    fn s(&mut self, v: &[u8]) {
        let mut body = (v.len() as u32).to_le_bytes().to_vec();
        body.extend_from_slice(v);
        self.prop(b'S', &body);
    }

    /// 配列プロパティを書く。圧縮は使わずrawだけにする。
    fn array(&mut self, code: u8, count: u32, payload: &[u8]) {
        let mut body = count.to_le_bytes().to_vec();
        body.extend_from_slice(&0u32.to_le_bytes());
        body.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        body.extend_from_slice(payload);
        self.prop(code, &body);
    }

    /// `f64` 列をrawの `d` 配列として書く。
    fn d_raw(&mut self, values: &[f64]) {
        let mut payload = Vec::with_capacity(values.len() * 8);
        for v in values {
            payload.extend_from_slice(&v.to_le_bytes());
        }
        self.array(b'd', values.len() as u32, &payload);
    }

    /// `i32` 列をrawの `i` 配列として書く。
    fn i_raw(&mut self, values: &[i32]) {
        let mut payload = Vec::with_capacity(values.len() * 4);
        for v in values {
            payload.extend_from_slice(&v.to_le_bytes());
        }
        self.array(b'i', values.len() as u32, &payload);
    }
}

/// 論理ノード。シリアライズ時に絶対オフセットを確定させる。
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

/// 論理ノードを絶対オフセット付きで書き出す。
fn ser(node: &TNode, buf: &mut Vec<u8>) {
    let start = buf.len();
    buf.extend_from_slice(&[0u8; 13]);
    buf.extend_from_slice(&node.name);
    buf.extend_from_slice(&node.props.buf);
    for child in &node.children {
        ser(child, buf);
    }
    if !node.children.is_empty() {
        // 子列の終端であり、親範囲の一部である。
        buf.extend_from_slice(&[0u8; 13]);
    }
    let end = buf.len() as u32;
    buf[start..start + 4].copy_from_slice(&end.to_le_bytes());
    buf[start + 4..start + 8].copy_from_slice(&node.props.count.to_le_bytes());
    buf[start + 8..start + 12].copy_from_slice(&(node.props.buf.len() as u32).to_le_bytes());
    buf[start + 12] = node.name.len() as u8;
}

/// 直下ノード列からバイナリFBXを組み立てる。
fn file(version: u32, top: Vec<TNode>) -> Vec<u8> {
    let mut buf = b"Kaydara FBX Binary  \x00\x1a\x00".to_vec();
    buf.extend_from_slice(&version.to_le_bytes());
    for node in &top {
        ser(node, &mut buf);
    }
    // トップレベルの終端以降はフッタであり、読まない。
    buf.extend_from_slice(&[0u8; 13]);
    buf
}

/// `Model` ノードを組み立てる。
fn model(id: i64, name: &[u8], subtype: &str) -> TNode {
    let mut props = Props::default();
    props.l(id);
    props.s(name);
    props.s(subtype.as_bytes());
    TNode::branch(b"Model", props, Vec::new())
}

/// `Material` ノードを組み立てる。IDと名前だけである。
fn material(id: i64, name: &[u8]) -> TNode {
    let mut props = Props::default();
    props.l(id);
    props.s(name);
    props.s(b"");
    TNode::branch(b"Material", props, Vec::new())
}

/// `P` ノードを組み立てる。値は呼び出し側が詰める。
fn p_node(name: &str, typ: &str, values: Props) -> TNode {
    let mut props = Props::default();
    props.s(name.as_bytes());
    props.s(typ.as_bytes());
    props.s(b"");
    props.s(b"");
    props.buf.extend_from_slice(&values.buf);
    props.count += values.count;
    TNode::leaf(b"P", props)
}

/// 整数値1件の `P` を組み立てる。
fn p_int(name: &str, typ: &str, v: i32) -> TNode {
    let mut values = Props::default();
    values.i(v);
    p_node(name, typ, values)
}

/// `Geometry`（Mesh）ノードを組み立てる。
fn geometry(id: i64, name: &[u8], children: Vec<TNode>) -> TNode {
    let mut props = Props::default();
    props.l(id);
    props.s(name);
    props.s(b"Mesh");
    TNode::branch(b"Geometry", props, children)
}

/// `Vertices` ノードを組み立てる。
fn vertices_node(values: &[f64]) -> TNode {
    let mut props = Props::default();
    props.d_raw(values);
    TNode::leaf(b"Vertices", props)
}

/// `PolygonVertexIndex` ノードを組み立てる。
fn pvi_node(values: &[i32]) -> TNode {
    let mut props = Props::default();
    props.i_raw(values);
    TNode::leaf(b"PolygonVertexIndex", props)
}

/// 文字列プロパティ1件の子ノードを組み立てる。
fn str_child(name: &[u8], value: &[u8]) -> TNode {
    let mut props = Props::default();
    props.s(value);
    TNode::leaf(name, props)
}

/// `LayerElementNormal` を組み立てる。
fn normal_layer(mapping: &[u8], reference: &[u8], data: &[f64], index: Option<&[i32]>) -> TNode {
    let mut children = vec![
        str_child(b"MappingInformationType", mapping),
        str_child(b"ReferenceInformationType", reference),
    ];
    let mut data_props = Props::default();
    data_props.d_raw(data);
    children.push(TNode::leaf(b"Normals", data_props));
    if let Some(index) = index {
        let mut index_props = Props::default();
        index_props.i_raw(index);
        children.push(TNode::leaf(b"NormalsIndex", index_props));
    }
    let mut props = Props::default();
    props.i(0);
    TNode::branch(b"LayerElementNormal", props, children)
}

/// `LayerElementUV` を組み立てる。
fn uv_layer(
    set_name: &[u8],
    mapping: &[u8],
    reference: &[u8],
    data: &[f64],
    index: Option<&[i32]>,
) -> TNode {
    let mut children = vec![
        str_child(b"MappingInformationType", mapping),
        str_child(b"ReferenceInformationType", reference),
        str_child(b"Name", set_name),
    ];
    let mut data_props = Props::default();
    data_props.d_raw(data);
    children.push(TNode::leaf(b"UV", data_props));
    if let Some(index) = index {
        let mut index_props = Props::default();
        index_props.i_raw(index);
        children.push(TNode::leaf(b"UVIndex", index_props));
    }
    let mut props = Props::default();
    props.i(0);
    TNode::branch(b"LayerElementUV", props, children)
}

/// `LayerElementMaterial` を組み立てる。
fn material_layer(mapping: &[u8], reference: &[u8], materials: &[i32]) -> TNode {
    let children = vec![
        str_child(b"MappingInformationType", mapping),
        str_child(b"ReferenceInformationType", reference),
        {
            let mut props = Props::default();
            props.i_raw(materials);
            TNode::leaf(b"Materials", props)
        },
    ];
    let mut props = Props::default();
    props.i(0);
    TNode::branch(b"LayerElementMaterial", props, children)
}

/// `Connections` の `OO` 接続（子→親）を組み立てる。
fn conn(child: i64, parent: i64) -> TNode {
    let mut props = Props::default();
    props.s(b"OO");
    props.l(child);
    props.l(parent);
    TNode::leaf(b"C", props)
}

/// `Objects` ノードを組み立てる。
fn objects(children: Vec<TNode>) -> TNode {
    TNode::branch(b"Objects", Props::default(), children)
}

/// `Connections` ノードを組み立てる。
fn connections(children: Vec<TNode>) -> TNode {
    TNode::branch(b"Connections", Props::default(), children)
}

/// `Deformer` ノードを組み立てる。
fn deformer(id: i64, name: &[u8], subtype: &str, children: Vec<TNode>) -> TNode {
    let mut props = Props::default();
    props.l(id);
    props.s(name);
    props.s(subtype.as_bytes());
    TNode::branch(b"Deformer", props, children)
}

/// `Indexes` ノードを組み立てる。
fn indexes_node(values: &[i32]) -> TNode {
    let mut props = Props::default();
    props.i_raw(values);
    TNode::leaf(b"Indexes", props)
}

/// `Weights` ノードを組み立てる。
fn weights_node(values: &[f64]) -> TNode {
    let mut props = Props::default();
    props.d_raw(values);
    TNode::leaf(b"Weights", props)
}

/// `Pose` ノードを組み立てる。
fn pose(id: i64, name: &[u8], subtype: &str, children: Vec<TNode>) -> TNode {
    let mut props = Props::default();
    props.l(id);
    props.s(name);
    props.s(subtype.as_bytes());
    TNode::branch(b"Pose", props, children)
}

/// `PoseNode` を組み立てる。行列は16値の `d` 配列である。
fn pose_node(model: i64, matrix: &[f64; 16]) -> TNode {
    let mut matrix_props = Props::default();
    matrix_props.d_raw(matrix);
    let mut node_props = Props::default();
    node_props.l(model);
    TNode::branch(
        b"PoseNode",
        Props::default(),
        vec![
            TNode::leaf(b"Node", node_props),
            TNode::leaf(b"Matrix", matrix_props),
        ],
    )
}

/// 平行移動だけが異なる行優先のバインド行列を組み立てる。
fn bind_matrix(x: f64, y: f64, z: f64) -> [f64; 16] {
    [
        1.0, 0.0, 0.0, 0.0, //
        0.0, 1.0, 0.0, 0.0, //
        0.0, 0.0, 1.0, 0.0, //
        x, y, z, 1.0, //
    ]
}

/// モデル列に対する `BindPose` を組み立てる。
fn bind_pose(id: i64, nodes: &[(i64, [f64; 16])]) -> TNode {
    pose(
        id,
        b"Bind\x00\x01Pose",
        "BindPose",
        nodes
            .iter()
            .map(|(model, matrix)| pose_node(*model, matrix))
            .collect(),
    )
}

/// 軸系と単位換算の `GlobalSettings` を組み立てる。
#[allow(clippy::too_many_arguments)]
fn global_settings(
    up: i32,
    up_sign: i32,
    front: i32,
    front_sign: i32,
    coord: i32,
    coord_sign: i32,
    unit: f64,
) -> TNode {
    let mut unit_values = Props::default();
    unit_values.d(unit);
    TNode::branch(
        b"GlobalSettings",
        Props::default(),
        vec![TNode::branch(
            b"Properties70",
            Props::default(),
            vec![
                p_int("UpAxis", "int", up),
                p_int("UpAxisSign", "int", up_sign),
                p_int("FrontAxis", "int", front),
                p_int("FrontAxisSign", "int", front_sign),
                p_int("CoordAxis", "int", coord),
                p_int("CoordAxisSign", "int", coord_sign),
                p_node("UnitScaleFactor", "double", unit_values),
            ],
        )],
    )
}

/// 頂上ノード列からファイルを組み立てる。設定ノードは先頭に置く。
fn assemble(globals: Option<TNode>, objs: Vec<TNode>, conns: Vec<TNode>) -> Vec<u8> {
    let mut top = Vec::new();
    if let Some(globals) = globals {
        top.push(globals);
    }
    top.push(objects(objs));
    top.push(connections(conns));
    file(7400, top)
}

/// 三角形1面の幾何を組み立てる。
fn triangle_geometry(id: i64, name: &[u8], children: Vec<TNode>) -> TNode {
    let mut all = vec![
        vertices_node(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]),
        // `!2 == -3` が終端を表す。
        pvi_node(&[0, 1, -3]),
    ];
    all.extend(children);
    geometry(id, name, all)
}

/// 三角形1枚・所有モデル付きの最小ファイルを組み立てる。
///
/// モデル1（親なし）・モデル2（モデル1の子・メッシュ所有）・幾何10・
/// バインド行列は引数で渡す。追加の `Objects` と `Connections` を足せる。
fn 最小成功ファイル(
    bind1: [f64; 16],
    bind2: [f64; 16],
    extra_objects: Vec<TNode>,
    extra_connections: Vec<TNode>,
) -> Vec<u8> {
    let mut objs = vec![
        model(1, b"Root\x00\x01Model", "Null"),
        model(2, b"Owner\x00\x01Model", "Mesh"),
        triangle_geometry(10, b"Tri\x00\x01Geometry", vec![]),
        bind_pose(100, &[(1, bind1), (2, bind2)]),
    ];
    objs.extend(extra_objects);
    let mut conns = vec![conn(2, 1), conn(10, 2), conn(1, 0)];
    conns.extend(extra_connections);
    assemble(None, objs, conns)
}

/// 4x4行列積。列ベクトル規約であり、`b` を先に適用する。
fn mul4(a: &[f64; 16], b: &[f64; 16]) -> [f64; 16] {
    let mut out = [0.0; 16];
    for row in 0..4 {
        for col in 0..4 {
            let mut sum = 0.0;
            for k in 0..4 {
                sum += a[k * 4 + row] * b[col * 4 + k];
            }
            out[col * 4 + row] = sum;
        }
    }
    out
}

/// 単位行列との差の絶対値の最大値を返す。
fn identity_distance(m: &[f64; 16]) -> f64 {
    m.iter()
        .enumerate()
        .map(|(index, value)| {
            let want = if index % 5 == 0 { 1.0 } else { 0.0 };
            (value - want).abs()
        })
        .fold(0.0, f64::max)
}

/// 全ノードで親子の双方向整合を確かめる。
///
/// `children` の各子の `parent` が自分を指し、`parent` を持つノードは
/// 親の `children` に含まれることを確かめる。
fn ノード双方向整合を確かめる(model: &VrmModel) {
    for (index, node) in model.nodes.iter().enumerate() {
        for child in &node.children {
            assert_eq!(
                model.nodes[*child].parent,
                Some(index),
                "子の親が自分を指す: node={index} child={child}"
            );
        }
        if let Some(parent) = node.parent {
            assert!(
                model.nodes[parent].children.contains(&index),
                "親の子に自分が含まれる: node={index} parent={parent}"
            );
        }
    }
}

/// 指定ノードの世界行列を、根から掛け合わせて求める。
fn ノードの世界行列(model: &VrmModel, mut index: usize) -> [f64; 16] {
    let mut lineage = vec![index];
    while let Some(parent) = model.nodes[index].parent {
        lineage.push(parent);
        index = parent;
    }
    let mut world = [
        1.0, 0.0, 0.0, 0.0, //
        0.0, 1.0, 0.0, 0.0, //
        0.0, 0.0, 1.0, 0.0, //
        0.0, 0.0, 0.0, 1.0, //
    ];
    for node in lineage.iter().rev() {
        world = mul4(&world, &model.nodes[*node].local);
    }
    world
}

/// 行列の差の絶対値の最大値を返す。
fn 行列の差(a: &[f64; 16], b: &[f64; 16]) -> f64 {
    a.iter()
        .zip(b.iter())
        .map(|(x, y)| (x - y).abs())
        .fold(0.0, f64::max)
}

/// X軸まわり-90度回転のバインド行列を組み立てる。
fn 回転バインド() -> [f64; 16] {
    [
        1.0, 0.0, 0.0, 0.0, //
        0.0, 0.0, -1.0, 0.0, //
        0.0, 1.0, 0.0, 0.0, //
        0.0, 0.0, 0.0, 1.0, //
    ]
}

#[test]
fn 三角形1枚のメッシュを読みノードと頂点と報告が期待どおりになる() {
    let bytes = 最小成功ファイル(
        bind_matrix(0.0, 0.0, 0.0),
        bind_matrix(1.0, 2.0, 3.0),
        vec![],
        vec![],
    );
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("最小構成は読める");
    let (model, report) =
        VrmModel::from_fbx(&parsed, &VrmOptions::new()).expect("最小構成は変換できる");

    // 単位換算の既定値（係数1）は0.01倍の拡大である。
    assert_eq!(
        model.root,
        [
            0.01, 0.0, 0.0, 0.0, //
            0.0, 0.01, 0.0, 0.0, //
            0.0, 0.0, 0.01, 0.0, //
            0.0, 0.0, 0.0, 1.0, //
        ]
    );
    // ノードは `models()` 順である。
    assert_eq!(model.nodes.len(), 2);
    assert_eq!(model.nodes[0].name, "Root");
    assert_eq!(model.nodes[0].parent, None);
    // メッシュ所有モデルは頂点ベイク済みのため親階層から外す。
    assert!(model.nodes[0].children.is_empty());
    assert_eq!(model.nodes[0].local, bind_matrix(0.0, 0.0, 0.0));
    // メッシュを持つモデルは元の親子を使わず、単位行列のルートになる。
    assert_eq!(model.nodes[1].name, "Owner");
    assert_eq!(model.nodes[1].parent, None);
    assert_eq!(model.nodes[1].children, Vec::<usize>::new());
    assert_eq!(
        model.nodes[1].local,
        [
            1.0, 0.0, 0.0, 0.0, //
            0.0, 1.0, 0.0, 0.0, //
            0.0, 0.0, 1.0, 0.0, //
            0.0, 0.0, 0.0, 1.0, //
        ]
    );
    assert!(model.skins.is_empty());

    // 頂点は所有モデルのバインド行列でシーン空間へ移る。
    assert_eq!(model.meshes.len(), 1);
    let mesh = &model.meshes[0];
    assert_eq!(mesh.node, 1);
    assert_eq!(mesh.geometry, ObjectId::new(10));
    assert_eq!(mesh.model, ObjectId::new(2));
    assert_eq!(mesh.name, "Tri");
    assert_eq!(mesh.skin, None);
    assert!(mesh.material_slots.is_empty());
    assert_eq!(
        mesh.vertices.positions,
        vec![[1.0, 2.0, 3.0], [2.0, 2.0, 3.0], [1.0, 3.0, 3.0]]
    );
    assert!(mesh.vertices.normals.is_none());
    assert!(mesh.vertices.uv_sets.is_empty());
    assert!(mesh.vertices.joints.is_none());
    assert!(mesh.vertices.weights.is_none());
    assert_eq!(mesh.primitives.len(), 1);
    assert_eq!(mesh.primitives[0].material, 0);
    assert_eq!(mesh.primitives[0].indices, vec![0, 1, 2]);

    assert_eq!(report.node_count, 2);
    assert_eq!(report.skin_count, 0);
    assert_eq!(report.mesh_count, 1);
    assert_eq!(report.skipped_meshes, 0);
    assert_eq!(report.primitive_count, 1);
    assert_eq!(report.vertex_count, 3);
    assert_eq!(report.triangle_count, 1);
    assert_eq!(report.truncated_influence_vertices, 0);
    assert_eq!(report.unweighted_vertices, 0);
    assert_eq!(report.non_triangle_polygons, 0);
    assert_eq!(report.mirrored_meshes, 0);
}

#[test]
fn 親を持つメッシュモデルは親階層から外れ双方向整合を保つ() {
    let bytes = 最小成功ファイル(
        bind_matrix(0.0, 0.0, 0.0),
        bind_matrix(1.0, 2.0, 3.0),
        vec![],
        vec![],
    );
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("親子構成は読める");
    let (model, _) = VrmModel::from_fbx(&parsed, &VrmOptions::new()).expect("親子構成は変換できる");
    assert_eq!(model.nodes.len(), 2);
    // メッシュ所有モデルはベイク済みのため親なし・子なしのルートになる。
    assert_eq!(model.nodes[1].parent, None);
    assert!(model.nodes[1].children.is_empty());
    // 親の `children` にメッシュ所有モデルは含まれない。
    assert!(!model.nodes[0].children.contains(&1));
    ノード双方向整合を確かめる(&model);
}

#[test]
fn 四角形と五角形はfan三角形化され非三角形に数えられる() {
    let quad = geometry(
        10,
        b"Quad\x00\x01Geometry",
        vec![
            vertices_node(&[
                0.0, 0.0, 0.0, //
                1.0, 0.0, 0.0, //
                1.0, 1.0, 0.0, //
                0.0, 1.0, 0.0, //
            ]),
            pvi_node(&[0, 1, 2, -4]),
        ],
    );
    let penta = geometry(
        11,
        b"Penta\x00\x01Geometry",
        vec![
            vertices_node(&[
                0.0, 0.0, 0.0, //
                1.0, 0.0, 0.0, //
                1.0, 1.0, 0.0, //
                0.5, 1.5, 0.0, //
                0.0, 1.0, 0.0, //
            ]),
            pvi_node(&[0, 1, 2, 3, -5]),
        ],
    );
    let bytes = assemble(
        None,
        vec![
            model(1, b"Root\x00\x01Model", "Null"),
            model(2, b"QuadOwner\x00\x01Model", "Mesh"),
            model(3, b"PentaOwner\x00\x01Model", "Mesh"),
            quad,
            penta,
            bind_pose(
                100,
                &[
                    (1, bind_matrix(0.0, 0.0, 0.0)),
                    (2, bind_matrix(0.0, 0.0, 0.0)),
                    (3, bind_matrix(0.0, 0.0, 0.0)),
                ],
            ),
        ],
        vec![conn(10, 2), conn(11, 3), conn(1, 0), conn(2, 0), conn(3, 0)],
    );
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("多角形は読める");
    let (model, report) =
        VrmModel::from_fbx(&parsed, &VrmOptions::new()).expect("多角形は変換できる");
    assert_eq!(model.meshes.len(), 2);
    // 四角形は2三角形、五角形は3三角形であり、頂点順を保つ。
    assert_eq!(model.meshes[0].primitives.len(), 1);
    assert_eq!(
        model.meshes[0].primitives[0].indices,
        vec![0, 1, 2, 0, 2, 3]
    );
    assert_eq!(model.meshes[1].primitives.len(), 1);
    assert_eq!(
        model.meshes[1].primitives[0].indices,
        vec![0, 1, 2, 0, 2, 3, 0, 3, 4]
    );
    assert_eq!(report.triangle_count, 5);
    assert_eq!(report.vertex_count, 9);
    assert_eq!(report.non_triangle_polygons, 2);
}

#[test]
fn 同じ属性のcornerは統合され法線違いは別頂点になる() {
    // 2枚の三角形は同じ位置を指すが、1箇所だけ法線が異なる。
    let bytes = assemble(
        None,
        vec![
            model(1, b"Root\x00\x01Model", "Null"),
            model(2, b"Owner\x00\x01Model", "Mesh"),
            geometry(
                10,
                b"G\x00\x01Geometry",
                vec![
                    vertices_node(&[
                        0.0, 0.0, 0.0, //
                        1.0, 0.0, 0.0, //
                        0.0, 1.0, 0.0, //
                    ]),
                    pvi_node(&[0, 1, -3, 0, 1, -3]),
                    // 2枚の三角形は同じ位置を指すが、1箇所だけ法線索引が異なる。
                    // `IndexToDirect` で索引を共有するため、値が同じcornerは統合される。
                    normal_layer(
                        b"ByPolygonVertex",
                        b"IndexToDirect",
                        &[
                            0.0, 0.0, 1.0, //
                            0.0, 0.0, 1.0, //
                            0.0, 0.0, 1.0, //
                            1.0, 0.0, 0.0, //
                        ],
                        Some(&[0, 1, 2, 3, 1, 2]),
                    ),
                    uv_layer(
                        b"UVMap",
                        b"ByPolygonVertex",
                        b"IndexToDirect",
                        &[
                            0.0, 0.0, //
                            1.0, 0.0, //
                            0.0, 1.0, //
                        ],
                        Some(&[0, 1, 2, 0, 1, 2]),
                    ),
                ],
            ),
            bind_pose(
                100,
                &[
                    (1, bind_matrix(0.0, 0.0, 0.0)),
                    (2, bind_matrix(0.0, 0.0, 0.0)),
                ],
            ),
        ],
        vec![conn(2, 1), conn(10, 2), conn(1, 0)],
    );
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("層付きは読める");
    let (model, report) =
        VrmModel::from_fbx(&parsed, &VrmOptions::new()).expect("層付きは変換できる");
    let mesh = &model.meshes[0];
    // 法線が異なる1箇所だけ別頂点になり、他は統合される。
    assert_eq!(mesh.vertices.positions.len(), 4);
    assert_eq!(mesh.vertices.normals.as_ref().expect("法線がある").len(), 4);
    assert_eq!(mesh.vertices.uv_sets.len(), 1);
    assert_eq!(mesh.vertices.uv_sets[0].len(), 4);
    assert_eq!(report.vertex_count, 4);
    assert_eq!(report.triangle_count, 2);
}

#[test]
fn スキンがあると関節と重みが正規化される() {
    let bytes = assemble(
        None,
        vec![
            model(1, b"Root\x00\x01Model", "Null"),
            model(2, b"Owner\x00\x01Model", "Mesh"),
            model(3, b"JointA\x00\x01Model", "LimbNode"),
            model(4, b"JointB\x00\x01Model", "LimbNode"),
            triangle_geometry(10, b"Tri\x00\x01Geometry", vec![]),
            deformer(20, b"Skin\x00\x01Deformer", "Skin", vec![]),
            deformer(
                21,
                b"ClusterA\x00\x01Deformer",
                "Cluster",
                vec![indexes_node(&[0, 1]), weights_node(&[0.5, 1.5])],
            ),
            deformer(
                22,
                b"ClusterB\x00\x01Deformer",
                "Cluster",
                vec![indexes_node(&[1, 2]), weights_node(&[0.5, 0.5])],
            ),
            bind_pose(
                100,
                &[
                    (1, bind_matrix(0.0, 0.0, 0.0)),
                    (2, bind_matrix(1.0, 0.0, 0.0)),
                    // 拡大を含む行列で逆行列の計算を確かめる。
                    (
                        3,
                        [
                            2.0, 0.0, 0.0, 0.0, //
                            0.0, 1.0, 0.0, 0.0, //
                            0.0, 0.0, 1.0, 0.0, //
                            0.0, 3.0, 0.0, 1.0, //
                        ],
                    ),
                    (4, bind_matrix(0.0, 0.0, 4.0)),
                ],
            ),
        ],
        vec![
            conn(2, 1),
            conn(3, 1),
            conn(4, 1),
            conn(10, 2),
            conn(20, 10),
            conn(21, 20),
            conn(22, 20),
            conn(3, 21),
            conn(4, 22),
            conn(1, 0),
        ],
    );
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("スキン付きは読める");
    let (model, report) =
        VrmModel::from_fbx(&parsed, &VrmOptions::new()).expect("スキン付きは変換できる");
    assert_eq!(model.skins.len(), 1);
    let skin = &model.skins[0];
    // 関節列はクラスタ順であり、モデルIDを併せ持つ。
    assert_eq!(skin.joints, vec![2, 3]);
    assert_eq!(skin.joint_models, vec![ObjectId::new(3), ObjectId::new(4)]);
    assert_eq!(skin.inverse_bind_matrices.len(), 2);
    // 逆バインド行列とバインド行列の積は単位行列になる。
    let binds = parsed.bind_pose_nodes().expect("バインドは統合できる");
    let by_model: std::collections::HashMap<i64, [f64; 16]> = binds
        .into_iter()
        .map(|node| (node.model.value(), node.matrix))
        .collect();
    for (inverse, joint) in skin
        .inverse_bind_matrices
        .iter()
        .zip(skin.joint_models.iter())
    {
        let product = mul4(inverse, &by_model[&joint.value()]);
        assert!(
            identity_distance(&product) < 1e-6,
            "逆バインド行列とバインド行列の積は単位行列になる"
        );
    }

    let mesh = &model.meshes[0];
    assert_eq!(mesh.skin, Some(0));
    let joints = mesh.vertices.joints.as_ref().expect("関節がある");
    let weights = mesh.vertices.weights.as_ref().expect("重みがある");
    assert_eq!(joints.len(), 3);
    assert_eq!(weights.len(), 3);
    // 制御点0は関節0に重み1、制御点1は0.75と0.25、制御点2は関節1に重み1である。
    assert_eq!(joints[0], [0, 0, 0, 0]);
    assert_eq!(joints[1], [0, 1, 0, 0]);
    assert_eq!(joints[2], [1, 0, 0, 0]);
    assert_eq!(weights[0], [1.0, 0.0, 0.0, 0.0]);
    assert!((weights[1][0] - 0.75).abs() < 1e-6);
    assert!((weights[1][1] - 0.25).abs() < 1e-6);
    assert_eq!(weights[2], [1.0, 0.0, 0.0, 0.0]);
    // すべての頂点で重み和が1であり、関節索引は範囲内である。
    for (joint, weight) in joints.iter().zip(weights.iter()) {
        let sum: f32 = weight.iter().sum();
        assert!((sum - 1.0).abs() < 1e-5, "重み和は1になる");
        for j in joint {
            assert!((*j as usize) < skin.joints.len(), "関節索引は範囲内である");
        }
    }
    // 頂点は所有モデルの平行移動でシーン空間へ移る。
    assert_eq!(
        model.meshes[0].vertices.positions,
        vec![[1.0, 0.0, 0.0], [2.0, 0.0, 0.0], [1.0, 1.0, 0.0]]
    );
    assert_eq!(report.truncated_influence_vertices, 0);
    assert_eq!(report.unweighted_vertices, 0);
}

#[test]
fn 非関節ルートは単位行列になり関節のワールドはバインドと一致する() {
    // 非関節ルート（Null）の下に関節（Hips）がある構成である。
    // ルートのバインド行列はX軸-90度回転であり、正規化で消える。
    let root_bind = 回転バインド();
    let hips_bind = mul4(&root_bind, &bind_matrix(0.0, 1.0, 0.0));
    let bytes = assemble(
        None,
        vec![
            model(1, b"Armature\x00\x01Model", "Null"),
            model(2, b"Hips\x00\x01Model", "LimbNode"),
            model(3, b"Owner\x00\x01Model", "Mesh"),
            triangle_geometry(10, b"Tri\x00\x01Geometry", vec![]),
            deformer(20, b"Skin\x00\x01Deformer", "Skin", vec![]),
            deformer(
                21,
                b"Cluster\x00\x01Deformer",
                "Cluster",
                vec![indexes_node(&[0]), weights_node(&[1.0])],
            ),
            bind_pose(
                100,
                &[
                    (1, root_bind),
                    (2, hips_bind),
                    (3, bind_matrix(0.0, 0.0, 0.0)),
                ],
            ),
        ],
        vec![
            conn(2, 1),
            conn(3, 1),
            conn(10, 3),
            conn(20, 10),
            conn(21, 20),
            conn(2, 21),
            conn(1, 0),
        ],
    );
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("親子構成は読める");
    let (model, _) = VrmModel::from_fbx(&parsed, &VrmOptions::new()).expect("親子構成は変換できる");
    assert_eq!(model.nodes.len(), 3);
    // 非関節ルートは単位行列に正規化される。
    assert_eq!(model.nodes[0].name, "Armature");
    assert_eq!(model.nodes[0].parent, None);
    assert_eq!(
        model.nodes[0].local,
        [
            1.0, 0.0, 0.0, 0.0, //
            0.0, 1.0, 0.0, 0.0, //
            0.0, 0.0, 1.0, 0.0, //
            0.0, 0.0, 0.0, 1.0, //
        ]
    );
    // メッシュ所有モデルは除外されるため、子はHipsだけである。
    assert_eq!(model.nodes[0].children, vec![1]);
    // Hipsの親はArmatureであり、親の出力世界行列が単位行列のため
    // ローカル行列はバインド世界行列そのものになる。
    assert_eq!(model.nodes[1].parent, Some(0));
    assert!(
        行列の差(&model.nodes[1].local, &hips_bind) < 1e-9,
        "関節でないルートの子はバインド世界行列を保つ"
    );
    // Hipsのワールド行列はバインド世界行列と一致する。
    let world = ノードの世界行列(&model, 1);
    assert!(
        行列の差(&world, &hips_bind) < 1e-9,
        "関節のワールド行列はバインド世界行列と一致する"
    );
    // 逆バインド行列との積は単位行列になる。
    let skin = &model.skins[0];
    let product = mul4(&skin.inverse_bind_matrices[0], &world);
    assert!(
        identity_distance(&product) < 1e-6,
        "逆バインド行列とワールド行列の積は単位行列になる"
    );
    ノード双方向整合を確かめる(&model);
}

#[test]
fn 関節であるルートはバインド行列を保つ() {
    // ルート自体が関節（親なしのHips）の構成である。
    let hips_bind = bind_matrix(1.0, 2.0, 3.0);
    let bytes = assemble(
        None,
        vec![
            model(1, b"Hips\x00\x01Model", "LimbNode"),
            model(2, b"Owner\x00\x01Model", "Mesh"),
            triangle_geometry(10, b"Tri\x00\x01Geometry", vec![]),
            deformer(20, b"Skin\x00\x01Deformer", "Skin", vec![]),
            deformer(
                21,
                b"Cluster\x00\x01Deformer",
                "Cluster",
                vec![indexes_node(&[0]), weights_node(&[1.0])],
            ),
            bind_pose(100, &[(1, hips_bind), (2, bind_matrix(0.0, 0.0, 0.0))]),
        ],
        vec![
            conn(1, 0),
            conn(2, 0),
            conn(10, 2),
            conn(20, 10),
            conn(21, 20),
            conn(1, 21),
        ],
    );
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("関節ルートは読める");
    let (model, _) =
        VrmModel::from_fbx(&parsed, &VrmOptions::new()).expect("関節ルートは変換できる");
    assert_eq!(model.nodes[0].name, "Hips");
    assert_eq!(model.nodes[0].parent, None);
    assert_eq!(
        model.nodes[0].local, hips_bind,
        "関節であるルートはバインド世界行列のままである"
    );
    ノード双方向整合を確かめる(&model);
}

#[test]
fn 五つの影響は四つに切り捨てられ正規化される() {
    let mut objs = vec![
        model(1, b"Root\x00\x01Model", "Null"),
        model(2, b"Owner\x00\x01Model", "Mesh"),
        triangle_geometry(10, b"Tri\x00\x01Geometry", vec![]),
        deformer(20, b"Skin\x00\x01Deformer", "Skin", vec![]),
    ];
    let mut conns = vec![conn(2, 1), conn(10, 2), conn(20, 10), conn(1, 0)];
    let mut binds = vec![
        (1, bind_matrix(0.0, 0.0, 0.0)),
        (2, bind_matrix(0.0, 0.0, 0.0)),
    ];
    // 5つの関節が制御点0に影響する。重みは降順で0.5・0.4・0.3・0.2・0.1である。
    for (i, weight) in [0.5, 0.4, 0.3, 0.2, 0.1].iter().enumerate() {
        let joint = 30 + i as i64;
        let cluster = 40 + i as i64;
        objs.push(model(joint, b"J\x00\x01Model", "LimbNode"));
        objs.push(deformer(
            cluster,
            b"C\x00\x01Deformer",
            "Cluster",
            vec![indexes_node(&[0]), weights_node(&[*weight])],
        ));
        conns.push(conn(cluster, 20));
        conns.push(conn(joint, cluster));
        binds.push((joint, bind_matrix(0.0, 0.0, 0.0)));
    }
    objs.push(bind_pose(100, &binds));
    let bytes = assemble(None, objs, conns);
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("5影響は読める");
    let (model, report) =
        VrmModel::from_fbx(&parsed, &VrmOptions::new()).expect("5影響は変換できる");
    let mesh = &model.meshes[0];
    let joints = mesh.vertices.joints.as_ref().expect("関節がある");
    let weights = mesh.vertices.weights.as_ref().expect("重みがある");
    // 上位4件が残り、合計1.2で正規化される。
    assert_eq!(joints[0], [0, 1, 2, 3]);
    let sum = 0.5 + 0.4 + 0.3 + 0.2;
    for (i, want) in [0.5 / sum, 0.4 / sum, 0.3 / sum, 0.2 / sum]
        .iter()
        .enumerate()
    {
        assert!(
            (weights[0][i] as f64 - want).abs() < 1e-6,
            "上位4件が正規化される"
        );
    }
    let total: f32 = weights[0].iter().sum();
    assert!((total - 1.0).abs() < 1e-5);
    assert_eq!(report.truncated_influence_vertices, 1);
    // 影響が無い制御点1と2は既定値になる。
    assert_eq!(report.unweighted_vertices, 2);
    assert_eq!(joints[1], [0, 0, 0, 0]);
    assert_eq!(weights[1], [1.0, 0.0, 0.0, 0.0]);
}

#[test]
fn 重みなし頂点は関節0重み1になる() {
    let bytes = assemble(
        None,
        vec![
            model(1, b"Root\x00\x01Model", "Null"),
            model(2, b"Owner\x00\x01Model", "Mesh"),
            model(3, b"JointA\x00\x01Model", "LimbNode"),
            triangle_geometry(10, b"Tri\x00\x01Geometry", vec![]),
            deformer(20, b"Skin\x00\x01Deformer", "Skin", vec![]),
            deformer(21, b"ClusterA\x00\x01Deformer", "Cluster", vec![]),
            bind_pose(
                100,
                &[
                    (1, bind_matrix(0.0, 0.0, 0.0)),
                    (2, bind_matrix(0.0, 0.0, 0.0)),
                    (3, bind_matrix(0.0, 0.0, 0.0)),
                ],
            ),
        ],
        vec![
            conn(2, 1),
            conn(3, 1),
            conn(10, 2),
            conn(20, 10),
            conn(21, 20),
            conn(3, 21),
            conn(1, 0),
        ],
    );
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("重みなしは読める");
    let (model, report) =
        VrmModel::from_fbx(&parsed, &VrmOptions::new()).expect("重みなしは変換できる");
    let mesh = &model.meshes[0];
    let joints = mesh.vertices.joints.as_ref().expect("関節がある");
    let weights = mesh.vertices.weights.as_ref().expect("重みがある");
    for (joint, weight) in joints.iter().zip(weights.iter()) {
        assert_eq!(*joint, [0, 0, 0, 0]);
        assert_eq!(*weight, [1.0, 0.0, 0.0, 0.0]);
    }
    assert_eq!(report.unweighted_vertices, 3);
    assert_eq!(report.truncated_influence_vertices, 0);
}

#[test]
fn 二つの材質の面は別プリミティブに分かれ番号順に並ぶ() {
    let bytes = assemble(
        None,
        vec![
            model(1, b"M\x00\x01Model", "Mesh"),
            geometry(
                2,
                b"Two\x00\x01Geometry",
                vec![
                    vertices_node(&[
                        0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 2.0, 0.0, 0.0, 2.0, 1.0, 0.0,
                        3.0, 0.0, 0.0,
                    ]),
                    pvi_node(&[0, 1, -3, 3, 4, -6]),
                    material_layer(b"ByPolygon", b"IndexToDirect", &[1, 0]),
                ],
            ),
            material(10, b"MatA\x00\x01Material"),
            material(11, b"MatB\x00\x01Material"),
            bind_pose(100, &[(1, bind_matrix(0.0, 0.0, 0.0))]),
        ],
        vec![conn(1, 0), conn(2, 1), conn(10, 1), conn(11, 1)],
    );
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("2材質は読める");
    let (model, _) = VrmModel::from_fbx(&parsed, &VrmOptions::new()).expect("2材質は変換できる");
    let mesh = &model.meshes[0];
    // スロットは接続順であり、プリミティブは材質番号の昇順である。
    assert_eq!(mesh.material_slots, vec!["MatA", "MatB"]);
    assert_eq!(mesh.primitives.len(), 2);
    assert_eq!(mesh.primitives[0].material, 0);
    assert_eq!(mesh.primitives[0].indices, vec![3, 4, 5]);
    assert_eq!(mesh.primitives[1].material, 1);
    assert_eq!(mesh.primitives[1].indices, vec![0, 1, 2]);
}

#[test]
fn 鏡像バインドでは巻き方向が反転する() {
    // Xを反転するバインド行列であり、行列式は負である。
    let mirrored = [
        -1.0, 0.0, 0.0, 0.0, //
        0.0, 1.0, 0.0, 0.0, //
        0.0, 0.0, 1.0, 0.0, //
        5.0, 0.0, 0.0, 1.0, //
    ];
    let bytes = 最小成功ファイル(bind_matrix(0.0, 0.0, 0.0), mirrored, vec![], vec![]);
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("鏡像は読める");
    let (model, report) =
        VrmModel::from_fbx(&parsed, &VrmOptions::new()).expect("鏡像は変換できる");
    let mesh = &model.meshes[0];
    // 位置は鏡像で移り、三角形の頂点順は反転する。
    assert_eq!(
        mesh.vertices.positions,
        vec![[5.0, 0.0, 0.0], [4.0, 0.0, 0.0], [5.0, 1.0, 0.0]]
    );
    assert_eq!(mesh.primitives[0].indices, vec![0, 2, 1]);
    assert_eq!(report.mirrored_meshes, 1);
}

#[test]
fn 非標準軸では失敗する() {
    // Z-upの軸系は受け付けない。
    let globals = global_settings(2, 1, 0, 1, 1, 1, 1.0);
    let bytes = assemble(
        Some(globals),
        vec![
            model(1, b"Root\x00\x01Model", "Null"),
            model(2, b"Owner\x00\x01Model", "Mesh"),
            triangle_geometry(10, b"Tri\x00\x01Geometry", vec![]),
            bind_pose(
                100,
                &[
                    (1, bind_matrix(0.0, 0.0, 0.0)),
                    (2, bind_matrix(0.0, 0.0, 0.0)),
                ],
            ),
        ],
        vec![conn(2, 1), conn(10, 2), conn(1, 0)],
    );
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("非標準軸も読める");
    let error = VrmModel::from_fbx(&parsed, &VrmOptions::new()).expect_err("非標準軸は失敗する");
    assert_eq!(error.kind(), VrmErrorKind::UnsupportedAxis);
    assert_eq!(error.object_id(), None);
}

#[test]
fn 単位がゼロと負では失敗する() {
    for unit in [0.0, -1.0] {
        let globals = global_settings(1, 1, 2, 1, 0, 1, unit);
        let bytes = assemble(
            Some(globals),
            vec![
                model(1, b"Root\x00\x01Model", "Null"),
                model(2, b"Owner\x00\x01Model", "Mesh"),
                triangle_geometry(10, b"Tri\x00\x01Geometry", vec![]),
                bind_pose(
                    100,
                    &[
                        (1, bind_matrix(0.0, 0.0, 0.0)),
                        (2, bind_matrix(0.0, 0.0, 0.0)),
                    ],
                ),
            ],
            vec![conn(2, 1), conn(10, 2), conn(1, 0)],
        );
        let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("単位不良も読める");
        let error =
            VrmModel::from_fbx(&parsed, &VrmOptions::new()).expect_err("単位0以下は失敗する");
        assert_eq!(error.kind(), VrmErrorKind::UnsupportedUnitScale);
        assert_eq!(error.object_id(), None);
    }
}

#[test]
fn バインド欠落では失敗する() {
    // `BindPose` が無いため、最初のモデルで失敗する。
    let bytes = assemble(
        None,
        vec![
            model(1, b"Root\x00\x01Model", "Null"),
            model(2, b"Owner\x00\x01Model", "Mesh"),
            triangle_geometry(10, b"Tri\x00\x01Geometry", vec![]),
        ],
        vec![conn(2, 1), conn(10, 2), conn(1, 0)],
    );
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("姿勢なしも読める");
    let error =
        VrmModel::from_fbx(&parsed, &VrmOptions::new()).expect_err("バインド欠落は失敗する");
    assert_eq!(error.kind(), VrmErrorKind::MissingBindPose);
    assert_eq!(error.object_id(), Some(ObjectId::new(1)));
}

#[test]
fn メッシュモデルに子があると失敗する() {
    let bytes = assemble(
        None,
        vec![
            model(1, b"Root\x00\x01Model", "Null"),
            model(2, b"Owner\x00\x01Model", "Mesh"),
            model(3, b"Child\x00\x01Model", "Null"),
            triangle_geometry(10, b"Tri\x00\x01Geometry", vec![]),
            bind_pose(
                100,
                &[
                    (1, bind_matrix(0.0, 0.0, 0.0)),
                    (2, bind_matrix(0.0, 0.0, 0.0)),
                    (3, bind_matrix(0.0, 0.0, 0.0)),
                ],
            ),
        ],
        vec![conn(2, 1), conn(3, 2), conn(10, 2), conn(1, 0)],
    );
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("子付きも読める");
    let error =
        VrmModel::from_fbx(&parsed, &VrmOptions::new()).expect_err("メッシュモデルの子は失敗する");
    assert_eq!(error.kind(), VrmErrorKind::MeshModelHasChildren);
    assert_eq!(error.object_id(), Some(ObjectId::new(2)));
}

#[test]
fn 複数スキンでは失敗する() {
    let bytes = assemble(
        None,
        vec![
            model(1, b"Root\x00\x01Model", "Null"),
            model(2, b"Owner\x00\x01Model", "Mesh"),
            model(3, b"JointA\x00\x01Model", "LimbNode"),
            model(4, b"JointB\x00\x01Model", "LimbNode"),
            triangle_geometry(10, b"Tri\x00\x01Geometry", vec![]),
            deformer(20, b"SkinA\x00\x01Deformer", "Skin", vec![]),
            deformer(
                21,
                b"ClusterA\x00\x01Deformer",
                "Cluster",
                vec![indexes_node(&[0]), weights_node(&[1.0])],
            ),
            deformer(30, b"SkinB\x00\x01Deformer", "Skin", vec![]),
            deformer(
                31,
                b"ClusterB\x00\x01Deformer",
                "Cluster",
                vec![indexes_node(&[1]), weights_node(&[1.0])],
            ),
            bind_pose(
                100,
                &[
                    (1, bind_matrix(0.0, 0.0, 0.0)),
                    (2, bind_matrix(0.0, 0.0, 0.0)),
                    (3, bind_matrix(0.0, 0.0, 0.0)),
                    (4, bind_matrix(0.0, 0.0, 0.0)),
                ],
            ),
        ],
        vec![
            conn(2, 1),
            conn(3, 1),
            conn(4, 1),
            conn(10, 2),
            conn(20, 10),
            conn(21, 20),
            conn(3, 21),
            conn(30, 10),
            conn(31, 30),
            conn(4, 31),
            conn(1, 0),
        ],
    );
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("複数スキンも読める");
    let error = VrmModel::from_fbx(&parsed, &VrmOptions::new()).expect_err("複数スキンは失敗する");
    assert_eq!(error.kind(), VrmErrorKind::MultipleSkins);
    assert_eq!(error.object_id(), Some(ObjectId::new(10)));
}

#[test]
fn 上限を超えると失敗する() {
    let bytes = 最小成功ファイル(
        bind_matrix(0.0, 0.0, 0.0),
        bind_matrix(0.0, 0.0, 0.0),
        vec![],
        vec![],
    );
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("最小構成は読める");
    // メッシュ・頂点・三角形のいずれも、0件の上限では通らない。
    let error = VrmModel::from_fbx(&parsed, &VrmOptions::new().with_max_meshes(0))
        .expect_err("メッシュ上限の超過は失敗する");
    assert_eq!(error.kind(), VrmErrorKind::LimitExceeded);
    let error = VrmModel::from_fbx(&parsed, &VrmOptions::new().with_max_vertices(2))
        .expect_err("頂点上限の超過は失敗する");
    assert_eq!(error.kind(), VrmErrorKind::LimitExceeded);
    let error = VrmModel::from_fbx(&parsed, &VrmOptions::new().with_max_triangles(0))
        .expect_err("三角形上限の超過は失敗する");
    assert_eq!(error.kind(), VrmErrorKind::LimitExceeded);
    // 既定値は最小構成を通す。
    VrmModel::from_fbx(&parsed, &VrmOptions::default()).expect("既定値では通る");
}

#[test]
fn 長い鎖でも現実的な時間で変換できる() {
    // 子を先に並べた数千ノードの鎖であり、最初の走査で根まで遡る配置である。
    // 鎖内の所属判定が線形走査の場合は二乗時間になるため、
    // O(1) 判定であることを成功と双方向整合で確かめる。
    const N: i64 = 6000;
    let mut objs = Vec::new();
    for id in (1..=N).rev() {
        let label = format!("Node{id}\x00\x01Model");
        objs.push(model(id, label.as_bytes(), "Null"));
    }
    let mut binds = Vec::with_capacity(N as usize);
    for id in 1..=N {
        binds.push((id, bind_matrix(0.0, 0.0, 0.0)));
    }
    // モデルIDと重ならないPose IDを使う（1..=N がモデルで埋まるため）。
    objs.push(bind_pose(7_000_001, &binds));
    let mut conns = Vec::with_capacity(N as usize);
    for id in 2..=N {
        conns.push(conn(id, id - 1));
    }
    conns.push(conn(1, 0));
    let bytes = assemble(None, objs, conns);
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("長い鎖は読める");
    let (model, report) =
        VrmModel::from_fbx(&parsed, &VrmOptions::new()).expect("長い鎖は変換できる");
    assert_eq!(model.nodes.len(), N as usize);
    assert_eq!(report.node_count, N as usize);
    // 葉から根への一本鎖であり、末尾が根である。
    for (index, node) in model.nodes.iter().enumerate() {
        if index + 1 < N as usize {
            assert_eq!(node.parent, Some(index + 1), "鎖は後方へつながる");
        } else {
            assert_eq!(node.parent, None, "末尾が根である");
        }
    }
    ノード双方向整合を確かめる(&model);
}

#[test]
fn ルートは単位換算だけを含む() {
    // 係数100は等倍、係数1は0.01倍である。
    for (unit, scale) in [(100.0, 1.0), (1.0, 0.01)] {
        let globals = global_settings(1, 1, 2, 1, 0, 1, unit);
        let bytes = assemble(
            Some(globals),
            vec![
                model(1, b"Root\x00\x01Model", "Null"),
                model(2, b"Owner\x00\x01Model", "Mesh"),
                triangle_geometry(10, b"Tri\x00\x01Geometry", vec![]),
                bind_pose(
                    100,
                    &[
                        (1, bind_matrix(0.0, 0.0, 0.0)),
                        (2, bind_matrix(0.0, 0.0, 0.0)),
                    ],
                ),
            ],
            vec![conn(2, 1), conn(10, 2), conn(1, 0)],
        );
        let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("単位換算は読める");
        let (model, _) =
            VrmModel::from_fbx(&parsed, &VrmOptions::new()).expect("単位換算は変換できる");
        assert_eq!(
            model.root,
            [
                scale, 0.0, 0.0, 0.0, //
                0.0, scale, 0.0, 0.0, //
                0.0, 0.0, scale, 0.0, //
                0.0, 0.0, 0.0, 1.0, //
            ],
            "単位{unit}は{scale}倍のルートになる"
        );
    }
}

/// 実fixtureから中間モデルを作り、不変条件を確かめる。
///
/// 固定件数に依存せず、報告と幾何の対応・逆行列の整合・重みの正規化・
/// Y-upの向き・全体の高さを検証する。報告を1行で標準エラーに出す。
#[test]
#[cfg(not(target_arch = "wasm32"))]
fn 実fixtureから中間モデルを作り不変条件を満たす() {
    let Ok(path) = std::env::var("VRCTOM_FBX_FIXTURE") else {
        eprintln!("VRCTOM_FBX_FIXTUREが未設定のため跳ばす");
        return;
    };
    let bytes = std::fs::read(&path).expect("実fixtureは読める");
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("実fixtureは読める");
    let (model, report) =
        VrmModel::from_fbx(&parsed, &VrmOptions::new()).expect("実fixtureは変換できる");

    // 親子の双方向整合を確かめる。固定件数には依存しない。
    ノード双方向整合を確かめる(&model);

    // 報告と幾何の対応を確かめる。
    let owned: Vec<_> = parsed
        .meshes()
        .iter()
        .filter(|mesh| {
            parsed
                .models()
                .iter()
                .any(|owner| owner.mesh == Some(mesh.id))
        })
        .collect();
    assert_eq!(report.node_count, parsed.models().len());
    assert_eq!(report.skin_count, parsed.skins().len());
    assert_eq!(report.mesh_count, owned.len());
    assert_eq!(model.meshes.len(), owned.len());
    let mut want_triangles = 0usize;
    let mut want_primitives = 0usize;
    let mut want_vertices = 0usize;
    for mesh in &owned {
        let data = parsed
            .mesh_data(mesh.id)
            .expect("実fixtureの幾何は展開できる");
        want_triangles += data
            .polygon_corner_counts
            .iter()
            .map(|count| (*count as usize).saturating_sub(2))
            .sum::<usize>();
        want_primitives += if data.material_indices.is_empty() {
            1
        } else {
            let mut kinds = data.material_indices.clone();
            kinds.sort_unstable();
            kinds.dedup();
            kinds.len()
        };
        want_vertices += model
            .meshes
            .iter()
            .find(|converted| converted.geometry == mesh.id)
            .expect("所有メッシュは出力にある")
            .vertices
            .positions
            .len();
    }
    assert_eq!(report.triangle_count, want_triangles);
    assert_eq!(report.primitive_count, want_primitives);
    assert_eq!(report.vertex_count, want_vertices);

    // 逆バインド行列とバインド行列の積は単位行列になる。
    let binds = parsed
        .bind_pose_nodes()
        .expect("実fixtureのBindPoseは統合できる");
    let by_model: std::collections::HashMap<i64, [f64; 16]> = binds
        .into_iter()
        .map(|node| (node.model.value(), node.matrix))
        .collect();
    for skin in &model.skins {
        assert_eq!(skin.joints.len(), skin.joint_models.len());
        assert_eq!(skin.inverse_bind_matrices.len(), skin.joint_models.len());
        for (inverse, joint) in skin
            .inverse_bind_matrices
            .iter()
            .zip(skin.joint_models.iter())
        {
            let product = mul4(inverse, &by_model[&joint.value()]);
            assert!(
                identity_distance(&product) < 1e-6,
                "逆バインド行列とバインド行列の積は単位行列になる"
            );
        }
    }

    // 全頂点の重みと座標の不変条件を確かめ、全体の広がりを求める。
    // 平らな板など高さより幅が大きいメッシュが混ざるため、Y-upの向きは
    // メッシュ単体ではなく全体の広がりで確かめる。
    let (mut min_x, mut max_x) = (f32::INFINITY, f32::NEG_INFINITY);
    let (mut min_y, mut max_y) = (f32::INFINITY, f32::NEG_INFINITY);
    let (mut min_z, mut max_z) = (f32::INFINITY, f32::NEG_INFINITY);
    for mesh in &model.meshes {
        let positions = &mesh.vertices.positions;
        assert!(!positions.is_empty(), "出力メッシュは頂点を持つ");
        let (mut mesh_min_x, mut mesh_max_x) = (f32::INFINITY, f32::NEG_INFINITY);
        let (mut mesh_min_y, mut mesh_max_y) = (f32::INFINITY, f32::NEG_INFINITY);
        let (mut mesh_min_z, mut mesh_max_z) = (f32::INFINITY, f32::NEG_INFINITY);
        for position in positions {
            assert!(position.iter().all(|v| v.is_finite()), "位置は有限である");
            mesh_min_x = mesh_min_x.min(position[0]);
            mesh_max_x = mesh_max_x.max(position[0]);
            mesh_min_y = mesh_min_y.min(position[1]);
            mesh_max_y = mesh_max_y.max(position[1]);
            mesh_min_z = mesh_min_z.min(position[2]);
            mesh_max_z = mesh_max_z.max(position[2]);
        }
        eprintln!(
            "mesh_extent name={} x={:.3} y={:.3} z={:.3} triangles={} materials={}",
            mesh.name,
            mesh_max_x - mesh_min_x,
            mesh_max_y - mesh_min_y,
            mesh_max_z - mesh_min_z,
            mesh.primitives
                .iter()
                .map(|primitive| primitive.indices.len() / 3)
                .sum::<usize>(),
            mesh.material_slots.len()
        );
        min_x = min_x.min(mesh_min_x);
        max_x = max_x.max(mesh_max_x);
        min_y = min_y.min(mesh_min_y);
        max_y = max_y.max(mesh_max_y);
        min_z = min_z.min(mesh_min_z);
        max_z = max_z.max(mesh_max_z);
        if let Some(normals) = &mesh.vertices.normals {
            assert_eq!(normals.len(), positions.len());
            for normal in normals {
                assert!(normal.iter().all(|v| v.is_finite()), "法線は有限である");
            }
        }
        for set in &mesh.vertices.uv_sets {
            assert_eq!(set.len(), positions.len());
            for uv in set {
                assert!(uv.iter().all(|v| v.is_finite()), "UVは有限である");
            }
        }
        match (&mesh.vertices.joints, &mesh.vertices.weights) {
            (Some(joints), Some(weights)) => {
                assert_eq!(joints.len(), positions.len());
                assert_eq!(weights.len(), positions.len());
                let skin = &model.skins[mesh.skin.expect("スキン付きは索引を持つ")];
                for (joint, weight) in joints.iter().zip(weights.iter()) {
                    assert!(weight.iter().all(|v| v.is_finite()), "重みは有限である");
                    let sum: f32 = weight.iter().sum();
                    assert!((sum - 1.0).abs() < 1e-4, "重み和は1になる");
                    for j in joint {
                        assert!((*j as usize) < skin.joints.len(), "関節索引は範囲内である");
                    }
                }
            }
            (None, None) => {}
            _ => panic!("関節と重みの有無は一致する"),
        }
    }
    // 全体がY-upである。すなわち全体の高さがX/Zの広がりより大きい。
    assert!(
        max_y - min_y > max_x - min_x,
        "全体の高さがXの広がりより大きい"
    );
    assert!(
        max_y - min_y > max_z - min_z,
        "全体の高さがZの広がりより大きい"
    );
    // ルートの拡大率でメートルに直し、全体の高さが人型の範囲に収まる。
    let height_metres = (max_y - min_y) * model.root[0] as f32;
    assert!(
        (0.5..=3.0).contains(&height_metres),
        "全体の高さが0.5〜3.0メートル程度である: {height_metres}"
    );

    // 非関節ルート `Armature` は単位行列に正規化される。
    // 存在しないfixtureでは何も確かめない。
    if let Some(armature) = model.nodes.iter().position(|node| node.name == "Armature") {
        assert_eq!(
            model.nodes[armature].local,
            [
                1.0, 0.0, 0.0, 0.0, //
                0.0, 1.0, 0.0, 0.0, //
                0.0, 0.0, 1.0, 0.0, //
                0.0, 0.0, 0.0, 1.0, //
            ],
            "非関節ルートArmatureのローカルは単位行列である"
        );
        // 子のHipsのワールド行列はバインド世界行列と一致する。
        if let Some(&hips) = model.nodes[armature]
            .children
            .iter()
            .find(|&&child| model.nodes[child].name == "Hips")
        {
            let world = ノードの世界行列(&model, hips);
            let model_id = parsed.models()[hips].id.value();
            let bind = &by_model[&model_id];
            assert!(
                行列の差(&world, bind) < 1e-6,
                "Hipsのワールド行列はバインド世界行列と一致する"
            );
        }
    }

    eprintln!(
        "vrm_report node_count={} skin_count={} mesh_count={} skipped_meshes={} primitive_count={} vertex_count={} triangle_count={} truncated_influence_vertices={} unweighted_vertices={} non_triangle_polygons={} mirrored_meshes={} height_metres={height_metres:.3}",
        report.node_count,
        report.skin_count,
        report.mesh_count,
        report.skipped_meshes,
        report.primitive_count,
        report.vertex_count,
        report.triangle_count,
        report.truncated_influence_vertices,
        report.unweighted_vertices,
        report.non_triangle_polygons,
        report.mirrored_meshes,
    );
}

#[test]
fn 三角形未満の多角形は失敗する() {
    // 2 cornerだけの面であり、読み取り時に破損として拒否される。
    let bytes = assemble(
        None,
        vec![
            model(1, b"Root\x00\x01Model", "Null"),
            model(2, b"Owner\x00\x01Model", "Mesh"),
            geometry(
                10,
                b"G\x00\x01Geometry",
                vec![
                    vertices_node(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0]),
                    pvi_node(&[0, -2]),
                ],
            ),
            bind_pose(
                100,
                &[
                    (1, bind_matrix(0.0, 0.0, 0.0)),
                    (2, bind_matrix(0.0, 0.0, 0.0)),
                ],
            ),
        ],
        vec![conn(2, 1), conn(10, 2), conn(1, 0)],
    );
    // 要約時の面数え上げで3未満を検出するため、解析時に失敗する。
    let error = FbxFile::parse(&bytes, &FbxOptions::new()).expect_err("2角形は失敗する");
    assert_eq!(error.kind(), vrctom::fbx::FbxErrorKind::InvalidStructure);
}

#[test]
fn 材質番号がスロット範囲外では失敗する() {
    // 1面だけあるが材質番号5を指し、スロット1件に収まらない。
    let bytes = assemble(
        None,
        vec![
            model(1, b"M\x00\x01Model", "Mesh"),
            geometry(
                2,
                b"G\x00\x01Geometry",
                vec![
                    vertices_node(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]),
                    pvi_node(&[0, 1, -3]),
                    material_layer(b"ByPolygon", b"IndexToDirect", &[5]),
                ],
            ),
            material(10, b"MatA\x00\x01Material"),
            bind_pose(100, &[(1, bind_matrix(0.0, 0.0, 0.0))]),
        ],
        vec![conn(1, 0), conn(2, 1), conn(10, 1)],
    );
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("材質範囲外も読める");
    let error =
        VrmModel::from_fbx(&parsed, &VrmOptions::new()).expect_err("材質番号範囲外は失敗する");
    assert_eq!(error.kind(), VrmErrorKind::InvalidStructure);
}

#[test]
fn スロットなしで材質番号1は失敗する() {
    // スロットが無いのに材質番号1を指す面であり、0だけが許される。
    let bytes = assemble(
        None,
        vec![
            model(1, b"M\x00\x01Model", "Mesh"),
            geometry(
                2,
                b"G\x00\x01Geometry",
                vec![
                    vertices_node(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]),
                    pvi_node(&[0, 1, -3]),
                    material_layer(b"ByPolygon", b"IndexToDirect", &[1]),
                ],
            ),
            bind_pose(100, &[(1, bind_matrix(0.0, 0.0, 0.0))]),
        ],
        vec![conn(1, 0), conn(2, 1)],
    );
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("スロットなしも読める");
    let error =
        VrmModel::from_fbx(&parsed, &VrmOptions::new()).expect_err("スロットなしの番号1は失敗する");
    assert_eq!(error.kind(), VrmErrorKind::InvalidStructure);
}

/// 実fixtureで幾何法線と頂点法線の向きが一致することを確かめる。
///
/// 各メッシュから数三角形ずつ取り出し、位置から求めた幾何法線と
/// ベイク済み頂点法線の内積が負にならないことを確かめる。
/// 逆転置の誤りや巻き方向の誤りがあると負の内積が現れる。
#[test]
#[cfg(not(target_arch = "wasm32"))]
fn 実fixtureの幾何法線と頂点法線の向きが一致する() {
    let Ok(path) = std::env::var("VRCTOM_FBX_FIXTURE") else {
        eprintln!("VRCTOM_FBX_FIXTUREが未設定のため跳ばす");
        return;
    };
    let bytes = std::fs::read(&path).expect("実fixtureは読める");
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("実fixtureは読める");
    let (model, _) =
        VrmModel::from_fbx(&parsed, &VrmOptions::new()).expect("実fixtureは変換できる");
    let mut checked = 0usize;
    for mesh in &model.meshes {
        let Some(normals) = &mesh.vertices.normals else {
            continue;
        };
        // プリミティブごとに先頭から数三角形ずつ取り出す。
        // 全数では時間がかかるため、向きの誤りは先頭でも検出できる。
        for primitive in &mesh.primitives {
            for triangle in primitive.indices.chunks_exact(3).take(3) {
                let [i0, i1, i2] = [
                    triangle[0] as usize,
                    triangle[1] as usize,
                    triangle[2] as usize,
                ];
                let [p0, p1, p2] = [
                    mesh.vertices.positions[i0],
                    mesh.vertices.positions[i1],
                    mesh.vertices.positions[i2],
                ];
                let e1 = [p1[0] - p0[0], p1[1] - p0[1], p1[2] - p0[2]];
                let e2 = [p2[0] - p0[0], p2[1] - p0[1], p2[2] - p0[2]];
                let cross = [
                    e1[1] * e2[2] - e1[2] * e2[1],
                    e1[2] * e2[0] - e1[0] * e2[2],
                    e1[0] * e2[1] - e1[1] * e2[0],
                ];
                let len = (cross[0] * cross[0] + cross[1] * cross[1] + cross[2] * cross[2]).sqrt();
                // 縮退した三角形は向きが定まらないため飛ばす。
                if len <= 1e-12 {
                    continue;
                }
                let geometric = [cross[0] / len, cross[1] / len, cross[2] / len];
                for &index in &[i0, i1, i2] {
                    let vertex = normals[index];
                    let dot = geometric[0] * vertex[0]
                        + geometric[1] * vertex[1]
                        + geometric[2] * vertex[2];
                    assert!(
                        dot >= 0.0,
                        "幾何法線と頂点法線の内積は負にならない: mesh={} dot={dot}",
                        mesh.name
                    );
                    checked += 1;
                }
            }
        }
    }
    assert!(checked > 0, "法線付き三角形を1件以上確かめる");
    eprintln!("normal_orientation_checked={checked}");
}

/// 式とターゲットの対応付け1件を組み立てる。
fn 束ね(profile_expression: &str, target: &str) -> vrctom::vrm::VrmExpressionBinding {
    vrctom::vrm::VrmExpressionBinding {
        expression: profile_expression.to_string(),
        target: target.to_string(),
    }
}

/// `Geometry`（Shape）ノードを組み立てる。
fn 形状(id: i64, name: &[u8], indexes: &[i32], deltas: &[f64]) -> TNode {
    let mut index_props = Props::default();
    index_props.i_raw(indexes);
    let mut delta_props = Props::default();
    delta_props.d_raw(deltas);
    let mut props = Props::default();
    props.l(id);
    props.s(name);
    props.s(b"Shape");
    TNode::branch(
        b"Geometry",
        props,
        vec![
            TNode::leaf(b"Indexes", index_props),
            TNode::leaf(b"Vertices", delta_props),
        ],
    )
}

/// `Deformer`（BlendShapeChannel）ノードを組み立てる。
fn 伝達子(id: i64, name: &[u8], deform: Option<f64>, weights: Option<&[f64]>) -> TNode {
    let mut children = Vec::new();
    if let Some(value) = deform {
        let mut props = Props::default();
        props.d(value);
        children.push(TNode::leaf(b"DeformPercent", props));
    }
    if let Some(values) = weights {
        let mut props = Props::default();
        props.d_raw(values);
        children.push(TNode::leaf(b"FullWeights", props));
    }
    deformer(id, name, "BlendShapeChannel", children)
}

/// `Deformer`（BlendShape）ノードを組み立てる。
fn 束ねノード(id: i64, name: &[u8]) -> TNode {
    deformer(id, name, "BlendShape", vec![])
}

#[test]
fn 表情つきの中間モデルはターゲットと束ねを選択順に持つ() {
    let bytes = 表情最小ファイル(
        &[
            ("t1", "t1-shape", &[0], &[0.5, 0.0, 0.0], Some(0.0), None),
            (
                "t2",
                "t2-shape",
                &[1],
                &[0.0, 0.5, 0.0],
                Some(0.0),
                Some(&[100.0]),
            ),
        ],
        vec![],
        vec![],
        &[],
    );
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("表情付きは読める");
    let profile = VrmExpressionProfile {
        bindings: vec![束ね("aa", "t1"), 束ね("blink", "t2")],
    };
    let options = VrmOptions::new().with_expressions(profile);
    let (model, report) = VrmModel::from_fbx(&parsed, &options).expect("表情付きは変換できる");
    // ターゲットは選択順であり、名前はチャンネル名である。
    assert_eq!(model.meshes.len(), 1);
    let mesh = &model.meshes[0];
    assert_eq!(mesh.targets.len(), 2);
    assert_eq!(mesh.targets[0].name, "t1");
    assert_eq!(mesh.targets[1].name, "t2");
    // 所有モデルのバインドは平行移動だけのため、線形部分は単位行列であり差分は変わらない。
    // 頂点は所有モデルの平行移動 (1,2,3) でベイク済みである。
    assert_eq!(mesh.vertices.positions[0], [1.0, 2.0, 3.0]);
    assert_eq!(mesh.targets[0].positions.len(), 3);
    assert_eq!(mesh.targets[0].positions[0], [0.5, 0.0, 0.0]);
    assert_eq!(mesh.targets[0].positions[1], [0.0, 0.0, 0.0]);
    assert_eq!(mesh.targets[1].positions[1], [0.0, 0.5, 0.0]);
    assert_eq!(mesh.targets[1].positions[0], [0.0, 0.0, 0.0]);
    // 束ねはプロファイル順であり、重み1・非二値・上書きなしである。
    assert_eq!(model.expressions.preset.len(), 2);
    assert_eq!(model.expressions.preset[0].name, "aa");
    assert_eq!(model.expressions.preset[0].morph_target_binds.len(), 1);
    let bind = &model.expressions.preset[0].morph_target_binds[0];
    assert_eq!(bind.mesh, 0);
    assert_eq!(bind.target, 0);
    assert_eq!(bind.weight, 1.0);
    assert!(!model.expressions.preset[0].is_binary);
    assert_eq!(
        model.expressions.preset[0].override_blink,
        vrctom::vrm::VrmExpressionOverride::None
    );
    assert_eq!(
        model.expressions.preset[0].override_look_at,
        vrctom::vrm::VrmExpressionOverride::None
    );
    assert_eq!(
        model.expressions.preset[0].override_mouth,
        vrctom::vrm::VrmExpressionOverride::None
    );
    assert_eq!(model.expressions.preset[1].name, "blink");
    assert_eq!(model.expressions.preset[1].morph_target_binds[0].target, 1);
    assert_eq!(report.morph_target_count, 2);
    assert_eq!(report.expression_count, 2);
}

#[test]
fn 表情の要求が無い場合は従来どおり作らない() {
    let bytes = 表情最小ファイル(
        &[("t1", "t1-shape", &[0], &[0.5, 0.0, 0.0], Some(0.0), None)],
        vec![],
        vec![],
        &[],
    );
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("表情付きは読める");
    let (model, report) =
        VrmModel::from_fbx(&parsed, &VrmOptions::new()).expect("要求なしは変換できる");
    assert!(model.meshes[0].targets.is_empty());
    assert!(model.expressions.preset.is_empty());
    assert_eq!(report.morph_target_count, 0);
    assert_eq!(report.expression_count, 0);
}

/// 表情チャンネル1件分の入力（名前、形状名、制御点、差分、初期値、重み率）。
type 表情チャンネル<'a> = (
    &'a str,
    &'a str,
    &'a [i32],
    &'a [f64],
    Option<f64>,
    Option<&'a [f64]>,
);

/// 表情付きの最小ファイル（三角形・所有モデル・束ね1組）を組み立てる。
///
/// チャンネルごとの形状・初期値・重み率を引数で変える。
/// 追加の `Objects` と `Connections` とバインド行列を足せる。
#[allow(clippy::too_many_arguments)]
fn 表情最小ファイル(
    channels: &[表情チャンネル<'_>],
    extra_objects: Vec<TNode>,
    extra_connections: Vec<TNode>,
    extra_binds: &[(i64, [f64; 16])],
) -> Vec<u8> {
    let mut objs = vec![
        model(1, b"Root\x00\x01Model", "Null"),
        model(2, b"Owner\x00\x01Model", "Mesh"),
        triangle_geometry(10, b"Tri\x00\x01Geometry", vec![]),
    ];
    let mut conns = vec![conn(2, 1), conn(10, 2), conn(1, 0)];
    let mut binds = vec![
        (1, bind_matrix(0.0, 0.0, 0.0)),
        (2, bind_matrix(1.0, 2.0, 3.0)),
    ];
    binds.extend_from_slice(extra_binds);
    // 形状・チャンネルのIDは数百件のダミーでも固定IDと衝突しないよう離れた基数から採る。
    objs.push(束ねノード(3000, b"Blend\x00\x01Deformer"));
    for (offset, (channel_name, shape_name, indexes, deltas, deform, weights)) in
        channels.iter().enumerate()
    {
        let shape_id = 1000 + offset as i64;
        let channel_id = 2000 + offset as i64;
        let mut channel_full = channel_name.as_bytes().to_vec();
        channel_full.extend_from_slice(b"\x00\x01SubDeformer");
        let mut shape_full = shape_name.as_bytes().to_vec();
        shape_full.extend_from_slice(b"\x00\x01Shape");
        objs.push(形状(shape_id, &shape_full, indexes, deltas));
        objs.push(伝達子(channel_id, &channel_full, *deform, *weights));
        conns.push(conn(shape_id, channel_id));
        conns.push(conn(channel_id, 3000));
        conns.push(conn(3000, 10));
    }
    objs.extend(extra_objects);
    conns.extend(extra_connections);
    objs.push(bind_pose(100, &binds));
    assemble(None, objs, conns)
}

#[test]
fn 部分一致は存在する式だけを作り欠落を警告する() {
    // 要求3件のうち2件だけが採用メッシュにある。存在する式だけを作る。
    let bytes = 表情最小ファイル(
        &[
            ("t1", "t1-shape", &[0], &[0.5, 0.0, 0.0], Some(0.0), None),
            (
                "t2",
                "t2-shape",
                &[1],
                &[0.0, 0.5, 0.0],
                Some(0.0),
                Some(&[100.0]),
            ),
        ],
        vec![],
        vec![],
        &[],
    );
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("表情付きは読める");
    let profile = VrmExpressionProfile {
        bindings: vec![
            束ね("aa", "t1"),
            束ね("blink", "t2"),
            束ね("ou", "無い名前"),
        ],
    };
    let options = VrmOptions::new().with_expressions(profile);
    let (model, report) = VrmModel::from_fbx(&parsed, &options).expect("部分一致は成功する");
    // 存在する2式だけが要求順に残る。
    assert_eq!(model.expressions.preset.len(), 2);
    assert_eq!(model.expressions.preset[0].name, "aa");
    assert_eq!(model.expressions.preset[1].name, "blink");
    assert_eq!(report.expression_count, 2, "式数は生成式数と一致する");
    // 欠落警告は要求順に1件ずつであり、名前・未適用・採用メッシュ識別を含む。
    assert_eq!(report.warnings.len(), 1);
    assert!(
        report.warnings[0].contains("無い名前"),
        "欠落名を含む: {}",
        report.warnings[0]
    );
}

#[test]
fn 一致0件は成功し式なしで全要求を警告する() {
    // 要求のどれにも一致しない場合は表情を作らず、全要求を警告する。
    let bytes = 表情最小ファイル(
        &[("t1", "t1-shape", &[0], &[0.5, 0.0, 0.0], Some(0.0), None)],
        vec![],
        vec![],
        &[],
    );
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("表情付きは読める");
    let profile = VrmExpressionProfile {
        bindings: vec![束ね("aa", "無い1"), 束ね("blink", "無い2")],
    };
    let options = VrmOptions::new().with_expressions(profile);
    let (model, report) = VrmModel::from_fbx(&parsed, &options).expect("0件一致は成功する");
    assert!(model.expressions.preset.is_empty());
    assert!(model.meshes[0].targets.is_empty());
    assert_eq!(report.expression_count, 0);
    // 欠落警告は要求順に全件であり、追加の集約警告は重ねない。
    assert_eq!(report.warnings.len(), 2);
    assert!(
        report.warnings[0].contains("無い1"),
        "要求順: {}",
        report.warnings[0]
    );
    assert!(
        report.warnings[1].contains("無い2"),
        "要求順: {}",
        report.warnings[1]
    );
}

#[test]
fn 空プロファイルは警告なしで表情を作らない() {
    // 表情指定なし・束ね空のいずれも表情なし・警告なしである。
    let bytes = 表情最小ファイル(
        &[("t1", "t1-shape", &[0], &[0.5, 0.0, 0.0], Some(0.0), None)],
        vec![],
        vec![],
        &[],
    );
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("表情付きは読める");
    for profile in [
        None,
        Some(VrmExpressionProfile {
            bindings: Vec::new(),
        }),
    ] {
        let mut options = VrmOptions::new();
        if let Some(profile) = profile {
            options = options.with_expressions(profile);
        }
        let (model, report) = VrmModel::from_fbx(&parsed, &options).expect("要求なしは変換できる");
        assert!(model.expressions.preset.is_empty());
        assert!(report.warnings.is_empty());
        assert_eq!(report.expression_count, 0);
    }
}

#[test]
fn 顔メッシュが最多なら他メッシュの少数一致より選ばれる() {
    // 顔メッシュが2件一致・他メッシュが1件一致なら顔メッシュを採用する。
    let bytes = 表情最小ファイル(
        &[
            ("t1", "t1-shape", &[0], &[0.5, 0.0, 0.0], Some(0.0), None),
            ("t2", "t2-shape", &[1], &[0.0, 0.5, 0.0], Some(0.0), None),
        ],
        vec![
            model(3, b"Owner2\x00\x01Model", "Mesh"),
            triangle_geometry(11, b"Second\x00\x01Geometry", vec![]),
            形状(50, b"t1-copy\x00\x01Shape", &[0], &[0.0, 0.5, 0.0]),
            伝達子(60, b"t1\x00\x01SubDeformer", Some(0.0), None),
            束ねノード(70, b"SecondBlend\x00\x01Deformer"),
        ],
        vec![
            conn(3, 1),
            conn(11, 3),
            conn(50, 60),
            conn(60, 70),
            conn(70, 11),
        ],
        &[(3, bind_matrix(0.0, 0.0, 0.0))],
    );
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("2メッシュは読める");
    let profile = VrmExpressionProfile {
        bindings: vec![束ね("aa", "t1"), 束ね("blink", "t2")],
    };
    let options = VrmOptions::new().with_expressions(profile);
    let (model, report) = VrmModel::from_fbx(&parsed, &options).expect("最多メッシュを採用する");
    // 表情を持つのは顔メッシュ（先頭）だけであり、警告は無い。
    assert!(!model.meshes[0].targets.is_empty());
    assert!(model.meshes[1].targets.is_empty());
    assert!(report.warnings.is_empty());
    assert_eq!(model.expressions.preset.len(), 2);
}

#[test]
fn 他メッシュだけにある要求は合成せず未適用で警告する() {
    // 採用メッシュ（2件一致）に無い要求は、他メッシュにあっても警告する。
    let bytes = 表情最小ファイル(
        &[
            ("t1", "t1-shape", &[0], &[0.5, 0.0, 0.0], Some(0.0), None),
            ("t3", "t3-shape", &[2], &[0.0, 0.0, 0.5], Some(0.0), None),
        ],
        vec![
            model(3, b"Owner2\x00\x01Model", "Mesh"),
            triangle_geometry(11, b"Second\x00\x01Geometry", vec![]),
            形状(50, b"t2-copy\x00\x01Shape", &[0], &[0.0, 0.5, 0.0]),
            伝達子(60, b"t2\x00\x01SubDeformer", Some(0.0), None),
            束ねノード(70, b"SecondBlend\x00\x01Deformer"),
        ],
        vec![
            conn(3, 1),
            conn(11, 3),
            conn(50, 60),
            conn(60, 70),
            conn(70, 11),
        ],
        &[(3, bind_matrix(0.0, 0.0, 0.0))],
    );
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("2メッシュは読める");
    let profile = VrmExpressionProfile {
        bindings: vec![束ね("a", "t1"), 束ね("b", "t2"), 束ね("c", "t3")],
    };
    let options = VrmOptions::new().with_expressions(profile);
    let (model, report) = VrmModel::from_fbx(&parsed, &options).expect("部分適用は成功する");
    // 他メッシュにだけある `t2` の式は作らず、採用メッシュ内の2件だけ作る。
    assert_eq!(model.expressions.preset.len(), 2);
    assert_eq!(model.expressions.preset[0].name, "a");
    assert_eq!(model.expressions.preset[1].name, "c");
    assert_eq!(report.warnings.len(), 1);
    assert!(
        report.warnings[0].contains("t2"),
        "未適用名を含む: {}",
        report.warnings[0]
    );
}

#[test]
fn 同名チャンネルが複数あっても一致数は増えない() {
    // 採用候補の数え方は異なる名前の数であり、同名の重なりで加点しない。
    // 先頭メッシュに `t1` が2件・後続メッシュに `t1` が1件なら同率であり、
    // 重複側を採用して束ね重複で失敗するのではなく同率失敗になる。
    let bytes = 表情最小ファイル(
        &[("t1", "t1-shape", &[0], &[0.5, 0.0, 0.0], Some(0.0), None)],
        vec![
            model(3, b"Owner2\x00\x01Model", "Mesh"),
            triangle_geometry(11, b"Second\x00\x01Geometry", vec![]),
            形状(50, b"t1-dup\x00\x01Shape", &[1], &[0.0, 0.5, 0.0]),
            伝達子(60, b"t1\x00\x01SubDeformer", Some(0.0), None),
            形状(51, b"t1-second-dup\x00\x01Shape", &[2], &[0.0, 0.0, 0.5]),
            伝達子(61, b"t1\x00\x01SubDeformer", Some(0.0), None),
            束ねノード(70, b"SecondBlend\x00\x01Deformer"),
            // 先頭メッシュ側にも同名を2件繋ぎ、両メッシュとも異なる名前数は1にする。
            形状(52, b"t1-face-dup\x00\x01Shape", &[1], &[0.0, 0.5, 0.0]),
            伝達子(62, b"t1\x00\x01SubDeformer", Some(0.0), None),
        ],
        vec![
            conn(3, 1),
            conn(11, 3),
            conn(50, 60),
            conn(60, 70),
            conn(51, 61),
            conn(61, 70),
            conn(70, 11),
            conn(52, 62),
            conn(62, 3000),
        ],
        &[(3, bind_matrix(0.0, 0.0, 0.0))],
    );
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("重複があっても読める");
    let profile = VrmExpressionProfile {
        bindings: vec![束ね("aa", "t1")],
    };
    let options = VrmOptions::new().with_expressions(profile);
    let error = VrmModel::from_fbx(&parsed, &options).expect_err("同率は失敗する");
    assert_eq!(error.kind(), VrmErrorKind::AmbiguousExpressionTarget);
    assert!(
        format!("{error}").contains("重複する"),
        "同名重複は空扱いにせず重複失敗である: {error}"
    );
}

#[test]
fn 同一ターゲットの複数束ねでも欠落警告は1件である() {
    // 同一ターゲットを指す束ねが複数あっても、欠落警告は要求ごとに1件である。
    let bytes = 表情最小ファイル(
        &[("t1", "t1-shape", &[0], &[0.5, 0.0, 0.0], Some(0.0), None)],
        vec![],
        vec![],
        &[],
    );
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("表情付きは読める");
    let profile = VrmExpressionProfile {
        bindings: vec![束ね("aa", "無い名前"), 束ね("blink", "無い名前")],
    };
    let options = VrmOptions::new().with_expressions(profile);
    let (model, report) = VrmModel::from_fbx(&parsed, &options).expect("欠落は成功する");
    assert!(model.expressions.preset.is_empty());
    assert_eq!(
        report.warnings.len(),
        1,
        "同一ターゲットの警告は重複排除する"
    );
    assert!(report.warnings[0].contains("無い名前"));
}

#[test]
fn 正の最大一致数で同率なら失敗する() {
    // 2件のメッシュが同じ最大一致数なら、採用を決められず失敗する。
    let bytes = 表情最小ファイル(
        &[("t1", "t1-shape", &[0], &[0.5, 0.0, 0.0], Some(0.0), None)],
        vec![
            model(3, b"Owner2\x00\x01Model", "Mesh"),
            triangle_geometry(11, b"Second\x00\x01Geometry", vec![]),
            形状(50, b"t1-copy\x00\x01Shape", &[0], &[0.0, 0.5, 0.0]),
            伝達子(60, b"t1\x00\x01SubDeformer", Some(0.0), None),
            束ねノード(70, b"SecondBlend\x00\x01Deformer"),
        ],
        vec![
            conn(3, 1),
            conn(11, 3),
            conn(50, 60),
            conn(60, 70),
            conn(70, 11),
        ],
        &[(3, bind_matrix(0.0, 0.0, 0.0))],
    );
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("2メッシュは読める");
    let profile = VrmExpressionProfile {
        bindings: vec![束ね("aa", "t1"), 束ね("blink", "無い名前")],
    };
    let options = VrmOptions::new().with_expressions(profile);
    let error = VrmModel::from_fbx(&parsed, &options).expect_err("同率は失敗する");
    assert_eq!(error.kind(), VrmErrorKind::AmbiguousExpressionTarget);
}

#[test]
fn 採用メッシュに所有モデルが無い場合は失敗する() {
    // 採用候補のメッシュに所有モデルが無い場合は束ねが宙に浮くため失敗する。
    // 所有者ありの先頭メッシュには要求外のチャンネルだけを置き、
    // 要求名を持つ所有者なしメッシュを採用候補にする。
    let bytes = 表情最小ファイル(
        &[(
            "other",
            "other-shape",
            &[0],
            &[0.5, 0.0, 0.0],
            Some(0.0),
            None,
        )],
        vec![
            形状(50, b"t9-shape\x00\x01Shape", &[0], &[0.5, 0.0, 0.0]),
            伝達子(60, b"t9\x00\x01SubDeformer", Some(0.0), None),
            束ねノード(70, b"OrphanBlend\x00\x01Deformer"),
            triangle_geometry(11, b"Orphan\x00\x01Geometry", vec![]),
        ],
        vec![conn(50, 60), conn(60, 70), conn(70, 11)],
        &[],
    );
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("所有者なしも読める");
    let profile = VrmExpressionProfile {
        bindings: vec![束ね("aa", "t9")],
    };
    let options = VrmOptions::new().with_expressions(profile);
    assert_eq!(
        VrmModel::from_fbx(&parsed, &options).unwrap_err().kind(),
        VrmErrorKind::InvalidStructure
    );
}

/// 2メッシュ同率の合成ファイルを作る。先頭と後続のチャンネル差分を変えられる。
#[allow(clippy::too_many_arguments)]
fn 同率2メッシュファイル(
    先頭差分: &[f64],
    後続差分: &[f64],
    追加bind: &[(i64, [f64; 16])],
) -> Vec<u8> {
    表情最小ファイル(
        &[("t1", "t1-shape", &[0], 先頭差分, Some(0.0), None)],
        vec![
            model(3, b"Owner2\x00\x01Model", "Mesh"),
            triangle_geometry(11, b"Second\x00\x01Geometry", vec![]),
            形状(50, b"t1-copy\x00\x01Shape", &[0], 後続差分),
            伝達子(60, b"t1\x00\x01SubDeformer", Some(0.0), None),
            束ねノード(70, b"SecondBlend\x00\x01Deformer"),
        ],
        vec![
            conn(3, 1),
            conn(11, 3),
            conn(50, 60),
            conn(60, 70),
            conn(70, 11),
        ],
        追加bind,
    )
}

#[test]
fn 同一集合で唯一非ゼロなら採用して選択警告を出す() {
    // 後続メッシュの差分がゼロなら先頭を採用し、選択警告の後に欠落警告を置く。
    let bytes = 同率2メッシュファイル(&[0.5, 0.0, 0.0], &[0.0, 0.0, 0.0], &[]);
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("2メッシュは読める");
    let profile = VrmExpressionProfile {
        bindings: vec![束ね("aa", "t1"), 束ね("blink", "無い名前")],
    };
    let options = VrmOptions::new().with_expressions(profile);
    let (model, report) = VrmModel::from_fbx(&parsed, &options).expect("限定解消は成功する");
    assert_eq!(model.expressions.preset.len(), 1);
    assert_eq!(model.expressions.preset[0].name, "aa");
    // 表情を持つのは先頭メッシュだけである。
    assert!(!model.meshes[0].targets.is_empty());
    assert!(model.meshes[1].targets.is_empty());
    assert_eq!(report.expression_count, 1);
    assert_eq!(report.warnings.len(), 2);
    assert!(
        report.warnings[0].contains("同率候補から"),
        "選択警告が先頭である: {}",
        report.warnings[0]
    );
    assert!(
        !report.warnings[0].contains("無い名前"),
        "選択警告に欠落名は含まない: {}",
        report.warnings[0]
    );
    assert!(
        report.warnings[1].contains("無い名前"),
        "欠落警告が要求順に続く: {}",
        report.warnings[1]
    );
}

#[test]
fn 非ゼロが後続メッシュでも順序に依らず採用する() {
    // 先頭がゼロ・後続が非ゼロなら後続を採用する。FBX順・ObjectId大小に依らない。
    let bytes = 同率2メッシュファイル(&[0.0, 0.0, 0.0], &[0.5, 0.0, 0.0], &[]);
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("2メッシュは読める");
    let profile = VrmExpressionProfile {
        bindings: vec![束ね("aa", "t1")],
    };
    let options = VrmOptions::new().with_expressions(profile);
    let (model, report) = VrmModel::from_fbx(&parsed, &options).expect("後続を採用する");
    assert_eq!(model.expressions.preset.len(), 1);
    let bind = &model.expressions.preset[0].morph_target_binds[0];
    assert_eq!(bind.mesh, 1, "後続メッシュを指す");
    assert!(model.meshes[0].targets.is_empty());
    assert!(!model.meshes[1].targets.is_empty());
    assert_eq!(report.warnings.len(), 1);
    assert!(report.warnings[0].contains("同率候補から"));
}

#[test]
fn 同一集合で全候補ゼロなら曖昧さエラーになる() {
    let bytes = 同率2メッシュファイル(&[0.0, 0.0, 0.0], &[0.0, 0.0, 0.0], &[]);
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("2メッシュは読める");
    let profile = VrmExpressionProfile {
        bindings: vec![束ね("aa", "t1")],
    };
    let options = VrmOptions::new().with_expressions(profile);
    let error = VrmModel::from_fbx(&parsed, &options).expect_err("全ゼロは失敗する");
    assert_eq!(error.kind(), VrmErrorKind::AmbiguousExpressionTarget);
}

#[test]
fn 同率で異なる集合なら曖昧さエラーになる() {
    // 先頭 `{t1,t2}`・後続 `{t1,t3}` で同率2件の場合は集合が異なるため失敗する。
    let bytes = 表情最小ファイル(
        &[
            ("t1", "t1-shape", &[0], &[0.5, 0.0, 0.0], Some(0.0), None),
            ("t2", "t2-shape", &[1], &[0.0, 0.5, 0.0], Some(0.0), None),
        ],
        vec![
            model(3, b"Owner2\x00\x01Model", "Mesh"),
            triangle_geometry(11, b"Second\x00\x01Geometry", vec![]),
            形状(50, b"t1-copy\x00\x01Shape", &[0], &[0.0, 0.5, 0.0]),
            伝達子(60, b"t1\x00\x01SubDeformer", Some(0.0), None),
            形状(51, b"t3-copy\x00\x01Shape", &[1], &[0.0, 0.0, 0.5]),
            伝達子(61, b"t3\x00\x01SubDeformer", Some(0.0), None),
            束ねノード(70, b"SecondBlend\x00\x01Deformer"),
        ],
        vec![
            conn(3, 1),
            conn(11, 3),
            conn(50, 60),
            conn(60, 70),
            conn(51, 61),
            conn(61, 70),
            conn(70, 11),
        ],
        &[(3, bind_matrix(0.0, 0.0, 0.0))],
    );
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("2メッシュは読める");
    let profile = VrmExpressionProfile {
        bindings: vec![束ね("a", "t1"), 束ね("b", "t2"), 束ね("c", "t3")],
    };
    let options = VrmOptions::new().with_expressions(profile);
    let error = VrmModel::from_fbx(&parsed, &options).expect_err("集合が異なれば失敗する");
    assert_eq!(error.kind(), VrmErrorKind::AmbiguousExpressionTarget);
    assert!(
        format!("{error}").contains("t2") && format!("{error}").contains("t3"),
        "一致した要求名を診断に含める: {error}"
    );
}

#[test]
fn マイナスゼロと空と微小値の非ゼロ判定に従う() {
    // `-0.0` はゼロ、微小でも非ゼロ（`1e-300`）は非ゼロである。
    for (先頭, 後続, 成功か) in [
        (&[-0.0, 0.0, 0.0][..], &[0.0, 0.0, 0.0][..], false),
        (&[1e-300, 0.0, 0.0][..], &[0.0, 0.0, 0.0][..], true),
    ] {
        let bytes = 同率2メッシュファイル(先頭, 後続, &[]);
        let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("2メッシュは読める");
        let profile = VrmExpressionProfile {
            bindings: vec![束ね("aa", "t1")],
        };
        let options = VrmOptions::new().with_expressions(profile);
        if 成功か {
            let (model, report) =
                VrmModel::from_fbx(&parsed, &options).expect("微小非ゼロは採用する");
            assert_eq!(model.expressions.preset.len(), 1);
            assert_eq!(report.warnings.len(), 1);
        } else {
            assert_eq!(
                VrmModel::from_fbx(&parsed, &options).unwrap_err().kind(),
                VrmErrorKind::AmbiguousExpressionTarget,
                "-0.0はゼロとして全ゼロ失敗になる"
            );
        }
    }
    // 空の形状（対象制御点なし）はゼロとして扱う。
    let bytes = 表情最小ファイル(
        &[("t1", "t1-shape", &[0], &[0.5, 0.0, 0.0], Some(0.0), None)],
        vec![
            model(3, b"Owner2\x00\x01Model", "Mesh"),
            triangle_geometry(11, b"Second\x00\x01Geometry", vec![]),
            形状(50, b"t1-empty\x00\x01Shape", &[], &[]),
            伝達子(60, b"t1\x00\x01SubDeformer", Some(0.0), None),
            束ねノード(70, b"SecondBlend\x00\x01Deformer"),
        ],
        vec![
            conn(3, 1),
            conn(11, 3),
            conn(50, 60),
            conn(60, 70),
            conn(70, 11),
        ],
        &[(3, bind_matrix(0.0, 0.0, 0.0))],
    );
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("空形状も読める");
    let profile = VrmExpressionProfile {
        bindings: vec![束ね("aa", "t1")],
    };
    let options = VrmOptions::new().with_expressions(profile);
    let (model, _) =
        VrmModel::from_fbx(&parsed, &options).expect("空形状はゼロ扱いで先頭を採用する");
    assert_eq!(model.expressions.preset.len(), 1);
}

#[test]
fn 同率候補の不正は空扱いにせず既存エラーを返す() {
    // 後続候補の一致ターゲットが複数フレームなら、次点へ移らず失敗する。
    let bytes = 表情最小ファイル(
        &[("t1", "t1-shape", &[0], &[0.5, 0.0, 0.0], Some(0.0), None)],
        vec![
            model(3, b"Owner2\x00\x01Model", "Mesh"),
            triangle_geometry(11, b"Second\x00\x01Geometry", vec![]),
            形状(50, b"t1-copy\x00\x01Shape", &[0], &[0.0, 0.5, 0.0]),
            形状(51, b"t1-second\x00\x01Shape", &[1], &[0.0, 0.0, 0.5]),
            伝達子(60, b"t1\x00\x01SubDeformer", Some(0.0), None),
            束ねノード(70, b"SecondBlend\x00\x01Deformer"),
        ],
        vec![
            conn(3, 1),
            conn(11, 3),
            conn(50, 60),
            conn(51, 60),
            conn(60, 70),
            conn(70, 11),
        ],
        &[(3, bind_matrix(0.0, 0.0, 0.0))],
    );
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("複数形状でも読める");
    let profile = VrmExpressionProfile {
        bindings: vec![束ね("aa", "t1")],
    };
    let options = VrmOptions::new().with_expressions(profile);
    assert_eq!(
        VrmModel::from_fbx(&parsed, &options).unwrap_err().kind(),
        VrmErrorKind::UnsupportedBlendShape,
        "複数フレームは空扱いにしない"
    );
    // 非有限の重み率も空扱いにせず読み取り層の破損として失敗する。
    let bytes = 表情最小ファイル(
        &[("t1", "t1-shape", &[0], &[0.5, 0.0, 0.0], Some(0.0), None)],
        vec![
            model(3, b"Owner2\x00\x01Model", "Mesh"),
            triangle_geometry(11, b"Second\x00\x01Geometry", vec![]),
            形状(50, b"t1-copy\x00\x01Shape", &[0], &[0.0, 0.5, 0.0]),
            伝達子(60, b"t1\x00\x01SubDeformer", Some(0.0), Some(&[f64::NAN])),
            束ねノード(70, b"SecondBlend\x00\x01Deformer"),
        ],
        vec![
            conn(3, 1),
            conn(11, 3),
            conn(50, 60),
            conn(60, 70),
            conn(70, 11),
        ],
        &[(3, bind_matrix(0.0, 0.0, 0.0))],
    );
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("破損値でも読める");
    let profile = VrmExpressionProfile {
        bindings: vec![束ね("aa", "t1")],
    };
    let options = VrmOptions::new().with_expressions(profile);
    assert_eq!(
        VrmModel::from_fbx(&parsed, &options).unwrap_err().kind(),
        VrmErrorKind::InvalidStructure,
        "非有限の重み率は空扱いにしない"
    );
}

#[test]
fn 先頭非ゼロでも後続の不正は隠さず失敗する() {
    // 両メッシュとも `{t1,t2}` に一致し、先頭の `t1` は非ゼロ。
    // 後続の `t2` が複数フレームでも、非ゼロ確定で返さず全要求を検査する。
    let bytes = 表情最小ファイル(
        &[
            ("t1", "t1-shape", &[0], &[0.5, 0.0, 0.0], Some(0.0), None),
            ("t2", "t2-shape", &[1], &[0.0, 0.5, 0.0], Some(0.0), None),
        ],
        vec![
            model(3, b"Owner2\x00\x01Model", "Mesh"),
            triangle_geometry(11, b"Second\x00\x01Geometry", vec![]),
            形状(50, b"t1-copy\x00\x01Shape", &[0], &[0.0, 0.5, 0.0]),
            伝達子(60, b"t1\x00\x01SubDeformer", Some(0.0), None),
            形状(51, b"t2-copy\x00\x01Shape", &[1], &[0.0, 0.0, 0.5]),
            形状(52, b"t2-second\x00\x01Shape", &[2], &[0.5, 0.0, 0.0]),
            伝達子(61, b"t2\x00\x01SubDeformer", Some(0.0), None),
            束ねノード(70, b"SecondBlend\x00\x01Deformer"),
        ],
        vec![
            conn(3, 1),
            conn(11, 3),
            conn(50, 60),
            conn(60, 70),
            conn(51, 61),
            conn(52, 61),
            conn(61, 70),
            conn(70, 11),
        ],
        &[(3, bind_matrix(0.0, 0.0, 0.0))],
    );
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("複数形状でも読める");
    let profile = VrmExpressionProfile {
        bindings: vec![束ね("a", "t1"), 束ね("b", "t2")],
    };
    let options = VrmOptions::new().with_expressions(profile);
    assert_eq!(
        VrmModel::from_fbx(&parsed, &options).unwrap_err().kind(),
        VrmErrorKind::UnsupportedBlendShape,
        "後続ターゲットの不正は隠さない"
    );
}

#[test]
fn 同率候補でも総量上限の超過は形状不良より先に失敗する() {
    // `max_morph_bytes(0)` では単独候補と同様に `LimitExceeded` になる。
    let bytes = 同率2メッシュファイル(&[0.5, 0.0, 0.0], &[0.0, 0.0, 0.0], &[]);
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("2メッシュは読める");
    let profile = VrmExpressionProfile {
        bindings: vec![束ね("aa", "t1")],
    };
    let options = VrmOptions::new()
        .with_expressions(profile)
        .with_max_morph_bytes(0);
    assert_eq!(
        VrmModel::from_fbx(&parsed, &options).unwrap_err().kind(),
        VrmErrorKind::LimitExceeded,
        "同率経路でも上限で拒否する"
    );
}

#[test]
fn 束ね省略後もモーフ索引は正しい() {
    // 欠落した束ねを除いた後も、残る束ねは絞り込み後の正しい索引を指す。
    let bytes = 表情最小ファイル(
        &[
            ("t1", "t1-shape", &[0], &[0.5, 0.0, 0.0], Some(0.0), None),
            (
                "vrc.v_aa",
                "vaa-shape",
                &[1],
                &[0.0, 0.5, 0.0],
                Some(0.0),
                None,
            ),
            ("t2", "t2-shape", &[2], &[0.0, 0.0, 0.5], Some(0.0), None),
        ],
        vec![],
        vec![],
        &[],
    );
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("表情付きは読める");
    let profile = VrmExpressionProfile {
        bindings: vec![
            束ね("b2", "t2"),
            束ね("b1", "t1"),
            束ね("missing", "無い名前"),
        ],
    };
    let options = VrmOptions::new().with_expressions(profile);
    let (model, report) = VrmModel::from_fbx(&parsed, &options).expect("部分適用は成功する");
    // 絞り込み後はFBX順の3件であり、式は要求順（`t2`・`t1`）に正しい索引を指す。
    let mesh = &model.meshes[0];
    assert_eq!(mesh.targets.len(), 3);
    assert_eq!(mesh.targets[0].name, "t1");
    assert_eq!(mesh.targets[1].name, "vrc.v_aa");
    assert_eq!(mesh.targets[2].name, "t2");
    assert_eq!(model.expressions.preset.len(), 2);
    assert_eq!(model.expressions.preset[0].morph_target_binds[0].target, 2);
    assert_eq!(model.expressions.preset[1].morph_target_binds[0].target, 0);
    assert_eq!(report.expression_count, 2);
    assert_eq!(report.warnings.len(), 1);
}

#[test]
fn 全要求を含むメッシュが複数ある場合は失敗する() {
    // 2件目のメッシュにも同じターゲット名を持たせ、曖昧さを確かめる。
    let bytes = 表情最小ファイル(
        &[("t1", "t1-shape", &[0], &[0.5, 0.0, 0.0], Some(0.0), None)],
        vec![
            model(3, b"Owner2\x00\x01Model", "Mesh"),
            triangle_geometry(11, b"Second\x00\x01Geometry", vec![]),
            形状(50, b"t1-copy\x00\x01Shape", &[0], &[0.0, 0.5, 0.0]),
            伝達子(60, b"t1\x00\x01SubDeformer", Some(0.0), None),
            束ねノード(70, b"SecondBlend\x00\x01Deformer"),
        ],
        vec![
            conn(3, 1),
            conn(11, 3),
            conn(50, 60),
            conn(60, 70),
            conn(70, 11),
        ],
        &[(3, bind_matrix(0.0, 0.0, 0.0))],
    );
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("2メッシュは読める");
    let profile = VrmExpressionProfile {
        bindings: vec![束ね("aa", "t1")],
    };
    let options = VrmOptions::new().with_expressions(profile);
    assert_eq!(
        VrmModel::from_fbx(&parsed, &options).unwrap_err().kind(),
        VrmErrorKind::AmbiguousExpressionTarget
    );
}

#[test]
fn 同名ターゲットの重複は失敗する() {
    // 同一メッシュに同名チャンネルを2件繋ぎ、重複を確かめる。
    let bytes = 表情最小ファイル(
        &[("t1", "t1-shape", &[0], &[0.5, 0.0, 0.0], Some(0.0), None)],
        vec![
            形状(50, b"t1-dup\x00\x01Shape", &[1], &[0.0, 0.5, 0.0]),
            伝達子(60, b"t1\x00\x01SubDeformer", Some(0.0), None),
        ],
        vec![conn(50, 60), conn(60, 3000)],
        &[],
    );
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("重複があっても読める");
    let profile = VrmExpressionProfile {
        bindings: vec![束ね("aa", "t1")],
    };
    let options = VrmOptions::new().with_expressions(profile);
    assert_eq!(
        VrmModel::from_fbx(&parsed, &options).unwrap_err().kind(),
        VrmErrorKind::AmbiguousExpressionTarget
    );
}

#[test]
fn 単一フレームでないチャンネルは失敗する() {
    // 1チャンネルに形状を2件繋ぐ。
    let bytes = 表情最小ファイル(
        &[("t1", "t1-shape", &[0], &[0.5, 0.0, 0.0], Some(0.0), None)],
        vec![形状(
            50,
            b"t1-second\x00\x01Shape",
            &[1],
            &[0.0, 0.5, 0.0],
        )],
        vec![conn(50, 2000)],
        &[],
    );
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("複数形状でも読める");
    let profile = VrmExpressionProfile {
        bindings: vec![束ね("aa", "t1")],
    };
    let options = VrmOptions::new().with_expressions(profile);
    assert_eq!(
        VrmModel::from_fbx(&parsed, &options).unwrap_err().kind(),
        VrmErrorKind::UnsupportedBlendShape
    );
}

#[test]
fn 非100の重み率と非ゼロの初期値は失敗する() {
    for (重み率, 初期値, 期待) in [
        (
            Some(&[100.0, 50.0][..]),
            Some(0.0),
            VrmErrorKind::UnsupportedBlendShape,
        ),
        // 非有限の重み率は読み取り層で破損として失敗する。
        (
            Some(&[100.0, f64::NAN][..]),
            Some(0.0),
            VrmErrorKind::InvalidStructure,
        ),
        (
            Some(&[100.0][..]),
            Some(0.5),
            VrmErrorKind::UnsupportedBlendShape,
        ),
        (
            Some(&[100.0][..]),
            Some(f64::INFINITY),
            VrmErrorKind::UnsupportedBlendShape,
        ),
        (
            Some(&[100.0][..]),
            None,
            VrmErrorKind::UnsupportedBlendShape,
        ),
    ] {
        let bytes = 表情最小ファイル(
            &[(
                "t1",
                "t1-shape",
                &[0, 1],
                &[0.5, 0.0, 0.0, 0.0, 0.0, 0.5],
                初期値,
                重み率,
            )],
            vec![],
            vec![],
            &[],
        );
        let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("破損値は読める");
        let profile = VrmExpressionProfile {
            bindings: vec![束ね("aa", "t1")],
        };
        let options = VrmOptions::new().with_expressions(profile);
        assert_eq!(
            VrmModel::from_fbx(&parsed, &options).unwrap_err().kind(),
            期待,
            "重み率={重み率:?} 初期値={初期値:?} は失敗する"
        );
    }
}

#[test]
fn 空と100と長い全100の重み率は受理する() {
    // 空・単一100・形状数と同じ長さの全100を受け付ける。
    let bytes = 表情最小ファイル(
        &[
            ("t1", "t1-shape", &[0], &[0.5, 0.0, 0.0], Some(0.0), None),
            (
                "t2",
                "t2-shape",
                &[1],
                &[0.0, 0.5, 0.0],
                Some(0.0),
                Some(&[100.0]),
            ),
            (
                "t3",
                "t3-shape",
                &[0, 1, 2],
                &[0.1, 0.0, 0.0, 0.0, 0.1, 0.0, 0.0, 0.0, 0.1],
                Some(0.0),
                Some(&[100.0, 100.0, 100.0]),
            ),
        ],
        vec![],
        vec![],
        &[],
    );
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("全100は読める");
    let profile = VrmExpressionProfile {
        bindings: vec![束ね("a", "t1"), 束ね("b", "t2"), 束ね("c", "t3")],
    };
    let options = VrmOptions::new().with_expressions(profile);
    let (model, _) = VrmModel::from_fbx(&parsed, &options).expect("全100は受理する");
    assert_eq!(model.meshes[0].targets.len(), 3);
}

/// 互換形状の絞り込みはFBX順に束ね参照・`vrc.`・MMD名だけを残す。
///
/// 667件相当のダミー（体形スライダ名600件を含む）から、束ね参照・大小文字混在の
/// `vrc.*`・MMD名・NFKC差（半角カタカナ・全角数字）がFBX順に残り、
/// 体形スライダ・ARKit形状名・その他の名前が落ちることを確かめる。
/// 顔追跡を書かないためARKit名は残さない。
#[test]
fn 互換形状だけがfbx順に残り体形スライダは落ちる() {
    // 残す名前と落とす名前を混ぜてFBX順に並べる。真偽は残すかどうかである。
    let mut names: Vec<(String, bool)> = Vec::new();
    for i in 0..600 {
        names.push((format!("スライダ_身長_{i:03}"), false));
    }
    for i in 0..44 {
        names.push((format!("スライダ_体重_{i:03}"), false));
    }
    names.extend(
        [
            ("my_special_blink", true),
            ("vrc.v_aa", true),
            ("VRC.V_OU", true),
            ("eyeBlinkLeft", false),
            ("笑い", true),
            ("jawOpen", false),
            ("ウィンク", true),
            ("mouthOpenCustom", false),
            ("ｳｨﾝｸ２右", true),
            ("スライダ_体重", false),
            ("ｷﾘｯ", true),
            ("Body_Slim", false),
            ("あ2", true),
            ("てへぺろ２", true),
            ("にやり２", true),
            ("瞳縦潰れ", true),
            ("ハイライト消", true),
            ("映り込み消", true),
            ("∧", true),
            ("前", true),
            ("ω□", true),
            ("口角上げ", true),
            ("まばたき", true),
        ]
        .iter()
        .map(|(name, keep)| ((*name).to_owned(), *keep)),
    );
    assert_eq!(names.len(), 667, "667件相当のダミーである");
    const INDEXES: &[i32] = &[0];
    const DELTAS: &[f64] = &[0.5, 0.0, 0.0];
    let channels: Vec<表情チャンネル<'_>> = names
        .iter()
        .map(|(name, _)| (name.as_str(), "shape", INDEXES, DELTAS, Some(0.0), None))
        .collect();
    let bytes = 表情最小ファイル(&channels, vec![], vec![], &[]);
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("667件相当は読める");
    let profile = VrmExpressionProfile {
        bindings: vec![束ね("aa", "vrc.v_aa"), 束ね("blink", "my_special_blink")],
    };
    let options = VrmOptions::new().with_expressions(profile);
    let (model, report) = VrmModel::from_fbx(&parsed, &options).expect("絞り込みは通る");
    // 残す名前だけがFBX順に並ぶ。
    let want: Vec<&str> = names
        .iter()
        .filter(|(_, keep)| *keep)
        .map(|(name, _)| name.as_str())
        .collect();
    let mesh = &model.meshes[0];
    assert_eq!(mesh.targets.len(), want.len(), "残す名前だけが残る");
    for (target, name) in mesh.targets.iter().zip(want.iter()) {
        assert_eq!(&target.name, name);
    }
    // 束ねはプロファイル順であり、絞り込み後の索引を名前で指す。
    assert_eq!(model.expressions.preset.len(), 2);
    for (preset, target) in [("aa", "vrc.v_aa"), ("blink", "my_special_blink")] {
        let entry = model
            .expressions
            .preset
            .iter()
            .find(|entry| entry.name == preset)
            .expect("式がある");
        assert_eq!(entry.morph_target_binds.len(), 1);
        let bind = &entry.morph_target_binds[0];
        let index = want
            .iter()
            .position(|name| *name == target)
            .expect("束ね先は残る");
        assert_eq!(bind.mesh, 0);
        assert_eq!(bind.target, index);
        assert_eq!(bind.weight, 1.0);
    }
    assert_eq!(report.morph_target_count, want.len());
    assert_eq!(report.expression_count, 2);
}

/// Unity出力と同じ57件のターゲット名（`milfy-unity.vrm` の `Body` の `targetNames`）。
const UNITY_TARGET_NAMES_57: &[&str] = &[
    "vrc.v_sil",
    "vrc.v_pp",
    "vrc.v_aa",
    "vrc.v_oh",
    "vrc.v_ou",
    "vrc.v_ih",
    "vrc.v_th",
    "vrc.v_nn",
    "vrc.v_dd",
    "vrc.v_kk",
    "vrc.v_ff",
    "vrc.v_e",
    "vrc.v_ch",
    "vrc.v_rr",
    "vrc.v_ss",
    "vrc.blink",
    "vrc.looking_up",
    "vrc.looking_down",
    "あ",
    "い",
    "う",
    "え",
    "お",
    "まばたき",
    "なごみ",
    "笑い",
    "ウィンク",
    "ウィンク右",
    "ウィンク２",
    "ｳｨﾝｸ２右",
    "はぅ",
    "びっくり",
    "じと目",
    "ｷﾘｯ",
    "星目",
    "はぁと",
    "瞳小",
    "光下",
    "▲",
    "ワ",
    "ω",
    "ω□",
    "□",
    "ん",
    "ぺろっ",
    "にやり",
    "にっこり",
    "口角上げ",
    "口角下げ",
    "口横広げ",
    "真面目",
    "困る",
    "にこり",
    "怒り",
    "上",
    "下",
    "はちゅ目",
];

/// 実fixtureの表情はUnityと同じ57件のターゲットと8式の束ねを持つ。
///
/// ターゲット集合がUnityの `targetNames` と完全一致し、8式の束ねが
/// （メッシュ・ターゲット名・重み1.0）でUnityと一致することを確かめる。
/// 順序はUnityと異なりFBXのチャンネル順であるため、束ねは名前で突き合わせ、
/// 順序自体がFBX順であることも確かめる。
#[test]
#[cfg(not(target_arch = "wasm32"))]
fn 実fixtureの表情はunityと同じ57件と8束ねを持つ() {
    let Ok(path) = std::env::var("VRCTOM_FBX_FIXTURE") else {
        eprintln!("VRCTOM_FBX_FIXTUREが未設定のため跳ばす");
        return;
    };
    let bytes = std::fs::read(&path).expect("実fixtureは読める");
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("実fixtureは読める");
    let options = VrmOptions::new().with_expressions(VrmExpressionProfile::vrchat());
    let (model, report) =
        VrmModel::from_fbx(&parsed, &options).expect("実fixtureは表情付きで変換できる");
    assert_eq!(report.morph_target_count, 57);
    assert_eq!(report.expression_count, 8);
    // 表情を持つメッシュは1件だけである。
    let 表情あり: Vec<usize> = model
        .meshes
        .iter()
        .enumerate()
        .filter(|(_, mesh)| !mesh.targets.is_empty())
        .map(|(index, _)| index)
        .collect();
    assert_eq!(表情あり.len(), 1, "表情を持つメッシュは1件である");
    let mesh_index = 表情あり[0];
    let mesh = &model.meshes[mesh_index];
    // 集合がUnityの57件と完全一致する。
    let got: std::collections::HashSet<&str> = mesh
        .targets
        .iter()
        .map(|target| target.name.as_str())
        .collect();
    let want: std::collections::HashSet<&str> = UNITY_TARGET_NAMES_57.iter().copied().collect();
    assert_eq!(got, want, "targetNamesの集合がUnityと一致する");
    // 順序はFBXのチャンネル順である。重複は無い。
    assert_eq!(got.len(), mesh.targets.len(), "ターゲット名に重複は無い");
    let all: Vec<String> = parsed
        .blend_shape_channels(mesh.geometry)
        .iter()
        .map(|channel| channel.name.clone())
        .collect();
    eprintln!(
        "expression_channels_total={} kept={}",
        all.len(),
        mesh.targets.len()
    );
    let mut last: Option<usize> = None;
    for target in &mesh.targets {
        let pos = all
            .iter()
            .position(|name| name == &target.name)
            .expect("ターゲットはFBXのチャンネルにある");
        if let Some(prev) = last {
            assert!(prev < pos, "ターゲット順はFBX順である: {}", target.name);
        }
        last = Some(pos);
    }
    // 8式の束ねが（メッシュ・ターゲット名・重み）でUnityと一致する。
    assert_eq!(model.expressions.preset.len(), 8);
    for binding in &VrmExpressionProfile::vrchat().bindings {
        let entry = model
            .expressions
            .preset
            .iter()
            .find(|entry| entry.name == binding.expression)
            .expect("8式がある");
        assert_eq!(entry.morph_target_binds.len(), 1);
        let bind = &entry.morph_target_binds[0];
        assert_eq!(bind.mesh, mesh_index);
        let index = mesh
            .targets
            .iter()
            .position(|target| target.name == binding.target)
            .expect("束ね先は残る");
        assert_eq!(bind.target, index);
        assert_eq!(bind.weight, 1.0);
    }
}

#[test]
fn 表情の上限は確保前に発動する() {
    let bytes = 表情最小ファイル(
        &[
            ("t1", "t1-shape", &[0], &[0.5, 0.0, 0.0], Some(0.0), None),
            ("t2", "t2-shape", &[1], &[0.0, 0.5, 0.0], Some(0.0), None),
        ],
        vec![],
        vec![],
        &[],
    );
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("表情付きは読める");
    let profile = || VrmExpressionProfile {
        bindings: vec![束ね("a", "t1"), 束ね("b", "t2")],
    };
    // ターゲット数上限。
    let options = VrmOptions::new()
        .with_expressions(profile())
        .with_max_morph_targets(1);
    assert_eq!(
        VrmModel::from_fbx(&parsed, &options).unwrap_err().kind(),
        VrmErrorKind::LimitExceeded
    );
    // 総バイト数上限。差分を展開する前の検査で0バイトは失敗する。
    let options = VrmOptions::new()
        .with_expressions(profile())
        .with_max_morph_bytes(0);
    assert_eq!(
        VrmModel::from_fbx(&parsed, &options).unwrap_err().kind(),
        VrmErrorKind::LimitExceeded
    );
    // 総バイト数上限。3頂点×2件×12バイト=72バイトのため、71バイトでは足りない。
    let options = VrmOptions::new()
        .with_expressions(profile())
        .with_max_morph_bytes(71);
    assert_eq!(
        VrmModel::from_fbx(&parsed, &options).unwrap_err().kind(),
        VrmErrorKind::LimitExceeded
    );
    // 72バイトちょうどは通る。
    let options = VrmOptions::new()
        .with_expressions(profile())
        .with_max_morph_bytes(72);
    let (model, _) = VrmModel::from_fbx(&parsed, &options).expect("上限ちょうどは通る");
    assert_eq!(model.meshes[0].targets.len(), 2);
}

/// 四角形の角0を2回使い、UVを変えて4頂点に分けるファイルを組み立てる。
/// `!0 == -1` が最終角の終端を表す。
fn 表情四角形ファイル() -> Vec<u8> {
    let quad = geometry(
        10,
        b"Quad\x00\x01Geometry",
        vec![
            vertices_node(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0]),
            pvi_node(&[0, 1, 2, -1]),
            uv_layer(
                b"map1",
                b"ByPolygonVertex",
                b"IndexToDirect",
                &[0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0, 1.0],
                Some(&[0, 1, 2, 3]),
            ),
        ],
    );
    let mut objs = vec![
        model(1, b"Root\x00\x01Model", "Null"),
        model(2, b"Owner\x00\x01Model", "Mesh"),
        quad,
        束ねノード(40, b"Blend\x00\x01Deformer"),
        形状(20, b"t1-shape\x00\x01Shape", &[0], &[0.5, 0.0, 0.0]),
        伝達子(30, b"t1\x00\x01SubDeformer", Some(0.0), None),
    ];
    let conns = vec![
        conn(2, 1),
        conn(10, 2),
        conn(1, 0),
        conn(20, 30),
        conn(30, 40),
        conn(40, 10),
    ];
    let binds = vec![
        (1, bind_matrix(0.0, 0.0, 0.0)),
        (2, bind_matrix(1.0, 2.0, 3.0)),
    ];
    objs.push(bind_pose(100, &binds));
    assemble(None, objs, conns)
}

#[test]
fn 継ぎ目で複製された頂点すべてに差分が配られる() {
    let bytes = 表情四角形ファイル();
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("四角形は読める");
    let profile = VrmExpressionProfile {
        bindings: vec![束ね("aa", "t1")],
    };
    let options = VrmOptions::new().with_expressions(profile);
    let (model, _) = VrmModel::from_fbx(&parsed, &options).expect("表情付き四角形は変換できる");
    let mesh = &model.meshes[0];
    // 角0がUV違いで2頂点になり、角1・角2と合わせて4頂点になる。
    assert_eq!(mesh.vertices.positions.len(), 4);
    let target = &mesh.targets[0];
    assert_eq!(target.name, "t1");
    let mut seam_vertices = 0;
    for (position, delta) in mesh.vertices.positions.iter().zip(&target.positions) {
        if position == &[1.0, 2.0, 3.0] {
            seam_vertices += 1;
            assert_eq!(*delta, [0.5, 0.0, 0.0], "継ぎ目の複製にも差分が要る");
        } else {
            assert_eq!(*delta, [0.0, 0.0, 0.0]);
        }
    }
    assert_eq!(seam_vertices, 2, "角0はUV違いで2頂点になる");
}

/// 基準姿勢の重み1件を作る。キーはジオメトリID、値はチャンネルIDとUnity重みである。
fn 基準姿勢(geometry: i64, channel: i64, weight: f64) -> BaselineWeights {
    BaselineWeights {
        meshes: HashMap::from([(
            ObjectId::new(geometry),
            vec![BaselineChannel {
                channel: ObjectId::new(channel),
                weight,
            }],
        )]),
    }
}

#[test]
fn 基準姿勢50はベースへ半分焼き込みターゲットを半分にする() {
    // チャンネルt1（ID2000）の差分 [0.5,0,0] を重み50で焼き込む。
    // ベースには (50/100)*差分を足し、ターゲットには (1-50/100) を掛ける。
    let bytes = 表情最小ファイル(
        &[("t1", "t1-shape", &[0], &[0.5, 0.0, 0.0], Some(0.0), None)],
        vec![],
        vec![],
        &[],
    );
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("表情付きは読める");
    let profile = VrmExpressionProfile {
        bindings: vec![束ね("aa", "t1")],
    };
    let options = VrmOptions::new()
        .with_expressions(profile)
        .with_baseline_weights(基準姿勢(10, 2000, 50.0));
    let (model, report) = VrmModel::from_fbx(&parsed, &options).expect("焼き込みは変換できる");
    // 所有モデルの平行移動 (1,2,3) でベイクした頂点へ半分を足す。
    assert_eq!(model.meshes[0].vertices.positions[0], [1.25, 2.0, 3.0]);
    assert_eq!(model.meshes[0].vertices.positions[1], [2.0, 2.0, 3.0]);
    // ターゲットは半分に縮む。
    assert_eq!(model.meshes[0].targets.len(), 1);
    assert_eq!(model.meshes[0].targets[0].positions[0], [0.25, 0.0, 0.0]);
    assert_eq!(model.meshes[0].targets[0].positions[1], [0.0, 0.0, 0.0]);
    // 束ねは残り、件数は一致する。
    assert_eq!(model.expressions.preset.len(), 1);
    assert_eq!(report.expression_count, 1);
    assert_eq!(report.baseline_meshes, 1);
    assert_eq!(report.baseline_shapes, 1);
    assert!(report.warnings.is_empty(), "欠落も基準姿勢警告も無い");
}

#[test]
fn 基準姿勢なしは報告ゼロで従来どおりになる() {
    // 同じ入力を基準姿勢なしで変換し、焼き込みが無いことを確かめる。
    let bytes = 表情最小ファイル(
        &[("t1", "t1-shape", &[0], &[0.5, 0.0, 0.0], Some(0.0), None)],
        vec![],
        vec![],
        &[],
    );
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("表情付きは読める");
    let profile = VrmExpressionProfile {
        bindings: vec![束ね("aa", "t1")],
    };
    let options = VrmOptions::new().with_expressions(profile);
    let (model, report) = VrmModel::from_fbx(&parsed, &options).expect("要求なしでも変換できる");
    assert_eq!(model.meshes[0].vertices.positions[0], [1.0, 2.0, 3.0]);
    assert_eq!(model.meshes[0].targets[0].positions[0], [0.5, 0.0, 0.0]);
    assert_eq!(report.baseline_meshes, 0);
    assert_eq!(report.baseline_shapes, 0);
}

#[test]
fn 空の基準姿勢は結果を変えない() {
    // 空の適用値は焼き込みなしと同じであり、既存のvrc経路を変えない。
    let bytes = 表情最小ファイル(
        &[("t1", "t1-shape", &[0], &[0.5, 0.0, 0.0], Some(0.0), None)],
        vec![],
        vec![],
        &[],
    );
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("表情付きは読める");
    let profile = VrmExpressionProfile {
        bindings: vec![束ね("aa", "t1")],
    };
    let plain = VrmOptions::new().with_expressions(profile.clone());
    let (plain_model, plain_report) =
        VrmModel::from_fbx(&parsed, &plain).expect("基準姿勢なしは変換できる");
    let with_empty = VrmOptions::new()
        .with_expressions(profile)
        .with_baseline_weights(BaselineWeights::default());
    let (empty_model, empty_report) =
        VrmModel::from_fbx(&parsed, &with_empty).expect("空の基準姿勢は変換できる");
    assert_eq!(
        empty_model.meshes[0].vertices.positions,
        plain_model.meshes[0].vertices.positions
    );
    assert_eq!(
        empty_model.meshes[0].targets[0].positions,
        plain_model.meshes[0].targets[0].positions
    );
    assert_eq!(empty_report.baseline_meshes, 0);
    assert_eq!(empty_report.baseline_shapes, 0);
    assert_eq!(empty_report, plain_report);
}

#[test]
fn 基準姿勢100は束ねを落として警告しターゲットをゼロにする() {
    // ベースにはフルポーズが入り、ターゲットはゼロ差分になる。
    // 束ねはVRMランタイムが戻す分と二重になるため落として警告する。
    let bytes = 表情最小ファイル(
        &[("t1", "t1-shape", &[0], &[0.5, 0.0, 0.0], Some(0.0), None)],
        vec![],
        vec![],
        &[],
    );
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("表情付きは読める");
    let profile = VrmExpressionProfile {
        bindings: vec![束ね("aa", "t1")],
    };
    let options = VrmOptions::new()
        .with_expressions(profile)
        .with_baseline_weights(基準姿勢(10, 2000, 100.0));
    let (model, report) = VrmModel::from_fbx(&parsed, &options).expect("100でも変換できる");
    assert_eq!(model.meshes[0].vertices.positions[0], [1.5, 2.0, 3.0]);
    // 全束ねが落ちたら式なしであり、選択も無いためターゲット自体を作らない。
    assert!(model.meshes[0].targets.is_empty());
    // 束ねは落ち、警告1件と件数が一致する。
    assert!(model.expressions.preset.is_empty());
    assert_eq!(report.expression_count, 0);
    assert_eq!(report.baseline_meshes, 1);
    assert_eq!(report.baseline_shapes, 1);
    assert_eq!(report.warnings.len(), 1);
    assert!(
        report.warnings[0].contains("t1")
            && report.warnings[0].contains("基準姿勢で既に最大のため"),
        "束ね落としの理由が分かる: {}",
        report.warnings[0]
    );
}

#[test]
fn 基準姿勢100の束ねだけを落としても索引は維持する() {
    // t1へ100・t2へ0を焼き、t1の束ねだけを落とす。残ったt2の索引は従来どおり1である。
    // 落としたt1のターゲットは残ってゼロ差分になる。
    let bytes = 表情最小ファイル(
        &[
            ("t1", "t1-shape", &[0], &[0.5, 0.0, 0.0], Some(0.0), None),
            (
                "t2",
                "t2-shape",
                &[1],
                &[0.0, 0.5, 0.0],
                Some(0.0),
                Some(&[100.0]),
            ),
        ],
        vec![],
        vec![],
        &[],
    );
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("表情付きは読める");
    let profile = VrmExpressionProfile {
        bindings: vec![束ね("aa", "t1"), 束ね("blink", "t2")],
    };
    // t2の重みは無し（ゼロ）のため、ベースもターゲットも変わらない。
    let weights = 基準姿勢(10, 2000, 100.0);
    let options = VrmOptions::new()
        .with_expressions(profile)
        .with_baseline_weights(weights);
    let (model, report) = VrmModel::from_fbx(&parsed, &options).expect("部分落としは変換できる");
    // ベースにはt1のフルが入り、t2は変わらない。
    assert_eq!(model.meshes[0].vertices.positions[0], [1.5, 2.0, 3.0]);
    assert_eq!(model.meshes[0].vertices.positions[1], [2.0, 2.0, 3.0]);
    // チャンネル列は維持し、t1のターゲットはゼロ差分になる。
    assert_eq!(model.meshes[0].targets.len(), 2);
    assert!(
        model.meshes[0].targets[0]
            .positions
            .iter()
            .all(|delta| *delta == [0.0, 0.0, 0.0]),
        "100のターゲットはゼロ差分になる"
    );
    assert_eq!(model.meshes[0].targets[1].positions[1], [0.0, 0.5, 0.0]);
    // 束ねはblinkだけ残り、索引は解決順のまま1を指す。
    assert_eq!(model.expressions.preset.len(), 1);
    assert_eq!(model.expressions.preset[0].name, "blink");
    assert_eq!(model.expressions.preset[0].morph_target_binds[0].target, 1);
    assert_eq!(report.expression_count, 1);
    assert_eq!(report.warnings.len(), 1);
    assert!(
        report.warnings[0].contains("t1")
            && report.warnings[0].contains("基準姿勢で既に最大のため"),
        "束ね落としの理由が分かる: {}",
        report.warnings[0]
    );
}

#[test]
fn 複数チャンネルは累積し刈り込み落ちもベースへ寄与する() {
    // t1は束ね参照、otherは束ねなし・vrc/MMD名でもないため刈り込まれる。
    // ベースには両方の非ゼロ分を累積し、ターゲットはt1だけが半分になる。
    let bytes = 表情最小ファイル(
        &[
            ("t1", "t1-shape", &[0], &[0.5, 0.0, 0.0], Some(0.0), None),
            (
                "other",
                "other-shape",
                &[1],
                &[0.0, 0.5, 0.0],
                Some(0.0),
                None,
            ),
        ],
        vec![],
        vec![],
        &[],
    );
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("表情付きは読める");
    let profile = VrmExpressionProfile {
        bindings: vec![束ね("aa", "t1")],
    };
    let mut weights = 基準姿勢(10, 2000, 50.0);
    weights
        .meshes
        .get_mut(&ObjectId::new(10))
        .expect("t1の重みがある")
        .push(BaselineChannel {
            channel: ObjectId::new(2001),
            weight: 100.0,
        });
    let options = VrmOptions::new()
        .with_expressions(profile)
        .with_baseline_weights(weights);
    let (model, report) = VrmModel::from_fbx(&parsed, &options).expect("複数でも変換できる");
    // t1は半分、otherはフルでベースへ累積する。
    assert_eq!(model.meshes[0].vertices.positions[0], [1.25, 2.0, 3.0]);
    assert_eq!(model.meshes[0].vertices.positions[1], [2.0, 2.5, 3.0]);
    // 刈り込まれたotherのターゲットは無く、t1だけが残って半分になる。
    assert_eq!(model.meshes[0].targets.len(), 1);
    assert_eq!(model.meshes[0].targets[0].name, "t1");
    assert_eq!(model.meshes[0].targets[0].positions[0], [0.25, 0.0, 0.0]);
    // 束ねはt1だけ残り、焼き込みは2件数える。
    assert_eq!(model.expressions.preset.len(), 1);
    assert_eq!(report.baseline_meshes, 1);
    assert_eq!(report.baseline_shapes, 2);
}

#[test]
fn 継ぎ目の複製すべてに同じ基準姿勢が焼き込まれる() {
    // 角0はUV違いで2頂点になる。四角形のチャンネルIDは30である。
    let bytes = 表情四角形ファイル();
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("四角形は読める");
    let profile = VrmExpressionProfile {
        bindings: vec![束ね("aa", "t1")],
    };
    let options = VrmOptions::new()
        .with_expressions(profile)
        .with_baseline_weights(基準姿勢(10, 30, 50.0));
    let (model, _) = VrmModel::from_fbx(&parsed, &options).expect("焼き込み四角形は変換できる");
    let mesh = &model.meshes[0];
    assert_eq!(mesh.vertices.positions.len(), 4);
    let mut seam_vertices = 0;
    for (position, delta) in mesh
        .vertices
        .positions
        .iter()
        .zip(&mesh.targets[0].positions)
    {
        if position == &[1.25, 2.0, 3.0] {
            seam_vertices += 1;
            assert_eq!(
                *delta,
                [0.25, 0.0, 0.0],
                "継ぎ目の複製にも同じ焼き込みが要る"
            );
        }
    }
    assert_eq!(seam_vertices, 2, "角0の複製2頂点に焼き込む");
}

/// 所有モデルのバインド行列を指定できる表情最小ファイル。
/// `表情最小ファイル` と同じ組み立てであり、所有者（ID2）の行列だけ変える。
/// 同IDのバインドを2件にすると読み取りで失敗するため、置き換えで渡す。
fn 表情可変バインドファイル(owner_bind: [f64; 16]) -> Vec<u8> {
    let mut objs = vec![
        model(1, b"Root\x00\x01Model", "Null"),
        model(2, b"Owner\x00\x01Model", "Mesh"),
        triangle_geometry(10, b"Tri\x00\x01Geometry", vec![]),
    ];
    let mut conns = vec![conn(2, 1), conn(10, 2), conn(1, 0)];
    objs.push(束ねノード(3000, b"Blend\x00\x01Deformer"));
    let mut channel_full = b"t1".to_vec();
    channel_full.extend_from_slice(b"\x00\x01SubDeformer");
    let mut shape_full = b"t1-shape".to_vec();
    shape_full.extend_from_slice(b"\x00\x01Shape");
    objs.push(形状(1000, &shape_full, &[0], &[0.5, 0.0, 0.0]));
    objs.push(伝達子(2000, &channel_full, Some(0.0), None));
    conns.push(conn(1000, 2000));
    conns.push(conn(2000, 3000));
    conns.push(conn(3000, 10));
    objs.push(bind_pose(
        100,
        &[(1, bind_matrix(0.0, 0.0, 0.0)), (2, owner_bind)],
    ));
    assemble(None, objs, conns)
}

#[test]
fn 非一様拡大でも線形変換どおりに焼き込まれる() {
    // X軸2倍のバインドでは差分 [0.5,0,0] が [1.0,0,0] になり、半分で [0.5,0,0] 足す。
    let scaled = [
        2.0, 0.0, 0.0, 0.0, //
        0.0, 1.0, 0.0, 0.0, //
        0.0, 0.0, 1.0, 0.0, //
        1.0, 2.0, 3.0, 1.0, //
    ];
    let bytes = 表情可変バインドファイル(scaled);
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("拡大付きは読める");
    let profile = VrmExpressionProfile {
        bindings: vec![束ね("aa", "t1")],
    };
    let options = VrmOptions::new()
        .with_expressions(profile)
        .with_baseline_weights(基準姿勢(10, 2000, 50.0));
    let (model, _) = VrmModel::from_fbx(&parsed, &options).expect("拡大付きは変換できる");
    assert_eq!(model.meshes[0].vertices.positions[0], [1.5, 2.0, 3.0]);
    assert_eq!(model.meshes[0].targets[0].positions[0], [0.5, 0.0, 0.0]);
}

#[test]
fn 鏡像でも符号を反転して焼き込まれる() {
    // X軸-1倍のバインドでは巻き方向が反転し、差分 [0.5,0,0] が [-0.5,0,0] になる。
    let mirrored = [
        -1.0, 0.0, 0.0, 0.0, //
        0.0, 1.0, 0.0, 0.0, //
        0.0, 0.0, 1.0, 0.0, //
        1.0, 2.0, 3.0, 1.0, //
    ];
    let bytes = 表情可変バインドファイル(mirrored);
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("鏡像付きは読める");
    let profile = VrmExpressionProfile {
        bindings: vec![束ね("aa", "t1")],
    };
    let options = VrmOptions::new()
        .with_expressions(profile)
        .with_baseline_weights(基準姿勢(10, 2000, 50.0));
    let (model, report) = VrmModel::from_fbx(&parsed, &options).expect("鏡像付きは変換できる");
    assert_eq!(model.meshes[0].vertices.positions[0], [0.75, 2.0, 3.0]);
    assert_eq!(model.meshes[0].targets[0].positions[0], [-0.25, 0.0, 0.0]);
    assert_eq!(report.mirrored_meshes, 1);
}

#[test]
fn 範囲外の基準姿勢の重みは失敗する() {
    for weight in [150.0, -1.0, f64::NAN] {
        let bytes = 表情最小ファイル(
            &[("t1", "t1-shape", &[0], &[0.5, 0.0, 0.0], Some(0.0), None)],
            vec![],
            vec![],
            &[],
        );
        let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("表情付きは読める");
        let options = VrmOptions::new().with_baseline_weights(基準姿勢(10, 2000, weight));
        let 結果 = VrmModel::from_fbx(&parsed, &options);
        assert!(
            matches!(
                結果,
                Err(ref error) if error.kind() == VrmErrorKind::InvalidStructure
            ),
            "重み{weight}は構造破損で失敗する"
        );
    }
}

#[test]
fn 無いチャンネルの基準姿勢は失敗する() {
    let bytes = 表情最小ファイル(
        &[("t1", "t1-shape", &[0], &[0.5, 0.0, 0.0], Some(0.0), None)],
        vec![],
        vec![],
        &[],
    );
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("表情付きは読める");
    let options = VrmOptions::new().with_baseline_weights(基準姿勢(10, 9999, 50.0));
    let 結果 = VrmModel::from_fbx(&parsed, &options);
    assert!(
        matches!(
            結果,
            Err(ref error) if error.kind() == VrmErrorKind::InvalidStructure
        ),
        "メッシュに無いチャンネルは失敗する"
    );
}

#[test]
fn 単一フレームでない基準姿勢は失敗する() {
    // t1に形状を2件付ける。表情のkept検査を避けるため束ねは付けない。
    let bytes = 表情最小ファイル(
        &[("t1", "t1-shape", &[0], &[0.5, 0.0, 0.0], Some(0.0), None)],
        vec![形状(
            1100,
            b"t1-extra\x00\x01Shape",
            &[1],
            &[0.0, 0.5, 0.0],
        )],
        vec![conn(1100, 2000)],
        &[],
    );
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("2形状は読める");
    let options = VrmOptions::new().with_baseline_weights(基準姿勢(10, 2000, 50.0));
    let 結果 = VrmModel::from_fbx(&parsed, &options);
    assert!(
        matches!(
            結果,
            Err(ref error) if error.kind() == VrmErrorKind::UnsupportedBlendShape
        ),
        "複数フレームの基準姿勢は未対応で失敗する"
    );
}

#[test]
fn 重み率が全100でない基準姿勢は失敗する() {
    // FullWeights が [50] のチャンネルへ非ゼロの基準姿勢を焼くと失敗する。
    let bytes = 表情最小ファイル(
        &[(
            "t1",
            "t1-shape",
            &[0],
            &[0.5, 0.0, 0.0],
            Some(0.0),
            Some(&[50.0]),
        )],
        vec![],
        vec![],
        &[],
    );
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("重み率付きは読める");
    let options = VrmOptions::new().with_baseline_weights(基準姿勢(10, 2000, 50.0));
    let 結果 = VrmModel::from_fbx(&parsed, &options);
    assert!(
        matches!(
            結果,
            Err(ref error) if error.kind() == VrmErrorKind::UnsupportedBlendShape
        ),
        "全100でない重み率の基準姿勢は未対応で失敗する"
    );
}

/// 試験用のFBX GUID。小文字hexである。
const 基準姿勢試験GUID: &str = "9ef94258554e06b4ab03fbe9fc914c5c";

/// 直接記述の描画設定文書を作る。重み列はYAMLの数値列になる。
fn 直接基準姿勢文書(文書id: i64, 網guid: Option<&str>, 重み: &str) -> Vec<u8> {
    直接基準姿勢の網文書(文書id, mesh_file_id("Owner"), 網guid, 重み)
}

/// 直接記述の描画設定文書の部品を作る。メッシュ参照のfileIDを指定できる。
fn 直接基準姿勢の網文書(
    文書id: i64,
    網fid: i64,
    網guid: Option<&str>,
    重み: &str,
) -> Vec<u8> {
    let 網 = match 網guid {
        Some(guid) => format!("{{fileID: {網fid}, guid: {guid}, type: 3}}"),
        None => format!("{{fileID: {網fid}}}"),
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

/// 部品列から試験Prefabを読む。
fn 基準姿勢試験prefabを読む(部品ら: &[Vec<u8>]) -> SerializedFile {
    let mut 本文 = b"%YAML 1.1\n%TAG !u! tag:unity3d.com,2011:\n".to_vec();
    for 部品 in 部品ら {
        本文.extend_from_slice(部品);
    }
    SerializedFile::parse(&本文, &ParseOptions::new()).expect("試験prefabは読める")
}

/// 表情2チャンネルの試験FBXを読む。チャンネルIDは2000・2001、形状は1000・1001である。
fn 基準姿勢試験fbxを読む() -> FbxFile<'static> {
    // 返却値の借用を保つため、バイト列を漏らして静的化する。
    let bytes = 表情最小ファイル(
        &[
            ("t1", "t1-shape", &[0], &[0.5, 0.0, 0.0], Some(0.0), None),
            ("t2", "t2-shape", &[1], &[0.0, 0.5, 0.0], Some(0.0), None),
        ],
        vec![],
        vec![],
        &[],
    );
    let bytes: &'static [u8] = Box::leak(bytes.into_boxed_slice());
    FbxFile::parse(bytes, &FbxOptions::new()).expect("試験FBXは読める")
}

#[test]
fn 直接記述の重みから基準姿勢を解決して焼き込める() {
    // FBXチャンネル順をUnity索引と同一視し、t1（索引0）へ50を焼き込む。
    let fbx = 基準姿勢試験fbxを読む();
    let prefab = 基準姿勢試験prefabを読む(&[直接基準姿勢文書(
        600,
        Some(基準姿勢試験GUID),
        "50, 0",
    )]);
    let weights =
        prefab_baseline_weights(&prefab, "試験", &fbx, 基準姿勢試験GUID).expect("解決できる");
    assert_eq!(weights.meshes.len(), 1);
    let channels = &weights.meshes[&ObjectId::new(10)];
    assert_eq!(channels.len(), 1);
    assert_eq!(channels[0].channel, ObjectId::new(2000));
    assert_eq!(channels[0].weight, 50.0);
    // 解決した適用値で変換し、ベースへ半分焼き込む。
    let profile = VrmExpressionProfile {
        bindings: vec![束ね("aa", "t1")],
    };
    let options = VrmOptions::new()
        .with_expressions(profile)
        .with_baseline_weights(weights);
    let (model, report) = VrmModel::from_fbx(&fbx, &options).expect("焼き込みは変換できる");
    assert_eq!(model.meshes[0].vertices.positions[0], [1.25, 2.0, 3.0]);
    assert_eq!(model.meshes[0].targets[0].positions[0], [0.25, 0.0, 0.0]);
    assert_eq!(report.baseline_meshes, 1);
    assert_eq!(report.baseline_shapes, 1);
}

#[test]
fn 短い重み配列は足りない索引をゼロとして扱う() {
    // 要素1件の配列では索引1（t2）をゼロとみなし、t1だけ残る。
    let fbx = 基準姿勢試験fbxを読む();
    let prefab =
        基準姿勢試験prefabを読む(&[直接基準姿勢文書(600, Some(基準姿勢試験GUID), "50")]);
    let weights =
        prefab_baseline_weights(&prefab, "試験", &fbx, 基準姿勢試験GUID).expect("解決できる");
    let channels = &weights.meshes[&ObjectId::new(10)];
    assert_eq!(channels.len(), 1);
    assert_eq!(channels[0].channel, ObjectId::new(2000));
}

#[test]
fn 別資産の描画設定は基準姿勢に使わない() {
    // GUID不一致の文書は出所が検証できないため使わず、空になる。
    let fbx = 基準姿勢試験fbxを読む();
    let prefab = 基準姿勢試験prefabを読む(&[直接基準姿勢文書(
        600,
        Some("7464d0b6ac9485e4c9b53c7a0f61a372"),
        "50, 50",
    )]);
    let weights =
        prefab_baseline_weights(&prefab, "試験", &fbx, 基準姿勢試験GUID).expect("解決できる");
    assert!(weights.meshes.is_empty(), "別資産は使わない");
}

#[test]
fn 範囲外の非ゼロ重みは解決時に失敗する() {
    // チャンネル2件に対し索引2へ非ゼロを置くと失敗する。
    let fbx = 基準姿勢試験fbxを読む();
    let prefab = 基準姿勢試験prefabを読む(&[直接基準姿勢文書(
        600,
        Some(基準姿勢試験GUID),
        "0, 0, 50",
    )]);
    let 結果 = prefab_baseline_weights(&prefab, "試験", &fbx, 基準姿勢試験GUID);
    assert!(
        matches!(
            結果,
            Err(ref error) if error.kind() == VrmErrorKind::InvalidPrefab
        ),
        "範囲外の非ゼロ重みは失敗する"
    );
}

#[test]
fn 同じ重みの複数描画設定は通し違う重みは先勝ちにしない() {
    // 同じジオメトリの2文書が同じ実効値なら通り、違えば失敗する。
    let fbx = 基準姿勢試験fbxを読む();
    let 同じ = 基準姿勢試験prefabを読む(&[
        直接基準姿勢文書(600, Some(基準姿勢試験GUID), "50, 0"),
        直接基準姿勢文書(601, Some(基準姿勢試験GUID), "50"),
    ]);
    prefab_baseline_weights(&同じ, "試験", &fbx, 基準姿勢試験GUID).expect("一致は通る");
    let 違い = 基準姿勢試験prefabを読む(&[
        直接基準姿勢文書(600, Some(基準姿勢試験GUID), "50, 0"),
        直接基準姿勢文書(601, Some(基準姿勢試験GUID), "25, 0"),
    ]);
    let 結果 = prefab_baseline_weights(&違い, "試験", &fbx, 基準姿勢試験GUID);
    assert!(
        matches!(
            結果,
            Err(ref error) if error.kind() == VrmErrorKind::InvalidPrefab
        ),
        "不一致の重みは先勝ちにしない"
    );
}

#[test]
fn 改変内容の大きさと成分から重みを復元できる() {
    // stripped文書とPrefabInstanceの改変で、t1（索引0）へ25を焼き込む。
    let fbx = 基準姿勢試験fbxを読む();
    let 描画fid = skinned_mesh_renderer_file_id("Root/Owner");
    let guid = 基準姿勢試験GUID;
    let 本文 = format!(
        "%YAML 1.1\n%TAG !u! tag:unity3d.com,2011:\n\
         --- !u!1001 &100\n\
         PrefabInstance:\n\
         \x20 m_ObjectHideFlags: 0\n\
         \x20 m_Modification:\n\
         \x20\x20\x20 m_Modifications:\n\
         \x20\x20\x20 - target: {{fileID: {描画fid}, guid: {guid}, type: 3}}\n\
         \x20\x20\x20   propertyPath: m_BlendShapeWeights.Array.size\n\
         \x20\x20\x20   value: 2\n\
         \x20\x20\x20   objectReference: {{fileID: 0}}\n\
         \x20\x20\x20 - target: {{fileID: {描画fid}, guid: {guid}, type: 3}}\n\
         \x20\x20\x20   propertyPath: m_BlendShapeWeights.Array.data[0]\n\
         \x20\x20\x20   value: 25\n\
         \x20\x20\x20   objectReference: {{fileID: 0}}\n\
         \x20 m_SourcePrefab: {{fileID: 100100000, guid: {guid}, type: 3}}\n\
         --- !u!137 &500 stripped\n\
         SkinnedMeshRenderer:\n\
         \x20 m_CorrespondingSourceObject: {{fileID: {描画fid}, guid: {guid}, type: 3}}\n\
         \x20 m_PrefabInstance: {{fileID: 100}}\n\
         \x20 m_PrefabAsset: {{fileID: 0}}\n"
    );
    let prefab =
        SerializedFile::parse(本文.as_bytes(), &ParseOptions::new()).expect("改変付きは読める");
    let weights = prefab_baseline_weights(&prefab, "試験", &fbx, guid).expect("改変から解決できる");
    let channels = &weights.meshes[&ObjectId::new(10)];
    assert_eq!(channels.len(), 1);
    assert_eq!(channels[0].channel, ObjectId::new(2000));
    assert_eq!(channels[0].weight, 25.0);
    // 解決した適用値で変換し、ベースへ1/4焼き込む。
    let profile = VrmExpressionProfile {
        bindings: vec![束ね("aa", "t1")],
    };
    let options = VrmOptions::new()
        .with_expressions(profile)
        .with_baseline_weights(weights);
    let (model, _) = VrmModel::from_fbx(&fbx, &options).expect("焼き込みは変換できる");
    assert_eq!(model.meshes[0].vertices.positions[0], [1.125, 2.0, 3.0]);
    assert_eq!(model.meshes[0].targets[0].positions[0], [0.375, 0.0, 0.0]);
}

/// stripped描画設定文書の部品を作る。所属PrefabInstanceを指定できる。
fn stripped基準姿勢文書(文書id: i64, 描画fid: i64, 所属: i64, guid: &str) -> Vec<u8> {
    format!(
        "--- !u!137 &{文書id} stripped\n\
         SkinnedMeshRenderer:\n\
         \x20 m_CorrespondingSourceObject: {{fileID: {描画fid}, guid: {guid}, type: 3}}\n\
         \x20 m_PrefabInstance: {{fileID: {所属}}}\n\
         \x20 m_PrefabAsset: {{fileID: 0}}\n"
    )
    .into_bytes()
}

/// PrefabInstance文書の部品を作る。改変列は呼び出し側がYAML断片で渡す。
fn 実体基準姿勢文書(文書id: i64, guid: &str, 改変ら: &str) -> Vec<u8> {
    format!(
        "--- !u!1001 &{文書id}\n\
         PrefabInstance:\n\
         \x20 m_ObjectHideFlags: 0\n\
         \x20 m_Modification:\n\
         \x20\x20\x20 m_Modifications:\n\
         {改変ら}\
         \x20 m_SourcePrefab: {{fileID: 100100000, guid: {guid}, type: 3}}\n"
    )
    .into_bytes()
}

/// 描画設定1件への重み改変の断片を作る。大きさと成分0の2件である。
fn 重み改変(描画fid: i64, guid: &str, 値: &str) -> String {
    format!(
        "\x20\x20\x20 - target: {{fileID: {描画fid}, guid: {guid}, type: 3}}\n\
         \x20\x20\x20   propertyPath: m_BlendShapeWeights.Array.size\n\
         \x20\x20\x20   value: 2\n\
         \x20\x20\x20   objectReference: {{fileID: 0}}\n\
         \x20\x20\x20 - target: {{fileID: {描画fid}, guid: {guid}, type: 3}}\n\
         \x20\x20\x20   propertyPath: m_BlendShapeWeights.Array.data[0]\n\
         \x20\x20\x20   value: {値}\n\
         \x20\x20\x20   objectReference: {{fileID: 0}}\n"
    )
}

#[test]
fn 所属の違うstripped文書の重みは復活させない() {
    // 再現: instanceは100、stripped文書の所属は999、instance100の改変は重み25。
    // 検証済み文書が無いsourceの非ゼロ改変は、出所検証の迂回になるため失敗する。
    let fbx = 基準姿勢試験fbxを読む();
    let 描画fid = skinned_mesh_renderer_file_id("Root/Owner");
    let guid = 基準姿勢試験GUID;
    let prefab = 基準姿勢試験prefabを読む(&[
        実体基準姿勢文書(100, guid, &重み改変(描画fid, guid, "25")),
        stripped基準姿勢文書(500, 描画fid, 999, guid),
    ]);
    let 結果 = prefab_baseline_weights(&prefab, "試験", &fbx, guid);
    assert!(
        matches!(
            結果,
            Err(ref error) if error.kind() == VrmErrorKind::InvalidPrefab
        ),
        "未検証sourceの非ゼロ改変は失敗する"
    );
}

#[test]
fn 直接と改変の混在は突き合わせて成功と失敗を分ける() {
    // 同じジオメトリの直接記述と検証済み改変が一致すれば通り、違えば失敗する。
    // ゼロ対非ゼロも不一致であり失敗する。
    let fbx = 基準姿勢試験fbxを読む();
    let 描画fid = skinned_mesh_renderer_file_id("Root/Owner");
    let guid = 基準姿勢試験GUID;
    let 混在 = |直接重み: &str, 改変値: &str| {
        基準姿勢試験prefabを読む(&[
            直接基準姿勢文書(600, Some(guid), 直接重み),
            実体基準姿勢文書(100, guid, &重み改変(描画fid, guid, 改変値)),
            stripped基準姿勢文書(500, 描画fid, 100, guid),
        ])
    };
    let 一致 = prefab_baseline_weights(&混在("25, 0", "25"), "試験", &fbx, guid)
        .expect("一致する混在は通る");
    assert_eq!(一致.meshes[&ObjectId::new(10)].len(), 1);
    for 直接重み in ["0, 0", "50, 0"] {
        let 結果 = prefab_baseline_weights(&混在(直接重み, "25"), "試験", &fbx, guid);
        assert!(
            matches!(
                結果,
                Err(ref error) if error.kind() == VrmErrorKind::InvalidPrefab
            ),
            "直接{直接重み}と改変25の混在は失敗する"
        );
    }
}

#[test]
fn 壊れたnullメッシュ参照の非ゼロは解決不能で失敗する() {
    // `{fileID: 0, guid: fbx}` は対象FBXを指すため、解決不能は欠落させず失敗する。
    // ゼロ重みと別GUIDは無視して成功する。
    let fbx = 基準姿勢試験fbxを読む();
    let guid = 基準姿勢試験GUID;
    let 非ゼロ =
        基準姿勢試験prefabを読む(&[直接基準姿勢の網文書(600, 0, Some(guid), "50, 0")]);
    let 結果 = prefab_baseline_weights(&非ゼロ, "試験", &fbx, guid);
    assert!(
        matches!(
            結果,
            Err(ref error) if error.kind() == VrmErrorKind::InvalidPrefab
        ),
        "null参照の非ゼロ重みは失敗する"
    );
    let ゼロ =
        基準姿勢試験prefabを読む(&[直接基準姿勢の網文書(600, 0, Some(guid), "0, 0")]);
    let 空 = prefab_baseline_weights(&ゼロ, "試験", &fbx, guid).expect("ゼロは無視する");
    assert!(空.meshes.is_empty());
    let 別資産 = 基準姿勢試験prefabを読む(&[直接基準姿勢の網文書(
        600,
        0,
        Some("7464d0b6ac9485e4c9b53c7a0f61a372"),
        "50, 0",
    )]);
    let 空 = prefab_baseline_weights(&別資産, "試験", &fbx, guid).expect("別GUIDは無視する");
    assert!(空.meshes.is_empty());
}

#[test]
fn 文書の無いsourceの改変は矛盾が無ければ受け入れる() {
    // 実データ（ChiffonのBody_base型）: stripped文書自体が無いsourceへの
    // 非ゼロ改変は、出所の矛盾が無いため受け入れて焼き込む。
    let fbx = 基準姿勢試験fbxを読む();
    let 描画fid = skinned_mesh_renderer_file_id("Root/Owner");
    let guid = 基準姿勢試験GUID;
    let prefab = 基準姿勢試験prefabを読む(&[実体基準姿勢文書(
        100,
        guid,
        &重み改変(描画fid, guid, "25"),
    )]);
    let weights =
        prefab_baseline_weights(&prefab, "試験", &fbx, guid).expect("矛盾の無い改変は受け入れる");
    let channels = &weights.meshes[&ObjectId::new(10)];
    assert_eq!(channels.len(), 1);
    assert_eq!(channels[0].channel, ObjectId::new(2000));
    assert_eq!(channels[0].weight, 25.0);
}

/// 描画設定1件への成分のみの改変断片を作る。
fn 成分改変(描画fid: i64, guid: &str, 索引: usize, 値: &str) -> String {
    format!(
        "\x20\x20\x20 - target: {{fileID: {描画fid}, guid: {guid}, type: 3}}\n\
         \x20\x20\x20   propertyPath: m_BlendShapeWeights.Array.data[{索引}]\n\
         \x20\x20\x20   value: {値}\n\
         \x20\x20\x20   objectReference: {{fileID: 0}}\n"
    )
}

/// 描画設定1件への大きさのみの改変断片を作る。
fn 大きさ改変(描画fid: i64, guid: &str, 大きさ: usize) -> String {
    format!(
        "\x20\x20\x20 - target: {{fileID: {描画fid}, guid: {guid}, type: 3}}\n\
         \x20\x20\x20   propertyPath: m_BlendShapeWeights.Array.size\n\
         \x20\x20\x20   value: {大きさ}\n\
         \x20\x20\x20   objectReference: {{fileID: 0}}\n"
    )
}

/// 任意クラスのstripped文書の部品を作る。所属PrefabInstanceを指定できる。
fn stripped任意文書(
    文書id: i64,
    クラスid: i32,
    クラス名: &str,
    対応fid: i64,
    所属: i64,
    guid: &str,
) -> Vec<u8> {
    format!(
        "--- !u!{クラスid} &{文書id} stripped\n\
         {クラス名}:\n\
         \x20 m_CorrespondingSourceObject: {{fileID: {対応fid}, guid: {guid}, type: 3}}\n\
         \x20 m_PrefabInstance: {{fileID: {所属}}}\n\
         \x20 m_PrefabAsset: {{fileID: 0}}\n"
    )
    .into_bytes()
}

#[test]
fn 文書なしsourceのゼロ改変も比較に参加する() {
    // 直接 [25,0] に対し、文書なしsourceの [0,0] や大きさのみは不一致であり失敗する。
    // 全ゼロ同士は成功し、焼き込み0件になる。
    let fbx = 基準姿勢試験fbxを読む();
    let 描画fid = skinned_mesh_renderer_file_id("Root/Owner");
    let guid = 基準姿勢試験GUID;
    for 改変 in [
        成分改変(描画fid, guid, 0, "0"),
        大きさ改変(描画fid, guid, 2),
    ] {
        let prefab = 基準姿勢試験prefabを読む(&[
            直接基準姿勢文書(600, Some(guid), "25, 0"),
            実体基準姿勢文書(100, guid, &改変),
        ]);
        let 結果 = prefab_baseline_weights(&prefab, "試験", &fbx, guid);
        assert!(
            matches!(
                結果,
                Err(ref error) if error.kind() == VrmErrorKind::InvalidPrefab
            ),
            "ゼロ改変でも実効値が違えば失敗する"
        );
    }
    let 両ゼロ = 基準姿勢試験prefabを読む(&[
        直接基準姿勢文書(600, Some(guid), "0, 0"),
        実体基準姿勢文書(100, guid, &成分改変(描画fid, guid, 0, "0")),
    ]);
    let 空 = prefab_baseline_weights(&両ゼロ, "試験", &fbx, guid).expect("全ゼロ同士は成功する");
    assert!(空.meshes.is_empty(), "焼き込みは0件になる");
}

#[test]
fn 別クラスのstripped文書は対応元を汚染する() {
    // 既知renderer IDを指す stripped Transform（classID=4）と同sourceへの非ゼロ改変は失敗する。
    let fbx = 基準姿勢試験fbxを読む();
    let 描画fid = skinned_mesh_renderer_file_id("Root/Owner");
    let guid = 基準姿勢試験GUID;
    let prefab = 基準姿勢試験prefabを読む(&[
        stripped任意文書(700, 4, "Transform", 描画fid, 100, guid),
        実体基準姿勢文書(100, guid, &成分改変(描画fid, guid, 0, "25")),
    ]);
    let 結果 = prefab_baseline_weights(&prefab, "試験", &fbx, guid);
    assert!(
        matches!(
            結果,
            Err(ref error) if error.kind() == VrmErrorKind::InvalidPrefab
        ),
        "別クラスのstripped文書の対応元への改変は失敗する"
    );
}

#[test]
fn 無関係なstripped文書は対応元を汚染しない() {
    // 対応元がrenderer IDでないTransformはtaintedにせず、実rendererへの改変は受け入れる。
    let fbx = 基準姿勢試験fbxを読む();
    let 描画fid = skinned_mesh_renderer_file_id("Root/Owner");
    let guid = 基準姿勢試験GUID;
    let prefab = 基準姿勢試験prefabを読む(&[
        stripped任意文書(701, 4, "Transform", 12345, 100, guid),
        実体基準姿勢文書(100, guid, &成分改変(描画fid, guid, 0, "25")),
    ]);
    let weights =
        prefab_baseline_weights(&prefab, "試験", &fbx, guid).expect("無関係な文書は汚染しない");
    assert_eq!(weights.meshes[&ObjectId::new(10)].len(), 1);
}

#[test]
fn 別資産のstripped文書は対応元を汚染しない() {
    // 対応元GUIDが別資産のstripped文書は検証対象外であり、実rendererへの改変は受け入れる。
    let fbx = 基準姿勢試験fbxを読む();
    let 描画fid = skinned_mesh_renderer_file_id("Root/Owner");
    let guid = 基準姿勢試験GUID;
    let 別 = "7464d0b6ac9485e4c9b53c7a0f61a372";
    let prefab = 基準姿勢試験prefabを読む(&[
        stripped任意文書(702, 137, "SkinnedMeshRenderer", 描画fid, 100, 別),
        実体基準姿勢文書(100, guid, &成分改変(描画fid, guid, 0, "25")),
    ]);
    let weights =
        prefab_baseline_weights(&prefab, "試験", &fbx, guid).expect("別資産の文書は汚染しない");
    assert_eq!(weights.meshes[&ObjectId::new(10)].len(), 1);
}
