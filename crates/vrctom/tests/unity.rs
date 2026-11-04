//! Unity保存データ読み取り層の契約テスト（公開 API のみを使う）。
//!
//! 生成・手書きの最小データで完結し、実fixtureを必須にしない。
//! 実fixtureを使う計測は `VRCTOM_UNITY_FIXTURE_DIR` が指す展開済みディレクトリだけを
//! 読み、未設定なら跳ばす。

#![cfg(not(target_arch = "wasm32"))]

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;

use vrctom::unity::{
    AssetIndex, AssetKind, AssetMeta, FileId, Guid, HierarchyOptions, IndexOptions, ObjectRef,
    ParseOptions, ReferenceLocation, SerializedFile, TypedObject, UnityErrorKind,
};

// テスト名は日本語で書く（テストは仕様・契約であるため）。

/// テスト用GUID（32桁hex）を生成する。
fn guid(n: u128) -> String {
    format!("{n:032x}")
}

/// テスト用GUIDをパースする。
fn parsed_guid(n: u128) -> Guid {
    Guid::parse(&guid(n)).expect("テスト用GUIDは正当である")
}

/// 最小のフォルダmetaを組み立てる。
fn folder_meta(n: u128) -> Vec<u8> {
    format!(
        "fileFormatVersion: 2\nguid: {}\nfolderAsset: yes\nDefaultImporter:\n  externalObjects: {{}}\n  userData: \n  assetBundleName: \n  assetBundleVariant: \n",
        guid(n)
    )
    .into_bytes()
}

/// 最小のファイルmetaを組み立てる。
fn file_meta(n: u128, importer: &str, body: &str) -> Vec<u8> {
    format!(
        "fileFormatVersion: 2\nguid: {}\n{importer}:\n{body}",
        guid(n)
    )
    .into_bytes()
}

/// テクスチャmetaの本文。
const TEXTURE_IMPORTER_BODY: &str =
    "  externalObjects: {}\n  textureType: 0\n  sRGBTexture: 1\n  userData: \n";

/// prefabmetaの本文。
const PREFAB_IMPORTER_BODY: &str = "  externalObjects: {}\n  userData: \n";

/// Unity YAMLの先頭directive。
const YAML_DIRECTIVES: &str = "%YAML 1.1\n%TAG !u! tag:unity3d.com,2011:\n";

/// 1文書を組み立てる。
fn document(
    class_id: impl std::fmt::Display,
    file_id: impl std::fmt::Display,
    stripped: bool,
    class_name: &str,
    body: &str,
) -> String {
    let marker = if stripped { " stripped" } else { "" };
    format!("--- !u!{class_id} &{file_id}{marker}\n{class_name}:\n{body}")
}

/// ゲームオブジェクト1件分の本文。
fn game_object_body(name: &str, components: &[FileId], active: i64) -> String {
    let mut out = String::from(
        "  m_ObjectHideFlags: 0\n  m_CorrespondingSourceObject: {fileID: 0}\n  m_PrefabInstance: {fileID: 0}\n  m_PrefabAsset: {fileID: 0}\n  serializedVersion: 6\n",
    );
    if components.is_empty() {
        out.push_str("  m_Component: []\n");
    } else {
        out.push_str("  m_Component:\n");
        for component in components {
            out.push_str(&format!("  - component: {{fileID: {component}}}\n"));
        }
    }
    out.push_str(&format!(
        "  m_Layer: 0\n  m_Name: {name}\n  m_TagString: Untagged\n  m_Icon: {{fileID: 0}}\n  m_NavMeshLayer: 0\n  m_StaticEditorFlags: 0\n  m_IsActive: {active}\n"
    ));
    out
}

/// Transform1件分の本文。
fn transform_body(game_object: FileId, father: &str, children: &[FileId]) -> String {
    let mut out = format!(
        "  m_ObjectHideFlags: 0\n  m_CorrespondingSourceObject: {{fileID: 0}}\n  m_PrefabInstance: {{fileID: 0}}\n  m_PrefabAsset: {{fileID: 0}}\n  m_GameObject: {{fileID: {game_object}}}\n  serializedVersion: 2\n  m_LocalRotation: {{x: 0, y: 0, z: 0, w: 1}}\n  m_LocalPosition: {{x: 0, y: 0, z: 0}}\n  m_LocalScale: {{x: 1, y: 1, z: 1}}\n  m_ConstrainProportionsScale: 0\n"
    );
    if children.is_empty() {
        out.push_str("  m_Children: []\n");
    } else {
        out.push_str("  m_Children:\n");
        for child in children {
            out.push_str(&format!("  - {{fileID: {child}}}\n"));
        }
    }
    out.push_str(&format!(
        "  m_Father: {father}\n  m_LocalEulerAnglesHint: {{x: 0, y: 0, z: 0}}\n"
    ));
    out
}

/// 親子2世代の最小prefabを組み立てる。
fn two_level_prefab() -> String {
    [
        YAML_DIRECTIVES.to_owned(),
        document(
            1,
            100,
            false,
            "GameObject",
            &game_object_body("Root", &[200], 1),
        ),
        document(
            4,
            200,
            false,
            "Transform",
            &transform_body(100, "{fileID: 0}", &[201]),
        ),
        document(
            1,
            101,
            false,
            "GameObject",
            &game_object_body("Child", &[201], 1),
        ),
        document(
            4,
            201,
            false,
            "Transform",
            &transform_body(101, "{fileID: 200}", &[]),
        ),
    ]
    .join("")
}

#[test]
fn 代表importerと未知importerとフォルダmetaを読める() {
    // TextureImporterの代表設定を保持する。
    let meta = AssetMeta::parse(&file_meta(1, "TextureImporter", TEXTURE_IMPORTER_BODY))
        .expect("代表importerは読める");
    assert_eq!(meta.guid().as_str(), guid(1).as_str());
    assert!(!meta.is_folder());
    assert_eq!(meta.importer().name, "TextureImporter");
    assert!(
        meta.external_object_remaps()
            .expect("空の置換表は読める")
            .is_empty()
    );

    // 未知のimporterも捨てずに保持する。
    let meta = AssetMeta::parse(&file_meta(2, "MyCustomImporter", "  foo: 1\n"))
        .expect("未知importerは読める");
    assert_eq!(meta.importer().name, "MyCustomImporter");

    // フォルダmetaはフォルダ識別を持つ。
    let meta = AssetMeta::parse(&folder_meta(3)).expect("フォルダmetaは読める");
    assert!(meta.is_folder());
    assert_eq!(meta.importer().name, "DefaultImporter");
}

#[test]
fn modelimporterの置換表を元の型名付きで保持する() {
    let body = r#"  serializedVersion: 22200
  externalObjects:
  - first:
      type: UnityEngine:Material
      assembly: UnityEngine.CoreModule
      name: Body
    second: {fileID: 2100000, guid: 9ef94258554e06b4ab03fbe9fc914c5c, type: 2}
  materials:
    materialImportMode: 2
"#;
    let meta =
        AssetMeta::parse(&file_meta(9, "ModelImporter", body)).expect("ModelImporterは読める");
    assert!(!meta.is_folder());
    let remaps = meta.external_object_remaps().expect("置換表は読める");
    assert_eq!(remaps.len(), 1);
    assert_eq!(remaps[0].original_type, "UnityEngine:Material");
    assert_eq!(remaps[0].original_assembly, "UnityEngine.CoreModule");
    assert_eq!(remaps[0].original_name, "Body");
    assert_eq!(remaps[0].replacement.file_id, 2100000);
    assert_eq!(
        remaps[0]
            .replacement
            .guid
            .as_ref()
            .expect("GUIDがある")
            .as_str(),
        "9ef94258554e06b4ab03fbe9fc914c5c"
    );
}

#[test]
fn guidの大文字小文字と形式不良を区別する() {
    // 大文字は受け付け、小文字に統一する。
    let upper = guid(0xabc).to_ascii_uppercase();
    let parsed = Guid::parse(&upper).expect("大文字GUIDは読める");
    assert_eq!(parsed.as_str(), upper.to_ascii_lowercase().as_str());

    // 全ゼロも有効値として保持する。
    let zero = Guid::parse("00000000000000000000000000000000").expect("全ゼロは有効である");
    assert_eq!(zero.as_str(), "00000000000000000000000000000000");

    // 形式不良は拒否する。
    for bad in [
        "",
        "xyz",
        "0123",
        "4c935a71144654748983642acc5c826",
        "zz235a71144654748983642acc5c8265",
    ] {
        assert!(Guid::parse(bad).is_err(), "形式不良は拒否する: {bad:?}");
    }
    // meta内の形式不良はInvalidMetadataになる。
    let bad =
        b"fileFormatVersion: 2\nguid: xyz\nDefaultImporter:\n  externalObjects: {}\n".as_slice();
    let error = AssetMeta::parse(bad).expect_err("形式不良GUIDは失敗する");
    assert_eq!(error.kind(), UnityErrorKind::InvalidMetadata);
}

#[test]
fn 数値化されたguidは復元不能として拒否し黙って壊さない() {
    // 短い数値は数値として読めた上で復元不能になる。
    let bad =
        b"fileFormatVersion: 2\nguid: 123\nDefaultImporter:\n  externalObjects: {}\n".as_slice();
    let error = AssetMeta::parse(bad).expect_err("数値GUIDは失敗する");
    assert_eq!(error.kind(), UnityErrorKind::InvalidMetadata);

    // 精度を失わないこと: 大きなfileIDは整数のまま正確に読む。
    let yaml = format!(
        "{}{}",
        YAML_DIRECTIVES,
        document(
            114,
            1,
            false,
            "MonoBehaviour",
            "  m_GameObject: {fileID: 0}\n  m_Enabled: 1\n  m_Script: {fileID: 8008639217611773955, guid: 9ef94258554e06b4ab03fbe9fc914c5c, type: 3}\n  m_Name: Probe\n  m_EditorClassIdentifier: \n"
        )
    );
    let file = SerializedFile::parse(yaml.as_bytes(), &ParseOptions::new()).expect("読める");
    let TypedObject::MonoBehaviour(behaviour) =
        file.document(1).expect("ある").decode().expect("読める")
    else {
        panic!("MonoBehaviourになる");
    };
    assert_eq!(behaviour.script.file_id, 8008639217611773955);
}

#[test]
fn 複数文書と同クラス複数と改行種別とbomを読める() {
    let prefab = two_level_prefab();
    // CRLFでも読める。
    let crlf = prefab.replace('\n', "\r\n");
    let file = SerializedFile::parse(crlf.as_bytes(), &ParseOptions::new()).expect("CRLFは読める");
    assert_eq!(file.documents().len(), 4);
    // BOM付きでも読める。
    let bom = format!("\u{feff}{prefab}");
    let file = SerializedFile::parse(bom.as_bytes(), &ParseOptions::new()).expect("BOMは読める");
    assert_eq!(file.documents().len(), 4);
    // 同一クラスの複数文書を区別する。
    let games: Vec<_> = file
        .documents()
        .iter()
        .filter(|doc| doc.class_id() == 1)
        .collect();
    assert_eq!(games.len(), 2);
    assert_eq!(file.document(100).expect("ある").class_name(), "GameObject");
    assert!(file.document(999).is_none());
}

