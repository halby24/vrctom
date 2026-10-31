//! VRM humanoidの骨対応付けと検証。
//!
//! 入口は [`HumanoidBones::detect`] であり、ノード名の正規化で骨へ対応付ける。
//! 呼び出し側は [`HumanoidBones`] を明示指定して名前推測を使わないこともできる。
//! 親子・拡大率の検証は書き出し時に行い、ここでは対応付けだけを担う。

use std::collections::BTreeMap;

use super::VrmModel;
use super::export::VrmExportError;

/// VRM humanoidの骨。
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
    /// 左親指の中手骨。
    LeftThumbMetacarpal,
    /// 左親指の基節骨。
    LeftThumbProximal,
    /// 左親指の末節骨。
    LeftThumbDistal,
    /// 左人差し指の付け根。
    LeftIndexProximal,
    /// 左人差し指の中節。
    LeftIndexIntermediate,
    /// 左人差し指の先端。
    LeftIndexDistal,
    /// 左中指の付け根。
    LeftMiddleProximal,
    /// 左中指の中節。
    LeftMiddleIntermediate,
    /// 左中指の先端。
    LeftMiddleDistal,
    /// 左薬指の付け根。
    LeftRingProximal,
    /// 左薬指の中節。
    LeftRingIntermediate,
    /// 左薬指の先端。
    LeftRingDistal,
    /// 左小指の付け根。
    LeftLittleProximal,
    /// 左小指の中節。
    LeftLittleIntermediate,
    /// 左小指の先端。
    LeftLittleDistal,
    /// 右親指の中手骨。
    RightThumbMetacarpal,
    /// 右親指の基節骨。
    RightThumbProximal,
    /// 右親指の末節骨。
    RightThumbDistal,
    /// 右人差し指の付け根。
    RightIndexProximal,
    /// 右人差し指の中節。
    RightIndexIntermediate,
    /// 右人差し指の先端。
    RightIndexDistal,
    /// 右中指の付け根。
    RightMiddleProximal,
    /// 右中指の中節。
    RightMiddleIntermediate,
    /// 右中指の先端。
    RightMiddleDistal,
    /// 右薬指の付け根。
    RightRingProximal,
    /// 右薬指の中節。
    RightRingIntermediate,
    /// 右薬指の先端。
    RightRingDistal,
    /// 右小指の付け根。
    RightLittleProximal,
    /// 右小指の中節。
    RightLittleIntermediate,
    /// 右小指の先端。
    RightLittleDistal,
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
            Self::LeftThumbMetacarpal,
            Self::LeftThumbProximal,
            Self::LeftThumbDistal,
            Self::LeftIndexProximal,
            Self::LeftIndexIntermediate,
            Self::LeftIndexDistal,
            Self::LeftMiddleProximal,
            Self::LeftMiddleIntermediate,
            Self::LeftMiddleDistal,
            Self::LeftRingProximal,
            Self::LeftRingIntermediate,
            Self::LeftRingDistal,
            Self::LeftLittleProximal,
            Self::LeftLittleIntermediate,
            Self::LeftLittleDistal,
            Self::RightThumbMetacarpal,
            Self::RightThumbProximal,
            Self::RightThumbDistal,
            Self::RightIndexProximal,
            Self::RightIndexIntermediate,
            Self::RightIndexDistal,
            Self::RightMiddleProximal,
            Self::RightMiddleIntermediate,
            Self::RightMiddleDistal,
            Self::RightRingProximal,
            Self::RightRingIntermediate,
            Self::RightRingDistal,
            Self::RightLittleProximal,
            Self::RightLittleIntermediate,
            Self::RightLittleDistal,
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
            Self::LeftThumbMetacarpal => "leftThumbMetacarpal",
            Self::LeftThumbProximal => "leftThumbProximal",
            Self::LeftThumbDistal => "leftThumbDistal",
            Self::LeftIndexProximal => "leftIndexProximal",
            Self::LeftIndexIntermediate => "leftIndexIntermediate",
            Self::LeftIndexDistal => "leftIndexDistal",
            Self::LeftMiddleProximal => "leftMiddleProximal",
            Self::LeftMiddleIntermediate => "leftMiddleIntermediate",
            Self::LeftMiddleDistal => "leftMiddleDistal",
            Self::LeftRingProximal => "leftRingProximal",
            Self::LeftRingIntermediate => "leftRingIntermediate",
            Self::LeftRingDistal => "leftRingDistal",
            Self::LeftLittleProximal => "leftLittleProximal",
            Self::LeftLittleIntermediate => "leftLittleIntermediate",
            Self::LeftLittleDistal => "leftLittleDistal",
            Self::RightThumbMetacarpal => "rightThumbMetacarpal",
            Self::RightThumbProximal => "rightThumbProximal",
            Self::RightThumbDistal => "rightThumbDistal",
            Self::RightIndexProximal => "rightIndexProximal",
            Self::RightIndexIntermediate => "rightIndexIntermediate",
            Self::RightIndexDistal => "rightIndexDistal",
            Self::RightMiddleProximal => "rightMiddleProximal",
            Self::RightMiddleIntermediate => "rightMiddleIntermediate",
            Self::RightMiddleDistal => "rightMiddleDistal",
            Self::RightRingProximal => "rightRingProximal",
            Self::RightRingIntermediate => "rightRingIntermediate",
            Self::RightRingDistal => "rightRingDistal",
            Self::RightLittleProximal => "rightLittleProximal",
            Self::RightLittleIntermediate => "rightLittleIntermediate",
            Self::RightLittleDistal => "rightLittleDistal",
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
            | Self::LeftToes
            | Self::LeftThumbMetacarpal
            | Self::LeftThumbProximal
            | Self::LeftThumbDistal
            | Self::LeftIndexProximal
            | Self::LeftIndexIntermediate
            | Self::LeftIndexDistal
            | Self::LeftMiddleProximal
            | Self::LeftMiddleIntermediate
            | Self::LeftMiddleDistal
            | Self::LeftRingProximal
            | Self::LeftRingIntermediate
            | Self::LeftRingDistal
            | Self::LeftLittleProximal
            | Self::LeftLittleIntermediate
            | Self::LeftLittleDistal => Some(Side::Left),
            Self::RightEye
            | Self::RightShoulder
            | Self::RightUpperArm
            | Self::RightLowerArm
            | Self::RightHand
            | Self::RightUpperLeg
            | Self::RightLowerLeg
            | Self::RightFoot
            | Self::RightToes
            | Self::RightThumbMetacarpal
            | Self::RightThumbProximal
            | Self::RightThumbDistal
            | Self::RightIndexProximal
            | Self::RightIndexIntermediate
            | Self::RightIndexDistal
            | Self::RightMiddleProximal
            | Self::RightMiddleIntermediate
            | Self::RightMiddleDistal
            | Self::RightRingProximal
            | Self::RightRingIntermediate
            | Self::RightRingDistal
            | Self::RightLittleProximal
            | Self::RightLittleIntermediate
            | Self::RightLittleDistal => Some(Side::Right),
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
            (Self::LeftThumbMetacarpal | Self::RightThumbMetacarpal, Side::Left) => {
                Self::LeftThumbMetacarpal
            }
            (Self::LeftThumbMetacarpal | Self::RightThumbMetacarpal, Side::Right) => {
                Self::RightThumbMetacarpal
            }
            (Self::LeftThumbProximal | Self::RightThumbProximal, Side::Left) => {
                Self::LeftThumbProximal
            }
            (Self::LeftThumbProximal | Self::RightThumbProximal, Side::Right) => {
                Self::RightThumbProximal
            }
            (Self::LeftThumbDistal | Self::RightThumbDistal, Side::Left) => Self::LeftThumbDistal,
            (Self::LeftThumbDistal | Self::RightThumbDistal, Side::Right) => Self::RightThumbDistal,
            (Self::LeftIndexProximal | Self::RightIndexProximal, Side::Left) => {
                Self::LeftIndexProximal
            }
            (Self::LeftIndexProximal | Self::RightIndexProximal, Side::Right) => {
                Self::RightIndexProximal
            }
            (Self::LeftIndexIntermediate | Self::RightIndexIntermediate, Side::Left) => {
                Self::LeftIndexIntermediate
            }
            (Self::LeftIndexIntermediate | Self::RightIndexIntermediate, Side::Right) => {
                Self::RightIndexIntermediate
            }
            (Self::LeftIndexDistal | Self::RightIndexDistal, Side::Left) => Self::LeftIndexDistal,
            (Self::LeftIndexDistal | Self::RightIndexDistal, Side::Right) => Self::RightIndexDistal,
            (Self::LeftMiddleProximal | Self::RightMiddleProximal, Side::Left) => {
                Self::LeftMiddleProximal
            }
            (Self::LeftMiddleProximal | Self::RightMiddleProximal, Side::Right) => {
                Self::RightMiddleProximal
            }
            (Self::LeftMiddleIntermediate | Self::RightMiddleIntermediate, Side::Left) => {
                Self::LeftMiddleIntermediate
            }
            (Self::LeftMiddleIntermediate | Self::RightMiddleIntermediate, Side::Right) => {
                Self::RightMiddleIntermediate
            }
            (Self::LeftMiddleDistal | Self::RightMiddleDistal, Side::Left) => {
                Self::LeftMiddleDistal
            }
            (Self::LeftMiddleDistal | Self::RightMiddleDistal, Side::Right) => {
                Self::RightMiddleDistal
            }
            (Self::LeftRingProximal | Self::RightRingProximal, Side::Left) => {
                Self::LeftRingProximal
            }
            (Self::LeftRingProximal | Self::RightRingProximal, Side::Right) => {
                Self::RightRingProximal
            }
            (Self::LeftRingIntermediate | Self::RightRingIntermediate, Side::Left) => {
                Self::LeftRingIntermediate
            }
            (Self::LeftRingIntermediate | Self::RightRingIntermediate, Side::Right) => {
                Self::RightRingIntermediate
            }
            (Self::LeftRingDistal | Self::RightRingDistal, Side::Left) => Self::LeftRingDistal,
            (Self::LeftRingDistal | Self::RightRingDistal, Side::Right) => Self::RightRingDistal,
            (Self::LeftLittleProximal | Self::RightLittleProximal, Side::Left) => {
                Self::LeftLittleProximal
            }
            (Self::LeftLittleProximal | Self::RightLittleProximal, Side::Right) => {
                Self::RightLittleProximal
            }
            (Self::LeftLittleIntermediate | Self::RightLittleIntermediate, Side::Left) => {
                Self::LeftLittleIntermediate
            }
            (Self::LeftLittleIntermediate | Self::RightLittleIntermediate, Side::Right) => {
                Self::RightLittleIntermediate
            }
            (Self::LeftLittleDistal | Self::RightLittleDistal, Side::Left) => {
                Self::LeftLittleDistal
            }
            (Self::LeftLittleDistal | Self::RightLittleDistal, Side::Right) => {
                Self::RightLittleDistal
            }
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
            Self::LeftThumbMetacarpal
            | Self::RightThumbMetacarpal
            | Self::LeftIndexProximal
            | Self::RightIndexProximal
            | Self::LeftMiddleProximal
            | Self::RightMiddleProximal
            | Self::LeftRingProximal
            | Self::RightRingProximal
            | Self::LeftLittleProximal
            | Self::RightLittleProximal => &[Self::LeftHand],
            Self::LeftThumbProximal | Self::RightThumbProximal => &[Self::LeftThumbMetacarpal],
            Self::LeftThumbDistal | Self::RightThumbDistal => &[Self::LeftThumbProximal],
            Self::LeftIndexIntermediate | Self::RightIndexIntermediate => {
                &[Self::LeftIndexProximal]
            }
            Self::LeftIndexDistal | Self::RightIndexDistal => &[Self::LeftIndexIntermediate],
            Self::LeftMiddleIntermediate | Self::RightMiddleIntermediate => {
                &[Self::LeftMiddleProximal]
            }
            Self::LeftMiddleDistal | Self::RightMiddleDistal => &[Self::LeftMiddleIntermediate],
            Self::LeftRingIntermediate | Self::RightRingIntermediate => &[Self::LeftRingProximal],
            Self::LeftRingDistal | Self::RightRingDistal => &[Self::LeftRingIntermediate],
            Self::LeftLittleIntermediate | Self::RightLittleIntermediate => {
                &[Self::LeftLittleProximal]
            }
            Self::LeftLittleDistal | Self::RightLittleDistal => &[Self::LeftLittleIntermediate],
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
    /// 指の番号別名（`thumb1` など）と `pinky` 系は小指として扱う。
    /// 親指は3本鎖であり、最初の骨（例: `Thumb Proximal`）は中手骨に当たる。
    /// そのため `thumbproximal`・`thumbintermediate` は1つずつずらして
    /// `ThumbMetacarpal`・`ThumbProximal` に対応付ける。
    /// 明示的な `thumbmetacarpal` 名と `thumbproximal` 名が併存する場合は
    /// 同じ骨への重複として失敗する。
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
        // 親指は3本鎖のため、最初の骨は中手骨に当たる。
        // Unityの `Thumb Proximal` はVRMの `thumbMetacarpal`、
        // `Thumb Intermediate` は `thumbProximal` に対応する。
        // 明示的な `thumbmetacarpal` 名と `thumbproximal` 名の併存は
        // 同じ骨への重複として `detect` が失敗する。
        (Some(Side::Left), "thumbmetacarpal" | "thumbproximal" | "thumb1") => {
            HumanoidBone::LeftThumbMetacarpal
        }
        (Some(Side::Right), "thumbmetacarpal" | "thumbproximal" | "thumb1") => {
            HumanoidBone::RightThumbMetacarpal
        }
        (Some(Side::Left), "thumbintermediate" | "thumb2") => HumanoidBone::LeftThumbProximal,
        (Some(Side::Right), "thumbintermediate" | "thumb2") => HumanoidBone::RightThumbProximal,
        (Some(Side::Left), "thumbdistal" | "thumb3") => HumanoidBone::LeftThumbDistal,
        (Some(Side::Right), "thumbdistal" | "thumb3") => HumanoidBone::RightThumbDistal,
        (Some(Side::Left), "indexproximal" | "index1") => HumanoidBone::LeftIndexProximal,
        (Some(Side::Right), "indexproximal" | "index1") => HumanoidBone::RightIndexProximal,
        (Some(Side::Left), "indexintermediate" | "index2") => HumanoidBone::LeftIndexIntermediate,
        (Some(Side::Right), "indexintermediate" | "index2") => HumanoidBone::RightIndexIntermediate,
        (Some(Side::Left), "indexdistal" | "index3") => HumanoidBone::LeftIndexDistal,
        (Some(Side::Right), "indexdistal" | "index3") => HumanoidBone::RightIndexDistal,
        (Some(Side::Left), "middleproximal" | "middle1") => HumanoidBone::LeftMiddleProximal,
        (Some(Side::Right), "middleproximal" | "middle1") => HumanoidBone::RightMiddleProximal,
        (Some(Side::Left), "middleintermediate" | "middle2") => {
            HumanoidBone::LeftMiddleIntermediate
        }
        (Some(Side::Right), "middleintermediate" | "middle2") => {
            HumanoidBone::RightMiddleIntermediate
        }
        (Some(Side::Left), "middledistal" | "middle3") => HumanoidBone::LeftMiddleDistal,
        (Some(Side::Right), "middledistal" | "middle3") => HumanoidBone::RightMiddleDistal,
        (Some(Side::Left), "ringproximal" | "ring1") => HumanoidBone::LeftRingProximal,
        (Some(Side::Right), "ringproximal" | "ring1") => HumanoidBone::RightRingProximal,
        (Some(Side::Left), "ringintermediate" | "ring2") => HumanoidBone::LeftRingIntermediate,
        (Some(Side::Right), "ringintermediate" | "ring2") => HumanoidBone::RightRingIntermediate,
        (Some(Side::Left), "ringdistal" | "ring3") => HumanoidBone::LeftRingDistal,
        (Some(Side::Right), "ringdistal" | "ring3") => HumanoidBone::RightRingDistal,
        (Some(Side::Left), "littleproximal" | "little1" | "pinkyproximal" | "pinky1") => {
            HumanoidBone::LeftLittleProximal
        }
        (Some(Side::Right), "littleproximal" | "little1" | "pinkyproximal" | "pinky1") => {
            HumanoidBone::RightLittleProximal
        }
        (Some(Side::Left), "littleintermediate" | "little2" | "pinkyintermediate" | "pinky2") => {
            HumanoidBone::LeftLittleIntermediate
        }
        (Some(Side::Right), "littleintermediate" | "little2" | "pinkyintermediate" | "pinky2") => {
            HumanoidBone::RightLittleIntermediate
        }
        (Some(Side::Left), "littledistal" | "little3" | "pinkydistal" | "pinky3") => {
            HumanoidBone::LeftLittleDistal
        }
        (Some(Side::Right), "littledistal" | "little3" | "pinkydistal" | "pinky3") => {
            HumanoidBone::RightLittleDistal
        }
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
            | "thumbmetacarpal"
            | "thumbproximal"
            | "thumbintermediate"
            | "thumbdistal"
            | "thumb1"
            | "thumb2"
            | "thumb3"
            | "indexproximal"
            | "indexintermediate"
            | "indexdistal"
            | "index1"
            | "index2"
            | "index3"
            | "middleproximal"
            | "middleintermediate"
            | "middledistal"
            | "middle1"
            | "middle2"
            | "middle3"
            | "ringproximal"
            | "ringintermediate"
            | "ringdistal"
            | "ring1"
            | "ring2"
            | "ring3"
            | "littleproximal"
            | "littleintermediate"
            | "littledistal"
            | "little1"
            | "little2"
            | "little3"
            | "pinkyproximal"
            | "pinkyintermediate"
            | "pinkydistal"
            | "pinky1"
            | "pinky2"
            | "pinky3"
    )
}

