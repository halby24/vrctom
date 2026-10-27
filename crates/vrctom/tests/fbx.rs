//! バイナリFBX読み取り層の契約テスト（公開 API のみを使う）。
//!
//! テスト内でバイナリFBXを組み立て、ヘッダ・ノード境界・属性型・各対応方式・
//! 失敗条件を固定する。実fixtureを使う検証は `VRCTOM_FBX_FIXTURE` が指す
//! ファイルだけを読み、未設定なら跳ばす。

use std::io::Write as _;

use vrctom::fbx::{Axis, FbxErrorKind, FbxFile, FbxOptions, ModelKind, ObjectId};

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

    fn y(&mut self, v: i16) {
        self.prop(b'Y', &v.to_le_bytes());
    }

    fn c(&mut self, v: bool) {
        self.prop(b'C', &[u8::from(v)]);
    }

    fn i(&mut self, v: i32) {
        self.prop(b'I', &v.to_le_bytes());
    }

    fn f(&mut self, v: f32) {
        self.prop(b'F', &v.to_le_bytes());
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

    /// 配列プロパティを書く。圧縮方式とペイロードは呼び出し側が決める。
    fn array(&mut self, code: u8, count: u32, encoding: u32, payload: &[u8]) {
        let mut body = count.to_le_bytes().to_vec();
        body.extend_from_slice(&encoding.to_le_bytes());
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
        self.array(b'd', values.len() as u32, 0, &payload);
    }

    /// `f64` 列をzlib圧縮の `d` 配列として書く。
    fn d_zlib(&mut self, values: &[f64]) {
        let mut payload = Vec::with_capacity(values.len() * 8);
        for v in values {
            payload.extend_from_slice(&v.to_le_bytes());
        }
        self.array(b'd', values.len() as u32, 1, &zlib(&payload));
    }

    /// `i32` 列をrawの `i` 配列として書く。
    fn i_raw(&mut self, values: &[i32]) {
        let mut payload = Vec::with_capacity(values.len() * 4);
        for v in values {
            payload.extend_from_slice(&v.to_le_bytes());
        }
        self.array(b'i', values.len() as u32, 0, &payload);
    }

    /// `i32` 列をzlib圧縮の `i` 配列として書く。
    fn i_zlib(&mut self, values: &[i32]) {
        let mut payload = Vec::with_capacity(values.len() * 4);
        for v in values {
            payload.extend_from_slice(&v.to_le_bytes());
        }
        self.array(b'i', values.len() as u32, 1, &zlib(&payload));
    }

    /// 未知の型コードを書く。非対応検出の確認に使う。
    fn unknown_type(&mut self) {
        self.prop(b'Z', &[0, 0, 0, 0]);
    }
}