#[test]
fn 正負fileidと範囲の内外を区別する() {
    // i64の最小最大は読める。
    let yaml = format!(
        "{}{}{}",
        YAML_DIRECTIVES,
        document(
            1,
            i64::MAX,
            false,
            "GameObject",
            &game_object_body("Max", &[], 1)
        ),
        document(
            1,
            i64::MIN,
            false,
            "GameObject",
            &game_object_body("Min", &[], 0)
        ),
    );
    let file =
        SerializedFile::parse(yaml.as_bytes(), &ParseOptions::new()).expect("範囲内は読める");
    assert_eq!(file.documents().len(), 2);

    // ヘッダの範囲外はInvalidYamlになる（浮動小数点経由では読まない）。
    for bad_id in ["9223372036854775808", "-9223372036854775809"] {
        let yaml = format!(
            "{}{}",
            YAML_DIRECTIVES,
            document(
                1,
                bad_id,
                false,
                "GameObject",
                &game_object_body("X", &[], 1)
            )
        );
        let error = SerializedFile::parse(yaml.as_bytes(), &ParseOptions::new())
            .expect_err("範囲外は失敗する");
        assert_eq!(
            error.kind(),
            UnityErrorKind::InvalidYaml,
            "fileID: {bad_id}"
        );
    }
    // 本文の浮動小数点fileIDは参照として拒否する。
    let yaml = format!(
        "{}{}",
        YAML_DIRECTIVES,
        document(
            4,
            200,
            false,
            "Transform",
            &transform_body(100, "{fileID: 1.5}", &[])
        )
    );
    let file =
        SerializedFile::parse(yaml.as_bytes(), &ParseOptions::new()).expect("枠自体は読める");
    let error = file
        .document(200)
        .expect("ある")
        .decode()
        .expect_err("小数は失敗する");
    assert_eq!(error.kind(), UnityErrorKind::InvalidReference);
}

#[test]
fn 数値と空の名前を読める() {
    // Unity送出にある数値名と空名（実データ由来の形状）。
    let yaml = format!(
        "{}{}{}",
        YAML_DIRECTIVES,
        document(
            1,
            100,
            false,
            "GameObject",
            "  m_Component: []\n  m_Name: 6\n  m_IsActive: 1\n"
        ),
        document(
            114,
            101,
            false,
            "MonoBehaviour",
            "  m_GameObject: {fileID: 100}\n  m_Enabled: 1\n  m_Script: {fileID: 1}\n  m_Name:\n  m_EditorClassIdentifier: \n",
        ),
    );
    let file = SerializedFile::parse(yaml.as_bytes(), &ParseOptions::new()).expect("読める");
    let TypedObject::GameObject(game) = file.document(100).expect("ある").decode().expect("読める")
    else {
        panic!("GameObjectになる");
    };
    assert_eq!(game.name, "6");
    let TypedObject::MonoBehaviour(behaviour) =
        file.document(101).expect("ある").decode().expect("読める")
    else {
        panic!("MonoBehaviourになる");
    };
    assert_eq!(behaviour.name, "");
    // 欄自体の欠落は埋めない。
    let yaml = format!(
        "{}{}",
        YAML_DIRECTIVES,
        document(
            1,
            102,
            false,
            "GameObject",
            "  m_Component: []\n  m_IsActive: 1\n"
        ),
    );
    let file =
        SerializedFile::parse(yaml.as_bytes(), &ParseOptions::new()).expect("枠自体は読める");
    let error = file
        .document(102)
        .expect("ある")
        .decode()
        .expect_err("欠落は失敗する");
    assert_eq!(error.kind(), UnityErrorKind::TypeMismatch);
}

#[test]
fn strippedと未知クラスと未知欄を読める() {
    let yaml = format!(
        "{}{}{}",
        YAML_DIRECTIVES,
        document(
            137,
            5,
            true,
            "SkinnedMeshRenderer",
            "  m_CorrespondingSourceObject: {fileID: 1630794972795428178, guid: 9ef94258554e06b4ab03fbe9fc914c5c, type: 3}\n  m_PrefabInstance: {fileID: 896484902725815322}\n  m_PrefabAsset: {fileID: 0}\n"
        ),
        document(99999, 6, false, "MysteryWidget", "  custom: 1\n"),
    );
    let file = SerializedFile::parse(yaml.as_bytes(), &ParseOptions::new()).expect("読める");
    assert!(file.document(5).expect("ある").stripped());
    let TypedObject::Stripped(link) = file.document(5).expect("ある").decode().expect("読める")
    else {
        panic!("Strippedになる");
    };
    assert_eq!(link.corresponding_source.file_id, 1630794972795428178);
    // 未知クラスはUnknownになり、元データは残る。
    let unknown = file.document(6).expect("ある");
    let TypedObject::Unknown = unknown.decode().expect("読める") else {
        panic!("Unknownになる");
    };
    assert!(
        unknown
            .raw()
            .as_mapping()
            .expect("写像である")
            .contains_key("custom")
    );

    // classIDとクラス名の明白な不一致は拒否する。
    let yaml = format!(
        "{}{}",
        YAML_DIRECTIVES,
        document(
            1,
            7,
            false,
            "Transform",
            &transform_body(7, "{fileID: 0}", &[])
        )
    );
    let file =
        SerializedFile::parse(yaml.as_bytes(), &ParseOptions::new()).expect("枠自体は読める");
    let error = file
        .document(7)
        .expect("ある")
        .decode()
        .expect_err("不一致は失敗する");
    assert_eq!(error.kind(), UnityErrorKind::TypeMismatch);
}

#[test]
fn 複数行flowmappingとblockscalar内のヘッダ風文字列を読める() {
    // 実データにある複数行flow mapping（行末で折り返す参照）。
    let yaml = format!(
        "{}{}",
        YAML_DIRECTIVES,
        document(
            114,
            1,
            false,
            "MonoBehaviour",
            "  m_GameObject: {fileID: 0}\n  m_Enabled: 1\n  m_Script: {fileID: -1888410255, guid: 67cc4cb7839cd3741b63733d5adf0442,\n    type: 3}\n  m_Name: Probe\n  m_EditorClassIdentifier: \n  customText: |\n    one\n    --- !u!1 &999\n    GameObject:\n    two\n"
        )
    );
    let file = SerializedFile::parse(yaml.as_bytes(), &ParseOptions::new()).expect("読める");
    // block scalar内のヘッダ風文字列で文書が割れない。
    assert_eq!(file.documents().len(), 1);
    let TypedObject::MonoBehaviour(behaviour) =
        file.document(1).expect("ある").decode().expect("読める")
    else {
        panic!("MonoBehaviourになる");
    };
    assert_eq!(behaviour.script.file_id, -1888410255);
    // 独自欄は汎用データのまま残る。
    assert!(behaviour.extra.contains_key("customText"));
}

#[test]
fn 写像の重複キーは拒否する() {
    let yaml = format!(
        "{}{}",
        YAML_DIRECTIVES,
        document(
            1,
            100,
            false,
            "GameObject",
            "  m_Name: A\n  m_Name: B\n  m_IsActive: 1\n  m_Component: []\n"
        )
    );
    let error = SerializedFile::parse(yaml.as_bytes(), &ParseOptions::new())
        .expect_err("重複キーは失敗する");
    assert_eq!(error.kind(), UnityErrorKind::InvalidYaml);
}

#[test]
fn materialの一覧は順序と同名項目を保つ() {
    let body = r#"  serializedVersion: 8
  m_ObjectHideFlags: 0
  m_CorrespondingSourceObject: {fileID: 0}
  m_PrefabInstance: {fileID: 0}
  m_PrefabAsset: {fileID: 0}
  m_Name: Green
  m_Shader: {fileID: 0}
  m_Parent: {fileID: 0}
  m_ModifiedSerializedProperties: 0
  m_ValidKeywords: []
  m_InvalidKeywords: []
  m_LightmapFlags: 4
  m_EnableInstancingVariants: 0
  m_DoubleSidedGI: 0
  m_CustomRenderQueue: -1
  stringTagMap: {}
  disabledShaderPasses: []
  m_LockedProperties:
  m_SavedProperties:
    serializedVersion: 3
    m_TexEnvs:
    - _MainTex:
        m_Texture: {fileID: 0}
        m_Scale: {x: 1, y: 1}
        m_Offset: {x: 0, y: 0}
    - _MainTex:
        m_Texture: {fileID: 2800000, guid: 27f6bef13af40aa4bab1983bb1711cd0, type: 3}
        m_Scale: {x: 2, y: 2}
        m_Offset: {x: 0.5, y: 0}
    m_Ints: []
    m_Floats:
    - _Cutoff: 0.5
    - _Cutoff: 0.75
    m_Colors:
    - _Color: {r: 0, g: 1, b: 0, a: 1}
"#;
    let yaml = format!(
        "{}{}",
        YAML_DIRECTIVES,
        document(21, 2100000, false, "Material", body)
    );
    let file = SerializedFile::parse(yaml.as_bytes(), &ParseOptions::new()).expect("読める");
    let TypedObject::Material(material) = file
        .document(2100000)
        .expect("ある")
        .decode()
        .expect("読める")
    else {
        panic!("Materialになる");
    };
    assert_eq!(material.name, "Green");
    assert!(material.shader.is_null());
    // 同名のtexEnvが2件とも順序通りに残る。
    assert_eq!(material.tex_envs.len(), 2);
    assert_eq!(material.tex_envs[0].name, "_MainTex");
    assert_eq!(material.tex_envs[1].name, "_MainTex");
    assert!(material.tex_envs[0].value.texture.is_null());
    assert_eq!(material.tex_envs[1].value.texture.file_id, 2800000);
    assert_eq!(material.tex_envs[1].value.scale, [2.0, 2.0]);
    assert_eq!(material.tex_envs[1].value.offset, [0.5, 0.0]);
    // 同名のfloatも順序通りに残る。
    assert_eq!(material.floats.len(), 2);
    assert_eq!(material.floats[0].value, 0.5);
    assert_eq!(material.floats[1].value, 0.75);
    assert_eq!(material.colors.len(), 1);
    assert_eq!(material.colors[0].value, [0.0, 1.0, 0.0, 1.0]);
    assert!(material.ints.is_empty());
}

