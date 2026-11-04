//! Prefabの `VRCAvatarDescriptor` からVRM 1.0表情の束ねを作る。
//!
//! 入口は [`expressions_from_avatar_descriptor`] であり、Unityを回さず
//! descriptorの既定生成（参照実装のSoulFlame VRM Exporterと同じ対応）
//! だけを扱う。ベースライン焼き込み・複数メッシュ束ね・法線デルタ・
//! カスタム式・override・ARKitは対象外である。descriptor由来の対応も
//! [`VrmExpressionProfile`] で表し、適用先メッシュのヒントと分離する。
//!
//! ```mermaid
//! flowchart LR
//!     prefab[SerializedFile<br/>Prefab] --> parse[parse_descriptor<br/>lipSyncと目設定]
//!     fbx[FbxFile<br/>ModelとBlendShape] --> table[renderer対応表<br/>GUIDとinstanceを検証]
//!     fbx --> channels[チャンネル名列<br/>FBX順]
//!     parse --> resolve[参照の解決<br/>外部・danglingは失敗]
//!     table --> resolve
//!     resolve --> plan[build_plan<br/>実在候補と警告]
//!     channels --> plan
//! ```

use std::collections::{HashMap, HashSet};

use yaml_serde::Value;

use super::prefab_hierarchy_paths;
use super::{VrmError, VrmErrorKind, VrmExpressionBinding, VrmExpressionProfile};
use crate::fbx::{FbxFile, ObjectId};
use crate::unity::{
    FileId, ObjectRef, SerializedFile, TypedObject, mesh_file_id, skinned_mesh_renderer_file_id,
};

/// descriptorから求めた表情の計画。
///
/// 束ねの生成条件を満たさない場合は空の対応と警告だけを返す。
/// 呼び出し側は束ねが非空の場合だけ中間モデルへ渡す。
#[derive(Debug, Clone)]
pub struct DescriptorExpressionPlan {
    /// 式とターゲットの対応付け。順序は aa・ih・ou・ee・oh・blink・lookUp・lookDown である。
    pub profile: VrmExpressionProfile,
    /// 適用先のメッシュ。候補が無い場合は `None` である。
    pub mesh: Option<ObjectId>,
    /// 生成しなかった理由などの警告文。本文だけを持ち、見出しは呼び出し側が付ける。
    pub warnings: Vec<String>,
}

/// Prefab内の `VRCAvatarDescriptor` から表情の計画を作る。
///
/// `fbx_guid` は入力FBXの `.fbx.meta` のGUIDであり、PrefabInstanceの特定と
/// renderer参照の出所検証に使う。descriptorが無い場合は警告1件と空の対応を
/// 返し、続行できるようにする。曖昧な参照・外部参照・danglingは
/// 部分成功にせず失敗する。警告文は日本語で理由が分かるように書く。
/// panicは使わない。
///
/// # Errors
///
/// `VRCAvatarDescriptor` が複数ある場合・入力FBXへのPrefabInstanceが複数ある場合・
/// 参照の形状不良・外部参照・dangling・解決不能・別メッシュの両候補・
/// 階層パスが作れない場合に返す。
pub fn expressions_from_avatar_descriptor(
    prefab: &SerializedFile,
    prefab_label: &str,
    fbx: &FbxFile<'_>,
    fbx_guid: &str,
    humanoid_eyes: bool,
) -> Result<DescriptorExpressionPlan, VrmError> {
    let Some(settings) = parse_descriptor(prefab, prefab_label)? else {
        return Ok(DescriptorExpressionPlan {
            profile: VrmExpressionProfile {
                bindings: Vec::new(),
            },
            mesh: None,
            warnings: vec!["VRCAvatarDescriptorが無いため表情を作らない".to_owned()],
        });
    };
    // FBXのモデル列から取り込みID表を作る。prefab内の参照解決に使う。
    let nodes = prefab_hierarchy_paths(fbx)?;
    let mut mesh_ids: HashMap<FileId, ObjectId> = HashMap::new();
    for model in fbx.models() {
        let Some(geometry) = model.mesh else {
            continue;
        };
        let fid = mesh_file_id(&model.name);
        match mesh_ids.get(&fid) {
            Some(&existing) if existing != geometry => {
                return Err(VrmError::new(
                    VrmErrorKind::InvalidPrefab,
                    format!(
                        "参照FBXのメッシュIDが重複するため先勝ちにしない: {}（{}）",
                        crate::escape_diag(&model.name),
                        crate::escape_diag(prefab_label),
                    ),
                ));
            }
            Some(_) => {}
            None => {
                mesh_ids.insert(fid, geometry);
            }
        }
    }
    let mut renderer_ids: HashMap<FileId, ObjectId> = HashMap::new();
    for (index, model) in fbx.models().iter().enumerate() {
        let Some(geometry) = model.mesh else {
            continue;
        };
        let Some(node) = nodes.get(index) else {
            continue;
        };
        let fid = skinned_mesh_renderer_file_id(&node.path);
        match renderer_ids.get(&fid) {
            Some(&existing) if existing != geometry => {
                return Err(VrmError::new(
                    VrmErrorKind::InvalidPrefab,
                    format!(
                        "参照FBXの描画設定IDが重複するため先勝ちにしない: {}（{}）",
                        crate::escape_diag(&model.name),
                        crate::escape_diag(prefab_label),
                    ),
                ));
            }
            Some(_) => {}
            None => {
                renderer_ids.insert(fid, geometry);
            }
        }
    }
    // Prefab内の文書ID集合。dangling（存在しない参照先）の検出に使う。
    let doc_ids: HashSet<FileId> = prefab.documents().iter().map(|doc| doc.file_id()).collect();
    // 入力FBXへのPrefabInstanceを特定する。複数は先勝ちにしない。
    // 0件なら直接記述方式とみなし、stripped文書は対応付けに使わない。
    let instance = find_fbx_instance(prefab, prefab_label, fbx_guid)?;
    // rendererのfileIDからメッシュへの対応表。GUIDとinstanceで出所を検証する。
    let renderer_meshes = build_renderer_meshes(
        prefab,
        prefab_label,
        &mesh_ids,
        &renderer_ids,
        fbx_guid,
        instance,
    )?;
    // FBXのメッシュごとのブレンドシェイプ名列。候補の実在判定に使う。
    // FBXチャンネル順をUnity側のブレンドシェイプ索引と同一視する前提である。
    let mut channels: HashMap<ObjectId, Vec<String>> = HashMap::new();
    for mesh in fbx.meshes() {
        channels.insert(
            mesh.id,
            fbx.blend_shape_channels(mesh.id)
                .iter()
                .map(|channel| channel.name.clone())
                .collect(),
        );
    }
    // 使用する側の参照だけを解決する。使わない側の参照は無視する。
    let use_viseme = matches!(settings.lip_sync, 2 | 3);
    let use_eyelids = settings.enable_eye_look && settings.eyelid_type == 2;
    let viseme_mesh = if use_viseme {
        resolve_side_ref(
            settings.viseme_renderer.as_ref(),
            "VisemeSkinnedMesh",
            &doc_ids,
            &renderer_meshes,
            prefab_label,
        )?
    } else {
        None
    };
    let eyelids_mesh = if use_eyelids {
        resolve_side_ref(
            settings.eyelids_renderer.as_ref(),
            "eyelidsSkinnedMesh",
            &doc_ids,
            &renderer_meshes,
            prefab_label,
        )?
    } else {
        None
    };
    let resolved = ResolvedDescriptor {
        lip_sync: settings.lip_sync,
        viseme_names: settings.viseme_names,
        mouth_open: settings.mouth_open,
        enable_eye_look: settings.enable_eye_look,
        eyelid_type: settings.eyelid_type,
        eyelids_indices: settings.eyelids_indices,
        viseme_mesh,
        eyelids_mesh,
    };
    build_plan(&resolved, &channels, prefab_label, humanoid_eyes)
}

/// descriptorの設定値。参照はGUID検査のため [`ObjectRef`] のまま保持する。
///
/// 欠落と `fileID: 0` は `None`、非nullは `Some` である。
/// 参照の形状不良は読み取り時に破損として失敗させる。
#[derive(Debug, Clone)]
struct DescriptorSettings {
    /// `lipSync` の生値。0=Default・1=JawFlapBone・2=JawFlapBlendShape・
    /// 3=VisemeBlendShape・4=VisemeParameterOnly。読めない場合は -1 である。
    lip_sync: i64,
    /// viseme参照先のrenderer。null・欠落の場合は `None` である。
    viseme_renderer: Option<ObjectRef>,
    /// `VisemeBlendShapes` の文字列列。SDK順の15件を想定する。
    viseme_names: Vec<String>,
    /// `MouthOpenBlendShapeName`。空・欠落の場合は `None` である。
    mouth_open: Option<String>,
    /// `enableEyeLook` の生値。欠落時は偽である。
    enable_eye_look: bool,
    /// `eyelidType` の生値。0=None・1=Bones・2=Blendshapes。欠落時は0である。
    eyelid_type: i64,
    /// まぶた参照先のrenderer。null・欠落の場合は `None` である。
    eyelids_renderer: Option<ObjectRef>,
    /// `eyelidsBlendshapes` の3要素。読めない・無い場合は `None` である。
    eyelids_indices: Option<Vec<i32>>,
}

/// 参照解決済みのdescriptor。`build_plan` はこの実在判定だけを行う。
///
/// `viseme_mesh`・`eyelids_mesh` は出所検証済みの適用先候補であり、
/// 候補の実在判定で落とすことがある。`None` は参照なし（警告対象）を表す。
#[derive(Debug, Clone)]
struct ResolvedDescriptor {
    lip_sync: i64,
    viseme_names: Vec<String>,
    mouth_open: Option<String>,
    enable_eye_look: bool,
    eyelid_type: i64,
    eyelids_indices: Option<Vec<i32>>,
    viseme_mesh: Option<ObjectId>,
    eyelids_mesh: Option<ObjectId>,
}

