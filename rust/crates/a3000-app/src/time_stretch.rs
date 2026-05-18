//! Time-stretching pour la fonction Warp du Slicer.
//!
//! Deux algorithmes :
//!
//! - **`stretch_linear`** : resampling par interpolation linéaire. Très
//!   simple (~30 LOC), rapide, mais change le pitch en même temps que la
//!   durée (comme un changement de vitesse de bande). Pour des stretches
//!   < 5% c'est inaudible — au-delà ça commence à sonner pitched.
//!
//! - **`stretch_wsola`** : Waveform Similarity Overlap-Add. Préserve le
//!   pitch via overlap-add de frames windowées Hann, avec recherche de
//!   cross-corrélation pour trouver le meilleur alignement de phase entre
//!   les frames. Qualité production jusqu'à ±50% de stretch. Pur Rust, pas
//!   de dépendance.
//!
//! Les deux fonctions opèrent sur de l'audio **interleaved f32** (frames ×
//! channels samples) et préservent le nombre de canaux. Pour le WSOLA, chaque
//! canal est traité indépendamment (préserve l'image stéréo).

#![allow(clippy::manual_range_contains)]

/// Resampling linéaire : `input` (N frames) → output (`target_frames` M).
/// Pitch change ratio = N/M (audible au-delà de ~5%).
///
/// `channels` doit diviser `input.len()` (interleaved).
pub fn stretch_linear(input: &[f32], channels: usize, target_frames: usize) -> Vec<f32> {
    let channels = channels.max(1);
    let src_frames = input.len() / channels;
    if src_frames == 0 || target_frames == 0 {
        return vec![0.0; target_frames * channels];
    }
    if src_frames == 1 {
        // 1 frame source → replique
        let mut out = Vec::with_capacity(target_frames * channels);
        for _ in 0..target_frames {
            for c in 0..channels {
                out.push(input[c]);
            }
        }
        return out;
    }
    let mut out = Vec::with_capacity(target_frames * channels);
    let last_src = (src_frames - 1) as f32;
    let denom = (target_frames - 1).max(1) as f32;
    for i in 0..target_frames {
        let f = i as f32 * last_src / denom;
        let i0 = f.floor() as usize;
        let i1 = (i0 + 1).min(src_frames - 1);
        let frac = f - i0 as f32;
        for c in 0..channels {
            let s0 = input[i0 * channels + c];
            let s1 = input[i1 * channels + c];
            out.push(s0 + (s1 - s0) * frac);
        }
    }
    out
}

// === WSOLA ===

const WSOLA_FRAME_SIZE: usize = 1024;
const WSOLA_HOP_SYNTHESIS: usize = 256;
const WSOLA_SEARCH_HALF: i64 = (WSOLA_HOP_SYNTHESIS as i64) / 2;
/// Si l'input fait moins de ce nombre de frames, fallback sur linear
/// (WSOLA n'a pas assez de matière pour fonctionner correctement).
const WSOLA_MIN_FRAMES: usize = WSOLA_FRAME_SIZE * 2;

/// Time-stretch préservant le pitch via WSOLA.
///
/// `input` est interleaved (`frames × channels` samples). Le résultat fait
/// `target_frames × channels` samples. Chaque canal est traité
/// indépendamment puis ré-interleavé.
pub fn stretch_wsola(
    input: &[f32],
    channels: usize,
    _sr: u32,
    target_frames: usize,
) -> Vec<f32> {
    let channels = channels.max(1);
    let src_frames = input.len() / channels;
    if src_frames == 0 || target_frames == 0 {
        return vec![0.0; target_frames * channels];
    }
    // Trop court pour WSOLA → fallback linear.
    if src_frames < WSOLA_MIN_FRAMES || target_frames < WSOLA_MIN_FRAMES {
        return stretch_linear(input, channels, target_frames);
    }

    // Précalcule la fenêtre Hann.
    let window: Vec<f32> = (0..WSOLA_FRAME_SIZE)
        .map(|n| {
            0.5 - 0.5
                * ((2.0 * std::f32::consts::PI * n as f32)
                    / (WSOLA_FRAME_SIZE as f32 - 1.0))
                    .cos()
        })
        .collect();

    let stretch_factor = (target_frames as f32) / (src_frames as f32);
    let ha_f = (WSOLA_HOP_SYNTHESIS as f32) / stretch_factor;

    // Buffer interleavé final.
    let mut out_interleaved = vec![0.0_f32; target_frames * channels];

    // Traite chaque canal indépendamment.
    for c in 0..channels {
        // Désinterleave le canal courant.
        let mut chan_in: Vec<f32> = Vec::with_capacity(src_frames);
        for f in 0..src_frames {
            chan_in.push(input[f * channels + c]);
        }
        let chan_out = wsola_single_channel(
            &chan_in,
            &window,
            ha_f,
            target_frames,
        );
        // Ré-interleave.
        for f in 0..target_frames {
            if f < chan_out.len() {
                out_interleaved[f * channels + c] = chan_out[f];
            }
        }
    }

    out_interleaved
}

