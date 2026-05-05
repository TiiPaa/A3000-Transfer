//! IOCTL_SCSI_PASS_THROUGH_DIRECT via le crate `windows`.
//!
//! Référence Python : `python/a3000_transfer/scsi_passthrough.py`
//!
//! ScsiHandle = newtype RAII sur `HANDLE` (Drop ferme le handle).

// TODO Phase 1 : open_adapter, pass_through_direct, retry sur ERROR_IO_DEVICE
