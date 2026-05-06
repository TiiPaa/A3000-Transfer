//! Worker process : tourne en admin, communique avec la GUI via socket
//! TCP localhost line-delimited JSON. Port direct de `_worker.py`.
//!
//! Pourquoi pas stdin/stdout : UIPI bloque le redirect cross-privilege quand
//! le worker est élevé via UAC. La socket localhost passe.

#![cfg(windows)]

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::path::Path;
use std::time::Duration;

use a3000_core::scsi::ScsiHandle;
use a3000_core::transfer::{
    self, find_first_free_sample, query_sample_header, receive_sample, save_smdi_to_wav,
    transfer_sample, ScsiTarget, TransferOptions, WaveInput,
};
use a3000_core::wav::load_wave;

use crate::ipc::{Cmd, Event, SampleInfo};

/// Entry point du worker process.
pub fn run(host: &str, port: u16) -> anyhow::Result<()> {
    let log_path = std::env::temp_dir().join("a3000_worker.log");
    log_line(&log_path, &format!(
        "=== Worker boot, PID={}, connect {host}:{port} ===",
        std::process::id(),
    ));

    let stream = match TcpStream::connect((host, port)) {
        Ok(s) => s,
        Err(e) => {
            log_line(&log_path, &format!("connect failed: {e}"));
            return Err(e.into());
        }
    };
    stream.set_read_timeout(None).ok();

    let writer_for_main = stream.try_clone()?;
    let mut reader = BufReader::new(stream);

    {
        let mut w = writer_for_main.try_clone()?;
        send_event(&mut w, &Event::Ready)?;
    }
    log_line(&log_path, "ready envoyé");

    let mut handle: Option<ScsiHandle> = None;
    let mut current_ha: Option<u32> = None;

    loop {
        let mut line = String::new();
        let n = match reader.read_line(&mut line) {
            Ok(n) => n,
            Err(e) => {
                log_line(&log_path, &format!("read_line err: {e}"));
                break;
            }
        };
        if n == 0 {
            log_line(&log_path, "EOF — GUI a fermé la connexion");
            break;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let cmd: Cmd = match serde_json::from_str(trimmed) {
            Ok(c) => c,
            Err(e) => {
                let mut w = writer_for_main.try_clone()?;
                let _ = send_event(&mut w, &Event::Error {
                    msg: format!("bad JSON: {e}"),
                    traceback: None,
                });
                continue;
            }
        };
        log_line(&log_path, &format!("cmd: {:?}", &cmd));

        if matches!(cmd, Cmd::Exit) {
            log_line(&log_path, "exit demandé");
            break;
        }

        let writer_clone = writer_for_main.try_clone()?;
        if let Err(err) = dispatch(&cmd, &mut handle, &mut current_ha, writer_clone, &log_path) {
            let mut w = writer_for_main.try_clone()?;
            let _ = send_event(&mut w, &Event::Error {
                msg: err.to_string(),
                traceback: None,
            });
        }
    }

    log_line(&log_path, "=== Worker terminé ===");
    Ok(())
}

fn dispatch(
    cmd: &Cmd,
    handle: &mut Option<ScsiHandle>,
    current_ha: &mut Option<u32>,
    writer: TcpStream,
    log_path: &Path,
) -> anyhow::Result<()> {
    let ha = match cmd {
        Cmd::FindFreeSlot { ha, .. }
        | Cmd::ListSamples { ha, .. }
        | Cmd::Receive { ha, .. }
        | Cmd::Transfer { ha, .. } => *ha,
        Cmd::Exit => return Ok(()),
    };

    if handle.is_none() || *current_ha != Some(ha) {
        // Le drop ferme le handle précédent (RAII).
        *handle = Some(ScsiHandle::open(ha)?);
        *current_ha = Some(ha);
    }
    let h = handle.as_ref().ok_or_else(|| anyhow::anyhow!("handle non ouvert"))?;
    let mut w = writer.try_clone()?;

    match cmd {
        Cmd::FindFreeSlot { bus, target, lun, start, .. } => {
            let target = ScsiTarget { path_id: *bus, target_id: *target, lun: *lun };
            let slot = find_first_free_sample(h, target, *start, 256)?;
            send_event(&mut w, &Event::FreeSlot { slot })?;
        }

        Cmd::ListSamples { bus, target, lun, start, limit, .. } => {
            let scsi_target = ScsiTarget { path_id: *bus, target_id: *target, lun: *lun };
            let mut samples: Vec<SampleInfo> = Vec::new();
            let end = start.saturating_add(*limit);
            for n in *start..end {
                match query_sample_header(h, scsi_target, n) {
                    Ok(Some(hdr)) => {
                        let sr = if hdr.sample_period_ns != 0 {
                            ((1_000_000_000.0 / hdr.sample_period_ns as f64).round()) as u32
                        } else { 0 };
                        let duration = if sr > 0 {
                            hdr.sample_length_words as f64 / sr as f64
                        } else { 0.0 };
                        samples.push(SampleInfo {
                            slot: n, name: hdr.name.clone(),
                            channels: hdr.channels, bits: hdr.bits_per_word,
                            sample_rate: sr, frames: hdr.sample_length_words,
                            duration,
                        });
                        send_event(&mut w, &Event::ScanProgress {
                            scanned: n - start + 1, found: samples.len() as u32,
                        })?;
                    }
                    Ok(None) => {} // slot vide, continue
                    Err(transfer::TransferError::Rejected { code: 0x0020, sub: 0x0000 }) => {
                        break; // out of range
                    }
                    Err(e) => {
                        log_line(log_path, &format!("list_samples slot {n}: {e}"));
                    }
                }
            }
            send_event(&mut w, &Event::SamplesList { samples })?;
        }

        Cmd::Receive { bus, target, lun, sample_number, output_path, .. } => {
            let target = ScsiTarget { path_id: *bus, target_id: *target, lun: *lun };
            let writer_progress = w.try_clone()?;
            let mut last_pct: i32 = -1;
            let mut on_progress = move |_pkt: usize, sent: usize, total: usize| {
                let pct = (sent as i64 * 100 / total.max(1) as i64) as i32;
                if pct != last_pct {
                    if let Ok(mut wp) = writer_progress.try_clone() {
                        let _ = send_event(&mut wp, &Event::Progress {
                            sent: sent as u64, total: total as u64,
                        });
                    }
                    last_pct = pct;
                }
            };

            let (header, data) = receive_sample(
                h, target, *sample_number, 4096, 30, 1.0, Some(&mut on_progress),
            )?;
            save_smdi_to_wav(Path::new(output_path), &header, &data)?;
            let sr = if header.sample_period_ns != 0 {
                ((1_000_000_000.0 / header.sample_period_ns as f64).round()) as u32
            } else { 0 };
            send_event(&mut w, &Event::Received {
                sample_number: *sample_number,
                output_path: output_path.clone(),
                name: header.name,
                channels: header.channels,
                bits_per_word: header.bits_per_word,
                frames: header.sample_length_words,
                sample_rate: sr,
                bytes_received: data.len() as u64,
            })?;
        }

        Cmd::Transfer { bus, target, lun, sample_number, name, wave_path, .. } => {
            let scsi_target = ScsiTarget { path_id: *bus, target_id: *target, lun: *lun };
            let wave = load_wave(Path::new(wave_path))?;
            let input = WaveInput {
                channels: wave.channels,
                sample_rate: wave.sample_rate,
                bits_per_sample: 16,
                frame_count: wave.frame_count as u32,
                pcm_data_le: &wave.pcm_data,
            };
            let opts = TransferOptions::default();
            let writer_progress = w.try_clone()?;
            let mut last_pct: i32 = -1;
            let mut on_progress = move |_pkt: usize, sent: usize, total: usize| {
                let pct = (sent as i64 * 100 / total.max(1) as i64) as i32;
                if pct != last_pct {
                    if let Ok(mut wp) = writer_progress.try_clone() {
                        let _ = send_event(&mut wp, &Event::Progress {
                            sent: sent as u64, total: total as u64,
                        });
                    }
                    last_pct = pct;
                }
            };
            let stats = transfer_sample(
                h, scsi_target, *sample_number, input, name, &opts, Some(&mut on_progress),
            )?;
            send_event(&mut w, &Event::Done {
                sample_number: stats.sample_number,
                bytes_sent: stats.bytes_sent as u64,
                packet_count: stats.packet_count as u32,
            })?;
        }

        Cmd::Exit => {}
    }
    Ok(())
}

/// Sérialise un Event et écrit `<json>\n` sur le writer.
pub fn send_event<W: Write>(writer: &mut W, event: &Event) -> anyhow::Result<()> {
    let line = serde_json::to_string(event)?;
    writer.write_all(line.as_bytes())?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}

fn log_line(path: &Path, msg: &str) {
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true).append(true).open(path)
    {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or(Duration::ZERO);
        let _ = writeln!(f, "[{}.{:03}] {}", now.as_secs(), now.subsec_millis(), msg);
    }
}
