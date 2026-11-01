//! 同一関節の `TransformLink` 候補の許容差の契約テスト（公開 API のみを使う）。
//!
//! テスト内でバイナリFBXを組み立て、丸め相当の差は代表値で通り、
//! 許容超過・非有限・特異は失敗すること、代表値の決め方が決定的であることを固定する。

use vrctom::fbx::{FbxFile, FbxOptions, ObjectId};
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

/// 頂上ノード列からバイナリFBXを組み立てる。
fn file(top: Vec<TNode>) -> Vec<u8> {
    let mut buf = b"Kaydara FBX Binary  \x00\x1a\x00".to_vec();
    buf.extend_from_slice(&7400u32.to_le_bytes());
    for node in &top {
        ser(node, &mut buf);
    }
    buf.extend_from_slice(&[0u8; 13]);
    buf
}

/// `Model` ノードを組み立てる。
fn model_p(id: i64, name: &[u8], subtype: &str) -> TNode {
    let mut props = Props::default();
    props.l(id);
    props.s(name);
    props.s(subtype.as_bytes());
    TNode::branch(b"Model", props, Vec::new())
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

/// `Geometry`（Mesh）ノードを組み立てる。三角形1件の最小形状である。
fn geometry(id: i64, name: &[u8]) -> TNode {
    let mut vertices = Props::default();
    vertices.d_raw(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
    let mut pvi = Props::default();
    pvi.i_raw(&[0, 1, -3]);
    TNode::branch(
        b"Geometry",
        {
            let mut props = Props::default();
            props.l(id);
            props.s(name);
            props.s(b"Mesh");
            props
        },
        vec![
            TNode::leaf(b"Vertices", vertices),
            TNode::leaf(b"PolygonVertexIndex", pvi),
        ],
    )
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

/// `UnitScaleFactor` だけを持つ `GlobalSettings` を組み立てる。
fn global_settings(unit_scale_factor: f64) -> TNode {
    let mut props = Props::default();
    props.s(b"UnitScaleFactor");
    props.s(b"double");
    props.s(b"");
    props.s(b"");
    props.d(unit_scale_factor);
    TNode::branch(
        b"GlobalSettings",
        Props::default(),
        vec![TNode::branch(
            b"Properties70",
            Props::default(),
            vec![TNode::leaf(b"P", props)],
        )],
    )
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

/// Z軸回転の行列を作る。列ベクトル規約の右手系回転である。
fn rot_z(degrees: f64) -> [f64; 16] {
    let (sin, cos) = (degrees * std::f64::consts::PI / 180.0).sin_cos();
    [
        cos, sin, 0.0, 0.0, //
        -sin, cos, 0.0, 0.0, //
        0.0, 0.0, 1.0, 0.0, //
        0.0, 0.0, 0.0, 1.0, //
    ]
}

/// 関節1件を2スキンが共有するファイルを組み立てる。
///
/// モデルはルート(1)・所有者A(2)・関節(3)・所有者B(4)であり、
/// ジオメトリ10（所有者A）と11（所有者B）にスキン20・30が付く。
/// 関節は `BindPose` に載せず、両クラスタの `TransformLink` だけが手掛かりになる。
/// `swap_order` が真の場合は `Objects` 内のスキン系の並びと関節→クラスタの
/// 接続順を入れ替え、代表値の決め方が出現順に左右されないことを確かめられるようにする。
fn two_skin_file(
    link_a: &[f64; 16],
    link_b: &[f64; 16],
    swap_order: bool,
    unit_scale_factor: Option<f64>,
) -> Vec<u8> {
    let skin_a = vec![
        deformer(20, b"Skin\x00\x01Skin", "Skin", vec![]),
        deformer(
            21,
            b"Cluster\x00\x01Cluster",
            "Cluster",
            vec![d_array_node(b"TransformLink", link_a)],
        ),
    ];
    let skin_b = vec![
        deformer(30, b"Skin\x00\x01Skin", "Skin", vec![]),
        deformer(
            31,
            b"Cluster\x00\x01Cluster",
            "Cluster",
            vec![d_array_node(b"TransformLink", link_b)],
        ),
    ];
    let mut objs = vec![
        model_p(1, b"Root\x00\x01Model", "Null"),
        model_p(2, b"OwnerA\x00\x01Model", "Mesh"),
        model_p(3, b"Joint\x00\x01Model", "LimbNode"),
        model_p(4, b"OwnerB\x00\x01Model", "Mesh"),
        geometry(10, b"GeoA\x00\x01Geometry"),
        geometry(11, b"GeoB\x00\x01Geometry"),
    ];
    if swap_order {
        objs.extend(skin_b);
        objs.extend(skin_a);
    } else {
        objs.extend(skin_a);
        objs.extend(skin_b);
    }
    objs.push(bind_pose(&[
        (1, identity_matrix()),
        (2, identity_matrix()),
        (4, identity_matrix()),
    ]));
    let mut top = Vec::new();
    if let Some(factor) = unit_scale_factor {
        top.push(global_settings(factor));
    }
    top.push(objects(objs));
    let mut conns = vec![
        conn(2, 1),
        conn(4, 1),
        conn(3, 1),
        conn(1, 0),
        conn(10, 2),
        conn(11, 4),
        conn(20, 10),
        conn(30, 11),
        conn(21, 20),
        conn(31, 30),
    ];
    if swap_order {
        conns.push(conn(3, 31));
        conns.push(conn(3, 21));
    } else {
        conns.push(conn(3, 21));
        conns.push(conn(3, 31));
    }
    top.push(connections(conns));
    file(top)
}

/// 単一スキンの最小ファイルを組み立てる。非有限・特異の検査に使う。
fn single_skin_file(link: &[f64; 16]) -> Vec<u8> {
    let objs = vec![
        model_p(1, b"Root\x00\x01Model", "Null"),
        model_p(2, b"Owner\x00\x01Model", "Mesh"),
        model_p(3, b"Joint\x00\x01Model", "LimbNode"),
        geometry(10, b"G\x00\x01Geometry"),
        deformer(20, b"Skin\x00\x01Skin", "Skin", vec![]),
        deformer(
            21,
            b"Cluster\x00\x01Cluster",
            "Cluster",
            vec![d_array_node(b"TransformLink", link)],
        ),
        bind_pose(&[(1, identity_matrix()), (2, identity_matrix())]),
    ];
    file(vec![
        objects(objs),
        connections(vec![
            conn(2, 1),
            conn(10, 2),
            conn(20, 10),
            conn(21, 20),
            conn(3, 21),
            conn(3, 1),
            conn(1, 0),
        ]),
    ])
}

/// モデル列から関節（ID 3）のノード索引を求める。
fn joint_index(parsed: &FbxFile, bytes_id: i64) -> usize {
    parsed
        .models()
        .iter()
        .position(|model| model.id == ObjectId::new(bytes_id))
        .expect("関節モデルがある")
}

#[test]
fn 許容内の差は代表値で通り決定的である() {
    // 平行移動差 1e-5（FBX単位）はメートル換算 1e-7 m であり許容（1μm）内、
    // 線形部差 1e-6 も許容（1e-5）内である。代表値はスキンID昇順の最初であり、
    // スキン20の候補がそのまま関節のバインド行列になる。
    let mut link_b = translation_matrix(100.0, 200.0, 300.0);
    link_b[12] += 1e-5;
    link_b[0] += 1e-6;
    let link_a = translation_matrix(100.0, 200.0, 300.0);
    let bytes = two_skin_file(&link_a, &link_b, false, None);
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("読める");
    let (model, report) =
        VrmModel::from_fbx(&parsed, &VrmOptions::new()).expect("許容内の差は通る");
    assert_eq!(report.bind_pose_models, 3);
    assert_eq!(report.bind_cluster_models, 1);
    assert_eq!(report.bind_rest_models, 0);
    assert_eq!(model.nodes[joint_index(&parsed, 3)].local, link_a);
    // 同じ入力は同じ結果になる。`HashMap` の列挙順に左右されない。
    let reparsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("読める");
    let (again, _) = VrmModel::from_fbx(&reparsed, &VrmOptions::new()).expect("再変換できる");
    let locals: Vec<[f64; 16]> = model.nodes.iter().map(|node| node.local).collect();
    let again_locals: Vec<[f64; 16]> = again.nodes.iter().map(|node| node.local).collect();
    assert_eq!(locals, again_locals);
    assert_eq!(
        model.skins[0].inverse_bind_matrices,
        again.skins[0].inverse_bind_matrices
    );
    assert_eq!(
        model.skins[1].inverse_bind_matrices,
        again.skins[1].inverse_bind_matrices
    );
}

#[test]
fn 順序を入れ替えても代表値は変わらない() {
    // `Objects` と `Connections` の並びを逆にしても、代表値はスキンID昇順の
    // 最初（スキン20の候補）であり、関節のバインド行列は変わらない。
    let mut link_b = translation_matrix(100.0, 200.0, 300.0);
    link_b[12] += 1e-5;
    link_b[0] += 1e-6;
    let link_a = translation_matrix(100.0, 200.0, 300.0);
    let normal = two_skin_file(&link_a, &link_b, false, None);
    let swapped = two_skin_file(&link_a, &link_b, true, None);
    let parsed = FbxFile::parse(&normal, &FbxOptions::new()).expect("読める");
    let (model, _) = VrmModel::from_fbx(&parsed, &VrmOptions::new()).expect("通る");
    let swapped_parsed = FbxFile::parse(&swapped, &FbxOptions::new()).expect("読める");
    let (swapped_model, _) =
        VrmModel::from_fbx(&swapped_parsed, &VrmOptions::new()).expect("入れ替えても通る");
    assert_eq!(model.nodes[joint_index(&parsed, 3)].local, link_a);
    assert_eq!(
        swapped_model.nodes[joint_index(&swapped_parsed, 3)].local,
        link_a
    );
}

#[test]
fn 許容外の平行移動は失敗し関節と差を報告する() {
    // 0.1（FBX単位）はメートル換算 1e-3 m であり許容（1μm）を超える。
    let link_a = translation_matrix(100.0, 200.0, 300.0);
    let mut link_b = link_a;
    link_b[12] += 0.1;
    let bytes = two_skin_file(&link_a, &link_b, false, None);
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("読める");
    let error = VrmModel::from_fbx(&parsed, &VrmOptions::new()).expect_err("許容外の差は失敗する");
    assert_eq!(error.kind(), vrctom::vrm::VrmErrorKind::InvalidStructure);
    let message = error.to_string();
    assert!(
        message.contains("結合モデルのバインド時行列が一致しない"),
        "原因が分かる: {message}"
    );
    assert!(message.contains("関節 3"), "関節が分かる: {message}");
    assert!(message.contains("成分[12]"), "成分が分かる: {message}");
}

#[test]
fn 許容外の回転は失敗する() {
    // 1度の回転は成分差 約 1.7e-2 であり線形部の許容（1e-5）を超える。
    let link_a = identity_matrix();
    let link_b = rot_z(1.0);
    let bytes = two_skin_file(&link_a, &link_b, false, None);
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("読める");
    let error =
        VrmModel::from_fbx(&parsed, &VrmOptions::new()).expect_err("許容外の回転は失敗する");
    assert_eq!(error.kind(), vrctom::vrm::VrmErrorKind::InvalidStructure);
    let message = error.to_string();
    assert!(
        message.contains("結合モデルのバインド時行列が一致しない"),
        "原因が分かる: {message}"
    );
    assert!(message.contains("関節 3"), "関節が分かる: {message}");
    assert!(message.contains("成分[0]"), "成分が分かる: {message}");
}

#[test]
fn 平行移動の許容は単位換算で評価する() {
    // 同じFBX単位の差 5e-5 でも、`UnitScaleFactor` が1（1単位=0.01m）なら
    // 5e-7 m で許容内、100（1単位=1m）なら 5e-5 m で許容外になる。
    let link_a = translation_matrix(100.0, 200.0, 300.0);
    let mut link_b = link_a;
    link_b[12] += 5e-5;
    let bytes = two_skin_file(&link_a, &link_b, false, None);
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("読める");
    VrmModel::from_fbx(&parsed, &VrmOptions::new()).expect("cm換算では許容内である");
    let bytes = two_skin_file(&link_a, &link_b, false, Some(100.0));
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("読める");
    let error = VrmModel::from_fbx(&parsed, &VrmOptions::new()).expect_err("m換算では許容外になる");
    assert_eq!(error.kind(), vrctom::vrm::VrmErrorKind::InvalidStructure);
}

#[test]
fn 非有限の候補は失敗する() {
    // 非有限の `TransformLink` は丸めではなく破損であり、単一スキンでも失敗する。
    let mut link = identity_matrix();
    link[12] = f64::NAN;
    let bytes = single_skin_file(&link);
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("読める");
    let error = VrmModel::from_fbx(&parsed, &VrmOptions::new()).expect_err("非有限は失敗する");
    assert_eq!(error.kind(), vrctom::vrm::VrmErrorKind::InvalidStructure);
}

#[test]
fn 特異な候補は失敗する() {
    // 退化した `TransformLink` は逆行列を持たないため、単一スキンでも失敗する。
    let mut singular = identity_matrix();
    singular[0] = 0.0;
    singular[4] = 0.0;
    singular[8] = 0.0;
    let bytes = single_skin_file(&singular);
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("読める");
    let error = VrmModel::from_fbx(&parsed, &VrmOptions::new()).expect_err("特異は失敗する");
    assert_eq!(error.kind(), vrctom::vrm::VrmErrorKind::InvalidStructure);
}
