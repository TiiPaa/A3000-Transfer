//! Config persistée dans `%APPDATA%/a3000_transfer/config.json` (Windows)
//! ou `~/.config/a3000_transfer/config.json` (autres). Port direct des
//! options sauvegardées par `gui.py:_load_config` / `_save_config`.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Host adapter (typiquement 1).
    #[serde(default = "default_ha")]
    pub ha: u32,
    /// SCSI bus (PathId).
    #[serde(default)]
    pub bus: u8,
    /// Target ID.
    #[serde(default)]
    pub target: u8,
    /// LUN.
    #[serde(default)]
    pub lun: u8,
    /// Mode de sélection du slot de départ : auto = scan first free.
    #[serde(default = "default_auto_start_slot")]
    pub auto_start_slot: bool,
    /// Slot de départ explicite (si auto désactivé).
    #[serde(default = "default_manual_start_slot")]
    pub manual_start_slot: u32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            ha: default_ha(),
            bus: 0,
            target: 0,
            lun: 0,
            auto_start_slot: true,
            manual_start_slot: 7,
        }
    }
}

fn default_ha() -> u32 { 1 }
fn default_auto_start_slot() -> bool { true }
fn default_manual_start_slot() -> u32 { 7 }

impl Config {
    pub fn config_path() -> PathBuf {
        let base = if cfg!(windows) {
            std::env::var_os("APPDATA")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("."))
        } else {
            dirs_home().join(".config")
        };
        base.join("a3000_transfer").join("config.json")
    }

    pub fn load() -> Self {
        let path = Self::config_path();
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> std::io::Result<()> {
        let path = Self::config_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let s = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(&path, s)?;
        Ok(())
    }
}

fn dirs_home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}
