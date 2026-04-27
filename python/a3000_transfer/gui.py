"""GUI tkinter (non-admin) pour transférer des WAV vers le Yamaha A3000.

Architecture split :
- GUI tourne en non-admin → drag'n'drop fonctionne (Explorer non-admin → app non-admin)
- Au premier transfert, la GUI lance via UAC un sub-process worker admin
  (`python -m a3000_transfer._worker`) qui fait les commandes SCSI
- Communication via socket TCP localhost (cross-privilege OK contrairement aux
  pipes stdin/stdout qui sont bloquées par UIPI)
- Le worker reste vivant tant que la GUI tourne → un seul UAC popup par session
"""
from __future__ import annotations

import json
import queue
import socket
import sys
import threading
import tkinter as tk
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
# Worker client — gère le subprocess admin et la socket TCP localhost
# ─────────────────────────────────────────────────────────────────────────────

class WorkerError(Exception):
    pass


class WorkerClient:
    """Lance un worker admin via UAC et communique via socket TCP."""

    def __init__(self) -> None:
        self.server_sock: socket.socket | None = None
        self.client_sock: socket.socket | None = None
        self.client_fp = None  # file-like
        self.port: int = 0
        self.worker_handle = None  # HANDLE du process pour TerminateProcess
        self._lock = threading.Lock()

    def start(self, timeout: float = 30.0) -> None:
        """Bind un port libre, lance le worker via UAC, attend sa connexion."""
        # Bind un port libre sur localhost
        srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        srv.bind(("127.0.0.1", 0))
        srv.listen(1)
        self.port = srv.getsockname()[1]
        self.server_sock = srv

        # Lancer le worker via UAC (popup utilisateur)
        self._launch_worker_elevated(self.port)

        # Attendre la connexion
        srv.settimeout(timeout)
        try:
            conn, _addr = srv.accept()
        except socket.timeout as exc:
            raise WorkerError(
                "Le worker admin ne s'est pas connecté dans le délai imparti.\n"
                "L'invite UAC a peut-être été refusée."
            ) from exc

        conn.settimeout(None)
        self.client_sock = conn
        self.client_fp = conn.makefile("rwb", buffering=0)

        # Premier event = "ready"
        first = self._recv_event(blocking=True)
        if first is None or first.get("event") != "ready":
            raise WorkerError(f"Worker n'a pas envoyé 'ready' : {first}")

    def _launch_worker_elevated(self, port: int) -> None:
        """ShellExecuteExW avec verb 'runas' (popup UAC) ; on récupère le HANDLE
        du process pour pouvoir le TerminateProcess plus tard."""
        if sys.platform != "win32":
            raise WorkerError("Cette architecture nécessite Windows (UAC).")
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
                f"ShellExecuteExW a échoué (LastError={err}). "
                "L'élévation UAC a peut-être été refusée."
            )

        self.worker_handle = info.hProcess

    def terminate_worker(self) -> None:
        """Tue le worker process via TerminateProcess. À utiliser pour interrompre."""
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
                raise WorkerError("Worker non connecté.")
            line = json.dumps(cmd).encode("utf-8") + b"\n"
            self.client_fp.write(line)
            self.client_fp.flush()

    def _recv_event(self, blocking: bool = True) -> dict | None:
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
        """Lit un event en bloquant. Retourne None si fermé."""
        return self._recv_event(blocking=True)

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
        # Cleanup HANDLE process si on l'a encore
        if self.worker_handle is not None:
            try:
                import ctypes
                ctypes.windll.kernel32.CloseHandle(self.worker_handle)
            except Exception:
                pass
            self.worker_handle = None


# ─────────────────────────────────────────────────────────────────────────────
# UI state
# ─────────────────────────────────────────────────────────────────────────────

@dataclass
class QueueItem:
    path: Path
    state: str = "pending"        # pending / running / done / error
    progress: float = 0.0         # 0..1
    sent_bytes: int = 0
    total_bytes: int = 0
    sample_slot: int | None = None
    error_msg: str = ""


@dataclass
class UiEvent:
    """Événement posté par le worker thread vers le main thread tkinter."""
    kind: str                     # 'start' / 'progress' / 'done' / 'error' / 'allfinished' / 'status'
    item_index: int = -1
    sample_slot: int | None = None
    sent: int = 0
    total: int = 0
    msg: str = ""


