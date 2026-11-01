//! Unity材質の一覧の任意扱いの契約テスト（公開 API のみを使う）。
//!
//! `m_SavedProperties` の4一覧（テクスチャ・浮動小数点・整数・色）は、
//! 項目を持たない場合に欄ごと書かない材質がある（lilToonなど）。
//! 欠落とnullは空として読み、存在する欄の形状違いは失敗にする。
//! 実fixtureを使う検証は `VRCTOM_MANUKA_DIR` が指す展開済みディレクトリだけを読み、
//! 未設定なら跳ばす。

use vrctom::unity::{ParseOptions, SerializedFile, TypedObject};

// テスト名は日本語で書く（テストは仕様・契約であるため）。

/// Unity YAMLの先頭directive。
const YAML_DIRECTIVES: &str = "%YAML 1.1\n%TAG !u! tag:unity3d.com,2011:\n";

/// 1文書を組み立てる。
fn document(
    class_id: impl std::fmt::Display,
    file_id: impl std::fmt::Display,
    class_name: &str,
    body: &str,
) -> String {
    format!("--- !u!{class_id} &{file_id}\n{class_name}:\n{body}")
}

/// `m_SavedProperties` の4一覧を指定どおりに持つ材質文書を読み、型付き材質を返す。
/// `None` の欄は書かず、`Some(None)` の欄は `null` 値として書く。
fn decode_material_with_lists(
    tex_envs: Option<Option<&str>>,
    floats: Option<Option<&str>>,
    ints: Option<Option<&str>>,
    colors: Option<Option<&str>>,
) -> Result<vrctom::unity::Material, vrctom::unity::UnityError> {
    fn section(key: &str, body: Option<Option<&str>>) -> String {
        match body {
            None => String::new(),
            Some(None) => format!("    {key}:\n"),
            Some(Some(items)) => format!("    {key}:\n{items}"),
        }
    }
    let body = format!(
        "  m_ObjectHideFlags: 0\n  m_Name: Green\n  m_Shader: {{fileID: 0}}\n  m_CustomRenderQueue: -1\n  m_SavedProperties:\n    serializedVersion: 3\n{}",
        [
            section("m_TexEnvs", tex_envs),
            section("m_Floats", floats),
            section("m_Ints", ints),
            section("m_Colors", colors),
        ]
        .concat()
    );
    let yaml = format!(
        "{}{}",
        YAML_DIRECTIVES,
        document(21, 2100000, "Material", &body)
    );
    let file = SerializedFile::parse(yaml.as_bytes(), &ParseOptions::new()).expect("読める");
    match file.document(2100000).expect("ある").decode() {
        Ok(TypedObject::Material(material)) => Ok(material),
        Ok(_) => panic!("Materialになる"),
        Err(error) => Err(error),
    }
}

/// 材質一覧4欄の有無。`None` は欄ごと欠落、`Some(None)` はnull値である。
type ListSpecs = (
    Option<Option<&'static str>>,
    Option<Option<&'static str>>,
    Option<Option<&'static str>>,
    Option<Option<&'static str>>,
);

/// lilToonの実データ（`m_Ints` 欠落、他は空でない）を模した本文。
fn liltoon_like_lists() -> ListSpecs {
    (
        Some(Some(
            "    - _MainTex:\n        m_Texture: {fileID: 0}\n        m_Scale: {x: 1, y: 1}\n        m_Offset: {x: 0, y: 0}\n",
        )),
        Some(Some("    - _Cutoff: 0.5\n")),
        None,
        Some(Some("    - _Color: {r: 0, g: 1, b: 0, a: 1}\n")),
    )
}

#[test]
fn ints欠落の材質は空として読める() {
    // lilToonの実データは `m_Ints` 欄を持たない。欠落は空と同じ意味である。
    let (tex_envs, floats, ints, colors) = liltoon_like_lists();
    let material = decode_material_with_lists(tex_envs, floats, ints, colors).expect("読める");
    assert_eq!(material.name, "Green");
    assert_eq!(material.tex_envs.len(), 1);
    assert_eq!(material.floats.len(), 1);
    assert!(material.ints.is_empty());
    assert_eq!(material.colors.len(), 1);
    assert_eq!(material.floats[0].value, 0.5);
    assert_eq!(material.colors[0].value, [0.0, 1.0, 0.0, 1.0]);
}

#[test]
fn 四つの一覧の欠落とnullは空になる() {
    // 4欄とも無い材質はすべて空として読める。
    let material = decode_material_with_lists(None, None, None, None).expect("読める");
    assert!(material.tex_envs.is_empty());
    assert!(material.floats.is_empty());
    assert!(material.ints.is_empty());
    assert!(material.colors.is_empty());
    // `null` 値（`m_Ints:` のように値が無い場合）も空として読める。
    let material =
        decode_material_with_lists(Some(None), Some(None), Some(None), Some(None)).expect("読める");
    assert!(material.tex_envs.is_empty());
    assert!(material.floats.is_empty());
    assert!(material.ints.is_empty());
    assert!(material.colors.is_empty());
}

#[test]
fn 存在する一覧の形状違いは失敗する() {
    // 欠落の寛容は、存在する欄の形状違い（シーケンスでない）まで広げない。
    let material = decode_material_with_lists(
        Some(Some("      a: 1\n")),
        Some(Some("    - _Cutoff: 0.5\n")),
        None,
        None,
    );
    assert!(material.is_err(), "写像でない一覧は失敗する: {material:?}");
}

#[test]
fn 空の一覧は従来どおり空になる() {
    // `m_Ints: []` のように空シーケンスが書かれている場合も空である。
    let material = decode_material_with_lists(
        Some(Some("")),
        Some(Some("")),
        Some(Some("")),
        Some(Some("")),
    )
    .expect("読める");
    assert!(material.tex_envs.is_empty());
    assert!(material.floats.is_empty());
    assert!(material.ints.is_empty());
    assert!(material.colors.is_empty());
}

/// `VRCTOM_MANUKA_DIR` が指す展開済みUnityプロジェクトで、lilToon材質を読む。
///
/// 実データの `.mat` は `m_Ints` 欄を持たず、写像はMToonになる。
/// 未設定なら跳ばす。
#[test]
fn 実fixtureのliltoon材質はmtoonになる() {
    let Some(dir) = std::env::var_os("VRCTOM_MANUKA_DIR") else {
        eprintln!("VRCTOM_MANUKA_DIR未設定のため跳ばす");
        return;
    };
    let root = std::path::PathBuf::from(dir);
    let fbx = root.join("Assets/MANUKA/FBX/MANUKA.fbx");
    let (mats, report) =
        vrctom::vrm::VrmMaterials::load(&root, &fbx, &vrctom::vrm::MaterialOptions::new())
            .expect("材質は読める");
    let mats = mats.expect("置換表がある");
    assert!(
        !mats.entries().is_empty(),
        "材質項目がある: {:?}",
        report.warnings()
    );
    for entry in mats.entries() {
        assert!(
            entry.material.mtoon.is_some(),
            "lilToon材質はMToonになる: {}",
            entry.slot
        );
    }
    assert!(
        !report
            .warnings()
            .iter()
            .any(|warning| warning.contains("材質文書がない")),
        "失敗理由の取り違えが無い: {:?}",
        report.warnings()
    );
}
