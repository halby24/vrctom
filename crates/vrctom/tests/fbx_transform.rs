//! FBXのローカル変換合成とバインド行列の代替の契約テスト（公開 API のみを使う）。
//!
//! テスト内でバイナリFBXを組み立て、手計算の合成結果・代替経路・失敗条件を固定する。
//! 実fixtureを使う検証は `VRCTOM_FBX_FIXTURE` が指すファイルだけを読み、未設定なら跳ばす。

use vrctom::fbx::{
    FbxErrorKind, FbxFile, FbxOptions, LocalTransform, ObjectId, inherit_world_matrix,
};
use vrctom::vrm::{VrmModel, VrmOptions};

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

    /// `f64` 列をrawの `d` 配列として書く。
    fn d_raw(&mut self, values: &[f64]) {
        let mut payload = Vec::with_capacity(values.len() * 8);
        for v in values {
            payload.extend_from_slice(&v.to_le_bytes());
        }
        let mut body = (values.len() as u32).to_le_bytes().to_vec();
        body.extend_from_slice(&0u32.to_le_bytes());
        body.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        body.extend_from_slice(&payload);
        self.prop(b'd', &body);
    }

    /// `i32` 列をrawの `i` 配列として書く。
    fn i_raw(&mut self, values: &[i32]) {
        let mut payload = Vec::with_capacity(values.len() * 4);
        for v in values {
            payload.extend_from_slice(&v.to_le_bytes());
        }
        let mut body = (values.len() as u32).to_le_bytes().to_vec();
        body.extend_from_slice(&0u32.to_le_bytes());
        body.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        body.extend_from_slice(&payload);
        self.prop(b'i', &body);
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

/// 論理ノードを絶対オフセット付きで書き出す。7400版の32bit幅である。
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

/// 直下ノード列からバイナリFBXを組み立てる。
fn file(top: Vec<TNode>) -> Vec<u8> {
    let mut buf = b"Kaydara FBX Binary  \x00\x1a\x00".to_vec();
    buf.extend_from_slice(&7400u32.to_le_bytes());
    for node in &top {
        ser(node, &mut buf);
    }
    buf.extend_from_slice(&[0u8; 13]);
    buf
}

/// `Model` ノードを組み立てる。変換成分は呼び出し側が渡す。
fn model_p(id: i64, name: &[u8], subtype: &str, ps: Vec<TNode>) -> TNode {
    let mut props = Props::default();
    props.l(id);
    props.s(name);
    props.s(subtype.as_bytes());
    let children = if ps.is_empty() {
        Vec::new()
    } else {
        vec![TNode::branch(b"Properties70", Props::default(), ps)]
    };
    TNode::branch(b"Model", props, children)
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

/// 3数値の `P` を組み立てる。
fn p_triplet(name: &str, v: [f64; 3]) -> TNode {
    let mut values = Props::default();
    values.d(v[0]);
    values.d(v[1]);
    values.d(v[2]);
    p_node(name, name, values)
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

/// `f64` 配列1件の子ノードを組み立てる。
fn d_array_node(name: &[u8], values: &[f64]) -> TNode {
    let mut props = Props::default();
    props.d_raw(values);
    TNode::leaf(name, props)
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

/// モデル列に対する `BindPose` を組み立てる。
fn bind_pose(nodes: &[(i64, [f64; 16])]) -> TNode {
    let mut props = Props::default();
    props.l(100);
    props.s(b"Bind\x00\x01Pose");
    props.s(b"BindPose");
    TNode::branch(
        b"Pose",
        props,
        nodes
            .iter()
            .map(|(model, matrix)| pose_node(*model, matrix))
            .collect(),
    )
}

/// 頂上ノード列からファイルを組み立てる。
fn assemble(objs: Vec<TNode>, conns: Vec<TNode>) -> Vec<u8> {
    file(vec![objects(objs), connections(conns)])
}

/// 平行移動だけが異なる行列を作る。
fn translation_matrix(x: f64, y: f64, z: f64) -> [f64; 16] {
    [
        1.0, 0.0, 0.0, 0.0, //
        0.0, 1.0, 0.0, 0.0, //
        0.0, 0.0, 1.0, 0.0, //
        x, y, z, 1.0, //
    ]
}

/// 単位行列を作る。
fn identity_matrix() -> [f64; 16] {
    [
        1.0, 0.0, 0.0, 0.0, //
        0.0, 1.0, 0.0, 0.0, //
        0.0, 0.0, 1.0, 0.0, //
        0.0, 0.0, 0.0, 1.0, //
    ]
}

/// 行列の各成分が期待値の許容誤差に収まることを確かめる。
fn assert_matrix_near(actual: &[f64; 16], expected: &[f64; 16], tolerance: f64) {
    for (index, (a, e)) in actual.iter().zip(expected.iter()).enumerate() {
        assert!(
            (a - e).abs() <= tolerance,
            "行列が手計算と一致する [{index}]: {a} != {e}"
        );
    }
}

/// Z軸90度回転の行列を作る。FBXのXYZ順では `Rz * Ry * Rx` であり、
/// XとYが0の場合は `Rz` だけが残る。
fn rot_z90() -> [f64; 16] {
    [
        0.0, 1.0, 0.0, 0.0, //
        -1.0, 0.0, 0.0, 0.0, //
        0.0, 0.0, 1.0, 0.0, //
        0.0, 0.0, 0.0, 1.0, //
    ]
}

/// 変換成分だけを持つモデル1件のファイルを組み立てる。
fn single_model_file(ps: Vec<TNode>) -> Vec<u8> {
    assemble(
        vec![model_p(1, b"M\x00\x01Model", "LimbNode", ps)],
        vec![conn(1, 0)],
    )
}

/// モデル1件のローカル行列だけを取り出す。
fn local_of(ps: Vec<TNode>) -> [f64; 16] {
    let bytes = single_model_file(ps);
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("読める");
    parsed
        .model(ObjectId::new(1))
        .expect("節点がある")
        .transform
        .local_matrix()
        .expect("合成できる")
}

#[test]
fn 既定の成分では従来のtrsと一致する() {
    // 補正成分が無い場合は `T * R * S` と1ビットも変わらない。
    // `Rz(90)` の列は `(0,1,0)`・`(-1,0,0)`・`(0,0,1)` である。
    let local = local_of(vec![
        p_triplet("Lcl Translation", [1.0, 2.0, 3.0]),
        p_triplet("Lcl Rotation", [0.0, 0.0, 90.0]),
        p_triplet("Lcl Scaling", [2.0, 1.0, 1.0]),
    ]);
    assert_matrix_near(
        &local,
        &[
            0.0, 2.0, 0.0, 0.0, //
            -1.0, 0.0, 0.0, 0.0, //
            0.0, 0.0, 1.0, 0.0, //
            1.0, 2.0, 3.0, 1.0, //
        ],
        1e-12,
    );
}

#[test]
fn 事前回転だけの合成は手計算と一致する() {
    // `PreRotation` だけの場合はその回転がそのまま残る。
    let local = local_of(vec![p_triplet("PreRotation", [0.0, 0.0, 90.0])]);
    assert_matrix_near(&local, &rot_z90(), 1e-12);
}

#[test]
fn 回転ピボットありの合成は手計算と一致する() {
    // `Rp * R * Rp^-1` であり、平行移動は `p - R * p` になる。
    // `Rz(90) * (1,2,3) = (-2,1,3)` のため、平行移動は `(3,1,0)` である。
    let local = local_of(vec![
        p_triplet("RotationPivot", [1.0, 2.0, 3.0]),
        p_triplet("Lcl Rotation", [0.0, 0.0, 90.0]),
    ]);
    let mut expected = rot_z90();
    expected[12] = 3.0;
    expected[13] = 1.0;
    expected[14] = 0.0;
    assert_matrix_near(&local, &expected, 1e-12);
}

#[test]
fn 幾何変換は手計算と一致する() {
    // `G = Tgeo * Rgeo * Sgeo` であり、平行移動は幾何平行移動のまま残る。
    // 線形部は `Rz(90) * S(2,1,1)` であり、列は `(0,2,0)`・`(-1,0,0)`・`(0,0,1)` である。
    let bytes = single_model_file(vec![
        p_triplet("GeometricTranslation", [4.0, 5.0, 6.0]),
        p_triplet("GeometricRotation", [0.0, 0.0, 90.0]),
        p_triplet("GeometricScaling", [2.0, 1.0, 1.0]),
    ]);
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("読める");
    let geometric = parsed
        .model(ObjectId::new(1))
        .expect("節点がある")
        .transform
        .geometric_matrix()
        .expect("合成できる");
    assert_matrix_near(
        &geometric,
        &[
            0.0, 2.0, 0.0, 0.0, //
            -1.0, 0.0, 0.0, 0.0, //
            0.0, 0.0, 1.0, 0.0, //
            4.0, 5.0, 6.0, 1.0, //
        ],
        1e-12,
    );
    // 幾何成分が既定値の場合は単位行列になる。
    let plain = local_of(vec![p_triplet("Lcl Translation", [1.0, 0.0, 0.0])]);
    assert_matrix_near(
        &plain,
        &[
            1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 1.0, 0.0, 0.0, 1.0,
        ],
        0.0,
    );
}

#[test]
fn 継承方式ごとの合成は手計算と一致する() {
    // 親は `S(2,1,1)`、子は `T(1,0,0) * Rz(90)` である。
    let parent = [
        2.0, 0.0, 0.0, 0.0, //
        0.0, 1.0, 0.0, 0.0, //
        0.0, 0.0, 1.0, 0.0, //
        0.0, 0.0, 0.0, 1.0, //
    ];
    let local = [
        0.0, 1.0, 0.0, 0.0, //
        -1.0, 0.0, 0.0, 0.0, //
        0.0, 0.0, 1.0, 0.0, //
        1.0, 0.0, 0.0, 1.0, //
    ];
    // RrSs(0)は親の行列をそのまま継承する。線形部は `P * R`、
    // 平行移動は `P * (1,0,0) = (2,0,0)` になる。
    let world = inherit_world_matrix(&parent, &local, 0).expect("合成できる");
    assert_matrix_near(
        &world,
        &[
            0.0, 1.0, 0.0, 0.0, //
            -2.0, 0.0, 0.0, 0.0, //
            0.0, 0.0, 1.0, 0.0, //
            2.0, 0.0, 0.0, 1.0, //
        ],
        1e-12,
    );
    // RSrs(1)は子の回転が親の拡大縮小の左に来て、平行移動は `(0,2,0)` になる。
    let world = inherit_world_matrix(&parent, &local, 1).expect("合成できる");
    assert_matrix_near(
        &world,
        &[
            0.0, 2.0, 0.0, 0.0, //
            -1.0, 0.0, 0.0, 0.0, //
            0.0, 0.0, 1.0, 0.0, //
            0.0, 2.0, 0.0, 1.0, //
        ],
        1e-12,
    );
    // Rrs(2)は親の拡大縮小を無視し、子の行列がそのまま残る。
    let world = inherit_world_matrix(&parent, &local, 2).expect("合成できる");
    assert_matrix_near(&world, &local, 1e-12);
    // 範囲外の継承方式は非対応として失敗する。
    assert_eq!(
        inherit_world_matrix(&parent, &local, 3).unwrap_err().kind(),
        FbxErrorKind::UnsupportedFeature
    );
    // 非有限の入力は構造破損として失敗する。
    let mut bad = local;
    bad[0] = f64::NAN;
    assert_eq!(
        inherit_world_matrix(&parent, &bad, 0).unwrap_err().kind(),
        FbxErrorKind::InvalidStructure
    );
}

#[test]
fn 非有限値の変換は失敗する() {
    // 読み取り時に非有限値がある場合は構造破損として失敗する。
    let mut nan_values = Props::default();
    nan_values.d(f64::NAN);
    nan_values.d(0.0);
    nan_values.d(0.0);
    let bytes = single_model_file(vec![p_node(
        "Lcl Translation",
        "Lcl Translation",
        nan_values,
    )]);
    assert_eq!(
        FbxFile::parse(&bytes, &FbxOptions::new())
            .unwrap_err()
            .kind(),
        FbxErrorKind::InvalidStructure
    );
    // 公開構造体に直接載せた非有限値も合成時に失敗する。
    let mut transform = LocalTransform {
        translation: [f64::NAN, 0.0, 0.0],
        rotation: [0.0, 0.0, 0.0],
        scale: [1.0, 1.0, 1.0],
        rotation_offset: [0.0, 0.0, 0.0],
        rotation_pivot: [0.0, 0.0, 0.0],
        pre_rotation: [0.0, 0.0, 0.0],
        post_rotation: [0.0, 0.0, 0.0],
        scaling_offset: [0.0, 0.0, 0.0],
        scaling_pivot: [0.0, 0.0, 0.0],
        geometric_translation: [0.0, 0.0, 0.0],
        geometric_rotation: [0.0, 0.0, 0.0],
        geometric_scaling: [1.0, 1.0, 1.0],
        rotation_active: true,
    };
    assert_eq!(
        transform.local_matrix().unwrap_err().kind(),
        FbxErrorKind::InvalidStructure
    );
    transform.translation = [0.0, 0.0, 0.0];
    transform.geometric_scaling = [f64::INFINITY, 1.0, 1.0];
    assert_eq!(
        transform.geometric_matrix().unwrap_err().kind(),
        FbxErrorKind::InvalidStructure
    );
}

#[test]
fn クラスタのバインド時行列を読める() {
    // `Transform` と `TransformLink` を持つクラスタは両方読める。
    let link = translation_matrix(5.0, 6.0, 7.0);
    let mesh_world = translation_matrix(1.0, 2.0, 3.0);
    let bytes = assemble(
        vec![
            model_p(1, b"Root\x00\x01Model", "Null", vec![]),
            model_p(2, b"Owner\x00\x01Model", "Mesh", vec![]),
            model_p(3, b"Joint\x00\x01Model", "LimbNode", vec![]),
            geometry(
                10,
                b"G\x00\x01Geometry",
                vec![
                    vertices_node(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]),
                    pvi_node(&[0, 1, -3]),
                ],
            ),
            deformer(20, b"Skin\x00\x01Skin", "Skin", vec![]),
            deformer(
                21,
                b"Cluster\x00\x01Cluster",
                "Cluster",
                vec![
                    d_array_node(b"Transform", &mesh_world),
                    d_array_node(b"TransformLink", &link),
                ],
            ),
        ],
        vec![
            conn(2, 1),
            conn(10, 2),
            conn(20, 10),
            conn(21, 20),
            conn(3, 21),
            conn(3, 1),
            conn(1, 0),
        ],
    );
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("読める");
    let binds = parsed
        .cluster_bind_matrices(ObjectId::new(20))
        .expect("展開できる");
    assert_eq!(binds.len(), 1);
    assert_eq!(binds[0].joint, ObjectId::new(3));
    assert_eq!(binds[0].transform, Some(mesh_world));
    assert_eq!(binds[0].transform_link, Some(link));
    // 配列を持たないクラスタは `None` になる。
    let bytes = assemble(
        vec![
            model_p(1, b"Root\x00\x01Model", "Null", vec![]),
            model_p(2, b"Owner\x00\x01Model", "Mesh", vec![]),
            model_p(3, b"Joint\x00\x01Model", "LimbNode", vec![]),
            geometry(
                10,
                b"G\x00\x01Geometry",
                vec![
                    vertices_node(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]),
                    pvi_node(&[0, 1, -3]),
                ],
            ),
            deformer(20, b"Skin\x00\x01Skin", "Skin", vec![]),
            deformer(21, b"Cluster\x00\x01Cluster", "Cluster", vec![]),
        ],
        vec![
            conn(2, 1),
            conn(10, 2),
            conn(20, 10),
            conn(21, 20),
            conn(3, 21),
            conn(3, 1),
            conn(1, 0),
        ],
    );
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("読める");
    let binds = parsed
        .cluster_bind_matrices(ObjectId::new(20))
        .expect("展開できる");
    assert_eq!(binds[0].transform, None);
    assert_eq!(binds[0].transform_link, None);
    // 要素数が16でない行列は構造破損として失敗する。
    let bytes = assemble(
        vec![
            model_p(1, b"Root\x00\x01Model", "Null", vec![]),
            model_p(2, b"Owner\x00\x01Model", "Mesh", vec![]),
            model_p(3, b"Joint\x00\x01Model", "LimbNode", vec![]),
            geometry(
                10,
                b"G\x00\x01Geometry",
                vec![
                    vertices_node(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]),
                    pvi_node(&[0, 1, -3]),
                ],
            ),
            deformer(20, b"Skin\x00\x01Skin", "Skin", vec![]),
            deformer(
                21,
                b"Cluster\x00\x01Cluster",
                "Cluster",
                vec![d_array_node(b"TransformLink", &[0.0; 15])],
            ),
        ],
        vec![
            conn(2, 1),
            conn(10, 2),
            conn(20, 10),
            conn(21, 20),
            conn(3, 21),
            conn(3, 1),
            conn(1, 0),
        ],
    );
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("読める");
    assert_eq!(
        parsed
            .cluster_bind_matrices(ObjectId::new(20))
            .unwrap_err()
            .kind(),
        FbxErrorKind::InvalidStructure
    );
}

/// 関節1件・所有者1件・クラスタ1件の最小スキン付きファイルを組み立てる。
fn skinned_file(cluster_children: Vec<TNode>, pose: Option<Vec<(i64, [f64; 16])>>) -> Vec<u8> {
    let mut objs = vec![
        model_p(1, b"Root\x00\x01Model", "Null", vec![]),
        model_p(2, b"Owner\x00\x01Model", "Mesh", vec![]),
        model_p(3, b"Joint\x00\x01Model", "LimbNode", vec![]),
        geometry(
            10,
            b"G\x00\x01Geometry",
            vec![
                vertices_node(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]),
                pvi_node(&[0, 1, -3]),
            ],
        ),
        deformer(20, b"Skin\x00\x01Skin", "Skin", vec![]),
        deformer(21, b"Cluster\x00\x01Cluster", "Cluster", cluster_children),
    ];
    if let Some(nodes) = pose {
        objs.push(bind_pose(&nodes));
    }
    assemble(
        objs,
        vec![
            conn(2, 1),
            conn(10, 2),
            conn(20, 10),
            conn(21, 20),
            conn(3, 21),
            conn(3, 1),
            conn(1, 0),
        ],
    )
}

