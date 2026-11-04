//! 共有変換入口の契約テスト（公開 API のみを使う）。
//!
//! テスト内でバイナリFBXを組み立て、`convert_fbx`・`build_model`・
//! `export_model` の振る舞いと段階ごとの失敗の区別を固定する。

use vrctom::conversion::{ConversionError, build_model, convert_fbx, export_model};
use vrctom::fbx::{FbxFile, FbxOptions};
use vrctom::vrm::{VrmExportOptions, VrmMeta, VrmModel, VrmOptions};

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
fn file(top: Vec<TNode>) -> Vec<u8> {
    let mut buf = b"Kaydara FBX Binary  \x00\x1a\x00".to_vec();
    buf.extend_from_slice(&7400u32.to_le_bytes());
    for node in &top {
        ser(node, &mut buf);
    }
    // トップレベルの終端以降はフッタであり、読まない。
    buf.extend_from_slice(&[0u8; 13]);
    buf
}

/// `Model` ノードを組み立てる。
fn model(id: i64, name: &str, subtype: &str) -> TNode {
    let mut props = Props::default();
    props.l(id);
    props.s(format!("{name}\x00\x01Model").as_bytes());
    props.s(subtype.as_bytes());
    TNode::branch(b"Model", props, Vec::new())
}

/// `Geometry`（Mesh）ノードを組み立てる。
fn geometry(id: i64, name: &str, children: Vec<TNode>) -> TNode {
    let mut props = Props::default();
    props.l(id);
    props.s(format!("{name}\x00\x01Geometry").as_bytes());
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

/// 必須15骨を持つモデル列を組み立てる。末尾はメッシュ所有モデルである。
///
/// 親子関係は書き出し層の許容（上腕は肩・胸・脊椎のいずれかの下など）を満たす。
fn 必須骨モデル列() -> Vec<(i64, &'static str, Option<i64>)> {
    vec![
        (1, "Hips", None),
        (2, "Spine", Some(1)),
        (3, "Head", Some(2)),
        (4, "LeftUpperArm", Some(2)),
        (5, "LeftLowerArm", Some(4)),
        (6, "LeftHand", Some(5)),
        (7, "RightUpperArm", Some(2)),
        (8, "RightLowerArm", Some(7)),
        (9, "RightHand", Some(8)),
        (10, "LeftUpperLeg", Some(1)),
        (11, "LeftLowerLeg", Some(10)),
        (12, "LeftFoot", Some(11)),
        (13, "RightUpperLeg", Some(1)),
        (14, "RightLowerLeg", Some(13)),
        (15, "RightFoot", Some(14)),
        (16, "Owner", Some(1)),
    ]
}

/// 必須15骨と三角形1枚を持つFBXを組み立てる。変換から書き出しまで通る。
fn 書き出し可能なfbxを組み立てる() -> Vec<u8> {
    let mut objs: Vec<TNode> = 必須骨モデル列()
        .iter()
        .map(|(id, name, _)| {
            let subtype = if *name == "Owner" { "Mesh" } else { "LimbNode" };
            model(*id, name, subtype)
        })
        .collect();
    objs.push(geometry(
        100,
        "Tri",
        vec![
            vertices_node(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]),
            // `!2 == -3` が終端を表す。
            pvi_node(&[0, 1, -3]),
        ],
    ));
    let mut pose_props = Props::default();
    pose_props.l(200);
    pose_props.s(b"Bind\x00\x01Pose");
    pose_props.s(b"BindPose");
    let binds: Vec<TNode> = 必須骨モデル列()
        .iter()
        .map(|(id, _, _)| pose_node(*id, &bind_matrix(0.0, 0.0, 0.0)))
        .collect();
    objs.push(TNode::branch(b"Pose", pose_props, binds));
    let mut conns: Vec<TNode> = 必須骨モデル列()
        .iter()
        .filter_map(|(id, _, parent)| parent.map(|parent| conn(*id, parent)))
        .collect();
    conns.push(conn(100, 16));
    conns.push(conn(1, 0));
    file(vec![objects(objs), connections(conns)])
}

