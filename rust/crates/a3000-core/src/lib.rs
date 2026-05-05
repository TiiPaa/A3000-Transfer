//! `a3000-core` — primitives SCSI/SMDI/WAV/MIDI sans UI ni audio playback.
//!
//! Crate sans dépendance vers le runtime GUI ou audio. Doit pouvoir compiler
//! et tester en isolation, c'est le filet de sécurité pour la conversion
//! Python → Rust.
//!
//! Référence Python : `python/a3000_transfer/{scsi_passthrough,smdi,transfer,wav_reader}.py`
//! et `python/a3000_transfer/slicer/view.py:_generate_midi_temp` (pour midi.rs).

pub mod smdi;
pub mod wav;
pub mod midi;

#[cfg(windows)]
pub mod scsi;
#[cfg(windows)]
pub mod transfer;