#[test]
fn poseが欠けた関節はクラスタ行列で補い報告に残る() {
    // 関節のバインド行列が `Pose` に無く、クラスタにだけある場合の代替である。
    let link = translation_matrix(5.0, 6.0, 7.0);
    let bytes = skinned_file(
        vec![d_array_node(b"TransformLink", &link)],
        Some(vec![(1, identity_matrix()), (2, identity_matrix())]),
    );
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("読める");
    let (model, report) =
        VrmModel::from_fbx(&parsed, &VrmOptions::new()).expect("代替で変換できる");
    assert_eq!(report.bind_pose_models, 2);
    assert_eq!(report.bind_cluster_models, 1);
    assert_eq!(report.bind_rest_models, 0);
    // 関節ノードのローカル行列が代替の行列と一致する。
    assert_eq!(model.nodes[2].local, link);
}

#[test]
fn poseが欠けた非関節はrest合成で補う() {
    // クラスタから参照されない末端は、ローカル変換の合成で補う。
    let bytes = assemble(
        vec![
            model_p(1, b"Root\x00\x01Model", "Null", vec![]),
            model_p(2, b"Owner\x00\x01Model", "Mesh", vec![]),
            model_p(
                3,
                b"Leaf\x00\x01Model",
                "LimbNode",
                vec![p_triplet("Lcl Translation", [1.0, 0.0, 0.0])],
            ),
            geometry(
                10,
                b"G\x00\x01Geometry",
                vec![
                    vertices_node(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]),
                    pvi_node(&[0, 1, -3]),
                ],
            ),
            bind_pose(&[(1, identity_matrix()), (2, identity_matrix())]),
        ],
        vec![conn(2, 1), conn(10, 2), conn(3, 1), conn(1, 0)],
    );
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("読める");
    let (model, report) =
        VrmModel::from_fbx(&parsed, &VrmOptions::new()).expect("代替で変換できる");
    assert_eq!(report.bind_pose_models, 2);
    assert_eq!(report.bind_cluster_models, 0);
    assert_eq!(report.bind_rest_models, 1);
    assert_eq!(model.nodes[2].local, translation_matrix(1.0, 0.0, 0.0));
}