#[test]
fn 参照の所在を区別する() {
    let mut index = AssetIndex::new();
    index
        .insert(
            "Assets/A.prefab",
            AssetMeta::parse(&file_meta(1, "PrefabImporter", PREFAB_IMPORTER_BODY))
                .expect("metaは読める"),
        )
        .expect("挿入は成功する");
    index
        .insert(
            "Assets/B.mat",
            AssetMeta::parse(&file_meta(
                2,
                "NativeFormatImporter",
                "  externalObjects: {}\n",
            ))
            .expect("metaは読める"),
        )
        .expect("挿入は成功する");
    let source = parsed_guid(1);

    // nullはGUIDがあってもnullを優先する。
    let reference = ObjectRef {
        file_id: 0,
        guid: Some(parsed_guid(2)),
        type_hint: Some(3),
    };
    assert!(matches!(
        index.locate(&source, &reference),
        Ok(ReferenceLocation::Null)
    ));

    // GUID省略は同一ファイル内として参照元自身で解決する。
    let reference = ObjectRef {
        file_id: 100,
        guid: None,
        type_hint: None,
    };
    match index.locate(&source, &reference).expect("解決は成功する") {
        ReferenceLocation::Indexed { asset, file_id } => {
            assert_eq!(asset.path, "Assets/A.prefab");
            assert_eq!(file_id, 100);
        }
        other => panic!("同一ファイル内になる: {other:?}"),
    }

    // 索引にあるGUIDは所在付きで返す。
    let reference = ObjectRef {
        file_id: 2100000,
        guid: Some(parsed_guid(2)),
        type_hint: Some(2),
    };
    match index.locate(&source, &reference).expect("解決は成功する") {
        ReferenceLocation::Indexed { asset, file_id } => {
            assert_eq!(asset.path, "Assets/B.mat");
            assert_eq!(file_id, 2100000);
        }
        other => panic!("索引付きになる: {other:?}"),
    }

    // 索引にないGUIDは欠落と組み込みを区別せず外部として返す。
    let missing = Guid::parse("ffffffffffffffffffffffffffffffff").expect("正当である");
    let reference = ObjectRef {
        file_id: 4800000,
        guid: Some(missing.clone()),
        type_hint: Some(3),
    };
    match index.locate(&source, &reference).expect("解決は成功する") {
        ReferenceLocation::External { guid, file_id } => {
            assert_eq!(guid, missing);
            assert_eq!(file_id, 4800000);
        }
        other => panic!("外部になる: {other:?}"),
    }

    // 参照元自身が索引にない場合は失敗する。
    let unknown = Guid::parse("eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee").expect("正当である");
    let reference = ObjectRef {
        file_id: 1,
        guid: None,
        type_hint: None,
    };
    let error = index
        .locate(&unknown, &reference)
        .expect_err("参照元不明は失敗する");
    assert_eq!(error.kind(), UnityErrorKind::MissingObject);
}

#[test]
fn guidとパスとfileidの重複は後勝ちにせず拒否する() {
    let mut index = AssetIndex::new();
    index
        .insert(
            "Assets/A.png",
            AssetMeta::parse(&file_meta(1, "TextureImporter", TEXTURE_IMPORTER_BODY))
                .expect("metaは読める"),
        )
        .expect("挿入は成功する");
    // 同一GUIDの別パスは拒否する。
    let error = index
        .insert(
            "Assets/B.png",
            AssetMeta::parse(&file_meta(1, "TextureImporter", TEXTURE_IMPORTER_BODY))
                .expect("metaは読める"),
        )
        .expect_err("GUID重複は失敗する");
    assert_eq!(error.kind(), UnityErrorKind::Conflict);
    // 同一パスの別GUIDは拒否する。
    let error = index
        .insert(
            "Assets/A.png",
            AssetMeta::parse(&file_meta(2, "TextureImporter", TEXTURE_IMPORTER_BODY))
                .expect("metaは読める"),
        )
        .expect_err("パス重複は失敗する");
    assert_eq!(error.kind(), UnityErrorKind::Conflict);
    // 大小文字のみ異なるパスも拒否する。
    let error = index
        .insert(
            "Assets/a.png",
            AssetMeta::parse(&file_meta(2, "TextureImporter", TEXTURE_IMPORTER_BODY))
                .expect("metaは読める"),
        )
        .expect_err("大小文字の重複は失敗する");
    assert_eq!(error.kind(), UnityErrorKind::Conflict);
    // 同一内容の再挿入も後勝ちにしない。
    let error = index
        .insert(
            "Assets/A.png",
            AssetMeta::parse(&file_meta(1, "TextureImporter", TEXTURE_IMPORTER_BODY))
                .expect("metaは読める"),
        )
        .expect_err("同一再挿入も失敗する");
    assert_eq!(error.kind(), UnityErrorKind::Conflict);
    assert_eq!(index.entries().len(), 1);

    // 同一ファイル内のfileID重複は拒否する。
    let yaml = format!(
        "{}{}{}",
        YAML_DIRECTIVES,
        document(1, 100, false, "GameObject", &game_object_body("A", &[], 1)),
        document(1, 100, false, "GameObject", &game_object_body("B", &[], 1)),
    );
    let error = SerializedFile::parse(yaml.as_bytes(), &ParseOptions::new())
        .expect_err("fileID重複は失敗する");
    assert_eq!(error.kind(), UnityErrorKind::Conflict);
    assert_eq!(error.file_id(), Some(100));
}

#[test]
fn 正常階層と複数rootを読める() {
    let yaml = format!(
        "{}{}{}{}{}{}{}",
        YAML_DIRECTIVES,
        document(
            1,
            100,
            false,
            "GameObject",
            &game_object_body("Root", &[200], 1)
        ),
        document(
            4,
            200,
            false,
            "Transform",
            &transform_body(100, "{fileID: 0}", &[201, 301])
        ),
        document(
            1,
            101,
            false,
            "GameObject",
            &game_object_body("Child", &[201], 1)
        ),
        document(
            4,
            201,
            false,
            "Transform",
            &transform_body(101, "{fileID: 200}", &[])
        ),
        document(
            1,
            102,
            false,
            "GameObject",
            &game_object_body("Second", &[301], 0)
        ),
        document(
            4,
            301,
            false,
            "Transform",
            &transform_body(102, "{fileID: 200}", &[])
        ),
    );
    let file = SerializedFile::parse(yaml.as_bytes(), &ParseOptions::new()).expect("読める");
    let hierarchy = file.local_hierarchy().expect("階層は組める");
    // 子順序は m_Children のまま保つ。
    assert_eq!(hierarchy.roots().len(), 1);
    assert_eq!(hierarchy.roots()[0].name, "Root");
    assert_eq!(hierarchy.len(), 3);
    let children: Vec<_> = hierarchy.roots()[0]
        .children
        .iter()
        .map(|node| node.name.as_str())
        .collect();
    assert_eq!(children, vec!["Child", "Second"]);

    // rootが複数あっても読める。
    let yaml = format!(
        "{}{}{}{}",
        YAML_DIRECTIVES,
        document(
            1,
            100,
            false,
            "GameObject",
            &game_object_body("A", &[200], 1)
        ),
        document(
            4,
            200,
            false,
            "Transform",
            &transform_body(100, "{fileID: 0}", &[])
        ),
        document(
            1,
            101,
            false,
            "GameObject",
            &game_object_body("B", &[201], 1)
        ),
    );
    // BにはTransformがないため失敗する（対応検証）。
    let file = SerializedFile::parse(yaml.as_bytes(), &ParseOptions::new()).expect("読める");
    let error = file.local_hierarchy().expect_err("対応欠落は失敗する");
    assert_eq!(error.kind(), UnityErrorKind::InvalidHierarchy);
}

#[test]
fn 複数rootの正常階層を読める() {
    let yaml = format!(
        "{}{}{}{}{}",
        YAML_DIRECTIVES,
        document(
            1,
            100,
            false,
            "GameObject",
            &game_object_body("A", &[200], 1)
        ),
        document(
            4,
            200,
            false,
            "Transform",
            &transform_body(100, "{fileID: 0}", &[])
        ),
        document(
            1,
            101,
            false,
            "GameObject",
            &game_object_body("B", &[201], 1)
        ),
        document(
            4,
            201,
            false,
            "Transform",
            &transform_body(101, "{fileID: 0}", &[])
        ),
    );
    let file = SerializedFile::parse(yaml.as_bytes(), &ParseOptions::new()).expect("読める");
    let hierarchy = file.local_hierarchy().expect("複数rootは組める");
    assert_eq!(hierarchy.roots().len(), 2);
    assert!(
        hierarchy
            .roots()
            .iter()
            .all(|node| node.children.is_empty())
    );
}

#[test]
fn 循環と親子不整合は失敗する() {
    // 循環（AとBが互いの親）。
    let yaml = format!(
        "{}{}{}{}{}",
        YAML_DIRECTIVES,
        document(
            1,
            100,
            false,
            "GameObject",
            &game_object_body("A", &[200], 1)
        ),
        document(
            4,
            200,
            false,
            "Transform",
            &transform_body(100, "{fileID: 201}", &[201])
        ),
        document(
            1,
            101,
            false,
            "GameObject",
            &game_object_body("B", &[201], 1)
        ),
        document(
            4,
            201,
            false,
            "Transform",
            &transform_body(101, "{fileID: 200}", &[200])
        ),
    );
    let file =
        SerializedFile::parse(yaml.as_bytes(), &ParseOptions::new()).expect("枠自体は読める");
    let error = file.local_hierarchy().expect_err("循環は失敗する");
    assert_eq!(error.kind(), UnityErrorKind::InvalidHierarchy);

    // 親子不整合（親の子一覧に自分がない）。
    let yaml = format!(
        "{}{}{}{}{}",
        YAML_DIRECTIVES,
        document(
            1,
            100,
            false,
            "GameObject",
            &game_object_body("A", &[200], 1)
        ),
        document(
            4,
            200,
            false,
            "Transform",
            &transform_body(100, "{fileID: 0}", &[])
        ),
        document(
            1,
            101,
            false,
            "GameObject",
            &game_object_body("B", &[201], 1)
        ),
        document(
            4,
            201,
            false,
            "Transform",
            &transform_body(101, "{fileID: 200}", &[])
        ),
    );
    let file =
        SerializedFile::parse(yaml.as_bytes(), &ParseOptions::new()).expect("枠自体は読める");
    let error = file.local_hierarchy().expect_err("不整合は失敗する");
    assert_eq!(error.kind(), UnityErrorKind::InvalidHierarchy);

    // 存在しない参照（構成要素の指す文書がない）。
    let yaml = format!(
        "{}{}{}",
        YAML_DIRECTIVES,
        document(
            1,
            100,
            false,
            "GameObject",
            &game_object_body("A", &[999], 1)
        ),
        document(
            4,
            200,
            false,
            "Transform",
            &transform_body(100, "{fileID: 0}", &[])
        ),
    );
    let file =
        SerializedFile::parse(yaml.as_bytes(), &ParseOptions::new()).expect("枠自体は読める");
    let error = file.local_hierarchy().expect_err("欠落参照は失敗する");
    assert_eq!(error.kind(), UnityErrorKind::InvalidHierarchy);
}

#[test]
fn prefab実体はパースでき階層は明示的に非対応になる() {
    // PrefabInstanceを含む文書列は汎用読み取りできる。
    let yaml = format!(
        "{}{}{}",
        YAML_DIRECTIVES,
        document(
            1001,
            900,
            false,
            "PrefabInstance",
            "  m_ObjectHideFlags: 0\n  serializedVersion: 2\n  m_Modification:\n    m_TransformParent: {fileID: 0}\n  m_SourcePrefab: {fileID: 100, guid: 9ef94258554e06b4ab03fbe9fc914c5c, type: 3}\n"
        ),
        document(
            1,
            100,
            false,
            "GameObject",
            &game_object_body("Root", &[200], 1)
        ),
    );
    let file =
        SerializedFile::parse(yaml.as_bytes(), &ParseOptions::new()).expect("パースは成功する");
    let TypedObject::PrefabInstance(instance) =
        file.document(900).expect("ある").decode().expect("読める")
    else {
        panic!("PrefabInstanceになる");
    };
    assert_eq!(instance.source.file_id, 100);
    assert!(instance.extra.contains_key("m_Modification"));
    let error = file.local_hierarchy().expect_err("階層は非対応で失敗する");
    assert_eq!(error.kind(), UnityErrorKind::UnsupportedPrefabComposition);
}

