//! VRChat SDKのMobile系シェーダ（`VRChat/Mobile/*`）をMToonへ写す。
//!
//! シェーダ本体はVRChat SDK（`com.vrchat.base` の `Sample Assets/Shaders/Mobile`）にあり、
//! アバターのUnityPackageには普通含まれない。そのため判定は `.shader.meta` の
//! GUID表を先に使い、索引に `.shader` がある場合だけシェーダ名でも判定する。
//! GUIDはSDK 3.0.9から3.11.0まで変わっていないことを確かめてある。
//!
//! 写像は各シェーダの計算に合わせる。
//!
//! - Toon Lit・MatCap Lit は光の向きを使わない（`albedo × (直接光 + 間接光)`）ため、
//!   影色を基準色と同じにして陰影を付けない。MatCap Lit の乗算MatCapは
//!   内接円の平均色を基準色に掛けて近似する。
//! - Diffuse・Bumped Diffuse・Bumped Mapped Specular・Standard Lite はLambertであり、
//!   影色を黒、境界を `shift = -0.5`・`toony = 0.5` として `max(0, N·L)` に合わせる。
//! - Particles は半透明・両面で描く。Additive は半透明で近似し、Multiply は扱わない。
//! - Toon Standard は影のRampをGUID表（SDK付属の7種）か索引の画像から当てはめ、
//!   法線・MatCap・リムなどの機能は材質のシェーダキーワードで有効かどうかを判定する。
//!
//! 頂点色の乗算はメッシュ側の値であり、どのシェーダでも写像しない
//! （未設定のメッシュでは白であり差が出ない）。

use super::*;

/// Mobile系シェーダの種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MobileShader {
    /// `VRChat/Mobile/Toon Lit`。
    ToonLit,
    /// `VRChat/Mobile/MatCap Lit`。
    MatCapLit,
    /// `VRChat/Mobile/Diffuse`。
    Diffuse,
    /// `VRChat/Mobile/Bumped Diffuse`。
    BumpedDiffuse,
    /// `VRChat/Mobile/Bumped Mapped Specular`（同名の2ファイル）。
    BumpedSpecular,
    /// `VRChat/Mobile/Standard Lite`。
    StandardLite,
    /// `VRChat/Mobile/Particles/Alpha Blended`。
    ParticleAlpha,
    /// `VRChat/Mobile/Particles/Additive`。
    ParticleAdditive,
    /// `VRChat/Mobile/Particles/Multiply`。乗算合成は表せないため扱わない。
    ParticleMultiply,
    /// `VRChat/Mobile/Toon Standard`。
    ToonStandard,
    /// `VRChat/Mobile/Toon Standard (Outline)`。
    ToonStandardOutline,
    /// ワールド用（Skybox・Lightmapped・Supersampled UI）。アバターでは扱わない。
    WorldOnly,
}

/// Mobile系シェーダの `.shader.meta` のGUID表（小文字）。
///
/// VRChat SDK 3.0.9と3.11.0-beta.1の `com.vrchat.base` で一致を確かめた。
/// Toon StandardはSDK 3.8以降にだけある。
const MOBILE_SHADER_GUIDS: [(&str, MobileShader); 15] = [
    ("affc81f3d164d734d8f13053effb1c5c", MobileShader::ToonLit),
    ("3ad043b7f9839cb48a75a9238d433dec", MobileShader::MatCapLit),
    ("2dcd9e0568e0a6f45b92c60ba2eb16a0", MobileShader::Diffuse),
    (
        "f8c1f8ac363df824899534a0b30eef00",
        MobileShader::BumpedDiffuse,
    ),
    (
        "528d55c4e8adab14b974ca665ed1b996",
        MobileShader::BumpedSpecular,
    ),
    (
        "584dc70fbb9834e48beb29e3206e3ca0",
        MobileShader::BumpedSpecular,
    ),
    (
        "0b7113dea2069fc4e8943843eff19f70",
        MobileShader::StandardLite,
    ),
    (
        "8b39b95ac85682040beff730e0cfc77a",
        MobileShader::ParticleAlpha,
    ),
    (
        "9200bec112b65ec4fbbbd33fa89c20f4",
        MobileShader::ParticleAdditive,
    ),
    (
        "d5b89f0c74ccf5049ba803c14a090378",
        MobileShader::ParticleMultiply,
    ),
    (
        "e765db0afa7ecfc44ade2e4e2491f65a",
        MobileShader::ToonStandard,
    ),
    (
        "051a0ed2f2aedd741aa8186ae92f97e0",
        MobileShader::ToonStandardOutline,
    ),
    ("c0d3cb006bb294142bef136f492f2568", MobileShader::WorldOnly),
    ("b1f7ecc80417c414b9d62ce541d5bcbf", MobileShader::WorldOnly),
    ("44c1b7c3827912f45b473dbe106c1ecf", MobileShader::WorldOnly),
];

