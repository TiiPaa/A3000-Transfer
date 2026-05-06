//! Mel filterbank et mel power spectrogram — port `librosa.filters.mel` +
//! `librosa.feature.melspectrogram`.
//!
//! Paramètres par défaut (au sr=44100) : n_mels=128, fmin=0, fmax=sr/2,
//! htk=false (Slaney), norm='slaney' (area-normalize), power=2.0.
//!
//! Algorithme :
//!   1. Construire n_mels+2 fréquences mel-spaced entre fmin et fmax,
//!      converties en Hz via formule Slaney.
//!   2. Pour chaque bin mel m, filtre triangulaire sur les freqs FFT :
//!      `max(0, min((f - left)/(center-left), (right - f)/(right-center)))`
//!   3. Normalisation Slaney : `filter[m] *= 2 / (right_hz - left_hz)`
//!   4. mel_spectrogram = mel_filter @ power_spec  (matmul)
//!
//! Référence : `librosa/filters.py:mel` + `librosa/convert.py:hz_to_mel` /
//! `mel_to_hz`.

use ndarray::{Array1, Array2};

/// Slaney mel scale : linéaire jusqu'à 1000 Hz, log au-delà.
pub fn hz_to_mel_slaney(hz: f32) -> f32 {
    let f_sp = 200.0_f32 / 3.0;
    let min_log_hz = 1000.0_f32;
    let min_log_mel = min_log_hz / f_sp; // = 15.0
    let logstep = (6.4_f32).ln() / 27.0;
    if hz >= min_log_hz {
        min_log_mel + (hz / min_log_hz).ln() / logstep
    } else {
        hz / f_sp
    }
}

pub fn mel_to_hz_slaney(mel: f32) -> f32 {
    let f_sp = 200.0_f32 / 3.0;
    let min_log_hz = 1000.0_f32;
    let min_log_mel = min_log_hz / f_sp;
    let logstep = (6.4_f32).ln() / 27.0;
    if mel >= min_log_mel {
        min_log_hz * ((mel - min_log_mel) * logstep).exp()
    } else {
        f_sp * mel
    }
}

/// Construit le mel filterbank Slaney (htk=false, norm='slaney').
///
/// Retourne une matrice `(n_mels, n_freq)` row-major, dtype f32.
/// `n_freq = n_fft / 2 + 1`.
pub fn mel_filterbank(
    sr: u32,
    n_fft: usize,
    n_mels: usize,
    fmin: f32,
    fmax: f32,
) -> Array2<f32> {
    let n_freq = n_fft / 2 + 1;
    let sr_f = sr as f32;

    // Fréquences FFT : np.fft.rfftfreq(n_fft, d=1/sr) = k * sr / n_fft pour k=0..n_freq-1
    let fft_freqs: Array1<f32> = Array1::from_iter(
        (0..n_freq).map(|k| (k as f32) * sr_f / (n_fft as f32)),
    );

    // n_mels+2 fréquences mel-spaced entre fmin (mel) et fmax (mel).
    let mel_lo = hz_to_mel_slaney(fmin);
    let mel_hi = hz_to_mel_slaney(fmax);
    let mut mel_f: Array1<f32> = Array1::zeros(n_mels + 2);
    let step = (mel_hi - mel_lo) / ((n_mels + 1) as f32);
    for i in 0..(n_mels + 2) {
        mel_f[i] = mel_to_hz_slaney(mel_lo + step * (i as f32));
    }

    let mut weights: Array2<f32> = Array2::zeros((n_mels, n_freq));
    for m in 0..n_mels {
        let f_left = mel_f[m];
        let f_center = mel_f[m + 1];
        let f_right = mel_f[m + 2];
        let d_lower = f_center - f_left;
        let d_upper = f_right - f_center;
        for k in 0..n_freq {
            let f = fft_freqs[k];
            let lower = (f - f_left) / d_lower;
            let upper = (f_right - f) / d_upper;
            let v = lower.min(upper).max(0.0);
            weights[(m, k)] = v;
        }
        // Slaney : * 2 / (f_right - f_left)
        let enorm = 2.0 / (f_right - f_left);
        for k in 0..n_freq {
            weights[(m, k)] *= enorm;
        }
    }
    weights
}

