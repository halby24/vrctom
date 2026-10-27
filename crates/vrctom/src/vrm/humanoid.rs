//! VRM humanoidの骨対応付けと検証。
//!
//! 入口は [`HumanoidBones::detect`] であり、ノード名の正規化で骨へ対応付ける。
//! 呼び出し側は [`HumanoidBones`] を明示指定して名前推測を使わないこともできる。
//! 親子・拡大率の検証は書き出し時に行い、ここでは対応付けだけを担う。

use std::collections::BTreeMap;

use super::VrmModel;
use super::export::VrmExportError;

/// VRM humanoidの骨。指は対応付けない。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum HumanoidBone {
    /// 腰。
    Hips,
    /// 背骨。
    Spine,
    /// 胸。
    Chest,
    /// 上胸。
    UpperChest,
    /// 首。
    Neck,
    /// 頭。
    Head,
    /// 左目。
    LeftEye,
    /// 右目。
    RightEye,
    /// 左肩。
    LeftShoulder,
    /// 右肩。
    RightShoulder,
    /// 左上腕。
    LeftUpperArm,
    /// 右上腕。
    RightUpperArm,
    /// 左前腕。
    LeftLowerArm,
    /// 右前腕。
    RightLowerArm,
    /// 左手。
    LeftHand,
    /// 右手。
    RightHand,
    /// 左大腿。
    LeftUpperLeg,
    /// 右大腿。
    RightUpperLeg,
    /// 左下腿。
    LeftLowerLeg,
    /// 右下腿。
    RightLowerLeg,
    /// 左足。
    LeftFoot,
    /// 右足。
    RightFoot,
    /// 左足先。
    LeftToes,
    /// 右足先。
    RightToes,
}

impl HumanoidBone {
    /// すべての骨を列挙順で返す。
    pub(crate) fn all() -> impl Iterator<Item = Self> {
        [
            Self::Hips,
            Self::Spine,
            Self::Chest,
            Self::UpperChest,
            Self::Neck,
            Self::Head,
            Self::LeftEye,
            Self::RightEye,
            Self::LeftShoulder,
            Self::RightShoulder,
            Self::LeftUpperArm,
            Self::RightUpperArm,
            Self::LeftLowerArm,
            Self::RightLowerArm,
            Self::LeftHand,
            Self::RightHand,
            Self::LeftUpperLeg,
            Self::RightUpperLeg,
            Self::LeftLowerLeg,
            Self::RightLowerLeg,
            Self::LeftFoot,
            Self::RightFoot,
            Self::LeftToes,
            Self::RightToes,
        ]
        .into_iter()
    }