/// シェーダ名からMobile系シェーダの種類を求める。Mobile系でなければ `None` である。
fn mobile_of_name(name: &str) -> Option<MobileShader> {
    let kind = match name {
        "VRChat/Mobile/Toon Lit" => MobileShader::ToonLit,
        "VRChat/Mobile/MatCap Lit" => MobileShader::MatCapLit,
        "VRChat/Mobile/Diffuse" => MobileShader::Diffuse,
        "VRChat/Mobile/Bumped Diffuse" => MobileShader::BumpedDiffuse,
        "VRChat/Mobile/Bumped Mapped Specular" => MobileShader::BumpedSpecular,
        "VRChat/Mobile/Standard Lite" => MobileShader::StandardLite,
        "VRChat/Mobile/Particles/Alpha Blended" => MobileShader::ParticleAlpha,
        "VRChat/Mobile/Particles/Additive" => MobileShader::ParticleAdditive,
        "VRChat/Mobile/Particles/Multiply" => MobileShader::ParticleMultiply,
        "VRChat/Mobile/Toon Standard" => MobileShader::ToonStandard,
        "VRChat/Mobile/Toon Standard (Outline)" => MobileShader::ToonStandardOutline,
        other if other.starts_with("VRChat/Mobile/") => MobileShader::WorldOnly,
        _ => return None,
    };
    Some(kind)
}

/// Toon Standardの影Rampの標本。
struct RampProfile {
    /// テクスチャのGUID（小文字）。
    guid: &'static str,
    /// 横方向の値（0〜255、線形）。等間隔の標本である。
    samples: &'static [u8],
    /// 点標本化（`filterMode: 0`）かどうか。真なら段の境界で値が切り替わる。
    point: bool,
}

/// SDK付属の影Ramp（`ToonStandard/Resources/VRChat/ShadowRamp*.png`）の標本。
///
/// いずれも `sRGBTexture: 0` の灰色である。256幅の画像は8テクセルおきの32標本、
/// 8幅以下の画像は全テクセルを持つ。Flatは1x1の白である。
const BUILTIN_RAMPS: [RampProfile; 7] = [
    RampProfile {
        guid: "8ed41581528c4fa4fa11970aca4edb8d",
        samples: &[255],
        point: true,
    },
    RampProfile {
        guid: "348500adef1d2da428abc7b720b8b699",
        samples: &[
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 11, 56, 114, 136, 147, 158, 168, 179, 188,
            199, 207, 216, 224, 233, 239, 245, 251,
        ],
        point: false,
    },
    RampProfile {
        guid: "636cf1b5dfca6f54b94ca3d2ff8216c9",
        samples: &[
            0, 0, 0, 0, 0, 0, 0, 0, 3, 11, 20, 32, 44, 58, 72, 87, 102, 119, 132, 143, 155, 165,
            176, 186, 196, 206, 215, 223, 232, 238, 245, 250,
        ],
        point: false,
    },
    RampProfile {
        guid: "5f304bf7a07313d43b8562d9eabce646",
        samples: &[
            0, 0, 0, 0, 0, 4, 10, 17, 25, 33, 42, 51, 61, 71, 82, 93, 104, 116, 128, 139, 150, 162,
            173, 183, 193, 204, 213, 222, 230, 238, 245, 251,
        ],
        point: false,
    },
    RampProfile {
        guid: "dfafc89321615114fb6dbecdba0c8214",
        samples: &[0, 0, 0, 0, 255, 255, 255, 255],
        point: true,
    },
    RampProfile {
        guid: "5d1b50be612cf1248b6e101f8d1c5b53",
        samples: &[0, 0, 0, 128, 128, 255, 255, 255],
        point: true,
    },
    RampProfile {
        guid: "0d6a7a9ec31ab7448a777f0e2daef4af",
        samples: &[0, 0, 64, 64, 191, 191, 255, 255],
        point: true,
    },
];

/// 影Rampを当てはめたMToonの陰影。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct RampFit {
    /// MToonの `shadingShiftFactor`。
    pub(super) shift: f32,
    /// MToonの `shadingToonyFactor`。
    pub(super) toony: f32,
    /// 影側の明るさ（Rampの最小値）。
    pub(super) dark: f32,
    /// 明側の明るさ（Rampの最大値）。
    pub(super) light: f32,
}

