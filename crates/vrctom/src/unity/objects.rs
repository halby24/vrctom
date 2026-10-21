//! 代表型の型付き読み取り。
//!
//! 公開契約の入口は [`crate::unity`] を参照。
//!
//! 未知クラス・未知フィールドは捨てない。型付きで読めない文書は
//! [`TypedObject::Unknown`] として扱い、元データは [`Document`] に残る。
//! 意味上必須の欄の欠落を `Default` で埋めることはしない。

use yaml_serde::{Mapping, Value};

use super::{Document, ObjectRef, UnityError, UnityErrorKind};

/// 型付き読み取りの結果。
///
/// 代表型・`Stripped`・未知を区別する。未知でも元文書に全データが残るため、
/// 情報は失われない。
#[non_exhaustive]
#[derive(Debug, Clone)]
pub enum TypedObject {
    /// ゲームオブジェクト。
    GameObject(GameObject),
    /// 位置・回転・拡大縮小と親子関係。
    Transform(Transform),
    /// スキンドメッシュ描画設定。
    SkinnedMeshRenderer(SkinnedMeshRenderer),
    /// マテリアル。
    Material(Material),
    /// スクリプト参照と独自欄を持つ振る舞い。
    MonoBehaviour(MonoBehaviour),
    /// prefabの実体参照。
    PrefabInstance(PrefabInstance),
    /// `stripped` 文書の参照先。
    Stripped(PrefabLink),
    /// 代表型に当てはまらない文書。
    Unknown,
}

/// ゲームオブジェクト。
#[derive(Debug, Clone)]
pub struct GameObject {
    /// オブジェクト名。
    pub name: String,
    /// 有効かどうか。
    pub active: bool,
    /// 構成要素への参照。
    pub components: Vec<ObjectRef>,
}

/// 位置・回転・拡大縮小と親子関係。
///
/// 回転はQuaternionの `x, y, z, w` 順のまま保持し、変換や正規化はしない。
#[derive(Debug, Clone)]
pub struct Transform {
    /// 所属するゲームオブジェクトへの参照。
    pub game_object: ObjectRef,
    /// 親Transformへの参照。nullの場合はrootである。
    pub father: ObjectRef,
    /// 子Transformへの参照。順序はファイル内の `m_Children` のまま保つ。
    pub children: Vec<ObjectRef>,
    /// ローカル位置。
    pub position: [f64; 3],
    /// ローカル回転（`x, y, z, w` 順）。
    pub rotation: [f64; 4],
    /// ローカル拡大縮小。
    pub scale: [f64; 3],
}

/// スキンドメッシュ描画設定。
#[derive(Debug, Clone)]
pub struct SkinnedMeshRenderer {
    /// 所属するゲームオブジェクトへの参照。
    pub game_object: ObjectRef,
    /// メッシュへの参照。nullの場合もある。
    pub mesh: ObjectRef,
    /// マテリアルへの参照列。
    pub materials: Vec<ObjectRef>,
    /// ボーンへの参照列。
    pub bones: Vec<ObjectRef>,
    /// ルートボーンへの参照。
    pub root_bone: ObjectRef,
    /// ブレンドシェイプの重み。欄がない版では `None` になる。
    pub blend_shape_weights: Option<Vec<f64>>,
    /// 有効かどうか。
    pub enabled: bool,
}

/// テクスチャ環境（`m_TexEnvs` の1項目の値部）。
#[derive(Debug, Clone)]
pub struct TextureEnvironment {
    /// テクスチャへの参照。
    pub texture: ObjectRef,
    /// 繰り返し量。
    pub scale: [f64; 2],
    /// ずらし量。
    pub offset: [f64; 2],
}

/// 順序付きの名前付きプロパティ。
///
/// マテリアルの一覧は順序と重複を保つため、写像へ潰さず列のまま持つ。
/// リスト内の同名項目は別物として区別する。
#[derive(Debug, Clone)]
pub struct NamedProperty<T> {
    /// プロパティ名。
    pub name: String,
    /// 値。
    pub value: T,
}