/// Prefabからdescriptor設定を読む。無い場合は `None` を返す。
///
/// `lipSync` 欄に加え、`VisemeBlendShapes`・`VisemeSkinnedMesh`・
/// `customEyeLookSettings` のいずれかを持つ `MonoBehaviour` 文書を探す。
/// 別スクリプトの同名欄をdescriptorと誤認しないためである。
/// 複数ある場合は部分成功にせず失敗する。
/// 文書の型付き読み取り・参照の形状不良は破損として失敗する。
///
/// # Errors
///
/// descriptorが複数ある場合・文書や参照の読み取りに失敗した場合に返す。
fn parse_descriptor(
    prefab: &SerializedFile,
    prefab_label: &str,
) -> Result<Option<DescriptorSettings>, VrmError> {
    let mut found: Option<(crate::unity::MonoBehaviour, String)> = None;
    for doc in prefab.documents() {
        let typed = doc.decode().map_err(|error| {
            VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!(
                    "Prefabの文書を読めない（{}）: {error}",
                    crate::escape_diag(prefab_label),
                ),
            )
        })?;
        let TypedObject::MonoBehaviour(mono) = typed else {
            continue;
        };
        // descriptorの識別は `lipSync` 単独でなく、口・目の欄のいずれかを伴う。
        let is_descriptor = mono.extra.contains_key("lipSync")
            && (mono.extra.contains_key("VisemeBlendShapes")
                || mono.extra.contains_key("VisemeSkinnedMesh")
                || mono.extra.contains_key("customEyeLookSettings"));
        if !is_descriptor {
            continue;
        }
        if found.is_some() {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!(
                    "VRCAvatarDescriptorが複数あるため先勝ちにしない（{}）",
                    crate::escape_diag(prefab_label),
                ),
            ));
        }
        found = Some((mono, doc.body_text().to_owned()));
    }
    let Some((mono, body)) = found else {
        return Ok(None);
    };
    let extra = &mono.extra;
    let lip_sync = extra.get("lipSync").and_then(read_i64).unwrap_or(-1);
    let viseme_renderer = match extra.get("VisemeSkinnedMesh") {
        None => None,
        Some(value) => read_ref(value, "VisemeSkinnedMesh", prefab_label)?,
    };
    let viseme_names = match extra.get("VisemeBlendShapes") {
        Some(Value::Sequence(items)) => items
            .iter()
            .map(|item| read_string_like(item).unwrap_or_default())
            .collect(),
        _ => Vec::new(),
    };
    let mouth_open = extra
        .get("MouthOpenBlendShapeName")
        .and_then(read_string_like)
        .filter(|name| !name.is_empty());
    let enable_eye_look = extra
        .get("enableEyeLook")
        .and_then(read_flag)
        .unwrap_or(false);
    let eye = extra
        .get("customEyeLookSettings")
        .and_then(|value| value.as_mapping());
    let eyelid_type = eye
        .and_then(|eye| eye.get("eyelidType"))
        .and_then(read_i64)
        .unwrap_or(0);
    let eyelids_renderer = match eye.and_then(|eye| eye.get("eyelidsSkinnedMesh")) {
        None => None,
        Some(value) => read_ref(value, "eyelidsSkinnedMesh", prefab_label)?,
    };
    let eyelids_indices = eye
        .map(|eye| eyelids_indices_from(&body, eye))
        .unwrap_or(None);
    Ok(Some(DescriptorSettings {
        lip_sync,
        viseme_renderer,
        viseme_names,
        mouth_open,
        enable_eye_look,
        eyelid_type,
        eyelids_renderer,
        eyelids_indices,
    }))
}

/// 数値を `i64` として読む。浮動小数点経由では読まない。
fn read_i64(value: &Value) -> Option<i64> {
    match value {
        Value::Number(number) => number.as_i64(),
        _ => None,
    }
}

/// 0/1フラグまたは真偽値を真偽値として読む。
fn read_flag(value: &Value) -> Option<bool> {
    match value {
        Value::Bool(flag) => Some(*flag),
        Value::Number(number) => match number.as_i64() {
            Some(0) => Some(false),
            Some(1) => Some(true),
            _ => None,
        },
        _ => None,
    }
}

/// 名前概念の値を文字列として読む。
///
/// 文字列はそのまま、数値は文字形に、bignumは桁を保って読む。
/// GUIDとは異なり名前は数値解決の値を文字列化しても桁落ちがない。
fn read_string_like(value: &Value) -> Option<String> {
    if let Some(digits) = crate::unity::bignum_digits(value) {
        return Some(digits.to_owned());
    }
    match value {
        Value::Null => None,
        Value::String(name) => Some(name.clone()),
        Value::Number(number) => {
            if let Some(int) = number.as_i64() {
                Some(int.to_string())
            } else if let Some(int) = number.as_u64() {
                Some(int.to_string())
            } else {
                number.as_f64().map(|float| format!("{float}"))
            }
        }
        _ => None,
    }
}

/// 参照欄を読む。欠落と `fileID: 0` は `None`、非nullは `Some` である。
///
/// GUIDは保持し、外部参照の判定は解決側で行う。
/// 参照の形状不良（マッピングでない・fileID欠落・整数でない・GUID不良）は
/// 欠落に読み替えず、破損として失敗する。
///
/// # Errors
///
/// 参照の形状が壊れている場合に返す。
fn read_ref(value: &Value, role: &str, prefab_label: &str) -> Result<Option<ObjectRef>, VrmError> {
    let reference = ObjectRef::decode_body(value).map_err(|error| {
        VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!(
                "{role}の参照が壊れている（{}）: {error}",
                crate::escape_diag(prefab_label),
            ),
        )
    })?;
    if reference.is_null() {
        Ok(None)
    } else {
        Ok(Some(reference))
    }
}

/// 入力FBXへのPrefabInstance文書を特定する。無い場合は `None` を返す。
///
/// `m_SourcePrefab.guid` が `fbx_guid` と一致するものを探す。
/// 複数ある場合は先勝ちにせず失敗する。
///
/// # Errors
///
/// 一致する文書が複数ある場合・文書の読み取りに失敗した場合に返す。
fn find_fbx_instance(
    prefab: &SerializedFile,
    prefab_label: &str,
    fbx_guid: &str,
) -> Result<Option<FileId>, VrmError> {
    let mut found: Option<FileId> = None;
    for doc in prefab.documents() {
        if doc.stripped() {
            continue;
        }
        let typed = doc.decode().map_err(|error| {
            VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!(
                    "Prefabの文書を読めない（{}）: {error}",
                    crate::escape_diag(prefab_label),
                ),
            )
        })?;
        let TypedObject::PrefabInstance(instance) = typed else {
            continue;
        };
        if instance.source.is_null() {
            continue;
        }
        let matches = instance
            .source
            .guid
            .as_ref()
            .is_some_and(|guid| guid.as_str().eq_ignore_ascii_case(fbx_guid));
        if !matches {
            continue;
        }
        if found.is_some() {
            return Err(VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!(
                    "入力FBXへのPrefabInstanceが複数あるため先勝ちにしない（{}）",
                    crate::escape_diag(prefab_label),
                ),
            ));
        }
        found = Some(doc.file_id());
    }
    Ok(found)
}

/// rendererのfileIDからメッシュへの対応表を作る。出所を検証する。
///
/// 取り込みIDは対応元の内部照合にだけ使い、最終表には検証済みの文書IDだけを張る。
/// stripped文書はclassID 137（SkinnedMeshRenderer）であり、対応元のGUIDが
/// `fbx_guid` と一致し、所属のPrefabInstanceが外部GUIDを持たないローカル参照で
/// 特定した文書と一致する場合だけ使う。直接記述の描画設定はメッシュ参照の
/// GUIDが `fbx_guid` と一致する場合だけ使う。一致しない文書は入れない。
/// 同じfileIDが異なるメッシュに解決したら曖昧として失敗する。
///
/// # Errors
///
/// 文書の読み取りに失敗した場合・参照が曖昧な場合に返す。
fn build_renderer_meshes(
    prefab: &SerializedFile,
    prefab_label: &str,
    mesh_ids: &HashMap<FileId, ObjectId>,
    renderer_ids: &HashMap<FileId, ObjectId>,
    fbx_guid: &str,
    instance: Option<FileId>,
) -> Result<HashMap<FileId, ObjectId>, VrmError> {
    // 検証済みのPrefab文書IDだけを最終表に入れる。取り込みIDそのものは
    // 文書IDではないため、descriptor参照が取り込みIDを直接指した場合は
    // 後段の doc_ids 検査でdanglingとして失敗する。
    let mut renderer_meshes: HashMap<FileId, ObjectId> = HashMap::new();
    let mut insert_renderer = |file_id: FileId, geometry: ObjectId| -> Result<(), VrmError> {
        match renderer_meshes.get(&file_id) {
            Some(&existing) if existing != geometry => Err(VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!(
                    "renderer参照が曖昧なため先勝ちにしない: fileID {file_id}（{}）",
                    crate::escape_diag(prefab_label),
                ),
            )),
            Some(_) => Ok(()),
            None => {
                renderer_meshes.insert(file_id, geometry);
                Ok(())
            }
        }
    };
    for doc in prefab.documents() {
        if doc.stripped() {
            // 直接記述方式ではstripped文書は対応付けに使わない。
            // 参照側が指した場合は解決不能として失敗させる。
            let Some(instance_id) = instance else {
                continue;
            };
            // stripped文書はSkinnedMeshRenderer（classID 137）だけを対象にする。
            // TypedObject::Stripped はクラスを表さないため、Transform等を
            // 描画設定として受理しないよう文書側で確かめる。
            if doc.class_id() != 137 {
                continue;
            }
            let typed = doc.decode().map_err(|error| {
                VrmError::new(
                    VrmErrorKind::InvalidPrefab,
                    format!(
                        "Prefabの文書を読めない（{}）: {error}",
                        crate::escape_diag(prefab_label),
                    ),
                )
            })?;
            let TypedObject::Stripped(link) = typed else {
                continue;
            };
            if link.corresponding_source.is_null() {
                continue;
            }
            let guid_matches = link
                .corresponding_source
                .guid
                .as_ref()
                .is_some_and(|guid| guid.as_str().eq_ignore_ascii_case(fbx_guid));
            if !guid_matches {
                continue;
            }
            // 所属のPrefabInstanceが特定した文書と一致する場合だけ使う。
            // 外部GUID付きの参照は別アセットを指し得るため受理しない。
            let owner = link
                .extra
                .get("m_PrefabInstance")
                .and_then(|value| ObjectRef::decode_body(value).ok());
            let Some(owner) = owner else {
                continue;
            };
            if owner.guid.is_some() || owner.file_id != instance_id {
                continue;
            }
            if let Some(&geometry) = renderer_ids.get(&link.corresponding_source.file_id) {
                insert_renderer(doc.file_id(), geometry)?;
            }
            continue;
        }
        let typed = doc.decode().map_err(|error| {
            VrmError::new(
                VrmErrorKind::InvalidPrefab,
                format!(
                    "Prefabの文書を読めない（{}）: {error}",
                    crate::escape_diag(prefab_label),
                ),
            )
        })?;
        if let TypedObject::SkinnedMeshRenderer(renderer) = typed {
            if renderer.mesh.is_null() {
                continue;
            }
            let guid_matches = renderer
                .mesh
                .guid
                .as_ref()
                .is_some_and(|guid| guid.as_str().eq_ignore_ascii_case(fbx_guid));
            if !guid_matches {
                continue;
            }
            if let Some(&geometry) = mesh_ids.get(&renderer.mesh.file_id) {
                insert_renderer(doc.file_id(), geometry)?;
            }
        }
    }
    Ok(renderer_meshes)
}

/// descriptorの参照1件を使用側のメッシュに解決する。`None` は参照なしである。
///
/// 参照にGUIDがある（外部アセット参照）・参照先がPrefabに無い（dangling）・
/// 文書はあるが入力FBXのrendererに解決できない場合は失敗する。
/// null・欠落は `None` を返し、呼び出し側が警告する。
///
/// # Errors
///
/// 外部参照・dangling・解決不能の場合に返す。
fn resolve_side_ref(
    reference: Option<&ObjectRef>,
    role: &str,
    doc_ids: &HashSet<FileId>,
    renderer_meshes: &HashMap<FileId, ObjectId>,
    prefab_label: &str,
) -> Result<Option<ObjectId>, VrmError> {
    let Some(reference) = reference else {
        return Ok(None);
    };
    if reference.guid.is_some() {
        return Err(VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!(
                "{role}が外部アセットを指すため解決しない（fileID {}）（{}）",
                reference.file_id,
                crate::escape_diag(prefab_label),
            ),
        ));
    }
    if !doc_ids.contains(&reference.file_id) {
        return Err(VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!(
                "{role}の参照先がPrefabに無い（fileID {}）（{}）",
                reference.file_id,
                crate::escape_diag(prefab_label),
            ),
        ));
    }
    match renderer_meshes.get(&reference.file_id) {
        Some(&geometry) => Ok(Some(geometry)),
        None => Err(VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!(
                "{role}の参照先が入力FBXの描画設定に解決できない（fileID {}）（{}）",
                reference.file_id,
                crate::escape_diag(prefab_label),
            ),
        )),
    }
}

