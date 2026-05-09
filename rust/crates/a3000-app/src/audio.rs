//! Lecture audio via cpal — partagée entre Slicer (preview slice + Loop)
//! et Upload (preview WAV avant transfert).
//!
//! `cpal::Stream` est `!Send` sur Windows : la struct vit sur le thread GUI.
//! Position partagée via `AtomicU64` lock-free pour que la GUI puisse lire
//! la playhead sans bloquer le callback audio.
//!
//! Resampling linéaire fixed-point 32.32 du SR source vers le SR du device :
//! la hauteur reste correcte même si le device tourne en 48k et le sample
//! en 44.1k.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use a3000_core::wav::WavePayload;

const FIX_SHIFT: u64 = 32;

pub struct Playback {
    /// Le stream est dropped pour stop.
    _stream: cpal::Stream,
    /// Position fixed-point 32.32 dans le buffer source (en samples × 2^32).
    position_fixed: Arc<AtomicU64>,
    /// Longueur du buffer source (samples).
    audio_len: usize,
}

impl Playback {
    /// Lecture en boucle (utilisée par le bouton Loop full audio).
    pub fn start_loop(audio_mono: Vec<f32>, source_sr: u32) -> Result<Self, anyhow::Error> {
        Self::start_inner(audio_mono, source_sr, true)
    }

    /// Lecture one-shot (silence après la fin) — preview slice ou item Upload.
    pub fn start_oneshot(audio_mono: Vec<f32>, source_sr: u32) -> Result<Self, anyhow::Error> {
        Self::start_inner(audio_mono, source_sr, false)
    }

    fn start_inner(audio_mono: Vec<f32>, source_sr: u32, looping: bool) -> Result<Self, anyhow::Error> {
        let host = cpal::default_host();
        let device = host.default_output_device()
            .ok_or_else(|| anyhow::anyhow!("Aucun device de sortie audio par défaut"))?;
        let supported = device.default_output_config()
            .map_err(|e| anyhow::anyhow!("default_output_config: {e}"))?;
        let n_channels = supported.channels() as usize;
        let device_sr = supported.sample_rate().0;
        let sample_format = supported.sample_format();
        let stream_config: cpal::StreamConfig = supported.into();

        let audio_len = audio_mono.len();
        let audio = Arc::new(audio_mono);
        let position_fixed = Arc::new(AtomicU64::new(0));

        let step: u64 = ((source_sr as u64) << FIX_SHIFT) / device_sr.max(1) as u64;

        let audio_clone = Arc::clone(&audio);
        let pos_clone = Arc::clone(&position_fixed);
        let total_fixed = (audio_len as u64).checked_shl(FIX_SHIFT as u32).unwrap_or(u64::MAX);

        let err_fn = |err| eprintln!("cpal stream err: {err}");

        let stream = match sample_format {
            cpal::SampleFormat::F32 => device.build_output_stream(
                &stream_config,
                move |output: &mut [f32], _info: &cpal::OutputCallbackInfo| {
                    if audio_clone.is_empty() || total_fixed == 0 {
                        output.fill(0.0);
                        return;
                    }
                    let n_frames = output.len() / n_channels.max(1);
                    let mut pos = pos_clone.load(Ordering::Relaxed);
                    let mask = (1u64 << FIX_SHIFT) - 1;
                    let inv_one: f32 = 1.0 / ((1u64 << FIX_SHIFT) as f64) as f32;
                    for f in 0..n_frames {
                        if !looping && pos >= total_fixed {
                            for c in 0..n_channels {
                                output[f * n_channels + c] = 0.0;
                            }
                            continue;
                        }
                        let i0 = (pos >> FIX_SHIFT) as usize;
                        let frac = (pos & mask) as f32 * inv_one;
                        let i1 = (i0 + 1) % audio_len;
                        let s = audio_clone[i0] * (1.0 - frac) + audio_clone[i1] * frac;
                        for c in 0..n_channels {
                            output[f * n_channels + c] = s;
                        }
                        pos = pos.wrapping_add(step);
                        if looping && pos >= total_fixed { pos -= total_fixed; }
                    }
                    pos_clone.store(pos, Ordering::Relaxed);
                },
                err_fn,
                None,
            )?,
            cpal::SampleFormat::I16 => device.build_output_stream(
                &stream_config,
                move |output: &mut [i16], _info: &cpal::OutputCallbackInfo| {
                    if audio_clone.is_empty() || total_fixed == 0 {
                        output.fill(0);
                        return;
                    }
                    let n_frames = output.len() / n_channels.max(1);
                    let mut pos = pos_clone.load(Ordering::Relaxed);
                    let mask = (1u64 << FIX_SHIFT) - 1;
                    let inv_one: f32 = 1.0 / ((1u64 << FIX_SHIFT) as f64) as f32;
                    for f in 0..n_frames {
                        if !looping && pos >= total_fixed {
                            for c in 0..n_channels {
                                output[f * n_channels + c] = 0;
                            }
                            continue;
                        }
                        let i0 = (pos >> FIX_SHIFT) as usize;
                        let frac = (pos & mask) as f32 * inv_one;
                        let i1 = (i0 + 1) % audio_len;
                        let s = audio_clone[i0] * (1.0 - frac) + audio_clone[i1] * frac;
                        let s16 = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
                        for c in 0..n_channels {
                            output[f * n_channels + c] = s16;
                        }
                        pos = pos.wrapping_add(step);
                        if looping && pos >= total_fixed { pos -= total_fixed; }
                    }
                    pos_clone.store(pos, Ordering::Relaxed);
                },
                err_fn,
                None,
            )?,
            other => anyhow::bail!("Format audio non supporté : {:?}", other),
        };
        stream.play()?;

        Ok(Self {
            _stream: stream,
            position_fixed,
            audio_len,
        })
    }

    /// Position de lecture courante en fraction [0, 1] du buffer.
    pub fn position_fraction(&self) -> f32 {
        if self.audio_len == 0 { return 0.0; }
        let pos = self.position_fixed.load(Ordering::Relaxed);
        let pos_int = (pos >> FIX_SHIFT) as usize;
        (pos_int as f32 / self.audio_len as f32).clamp(0.0, 1.0)
    }
}

/// Conversion PCM 16-bit LE interleaved → mono f32 normalisé [-1, 1].
/// Partagée entre Slicer (drop WAV → preview/découpe) et Upload (preview
/// d'un item de la queue).
pub fn pcm16_le_to_mono_f32(payload: &WavePayload) -> Vec<f32> {
    let channels = payload.channels.max(1) as usize;
    let frames = payload.frame_count as usize;
    let inv_max = 1.0 / 32768.0_f32;
    let inv_n = 1.0 / channels as f32;
    let mut mono = Vec::with_capacity(frames);
    let data = &payload.pcm_data;
    for f in 0..frames {
        let mut sum = 0.0_f32;
        for c in 0..channels {
            let off = (f * channels + c) * 2;
            if off + 1 < data.len() {
                let s = i16::from_le_bytes([data[off], data[off + 1]]);
                sum += (s as f32) * inv_max;
            }
        }
        mono.push(sum * inv_n);
    }
    mono
}