/// マテリアル。
#[derive(Debug, Clone)]
pub struct Material {
    /// マテリアル名。
    pub name: String,
    /// シェーダへの参照。
    pub shader: ObjectRef,
    /// テクスチャ環境の一覧。順序と重複を保つ。
    pub tex_envs: Vec<NamedProperty<TextureEnvironment>>,
    /// 浮動小数点プロパティの一覧。順序と重複を保つ。
    pub floats: Vec<NamedProperty<f64>>,
    /// 整数プロパティの一覧。順序と重複を保つ。
    pub ints: Vec<NamedProperty<i64>>,
    /// 色プロパティの一覧。順序と重複を保つ。色は `r, g, b, a` 順の `f64` である。
    pub colors: Vec<NamedProperty<[f64; 4]>>,
}

/// スクリプト参照と独自欄を持つ振る舞い。
///
/// 共通欄（スクリプト・所属・有効・名前）を型付きで持ち、
/// 独自欄は汎用データのまま [`MonoBehaviour::extra`] に残す。
#[derive(Debug, Clone)]
pub struct MonoBehaviour {
    /// スクリプトへの参照。
    pub script: ObjectRef,
    /// 所属するゲームオブジェクトへの参照。`.asset` 内ではnullの場合もある。
    pub game_object: ObjectRef,
    /// 有効かどうか。
    pub enabled: bool,
    /// 名前。
    pub name: String,
    /// 共通欄以外の独自欄。順序を保ったまま残す。
    pub extra: Mapping,
}

/// prefabの実体参照。
///
/// 参照元のprefabへの参照を型付きで持ち、改変内容などの残りは
/// 汎用データのまま [`PrefabInstance::extra`] に残す。
#[derive(Debug, Clone)]
pub struct PrefabInstance {
    /// 参照元のprefabへの参照。
    pub source: ObjectRef,
    /// 参照以外の改変内容など。順序を保ったまま残す。
    pub extra: Mapping,
}

/// `stripped` 文書の参照先。
///
/// 対応元オブジェクトへの参照を型付きで持ち、残りは
/// 汎用データのまま [`PrefabLink::extra`] に残す。
#[derive(Debug, Clone)]
pub struct PrefabLink {
    /// 対応元オブジェクトへの参照。
    pub corresponding_source: ObjectRef,
    /// 対応元以外の欄。順序を保ったまま残す。
    pub extra: Mapping,
}

/// 既知クラス名とclassIDの対応。
///
/// 型付き読み取りで明白な不一致を検出するための表である。
/// 未知の組み合わせは [`TypedObject::Unknown`] として扱い、エラーにしない。
const KNOWN_CLASSES: [(i32, &str); 6] = [
    (1, "GameObject"),
    (4, "Transform"),
    (21, "Material"),
    (114, "MonoBehaviour"),
    (137, "SkinnedMeshRenderer"),
    (1001, "PrefabInstance"),
];

/// 文書を代表型に読み替える。
///
/// classIDとクラス名の明白な不一致は、`stripped` より先に検出する。
/// 対応元への参照（`stripped` 文書）であっても、枠の対応関係は保つ。
pub(crate) fn decode_document(document: &Document) -> Result<TypedObject, UnityError> {
    let raw = document.raw().as_mapping().expect("文書本体は写像である");
    let known_id = KNOWN_CLASSES
        .iter()
        .find(|(_, name)| *name == document.class_name())
        .map(|(id, _)| *id);
    let known_name = KNOWN_CLASSES
        .iter()
        .find(|(id, _)| *id == document.class_id())
        .map(|(_, name)| *name);
    match (known_id, known_name) {
        // 両方既知で一致する場合だけ型付きに読む。
        (Some(id), Some(name)) if id == document.class_id() && name == document.class_name() => {}
        // 片方だけ既知の場合は明白な不一致として拒否する。
        (Some(_), _) | (_, Some(_)) => {
            return Err(UnityError::new(
                UnityErrorKind::TypeMismatch,
                format!(
                    "classIDとクラス名が一致しない: {}は{}でない",
                    document.class_id(),
                    document.class_name()
                ),
            )
            .with_file_id(document.file_id())
            .with_class_id(document.class_id()));
        }
        // 両方未知の場合は汎用扱いにする。
        (None, None) => {}
    }
    if document.stripped() {
        return Ok(TypedObject::Stripped(decode_prefab_link(document, raw)?));
    }
    let typed = match document.class_id() {
        1 => TypedObject::GameObject(decode_game_object(document, raw)?),
        4 => TypedObject::Transform(decode_transform(document, raw)?),
        21 => TypedObject::Material(decode_material(document, raw)?),
        114 => TypedObject::MonoBehaviour(decode_mono_behaviour(document, raw)?),
        137 => TypedObject::SkinnedMeshRenderer(decode_skinned_mesh_renderer(document, raw)?),
        1001 => TypedObject::PrefabInstance(decode_prefab_instance(document, raw)?),
        _ => TypedObject::Unknown,
    };
    Ok(typed)
}