/// 三角形1枚・所有モデル付きだが人体骨を持たないFBXを組み立てる。
///
/// 中間モデルまでは作れるが、書き出しのhumanoid検証で失敗する。
fn 人体骨なしfbxを組み立てる() -> Vec<u8> {
    let objs = vec![
        model(1, "Root", "Null"),
        model(2, "Owner", "Mesh"),
        geometry(
            10,
            "Tri",
            vec![
                vertices_node(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]),
                pvi_node(&[0, 1, -3]),
            ],
        ),
        {
            let mut pose_props = Props::default();
            pose_props.l(100);
            pose_props.s(b"Bind\x00\x01Pose");
            pose_props.s(b"BindPose");
            TNode::branch(
                b"Pose",
                pose_props,
                vec![
                    pose_node(1, &bind_matrix(0.0, 0.0, 0.0)),
                    pose_node(2, &bind_matrix(1.0, 2.0, 3.0)),
                ],
            )
        },
    ];
    file(vec![
        objects(objs),
        connections(vec![conn(2, 1), conn(10, 2), conn(1, 0)]),
    ])
}

/// バインド姿勢を持たないFBXを組み立てる。読み取りは通るが中間モデルで失敗する。
fn バインドなしfbxを組み立てる() -> Vec<u8> {
    let objs = vec![
        model(1, "Root", "Null"),
        model(2, "Owner", "Mesh"),
        geometry(
            10,
            "Tri",
            vec![
                vertices_node(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]),
                pvi_node(&[0, 1, -3]),
            ],
        ),
    ];
    file(vec![
        objects(objs),
        connections(vec![conn(2, 1), conn(10, 2), conn(1, 0)]),
    ])
}

/// 書き出し設定を作る。
fn 書き出し設定を作る() -> VrmExportOptions {
    VrmExportOptions::new(VrmMeta::new("試験", vec!["作者".to_string()]))
}

#[test]
fn 正常なfbxでconvert_fbxがvrmバイト列とレポートを返す() {
    let bytes = 書き出し可能なfbxを組み立てる();
    let (vrm_bytes, report) = convert_fbx(
        &bytes,
        &FbxOptions::new(),
        &VrmOptions::new(),
        &書き出し設定を作る(),
    )
    .expect("必須骨を持つFBXは変換できる");
    // GLBの先頭4バイトが `glTF` である。
    assert!(vrm_bytes.len() >= 4, "出力が短すぎる");
    assert_eq!(&vrm_bytes[..4], b"glTF");
    assert_eq!(report.node_count, 16);
    assert_eq!(report.mesh_count, 1);
    assert_eq!(report.skin_count, 0);
}

#[test]
fn 不正なバイト列ではfbx解析の段階で失敗する() {
    let bytes = b"not a fbx stream";
    let error = convert_fbx(
        bytes,
        &FbxOptions::new(),
        &VrmOptions::new(),
        &書き出し設定を作る(),
    )
    .expect_err("不正なバイト列は失敗する");
    assert!(
        matches!(error, ConversionError::Fbx(_)),
        "段階はFBX解析である"
    );
    assert_eq!(error.stage(), "FBXの解析");
    // 表示は読み取り層の既存表示を壊さず、原因を辿れる。
    let direct = FbxFile::parse(bytes, &FbxOptions::new()).expect_err("直接の解析も失敗する");
    assert_eq!(format!("{error}"), format!("{direct}"));
    assert!(std::error::Error::source(&error).is_some());
}

#[test]
fn バインド姿勢が無いfbxでは中間モデルの段階で失敗する() {
    let bytes = バインドなしfbxを組み立てる();
    // 前提として読み取り自体は通る。
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("姿勢なしも読める");
    let direct =
        VrmModel::from_fbx(&parsed, &VrmOptions::new()).expect_err("バインド欠落は失敗する");
    let error = convert_fbx(
        &bytes,
        &FbxOptions::new(),
        &VrmOptions::new(),
        &書き出し設定を作る(),
    )
    .expect_err("バインド欠落は失敗する");
    assert!(
        matches!(error, ConversionError::Model(_)),
        "段階は中間モデルである"
    );
    assert_eq!(error.stage(), "中間モデルの生成");
    assert_eq!(format!("{error}"), format!("{direct}"));
    assert!(std::error::Error::source(&error).is_some());
}

