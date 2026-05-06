//! Côté GUI : bind un port localhost, lance le worker élevé via UAC,
//! gère l'envoi des Cmd et la réception des Event.
//!
//! Architecture :
//!  - GUI (non-élevée) appelle `WorkerClient::start()` qui :
//!     1. Crée un `TcpListener` sur 127.0.0.1:0 (port éphémère).
//!     2. Lance le worker via `ShellExecuteExW` avec verb=`runas`
//!        (déclenche l'UAC) et `--worker --port N`.
//!     3. Accepte la connexion entrante, attend `{"event":"ready"}`.
//!  - Ensuite, `send_cmd` / `recv_event` parlent ce protocol JSON line.
//!  - `stop()` envoie `{"cmd":"exit"}` et ferme tout.
//!
//! Port direct de `python/a3000_transfer/gui.py:WorkerClient`.

#![cfg(windows)]
#![allow(dead_code)] // wiring vers app.rs viendra en Phase 3 plus tardive

use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, SocketAddrV4, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Threading::TerminateProcess;
use windows::Win32::UI::Shell::{
    ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW,
};
use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;

use crate::ipc::{Cmd, Event};

#[derive(Debug, thiserror::Error)]
pub enum WorkerError {
    #[error("worker not connected")]
    NotConnected,
    #[error("worker did not send 'ready' event: {0:?}")]
    NoReady(Option<Event>),
    #[error("ShellExecuteExW failed (LastError={0}). The UAC prompt may have been denied.")]
    ShellExec(u32),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("worker did not connect within {0:?}")]
    AcceptTimeout(Duration),
}

pub type Result<T> = std::result::Result<T, WorkerError>;

/// Client RAII : connexion socket + handle process worker.
pub struct WorkerClient {
    /// Le stream connecté au worker. `None` après `stop`.
    stream: Option<Arc<Mutex<TcpStream>>>,
    /// Reader bufferisé sur un clone du stream (events arrivent ligne par ligne).
    reader: Option<BufReader<TcpStream>>,
    /// Process handle du worker (Windows). Fermé dans Drop.
    process: Option<HANDLE>,
}

impl WorkerClient {
    /// Lance le worker élevé et attend qu'il dise `ready`.
    pub fn start(connect_timeout: Duration) -> Result<Self> {
        let listener = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))?;
        let port = listener.local_addr()?.port();
        listener.set_nonblocking(false)?;

        let process = launch_worker_elevated(port)?;

        // accept(...) bloque mais TcpListener n'a pas de timeout. On utilise
        // set_nonblocking + poll, ou on triche en mettant un read_timeout
        // après accept.
        listener.set_nonblocking(true)?;
        let deadline = std::time::Instant::now() + connect_timeout;
        let stream = loop {
            match listener.accept() {
                Ok((s, _)) => break s,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    if std::time::Instant::now() >= deadline {
                        // Cleanup : termine le worker si lancé mais non-connecté.
                        if !process.is_invalid() {
                            unsafe { let _ = TerminateProcess(process, 1); }
                            unsafe { let _ = CloseHandle(process); }
                        }
                        return Err(WorkerError::AcceptTimeout(connect_timeout));
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(e) => return Err(e.into()),
            }
        };
        stream.set_nonblocking(false)?;
        stream.set_read_timeout(None)?; // bloquant indéfiniment

        let reader_stream = stream.try_clone()?;
        let mut reader = BufReader::new(reader_stream);

        // Lit le premier event, doit être "ready".
        let mut line = String::new();
        let n = reader.read_line(&mut line)?;
        if n == 0 {
            return Err(WorkerError::NoReady(None));
        }
        let event: Event = serde_json::from_str(line.trim())?;
        if !matches!(event, Event::Ready) {
            return Err(WorkerError::NoReady(Some(event)));
        }

        Ok(Self {
            stream: Some(Arc::new(Mutex::new(stream))),
            reader: Some(reader),
            process: Some(process),
        })
    }

    /// Envoie une commande au worker (sérialisée en JSON line).
    pub fn send_cmd(&mut self, cmd: &Cmd) -> Result<()> {
        let stream = self.stream.as_ref().ok_or(WorkerError::NotConnected)?;
        let mut s = stream.lock()
            .map_err(|_| WorkerError::NotConnected)?
            .try_clone()?;
        let line = serde_json::to_string(cmd)?;
        s.write_all(line.as_bytes())?;
        s.write_all(b"\n")?;
        s.flush()?;
        Ok(())
    }

    /// Lit le prochain event (bloque jusqu'à arrivée d'une ligne).
    pub fn recv_event(&mut self) -> Result<Option<Event>> {
        let reader = self.reader.as_mut().ok_or(WorkerError::NotConnected)?;
        let mut line = String::new();
        let n = reader.read_line(&mut line)?;
        if n == 0 {
            return Ok(None);
        }
        Ok(Some(serde_json::from_str(line.trim())?))
    }

    /// Envoie `exit` au worker, attend la sortie, ferme tout.
    pub fn stop(&mut self) {
        if let Some(stream) = self.stream.as_ref() {
            if let Ok(guard) = stream.lock() {
                if let Ok(mut s) = guard.try_clone() {
                    let _ = serde_json::to_writer(&mut s, &Cmd::Exit);
                    let _ = s.write_all(b"\n");
                    let _ = s.flush();
                }
            }
        }
        self.stream = None;
        self.reader = None;
        if let Some(p) = self.process.take() {
            if !p.is_invalid() {
                unsafe { let _ = CloseHandle(p); }
            }
        }
    }
}

impl Drop for WorkerClient {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Lance l'exécutable courant avec `--worker --port N` via UAC.
fn launch_worker_elevated(port: u16) -> Result<HANDLE> {
    let exe = std::env::current_exe()?;
    let exe_w = wide(&exe.to_string_lossy());
    let verb = wide("runas");
    let params = wide(&format!("--worker --port {port}"));

    let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
    info.fMask = SEE_MASK_NOCLOSEPROCESS;
    info.lpVerb = PCWSTR(verb.as_ptr());
    info.lpFile = PCWSTR(exe_w.as_ptr());
    info.lpParameters = PCWSTR(params.as_ptr());
    info.nShow = SW_HIDE.0;

    let ok = unsafe { ShellExecuteExW(&mut info) };
    if ok.is_err() {
        let last = unsafe { windows::Win32::Foundation::GetLastError().0 };
        return Err(WorkerError::ShellExec(last));
    }
    Ok(info.hProcess)
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}
