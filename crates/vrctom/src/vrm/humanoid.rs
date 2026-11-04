//! VRM humanoidの骨対応付けと検証。
//!
//! 入口は [`HumanoidBones::detect`] であり、ノード名の正規化で骨へ対応付ける。
//! 呼び出し側は [`HumanoidBones`] を明示指定して名前推測を使わないこともできる。
//! 親子・拡大率の検証は書き出し時に行い、ここでは対応付けだけを担う。

use std::collections::{BTreeMap, BTreeSet, HashMap};

use super::VrmModel;
use super::export::VrmExportError;
use crate::unity::HumanBoneMapping;

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

/// Unityのhumanoid骨名からVRMの骨への変換表。
///
/// Unity名（`HumanBodyBones`、空白なし）の順序は物理sidecarの既存出力
/// （`build_humanoid_json` の旧 `ORDER`）と同じであり、順序・内容を変えない。
/// 親指はUnityとVRMで意味がずれるため、必ず変換する
/// （`ThumbProximal`→中手骨・`ThumbIntermediate`→基節骨・`ThumbDistal`→末節骨）。
/// `Jaw` はUnity名として保持するがVRMの [`HumanoidBone`] には無いため
/// 対応先を `None` とし、通常の書き出しでは警告して省く。
/// 物理sidecarは `Jaw` を含む全名を扱える。
pub(crate) const UNITY_HUMANOID_BONE_TABLE: &[(&str, Option<HumanoidBone>)] = &[
    ("Hips", Some(HumanoidBone::Hips)),
    ("LeftUpperLeg", Some(HumanoidBone::LeftUpperLeg)),
    ("RightUpperLeg", Some(HumanoidBone::RightUpperLeg)),
    ("LeftLowerLeg", Some(HumanoidBone::LeftLowerLeg)),
    ("RightLowerLeg", Some(HumanoidBone::RightLowerLeg)),
    ("LeftFoot", Some(HumanoidBone::LeftFoot)),
    ("RightFoot", Some(HumanoidBone::RightFoot)),
    ("Spine", Some(HumanoidBone::Spine)),
    ("Chest", Some(HumanoidBone::Chest)),
    ("UpperChest", Some(HumanoidBone::UpperChest)),
    ("Neck", Some(HumanoidBone::Neck)),
    ("Head", Some(HumanoidBone::Head)),
    ("LeftShoulder", Some(HumanoidBone::LeftShoulder)),
    ("RightShoulder", Some(HumanoidBone::RightShoulder)),
    ("LeftUpperArm", Some(HumanoidBone::LeftUpperArm)),
    ("RightUpperArm", Some(HumanoidBone::RightUpperArm)),
    ("LeftLowerArm", Some(HumanoidBone::LeftLowerArm)),
    ("RightLowerArm", Some(HumanoidBone::RightLowerArm)),
    ("LeftHand", Some(HumanoidBone::LeftHand)),
    ("RightHand", Some(HumanoidBone::RightHand)),
    ("LeftToes", Some(HumanoidBone::LeftToes)),
    ("RightToes", Some(HumanoidBone::RightToes)),
    ("LeftEye", Some(HumanoidBone::LeftEye)),
    ("RightEye", Some(HumanoidBone::RightEye)),
    ("Jaw", None),
    ("LeftThumbProximal", Some(HumanoidBone::LeftThumbMetacarpal)),
    (
        "LeftThumbIntermediate",
        Some(HumanoidBone::LeftThumbProximal),
    ),
    ("LeftThumbDistal", Some(HumanoidBone::LeftThumbDistal)),
    ("LeftIndexProximal", Some(HumanoidBone::LeftIndexProximal)),
    (
        "LeftIndexIntermediate",
        Some(HumanoidBone::LeftIndexIntermediate),
    ),
    ("LeftIndexDistal", Some(HumanoidBone::LeftIndexDistal)),
    ("LeftMiddleProximal", Some(HumanoidBone::LeftMiddleProximal)),
    (
        "LeftMiddleIntermediate",
        Some(HumanoidBone::LeftMiddleIntermediate),
    ),
    ("LeftMiddleDistal", Some(HumanoidBone::LeftMiddleDistal)),
    ("LeftRingProximal", Some(HumanoidBone::LeftRingProximal)),
    (
        "LeftRingIntermediate",
        Some(HumanoidBone::LeftRingIntermediate),
    ),
    ("LeftRingDistal", Some(HumanoidBone::LeftRingDistal)),
    ("LeftLittleProximal", Some(HumanoidBone::LeftLittleProximal)),
    (
        "LeftLittleIntermediate",
        Some(HumanoidBone::LeftLittleIntermediate),
    ),
    ("LeftLittleDistal", Some(HumanoidBone::LeftLittleDistal)),
    (
        "RightThumbProximal",
        Some(HumanoidBone::RightThumbMetacarpal),
    ),
    (
        "RightThumbIntermediate",
        Some(HumanoidBone::RightThumbProximal),
    ),
    ("RightThumbDistal", Some(HumanoidBone::RightThumbDistal)),
    ("RightIndexProximal", Some(HumanoidBone::RightIndexProximal)),
    (
        "RightIndexIntermediate",
        Some(HumanoidBone::RightIndexIntermediate),
    ),
    ("RightIndexDistal", Some(HumanoidBone::RightIndexDistal)),
    (
        "RightMiddleProximal",
        Some(HumanoidBone::RightMiddleProximal),
    ),
    (
        "RightMiddleIntermediate",
        Some(HumanoidBone::RightMiddleIntermediate),
    ),
    ("RightMiddleDistal", Some(HumanoidBone::RightMiddleDistal)),
    ("RightRingProximal", Some(HumanoidBone::RightRingProximal)),
    (
        "RightRingIntermediate",
        Some(HumanoidBone::RightRingIntermediate),
    ),
    ("RightRingDistal", Some(HumanoidBone::RightRingDistal)),
    (
        "RightLittleProximal",
        Some(HumanoidBone::RightLittleProximal),
    ),
    (
        "RightLittleIntermediate",
        Some(HumanoidBone::RightLittleIntermediate),
    ),
    ("RightLittleDistal", Some(HumanoidBone::RightLittleDistal)),
];