#[test]
fn 人体骨が無いfbxでは書き出しの段階で失敗する() {
    let bytes = 人体骨なしfbxを組み立てる();
    // 前提として中間モデルまでは作れる。
    let parsed = FbxFile::parse(&bytes, &FbxOptions::new()).expect("骨なしも読める");
    let (model, _) =
        VrmModel::from_fbx(&parsed, &VrmOptions::new()).expect("骨なしも中間モデルは作れる");
    let direct = model
        .to_vrm(&書き出し設定を作る())
        .expect_err("必須骨の欠落は失敗する");
    let error = convert_fbx(
        &bytes,
        &FbxOptions::new(),
        &VrmOptions::new(),
        &書き出し設定を作る(),
    )
    .expect_err("必須骨の欠落は失敗する");
    assert!(
        matches!(error, ConversionError::Export(_)),
        "段階は書き出しである"
    );
    assert_eq!(error.stage(), "VRMの書き出し");
    assert_eq!(format!("{error}"), format!("{direct}"));
    assert!(std::error::Error::source(&error).is_some());
}

#[test]
fn build_modelの中間結果でexport_modelがto_vrmと同じバイト列を返す() {
    let bytes = 書き出し可能なfbxを組み立てる();
    let built = build_model(&bytes, &FbxOptions::new(), &VrmOptions::new())
        .expect("必須骨を持つFBXは中間モデルを作れる");
    assert_eq!(built.report.node_count, 16);
    assert_eq!(built.report.mesh_count, 1);
    let options = 書き出し設定を作る();
    let via_shared = export_model(&built.model, &options).expect("共有入口で書き出せる");
    let direct = built.model.to_vrm(&options).expect("直接書き出せる");
    assert_eq!(
        via_shared, direct,
        "共有入口と直接書き出しは同じバイト列である"
    );
}

impl Props {
    /// 倍精度浮動小数点のプロパティを書く。
    fn d(&mut self, v: f64) {
        self.prop(b'D', &v.to_le_bytes());
    }
}

/// `Deformer` ノードを組み立てる。
fn 変形子(id: i64, 名前: &[u8], 種別: &str, 子ら: Vec<TNode>) -> TNode {
    let mut props = Props::default();
    props.l(id);
    props.s(名前);
    props.s(種別.as_bytes());
    TNode::branch(b"Deformer", props, 子ら)
}

/// ブレンドシェイプの `Shape` ジオメトリを組み立てる。
fn 形状(id: i64, 名前: &[u8], 制御点: &[i32], 差分: &[f64]) -> TNode {
    let mut index_props = Props::default();
    index_props.i_raw(制御点);
    let mut delta_props = Props::default();
    delta_props.d_raw(差分);
    let mut props = Props::default();
    props.l(id);
    props.s(名前);
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

/// ブレンドシェイプチャンネルを組み立てる。初期値はゼロ・重み率なしである。
fn 伝達子(id: i64, 名前: &[u8]) -> TNode {
    let mut deform_props = Props::default();
    deform_props.d(0.0);
    変形子(
        id,
        名前,
        "BlendShapeChannel",
        vec![TNode::leaf(b"DeformPercent", deform_props)],
    )
}

/// 表情チャンネル付きのFBXを組み立てる。必須骨と三角形1枚を持ち、変換から書き出しまで通る。
fn 表情付きfbxを組み立てる(チャンネル名ら: &[&str]) -> Vec<u8> {
    let mut objs: Vec<TNode> = 必須骨モデル列()
        .iter()
        .map(|(id, name, _)| {
            let subtype = if *name == "Owner" { "Mesh" } else { "LimbNode" };
            model(*id, name, subtype)
        })
        .collect();
    objs.push(geometry(
        100,
        "Tri",
        vec![
            vertices_node(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]),
            pvi_node(&[0, 1, -3]),
        ],
    ));
    objs.push(変形子(
        3000,
        b"Blend\x00\x01Deformer",
        "BlendShape",
        vec![],
    ));
    let mut conns_extra = Vec::new();
    for (offset, 名前) in チャンネル名ら.iter().enumerate() {
        let shape_id = 1000 + offset as i64;
        let channel_id = 2000 + offset as i64;
        let mut channel_full = 名前.as_bytes().to_vec();
        channel_full.extend_from_slice(b"\x00\x01SubDeformer");
        let mut shape_full = format!("{名前}-shape").into_bytes();
        shape_full.extend_from_slice(b"\x00\x01Shape");
        objs.push(形状(shape_id, &shape_full, &[0], &[0.5, 0.0, 0.0]));
        objs.push(伝達子(channel_id, &channel_full));
        conns_extra.push(conn(shape_id, channel_id));
        conns_extra.push(conn(channel_id, 3000));
    }
    conns_extra.push(conn(3000, 100));
    let mut pose_props = Props::default();
    pose_props.l(200);
    pose_props.s(b"Bind\x00\x01Pose");
    pose_props.s(b"BindPose");
    let binds: Vec<TNode> = 必須骨モデル列()
        .iter()
        .map(|(id, _, _)| pose_node(*id, &bind_matrix(0.0, 0.0, 0.0)))
        .collect();
    objs.push(TNode::branch(b"Pose", pose_props, binds));
    let mut conns: Vec<TNode> = 必須骨モデル列()
        .iter()
        .filter_map(|(id, _, parent)| parent.map(|parent| conn(*id, parent)))
        .collect();
    conns.push(conn(100, 16));
    conns.push(conn(1, 0));
    conns.extend(conns_extra);
    file(vec![objects(objs), connections(conns)])
}

