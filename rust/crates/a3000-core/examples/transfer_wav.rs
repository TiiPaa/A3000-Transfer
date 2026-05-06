//! End-to-end : envoie un WAV au Yamaha A3000 via SMDI, avec timing détaillé.
//!
//! Usage :
//!   .\transfer_wav.exe <path.wav> [--slot N] [--name X]
//!     [--ha 1] [--bus 0] [--target 0] [--lun 0]
//!     [--preferred-dpl 4096] [--settle 1.0] [--skip-drain] [--dry-run]
//!
//! Defaults : slot=300, HA=1, BUS=0, TARGET=0, LUN=0,
//!            preferred-dpl=4096, settle=1.0, skip-drain=false.
//!
//! Affiche en fin de transfert :
//!   - load_wave_ms : décodage WAV + dither + conversion PCM
//!   - open_scsi_ms : open_adapter + handshake initial
//!   - transfer_ms  : handshake SMDI + boucle SNP/DP + settle
//!   - throughput   : bytes_sent / transfer_ms
//!   - dpl_used     : data packet length effectif
//!
//! Requiert PowerShell **Administrator**.

#![cfg(windows)]
#![allow(clippy::expect_used, clippy::print_stdout, clippy::print_stderr)]

use std::env;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use a3000_core::scsi::ScsiHandle;
use a3000_core::transfer::{transfer_sample, ScsiTarget, TransferOptions, WaveInput};
use a3000_core::wav::load_wave;

struct Args {
    wav_path: PathBuf,
    slot: u32,
    name: String,
    target: ScsiTarget,
    ha: u32,
    dry_run: bool,
    preferred_dpl: u32,
    settle: f32,
    skip_drain: bool,
}

