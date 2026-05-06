//! Détection de transients — port pur Rust de `librosa.onset.onset_detect`
//! et de notre wrapper applicatif `engine.py:detect_transients`.
//!
//! Pipeline interne (n_fft=2048, hop=512 par défaut) :
//!   STFT → power → mel filterbank (Slaney 128 bins) → power_to_db
//!     → diff temporel + ReLU + mean (onset envelope)
//!     → normalize → peak_pick → backtrack
//!     → frames_to_samples → prepend [0] si premier onset > 10 ms
//!     → filtre min_gap_ms → snap_ms refine → dedup
//!
//! Validation A/B : tolérance ≤1 frame d'écart vs librosa sur ≥95 % des cas.
//! Cf. `docs/conversion/oracles/golden/onset/`.

pub mod stft;
pub mod mel;
pub mod flux;
pub mod peak;
pub mod backtrack;

use ndarray::Array1;

/// Constantes par défaut alignées sur `librosa` 0.11 et `engine.py`.
pub const DEFAULT_N_FFT: usize = 2048;
pub const DEFAULT_HOP_LENGTH: usize = 512;
pub const DEFAULT_N_MELS: usize = 128;
pub const DEFAULT_DELTA_BASE: f32 = 0.07;

/// Paramètres de détection avec defaults applicatifs (mirror `engine.py`).
#[derive(Debug, Clone)]
pub struct DetectOptions {
    pub sensitivity: f32,
    pub min_gap_ms: f32,
    pub snap_ms: f32,
    pub backtrack: bool,
    pub hop_length: usize,
    pub n_fft: usize,
}

impl Default for DetectOptions {
    fn default() -> Self {
        Self {
            sensitivity: 1.0,
            min_gap_ms: 30.0,
            snap_ms: 50.0,
            backtrack: true,
            hop_length: DEFAULT_HOP_LENGTH,
            n_fft: DEFAULT_N_FFT,
        }
    }
}

/// Calcule l'onset envelope (sortie de `librosa.onset.onset_strength`)
/// depuis le signal mono.
pub fn onset_strength(y: &[f32], sr: u32, n_fft: usize, hop_length: usize) -> Array1<f32> {
    let spec = stft::stft(y, n_fft, hop_length);
    let power = stft::power(&spec);
    let fb = mel::mel_filterbank(sr, n_fft, DEFAULT_N_MELS, 0.0, sr as f32 / 2.0);
    let mel_pow = mel::melspectrogram_power(&power, &fb);
    let mel_db = flux::power_to_db(&mel_pow);
    flux::onset_strength_from_mel_db(&mel_db, n_fft, hop_length)
}

/// Détection d'onsets en frames — équivalent direct de `librosa.onset.onset_detect`.
///
/// Inclut la normalisation `(env - min) / (max + tiny)` et le backtracking
/// optionnel. Paramètres `peak_pick` calculés depuis `sr` et `hop_length`
/// comme librosa : 30/0/100/100/30 ms → frames.
pub fn onset_detect_frames(
    y: &[f32],
    sr: u32,
    delta: f32,
    backtrack_events: bool,
    n_fft: usize,
    hop_length: usize,
) -> Vec<usize> {
    let env = onset_strength(y, sr, n_fft, hop_length);

    if env.iter().all(|&v| v == 0.0) || env.iter().any(|v| !v.is_finite()) {
        return Vec::new();
    }

    // env est construit via Array1::from / mapv : toujours contigu standard.
    #[allow(clippy::expect_used)]
    let env_slice = env.as_slice().expect("ndarray Array1 standard layout");
    let env_norm = peak::normalize_envelope(env_slice);
    let arr = Array1::from(env_norm);

    // Defaults librosa (0.03/0/0.10/0.10/0.03 sec) → frames
    let sr_f = sr as f32;
    let hop_f = hop_length as f32;
    let pre_max  = ((0.03 * sr_f / hop_f) as usize).max(1);
    let post_max = ((0.00 * sr_f / hop_f) as usize + 1).max(1);
    let pre_avg  = ((0.10 * sr_f / hop_f) as usize).max(1);
    let post_avg = ((0.10 * sr_f / hop_f) as usize + 1).max(1);
    let wait     = ((0.03 * sr_f / hop_f) as usize).max(1);

    let peaks = peak::peak_pick(arr.view(), pre_max, post_max, pre_avg, post_avg, delta, wait);
    if backtrack_events {
        #[allow(clippy::expect_used)]
        let arr_slice = arr.as_slice().expect("ndarray Array1 standard layout");
        backtrack::onset_backtrack(&peaks, arr_slice)
    } else {
        peaks
    }
}