/// `eyelidsBlendshapes` の索引列を読む。
///
/// `customEyeLookSettings` ブロックの直接の子である `eyelidsBlendshapes` の
/// スカラーを生本文から取り、16進として復号する。YAMLの数値解決では先頭ゼロが
/// 落ち桁が変わるため、生本文を優先する。ブロックが特定できない・空の場合は
/// `Value::Sequence`・`String`・bignumの順に読む。
/// 長さ不正・16進不正は `None` を返し、呼び出し側で警告する。
fn eyelids_indices_from(body: &str, eye: &yaml_serde::Mapping) -> Option<Vec<i32>> {
    // 解析済みの `Value` に欄が無い場合は、生本文の同名キーを信用しない。
    // 複数行の引用スカラー等に偽のキーがあっても、それを設定として採用しない。
    let value = eye.get("eyelidsBlendshapes");
    if value.is_some()
        && let Some(scalar) = scoped_eyelids_scalar(body)
        && let Some(indices) = decode_eyelids_hex(scalar)
    {
        return Some(indices);
    }
    let value = value?;
    match value {
        Value::Sequence(items) => decode_eyelids_seq(items),
        Value::String(text) => decode_eyelids_hex(text.trim()),
        Value::Null => None,
        tagged => {
            crate::unity::bignum_digits(tagged).and_then(|digits| decode_eyelids_hex(digits.trim()))
        }
    }
}

/// ブロックYAMLの1行。キーと、`:` 以降の生の値を持つ。
struct YamlLine<'a> {
    /// 行頭の空白幅（バイト数）。
    indent: usize,
    /// 引用符を外したキー。
    key: String,
    /// `:` 以降の値。block scalar・子を持つ場合は空である。
    value: &'a str,
}

/// `customEyeLookSettings` の直接の子である `eyelidsBlendshapes` のスカラーを返す。
///
/// Unity が使うブロックYAMLの範囲で構文を追う。コメント・空行・block scalar の
/// 内容・複数行の引用スカラー・sequence item の入れ子・ネストした mapping・
/// 別 mapping の同名キーを拾わない。引用符付きキーもキーとして扱う。
/// 追跡しきれない構文（複数行の引用スカラー、未対応のエスケープ、複数行の
/// フローcollection、複数行のplain scalar等）を見つけた場合は、原文抽出全体を
/// `None` にして呼び出し側を `Value` の読み取りへ委ねる。
fn scoped_eyelids_scalar(body: &str) -> Option<&str> {
    let lines = block_lines(body)?;
    // 文書本文の先頭クラスは `MonoBehaviour:` である。複数あれば曖昧とする。
    let mut root: Option<usize> = None;
    for (index, line) in lines.iter().enumerate() {
        if line.indent == 0 && line.key == "MonoBehaviour" {
            if root.is_some() {
                return None;
            }
            root = Some(index);
        }
    }
    let root = root?;
    let root_end = block_end(&lines, root)?;
    let (eye_index, eye) = direct_child(
        &lines,
        root + 1,
        root_end,
        lines[root].indent,
        "customEyeLookSettings",
    )?;
    let eye_end = block_end(&lines, eye_index)?;
    let (_, target) = direct_child(
        &lines,
        eye_index + 1,
        eye_end,
        eye.indent,
        "eyelidsBlendshapes",
    )?;
    let value = target.value.trim();
    if value.is_empty() {
        return None;
    }
    Some(value)
}

/// ブロックYAMLの行を、block scalar の内容・sequence itemの入れ子・コメントを
/// 除いて集める。
///
/// 引用符付きキーを外し、値が block scalar ヘッダ（`|`・`>`）の行は
/// 子を持たない行として記録する。追跡しきれない構文を見つけた場合は `None` を
/// 返し、呼び出し側に原文抽出を諦めさせる。
fn block_lines(body: &str) -> Option<Vec<YamlLine<'_>>> {
    let mut out = Vec::new();
    // 読み飛ばすブロックの親インデント。block scalar と sequence item の
    // 入れ子に使う。これより深い行は構文として解釈しない。
    let mut skip_block: Option<usize> = None;
    for line in body.lines() {
        let trimmed = line.trim_end_matches('\r').trim_start_matches([' ', '\t']);
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indent = leading_indent(line);
        if let Some(block_indent) = skip_block {
            if indent > block_indent {
                continue;
            }
            skip_block = None;
        }
        // sequence item は mapping のキーではない。入れ子の mapping を
        // 直接の子と誤認しないよう、item より深い行をまとめて読み飛ばす。
        if trimmed == "-" || trimmed.starts_with("- ") {
            skip_block = Some(indent);
            continue;
        }
        // 複合キー・anchor・alias・tag は追跡しない。
        if trimmed.starts_with("? ")
            || trimmed.starts_with('&')
            || trimmed.starts_with('*')
            || trimmed.starts_with('!')
        {
            return None;
        }
        let (key, raw_value) = parse_key_value(trimmed)?;
        let value = strip_inline_comment(raw_value);
        if value.starts_with('|') || value.starts_with('>') {
            skip_block = Some(indent);
            out.push(YamlLine {
                indent,
                key,
                value: "",
            });
            continue;
        }
        if value.starts_with('&') || value.starts_with('*') || value.starts_with('!') {
            return None;
        }
        if !flow_balanced(value) {
            return None;
        }
        if starts_with_quote(value) && !quoted_scalar_closed(value) {
            return None;
        }
        out.push(YamlLine { indent, key, value });
    }
    // 値を持つ行の次が深い行なら、複数行のplain scalar等であり、
    // 続きをmappingとして解釈できない。原文抽出を諦める。
    for (index, line) in out.iter().enumerate() {
        if line.value.is_empty() || line.value.starts_with('{') || line.value.starts_with('[') {
            continue;
        }
        if let Some(next) = out.get(index + 1)
            && next.indent > line.indent
        {
            return None;
        }
    }
    Some(out)
}

/// `key: value` を読む。引用符付きキー（`"..."`・`'...'`）を外す。
///
/// 引用符付きキーの復号に失敗した場合（閉じない・未対応のエスケープ）は
/// `None` を返し、呼び出し側は原文抽出を諦める。
fn parse_key_value(line: &str) -> Option<(String, &str)> {
    let first = line.chars().next()?;
    if first == '"' || first == '\'' {
        let (key, rest) = parse_quoted_key(line, first)?;
        let rest = rest.trim_start_matches([' ', '\t']);
        let rest = rest.strip_prefix(':')?;
        return Some((key, rest.trim_start_matches([' ', '\t'])));
    }
    // プレーンキーは、空白か行末が続く最初の `:` で区切る。
    let bytes = line.as_bytes();
    for (index, byte) in bytes.iter().enumerate() {
        if *byte != b':' {
            continue;
        }
        if index + 1 == bytes.len() || bytes[index + 1] == b' ' || bytes[index + 1] == b'\t' {
            let key = line[..index].trim();
            if key.is_empty() {
                return None;
            }
            return Some((
                key.to_owned(),
                line[index + 1..].trim_start_matches([' ', '\t']),
            ));
        }
    }
    None
}

/// 引用符付きキーを読み、閉じ引用符より後ろを返す。
///
/// ダブルクォートの一般的なエスケープと `\uXXXX`・`\xXX` を復号する。
/// 閉じない・未対応のエスケープは `None` を返す（安全側で原文抽出を諦める）。
fn parse_quoted_key(line: &str, quote: char) -> Option<(String, &str)> {
    let chars: Vec<char> = line.chars().collect();
    let mut key = String::new();
    let mut index = 1;
    while index < chars.len() {
        let ch = chars[index];
        if quote == '"' && ch == '\\' {
            let next = *chars.get(index + 1)?;
            match next {
                '"' => key.push('"'),
                '\\' => key.push('\\'),
                '/' => key.push('/'),
                'n' => key.push('\n'),
                'r' => key.push('\r'),
                't' => key.push('\t'),
                '0' => key.push('\0'),
                'u' => {
                    let digits: String = chars.get(index + 2..index + 6)?.iter().collect();
                    let value = u32::from_str_radix(&digits, 16).ok()?;
                    key.push(char::from_u32(value)?);
                    index += 4;
                }
                'x' => {
                    let digits: String = chars.get(index + 2..index + 4)?.iter().collect();
                    let value = u32::from_str_radix(&digits, 16).ok()?;
                    key.push(char::from_u32(value)?);
                    index += 2;
                }
                _ => return None,
            }
            index += 2;
            continue;
        }
        if ch == quote {
            if quote == '\'' && chars.get(index + 1) == Some(&'\'') {
                key.push('\'');
                index += 2;
                continue;
            }
            let byte_index = line.char_indices().nth(index)?.0 + ch.len_utf8();
            return Some((key, &line[byte_index..]));
        }
        key.push(ch);
        index += 1;
    }
    None
}

/// 行末コメントを除く。引用符で始まる値はそのまま返す。
fn strip_inline_comment(value: &str) -> &str {
    let value = value.trim();
    if value.starts_with('#') {
        return "";
    }
    if value.starts_with('"') || value.starts_with('\'') {
        return value;
    }
    match value.find(" #") {
        Some(index) => value[..index].trim_end(),
        None => value,
    }
}

/// 値が引用符で始まるか。
fn starts_with_quote(value: &str) -> bool {
    value.starts_with('"') || value.starts_with('\'')
}

/// 引用符で始まる値が同じ行で閉じているか。閉じない場合は複数行スカラーである。
fn quoted_scalar_closed(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(quote) = chars.next() else {
        return false;
    };
    let mut escaped = false;
    for ch in chars {
        if escaped {
            escaped = false;
            continue;
        }
        if quote == '"' && ch == '\\' {
            escaped = true;
            continue;
        }
        if ch == quote {
            return true;
        }
    }
    false
}

/// フローcollection（`{}`・`[]`）が同じ行で閉じているか。
///
/// 複数行のフローcollectionは追跡しない。引用符内の括弧は数えない。
fn flow_balanced(value: &str) -> bool {
    let value = value.trim_start();
    // フローcollectionでないplain/quoted scalarの `{}`・`[]`・`'` は
    // 文字であり、対応を数えない。
    if !(value.starts_with('{') || value.starts_with('[')) {
        return true;
    }
    let mut braces = 0i32;
    let mut brackets = 0i32;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for ch in value.chars() {
        if let Some(q) = quote {
            if escaped {
                escaped = false;
            } else if q == '"' && ch == '\\' {
                escaped = true;
            } else if ch == q {
                quote = None;
            }
            continue;
        }
        match ch {
            '"' | '\'' => quote = Some(ch),
            '{' => braces += 1,
            '}' => braces -= 1,
            '[' => brackets += 1,
            ']' => brackets -= 1,
            _ => {}
        }
        if braces < 0 || brackets < 0 {
            return false;
        }
    }
    quote.is_none() && braces == 0 && brackets == 0
}