#[test]
fn bindpose自体が無い場合は従来どおり失敗する() {
    // バインド姿勢の手掛かりが全く無い場合は、restを正と決められないため失敗する。
    let bytes = skinned_file(
        vec![d_array_node(b"TransformLink", &identity_matrix())],
        None,
    );
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("読める");
    let error =
        VrmModel::from_fbx(&parsed, &VrmOptions::new()).expect_err("バインド欠落は失敗する");
    assert_eq!(error.kind(), vrctom::vrm::VrmErrorKind::MissingBindPose);
}

#[test]
fn 特異なバインド時行列の関節は失敗する() {
    // 退化した代替行列は補わず、特異として失敗する。
    let mut singular = identity_matrix();
    singular[0] = 0.0;
    singular[4] = 0.0;
    singular[8] = 0.0;
    let bytes = skinned_file(
        vec![d_array_node(b"TransformLink", &singular)],
        Some(vec![(1, identity_matrix()), (2, identity_matrix())]),
    );
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("読める");
    let error = VrmModel::from_fbx(&parsed, &VrmOptions::new()).expect_err("特異は失敗する");
    assert_eq!(error.kind(), vrctom::vrm::VrmErrorKind::InvalidStructure);
}

#[test]
fn ゼロ法線は面法線で補い報告に残る() {
    // 長さ0の法線は方向を持たないため、属する面の法線で補う。
    // 三角形はXY平面にあり、面法線は `+Z` である。
    let bytes = assemble(
        vec![
            model_p(1, b"Root\x00\x01Model", "Null", vec![]),
            model_p(2, b"Owner\x00\x01Model", "Mesh", vec![]),
            geometry(
                10,
                b"G\x00\x01Geometry",
                vec![
                    vertices_node(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]),
                    pvi_node(&[0, 1, -3]),
                    normal_layer(
                        b"ByPolygonVertex",
                        b"Direct",
                        &[0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0],
                        None,
                    ),
                ],
            ),
            bind_pose(&[(1, identity_matrix()), (2, identity_matrix())]),
        ],
        vec![conn(2, 1), conn(10, 2), conn(1, 0)],
    );
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("読める");
    let (model, report) =
        VrmModel::from_fbx(&parsed, &VrmOptions::new()).expect("退化は補って変換できる");
    assert_eq!(report.degenerate_normals, 1);
    let normals = model.meshes[0]
        .vertices
        .normals
        .as_ref()
        .expect("法線がある");
    // 原点の頂点の法線が面法線 `+Z` で補われている。
    let index = model.meshes[0]
        .vertices
        .positions
        .iter()
        .position(|p| *p == [0.0, 0.0, 0.0])
        .expect("原点がある");
    assert_eq!(normals[index], [0.0, 0.0, 1.0]);
}

