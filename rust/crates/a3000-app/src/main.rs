//! Entry point — routage entre mode GUI et mode worker (`--worker`).
//!
//! Référence Python : `python/a3000_transfer/__main__.py` + `cli.py`.
//!
//! Modes :
//!   - `a3000-transfer` (sans args)         → GUI
//!   - `a3000-transfer --worker --port N`   → worker process élevé (admin)
//!   - autres args                          → futurs sous-commandes CLI (scan, send, …)
//!
//! Pas de `_setup_numba_cache` : pas de numba/JIT en Rust, démarrage instant.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod config;
mod ipc;
mod tabs;
mod theme;

#[cfg(windows)]
mod client;
#[cfg(windows)]
mod worker;

use std::process::ExitCode;

fn main() -> ExitCode {
    init_tracing();

    let args: Vec<String> = std::env::args().collect();

    // Mode worker : on cherche `--worker --port N` n'importe où dans argv.
    #[cfg(windows)]
    if let Some(port) = parse_worker_port(&args) {
        match worker::run("127.0.0.1", port) {
            Ok(()) => return ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("worker error: {e}");
                return ExitCode::from(2);
            }
        }
    }

    // Mode GUI (defaut).
    let native_options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1100.0, 720.0])
            .with_min_inner_size([700.0, 480.0])
            .with_title("A3000 Transfer"),
        ..Default::default()
    };
    match eframe::run_native(
        "A3000 Transfer",
        native_options,
        Box::new(|cc| Ok(Box::new(app::A3000App::new(cc)))),
    ) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("eframe error: {e}");
            ExitCode::from(1)
        }
    }
}

fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .with_target(false)
        .try_init();
}

#[cfg(windows)]
fn parse_worker_port(args: &[String]) -> Option<u16> {
    let mut iter = args.iter();
    let mut found_worker = false;
    while let Some(a) = iter.next() {
        if a == "--worker" {
            found_worker = true;
        } else if a == "--port" {
            if let Some(p) = iter.next().and_then(|s| s.parse::<u16>().ok()) {
                if found_worker {
                    return Some(p);
                }
            }
        }
    }
    None
}
