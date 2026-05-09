//! Protocol JSON line-delimited entre la GUI (non-élevée) et le worker
//! (élevé via UAC). Port 1:1 du protocole défini dans `_worker.py`.
//!
//! Format : un JSON object par ligne, terminée par `\n`.
//!
//! # Cmd (GUI → Worker)
//!
//! ```json
//! {"cmd": "find_free_slot", "ha": 1, "bus": 0, "target": 0, "lun": 0, "start": 7}
//! {"cmd": "list_samples", "ha": 1, "bus": 0, "target": 0, "lun": 0, "start": 0, "limit": 128}
//! {"cmd": "receive", "ha": 1, ..., "sample_number": 100, "output_path": "..."}
//! {"cmd": "transfer", "ha": 1, ..., "sample_number": 100, "name": "...", "wave_path": "..."}
//! {"cmd": "exit"}
//! ```
//!
//! # Event (Worker → GUI)
//!
//! ```json
//! {"event": "ready"}
//! {"event": "free_slot", "slot": 100}
//! {"event": "scan_progress", "scanned": 12, "found": 3}
//! {"event": "samples_list", "samples": [...]}
//! {"event": "progress", "sent": 12345, "total": 67890}
//! {"event": "done", "sample_number": 100, "bytes_sent": ..., "packet_count": ...}
//! {"event": "received", ...}
//! {"event": "error", "msg": "..."}
//! ```

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Cmd {
    /// Sondage SMDI : Master Identify → vérifie qu'un sampler répond.
    /// Utilisé juste après la connexion worker pour valider que le hardware
    /// est présent à l'adresse configurée.
    Probe {
        #[serde(default = "default_ha")] ha: u32,
        #[serde(default)] bus: u8,
        #[serde(default)] target: u8,
        #[serde(default)] lun: u8,
    },
    FindFreeSlot {
        #[serde(default = "default_ha")] ha: u32,
        #[serde(default)] bus: u8,
        #[serde(default)] target: u8,
        #[serde(default)] lun: u8,
        #[serde(default = "default_start_slot")] start: u32,
    },
    ListSamples {
        #[serde(default = "default_ha")] ha: u32,
        #[serde(default)] bus: u8,
        #[serde(default)] target: u8,
        #[serde(default)] lun: u8,
        #[serde(default)] start: u32,
        #[serde(default = "default_list_limit")] limit: u32,
    },
    Receive {
        #[serde(default = "default_ha")] ha: u32,
        #[serde(default)] bus: u8,
        #[serde(default)] target: u8,
        #[serde(default)] lun: u8,
        sample_number: u32,
        output_path: String,
    },
    Transfer {
        #[serde(default = "default_ha")] ha: u32,
        #[serde(default)] bus: u8,
        #[serde(default)] target: u8,
        #[serde(default)] lun: u8,
        sample_number: u32,
        #[serde(default)] name: String,
        wave_path: String,
    },
    Exit,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    Ready,
    /// Réponse à `Cmd::Probe` : le sampler a renvoyé un Slave Identify valide.
    ProbeOk,
    FreeSlot { slot: u32 },
    ScanProgress { scanned: u32, found: u32 },
    SamplesList { samples: Vec<SampleInfo> },
    Progress { sent: u64, total: u64 },
    Done {
        sample_number: u32,
        bytes_sent: u64,
        packet_count: u32,
    },
    Received {
        sample_number: u32,
        output_path: String,
        name: String,
        channels: u8,
        bits_per_word: u8,
        frames: u32,
        sample_rate: u32,
        bytes_received: u64,
    },
    Error {
        msg: String,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        traceback: Option<String>,
    },
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SampleInfo {
    pub slot: u32,
    pub name: String,
    pub channels: u8,
    pub bits: u8,
    pub sample_rate: u32,
    pub frames: u32,
    pub duration: f64,
}

fn default_ha() -> u32 { 1 }
fn default_start_slot() -> u32 { 7 }
fn default_list_limit() -> u32 { 128 }

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn cmd_exit_roundtrip() {
        let json = r#"{"cmd":"exit"}"#;
        let c: Cmd = serde_json::from_str(json).unwrap();
        assert!(matches!(c, Cmd::Exit));
        let back = serde_json::to_string(&c).unwrap();
        assert_eq!(back, json);
    }

    #[test]
    fn cmd_find_free_slot_with_defaults() {
        let json = r#"{"cmd":"find_free_slot"}"#;
        let c: Cmd = serde_json::from_str(json).unwrap();
        match c {
            Cmd::FindFreeSlot { ha, bus, target, lun, start } => {
                assert_eq!((ha, bus, target, lun, start), (1, 0, 0, 0, 7));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn cmd_transfer_full() {
        let json = r#"{"cmd":"transfer","ha":2,"bus":0,"target":5,"lun":0,"sample_number":42,"name":"kick","wave_path":"C:\\snd.wav"}"#;
        let c: Cmd = serde_json::from_str(json).unwrap();
        match c {
            Cmd::Transfer { ha, target, sample_number, name, wave_path, .. } => {
                assert_eq!(ha, 2);
                assert_eq!(target, 5);
                assert_eq!(sample_number, 42);
                assert_eq!(name, "kick");
                assert_eq!(wave_path, "C:\\snd.wav");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn event_ready_serialize() {
        let e = Event::Ready;
        let s = serde_json::to_string(&e).unwrap();
        assert_eq!(s, r#"{"event":"ready"}"#);
    }

    #[test]
    fn event_free_slot_serialize() {
        let e = Event::FreeSlot { slot: 100 };
        let s = serde_json::to_string(&e).unwrap();
        assert_eq!(s, r#"{"event":"free_slot","slot":100}"#);
    }

    #[test]
    fn event_progress_serialize() {
        let e = Event::Progress { sent: 1024, total: 4096 };
        let s = serde_json::to_string(&e).unwrap();
        assert_eq!(s, r#"{"event":"progress","sent":1024,"total":4096}"#);
    }

    #[test]
    fn event_error_no_traceback() {
        let e = Event::Error { msg: "boom".into(), traceback: None };
        let s = serde_json::to_string(&e).unwrap();
        assert_eq!(s, r#"{"event":"error","msg":"boom"}"#);
    }
}