/// 必須欄を取り出す。欠落を `Default` で埋めることはしない。
fn required<'a>(
    document: &Document,
    mapping: &'a Mapping,
    key: &str,
) -> Result<&'a Value, UnityError> {
    mapping.get(key).ok_or_else(|| {
        UnityError::new(UnityErrorKind::TypeMismatch, format!("必須欄がない: {key}"))
            .with_file_id(document.file_id())
            .with_class_id(document.class_id())
            .with_field(key.to_owned())
    })
}

/// 0/1フラグまたは真偽値を真偽値として読む。
fn read_flag(document: &Document, value: &Value, key: &str) -> Result<bool, UnityError> {
    match value {
        Value::Bool(flag) => Ok(*flag),
        Value::Number(number) => match number.as_i64() {
            Some(0) => Ok(false),
            Some(1) => Ok(true),
            _ => Err(shape_error(document, key, "0/1でない")),
        },
        _ => Err(shape_error(document, key, "真偽値でない")),
    }
}

/// 数値を `f64` として読む。整数も受け付ける。
fn read_f64(document: &Document, value: &Value, key: &str) -> Result<f64, UnityError> {
    match value {
        Value::Number(number) => number
            .as_f64()
            .ok_or_else(|| shape_error(document, key, "数値でない")),
        _ => Err(shape_error(document, key, "数値でない")),
    }
}

/// 整数を `i64` として読む。浮動小数点経由では読まない。
fn read_i64(document: &Document, value: &Value, key: &str) -> Result<i64, UnityError> {
    match value {
        Value::Number(number) => number
            .as_i64()
            .ok_or_else(|| shape_error(document, key, "整数でない")),
        _ => Err(shape_error(document, key, "整数でない")),
    }
}

/// 名前欄を読む。
///
/// UnityのYAML送出は数値に見える名前を引用符なしで書く（`m_Name: 6`）ことがあり、
/// 空名は値なし（`m_Name:`）で書く。名前の概念は文字列であるため、
/// 文字列はそのまま、数値は文字形に、空値は空文字列に読む。
/// 64bitに収まらない10進整数（`!vrctom:bignum` の印付き）は桁を保って受理する。
/// GUIDとは異なり、名前は数値解決の値を文字列化しても桁落ちがないためである。
/// 欄自体がない場合の `Default` 埋めはしない。
fn read_name(document: &Document, value: &Value, key: &str) -> Result<String, UnityError> {
    if let Some(digits) = super::yaml::bignum_digits(value) {
        return Ok(digits.to_owned());
    }
    match value {
        Value::Null => Ok(String::new()),
        Value::String(name) => Ok(name.clone()),
        Value::Number(number) => {
            if let Some(int) = number.as_i64() {
                Ok(int.to_string())
            } else if let Some(int) = number.as_u64() {
                Ok(int.to_string())
            } else if let Some(float) = number.as_f64() {
                // `Display` は短い往復形であり、名前の表示用途に足る。
                Ok(format!("{float}"))
            } else {
                Err(shape_error(document, key, "文字列でない"))
            }
        }
        _ => Err(shape_error(document, key, "文字列でない")),
    }
}

/// シーケンスを読む。
fn read_sequence<'a>(
    document: &Document,
    mapping: &'a Mapping,
    key: &str,
) -> Result<&'a Vec<Value>, UnityError> {
    required(document, mapping, key)?
        .as_sequence()
        .ok_or_else(|| shape_error(document, key, "シーケンスでない"))
}

