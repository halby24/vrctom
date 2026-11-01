//! 4x4行列の演算。
//!
//! FBXの16値行列は行優先・行ベクトル（右から掛ける）だが、glTFの列優先・
//! 列ベクトル（左から掛ける）と同じ配列になるため、転置せずそのまま扱う。
//! ここでは列ベクトル規約 `(M * v)` で計算し、平行移動は12〜14番に現れる。

/// 4x4単位行列。
///
/// メッシュノードのローカル行列など、変換しない箇所の既定値である。
pub(crate) const IDENTITY: [f64; 16] = [
    1.0, 0.0, 0.0, 0.0, //
    0.0, 1.0, 0.0, 0.0, //
    0.0, 0.0, 1.0, 0.0, //
    0.0, 0.0, 0.0, 1.0, //
];

/// 行列積 `a * b` を返す。列ベクトル規約であり、`b` を先に適用する。
///
/// ローカル行列の合成（`親の逆行列 * 自分の世界行列`）に使う。
pub(crate) fn mul(a: &[f64; 16], b: &[f64; 16]) -> [f64; 16] {
    let mut out = [0.0; 16];
    for row in 0..4 {
        for col in 0..4 {
            let mut sum = 0.0;
            for k in 0..4 {
                sum += a[k * 4 + row] * b[col * 4 + k];
            }
            out[col * 4 + row] = sum;
        }
    }
    out
}

/// 逆行列を返す。特異な場合は `None` を返す。
///
/// 4x8の拡大行列を部分ピボット選択付きのGauss-Jordan法で単位化する。
/// バインド世界行列はアフィンであることを仮定せず、一般の4x4として解く。
pub(crate) fn inverse(m: &[f64; 16]) -> Option<[f64; 16]> {
    let mut a = [[0.0f64; 8]; 4];
    for row in 0..4 {
        for col in 0..4 {
            a[row][col] = m[col * 4 + row];
        }
        a[row][4 + row] = 1.0;
    }
    for col in 0..4 {
        let mut pivot = col;
        for row in col..4 {
            if a[row][col].abs() > a[pivot][col].abs() {
                pivot = row;
            }
        }
        if a[pivot][col] == 0.0 {
            return None;
        }
        a.swap(col, pivot);
        let diag = a[col][col];
        for value in a[col].iter_mut() {
            *value /= diag;
        }
        let pivot_row = a[col];
        for (row, target) in a.iter_mut().enumerate() {
            if row != col {
                let factor = target[col];
                for (value, pivot) in target.iter_mut().zip(pivot_row.iter()) {
                    *value -= factor * pivot;
                }
            }
        }
    }
    let mut out = [0.0; 16];
    for row in 0..4 {
        for col in 0..4 {
            out[col * 4 + row] = a[row][4 + col];
        }
    }
    Some(out)
}

/// 点を同次座標で変換する。`w` が0または非有限の場合は `None` を返す。
pub(crate) fn transform_point(m: &[f64; 16], p: [f64; 3]) -> Option<[f64; 3]> {
    let w = m[3] * p[0] + m[7] * p[1] + m[11] * p[2] + m[15];
    if w == 0.0 || !w.is_finite() {
        return None;
    }
    let out = [
        (m[0] * p[0] + m[4] * p[1] + m[8] * p[2] + m[12]) / w,
        (m[1] * p[0] + m[5] * p[1] + m[9] * p[2] + m[13]) / w,
        (m[2] * p[0] + m[6] * p[1] + m[10] * p[2] + m[14]) / w,
    ];
    if out.iter().all(|v| v.is_finite()) {
        Some(out)
    } else {
        None
    }
}