    /// VRMのキー名を返す。
    pub fn name(self) -> &'static str {
        match self {
            Self::Hips => "hips",
            Self::Spine => "spine",
            Self::Chest => "chest",
            Self::UpperChest => "upperChest",
            Self::Neck => "neck",
            Self::Head => "head",
            Self::LeftEye => "leftEye",
            Self::RightEye => "rightEye",
            Self::LeftShoulder => "leftShoulder",
            Self::RightShoulder => "rightShoulder",
            Self::LeftUpperArm => "leftUpperArm",
            Self::RightUpperArm => "rightUpperArm",
            Self::LeftLowerArm => "leftLowerArm",
            Self::RightLowerArm => "rightLowerArm",
            Self::LeftHand => "leftHand",
            Self::RightHand => "rightHand",
            Self::LeftUpperLeg => "leftUpperLeg",
            Self::RightUpperLeg => "rightUpperLeg",
            Self::LeftLowerLeg => "leftLowerLeg",
            Self::RightLowerLeg => "rightLowerLeg",
            Self::LeftFoot => "leftFoot",
            Self::RightFoot => "rightFoot",
            Self::LeftToes => "leftToes",
            Self::RightToes => "rightToes",
        }
    }

    /// VRM 1.0の必須骨かどうかを返す。
    ///
    /// 必須は hips・spine・head・左右の upperArm/lowerArm/hand・
    /// 左右の upperLeg/lowerLeg/foot の15個である。
    pub fn is_required(self) -> bool {
        matches!(
            self,
            Self::Hips
                | Self::Spine
                | Self::Head
                | Self::LeftUpperArm
                | Self::RightUpperArm
                | Self::LeftLowerArm
                | Self::RightLowerArm
                | Self::LeftHand
                | Self::RightHand
                | Self::LeftUpperLeg
                | Self::RightUpperLeg
                | Self::LeftLowerLeg
                | Self::RightLowerLeg
                | Self::LeftFoot
                | Self::RightFoot
        )
    }

    /// 左右の別を返す。中央の骨は `None` である。
    fn side(self) -> Option<Side> {
        match self {
            Self::LeftEye
            | Self::LeftShoulder
            | Self::LeftUpperArm
            | Self::LeftLowerArm
            | Self::LeftHand
            | Self::LeftUpperLeg
            | Self::LeftLowerLeg
            | Self::LeftFoot
            | Self::LeftToes => Some(Side::Left),
            Self::RightEye
            | Self::RightShoulder
            | Self::RightUpperArm
            | Self::RightLowerArm
            | Self::RightHand
            | Self::RightUpperLeg
            | Self::RightLowerLeg
            | Self::RightFoot
            | Self::RightToes => Some(Side::Right),
            _ => None,
        }
    }

    /// 同じ側の骨に置き換える。左右を持たない骨は自分を返す。
    fn with_side(self, side: Side) -> Self {
        match (self, side) {
            (Self::LeftEye | Self::RightEye, Side::Left) => Self::LeftEye,
            (Self::LeftEye | Self::RightEye, Side::Right) => Self::RightEye,
            (Self::LeftShoulder | Self::RightShoulder, Side::Left) => Self::LeftShoulder,
            (Self::LeftShoulder | Self::RightShoulder, Side::Right) => Self::RightShoulder,
            (Self::LeftUpperArm | Self::RightUpperArm, Side::Left) => Self::LeftUpperArm,
            (Self::LeftUpperArm | Self::RightUpperArm, Side::Right) => Self::RightUpperArm,
            (Self::LeftLowerArm | Self::RightLowerArm, Side::Left) => Self::LeftLowerArm,
            (Self::LeftLowerArm | Self::RightLowerArm, Side::Right) => Self::RightLowerArm,
            (Self::LeftHand | Self::RightHand, Side::Left) => Self::LeftHand,
            (Self::LeftHand | Self::RightHand, Side::Right) => Self::RightHand,
            (Self::LeftUpperLeg | Self::RightUpperLeg, Side::Left) => Self::LeftUpperLeg,
            (Self::LeftUpperLeg | Self::RightUpperLeg, Side::Right) => Self::RightUpperLeg,
            (Self::LeftLowerLeg | Self::RightLowerLeg, Side::Left) => Self::LeftLowerLeg,
            (Self::LeftLowerLeg | Self::RightLowerLeg, Side::Right) => Self::RightLowerLeg,
            (Self::LeftFoot | Self::RightFoot, Side::Left) => Self::LeftFoot,
            (Self::LeftFoot | Self::RightFoot, Side::Right) => Self::RightFoot,
            (Self::LeftToes | Self::RightToes, Side::Left) => Self::LeftToes,
            (Self::LeftToes | Self::RightToes, Side::Right) => Self::RightToes,
            (itself, _) => itself,
        }
    }

    /// 最も近いhumanoid祖先として許す骨を返す。
    ///
    /// 空は祖先なしだけを許す。左右のある骨は同側で置き換えて使う。
    pub(crate) fn allowed_parents(self) -> &'static [Self] {
        match self {
            Self::Hips => &[],
            Self::Spine => &[Self::Hips],
            Self::Chest => &[Self::Spine],
            Self::UpperChest => &[Self::Chest],
            Self::Neck => &[Self::Chest, Self::UpperChest, Self::Spine],
            Self::Head => &[Self::Neck, Self::Chest, Self::UpperChest, Self::Spine],
            Self::LeftEye | Self::RightEye => &[Self::Head],
            Self::LeftShoulder | Self::RightShoulder => {
                &[Self::Chest, Self::UpperChest, Self::Spine]
            }
            Self::LeftUpperArm | Self::RightUpperArm => &[
                Self::LeftShoulder,
                Self::Chest,
                Self::UpperChest,
                Self::Spine,
            ],
            Self::LeftLowerArm | Self::RightLowerArm => &[Self::LeftUpperArm],
            Self::LeftHand | Self::RightHand => &[Self::LeftLowerArm],
            Self::LeftUpperLeg | Self::RightUpperLeg => &[Self::Hips],
            Self::LeftLowerLeg | Self::RightLowerLeg => &[Self::LeftUpperLeg],
            Self::LeftFoot | Self::RightFoot => &[Self::LeftLowerLeg],
            Self::LeftToes | Self::RightToes => &[Self::LeftFoot],
        }
    }
}

