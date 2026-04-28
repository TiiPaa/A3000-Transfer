"""GUI tkinter (non-admin) pour transférer des WAV vers/depuis le Yamaha A3000.

Architecture split :
- GUI tourne en non-admin → drag'n'drop fonctionne
- Au premier transfert/download, la GUI lance via UAC un sub-process worker admin
- Communication via socket TCP localhost
- Le worker reste vivant tant que la GUI tourne → un seul UAC popup par session

Layout :
- Cadre Cible SCSI en haut (partagé entre onglets)
- Notebook : onglet Upload + onglet Download
- Bouton Interrompre + Status bar en bas
"""
from __future__ import annotations

import json
import os
import queue
import shutil
import socket
import sys
import tarfile
import tempfile
import threading
import tkinter as tk
import zipfile
from dataclasses import dataclass
from pathlib import Path
from tkinter import filedialog, messagebox, ttk

try:
    from tkinterdnd2 import DND_FILES, TkinterDnD
    HAS_DND = True
except ImportError:
    HAS_DND = False

from .wav_reader import WaveValidationError, load_wave


# ─────────────────────────────────────────────────────────────────────────────
# Worker client
# ─────────────────────────────────────────────────────────────────────────────

class WorkerError(Exception):
    pass


class WorkerClient:
    """Lance un worker admin via UAC et communique via socket TCP localhost."""

    def __init__(self) -> None:
        self.server_sock: socket.socket | None = None
        self.client_sock: socket.socket | None = None
        self.client_fp = None
        self.port: int = 0
        self.worker_handle = None
        self._lock = threading.Lock()

    def start(self, timeout: float = 30.0) -> None:
        srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        srv.bind(("127.0.0.1", 0))
        srv.listen(1)
        self.port = srv.getsockname()[1]
        self.server_sock = srv

        self._launch_worker_elevated(self.port)

        srv.settimeout(timeout)
        try:
            conn, _ = srv.accept()
        except socket.timeout as exc:
            raise WorkerError(
                "The admin worker did not connect in time.\n"
                "The UAC prompt may have been denied."
            ) from exc

        conn.settimeout(None)
        self.client_sock = conn
        self.client_fp = conn.makefile("rwb", buffering=0)

        first = self._recv_event()
        if first is None or first.get("event") != "ready":
            raise WorkerError(f"Worker did not send 'ready': {first}")

    def _launch_worker_elevated(self, port: int) -> None:
        if sys.platform != "win32":
            raise WorkerError("This architecture requires Windows (UAC).")
        import ctypes
        from ctypes import wintypes

        SEE_MASK_NOCLOSEPROCESS = 0x00000040
        SW_HIDE = 0

        class SHELLEXECUTEINFO(ctypes.Structure):
            _fields_ = [
                ("cbSize", wintypes.DWORD),
                ("fMask", ctypes.c_ulong),
                ("hwnd", wintypes.HWND),
                ("lpVerb", wintypes.LPCWSTR),
                ("lpFile", wintypes.LPCWSTR),
                ("lpParameters", wintypes.LPCWSTR),
                ("lpDirectory", wintypes.LPCWSTR),
                ("nShow", ctypes.c_int),
                ("hInstApp", wintypes.HINSTANCE),
                ("lpIDList", wintypes.LPVOID),
                ("lpClass", wintypes.LPCWSTR),
                ("hkeyClass", wintypes.HKEY),
                ("dwHotKey", wintypes.DWORD),
                ("hIcon", wintypes.HANDLE),
                ("hProcess", wintypes.HANDLE),
            ]

        info = SHELLEXECUTEINFO()
        info.cbSize = ctypes.sizeof(info)
        info.fMask = SEE_MASK_NOCLOSEPROCESS
        info.lpVerb = "runas"
        info.lpFile = sys.executable
        info.lpParameters = f'-m a3000_transfer._worker --port {port}'
        info.nShow = SW_HIDE

        ctypes.windll.shell32.ShellExecuteExW.argtypes = [ctypes.POINTER(SHELLEXECUTEINFO)]
        ctypes.windll.shell32.ShellExecuteExW.restype = wintypes.BOOL

        if not ctypes.windll.shell32.ShellExecuteExW(ctypes.byref(info)):
            err = ctypes.get_last_error()
            raise WorkerError(
                f"ShellExecuteExW failed (LastError={err}). "
                "The UAC prompt may have been denied."
            )

        self.worker_handle = info.hProcess

    def terminate_worker(self) -> None:
        if self.worker_handle is None:
            return
        try:
            import ctypes
            ctypes.windll.kernel32.TerminateProcess(self.worker_handle, 1)
            ctypes.windll.kernel32.CloseHandle(self.worker_handle)
        except Exception:
            pass
        self.worker_handle = None

    def send_command(self, cmd: dict) -> None:
        with self._lock:
            if self.client_fp is None:
                raise WorkerError("Worker not connected.")
            self.client_fp.write(json.dumps(cmd).encode("utf-8") + b"\n")
            self.client_fp.flush()

    def _recv_event(self) -> dict | None:
        if self.client_fp is None:
            return None
        line = self.client_fp.readline()
        if not line:
            return None
        try:
            return json.loads(line)
        except json.JSONDecodeError:
            return {"event": "error", "msg": f"bad JSON from worker: {line!r}"}

    def recv_event(self) -> dict | None:
        return self._recv_event()

    def stop(self) -> None:
        try:
            if self.client_fp is not None:
                self.send_command({"cmd": "exit"})
        except Exception:
            pass
        for s in (self.client_sock, self.server_sock):
            if s is not None:
                try:
                    s.close()
                except Exception:
                    pass
        self.client_sock = None
        self.server_sock = None
        self.client_fp = None
        if self.worker_handle is not None:
            try:
                import ctypes
                ctypes.windll.kernel32.CloseHandle(self.worker_handle)
            except Exception:
                pass
            self.worker_handle = None


# ─────────────────────────────────────────────────────────────────────────────
# Data classes
# ─────────────────────────────────────────────────────────────────────────────

@dataclass
class UploadItem:
    path: Path
    state: str = "pending"          # pending / running / done / error / cancelled
    progress: float = 0.0
    sent_bytes: int = 0
    total_bytes: int = 0
    sample_slot: int | None = None
    error_msg: str = ""
    file_size: int = 0              # raw file size in bytes
    duration_s: float = 0.0
    fmt_str: str = ""               # cached format string for display
    checked: bool = True             # included in upload batch
    sample_name: str = ""           # name on the A3000, max 16 chars


@dataclass
class DownloadItem:
    slot: int
    name: str
    channels: int
    bits: int
    sample_rate: int
    frames: int
    duration: float
    state: str = "available"        # available / pending / running / done / error / cancelled
    progress: float = 0.0
    sent_bytes: int = 0
    total_bytes: int = 0
    output_path: str = ""
    error_msg: str = ""


