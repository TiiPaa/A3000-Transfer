//! Peak picking — port `librosa.util.peak_pick` (`__peak_pick`).
//!
//! Un sample n est sélectionné si :
//!   1. `x[n] == max(x[n - pre_max : n + post_max])`  (slicing Python : right-open)
//!   2. `x[n] >= mean(x[n - pre_avg : n + post_avg]) + delta`
//!   3. `n - last_picked > wait`  (greedy)
//!
//! Cas particulier de la frame 0 : on regarde `x[:post_max]` et `x[:post_avg]`
//! avec inégalité large `>=` au lieu de `==`.
//!
//! Référence : `librosa/util/utils.py:__peak_pick` (lignes 1202-1232).

use ndarray::ArrayView1;

#[allow(clippy::too_many_arguments)]
pub fn peak_pick(
    x: ArrayView1<'_, f32>,
    pre_max: usize,
    post_max: usize,
    pre_avg: usize,
    post_avg: usize,
    delta: f32,
    wait: usize,
) -> Vec<usize> {
    let n_total = x.len();
    if n_total == 0 {
        return Vec::new();
    }
    let mut peaks = Vec::<usize>::new();

    // Cas spécial frame 0 : x[:post_max] et x[:post_avg] (en clamped)
    let p0_end_max = post_max.min(n_total);
    let p0_end_avg = post_avg.min(n_total);
    let head_max_raw = window_max(&x, 0, p0_end_max);
    let head_avg_raw = window_mean(&x, 0, p0_end_avg);
    let mut peak0 = false;
    if let (Some(maxv), Some(avgv)) = (head_max_raw, head_avg_raw) {
        peak0 = (x[0] >= maxv) && (x[0] >= avgv + delta);
    }
    if peak0 {
        peaks.push(0);
    }

    let mut n = if peak0 { wait + 1 } else { 1 };
    while n < n_total {
        let lo = n.saturating_sub(pre_max);
        let hi = (n + post_max).min(n_total);
        let maxn = match window_max(&x, lo, hi) {
            Some(v) => v,
            None => { n += 1; continue; }
        };

        // Égalité stricte avec le max local (comme librosa).
        if x[n] != maxn {
            n += 1;
            continue;
        }

        let lo_avg = n.saturating_sub(pre_avg);
        let hi_avg = (n + post_avg).min(n_total);
        let avgn = match window_mean(&x, lo_avg, hi_avg) {
            Some(v) => v,
            None => { n += 1; continue; }
        };

        if x[n] < avgn + delta {
            n += 1;
            continue;
        }

        peaks.push(n);
        n += wait + 1;
    }
    peaks
}

fn window_max(x: &ArrayView1<'_, f32>, lo: usize, hi: usize) -> Option<f32> {
    if lo >= hi { return None; }
    let mut m = x[lo];
    for i in (lo + 1)..hi {
        if x[i] > m { m = x[i]; }
    }
    Some(m)
}

fn window_mean(x: &ArrayView1<'_, f32>, lo: usize, hi: usize) -> Option<f32> {
    if lo >= hi { return None; }
    let mut sum = 0.0_f32;
    for i in lo..hi {
        sum += x[i];
    }
    Some(sum / ((hi - lo) as f32))
}

/// Normalise l'enveloppe à `[0, 1]` comme `librosa.onset.onset_detect(normalize=True)` :
///   `env_n = (env - min(env)) / (max(env - min(env)) + tiny)`
///
/// `tiny` reproduit `librosa.util.tiny(env)` qui retourne `np.finfo(env.dtype).tiny`,
/// soit `f32::MIN_POSITIVE` ≈ 1.18e-38 pour float32.
pub fn normalize_envelope(env: &[f32]) -> Vec<f32> {
    if env.is_empty() {
        return Vec::new();
    }
    let mut min_v = env[0];
    for &v in env.iter().skip(1) {
        if v < min_v { min_v = v; }
    }
    let shifted: Vec<f32> = env.iter().map(|&v| v - min_v).collect();
    let mut max_v = shifted[0];
    for &v in shifted.iter().skip(1) {
        if v > max_v { max_v = v; }
    }
    let denom = max_v + f32::MIN_POSITIVE;
    shifted.iter().map(|&v| v / denom).collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use ndarray::Array1;

    #[test]
    fn peak_pick_simple_triangle() {
        // Vecteur : monte vers le pic à 10, redescend.
        let x: Vec<f32> = (0..21).map(|i| 10.0 - (i as f32 - 10.0).abs()).collect();
        let arr = Array1::from(x);
        // Avec une fenêtre globale (pre_max=10), seul le sommet à 10 est élu.
        let peaks = peak_pick(arr.view(), 10, 11, 10, 11, 0.5, 5);
        assert_eq!(peaks, vec![10]);
    }

    #[test]
    fn peak_pick_oracle_matches_librosa() {
        let oracle_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../docs/conversion/oracles/golden/onset");
        if !oracle_root.exists() {
            eprintln!("oracle missing — run `python generate_oracles.py --onset` first");
            return;
        }
        let manifest: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(oracle_root.join("manifest.json")).unwrap(),
        ).unwrap();

        for case_meta in manifest["cases"].as_array().unwrap() {
            let case_name = case_meta["name"].as_str().unwrap();
            let case_dir = oracle_root.join(case_name);
            let env_normalized = read_f32_le(&case_dir.join("onset_env_normalized.f32"));
            let expected_peaks: Vec<usize> = serde_json::from_str(
                &std::fs::read_to_string(case_dir.join("peaks.json")).unwrap(),
            ).unwrap();

            let pp = &case_meta["peak_pick_params"];
            let pre_max  = pp["pre_max"].as_u64().unwrap()  as usize;
            let post_max = pp["post_max"].as_u64().unwrap() as usize;
            let pre_avg  = pp["pre_avg"].as_u64().unwrap()  as usize;
            let post_avg = pp["post_avg"].as_u64().unwrap() as usize;
            let delta    = pp["delta"].as_f64().unwrap()    as f32;
            let wait     = pp["wait"].as_u64().unwrap()     as usize;

            let arr = Array1::from(env_normalized);
            let peaks = peak_pick(arr.view(), pre_max, post_max, pre_avg, post_avg, delta, wait);

            assert_eq!(
                peaks, expected_peaks,
                "{case_name} mismatch (got {} peaks, expected {})",
                peaks.len(), expected_peaks.len()
            );
            eprintln!("peak_pick {case_name}: {} peaks ✓", peaks.len());
        }
    }

    #[test]
    fn normalize_envelope_basic() {
        let v = vec![2.0_f32, 4.0, 8.0, 6.0];
        let n = normalize_envelope(&v);
        assert!((n[0] - 0.0).abs() < 1e-6);
        assert!((n[2] - 1.0).abs() < 1e-6);
        assert!((n[1] - 1.0/3.0).abs() < 1e-6);
        assert!((n[3] - 2.0/3.0).abs() < 1e-6);
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