/// 写像を読む。
fn read_mapping<'a>(
    document: &Document,
    mapping: &'a Mapping,
    key: &str,
) -> Result<&'a Mapping, UnityError> {
    required(document, mapping, key)?
        .as_mapping()
        .ok_or_else(|| shape_error(document, key, "マッピングでない"))
}

/// 形状違いのエラーを作る。
fn shape_error(document: &Document, key: &str, detail: &str) -> UnityError {
    UnityError::new(
        UnityErrorKind::TypeMismatch,
        format!("欄の形状が合わない: {key}: {detail}"),
    )
    .with_file_id(document.file_id())
    .with_class_id(document.class_id())
    .with_field(key.to_owned())
}

/// 参照欄を読む。
fn read_ref(document: &Document, mapping: &Mapping, key: &str) -> Result<ObjectRef, UnityError> {
    ObjectRef::decode_body(required(document, mapping, key)?).map_err(|error| {
        // 参照の診断に文書と欄を添える。GUID不良の種別は変えない。
        attach_context(error, document, key)
    })
}

/// 参照のシーケンスを読む。
fn read_ref_list(
    document: &Document,
    mapping: &Mapping,
    key: &str,
) -> Result<Vec<ObjectRef>, UnityError> {
    read_sequence(document, mapping, key)?
        .iter()
        .map(|value| {
            ObjectRef::decode_body(value).map_err(|error| attach_context(error, document, key))
        })
        .collect()
}

/// エラーに文書と欄の情報を添える。種別は変えない。
fn attach_context(error: UnityError, document: &Document, key: &str) -> UnityError {
    let mut out = error
        .with_file_id(document.file_id())
        .with_class_id(document.class_id());
    if out.field().is_none() {
        out = out.with_field(key.to_owned());
    }
    out
}

/// 消費済み以外の欄を順序通りに残す。
fn extra_of(mapping: &Mapping, consumed: &[&str]) -> Mapping {
    mapping
        .iter()
        .filter(|(key, _)| !key.as_str().is_some_and(|name| consumed.contains(&name)))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

/// `{x, y}` 形を読む。
fn read_xy(document: &Document, value: &Value, key: &str) -> Result<[f64; 2], UnityError> {
    let mapping = value
        .as_mapping()
        .ok_or_else(|| shape_error(document, key, "xとyの写像でない"))?;
    let x = mapping
        .get("x")
        .map(|value| read_f64(document, value, key))
        .transpose()?
        .ok_or_else(|| shape_error(document, key, "xがない"))?;
    let y = mapping
        .get("y")
        .map(|value| read_f64(document, value, key))
        .transpose()?
        .ok_or_else(|| shape_error(document, key, "yがない"))?;
    Ok([x, y])
}

/// `{x, y, z}` 形を読む。
fn read_xyz(document: &Document, value: &Value, key: &str) -> Result<[f64; 3], UnityError> {
    let mapping = value
        .as_mapping()
        .ok_or_else(|| shape_error(document, key, "xとyとzの写像でない"))?;
    let mut out = [0.0; 3];
    for (index, axis) in ["x", "y", "z"].iter().enumerate() {
        out[index] = mapping
            .get(*axis)
            .map(|value| read_f64(document, value, key))
            .transpose()?
            .ok_or_else(|| shape_error(document, key, &format!("{axis}がない")))?;
    }
    Ok(out)
}

/// `{x, y, z, w}` 形を読む。Quaternionの `x, y, z, w` 順を維持する。
fn read_xyzw(document: &Document, value: &Value, key: &str) -> Result<[f64; 4], UnityError> {
    let mapping = value
        .as_mapping()
        .ok_or_else(|| shape_error(document, key, "xとyとzとwの写像でない"))?;
    let mut out = [0.0; 4];
    for (index, axis) in ["x", "y", "z", "w"].iter().enumerate() {
        out[index] = mapping
            .get(*axis)
            .map(|value| read_f64(document, value, key))
            .transpose()?
            .ok_or_else(|| shape_error(document, key, &format!("{axis}がない")))?;
    }
    Ok(out)
}

/// `{r, g, b, a}` 形を読む。
fn read_rgba(document: &Document, value: &Value, key: &str) -> Result<[f64; 4], UnityError> {
    let mapping = value
        .as_mapping()
        .ok_or_else(|| shape_error(document, key, "rとgとbとaの写像でない"))?;
    let mut out = [0.0; 4];
    for (index, channel) in ["r", "g", "b", "a"].iter().enumerate() {
        out[index] = mapping
            .get(*channel)
            .map(|value| read_f64(document, value, key))
            .transpose()?
            .ok_or_else(|| shape_error(document, key, &format!("{channel}がない")))?;
    }
    Ok(out)
}

/// `- 名前: 値` 形の1件列を順序通りに読む。重複名は別物として保つ。
fn read_named_list<T>(
    document: &Document,
    items: &[Value],
    key: &str,
    read_value: impl Fn(&Document, &Value, &str) -> Result<T, UnityError>,
) -> Result<Vec<NamedProperty<T>>, UnityError> {
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let mapping = item
            .as_mapping()
            .filter(|mapping| mapping.len() == 1)
            .ok_or_else(|| shape_error(document, key, "名前付き項目でない"))?;
        let (name, value) = mapping.iter().next().expect("1件あることは確かめた");
        let name = name
            .as_str()
            .ok_or_else(|| shape_error(document, key, "項目名が文字列でない"))?;
        out.push(NamedProperty {
            name: name.to_owned(),
            value: read_value(document, value, key)?,
        });
    }
    Ok(out)
}