/// ベクトルを3x3部分の線形変換だけで変換する。平行移動は掛けない。
///
/// モーフターゲットの差分は位置の差であり、平行移動の影響を受けないため、
/// 点変換ではなく線形部分だけを使う。法線変換の逆転置は使わない。
/// 非一様拡大では差分の向きと大きさが変わるが、これは正しい挙動である。
/// 入力が非有限、変換結果が非有限の場合は `None` を返す。
pub(crate) fn transform_vector(m: &[f64; 16], v: [f64; 3]) -> Option<[f64; 3]> {
    if !v.iter().all(|v| v.is_finite()) {
        return None;
    }
    // 列ベクトル規約の3x3部分だけを掛ける。平行移動（12〜14番）は使わない。
    let out = [
        m[0] * v[0] + m[4] * v[1] + m[8] * v[2],
        m[1] * v[0] + m[5] * v[1] + m[9] * v[2],
        m[2] * v[0] + m[6] * v[1] + m[10] * v[2],
    ];
    if out.iter().all(|v| v.is_finite()) {
        Some(out)
    } else {
        None
    }
}

/// 法線を3x3部分の逆転置で変換し、正規化して返す。
///
/// 入力が非有限、3x3部分が特異、変換結果の長さが0の場合は `None` を返す。
pub(crate) fn transform_normal(m: &[f64; 16], n: [f64; 3]) -> Option<[f64; 3]> {
    if !n.iter().all(|v| v.is_finite()) {
        return None;
    }
    let inv = inverse3(m)?;
    // 逆転置 `N = inv^T` を掛ける。列優先の逆行列 `inv` に対して
    // `N[r][c] = inv[c][r]` であり、平坦化すると `N` の `(row, col)` 成分は
    // `inv[row * 3 + col]` に現れる。
    let mut out = [0.0; 3];
    for row in 0..3 {
        out[row] = inv[row * 3] * n[0] + inv[row * 3 + 1] * n[1] + inv[row * 3 + 2] * n[2];
    }
    let len = (out[0] * out[0] + out[1] * out[1] + out[2] * out[2]).sqrt();
    if len == 0.0 || !len.is_finite() {
        return None;
    }
    Some([out[0] / len, out[1] / len, out[2] / len])
}

/// 3x3部分の逆行列を返す。行列式が0の場合は `None` を返す。
fn inverse3(m: &[f64; 16]) -> Option<[f64; 9]> {
    let (a, b, c) = (m[0], m[1], m[2]);
    let (d, e, f) = (m[4], m[5], m[6]);
    let (g, h, i) = (m[8], m[9], m[10]);
    let det = a * (e * i - f * h) - d * (b * i - c * h) + g * (b * f - c * e);
    if det == 0.0 || !det.is_finite() {
        return None;
    }
    // 余因子行列の転置を行列式で割る。列優先で並べる。
    Some([
        (e * i - f * h) / det,
        (c * h - b * i) / det,
        (b * f - c * e) / det,
        (f * g - d * i) / det,
        (a * i - c * g) / det,
        (c * d - a * f) / det,
        (d * h - e * g) / det,
        (b * g - a * h) / det,
        (a * e - b * d) / det,
    ])
}

/// アフィン行列のw行を `[0, 0, 0, 1]` に寄せる。
///
/// 逆行列の丸めで生じる微小残差を消す。読み取り層の行列はアフィンであることが
/// 前提であり、glTFのノード行列と逆バインド行列は正確な `[0, 0, 0, 1]` を要求する
/// （Khronos Validatorの `NODE_MATRIX_NON_TRS` と `ACCESSOR_INVALID_IBM`）。
/// いずれかの残差が許容値（1e-9）を超える場合は何もせず、
/// 非アフィンな入力は呼び出し側の検査（Validatorを含む）に譲る。
/// 残差が既に0の場合は値を変えない。
pub(crate) fn snap_affine_row(m: &mut [f64; 16]) {
    const TOLERANCE: f64 = 1e-9;
    if m[3].abs() <= TOLERANCE
        && m[7].abs() <= TOLERANCE
        && m[11].abs() <= TOLERANCE
        && (m[15] - 1.0).abs() <= TOLERANCE
    {
        m[3] = 0.0;
        m[7] = 0.0;
        m[11] = 0.0;
        m[15] = 1.0;
    }
}