/// Recalibre un onset vers la première montée d'énergie significative
/// (port de `engine.py:refine_onset`).
pub fn refine_onset(y: &[f32], onset: usize, sr: u32, max_samples: usize) -> usize {
    let smooth_ms: f32 = 1.0;
    let bg_factor: f32 = 5.0;
    let abs_floor: f32 = 0.005;

    let n = y.len();
    if onset >= n.saturating_sub(1) || max_samples < 2 {
        return onset;
    }
    let hi = (onset + max_samples + 1).min(n);
    if hi <= onset { return onset; }
    let region: Vec<f32> = y[onset..hi].iter().map(|v| v.abs()).collect();
    if region.len() < 8 {
        return onset;
    }

    let bg_samples = ((sr as f32 * 0.005) as usize)
        .min(region.len() / 4)
        .max(8);
    let background = median_f32(&region[..bg_samples.min(region.len())]);
    let threshold = (background * bg_factor).max(abs_floor);

    let win = ((smooth_ms * sr as f32 / 1000.0) as usize).max(2);
    let smoothed: Vec<f32> = if region.len() > win {
        // moyenne mobile via cumulative sum
        let mut cumsum = Vec::with_capacity(region.len() + 1);
        cumsum.push(0.0_f32);
        let mut acc = 0.0_f32;
        for &v in &region {
            acc += v;
            cumsum.push(acc);
        }
        let win_f = win as f32;
        (win..cumsum.len())
            .map(|i| (cumsum[i] - cumsum[i - win]) / win_f)
            .collect()
    } else {
        region.clone()
    };

    for (i, &v) in smoothed.iter().enumerate() {
        if v > threshold {
            return onset + i;
        }
    }
    onset
}

/// API publique : détection complète en indices d'échantillons (port direct
/// de `engine.py:detect_transients`).
pub fn detect_transients(y: &[f32], sr: u32, opts: &DetectOptions) -> Vec<usize> {
    let delta = DEFAULT_DELTA_BASE / opts.sensitivity.max(1e-3);

    // 1. onset_detect en frames (avec backtrack si demandé), puis frames → samples.
    let onset_frames = onset_detect_frames(
        y, sr, delta, opts.backtrack, opts.n_fft, opts.hop_length,
    );
    let mut onset_samples: Vec<usize> = onset_frames
        .into_iter()
        .map(|f| f * opts.hop_length)
        .collect();

    // 2. Prepend [0] si vide ou si premier onset > 10 ms.
    let prepend_threshold = (sr as f32 * 0.01) as usize;
    if onset_samples.is_empty() || onset_samples[0] > prepend_threshold {
        onset_samples.insert(0, 0);
    }

    // 3. Filtre min_gap.
    let min_gap = (sr as f32 * opts.min_gap_ms / 1000.0) as usize;
    let mut filtered: Vec<usize> = Vec::with_capacity(onset_samples.len());
    if let Some(&first) = onset_samples.first() {
        filtered.push(first);
        let mut last = first;
        for &s in onset_samples.iter().skip(1) {
            if s - last >= min_gap {
                filtered.push(s);
                last = s;
            }
        }
    }

    // 4. Snap forward (refine_onset) si snap_ms > 0.
    if opts.snap_ms > 0.0 && filtered.len() > 1 {
        let snap_samples = (opts.snap_ms * sr as f32 / 1000.0) as usize;
        let buffer_samples = (0.005 * sr as f32) as usize;
        let len_y = y.len();
        let mut refined = Vec::<usize>::with_capacity(filtered.len());
        refined.push(filtered[0]);
        for i in 1..filtered.len() {
            let room = if i + 1 < filtered.len() {
                filtered[i + 1].saturating_sub(filtered[i]).saturating_sub(buffer_samples)
            } else {
                len_y.saturating_sub(filtered[i]).saturating_sub(1)
            };
            let max_fwd = room.min(snap_samples);
            if max_fwd < 2 {
                refined.push(filtered[i]);
            } else {
                refined.push(refine_onset(y, filtered[i], sr, max_fwd));
            }
        }
        // 5. Dedup avec min_gap après refine.
        let mut deduped: Vec<usize> = Vec::with_capacity(refined.len());
        deduped.push(refined[0]);
        let mut last = refined[0];
        for s in refined.into_iter().skip(1) {
            if s - last >= min_gap {
                deduped.push(s);
                last = s;
            }
        }
        filtered = deduped;
    }

    filtered
}

