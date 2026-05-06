//! Côté GUI : bind un port localhost, lance le worker élevé via UAC,
//! gère l'envoi des Cmd et la réception des Event.
//!
//! Architecture :
//!  - `WorkerHandle::start()` :
//!     1. Crée un `TcpListener` sur 127.0.0.1:0 (port éphémère).
//!     2. Lance le worker via `ShellExecuteExW` avec verb=`runas`
//!        (déclenche l'UAC) et `--worker --port N`.
//!     3. Accepte la connexion entrante, lit `{"event":"ready"}`.
//!     4. Spawn un thread reader qui lit le BufReader<TcpStream> en boucle
//!        et pousse chaque Event dans un `mpsc::Sender<Event>`.
//!  - `WorkerSender::send_cmd(&Cmd)` est cloneable et thread-safe : prend
//!    le mutex du writer, sérialise, envoie une ligne JSON.
//!  - `Drop` du handle : ferme la socket et terminate le process si encore vivant.
//!
//! Port direct de `python/a3000_transfer/gui.py:WorkerClient`, threadé proprement.

#![cfg(windows)]

use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, SocketAddrV4, TcpListener, TcpStream};
use std::sync::{mpsc, Arc, Mutex};
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

/// Côté envoi (cloneable pour réutilisation depuis plusieurs threads).
#[derive(Clone)]
pub struct WorkerSender {
    stream: Arc<Mutex<TcpStream>>,
}

impl WorkerSender {
    pub fn send_cmd(&self, cmd: &Cmd) -> Result<()> {
        let guard = self.stream.lock().map_err(|_| WorkerError::NotConnected)?;
        let mut s = guard.try_clone()?;
        drop(guard);
        let line = serde_json::to_string(cmd)?;
        s.write_all(line.as_bytes())?;
        s.write_all(b"\n")?;
        s.flush()?;
        Ok(())
    }
}

/// Handle complet : sender + receiver d'events + process Win32.
pub struct WorkerHandle {
    pub sender: WorkerSender,
    pub events: mpsc::Receiver<Event>,
    process: Option<HANDLE>,
}

// SAFETY : HANDLE est juste un usize wrapper (pointeur opaque) ; le Win32
// API garantit que CloseHandle/TerminateProcess sont thread-safe.
unsafe impl Send for WorkerHandle {}

impl WorkerHandle {
    /// Lance le worker élevé et attend le ready handshake. Démarre aussi
    /// le thread reader qui pousse les Events suivants dans le channel.
    pub fn start(connect_timeout: Duration) -> Result<Self> {
        let listener = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))?;
        let port = listener.local_addr()?.port();

        let process = launch_worker_elevated(port)?;

        // accept(...) bloquant avec timeout via non-blocking poll.
        listener.set_nonblocking(true)?;
        let deadline = std::time::Instant::now() + connect_timeout;
        let stream = loop {
            match listener.accept() {
                Ok((s, _)) => break s,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    if std::time::Instant::now() >= deadline {
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
        stream.set_read_timeout(None)?;

        let writer_stream = stream.try_clone()?;
        let mut reader = BufReader::new(stream);

        // 1er event = ready.
        let mut line = String::new();
        let n = reader.read_line(&mut line)?;
        if n == 0 {
            return Err(WorkerError::NoReady(None));
        }
        let event: Event = serde_json::from_str(line.trim())?;
        if !matches!(event, Event::Ready) {
            return Err(WorkerError::NoReady(Some(event)));
        }

        // Thread reader : lit le reste indéfiniment, push dans le channel.
        let (event_tx, event_rx) = mpsc::channel::<Event>();
        std::thread::Builder::new()
            .name("worker-reader".into())
            .spawn(move || {
                let mut reader = reader;
                loop {
                    let mut line = String::new();
                    let n = match reader.read_line(&mut line) {
                        Ok(n) => n,
                        Err(_) => break,
                    };
                    if n == 0 {
                        break;
                    }
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    match serde_json::from_str::<Event>(trimmed) {
                        Ok(e) => {
                            if event_tx.send(e).is_err() {
                                break;
                            }
                        }
                        Err(_) => continue,
                    }
                }
            })?;

        Ok(Self {
            sender: WorkerSender {
                stream: Arc::new(Mutex::new(writer_stream)),
            },
            events: event_rx,
            process: Some(process),
        })
    }

    /// Clone le sender (peut servir à partager l'envoi entre threads).
    #[allow(dead_code)] // utilisé en Phase 3d.3 (Upload batch)
    pub fn sender(&self) -> WorkerSender {
        self.sender.clone()
    }

    /// Envoie `exit` au worker ; le reader thread sortira sur EOF.
    pub fn shutdown(&mut self) {
        let _ = self.sender.send_cmd(&Cmd::Exit);
        if let Some(p) = self.process.take() {
            if !p.is_invalid() {
                // On ne TerminateProcess pas : `exit` cmd suffit en général,
                // et on ne veut pas tuer le worker en plein milieu d'un IOCTL.
                unsafe { let _ = CloseHandle(p); }
            }
        }
    }
}

impl Drop for WorkerHandle {
    fn drop(&mut self) {
        self.shutdown();
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