/// 親行の直後の子の範囲を返す。インデントが親以下になった行で終わる。
fn block_end(lines: &[YamlLine<'_>], parent: usize) -> Option<usize> {
    let parent_indent = lines.get(parent)?.indent;
    for (index, line) in lines.iter().enumerate().skip(parent + 1) {
        if line.indent <= parent_indent {
            return Some(index);
        }
    }
    Some(lines.len())
}

/// 範囲内の直接の子から、指定キーの行を1件だけ返す。
///
/// 直接の子は、範囲内で親より深い最小インデントの行である。ネストした
/// mapping の同名キーは深いため選ばない。複数あれば曖昧として `None`。
fn direct_child<'a, 'b>(
    lines: &'b [YamlLine<'a>],
    start: usize,
    end: usize,
    parent_indent: usize,
    key: &str,
) -> Option<(usize, &'b YamlLine<'a>)> {
    let end = end.min(lines.len());
    if start >= end {
        return None;
    }
    let child_indent = lines[start..end]
        .iter()
        .filter(|line| line.indent > parent_indent)
        .map(|line| line.indent)
        .min()?;
    let mut found: Option<(usize, &YamlLine<'_>)> = None;
    for (index, line) in lines.iter().enumerate().take(end).skip(start) {
        if line.indent != child_indent || line.key != key {
            continue;
        }
        if found.is_some() {
            return None;
        }
        found = Some((index, line));
    }
    found
}

/// 行頭の空白（半角・タブ）の幅をバイト数で返す。
fn leading_indent(line: &str) -> usize {
    line.len() - line.trim_start_matches([' ', '\t']).len()
}

/// 16進文字列をリトルエンディアンi32列に復号する。
///
/// 8文字で1件のi32であり、バイト列をリトルエンディアンで読む。
/// 例えば `10000000` はバイト列 `[0x10, 0x00, 0x00, 0x00]` として16になる。
/// `-1`（`ffffffff`）を保持する。先頭ゼロを含む字面もそのまま読める。
/// 長さが8の倍数でない・16進以外の字面は `None` を返す。
fn decode_eyelids_hex(text: &str) -> Option<Vec<i32>> {
    if text.is_empty() || !text.len().is_multiple_of(8) {
        return None;
    }
    if !text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let mut out = Vec::with_capacity(text.len() / 8);
    for chunk in text.as_bytes().chunks_exact(8) {
        let mut bytes = [0u8; 4];
        for (index, pair) in chunk.chunks_exact(2).enumerate() {
            let text = str::from_utf8(pair).ok()?;
            bytes[index] = u8::from_str_radix(text, 16).ok()?;
        }
        out.push(i32::from_le_bytes(bytes));
    }
    Some(out)
}

/// `Value::Sequence` をi32列に読む。各要素がi32に収まらない場合は `None` を返す。
fn decode_eyelids_seq(items: &[Value]) -> Option<Vec<i32>> {
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        match item {
            Value::Number(number) => {
                let value = number.as_i64()?;
                out.push(i32::try_from(value).ok()?);
            }
            _ => return None,
        }
    }
    Some(out)
}

/// 設定と対応表から表情の計画を作る。
///
/// 口側・目側とも、メッシュのチャンネルに実在する候補だけを束ねる。
/// 実在候補が1件も無い側のメッシュは立てず、別メッシュ失敗は両側に
/// 実在候補がある場合だけ出す。片方だけならそのメッシュを使う。
/// 重みはすべて100（VRM束ねは1.0）、overrideはnone、`isBinary` は偽である。
/// 式の順序は aa・ih・ou・ee・oh・blink・lookUp・lookDown である。
///
/// # Errors
///
/// visemeとeyelidで別メッシュに実在候補がある場合に返す。
fn build_plan(
    resolved: &ResolvedDescriptor,
    channels: &HashMap<ObjectId, Vec<String>>,
    prefab_label: &str,
    humanoid_eyes: bool,
) -> Result<DescriptorExpressionPlan, VrmError> {
    let mut warnings = Vec::new();
    let mut bindings: Vec<VrmExpressionBinding> = Vec::new();
    let mut viseme_mesh = resolved.viseme_mesh;
    let mut eyelid_mesh = resolved.eyelids_mesh;

    // 口側の候補。メッシュに実在する名前だけ束ねる。
    // 候補を作らない分岐ではメッシュも立てない。
    match resolved.lip_sync {
        3 => {
            if let Some(geometry) = viseme_mesh {
                let names = channels.get(&geometry).cloned().unwrap_or_default();
                // SDK順の索引からVRM式への対応。aa=10・E=11・ih=12・oh=13・ou=14。
                const PAIRS: &[(&str, usize)] =
                    &[("aa", 10), ("ih", 12), ("ou", 14), ("ee", 11), ("oh", 13)];
                let mut real = 0;
                for (expression, index) in PAIRS {
                    let target = resolved
                        .viseme_names
                        .get(*index)
                        .cloned()
                        .unwrap_or_default();
                    if target.is_empty() {
                        warnings.push(format!(
                            "viseme名が無いため{expression}を作らない（SDK索引{index}）"
                        ));
                        continue;
                    }
                    if !names.contains(&target) {
                        warnings.push(format!(
                            "viseme名がメッシュに無いため{expression}を作らない（{target}）"
                        ));
                        continue;
                    }
                    bindings.push(VrmExpressionBinding {
                        expression: (*expression).to_owned(),
                        target,
                    });
                    real += 1;
                }
                if real == 0 {
                    viseme_mesh = None;
                }
            } else {
                warnings.push("VisemeSkinnedMeshが無いためvisemeから表情を作らない".to_owned());
            }
        }
        2 => {
            if let Some(geometry) = viseme_mesh {
                let names = channels.get(&geometry).cloned().unwrap_or_default();
                match resolved.mouth_open.clone() {
                    Some(target) if names.contains(&target) => {
                        bindings.push(VrmExpressionBinding {
                            expression: "aa".to_owned(),
                            target,
                        });
                    }
                    Some(target) => {
                        warnings.push(format!(
                            "MouthOpen名がメッシュに無いためaaを作らない（{target}）"
                        ));
                        viseme_mesh = None;
                    }
                    None => {
                        warnings.push(
                            "MouthOpenBlendShapeNameが無いためJawFlapBlendShapeから表情を作らない"
                                .to_owned(),
                        );
                        viseme_mesh = None;
                    }
                }
            } else {
                warnings.push("VisemeSkinnedMeshが無いためMouthOpenから表情を作らない".to_owned());
            }
        }
        1 => {
            warnings
                .push("lipSyncがJawFlapBoneのためブレンドシェイプから表情を作らない".to_owned());
            viseme_mesh = None;
        }
        4 => {
            warnings.push(
                "lipSyncがVisemeParameterOnlyのためブレンドシェイプから表情を作らない".to_owned(),
            );
            viseme_mesh = None;
        }
        0 => {
            warnings.push("lipSyncがDefaultのため表情を作らない".to_owned());
            viseme_mesh = None;
        }
        other => {
            warnings.push(format!("lipSyncが想定外のため表情を作らない（{other}）"));
            viseme_mesh = None;
        }
    }

    // 目側の候補。blinkはeyelid設定だけ、lookUp/lookDownはhumanoidの左右Eyeも要る。
    // 実在ターゲットが1件も無い場合はメッシュを立てない。
    if !resolved.enable_eye_look {
        warnings.push("eyeLookが無効のためまぶたの表情を作らない".to_owned());
        eyelid_mesh = None;
    } else if resolved.eyelid_type != 2 {
        warnings.push(format!(
            "eyelidTypeがBlendshapesでないためまぶたの表情を作らない（{}）",
            resolved.eyelid_type,
        ));
        eyelid_mesh = None;
    } else if let Some(geometry) = eyelid_mesh {
        let names = channels.get(&geometry).cloned().unwrap_or_default();
        match &resolved.eyelids_indices {
            Some(indices) if indices.len() == 3 => {
                let mut real = 0;
                if let Some(target) = eyelid_target(&names, indices[0], "blink", &mut warnings) {
                    bindings.push(VrmExpressionBinding {
                        expression: "blink".to_owned(),
                        target,
                    });
                    real += 1;
                }
                if humanoid_eyes {
                    if let Some(target) = eyelid_target(&names, indices[1], "lookUp", &mut warnings)
                    {
                        bindings.push(VrmExpressionBinding {
                            expression: "lookUp".to_owned(),
                            target,
                        });
                        real += 1;
                    }
                    if let Some(target) =
                        eyelid_target(&names, indices[2], "lookDown", &mut warnings)
                    {
                        bindings.push(VrmExpressionBinding {
                            expression: "lookDown".to_owned(),
                            target,
                        });
                        real += 1;
                    }
                } else {
                    warnings.push(
                        "humanoidの左右Eyeが解決できないためlookUpとlookDownを作らない".to_owned(),
                    );
                }
                if real == 0 {
                    eyelid_mesh = None;
                }
            }
            _ => {
                warnings.push(
                    "eyelidsBlendshapesが3件の索引でないためまぶたの表情を作らない".to_owned(),
                );
                eyelid_mesh = None;
            }
        }
    } else {
        warnings.push("eyelidsSkinnedMeshが無いためまぶたの表情を作らない".to_owned());
        eyelid_mesh = None;
    }

    // 両側に実在候補があり別メッシュなら明示的に失敗する。部分成功にしない。
    if let (Some(viseme), Some(eyelid)) = (viseme_mesh, eyelid_mesh)
        && viseme != eyelid
    {
        return Err(VrmError::new(
            VrmErrorKind::InvalidPrefab,
            format!(
                "visemeとまぶたで表情対象のメッシュが異なるため先勝ちにしない（viseme [object: {}]・まぶた [object: {}]）（{}）",
                viseme.value(),
                eyelid.value(),
                crate::escape_diag(prefab_label),
            ),
        ));
    }
    let mesh = viseme_mesh.or(eyelid_mesh);
    Ok(DescriptorExpressionPlan {
        profile: VrmExpressionProfile { bindings },
        mesh,
        warnings,
    })
}