fn parse_args() -> Result<Args, String> {
    let raw: Vec<String> = env::args().skip(1).collect();
    if raw.is_empty() {
        return Err("Usage: transfer_wav <path.wav> [flags] (voir docstring)".into());
    }
    let mut wav_path: Option<PathBuf> = None;
    let mut slot: u32 = 300;
    let mut name = String::new();
    let mut ha: u32 = 1;
    let mut bus: u8 = 0;
    let mut target: u8 = 0;
    let mut lun: u8 = 0;
    let mut dry_run = false;
    let mut preferred_dpl: u32 = 4096;
    let mut settle: f32 = 1.0;
    let mut skip_drain = false;

    let mut i = 0;
    while i < raw.len() {
        match raw[i].as_str() {
            "--slot"          => { i += 1; slot = raw[i].parse().map_err(|_| "--slot N")?; }
            "--name"          => { i += 1; name = raw[i].clone(); }
            "--ha"            => { i += 1; ha = raw[i].parse().map_err(|_| "--ha N")?; }
            "--bus"           => { i += 1; bus = raw[i].parse().map_err(|_| "--bus N")?; }
            "--target"        => { i += 1; target = raw[i].parse().map_err(|_| "--target N")?; }
            "--lun"           => { i += 1; lun = raw[i].parse().map_err(|_| "--lun N")?; }
            "--preferred-dpl" => { i += 1; preferred_dpl = raw[i].parse().map_err(|_| "--preferred-dpl N")?; }
            "--settle"        => { i += 1; settle = raw[i].parse().map_err(|_| "--settle SECONDS")?; }
            "--skip-drain"    => { skip_drain = true; }
            "--dry-run"       => { dry_run = true; }
            other if !other.starts_with("--") => { wav_path = Some(PathBuf::from(other)); }
            other => return Err(format!("flag inconnu : {other}")),
        }
        i += 1;
    }
    let wav_path = wav_path.ok_or("path WAV requis")?;
    if name.is_empty() {
        name = wav_path
            .file_stem().and_then(|s| s.to_str()).unwrap_or("sample")
            .chars().take(16).collect();
    }
    Ok(Args {
        wav_path, slot, name, ha,
        target: ScsiTarget { path_id: bus, target_id: target, lun },
        dry_run, preferred_dpl, settle, skip_drain,
    })
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => { eprintln!("{e}"); return ExitCode::from(2); }
    };

    println!("=== a3000-core transfer_wav (timed) ===");
    println!("WAV   : {}", args.wav_path.display());
    println!("Slot  : #{} '{}'", args.slot, args.name);
    println!("SCSI  : HA{} BUS{} ID{} LUN{}",
             args.ha, args.target.path_id, args.target.target_id, args.target.lun);
    println!("Opts  : preferred-dpl={} settle={}s skip-drain={} dry-run={}",
             args.preferred_dpl, args.settle, args.skip_drain, args.dry_run);
    println!();

    // 1. Load WAV (décodage + dither + conversion PCM 16-bit LE)
    let t0 = Instant::now();
    let wave = match load_wave(&args.wav_path) {
        Ok(w) => w,
        Err(e) => { eprintln!("✗ load_wave : {e}"); return ExitCode::from(3); }
    };
    let load_wave_ms = t0.elapsed().as_secs_f64() * 1000.0;
    println!("→ load_wave       : {:>8.1} ms ({} ch, {} Hz, {} frames, {} bytes PCM)",
             load_wave_ms, wave.channels, wave.sample_rate, wave.frame_count, wave.pcm_data.len());

    // 2. Open SCSI handle
    let t1 = Instant::now();
    let handle = match ScsiHandle::open(args.ha) {
        Ok(h) => h,
        Err(e) => { eprintln!("✗ {e}"); return ExitCode::from(4); }
    };
    let open_scsi_ms = t1.elapsed().as_secs_f64() * 1000.0;
    println!("→ open_scsi       : {open_scsi_ms:>8.1} ms");

    // 3. Transfer (handshake + boucle DP + settle)
    let opts = TransferOptions {
        dry_run: args.dry_run,
        preferred_packet_length: args.preferred_dpl,
        settle_seconds: args.settle,
        skip_initial_drain: args.skip_drain,
        ..Default::default()
    };
    let input = WaveInput {
        channels: wave.channels,
        sample_rate: wave.sample_rate,
        bits_per_sample: 16,
        frame_count: wave.frame_count as u32,
        pcm_data_le: &wave.pcm_data,
    };

    let mut last_pct: u32 = 0;
    let mut on_progress = move |packets: usize, sent: usize, total: usize| {
        let pct = ((sent as u64 * 100) / total.max(1) as u64) as u32;
        if pct >= last_pct + 10 || pct == 100 {
            println!("    pkt #{packets:>4}  {sent:>10}/{total} ({pct}%)");
            last_pct = pct;
        }
    };

    let t2 = Instant::now();
    let stats = match transfer_sample(
        &handle, args.target, args.slot, input, &args.name, &opts, Some(&mut on_progress),
    ) {
        Ok(s) => s,
        Err(e) => { eprintln!("✗ Transfer error : {e}"); return ExitCode::from(5); }
    };
    let transfer_ms = t2.elapsed().as_secs_f64() * 1000.0;

    println!("→ transfer        : {transfer_ms:>8.1} ms");
    let total_ms = load_wave_ms + open_scsi_ms + transfer_ms;
    println!();
    println!("✓✓✓ Transfer OK");
    println!("  bytes_sent     = {} ({:.2} KB)", stats.bytes_sent, stats.bytes_sent as f64 / 1024.0);
    println!("  packet_count   = {}", stats.packet_count);
    println!("  packet_length  = {} (negotiated DPL)", stats.packet_length);
    println!("  total_ms       = {total_ms:.1} (load={load_wave_ms:.0} + open={open_scsi_ms:.0} + transfer={transfer_ms:.0})");
    if !args.dry_run && stats.bytes_sent > 0 && transfer_ms > 0.0 {
        let throughput_kbps = (stats.bytes_sent as f64 / 1024.0) / (transfer_ms / 1000.0);
        println!("  throughput     = {throughput_kbps:.1} KB/s");
    }
    ExitCode::SUCCESS
}