/// GLBのJSON塊を取り出す。式数の突き合わせに使う。
fn glbのjsonを取り出す(バイト列: &[u8]) -> serde_json::Value {
    assert!(バイト列.len() >= 12, "GLBは12バイトのヘッダを持つ");
    assert_eq!(&バイト列[0..4], b"glTF");
    let json_len = u32::from_le_bytes(
        バイト列[12..16]
            .try_into()
            .expect("JSON塊長は4バイトである"),
    ) as usize;
    let json_bytes = &バイト列[20..20 + json_len];
    serde_json::from_slice(json_bytes).expect("GLBのJSONは読める")
}

#[test]
fn build_modelで部分適用の警告と件数が伝わる() {
    use vrctom::vrm::{VrmExpressionBinding, VrmExpressionProfile};
    let bytes = 表情付きfbxを組み立てる(&["t1", "t2"]);
    let profile = VrmExpressionProfile {
        bindings: vec![
            VrmExpressionBinding {
                expression: "aa".to_owned(),
                target: "t1".to_owned(),
            },
            VrmExpressionBinding {
                expression: "blink".to_owned(),
                target: "t2".to_owned(),
            },
            VrmExpressionBinding {
                expression: "ou".to_owned(),
                target: "無い名前".to_owned(),
            },
        ],
    };
    let built = build_model(
        &bytes,
        &FbxOptions::new(),
        &VrmOptions::new().with_expressions(profile),
    )
    .expect("部分適用は成功する");
    // 適用できた2式だけが件数になり、欠落1件が警告に残る。
    assert_eq!(built.report.expression_count, 2);
    assert_eq!(built.model.expressions.preset.len(), 2);
    assert_eq!(built.report.warnings.len(), 1);
    assert!(built.report.warnings[0].contains("無い名前"));
}

#[test]
fn convert_fbxでglbの実際の式とレポート件数が一致する() {
    use vrctom::vrm::{VrmExpressionBinding, VrmExpressionProfile};
    let bytes = 表情付きfbxを組み立てる(&["t1", "t2"]);
    let profile = VrmExpressionProfile {
        bindings: vec![
            VrmExpressionBinding {
                expression: "aa".to_owned(),
                target: "t1".to_owned(),
            },
            VrmExpressionBinding {
                expression: "blink".to_owned(),
                target: "t2".to_owned(),
            },
            VrmExpressionBinding {
                expression: "ou".to_owned(),
                target: "無い名前".to_owned(),
            },
        ],
    };
    let (vrm_bytes, report) = convert_fbx(
        &bytes,
        &FbxOptions::new(),
        &VrmOptions::new().with_expressions(profile),
        &書き出し設定を作る(),
    )
    .expect("部分適用でも書き出せる");
    // GLBの実際のプリセット式数がレポート件数と一致する。
    let json = glbのjsonを取り出す(&vrm_bytes);
    let preset = &json["extensions"]["VRMC_vrm"]["expressions"]["preset"];
    let glb_count = preset.as_object().map(|map| map.len()).unwrap_or(0);
    assert_eq!(glb_count, 2, "GLBの式は適用できた分だけである");
    assert_eq!(
        report.expression_count, glb_count,
        "レポート件数はGLBと一致する"
    );
    assert_eq!(report.warnings.len(), 1);
    assert!(report.warnings[0].contains("無い名前"));
}