/// Toony係数の上限。境界が硬い場合も0除算にならないよう、Flat Lit Toonと揃える。
const MAX_TOONY: f32 = 0.95;

/// 影Rampの標本からMToonの陰影を当てはめる。
///
/// Rampの横軸はToon Standardの `N·L × 0.5 + 0.5` である。
/// 最小値から最大値への遷移の10%・90%の位置から線形区間を求め、
/// MToonの `linearstep(-1 + toony, 1 - toony, N·L + shift)` に合わせる。
/// 区間 `[u0, u1]` に対して `shift = 1 - (u0 + u1)`、`toony = 1 - (u1 - u0)` である。
/// 値が一定の場合は遷移が無く、`dark == light` を返す。
pub(super) fn fit_ramp(samples: &[f32], point: bool) -> RampFit {
    let dark = samples.iter().copied().fold(f32::INFINITY, f32::min);
    let light = samples.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    // 非数を含む標本も遷移なしとして扱うため、`>` の否定で判定する。
    let has_transition = light - dark > 1e-3;
    if samples.is_empty() || !has_transition {
        let value = if samples.is_empty() { 1.0 } else { light };
        return RampFit {
            shift: 0.0,
            toony: MAX_TOONY,
            dark: value,
            light: value,
        };
    }
    let count = samples.len() as f32;
    let crossing = |fraction: f32| -> f32 {
        let level = dark + (light - dark) * fraction;
        if point {
            // 点標本化は段の境界（i / n）で切り替わる。
            let index = samples
                .iter()
                .position(|&value| value >= level)
                .unwrap_or(samples.len() - 1);
            return index as f32 / count;
        }
        // 双線形は標本の中心（(i + 0.5) / n）の間を線形に補う。
        for index in 1..samples.len() {
            let (before, after) = (samples[index - 1], samples[index]);
            if after >= level && before < level {
                let t = (level - before) / (after - before);
                return (index as f32 - 0.5 + t) / count;
            }
        }
        if samples[0] >= level { 0.0 } else { 1.0 }
    };
    let (u10, u90) = (crossing(0.1), crossing(0.9));
    // 線形のRampは10%〜90%から区間全体へ伸ばし、段のRampは境界の間をそのまま使う。
    let (u0, u1) = if point {
        (u10, u90)
    } else {
        let width = (u90 - u10) / 0.8;
        (u10 - width * 0.1, u90 + width * 0.1)
    };
    RampFit {
        shift: (1.0 - (u0 + u1)).clamp(-1.0, 1.0),
        toony: (1.0 - (u1 - u0)).clamp(0.0, MAX_TOONY),
        dark,
        light,
    }
}

/// 材質がシェーダキーワードを持つかどうかを返す。
fn has_keyword(material: &crate::unity::Material, keyword: &str) -> bool {
    material
        .shader_keywords
        .iter()
        .any(|known| known == keyword)
}

/// `_Color` などのsRGB色を線形のRGBと元のアルファにする。
fn linear_rgba(value: [f64; 4]) -> [f32; 4] {
    [
        srgb_to_linear(value[0]),
        srgb_to_linear(value[1]),
        srgb_to_linear(value[2]),
        clamp01(value[3]) as f32,
    ]
}

/// 色の各成分に係数を掛ける。
fn scale3(value: [f32; 3], factor: [f32; 3]) -> [f32; 3] {
    [
        value[0] * factor[0],
        value[1] * factor[1],
        value[2] * factor[2],
    ]
}

/// `lerp(a, b, t)` を成分ごとに求める。
fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

/// 光の当たり方。
#[derive(Debug, Clone, Copy, PartialEq)]
enum MobileShading {
    /// 光の向きを使わない（影色を基準色と同じにする）。
    Flat,
    /// Lambert（影色を黒、`max(0, N·L)` に合わせる）。
    Lambert,
}