/// Unityの骨名を照合用に正規化する。ASCII空白だけを除去する。
///
/// `Left Shoulder` のような表記を `LeftShoulder` に寄せる。
/// ノード名（`boneName`）には使わない。完全一致で突き合わせる。
pub(crate) fn normalize_unity_bone_name(name: &str) -> String {
    name.chars().filter(|c| !c.is_ascii_whitespace()).collect()
}

/// 正規化済みのUnity骨名からVRMの骨を求める。
///
/// 外側の `None` は未知・未対応のUnity名、
/// 内側の `None` はVRMに対応が無い既知名（`Jaw`）である。
pub(crate) fn unity_bone_to_vrm(normalized: &str) -> Option<Option<HumanoidBone>> {
    UNITY_HUMANOID_BONE_TABLE
        .iter()
        .find(|(name, _)| *name == normalized)
        .map(|(_, bone)| *bone)
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
    /// `mixamorig:Hips` のような名前空間接頭辞（`:` より前）は取り除く。
    /// 側は末尾の区切り付き `L`/`R`、または先頭の `left`/`right` で判定する。
    /// 指の番号別名（`thumb1` など）と `pinky` 系は小指として扱う。
    /// 親指は3本鎖であり、最初の骨（例: `Thumb Proximal`）は中手骨に当たる。
    /// そのため `thumbproximal`・`thumbintermediate` は1つずつずらして
    /// `ThumbMetacarpal`・`ThumbProximal` に対応付ける。
    /// 明示的な `thumbmetacarpal` 名と `thumbproximal` 名が併存する場合は
    /// 同じ骨への重複として失敗する。
    ///
    /// 対応付けは2パスで行う。1パス目は正確名だけを対応付け、重複は失敗する。
    /// 2パス目はMixamo由来の別名（`arm`・`forearm`・`upleg`・`leg`・`toebase`）を、
    /// 正確名で埋まらなかった枠にだけ適用する。正確名で埋まった枠への別名は
    /// 無視し、正確名が勝つ。同じ枠に別名が複数当たる場合は推測せず、
    /// 重複として失敗する（先勝ちにしない）。
    ///
    /// # Errors
    ///
    /// 同じ骨に複数ノードが一致した場合・必須骨が無い場合に返す。
    pub fn detect(model: &VrmModel) -> Result<Self, VrmExportError> {
        Ok(Self::resolve(model, &[])?.0)
    }

    /// Unityの対応表とノード名から対応付ける。
    ///
    /// 対応表（[`.fbx.meta` の `humanDescription.human`](crate::unity::AssetMeta::human_bone_mappings) を
    /// 想定）は利用者の明示指定（[`VrmExportOptions::with_humanoid`](super::export::VrmExportOptions::with_humanoid)）より弱く、
    /// 名前推測より強い。手順はメタの解決・確定、メタ確定骨のロック、
    /// メタ使用済みノードの除外、未充足骨の正確名補完、
    /// さらに未充足骨の別名補完、必須骨の存在検査の順である。
    ///
    /// `humanName` はASCII空白を除去して既知のUnity骨名に照合する。
    /// `boneName` はモデルのノード名と完全一致で突き合わせ、
    /// 大文字小文字の折り畳み・部分一致・区切り除去はしない。
    /// 同一対応の反復は重複排除して許容する。
    /// 同一Unity骨に異なる `boneName`・同一ノードを別の骨への割当て・
    /// 対応先ノードの欠落や重複は先勝ち・後勝ちにせず失敗する。
    /// 未知・未対応の `humanName` とVRMに対応が無い既知名（`Jaw`）は
    /// 警告して省く。空の対応表は [`detect`](Self::detect) と同じ名前推測になる。
    ///
    /// 戻り値は対応付けと警告文の組である。警告の順序は対応表の順序に従う。
    /// `skeleton`（T-pose）は使わない。通常の書き出しのrest姿勢は変えない。
    ///
    /// # Errors
    ///
    /// 対応先ノードの欠落や重複・同じ骨や同じノードへの複数対応付け・
    /// 名前推測の重複・必須骨の欠落の場合に返す。
    pub fn detect_with_mapping(
        model: &VrmModel,
        mappings: &[HumanBoneMapping],
    ) -> Result<(Self, Vec<String>), VrmExportError> {
        Self::resolve(model, mappings)
    }

    /// 対応表と名前推測で対応付ける共通処理。
    ///
    /// 空の対応表では名前推測だけになり、[`detect`](Self::detect) と同じ結果になる。
    /// ロック済みの骨に対する名前候補は複数あっても無視し、
    /// メタの確定を名前の曖昧さで失敗させない。
    /// 未充足の同一骨に同順位の候補が複数ある場合は推測せず失敗する。
    fn resolve(
        model: &VrmModel,
        mappings: &[HumanBoneMapping],
    ) -> Result<(Self, Vec<String>), VrmExportError> {
        let mut warnings = Vec::new();
        let mut warned: BTreeSet<String> = BTreeSet::new();
        // ノード名から索引を引く。完全一致で突き合わせる。
        let mut by_name: HashMap<&str, Vec<usize>> = HashMap::new();
        for (index, node) in model.nodes.iter().enumerate() {
            by_name.entry(node.name.as_str()).or_default().push(index);
        }
        let mut bones = Self::new();
        let mut used: BTreeSet<usize> = BTreeSet::new();
        // メタの有効な対応を解決・確定する。
        for mapping in mappings {
            let normalized = normalize_unity_bone_name(&mapping.human_name);
            let Some(mapped) = unity_bone_to_vrm(&normalized) else {
                // 未知・未対応の `humanName` は警告して無視する。
                if warned.insert(format!("unknown:{normalized}")) {
                    warnings.push(format!(
                        "未知のhumanNameのため対応を省く: {}",
                        mapping.human_name
                    ));
                }
                continue;
            };
            let Some(bone) = mapped else {
                // VRMに対応が無い既知名（`Jaw`）は警告して省く。
                if warned.insert(format!("unsupported:{normalized}")) {
                    warnings.push(format!(
                        "VRMのhumanoidに対応が無いため省く: {}",
                        mapping.human_name
                    ));
                }
                continue;
            };
            let candidates = by_name
                .get(mapping.bone_name.as_str())
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            let &[node] = candidates else {
                if candidates.is_empty() {
                    return Err(VrmExportError::missing_bone(
                        bone,
                        format!(
                            "metaの対応先ノードが無い: {} -> {}",
                            mapping.human_name, mapping.bone_name
                        ),
                    ));
                }
                return Err(VrmExportError::duplicate_bone(
                    bone,
                    candidates[0],
                    format!("metaの対応先ノードが重複する: {}", mapping.bone_name),
                ));
            };
            if let Some(existing) = bones.get(bone) {
                // 同一対応の反復は重複排除して許容する。
                if existing == node {
                    continue;
                }
                return Err(VrmExportError::duplicate_bone(
                    bone,
                    node,
                    format!(
                        "metaで同じ骨に複数のノードが対応する: {}",
                        mapping.human_name
                    ),
                ));
            }
            if used.contains(&node) {
                return Err(VrmExportError::duplicate_bone(
                    bone,
                    node,
                    format!(
                        "metaで同じノードに複数の骨が対応する: {}",
                        mapping.bone_name
                    ),
                ));
            }
            bones.set(bone, node);
            used.insert(node);
        }
        // メタで確定した骨をロックし、メタで使用済みのノードを名前検出から除外する。
        let locked: BTreeSet<HumanoidBone> = bones.iter().map(|(bone, _)| bone).collect();
        // 未充足の骨だけ正確名で補完する。
        let mut consumed = used;
        for (index, node) in model.nodes.iter().enumerate() {
            if consumed.contains(&index) {
                continue;
            }
            let Some(bone) = match_bone(&node.name) else {
                continue;
            };
            if locked.contains(&bone) {
                consumed.insert(index);
                continue;
            }
            if bones.get(bone).is_some() {
                return Err(VrmExportError::duplicate_bone(
                    bone,
                    index,
                    "同じ骨に複数のノードが一致した",
                ));
            }
            bones.set(bone, index);
            consumed.insert(index);
        }
        // さらに未充足の骨だけ別名で補完する。
        let mut alias_filled: BTreeSet<HumanoidBone> = BTreeSet::new();
        for (index, node) in model.nodes.iter().enumerate() {
            if consumed.contains(&index) {
                continue;
            }
            let Some(bone) = match_alias_bone(&node.name) else {
                continue;
            };
            if locked.contains(&bone) {
                continue;
            }
            if bones.get(bone).is_some() {
                if alias_filled.contains(&bone) {
                    return Err(VrmExportError::duplicate_bone(
                        bone,
                        index,
                        "同じ骨に複数の別名ノードが一致した",
                    ));
                }
                // 正確名で埋まった枠への別名は無視し、正確名が勝つ。
                continue;
            }
            bones.set(bone, index);
            alias_filled.insert(bone);
        }
        for bone in HumanoidBone::all() {
            if bone.is_required() && bones.get(bone).is_none() {
                return Err(VrmExportError::missing_bone(bone, "必須のhumanoid骨が無い"));
            }
        }
        Ok((bones, warnings))
    }
}

