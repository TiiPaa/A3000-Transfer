//! Génération MIDI Standard File (SMF) chromatique pour l'export slicer.
//!
//! Référence Python : `python/a3000_transfer/slicer/view.py:_generate_midi_temp`
//! Lib : `midly` (parse + write SMF).
//!
//! Algorithme :
//! - 1 note par slice, ascendante depuis C2 (note 36), capée à 127
//! - Tempo BPM = N_beats × 60 / total_duration_sec → DAW aligne sur sa grille
//!   de beats à l'import (sauf cas total_samples=0 → fallback 120 BPM)
//! - PPQ = 480
//! - Velocity 100, channel 0
//! - Track meta : set_tempo + track_name (32 chars max)

use midly::num::{u15, u24, u28, u4, u7};
use midly::{Format, Header, MetaMessage, MidiMessage, Smf, Timing, TrackEvent, TrackEventKind};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum MidiError {
    #[error("Empty onsets list — at least one slice required")]
    EmptyOnsets,
    #[error("Failed to write SMF bytes: {0}")]
    Write(String),
}

/// Génère un fichier MIDI Standard File (Type 1) à partir d'une liste d'onsets.
///
/// # Arguments
/// - `onsets` : indices d'échantillons de début de chaque slice (ordonnés)
/// - `total_samples` : longueur totale du WAV en échantillons (pour la fin
///   de la dernière slice). Si `0`, BPM par défaut 120 (pas de tempo-sync).
/// - `sr` : sample rate du WAV
/// - `n_beats` : nombre de beats voulu (calcule le tempo). 0 → 1.
/// - `track_name` : nom de la track MIDI (tronqué à 32 chars)
///
/// # Returns
/// Bytes du fichier SMF prêts à écrire sur disque ou drag-drop.
///
/// # Errors
/// `EmptyOnsets`, `Write`.
pub fn generate_midi(
    onsets: &[i64],
    total_samples: u64,
    sr: u32,
    n_beats: u32,
    track_name: &str,
) -> Result<Vec<u8>, MidiError> {
    if onsets.is_empty() {
        return Err(MidiError::EmptyOnsets);
    }
    let n_beats = n_beats.max(1);
    let total_dur_sec = if total_samples > 0 && sr > 0 {
        total_samples as f64 / f64::from(sr)
    } else {
        0.0
    };
    let bpm = if total_dur_sec > 0.0 {
        f64::from(n_beats) * 60.0 / total_dur_sec
    } else {
        120.0
    };
    // mido.bpm2tempo : int(round(60_000_000 / bpm))
    let tempo_us = ((60_000_000.0 / bpm).round() as u32).min(0x00FF_FFFF);

    let ppq: u16 = 480;
    let header = Header::new(Format::Parallel, Timing::Metrical(u15::from(ppq)));

    // Tronque le track name à 32 chars (matche [:32] côté Python sur str)
    let name: String = track_name.chars().take(32).collect();
    let name_bytes: &[u8] = name.as_bytes();

    let mut track: Vec<TrackEvent<'_>> = Vec::with_capacity(3 + onsets.len() * 2);

    // Meta : set_tempo
    track.push(TrackEvent {
        delta: u28::from(0_u32),
        kind: TrackEventKind::Meta(MetaMessage::Tempo(u24::from(tempo_us))),
    });
    // Meta : track_name
    track.push(TrackEvent {
        delta: u28::from(0_u32),
        kind: TrackEventKind::Meta(MetaMessage::TrackName(name_bytes)),
    });

    // sec → ticks (matche Python : int(round(s * bpm / 60 * ppq)))
    let sec_to_ticks = |s: f64| -> u32 { (s * bpm / 60.0 * f64::from(ppq)).round() as u32 };

    let base_note: u8 = 36; // C2
    let mut last_tick: u32 = 0;
    let n = onsets.len();
    for (i, &onset) in onsets.iter().enumerate() {
        let start_sec = onset as f64 / f64::from(sr);
        let end_sample: u64 = if i + 1 < n {
            onsets[i + 1] as u64
        } else {
            total_samples
        };
        let end_sec = end_sample as f64 / f64::from(sr);
        let start_tick = sec_to_ticks(start_sec);
        let end_tick = sec_to_ticks(end_sec);
        let note = u8::try_from(base_note as usize + i).unwrap_or(127).min(127);
        let delta_on = start_tick.saturating_sub(last_tick);
        let delta_off = end_tick.saturating_sub(start_tick).max(1);

        track.push(TrackEvent {
            delta: u28::from(delta_on),
            kind: TrackEventKind::Midi {
                channel: u4::from(0),
                message: MidiMessage::NoteOn { key: u7::from(note), vel: u7::from(100) },
            },
        });
        track.push(TrackEvent {
            delta: u28::from(delta_off),
            kind: TrackEventKind::Midi {
                channel: u4::from(0),
                message: MidiMessage::NoteOff { key: u7::from(note), vel: u7::from(0) },
            },
        });
        last_tick = end_tick;
    }

    // End of track (obligatoire en SMF)
    track.push(TrackEvent {
        delta: u28::from(0_u32),
        kind: TrackEventKind::Meta(MetaMessage::EndOfTrack),
    });

    let mut smf = Smf::new(header);
    smf.tracks.push(track);

    let mut buf = Vec::new();
    smf.write_std(&mut buf).map_err(|e| MidiError::Write(e.to_string()))?;
    Ok(buf)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty_onsets() {
        let r = generate_midi(&[], 100, 44100, 16, "test");
        assert!(matches!(r, Err(MidiError::EmptyOnsets)));
    }

    #[test]
    fn produces_valid_smf_header() {
        let bytes = generate_midi(&[0, 22050, 44100], 88200, 44100, 4, "test").unwrap();
        // SMF type 1 commence par "MThd" + length=6
        assert_eq!(&bytes[0..4], b"MThd");
        assert_eq!(&bytes[4..8], &[0, 0, 0, 6]); // length
        assert_eq!(&bytes[8..10], &[0, 1]); // format = 1
        assert_eq!(&bytes[10..12], &[0, 1]); // ntrks = 1
        assert_eq!(&bytes[12..14], &[0x01, 0xE0]); // ppq = 480
        // "MTrk" suit
        assert_eq!(&bytes[14..18], b"MTrk");
    }

    #[test]
    fn track_name_truncated_to_32_chars() {
        let long_name = "abcdefghijklmnopqrstuvwxyz0123456789"; // 36 chars
        let bytes = generate_midi(&[0], 100000, 44100, 16, long_name).unwrap();
        // Le nom tronqué doit apparaitre dans les bytes
        let truncated = &long_name[..32];
        let found = bytes.windows(truncated.len()).any(|w| w == truncated.as_bytes());
        assert!(found, "track name truncated to 32 chars should be in SMF");
    }

    #[test]
    fn fallback_bpm_120_when_total_samples_zero() {
        let b1 = generate_midi(&[0, 1000], 0, 44100, 16, "x").unwrap();
        let b2 = generate_midi(&[0, 1000], 0, 44100, 8, "x").unwrap(); // n_beats ignoré
        // Les deux doivent encoder le même tempo (120 BPM = 500_000 us/beat)
        // → bytes identiques (sauf timestamps qui ne dépendent pas de n_beats ici)
        assert_eq!(b1, b2);
    }
}