/// 3x3部分の行列式を返す。巻き方向（鏡像）の判定に使う。
pub(crate) fn det3(m: &[f64; 16]) -> f64 {
    let (a, b, c) = (m[0], m[1], m[2]);
    let (d, e, f) = (m[4], m[5], m[6]);
    let (g, h, i) = (m[8], m[9], m[10]);
    a * (e * i - f * h) - d * (b * i - c * h) + g * (b * f - c * e)
}

/// 一様拡大の行列を返す。単位換算のシーンルートに使う。
pub(crate) fn scale_matrix(s: f64) -> [f64; 16] {
    [
        s, 0.0, 0.0, 0.0, //
        0.0, s, 0.0, 0.0, //
        0.0, 0.0, s, 0.0, //
        0.0, 0.0, 0.0, 1.0, //
    ]
}

/// 平行移動の行列を返す。FBXのオフセットとピボットに使う。
pub(crate) fn translation_matrix(t: [f64; 3]) -> [f64; 16] {
    [
        1.0, 0.0, 0.0, 0.0, //
        0.0, 1.0, 0.0, 0.0, //
        0.0, 0.0, 1.0, 0.0, //
        t[0], t[1], t[2], 1.0, //
    ]
}

/// 非一様拡大縮小の行列を返す。FBXの `Scaling` 系に使う。
pub(crate) fn non_uniform_scale_matrix(s: [f64; 3]) -> [f64; 16] {
    [
        s[0], 0.0, 0.0, 0.0, //
        0.0, s[1], 0.0, 0.0, //
        0.0, 0.0, s[2], 0.0, //
        0.0, 0.0, 0.0, 1.0, //
    ]
}

/// 単軸の回転行列を作る。列ベクトル規約の右手系回転である。
fn axis_rotation_matrix(axis: usize, radians: f64) -> [f64; 16] {
    let (sin, cos) = radians.sin_cos();
    let mut out = IDENTITY;
    let (a, b) = match axis {
        0 => (1, 2),
        1 => (0, 2),
        _ => (0, 1),
    };
    // 符号は `transform_normal` の回転試験（X+90度で `y` が `z` に向く）と一致させる。
    let sign = if axis == 1 { -1.0 } else { 1.0 };
    out[a * 4 + a] = cos;
    out[b * 4 + b] = cos;
    out[a * 4 + b] = sign * sin;
    out[b * 4 + a] = -sign * sin;
    out
}

/// FBXのXYZ順のオイラー角（度）から回転行列を作る。`R = Rz * Ry * Rx` である。
///
/// FBX SDKは行ベクトル規約であり、`RotationOrder` の0（XYZ順）は
/// X・Y・Zの順に掛ける（`v * Rx * Ry * Rz`）。列ベクトル規約の本層では
/// 掛ける順序が逆になり、`Rz * Ry * Rx` と等しくなる。
/// 購入アバターの実ファイル（PreRotationを持つモデル）のバインド行列との
/// 突き合わせで確認した。回転が無い場合は単位行列になる。
pub(crate) fn fbx_euler_xyz_matrix(degrees: [f64; 3]) -> [f64; 16] {
    let factor = std::f64::consts::PI / 180.0;
    let rx = axis_rotation_matrix(0, degrees[0] * factor);
    let ry = axis_rotation_matrix(1, degrees[1] * factor);
    let rz = axis_rotation_matrix(2, degrees[2] * factor);
    mul(&mul(&rz, &ry), &rx)
}

/// 行列の全成分が有限かどうかを返す。
pub(crate) fn is_finite_matrix(m: &[f64; 16]) -> bool {
    m.iter().all(|v| v.is_finite())
}

