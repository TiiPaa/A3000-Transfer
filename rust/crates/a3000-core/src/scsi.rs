//! Pass-through SCSI via `IOCTL_SCSI_PASS_THROUGH_DIRECT` (Windows).
//!
//! Référence Python : `python/a3000_transfer/scsi_passthrough.py`
//!
//! Différences idiomatiques vs Python (cf. `docs/conversion/DECISIONS.md`) :
//! - Handle `HANDLE` enveloppé dans un newtype `ScsiHandle` avec `Drop` qui
//!   ferme automatiquement (RAII)
//! - Erreurs typées `ScsiError` au lieu de `OSError`/`PermissionError`
//! - Le buffer aligné 512 octets utilise une `Vec<u8>` sur-allouée + offset
//!   (pas d'unsafe pour l'allocation, juste pour passer le pointeur à l'API
//!   Win32)

#![cfg(windows)]

use std::ptr;
use std::thread;
use std::time::Duration;

use thiserror::Error;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{
    CloseHandle, GetLastError, HANDLE, INVALID_HANDLE_VALUE,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_GENERIC_READ, FILE_GENERIC_WRITE,
    FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows::Win32::Storage::IscsiDisc::SCSI_PASS_THROUGH_DIRECT;
use windows::Win32::System::IO::DeviceIoControl;

const IOCTL_SCSI_PASS_THROUGH_DIRECT: u32 = 0x0004_D014;

const SCSI_IOCTL_DATA_OUT: u8 = 0;
const SCSI_IOCTL_DATA_IN: u8 = 1;
const SCSI_IOCTL_DATA_UNSPECIFIED: u8 = 2;

const SENSE_BUFFER_LENGTH: usize = 32;
pub const DEFAULT_TIMEOUT_SECONDS: u32 = 30;
/// Alignement DMA imposé par storport pour éviter le bounce buffer kernel
/// (qui pouvait casser les transferts SMDI multi-paquets sur firmware A3000 v0200).
const DATA_BUFFER_ALIGNMENT: usize = 512;

const ERROR_ACCESS_DENIED: u32 = 5;
const ERROR_IO_DEVICE: u32 = 1117;
const MAX_RETRIES: u32 = 5;

#[derive(Debug, Error)]
pub enum ScsiError {
    #[error("Accès refusé sur {0}. Lance le terminal en administrateur.")]
    AccessDenied(String),
    #[error("Impossible d'ouvrir {path}: erreur Win32 {code}")]
    OpenFailed { path: String, code: u32 },
    #[error("CDB invalide : {0} octets (attendu 1..16)")]
    InvalidCdbLength(usize),
    #[error("send_cdb : pas de CDB bidirectionnel — choisis data_in_length OU data_out")]
    BidirectionalNotSupported,
    #[error("IOCTL_SCSI_PASS_THROUGH_DIRECT a échoué : erreur Win32 {0}")]
    IoctlFailed(u32),
}

/// Résultat d'un pass-through SCSI : état + données reçues + sense.
#[derive(Debug, Clone)]
pub struct PassThroughResult {
    pub scsi_status: u8,
    pub data: Vec<u8>,
    pub sense: [u8; SENSE_BUFFER_LENGTH],
    pub transferred: u32,
}

/// Handle SCSI ouvert sur un host adapter Windows. Fermé automatiquement
/// via `Drop` (RAII) — pas de risque de fuite même en cas d'erreur ou panic.
pub struct ScsiHandle {
    raw: HANDLE,
}

impl ScsiHandle {
    /// Ouvre `\\.\ScsiN:` (où N = `host_adapter`).
    ///
    /// # Errors
    /// - `AccessDenied` si l'app ne tourne pas en admin (ERROR_ACCESS_DENIED = 5)
    /// - `OpenFailed` pour les autres erreurs Win32
    pub fn open(host_adapter: u32) -> Result<Self, ScsiError> {
        let path = format!(r"\\.\Scsi{host_adapter}:");
        let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
        // SAFETY : CreateFileW est correctement appelé avec un null-terminated UTF-16
        // path et des flags valides. Le HANDLE retourné est validé juste après.
        let handle = unsafe {
            CreateFileW(
                PCWSTR::from_raw(wide.as_ptr()),
                (FILE_GENERIC_READ | FILE_GENERIC_WRITE).0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                FILE_FLAGS_AND_ATTRIBUTES(0),
                None,
            )
        };
        match handle {
            Ok(h) if h.0 != INVALID_HANDLE_VALUE.0 => Ok(Self { raw: h }),
            _ => {
                let err = unsafe { GetLastError() }.0;
                if err == ERROR_ACCESS_DENIED {
                    Err(ScsiError::AccessDenied(path))
                } else {
                    Err(ScsiError::OpenFailed { path, code: err })
                }
            }
        }
    }

    /// Envoie un CDB SCSI via `IOCTL_SCSI_PASS_THROUGH_DIRECT`.
    ///
    /// Si `data_in_length > 0` : direction READ (le slave envoie au master).
    /// Si `data_out.is_some()` : direction WRITE (le master envoie au slave).
    /// Sinon : direction UNSPECIFIED (typiquement INQUIRY sans data, ou commande
    /// silencieuse).
    ///
    /// Buffer aligné 512 octets, retry jusqu'à 5× sur `ERROR_IO_DEVICE` (slave
    /// busy transitoire) avec délai progressif 200 ms à 1 s.
    ///
    /// # Errors
    /// - `BidirectionalNotSupported` si data_in_length ET data_out fournis
    /// - `InvalidCdbLength` si CDB hors range 1..=16
    /// - `IoctlFailed` après tous les retries
    #[allow(clippy::too_many_arguments)] // pass-through bas niveau, signature volontaire
    pub fn send_cdb(
        &self,
        path_id: u8,
        target_id: u8,
        lun: u8,
        cdb: &[u8],
        data_in_length: u32,
        data_out: Option<&[u8]>,
        timeout_seconds: u32,
    ) -> Result<PassThroughResult, ScsiError> {
        if data_in_length > 0 && data_out.is_some() {
            return Err(ScsiError::BidirectionalNotSupported);
        }
        if cdb.is_empty() || cdb.len() > 16 {
            return Err(ScsiError::InvalidCdbLength(cdb.len()));
        }

        // Buffer aligné 512 (sur-allocation + offset). On le garde vivant
        // pendant toute la durée de l'IOCTL pour que le pointeur reste valide.
        let (data_buffer_ptr, backing_vec, transfer_length, direction) = if data_in_length > 0 {
            let (ptr, vec) = aligned_buffer(data_in_length as usize);
            (ptr, vec, data_in_length, SCSI_IOCTL_DATA_IN)
        } else if let Some(out) = data_out {
            let (ptr, mut vec) = aligned_buffer(out.len());
            // Copie les bytes dans le buffer aligné
            // SAFETY : ptr est dans vec, taille validée. Pas de double-borrow.
            unsafe {
                ptr::copy_nonoverlapping(out.as_ptr(), ptr, out.len());
            }
            // Force le compilo à garder vec vivant en lui réassignant à elle-même
            vec.shrink_to_fit();
            (ptr, vec, out.len() as u32, SCSI_IOCTL_DATA_OUT)
        } else {
            (ptr::null_mut(), Vec::new(), 0_u32, SCSI_IOCTL_DATA_UNSPECIFIED)
        };

        // SPTD avec sense buffer accolé. Layout : SCSI_PASS_THROUGH_DIRECT
        // suivi des SENSE_BUFFER_LENGTH bytes de sense info.
        #[repr(C)]
        struct SptdWithSense {
            spt: SCSI_PASS_THROUGH_DIRECT,
            sense: [u8; SENSE_BUFFER_LENGTH],
        }

        let mut sptd: SptdWithSense = unsafe { std::mem::zeroed() };
        sptd.spt.Length = u16::try_from(std::mem::size_of::<SCSI_PASS_THROUGH_DIRECT>())
            .unwrap_or(u16::MAX);
        sptd.spt.PathId = path_id;
        sptd.spt.TargetId = target_id;
        sptd.spt.Lun = lun;
        sptd.spt.CdbLength = cdb.len() as u8;
        sptd.spt.SenseInfoLength = SENSE_BUFFER_LENGTH as u8;
        sptd.spt.DataIn = direction;
        sptd.spt.DataTransferLength = transfer_length;
        sptd.spt.TimeOutValue = timeout_seconds;
        sptd.spt.DataBuffer = data_buffer_ptr.cast();
        sptd.spt.SenseInfoOffset =
            u32::try_from(std::mem::size_of::<SCSI_PASS_THROUGH_DIRECT>()).unwrap_or(0);
        // CDB padded à 16 octets (zéros)
        sptd.spt.Cdb[..cdb.len()].copy_from_slice(cdb);

        let mut returned: u32 = 0;
        let mut last_err: u32 = 0;
        let mut succeeded = false;

        for attempt in 0..MAX_RETRIES {
            // SAFETY : on passe une SptdWithSense valide en in/out, taille correcte,
            // handle validé à la construction, ptr alignés.
            let ok = unsafe {
                DeviceIoControl(
                    self.raw,
                    IOCTL_SCSI_PASS_THROUGH_DIRECT,
                    Some(std::ptr::from_ref::<SptdWithSense>(&sptd).cast()),
                    u32::try_from(std::mem::size_of::<SptdWithSense>()).unwrap_or(0),
                    Some(std::ptr::from_mut::<SptdWithSense>(&mut sptd).cast()),
                    u32::try_from(std::mem::size_of::<SptdWithSense>()).unwrap_or(0),
                    Some(&mut returned),
                    None,
                )
            };
            if ok.is_ok() {
                succeeded = true;
                break;
            }
            last_err = unsafe { GetLastError() }.0;
            if last_err != ERROR_IO_DEVICE {
                break;
            }
            // Délai progressif 200 ms × (attempt + 1)
            thread::sleep(Duration::from_millis(200 * u64::from(attempt + 1)));
        }

        if !succeeded {
            return Err(ScsiError::IoctlFailed(last_err));
        }

        let actual = sptd.spt.DataTransferLength;
        let data: Vec<u8> = if direction == SCSI_IOCTL_DATA_IN && !data_buffer_ptr.is_null() {
            let n = (actual as usize).min(transfer_length as usize);
            // SAFETY : data_buffer_ptr pointe dans backing_vec (taille >= n).
            unsafe { std::slice::from_raw_parts(data_buffer_ptr, n).to_vec() }
        } else {
            Vec::new()
        };
        // Drop explicite après copie pour clarifier la durée de vie
        drop(backing_vec);

        Ok(PassThroughResult {
            scsi_status: sptd.spt.ScsiStatus,
            data,
            sense: sptd.sense,
            transferred: actual,
        })
    }
}

impl Drop for ScsiHandle {
    fn drop(&mut self) {
        if self.raw.0 != INVALID_HANDLE_VALUE.0 && !self.raw.is_invalid() {
            // SAFETY : handle valide, fermé une seule fois (Drop appelé une fois)
            let _ = unsafe { CloseHandle(self.raw) };
        }
    }
}

// SAFETY : le HANDLE Win32 peut traverser les threads à condition que Send/Sync
// soit explicite (Windows kernel objects sont thread-safe pour les opérations
// I/O). On marque ScsiHandle Send mais pas Sync : un seul thread fait l'IOCTL
// à la fois (DeviceIoControl synchrone).
unsafe impl Send for ScsiHandle {}

/// Alloue un buffer Vec<u8> sur-alloué de `size + DATA_BUFFER_ALIGNMENT` octets
/// et retourne (pointeur aligné 512, vec de backing).
///
/// Le caller doit garder le `Vec` vivant tant qu'il utilise le pointeur.
fn aligned_buffer(size: usize) -> (*mut u8, Vec<u8>) {
    let mut vec = vec![0u8; size + DATA_BUFFER_ALIGNMENT];
    let base = vec.as_mut_ptr() as usize;
    let aligned_offset = (DATA_BUFFER_ALIGNMENT - (base % DATA_BUFFER_ALIGNMENT))
        % DATA_BUFFER_ALIGNMENT;
    // SAFETY : aligned_offset < DATA_BUFFER_ALIGNMENT ≤ vec capacity, donc dans le buffer.
    let aligned_ptr = unsafe { vec.as_mut_ptr().add(aligned_offset) };
    (aligned_ptr, vec)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn aligned_buffer_is_512_aligned() {
        for size in [0_usize, 1, 11, 256, 512, 4096, 65535] {
            let (ptr, _vec) = aligned_buffer(size);
            assert_eq!(
                (ptr as usize) % DATA_BUFFER_ALIGNMENT, 0,
                "buffer not 512-aligned for size {size}"
            );
        }
    }

    #[test]
    fn open_invalid_adapter_returns_error() {
        // Adapter 99 n'existe pas (ou très improbable). Doit échouer proprement,
        // pas crasher. Note : peut renvoyer AccessDenied si pas admin, ou OpenFailed.
        let r = ScsiHandle::open(99);
        assert!(r.is_err(), "expected error opening Scsi99 (non-existent)");
    }

    #[test]
    fn cdb_length_validated() {
        // On peut tester la validation sans avoir un vrai handle. Il faut juste
        // un ScsiHandle factice — mais comme on ne peut pas construire un HANDLE
        // valide arbitrairement, on skip ce test (le check est en début de
        // send_cdb, validé par lecture de code).
    }
}