#[test]
fn 幾何変換はメッシュのベイクに掛かる() {
    // 所有モデルの幾何平行移動は、頂点のベイク時に世界行列へ掛かる。
    let bytes = assemble(
        vec![
            model_p(1, b"Root\x00\x01Model", "Null", vec![]),
            model_p(
                2,
                b"Owner\x00\x01Model",
                "Mesh",
                vec![p_triplet("GeometricTranslation", [1.0, 0.0, 0.0])],
            ),
            geometry(
                10,
                b"G\x00\x01Geometry",
                vec![
                    vertices_node(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]),
                    pvi_node(&[0, 1, -3]),
                ],
            ),
            bind_pose(&[(1, identity_matrix()), (2, identity_matrix())]),
        ],
        vec![conn(2, 1), conn(10, 2), conn(1, 0)],
    );
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("読める");
    let (model, _) = VrmModel::from_fbx(&parsed, &VrmOptions::new()).expect("変換できる");
    assert_eq!(
        model.meshes[0].vertices.positions,
        vec![[1.0, 0.0, 0.0], [2.0, 0.0, 0.0], [1.0, 1.0, 0.0]]
    );
}

/// `VRCTOM_FBX_FIXTURE` が指す実ファイルで、rest合成とバインド行列を突き合わせる。
///
/// 購入アバターの実ファイルでは両者が一致することが正しさの証拠になる。
/// 未設定なら跳ばす。
#[test]
fn 実fixtureのrest合成はバインド行列と一致する() {
    let Ok(path) = std::env::var("VRCTOM_FBX_FIXTURE") else {
        eprintln!("VRCTOM_FBX_FIXTURE未設定のため跳ばす");
        return;
    };
    let bytes = std::fs::read(&path).expect("実ファイルは読める");
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("実ファイルは読める");
    let pose: std::collections::HashMap<i64, [f64; 16]> = parsed
        .bind_pose_nodes()
        .expect("バインド行列は読める")
        .into_iter()
        .map(|node| (node.model.value(), node.matrix))
        .collect();
    // 親子ともバインド行列があるモデルのローカルを比べる。
    // `f32` 由来の丸めを見込み、許容誤差は0.01度にする。
    let mut worst = 0.0f64;
    let mut worst_name = String::new();
    for model in parsed.models() {
        let (Some(child), Some(parent)) = (
            pose.get(&model.id.value()),
            model.parent.and_then(|id| pose.get(&id.value())),
        ) else {
            continue;
        };
        let derived = fbx_math::mul4(&fbx_math::inverse4(parent), child);
        let rest = model.transform.local_matrix().expect("合成できる");
        let angle = fbx_math::rotation_angle(&derived, &rest);
        if angle > worst {
            worst = angle;
            worst_name = model.name.clone();
        }
    }
    eprintln!("restとバインドの最大角差: {worst}度（{worst_name}）");
    assert!(worst < 0.01, "rest合成はバインド行列と一致する: {worst}度");
}