#[test]
fn 画像とfbxを読まずに索引できる() {
    let root = tempfile::TempDir::new().expect("親領域は作れる");
    let assets = root.path().join("Assets");
    fs::create_dir(&assets).expect("準備は成功する");
    // PNGとFBXの本文は読まず、metaだけで索引する。
    fs::write(assets.join("tex.png"), b"\x89PNG-fake").expect("準備は成功する");
    fs::write(
        assets.join("tex.png.meta"),
        file_meta(1, "TextureImporter", TEXTURE_IMPORTER_BODY),
    )
    .expect("準備は成功する");
    fs::write(assets.join("model.fbx"), b"Kaydara FBX Binary  \x00-fake").expect("準備は成功する");
    fs::write(
        assets.join("model.fbx.meta"),
        file_meta(2, "ModelImporter", "  externalObjects: {}\n"),
    )
    .expect("準備は成功する");
    let index =
        vrctom::unity::scan_directory(&assets, &IndexOptions::new()).expect("索引は成功する");
    assert_eq!(index.entries().len(), 2);
    assert_eq!(
        index.get(&parsed_guid(1)).expect("ある").kind,
        AssetKind::Texture
    );
    assert_eq!(
        index.get(&parsed_guid(2)).expect("ある").kind,
        AssetKind::Model
    );
}

#[test]
fn symlink拒否とmeta欠落と上限超過を確認する() {
    // symlinkは追従せず拒否する。
    #[cfg(unix)]
    {
        let root = tempfile::TempDir::new().expect("親領域は作れる");
        let assets = root.path().join("Assets");
        fs::create_dir(&assets).expect("準備は成功する");
        fs::write(assets.join("real.txt"), b"x").expect("準備は成功する");
        fs::write(
            assets.join("real.txt.meta"),
            file_meta(1, "TextImporter", "  userData: \n"),
        )
        .expect("準備は成功する");
        std::os::unix::fs::symlink(assets.join("real.txt"), assets.join("link.txt"))
            .expect("準備は成功する");
        let error = vrctom::unity::scan_directory(&assets, &IndexOptions::new())
            .expect_err("symlinkは失敗する");
        assert_eq!(error.kind(), UnityErrorKind::InvalidPath);
    }

    // metaのない通常ファイルは拒否する。
    {
        let root = tempfile::TempDir::new().expect("親領域は作れる");
        let assets = root.path().join("Assets");
        fs::create_dir(&assets).expect("準備は成功する");
        fs::write(assets.join("orphan.txt"), b"x").expect("準備は成功する");
        let error = vrctom::unity::scan_directory(&assets, &IndexOptions::new())
            .expect_err("meta欠落は失敗する");
        assert_eq!(error.kind(), UnityErrorKind::MissingMetadata);
    }

    // 対応する実体のないmetaは拒否する。
    {
        let root = tempfile::TempDir::new().expect("親領域は作れる");
        let assets = root.path().join("Assets");
        fs::create_dir(&assets).expect("準備は成功する");
        fs::write(
            assets.join("ghost.txt.meta"),
            file_meta(1, "TextImporter", "  userData: \n"),
        )
        .expect("準備は成功する");
        let error = vrctom::unity::scan_directory(&assets, &IndexOptions::new())
            .expect_err("実体欠落は失敗する");
        assert_eq!(error.kind(), UnityErrorKind::MissingMetadata);
    }

    // アセット数とmeta合計の上限超過を確認する。
    {
        let root = tempfile::TempDir::new().expect("親領域は作れる");
        let assets = root.path().join("Assets");
        fs::create_dir(&assets).expect("準備は成功する");
        for n in [1u128, 2] {
            fs::write(assets.join(format!("f{n}.txt")), b"x").expect("準備は成功する");
            fs::write(
                assets.join(format!("f{n}.txt.meta")),
                file_meta(n, "TextImporter", "  userData: \n"),
            )
            .expect("準備は成功する");
        }
        let options = IndexOptions::new().with_max_assets(1);
        let error =
            vrctom::unity::scan_directory(&assets, &options).expect_err("件数超過は失敗する");
        assert_eq!(error.kind(), UnityErrorKind::LimitExceeded);
        let options = IndexOptions::new().with_max_meta_total_bytes(1);
        let error =
            vrctom::unity::scan_directory(&assets, &options).expect_err("合計超過は失敗する");
        assert_eq!(error.kind(), UnityErrorKind::LimitExceeded);
    }

    // 文書枠の上限超過を確認する。
    {
        let prefab = two_level_prefab();
        let options = ParseOptions::new().with_max_file_bytes(10);
        let error =
            SerializedFile::parse(prefab.as_bytes(), &options).expect_err("容量超過は失敗する");
        assert_eq!(error.kind(), UnityErrorKind::LimitExceeded);
        let options = ParseOptions::new().with_max_documents(2);
        let error =
            SerializedFile::parse(prefab.as_bytes(), &options).expect_err("件数超過は失敗する");
        assert_eq!(error.kind(), UnityErrorKind::LimitExceeded);
    }
}

/// `.unitypackage` を生成・展開し、そのまま索引と読み取りへ接続する。
#[test]
fn 生成パッケージの展開から索引と読み取りへ接続できる() {
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use std::io::Write as _;

    /// tarの1件を組み立てる。
    fn append(builder: &mut tar::Builder<Vec<u8>>, path: &str, content: &[u8]) {
        let mut header = tar::Header::new_gnu();
        header.set_size(content.len() as u64);
        header.set_mode(0o644);
        header.set_path(path).expect("短いパスのため成功する");
        header.set_cksum();
        builder
            .append(&header, content)
            .expect("書き込みは成功する");
    }

    let guids = [0x11u128, 0x22];
    let full_guids: Vec<String> = guids.iter().map(|n| guid(*n)).collect();
    let prefab_yaml = two_level_prefab();
    // 1GUIDにつき `pathname` は1件だけ書く。順序を混ぜても対応付けできる。
    struct Staged<'a> {
        guid: &'a str,
        pathname: &'a str,
        asset: Option<Vec<u8>>,
        meta: Vec<u8>,
    }
    let staged = vec![
        Staged {
            guid: &full_guids[0],
            pathname: "Assets",
            asset: None,
            meta: folder_meta(0x11),
        },
        Staged {
            guid: &full_guids[1],
            pathname: "Assets/Hero.prefab",
            asset: Some(prefab_yaml.clone().into_bytes()),
            meta: file_meta(0x22, "PrefabImporter", PREFAB_IMPORTER_BODY),
        },
    ];
    let mut builder = tar::Builder::new(Vec::new());
    for entry in &staged {
        append(
            &mut builder,
            &format!("{}/pathname", entry.guid),
            entry.pathname.as_bytes(),
        );
        if let Some(asset) = &entry.asset {
            append(&mut builder, &format!("{}/asset", entry.guid), asset);
        }
        append(
            &mut builder,
            &format!("{}/asset.meta", entry.guid),
            &entry.meta,
        );
        // フォルダGUIDはディレクトリエントリも持つ。
        if entry.asset.is_none() {
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Directory);
            header.set_size(0);
            header
                .set_path(format!("{}/", entry.guid))
                .expect("短いパスのため成功する");
            header.set_cksum();
            builder
                .append(&header, &[][..])
                .expect("書き込みは成功する");
        }
    }
    // メタのみ・実体のみの混在でも対応付けできるよう、順序を混ぜる。
    let tar_bytes = builder.into_inner().expect("tarは完成する");
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(&tar_bytes)
        .expect("gzip書き込みは成功する");
    let package = encoder.finish().expect("gzipは完成する");

    let parent = tempfile::TempDir::new().expect("親領域は作れる");
    let destination = parent.path().join("out");
    vrctom::unitypackage::unpack_to(
        &package[..],
        &destination,
        &vrctom::unitypackage::UnpackOptions::new(),
    )
    .expect("展開は成功する");
    let index =
        vrctom::unity::scan_directory(&destination, &IndexOptions::new()).expect("索引は成功する");
    assert_eq!(index.entries().len(), 2);
    let prefab = index
        .get_by_path("Assets/Hero.prefab")
        .expect("prefabがある");
    assert_eq!(prefab.kind, AssetKind::Prefab);

    // prefab本文を読み取り、階層まで接続する。
    let bytes = fs::read(destination.join("Assets/Hero.prefab")).expect("本文は読める");
    let file = SerializedFile::parse(&bytes, &ParseOptions::new()).expect("パースは成功する");
    let hierarchy = file.local_hierarchy().expect("階層は組める");
    assert_eq!(hierarchy.roots().len(), 1);
    assert_eq!(hierarchy.roots()[0].name, "Root");
    // 同一ファイル内参照は参照元自身で解決する。
    let room = Guid::parse(&full_guids[1]).expect("正当である");
    let reference = ObjectRef {
        file_id: 200,
        guid: None,
        type_hint: None,
    };
    assert!(matches!(
        index.locate(&room, &reference),
        Ok(ReferenceLocation::Indexed { .. })
    ));
}

/// Unity YAMLとしてパースすべきアセットかどうかを拡張子で判定する。
///
/// `.prefab`/`.mat`/`.anim`/`.controller`/`.asset`/`.renderTexture`/`.unity` を
/// 既知のUnity YAMLとして扱い、それ以外（`.shader`/`.cs`/`.txt` など）は
/// 読み飛ばす。判定は小文字化した拡張子で行う。
fn is_unity_yaml_asset(path: &str) -> bool {
    let extension = path
        .rsplit('/')
        .next()
        .and_then(|file| file.rsplit_once('.'))
        .filter(|(stem, _)| !stem.is_empty())
        .map(|(_, extension)| extension.to_ascii_lowercase());
    matches!(
        extension.as_deref(),
        Some("prefab" | "mat" | "anim" | "controller" | "asset" | "rendertexture" | "unity")
    )
}

