//! VRM中間モデル生成の契約テスト（公開 API のみを使う）。
//!
//! テスト内でバイナリFBXを組み立て、ノード・スキン・メッシュ・報告の変換と
//! 失敗条件を固定する。実fixtureを使う検証は `VRCTOM_FBX_FIXTURE` が指す
//! ファイルだけを読み、未設定なら跳ばす。

use vrctom::fbx::{FbxFile, FbxOptions, ObjectId};
use vrctom::vrm::{VrmErrorKind, VrmExpressionProfile, VrmModel, VrmOptions};

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
    objs.push(束ねノード(40, b"Blend\x00\x01Deformer"));
    for (offset, (channel_name, shape_name, indexes, deltas, deform, weights)) in
        channels.iter().enumerate()
    {
        let shape_id = 20 + offset as i64;
        let channel_id = 30 + offset as i64;
        let mut channel_full = channel_name.as_bytes().to_vec();
        channel_full.extend_from_slice(b"\x00\x01SubDeformer");
        let mut shape_full = shape_name.as_bytes().to_vec();
        shape_full.extend_from_slice(b"\x00\x01Shape");
        objs.push(形状(shape_id, &shape_full, indexes, deltas));
        objs.push(伝達子(channel_id, &channel_full, *deform, *weights));
        conns.push(conn(shape_id, channel_id));
        conns.push(conn(channel_id, 40));
        conns.push(conn(40, 10));
    }
    objs.extend(extra_objects);
    conns.extend(extra_connections);
    objs.push(bind_pose(100, &binds));
    assemble(None, objs, conns)
}

#[test]
fn 見つからない表情ターゲットは失敗する() {
    let bytes = 表情最小ファイル(
        &[("t1", "t1-shape", &[0], &[0.5, 0.0, 0.0], Some(0.0), None)],
        vec![],
        vec![],
        &[],
    );
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("表情付きは読める");
    let profile = VrmExpressionProfile {
        bindings: vec![束ね("aa", "無い名前")],
    };
    let options = VrmOptions::new().with_expressions(profile);
    assert_eq!(
        VrmModel::from_fbx(&parsed, &options).unwrap_err().kind(),
        VrmErrorKind::MissingExpressionTarget
    );
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
        vec![conn(50, 60), conn(60, 40)],
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
        vec![conn(50, 30)],
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
