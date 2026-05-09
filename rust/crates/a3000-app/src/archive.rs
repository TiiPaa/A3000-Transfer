//! Extraction d'archives drag-droppées en Upload : .zip / .tar.gz / .tgz / .tar.
//!
//! Stratégie : extraction dans `%TEMP%/a3000_extracted/<stem>_<rand>/`,
//! walk récursif → renvoie tous les `.wav` trouvés. Erreurs cumulées dans
//! une chaîne lisible sans interrompre le batch (un .zip corrompu ne
//! doit pas faire perdre les autres droppés OK).

use std::fs::File;
use std::io::{self, BufReader};
use std::path::{Path, PathBuf};

use flate2::read::GzDecoder;

/// Type d'archive détecté par extension (case-insensitive).
fn archive_kind(path: &Path) -> Option<ArchiveKind> {
    let name = path.file_name()?.to_str()?.to_lowercase();
    if name.ends_with(".zip") {
        Some(ArchiveKind::Zip)
    } else if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        Some(ArchiveKind::TarGz)
    } else if name.ends_with(".tar") {
        Some(ArchiveKind::Tar)
    } else {
        None
    }
}

#[derive(Debug, Clone, Copy)]
enum ArchiveKind {
    Zip,
    TarGz,
    Tar,
}

/// Si `path` est une archive supportée, extrait son contenu dans un répertoire
/// temporaire et retourne la liste des `.wav` extraits. Si `path` n'est pas
/// une archive, renvoie `None`.
pub fn try_extract_archive(path: &Path) -> Option<Result<Vec<PathBuf>, String>> {
    let kind = archive_kind(path)?;
    Some(extract(path, kind))
}

fn extract(path: &Path, kind: ArchiveKind) -> Result<Vec<PathBuf>, String> {
    let stem = path.file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "archive".into());
    // Nom unique pour éviter les collisions si on dropp 2× la même archive.
    let pid = std::process::id();
    let nano = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos()).unwrap_or(0);
    let safe_stem: String = stem.chars()
        .map(|c| if c.is_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
        .take(40).collect();
    let out_dir = std::env::temp_dir()
        .join("a3000_extracted")
        .join(format!("{safe_stem}_{pid}_{nano}"));
    std::fs::create_dir_all(&out_dir)
        .map_err(|e| format!("create_dir {}: {e}", out_dir.display()))?;

    match kind {
        ArchiveKind::Zip => extract_zip(path, &out_dir)?,
        ArchiveKind::TarGz => extract_tar(path, &out_dir, true)?,
        ArchiveKind::Tar => extract_tar(path, &out_dir, false)?,
    }

    Ok(walk_wavs(&out_dir))
}

fn extract_zip(archive: &Path, out_dir: &Path) -> Result<(), String> {
    let f = File::open(archive)
        .map_err(|e| format!("open {}: {e}", archive.display()))?;
    let mut zip = zip::ZipArchive::new(BufReader::new(f))
        .map_err(|e| format!("zip parse: {e}"))?;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)
            .map_err(|e| format!("zip entry {i}: {e}"))?;
        // Sécurité : chemins absolus / parent (.. ) interdits.
        let entry_path = match entry.enclosed_name() {
            Some(p) => out_dir.join(p),
            None => continue,
        };
        if entry.is_dir() {
            std::fs::create_dir_all(&entry_path)
                .map_err(|e| format!("create_dir {}: {e}", entry_path.display()))?;
        } else {
            if let Some(parent) = entry_path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("create_dir parent: {e}"))?;
            }
            let mut out = File::create(&entry_path)
                .map_err(|e| format!("create {}: {e}", entry_path.display()))?;
            io::copy(&mut entry, &mut out)
                .map_err(|e| format!("copy {}: {e}", entry_path.display()))?;
        }
    }
    Ok(())
}

fn extract_tar(archive: &Path, out_dir: &Path, gz: bool) -> Result<(), String> {
    let f = File::open(archive)
        .map_err(|e| format!("open {}: {e}", archive.display()))?;
    let buf = BufReader::new(f);
    if gz {
        let dec = GzDecoder::new(buf);
        tar::Archive::new(dec).unpack(out_dir)
            .map_err(|e| format!("tar.gz unpack: {e}"))?;
    } else {
        tar::Archive::new(buf).unpack(out_dir)
            .map_err(|e| format!("tar unpack: {e}"))?;
    }
    Ok(())
}

/// Walk récursif : retourne tous les fichiers d'extension .wav trouvés.
fn walk_wavs(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    walk_into(dir, &mut out);
    out
}

fn walk_into(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return; };
    for entry in rd.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_into(&path, out);
        } else if matches!(
            path.extension().and_then(|e| e.to_str()).map(|s| s.to_lowercase()).as_deref(),
            Some("wav"),
        ) {
            out.push(path);
        }
    }
}