/// ペイロードをzlib圧縮する。
fn zlib(payload: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(payload).expect("圧縮は失敗しない");
    encoder.finish().expect("圧縮は失敗しない")
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
fn model(id: i64, name: &[u8], subtype: &str, props70: Option<Vec<TNode>>) -> TNode {
    let mut props = Props::default();
    props.l(id);
    props.s(name);
    props.s(subtype.as_bytes());
    let mut children = Vec::new();
    if let Some(ps) = props70 {
        children.push(TNode::branch(b"Properties70", Props::default(), ps));
    }
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

/// `Lcl Translation` などの3数値 `P` を組み立てる。
fn p_triplet(name: &str, v: [f64; 3]) -> TNode {
    let mut values = Props::default();
    values.d(v[0]);
    values.d(v[1]);
    values.d(v[2]);
    p_node(name, name, values)
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
fn vertices_node(values: &[f64], compressed: bool) -> TNode {
    let mut props = Props::default();
    if compressed {
        props.d_zlib(values);
    } else {
        props.d_raw(values);
    }
    TNode::leaf(b"Vertices", props)
}

/// `PolygonVertexIndex` ノードを組み立てる。
fn pvi_node(values: &[i32], compressed: bool) -> TNode {
    let mut props = Props::default();
    if compressed {
        props.i_zlib(values);
    } else {
        props.i_raw(values);
    }
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

/// 四角形1面（頂点4・面1・corner4）の幾何を組み立てる。
fn quad_geometry(id: i64, compressed: bool) -> TNode {
    geometry(
        id,
        b"Quad\x00\x01Geometry",
        vec![
            vertices_node(
                &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0, 0.0],
                compressed,
            ),
            // `!3 == -4` が最終cornerの終端を表す。
            pvi_node(&[0, 1, 2, -4], compressed),
        ],
    )
}

/// 最小構成（rootと子Meshモデルと四角形幾何）を組み立てる。
fn 最小構成ファイル(version: u32) -> Vec<u8> {
    file(
        version,
        vec![
            objects(vec![
                model(1, b"Root\x00\x01Model", "Null", None),
                model(
                    2,
                    b"Child\x00\x01Model",
                    "Mesh",
                    Some(vec![p_triplet("Lcl Translation", [1.0, 2.0, 3.0])]),
                ),
                quad_geometry(3, false),
            ]),
            connections(vec![conn(2, 1), conn(3, 2), conn(1, 0)]),
        ],
    )
}

#[test]
fn 最小構成のモデル階層とメッシュを読める() {
    let bytes = 最小構成ファイル(7400);
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("最小構成は読める");
    assert_eq!(parsed.version(), 7400);
    assert_eq!(parsed.models().len(), 2);
    assert_eq!(parsed.meshes().len(), 1);

    // 名前は `\x00` 以降のクラス名を落とす。
    let root = parsed.model(ObjectId::new(1)).expect("rootがある");
    assert_eq!(root.name, "Root");
    assert_eq!(root.kind, ModelKind::Null);
    assert_eq!(root.parent, None);
    assert_eq!(root.children, vec![ObjectId::new(2)]);
    assert_eq!(root.mesh, None);
    assert_eq!(ObjectId::new(1).value(), 1);

    let child = parsed.model(ObjectId::new(2)).expect("子がある");
    assert_eq!(child.name, "Child");
    assert_eq!(child.kind, ModelKind::Mesh);
    assert_eq!(child.parent, Some(ObjectId::new(1)));
    assert_eq!(child.mesh, Some(ObjectId::new(3)));

    // 親なしの列挙は出現順である。
    let roots: Vec<i64> = parsed.root_models().map(|model| model.id.value()).collect();
    assert_eq!(roots, vec![1]);

    // 未知IDの参照は空を返し、失敗にしない。
    assert!(parsed.model(ObjectId::new(999)).is_none());
    assert!(parsed.mesh(ObjectId::new(999)).is_none());
    assert_eq!(
        parsed.mesh_data(ObjectId::new(999)).unwrap_err().kind(),
        FbxErrorKind::NotFound
    );

    let mesh = parsed.mesh(ObjectId::new(3)).expect("メッシュがある");
    assert_eq!(mesh.vertex_count, 4);
    assert_eq!(mesh.polygon_count, 1);
    assert_eq!(mesh.corner_count, 4);
    assert_eq!(mesh.uv_set_count, 0);
    assert!(!mesh.has_normals);
    assert!(!mesh.has_materials);

    let data = parsed
        .mesh_data(ObjectId::new(3))
        .expect("幾何は展開できる");
    assert_eq!(
        data.positions,
        vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 0.0]
        ]
    );
    assert_eq!(data.polygon_corner_counts, vec![4]);
    assert_eq!(data.corner_positions, vec![0, 1, 2, 3]);
    assert!(data.normals.is_none());
    assert!(data.uv_sets.is_empty());
    assert!(data.material_indices.is_empty());

    // 既定の軸系はY-up・符号1、単位は1である。
    let axis = parsed.axis_system();
    assert_eq!(axis.up, Axis::Y);
    assert_eq!(axis.up_sign, 1);
    assert_eq!(axis.front, Axis::Z);
    assert_eq!(axis.front_sign, 1);
    assert_eq!(axis.coord, Axis::X);
    assert_eq!(axis.coord_sign, 1);
    assert_eq!(parsed.unit_scale_factor(), 1.0);
}

#[test]
fn 軸系と単位を読める() {
    let ps = vec![
        p_int("UpAxis", "int", 2),
        p_int("UpAxisSign", "int", -1),
        p_int("FrontAxis", "int", 0),
        p_int("FrontAxisSign", "int", 1),
        p_int("CoordAxis", "int", 1),
        p_int("CoordAxisSign", "int", 1),
        {
            let mut values = Props::default();
            values.d(100.0);
            p_node("UnitScaleFactor", "double", values)
        },
    ];
    let bytes = file(
        7400,
        vec![
            TNode::branch(
                b"GlobalSettings",
                Props::default(),
                vec![TNode::branch(b"Properties70", Props::default(), ps)],
            ),
            objects(vec![model(1, b"A\x00\x01Model", "Null", None)]),
            connections(vec![conn(1, 0)]),
        ],
    );
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("軸系は読める");
    let axis = parsed.axis_system();
    assert_eq!(axis.up, Axis::Z);
    assert_eq!(axis.up_sign, -1);
    assert_eq!(axis.front, Axis::X);
    assert_eq!(axis.coord, Axis::Y);
    assert_eq!(parsed.unit_scale_factor(), 100.0);
}

#[test]
fn ローカル変換と継承方式を読めて未指定は既定値になる() {
    let bytes = file(
        7400,
        vec![
            objects(vec![
                model(
                    1,
                    b"Moved\x00\x01Model",
                    "LimbNode",
                    Some(vec![
                        p_triplet("Lcl Translation", [1.0, 2.0, 3.0]),
                        p_triplet("Lcl Rotation", [10.0, 20.0, 30.0]),
                        p_triplet("Lcl Scaling", [2.0, 2.0, 2.0]),
                        p_int("InheritType", "enum", 2),
                    ]),
                ),
                model(2, b"Plain\x00\x01Model", "Null", None),
                quad_geometry(3, false),
            ]),
            connections(vec![conn(1, 0), conn(2, 1), conn(3, 1)]),
        ],
    );
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("変換は読める");
    let moved = parsed.model(ObjectId::new(1)).expect("移動節点がある");
    assert_eq!(moved.transform.translation, [1.0, 2.0, 3.0]);
    assert_eq!(moved.transform.rotation, [10.0, 20.0, 30.0]);
    assert_eq!(moved.transform.scale, [2.0, 2.0, 2.0]);
    assert_eq!(moved.inherit_type, 2);
    assert_eq!(moved.kind, ModelKind::LimbNode);

    // 未指定の成分は既定値（移動0・回転0・倍率1・継承1）である。
    let plain = parsed.model(ObjectId::new(2)).expect("素の節点がある");
    assert_eq!(plain.transform.translation, [0.0, 0.0, 0.0]);
    assert_eq!(plain.transform.rotation, [0.0, 0.0, 0.0]);
    assert_eq!(plain.transform.scale, [1.0, 1.0, 1.0]);
    assert_eq!(plain.inherit_type, 1);
}

#[test]
fn 数値の型違いを変換値として読める() {
    // 単精度・2バイト整数・真偽値のプロパティも数値として受け付ける。
    // 意味が変わらない範囲の型違いで拒否しないためである。
    let mut scale_values = Props::default();
    scale_values.f(2.0);
    scale_values.f(3.0);
    scale_values.f(4.0);
    let mut inherit_values = Props::default();
    inherit_values.y(0);
    let mut order_values = Props::default();
    order_values.c(false);
    let bytes = file(
        7400,
        vec![
            objects(vec![model(
                1,
                b"M\x00\x01Model",
                "LimbNode",
                Some(vec![
                    p_node("Lcl Scaling", "Lcl Scaling", scale_values),
                    p_node("InheritType", "enum", inherit_values),
                    p_node("RotationOrder", "enum", order_values),
                ]),
            )]),
            connections(vec![conn(1, 0)]),
        ],
    );
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("型違いは読める");
    let model = parsed.model(ObjectId::new(1)).expect("節点がある");
    assert_eq!(model.transform.scale, [2.0, 3.0, 4.0]);
    assert_eq!(model.inherit_type, 0);
}

#[test]
fn 法線の対応方式と参照方式を解決できる() {
    // ByPolygonVertex + IndexToDirect はcorner列で索引を引く。
    let bytes = file(
        7400,
        vec![
            objects(vec![
                model(1, b"M\x00\x01Model", "Mesh", None),
                geometry(
                    2,
                    b"G\x00\x01Geometry",
                    vec![
                        vertices_node(
                            &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0, 0.0],
                            false,
                        ),
                        pvi_node(&[0, 1, 2, -4], false),
                        normal_layer(
                            b"ByPolygonVertex",
                            b"IndexToDirect",
                            &[0.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0],
                            Some(&[3, 2, 1, 0]),
                        ),
                    ],
                ),
            ]),
            connections(vec![conn(1, 0), conn(2, 1)]),
        ],
    );
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("法線層は読める");
    let data = parsed
        .mesh_data(ObjectId::new(2))
        .expect("法線は解決できる");
    let normals = data.normals.expect("法線がある");
    assert_eq!(normals.values.len(), 4);
    assert_eq!(normals.corner_indices, vec![3, 2, 1, 0]);

    // ByVertice + IndexToDirect は頂点経由でcornerへ写す。
    let bytes = file(
        7400,
        vec![
            objects(vec![
                model(1, b"M\x00\x01Model", "Mesh", None),
                geometry(
                    2,
                    b"G\x00\x01Geometry",
                    vec![
                        vertices_node(
                            &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0, 0.0],
                            false,
                        ),
                        pvi_node(&[0, 1, 2, -4], false),
                        normal_layer(
                            b"ByVertice",
                            b"IndexToDirect",
                            &[0.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0],
                            Some(&[0, 1, 2, 3]),
                        ),
                    ],
                ),
            ]),
            connections(vec![conn(1, 0), conn(2, 1)]),
        ],
    );
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("法線層は読める");
    let data = parsed
        .mesh_data(ObjectId::new(2))
        .expect("法線は解決できる");
    let normals = data.normals.expect("法線がある");
    assert_eq!(normals.corner_indices, vec![0, 1, 2, 3]);

    // Direct は領域番号をそのまま索引にする。
    let bytes = file(
        7400,
        vec![
            objects(vec![
                model(1, b"M\x00\x01Model", "Mesh", None),
                geometry(
                    2,
                    b"G\x00\x01Geometry",
                    vec![
                        vertices_node(
                            &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0, 0.0],
                            false,
                        ),
                        pvi_node(&[0, 1, 2, -4], false),
                        normal_layer(
                            b"ByPolygonVertex",
                            b"Direct",
                            &[0.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0],
                            None,
                        ),
                    ],
                ),
            ]),
            connections(vec![conn(1, 0), conn(2, 1)]),
        ],
    );
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("法線層は読める");
    let data = parsed
        .mesh_data(ObjectId::new(2))
        .expect("法線は解決できる");
    let normals = data.normals.expect("法線がある");
    assert_eq!(normals.values.len(), 4);
    assert_eq!(normals.corner_indices, vec![0, 1, 2, 3]);
}