/// VRM 1.0のhumanoid骨キー55件。
///
/// `VRMC_vrm.humanoid.humanBones` の書式が定めるキー集合と一致し、
/// 親指は `thumbMetacarpal`・`thumbProximal`・`thumbDistal` の3本である
///（`thumbIntermediate` は存在しない）。
/// `jaw` は書式に含まれるが対応付ける手段が無いため列挙子を持たない。
/// 書き出し時はこの一覧に無いキーを拒否し、将来の列挙子追加の取り違えを防ぐ。
pub const HUMANOID_BONE_NAMES: [&str; 55] = [
    "chest",
    "head",
    "hips",
    "jaw",
    "leftEye",
    "leftFoot",
    "leftHand",
    "leftIndexDistal",
    "leftIndexIntermediate",
    "leftIndexProximal",
    "leftLittleDistal",
    "leftLittleIntermediate",
    "leftLittleProximal",
    "leftLowerArm",
    "leftLowerLeg",
    "leftMiddleDistal",
    "leftMiddleIntermediate",
    "leftMiddleProximal",
    "leftRingDistal",
    "leftRingIntermediate",
    "leftRingProximal",
    "leftShoulder",
    "leftThumbDistal",
    "leftThumbMetacarpal",
    "leftThumbProximal",
    "leftToes",
    "leftUpperArm",
    "leftUpperLeg",
    "neck",
    "rightEye",
    "rightFoot",
    "rightHand",
    "rightIndexDistal",
    "rightIndexIntermediate",
    "rightIndexProximal",
    "rightLittleDistal",
    "rightLittleIntermediate",
    "rightLittleProximal",
    "rightLowerArm",
    "rightLowerLeg",
    "rightMiddleDistal",
    "rightMiddleIntermediate",
    "rightMiddleProximal",
    "rightRingDistal",
    "rightRingIntermediate",
    "rightRingProximal",
    "rightShoulder",
    "rightThumbDistal",
    "rightThumbMetacarpal",
    "rightThumbProximal",
    "rightToes",
    "rightUpperArm",
    "rightUpperLeg",
    "spine",
    "upperChest",
];