# ─────────────────────────────────────────────────────────────────────────────
# Application
# ─────────────────────────────────────────────────────────────────────────────

class A3000TransferApp:
    def __init__(self, root) -> None:
        self.root = root
        root.title("A3000 Sample Transfer")
        root.geometry("750x520")
        root.protocol("WM_DELETE_WINDOW", self._on_close)

        self.items: list[QueueItem] = []
        self.ui_queue: queue.Queue[UiEvent] = queue.Queue()
        self.worker_thread: threading.Thread | None = None
        self.worker_client: WorkerClient | None = None
        self.interrupted = False  # set par _on_stop_click pour différencier annulation vs erreur

        self._build_ui()
        self.root.after(100, self._drain_ui_events)

    # ── UI building ──────────────────────────────────────────────────────────

    def _build_ui(self) -> None:
        # Cible SCSI
        cfg = ttk.LabelFrame(self.root, text="Cible SCSI", padding=8)
        cfg.pack(fill="x", padx=10, pady=(10, 5))

        self.ha_var = tk.IntVar(value=1)
        self.bus_var = tk.IntVar(value=0)
        self.target_var = tk.IntVar(value=0)
        self.lun_var = tk.IntVar(value=0)
        self.start_slot_var = tk.StringVar(value="auto")

        for i, (label, var, w) in enumerate([
            ("HA", self.ha_var, 4),
            ("Bus", self.bus_var, 4),
            ("Target", self.target_var, 4),
            ("LUN", self.lun_var, 4),
            ("Slot début", self.start_slot_var, 8),
        ]):
            ttk.Label(cfg, text=label).grid(row=0, column=i * 2, padx=(0 if i == 0 else 8, 2))
            ttk.Entry(cfg, textvariable=var, width=w).grid(row=0, column=i * 2 + 1)

        # Liste / drop
        drop_frame = ttk.LabelFrame(self.root, text="Fichiers à transférer", padding=8)
        drop_frame.pack(fill="both", expand=True, padx=10, pady=5)

        cols = ("file", "format", "slot", "state", "progress")
        self.tree = ttk.Treeview(drop_frame, columns=cols, show="headings", height=10)
        for col, label, w, anchor in [
            ("file", "Fichier", 220, "w"),
            ("format", "Format", 130, "w"),
            ("slot", "Slot", 60, "center"),
            ("state", "État", 80, "center"),
            ("progress", "Progression", 200, "w"),
        ]:
            self.tree.heading(col, text=label)
            self.tree.column(col, width=w, anchor=anchor)
        self.tree.pack(fill="both", expand=True)

        # Drop & Ctrl+V
        if HAS_DND:
            self.root.drop_target_register(DND_FILES)
            self.root.dnd_bind("<<Drop>>", self._on_drop)
        self.root.bind("<Control-v>", self._on_paste)
        self.root.bind("<Control-V>", self._on_paste)

        hint = "Glissez des WAV ici"
        if not HAS_DND:
            hint = "Utilisez « Ajouter fichiers… » (drag'n'drop indispo)"
        else:
            hint += "  •  Ctrl+V pour coller depuis l'Explorer  •  ou « Ajouter fichiers… »"
        self.hint_label = ttk.Label(drop_frame, text=hint, foreground="gray")
        self.hint_label.place(relx=0.5, rely=0.5, anchor="center")

        # Boutons
        bf = ttk.Frame(self.root, padding=(10, 0))
        bf.pack(fill="x", padx=10, pady=5)
        self.add_btn = ttk.Button(bf, text="Ajouter fichiers…", command=self._on_add_click)
        self.add_btn.pack(side="left")
        self.send_btn = ttk.Button(bf, text="Transférer", command=self._on_send_click)
        self.send_btn.pack(side="left", padx=(8, 0))
        self.stop_btn = ttk.Button(bf, text="Interrompre", command=self._on_stop_click,
                                   state="disabled")
        self.stop_btn.pack(side="left", padx=(8, 0))
        self.retry_btn = ttk.Button(bf, text="Retenter erreurs", command=self._on_retry_click)
        self.retry_btn.pack(side="left", padx=(8, 0))
        self.clear_btn = ttk.Button(bf, text="Effacer la liste", command=self._on_clear_click)
        self.clear_btn.pack(side="left", padx=(8, 0))
        self.download_btn = ttk.Button(bf, text="Télécharger sample…",
                                       command=self._on_download_click)
        self.download_btn.pack(side="right")

        # Status
        self.status_var = tk.StringVar(value="Prêt. Glissez des WAV pour commencer.")
        ttk.Label(self.root, textvariable=self.status_var, relief="sunken",
                  anchor="w", padding=4).pack(fill="x", side="bottom")

    # ── Drop / paste / picker ────────────────────────────────────────────────

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
            title="Ajouter des fichiers WAV",
            filetypes=[("WAV PCM", "*.wav"), ("Tous les fichiers", "*.*")],
        )
        self._add_paths(paths, source="picker")

    def _add_paths(self, paths, source: str) -> None:
        added = 0
        for p in paths:
            path = Path(p)
            if path.is_file() and path.suffix.lower() == ".wav":
                self._add_item(path)
                added += 1
        if added:
            self.hint_label.place_forget()
            label = {"drop": "déposé", "paste": "collé", "picker": "ajouté"}[source]
            s = "s" if added > 1 else ""
            self.status_var.set(f"{added} fichier{s} {label}{s}.")
        elif source == "paste":
            self.status_var.set("Clipboard : aucun .wav.")

    def _add_item(self, path: Path) -> None:
        try:
            wave = load_wave(path)
            ch = "stéréo" if wave.channels == 2 else "mono"
            fmt = f"{wave.sample_rate / 1000:.1f}k {ch} {wave.bits_per_sample}b"
            total = wave.byte_count
        except WaveValidationError as exc:
            fmt = f"<invalide: {exc}>"
            total = 0
        item = QueueItem(path=path, total_bytes=total)
        self.items.append(item)
        self.tree.insert("", "end", iid=str(len(self.items) - 1),
                         values=(path.name, fmt, "", item.state, ""))

    def _on_download_click(self) -> None:
        """Lance un scan du sampler puis ouvre un browser pour choisir le sample."""
        if self.worker_thread and self.worker_thread.is_alive():
            self.status_var.set("Opération en cours, attendre la fin.")
            return
        config = {
            "ha": self.ha_var.get(),
            "bus": self.bus_var.get(),
            "target": self.target_var.get(),
            "lun": self.lun_var.get(),
        }
        self._set_buttons_busy()
        self.status_var.set("Scan des samples en cours…")
        self.worker_thread = threading.Thread(
            target=self._browse_thread_run, args=(config,), daemon=True,
        )
        self.worker_thread.start()

    def _set_buttons_busy(self) -> None:
        self.send_btn.configure(state="disabled")
        self.add_btn.configure(state="disabled")
        self.clear_btn.configure(state="disabled")
        self.retry_btn.configure(state="disabled")
        self.download_btn.configure(state="disabled")
        self.stop_btn.configure(state="normal")

    def _browse_thread_run(self, config: dict) -> None:
        """Scan le sampler via le worker, puis pousse les résultats à l'UI."""
        self.interrupted = False
        client = self._ensure_worker()
        if client is None:
            self.ui_queue.put(UiEvent(kind="allfinished"))
            return
        common = {
            "ha": int(config["ha"]),
            "bus": int(config["bus"]),
            "target": int(config["target"]),
            "lun": int(config["lun"]),
        }
        try:
            client.send_command({
                "cmd": "list_samples",
                "start": 0,
                "limit": 128,
                **common,
            })
            samples = None
            while samples is None:
                ev = client.recv_event()
                if ev is None:
                    if self.interrupted:
                        self.ui_queue.put(UiEvent(kind="status", msg="Scan annulé."))
                    else:
                        self.ui_queue.put(UiEvent(kind="error",
                                                  msg="Worker s'est déconnecté pendant le scan."))
                    self.worker_client = None
                    self.ui_queue.put(UiEvent(kind="allfinished"))
                    return
                e = ev.get("event")
                if e == "scan_progress":
                    self.ui_queue.put(UiEvent(
                        kind="status",
                        msg=f"Scan : {ev['scanned']} slots, {ev['found']} samples trouvés…",
                    ))
                elif e == "samples_list":
                    samples = ev["samples"]
                elif e == "error":
                    self.ui_queue.put(UiEvent(kind="error", msg=ev.get("msg", "?")))
                    self.ui_queue.put(UiEvent(kind="allfinished"))
                    return
        except Exception as exc:
            if not self.interrupted:
                self.ui_queue.put(UiEvent(kind="error", msg=f"Communication worker: {exc}"))
            self.ui_queue.put(UiEvent(kind="allfinished"))
            return

        # On a la liste — l'envoyer à l'UI thread pour ouvrir le dialog
        self.ui_queue.put(UiEvent(kind="open_browser", msg=json.dumps({
            "samples": samples,
            "config": config,
        })))
        self.ui_queue.put(UiEvent(kind="allfinished"))

    def _open_sample_browser(self, samples: list, config: dict) -> None:
        """Ouvert depuis _handle_ui_event (main thread tkinter)."""
        if not samples:
            messagebox.showinfo("Aucun sample", "Aucun sample détecté dans le sampler.")
            return
        dlg = SampleBrowserDialog(self.root, samples)
        self.root.wait_window(dlg.top)
        if dlg.result is None:
            return
        slot_num, output_path = dlg.result
        download_config = {**config, "sample_number": slot_num, "output_path": output_path}
        self._set_buttons_busy()
        self.status_var.set(f"Téléchargement sample #{slot_num} → {Path(output_path).name}…")
        self.worker_thread = threading.Thread(
            target=self._download_thread_run, args=(download_config,), daemon=True,
        )
        self.worker_thread.start()

    def _download_thread_run(self, config: dict) -> None:
        self.interrupted = False
        client = self._ensure_worker()
        if client is None:
            self.ui_queue.put(UiEvent(kind="allfinished"))
            return
        common = {
            "ha": int(config["ha"]),
            "bus": int(config["bus"]),
            "target": int(config["target"]),
            "lun": int(config["lun"]),
        }
        try:
            client.send_command({
                "cmd": "receive",
                "sample_number": config["sample_number"],
                "output_path": config["output_path"],
                **common,
            })
            while True:
                ev = client.recv_event()
                if ev is None:
                    if self.interrupted:
                        self.ui_queue.put(UiEvent(kind="status", msg="Téléchargement annulé."))
                    else:
                        self.ui_queue.put(UiEvent(
                            kind="error",
                            msg="Worker s'est déconnecté pendant le téléchargement.",
                        ))
                    self.worker_client = None
                    break
                e = ev.get("event")
                if e == "progress":
                    sent = int(ev["sent"])
                    total = int(ev["total"])
                    pct = (sent / total * 100) if total else 0
                    self.ui_queue.put(UiEvent(
                        kind="status",
                        msg=f"Téléchargement : {sent}/{total} ({pct:.1f}%)",
                    ))
                elif e == "received":
                    self.ui_queue.put(UiEvent(
                        kind="status",
                        msg=f"Téléchargé : {ev['name']!r} → {ev['output_path']} "
                            f"({ev['channels']}ch {ev['sample_rate']}Hz {ev['frames']} frames)",
                    ))
                    break
                elif e == "error":
                    self.ui_queue.put(UiEvent(kind="error", msg=ev.get("msg", "?")))
                    break
        except Exception as exc:
            if not self.interrupted:
                self.ui_queue.put(UiEvent(kind="error", msg=f"Communication worker: {exc}"))
        self.ui_queue.put(UiEvent(kind="allfinished"))

    def _on_stop_click(self) -> None:
        """Tue le worker process. Le sample en cours sera marqué annulé.
        Les pending suivants restent intouchés et pourront être relancés."""
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
        self.status_var.set("Interruption demandée — worker tué.")
        # Le worker_thread va voir la socket morte et finir tout seul, ce qui
        # postera l'event 'allfinished' et réactivera les boutons.

    def _on_retry_click(self) -> None:
        """Reset les items en erreur ou annulés vers pending pour qu'ils soient
        retraités au prochain Transfer."""
        if self.worker_thread and self.worker_thread.is_alive():
            self.status_var.set("Transfert en cours, attendre la fin avant de retenter.")
            return
        retried = 0
        for idx, it in enumerate(self.items):
            if it.state in ("error", "cancelled"):
                it.state = "pending"
                it.progress = 0
                it.sent_bytes = 0
                it.error_msg = ""
                self._refresh_row(idx)
                retried += 1
        if retried:
            s = "s" if retried > 1 else ""
            self.status_var.set(f"{retried} item{s} prêt{s} à retenter.")
        else:
            self.status_var.set("Rien à retenter.")

    def _on_clear_click(self) -> None:
        if self.worker_thread and self.worker_thread.is_alive():
            messagebox.showwarning("Transfert en cours", "Impossible d'effacer pendant un transfert.")
            return
        self.items.clear()
        for child in self.tree.get_children():
            self.tree.delete(child)
        self.hint_label.place(relx=0.5, rely=0.5, anchor="center")
        self.status_var.set("Liste effacée.")

    # ── Transfert ────────────────────────────────────────────────────────────

    def _on_send_click(self) -> None:
        if not self.items:
            self.status_var.set("Rien à transférer.")
            return
        if self.worker_thread and self.worker_thread.is_alive():
            self.status_var.set("Transfert déjà en cours.")
            return
        pending_count = sum(1 for it in self.items if it.state == "pending")
        if pending_count == 0:
            self.status_var.set(
                "Tous les fichiers sont déjà traités. Effacez la liste pour recommencer."
            )
            return

        config = {
            "ha": self.ha_var.get(),
            "bus": self.bus_var.get(),
            "target": self.target_var.get(),
            "lun": self.lun_var.get(),
            "start_slot": self.start_slot_var.get().strip(),
        }

        self.send_btn.configure(state="disabled")
        self.clear_btn.configure(state="disabled")
        self.add_btn.configure(state="disabled")
        self.retry_btn.configure(state="disabled")
        self.stop_btn.configure(state="normal")
        self.status_var.set("Démarrage du worker admin (popup UAC)…")
        self.worker_thread = threading.Thread(
            target=self._send_thread_run, args=(config,), daemon=True,
        )
        self.worker_thread.start()

    def _ensure_worker(self) -> WorkerClient | None:
        """Garantit qu'un worker vivant est dispo. Re-lance si nécessaire."""
        if self.worker_client is not None:
            # Tester si la connexion est encore vivante via un check non-bloquant
            try:
                # Ping inoffensif : send une commande qui est toujours valide
                # (find_free_slot avec start très haut → reviendra vite)
                # Plus simple : vérifier via getsockopt SO_ERROR
                err = self.worker_client.client_sock.getsockopt(socket.SOL_SOCKET, socket.SO_ERROR)
                if err == 0:
                    return self.worker_client
            except Exception:
                pass
            # Worker mort, on le clean
            try:
                self.worker_client.stop()
            except Exception:
                pass
            self.worker_client = None
            self.ui_queue.put(UiEvent(kind="status", msg="Worker précédent perdu — relance avec UAC..."))

        client = WorkerClient()
        try:
            client.start()
        except WorkerError as exc:
            self.ui_queue.put(UiEvent(kind="error", msg=str(exc)))
            return None
        self.worker_client = client
        self.ui_queue.put(UiEvent(kind="status", msg="Worker admin connecté."))
        return client

    def _send_thread_run(self, config: dict) -> None:
        self.interrupted = False  # reset à chaque session
        client = self._ensure_worker()
        if client is None:
            self.ui_queue.put(UiEvent(kind="allfinished"))
            return
        common = {
            "ha": int(config["ha"]),
            "bus": int(config["bus"]),
            "target": int(config["target"]),
            "lun": int(config["lun"]),
        }

        # Slot de départ
        start_slot_str = config["start_slot"].lower()
        if start_slot_str in ("auto", ""):
            try:
                client.send_command({"cmd": "find_free_slot", **common})
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
                self.ui_queue.put(UiEvent(kind="error", msg=f"Slot début invalide: {start_slot_str!r}"))
                self.ui_queue.put(UiEvent(kind="allfinished"))
                return

        # Boucle transferts : seulement les "pending"
        for idx, item in enumerate(self.items):
            if item.state != "pending":
                continue
            self.ui_queue.put(UiEvent(kind="start", item_index=idx, sample_slot=next_slot))
            try:
                client.send_command({
                    "cmd": "transfer",
                    "wave_path": str(item.path),
                    "sample_number": next_slot,
                    "name": item.path.stem[:16],
                    **common,
                })
                done = False
                while not done:
                    ev = client.recv_event()
                    if ev is None:
                        if self.interrupted:
                            self.ui_queue.put(UiEvent(
                                kind="cancelled", item_index=idx,
                                msg="Annulé",
                            ))
                        else:
                            self.ui_queue.put(UiEvent(
                                kind="error", item_index=idx,
                                msg="Worker s'est déconnecté.",
                            ))
                        self.worker_client = None
                        self.ui_queue.put(UiEvent(kind="allfinished"))
                        return
                    e = ev.get("event")
                    if e == "progress":
                        self.ui_queue.put(UiEvent(
                            kind="progress", item_index=idx,
                            sent=int(ev["sent"]), total=int(ev["total"]),
                        ))
                    elif e == "done":
                        self.ui_queue.put(UiEvent(
                            kind="done", item_index=idx,
                            sample_slot=int(ev["sample_number"]),
                        ))
                        done = True
                    elif e == "error":
                        self.ui_queue.put(UiEvent(
                            kind="error", item_index=idx, msg=ev.get("msg", "?"),
                        ))
                        done = True
                    else:
                        # event inconnu, ignore
                        pass
            except Exception as exc:
                if self.interrupted:
                    self.ui_queue.put(UiEvent(
                        kind="cancelled", item_index=idx, msg="Annulé",
                    ))
                    self.ui_queue.put(UiEvent(kind="allfinished"))
                    return
                self.ui_queue.put(UiEvent(
                    kind="error", item_index=idx, msg=f"Communication worker: {exc}",
                ))
                continue
            next_slot += 1

        self.ui_queue.put(UiEvent(kind="allfinished"))

    # ── Pump UI events ───────────────────────────────────────────────────────

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
        if ev.kind == "start":
            it = self.items[ev.item_index]
            it.state = "running"
            it.sample_slot = ev.sample_slot
            self._refresh_row(ev.item_index)
            self.status_var.set(f"Transfert : {it.path.name} → slot #{ev.sample_slot}")
            return
        if ev.kind == "progress":
            it = self.items[ev.item_index]
            it.sent_bytes = ev.sent
            it.total_bytes = ev.total
            it.progress = ev.sent / ev.total if ev.total else 0
            self._refresh_row(ev.item_index)
            return
        if ev.kind == "done":
            it = self.items[ev.item_index]
            it.state = "done"
            it.progress = 1.0
            self._refresh_row(ev.item_index)
            return
        if ev.kind == "error":
            if ev.item_index >= 0:
                it = self.items[ev.item_index]
                it.state = "error"
                it.error_msg = ev.msg
                self._refresh_row(ev.item_index)
                self.status_var.set(f"Erreur : {it.path.name} — {ev.msg}")
            else:
                self.status_var.set(f"Erreur : {ev.msg}")
                messagebox.showerror("Erreur", ev.msg)
            return
        if ev.kind == "open_browser":
            data = json.loads(ev.msg)
            self._open_sample_browser(data["samples"], data["config"])
            return
        if ev.kind == "cancelled":
            it = self.items[ev.item_index]
            it.state = "cancelled"
            it.error_msg = ev.msg or "Annulé"
            self._refresh_row(ev.item_index)
            return
        if ev.kind == "allfinished":
            self.send_btn.configure(state="normal")
            self.clear_btn.configure(state="normal")
            self.add_btn.configure(state="normal")
            self.retry_btn.configure(state="normal")
            self.download_btn.configure(state="normal")
            self.stop_btn.configure(state="disabled")
            done_n = sum(1 for it in self.items if it.state == "done")
            err_n = sum(1 for it in self.items if it.state == "error")
            self.status_var.set(f"Terminé. {done_n} OK, {err_n} erreur{'s' if err_n > 1 else ''}.")
            return

    def _refresh_row(self, idx: int) -> None:
        item = self.items[idx]
        slot_str = f"#{item.sample_slot}" if item.sample_slot is not None else ""
        if item.state == "running":
            pct = int(item.progress * 100)
            progress_str = f"{pct:3d}%  {item.sent_bytes}/{item.total_bytes}"
        elif item.state == "done":
            progress_str = "OK"
        elif item.state == "error":
            progress_str = item.error_msg[:40]
        elif item.state == "cancelled":
            progress_str = "Annulé"
        else:
            progress_str = ""
        existing = self.tree.item(str(idx), "values")
        fmt = existing[1] if existing else ""
        self.tree.item(str(idx), values=(item.path.name, fmt, slot_str, item.state, progress_str))

    def _refresh_all(self) -> None:
        for idx in range(len(self.items)):
            self._refresh_row(idx)

    # ── Cleanup ──────────────────────────────────────────────────────────────

    def _on_close(self) -> None:
        if self.worker_client is not None:
            try:
                self.worker_client.stop()
            except Exception:
                pass
        self.root.destroy()