#[test]
fn 複数のuvセットを順序どおり読み解決できる() {
    let bytes = file(
        7400,
        vec![
            objects(vec![
                model(1, b"M\x00\x01Model", "Mesh", None),
                geometry(
                    2,
                    b"G\x00\x01Geometry",
                    vec![
                        vertices_node(
                            &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0, 0.0],
                            false,
                        ),
                        pvi_node(&[0, 1, 2, -4], false),
                        uv_layer(
                            b"UVMap",
                            b"ByPolygonVertex",
                            b"IndexToDirect",
                            &[0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0, 1.0],
                            Some(&[0, 1, 2, 3]),
                        ),
                        uv_layer(
                            b"UV2",
                            b"ByPolygonVertex",
                            b"IndexToDirect",
                            &[0.5, 0.5, 0.25, 0.75],
                            Some(&[1, 1, 0, 0]),
                        ),
                    ],
                ),
            ]),
            connections(vec![conn(1, 0), conn(2, 1)]),
        ],
    );
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("UV層は読める");
    let mesh = parsed.mesh(ObjectId::new(2)).expect("メッシュがある");
    assert_eq!(mesh.uv_set_count, 2);
    let data = parsed.mesh_data(ObjectId::new(2)).expect("UVは解決できる");
    assert_eq!(data.uv_sets.len(), 2);
    // 出現順を保つ。
    assert_eq!(data.uv_sets[0].name, "UVMap");
    assert_eq!(data.uv_sets[1].name, "UV2");
    assert_eq!(data.uv_sets[0].corner_indices, vec![0, 1, 2, 3]);
    assert_eq!(data.uv_sets[1].corner_indices, vec![1, 1, 0, 0]);
    assert_eq!(data.uv_sets[1].values.len(), 2);
}

/// 三角形2面の幾何を組み立てる。面数は2・corner数は6である。
fn two_triangle_geometry(id: i64, material: TNode) -> TNode {
    geometry(
        id,
        b"Two\x00\x01Geometry",
        vec![
            vertices_node(
                &[
                    0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 2.0, 0.0, 0.0, 2.0, 1.0, 0.0, 3.0,
                    0.0, 0.0,
                ],
                false,
            ),
            // `!2 == -3` と `!5 == -6` が各面の終端を表す。
            pvi_node(&[0, 1, -3, 3, 4, -6], false),
            material,
        ],
    )
}

#[test]
fn 材質の全同一と面ごとを面番号で解決できる() {
    // AllSame は全要素同一であり、1件の値を全面へ広げる。
    let bytes = file(
        7400,
        vec![
            objects(vec![
                model(1, b"M\x00\x01Model", "Mesh", None),
                two_triangle_geometry(2, material_layer(b"AllSame", b"IndexToDirect", &[5])),
            ]),
            connections(vec![conn(1, 0), conn(2, 1)]),
        ],
    );
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("材質層は読める");
    let data = parsed
        .mesh_data(ObjectId::new(2))
        .expect("材質は解決できる");
    assert_eq!(data.polygon_corner_counts, vec![3, 3]);
    assert_eq!(data.material_indices, vec![5, 5]);

    // ByPolygon は面ごとの番号をそのまま返す。
    let bytes = file(
        7400,
        vec![
            objects(vec![
                model(1, b"M\x00\x01Model", "Mesh", None),
                two_triangle_geometry(2, material_layer(b"ByPolygon", b"IndexToDirect", &[1, 2])),
            ]),
            connections(vec![conn(1, 0), conn(2, 1)]),
        ],
    );
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("材質層は読める");
    let data = parsed
        .mesh_data(ObjectId::new(2))
        .expect("材質は解決できる");
    assert_eq!(data.material_indices, vec![1, 2]);
}

#[test]
fn 圧縮配列と非圧縮配列の両方を読める() {
    // 頂点は圧縮・索引は非圧縮の組み合わせで読める。
    let bytes = file(
        7400,
        vec![
            objects(vec![
                model(1, b"M\x00\x01Model", "Mesh", None),
                quad_geometry(2, true),
            ]),
            connections(vec![conn(1, 0), conn(2, 1)]),
        ],
    );
    // 圧縮側の検証のため、非圧縮の最小構成と同値であることを確かめる。
    let plain = 最小構成ファイル(7400);
    let plain_parsed = FbxFile::parse(&plain, &FbxOptions::new()).expect("非圧縮は読める");
    let plain_data = plain_parsed
        .mesh_data(ObjectId::new(3))
        .expect("非圧縮は展開できる");
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("圧縮は読める");
    let data = parsed
        .mesh_data(ObjectId::new(2))
        .expect("圧縮は展開できる");
    assert_eq!(data.positions, plain_data.positions);
    assert_eq!(data.polygon_corner_counts, plain_data.polygon_corner_counts);
    assert_eq!(data.corner_positions, plain_data.corner_positions);
}