/// アフィン行列を平行移動・回転・拡大縮小に分ける。
///
/// 回転は3列の正規化で求めるため、せん断を含む行列には使えない。
/// `InheritType` のRSrs・Rrs（親の拡大縮小を外す合成）だけに使い、
/// 一般の行列分解には使わない。いずれかの軸の長さが0または非有限の場合、
/// および平行移動が非有限の場合は `None` を返す。
pub(crate) fn decompose_trs(m: &[f64; 16]) -> Option<([f64; 3], [f64; 16], [f64; 3])> {
    let translation = [m[12], m[13], m[14]];
    if !translation.iter().all(|v| v.is_finite()) {
        return None;
    }
    let mut scales = [0.0; 3];
    for col in 0..3 {
        let length = (m[col * 4] * m[col * 4]
            + m[col * 4 + 1] * m[col * 4 + 1]
            + m[col * 4 + 2] * m[col * 4 + 2])
            .sqrt();
        if length == 0.0 || !length.is_finite() {
            return None;
        }
        scales[col] = length;
    }
    let mut rotation = IDENTITY;
    for col in 0..3 {
        for row in 0..3 {
            rotation[col * 4 + row] = m[col * 4 + row] / scales[col];
        }
    }
    Some((translation, rotation, scales))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 行列積と逆行列が単位行列に戻る。
    #[test]
    fn 逆行列を掛けると単位行列に戻る() {
        // 回転・非一様拡大・平行移動・鏡像を含む一般の行列で確かめる。
        let cases: [[f64; 16]; 3] = [
            IDENTITY,
            [
                0.0, 1.0, 0.0, 0.0, //
                -1.0, 0.0, 0.0, 0.0, //
                0.0, 0.0, 2.0, 0.0, //
                3.0, -4.0, 5.0, 1.0, //
            ],
            [
                1.0, 0.2, 0.0, 0.0, //
                0.0, -3.0, 0.5, 0.0, //
                0.3, 0.0, 1.0, 0.0, //
                -2.0, 7.0, 0.5, 1.0, //
            ],
        ];
        for m in cases {
            let inv = inverse(&m).expect("正則な行列は逆行列を持つ");
            for product in [mul(&m, &inv), mul(&inv, &m)] {
                for (index, value) in product.iter().enumerate() {
                    let want = if index % 5 == 0 { 1.0 } else { 0.0 };
                    assert!(
                        (value - want).abs() < 1e-9,
                        "M * inv(M) が単位行列になる: [{index}] {value} != {want}"
                    );
                }
            }
        }
    }

    /// 特異な行列は逆行列を持たない。
    #[test]
    fn 特異な行列は逆行列を持たない() {
        // 零行列と、1列が他の列の和である行列は特異である。
        let zero = [0.0; 16];
        assert!(inverse(&zero).is_none());
        let mut singular = IDENTITY;
        singular[8] = 1.0;
        singular[9] = 1.0;
        singular[10] = 0.0;
        assert!(inverse(&singular).is_none());
    }

    /// 点と法線の変換が期待どおりになる。
    #[test]
    fn 点と法線を変換できる() {
        // X軸方向へ2倍し、(1,2,3)だけ平行移動する。
        let m = [
            2.0, 0.0, 0.0, 0.0, //
            0.0, 1.0, 0.0, 0.0, //
            0.0, 0.0, 1.0, 0.0, //
            1.0, 2.0, 3.0, 1.0, //
        ];
        let p = transform_point(&m, [1.0, 1.0, 1.0]).expect("点は変換できる");
        assert_eq!(p, [3.0, 3.0, 4.0]);
        // 法線は拡大の影響を逆転置で打ち消し、正規化される。
        let n = transform_normal(&m, [1.0, 0.0, 0.0]).expect("法線は変換できる");
        assert!((n[0] - 1.0).abs() < 1e-12 && n[1] == 0.0 && n[2] == 0.0);
        // 長さ0の法線と `w` が0の点は変換できない。
        assert!(transform_normal(&m, [0.0, 0.0, 0.0]).is_none());
        let mut bad = IDENTITY;
        bad[15] = 0.0;
        assert!(transform_point(&bad, [0.0, 0.0, 0.0]).is_none());
    }

    /// 回転を含む行列で法線が逆転置どおりに向く。
    #[test]
    fn 回転と非一様拡大で法線が逆転置どおりに向く() {
        // 対角行列では `inv * n` と `inv^T * n` が一致するため、回転を含む行列で確かめる。
        // X軸+90度回転であり、`y` 軸が `z` 軸へ向く。
        let rot_x90 = [
            1.0, 0.0, 0.0, 0.0, //
            0.0, 0.0, 1.0, 0.0, //
            0.0, -1.0, 0.0, 0.0, //
            0.0, 0.0, 0.0, 1.0, //
        ];
        let n = transform_normal(&rot_x90, [0.0, 1.0, 0.0]).expect("回転で法線は変換できる");
        assert!(
            n[0].abs() < 1e-12 && n[1].abs() < 1e-12 && (n[2] - 1.0).abs() < 1e-12,
            "X+90度回転で y 法線は z に向く: {n:?}"
        );
        // 回転と非一様拡大の合成であり、対角成分だけでは向きが決まらない。
        // `R(X+90) * S(2,1,1)` であり、`y` 法線は回転で `z` に向く。
        let rot_scaled = [
            2.0, 0.0, 0.0, 0.0, //
            0.0, 0.0, 1.0, 0.0, //
            0.0, -1.0, 0.0, 0.0, //
            0.0, 0.0, 0.0, 1.0, //
        ];
        let n = transform_normal(&rot_scaled, [0.0, 1.0, 0.0]).expect("合成行列で法線は変換できる");
        assert!(
            n[0].abs() < 1e-12 && n[1].abs() < 1e-12 && (n[2] - 1.0).abs() < 1e-12,
            "回転と非一様拡大で y 法線は z に向く: {n:?}"
        );
        // `x` 法線は拡大の影響を逆転置で打ち消し、正規化で `x` に戻る。
        let n = transform_normal(&rot_scaled, [1.0, 0.0, 0.0]).expect("x 法線は変換できる");
        assert!(
            (n[0] - 1.0).abs() < 1e-12 && n[1].abs() < 1e-12 && n[2].abs() < 1e-12,
            "x 法線は拡大を打ち消して x に戻る: {n:?}"
        );
    }

    /// 差分ベクトルは線形部分だけで変換し、平行移動を掛けない。
    #[test]
    fn 差分ベクトルは平行移動を掛けずに変換できる() {
        // X軸方向へ2倍し、(1,2,3)だけ平行移動する。
        let m = [
            2.0, 0.0, 0.0, 0.0, //
            0.0, 1.0, 0.0, 0.0, //
            0.0, 0.0, 1.0, 0.0, //
            1.0, 2.0, 3.0, 1.0, //
        ];
        let v = transform_vector(&m, [1.0, 1.0, 1.0]).expect("差分は変換できる");
        assert_eq!(v, [2.0, 1.0, 1.0]);
        // 鏡像を含む行列でも符号が反転するだけで、平行移動は入らない。
        let mut mirrored = IDENTITY;
        mirrored[0] = -1.0;
        mirrored[12] = 5.0;
        let v = transform_vector(&mirrored, [1.0, 2.0, 3.0]).expect("鏡像でも変換できる");
        assert_eq!(v, [-1.0, 2.0, 3.0]);
        // 非有限の入力と非有限の結果は変換できない。
        assert!(transform_vector(&m, [f64::NAN, 0.0, 0.0]).is_none());
        let mut bad = IDENTITY;
        bad[0] = f64::INFINITY;
        assert!(transform_vector(&bad, [1.0, 0.0, 0.0]).is_none());
    }
}