impl LoadContext<'_> {
    /// シェーダ参照がMobile系シェーダならその種類を返す。
    ///
    /// GUID表を先に引き、無ければ索引の `.shader` の先頭のシェーダ名で判定する。
    pub(super) fn mobile_shader(
        &mut self,
        material: &crate::unity::Material,
        slot: &str,
    ) -> Option<MobileShader> {
        if material.shader.is_null() {
            return None;
        }
        let guid = material.shader.guid.as_ref()?;
        if let Some((_, kind)) = MOBILE_SHADER_GUIDS
            .iter()
            .find(|(known, _)| *known == guid.as_str())
        {
            return Some(*kind);
        }
        let record = self.index.get(guid)?;
        if record.kind != AssetKind::Shader {
            return None;
        }
        let path = record.path.clone();
        match self.source.read_limited(&path, MAX_SHADER_BYTES) {
            Ok(bytes) => std::str::from_utf8(&bytes)
                .ok()
                .and_then(first_shader_name)
                .and_then(|name| mobile_of_name(&name)),
            Err(_) => {
                self.warn(format!("シェーダ名を読めないためGUID表で判定する: {slot}"));
                None
            }
        }
    }

    /// Mobile系シェーダの材質を写像し、実効描画順と組で返す。
    ///
    /// 扱わない種類（乗算パーティクル・ワールド用）と不正値は警告して灰色材質にする。
    pub(super) fn resolve_mobile(
        &mut self,
        material: &crate::unity::Material,
        slot: &str,
        kind: MobileShader,
    ) -> Result<(VrmMaterial, i32), MaterialError> {
        let series = match kind {
            MobileShader::ParticleAlpha
            | MobileShader::ParticleAdditive
            | MobileShader::ParticleMultiply => ShaderSeries::Blend,
            _ => ShaderSeries::Opaque,
        };
        let mapped = match kind {
            MobileShader::ParticleMultiply => {
                self.warn(format!(
                    "乗算合成のMobileパーティクルは表せないため灰色材質にする: {slot}: {}",
                    material.name
                ));
                return Ok((placeholder(slot), effective_queue(material, None)));
            }
            MobileShader::WorldOnly => {
                self.warn(format!(
                    "ワールド用のMobileシェーダは扱わないため灰色材質にする: {slot}: {}",
                    material.name
                ));
                return Ok((placeholder(slot), effective_queue(material, None)));
            }
            MobileShader::ToonStandard => self.map_toon_standard(material, slot, false)?,
            MobileShader::ToonStandardOutline => self.map_toon_standard(material, slot, true)?,
            MobileShader::ParticleAlpha | MobileShader::ParticleAdditive => {
                self.map_mobile_particle(material, slot, kind)?
            }
            _ => self.map_mobile_legacy(material, slot, kind)?,
        };
        let queue = effective_queue(material, Some(series));
        if let Err(field) = validate_material(&mapped) {
            self.warn(format!("材質値が不正なため灰色材質にする: {slot}: {field}"));
            return Ok((placeholder(slot), queue));
        }
        Ok((mapped, queue))
    }

    /// Toon Lit・MatCap Lit・Diffuse・Bumped系・Standard LiteをMToonへ写す。
    fn map_mobile_legacy(
        &mut self,
        material: &crate::unity::Material,
        slot: &str,
        kind: MobileShader,
    ) -> Result<VrmMaterial, MaterialError> {
        let shading = match kind {
            MobileShader::ToonLit | MobileShader::MatCapLit => MobileShading::Flat,
            _ => MobileShading::Lambert,
        };
        // Standard Liteだけが `_Color` を持つ。他は `_MainTex` だけである。
        let tint = if kind == MobileShader::StandardLite {
            linear_rgba(color(material, "_Color").unwrap_or([1.0, 1.0, 1.0, 1.0]))
        } else {
            [1.0, 1.0, 1.0, 1.0]
        };
        let mut base_rgb = [tint[0], tint[1], tint[2]];
        let base_color_image = self.resolve_texture(material, slot, "_MainTex")?;

        if kind == MobileShader::MatCapLit {
            // `albedo × matcap` の乗算。視線で変わる値は表せないため、
            // 球が見せる範囲（内接円）の平均色を基準色に掛ける。
            if let Some(input) =
                self.bake_input_of(material, slot, "_MatCap", BakeChannel::Rgb, false)?
            {
                let average = matcap_disc_average(&input.image, &BakeSt::identity());
                base_rgb = scale3(base_rgb, average);
                self.warn(format!(
                    "乗算MatCapは平均色を基準色に掛けて近似する: {slot}"
                ));
            }
        }

        let (normal_image, normal_scale) = match kind {
            MobileShader::BumpedDiffuse | MobileShader::BumpedSpecular => {
                (self.resolve_texture(material, slot, "_BumpMap")?, 1.0)
            }
            MobileShader::StandardLite => (
                self.resolve_texture(material, slot, "_BumpMap")?,
                number(material, "_BumpScale").unwrap_or(1.0) as f32,
            ),
            _ => (None, 1.0),
        };
        if kind == MobileShader::BumpedSpecular {
            self.warn(format!("鏡面反射は写像しない: {slot}"));
        }

        let (emissive_factor, emissive_image) =
            if kind == MobileShader::StandardLite && has_keyword(material, "_EMISSION") {
                let (factor, clamped) = emission_linear_or(material, [0.0, 0.0, 0.0]);
                if clamped {
                    self.warn(format!("1を超える発光色は1に丸める: {slot}"));
                }
                let image = if factor == [0.0, 0.0, 0.0] {
                    None
                } else {
                    self.resolve_texture(material, slot, "_EmissionMap")?
                };
                (factor, image)
            } else {
                ([0.0, 0.0, 0.0], None)
            };
        if kind == MobileShader::StandardLite {
            if has_texture(material, "_MetallicGlossMap") || has_texture(material, "_OcclusionMap")
            {
                self.warn(format!(
                    "金属度・平滑度・遮蔽のテクスチャは写像しない: {slot}"
                ));
            }
            if has_keyword(material, "_DETAIL") {
                self.warn(format!("ディテールマップは写像しない: {slot}"));
            }
        }

        let (shade_color_factor, shading_shift_factor, shading_toony_factor) = match shading {
            MobileShading::Flat => (base_rgb, 0.0, 0.9),
            // `linearstep(-0.5, 0.5, N·L - 0.5)` は `saturate(N·L)` に等しい。
            MobileShading::Lambert => ([0.0, 0.0, 0.0], -0.5, 0.5),
        };
        Ok(VrmMaterial {
            name: material.name.clone(),
            alpha_mode: VrmAlphaMode::Opaque,
            alpha_cutoff: 0.5,
            double_sided: false,
            base_color_factor: [base_rgb[0], base_rgb[1], base_rgb[2], tint[3]],
            base_color_image,
            emissive_factor,
            emissive_image,
            normal_image,
            normal_scale,
            mtoon: Some(VrmMtoonParameters {
                shade_color_factor,
                shade_multiply_image: base_color_image,
                shading_shift_factor,
                shading_toony_factor,
                ..VrmMtoonParameters::default()
            }),
        })
    }

    /// Mobileパーティクル（Alpha Blended・Additive）をMToonへ写す。
    ///
    /// 照明を受けない描画のため、影色を基準色と同じにして陰影を付けない。
    fn map_mobile_particle(
        &mut self,
        material: &crate::unity::Material,
        slot: &str,
        kind: MobileShader,
    ) -> Result<VrmMaterial, MaterialError> {
        if kind == MobileShader::ParticleAdditive {
            self.warn(format!("加算合成は半透明で近似する: {slot}"));
        }
        self.warn(format!(
            "照明を受けないパーティクルは陰影なしで近似する: {slot}"
        ));
        let base_color_image = self.resolve_texture(material, slot, "_MainTex")?;
        Ok(VrmMaterial {
            name: material.name.clone(),
            alpha_mode: VrmAlphaMode::Blend,
            alpha_cutoff: 0.5,
            // `Cull Off` で描く。
            double_sided: true,
            base_color_factor: [1.0, 1.0, 1.0, 1.0],
            base_color_image,
            emissive_factor: [0.0, 0.0, 0.0],
            emissive_image: None,
            normal_image: None,
            normal_scale: 1.0,
            mtoon: Some(VrmMtoonParameters {
                // `ZWrite Off` で描く。
                transparent_with_z_write: false,
                shade_color_factor: [1.0, 1.0, 1.0],
                shade_multiply_image: base_color_image,
                ..VrmMtoonParameters::default()
            }),
        })
    }

    /// Toon Standardの影Rampを当てはめる。
    ///
    /// 未割り当てはシェーダ既定の白（陰影なし）、SDK付属のRampはGUID表、
    /// それ以外は索引の画像の中央の行から当てはめる。読めない場合は
    /// 2段のRampとして扱い警告する。
    fn toon_standard_ramp(
        &mut self,
        material: &crate::unity::Material,
        slot: &str,
    ) -> Result<RampFit, MaterialError> {
        let Some(environment) = texture(material, "_Ramp") else {
            return Ok(fit_ramp(&[1.0], true));
        };
        if environment.texture.is_null() {
            return Ok(fit_ramp(&[1.0], true));
        }
        if let Some(guid) = environment.texture.guid.as_ref()
            && let Some(profile) = BUILTIN_RAMPS
                .iter()
                .find(|profile| profile.guid == guid.as_str())
        {
            let samples: Vec<f32> = profile
                .samples
                .iter()
                .map(|&value| f32::from(value) / 255.0)
                .collect();
            return Ok(fit_ramp(&samples, profile.point));
        }
        let fallback = fit_ramp(&[0.0, 1.0], true);
        let Some(input) = self.bake_input_of(material, slot, "_Ramp", BakeChannel::Rgb, false)?
        else {
            self.warn(format!("影Rampを読めないため2段の陰影で近似する: {slot}"));
            return Ok(fallback);
        };
        let image = &input.image;
        let (width, height) = (image.width() as usize, image.height() as usize);
        let row = height / 2;
        let pixels = image.pixels();
        let samples: Vec<f32> = (0..width)
            .map(|x| {
                let at = (row * width + x) * 4;
                let mut sum = 0.0f32;
                for channel in 0..3 {
                    let raw = f64::from(pixels[at + channel]) / 255.0;
                    sum += if image.srgb() {
                        srgb_to_linear(raw)
                    } else {
                        raw as f32
                    };
                }
                sum / 3.0
            })
            .collect();
        self.warn(format!("影Rampは明暗の境界と最小値で近似する: {slot}"));
        Ok(fit_ramp(&samples, false))
    }

    /// Toon StandardをMToonへ写す。`outline` は `(Outline)` 変種かどうかである。
    ///
    /// 写像は `ToonStandard/CG/Helpers.cginc` の `GetDiffuseBRDFBase` に合わせる。
    /// 影側は `albedo × lerp(albedo × _ShadowAlbedo, 1, min(1, ramp + _ShadowBoost))`
    /// の明るさであり、MToonの影色は基準色にこの係数（テクスチャを除く）を掛けた色にする。
    fn map_toon_standard(
        &mut self,
        material: &crate::unity::Material,
        slot: &str,
        outline: bool,
    ) -> Result<VrmMaterial, MaterialError> {
        let base = linear_rgba(color(material, "_Color").unwrap_or([1.0, 1.0, 1.0, 1.0]));
        let mut base_rgb = [base[0], base[1], base[2]];
        let base_color_image = self.resolve_texture(material, slot, "_MainTex")?;

        let culling = number(material, "_Culling").unwrap_or(2.0);
        let double_sided = culling != 2.0;
        if culling == 1.0 {
            self.warn(format!("前面カリングは両面で近似する: {slot}"));
        }
        if number(material, "_VertexColor").unwrap_or(0.0) != 0.0 {
            self.warn(format!("頂点色の乗算は写像しない: {slot}"));
        }

        // 乗算MatCapは基準色と影色の両方に掛かるため、影の計算の前に畳み込む。
        let use_matcap = has_keyword(material, "USE_MATCAP");
        let matcap_type = number(material, "_MatcapType").unwrap_or(0.0) as i64;
        let matcap_strength = clamp01(number(material, "_MatcapStrength").unwrap_or(1.0)) as f32;
        if use_matcap && has_texture(material, "_MatcapMask") {
            self.warn(format!("MatCapのマスクは写像しない: {slot}"));
        }
        if use_matcap
            && matcap_type == 1
            && let Some(input) =
                self.bake_input_of(material, slot, "_Matcap", BakeChannel::Rgb, false)?
        {
            let average = matcap_disc_average(&input.image, &BakeSt::identity());
            let factor = lerp3([1.0, 1.0, 1.0], average, matcap_strength);
            base_rgb = scale3(base_rgb, factor);
            self.warn(format!(
                "乗算MatCapは平均色を基準色に掛けて近似する: {slot}"
            ));
        }

        let ramp = self.toon_standard_ramp(material, slot)?;
        let boost = clamp01(number(material, "_ShadowBoost").unwrap_or(0.0)) as f32;
        let shadow_albedo = clamp01(number(material, "_ShadowAlbedo").unwrap_or(0.5)) as f32;
        let brightness = |value: f32| (value + boost).min(1.0);
        let contribution = |value: f32| {
            lerp3(
                scale3(base_rgb, [shadow_albedo; 3]),
                [1.0; 3],
                brightness(value),
            )
        };
        let lit = scale3(base_rgb, contribution(ramp.light));
        let shade = scale3(base_rgb, contribution(ramp.dark));
        if base_color_image.is_some() && shadow_albedo > 0.0 && brightness(ramp.dark) < 1.0 {
            self.warn(format!(
                "影色のアルベド掛けはテクスチャを含めず色だけで近似する: {slot}"
            ));
        }
        if number(material, "_MinBrightness").unwrap_or(0.0) > 0.0 {
            self.warn(format!("最低明るさは写像しない: {slot}"));
        }

        let (normal_image, normal_scale) = if has_keyword(material, "USE_NORMAL_MAPS") {
            (
                self.resolve_texture(material, slot, "_BumpMap")?,
                number(material, "_BumpScale").unwrap_or(1.0) as f32,
            )
        } else {
            (None, 1.0)
        };

        // 発光は常に有効であり、`_EmissionMap × _EmissionColor × _EmissionStrength` である。
        let emission = color(material, "_EmissionColor").unwrap_or([0.0, 0.0, 0.0, 1.0]);
        let strength = number(material, "_EmissionStrength")
            .unwrap_or(1.0)
            .max(0.0);
        let mut emissive_factor = [0.0f32; 3];
        let mut clamped = false;
        for (out, value) in emissive_factor.iter_mut().zip(emission.iter()) {
            let linear = srgb_to_linear_raw(*value) * strength;
            if linear > 1.0 {
                clamped = true;
            }
            *out = clamp01(linear) as f32;
        }
        if clamped {
            self.warn(format!("1を超える発光色は1に丸める: {slot}"));
        }
        let emissive_image = if emissive_factor == [0.0, 0.0, 0.0] {
            None
        } else {
            if number(material, "_EmissionUV").unwrap_or(0.0) != 0.0 {
                self.warn(format!("発光のUV1指定は反映しない: {slot}"));
            }
            self.resolve_texture(material, slot, "_EmissionMap")?
        };

        // 加算MatCapはMToonのMatCapへ載せる。Toon Standardは照明前に足すため近似である。
        let (matcap_factor, matcap_image) = if use_matcap && matcap_type == 0 {
            match self.resolve_texture(material, slot, "_Matcap")? {
                Some(image) => ([matcap_strength; 3], Some(image)),
                None => ([0.0; 3], None),
            }
        } else {
            ([0.0; 3], None)
        };

        let mut rim_color = [0.0f32; 3];
        let mut rim_power = 5.0f32;
        let mut rim_mix = 1.0f32;
        if has_keyword(material, "USE_RIMLIGHT") {
            let color_value = linear_rgba(color(material, "_RimColor").unwrap_or([1.0; 4]));
            let intensity = clamp01(number(material, "_RimIntensity").unwrap_or(0.5)) as f32;
            rim_color = scale3(
                [color_value[0], color_value[1], color_value[2]],
                [intensity; 3],
            );
            // `smoothstep(R - S, R + S, 1 - |N·V|)` の中点 `R = 1 - _RimRange` で
            // `pow(1 - N·V, p)` が0.5になるよう `p` を選ぶ。
            let range = clamp01(number(material, "_RimRange").unwrap_or(0.3)) as f32;
            let midpoint = (1.0 - range).clamp(0.01, 0.99);
            rim_power = (0.5f32.ln() / midpoint.ln()).clamp(0.01, 100.0);
            rim_mix = if number(material, "_RimEnvironmental").unwrap_or(0.0) != 0.0 {
                1.0
            } else {
                0.0
            };
            if number(material, "_RimAlbedoTint").unwrap_or(0.0) > 0.0 {
                self.warn(format!("リムのアルベド色付けは写像しない: {slot}"));
            }
            self.warn(format!(
                "リムの範囲と柔らかさはフレネル指数で近似する: {slot}"
            ));
        }

        for (keyword, feature) in [
            ("USE_SPECULAR", "鏡面反射"),
            ("USE_OCCLUSION_MAP", "遮蔽マップ"),
            ("USE_DETAIL_MAPS", "ディテールマップ"),
            ("USE_HUE_SHIFT", "色相シフト"),
            ("USE_COLOR_MASK", "カラーマスク"),
            ("USE_AUDIOLINK", "AudioLink"),
        ] {
            if has_keyword(material, keyword) {
                self.warn(format!("{feature}は写像しない: {slot}"));
            }
        }

        let mut outline_width_mode = VrmOutlineWidthMode::None;
        let mut outline_width_factor = 0.0f32;
        let mut outline_width_multiply_image = None;
        let mut outline_color_factor = [0.0f32; 3];
        let mut outline_lighting_mix_factor = 1.0f32;
        let thickness = number(material, "_OutlineThickness")
            .unwrap_or(0.05)
            .max(0.0);
        if outline && thickness > 0.0 {
            // 頂点を世界座標の法線方向へ `thickness × 0.01` ずらす。
            // 画面上の太さの上限による補正は再現しない。
            outline_width_mode = VrmOutlineWidthMode::WorldCoordinates;
            outline_width_factor = (thickness * 0.01) as f32;
            let line =
                linear_rgba(color(material, "_OutlineColor").unwrap_or([0.0, 0.0, 0.0, 1.0]));
            let from_albedo = clamp01(number(material, "_OutlineFromAlbedo").unwrap_or(0.0)) as f32;
            // 線色は照明を受けない。基準色から取る割合は照明混合で近似する。
            outline_color_factor = lerp3([line[0], line[1], line[2]], [1.0; 3], from_albedo);
            outline_lighting_mix_factor = from_albedo;
            if from_albedo > 0.0 {
                self.warn(format!(
                    "基準色から取るアウトライン色は照明を受ける色で近似する: {slot}"
                ));
            }
            if has_texture(material, "_OutlineMask") {
                // MToonの幅乗算テクスチャは緑チャンネルを読む。
                if number(material, "_OutlineMaskChannel").unwrap_or(0.0) as i64 == 1 {
                    outline_width_multiply_image =
                        self.resolve_texture(material, slot, "_OutlineMask")?;
                } else {
                    self.warn(format!(
                        "緑以外のチャンネルを読むアウトラインのマスクは写像しない: {slot}"
                    ));
                }
            }
        }

        Ok(VrmMaterial {
            name: material.name.clone(),
            alpha_mode: VrmAlphaMode::Opaque,
            alpha_cutoff: 0.5,
            double_sided,
            base_color_factor: [lit[0], lit[1], lit[2], base[3]],
            base_color_image,
            emissive_factor,
            emissive_image,
            normal_image,
            normal_scale,
            mtoon: Some(VrmMtoonParameters {
                shade_color_factor: shade,
                shade_multiply_image: base_color_image,
                shading_shift_factor: ramp.shift,
                shading_toony_factor: ramp.toony,
                matcap_factor,
                matcap_image,
                outline_width_mode,
                outline_width_factor,
                outline_width_multiply_image,
                outline_color_factor,
                outline_lighting_mix_factor,
                parametric_rim_color_factor: rim_color,
                parametric_rim_fresnel_power_factor: rim_power,
                rim_lighting_mix_factor: rim_mix,
                ..VrmMtoonParameters::default()
            }),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 当てはめた区間 `[u0, u1]` を `shift`・`toony` から戻す。
    fn interval(fit: RampFit) -> (f32, f32) {
        let width = 1.0 - fit.toony;
        let sum = 1.0 - fit.shift;
        ((sum - width) / 2.0, (sum + width) / 2.0)
    }

    #[test]
    fn 一定のrampは遷移なしになる() {
        let fit = fit_ramp(&[1.0], true);
        assert_eq!(fit.dark, 1.0);
        assert_eq!(fit.light, 1.0);
    }

    #[test]
    fn 二段のrampは中央の硬い境界になる() {
        let fit = fit_ramp(&[0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0], true);
        assert!(fit.shift.abs() < 1e-6, "{fit:?}");
        assert_eq!(fit.toony, MAX_TOONY);
        assert_eq!((fit.dark, fit.light), (0.0, 1.0));
    }

    #[test]
    fn 三段のrampは中段の範囲を区間にする() {
        let fit = fit_ramp(&[0.0, 0.0, 0.0, 0.5, 0.5, 1.0, 1.0, 1.0], true);
        let (u0, u1) = interval(fit);
        assert!((u0 - 3.0 / 8.0).abs() < 1e-5, "{fit:?}");
        assert!((u1 - 5.0 / 8.0).abs() < 1e-5, "{fit:?}");
    }

    #[test]
    fn 線形のrampは区間全体へ伸ばす() {
        // u = 0.25〜0.75で0から1へ線形に上がる標本。
        let samples: Vec<f32> = (0..64)
            .map(|index| {
                let u = (index as f32 + 0.5) / 64.0;
                ((u - 0.25) / 0.5).clamp(0.0, 1.0)
            })
            .collect();
        let (u0, u1) = interval(fit_ramp(&samples, false));
        assert!((u0 - 0.25).abs() < 0.01, "{u0}");
        assert!((u1 - 0.75).abs() < 0.01, "{u1}");
    }

    #[test]
    fn 付属rampはいずれも範囲内に収まる() {
        for profile in &BUILTIN_RAMPS {
            let samples: Vec<f32> = profile
                .samples
                .iter()
                .map(|&value| f32::from(value) / 255.0)
                .collect();
            let fit = fit_ramp(&samples, profile.point);
            assert!(
                (-1.0..=1.0).contains(&fit.shift),
                "{}: {fit:?}",
                profile.guid
            );
            assert!(
                (0.0..=MAX_TOONY).contains(&fit.toony),
                "{}: {fit:?}",
                profile.guid
            );
        }
    }

    #[test]
    fn シェーダ名で種類を引ける() {
        assert_eq!(
            mobile_of_name("VRChat/Mobile/Toon Lit"),
            Some(MobileShader::ToonLit)
        );
        assert_eq!(
            mobile_of_name("VRChat/Mobile/Skybox"),
            Some(MobileShader::WorldOnly)
        );
        assert_eq!(mobile_of_name("lilToon"), None);
    }
}
