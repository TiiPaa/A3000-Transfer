//! Génération MIDI Standard File (SMF) chromatique pour l'export slicer.
//!
//! Référence Python : `python/a3000_transfer/slicer/view.py:_generate_midi_temp`
//! Lib : `midly` pour SMF write.
//!
//! 1 note par slice, ascendante depuis C2 (note 36), tempo BPM = N_beats × 60 / total_duration_sec.
//! PPQ = 480.

// TODO Phase 1 : generate_midi(onsets, sr, duration_sec, n_beats) -> Vec<u8>