/// Mel power spectrogram = filterbank @ power_spec.
///
/// `power_spec` est `(n_freq, n_frames)` (sortie de `stft::power`).
/// Retourne `(n_mels, n_frames)` row-major.
pub fn melspectrogram_power(
    power_spec: &Array2<f32>,
    filterbank: &Array2<f32>,
) -> Array2<f32> {
    debug_assert_eq!(power_spec.shape()[0], filterbank.shape()[1]);
    filterbank.dot(power_spec)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn slaney_roundtrip_known_points() {
        // Points connus :
        //   0 Hz -> 0 mel
        //   1000 Hz -> 15 mel (boundary)
        //   200 Hz -> 3 mel (linear region, 200 / (200/3))
        for &(hz, expected_mel) in &[(0.0_f32, 0.0_f32), (200.0, 3.0), (1000.0, 15.0)] {
            let m = hz_to_mel_slaney(hz);
            assert!((m - expected_mel).abs() < 1e-5,
                    "hz={hz} expected_mel={expected_mel} got={m}");
            let back = mel_to_hz_slaney(m);
            assert!((back - hz).abs() < 1e-3, "round-trip {hz} -> {m} -> {back}");
        }
        // Région log : 6400 Hz devrait donner 15 + 27 = 42 mel
        let m6400 = hz_to_mel_slaney(6400.0);
        assert!((m6400 - 42.0).abs() < 1e-4, "got {m6400}");
        let h42 = mel_to_hz_slaney(42.0);
        assert!((h42 - 6400.0).abs() < 1e-2, "got {h42}");
    }

    #[test]
    fn filterbank_shape_and_basic_props() {
        let fb = mel_filterbank(44100, 2048, 128, 0.0, 22050.0);
        assert_eq!(fb.shape(), &[128, 1025]);
        assert!(fb.iter().all(|&v| v >= 0.0));
        for m in 0..128 {
            assert!(
                fb.row(m).iter().any(|&v| v > 0.0),
                "filtre mel {m} entièrement nul"
            );
        }
    }

    #[test]
    fn filterbank_oracle_matches_librosa() {
        use crate::stft;

        let oracle_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../docs/conversion/oracles/golden/onset");
        if !oracle_root.exists() {
            eprintln!("oracle missing — run `python generate_oracles.py --onset` first");
            return;
        }

        let fb = mel_filterbank(44100, 2048, 128, 0.0, 22050.0);

        for case_name in ["impulses_4_44100", "loop01", "loop02", "loop03", "reese02"] {
            let case_dir = oracle_root.join(case_name);
            let y = read_f32_le(&case_dir.join("y.f32"));
            let expected_mel = read_f32_le(&case_dir.join("mel.f32"));

            let spec = stft::stft(&y, 2048, 512);
            let power = stft::power(&spec);
            let mel = melspectrogram_power(&power, &fb);
            let n_frames = 1 + y.len() / 512;
            assert_eq!(mel.shape(), &[128, n_frames]);
            assert_eq!(expected_mel.len(), 128 * n_frames);

            let mut max_abs = 0.0_f32;
            let mut max_rel = 0.0_f32;
            for (i, (got, &exp)) in mel.iter().zip(expected_mel.iter()).enumerate() {
                let abs_err = (got - exp).abs();
                let rel_err = if exp.abs() > 1e-6 { abs_err / exp.abs() } else { abs_err };
                if abs_err > max_abs { max_abs = abs_err; }
                if rel_err > max_rel { max_rel = rel_err; }
                // Accumulation 1025 bins × power → tolérance plus large que STFT.
                assert!(
                    abs_err < 1e-2 || rel_err < 1e-2,
                    "{case_name} bin {i}: got={got} exp={exp} abs={abs_err} rel={rel_err}"
                );
            }
            eprintln!("mel {case_name}: max_abs={max_abs:.3e} max_rel={max_rel:.3e}");
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