/// GameObjectの共通欄を読む。
fn decode_game_object(document: &Document, mapping: &Mapping) -> Result<GameObject, UnityError> {
    let name = read_name(document, required(document, mapping, "m_Name")?, "m_Name")?;
    let active = read_flag(
        document,
        required(document, mapping, "m_IsActive")?,
        "m_IsActive",
    )?;
    let mut components = Vec::new();
    for item in read_sequence(document, mapping, "m_Component")? {
        let entry = item
            .as_mapping()
            .ok_or_else(|| shape_error(document, "m_Component", "component項目でない"))?;
        components.push(
            ObjectRef::decode_body(
                entry
                    .get("component")
                    .ok_or_else(|| shape_error(document, "m_Component", "componentがない"))?,
            )
            .map_err(|error| attach_context(error, document, "m_Component"))?,
        );
    }
    Ok(GameObject {
        name,
        active,
        components,
    })
}

/// Transformの共通欄を読む。
fn decode_transform(document: &Document, mapping: &Mapping) -> Result<Transform, UnityError> {
    Ok(Transform {
        game_object: read_ref(document, mapping, "m_GameObject")?,
        father: read_ref(document, mapping, "m_Father")?,
        children: read_ref_list(document, mapping, "m_Children")?,
        position: read_xyz(
            document,
            required(document, mapping, "m_LocalPosition")?,
            "m_LocalPosition",
        )?,
        rotation: read_xyzw(
            document,
            required(document, mapping, "m_LocalRotation")?,
            "m_LocalRotation",
        )?,
        scale: read_xyz(
            document,
            required(document, mapping, "m_LocalScale")?,
            "m_LocalScale",
        )?,
    })
}

/// SkinnedMeshRendererの共通欄を読む。
fn decode_skinned_mesh_renderer(
    document: &Document,
    mapping: &Mapping,
) -> Result<SkinnedMeshRenderer, UnityError> {
    let blend_shape_weights = match mapping.get("m_BlendShapeWeights") {
        None | Some(Value::Null) => None,
        Some(value) => Some(
            value
                .as_sequence()
                .ok_or_else(|| shape_error(document, "m_BlendShapeWeights", "シーケンスでない"))?
                .iter()
                .map(|entry| read_f64(document, entry, "m_BlendShapeWeights"))
                .collect::<Result<Vec<_>, _>>()?,
        ),
    };
    Ok(SkinnedMeshRenderer {
        game_object: read_ref(document, mapping, "m_GameObject")?,
        mesh: read_ref(document, mapping, "m_Mesh")?,
        materials: read_ref_list(document, mapping, "m_Materials")?,
        bones: read_ref_list(document, mapping, "m_Bones")?,
        root_bone: read_ref(document, mapping, "m_RootBone")?,
        blend_shape_weights,
        enabled: read_flag(
            document,
            required(document, mapping, "m_Enabled")?,
            "m_Enabled",
        )?,
    })
}

