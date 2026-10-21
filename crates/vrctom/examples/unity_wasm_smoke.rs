//! wasm実行検証用の煙試験。
//!
//! Native/Web共通の同期 `&[u8]` 入力API（パース・参照・階層）を
//! wasm上で動かす。`--all-targets` の検査対象になるため、警告なく通ること。
//! 実行は `wasm-bindgen` で生成した glue を node から呼ぶ。

use vrctom::unity::{
    AssetIndex, AssetMeta, Guid, IndexOptions, ObjectRef, ParseOptions, ReferenceLocation,
    SerializedFile, TypedObject,
};

/// 最小prefab（親子2世代）。
const PREFAB: &str = "%YAML 1.1\n%TAG !u! tag:unity3d.com,2011:\n--- !u!1 &100\nGameObject:\n  m_ObjectHideFlags: 0\n  m_CorrespondingSourceObject: {fileID: 0}\n  m_PrefabInstance: {fileID: 0}\n  m_PrefabAsset: {fileID: 0}\n  serializedVersion: 6\n  m_Component:\n  - component: {fileID: 200}\n  m_Layer: 0\n  m_Name: Root\n  m_TagString: Untagged\n  m_Icon: {fileID: 0}\n  m_NavMeshLayer: 0\n  m_StaticEditorFlags: 0\n  m_IsActive: 1\n--- !u!4 &200\nTransform:\n  m_ObjectHideFlags: 0\n  m_CorrespondingSourceObject: {fileID: 0}\n  m_PrefabInstance: {fileID: 0}\n  m_PrefabAsset: {fileID: 0}\n  m_GameObject: {fileID: 100}\n  serializedVersion: 2\n  m_LocalRotation: {x: 0, y: 0, z: 0, w: 1}\n  m_LocalPosition: {x: 0, y: 0, z: 0}\n  m_LocalScale: {x: 1, y: 1, z: 1}\n  m_ConstrainProportionsScale: 0\n  m_Children:\n  - {fileID: 201}\n  m_Father: {fileID: 0}\n  m_LocalEulerAnglesHint: {x: 0, y: 0, z: 0}\n--- !u!1 &101\nGameObject:\n  m_ObjectHideFlags: 0\n  m_CorrespondingSourceObject: {fileID: 0}\n  m_PrefabInstance: {fileID: 0}\n  m_PrefabAsset: {fileID: 0}\n  serializedVersion: 6\n  m_Component:\n  - component: {fileID: 201}\n  m_Layer: 0\n  m_Name: Child\n  m_TagString: Untagged\n  m_Icon: {fileID: 0}\n  m_NavMeshLayer: 0\n  m_StaticEditorFlags: 0\n  m_IsActive: 1\n--- !u!4 &201\nTransform:\n  m_ObjectHideFlags: 0\n  m_CorrespondingSourceObject: {fileID: 0}\n  m_PrefabInstance: {fileID: 0}\n  m_PrefabAsset: {fileID: 0}\n  m_GameObject: {fileID: 101}\n  serializedVersion: 2\n  m_LocalRotation: {x: 0, y: 0, z: 0, w: 1}\n  m_LocalPosition: {x: 1, y: 2, z: 3}\n  m_LocalScale: {x: 1, y: 1, z: 1}\n  m_ConstrainProportionsScale: 0\n  m_Children: []\n  m_Father: {fileID: 200}\n  m_LocalEulerAnglesHint: {x: 0, y: 0, z: 0}\n";

/// 最小マテリアル（同名texEnvの順序保持つき）。
const MATERIAL: &str = "%YAML 1.1\n%TAG !u! tag:unity3d.com,2011:\n--- !u!21 &2100000\nMaterial:\n  serializedVersion: 8\n  m_ObjectHideFlags: 0\n  m_CorrespondingSourceObject: {fileID: 0}\n  m_PrefabInstance: {fileID: 0}\n  m_PrefabAsset: {fileID: 0}\n  m_Name: Green\n  m_Shader: {fileID: 0}\n  m_Parent: {fileID: 0}\n  m_ModifiedSerializedProperties: 0\n  m_ValidKeywords: []\n  m_InvalidKeywords: []\n  m_LightmapFlags: 4\n  m_EnableInstancingVariants: 0\n  m_DoubleSidedGI: 0\n  m_CustomRenderQueue: -1\n  stringTagMap: {}\n  disabledShaderPasses: []\n  m_LockedProperties:\n  m_SavedProperties:\n    serializedVersion: 3\n    m_TexEnvs:\n    - _MainTex:\n        m_Texture: {fileID: 0}\n        m_Scale: {x: 1, y: 1}\n        m_Offset: {x: 0, y: 0}\n    - _MainTex:\n        m_Texture: {fileID: 2800000, guid: 27f6bef13af40aa4bab1983bb1711cd0, type: 3}\n        m_Scale: {x: 2, y: 2}\n        m_Offset: {x: 0.5, y: 0}\n    m_Ints: []\n    m_Floats:\n    - _Cutoff: 0.5\n    m_Colors:\n    - _Color: {r: 0, g: 1, b: 0, a: 1}\n";

