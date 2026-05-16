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
//!
//! Buffer source en **interleaved f32** (`audio[frame * src_ch + c]`) avec
//! routage vers les canaux du device :
//!   - source mono   → réplique src[0] sur tous les canaux du device
//!   - source stéréo + 1 canal device → downmix (L+R)/2
//!   - source stéréo + ≥2 canaux device → L=ch0, R=ch1, autres canaux = 0

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use a3000_core::wav::WavePayload;

const FIX_SHIFT: u64 = 32;

pub struct Playback {
    /// Le stream est dropped pour stop.
    _stream: cpal::Stream,
    /// Position fixed-point 32.32 dans le buffer source (en **frames** × 2^32).
    position_fixed: Arc<AtomicU64>,
    /// Nombre de frames source.
    frame_count: usize,
}

impl Playback {
    /// Lecture en boucle (utilisée par le bouton Loop full audio).
    /// `audio` est interleaved (frames × src_channels samples), `src_channels`
    /// indique combien de samples par frame.
    pub fn start_loop(audio: Vec<f32>, source_sr: u32, src_channels: u16) -> Result<Self, anyhow::Error> {
        Self::start_inner(audio, source_sr, src_channels, true)
    }

    /// Lecture one-shot (silence après la fin) — preview slice ou item Upload.
    pub fn start_oneshot(audio: Vec<f32>, source_sr: u32, src_channels: u16) -> Result<Self, anyhow::Error> {
        Self::start_inner(audio, source_sr, src_channels, false)
    }

