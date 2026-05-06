//! Backtracking : recule chaque onset vers le minimum local précédent.
//!
//! Port de `librosa.onset.onset_backtrack` :
//!   1. Trouver les minima locaux : `i` t.q. `energy[i] <= energy[i-1]` ET
//!      `energy[i] < energy[i+1]`. Indices indexés à `i+1` puis 0 prepended.
//!   2. Pour chaque event, prendre le minimum le plus proche **à gauche**
//!      (=∈ ≤). Implémenté par binary search.
//!
//! Référence : `librosa/onset.py:onset_backtrack` (lignes 369-441) +
//! `librosa.util.match_events(events, minima, right=False)`.

/// Calcule les minima locaux selon la définition librosa :
///   `energy[i] <= energy[i-1]` ET `energy[i] < energy[i+1]`
/// Indexés sur l'array d'entrée (i ∈ 1..n-1). On prepend 0 pour gérer les
/// onsets précoces.
pub fn local_minima(energy: &[f32]) -> Vec<usize> {
    let n = energy.len();
    let mut minima = Vec::<usize>::new();
    minima.push(0);
    if n < 3 {
        return minima;
    }
    for i in 1..(n - 1) {
        if energy[i] <= energy[i - 1] && energy[i] < energy[i + 1] {
            minima.push(i);
        }
    }
    minima
}

/// Pour chaque event, trouve le plus grand minima ≤ event.
/// `minima` doit être trié croissant et contenir 0.
pub fn backtrack(events: &[usize], minima: &[usize]) -> Vec<usize> {
    debug_assert!(!minima.is_empty(), "minima doit contenir au moins 0");
    debug_assert!(minima.windows(2).all(|w| w[0] <= w[1]));

    events.iter().map(|&e| {
        // largest minima ≤ e : partition_point(|m| m <= e) - 1
        let idx = minima.partition_point(|&m| m <= e);
        // idx > 0 garanti car minima[0] = 0 ≤ e (events sont dans 0..n_frames).
        if idx == 0 { minima[0] } else { minima[idx - 1] }
    }).collect()
}

/// Combo : minima locaux + match left.
pub fn onset_backtrack(events: &[usize], energy: &[f32]) -> Vec<usize> {
    let minima = local_minima(energy);
    backtrack(events, &minima)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn local_minima_simple() {
        // V-shape : energy descend puis remonte.
        let e = [3.0_f32, 2.0, 1.0, 2.0, 3.0];
        let m = local_minima(&e);
        // i=1: 2 <= 3 ✓ AND 2 < 1 ✗ → non
        // i=2: 1 <= 2 ✓ AND 1 < 2 ✓ → oui
        // i=3: 2 <= 1 ✗ → non
        assert_eq!(m, vec![0, 2]);
    }

    #[test]
    fn local_minima_plateau() {
        // Plateau égalité-stricte : 1 <= 1 ✓ AND 1 < 2 ✓
        let e = [2.0_f32, 1.0, 1.0, 2.0];
        let m = local_minima(&e);
        // i=1: 1 <= 2 ✓ AND 1 < 1 ✗ → non
        // i=2: 1 <= 1 ✓ AND 1 < 2 ✓ → oui
        assert_eq!(m, vec![0, 2]);
    }

    #[test]
    fn backtrack_picks_largest_le() {
        let minima = [0, 5, 10, 15];
        let events = [3, 7, 10, 12, 20];
        let bt = backtrack(&events, &minima);
        assert_eq!(bt, vec![0, 5, 10, 10, 15]);
    }

    #[test]
    fn backtrack_oracle_matches_librosa() {
        let oracle_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../docs/conversion/oracles/golden/onset");
        if !oracle_root.exists() {
            eprintln!("oracle missing — run `python generate_oracles.py --onset` first");
            return;
        }

        for case_name in ["impulses_4_44100", "loop01", "loop02", "loop03", "reese02"] {
            let case_dir = oracle_root.join(case_name);
            let env_norm = read_f32_le(&case_dir.join("onset_env_normalized.f32"));
            let peaks: Vec<usize> = serde_json::from_str(
                &std::fs::read_to_string(case_dir.join("peaks.json")).unwrap(),
            ).unwrap();
            let expected: Vec<usize> = serde_json::from_str(
                &std::fs::read_to_string(case_dir.join("backtracked.json")).unwrap(),
            ).unwrap();

            let got = onset_backtrack(&peaks, &env_norm);
            assert_eq!(
                got, expected,
                "{case_name} mismatch (got {} ; expected {})",
                got.len(), expected.len()
            );
            eprintln!("backtrack {case_name}: {} events ✓", got.len());
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