/// 展開済み実fixtureの全体観測（オプトイン）。
///
/// `VRCTOM_UNITY_FIXTURE_DIR` が指す展開済みディレクトリだけを読み、
/// 未設定なら通常テストを壊さないよう跳ばす。未供給GUIDは失敗条件にしない。
#[test]
fn 実fixtureの全体を観測する() {
    let Some(dir) = std::env::var_os("VRCTOM_UNITY_FIXTURE_DIR") else {
        eprintln!("VRCTOM_UNITY_FIXTURE_DIR 未設定のため実fixture観測を跳ばす");
        return;
    };
    let root = PathBuf::from(dir);
    let index = vrctom::unity::scan_directory(&root, &IndexOptions::new())
        .expect("実fixtureの索引は成功する");
    eprintln!("実fixture索引: {}件", index.entries().len());

    // 全metaの読み直しと衝突有無、importerと種別の内訳。
    let mut importer_counts: HashMap<String, usize> = HashMap::new();
    let mut kind_counts: HashMap<String, usize> = HashMap::new();
    let mut remap_total = 0usize;
    let mut remap_indexed = 0usize;
    let mut remap_external = 0usize;
    for record in index.entries() {
        let meta_bytes =
            fs::read(root.join(format!("{}.meta", record.path))).expect("metaは読める");
        let meta = AssetMeta::parse(&meta_bytes).expect("全metaは読める");
        assert_eq!(meta.guid(), &record.guid, "GUIDは一致する: {}", record.path);
        *importer_counts
            .entry(meta.importer().name.clone())
            .or_default() += 1;
        *kind_counts.entry(format!("{:?}", record.kind)).or_default() += 1;
        for remap in meta.external_object_remaps().expect("置換表は読める") {
            remap_total += 1;
            match index
                .locate(&record.guid, &remap.replacement)
                .expect("所在特定は成功する")
            {
                ReferenceLocation::Indexed { .. } => remap_indexed += 1,
                ReferenceLocation::External { .. } => remap_external += 1,
                ReferenceLocation::Null => panic!("置換先がnullになることはない"),
            }
        }
    }
    eprintln!("importer内訳: {importer_counts:?}");
    eprintln!("種別内訳: {kind_counts:?}");
    eprintln!("置換表: {remap_total}件（索引{remap_indexed}件 外部{remap_external}件）");

    // Unity YAML全件の汎用パースと代表型decode、未知class/importerの件数。
    let mut class_counts: HashMap<String, usize> = HashMap::new();
    let mut unknown_classes: HashSet<String> = HashSet::new();
    let mut stripped_docs = 0usize;
    let mut prefab_instances = 0usize;
    let mut hierarchy_ok = 0usize;
    let mut hierarchy_unsupported = 0usize;
    let mut parsed_files = 0usize;
    let mut skipped_binary = 0usize;
    // 参照の所在内訳。`fileID`/形だけの参照も含めて数える。
    let mut ref_indexed = 0usize;
    let mut ref_external = 0usize;
    let mut ref_null = 0usize;
    let mut ref_model = 0usize;
    let mut ref_material = 0usize;
    let mut missing_guids: HashMap<String, usize> = HashMap::new();
    for record in index.entries() {
        if record.meta.is_folder() {
            continue;
        }
        // Unity YAMLとして読むのは既知の拡張子だけにする。
        // `.shader`/`.cs`/`.txt` などの非YAMLは読み飛ばし、
        // 既知YAML拡張子の読み取り失敗はテスト失敗にする。
        if !is_unity_yaml_asset(&record.path) {
            skipped_binary += 1;
            continue;
        }
        let bytes = fs::read(root.join(&record.path)).expect("本文は読める");
        let file = match SerializedFile::parse(&bytes, &ParseOptions::new()) {
            Ok(file) => file,
            Err(error) => {
                panic!("Unity YAMLの読み取りに失敗した: {}: {error}", record.path)
            }
        };
        parsed_files += 1;
        for document in file.documents() {
            let key = format!("{}:{}", document.class_id(), document.class_name());
            *class_counts.entry(key.clone()).or_default() += 1;
            if document.stripped() {
                stripped_docs += 1;
            }
            match document.decode().expect("代表型読み取りは成功する") {
                TypedObject::Unknown => {
                    unknown_classes.insert(key);
                }
                TypedObject::PrefabInstance(_) => prefab_instances += 1,
                _ => {}
            }
            // 本文内の正準形参照を集めて所在を数える。
            for reference in collect_refs(document.raw()) {
                match index
                    .locate(&record.guid, &reference)
                    .expect("所在特定は成功する")
                {
                    ReferenceLocation::Indexed { asset, .. } => {
                        ref_indexed += 1;
                        // renderer→FBX・参照→マテリアルの所在特定を確認する。
                        match asset.kind {
                            vrctom::unity::AssetKind::Model => ref_model += 1,
                            vrctom::unity::AssetKind::Material => ref_material += 1,
                            _ => {}
                        }
                    }
                    ReferenceLocation::External { guid, .. } => {
                        ref_external += 1;
                        *missing_guids.entry(guid.to_string()).or_default() += 1;
                    }
                    ReferenceLocation::Null => ref_null += 1,
                }
            }
        }
        match file.local_hierarchy() {
            Ok(_) => hierarchy_ok += 1,
            Err(error) => {
                assert_eq!(
                    error.kind(),
                    UnityErrorKind::UnsupportedPrefabComposition,
                    "階層失敗は非対応のみ: {}: {error}",
                    record.path
                );
                hierarchy_unsupported += 1;
            }
        }
    }
    eprintln!("YAML文書のclass内訳: {class_counts:?}");
    eprintln!("未知class: {unknown_classes:?}");
    eprintln!("stripped文書: {stripped_docs}件 PrefabInstance: {prefab_instances}件");
    eprintln!("階層: 成功{hierarchy_ok}件 非対応{hierarchy_unsupported}件");
    eprintln!("パース成功: {parsed_files}件 除外(画像等): {skipped_binary}件");
    eprintln!("参照: 索引{ref_indexed}件 外部{ref_external}件 null{ref_null}件");
    eprintln!("参照先の所在: Model{ref_model}件 Material{ref_material}件");
    eprintln!("未供給GUID上位: {:?}", top_counts(&missing_guids, 10));
    // 未供給GUIDは失敗条件にしない。製品名や固定件数にも依存しない。
    assert!(parsed_files > 0, "YAML文書は1件以上ある");
    assert!(ref_model > 0, "FBXへの所在特定は1件以上ある");
    assert!(ref_material > 0, "マテリアルへの所在特定は1件以上ある");
}

/// YAML値内の正準形参照（`fileID` のみ・`fileID/guid/type`）を集める。
fn collect_refs(value: &yaml_serde::Value) -> Vec<ObjectRef> {
    let mut out = Vec::new();
    collect_refs_into(value, &mut out);
    out
}

/// 正準形の写像だけを参照として集める。
fn collect_refs_into(value: &yaml_serde::Value, out: &mut Vec<ObjectRef>) {
    match value {
        yaml_serde::Value::Mapping(mapping) => {
            // `fileID` を持ち、`guid` と `type` 以外の欄を持たない写像だけを集める。
            // 独自欄や改変内容の数値・文字列は参照ではない。
            if mapping.contains_key("fileID")
                && mapping
                    .iter()
                    .all(|(key, _)| matches!(key.as_str(), Some("fileID" | "guid" | "type")))
            {
                out.push(ObjectRef::decode(value).expect("正準形は読める"));
            } else {
                for (_, entry) in mapping {
                    collect_refs_into(entry, out);
                }
            }
        }
        yaml_serde::Value::Sequence(sequence) => {
            for entry in sequence {
                collect_refs_into(entry, out);
            }
        }
        _ => {}
    }
}

/// 件数上位を返す。
fn top_counts(counts: &HashMap<String, usize>, limit: usize) -> Vec<(String, usize)> {
    let mut items: Vec<_> = counts
        .iter()
        .map(|(key, count)| (key.clone(), *count))
        .collect();
    items.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    items.truncate(limit);
    items
}

/// 外部GUID付き参照の形状を作る。
fn guid_ref(file_id: FileId, guid: &str) -> String {
    format!("{{fileID: {file_id}, guid: {guid}, type: 3}}")
}

/// テスト用の外部GUID（32桁hex）。
const EXT_GUID: &str = "9ef94258554e06b4ab03fbe9fc914c5c";

#[test]
fn blockscalarの末尾改行は文書位置と改行符号で変わらない() {
    // 途中文書と最終文書でblock scalarの値が変わらないことを確かめる。
    // 回帰: 行を集めて再結合すると末尾改行が落ち、文書位置で値が変わった。
    fn mono_with(custom: &str) -> String {
        format!(
            "  m_GameObject: {{fileID: 0}}\n  m_Enabled: 1\n  m_Script: {{fileID: 1}}\n  m_Name: Probe\n  m_EditorClassIdentifier: \n{custom}"
        )
    }
    fn scalar_value(yaml: &str) -> String {
        let file =
            SerializedFile::parse(yaml.as_bytes(), &ParseOptions::new()).expect("枠は読める");
        assert_eq!(file.documents().len(), 2);
        let TypedObject::MonoBehaviour(behaviour) =
            file.document(1).expect("ある").decode().expect("読める")
        else {
            panic!("MonoBehaviourになる");
        };
        behaviour
            .extra
            .get("customText")
            .and_then(|value| value.as_str())
            .expect("文字列である")
            .to_owned()
    }
    // 字下げした完全形のヘッダ風文字列はscalar内に残り、文書を割らない。
    let styles = [
        (
            "pipe",
            "  customText: |\n    one\n    --- !u!1 &1\n    two\n",
        ),
        ("keep", "  customText: |+\n    one\n    two\n\n"),
        ("fold", "  customText: >\n    one\n    two\n"),
    ];
    for (name, custom) in styles {
        let middle = format!(
            "{}{}{}",
            YAML_DIRECTIVES,
            document(114, 1, false, "MonoBehaviour", &mono_with(custom)),
            document(1, 100, false, "GameObject", &game_object_body("X", &[], 1)),
        );
        let last = format!(
            "{}{}{}",
            YAML_DIRECTIVES,
            document(1, 100, false, "GameObject", &game_object_body("X", &[], 1)),
            document(114, 1, false, "MonoBehaviour", &mono_with(custom)),
        );
        let mid_value = scalar_value(&middle);
        let last_value = scalar_value(&last);
        assert_eq!(mid_value, last_value, "文書位置で値が変わらない: {name}");
        // CRLFでもLFと同じ値になる。
        assert_eq!(
            scalar_value(&middle.replace('\n', "\r\n")),
            mid_value,
            "CRLFで値が変わらない: {name}"
        );
        assert_eq!(
            scalar_value(&last.replace('\n', "\r\n")),
            mid_value,
            "CRLFで値が変わらない: {name}"
        );
    }
    // `|` の値は末尾改行を保つ。
    let middle = format!(
        "{}{}{}",
        YAML_DIRECTIVES,
        document(
            114,
            1,
            false,
            "MonoBehaviour",
            &mono_with("  customText: |\n    one\n    --- !u!1 &1\n    two\n")
        ),
        document(1, 100, false, "GameObject", &game_object_body("X", &[], 1)),
    );
    assert_eq!(
        scalar_value(&middle),
        "one\n--- !u!1 &1\ntwo\n",
        "末尾改行を保つ"
    );
}