@dataclass
class UiEvent:
    """Événement worker thread → main thread tkinter."""
    kind: str                       # 'start' / 'progress' / 'done' / 'error' / 'cancelled' /
                                    # 'allfinished' / 'status' / 'samples_listed'
    tab: str = "upload"             # 'upload' / 'download'
    item_index: int = -1
    sample_slot: int | None = None
    sent: int = 0
    total: int = 0
    msg: str = ""


# ─────────────────────────────────────────────────────────────────────────────
# App
# ─────────────────────────────────────────────────────────────────────────────

class A3000TransferApp:
    def __init__(self, root) -> None:
        self.root = root
        root.title("A3000 Sample Transfer")
        root.geometry("960x600")
        root.protocol("WM_DELETE_WINDOW", self._on_close)

        # State
        self.upload_items: list[UploadItem] = []
        self.download_items: list[DownloadItem] = []
        self.ui_queue: queue.Queue[UiEvent] = queue.Queue()
        self.worker_thread: threading.Thread | None = None
        self.worker_client: WorkerClient | None = None
        self.interrupted = False
        self.current_tab = "upload"  # tab actif pour l'op en cours
        self._temp_dirs: list[Path] = []  # extracted archive dirs, cleaned on close
        self._playing_path: Path | None = None
        self._pending_toggle = None  # after-id for deferred checkbox toggle

        self._build_ui()
        self.root.after(100, self._drain_ui_events)

    # ── UI building ──────────────────────────────────────────────────────────

    def _build_ui(self) -> None:
        # Variables SCSI initialisées depuis la config persistée
        saved = self._load_config()
        self.ha_var = tk.IntVar(value=int(saved.get("ha", 1)))
        self.bus_var = tk.IntVar(value=int(saved.get("bus", 0)))
        self.target_var = tk.IntVar(value=int(saved.get("target", 0)))
        self.lun_var = tk.IntVar(value=int(saved.get("lun", 0)))

        # Notebook
        self.notebook = ttk.Notebook(self.root)
        self.notebook.pack(fill="both", expand=True, padx=10, pady=(10, 5))

        upload_tab = ttk.Frame(self.notebook, padding=8)
        download_tab = ttk.Frame(self.notebook, padding=8)
        self.notebook.add(upload_tab, text="Upload (PC → A3000)")
        self.notebook.add(download_tab, text="Download (A3000 → PC)")

        self._build_upload_tab(upload_tab)
        self._build_download_tab(download_tab)

        # Bottom bar : Interrompre + Status
        bottom = ttk.Frame(self.root)
        bottom.pack(fill="x", padx=10, pady=(5, 10))
        self.stop_btn = ttk.Button(bottom, text="Stop",
                                   command=self._on_stop_click, state="disabled")
        self.stop_btn.pack(side="left")
        ttk.Button(bottom, text="Settings…",
                   command=self._open_settings_dialog).pack(side="right")
        self.status_var = tk.StringVar(value="Ready.")
        ttk.Label(bottom, textvariable=self.status_var, anchor="w").pack(
            side="left", fill="x", expand=True, padx=(8, 8)
        )

    def _build_upload_tab(self, tab) -> None:
        # Slot début : switch Auto / Manual + entry
        opts = ttk.Frame(tab)
        opts.pack(fill="x", pady=(0, 5))
        ttk.Label(opts, text="Start slot:").pack(side="left")
        self.auto_slot_var = tk.BooleanVar(value=True)
        ttk.Checkbutton(opts, text="Auto (first free)",
                        variable=self.auto_slot_var,
                        command=self._update_slot_entry_state).pack(side="left", padx=(8, 0))
        self.manual_slot_var = tk.StringVar(value="1")
        self.manual_slot_entry = ttk.Entry(opts, textvariable=self.manual_slot_var, width=8)
        self.manual_slot_entry.pack(side="left", padx=(8, 0))
        self._update_slot_entry_state()

        # Liste / drop zone
        list_frame = ttk.LabelFrame(tab, text="Files to upload", padding=6)
        list_frame.pack(fill="both", expand=True)

        cols = ("checked", "file", "name", "format", "size", "duration", "slot", "state", "progress")
        self.upload_tree = ttk.Treeview(list_frame, columns=cols, show="headings",
                                         height=10, selectmode="extended")
        for col, label, w, anchor in [
            ("checked", "✓", 30, "center"),
            ("file", "File", 180, "w"),
            ("name", "Sample name", 130, "w"),
            ("format", "Format", 120, "w"),
            ("size", "Size", 65, "e"),
            ("duration", "Duration", 65, "e"),
            ("slot", "Slot", 45, "center"),
            ("state", "State", 65, "center"),
            ("progress", "Progress", 140, "w"),
        ]:
            self.upload_tree.heading(col, text=label)
            self.upload_tree.column(col, width=w, anchor=anchor, stretch=False)
        self.upload_tree.pack(fill="both", expand=True)
        self.upload_tree.bind("<Delete>", lambda _e: self._on_remove_click())
        self.upload_tree.bind("<Double-1>", self._on_tree_double_click)
        self.upload_tree.bind("<Button-1>", self._on_tree_click, add="+")
        self.upload_tree.bind("<F2>", lambda _e: self._on_rename_click())
        self.upload_tree.bind("<Return>", lambda _e: self._on_preview_click())
        self.upload_tree.heading("checked", command=self._on_toggle_all_click)

        if HAS_DND:
            self.root.drop_target_register(DND_FILES)
            self.root.dnd_bind("<<Drop>>", self._on_drop)
        self.root.bind("<Control-v>", self._on_paste)
        self.root.bind("<Control-V>", self._on_paste)

        hint = ("Drag WAV files or archives (.zip / .tar.gz) here  •  Ctrl+V  •  or \"Add…\"\n"
                "8/16/24-bit supported (auto-converted to 16-bit with TPDF dither)")
        if not HAS_DND:
            hint = "Use \"Add files…\" (drag'n'drop unavailable)"
        self.upload_hint = ttk.Label(list_frame, text=hint, foreground="gray", justify="center")
        self.upload_hint.place(relx=0.5, rely=0.5, anchor="center")

        # Boutons
        btns = ttk.Frame(tab)
        btns.pack(fill="x", pady=(5, 0))
        self.upload_add_btn = ttk.Button(btns, text="Add files…", command=self._on_add_click)
        self.upload_add_btn.pack(side="left")
        self.upload_preview_btn = ttk.Button(btns, text="▶ Preview", command=self._on_preview_click)
        self.upload_preview_btn.pack(side="left", padx=(8, 0))
        self.upload_rename_btn = ttk.Button(btns, text="Rename", command=self._on_rename_click)
        self.upload_rename_btn.pack(side="left", padx=(8, 0))
        self.upload_remove_btn = ttk.Button(btns, text="Remove", command=self._on_remove_click)
        self.upload_remove_btn.pack(side="left", padx=(8, 0))
        self.upload_check_all_btn = ttk.Button(btns, text="☑ All",
                                                command=lambda: self._set_all_checked(True))
        self.upload_check_all_btn.pack(side="left", padx=(16, 0))
        self.upload_uncheck_all_btn = ttk.Button(btns, text="☐ None",
                                                  command=lambda: self._set_all_checked(False))
        self.upload_uncheck_all_btn.pack(side="left", padx=(4, 0))
        self.upload_send_btn = ttk.Button(btns, text="Upload", command=self._on_send_click)
        self.upload_send_btn.pack(side="left", padx=(16, 0))
        self.upload_retry_btn = ttk.Button(btns, text="Retry errors", command=self._on_retry_click)
        self.upload_retry_btn.pack(side="left", padx=(8, 0))
        self.upload_clear_btn = ttk.Button(btns, text="Clear list", command=self._on_clear_click)
        self.upload_clear_btn.pack(side="left", padx=(8, 0))

        # Résumé total
        self.upload_summary_var = tk.StringVar(value="")
        ttk.Label(tab, textvariable=self.upload_summary_var,
                  foreground="gray").pack(fill="x", pady=(4, 0))

    def _build_download_tab(self, tab) -> None:
        # Liste des samples du sampler
        list_frame = ttk.LabelFrame(tab, text="Samples on the sampler", padding=6)
        list_frame.pack(fill="both", expand=True)

        cols = ("slot", "name", "format", "duration", "state", "progress")
        self.download_tree = ttk.Treeview(list_frame, columns=cols, show="headings",
                                          height=12, selectmode="extended")
        for col, label, w, anchor in [
            ("slot", "#", 50, "center"),
            ("name", "Name", 180, "w"),
            ("format", "Format", 140, "w"),
            ("duration", "Duration", 80, "e"),
            ("state", "State", 80, "center"),
            ("progress", "Progress", 180, "w"),
        ]:
            self.download_tree.heading(col, text=label)
            self.download_tree.column(col, width=w, anchor=anchor)
        self.download_tree.pack(fill="both", expand=True)

        self.download_hint = ttk.Label(
            list_frame,
            text="Click \"Scan\" to list the samples on the sampler.",
            foreground="gray",
        )
        self.download_hint.place(relx=0.5, rely=0.5, anchor="center")

        # Boutons
        btns = ttk.Frame(tab)
        btns.pack(fill="x", pady=(5, 0))
        self.download_scan_btn = ttk.Button(btns, text="Scan", command=self._on_scan_click)
        self.download_scan_btn.pack(side="left")
        self.download_btn = ttk.Button(btns, text="Download selection…",
                                       command=self._on_download_selection_click)
        self.download_btn.pack(side="left", padx=(8, 0))
        self.download_retry_btn = ttk.Button(btns, text="Retry errors",
                                             command=self._on_download_retry_click)
        self.download_retry_btn.pack(side="left", padx=(8, 0))
        self.download_clear_btn = ttk.Button(btns, text="Clear list",
                                             command=self._on_download_clear_click)
        self.download_clear_btn.pack(side="left", padx=(8, 0))

        ttk.Label(btns, text="Tip: Ctrl+click / Shift+click for multi-select.",
                  foreground="gray").pack(side="right")

    # ── Common helpers ───────────────────────────────────────────────────────

    def _common_scsi(self) -> dict:
        return {
            "ha": int(self.ha_var.get()),
            "bus": int(self.bus_var.get()),
            "target": int(self.target_var.get()),
            "lun": int(self.lun_var.get()),
        }

    # ── Config persistance ──────────────────────────────────────────────────

    @staticmethod
    def _config_path() -> Path:
        if sys.platform == "win32":
            base = Path(os.environ.get("APPDATA") or Path.home())
        else:
            base = Path.home() / ".config"
        return base / "a3000_transfer" / "config.json"

    def _load_config(self) -> dict:
        try:
            with open(self._config_path(), encoding="utf-8") as f:
                return json.load(f)
        except (OSError, json.JSONDecodeError):
            return {}

    def _save_config(self, cfg: dict) -> None:
        p = self._config_path()
        try:
            p.parent.mkdir(parents=True, exist_ok=True)
            with open(p, "w", encoding="utf-8") as f:
                json.dump(cfg, f, indent=2)
        except OSError:
            pass

    def _open_settings_dialog(self) -> None:
        dlg = tk.Toplevel(self.root)
        dlg.title("SCSI Target")
        dlg.transient(self.root)
        dlg.resizable(False, False)

        frame = ttk.Frame(dlg, padding=12)
        frame.pack(fill="both", expand=True)

        tmp_ha = tk.IntVar(value=self.ha_var.get())
        tmp_bus = tk.IntVar(value=self.bus_var.get())
        tmp_target = tk.IntVar(value=self.target_var.get())
        tmp_lun = tk.IntVar(value=self.lun_var.get())

        for i, (label, var) in enumerate([
            ("HA (Host Adapter)", tmp_ha),
            ("Bus (PathId)", tmp_bus),
            ("Target (SCSI ID)", tmp_target),
            ("LUN", tmp_lun),
        ]):
            ttk.Label(frame, text=label).grid(row=i, column=0, sticky="w", pady=2)
            ttk.Entry(frame, textvariable=var, width=8).grid(
                row=i, column=1, padx=(12, 0), pady=2, sticky="w",
            )

        ttk.Label(
            frame,
            text='Run "a3000-transfer scan" in a terminal to find these values.',
            foreground="gray",
        ).grid(row=4, column=0, columnspan=2, pady=(10, 0))

        btns = ttk.Frame(frame)
        btns.grid(row=5, column=0, columnspan=2, pady=(12, 0), sticky="e")

        def _on_ok() -> None:
            self.ha_var.set(int(tmp_ha.get()))
            self.bus_var.set(int(tmp_bus.get()))
            self.target_var.set(int(tmp_target.get()))
            self.lun_var.set(int(tmp_lun.get()))
            self._save_config(self._common_scsi())
            dlg.destroy()

        ttk.Button(btns, text="Cancel", command=dlg.destroy).pack(side="right", padx=(8, 0))
        ttk.Button(btns, text="OK", command=_on_ok).pack(side="right")

        dlg.bind("<Return>", lambda _e: _on_ok())
        dlg.bind("<Escape>", lambda _e: dlg.destroy())
        dlg.grab_set()
        dlg.focus_set()

    def _set_busy(self, tab: str) -> None:
        self.current_tab = tab
        # Désactive tous les boutons d'action
        for b in (self.upload_add_btn, self.upload_send_btn, self.upload_retry_btn, self.upload_clear_btn,
                  self.download_scan_btn, self.download_btn, self.download_retry_btn, self.download_clear_btn):
            b.configure(state="disabled")
        self.stop_btn.configure(state="normal")

    def _set_idle(self) -> None:
        for b in (self.upload_add_btn, self.upload_send_btn, self.upload_retry_btn, self.upload_clear_btn,
                  self.download_scan_btn, self.download_btn, self.download_retry_btn, self.download_clear_btn):
            b.configure(state="normal")
        self.stop_btn.configure(state="disabled")

    def _ensure_worker(self) -> WorkerClient | None:
        if self.worker_client is not None:
            try:
                err = self.worker_client.client_sock.getsockopt(socket.SOL_SOCKET, socket.SO_ERROR)
                if err == 0:
                    return self.worker_client
            except Exception:
                pass
            try:
                self.worker_client.stop()
            except Exception:
                pass
            self.worker_client = None
            self.ui_queue.put(UiEvent(kind="status", msg="Previous worker lost — relaunching UAC..."))

        client = WorkerClient()
        try:
            client.start()
        except WorkerError as exc:
            self.ui_queue.put(UiEvent(kind="error", msg=str(exc)))
            return None
        self.worker_client = client
        self.ui_queue.put(UiEvent(kind="status", msg="Admin worker connected."))
        return client

    def _update_slot_entry_state(self) -> None:
        state = "disabled" if self.auto_slot_var.get() else "normal"
        self.manual_slot_entry.configure(state=state)

    # ── Upload tab handlers ──────────────────────────────────────────────────

    def _on_drop(self, event) -> None:
        paths = self.root.tk.splitlist(event.data)
        self._add_paths(paths, source="drop")

    def _on_paste(self, event=None) -> None:
        if sys.platform != "win32":
            return
        paths = _read_clipboard_filelist_win32()
        self._add_paths(paths, source="paste")

    def _on_add_click(self) -> None:
        paths = filedialog.askopenfilenames(
            title="Add WAV files or archives",
            filetypes=[
                ("WAV or archives", "*.wav *.zip *.tar *.tar.gz *.tgz *.tar.bz2 *.tbz2"),
                ("WAV PCM", "*.wav"),
                ("Archives", "*.zip *.tar *.tar.gz *.tgz *.tar.bz2 *.tbz2"),
                ("All files", "*.*"),
            ],
        )
        self._add_paths(paths, source="picker")

    @staticmethod
    def _is_archive(path: Path) -> bool:
        name = path.name.lower()
        if name.endswith((".tar.gz", ".tar.bz2")):
            return True
        return path.suffix.lower() in {".zip", ".tar", ".tgz", ".tbz2"}

    def _extract_archive(self, archive_path: Path) -> list[Path]:
        size_mb = archive_path.stat().st_size / (1024 * 1024)
        self.status_var.set(f"Extracting {archive_path.name} ({size_mb:.1f} MB)…")
        self.root.config(cursor="watch")
        self.root.update_idletasks()
        try:
            try:
                temp_dir = Path(tempfile.mkdtemp(prefix="a3000_transfer_"))
            except OSError as exc:
                self.status_var.set(f"Cannot create temp dir: {exc}")
                return []
            self._temp_dirs.append(temp_dir)
            try:
                if archive_path.suffix.lower() == ".zip":
                    with zipfile.ZipFile(archive_path) as z:
                        z.extractall(temp_dir)
                else:
                    with tarfile.open(archive_path) as t:
                        t.extractall(temp_dir, filter="data")
            except (zipfile.BadZipFile, tarfile.TarError, OSError) as exc:
                self.status_var.set(f"Failed to extract {archive_path.name}: {exc}")
                return []
            wavs = sorted(p for p in temp_dir.rglob("*")
                          if p.is_file() and p.suffix.lower() == ".wav")
            if not wavs:
                self.status_var.set(f"No WAV files in {archive_path.name}.")
            return wavs
        finally:
            self.root.config(cursor="")

    def _add_paths(self, paths, source: str) -> None:
        expanded: list[Path] = []
        archives_extracted = 0
        for p in paths:
            path = Path(p)
            if not path.is_file():
                continue
            if path.suffix.lower() == ".wav":
                expanded.append(path)
            elif self._is_archive(path):
                wavs = self._extract_archive(path)
                if wavs:
                    archives_extracted += 1
                expanded.extend(wavs)

        added = 0
        for wp in expanded:
            self._add_upload_item(wp)
            added += 1

        if added:
            self.upload_hint.place_forget()
            label = {"drop": "dropped", "paste": "pasted", "picker": "added"}[source]
            s = "s" if added > 1 else ""
            arc = f" ({archives_extracted} archive{'s' if archives_extracted > 1 else ''})" if archives_extracted else ""
            self.status_var.set(f"{added} file{s} {label}{arc}.")
        else:
            n_paths = sum(1 for _ in paths)
            if n_paths:
                self.status_var.set(
                    f"Ignored {n_paths} item(s): only WAV or .zip/.tar archives are accepted."
                )
            elif source == "paste":
                self.status_var.set("Clipboard: no .wav or archive.")

    def _add_upload_item(self, path: Path) -> None:
        try:
            wave = load_wave(path)
            ch = "stereo" if wave.channels == 2 else "mono"
            fmt = f"{wave.sample_rate / 1000:.1f}k {ch} {wave.bits_per_sample}bits"
            total = wave.byte_count
            duration = wave.frame_count / wave.sample_rate if wave.sample_rate else 0
        except WaveValidationError as exc:
            fmt = f"<invalid: {exc}>"
            total = 0
            duration = 0
        try:
            file_size = path.stat().st_size
        except OSError:
            file_size = 0
        sample_name = path.stem[:16]
        item = UploadItem(path=path, total_bytes=total, file_size=file_size,
                          duration_s=duration, fmt_str=fmt, sample_name=sample_name)
        self.upload_items.append(item)
        idx = len(self.upload_items) - 1
        self.upload_tree.insert("", "end", iid=f"u{idx}", values=("",) * 9)
        self._refresh_upload_row(idx)
        self._update_upload_summary()

    def _on_clear_click(self) -> None:
        if self.worker_thread and self.worker_thread.is_alive():
            messagebox.showwarning("Operation in progress", "Cannot clear during a transfer.")
            return
        self._stop_preview()
        self.upload_items.clear()
        for child in self.upload_tree.get_children():
            self.upload_tree.delete(child)
        self.upload_hint.place(relx=0.5, rely=0.5, anchor="center")
        self._update_upload_summary()
        self.status_var.set("Upload list cleared.")

    def _on_tree_click(self, event):
        col = self.upload_tree.identify_column(event.x)
        iid = self.upload_tree.identify_row(event.y)
        if col != "#1" or not iid or not iid.startswith("u"):
            return None
        # Capture la sélection AVANT que Tk ne la réduise. Si la ligne
        # cliquée fait partie d'une multi-sélection, on toggle tout le groupe.
        sel = self.upload_tree.selection()
        if iid in sel and len(sel) > 1:
            indices = tuple(int(i[1:]) for i in sel if i.startswith("u"))
        else:
            indices = (int(iid[1:]),)
        if self._pending_toggle is not None:
            self.root.after_cancel(self._pending_toggle)
        self._pending_toggle = self.root.after(
            250, lambda ix=indices: self._do_toggle_multi(ix)
        )
        return "break"  # empêche Tk de réduire la sélection à la seule ligne cliquée

    def _do_toggle_multi(self, indices) -> None:
        self._pending_toggle = None
        valid = [i for i in indices if 0 <= i < len(self.upload_items)]
        if not valid:
            return
        new_state = not self.upload_items[valid[0]].checked
        for idx in valid:
            self.upload_items[idx].checked = new_state
            self._refresh_upload_row(idx)
        self._update_upload_summary()

    def _on_tree_double_click(self, event) -> None:
        if self._pending_toggle is not None:
            self.root.after_cancel(self._pending_toggle)
            self._pending_toggle = None
        col = self.upload_tree.identify_column(event.x)
        iid = self.upload_tree.identify_row(event.y)
        if not iid or not iid.startswith("u"):
            return
        if col == "#1":
            sel = self.upload_tree.selection()
            if iid in sel and len(sel) > 1:
                indices = tuple(int(i[1:]) for i in sel if i.startswith("u"))
            else:
                indices = (int(iid[1:]),)
            self._do_toggle_multi(indices)
            return
        if col == "#3":
            self._on_rename_click()
            return
        self._on_preview_click()

    def _on_toggle_all_click(self) -> None:
        if not self.upload_items:
            return
        all_checked = all(it.checked for it in self.upload_items)
        self._set_all_checked(not all_checked)

    def _set_all_checked(self, value: bool) -> None:
        for idx, it in enumerate(self.upload_items):
            it.checked = value
            self._refresh_upload_row(idx)
        self._update_upload_summary()

    def _update_upload_summary(self) -> None:
        n_total = len(self.upload_items)
        if n_total == 0:
            self.upload_summary_var.set("")
            return
        checked = [it for it in self.upload_items if it.checked]
        total_bytes = sum(it.file_size for it in checked)
        total_dur = sum(it.duration_s for it in checked)
        size_str = self._format_size(total_bytes) if total_bytes else "0 B"
        dur_str = self._format_duration(total_dur) if total_dur else "0s"
        self.upload_summary_var.set(
            f"{len(checked)} / {n_total} files checked  •  {size_str}  •  {dur_str}"
        )

    def _on_rename_click(self) -> None:
        sel = self.upload_tree.selection()
        if not sel:
            self.status_var.set("Select a row to rename.")
            return
        iid = sel[0]
        if not iid.startswith("u"):
            return
        idx = int(iid[1:])
        item = self.upload_items[idx]

        dlg = tk.Toplevel(self.root)
        dlg.title("Rename sample")
        dlg.transient(self.root)
        dlg.resizable(False, False)
        frame = ttk.Frame(dlg, padding=12)
        frame.pack(fill="both", expand=True)
        ttk.Label(frame, text="Sample name (max 16 chars):").pack(anchor="w")
        var = tk.StringVar(value=item.sample_name)
        entry = ttk.Entry(frame, textvariable=var, width=20)
        entry.pack(fill="x", pady=(4, 8))
        entry.icursor("end")
        entry.select_range(0, "end")
        entry.focus_set()

        def _on_ok() -> None:
            new = var.get().strip()[:16]
            if new:
                item.sample_name = new
                self._refresh_upload_row(idx)
            dlg.destroy()

        btns = ttk.Frame(frame)
        btns.pack(fill="x")
        ttk.Button(btns, text="Cancel", command=dlg.destroy).pack(side="right", padx=(8, 0))
        ttk.Button(btns, text="OK", command=_on_ok).pack(side="right")
        dlg.bind("<Return>", lambda _e: _on_ok())
        dlg.bind("<Escape>", lambda _e: dlg.destroy())
        dlg.grab_set()

    def _on_remove_click(self) -> None:
        if self.worker_thread and self.worker_thread.is_alive():
            self.status_var.set("Cannot remove during a transfer.")
            return
        sel = self.upload_tree.selection()
        indices = sorted({int(iid[1:]) for iid in sel if iid.startswith("u")},
                         reverse=True)
        if not indices:
            self.status_var.set("Select one or more rows to remove.")
            return
        for idx in indices:
            removed = self.upload_items.pop(idx)
            if self._playing_path == removed.path:
                self._stop_preview()
        for child in self.upload_tree.get_children():
            self.upload_tree.delete(child)
        for i in range(len(self.upload_items)):
            self.upload_tree.insert("", "end", iid=f"u{i}", values=("",) * 9)
            self._refresh_upload_row(i)
        if not self.upload_items:
            self.upload_hint.place(relx=0.5, rely=0.5, anchor="center")
        self._update_upload_summary()
        n = len(indices)
        self.status_var.set(f"Removed {n} item{'s' if n > 1 else ''}.")

    def _on_preview_click(self) -> None:
        if sys.platform != "win32":
            self.status_var.set("Preview only available on Windows.")
            return
        sel = self.upload_tree.selection()
        idx = None
        if sel:
            iid = sel[0]
            if iid.startswith("u"):
                idx = int(iid[1:])
        if idx is None:
            self.status_var.set("Select a row to preview.")
            return
        item = self.upload_items[idx]
        if self._playing_path == item.path:
            self._stop_preview()
            return
        try:
            import winsound
            winsound.PlaySound(str(item.path),
                               winsound.SND_FILENAME | winsound.SND_ASYNC)
            self._playing_path = item.path
            self.status_var.set(f"Playing: {item.path.name}")
        except RuntimeError as exc:
            self.status_var.set(f"Preview failed: {exc}")

    def _stop_preview(self) -> None:
        if sys.platform != "win32" or self._playing_path is None:
            return
        try:
            import winsound
            winsound.PlaySound(None, winsound.SND_PURGE)
        except RuntimeError:
            pass
        self._playing_path = None

    def _on_retry_click(self) -> None:
        if self.worker_thread and self.worker_thread.is_alive():
            self.status_var.set("Operation in progress, wait before retrying.")
            return
        retried = 0
        for idx, it in enumerate(self.upload_items):
            if it.state in ("error", "cancelled"):
                it.state = "pending"
                it.progress = 0
                it.sent_bytes = 0
                it.error_msg = ""
                self._refresh_upload_row(idx)
                retried += 1
        self.status_var.set(
            f"{retried} item{'s' if retried > 1 else ''} ready to retry."
            if retried else "Nothing to retry."
        )

    def _on_send_click(self) -> None:
        if not self.upload_items:
            self.status_var.set("Nothing to upload.")
            return
        if self.worker_thread and self.worker_thread.is_alive():
            self.status_var.set("Operation already running.")
            return
        if not any(it.state == "pending" and it.checked for it in self.upload_items):
            self.status_var.set("No checked file pending. Check items or retry errors.")
            return
        start_slot = "auto" if self.auto_slot_var.get() else self.manual_slot_var.get().strip()
        config = {**self._common_scsi(), "start_slot": start_slot}
        self._set_busy("upload")
        self.status_var.set("Starting admin worker (UAC prompt on first launch)…")
        self.worker_thread = threading.Thread(
            target=self._upload_thread_run, args=(config,), daemon=True,
        )
        self.worker_thread.start()

    def _upload_thread_run(self, config: dict) -> None:
        self.interrupted = False
        client = self._ensure_worker()
        if client is None:
            self.ui_queue.put(UiEvent(kind="allfinished"))
            return
        common = {k: int(v) if k != "start_slot" else v for k, v in config.items()}

        # Slot de départ
        start_slot_str = config["start_slot"].lower()
        if start_slot_str in ("auto", ""):
            try:
                client.send_command({
                    "cmd": "find_free_slot",
                    "ha": int(config["ha"]), "bus": int(config["bus"]),
                    "target": int(config["target"]), "lun": int(config["lun"]),
                })
                ev = client.recv_event()
            except Exception as exc:
                self.ui_queue.put(UiEvent(kind="error", msg=f"find_free_slot: {exc}"))
                self.ui_queue.put(UiEvent(kind="allfinished"))
                return
            if ev is None or ev.get("event") != "free_slot":
                self.ui_queue.put(UiEvent(kind="error", msg=f"find_free_slot: {ev}"))
                self.ui_queue.put(UiEvent(kind="allfinished"))
                return
            next_slot = int(ev["slot"])
        else:
            try:
                next_slot = int(start_slot_str)
            except ValueError:
                self.ui_queue.put(UiEvent(kind="error", msg=f"Invalid start slot: {start_slot_str!r}"))
                self.ui_queue.put(UiEvent(kind="allfinished"))
                return

        for idx, item in enumerate(self.upload_items):
            if item.state != "pending" or not item.checked:
                continue
            self.ui_queue.put(UiEvent(kind="start", tab="upload",
                                      item_index=idx, sample_slot=next_slot))
            try:
                client.send_command({
                    "cmd": "transfer",
                    "wave_path": str(item.path),
                    "sample_number": next_slot,
                    "name": (item.sample_name or item.path.stem)[:16],
                    "ha": int(config["ha"]), "bus": int(config["bus"]),
                    "target": int(config["target"]), "lun": int(config["lun"]),
                })
                done = False
                while not done:
                    ev = client.recv_event()
                    if ev is None:
                        if self.interrupted:
                            self.ui_queue.put(UiEvent(kind="cancelled", tab="upload",
                                                      item_index=idx, msg="Cancelled"))
                        else:
                            self.ui_queue.put(UiEvent(kind="error", tab="upload",
                                                      item_index=idx,
                                                      msg="Worker disconnected."))
                        self.worker_client = None
                        self.ui_queue.put(UiEvent(kind="allfinished"))
                        return
                    e = ev.get("event")
                    if e == "progress":
                        self.ui_queue.put(UiEvent(kind="progress", tab="upload",
                                                  item_index=idx,
                                                  sent=int(ev["sent"]), total=int(ev["total"])))
                    elif e == "done":
                        self.ui_queue.put(UiEvent(kind="done", tab="upload",
                                                  item_index=idx,
                                                  sample_slot=int(ev["sample_number"])))
                        done = True
                    elif e == "error":
                        self.ui_queue.put(UiEvent(kind="error", tab="upload",
                                                  item_index=idx, msg=ev.get("msg", "?")))
                        done = True
            except Exception as exc:
                if self.interrupted:
                    self.ui_queue.put(UiEvent(kind="cancelled", tab="upload",
                                              item_index=idx, msg="Cancelled"))
                    self.ui_queue.put(UiEvent(kind="allfinished"))
                    return
                self.ui_queue.put(UiEvent(kind="error", tab="upload",
                                          item_index=idx,
                                          msg=f"Worker communication: {exc}"))
                continue
            next_slot += 1

        self.ui_queue.put(UiEvent(kind="allfinished"))

    # ── Download tab handlers ────────────────────────────────────────────────

    def _on_scan_click(self) -> None:
        if self.worker_thread and self.worker_thread.is_alive():
            self.status_var.set("Operation already running.")
            return
        config = self._common_scsi()
        self._set_busy("download")
        self.status_var.set("Scanning samples…")
        self.worker_thread = threading.Thread(
            target=self._scan_thread_run, args=(config,), daemon=True,
        )
        self.worker_thread.start()

    def _scan_thread_run(self, config: dict) -> None:
        self.interrupted = False
        client = self._ensure_worker()
        if client is None:
            self.ui_queue.put(UiEvent(kind="allfinished"))
            return
        try:
            client.send_command({"cmd": "list_samples", "start": 0, "limit": 128, **config})
            samples = None
            while samples is None:
                ev = client.recv_event()
                if ev is None:
                    if not self.interrupted:
                        self.ui_queue.put(UiEvent(kind="error", msg="Worker disconnected during scan."))
                    self.worker_client = None
                    self.ui_queue.put(UiEvent(kind="allfinished"))
                    return
                e = ev.get("event")
                if e == "scan_progress":
                    self.ui_queue.put(UiEvent(kind="status",
                                              msg=f"Scan: {ev['scanned']} slots, {ev['found']} samples found…"))
                elif e == "samples_list":
                    samples = ev["samples"]
                elif e == "error":
                    self.ui_queue.put(UiEvent(kind="error", msg=ev.get("msg", "?")))
                    self.ui_queue.put(UiEvent(kind="allfinished"))
                    return
        except Exception as exc:
            if not self.interrupted:
                self.ui_queue.put(UiEvent(kind="error", msg=f"Worker communication: {exc}"))
            self.ui_queue.put(UiEvent(kind="allfinished"))
            return

        self.ui_queue.put(UiEvent(kind="samples_listed", msg=json.dumps(samples)))
        self.ui_queue.put(UiEvent(kind="allfinished"))

    def _populate_download_list(self, samples: list) -> None:
        self.download_items.clear()
        for child in self.download_tree.get_children():
            self.download_tree.delete(child)
        for s in samples:
            it = DownloadItem(
                slot=int(s["slot"]),
                name=s["name"],
                channels=int(s["channels"]),
                bits=int(s["bits"]),
                sample_rate=int(s["sample_rate"]),
                frames=int(s["frames"]),
                duration=float(s.get("duration") or 0),
            )
            self.download_items.append(it)
            self._insert_download_row(len(self.download_items) - 1)
        if self.download_items:
            self.download_hint.place_forget()
            self.status_var.set(f"{len(self.download_items)} samples found.")
        else:
            self.download_hint.place(relx=0.5, rely=0.5, anchor="center")
            self.status_var.set("No samples found.")

    def _insert_download_row(self, idx: int) -> None:
        it = self.download_items[idx]
        ch = "stereo" if it.channels == 2 else "mono"
        sr = f"{it.sample_rate / 1000:.1f}k" if it.sample_rate else "?"
        fmt = f"{sr} {ch} {it.bits}bits"
        duration = f"{it.duration:.2f}s" if it.duration else "—"
        self.download_tree.insert("", "end", iid=f"d{idx}",
                                  values=(it.slot, it.name, fmt, duration, it.state, ""))

    def _on_download_clear_click(self) -> None:
        if self.worker_thread and self.worker_thread.is_alive():
            messagebox.showwarning("Operation in progress", "Cannot clear during a transfer.")
            return
        self.download_items.clear()
        for child in self.download_tree.get_children():
            self.download_tree.delete(child)
        self.download_hint.place(relx=0.5, rely=0.5, anchor="center")
        self.status_var.set("Sample list cleared.")

    def _on_download_retry_click(self) -> None:
        if self.worker_thread and self.worker_thread.is_alive():
            self.status_var.set("Operation in progress, wait before retrying.")
            return
        retried = 0
        for idx, it in enumerate(self.download_items):
            if it.state in ("error", "cancelled"):
                it.state = "pending"
                it.progress = 0
                it.sent_bytes = 0
                it.error_msg = ""
                self._refresh_download_row(idx)
                retried += 1
        self.status_var.set(
            f"{retried} item{'s' if retried > 1 else ''} ready to retry."
            if retried else "Nothing to retry."
        )

    def _on_download_selection_click(self) -> None:
        if self.worker_thread and self.worker_thread.is_alive():
            self.status_var.set("Operation in progress, wait for it to finish.")
            return
        # Récupère les indices sélectionnés OU ceux en pending (retry)
        sel = self.download_tree.selection()
        sel_indices = [int(iid[1:]) for iid in sel if iid.startswith("d")]
        pending_indices = [i for i, it in enumerate(self.download_items) if it.state == "pending"]

        # Logique : si on a sélectionné, on prend la sélection. Sinon on prend les pending.
        if sel_indices:
            indices = sel_indices
        elif pending_indices:
            indices = pending_indices
        else:
            self.status_var.set("Select one or more samples (Ctrl+click).")
            return

        # Demande le dossier de destination
        out_dir = filedialog.askdirectory(title="Destination folder",
                                          mustexist=True)
        if not out_dir:
            return
        out_dir_path = Path(out_dir)

        # Préparer chaque item : compute output_path, set state pending
        for idx in indices:
            it = self.download_items[idx]
            safe_name = "".join(ch if ch.isalnum() or ch in " ._-" else "_"
                                for ch in (it.name or f"sample_{it.slot:03d}")).strip()
            if not safe_name:
                safe_name = f"sample_{it.slot:03d}"
            it.output_path = str(out_dir_path / f"{safe_name}.wav")
            it.state = "pending"
            it.progress = 0
            it.sent_bytes = 0
            it.error_msg = ""
            self._refresh_download_row(idx)

        config = {**self._common_scsi(), "indices": indices}
        self._set_busy("download")
        self.status_var.set(f"Downloading {len(indices)} sample(s)…")
        self.worker_thread = threading.Thread(
            target=self._download_thread_run, args=(config,), daemon=True,
        )
        self.worker_thread.start()

    def _download_thread_run(self, config: dict) -> None:
        self.interrupted = False
        client = self._ensure_worker()
        if client is None:
            self.ui_queue.put(UiEvent(kind="allfinished"))
            return
        common = {"ha": int(config["ha"]), "bus": int(config["bus"]),
                  "target": int(config["target"]), "lun": int(config["lun"])}

        for idx in config["indices"]:
            item = self.download_items[idx]
            if item.state != "pending":
                continue
            self.ui_queue.put(UiEvent(kind="start", tab="download",
                                      item_index=idx, sample_slot=item.slot))
            try:
                client.send_command({
                    "cmd": "receive",
                    "sample_number": item.slot,
                    "output_path": item.output_path,
                    **common,
                })
                done = False
                while not done:
                    ev = client.recv_event()
                    if ev is None:
                        if self.interrupted:
                            self.ui_queue.put(UiEvent(kind="cancelled", tab="download",
                                                      item_index=idx, msg="Cancelled"))
                        else:
                            self.ui_queue.put(UiEvent(kind="error", tab="download",
                                                      item_index=idx,
                                                      msg="Worker disconnected."))
                        self.worker_client = None
                        self.ui_queue.put(UiEvent(kind="allfinished"))
                        return
                    e = ev.get("event")
                    if e == "progress":
                        self.ui_queue.put(UiEvent(kind="progress", tab="download",
                                                  item_index=idx,
                                                  sent=int(ev["sent"]), total=int(ev["total"])))
                    elif e == "received":
                        self.ui_queue.put(UiEvent(kind="done", tab="download", item_index=idx))
                        done = True
                    elif e == "error":
                        self.ui_queue.put(UiEvent(kind="error", tab="download",
                                                  item_index=idx, msg=ev.get("msg", "?")))
                        done = True
            except Exception as exc:
                if self.interrupted:
                    self.ui_queue.put(UiEvent(kind="cancelled", tab="download",
                                              item_index=idx, msg="Cancelled"))
                    self.ui_queue.put(UiEvent(kind="allfinished"))
                    return
                self.ui_queue.put(UiEvent(kind="error", tab="download",
                                          item_index=idx,
                                          msg=f"Worker communication: {exc}"))
                continue

        self.ui_queue.put(UiEvent(kind="allfinished"))

    # ── Stop ─────────────────────────────────────────────────────────────────

    def _on_stop_click(self) -> None:
        if not (self.worker_thread and self.worker_thread.is_alive()):
            return
        self.interrupted = True
        if self.worker_client is not None:
            try:
                self.worker_client.terminate_worker()
            except Exception:
                pass
            try:
                self.worker_client.stop()
            except Exception:
                pass
            self.worker_client = None
        self.status_var.set("Stop requested — worker killed.")

    # ── UI events pump ───────────────────────────────────────────────────────

    def _drain_ui_events(self) -> None:
        try:
            while True:
                ev = self.ui_queue.get_nowait()
                self._handle_ui_event(ev)
        except queue.Empty:
            pass
        self.root.after(100, self._drain_ui_events)

    def _handle_ui_event(self, ev: UiEvent) -> None:
        if ev.kind == "status":
            self.status_var.set(ev.msg)
            return
        if ev.kind == "samples_listed":
            samples = json.loads(ev.msg)
            self._populate_download_list(samples)
            return
        if ev.kind == "allfinished":
            self._set_idle()
            if self.current_tab == "upload":
                done_n = sum(1 for it in self.upload_items if it.state == "done")
                err_n = sum(1 for it in self.upload_items if it.state == "error")
                self.status_var.set(f"Finished. {done_n} uploaded, "
                                    f"{err_n} error{'s' if err_n > 1 else ''}.")
            elif self.current_tab == "download":
                done_n = sum(1 for it in self.download_items if it.state == "done")
                err_n = sum(1 for it in self.download_items if it.state == "error")
                if done_n + err_n > 0:
                    self.status_var.set(f"Finished. {done_n} downloaded, "
                                        f"{err_n} error{'s' if err_n > 1 else ''}.")
            return
        if ev.kind == "error" and ev.item_index < 0:
            self.status_var.set(f"Error: {ev.msg}")
            messagebox.showerror("Error", ev.msg)
            return

        # Events liés à un item spécifique d'un onglet
        if ev.tab == "upload":
            self._handle_upload_event(ev)
        else:
            self._handle_download_event(ev)

    def _handle_upload_event(self, ev: UiEvent) -> None:
        if ev.item_index < 0 or ev.item_index >= len(self.upload_items):
            return
        it = self.upload_items[ev.item_index]
        if ev.kind == "start":
            it.state = "running"
            it.sample_slot = ev.sample_slot
            self.status_var.set(f"Uploading: {it.path.name} → slot #{ev.sample_slot}")
        elif ev.kind == "progress":
            it.sent_bytes = ev.sent
            it.total_bytes = ev.total
            it.progress = ev.sent / ev.total if ev.total else 0
        elif ev.kind == "done":
            it.state = "done"
            it.progress = 1.0
        elif ev.kind == "error":
            it.state = "error"
            it.error_msg = ev.msg
            self.status_var.set(f"Error: {it.path.name} — {ev.msg}")
        elif ev.kind == "cancelled":
            it.state = "cancelled"
            it.error_msg = ev.msg or "Cancelled"
        self._refresh_upload_row(ev.item_index)

    def _handle_download_event(self, ev: UiEvent) -> None:
        if ev.item_index < 0 or ev.item_index >= len(self.download_items):
            return
        it = self.download_items[ev.item_index]
        if ev.kind == "start":
            it.state = "running"
            self.status_var.set(f"Downloading: sample #{it.slot} '{it.name}'")
        elif ev.kind == "progress":
            it.sent_bytes = ev.sent
            it.total_bytes = ev.total
            it.progress = ev.sent / ev.total if ev.total else 0
        elif ev.kind == "done":
            it.state = "done"
            it.progress = 1.0
        elif ev.kind == "error":
            it.state = "error"
            it.error_msg = ev.msg
            self.status_var.set(f"Error: sample #{it.slot} — {ev.msg}")
        elif ev.kind == "cancelled":
            it.state = "cancelled"
            it.error_msg = ev.msg or "Cancelled"
        self._refresh_download_row(ev.item_index)

    def _refresh_upload_row(self, idx: int) -> None:
        it = self.upload_items[idx]
        slot_str = f"#{it.sample_slot}" if it.sample_slot is not None else ""
        progress_str = self._progress_text(it.state, it.progress, it.sent_bytes,
                                           it.total_bytes, it.error_msg)
        size_str = self._format_size(it.file_size)
        duration_str = self._format_duration(it.duration_s)
        check_str = "☑" if it.checked else "☐"
        self.upload_tree.item(f"u{idx}", values=(
            check_str, it.path.name, it.sample_name, it.fmt_str,
            size_str, duration_str, slot_str, it.state, progress_str,
        ))

    @staticmethod
    def _format_size(bytes_: int) -> str:
        if bytes_ <= 0:
            return ""
        if bytes_ < 1024:
            return f"{bytes_} B"
        if bytes_ < 1024 * 1024:
            return f"{bytes_ / 1024:.1f} KB"
        return f"{bytes_ / (1024 * 1024):.1f} MB"

    @staticmethod
    def _format_duration(seconds: float) -> str:
        if seconds <= 0:
            return ""
        if seconds < 60:
            return f"{seconds:.2f}s"
        m, s = divmod(int(seconds), 60)
        return f"{m}:{s:02d}"

    def _refresh_download_row(self, idx: int) -> None:
        it = self.download_items[idx]
        ch = "stereo" if it.channels == 2 else "mono"
        sr = f"{it.sample_rate / 1000:.1f}k" if it.sample_rate else "?"
        fmt = f"{sr} {ch} {it.bits}bits"
        duration = f"{it.duration:.2f}s" if it.duration else "—"
        progress_str = self._progress_text(it.state, it.progress, it.sent_bytes,
                                           it.total_bytes, it.error_msg)
        self.download_tree.item(f"d{idx}",
                                values=(it.slot, it.name, fmt, duration, it.state, progress_str))

    @staticmethod
    def _progress_text(state: str, progress: float, sent: int, total: int, error_msg: str) -> str:
        if state == "running":
            pct = int(progress * 100)
            return f"{pct:3d}%  {sent}/{total}"
        if state == "done":
            return "OK"
        if state == "error":
            return error_msg[:40]
        if state == "cancelled":
            return "Cancelled"
        return ""

    # ── Cleanup ──────────────────────────────────────────────────────────────

    def _on_close(self) -> None:
        self._stop_preview()
        if self.worker_client is not None:
            try:
                self.worker_client.stop()
            except Exception:
                pass
        for d in self._temp_dirs:
            shutil.rmtree(d, ignore_errors=True)
        self.root.destroy()