/// 左右の別。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    Left,
    Right,
}

/// 骨とノード索引の対応付け。
///
/// フィールドは非公開で、`set` で上書きできる。`detect` は名前から作る。
#[derive(Debug, Clone, Default)]
pub struct HumanoidBones {
    bones: BTreeMap<HumanoidBone, usize>,
}

impl HumanoidBones {
    /// 空の対応付けを作る。
    pub fn new() -> Self {
        Self::default()
    }

    /// 骨にノードを対応付ける。同じ骨への再設定は上書きする。
    pub fn set(&mut self, bone: HumanoidBone, node: usize) {
        self.bones.insert(bone, node);
    }

    /// 骨のノード索引を返す。未対応は `None` である。
    pub fn get(&self, bone: HumanoidBone) -> Option<usize> {
        self.bones.get(&bone).copied()
    }

    /// 対応付けを骨の列挙順で返す。
    pub fn iter(&self) -> impl Iterator<Item = (HumanoidBone, usize)> + '_ {
        self.bones.iter().map(|(bone, node)| (*bone, *node))
    }

    /// ノード名から対応付ける。
    ///
    /// 名前は英字の大文字小文字を無視し、空白・`_`・`.`・`-` を除去して比べる。
    /// 側は末尾の区切り付き `L`/`R`、または先頭の `left`/`right` で判定する。
    /// 指は対応付けない。
    ///
    /// # Errors
    ///
    /// 同じ骨に複数ノードが一致した場合・必須骨が無い場合に返す。
    pub fn detect(model: &VrmModel) -> Result<Self, VrmExportError> {
        let mut bones = Self::new();
        for (index, node) in model.nodes.iter().enumerate() {
            let Some(bone) = match_bone(&node.name) else {
                continue;
            };
            if let Some(_previous) = bones.get(bone) {
                return Err(VrmExportError::duplicate_bone(
                    bone,
                    index,
                    "同じ骨に複数のノードが一致した",
                ));
            }
            bones.set(bone, index);
        }
        for bone in HumanoidBone::all() {
            if bone.is_required() && bones.get(bone).is_none() {
                return Err(VrmExportError::missing_bone(bone, "必須のhumanoid骨が無い"));
            }
        }
        Ok(bones)
    }
}