#[test]
fn 五角形を含む多角形の形を保ち三角形化しない() {
    let bytes = file(
        7400,
        vec![
            objects(vec![
                model(1, b"M\x00\x01Model", "Mesh", None),
                geometry(
                    2,
                    b"Penta\x00\x01Geometry",
                    vec![
                        vertices_node(
                            &[
                                0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.5, 1.5, 0.0, 0.0,
                                1.0, 0.0,
                            ],
                            false,
                        ),
                        // `!4 == -5` が五角形の終端を表す。
                        pvi_node(&[0, 1, 2, 3, -5], false),
                    ],
                ),
            ]),
            connections(vec![conn(1, 0), conn(2, 1)]),
        ],
    );
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("五角形は読める");
    let mesh = parsed.mesh(ObjectId::new(2)).expect("メッシュがある");
    assert_eq!(mesh.polygon_count, 1);
    assert_eq!(mesh.corner_count, 5);
    let data = parsed
        .mesh_data(ObjectId::new(2))
        .expect("五角形は展開できる");
    assert_eq!(data.polygon_corner_counts, vec![5]);
    assert_eq!(data.corner_positions, vec![0, 1, 2, 3, 4]);
}

#[test]
fn 宣言より大きく膨らむ圧縮配列は大きな確保なく失敗する() {
    // count=3（24バイト）の宣言に対し、8MiBに膨らむ圧縮データを頂点に入れる。
    // 展開先を宣言長+1に制限するため、大きな確保なく破損として失敗する。
    let mut raw = Vec::with_capacity(1 << 23);
    for _ in 0..(1 << 20) {
        raw.extend_from_slice(&0.0f64.to_le_bytes());
    }
    let mut vert_props = Props::default();
    vert_props.array(b'd', 3, 1, &zlib(&raw));
    let bytes = file(
        7400,
        vec![
            objects(vec![
                model(1, b"M\x00\x01Model", "Mesh", None),
                geometry(
                    2,
                    b"G\x00\x01Geometry",
                    vec![
                        TNode::leaf(b"Vertices", vert_props),
                        pvi_node(&[0, 1, 2, -4], false),
                    ],
                ),
            ]),
            connections(vec![conn(1, 0), conn(2, 1)]),
        ],
    );
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("要約は読める");
    let error = parsed
        .mesh_data(ObjectId::new(2))
        .expect_err("宣言超過の展開は失敗する");
    assert_eq!(error.kind(), FbxErrorKind::InvalidStructure);
    assert_eq!(error.object_id(), Some(ObjectId::new(2)));

    // 索引側の宣言超過は要約の計数で展開するため、解析時に失敗する。
    let mut index_raw = Vec::with_capacity(1 << 14);
    for _ in 0..(1 << 12) {
        index_raw.extend_from_slice(&0i32.to_le_bytes());
    }
    let mut pvi_props = Props::default();
    pvi_props.array(b'i', 1, 1, &zlib(&index_raw));
    let bytes = file(
        7400,
        vec![
            objects(vec![
                model(1, b"M\x00\x01Model", "Mesh", None),
                geometry(
                    2,
                    b"G\x00\x01Geometry",
                    vec![
                        vertices_node(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0], false),
                        TNode::leaf(b"PolygonVertexIndex", pvi_props),
                    ],
                ),
            ]),
            connections(vec![conn(1, 0), conn(2, 1)]),
        ],
    );
    let error = FbxFile::parse(&bytes, &FbxOptions::new()).expect_err("宣言超過の索引は失敗する");
    assert_eq!(error.kind(), FbxErrorKind::InvalidStructure);
    assert_eq!(error.object_id(), Some(ObjectId::new(2)));
}

#[test]
fn 不正な識別子と重複識別子は失敗する() {
    // ModelのIDが0以下は破損である。
    for bad in [0, -5] {
        let bytes = file(
            7400,
            vec![
                objects(vec![model(bad, b"M\x00\x01Model", "Null", None)]),
                connections(vec![conn(bad, 0)]),
            ],
        );
        let error =
            FbxFile::parse(&bytes, &FbxOptions::new()).expect_err("IDが0以下のModelは失敗する");
        assert_eq!(error.kind(), FbxErrorKind::InvalidStructure);
        assert_eq!(error.object_id(), Some(ObjectId::new(bad)));
    }

    // ModelのID重複は破損である。
    let bytes = file(
        7400,
        vec![
            objects(vec![
                model(1, b"A\x00\x01Model", "Null", None),
                model(1, b"B\x00\x01Model", "Null", None),
            ]),
            connections(vec![conn(1, 0)]),
        ],
    );
    let error = FbxFile::parse(&bytes, &FbxOptions::new()).expect_err("ModelのID重複は失敗する");
    assert_eq!(error.kind(), FbxErrorKind::InvalidStructure);
    assert_eq!(error.object_id(), Some(ObjectId::new(1)));

    // GeometryのIDが0以下や重複は破損である。
    for bad in [0, -3] {
        let bytes = file(
            7400,
            vec![
                objects(vec![
                    model(1, b"M\x00\x01Model", "Mesh", None),
                    quad_geometry(bad, false),
                ]),
                connections(vec![conn(1, 0), conn(bad, 1)]),
            ],
        );
        let error =
            FbxFile::parse(&bytes, &FbxOptions::new()).expect_err("IDが0以下のGeometryは失敗する");
        assert_eq!(error.kind(), FbxErrorKind::InvalidStructure);
        assert_eq!(error.object_id(), Some(ObjectId::new(bad)));
    }
    let bytes = file(
        7400,
        vec![
            objects(vec![
                model(1, b"M\x00\x01Model", "Mesh", None),
                quad_geometry(2, false),
                quad_geometry(2, false),
            ]),
            connections(vec![conn(1, 0), conn(2, 1)]),
        ],
    );
    let error = FbxFile::parse(&bytes, &FbxOptions::new()).expect_err("GeometryのID重複は失敗する");
    assert_eq!(error.kind(), FbxErrorKind::InvalidStructure);
    assert_eq!(error.object_id(), Some(ObjectId::new(2)));
}

