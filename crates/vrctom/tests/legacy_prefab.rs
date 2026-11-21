//! Unity 2018.2以前のPrefab表現と旧方式の取り込みfileIDの契約テスト（公開 API のみを使う）。
//!
//! 手組みの最小データで、旧版の欄名の読み替えと、`fileIDToRecycleName` から
//! 新方式（XXH64）のfileIDへの書き換えを固定する。

#![cfg(not(target_arch = "wasm32"))]

use vrctom::fbx::{FbxFile, FbxOptions};
use vrctom::unity::{
    AssetMeta, ParseOptions, SerializedFile, TypedObject, skinned_mesh_renderer_file_id,
    transform_file_id,
};
use vrctom::vrm::modernize_legacy_import_ids;

// テスト名は日本語で書く（テストは仕様・契約であるため）。

/// 入力FBXのGUID。
const FBX_GUID: &str = "a4bd84bc66f71754582f32c727ccb528";

/// 最小FBXの節点。版7400の13バイト見出しで書く。
struct 節点 {
    名前: &'static [u8],
    プロパティ: Vec<u8>,
    個数: u32,
    子: Vec<節点>,
}

impl 節点 {
    fn new(名前: &'static [u8]) -> Self {
        Self {
            名前,
            プロパティ: Vec::new(),
            個数: 0,
            子: Vec::new(),
        }
    }

    fn l(mut self, 値: i64) -> Self {
        self.プロパティ.push(b'L');
        self.プロパティ.extend_from_slice(&値.to_le_bytes());
        self.個数 += 1;
        self
    }

    fn s(mut self, 値: &[u8]) -> Self {
        self.プロパティ.push(b'S');
        self.プロパティ
            .extend_from_slice(&(値.len() as u32).to_le_bytes());
        self.プロパティ.extend_from_slice(値);
        self.個数 += 1;
        self
    }

    /// 無圧縮の配列プロパティを書く。
    fn array(mut self, code: u8, count: u32, payload: &[u8]) -> Self {
        self.プロパティ.push(code);
        self.プロパティ.extend_from_slice(&count.to_le_bytes());
        self.プロパティ.extend_from_slice(&0u32.to_le_bytes());
        self.プロパティ
            .extend_from_slice(&(payload.len() as u32).to_le_bytes());
        self.プロパティ.extend_from_slice(payload);
        self.個数 += 1;
        self
    }

    fn 子付き(mut self, 子: Vec<節点>) -> Self {
        self.子 = 子;
        self
    }

    fn 書く(&self, buf: &mut Vec<u8>) {
        let start = buf.len();
        buf.extend_from_slice(&[0u8; 13]);
        buf.extend_from_slice(self.名前);
        buf.extend_from_slice(&self.プロパティ);
        for 子 in &self.子 {
            子.書く(buf);
        }
        if !self.子.is_empty() {
            buf.extend_from_slice(&[0u8; 13]);
        }
        let end = buf.len() as u32;
        buf[start..start + 4].copy_from_slice(&end.to_le_bytes());
        buf[start + 4..start + 8].copy_from_slice(&self.個数.to_le_bytes());
        buf[start + 8..start + 12].copy_from_slice(&(self.プロパティ.len() as u32).to_le_bytes());
        buf[start + 12] = self.名前.len() as u8;
    }
}

fn 接続(子: i64, 親: i64) -> 節点 {
    節点::new(b"C").s(b"OO").l(子).l(親)
}

/// `Armature`（空）と `Body`（三角形1枚のメッシュ）を根に持つFBXを作る。
fn 最小fbx() -> Vec<u8> {
    let mut vertices = Vec::new();
    for value in [0.0f64, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0] {
        vertices.extend_from_slice(&value.to_le_bytes());
    }
    let mut indexes = Vec::new();
    for value in [0i32, 1, -3] {
        indexes.extend_from_slice(&value.to_le_bytes());
    }
    let objects = 節点::new(b"Objects").子付き(vec![
        節点::new(b"Model")
            .l(1)
            .s(b"Armature\x00\x01Model")
            .s(b"Null"),
        節点::new(b"Model").l(2).s(b"Body\x00\x01Model").s(b"Mesh"),
        節点::new(b"Geometry")
            .l(3)
            .s(b"Body\x00\x01Geometry")
            .s(b"Mesh")
            .子付き(vec![
                節点::new(b"Vertices").array(b'd', 9, &vertices),
                節点::new(b"PolygonVertexIndex").array(b'i', 3, &indexes),
            ]),
    ]);
    let connections = 節点::new(b"Connections").子付き(vec![接続(1, 0), 接続(2, 0), 接続(3, 2)]);
    let mut buf = b"Kaydara FBX Binary  \x00\x1a\x00".to_vec();
    buf.extend_from_slice(&7400u32.to_le_bytes());
    objects.書く(&mut buf);
    connections.書く(&mut buf);
    buf.extend_from_slice(&[0u8; 13]);
    buf
}

