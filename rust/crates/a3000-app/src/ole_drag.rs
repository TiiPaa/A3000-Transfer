//! OLE drag-and-drop : permet de **glisser un fichier** depuis l'app vers
//! une autre application Windows (typiquement un DAW). Implémente
//! `IDataObject` + `IDropSource` + `IEnumFORMATETC` minimaux pour fournir
//! le format `CF_HDROP` (chemin de fichier).
//!
//! Architecture :
//!   - `init_ole_thread()` : `OleInitialize` une fois par thread (idempotent).
//!   - `drag_file(path)` : construit un `IDataObject` + `IDropSource`,
//!     appelle `DoDragDrop` (BLOQUANT — ne retourne qu'après drop ou cancel),
//!     retourne le `DROPEFFECT` final.
//!
//! Référence Python : `python/a3000_transfer/slicer/view.py:_on_midi_drag`
//! et le pattern pythoncom IDataObject. Ici on implémente directement les
//! interfaces COM via le macro `windows::core::implement`.

#![cfg(windows)]
#![allow(non_snake_case)] // méthodes COM en PascalCase

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

use windows::core::implement;
use windows::Win32::Foundation::{
    BOOL, DATA_S_SAMEFORMATETC, DRAGDROP_S_CANCEL, DRAGDROP_S_DROP,
    DRAGDROP_S_USEDEFAULTCURSORS, DV_E_FORMATETC, DV_E_TYMED, E_NOTIMPL,
    OLE_E_ADVISENOTSUPPORTED, S_FALSE, S_OK,
};
use windows::Win32::System::Com::{
    IAdviseSink, IDataObject, IDataObject_Impl, IEnumFORMATETC,
    IEnumFORMATETC_Impl, IEnumSTATDATA, FORMATETC, STGMEDIUM, TYMED_HGLOBAL,
};
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GHND};
use windows::Win32::System::Ole::{
    DoDragDrop, IDropSource, IDropSource_Impl, OleInitialize, DROPEFFECT,
    DROPEFFECT_COPY, DROPEFFECT_NONE,
};
use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
use windows::Win32::UI::Shell::DROPFILES;

/// CF_HDROP — format clipboard standard pour shell file drops.
const CF_HDROP: u16 = 15;

#[derive(Debug, thiserror::Error)]
pub enum OleError {
    #[error("OleInitialize failed (HRESULT 0x{0:08X})")]
    Init(u32),
    #[error("DoDragDrop failed (HRESULT 0x{0:08X})")]
    DoDrag(u32),
    #[error("path conversion failed")]
    Path,
}

/// Initialise OLE pour le thread courant (à appeler une fois par thread).
/// Idempotent : `S_FALSE` si déjà initialisé est traité comme un succès.
pub fn init_ole_thread() -> Result<(), OleError> {
    unsafe {
        match OleInitialize(None) {
            Ok(()) => Ok(()),
            Err(e) if e.code() == S_FALSE => Ok(()),
            Err(e) => Err(OleError::Init(e.code().0 as u32)),
        }
    }
}

/// Lance un drag-drop OLE pour un chemin de fichier vers une autre app.
/// Bloque le thread courant jusqu'au drop ou cancel.
///
/// # Errors
/// Erreur si la conversion du path échoue ou si `DoDragDrop` retourne
/// un HRESULT autre que `DRAGDROP_S_DROP` / `DRAGDROP_S_CANCEL`.
pub fn drag_file(path: &Path) -> Result<DROPEFFECT, OleError> {
    let path_str = path.to_str().ok_or(OleError::Path)?;
    // UTF-16 + double null terminator (DROPFILES exige un MULTI_SZ).
    let mut path_w: Vec<u16> = path_str.encode_utf16().collect();
    path_w.push(0);
    path_w.push(0);

    let data: IDataObject = MidiDataObject { path_w }.into();
    let source: IDropSource = MidiDropSource.into();
    let mut effect = DROPEFFECT(0);

    let hr = unsafe { DoDragDrop(&data, &source, DROPEFFECT_COPY, &mut effect) };
    if hr == DRAGDROP_S_DROP {
        Ok(effect)
    } else if hr == DRAGDROP_S_CANCEL {
        Ok(DROPEFFECT_NONE)
    } else {
        Err(OleError::DoDrag(hr.0 as u32))
    }
}