/// ノード名から正確名の骨を求める。一致しない場合は `None` である。
///
/// 正規化は名前空間接頭辞（`mixamorig:` など）の除去、小文字化、
/// 区切り（空白・`_`・`.`・`-`）の除去であり、
/// 側の接尾辞だけでは中央骨の語尾と区別できないため、接尾辞の候補語幹が
/// 既知の語幹の場合だけ側として扱う。
/// Mixamo由来の別名（`arm` など）は含まず、[`match_alias_bone`] が担う。
/// 正確名と別名の語幹集合は重ならないため、1つの名前が両方に一致しない。
fn match_bone(name: &str) -> Option<HumanoidBone> {
    let normalized = normalize_name(name);
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

/// ノード名を比較用の小文字・区切りなしに正規化する。
///
/// `mixamorig:Hips` のような名前空間接頭辞は `:` より前を取り除く。
/// FBXの名前空間付き名に対応するための措置であり、`:` を骨名の一部に
/// 使う命名は想定しない。`:` を含まない名前はそのまま正規化する。
fn normalize_name(name: &str) -> String {
    let without_namespace = match name.rsplit_once(':') {
        Some((_, tail)) => tail,
        None => name,
    };
    without_namespace
        .to_lowercase()
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '_' && *c != '.' && *c != '-')
        .collect()
}