    fn start_inner(audio: Vec<f32>, source_sr: u32, src_channels: u16, looping: bool) -> Result<Self, anyhow::Error> {
        let host = cpal::default_host();
        let device = host.default_output_device()
            .ok_or_else(|| anyhow::anyhow!("Aucun device de sortie audio par défaut"))?;
        let supported = device.default_output_config()
            .map_err(|e| anyhow::anyhow!("default_output_config: {e}"))?;
        let n_channels = supported.channels() as usize;
        let device_sr = supported.sample_rate().0;
        let sample_format = supported.sample_format();
        let stream_config: cpal::StreamConfig = supported.into();

        let src_ch = usize::from(src_channels.max(1));
        let frame_count = audio.len() / src_ch;
        let audio = Arc::new(audio);
        let position_fixed = Arc::new(AtomicU64::new(0));

        let step: u64 = ((source_sr as u64) << FIX_SHIFT) / device_sr.max(1) as u64;

        let audio_clone = Arc::clone(&audio);
        let pos_clone = Arc::clone(&position_fixed);
        let total_fixed = (frame_count as u64).checked_shl(FIX_SHIFT as u32).unwrap_or(u64::MAX);

        let err_fn = |err| eprintln!("cpal stream err: {err}");

        // Récupère 2 frames adjacents interpolés depuis le buffer interleaved
        // et écrit `n_channels` samples dans `dst` en suivant la stratégie de
        // routage. Retourne le sample[0] pour l'écriture i16 (idem boucle f32).
        let sample_one = move |audio: &[f32], frame_count: usize, src_ch: usize,
                               pos: u64, mask: u64, inv_one: f32| -> [f32; 2] {
            let i0 = (pos >> FIX_SHIFT) as usize;
            let i1 = (i0 + 1) % frame_count.max(1);
            let frac = (pos & mask) as f32 * inv_one;
            let base0 = i0 * src_ch;
            let base1 = i1 * src_ch;
            let l = audio[base0] * (1.0 - frac) + audio[base1] * frac;
            let r = if src_ch >= 2 {
                audio[base0 + 1] * (1.0 - frac) + audio[base1 + 1] * frac
            } else {
                l
            };
            [l, r]
        };

        let stream = match sample_format {
            cpal::SampleFormat::F32 => device.build_output_stream(
                &stream_config,
                move |output: &mut [f32], _info: &cpal::OutputCallbackInfo| {
                    if audio_clone.is_empty() || total_fixed == 0 || frame_count == 0 {
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
                        let [l, r] = sample_one(&audio_clone, frame_count, src_ch, pos, mask, inv_one);
                        route(&mut output[f * n_channels..f * n_channels + n_channels], l, r, src_ch);
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
                    if audio_clone.is_empty() || total_fixed == 0 || frame_count == 0 {
                        output.fill(0);
                        return;
                    }
                    let n_frames = output.len() / n_channels.max(1);
                    let mut pos = pos_clone.load(Ordering::Relaxed);
                    let mask = (1u64 << FIX_SHIFT) - 1;
                    let inv_one: f32 = 1.0 / ((1u64 << FIX_SHIFT) as f64) as f32;
                    let mut tmp = vec![0.0f32; n_channels];
                    for f in 0..n_frames {
                        if !looping && pos >= total_fixed {
                            for c in 0..n_channels {
                                output[f * n_channels + c] = 0;
                            }
                            continue;
                        }
                        let [l, r] = sample_one(&audio_clone, frame_count, src_ch, pos, mask, inv_one);
                        route(&mut tmp, l, r, src_ch);
                        for c in 0..n_channels {
                            output[f * n_channels + c] = (tmp[c].clamp(-1.0, 1.0) * 32767.0) as i16;
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
            frame_count,
        })
    }

    /// Position de lecture courante en fraction [0, 1] du buffer.
    pub fn position_fraction(&self) -> f32 {
        if self.frame_count == 0 { return 0.0; }
        let pos = self.position_fixed.load(Ordering::Relaxed);
        let pos_int = (pos >> FIX_SHIFT) as usize;
        (pos_int as f32 / self.frame_count as f32).clamp(0.0, 1.0)
    }
}

/// Route une frame interpolée (L, R) vers les canaux d'un device.
///   - dst.len() == 1 (device mono) : L+R sommés et moyennés (downmix)
///   - dst.len() >= 2 : ch0=L, ch1=R, autres canaux = 0 (pas de bleed surround)
/// Pour une source mono, R == L (cf. `sample_one`) — la stratégie ci-dessus
/// revient alors à dupliquer L sur ch0/ch1 et 0 ailleurs.
#[inline]
fn route(dst: &mut [f32], l: f32, r: f32, src_ch: usize) {
    match dst.len() {
        0 => {}
        1 => {
            dst[0] = if src_ch >= 2 { (l + r) * 0.5 } else { l };
        }
        _ => {
            dst[0] = l;
            dst[1] = if src_ch >= 2 { r } else { l };
            for c in dst.iter_mut().skip(2) {
                *c = 0.0;
            }
        }
    }
}

/// Conversion PCM 16-bit LE interleaved → mono f32 normalisé [-1, 1].
/// Utilisée pour la **détection d'onsets** et le **rendu de waveform** dans
/// le Slicer (qui travaille en mono pour ces deux étapes). La preview audio
/// passe désormais par `pcm16_le_to_interleaved_f32` pour préserver la
/// stéréo.
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

/// Conversion PCM 16-bit LE interleaved → f32 interleaved normalisé [-1, 1].
/// Préserve le nombre de canaux du source. Utilisée par la preview audio
/// (Slicer + Upload) pour conserver la stéréo.
pub fn pcm16_le_to_interleaved_f32(payload: &WavePayload) -> Vec<f32> {
    let inv_max = 1.0 / 32768.0_f32;
    let data = &payload.pcm_data;
    let n = data.len() / 2;
    let mut out = Vec::with_capacity(n);
    for chunk in data.chunks_exact(2) {
        let s = i16::from_le_bytes([chunk[0], chunk[1]]);
        out.push((s as f32) * inv_max);
    }
    out
}

/// Convertit une tranche de `pcm16_le` (LE 16-bit interleaved) en f32
/// interleaved normalisé. Sert au Slicer pour préviewer une slice donnée
/// sans dupliquer le buffer.
pub fn pcm16_le_bytes_to_interleaved_f32(bytes: &[u8]) -> Vec<f32> {
    let inv_max = 1.0 / 32768.0_f32;
    let mut out = Vec::with_capacity(bytes.len() / 2);
    for chunk in bytes.chunks_exact(2) {
        let s = i16::from_le_bytes([chunk[0], chunk[1]]);
        out.push((s as f32) * inv_max);
    }
    out
}