#[test]
fn modelとgeometryのid衝突は失敗する() {
    // FBXのオブジェクトIDは全体で一意であり、衝突すると接続の分類が曖昧になるため破損とする。
    let bytes = file(
        7400,
        vec![
            objects(vec![
                model(1, b"M\x00\x01Model", "Mesh", None),
                quad_geometry(1, false),
            ]),
            connections(vec![conn(1, 0)]),
        ],
    );
    let error =
        FbxFile::parse(&bytes, &FbxOptions::new()).expect_err("ModelとGeometryのID衝突は失敗する");
    assert_eq!(error.kind(), FbxErrorKind::InvalidStructure);
    assert_eq!(error.object_id(), Some(ObjectId::new(1)));
}

#[test]
fn 不正なutf8のノード名は失敗する() {
    // ノード名は厳密なUTF-8として読み、損失置換しない。
    let mut name = b"Objects".to_vec();
    name[3] = 0xff;
    let bytes = file(
        7400,
        vec![
            TNode::branch(
                &name,
                Props::default(),
                vec![model(1, b"M\x00\x01Model", "Null", None)],
            ),
            connections(vec![conn(1, 0)]),
        ],
    );
    let error =
        FbxFile::parse(&bytes, &FbxOptions::new()).expect_err("不正UTF-8のノード名は失敗する");
    assert_eq!(error.kind(), FbxErrorKind::InvalidStructure);
}

#[test]
fn 長い親子鎖は循環誤検出なく通る() {
    // 5000件の鎖は循環なしに通る。訪問状態による線形検出の確認である。
    const N: i64 = 5000;
    let mut objs = Vec::new();
    for id in 1..=N {
        let name = format!("N{id}");
        objs.push(model(id, name.as_bytes(), "Null", None));
    }
    let mut conns = vec![conn(1, 0)];
    for id in 2..=N {
        conns.push(conn(id, id - 1));
    }
    let bytes = file(7400, vec![objects(objs), connections(conns)]);
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("長い鎖は通る");
    assert_eq!(parsed.models().len() as i64, N);
    let roots: Vec<i64> = parsed.root_models().map(|model| model.id.value()).collect();
    assert_eq!(roots, vec![1]);
    // 末端から根まで辿ると鎖の長さに戻る。
    let mut depth = 0;
    let mut current = Some(ObjectId::new(N));
    while let Some(id) = current {
        let model = parsed.model(id).expect("鎖上の節点がある");
        current = model.parent;
        depth += 1;
    }
    assert_eq!(depth, N);
}

#[test]
fn 非utf8の名前は対象識別子付きで失敗する() {
    // 非UTF-8のModel名は破損であり、対象IDを付ける。
    let bytes = file(
        7400,
        vec![
            objects(vec![model(7, b"\xff\xfe", "Null", None)]),
            connections(vec![conn(7, 0)]),
        ],
    );
    let error = FbxFile::parse(&bytes, &FbxOptions::new()).expect_err("非UTF-8のModel名は失敗する");
    assert_eq!(error.kind(), FbxErrorKind::InvalidStructure);
    assert_eq!(error.object_id(), Some(ObjectId::new(7)));

    // 非UTF-8のGeometry名も破損であり、対象IDを付ける。
    let bytes = file(
        7400,
        vec![
            objects(vec![
                model(1, b"M\x00\x01Model", "Mesh", None),
                geometry(
                    8,
                    b"\xff\xfe",
                    vec![
                        vertices_node(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0], false),
                        pvi_node(&[0, 1, -3], false),
                    ],
                ),
            ]),
            connections(vec![conn(1, 0), conn(8, 1)]),
        ],
    );
    let error =
        FbxFile::parse(&bytes, &FbxOptions::new()).expect_err("非UTF-8のGeometry名は失敗する");
    assert_eq!(error.kind(), FbxErrorKind::InvalidStructure);
    assert_eq!(error.object_id(), Some(ObjectId::new(8)));

    // 非UTF-8のUVセット名も破損であり、対象IDを付ける。
    let bytes = file(
        7400,
        vec![
            objects(vec![
                model(1, b"M\x00\x01Model", "Mesh", None),
                geometry(
                    9,
                    b"G\x00\x01Geometry",
                    vec![
                        vertices_node(
                            &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0, 0.0],
                            false,
                        ),
                        pvi_node(&[0, 1, 2, -4], false),
                        uv_layer(
                            b"\xff\xfe",
                            b"ByPolygonVertex",
                            b"IndexToDirect",
                            &[0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0, 1.0],
                            Some(&[0, 1, 2, 3]),
                        ),
                    ],
                ),
            ]),
            connections(vec![conn(1, 0), conn(9, 1)]),
        ],
    );
    let error =
        FbxFile::parse(&bytes, &FbxOptions::new()).expect_err("非UTF-8のUVセット名は失敗する");
    assert_eq!(error.kind(), FbxErrorKind::InvalidStructure);
    assert_eq!(error.object_id(), Some(ObjectId::new(9)));
}

#[test]
fn 多重親と循環と多重所有は失敗する() {
    // 1つのModelに親が2つある場合は不正である。
    let bytes = file(
        7400,
        vec![
            objects(vec![
                model(1, b"A\x00\x01Model", "Null", None),
                model(2, b"B\x00\x01Model", "Null", None),
                model(3, b"C\x00\x01Model", "Null", None),
            ]),
            connections(vec![conn(3, 1), conn(3, 2)]),
        ],
    );
    assert_eq!(
        FbxFile::parse(&bytes, &FbxOptions::new())
            .unwrap_err()
            .kind(),
        FbxErrorKind::InvalidStructure
    );

    // 親子が循環する場合は不正である。
    let bytes = file(
        7400,
        vec![
            objects(vec![
                model(1, b"A\x00\x01Model", "Null", None),
                model(2, b"B\x00\x01Model", "Null", None),
            ]),
            connections(vec![conn(1, 2), conn(2, 1)]),
        ],
    );
    assert_eq!(
        FbxFile::parse(&bytes, &FbxOptions::new())
            .unwrap_err()
            .kind(),
        FbxErrorKind::InvalidStructure
    );

    // 1つのGeometryを複数Modelが所有する場合は不正である。
    let bytes = file(
        7400,
        vec![
            objects(vec![
                model(1, b"A\x00\x01Model", "Mesh", None),
                model(2, b"B\x00\x01Model", "Mesh", None),
                quad_geometry(3, false),
            ]),
            connections(vec![conn(1, 0), conn(2, 0), conn(3, 1), conn(3, 2)]),
        ],
    );
    assert_eq!(
        FbxFile::parse(&bytes, &FbxOptions::new())
            .unwrap_err()
            .kind(),
        FbxErrorKind::InvalidStructure
    );

    // 未知IDへの接続は無視し、失敗にしない。
    let bytes = file(
        7400,
        vec![
            objects(vec![
                model(1, b"A\x00\x01Model", "Null", None),
                quad_geometry(2, false),
            ]),
            connections(vec![conn(1, 0), conn(9999, 1), conn(2, 8888)]),
        ],
    );
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("未知IDは無視する");
    assert_eq!(
        parsed.model(ObjectId::new(1)).expect("モデルがある").parent,
        None
    );
}