// ──────────────────────────────────────────────────────────────────────
// IDataObject : fournit CF_HDROP (le seul format qu'on supporte).
// ──────────────────────────────────────────────────────────────────────

#[implement(IDataObject)]
struct MidiDataObject {
    /// Chemin UTF-16 + double null terminator.
    path_w: Vec<u16>,
}

impl IDataObject_Impl for MidiDataObject_Impl {
    fn GetData(&self, pformatetcin: *const FORMATETC) -> windows::core::Result<STGMEDIUM> {
        let fe = unsafe { &*pformatetcin };
        if fe.cfFormat != CF_HDROP { return Err(DV_E_FORMATETC.into()); }
        if (fe.tymed & TYMED_HGLOBAL.0 as u32) == 0 { return Err(DV_E_TYMED.into()); }

        // Alloue HGLOBAL avec DROPFILES + chemin UTF-16.
        let dropfiles_size = std::mem::size_of::<DROPFILES>();
        let path_bytes = self.path_w.len() * 2;
        let total = dropfiles_size + path_bytes;
        unsafe {
            let hmem = GlobalAlloc(GHND, total).map_err(|_| windows::core::Error::from(E_NOTIMPL))?;
            let p = GlobalLock(hmem) as *mut u8;
            if p.is_null() {
                return Err(E_NOTIMPL.into());
            }
            // DROPFILES header
            let dropfiles = &mut *(p as *mut DROPFILES);
            dropfiles.pFiles = dropfiles_size as u32;
            dropfiles.fWide = BOOL(1);
            // Chemin juste après l'en-tête
            let path_ptr = p.add(dropfiles_size) as *mut u16;
            std::ptr::copy_nonoverlapping(self.path_w.as_ptr(), path_ptr, self.path_w.len());
            let _ = GlobalUnlock(hmem);

            let mut medium: STGMEDIUM = std::mem::zeroed();
            medium.tymed = TYMED_HGLOBAL.0 as u32;
            medium.u.hGlobal = hmem;
            // pUnkForRelease = NULL → le destinataire libère le HGLOBAL via
            // ReleaseStgMedium.
            Ok(medium)
        }
    }

    fn GetDataHere(&self, _pformatetc: *const FORMATETC, _pmedium: *mut STGMEDIUM)
        -> windows::core::Result<()> { Err(E_NOTIMPL.into()) }

    fn QueryGetData(&self, pformatetc: *const FORMATETC) -> windows::core::HRESULT {
        let fe = unsafe { &*pformatetc };
        if fe.cfFormat == CF_HDROP && (fe.tymed & TYMED_HGLOBAL.0 as u32) != 0 {
            S_OK
        } else {
            DV_E_FORMATETC
        }
    }

    fn GetCanonicalFormatEtc(&self, _in: *const FORMATETC, out: *mut FORMATETC)
        -> windows::core::HRESULT
    {
        if !out.is_null() {
            unsafe { (*out).ptd = std::ptr::null_mut(); }
        }
        DATA_S_SAMEFORMATETC
    }

    fn SetData(&self, _: *const FORMATETC, _: *const STGMEDIUM, _: BOOL)
        -> windows::core::Result<()> { Err(E_NOTIMPL.into()) }

    fn EnumFormatEtc(&self, dwdirection: u32) -> windows::core::Result<IEnumFORMATETC> {
        // Direction 1 = DATADIR_GET ; on n'expose qu'un format en lecture.
        if dwdirection == 1 {
            let formats = vec![FORMATETC {
                cfFormat: CF_HDROP,
                ptd: std::ptr::null_mut(),
                dwAspect: 1, // DVASPECT_CONTENT
                lindex: -1,
                tymed: TYMED_HGLOBAL.0 as u32,
            }];
            Ok(MidiEnumFormatEtc { formats, idx: AtomicUsize::new(0) }.into())
        } else {
            Err(E_NOTIMPL.into())
        }
    }