#[test]
fn 深い鎖でも組み立てと集計と破棄と複写ができる() {
    // 8,000節点の鎖を反復処理で組み立てる。再帰ではstackを使い果たしabortする。
    const DEPTH: usize = 8000;
    let mut yaml = YAML_DIRECTIVES.to_owned();
    for i in 0..DEPTH {
        let game_id = 100 + (i as i64) * 2;
        let transform_id = game_id + 1;
        yaml.push_str(&document(
            1,
            game_id,
            false,
            "GameObject",
            &game_object_body(&format!("N{i}"), &[transform_id], 1),
        ));
        let father = if i == 0 {
            "{fileID: 0}".to_owned()
        } else {
            format!("{{fileID: {}}}", transform_id - 2)
        };
        let children: &[i64] = if i + 1 < DEPTH {
            &[transform_id + 2]
        } else {
            &[]
        };
        yaml.push_str(&document(
            4,
            transform_id,
            false,
            "Transform",
            &transform_body(game_id, &father, children),
        ));
    }
    let file = SerializedFile::parse(yaml.as_bytes(), &ParseOptions::new()).expect("枠は読める");
    let hierarchy = file.local_hierarchy().expect("深い鎖は組める");
    assert_eq!(hierarchy.len(), DEPTH);
    assert_eq!(hierarchy.roots().len(), 1);
    assert_eq!(hierarchy.roots()[0].name, "N0");
    // 反復で辿り、鎖の順序を確かめる。
    let mut node = &hierarchy.roots()[0];
    for i in 0..DEPTH {
        assert_eq!(node.name, format!("N{i}"));
        if i + 1 < DEPTH {
            assert_eq!(node.children.len(), 1);
            node = &node.children[0];
        } else {
            assert!(node.children.is_empty());
        }
    }
    // 複写も再帰にせず行い、破棄もabortしない。
    let cloned = hierarchy.clone();
    assert_eq!(cloned.len(), DEPTH);
    drop(cloned);
    drop(hierarchy);

    // 深さの上限超過は資源上限として失敗する。鎖の深さは 7999（辺の数）である。
    let error = file
        .local_hierarchy_with_options(&HierarchyOptions::new().with_max_depth(100))
        .expect_err("上限超過は失敗する");
    assert_eq!(error.kind(), UnityErrorKind::LimitExceeded);
    let error = file
        .local_hierarchy_with_options(&HierarchyOptions::new().with_max_depth(DEPTH - 2))
        .expect_err("境界の内側は失敗する");
    assert_eq!(error.kind(), UnityErrorKind::LimitExceeded);
    assert_eq!(
        file.local_hierarchy_with_options(&HierarchyOptions::new().with_max_depth(DEPTH - 1))
            .expect("境界値は通る")
            .len(),
        DEPTH
    );
}

#[test]
fn 階層の構造参照にある外部guidはローカル一致として扱わない() {
    // 所属・親・子・componentの各参照に外部GUIDを付け、
    // ローカル同一fileIDへ結び付かないことを確かめる。
    // 回帰: fileIDだけで照合し、外部参照をローカル扱いした。
    let cases: Vec<(&str, String)> = vec![
        // 所属Transformの参照先が外部GUIDを持つ。
        (
            "所属",
            format!(
                "{}{}{}",
                YAML_DIRECTIVES,
                document(
                    1,
                    100,
                    false,
                    "GameObject",
                    &game_object_body("A", &[200], 1)
                ),
                document(
                    4,
                    200,
                    false,
                    "Transform",
                    &transform_body(100, "{fileID: 0}", &[])
                        .replace("{fileID: 100}", &guid_ref(100, EXT_GUID)),
                ),
            ),
        ),
        // 親Transformの参照先が外部GUIDを持つ。
        (
            "親",
            format!(
                "{}{}{}{}{}",
                YAML_DIRECTIVES,
                document(
                    1,
                    100,
                    false,
                    "GameObject",
                    &game_object_body("A", &[200], 1)
                ),
                document(
                    4,
                    200,
                    false,
                    "Transform",
                    &transform_body(100, "{fileID: 0}", &[201]),
                ),
                document(
                    1,
                    101,
                    false,
                    "GameObject",
                    &game_object_body("B", &[201], 1)
                ),
                document(
                    4,
                    201,
                    false,
                    "Transform",
                    &transform_body(101, &guid_ref(200, EXT_GUID), &[]),
                ),
            ),
        ),
        // 子Transformの参照先が外部GUIDを持つ。
        (
            "子",
            format!(
                "{}{}{}{}{}",
                YAML_DIRECTIVES,
                document(
                    1,
                    100,
                    false,
                    "GameObject",
                    &game_object_body("A", &[200], 1)
                ),
                document(
                    4,
                    200,
                    false,
                    "Transform",
                    &transform_body(100, "{fileID: 0}", &[]).replace(
                        "m_Children: []",
                        &format!("m_Children:\n  - {}", guid_ref(201, EXT_GUID))
                    ),
                ),
                document(
                    1,
                    101,
                    false,
                    "GameObject",
                    &game_object_body("B", &[201], 1)
                ),
                document(
                    4,
                    201,
                    false,
                    "Transform",
                    &transform_body(101, "{fileID: 200}", &[]),
                ),
            ),
        ),
        // 構成要素の参照先が外部GUIDを持つ。
        (
            "component",
            format!(
                "{}{}{}",
                YAML_DIRECTIVES,
                document(
                    1,
                    100,
                    false,
                    "GameObject",
                    &game_object_body("A", &[200], 1).replace(
                        "- component: {fileID: 200}",
                        &format!("- component: {}", guid_ref(200, EXT_GUID))
                    ),
                ),
                document(
                    4,
                    200,
                    false,
                    "Transform",
                    &transform_body(100, "{fileID: 0}", &[]),
                ),
            ),
        ),
    ];
    for (name, yaml) in cases {
        let file =
            SerializedFile::parse(yaml.as_bytes(), &ParseOptions::new()).expect("枠は読める");
        let error = file.local_hierarchy().expect_err("外部参照は失敗する");
        assert_eq!(
            error.kind(),
            UnityErrorKind::UnsupportedPrefabComposition,
            "外部GUIDは合成の非対応になる: {name}"
        );
    }
}

#[test]
fn gameobjectとtransformの双方向対応を検証する() {
    // 回帰: m_ComponentとTransformの対応を見ず、取り違えを通した。
    // 対応するTransformを含まない構成要素は拒否する。
    let yaml = format!(
        "{}{}{}",
        YAML_DIRECTIVES,
        document(1, 100, false, "GameObject", &game_object_body("A", &[], 1)),
        document(
            4,
            200,
            false,
            "Transform",
            &transform_body(100, "{fileID: 0}", &[])
        ),
    );
    let file = SerializedFile::parse(yaml.as_bytes(), &ParseOptions::new()).expect("枠は読める");
    let error = file.local_hierarchy().expect_err("空の構成要素は失敗する");
    assert_eq!(error.kind(), UnityErrorKind::InvalidHierarchy);

    // componentがGameObject自身を指す場合は拒否する。
    let yaml = format!(
        "{}{}{}",
        YAML_DIRECTIVES,
        document(
            1,
            100,
            false,
            "GameObject",
            &game_object_body("A", &[200, 100], 1)
        ),
        document(
            4,
            200,
            false,
            "Transform",
            &transform_body(100, "{fileID: 0}", &[])
        ),
    );
    let file = SerializedFile::parse(yaml.as_bytes(), &ParseOptions::new()).expect("枠は読める");
    let error = file.local_hierarchy().expect_err("自身への参照は失敗する");
    assert_eq!(error.kind(), UnityErrorKind::InvalidHierarchy);

    // componentが他のGameObjectのTransformを指す場合は拒否する。
    let yaml = format!(
        "{}{}{}{}{}",
        YAML_DIRECTIVES,
        document(
            1,
            100,
            false,
            "GameObject",
            &game_object_body("A", &[200, 201], 1)
        ),
        document(
            4,
            200,
            false,
            "Transform",
            &transform_body(100, "{fileID: 0}", &[201]),
        ),
        document(
            1,
            101,
            false,
            "GameObject",
            &game_object_body("B", &[201], 1)
        ),
        document(
            4,
            201,
            false,
            "Transform",
            &transform_body(101, "{fileID: 200}", &[]),
        ),
    );
    let file = SerializedFile::parse(yaml.as_bytes(), &ParseOptions::new()).expect("枠は読める");
    let error = file.local_hierarchy().expect_err("他者の参照は失敗する");
    assert_eq!(error.kind(), UnityErrorKind::InvalidHierarchy);
}

#[test]
fn 数値解決されたguidは拒否し文字列解決だけ受け付ける() {
    // `+` 付き32桁は符号落ちで有効にならない。
    let plus = "+12345678901234567890123456789012";
    assert_eq!(plus.strip_prefix('+').expect("ある").len(), 32);
    // 引用符なしの全数字32桁は数値解決される。
    let digits = "12345678901234567890123456789012";
    assert_eq!(digits.len(), 32);
    // meta内の数値解決GUIDはInvalidMetadataになる。
    for guid_text in [plus, digits] {
        let meta = format!(
            "fileFormatVersion: 2\nguid: {guid_text}\nDefaultImporter:\n  externalObjects: {{}}\n"
        );
        let error = AssetMeta::parse(meta.as_bytes()).expect_err("数値GUIDは失敗する");
        assert_eq!(error.kind(), UnityErrorKind::InvalidMetadata);
    }
    // 本文内の数値解決GUIDはInvalidYamlになる。
    for guid_text in [plus, digits] {
        let yaml = format!(
            "{}{}",
            YAML_DIRECTIVES,
            document(
                114,
                1,
                false,
                "MonoBehaviour",
                &format!(
                    "  m_GameObject: {{fileID: 0}}\n  m_Enabled: 1\n  m_Script: {{fileID: 1, guid: {guid_text}, type: 3}}\n  m_Name: Probe\n  m_EditorClassIdentifier: \n"
                )
            )
        );
        let file =
            SerializedFile::parse(yaml.as_bytes(), &ParseOptions::new()).expect("枠は読める");
        let error = file
            .document(1)
            .expect("ある")
            .decode()
            .expect_err("数値GUIDは失敗する");
        assert_eq!(error.kind(), UnityErrorKind::InvalidYaml);
    }
    // `1e` 形の字面も数値（浮動小数点）解決されて拒否する。
    let exp = "1e000000000000000000000000000001";
    assert_eq!(exp.len(), 32);
    let meta =
        format!("fileFormatVersion: 2\nguid: {exp}\nDefaultImporter:\n  externalObjects: {{}}\n");
    let error = AssetMeta::parse(meta.as_bytes()).expect_err("指数形は失敗する");
    assert_eq!(error.kind(), UnityErrorKind::InvalidMetadata);

    // 通常の32桁hexは引用符なしでも引用符付きでも受理する。
    for guid_text in [
        "9ef94258554e06b4ab03fbe9fc914c5c",
        "\"9ef94258554e06b4ab03fbe9fc914c5c\"",
    ] {
        let meta = format!(
            "fileFormatVersion: 2\nguid: {guid_text}\nDefaultImporter:\n  externalObjects: {{}}\n"
        );
        AssetMeta::parse(meta.as_bytes()).expect("正当なGUIDは読める");
        let yaml = format!(
            "{}{}",
            YAML_DIRECTIVES,
            document(
                114,
                1,
                false,
                "MonoBehaviour",
                &format!(
                    "  m_GameObject: {{fileID: 0}}\n  m_Enabled: 1\n  m_Script: {{fileID: 1, guid: {guid_text}, type: 3}}\n  m_Name: Probe\n  m_EditorClassIdentifier: \n"
                )
            )
        );
        let file =
            SerializedFile::parse(yaml.as_bytes(), &ParseOptions::new()).expect("枠は読める");
        file.document(1).expect("ある").decode().expect("読める");
    }
}