# ─────────────────────────────────────────────────────────────────────────────
# Helpers Win32
# ─────────────────────────────────────────────────────────────────────────────

def _read_clipboard_filelist_win32() -> list[str]:
    if sys.platform != "win32":
        return []
    import ctypes
    from ctypes import wintypes

    user32 = ctypes.windll.user32
    shell32 = ctypes.windll.shell32

    CF_HDROP = 15
    user32.OpenClipboard.argtypes = [wintypes.HWND]
    user32.OpenClipboard.restype = wintypes.BOOL
    user32.CloseClipboard.restype = wintypes.BOOL
    user32.GetClipboardData.argtypes = [wintypes.UINT]
    user32.GetClipboardData.restype = wintypes.HANDLE
    shell32.DragQueryFileW.argtypes = [wintypes.HANDLE, wintypes.UINT,
                                       wintypes.LPWSTR, wintypes.UINT]
    shell32.DragQueryFileW.restype = wintypes.UINT

    paths: list[str] = []
    if not user32.OpenClipboard(0):
        return paths
    try:
        h = user32.GetClipboardData(CF_HDROP)
        if not h:
            return paths
        count = shell32.DragQueryFileW(h, 0xFFFFFFFF, None, 0)
        for i in range(count):
            length = shell32.DragQueryFileW(h, i, None, 0)
            buf = ctypes.create_unicode_buffer(length + 1)
            shell32.DragQueryFileW(h, i, buf, length + 1)
            paths.append(buf.value)
    finally:
        user32.CloseClipboard()
    return paths


# ─────────────────────────────────────────────────────────────────────────────
# Entry point
# ─────────────────────────────────────────────────────────────────────────────

def main() -> int:
    if HAS_DND:
        root = TkinterDnD.Tk()
    else:
        root = tk.Tk()
    A3000TransferApp(root)
    root.mainloop()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