/// 旧方式の取り込みfileID表を持つ `.fbx.meta`。
fn 旧版メタ() -> AssetMeta {
    let text = format!(
        "fileFormatVersion: 2\nguid: {FBX_GUID}\ntimeCreated: 1519744173\nlicenseType: Free\nModelImporter:\n  serializedVersion: 19\n  fileIDToRecycleName:\n    100000: Armature\n    100002: Body\n    100004: //RootNode\n    100006: Ghost\n    400000: Armature\n    400002: Body\n    400004: //RootNode\n    4300000: Body\n    9500000: //RootNode\n    13700000: Body\n"
    );
    AssetMeta::parse(text.as_bytes()).expect("旧版metaは読める")
}

/// Unity 2018.2以前の書式で、入力FBXを置いたシーンを作る。
fn 旧版シーン() -> String {
    format!(
        "%YAML 1.1\n%TAG !u! tag:unity3d.com,2011:\n\
--- !u!1001 &100\n\
Prefab:\n\
\x20 m_ObjectHideFlags: 0\n\
\x20 serializedVersion: 2\n\
\x20 m_Modification:\n\
\x20   m_TransformParent: {{fileID: 0}}\n\
\x20   m_Modifications:\n\
\x20   - target: {{fileID: 13700000, guid: {FBX_GUID}, type: 3}}\n\
\x20     propertyPath: m_Materials.Array.data[0]\n\
\x20     value: \n\
\x20     objectReference: {{fileID: 2100000, guid: 4cc8ad43f169357408cb1aecb6054a86, type: 2}}\n\
\x20   - target: {{fileID: 400004, guid: {FBX_GUID}, type: 3}}\n\
\x20     propertyPath: m_LocalPosition.x\n\
\x20     value: 0\n\
\x20     objectReference: {{fileID: 0}}\n\
\x20   - target: {{fileID: 100006, guid: {FBX_GUID}, type: 3}}\n\
\x20     propertyPath: m_IsActive\n\
\x20     value: 0\n\
\x20     objectReference: {{fileID: 0}}\n\
\x20   - target: {{fileID: 9500000, guid: {FBX_GUID}, type: 3}}\n\
\x20     propertyPath: m_Enabled\n\
\x20     value: 1\n\
\x20     objectReference: {{fileID: 0}}\n\
\x20   m_RemovedComponents: []\n\
\x20 m_ParentPrefab: {{fileID: 100100000, guid: {FBX_GUID}, type: 3}}\n\
\x20 m_IsPrefabParent: 0\n\
--- !u!137 &200 stripped\n\
SkinnedMeshRenderer:\n\
\x20 m_PrefabParentObject: {{fileID: 13700000, guid: {FBX_GUID}, type: 3}}\n\
\x20 m_PrefabInternal: {{fileID: 100}}\n"
    )
}

#[test]
fn 旧版のprefab表現を新しい欄名で読める() {
    let file =
        SerializedFile::parse(旧版シーン().as_bytes(), &ParseOptions::new()).expect("読める");
    let instance = file.document(100).expect("インスタンスがある");
    assert_eq!(instance.class_name(), "PrefabInstance");
    match instance.decode().expect("型付きで読める") {
        TypedObject::PrefabInstance(instance) => {
            assert_eq!(
                instance.source.guid.as_ref().expect("GUIDがある").as_str(),
                FBX_GUID
            );
            assert_eq!(instance.modifications().expect("改変を読める").len(), 4);
        }
        other => panic!("PrefabInstanceでない: {other:?}"),
    }
    let stripped = file.document(200).expect("stripped文書がある");
    assert!(stripped.stripped());
    assert!(stripped.raw().get("m_CorrespondingSourceObject").is_some());
    assert_eq!(
        stripped
            .raw()
            .get("m_PrefabInstance")
            .and_then(|value| value.get("fileID"))
            .and_then(|value| value.as_i64()),
        Some(100)
    );
    assert!(stripped.raw().get("m_PrefabParentObject").is_none());
}

