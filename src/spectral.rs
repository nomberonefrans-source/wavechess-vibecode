//! Auxiliary dyadic diffusion of the existing field, not a move selector.
//!
//! A board index is (rank << 3) | file. Each of its six bits is mixed with
//! its XOR neighbour. This is a hypercube kernel, NOT Euclidean distance or
//! a replacement for the geometric rays and blockers in wave.rs.

const MIX: [f32; 6] = [0.25, 0.125, 0.0625, 0.25, 0.125, 0.0625];

/// Unnormalised Walsh–Hadamard transform: H(H(x)) = 64 x.
pub fn fwht(values: &mut [f32; 64]) {
    let mut half = 1;
    while half < 64 {
        for base in (0..64).step_by(half * 2) {
            for j in 0..half {
                let a = values[base + j];
                let b = values[base + j + half];
                values[base + j] = a + b;
                values[base + j + half] = a - b;
            }
        }
        half *= 2;
    }
}

/// H^-1 diag(lambda) H x, equivalent to convolution by a positive,
/// unit-sum XOR kernel. Thus bounded input remains bounded after diffusion.
pub fn diffuse(mut field: [f32; 64]) -> [f32; 64] {
    fwht(&mut field);
    for (mode, value) in field.iter_mut().enumerate() {
        let mut multiplier = 1.0 / 64.0;
        for (bit, mix) in MIX.iter().enumerate() {
            if mode & (1 << bit) != 0 {
                multiplier *= 1.0 - 2.0 * mix;
            }
        }
        *value *= multiplier;
    }
    fwht(&mut field);
    field
}

/// Signed control near material sources, bounded to [-1, 1].
/// All masses use the same square-root density law; no piece-square table.
pub fn support(control: [f32; 64], charge: &[f32; 64]) -> f32 {
    let smooth = diffuse(control);
    let mut sum = 0.0;
    let mut mass = 0.0;
    for s in 0..64 {
        let density = charge[s].abs().sqrt();
        sum += density * smooth[s];
        mass += density;
    }
    if mass == 0.0 { 0.0 } else { (sum / mass).clamp(-1.0, 1.0) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transform_involution_and_parseval() {
        let input = std::array::from_fn(|s| ((s * 17 % 29) as f32 - 14.0) / 16.0);
        let mut transformed = input;
        fwht(&mut transformed);
        let energy: f32 = input.iter().map(|v| v * v).sum();
        let spectral: f32 = transformed.iter().map(|v| v * v).sum();
        assert!((spectral / 64.0 - energy).abs() < 1e-4);
        fwht(&mut transformed);
        for s in 0..64 {
            assert!((transformed[s] / 64.0 - input[s]).abs() < 1e-5);
        }
    }

    #[test]
    fn diffusion_matches_independent_direct_xor_convolution() {
        let input = std::array::from_fn(|s| ((s * 13 % 31) as f32 - 15.0) / 16.0);
        let result = diffuse(input);
        for s in 0..64 {
            let direct: f32 = (0..64).map(|t| {
                let weight: f32 = MIX.iter().enumerate().map(|(bit, p)| {
                    if (s ^ t) & (1 << bit) != 0 { *p } else { 1.0 - p }
                }).product();
                weight * input[t]
            }).sum();
            assert!((direct - result[s]).abs() < 1e-5);
            assert!(result[s].abs() <= 1.0);
        }
        assert_eq!(diffuse([1.0; 64]), [1.0; 64]);
    }

    #[test]
    fn reflection_and_colour_reversal_preserve_symmetry() {
        let input = std::array::from_fn(|s| ((s * 19 % 37) as f32 - 18.0) / 20.0);
        let charge = std::array::from_fn(|s| if s % 5 == 0 { 100.0 } else { 0.0 });
        for reflection in [7, 56, 63] {
            let flipped = std::array::from_fn(|s| -input[s ^ reflection]);
            let mass = std::array::from_fn(|s| -charge[s ^ reflection]);
            assert!((support(input, &charge) + support(flipped, &mass)).abs() < 1e-5);
        }
        assert_eq!(support(input, &[0.0; 64]), 0.0);
    }
}