/// 行列積を求める。テスト用の最小実装である。
mod fbx_math {
    pub fn mul4(a: &[f64; 16], b: &[f64; 16]) -> [f64; 16] {
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

    /// 逆行列を求める。特異な場合は単位行列を返す（テスト入力は正則である）。
    pub fn inverse4(m: &[f64; 16]) -> [f64; 16] {
        let mut a = [[0.0f64; 8]; 4];
        for row in 0..4 {
            for col in 0..4 {
                a[row][col] = m[col * 4 + row];
            }
            a[row][4 + row] = 1.0;
        }
        for col in 0..4 {
            let mut pivot = col;
            for row in col..4 {
                if a[row][col].abs() > a[pivot][col].abs() {
                    pivot = row;
                }
            }
            a.swap(col, pivot);
            let diag = a[col][col];
            for value in a[col].iter_mut() {
                *value /= diag;
            }
            let pivot_row = a[col];
            for (row, target) in a.iter_mut().enumerate() {
                if row != col {
                    let factor = target[col];
                    for (value, pivot) in target.iter_mut().zip(pivot_row.iter()) {
                        *value -= factor * pivot;
                    }
                }
            }
        }
        let mut out = [0.0; 16];
        for row in 0..4 {
            for col in 0..4 {
                out[col * 4 + row] = a[row][4 + col];
            }
        }
        out
    }

    /// 回転部の列を正規化して角度差（度）を求める。スケールの違いは無視する。
    pub fn rotation_angle(a: &[f64; 16], b: &[f64; 16]) -> f64 {
        fn column(m: &[f64; 16], c: usize) -> [f64; 3] {
            let length =
                (m[c * 4] * m[c * 4] + m[c * 4 + 1] * m[c * 4 + 1] + m[c * 4 + 2] * m[c * 4 + 2])
                    .sqrt();
            [
                m[c * 4] / length,
                m[c * 4 + 1] / length,
                m[c * 4 + 2] / length,
            ]
        }
        let mut trace = 0.0;
        for i in 0..3 {
            let x = column(a, i);
            let y = column(b, i);
            trace += x[0] * y[0] + x[1] * y[1] + x[2] * y[2];
        }
        ((trace - 1.0) / 2.0).clamp(-1.0, 1.0).acos().to_degrees()
    }
}