#[test]
fn 範囲外索引と不正な多角形と非有限値と長さ不一致は失敗する() {
    // 頂点番号が範囲外の場合は展開時に失敗する。
    let bytes = file(
        7400,
        vec![
            objects(vec![
                model(1, b"M\x00\x01Model", "Mesh", None),
                geometry(
                    2,
                    b"G\x00\x01Geometry",
                    vec![
                        vertices_node(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0], false),
                        pvi_node(&[0, 1, -9], false),
                    ],
                ),
            ]),
            connections(vec![conn(1, 0), conn(2, 1)]),
        ],
    );
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("要約は読める");
    assert_eq!(
        parsed.mesh_data(ObjectId::new(2)).unwrap_err().kind(),
        FbxErrorKind::InvalidStructure
    );

    // corner数が3未満の多角形は不正である。
    let bytes = file(
        7400,
        vec![
            objects(vec![
                model(1, b"M\x00\x01Model", "Mesh", None),
                geometry(
                    2,
                    b"G\x00\x01Geometry",
                    vec![
                        vertices_node(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0], false),
                        pvi_node(&[0, -2], false),
                    ],
                ),
            ]),
            connections(vec![conn(1, 0), conn(2, 1)]),
        ],
    );
    assert_eq!(
        FbxFile::parse(&bytes, &FbxOptions::new())
            .unwrap_err()
            .kind(),
        FbxErrorKind::InvalidStructure
    );

    // NaN座標は不正である。
    let bytes = file(
        7400,
        vec![
            objects(vec![
                model(1, b"M\x00\x01Model", "Mesh", None),
                geometry(
                    2,
                    b"G\x00\x01Geometry",
                    vec![
                        vertices_node(
                            &[
                                f64::NAN,
                                0.0,
                                0.0,
                                1.0,
                                0.0,
                                0.0,
                                1.0,
                                1.0,
                                0.0,
                                0.0,
                                1.0,
                                0.0,
                            ],
                            false,
                        ),
                        pvi_node(&[0, 1, 2, -4], false),
                    ],
                ),
            ]),
            connections(vec![conn(1, 0), conn(2, 1)]),
        ],
    );
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("要約は読める");
    assert_eq!(
        parsed.mesh_data(ObjectId::new(2)).unwrap_err().kind(),
        FbxErrorKind::InvalidStructure
    );

    // 属性データ長が対応方式と一致しない場合は失敗する。
    let bytes = file(
        7400,
        vec![
            objects(vec![
                model(1, b"M\x00\x01Model", "Mesh", None),
                geometry(
                    2,
                    b"G\x00\x01Geometry",
                    vec![
                        vertices_node(
                            &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0, 0.0],
                            false,
                        ),
                        pvi_node(&[0, 1, 2, -4], false),
                        normal_layer(b"ByPolygonVertex", b"Direct", &[0.0, 0.0, 1.0], None),
                    ],
                ),
            ]),
            connections(vec![conn(1, 0), conn(2, 1)]),
        ],
    );
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("要約は読める");
    assert_eq!(
        parsed.mesh_data(ObjectId::new(2)).unwrap_err().kind(),
        FbxErrorKind::InvalidStructure
    );

    // 法線索引が範囲外の場合は失敗する。
    let bytes = file(
        7400,
        vec![
            objects(vec![
                model(1, b"M\x00\x01Model", "Mesh", None),
                geometry(
                    2,
                    b"G\x00\x01Geometry",
                    vec![
                        vertices_node(
                            &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0, 0.0],
                            false,
                        ),
                        pvi_node(&[0, 1, 2, -4], false),
                        normal_layer(
                            b"ByPolygonVertex",
                            b"IndexToDirect",
                            &[0.0, 0.0, 1.0],
                            Some(&[0, 0, 0, 9]),
                        ),
                    ],
                ),
            ]),
            connections(vec![conn(1, 0), conn(2, 1)]),
        ],
    );
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("要約は読める");
    assert_eq!(
        parsed.mesh_data(ObjectId::new(2)).unwrap_err().kind(),
        FbxErrorKind::InvalidStructure
    );

    // 材質番号が負の場合は失敗する。
    let bytes = file(
        7400,
        vec![
            objects(vec![
                model(1, b"M\x00\x01Model", "Mesh", None),
                two_triangle_geometry(2, material_layer(b"ByPolygon", b"IndexToDirect", &[1, -1])),
            ]),
            connections(vec![conn(1, 0), conn(2, 1)]),
        ],
    );
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("要約は読める");
    assert_eq!(
        parsed.mesh_data(ObjectId::new(2)).unwrap_err().kind(),
        FbxErrorKind::InvalidStructure
    );
}

#[test]
fn 非対応の変換成分で失敗し回転順序の0は通る() {
    // `PreRotation` は黙って落とすと姿勢が変わるため、非対応として失敗する。
    let bytes = file(
        7400,
        vec![
            objects(vec![model(
                1,
                b"M\x00\x01Model",
                "LimbNode",
                Some(vec![p_triplet("PreRotation", [0.0, 0.0, 0.0])]),
            )]),
            connections(vec![conn(1, 0)]),
        ],
    );
    assert_eq!(
        FbxFile::parse(&bytes, &FbxOptions::new())
            .unwrap_err()
            .kind(),
        FbxErrorKind::UnsupportedFeature
    );

    // `RotationOrder` の0は既定のXYZ順と一致するため、通す。
    let bytes = file(
        7400,
        vec![
            objects(vec![model(
                1,
                b"M\x00\x01Model",
                "LimbNode",
                Some(vec![p_int("RotationOrder", "enum", 0)]),
            )]),
            connections(vec![conn(1, 0)]),
        ],
    );
    FbxFile::parse(&bytes, &FbxOptions::new()).expect("RotationOrderの0は通る");

    // `RotationOrder` の0以外は非対応として失敗する。
    let bytes = file(
        7400,
        vec![
            objects(vec![model(
                1,
                b"M\x00\x01Model",
                "LimbNode",
                Some(vec![p_int("RotationOrder", "enum", 1)]),
            )]),
            connections(vec![conn(1, 0)]),
        ],
    );
    assert_eq!(
        FbxFile::parse(&bytes, &FbxOptions::new())
            .unwrap_err()
            .kind(),
        FbxErrorKind::UnsupportedFeature
    );
}