/// 最小テクスチャmeta。
const TEXTURE_META: &str = "fileFormatVersion: 2\nguid: 27f6bef13af40aa4bab1983bb1711cd0\nTextureImporter:\n  externalObjects: {}\n  textureType: 0\n";

/// パース・参照・階層の煙試験を行い、結果概要を返す。
///
/// 失敗時はパニックする（node側で例外として見える）。
fn smoke() -> String {
    // GUIDの検証（大文字の統一と全ゼロの保持）。
    let guid = Guid::parse("27F6BEF13AF40AA4BAB1983BB1711CD0").expect("大文字は読める");
    assert_eq!(guid.as_str(), "27f6bef13af40aa4bab1983bb1711cd0");
    let zero = Guid::parse("00000000000000000000000000000000").expect("全ゼロは有効である");
    assert_eq!(zero.as_str(), "00000000000000000000000000000000");

    // 索引の構築と所在特定。
    let mut index = AssetIndex::new();
    let meta = AssetMeta::parse(TEXTURE_META.as_bytes()).expect("metaは読める");
    index
        .insert("Assets/Tex.png", meta)
        .expect("挿入は成功する");
    let source = Guid::parse("11111111111111111111111111111111").expect("正当である");
    let local = ObjectRef {
        file_id: 7,
        guid: None,
        type_hint: None,
    };
    assert!(matches!(
        index.locate(&source, &local),
        Err(error) if error.kind() == vrctom::unity::UnityErrorKind::MissingObject
    ));
    let external = ObjectRef {
        file_id: 2800000,
        guid: Some(guid.clone()),
        type_hint: Some(3),
    };
    let (path, file_id) = match index.locate(&source, &external).expect("解決は成功する") {
        ReferenceLocation::Indexed { asset, file_id } => (asset.path.clone(), file_id),
        other => panic!("索引付きになる: {other:?}"),
    };
    assert_eq!(path, "Assets/Tex.png");
    assert_eq!(file_id, 2800000);

    // prefabのパース・型付き読み取り・階層。
    let file = SerializedFile::parse(PREFAB.as_bytes(), &ParseOptions::new()).expect("読める");
    assert_eq!(file.documents().len(), 4);
    let TypedObject::GameObject(root) = file.document(100).expect("ある").decode().expect("読める")
    else {
        panic!("GameObjectになる");
    };
    assert_eq!(root.name, "Root");
    assert!(root.active);
    let TypedObject::Transform(child) = file.document(201).expect("ある").decode().expect("読める")
    else {
        panic!("Transformになる");
    };
    assert_eq!(child.position, [1.0, 2.0, 3.0]);
    let hierarchy = file.local_hierarchy().expect("階層は組める");
    assert_eq!(hierarchy.roots().len(), 1);
    assert_eq!(hierarchy.len(), 2);
    assert_eq!(hierarchy.roots()[0].children[0].name, "Child");

    // マテリアルの順序保持。
    let file = SerializedFile::parse(MATERIAL.as_bytes(), &ParseOptions::new()).expect("読める");
    let TypedObject::Material(material) = file
        .document(2100000)
        .expect("ある")
        .decode()
        .expect("読める")
    else {
        panic!("Materialになる");
    };
    assert_eq!(material.tex_envs.len(), 2);
    assert_eq!(material.tex_envs[0].name, "_MainTex");
    assert_eq!(material.tex_envs[1].name, "_MainTex");

    let _ = IndexOptions::new();
    format!("ok docs=4 roots=1 nodes=2 texenvs=2 path={path} file={file_id}")
}

/// wasm用の入口。結果概要の文字列を返す。
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn run_unity_smoke() -> String {
    smoke()
}

fn main() {
    // Native側の実行は表示で確かめる。wasm側は `run_unity_smoke` を使う。
    println!("{}", smoke());
}