    fn DAdvise(&self, _: *const FORMATETC, _: u32, _: Option<&IAdviseSink>)
        -> windows::core::Result<u32> { Err(OLE_E_ADVISENOTSUPPORTED.into()) }
    fn DUnadvise(&self, _: u32) -> windows::core::Result<()> {
        Err(OLE_E_ADVISENOTSUPPORTED.into())
    }
    fn EnumDAdvise(&self) -> windows::core::Result<IEnumSTATDATA> {
        Err(OLE_E_ADVISENOTSUPPORTED.into())
    }
}

// ──────────────────────────────────────────────────────────────────────
// IEnumFORMATETC : enum trivial sur un seul format (CF_HDROP).
// ──────────────────────────────────────────────────────────────────────

#[implement(IEnumFORMATETC)]
struct MidiEnumFormatEtc {
    formats: Vec<FORMATETC>,
    idx: AtomicUsize,
}

impl IEnumFORMATETC_Impl for MidiEnumFormatEtc_Impl {
    fn Next(&self, celt: u32, rgelt: *mut FORMATETC, pceltfetched: *mut u32)
        -> windows::core::HRESULT
    {
        let i = self.idx.load(Ordering::Relaxed);
        let remaining = self.formats.len().saturating_sub(i);
        let take = (celt as usize).min(remaining);
        unsafe {
            for k in 0..take {
                *rgelt.add(k) = self.formats[i + k];
            }
            if !pceltfetched.is_null() {
                *pceltfetched = take as u32;
            }
        }
        self.idx.store(i + take, Ordering::Relaxed);
        if take as u32 == celt { S_OK } else { S_FALSE }
    }

    fn Skip(&self, celt: u32) -> windows::core::Result<()> {
        let i = self.idx.load(Ordering::Relaxed);
        let new_i = i + celt as usize;
        if new_i > self.formats.len() {
            self.idx.store(self.formats.len(), Ordering::Relaxed);
            Err(S_FALSE.into())
        } else {
            self.idx.store(new_i, Ordering::Relaxed);
            Ok(())
        }
    }

    fn Reset(&self) -> windows::core::Result<()> {
        self.idx.store(0, Ordering::Relaxed);
        Ok(())
    }

    fn Clone(&self) -> windows::core::Result<IEnumFORMATETC> {
        let cloned = MidiEnumFormatEtc {
            formats: self.formats.clone(),
            idx: AtomicUsize::new(self.idx.load(Ordering::Relaxed)),
        };
        Ok(cloned.into())
    }
}

// ──────────────────────────────────────────────────────────────────────
// IDropSource : pilote la durée du drag (continue/cancel + cursor).
// ──────────────────────────────────────────────────────────────────────

#[implement(IDropSource)]
struct MidiDropSource;

impl IDropSource_Impl for MidiDropSource_Impl {
    fn QueryContinueDrag(&self, fescapepressed: BOOL, grfkeystate: MODIFIERKEYS_FLAGS)
        -> windows::core::HRESULT
    {
        // ESC → cancel.
        if fescapepressed.as_bool() {
            return DRAGDROP_S_CANCEL;
        }
        // Bouton gauche relâché → drop.
        const MK_LBUTTON: u32 = 0x0001;
        if (grfkeystate.0 & MK_LBUTTON) == 0 {
            return DRAGDROP_S_DROP;
        }
        S_OK
    }

    fn GiveFeedback(&self, _dweffect: DROPEFFECT) -> windows::core::HRESULT {
        // Laisse Windows gérer le curseur par défaut (curseurs natifs OS).
        DRAGDROP_S_USEDEFAULTCURSORS
    }
}