#[test]
fn metaの1件上限を引き上げと引き下げで変えられる() {
    // 4MiB超・8MiB未満のmetaを用意する。
    let body = format!(
        "  externalObjects: {{}}\n  userData: {}\n",
        "x".repeat(5 * 1024 * 1024)
    );
    let meta = file_meta(7, "TextImporter", &body);
    assert!(meta.len() > 4 * 1024 * 1024);
    assert!(meta.len() < 8 * 1024 * 1024);
    let root = tempfile::TempDir::new().expect("親領域は作れる");
    let assets = root.path().join("Assets");
    fs::create_dir(&assets).expect("準備は成功する");
    fs::write(assets.join("blob.bin"), b"x").expect("準備は成功する");
    fs::write(assets.join("blob.bin.meta"), &meta).expect("準備は成功する");

    // 上限8MiBでは成功する（回帰: 内部の固定4MiBで再拒否した）。
    let index = vrctom::unity::scan_directory(
        &assets,
        &IndexOptions::new().with_max_meta_bytes(8 * 1024 * 1024),
    )
    .expect("引き上げた上限では読める");
    assert_eq!(index.entries().len(), 1);

    // 既定（4MiB）では上限超過になる。
    let error =
        vrctom::unity::scan_directory(&assets, &IndexOptions::new()).expect_err("既定では失敗する");
    assert_eq!(error.kind(), UnityErrorKind::LimitExceeded);

    // 引き下げも効く。
    let error =
        vrctom::unity::scan_directory(&assets, &IndexOptions::new().with_max_meta_bytes(10))
            .expect_err("引き下げでは失敗する");
    assert_eq!(error.kind(), UnityErrorKind::LimitExceeded);
}

#[test]
fn 大小文字の衝突はunicodeの小文字化で判定する() {
    // 回帰: ASCII限定の判定では `Ä` と `ä` の衝突を見逃した。
    let mut index = AssetIndex::new();
    index
        .insert(
            "Assets/Ä.prefab",
            AssetMeta::parse(&file_meta(1, "PrefabImporter", PREFAB_IMPORTER_BODY))
                .expect("metaは読める"),
        )
        .expect("挿入は成功する");
    let error = index
        .insert(
            "Assets/ä.prefab",
            AssetMeta::parse(&file_meta(2, "PrefabImporter", PREFAB_IMPORTER_BODY))
                .expect("metaは読める"),
        )
        .expect_err("大小文字の重複は失敗する");
    assert_eq!(error.kind(), UnityErrorKind::Conflict);
}

#[test]
fn 負のclassidは拒否し負のfileidは受け付ける() {
    // UnityのclassIDは非負であり、負値は枠不良として拒否する（契約は `i32`）。
    let yaml = format!(
        "{}{}",
        YAML_DIRECTIVES,
        document(
            -4,
            1,
            false,
            "Transform",
            &transform_body(1, "{fileID: 0}", &[])
        )
    );
    let error =
        SerializedFile::parse(yaml.as_bytes(), &ParseOptions::new()).expect_err("負値は失敗する");
    assert_eq!(error.kind(), UnityErrorKind::InvalidYaml);

    // fileIDの負値は符号付き10進として受け付ける。
    let yaml = format!(
        "{}{}",
        YAML_DIRECTIVES,
        document(
            1,
            -1,
            false,
            "GameObject",
            &game_object_body("Minus", &[], 1)
        )
    );
    let file =
        SerializedFile::parse(yaml.as_bytes(), &ParseOptions::new()).expect("負のfileIDは読める");
    assert_eq!(file.documents().len(), 1);
}

#[test]
fn strippedでもclassidとクラス名の不一致は拒否する() {
    // 回帰: stripped判定が先にあり、明白な不一致を見逃した。
    let yaml = format!(
        "{}{}",
        YAML_DIRECTIVES,
        document(
            1,
            7,
            true,
            "Transform",
            &transform_body(7, "{fileID: 0}", &[])
        )
    );
    let file =
        SerializedFile::parse(yaml.as_bytes(), &ParseOptions::new()).expect("枠自体は読める");
    let error = file
        .document(7)
        .expect("ある")
        .decode()
        .expect_err("不一致は失敗する");
    assert_eq!(error.kind(), UnityErrorKind::TypeMismatch);
}

#[test]
fn ヘッダのみで本文がない文書はpanicせず失敗する() {
    // 回帰: 最終ヘッダ直後に本文が無いと範囲外アクセスでpanicした。
    // 末尾改行あり/なし、LF/CRLF、BOM付き、正常文書+空最終文書を確認する。
    // いずれもpanicせず `InvalidYaml` になる。
    let header_only = "--- !u!1 &1";
    let normal = document(1, 100, false, "GameObject", &game_object_body("X", &[], 1));
    let mut cases: Vec<(&str, String)> = Vec::new();
    cases.push(("末尾改行なし", format!("{YAML_DIRECTIVES}{header_only}")));
    cases.push(("末尾改行あり", format!("{YAML_DIRECTIVES}{header_only}\n")));
    cases.push((
        "CRLF末尾改行なし",
        format!("{YAML_DIRECTIVES}{header_only}").replace('\n', "\r\n"),
    ));
    cases.push((
        "CRLF末尾改行あり",
        format!("{YAML_DIRECTIVES}{header_only}\n").replace('\n', "\r\n"),
    ));
    cases.push((
        "BOM付きヘッダのみ",
        format!("\u{feff}{YAML_DIRECTIVES}{header_only}"),
    ));
    cases.push((
        "正常文書+空最終文書（改行なし）",
        format!("{YAML_DIRECTIVES}{normal}{header_only}"),
    ));
    cases.push((
        "正常文書+空最終文書（改行あり）",
        format!("{YAML_DIRECTIVES}{normal}{header_only}\n"),
    ));
    for (name, yaml) in cases {
        let error = SerializedFile::parse(yaml.as_bytes(), &ParseOptions::new())
            .expect_err("本文なしは失敗する");
        assert_eq!(
            error.kind(),
            UnityErrorKind::InvalidYaml,
            "本文なしはInvalidYamlになる: {name}"
        );
    }
}

#[test]
fn 深い階層のdebug表示でもabortしない() {
    // 回帰: `Debug` の再帰で8,000節点の表示がstack overflowでabortした。
    // 2MiB stackの子スレッドで `{:?}` と `{:#?}` を整形し、abortしないことを確かめる。
    const DEPTH: usize = 8000;
    let mut yaml = YAML_DIRECTIVES.to_owned();
    for i in 0..DEPTH {
        let game_id = 100 + (i as i64) * 2;
        let transform_id = game_id + 1;
        yaml.push_str(&document(
            1,
            game_id,
            false,
            "GameObject",
            &game_object_body(&format!("N{i}"), &[transform_id], 1),
        ));
        let father = if i == 0 {
            "{fileID: 0}".to_owned()
        } else {
            format!("{{fileID: {}}}", transform_id - 2)
        };
        let children: &[i64] = if i + 1 < DEPTH {
            &[transform_id + 2]
        } else {
            &[]
        };
        yaml.push_str(&document(
            4,
            transform_id,
            false,
            "Transform",
            &transform_body(game_id, &father, children),
        ));
    }
    let file = SerializedFile::parse(yaml.as_bytes(), &ParseOptions::new()).expect("枠は読める");
    let hierarchy = file.local_hierarchy().expect("深い鎖は組める");
    let child = std::thread::Builder::new()
        .stack_size(2 * 1024 * 1024)
        .spawn(move || {
            let flat = format!("{hierarchy:?}");
            assert!(flat.contains("N0"), "root名を含む");
            assert!(flat.contains("N7999"), "末端名を含む");
            let pretty = format!("{hierarchy:#?}");
            assert!(pretty.contains("N0"), "pretty表示もroot名を含む");
        })
        .expect("子スレッドは作れる");
    child.join().expect("小スタックでもabortせず表示できる");
}

#[test]
fn bignumの名前は桁を保って読みguidは拒否する() {
    // 回帰: bignumタグ付きの10進 `m_Name` が汎用パース後に `TypeMismatch` になった。
    // 名前は桁を保って受理し、GUIDは数値解決として拒否する。
    const DIGITS: &str = "12345678901234567890123456789012";
    assert!(DIGITS.len() > 20, "u64に収まらない桁数である");
    let yaml = format!(
        "{}{}",
        YAML_DIRECTIVES,
        document(
            1,
            100,
            false,
            "GameObject",
            &format!("  m_Component: []\n  m_Name: {DIGITS}\n  m_IsActive: 1\n"),
        ),
    );
    let file = SerializedFile::parse(yaml.as_bytes(), &ParseOptions::new()).expect("枠は読める");
    let TypedObject::GameObject(game) = file.document(100).expect("ある").decode().expect("読める")
    else {
        panic!("GameObjectになる");
    };
    assert_eq!(game.name, DIGITS, "bignumの桁を保って読む");
    // 引用符付きは従来どおり読める。
    let yaml = format!(
        "{}{}",
        YAML_DIRECTIVES,
        document(
            1,
            101,
            false,
            "GameObject",
            &format!("  m_Component: []\n  m_Name: \"{DIGITS}\"\n  m_IsActive: 1\n"),
        ),
    );
    let file = SerializedFile::parse(yaml.as_bytes(), &ParseOptions::new()).expect("枠は読める");
    let TypedObject::GameObject(game) = file.document(101).expect("ある").decode().expect("読める")
    else {
        panic!("GameObjectになる");
    };
    assert_eq!(game.name, DIGITS);

    // bignumのGUIDは文字列化して受理せず拒否する。
    let yaml = format!(
        "{}{}",
        YAML_DIRECTIVES,
        document(
            114,
            1,
            false,
            "MonoBehaviour",
            &format!(
                "  m_GameObject: {{fileID: 0}}\n  m_Enabled: 1\n  m_Script: {{fileID: 1, guid: {DIGITS}, type: 3}}\n  m_Name: Probe\n  m_EditorClassIdentifier: \n"
            )
        )
    );
    let file =
        SerializedFile::parse(yaml.as_bytes(), &ParseOptions::new()).expect("枠自体は読める");
    let error = file
        .document(1)
        .expect("ある")
        .decode()
        .expect_err("bignumのGUIDは失敗する");
    assert_eq!(error.kind(), UnityErrorKind::InvalidYaml);
}