#[test]
fn ヘッダと版と暗号化と未知型と破損は失敗する() {
    // マジック不一致はASCII FBXなどを含め拒否する。
    let mut ascii = b"; FBX 7.4.0 project file".to_vec();
    ascii.resize(64, 0);
    assert_eq!(
        FbxFile::parse(&ascii, &FbxOptions::new())
            .unwrap_err()
            .kind(),
        FbxErrorKind::InvalidHeader
    );

    // 版が範囲外の場合は非対応である。
    for version in [6900, 7500] {
        let bytes = 最小構成ファイル(version);
        assert_eq!(
            FbxFile::parse(&bytes, &FbxOptions::new())
                .unwrap_err()
                .kind(),
            FbxErrorKind::UnsupportedVersion,
            "版{version}は非対応である"
        );
    }

    // 暗号化種別が0以外は非対応である。
    let mut header_props = Props::default();
    header_props.i(1);
    let bytes = file(
        7400,
        vec![
            TNode::branch(
                b"FBXHeaderExtension",
                Props::default(),
                vec![TNode::leaf(b"EncryptionType", header_props)],
            ),
            objects(vec![model(1, b"A\x00\x01Model", "Null", None)]),
            connections(vec![conn(1, 0)]),
        ],
    );
    assert_eq!(
        FbxFile::parse(&bytes, &FbxOptions::new())
            .unwrap_err()
            .kind(),
        FbxErrorKind::UnsupportedFeature
    );

    // 未知のプロパティ型は非対応である。
    let mut bad = Props::default();
    bad.l(1);
    bad.unknown_type();
    bad.s(b"Null");
    let bytes = file(
        7400,
        vec![
            TNode::branch(
                b"Objects",
                Props::default(),
                vec![TNode::leaf(b"Model", bad)],
            ),
            connections(vec![conn(1, 0)]),
        ],
    );
    assert_eq!(
        FbxFile::parse(&bytes, &FbxOptions::new())
            .unwrap_err()
            .kind(),
        FbxErrorKind::UnsupportedFeature
    );

    // 未知のMappingは非対応である。
    let bytes = file(
        7400,
        vec![
            objects(vec![
                model(1, b"M\x00\x01Model", "Mesh", None),
                geometry(
                    2,
                    b"G\x00\x01Geometry",
                    vec![
                        vertices_node(
                            &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0, 0.0],
                            false,
                        ),
                        pvi_node(&[0, 1, 2, -4], false),
                        normal_layer(b"ByEdge", b"Direct", &[0.0, 0.0, 1.0], None),
                    ],
                ),
            ]),
            connections(vec![conn(1, 0), conn(2, 1)]),
        ],
    );
    assert_eq!(
        FbxFile::parse(&bytes, &FbxOptions::new())
            .unwrap_err()
            .kind(),
        FbxErrorKind::UnsupportedFeature
    );

    // 未知の配列encodingは非対応である。
    let bytes = file(
        7400,
        vec![
            objects(vec![
                model(1, b"M\x00\x01Model", "Mesh", None),
                geometry(
                    2,
                    b"G\x00\x01Geometry",
                    vec![
                        {
                            let mut props = Props::default();
                            props.array(b'd', 12, 2, &[0u8; 12]);
                            TNode::leaf(b"Vertices", props)
                        },
                        pvi_node(&[0, 1, 2, -4], false),
                    ],
                ),
            ]),
            connections(vec![conn(1, 0), conn(2, 1)]),
        ],
    );
    assert_eq!(
        FbxFile::parse(&bytes, &FbxOptions::new())
            .unwrap_err()
            .kind(),
        FbxErrorKind::UnsupportedFeature
    );

    // 切り詰めは破損として失敗する。
    let bytes = 最小構成ファイル(7400);
    let cut = bytes[..60].to_vec();
    assert_eq!(
        FbxFile::parse(&cut, &FbxOptions::new()).unwrap_err().kind(),
        FbxErrorKind::InvalidStructure
    );

    // zlib破損は破損として失敗する。
    let mut bytes = file(
        7400,
        vec![
            objects(vec![
                model(1, b"M\x00\x01Model", "Mesh", None),
                quad_geometry(2, true),
            ]),
            connections(vec![conn(1, 0), conn(2, 1)]),
        ],
    );
    // 圧縮ペイロードの末尾を壊し、展開の一致を崩す。
    if let Some(last) = bytes.iter_mut().rev().nth(20) {
        *last ^= 0xff;
    }
    assert_eq!(
        FbxFile::parse(&bytes, &FbxOptions::new())
            .unwrap_err()
            .kind(),
        FbxErrorKind::InvalidStructure
    );

    // 終了位置が現在位置以前のノードは破損として失敗する。
    let mut bytes = 最小構成ファイル(7400);
    bytes[27..31].copy_from_slice(&20u32.to_le_bytes());
    assert_eq!(
        FbxFile::parse(&bytes, &FbxOptions::new())
            .unwrap_err()
            .kind(),
        FbxErrorKind::InvalidStructure
    );
}

