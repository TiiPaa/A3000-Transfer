//! Lecture WAV multi-format avec conversion vers PCM 16-bit + dither TPDF.
//!
//! Référence Python : `python/a3000_transfer/wav_reader.py`
//! Lib : `hound` pour le decode multi-format, `rand_chacha` pour le PRNG du dither.
//!
//! # Path "PCM_16 input"
//! Lecture directe sans dither. Bytes PCM 16-bit LE inchangés du fichier source
//! (sauf endianness — tous les WAV PCM sont LE par spec). Bit-à-bit avec Python.
//!
//! # Path "non-PCM_16 input" (8/24/32-bit int + 32-bit float)
//! Décodage en f32 normalisé [-1.0, 1.0], dither TPDF (somme de 2 uniformes
//! ±0.5 LSB), quantification i16 avec clipping. Le PRNG diffère de Python
//! (NumPy PCG64 vs notre ChaCha8) → pas bit-à-bit, mais même range et même
//! caractère psycho-acoustique. Cf. `docs/conversion/DECISIONS.md`.

use std::path::Path;

use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum WaveError {
    #[error("File not found: {0}")]
    NotFound(String),
    #[error("Cannot read WAV: {0}")]
    Read(#[from] hound::Error),
    #[error("Unsupported channels: {0} (only mono or stereo)")]
    UnsupportedChannels(u16),
    #[error("Unsupported format: {bits}-bit {format} (expected 8/16/24/32 int or 32-bit float)")]
    UnsupportedFormat { bits: u16, format: &'static str },
    #[error("Empty WAV file")]
    Empty,
}

/// Métadonnées WAV lues sans charger les données PCM.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WaveMetadata {
    pub channels: u16,
    pub sample_rate: u32,
    pub bits_per_sample: u16,
    pub frame_count: u64,
    /// Taille de la sortie 16-bit LE après conversion (frames × channels × 2).
    pub byte_count: u64,
}

impl WaveMetadata {
    #[must_use]
    pub fn duration_s(&self) -> f64 {
        if self.sample_rate == 0 {
            0.0
        } else {
            self.frame_count as f64 / f64::from(self.sample_rate)
        }
    }
}

/// Données PCM 16-bit LE prêtes à envoyer au sampler.
#[derive(Debug, Clone)]
pub struct WavePayload {
    pub channels: u16,
    pub sample_rate: u32,
    pub frame_count: u64,
    /// Octets PCM 16-bit signed little-endian, frames interleaved.
    pub pcm_data: Vec<u8>,
}

/// Lit uniquement le header WAV. Très rapide, indépendant de la taille.
///
/// # Errors
/// `NotFound`, `Read` (parse error), `UnsupportedChannels`, `UnsupportedFormat`, `Empty`.
pub fn peek_wave_metadata(path: &Path) -> Result<WaveMetadata, WaveError> {
    if !path.exists() {
        return Err(WaveError::NotFound(path.display().to_string()));
    }
    let reader = hound::WavReader::open(path)?;
    let spec = reader.spec();
    validate_spec(&spec)?;
    let frame_count = u64::from(reader.duration());
    if frame_count == 0 {
        return Err(WaveError::Empty);
    }
    Ok(WaveMetadata {
        channels: spec.channels,
        sample_rate: spec.sample_rate,
        bits_per_sample: spec.bits_per_sample,
        frame_count,
        byte_count: frame_count * u64::from(spec.channels) * 2,
    })
}

/// Charge un WAV et retourne du PCM 16-bit LE (avec TPDF dither pour les conversions).
///
/// PRNG : ChaCha8 seedé par l'OS (`rand::thread_rng()`-équivalent, time-based).
///
/// # Errors
/// Voir `WaveError`.
pub fn load_wave(path: &Path) -> Result<WavePayload, WaveError> {
    let mut rng = ChaCha8Rng::from_entropy();
    load_wave_with_rng(path, &mut rng)
}

/// Variante avec PRNG injecté — utile pour les tests reproductibles.
///
/// # Errors
/// Voir `WaveError`.
pub fn load_wave_with_rng<R: Rng>(path: &Path, rng: &mut R) -> Result<WavePayload, WaveError> {
    if !path.exists() {
        return Err(WaveError::NotFound(path.display().to_string()));
    }
    let mut reader = hound::WavReader::open(path)?;
    let spec = reader.spec();
    validate_spec(&spec)?;
    let frame_count = u64::from(reader.duration());
    if frame_count == 0 {
        return Err(WaveError::Empty);
    }

    let pcm_data: Vec<u8> = match (spec.sample_format, spec.bits_per_sample) {
        (hound::SampleFormat::Int, 16) => {
            // Lecture directe int16, pas de dither. Hound retourne les samples
            // dans l'ordre d'écriture (interleaved frames) ; on les remet en LE bytes.
            let samples: Result<Vec<i16>, _> = reader.samples::<i16>().collect();
            let samples = samples?;
            let mut bytes = Vec::with_capacity(samples.len() * 2);
            for s in samples {
                bytes.extend_from_slice(&s.to_le_bytes());
            }
            bytes
        }
        (hound::SampleFormat::Int, bits @ (8 | 24 | 32)) => {
            let samples: Result<Vec<i32>, _> = reader.samples::<i32>().collect();
            let samples = samples?;
            // Normalize en f32 [-1.0, 1.0] selon le bit depth, dither, quantize.
            let max_abs = match bits {
                8 => 128.0_f32,
                24 => 8_388_608.0_f32,
                32 => 2_147_483_648.0_f32,
                _ => unreachable!("bits validated by validate_spec"),
            };
            dither_and_quantize_int(&samples, max_abs, rng)
        }
        (hound::SampleFormat::Float, 32) => {
            let samples: Result<Vec<f32>, _> = reader.samples::<f32>().collect();
            let samples = samples?;
            dither_and_quantize_f32(&samples, rng)
        }
        (fmt, bits) => {
            let format_str = if fmt == hound::SampleFormat::Float { "float" } else { "int" };
            return Err(WaveError::UnsupportedFormat { bits, format: format_str });
        }
    };

    Ok(WavePayload {
        channels: spec.channels,
        sample_rate: spec.sample_rate,
        frame_count,
        pcm_data,
    })
}

fn validate_spec(spec: &hound::WavSpec) -> Result<(), WaveError> {
    if spec.channels != 1 && spec.channels != 2 {
        return Err(WaveError::UnsupportedChannels(spec.channels));
    }
    let format = if spec.sample_format == hound::SampleFormat::Float { "float" } else { "int" };
    let bits = spec.bits_per_sample;
    let supported = matches!(
        (spec.sample_format, bits),
        (hound::SampleFormat::Int, 8 | 16 | 24 | 32) | (hound::SampleFormat::Float, 32)
    );
    if !supported {
        return Err(WaveError::UnsupportedFormat { bits, format });
    }
    Ok(())
}

/// TPDF dither + quantize pour échantillons entiers.
fn dither_and_quantize_int<R: Rng>(samples: &[i32], max_abs: f32, rng: &mut R) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(samples.len() * 2);
    for &s in samples {
        let normalized = s as f32 / max_abs;
        let v = quantize_one(normalized, rng);
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    bytes
}

/// TPDF dither + quantize pour échantillons f32 déjà normalisés [-1, 1].
fn dither_and_quantize_f32<R: Rng>(samples: &[f32], rng: &mut R) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(samples.len() * 2);
    for &s in samples {
        let v = quantize_one(s, rng);
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    bytes
}

#[inline]
fn quantize_one<R: Rng>(s: f32, rng: &mut R) -> i16 {
    // TPDF dither = somme de 2 uniformes ±0.5 LSB → ±1 LSB total
    let d1: f32 = rng.gen();
    let d2: f32 = rng.gen();
    let dithered = s + (d1 - d2) / 32768.0;
    let scaled = dithered * 32767.0;
    let clamped = scaled.clamp(-32768.0, 32767.0);
    clamped as i16
}

// ─── Tests unitaires ──────────────────────────────────────────────────────

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use hound::{SampleFormat, WavSpec, WavWriter};
    use tempfile::TempDir;

    fn write_test_wav(dir: &Path, name: &str, spec: WavSpec, samples: &[i16]) {
        let path = dir.join(name);
        let mut w = WavWriter::create(&path, spec).unwrap();
        for &s in samples {
            w.write_sample(s).unwrap();
        }
        w.finalize().unwrap();
    }

    #[test]
    fn peek_metadata_pcm16_mono() {
        let tmp = TempDir::new().unwrap();
        let spec = WavSpec {
            channels: 1,
            sample_rate: 44100,
            bits_per_sample: 16,
            sample_format: SampleFormat::Int,
        };
        write_test_wav(tmp.path(), "test.wav", spec, &[0, 1000, -1000, 32767, -32768]);
        let meta = peek_wave_metadata(&tmp.path().join("test.wav")).unwrap();
        assert_eq!(meta.channels, 1);
        assert_eq!(meta.sample_rate, 44100);
        assert_eq!(meta.bits_per_sample, 16);
        assert_eq!(meta.frame_count, 5);
        assert_eq!(meta.byte_count, 10);
    }

    #[test]
    fn load_pcm16_passthrough() {
        let tmp = TempDir::new().unwrap();
        let spec = WavSpec {
            channels: 1,
            sample_rate: 44100,
            bits_per_sample: 16,
            sample_format: SampleFormat::Int,
        };
        let samples = [0i16, 1000, -1000, 32767, -32768];
        write_test_wav(tmp.path(), "test.wav", spec, &samples);
        let payload = load_wave(&tmp.path().join("test.wav")).unwrap();
        // Bytes attendus = LE des samples
        let expected: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        assert_eq!(payload.pcm_data, expected);
    }

    #[test]
    fn unsupported_channels_rejected() {
        let tmp = TempDir::new().unwrap();
        let spec = WavSpec {
            channels: 3,
            sample_rate: 44100,
            bits_per_sample: 16,
            sample_format: SampleFormat::Int,
        };
        let path = tmp.path().join("3ch.wav");
        let mut w = WavWriter::create(&path, spec).unwrap();
        w.write_sample(0i16).unwrap();
        w.write_sample(0i16).unwrap();
        w.write_sample(0i16).unwrap();
        w.finalize().unwrap();
        let r = peek_wave_metadata(&path);
        assert!(matches!(r, Err(WaveError::UnsupportedChannels(3))));
    }
}