#[test]
fn yaml対象の拡張子だけを読み非yamlを跳ばす() {
    // 既知YAML拡張子は読み対象、それ以外は読み飛ばし対象である。
    for path in [
        "Assets/Hero.prefab",
        "Assets/Chara.mat",
        "Assets/Walk.anim",
        "Assets/Ctrl.controller",
        "Assets/Data.asset",
        "Assets/RT.renderTexture",
        "Assets/Scene.unity",
        "Assets/UPPER.PREFAB",
    ] {
        assert!(is_unity_yaml_asset(path), "読み対象である: {path}");
    }
    for path in [
        "Assets/Effect.shader",
        "Assets/Script.cs",
        "Assets/Note.txt",
        "Assets/tex.png",
        "Assets/model.fbx",
        "Assets/noext",
    ] {
        assert!(!is_unity_yaml_asset(path), "読み飛ばし対象である: {path}");
    }
    // 非YAMLの本文はパース対象にしない（壊れていても読み飛ばす）。
    assert!(!is_unity_yaml_asset("Assets/Effect.shader"));
    // 壊れた `.prefab` は読み対象であり、パース失敗になる。
    assert!(is_unity_yaml_asset("Assets/Broken.prefab"));
    let broken = format!("{YAML_DIRECTIVES}--- !u!1 &1\nGameObject:\n  m_Name: [\n");
    let error = SerializedFile::parse(broken.as_bytes(), &ParseOptions::new())
        .expect_err("壊れたprefabは失敗する");
    assert_eq!(error.kind(), UnityErrorKind::InvalidYaml);
}

/// ModelImporterのmetaを組み立てる。本文は呼び出し側が渡す。
fn モデルmeta(本文: &str) -> Vec<u8> {
    file_meta(7, "ModelImporter", 本文)
}

/// humanDescriptionのhuman1件のYAMLを作る。
fn 対応項目(ノード名: &str, 骨名: &str) -> String {
    format!("    - boneName: {ノード名}\n      humanName: {骨名}\n")
}

/// human対応表を持つmetaを組み立てる。付帯欄は呼び出し側が渡す。
fn 対応表meta(項目ら: &[String], 付帯: &str) -> Vec<u8> {
    let mut 本文 = String::from("  humanDescription:\n    serializedVersion: 3\n    human:\n");
    for 項目 in 項目ら {
        本文.push_str(項目);
    }
    本文.push_str(付帯);
    モデルmeta(&本文)
}

#[test]
fn 空白入りhuman名とノード名をそのまま保つ() {
    // `.meta` の空白入り表記（`Left Thumb Proximal`・`Left Shoulder`）は
    // 呼び出し側の照合に残すため、そのまま保つ。
    let meta = 対応表meta(
        &[
            対応項目("Wrist.L", "LeftHand"),
            対応項目("Left Shoulder", "LeftShoulder"),
            対応項目("Thumb.L.001", "Left Thumb Intermediate"),
        ],
        "",
    );
    let 資産 = AssetMeta::parse(&meta).expect("対応表は読める");
    let 対応 = 資産.human_bone_mappings().expect("対応表は読める");
    assert_eq!(対応.len(), 3, "3件の対応が読める");
    assert_eq!(対応[0].bone_name, "Wrist.L");
    assert_eq!(対応[0].human_name, "LeftHand");
    assert_eq!(対応[1].bone_name, "Left Shoulder", "ノード名の空白は保つ");
    assert_eq!(対応[1].human_name, "LeftShoulder");
    assert_eq!(対応[2].bone_name, "Thumb.L.001");
    assert_eq!(
        対応[2].human_name, "Left Thumb Intermediate",
        "human名の空白は保つ"
    );
}

#[test]
fn 記述が無い対応表は空を返す() {
    // `humanDescription` が無い場合は名前推測に委ねるため空になる。
    let 無し = モデルmeta("  animationType: 3\n");
    let 資産 = AssetMeta::parse(&無し).expect("対応表なしも読める");
    assert!(
        資産
            .human_bone_mappings()
            .expect("欠落は空になる")
            .is_empty(),
        "humanDescriptionが無い場合は空である"
    );
    // nullの場合も同じである。
    let null = モデルmeta("  humanDescription: null\n  animationType: 3\n");
    let 資産 = AssetMeta::parse(&null).expect("nullも読める");
    assert!(
        資産
            .human_bone_mappings()
            .expect("nullは空になる")
            .is_empty(),
        "humanDescriptionがnullの場合は空である"
    );
    // `human` が空の場合も同じである。
    let 空 = モデルmeta("  humanDescription:\n    serializedVersion: 3\n    human: []\n");
    let 資産 = AssetMeta::parse(&空).expect("空も読める");
    assert!(
        資産.human_bone_mappings().expect("空は空になる").is_empty(),
        "humanが空の場合は空である"
    );
    // `human` 欄自体が無い場合も同じである。
    let human無し = モデルmeta("  humanDescription:\n    serializedVersion: 3\n");
    let 資産 = AssetMeta::parse(&human無し).expect("humanなしも読める");
    assert!(
        資産
            .human_bone_mappings()
            .expect("humanなしは空になる")
            .is_empty(),
        "human欄が無い場合は空である"
    );
}

#[test]
fn 非humanoidの取り込み方式では対応表を採用しない() {
    // 明示的にGenericの場合は対応表があっても採用しない。
    let 項目 = [対応項目("Wrist.L", "LeftHand")];
    let meta = 対応表meta(&項目, "  animationType: 2\n");
    let 資産 = AssetMeta::parse(&meta).expect("非humanoidも読める");
    assert!(
        資産
            .human_bone_mappings()
            .expect("非humanoidは空になる")
            .is_empty(),
        "animationTypeがGenericの場合は空である"
    );
    // 省略時は有効な対応表があれば採用する。
    let meta = 対応表meta(&項目, "");
    let 資産 = AssetMeta::parse(&meta).expect("省略も読める");
    assert_eq!(
        資産.human_bone_mappings().expect("省略は採用する").len(),
        1,
        "animationType省略時は対応表を採用する"
    );
    // Humanの明示は採用する。
    let meta = 対応表meta(&項目, "  animationType: 3\n");
    let 資産 = AssetMeta::parse(&meta).expect("humanoidも読める");
    assert_eq!(
        資産
            .human_bone_mappings()
            .expect("humanoidは採用する")
            .len(),
        1,
        "animationTypeがHumanの場合は対応表を採用する"
    );
}

#[test]
fn 空の対応は無視する() {
    // 空の `boneName` はその骨を補完対象に残すため無視する。
    let meta = 対応表meta(
        &[
            "    - boneName: \n      humanName: LeftHand\n".to_string(),
            対応項目("Hips", "Hips"),
            "    - boneName: Wrist.R\n      humanName: \n".to_string(),
        ],
        "",
    );
    let 資産 = AssetMeta::parse(&meta).expect("空混じりも読める");
    let 対応 = 資産.human_bone_mappings().expect("空は無視する");
    assert_eq!(対応.len(), 1, "空の対応は無視する");
    assert_eq!(対応[0].bone_name, "Hips");
}

#[test]
fn 壊れた対応表の形状は失敗する() {
    // `human` がシーケンスでない場合は壊れている。
    let meta = モデルmeta("  humanDescription:\n    human: {}\n");
    let 資産 = AssetMeta::parse(&meta).expect("meta自体は読める");
    let 失敗 = 資産
        .human_bone_mappings()
        .expect_err("humanが写像なら失敗する");
    assert_eq!(失敗.kind(), UnityErrorKind::InvalidMetadata);
    // 項目が写像でない場合は壊れている。
    let meta = モデルmeta("  humanDescription:\n    human:\n    - LeftHand\n");
    let 資産 = AssetMeta::parse(&meta).expect("meta自体は読める");
    let 失敗 = 資産
        .human_bone_mappings()
        .expect_err("項目が文字列なら失敗する");
    assert_eq!(失敗.kind(), UnityErrorKind::InvalidMetadata);
    // `boneName` が文字列でない場合は壊れている。
    let meta = モデルmeta(
        "  humanDescription:\n    human:\n    - boneName: 1\n      humanName: LeftHand\n",
    );
    let 資産 = AssetMeta::parse(&meta).expect("meta自体は読める");
    let 失敗 = 資産
        .human_bone_mappings()
        .expect_err("boneNameが数値なら失敗する");
    assert_eq!(失敗.kind(), UnityErrorKind::InvalidMetadata);
    // `animationType` が解釈できない形状の場合は壊れている。
    let meta = 対応表meta(&[対応項目("Hips", "Hips")], "  animationType: [3]\n");
    let 資産 = AssetMeta::parse(&meta).expect("meta自体は読める");
    let 失敗 = 資産
        .human_bone_mappings()
        .expect_err("animationTypeが配列なら失敗する");
    assert_eq!(失敗.kind(), UnityErrorKind::InvalidMetadata);
}

#[test]
fn 取り込み方式の整数判定は小数や非有限値を拒否する() {
    // 整数の `3` はHumanoidとして採用し、`2` は非Humanoidとして空になる。
    let 項目 = [対応項目("Hips", "Hips")];
    for (表記, 採用) in [("3", true), ("2", false)] {
        let meta = 対応表meta(&項目, &format!("  animationType: {表記}\n"));
        let 資産 = AssetMeta::parse(&meta).expect("整数metaは読める");
        let 対応 = 資産.human_bone_mappings().expect("整数は判定できる");
        assert_eq!(
            !対応.is_empty(),
            採用,
            "animationType {表記} の採用が変わらない"
        );
    }
    // 小数・非有限値は整数に丸めず、非Humanoidとしても扱わず失敗する。
    for 表記 in ["3.0", "3.5", ".nan", ".inf", "-.inf"] {
        let meta = 対応表meta(&項目, &format!("  animationType: {表記}\n"));
        let 資産 = AssetMeta::parse(&meta).expect("meta自体は読める");
        let 失敗 = 資産
            .human_bone_mappings()
            .expect_err(&format!("animationType {表記} は失敗する"));
        assert_eq!(
            失敗.kind(),
            UnityErrorKind::InvalidMetadata,
            "animationType {表記} は不正な値である"
        );
    }
    // 文字列の既存扱いは変えない。`human`・数値文字列は採用し、
    // `generic`・未知の文字列は従来どおり空・失敗になる。
    let meta = 対応表meta(&項目, "  animationType: human\n");
    let 資産 = AssetMeta::parse(&meta).expect("文字列metaは読める");
    assert_eq!(
        資産
            .human_bone_mappings()
            .expect("human文字列は採用する")
            .len(),
        1,
        "human文字列は採用する"
    );
    let meta = 対応表meta(&項目, "  animationType: generic\n");
    let 資産 = AssetMeta::parse(&meta).expect("文字列metaは読める");
    assert!(
        資産
            .human_bone_mappings()
            .expect("generic文字列は空になる")
            .is_empty(),
        "generic文字列は空である"
    );
}

#[test]
fn 骨名の型違いは文字列でないと診断する() {
    // `boneName: []` のような型違いは「ない」ではなく「文字列でない」と出す。
    // 欠落・null・空文字の許容は変えない。
    let meta = モデルmeta(
        "  humanDescription:\n    human:\n    - boneName: []\n      humanName: LeftHand\n",
    );
    let 資産 = AssetMeta::parse(&meta).expect("meta自体は読める");
    let 失敗 = 資産
        .human_bone_mappings()
        .expect_err("boneNameが配列なら失敗する");
    assert_eq!(失敗.kind(), UnityErrorKind::InvalidMetadata);
    let 文面 = format!("{失敗}");
    assert!(文面.contains("文字列でない"), "型違いの診断になる: {文面}");
    assert!(!文面.contains("がない"), "欠落の診断と区別する: {文面}");
}
