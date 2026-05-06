//! Spectral flux + onset envelope — port `librosa.onset.onset_strength`
//! (configuration par défaut : feature=mel, lag=1, max_size=1, aggregate=mean,
//! center=True).
//!
//! Pipeline complète depuis `y` :
//!   1. mel power spectrogram (cf. `crate::mel`)
//!   2. `power_to_db(mel)` (ref=1.0, amin=1e-10, top_db=80)
//!   3. diff temporel : `D[:, t] = S[:, t+lag] - S[:, t]`
//!   4. ReLU : `max(0, D)`
//!   5. moyenne sur les bins mel : `env = mean(D_relu, axis=0)`
//!   6. padding gauche de `lag + n_fft // (2*hop)` zéros
//!   7. trim à `n_frames` (shape originale du mel spectrogram)
//!
//! Référence : `librosa/onset.py:onset_strength_multi` (lignes 580-641)
//! et `librosa/core/spectrum.py:power_to_db`.

use ndarray::{Array1, Array2};

/// Power-to-dB conversion (ref=1.0, amin=1e-10, top_db=80).
///
/// `log_spec = 10 * log10(max(amin, S))` puis clipping à `max - top_db`.
pub fn power_to_db(mel: &Array2<f32>) -> Array2<f32> {
    let amin: f32 = 1e-10;
    let mut log_spec = mel.mapv(|s| 10.0 * s.max(amin).log10());
    let max_val = log_spec.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let floor = max_val - 80.0;
    log_spec.mapv_inplace(|v| v.max(floor));
    log_spec
}

/// Onset envelope sur la mel spectrogram en dB.
///
/// `n_fft` et `hop_length` sont utilisés uniquement pour calculer le
/// padding de compensation framing (`n_fft / (2*hop_length)` frames).
///
/// Retourne un vecteur de longueur `n_frames` (= mel_db.shape[1]).
pub fn onset_strength_from_mel_db(
    mel_db: &Array2<f32>,
    n_fft: usize,
    hop_length: usize,
) -> Array1<f32> {
    let n_frames = mel_db.shape()[1];
    let lag = 1usize;

    // Diff temporel + ReLU + mean(axis=0).
    // diff_mean[t] = mean_f(max(0, S[f, t+1] - S[f, t]))  pour t in 0..n_frames-1
    let n_mels = mel_db.shape()[0];
    let mut diff_mean: Vec<f32> = Vec::with_capacity(n_frames.saturating_sub(lag));
    for t in 0..n_frames.saturating_sub(lag) {
        let mut sum = 0.0_f32;
        for f in 0..n_mels {
            let d = mel_db[(f, t + lag)] - mel_db[(f, t)];
            if d > 0.0 {
                sum += d;
            }
        }
        diff_mean.push(sum / (n_mels as f32));
    }

    // Padding left + trim à n_frames.
    let pad_width = lag + n_fft / (2 * hop_length);
    let mut env = Array1::<f32>::zeros(n_frames);
    for (i, v) in diff_mean.iter().enumerate() {
        let target = pad_width + i;
        if target < n_frames {
            env[target] = *v;
        }
    }
    env
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::{mel, stft};

    #[test]
    fn power_to_db_basic() {
        // Cas simple : input [1.0, 0.1, 0.01, 1e-12] → 10*log10(...) puis floor à -80.
        let s: Array2<f32> = ndarray::arr2(&[[1.0, 0.1, 0.01, 1e-12]]);
        let db = power_to_db(&s);
        // log10(1.0) = 0 → 0 dB (max), floor = -80
        assert!((db[(0, 0)] - 0.0).abs() < 1e-5);
        assert!((db[(0, 1)] + 10.0).abs() < 1e-5);
        assert!((db[(0, 2)] + 20.0).abs() < 1e-5);
        // 1e-12 < amin=1e-10 → clamp à amin → 10*log10(1e-10) = -100,
        // mais clippé à max - 80 = -80.
        assert!((db[(0, 3)] + 80.0).abs() < 1e-5);
    }

    #[test]
    fn flux_oracle_matches_librosa() {
        let oracle_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../docs/conversion/oracles/golden/onset");
        if !oracle_root.exists() {
            eprintln!("oracle missing — run `python generate_oracles.py --onset` first");
            return;
        }

        let fb = mel::mel_filterbank(44100, 2048, 128, 0.0, 22050.0);

        for case_name in ["impulses_4_44100", "loop01", "loop02", "loop03", "reese02"] {
            let case_dir = oracle_root.join(case_name);
            let y = read_f32_le(&case_dir.join("y.f32"));
            let expected_env = read_f32_le(&case_dir.join("onset_env.f32"));

            let spec = stft::stft(&y, 2048, 512);
            let power = stft::power(&spec);
            let mel_pow = mel::melspectrogram_power(&power, &fb);
            let mel_db = power_to_db(&mel_pow);
            let env = onset_strength_from_mel_db(&mel_db, 2048, 512);

            assert_eq!(env.len(), expected_env.len(), "{case_name}");

            let mut max_abs = 0.0_f32;
            let mut max_rel = 0.0_f32;
            for (i, (&got, &exp)) in env.iter().zip(expected_env.iter()).enumerate() {
                let abs_err = (got - exp).abs();
                let rel_err = if exp.abs() > 1e-6 { abs_err / exp.abs() } else { abs_err };
                if abs_err > max_abs { max_abs = abs_err; }
                if rel_err > max_rel { max_rel = rel_err; }
                // tolérance large : le pipeline empile mel + dB + diff + mean
                // ce qui amplifie les écarts numériques entre rustfft et pocketfft.
                assert!(
                    abs_err < 5e-2 || rel_err < 5e-2,
                    "{case_name} frame {i}: got={got} exp={exp} abs={abs_err} rel={rel_err}"
                );
            }
            eprintln!("flux {case_name}: max_abs={max_abs:.3e} max_rel={max_rel:.3e}");
        }
    }

    fn read_f32_le(path: &std::path::Path) -> Vec<f32> {
        let bytes = std::fs::read(path).unwrap_or_else(|_| panic!("read {path:?}"));
        assert!(bytes.len() % 4 == 0);
        bytes
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect()
    }
}
