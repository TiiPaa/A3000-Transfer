//! Short-time Fourier transform — port `librosa.stft`.
//!
//! Algo (paramètres par défaut au sr=44100) :
//!   1. Pad `y` avec `n_fft/2` zéros de chaque côté (`pad_mode='constant'`).
//!   2. Hann périodique : `w[n] = 0.5 * (1 - cos(2π n / N))` pour n=0..N-1.
//!   3. n_frames = 1 + len(y) / hop_length.
//!   4. Pour chaque frame f : `frame = y_padded[f*hop : f*hop+n_fft] * w`,
//!      puis FFT (rustfft), on garde les `n_fft/2+1` premiers bins.
//!
//! Référence : `librosa/core/spectrum.py:stft` + `librosa/filters.py:get_window`.

use std::sync::Arc;

use ndarray::Array2;
use rustfft::{num_complex::Complex32, Fft, FftPlanner};

/// Hann window périodique (`fftbins=True` dans `scipy.signal.get_window`).
///
/// `w[n] = 0.5 * (1 - cos(2π n / N))` pour n=0..N-1.
pub fn hann_window(n: usize) -> Vec<f32> {
    if n == 0 {
        return Vec::new();
    }
    let n_f = n as f32;
    (0..n)
        .map(|i| {
            let theta = 2.0 * std::f32::consts::PI * (i as f32) / n_f;
            0.5 * (1.0 - theta.cos())
        })
        .collect()
}

/// Calcule la STFT centrée (`center=True`, `pad_mode='constant'`) avec fenêtre Hann.
///
/// Retourne une matrice complexe `(n_freq, n_frames)` row-major.
/// `n_freq = n_fft / 2 + 1`. `n_frames = 1 + y.len() / hop_length`.
pub fn stft(y: &[f32], n_fft: usize, hop_length: usize) -> Array2<Complex32> {
    assert!(n_fft > 0 && n_fft.is_power_of_two(), "n_fft doit être une puissance de 2");
    assert!(hop_length > 0, "hop_length > 0");

    let pad = n_fft / 2;
    let n_freq = pad + 1;
    let n_frames = 1 + y.len() / hop_length;
    let total_len = y.len() + 2 * pad;

    let window = hann_window(n_fft);
    let mut planner = FftPlanner::<f32>::new();
    let fft: Arc<dyn Fft<f32>> = planner.plan_fft_forward(n_fft);

    let mut output = Array2::<Complex32>::zeros((n_freq, n_frames));
    let mut buf = vec![Complex32::new(0.0, 0.0); n_fft];
    let mut scratch = vec![Complex32::new(0.0, 0.0); fft.get_inplace_scratch_len()];

    for f in 0..n_frames {
        let start = f * hop_length; // index dans le buffer paddé
        // Construit la frame : zéro-padding implicite hors [pad, pad+y.len()).
        for i in 0..n_fft {
            let k = start + i;
            let sample = if k < pad || k >= pad + y.len() {
                0.0
            } else {
                y[k - pad]
            };
            buf[i] = Complex32::new(sample * window[i], 0.0);
        }
        let _ = total_len; // documenté pour clarté
        fft.process_with_scratch(&mut buf, &mut scratch);
        for bin in 0..n_freq {
            output[(bin, f)] = buf[bin];
        }
    }
    output
}

/// `|S|` — magnitude bin par bin.
pub fn magnitude(spec: &Array2<Complex32>) -> Array2<f32> {
    spec.mapv(|c| c.norm())
}

/// `|S|²` — power bin par bin (utilisé par mel spectrogram avec `power=2.0`).
pub fn power(spec: &Array2<Complex32>) -> Array2<f32> {
    spec.mapv(|c| c.norm_sqr())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn hann_window_periodic_basics() {
        let w = hann_window(2048);
        assert_eq!(w.len(), 2048);
        assert!((w[0] - 0.0).abs() < 1e-7);
        assert!((w[1024] - 1.0).abs() < 1e-7);
        // Symétrie périodique : w[k] = w[N-k] pour k > 0 (tolérance f32)
        for k in 1..1024 {
            let diff = (w[k] - w[2048 - k]).abs();
            assert!(diff < 1e-6, "k={k} diff={diff}");
        }
    }

    #[test]
    fn stft_shape_silence() {
        let y = vec![0.0_f32; 10000];
        let s = stft(&y, 2048, 512);
        assert_eq!(s.shape(), &[1025, 1 + 10000 / 512]);
        for v in s.iter() {
            assert_eq!(v.norm(), 0.0);
        }
    }

    #[test]
    fn stft_oracle_all_cases_match_librosa() {
        let oracle_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../docs/conversion/oracles/golden/onset");
        if !oracle_root.exists() {
            eprintln!("oracle missing — run `python generate_oracles.py --onset` first");
            return;
        }
        for case_name in ["impulses_4_44100", "loop01", "loop02", "loop03", "reese02"] {
            let case_dir = oracle_root.join(case_name);
            let y = read_f32_le(&case_dir.join("y.f32"));
            let expected_mag = read_f32_le(&case_dir.join("stft_mag.f32"));

            let spec = stft(&y, 2048, 512);
            let mag = magnitude(&spec);
            let n_freq = 1025;
            let n_frames = 1 + y.len() / 512;
            assert_eq!(mag.shape(), &[n_freq, n_frames]);
            assert_eq!(expected_mag.len(), n_freq * n_frames);

            let mut max_abs = 0.0_f32;
            let mut max_rel = 0.0_f32;
            for (i, (got, &exp)) in mag.iter().zip(expected_mag.iter()).enumerate() {
                let abs_err = (got - exp).abs();
                let rel_err = if exp.abs() > 1e-6 { abs_err / exp.abs() } else { abs_err };
                if abs_err > max_abs { max_abs = abs_err; }
                if rel_err > max_rel { max_rel = rel_err; }
                // librosa scipy/pocketfft vs rustfft : tolérance de
                // 1e-3 abs *ou* relative est très généreuse pour de la magnitude STFT.
                assert!(
                    abs_err < 1e-3 || rel_err < 1e-3,
                    "{case_name} bin {i}: got={got} exp={exp} abs={abs_err} rel={rel_err}"
                );
            }
            eprintln!("stft {case_name}: max_abs={max_abs:.3e} max_rel={max_rel:.3e}");
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