#[test]
fn 資源上限の超過は失敗し引き上げで回避できる() {
    // ノード数上限は直下の数で確かめる。
    let many: Vec<TNode> = (0..8)
        .map(|n| {
            let mut props = Props::default();
            props.i(n);
            TNode::leaf(b"FileId", props)
        })
        .collect();
    let bytes = file(7400, many);
    assert_eq!(
        FbxFile::parse(&bytes, &FbxOptions::new().with_max_nodes(4))
            .unwrap_err()
            .kind(),
        FbxErrorKind::LimitExceeded
    );
    FbxFile::parse(&bytes, &FbxOptions::new().with_max_nodes(100)).expect("引き上げで通る");

    // 深さ上限は入れ子で確かめる。
    fn nested(depth: u32) -> TNode {
        if depth == 0 {
            return TNode::leaf(b"Leaf", Props::default());
        }
        TNode::branch(b"Level", Props::default(), vec![nested(depth - 1)])
    }
    let bytes = file(7400, vec![nested(6)]);
    assert_eq!(
        FbxFile::parse(&bytes, &FbxOptions::new().with_max_depth(2))
            .unwrap_err()
            .kind(),
        FbxErrorKind::LimitExceeded
    );
    FbxFile::parse(&bytes, &FbxOptions::new().with_max_depth(16)).expect("引き上げで通る");

    // 文字列長上限は長い名前で確かめる。
    let long = vec![b'x'; 64];
    let bytes = file(
        7400,
        vec![
            objects(vec![model(1, &long, "Null", None)]),
            connections(vec![]),
        ],
    );
    assert_eq!(
        FbxFile::parse(&bytes, &FbxOptions::new().with_max_string_bytes(8))
            .unwrap_err()
            .kind(),
        FbxErrorKind::LimitExceeded
    );
    FbxFile::parse(&bytes, &FbxOptions::new().with_max_string_bytes(128)).expect("引き上げで通る");

    // 配列要素数上限は頂点配列で確かめる。
    let bytes = file(
        7400,
        vec![
            objects(vec![
                model(1, b"M\x00\x01Model", "Mesh", None),
                geometry(
                    2,
                    b"G\x00\x01Geometry",
                    vec![
                        vertices_node(&[0.0; 30], false),
                        pvi_node(&[0, 1, 2, -4, 0, 1, 2, -4, 3, 4, 5, -6], false),
                    ],
                ),
            ]),
            connections(vec![conn(1, 0), conn(2, 1)]),
        ],
    );
    assert_eq!(
        FbxFile::parse(&bytes, &FbxOptions::new().with_max_array_elements(8))
            .unwrap_err()
            .kind(),
        FbxErrorKind::LimitExceeded
    );
    FbxFile::parse(&bytes, &FbxOptions::new().with_max_array_elements(64)).expect("引き上げで通る");

    // 合計上限は複数配列の積み上げで確かめる。
    assert_eq!(
        FbxFile::parse(
            &bytes,
            &FbxOptions::new()
                .with_max_array_elements(64)
                .with_max_total_array_elements(16)
        )
        .unwrap_err()
        .kind(),
        FbxErrorKind::LimitExceeded
    );
    FbxFile::parse(&bytes, &FbxOptions::new()).expect("既定では通る");
}

#[test]
fn 制御文字を含む名前でも表示に生の制御文字が出ない() {
    // 外部由来の対応方式名が診断に入る場合、逃がして表示する。
    let bytes = file(
        7400,
        vec![
            objects(vec![
                model(1, b"M\x00\x01Model", "Mesh", None),
                geometry(
                    2,
                    b"G\x00\x01Geometry",
                    vec![
                        vertices_node(
                            &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0, 0.0],
                            false,
                        ),
                        pvi_node(&[0, 1, 2, -4], false),
                        normal_layer(b"A\nB\x01", b"Direct", &[0.0, 0.0, 1.0], None),
                    ],
                ),
            ]),
            connections(vec![conn(1, 0), conn(2, 1)]),
        ],
    );
    let error = FbxFile::parse(&bytes, &FbxOptions::new()).expect_err("非対応方式は失敗する");
    assert_eq!(error.kind(), FbxErrorKind::UnsupportedFeature);
    let shown = error.to_string();
    assert!(
        !shown.contains('\n') && !shown.contains('\x01'),
        "生の制御文字を出さない: {shown:?}"
    );
    assert!(
        shown.contains("\\n") && shown.contains("\\x01"),
        "逃がした形で出す: {shown:?}"
    );
}

/// 実fixtureの全メッシュで不変条件を確かめる。
///
/// 固定件数に依存せず、面数・corner数・索引範囲・有限性を検証する。
/// 件数と時間を標準エラーに記録する。
#[test]
#[cfg(not(target_arch = "wasm32"))]
fn 実fixtureの全メッシュで不変条件を満たす() {
    let Ok(path) = std::env::var("VRCTOM_FBX_FIXTURE") else {
        eprintln!("VRCTOM_FBX_FIXTUREが未設定のため跳ばす");
        return;
    };
    let started = std::time::Instant::now();
    let bytes = std::fs::read(&path).expect("実fixtureは読める");
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("実fixtureは読める");
    assert_eq!(parsed.version(), 7400);
    let mut meshes = 0;
    let mut polygons = 0;
    let mut corners = 0;
    for mesh in parsed.meshes() {
        let data = parsed
            .mesh_data(mesh.id)
            .expect("実fixtureの幾何は展開できる");
        // 面ごとのcorner数の合計はcorner数と一致する。
        let summed: usize = data
            .polygon_corner_counts
            .iter()
            .map(|count| *count as usize)
            .sum();
        assert_eq!(summed, data.corner_positions.len());
        assert_eq!(data.corner_positions.len(), mesh.corner_count);
        // polygon数の報告と一致する。
        assert_eq!(data.polygon_corner_counts.len(), mesh.polygon_count);
        // 材質番号は面ごとであり、無い場合は空である。
        if mesh.has_materials {
            assert_eq!(data.material_indices.len(), mesh.polygon_count);
        } else {
            assert!(data.material_indices.is_empty());
        }
        for count in &data.polygon_corner_counts {
            assert!(*count >= 3);
        }
        // 全cornerの位置索引は範囲内であり、座標は有限である。
        for position in &data.positions {
            assert!(position.iter().all(|v| v.is_finite()));
        }
        for corner in &data.corner_positions {
            assert!((*corner as usize) < data.positions.len());
        }
        if let Some(normals) = &data.normals {
            for value in &normals.values {
                assert!(value.iter().all(|v| v.is_finite()));
            }
            assert_eq!(normals.corner_indices.len(), data.corner_positions.len());
            for index in &normals.corner_indices {
                assert!((*index as usize) < normals.values.len());
            }
        }
        for set in &data.uv_sets {
            for value in &set.values {
                assert!(value.iter().all(|v| v.is_finite()));
            }
            assert_eq!(set.corner_indices.len(), data.corner_positions.len());
            for index in &set.corner_indices {
                assert!((*index as usize) < set.values.len());
            }
        }
        meshes += 1;
        polygons += data.polygon_corner_counts.len();
        corners += data.corner_positions.len();
    }
    eprintln!(
        "fixture={path} meshes={meshes} polygons={polygons} corners={corners} elapsed={:?}",
        started.elapsed()
    );
    // Blenderオラクルとの突合用に、各メッシュの形状を昇順で1行に出す。件数は固定しない。
    let mut shapes: Vec<(usize, usize, usize)> = parsed
        .meshes()
        .iter()
        .map(|mesh| (mesh.vertex_count, mesh.polygon_count, mesh.corner_count))
        .collect();
    shapes.sort();
    eprintln!("mesh_shapes={shapes:?}");
    assert!(meshes > 0, "メッシュを1件以上検証する");
}