# ─────────────────────────────────────────────────────────────────────────────
# Dialog modal : télécharger un sample
# ─────────────────────────────────────────────────────────────────────────────

class SampleBrowserDialog:
    """Liste les samples du sampler, l'utilisateur choisit lequel télécharger."""

    def __init__(self, parent, samples: list) -> None:
        self.samples = samples
        self.result: tuple[int, str] | None = None
        self.top = tk.Toplevel(parent)
        self.top.title(f"Samples du sampler ({len(samples)})")
        self.top.transient(parent)
        self.top.grab_set()
        self.top.geometry("680x420")

        frm = ttk.Frame(self.top, padding=10)
        frm.pack(fill="both", expand=True)

        ttk.Label(frm, text=f"{len(samples)} samples détectés. Sélectionnez-en un puis cliquez Télécharger.",
                  foreground="gray").pack(anchor="w", pady=(0, 6))

        cols = ("slot", "name", "format", "duration", "frames")
        self.tree = ttk.Treeview(frm, columns=cols, show="headings", height=14, selectmode="browse")
        for col, label, w, anchor in [
            ("slot", "#", 50, "center"),
            ("name", "Nom", 200, "w"),
            ("format", "Format", 160, "w"),
            ("duration", "Durée", 80, "e"),
            ("frames", "Frames", 100, "e"),
        ]:
            self.tree.heading(col, text=label)
            self.tree.column(col, width=w, anchor=anchor)
        self.tree.pack(fill="both", expand=True)

        for s in samples:
            ch = "stéréo" if s["channels"] == 2 else "mono"
            sr = s["sample_rate"]
            sr_str = f"{sr / 1000:.1f}k" if sr else "?"
            fmt = f"{sr_str} {ch} {s['bits']}b"
            duration = f"{s['duration']:.2f}s" if s.get("duration") else "—"
            self.tree.insert("", "end", iid=str(s["slot"]),
                             values=(s["slot"], s["name"], fmt, duration, s["frames"]))

        # Sélectionner la première ligne par défaut
        if samples:
            self.tree.selection_set(str(samples[0]["slot"]))
            self.tree.focus(str(samples[0]["slot"]))

        # Double-clic = télécharger directement
        self.tree.bind("<Double-1>", lambda e: self._ok())

        btn_frm = ttk.Frame(frm)
        btn_frm.pack(fill="x", pady=(8, 0))
        ttk.Button(btn_frm, text="Annuler", command=self._cancel).pack(side="right", padx=(8, 0))
        ttk.Button(btn_frm, text="Télécharger…", command=self._ok).pack(side="right")

        self.top.bind("<Return>", lambda e: self._ok())
        self.top.bind("<Escape>", lambda e: self._cancel())

    def _ok(self) -> None:
        sel = self.tree.selection()
        if not sel:
            messagebox.showinfo("Sélection", "Choisis un sample dans la liste.", parent=self.top)
            return
        slot = int(sel[0])
        sample = next((s for s in self.samples if s["slot"] == slot), None)
        if sample is None:
            return
        # Save dialog avec nom auto-généré depuis le name du sample
        safe_name = "".join(ch if ch.isalnum() or ch in " ._-" else "_" for ch in (sample["name"] or "sample")).strip()
        if not safe_name:
            safe_name = f"sample_{slot:03d}"
        path = filedialog.asksaveasfilename(
            parent=self.top,
            defaultextension=".wav",
            filetypes=[("WAV PCM", "*.wav")],
            initialfile=f"{safe_name}.wav",
        )
        if not path:
            return
        self.result = (slot, path)
        self.top.destroy()

    def _cancel(self) -> None:
        self.result = None
        self.top.destroy()


# ─────────────────────────────────────────────────────────────────────────────
# Helpers Win32
# ─────────────────────────────────────────────────────────────────────────────

def _read_clipboard_filelist_win32() -> list[str]:
    """Lit la liste de fichiers (CF_HDROP) du clipboard Windows."""
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
    shell32.DragQueryFileW.argtypes = [wintypes.HANDLE, wintypes.UINT, wintypes.LPWSTR, wintypes.UINT]
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