#[test]
fn 旧版のprefab資産自身の印は除きインスタンスとみなさない() {
    let text = "%YAML 1.1\n%TAG !u! tag:unity3d.com,2011:\n\
--- !u!1001 &100100000\n\
Prefab:\n\
\x20 m_ObjectHideFlags: 1\n\
\x20 m_ParentPrefab: {fileID: 0}\n\
\x20 m_RootGameObject: {fileID: 1000}\n\
\x20 m_IsPrefabParent: 1\n\
--- !u!1 &1000\n\
GameObject:\n\
\x20 m_ObjectHideFlags: 0\n\
\x20 m_PrefabParentObject: {fileID: 0}\n\
\x20 m_PrefabInternal: {fileID: 100100000}\n\
\x20 serializedVersion: 5\n\
\x20 m_Component: []\n\
\x20 m_Layer: 0\n\
\x20 m_Name: Root\n\
\x20 m_IsActive: 1\n";
    let file = SerializedFile::parse(text.as_bytes(), &ParseOptions::new()).expect("読める");
    assert_eq!(file.documents().len(), 1);
    let root = file.document(1000).expect("ゲームオブジェクトがある");
    assert_eq!(
        root.raw()
            .get("m_PrefabAsset")
            .and_then(|value| value.get("fileID"))
            .and_then(|value| value.as_i64()),
        Some(100100000)
    );
    assert_eq!(
        root.raw()
            .get("m_PrefabInstance")
            .and_then(|value| value.get("fileID"))
            .and_then(|value| value.as_i64()),
        Some(0)
    );
    // 資産自身の印はインスタンスとして型付き読み取りに現れない。
    assert!(
        file.documents()
            .iter()
            .all(|document| document.class_id() != 1001)
    );
}

#[test]
fn 旧方式の取り込みidを新方式へ読み替える() {
    let bytes = 最小fbx();
    let fbx = FbxFile::parse(&bytes, &FbxOptions::new()).expect("最小FBXは読める");
    let meta = 旧版メタ();
    let recycle = meta.legacy_recycle_names();
    assert_eq!(recycle.len(), 10);
    let mut file =
        SerializedFile::parse(旧版シーン().as_bytes(), &ParseOptions::new()).expect("読める");
    let report = modernize_legacy_import_ids(&mut file, FBX_GUID, &recycle, &fbx).expect("写せる");
    // 描画設定（改変と対応先）とルートのTransformの3件を書き換える。
    assert_eq!(report.rewritten, 3);
    assert_eq!(report.unresolved, vec!["1:Ghost".to_owned()]);

    let TypedObject::PrefabInstance(instance) =
        file.document(100).expect("ある").decode().expect("読める")
    else {
        panic!("PrefabInstanceでない");
    };
    let targets: Vec<i64> = instance
        .modifications()
        .expect("改変を読める")
        .iter()
        .map(|modification| modification.target.file_id)
        .collect();
    assert_eq!(
        targets,
        vec![
            skinned_mesh_renderer_file_id("Body"),
            transform_file_id(""),
            // 見つからない名前とAnimator（95）は書き換えない。
            100006,
            9500000,
        ]
    );
    let corresponding = file
        .document(200)
        .expect("ある")
        .raw()
        .get("m_CorrespondingSourceObject")
        .and_then(|value| value.get("fileID"))
        .and_then(|value| value.as_i64());
    assert_eq!(corresponding, Some(skinned_mesh_renderer_file_id("Body")));
}

#[test]
fn 取り込みid表が無ければ何もしない() {
    let bytes = 最小fbx();
    let fbx = FbxFile::parse(&bytes, &FbxOptions::new()).expect("最小FBXは読める");
    let mut file =
        SerializedFile::parse(旧版シーン().as_bytes(), &ParseOptions::new()).expect("読める");
    let report = modernize_legacy_import_ids(&mut file, FBX_GUID, &[], &fbx).expect("写せる");
    assert_eq!(report.rewritten, 0);
    assert!(report.unresolved.is_empty());
}