/// Médiane d'une slice de f32 (sort partial). Pour des longueurs typiques
/// (10-200 échantillons) le coût est négligeable, et le tri partial via
/// `select_nth_unstable_by` est en O(n) attendu.
fn median_f32(slice: &[f32]) -> f32 {
    if slice.is_empty() {
        return 0.0;
    }
    let mut v: Vec<f32> = slice.to_vec();
    let mid = v.len() / 2;
    v.select_nth_unstable_by(mid, |a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    if v.len() % 2 == 1 {
        v[mid]
    } else {
        // Pour len pair, numpy renvoie la moyenne des deux centraux.
        let lower = v[..mid].iter().copied().fold(f32::NEG_INFINITY, f32::max);
        (v[mid] + lower) / 2.0
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn read_f32_le(path: &std::path::Path) -> Vec<f32> {
        let bytes = std::fs::read(path).unwrap_or_else(|_| panic!("read {path:?}"));
        bytes
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect()
    }

    #[test]
    fn onset_detect_frames_oracle() {
        let oracle_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../docs/conversion/oracles/golden/onset");
        if !oracle_root.exists() {
            eprintln!("oracle missing — run `python generate_oracles.py --onset` first");
            return;
        }

        for case_name in ["impulses_4_44100", "loop01", "loop02", "loop03", "reese02"] {
            let case_dir = oracle_root.join(case_name);
            let y = read_f32_le(&case_dir.join("y.f32"));
            let expected: Vec<usize> = serde_json::from_str(
                &std::fs::read_to_string(case_dir.join("onset_detect.json")).unwrap(),
            ).unwrap();

            let got = onset_detect_frames(&y, 44100, 0.07, true, 2048, 512);
            assert_eq!(
                got, expected,
                "{case_name} mismatch (got {} ; expected {})",
                got.len(), expected.len()
            );
            eprintln!("onset_detect_frames {case_name}: {} events ✓", got.len());
        }
    }

    #[test]
    fn detect_transients_oracle_within_tolerance() {
        let oracle_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../docs/conversion/oracles/golden/onset");
        if !oracle_root.exists() {
            eprintln!("oracle missing — run `python generate_oracles.py --onset` first");
            return;
        }

        let opts = DetectOptions::default();
        let mut total = 0usize;
        let mut within = 0usize;
        let frame_tolerance: i64 = 1; // ≤ 1 frame d'écart = 512 samples à hop=512
        let sample_tolerance: i64 = frame_tolerance * 512;

        for case_name in ["impulses_4_44100", "loop01", "loop02", "loop03", "reese02"] {
            let case_dir = oracle_root.join(case_name);
            let y = read_f32_le(&case_dir.join("y.f32"));
            let expected: Vec<i64> = serde_json::from_str(
                &std::fs::read_to_string(case_dir.join("engine_samples.json")).unwrap(),
            ).unwrap();

            let got = detect_transients(&y, 44100, &opts);
            let got_i64: Vec<i64> = got.iter().map(|&s| s as i64).collect();

            // Stratégie de comparaison : pour chaque onset attendu, trouver le
            // got le plus proche et compter ceux dans la tolérance.
            let mut matched_per_case = 0usize;
            for &e in &expected {
                let nearest = got_i64.iter()
                    .map(|&g| (g - e).abs())
                    .min()
                    .unwrap_or(i64::MAX);
                if nearest <= sample_tolerance {
                    matched_per_case += 1;
                }
            }
            total += expected.len();
            within += matched_per_case;
            eprintln!(
                "detect_transients {case_name}: got={} expected={} matched_within_1frame={}/{}",
                got.len(), expected.len(), matched_per_case, expected.len(),
            );
        }

        let pct = (within as f64 / total.max(1) as f64) * 100.0;
        eprintln!("Total : {within}/{total} ({pct:.1}%) onsets dans la tolérance ≤1 frame");
        assert!(
            pct >= 95.0,
            "tolérance non-atteinte : {within}/{total} ({pct:.1}%) < 95%"
        );
    }
}