/// ノード名から骨を求める。一致しない場合は `None` である。
///
/// 正規化は小文字化のうえ区切り（空白・`_`・`.`・`-`）の除去であり、
/// 側の接尾辞だけでは中央骨の語尾と区別できないため、接尾辞の候補語幹が
/// 既知の語幹の場合だけ側として扱う。
fn match_bone(name: &str) -> Option<HumanoidBone> {
    let normalized: String = name
        .to_lowercase()
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '_' && *c != '.' && *c != '-')
        .collect();
    if normalized.is_empty() {
        return None;
    }
    let (side, stem) = split_side(&normalized)?;
    Some(match (side, stem) {
        (None, "hips" | "hip") => HumanoidBone::Hips,
        (None, "spine") => HumanoidBone::Spine,
        (None, "chest") => HumanoidBone::Chest,
        (None, "upperchest") => HumanoidBone::UpperChest,
        (None, "neck") => HumanoidBone::Neck,
        (None, "head") => HumanoidBone::Head,
        (Some(Side::Left), "eye") => HumanoidBone::LeftEye,
        (Some(Side::Right), "eye") => HumanoidBone::RightEye,
        (Some(Side::Left), "shoulder") => HumanoidBone::LeftShoulder,
        (Some(Side::Right), "shoulder") => HumanoidBone::RightShoulder,
        (Some(Side::Left), "upperarm") => HumanoidBone::LeftUpperArm,
        (Some(Side::Right), "upperarm") => HumanoidBone::RightUpperArm,
        (Some(Side::Left), "lowerarm") => HumanoidBone::LeftLowerArm,
        (Some(Side::Right), "lowerarm") => HumanoidBone::RightLowerArm,
        (Some(Side::Left), "hand") => HumanoidBone::LeftHand,
        (Some(Side::Right), "hand") => HumanoidBone::RightHand,
        (Some(Side::Left), "upperleg") => HumanoidBone::LeftUpperLeg,
        (Some(Side::Right), "upperleg") => HumanoidBone::RightUpperLeg,
        (Some(Side::Left), "lowerleg") => HumanoidBone::LeftLowerLeg,
        (Some(Side::Right), "lowerleg") => HumanoidBone::RightLowerLeg,
        (Some(Side::Left), "foot") => HumanoidBone::LeftFoot,
        (Some(Side::Right), "foot") => HumanoidBone::RightFoot,
        (Some(Side::Left), "toe" | "toes") => HumanoidBone::LeftToes,
        (Some(Side::Right), "toe" | "toes") => HumanoidBone::RightToes,
        _ => return None,
    })
}

/// 正規化名を側と語幹に分ける。語幹が空の場合は `None` である。
fn split_side(normalized: &str) -> Option<(Option<Side>, &str)> {
    if let Some(stem) = normalized.strip_prefix("left") {
        if stem.is_empty() {
            return None;
        }
        return Some((Some(Side::Left), stem));
    }
    if let Some(stem) = normalized.strip_prefix("right") {
        if stem.is_empty() {
            return None;
        }
        return Some((Some(Side::Right), stem));
    }
    // 末尾の `l`/`r` は中央骨の語尾（例: shoulder）と区別できないため、
    // 残りが既知の語幹の場合だけ側の接尾辞として扱う。
    for side in [Side::Left, Side::Right] {
        let suffix = match side {
            Side::Left => "l",
            Side::Right => "r",
        };
        if let Some(stem) = normalized.strip_suffix(suffix)
            && is_side_stem(stem)
        {
            return Some((Some(side), stem));
        }
    }
    Some((None, normalized))
}

/// 側の接尾辞を受け付ける語幹かどうかを返す。
fn is_side_stem(stem: &str) -> bool {
    matches!(
        stem,
        "eye"
            | "shoulder"
            | "upperarm"
            | "lowerarm"
            | "hand"
            | "upperleg"
            | "lowerleg"
            | "foot"
            | "toe"
            | "toes"
    )
}

/// 親子検証のために、骨の許す親を同側で具体化する。
pub(crate) fn allowed_parents_for(bone: HumanoidBone) -> Vec<HumanoidBone> {
    match bone.side() {
        Some(side) => bone
            .allowed_parents()
            .iter()
            .map(|parent| parent.with_side(side))
            .collect(),
        None => bone.allowed_parents().to_vec(),
    }
}