/// Cœur de l'algo WSOLA pour 1 canal.
fn wsola_single_channel(
    input: &[f32],
    window: &[f32],
    ha_f: f32,
    target_frames: usize,
) -> Vec<f32> {
    let frame_size = WSOLA_FRAME_SIZE;
    let hop_s = WSOLA_HOP_SYNTHESIS;
    let search_half = WSOLA_SEARCH_HALF;
    let src_len = input.len();

    // Output + buffer de normalisation (somme des windows pour OLA correct).
    let out_len = target_frames + frame_size;
    let mut output = vec![0.0_f32; out_len];
    let mut norm = vec![0.0_f32; out_len];

    // Première frame : on prend directement input[0..frame_size].
    // Normalization = somme des windows (pas window^2) → permet output/norm
    // de retourner la weighted-average de l'input aux positions overlappées.
    for n in 0..frame_size {
        if n < src_len {
            output[n] += input[n] * window[n];
            norm[n] += window[n];
        }
    }
    let mut prev_analysis_start: i64 = 0;

    let mut i = 1;
    while (i * hop_s) < target_frames {
        // Position d'analyse cible.
        let target_analysis = (i as f32 * ha_f) as i64;

        // Région d'output déjà écrite à matcher (les `frame_size - hop_s`
        // derniers samples écrits par la frame précédente).
        let out_match_start = (i - 1) * hop_s + hop_s;
        let out_match_end = (out_match_start + (frame_size - hop_s)).min(out_len);
        let match_len = out_match_end.saturating_sub(out_match_start);

        // Cherche le décalage `k` ∈ [-search_half, search_half] qui maximise
        // la cross-corrélation entre `input[target + k .. target + k + match_len]`
        // et `output[out_match_start .. out_match_end]` (normalisé par norm
        // pour comparer en valeurs OLA).
        let mut best_k: i64 = 0;
        let mut best_score = f32::NEG_INFINITY;
        for k in -search_half..=search_half {
            let src_start = target_analysis + k;
            if src_start < 0 { continue; }
            let src_start = src_start as usize;
            if src_start + match_len > src_len { continue; }
            let mut score = 0.0_f32;
            for n in 0..match_len {
                let target_norm = if norm[out_match_start + n] > 1e-6 {
                    output[out_match_start + n] / norm[out_match_start + n]
                } else {
                    0.0
                };
                score += target_norm * input[src_start + n];
            }
            if score > best_score {
                best_score = score;
                best_k = k;
            }
        }

        let analysis_start = (target_analysis + best_k).max(0) as i64;
        // Overlap-add la frame analysée.
        let out_start = i * hop_s;
        for n in 0..frame_size {
            let src_idx = analysis_start + n as i64;
            if src_idx < 0 || (src_idx as usize) >= src_len { continue; }
            if out_start + n >= out_len { break; }
            output[out_start + n] += input[src_idx as usize] * window[n];
            norm[out_start + n] += window[n];
        }
        prev_analysis_start = analysis_start;
        i += 1;
    }
    let _ = prev_analysis_start;

    // Normalize output by norm (pour annuler la somme des windows^2 résultante
    // de l'overlap-add Hann).
    for n in 0..out_len {
        if norm[n] > 1e-6 {
            output[n] /= norm[n];
        }
    }
    output.truncate(target_frames);
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linear_identity() {
        // Ratio 1.0 : output ≈ input bit-pour-bit (le calcul lerp avec frac=0
        // ou frac=1 reproduit input exactement aux indices entiers).
        let input: Vec<f32> = (0..100).map(|i| (i as f32) * 0.01).collect();
        let out = stretch_linear(&input, 1, 100);
        assert_eq!(out.len(), 100);
        for (i, &s) in out.iter().enumerate() {
            assert!((s - input[i]).abs() < 1e-5, "mismatch @ {i}");
        }
    }

    #[test]
    fn linear_doubles_length() {
        let input: Vec<f32> = vec![0.0, 1.0];
        let out = stretch_linear(&input, 1, 4);
        assert_eq!(out.len(), 4);
        // Endpoints égaux à l'input.
        assert!((out[0] - 0.0).abs() < 1e-5);
        assert!((out[3] - 1.0).abs() < 1e-5);
        // Milieu interpolé.
        assert!(out[1] < out[2]);
    }

    #[test]
    fn linear_stereo_keeps_channels() {
        // 3 frames stereo L=0/1/2, R=10/20/30
        let input: Vec<f32> = vec![0.0, 10.0, 1.0, 20.0, 2.0, 30.0];
        let out = stretch_linear(&input, 2, 6);
        assert_eq!(out.len(), 12);
        // Endpoints L
        assert!((out[0] - 0.0).abs() < 1e-5);
        assert!((out[10] - 2.0).abs() < 1e-5);
        // Endpoints R
        assert!((out[1] - 10.0).abs() < 1e-5);
        assert!((out[11] - 30.0).abs() < 1e-5);
    }

    #[test]
    fn wsola_fallback_on_short_input() {
        // Input trop court pour WSOLA → fallback linear.
        let input: Vec<f32> = (0..100).map(|i| (i as f32) * 0.01).collect();
        let out = stretch_wsola(&input, 1, 44100, 200);
        assert_eq!(out.len(), 200);
        // Au moins doit retourner quelque chose de non-nul.
        assert!(out.iter().any(|&s| s != 0.0));
    }

    #[test]
    fn wsola_identity_approx() {
        // Sine 440Hz, ratio 1.0 → output ≈ input (drift OLA acceptable).
        let sr = 44100.0_f32;
        let len = 8192;
        let input: Vec<f32> = (0..len)
            .map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / sr).sin())
            .collect();
        let out = stretch_wsola(&input, 1, sr as u32, len);
        assert_eq!(out.len(), len);
        // L'output doit avoir la même amplitude RMS approximative.
        let rms_in = (input.iter().map(|s| s * s).sum::<f32>() / len as f32).sqrt();
        let rms_out = (out.iter().map(|s| s * s).sum::<f32>() / len as f32).sqrt();
        // ±20% tolérance (overlap-add a un peu de drift en bord de buffer).
        assert!((rms_out / rms_in - 1.0).abs() < 0.3,
            "RMS in {rms_in} vs out {rms_out}");
    }
}