/// ノード名からMixamo由来の別名の骨を求める。一致しない場合は `None` である。
///
/// 正規化と側の判定は正確名と同じ規則に従う。対応は `arm` が上腕・
/// `forearm` が前腕・`upleg` が大腿・`leg` が下腿・`toebase` が足先である。
/// 短い語幹（特に `leg`）の誤検出を避けるため、呼び出し側（[`HumanoidBones::detect`]）
/// が正確名で埋まらなかった枠にだけ適用し、正確名と別名の語幹集合は
/// 重ならないように保つ。同じ枠に別名が複数当たる場合は呼び出し側が
/// 推測せず失敗する。
fn match_alias_bone(name: &str) -> Option<HumanoidBone> {
    let normalized = normalize_name(name);
    if normalized.is_empty() {
        return None;
    }
    let (side, stem) = split_side(&normalized)?;
    Some(match (side, stem) {
        (Some(Side::Left), "arm") => HumanoidBone::LeftUpperArm,
        (Some(Side::Right), "arm") => HumanoidBone::RightUpperArm,
        (Some(Side::Left), "forearm") => HumanoidBone::LeftLowerArm,
        (Some(Side::Right), "forearm") => HumanoidBone::RightLowerArm,
        (Some(Side::Left), "upleg") => HumanoidBone::LeftUpperLeg,
        (Some(Side::Right), "upleg") => HumanoidBone::RightUpperLeg,
        (Some(Side::Left), "leg") => HumanoidBone::LeftLowerLeg,
        (Some(Side::Right), "leg") => HumanoidBone::RightLowerLeg,
        (Some(Side::Left), "toebase") => HumanoidBone::LeftToes,
        (Some(Side::Right), "toebase") => HumanoidBone::RightToes,
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
///
/// 正確名の語幹に加え、Mixamo由来の別名語幹（`arm`・`forearm`・
/// `upleg`・`leg`・`toebase`）も受け付ける。`Shoulder_L` のような
/// 区切り付き接尾辞の命名に対応するためである。
/// 別名語幹の追加で正確名の対応は変わらない。正確名と別名の語幹集合が
/// 重ならないため、接尾辞の有無で一致先が変わることは無い。
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
            | "arm"
            | "forearm"
            | "upleg"
            | "leg"
            | "toebase"
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
    fn 共通表は55件で物理sidecarの順序を保つ() {
        // 旧 `ORDER`（LastBone除く）と同じ順序・件数である。
        assert_eq!(UNITY_HUMANOID_BONE_TABLE.len(), 55, "共通表は55件である");
        let 名前: Vec<&str> = UNITY_HUMANOID_BONE_TABLE
            .iter()
            .map(|(名前, _)| *名前)
            .collect();
        assert_eq!(
            名前,
            vec![
                "Hips",
                "LeftUpperLeg",
                "RightUpperLeg",
                "LeftLowerLeg",
                "RightLowerLeg",
                "LeftFoot",
                "RightFoot",
                "Spine",
                "Chest",
                "UpperChest",
                "Neck",
                "Head",
                "LeftShoulder",
                "RightShoulder",
                "LeftUpperArm",
                "RightUpperArm",
                "LeftLowerArm",
                "RightLowerArm",
                "LeftHand",
                "RightHand",
                "LeftToes",
                "RightToes",
                "LeftEye",
                "RightEye",
                "Jaw",
                "LeftThumbProximal",
                "LeftThumbIntermediate",
                "LeftThumbDistal",
                "LeftIndexProximal",
                "LeftIndexIntermediate",
                "LeftIndexDistal",
                "LeftMiddleProximal",
                "LeftMiddleIntermediate",
                "LeftMiddleDistal",
                "LeftRingProximal",
                "LeftRingIntermediate",
                "LeftRingDistal",
                "LeftLittleProximal",
                "LeftLittleIntermediate",
                "LeftLittleDistal",
                "RightThumbProximal",
                "RightThumbIntermediate",
                "RightThumbDistal",
                "RightIndexProximal",
                "RightIndexIntermediate",
                "RightIndexDistal",
                "RightMiddleProximal",
                "RightMiddleIntermediate",
                "RightMiddleDistal",
                "RightRingProximal",
                "RightRingIntermediate",
                "RightRingDistal",
                "RightLittleProximal",
                "RightLittleIntermediate",
                "RightLittleDistal",
            ],
            "共通表の順序は既存出力と同じである"
        );
    }

    #[test]
    fn 共通表は親指を変換し顎に対応が無い() {
        // 親指はUnityとVRMで意味がずれるため変換する。
        assert_eq!(
            unity_bone_to_vrm("LeftThumbProximal"),
            Some(Some(HumanoidBone::LeftThumbMetacarpal))
        );
        assert_eq!(
            unity_bone_to_vrm("LeftThumbIntermediate"),
            Some(Some(HumanoidBone::LeftThumbProximal))
        );
        assert_eq!(
            unity_bone_to_vrm("RightThumbProximal"),
            Some(Some(HumanoidBone::RightThumbMetacarpal))
        );
        // 顎はUnity名として保持するがVRMの列挙子は無い。
        assert_eq!(unity_bone_to_vrm("Jaw"), Some(None));
        // 未知の名前は対応付けない。
        assert_eq!(unity_bone_to_vrm("LastBone"), None);
        assert_eq!(unity_bone_to_vrm("FooBone"), None);
        // 照合はASCII空白だけを除去する。ノード名の空白は保つ対象外である。
        assert_eq!(
            normalize_unity_bone_name("Left Thumb Proximal"),
            "LeftThumbProximal"
        );
        assert_eq!(normalize_unity_bone_name("LeftShoulder"), "LeftShoulder");
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

    #[test]
    fn 混合由来の別名は必須骨の枠に対応する() {
        // Mixamo由来の命名（Shaonなど）を正確名なしで対応付ける。
        assert_eq!(
            match_alias_bone("LeftArm"),
            Some(HumanoidBone::LeftUpperArm)
        );
        assert_eq!(
            match_alias_bone("RightArm"),
            Some(HumanoidBone::RightUpperArm)
        );
        assert_eq!(
            match_alias_bone("LeftForeArm"),
            Some(HumanoidBone::LeftLowerArm)
        );
        assert_eq!(
            match_alias_bone("mixamorig:RightForeArm"),
            Some(HumanoidBone::RightLowerArm)
        );
        assert_eq!(
            match_alias_bone("LeftUpLeg"),
            Some(HumanoidBone::LeftUpperLeg)
        );
        assert_eq!(
            match_alias_bone("LeftLeg"),
            Some(HumanoidBone::LeftLowerLeg)
        );
        assert_eq!(
            match_alias_bone("RightLeg"),
            Some(HumanoidBone::RightLowerLeg)
        );
        assert_eq!(
            match_alias_bone("LeftToeBase"),
            Some(HumanoidBone::LeftToes)
        );
        // 区切り付き接尾辞の命名（`Arm_L` など）にも対応する。
        assert_eq!(match_alias_bone("Arm_L"), Some(HumanoidBone::LeftUpperArm));
        assert_eq!(match_alias_bone("ToeBase_R"), Some(HumanoidBone::RightToes));
    }

    #[test]
    fn 別名の語幹は正確名の一致を変えない() {
        // 正確名と別名の語幹集合は重ならない。短い別名語幹（特に `leg`）が
        // 正確名（`upperleg`・`lowerleg`）を奪わない。
        assert_eq!(match_bone("LeftArm"), None);
        assert_eq!(match_bone("LeftUpperArm"), Some(HumanoidBone::LeftUpperArm));
        assert_eq!(match_bone("LeftLeg"), None);
        assert_eq!(match_bone("LeftLowerLeg"), Some(HumanoidBone::LeftLowerLeg));
        assert_eq!(match_bone("LeftToeBase"), None);
        assert_eq!(match_bone("LeftToes"), Some(HumanoidBone::LeftToes));
        // 装飾骨（ツイスト・末端・指の4本鎖）は別名でも対応付けない。
        assert_eq!(match_bone("UpperArm_twist_L"), None);
        assert_eq!(match_alias_bone("UpperArm_twist_L"), None);
        assert_eq!(match_alias_bone("LeftToeBase_end"), None);
        assert_eq!(match_alias_bone("LeftHandThumb1"), None);
        // 他モデルの足先綴り（Quicheの `Bese`）は推測しない。
        assert_eq!(match_bone("Bese_L"), None);
        assert_eq!(match_alias_bone("Bese_L"), None);
    }

    #[test]
    fn 名前空間接頭辞は正確名と別名の両方で取り除かれる() {
        // `mixamorig:Hips` のようなFBXの名前空間付き名に対応する。
        assert_eq!(match_bone("mixamorig:Hips"), Some(HumanoidBone::Hips));
        assert_eq!(
            match_alias_bone("mixamorig:LeftArm"),
            Some(HumanoidBone::LeftUpperArm)
        );
        assert_eq!(
            match_bone("mixamorig:LeftUpperArm"),
            Some(HumanoidBone::LeftUpperArm)
        );
    }
}