/// テクスチャ環境を読む。
fn read_texture_environment(
    document: &Document,
    value: &Value,
    key: &str,
) -> Result<TextureEnvironment, UnityError> {
    let mapping = value
        .as_mapping()
        .ok_or_else(|| shape_error(document, key, "テクスチャ環境でない"))?;
    Ok(TextureEnvironment {
        texture: ObjectRef::decode_body(
            mapping
                .get("m_Texture")
                .ok_or_else(|| shape_error(document, key, "m_Textureがない"))?,
        )
        .map_err(|error| attach_context(error, document, key))?,
        scale: read_xy(
            document,
            mapping
                .get("m_Scale")
                .ok_or_else(|| shape_error(document, key, "m_Scaleがない"))?,
            key,
        )?,
        offset: read_xy(
            document,
            mapping
                .get("m_Offset")
                .ok_or_else(|| shape_error(document, key, "m_Offsetがない"))?,
            key,
        )?,
    })
}

/// Materialの共通欄を読む。
fn decode_material(document: &Document, mapping: &Mapping) -> Result<Material, UnityError> {
    let saved = read_mapping(document, mapping, "m_SavedProperties")?;
    let tex_envs = read_named_list(
        document,
        read_sequence(document, saved, "m_TexEnvs")?,
        "m_TexEnvs",
        read_texture_environment,
    )?;
    let floats = read_named_list(
        document,
        read_sequence(document, saved, "m_Floats")?,
        "m_Floats",
        read_f64,
    )?;
    let ints = read_named_list(
        document,
        read_sequence(document, saved, "m_Ints")?,
        "m_Ints",
        read_i64,
    )?;
    let colors = read_named_list(
        document,
        read_sequence(document, saved, "m_Colors")?,
        "m_Colors",
        read_rgba,
    )?;
    Ok(Material {
        name: read_name(document, required(document, mapping, "m_Name")?, "m_Name")?,
        shader: read_ref(document, mapping, "m_Shader")?,
        tex_envs,
        floats,
        ints,
        colors,
    })
}

/// MonoBehaviourの共通欄を読む。独自欄は汎用データのまま残す。
fn decode_mono_behaviour(
    document: &Document,
    mapping: &Mapping,
) -> Result<MonoBehaviour, UnityError> {
    const CONSUMED: &[&str] = &[
        "m_ObjectHideFlags",
        "m_CorrespondingSourceObject",
        "m_PrefabInstance",
        "m_PrefabAsset",
        "m_GameObject",
        "m_Enabled",
        "m_EditorHideFlags",
        "m_Script",
        "m_Name",
        "m_EditorClassIdentifier",
    ];
    Ok(MonoBehaviour {
        script: read_ref(document, mapping, "m_Script")?,
        game_object: read_ref(document, mapping, "m_GameObject")?,
        enabled: read_flag(
            document,
            required(document, mapping, "m_Enabled")?,
            "m_Enabled",
        )?,
        name: read_name(document, required(document, mapping, "m_Name")?, "m_Name")?,
        extra: extra_of(mapping, CONSUMED),
    })
}

/// PrefabInstanceの共通欄を読む。改変内容は汎用データのまま残す。
fn decode_prefab_instance(
    document: &Document,
    mapping: &Mapping,
) -> Result<PrefabInstance, UnityError> {
    const CONSUMED: &[&str] = &["m_SourcePrefab"];
    Ok(PrefabInstance {
        source: read_ref(document, mapping, "m_SourcePrefab")?,
        extra: extra_of(mapping, CONSUMED),
    })
}

/// `stripped` 文書の参照先を読む。
fn decode_prefab_link(document: &Document, mapping: &Mapping) -> Result<PrefabLink, UnityError> {
    const CONSUMED: &[&str] = &["m_CorrespondingSourceObject"];
    Ok(PrefabLink {
        corresponding_source: read_ref(document, mapping, "m_CorrespondingSourceObject")?,
        extra: extra_of(mapping, CONSUMED),
    })
}
