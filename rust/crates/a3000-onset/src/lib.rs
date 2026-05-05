//! Détection de transients — port pur Rust de `librosa.onset.onset_detect`.
//!
//! Algo : STFT → Mel spectrogram → Spectral flux → Peak picking → Backtrack.
//! Référence Python : `librosa/onset.py:onset_strength_multi` + `onset_detect`
//! et notre wrapper `python/a3000_transfer/slicer/engine.py:detect_transients`.
//!
//! Validation A/B obligatoire : tolérance ≤1 frame d'écart sur ≥95% des cas
//! test. Cf. `docs/conversion/oracles/golden/onset/`.

pub mod stft;
pub mod mel;
pub mod flux;
pub mod peak;
pub mod backtrack;

// TODO Phase 2 : API publique
// pub fn detect_transients(
//     y: &[f32], sr: u32,
//     sensitivity: f32, min_gap_ms: f32, snap_ms: f32,
// ) -> Vec<i64>