/// VRM 1.0のhumanoid骨キーとして有効かどうかを返す。
///
/// 書き出し時の検査に使い、未知のキーは受け付けない。
pub fn is_known_humanoid_bone_name(name: &str) -> bool {
    HUMANOID_BONE_NAMES.contains(&name)
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    // テスト名は日本語で書く（テストは仕様・契約であるため）。

    #[test]
    fn 骨名の一覧は55件で重複が無い() {
        // 書式のキー集合との対応を数で固定する。
        assert_eq!(HUMANOID_BONE_NAMES.len(), 55, "骨名は55件である");
        let 重複なし: HashSet<&str> = HUMANOID_BONE_NAMES.into_iter().collect();
        assert_eq!(重複なし.len(), 55, "骨名に重複は無い");
    }

    #[test]
    fn 全列挙子のキー名が一覧にある() {
        // 将来の列挙子追加の取り違え（旧 `thumbIntermediate` など）を残さない。
        for 骨 in HumanoidBone::all() {
            assert!(
                is_known_humanoid_bone_name(骨.name()),
                "列挙子のキー名は一覧にある: {}",
                骨.name()
            );
        }
    }

    #[test]
    fn 親指は中手骨を含む3本で中間骨は無い() {
        // VRM 1.0に `thumbIntermediate` は存在しない。
        assert!(is_known_humanoid_bone_name("leftThumbMetacarpal"));
        assert!(is_known_humanoid_bone_name("rightThumbMetacarpal"));
        assert!(!is_known_humanoid_bone_name("leftThumbIntermediate"));
        assert!(!is_known_humanoid_bone_name("rightThumbIntermediate"));
        // 書式に含まれる顎は対応手段が無いため列挙子を持たないが一覧にはある。
        assert!(is_known_humanoid_bone_name("jaw"));
    }

    #[test]
    fn 親指の別名は1つずらして対応する() {
        // 3本鎖の最初の骨（`Thumb Proximal`）は中手骨に当たる。
        assert_eq!(
            match_bone("Thumb Proximal.L"),
            Some(HumanoidBone::LeftThumbMetacarpal)
        );
        assert_eq!(
            match_bone("Thumb Intermediate.L"),
            Some(HumanoidBone::LeftThumbProximal)
        );
        assert_eq!(
            match_bone("Thumb Distal.L"),
            Some(HumanoidBone::LeftThumbDistal)
        );
        assert_eq!(
            match_bone("thumb1.R"),
            Some(HumanoidBone::RightThumbMetacarpal)
        );
        assert_eq!(
            match_bone("thumbmetacarpal.R"),
            Some(HumanoidBone::RightThumbMetacarpal)
        );
    }
}