/// まぶた索引1件をチャンネル名に解決する。`-1`・範囲外・空名はその式だけ落として警告する。
fn eyelid_target(
    names: &[String],
    index: i32,
    expression: &str,
    warnings: &mut Vec<String>,
) -> Option<String> {
    if index < 0 {
        warnings.push(format!(
            "{expression}の索引が-1のため作らない（まぶた未設定）"
        ));
        return None;
    }
    match names.get(index as usize) {
        Some(target) if !target.is_empty() => Some(target.clone()),
        _ => {
            warnings.push(format!(
                "{expression}の索引が範囲外のため作らない（index {index}・チャンネル{}件）",
                names.len(),
            ));
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // テスト名は日本語で書く（テストは仕様・契約であるため）。

    #[test]
    fn 通常の16進を復号できる() {
        // Milfy実測の `100000001100000012000000` → 16,17,18。
        assert_eq!(
            decode_eyelids_hex("100000001100000012000000"),
            Some(vec![16, 17, 18]),
        );
    }

    #[test]
    fn 先頭ゼロを含む16進を桁落ちなく復号できる() {
        // YAMLの数値解決では先頭ゼロが落ちるため、生本文経由の復号で確かめる。
        assert_eq!(
            decode_eyelids_hex("000000000100000002000000"),
            Some(vec![0, 1, 2]),
        );
    }

    #[test]
    fn 英字を含む16進を復号できる() {
        assert_eq!(
            decode_eyelids_hex("0f0000001000000011000000"),
            Some(vec![15, 16, 17]),
        );
    }

    #[test]
    fn 負値を保持して復号できる() {
        // Ciel実測の `87000000ffffffffffffffff` → 135,-1,-1。
        assert_eq!(
            decode_eyelids_hex("87000000ffffffffffffffff"),
            Some(vec![135, -1, -1]),
        );
    }

    #[test]
    fn 長さ不正と16進不正は読めない() {
        assert_eq!(decode_eyelids_hex(""), None);
        assert_eq!(decode_eyelids_hex("123"), None);
        assert_eq!(decode_eyelids_hex("zzzzzzzz"), None);
        // 24桁の10進bignum相当でも8の倍数でなければ読めない。
        assert_eq!(decode_eyelids_hex("10000000110000001200000"), None);
    }

    #[test]
    fn bignumの桁から16進を復号できる() {
        // `yaml_serde` が64bit超の整数に付ける印を再現し、桁から読む。
        let tagged = Value::Tagged(Box::new(yaml_serde::value::TaggedValue {
            tag: yaml_serde::value::Tag::new("vrctom:bignum"),
            value: Value::String("100000001100000012000000".to_owned()),
        }));
        let digits = crate::unity::bignum_digits(&tagged).expect("bignumの桁がある");
        assert_eq!(
            decode_eyelids_hex(digits),
            Some(vec![16, 17, 18]),
            "bignumの10進桁は16進字面と一致する",
        );
    }

    #[test]
    fn 列の各要素をi32へ読む() {
        let items = vec![
            Value::Number(16.into()),
            Value::Number(17.into()),
            Value::Number((-1).into()),
        ];
        assert_eq!(decode_eyelids_seq(&items), Some(vec![16, 17, -1]));
    }

    #[test]
    fn 列の要素がi32に収まらなければ読めない() {
        let items = vec![Value::Number(16.into()), Value::String("x".to_owned())];
        assert_eq!(decode_eyelids_seq(&items), None);
    }

    #[test]
    fn 複数行の引用スカラーは原文抽出を諦める() {
        let body = "MonoBehaviour:\n\
            \x20customEyeLookSettings:\n\
            \x20\x20\x20eyelidType: 2\n\
            \x20\x20\x20note: \"some text\n\
            \x20\x20\x20eyelidsBlendshapes: 000000000000000000000000\n\
            \x20\x20\x20end\"\n\
            \x20\x20\x20eyelidsBlendshapes: 100000001100000012000000\n";
        assert_eq!(scoped_eyelids_scalar(body), None);
    }

    #[test]
    fn 引用符付きキーのユニコードエスケープを復号する() {
        let body = "MonoBehaviour:\n\
            \x20customEyeLookSettings:\n\
            \x20\x20\x20\"\\u0065yelidsBlendshapes\": 000000000100000002000000\n";
        assert_eq!(
            scoped_eyelids_scalar(body),
            Some("000000000100000002000000"),
        );
    }

    #[test]
    fn 複数行のplain_scalarは原文抽出を諦める() {
        let body = "MonoBehaviour:\n\
            \x20customEyeLookSettings:\n\
            \x20\x20\x20note: some text\n\
            \x20\x20\x20\x20\x20eyelidsBlendshapes: 000000000000000000000000\n\
            \x20\x20\x20eyelidsBlendshapes: 100000001100000012000000\n";
        assert_eq!(scoped_eyelids_scalar(body), None);
    }

    #[test]
    fn sequence_itemの入れ子を直接の子にしない() {
        let body = "MonoBehaviour:\n\
            \x20customEyeLookSettings:\n\
            \x20\x20\x20items:\n\
            \x20\x20\x20- fake:\n\
            \x20\x20\x20\x20\x20\x20eyelidsBlendshapes: 000000000000000000000000\n\
            \x20\x20\x20eyelidsBlendshapes: 100000001100000012000000\n";
        assert_eq!(
            scoped_eyelids_scalar(body),
            Some("100000001100000012000000"),
        );
    }

    #[test]
    fn 直接の子のスカラーだけを取る() {
        // コメント・別mapping・ネスト先の同名キーがあっても直接の子を取る。
        // インデントは半角空白で明示し、行継続の取り違えを避ける。
        let body = "MonoBehaviour:\n\
            \x20customEyeLookSettings:\n\
            \x20\x20\x20# eyelidsBlendshapes: 000000000000000000000000\n\
            \x20\x20\x20eyelidType: 2\n\
            \x20\x20\x20nested:\n\
            \x20\x20\x20\x20\x20eyelidsBlendshapes: ffffffffffffffff00000000\n\
            \x20\x20\x20eyelidsBlendshapes: 000000000100000002000000\n\
            \x20other:\n\
            \x20\x20\x20eyelidsBlendshapes: ffffffffffffffffffffffff\n";
        assert_eq!(
            scoped_eyelids_scalar(body),
            Some("000000000100000002000000"),
            "コメント・別mapping・ネスト先を拾わない",
        );
    }

    #[test]
    fn 別mappingの下の同名開始行は拾わない() {
        // `other` の下の `customEyeLookSettings` は直接の子ではない。
        let body = "MonoBehaviour:\n\
            \x20customEyeLookSettings:\n\
            \x20\x20\x20eyelidsBlendshapes: 000000000100000002000000\n\
            \x20other:\n\
            \x20\x20\x20customEyeLookSettings:\n\
            \x20\x20\x20\x20\x20eyelidsBlendshapes: ffffffffffffffff00000000\n";
        assert_eq!(
            scoped_eyelids_scalar(body),
            Some("000000000100000002000000"),
            "別mappingの下は直接の子ではない",
        );
    }

    #[test]
    fn 直接の子に同名キーが複数あれば曖昧として扱わない() {
        let body = "MonoBehaviour:\n\
            \x20customEyeLookSettings:\n\
            \x20\x20\x20eyelidsBlendshapes: 000000000100000002000000\n\
            \x20\x20\x20eyelidsBlendshapes: ffffffffffffffff00000000\n";
        assert_eq!(scoped_eyelids_scalar(body), None);
    }

    #[test]
    fn block_scalarの中の偽設定を拾わない() {
        // block scalar の内容は構文として解釈しない。引用符付きの正常キーを使う。
        let body = "MonoBehaviour:\n\
            \x20notes: |\n\
            \x20\x20\x20customEyeLookSettings:\n\
            \x20\x20\x20\x20\x20eyelidsBlendshapes: 000000000000000000000000\n\
            \x20\"customEyeLookSettings\":\n\
            \x20\x20\x20eyelidsBlendshapes: 000000000100000002000000\n";
        assert_eq!(
            scoped_eyelids_scalar(body),
            Some("000000000100000002000000"),
            "block scalar 内の偽設定を拾わない",
        );
    }

    #[test]
    fn block_scalarの中の偽eyelidsを拾わない() {
        let body = "MonoBehaviour:\n\
            \x20'customEyeLookSettings':\n\
            \x20\x20note: >-\n\
            \x20\x20\x20eyelidsBlendshapes: ffffffffffffffffffffffff\n\
            \x20\x20eyelidsBlendshapes: 100000001100000012000000\n";
        assert_eq!(
            scoped_eyelids_scalar(body),
            Some("100000001100000012000000"),
            "block scalar 内の偽キーを拾わない",
        );
    }

    #[test]
    fn 引用符付きキーと行末コメントを扱える() {
        let body = "MonoBehaviour:\n\
            \x20customEyeLookSettings: # 目\n\
            \x20\x20\x20eyelidsBlendshapes: 100000001100000012000000 # まぶた\n";
        assert_eq!(
            scoped_eyelids_scalar(body),
            Some("100000001100000012000000"),
        );
    }

    #[test]
    fn フロー形式と空値は拾わない() {
        let flow = "MonoBehaviour:\n\
            \x20customEyeLookSettings: {eyelidType: 2}\n";
        assert_eq!(scoped_eyelids_scalar(flow), None);
        let block = "MonoBehaviour:\n\
            \x20customEyeLookSettings:\n\
            \x20\x20\x20eyelidsBlendshapes: |\n\
            \x20\x20\x20\x20\x20eyelidsBlendshapes: 100000001100000012000000\n";
        assert_eq!(scoped_eyelids_scalar(block), None);
    }

    #[test]
    fn 値が空の場合は拾わない() {
        // Sequence形は生本文にスカラーが無いため、Value側の読みに委ねる。
        let body = "MonoBehaviour:\n\
            \x20customEyeLookSettings:\n\
            \x20\x20\x20eyelidsBlendshapes: \n\
            \x20\x20\x20eyelidType: 2\n";
        assert_eq!(scoped_eyelids_scalar(body), None);
    }

    #[test]
    fn ブロック終了を越えて拾わない() {
        // ブロック終了後の同名キーは別mappingのものであり、拾わない。
        let body = "MonoBehaviour:\n\
            \x20customEyeLookSettings:\n\
            \x20\x20\x20eyelidType: 2\n\
            \x20other:\n\
            \x20\x20\x20eyelidsBlendshapes: 000000000100000002000000\n";
        assert_eq!(scoped_eyelids_scalar(body), None);
    }

    #[test]
    fn 改行符号がcrlfでも拾える() {
        // CRLF文書でも生本文から直接の子を読む。行末の復帰は値に含めない。
        let body = "MonoBehaviour:\r\n\
            \x20customEyeLookSettings:\r\n\
            \x20\x20\x20eyelidsBlendshapes: 100000001100000012000000\r\n\
            \x20\x20\x20eyelidType: 2\r\n";
        assert_eq!(
            scoped_eyelids_scalar(body),
            Some("100000001100000012000000"),
        );
    }

    fn 最小prefab文書(
        lip_sync: &str,
        eye_look: &str,
        eyelid_type: &str,
        eyelids_scalar: &str,
    ) -> Vec<u8> {
        format!(
            "%YAML 1.1\n\
             %TAG !u! tag:unity3d.com,2011:\n\
             --- !u!114 &100\n\
             MonoBehaviour:\n\
             \x20 m_ObjectHideFlags: 0\n\
             \x20 m_CorrespondingSourceObject: {{fileID: 0}}\n\
             \x20 m_PrefabInstance: {{fileID: 0}}\n\
             \x20 m_PrefabAsset: {{fileID: 0}}\n\
             \x20 m_GameObject: {{fileID: 1}}\n\
             \x20 m_Enabled: 1\n\
             \x20 m_EditorHideFlags: 0\n\
             \x20 m_Script: {{fileID: 123, guid: 67cc4cb7839cd3741b63733d5adf0442, type: 3}}\n\
             \x20 m_Name: \n\
             \x20 m_EditorClassIdentifier: \n\
             \x20 lipSync: {lip_sync}\n\
             \x20 VisemeSkinnedMesh: {{fileID: 200}}\n\
             \x20 MouthOpenBlendShapeName: Jaw_Open\n\
             \x20 VisemeBlendShapes:\n\
             \x20 - vrc.v_sil\n\
             \x20 - vrc.v_PP\n\
             \x20 - vrc.v_FF\n\
             \x20 - vrc.v_TH\n\
             \x20 - vrc.v_DD\n\
             \x20 - vrc.v_kk\n\
             \x20 - vrc.v_CH\n\
             \x20 - vrc.v_SS\n\
             \x20 - vrc.v_nn\n\
             \x20 - vrc.v_RR\n\
             \x20 - vrc.v_aa\n\
             \x20 - vrc.v_E\n\
             \x20 - vrc.v_ih\n\
             \x20 - vrc.v_oh\n\
             \x20 - vrc.v_ou\n\
             \x20 enableEyeLook: {eye_look}\n\
             \x20 customEyeLookSettings:\n\
             \x20\x20\x20 leftEye: {{fileID: 300}}\n\
             \x20\x20\x20 rightEye: {{fileID: 400}}\n\
             \x20\x20\x20 eyelidType: {eyelid_type}\n\
             \x20\x20\x20 eyelidsSkinnedMesh: {{fileID: 500}}\n\
             \x20\x20\x20 eyelidsBlendshapes: {eyelids_scalar}\n",
        )
        .into_bytes()
    }

    fn 試験prefabを読む(本文: &[u8]) -> SerializedFile {
        SerializedFile::parse(本文, &crate::unity::ParseOptions::new()).expect("試験prefabは読める")
    }

    #[test]
    fn descriptorの欄を読む() {
        let prefab = 試験prefabを読む(&最小prefab文書(
            "3",
            "1",
            "2",
            "000000000100000002000000",
        ));
        let settings = parse_descriptor(&prefab, "試験")
            .expect("読める")
            .expect("ある");
        assert_eq!(settings.lip_sync, 3);
        assert_eq!(
            settings
                .viseme_renderer
                .as_ref()
                .expect("参照がある")
                .file_id,
            200
        );
        assert!(
            settings
                .viseme_renderer
                .as_ref()
                .expect("参照がある")
                .guid
                .is_none()
        );
        assert_eq!(settings.viseme_names.len(), 15);
        assert_eq!(settings.viseme_names[10], "vrc.v_aa");
        assert_eq!(settings.mouth_open.as_deref(), Some("Jaw_Open"));
        assert!(settings.enable_eye_look);
        assert_eq!(settings.eyelid_type, 2);
        assert_eq!(
            settings
                .eyelids_renderer
                .as_ref()
                .expect("参照がある")
                .file_id,
            500
        );
        // 生本文経由で先頭ゼロを保って読む。
        assert_eq!(settings.eyelids_indices, Some(vec![0, 1, 2]));
    }

    #[test]
    fn コメントがあっても正常値を読む() {
        // 正常値の前にコメントの同名キーがある。再現条件であり、正常値を取る。
        let 本文文字列 = String::from_utf8(最小prefab文書(
            "3",
            "1",
            "2",
            "100000001100000012000000",
        ))
        .expect("試験文書はUTF-8である");
        let 置換後 = 本文文字列.replacen(
            "    eyelidsBlendshapes: 100000001100000012000000",
            "    # eyelidsBlendshapes: 000000000000000000000000\n    eyelidsBlendshapes: 100000001100000012000000",
            1,
        );
        // 置換の成否は内容で確かめ、無関係な変更を残さない。
        assert!(置換後.contains("    # eyelidsBlendshapes: 000000000000000000000000\n"));
        let prefab = 試験prefabを読む(&置換後.into_bytes());
        let settings = parse_descriptor(&prefab, "試験")
            .expect("読める")
            .expect("ある");
        // コメントの全ゼロではなく、正常値を読む。
        assert_eq!(settings.eyelids_indices, Some(vec![16, 17, 18]));
    }

    #[test]
    fn 複数行の引用スカラー内の偽キーを設定として採用しない() {
        // 引用スカラーの続きをmappingとして解釈できないため、原文抽出を諦め、
        // 解析済みの Value（本物の eyelidsBlendshapes）へ委ねる。
        let 本文文字列 = String::from_utf8(最小prefab文書(
            "3",
            "1",
            "2",
            "100000001100000012000000",
        ))
        .expect("試験文書はUTF-8である");
        let 置換後 = 本文文字列.replacen(
            "    eyelidsBlendshapes: 100000001100000012000000",
            "    note: \"some text\n    eyelidsBlendshapes: 000000000000000000000000\n    end\"\n    eyelidsBlendshapes: 100000001100000012000000",
            1,
        );
        assert!(置換後.contains("note: \"some text"));
        let prefab = 試験prefabを読む(&置換後.into_bytes());
        let settings = parse_descriptor(&prefab, "試験")
            .expect("読める")
            .expect("ある");
        assert_eq!(settings.eyelids_indices, Some(vec![16, 17, 18]));
    }

    #[test]
    fn 実キーが無ければ引用スカラー内の偽キーを採用しない() {
        // 解析済みの Value に eyelidsBlendshapes が無い場合、生本文の偽キーを
        // 設定として採用しない。
        let 本文文字列 =
            String::from_utf8(最小prefab文書("3", "1", "2", "")).expect("試験文書はUTF-8である");
        let 置換後 = 本文文字列.replacen(
            "    eyelidsBlendshapes: \n",
            "    note: \"some text\n    eyelidsBlendshapes: 000000000000000000000000\n    end\"\n",
            1,
        );
        assert!(置換後.contains("note: \"some text"));
        let prefab = 試験prefabを読む(&置換後.into_bytes());
        let settings = parse_descriptor(&prefab, "試験")
            .expect("読める")
            .expect("ある");
        assert_eq!(settings.eyelids_indices, None);
    }

    #[test]
    fn crlf文書でも正常値を読む() {
        // 改行符号がCRLFでも生本文から直接の子を読む。行末の復帰は値に含めない。
        let 本文 = 最小prefab文書("3", "1", "2", "100000001100000012000000");
        let 本文文字列 = String::from_utf8(本文).expect("試験文書はUTF-8である");
        let crlf = 本文文字列.replace('\n', "\r\n");
        let prefab = 試験prefabを読む(crlf.as_bytes());
        let settings = parse_descriptor(&prefab, "試験")
            .expect("読める")
            .expect("ある");
        assert_eq!(settings.eyelids_indices, Some(vec![16, 17, 18]));
    }

    #[test]
    fn 壊れた参照は欠落に読み替えない() {
        // `{fileID: broken}` は形状不良であり、警告続行にせず失敗する。
        let 本文文字列 = String::from_utf8(最小prefab文書(
            "3",
            "1",
            "2",
            "100000001100000012000000",
        ))
        .expect("試験文書はUTF-8である");
        let 置換後 = 本文文字列.replacen(
            "VisemeSkinnedMesh: {fileID: 200}",
            "VisemeSkinnedMesh: {fileID: broken}",
            1,
        );
        assert!(置換後.contains("VisemeSkinnedMesh: {fileID: broken}"));
        let prefab = 試験prefabを読む(&置換後.into_bytes());
        let 結果 = parse_descriptor(&prefab, "試験");
        assert!(matches!(
            結果,
            Err(ref error) if error.kind() == VrmErrorKind::InvalidPrefab
        ));
    }

    #[test]
    fn null参照は警告して続行できる() {
        // `{fileID: 0}` は形状不良ではなく、参照なしとして読む。
        let 本文文字列 = String::from_utf8(最小prefab文書(
            "3",
            "1",
            "2",
            "100000001100000012000000",
        ))
        .expect("試験文書はUTF-8である");
        let 置換後 = 本文文字列.replacen(
            "VisemeSkinnedMesh: {fileID: 200}",
            "VisemeSkinnedMesh: {fileID: 0}",
            1,
        );
        let prefab = 試験prefabを読む(&置換後.into_bytes());
        let settings = parse_descriptor(&prefab, "試験")
            .expect("読める")
            .expect("ある");
        assert!(settings.viseme_renderer.is_none());
    }

    #[test]
    fn 口欄の無い同名フィールドはdescriptorとしない() {
        // `lipSync` 単独では別スクリプトの同名欄と区別できないため、対象外にする。
        let 本文 = b"%YAML 1.1\n\
            %TAG !u! tag:unity3d.com,2011:\n\
            --- !u!114 &100\n\
            MonoBehaviour:\n\
            \x20 m_ObjectHideFlags: 0\n\
            \x20 m_CorrespondingSourceObject: {fileID: 0}\n\
            \x20 m_PrefabInstance: {fileID: 0}\n\
            \x20 m_PrefabAsset: {fileID: 0}\n\
            \x20 m_GameObject: {fileID: 1}\n\
            \x20 m_Enabled: 1\n\
            \x20 m_EditorHideFlags: 0\n\
            \x20 m_Script: {fileID: 123, guid: 67cc4cb7839cd3741b63733d5adf0442, type: 3}\n\
            \x20 m_Name: \n\
            \x20 m_EditorClassIdentifier: \n\
            \x20 lipSync: 3\n";
        let prefab = 試験prefabを読む(本文);
        assert!(parse_descriptor(&prefab, "試験").expect("読める").is_none());
    }

    #[test]
    fn 複数のdescriptorは先勝ちにしない() {
        let mut 本文 = 最小prefab文書("3", "1", "2", "100000001100000012000000");
        本文.extend_from_slice(
            b"--- !u!114 &101\n\
              MonoBehaviour:\n\
              \x20 m_ObjectHideFlags: 0\n\
              \x20 m_CorrespondingSourceObject: {fileID: 0}\n\
              \x20 m_PrefabInstance: {fileID: 0}\n\
              \x20 m_PrefabAsset: {fileID: 0}\n\
              \x20 m_GameObject: {fileID: 1}\n\
              \x20 m_Enabled: 1\n\
              \x20 m_EditorHideFlags: 0\n\
              \x20 m_Script: {fileID: 123, guid: 67cc4cb7839cd3741b63733d5adf0442, type: 3}\n\
              \x20 m_Name: \n\
              \x20 m_EditorClassIdentifier: \n\
              \x20 lipSync: 3\n\
              \x20 VisemeSkinnedMesh: {fileID: 200}\n",
        );
        let prefab = 試験prefabを読む(&本文);
        let 結果 = parse_descriptor(&prefab, "試験");
        assert!(matches!(
            結果,
            Err(ref error) if error.kind() == VrmErrorKind::InvalidPrefab
        ));
    }

    /// 束ね試験の解決済み設定を作る。viseme15件とまぶた3件を持つ。
    fn 試験解決(
        viseme_mesh: Option<ObjectId>,
        eyelids_mesh: Option<ObjectId>,
        eyelids_indices: Option<Vec<i32>>,
    ) -> ResolvedDescriptor {
        ResolvedDescriptor {
            lip_sync: 3,
            viseme_names: vec![
                "vrc.v_sil".to_owned(),
                "vrc.v_PP".to_owned(),
                "vrc.v_FF".to_owned(),
                "vrc.v_TH".to_owned(),
                "vrc.v_DD".to_owned(),
                "vrc.v_kk".to_owned(),
                "vrc.v_CH".to_owned(),
                "vrc.v_SS".to_owned(),
                "vrc.v_nn".to_owned(),
                "vrc.v_RR".to_owned(),
                "vrc.v_aa".to_owned(),
                "vrc.v_E".to_owned(),
                "vrc.v_ih".to_owned(),
                "vrc.v_oh".to_owned(),
                "vrc.v_ou".to_owned(),
            ],
            mouth_open: Some("Jaw_Open".to_owned()),
            enable_eye_look: true,
            eyelid_type: 2,
            eyelids_indices,
            viseme_mesh,
            eyelids_mesh,
        }
    }

    /// 束ね試験のチャンネル列を作る。メッシュごとのFBX順の名前列である。
    fn 試験チャンネル() -> HashMap<ObjectId, Vec<String>> {
        let mut channels = HashMap::new();
        channels.insert(
            ObjectId::new(10),
            vec![
                "vrc.v_aa".to_owned(),
                "vrc.v_E".to_owned(),
                "vrc.v_ih".to_owned(),
                "vrc.v_oh".to_owned(),
                "vrc.v_ou".to_owned(),
                "vrc.blink".to_owned(),
                "vrc.looking_up".to_owned(),
                "vrc.looking_down".to_owned(),
            ],
        );
        channels
    }

    #[test]
    fn viseme形状は5件を順序通りに作る() {
        // aa・ih・ou・ee・ohの順であり、SDK索引10・12・14・11・13に対応する。
        let 解決 = ResolvedDescriptor {
            enable_eye_look: false,
            ..試験解決(Some(ObjectId::new(10)), None, Some(vec![1, 2, 3]))
        };
        let 計画 = build_plan(&解決, &試験チャンネル(), "試験", true).expect("作れる");
        let 式名: Vec<&str> = 計画
            .profile
            .bindings
            .iter()
            .map(|binding| binding.expression.as_str())
            .collect();
        assert_eq!(式名, vec!["aa", "ih", "ou", "ee", "oh"]);
        assert_eq!(計画.profile.bindings[0].target, "vrc.v_aa");
        assert_eq!(計画.profile.bindings[3].target, "vrc.v_E");
        assert_eq!(計画.mesh, Some(ObjectId::new(10)));
    }

    #[test]
    fn jaw開閉形状はaa1件を作る() {
        let 解決 = ResolvedDescriptor {
            lip_sync: 2,
            enable_eye_look: false,
            ..試験解決(Some(ObjectId::new(10)), None, Some(vec![1, 2, 3]))
        };
        let mut チャンネル = 試験チャンネル();
        チャンネル.insert(ObjectId::new(10), vec!["Jaw_Open".to_owned()]);
        let 計画 = build_plan(&解決, &チャンネル, "試験", true).expect("作れる");
        assert_eq!(計画.profile.bindings.len(), 1);
        assert_eq!(計画.profile.bindings[0].expression, "aa");
        assert_eq!(計画.profile.bindings[0].target, "Jaw_Open");
    }

    #[test]
    fn 骨と変数の口設定は候補を作らず警告する() {
        for lip in [0, 1, 4] {
            let 解決 = ResolvedDescriptor {
                lip_sync: lip,
                enable_eye_look: false,
                ..試験解決(Some(ObjectId::new(10)), None, Some(vec![1, 2, 3]))
            };
            let 計画 = build_plan(&解決, &試験チャンネル(), "試験", true).expect("作れる");
            assert!(計画.profile.bindings.is_empty(), "lipSync={lip}は候補なし");
            assert_eq!(計画.mesh, None);
            assert!(!計画.warnings.is_empty(), "理由を警告する");
        }
    }

    #[test]
    fn まぶた設定と目の解決で3件を作る() {
        let 解決 = 試験解決(
            Some(ObjectId::new(10)),
            Some(ObjectId::new(10)),
            Some(vec![5, 6, 7]),
        );
        let 計画 = build_plan(&解決, &試験チャンネル(), "試験", true).expect("作れる");
        let 式名: Vec<&str> = 計画
            .profile
            .bindings
            .iter()
            .map(|binding| binding.expression.as_str())
            .collect();
        assert_eq!(
            式名,
            vec!["aa", "ih", "ou", "ee", "oh", "blink", "lookUp", "lookDown"]
        );
    }

    #[test]
    fn 目視無効ではまぶたを作らず警告する() {
        let 解決 = ResolvedDescriptor {
            enable_eye_look: false,
            ..試験解決(
                Some(ObjectId::new(10)),
                Some(ObjectId::new(10)),
                Some(vec![5, 6, 7]),
            )
        };
        let 計画 = build_plan(&解決, &試験チャンネル(), "試験", true).expect("作れる");
        assert!(
            計画
                .profile
                .bindings
                .iter()
                .all(|binding| binding.expression != "blink"),
            "eyeLook無効ではblinkなし",
        );
        assert!(!計画.warnings.is_empty());
    }

    #[test]
    fn 目の未解決では瞬きだけを作る() {
        let 解決 = 試験解決(
            Some(ObjectId::new(10)),
            Some(ObjectId::new(10)),
            Some(vec![5, 6, 7]),
        );
        let 計画 = build_plan(&解決, &試験チャンネル(), "試験", false).expect("作れる");
        let 式名: Vec<&str> = 計画
            .profile
            .bindings
            .iter()
            .map(|binding| binding.expression.as_str())
            .collect();
        assert!(式名.contains(&"blink"), "blinkは作る");
        assert!(!式名.contains(&"lookUp"), "lookUpは作らない");
        assert!(!式名.contains(&"lookDown"), "lookDownは作らない");
        assert!(!計画.warnings.is_empty(), "理由を警告する");
    }

    #[test]
    fn 無効索引の式だけを落として警告する() {
        let 解決 = 試験解決(
            Some(ObjectId::new(10)),
            Some(ObjectId::new(10)),
            Some(vec![-1, 99, 7]),
        );
        let 計画 = build_plan(&解決, &試験チャンネル(), "試験", true).expect("作れる");
        let 式名: Vec<&str> = 計画
            .profile
            .bindings
            .iter()
            .map(|binding| binding.expression.as_str())
            .collect();
        assert!(!式名.contains(&"blink"), "-1は落とす");
        assert!(!式名.contains(&"lookUp"), "範囲外は落とす");
        assert!(式名.contains(&"lookDown"), "有効は残す");
        assert!(計画.warnings.len() >= 2, "落とした理由を警告する");
    }

    #[test]
    fn 候補ゼロの別メッシュでは成功する() {
        // 口5件 + まぶた別メッシュ + 索引[-1,-1,-1] → まぶた候補ゼロのため成功する。
        let 解決 = 試験解決(
            Some(ObjectId::new(10)),
            Some(ObjectId::new(20)),
            Some(vec![-1, -1, -1]),
        );
        let mut チャンネル = 試験チャンネル();
        チャンネル.insert(ObjectId::new(20), vec!["other".to_owned()]);
        let 計画 = build_plan(&解決, &チャンネル, "試験", true).expect("作れる");
        let 式名: Vec<&str> = 計画
            .profile
            .bindings
            .iter()
            .map(|binding| binding.expression.as_str())
            .collect();
        assert_eq!(式名, vec!["aa", "ih", "ou", "ee", "oh"]);
        assert_eq!(計画.mesh, Some(ObjectId::new(10)));
        assert!(!計画.warnings.is_empty(), "落とした理由を警告する");
    }

    #[test]
    fn 別メッシュの両候補は部分成功にしない() {
        let 解決 = 試験解決(
            Some(ObjectId::new(10)),
            Some(ObjectId::new(20)),
            Some(vec![5, 6, 7]),
        );
        let mut チャンネル = 試験チャンネル();
        チャンネル.insert(
            ObjectId::new(20),
            vec![
                "vrc.blink".to_owned(),
                "vrc.looking_up".to_owned(),
                "vrc.looking_down".to_owned(),
                "x".to_owned(),
                "x".to_owned(),
                "vrc.blink".to_owned(),
                "vrc.looking_up".to_owned(),
                "vrc.looking_down".to_owned(),
            ],
        );
        let 結果 = build_plan(&解決, &チャンネル, "試験", true);
        assert!(matches!(
            結果,
            Err(ref error) if error.kind() == VrmErrorKind::InvalidPrefab
        ));
    }

    #[test]
    fn 有効な瞬きだけでも別メッシュなら失敗する() {
        // 口5件 + まぶた別メッシュ + 索引[0,-1,-1] でblink名が実在 → 両側候補で失敗する。
        let 解決 = 試験解決(
            Some(ObjectId::new(10)),
            Some(ObjectId::new(20)),
            Some(vec![0, -1, -1]),
        );
        let mut チャンネル = 試験チャンネル();
        チャンネル.insert(ObjectId::new(20), vec!["vrc.blink".to_owned()]);
        let 結果 = build_plan(&解決, &チャンネル, "試験", true);
        assert!(matches!(
            結果,
            Err(ref error) if error.kind() == VrmErrorKind::InvalidPrefab
        ));
    }

    #[test]
    fn 短いviseme列は不足分だけ警告して残りを作る() {
        // 12件の列ではSDK索引10・11が作れ、12-14は警告する。
        let mut 解決 = 試験解決(Some(ObjectId::new(10)), None, Some(vec![5, 6, 7]));
        解決.enable_eye_look = false;
        解決.viseme_names = vec![
            "vrc.v_sil".to_owned(),
            "vrc.v_PP".to_owned(),
            "vrc.v_FF".to_owned(),
            "vrc.v_TH".to_owned(),
            "vrc.v_DD".to_owned(),
            "vrc.v_kk".to_owned(),
            "vrc.v_CH".to_owned(),
            "vrc.v_SS".to_owned(),
            "vrc.v_nn".to_owned(),
            "vrc.v_RR".to_owned(),
            "vrc.v_aa".to_owned(),
            "vrc.v_E".to_owned(),
        ];
        let 計画 = build_plan(&解決, &試験チャンネル(), "試験", true).expect("作れる");
        let 式名: Vec<&str> = 計画
            .profile
            .bindings
            .iter()
            .map(|binding| binding.expression.as_str())
            .collect();
        assert_eq!(式名, vec!["aa", "ee"]);
        assert_eq!(計画.mesh, Some(ObjectId::new(10)));
        for index in [12, 13, 14] {
            assert!(
                計画
                    .warnings
                    .iter()
                    .any(|警告| 警告.contains(&format!("SDK索引{index}"))),
                "SDK索引{index}の不足を警告する",
            );
        }
    }

    #[test]
    fn メッシュに無いviseme名はその式だけ落とす() {
        let mut 解決 = 試験解決(Some(ObjectId::new(10)), None, Some(vec![5, 6, 7]));
        解決.enable_eye_look = false;
        解決.viseme_names[10] = "vrc.v_missing".to_owned();
        let 計画 = build_plan(&解決, &試験チャンネル(), "試験", true).expect("作れる");
        let 式名: Vec<&str> = 計画
            .profile
            .bindings
            .iter()
            .map(|binding| binding.expression.as_str())
            .collect();
        assert_eq!(式名, vec!["ih", "ou", "ee", "oh"]);
        assert_eq!(計画.mesh, Some(ObjectId::new(10)));
        assert!(!計画.warnings.is_empty());
    }

    /// 参照解決試験のPrefabInstance文書を作る。
    fn instance文書(文書id: i64, 元guid: Option<&str>) -> Vec<u8> {
        let 参照 = match 元guid {
            Some(guid) => format!("{{fileID: 100100000, guid: {guid}, type: 3}}"),
            None => "{fileID: 100100000}".to_owned(),
        };
        format!(
            "--- !u!1001 &{文書id}\n\
             PrefabInstance:\n\
             \x20 m_SourcePrefab: {参照}\n"
        )
        .into_bytes()
    }

    /// 参照解決試験のstripped描画設定文書を作る。
    fn stripped文書(
        文書id: i64, 対応id: i64, 対応guid: Option<&str>, 所属: i64
    ) -> Vec<u8> {
        stripped任意class文書(
            文書id,
            137,
            "SkinnedMeshRenderer",
            対応id,
            対応guid,
            Some(所属),
        )
    }

    /// 参照解決試験のstripped文書を、クラスと所属を指定して作る。
    fn stripped任意class文書(
        文書id: i64,
        class_id: i32,
        class_name: &str,
        対応id: i64,
        対応guid: Option<&str>,
        所属: Option<i64>,
    ) -> Vec<u8> {
        let 対応 = match 対応guid {
            Some(guid) => format!("{{fileID: {対応id}, guid: {guid}, type: 3}}"),
            None => format!("{{fileID: {対応id}}}"),
        };
        let 所属 = match 所属 {
            Some(所属) => format!("{{fileID: {所属}}}"),
            None => "{fileID: 0}".to_owned(),
        };
        format!(
            "--- !u!{class_id} &{文書id} stripped\n\
             {class_name}:\n\
             \x20 m_CorrespondingSourceObject: {対応}\n\
             \x20 m_PrefabInstance: {所属}\n\
             \x20 m_PrefabAsset: {{fileID: 0}}\n"
        )
        .into_bytes()
    }

    /// 参照解決試験の直接記述の描画設定文書を作る。
    fn 直接文書(文書id: i64, 網id: i64, 網guid: Option<&str>) -> Vec<u8> {
        let 網 = match 網guid {
            Some(guid) => format!("{{fileID: {網id}, guid: {guid}, type: 3}}"),
            None => format!("{{fileID: {網id}}}"),
        };
        format!(
            "--- !u!137 &{文書id}\n\
             SkinnedMeshRenderer:\n\
             \x20 m_ObjectHideFlags: 0\n\
             \x20 m_CorrespondingSourceObject: {{fileID: 0}}\n\
             \x20 m_PrefabInstance: {{fileID: 0}}\n\
             \x20 m_PrefabAsset: {{fileID: 0}}\n\
             \x20 m_GameObject: {{fileID: 1}}\n\
             \x20 m_Enabled: 1\n\
             \x20 m_Mesh: {網}\n\
             \x20 m_Materials: []\n\
             \x20 m_Bones: []\n\
             \x20 m_RootBone: {{fileID: 0}}\n"
        )
        .into_bytes()
    }

    fn 試験表prefabを読む(部品ら: &[Vec<u8>]) -> SerializedFile {
        let mut 本文 = b"%YAML 1.1\n%TAG !u! tag:unity3d.com,2011:\n".to_vec();
        for 部品 in 部品ら {
            本文.extend_from_slice(部品);
        }
        試験prefabを読む(&本文)
    }

    const 試験GUID: &str = "9ef94258554e06b4ab03fbe9fc914c5c";
    const 別GUID: &str = "7464d0b6ac9485e4c9b53c7a0f61a372";

    #[test]
    fn 一致するfbxのinstanceを特定する() {
        let prefab = 試験表prefabを読む(&[
            instance文書(100, Some(試験GUID)),
            instance文書(101, Some(別GUID)),
        ]);
        assert_eq!(
            find_fbx_instance(&prefab, "試験", 試験GUID).expect("読める"),
            Some(100)
        );
    }

    #[test]
    fn 複数のfbxのinstanceは先勝ちにしない() {
        let prefab = 試験表prefabを読む(&[
            instance文書(100, Some(試験GUID)),
            instance文書(101, Some(試験GUID)),
        ]);
        let 結果 = find_fbx_instance(&prefab, "試験", 試験GUID);
        assert!(matches!(
            結果,
            Err(ref error) if error.kind() == VrmErrorKind::InvalidPrefab
        ));
    }

    #[test]
    fn 一致しないinstanceは直接記述とみなす() {
        let prefab = 試験表prefabを読む(&[instance文書(100, Some(別GUID))]);
        assert_eq!(
            find_fbx_instance(&prefab, "試験", 試験GUID).expect("読める"),
            None
        );
    }

    #[test]
    fn strippedはguidと所属が一致する場合だけ使う() {
        let mesh = ObjectId::new(10);
        let mut 網表 = HashMap::new();
        網表.insert(700, mesh);
        let mut 描画表 = HashMap::new();
        描画表.insert(900, mesh);
        // 正常・GUID不一致・所属不一致・対応先不明の4件を混ぜる。
        let prefab = 試験表prefabを読む(&[
            stripped文書(500, 900, Some(試験GUID), 100),
            stripped文書(501, 900, Some(別GUID), 100),
            stripped文書(502, 900, Some(試験GUID), 999),
            stripped文書(503, 901, Some(試験GUID), 100),
        ]);
        let 表 = build_renderer_meshes(&prefab, "試験", &網表, &描画表, 試験GUID, Some(100))
            .expect("作れる");
        assert_eq!(表.get(&500), Some(&mesh), "正常だけを使う");
        assert!(!表.contains_key(&501), "GUID不一致は使わない");
        assert!(!表.contains_key(&502), "所属不一致は使わない");
        assert!(!表.contains_key(&503), "対応先不明は使わない");
    }

    #[test]
    fn 直接記述は網guidが一致する場合だけ使う() {
        let mesh = ObjectId::new(10);
        let mut 網表 = HashMap::new();
        網表.insert(700, mesh);
        let 描画表 = HashMap::new();
        let prefab = 試験表prefabを読む(&[
            直接文書(600, 700, Some(試験GUID)),
            直接文書(601, 700, Some(別GUID)),
            直接文書(602, 700, None),
        ]);
        let 表 =
            build_renderer_meshes(&prefab, "試験", &網表, &描画表, 試験GUID, None).expect("作れる");
        assert_eq!(表.get(&600), Some(&mesh), "正常だけを使う");
        assert!(!表.contains_key(&601), "GUID不一致は使わない");
        assert!(!表.contains_key(&602), "GUID無しは使わない");
    }

    #[test]
    fn strippedはskinnedmeshrendererだけを使う() {
        let mesh = ObjectId::new(10);
        let mut 網表 = HashMap::new();
        網表.insert(700, mesh);
        let mut 描画表 = HashMap::new();
        描画表.insert(900, mesh);
        // class 4（Transform）のstripped文書は、対応元IDがrendererでも使わない。
        let prefab = 試験表prefabを読む(&[stripped任意class文書(
            500,
            4,
            "Transform",
            900,
            Some(試験GUID),
            Some(100),
        )]);
        let 表 = build_renderer_meshes(&prefab, "試験", &網表, &描画表, 試験GUID, Some(100))
            .expect("作れる");
        assert!(表.is_empty(), "Transformは描画設定として使わない");
    }

    #[test]
    fn strippedの所属はローカル参照だけを使う() {
        let mesh = ObjectId::new(10);
        let mut 網表 = HashMap::new();
        網表.insert(700, mesh);
        let mut 描画表 = HashMap::new();
        描画表.insert(900, mesh);
        // m_PrefabInstance に外部GUIDを付けても、fileIDが一致するだけでは使わない。
        let doc = format!(
            "--- !u!137 &500 stripped\n\
             SkinnedMeshRenderer:\n\
             \x20 m_CorrespondingSourceObject: {{fileID: 900, guid: {試験GUID}, type: 3}}\n\
             \x20 m_PrefabInstance: {{fileID: 100, guid: {別GUID}, type: 3}}\n\
             \x20 m_PrefabAsset: {{fileID: 0}}\n"
        );
        let prefab = 試験表prefabを読む(&[doc.into_bytes()]);
        let 表 = build_renderer_meshes(&prefab, "試験", &網表, &描画表, 試験GUID, Some(100))
            .expect("作れる");
        assert!(表.is_empty(), "外部GUID付きの所属は使わない");
    }

    #[test]
    fn 取り込みidを直接指す参照は解決できない() {
        // 最終表は文書IDだけを持つ。取り込みrenderer IDは文書ではないため、
        // 参照がそれを指した場合はdanglingとして失敗する。
        let mesh = ObjectId::new(10);
        let mut 描画表 = HashMap::new();
        描画表.insert(900, mesh);
        let 文書ら: HashSet<FileId> = HashSet::new();
        let 参照 = ObjectRef {
            file_id: 900,
            guid: None,
            type_hint: Some(3),
        };
        assert!(matches!(
            resolve_side_ref(Some(&参照), "VisemeSkinnedMesh", &文書ら, &描画表, "試験"),
            Err(ref error) if error.kind() == VrmErrorKind::InvalidPrefab
        ));
    }

    #[test]
    fn 対応表は検証済み文書だけを持つ() {
        let mesh = ObjectId::new(10);
        let mut 描画表 = HashMap::new();
        描画表.insert(900, mesh);
        let 網表 = HashMap::new();
        // 文書が無いPrefabでは、取り込みIDだけの表にならない。
        let prefab = 試験表prefabを読む(&[]);
        let 表 = build_renderer_meshes(&prefab, "試験", &網表, &描画表, 試験GUID, Some(100))
            .expect("作れる");
        assert!(表.is_empty(), "取り込みIDを最終表へcloneしない");
    }

    #[test]
    fn 参照解決は外部とdanglingと不能を失敗にする() {
        let mesh = ObjectId::new(10);
        let mut 表 = HashMap::new();
        表.insert(200, mesh);
        let 文書ら: HashSet<FileId> = [200].into_iter().collect();
        // 外部GUID付き参照は失敗する。
        let 外部 = ObjectRef {
            file_id: 200,
            guid: Some(crate::unity::Guid::parse(試験GUID).expect("試験GUIDは読める")),
            type_hint: Some(3),
        };
        assert!(matches!(
            resolve_side_ref(Some(&外部), "VisemeSkinnedMesh", &文書ら, &表, "試験"),
            Err(ref error) if error.kind() == VrmErrorKind::InvalidPrefab
        ));
        // danglingは失敗する。
        let 宙 = ObjectRef {
            file_id: 999,
            guid: None,
            type_hint: None,
        };
        assert!(matches!(
            resolve_side_ref(Some(&宙), "VisemeSkinnedMesh", &文書ら, &表, "試験"),
            Err(ref error) if error.kind() == VrmErrorKind::InvalidPrefab
        ));
        // 文書はあるが解決できない場合も失敗する。
        let 未対応 = ObjectRef {
            file_id: 300,
            guid: None,
            type_hint: None,
        };
        let 文書ら: HashSet<FileId> = [200, 300].into_iter().collect();
        assert!(matches!(
            resolve_side_ref(Some(&未対応), "VisemeSkinnedMesh", &文書ら, &表, "試験"),
            Err(ref error) if error.kind() == VrmErrorKind::InvalidPrefab
        ));
        // 正常とnullは通る。
        let 正常 = ObjectRef {
            file_id: 200,
            guid: None,
            type_hint: None,
        };
        assert_eq!(
            resolve_side_ref(Some(&正常), "VisemeSkinnedMesh", &文書ら, &表, "試験")
                .expect("解ける"),
            Some(mesh)
        );
        assert_eq!(
            resolve_side_ref(None, "VisemeSkinnedMesh", &文書ら, &表, "試験").expect("通る"),
            None
        );
    }
}
