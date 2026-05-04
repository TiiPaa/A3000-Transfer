"""Vue Slicer embarquable dans un Notebook Tk.

Refonte de slicer_gui.py:SlicerApp en `SlicerView(ttk.Frame)`. Le Frame
construit lui-même tous ses widgets ; le bouton "Exporter slices..." est
remplacé par deux boutons : "Send to Upload" (callback fourni par la GUI
parente) et "Export to folder…" (export libre vers un dossier au choix).
"""
from __future__ import annotations

import tempfile
import tkinter as tk
from pathlib import Path
from tkinter import ttk, filedialog, messagebox
from typing import Callable, Optional

import numpy as np
import librosa
import soundfile as sf

import matplotlib
matplotlib.use("TkAgg")
import matplotlib.ticker as mticker
from matplotlib.figure import Figure
from matplotlib.patches import Rectangle
from matplotlib.backends.backend_tkagg import FigureCanvasTkAgg

try:
    from tkinterdnd2 import DND_FILES
    DND_AVAILABLE = True
except ImportError:
    DND_FILES = None
    DND_AVAILABLE = False

try:
    import mido
    MIDI_AVAILABLE = True
except ImportError:
    mido = None
    MIDI_AVAILABLE = False

try:
    import sounddevice as sd
    AUDIO_AVAILABLE = True
except (ImportError, OSError):
    sd = None
    AUDIO_AVAILABLE = False

from .engine import detect_transients, slice_wave


WAVE_COLOR = "#4ec9b0"
WAVE_PEAK_ALPHA = 0.4
ONSET_COLOR = "#ff6b6b"
SELECTION_COLOR = "#5e9bd1"
BG_PLOT = "#1e1e1e"
BG_FIG = "#2b2b2b"


def _truncate_name(name: str, max_chars: int) -> str:
    """Tronque un nom de fichier en gardant le début et la fin (avec
    l'extension typiquement)."""
    if len(name) <= max_chars:
        return name
    keep_start = (max_chars - 1) // 2
    keep_end = max_chars - 1 - keep_start
    return name[:keep_start] + "…" + name[-keep_end:]


class SlicerView(ttk.Frame):
    """Frame embarquable contenant la waveform, les sliders de détection
    de transients et les boutons Send to Upload / Export to folder.

    Args:
        parent: widget parent (typiquement un onglet ttk.Notebook).
        on_send_to_upload: callback appelé avec ``list[Path]`` des slices
            générées quand l'utilisateur clique "Send to Upload".
    """

    def __init__(self, parent, on_send_to_upload: Optional[Callable[[list[Path]], None]] = None,
                 **kwargs):
        super().__init__(parent, **kwargs)
        self.on_send_to_upload = on_send_to_upload

        self.audio = None
        self.sr = None
        self.y_mono = None
        self.input_path: Optional[Path] = None
        self.onsets = np.array([], dtype=np.int64)
        self._onset_lines = []
        self._drag_idx = None
        self._drag_tolerance_px = 6
        self._click_motion_threshold_px = 4
        self._mode = None
        self._press_x_pixel = None
        self._press_xlim = None
        self._playing_span = None
        self._wave_artists = []
        self._hint_artist = None
        self._rendering = False
        # Selection trackée par INDEX de slice (pas par midpoint sample) :
        # plus stable quand on déplace les cuts (la slice garde son identité
        # même quand son midpoint passe dans une slice voisine)
        self._selected_indices: set[int] = set()
        # Slices marquées "supprimées" → skip à l'export (right-click sur cell)
        self._deleted_indices: set[int] = set()
        # State pour le drag-select dans le bandeau supérieur
        self._drag_state: bool = False         # True = on ajoute, False = on retire
        self._drag_visited: set[int] = set()
        # State pour le bouton ▶ Loop
        self._is_looping: bool = False
        self._play_start_time: float = 0.0
        self._playhead_line = None
        self._playhead_after_id = None
        self._selection_cells = []
        self._cycle_idx = -1  # index du cut actuellement centré (Space pour cycler)

        self._build_ui()
        self._wire_canvas_events()

    # ── Construction UI ──────────────────────────────────────────────────────

    def _build_ui(self):
        # Row 1 : file + playback + Reset
        top = ttk.Frame(self, padding=8)
        top.pack(side=tk.TOP, fill=tk.X)

        ttk.Button(top, text="Open WAV…", command=self.open_file).pack(side=tk.LEFT)
        # Bouton play + loop du sample entier
        # width=11 pour que la taille ne change pas entre "▶ Loop" et "⏹ Stop loop"
        self.loop_btn = tk.Button(
            top, text="▶ Loop", command=self._toggle_loop_play,
            bg="#1976D2", fg="white",
            activebackground="#1565C0", activeforeground="white",
            disabledforeground="#BDBDBD",
            font=("", 9, "bold"),
            relief="raised", borderwidth=1, padx=10, pady=2,
            cursor="hand2", width=11,
        )
        self.loop_btn.pack(side=tk.LEFT, padx=(8, 0))
        self.file_label = ttk.Label(top, text="No file loaded", width=44, anchor="w")
        self.file_label.pack(side=tk.LEFT, padx=10)
        ttk.Button(top, text="Reset", command=self._reset_to_default).pack(
            side=tk.RIGHT,
        )

        # Row 2 : édition (delete/select) + export (MIDI/Upload/Folder)
        top2 = ttk.Frame(self, padding=(8, 0, 8, 6))
        top2.pack(side=tk.TOP, fill=tk.X)

        # Bouton rouge : applique la suppression des sections marquées
        self.delete_marked_btn = tk.Button(
            top2, text="🗑 Delete marked", command=self._commit_deletions,
            bg="#d04848", fg="white",
            activebackground="#b03838", activeforeground="white",
            disabledforeground="#BDBDBD",
            font=("", 9, "bold"),
            relief="raised", borderwidth=1, padx=10, pady=2,
            cursor="hand2",
        )
        self.delete_marked_btn.pack(side=tk.LEFT)
        ttk.Button(top2, text="Select all", command=self._select_all).pack(
            side=tk.LEFT, padx=(8, 0),
        )
        ttk.Button(top2, text="Deselect all", command=self._deselect_all).pack(
            side=tk.LEFT, padx=(8, 0),
        )

        # Boutons d'export à droite : Send to Upload (bleu) + Export to folder
        self.send_btn = tk.Button(
            top2, text="Send to Upload", command=self._on_send_to_upload_click,
            bg="#1976D2", fg="white",
            activebackground="#1565C0", activeforeground="white",
            disabledforeground="#BDBDBD",
            font=("", 9, "bold"),
            relief="raised", borderwidth=1, padx=14, pady=2,
            cursor="hand2",
        )
        self.send_btn.pack(side=tk.RIGHT)
        ttk.Button(top2, text="Export to folder…", command=self.export_slices).pack(
            side=tk.RIGHT, padx=(0, 8),
        )
        # Drag-MIDI vers DAW
        self.midi_btn = tk.Label(
            top2, text="↓ Drag MIDI",
            bg="#4CAF50", fg="white",
            font=("", 9, "bold"),
            relief="raised", borderwidth=2, padx=10, pady=4,
            cursor="hand2",
        )
        self.midi_btn.pack(side=tk.RIGHT, padx=(0, 8))
        if DND_AVAILABLE and MIDI_AVAILABLE:
            self.midi_btn.drag_source_register(1, DND_FILES)
            self.midi_btn.dnd_bind("<<DragInitCmd>>", self._on_midi_drag_init)
            self.midi_btn.bind("<Enter>", lambda _e: self.midi_btn.config(bg="#388E3C"))
            self.midi_btn.bind("<Leave>", lambda _e: self.midi_btn.config(bg="#4CAF50"))
        # Beats spinbox pour calcul du BPM MIDI
        self.beats_var = tk.IntVar(value=16)
        ttk.Spinbox(top2, from_=1, to=512, increment=1, textvariable=self.beats_var,
                    width=5).pack(side=tk.RIGHT, padx=(0, 4))
        ttk.Label(top2, text="Beats:").pack(side=tk.RIGHT, padx=(0, 2))

        controls = ttk.LabelFrame(self, text="Detection", padding=8)
        controls.pack(side=tk.TOP, fill=tk.X, padx=8, pady=4)

        ttk.Label(controls, text="Sensitivity").grid(row=0, column=0, sticky=tk.W)
        self.sensitivity = tk.DoubleVar(value=1.0)
        self.sens_label = ttk.Label(controls, text="1.00", width=6)
        self.sens_label.grid(row=0, column=2, padx=4)
        ttk.Scale(
            controls, from_=0.2, to=3.0, variable=self.sensitivity,
            orient=tk.HORIZONTAL, command=self._on_sens_change,
        ).grid(row=0, column=1, sticky=tk.EW, padx=4)

        ttk.Label(controls, text="Min gap (ms)").grid(row=1, column=0, sticky=tk.W)
        self.min_gap = tk.DoubleVar(value=30)
        self.gap_label = ttk.Label(controls, text="30", width=6)
        self.gap_label.grid(row=1, column=2, padx=4)
        ttk.Scale(
            controls, from_=5, to=500, variable=self.min_gap,
            orient=tk.HORIZONTAL, command=self._on_gap_change,
        ).grid(row=1, column=1, sticky=tk.EW, padx=4)

        controls.columnconfigure(1, weight=1)

        self.count_label = ttk.Label(controls, text="—")
        self.count_label.grid(row=2, column=0, columnspan=3, sticky=tk.W, pady=(6, 0))

        help_text = (
            "Left click: play  ·  Drag cut: move  ·  Drag empty: pan  ·  "
            "Wheel: zoom  ·  Right click: add/remove cut\n"
            "Top strip — Left click: select for export  ·  "
            "Right click: mark for deletion (apply with 'Delete marked')"
        )
        ttk.Label(controls, text=help_text, foreground="#888").grid(
            row=3, column=0, columnspan=3, sticky=tk.W, pady=(2, 0),
        )

        self.fig = Figure(figsize=(10, 4.4), dpi=100)
        self.fig.patch.set_facecolor(BG_FIG)
        gs = self.fig.add_gridspec(2, 1, height_ratios=[1, 16], hspace=0.04)
        self.sel_ax = self.fig.add_subplot(gs[0])
        self.ax = self.fig.add_subplot(gs[1], sharex=self.sel_ax)
        self.ax.set_facecolor(BG_PLOT)
        self.ax.tick_params(colors="white")
        for spine in self.ax.spines.values():
            spine.set_color("white")
        self.sel_ax.set_facecolor("#252525")
        self.sel_ax.set_ylim(0, 1)
        self.sel_ax.set_yticks([])
        self.sel_ax.tick_params(left=False, bottom=False, labelleft=False, labelbottom=False)
        for spine in self.sel_ax.spines.values():
            spine.set_color("#555")

        self.canvas = FigureCanvasTkAgg(self.fig, master=self)
        self.canvas.get_tk_widget().pack(side=tk.TOP, fill=tk.BOTH, expand=True, padx=8, pady=4)

        self._setup_axes_styling()
        self.ax.callbacks.connect("xlim_changed", self._on_xlim_changed)
        self.canvas.mpl_connect("resize_event", lambda e: self._render_visible_waveform())

        self._show_drop_hint()
        self._register_drop_target()

        status_text = "Ready" if DND_AVAILABLE else "Ready (drag & drop unavailable — install tkinterdnd2)"
        self.status = ttk.Label(self, text=status_text, relief=tk.SUNKEN, anchor=tk.W)
        self.status.pack(side=tk.BOTTOM, fill=tk.X)

    def _wire_canvas_events(self):
        self.canvas.mpl_connect("scroll_event", self._on_scroll)
        self.canvas.mpl_connect("button_press_event", self._on_press)
        self.canvas.mpl_connect("motion_notify_event", self._on_motion)
        self.canvas.mpl_connect("button_release_event", self._on_release)
        self.canvas.mpl_connect("key_press_event", self._on_mpl_key)
        # Tk-level fallback (mpl key events nécessitent focus canvas)
        widget = self.canvas.get_tk_widget()
        widget.configure(takefocus=1)
        widget.bind("<space>", lambda _e: self._center_on_next_cut())
        widget.bind("<KeyPress-space>", lambda _e: self._center_on_next_cut())

    def _on_mpl_key(self, event):
        if event.key in (" ", "space"):
            self._center_on_next_cut()

    def _center_on_next_cut(self):
        if self.y_mono is None or len(self.onsets) == 0:
            return
        self._cycle_idx = (self._cycle_idx + 1) % len(self.onsets)
        onset_sec = float(self.onsets[self._cycle_idx]) / self.sr
        xmin, xmax = self.ax.get_xlim()
        width = xmax - xmin
        if width <= 0:
            width = 1.0
        max_t = len(self.y_mono) / self.sr
        new_min = onset_sec - width / 2
        new_max = onset_sec + width / 2
        if new_min < 0:
            new_max = min(max_t, new_max - new_min)
            new_min = 0
        elif new_max > max_t:
            new_min = max(0, new_min - (new_max - max_t))
            new_max = max_t
        self._rendering = True
        try:
            self.ax.set_xlim(new_min, new_max)
        finally:
            self._rendering = False
        self._render_visible_waveform()
        self.status.config(
            text=f"Cut #{self._cycle_idx + 1}/{len(self.onsets)} @ {onset_sec:.3f}s"
        )

    # ── Drag'n'drop ──────────────────────────────────────────────────────────

    def _register_drop_target(self):
        # Pas de drop target propre à la SlicerView : on utilise celui du root
        # (registered par gui.py) qui route selon l'onglet actif. Évite le
        # cold-load OLE 20-30s sur le premier drop dans le slicer canvas.
        pass

    def handle_dropped_path(self, path):
        """Appelé par gui.py quand un drop arrive sur l'onglet Slicer."""
        self._load_path(path)

    # ── Manipulation des onsets ──────────────────────────────────────────────

    def _nearest_onset(self, event):
        if self.y_mono is None or len(self.onsets) == 0 or event.x is None:
            return None
        transform = self.ax.transData.transform
        target_px = event.x
        best_idx = None
        best_dist = self._drag_tolerance_px
        for i, s in enumerate(self.onsets):
            px = transform((s / self.sr, 0))[0]
            d = abs(px - target_px)
            if d < best_dist:
                best_dist = d
                best_idx = i
        return best_idx

    def _on_scroll(self, event):
        if event.inaxes not in (self.ax, self.sel_ax) or event.xdata is None or self.y_mono is None:
            return
        factor = 1 / 1.3 if event.button == "up" else 1.3
        xmin, xmax = self.ax.get_xlim()
        cx = event.xdata
        new_min = cx - (cx - xmin) * factor
        new_max = cx + (xmax - cx) * factor
        max_t = len(self.y_mono) / self.sr
        new_min = max(0.0, new_min)
        new_max = min(max_t, new_max)
        if new_max - new_min < 1e-4:
            return
        self.ax.set_xlim(new_min, new_max)
        self.canvas.draw_idle()

    def _on_press(self, event):
        if event.x is None:
            return
        # Donne le focus au canvas pour que les key bindings (Space) marchent
        try:
            self.canvas.get_tk_widget().focus_set()
        except tk.TclError:
            pass

        if event.inaxes is self.sel_ax:
            if event.xdata is None or self.y_mono is None:
                return
            info = self._slice_range_at(float(event.xdata))
            if info is None:
                return
            idx = info[0]
            if event.button == 1:
                # Toggle ce cell + entre en mode drag-select pour étendre
                self._deleted_indices.discard(idx)
                if idx in self._selected_indices:
                    self._selected_indices.discard(idx)
                    self._drag_state = False
                else:
                    self._selected_indices.add(idx)
                    self._drag_state = True
                self._mode = "select_drag"
                self._drag_visited = {idx}
                self._render_selection_cells()
                n = len(self._selected_indices)
                self.status.config(text=f"{n} selected (drag to extend)")
                return
            if event.button == 3:
                self._selected_indices.discard(idx)
                if idx in self._deleted_indices:
                    self._deleted_indices.discard(idx)
                    self._drag_state = False
                else:
                    self._deleted_indices.add(idx)
                    self._drag_state = True
                self._mode = "delete_drag"
                self._drag_visited = {idx}
                self._render_selection_cells()
                n = len(self._deleted_indices)
                self.status.config(text=f"{n} marked (drag to extend, then 'Delete marked')")
                return
            return

        if event.inaxes is not self.ax:
            return

        if event.button == 3:
            if self.y_mono is None:
                return
            idx = self._nearest_onset(event)
            if idx is not None and idx > 0:
                self._delete_onset(idx)
            elif event.xdata is not None:
                self._add_onset_at(float(event.xdata))
            return

        if event.button != 1:
            return

        self._press_x_pixel = event.x
        self._press_xlim = self.ax.get_xlim()
        idx = self._nearest_onset(event)
        if idx is not None:
            self._drag_idx = idx
            self._mode = "drag_cut"
        else:
            self._mode = "click_or_pan"

    def _slice_range_at(self, x_sec):
        if len(self.onsets) == 0 or self.y_mono is None:
            return None
        sample_pos = int(x_sec * self.sr)
        idx = int(np.searchsorted(self.onsets, sample_pos, side="right") - 1)
        idx = max(0, idx)
        start = int(self.onsets[idx])
        end = int(self.onsets[idx + 1]) if idx + 1 < len(self.onsets) else len(self.y_mono)
        return idx, start, end

    def _toggle_selection_at(self, x_sec):
        info = self._slice_range_at(x_sec)
        if info is None:
            return
        idx, _, _ = info
        # Une slice "deleted" ne peut pas être sélectionnée — on retire le delete
        self._deleted_indices.discard(idx)
        if idx in self._selected_indices:
            self._selected_indices.discard(idx)
        else:
            self._selected_indices.add(idx)
        self._render_selection_cells()
        n = len(self._selected_indices)
        if n == 0:
            self.status.config(text="No slice selected — export will include all")
        else:
            self.status.config(text=f"{n} slice{'s' if n > 1 else ''} selected")

    def _toggle_delete_at(self, x_sec):
        """Right-click sur une cellule : marque la section en rouge.
        Le bouton "Delete marked" applique réellement la suppression."""
        info = self._slice_range_at(x_sec)
        if info is None:
            return
        idx, _, _ = info
        # Mutuellement exclusif avec la sélection
        self._selected_indices.discard(idx)
        if idx in self._deleted_indices:
            self._deleted_indices.discard(idx)
        else:
            self._deleted_indices.add(idx)
        self._render_selection_cells()
        n_del = len(self._deleted_indices)
        if n_del == 0:
            self.status.config(text="No section marked for deletion.")
        else:
            self.status.config(
                text=f"{n_del} section{'s' if n_del > 1 else ''} marked. "
                     "Click 'Delete marked' to apply."
            )

    def _toggle_loop_play(self):
        """Lance / arrête la lecture en boucle du sample entier."""
        if not AUDIO_AVAILABLE:
            self.status.config(text="Playback unavailable (install sounddevice)")
            return
        if self._is_looping:
            self._stop_loop_if_playing()
            return
        if self.audio is None or self.sr is None:
            self.status.config(text="Load a WAV first.")
            return
        try:
            sd.stop()
            sd.play(self.audio, self.sr, loop=True)
        except Exception as e:
            self.status.config(text=f"Playback error: {e}")
            return
        import time as _time
        self._is_looping = True
        self._play_start_time = _time.time()
        self.loop_btn.config(text="⏹ Stop loop", bg="#d04848",
                             activebackground="#b03838")
        # Crée la tête de lecture jaune et démarre la boucle de mise à jour
        self._playhead_line = self.ax.axvline(
            0, color="#ffd866", linewidth=1.6, alpha=0.95, zorder=5,
        )
        self._update_playhead()
        dur = len(self.y_mono) / self.sr if self.sr else 0
        self.status.config(text=f"▶ Looping ({dur:.2f}s)")

    def _update_playhead(self):
        """Met à jour la position de la tête de lecture toutes les ~33 ms."""
        if not self._is_looping or self.y_mono is None or self.sr is None:
            return
        import time as _time
        elapsed = _time.time() - self._play_start_time
        duration = len(self.y_mono) / self.sr
        if duration <= 0:
            return
        pos = elapsed % duration
        if self._playhead_line is not None:
            try:
                self._playhead_line.set_xdata([pos, pos])
                self.canvas.draw_idle()
            except Exception:
                pass
        self._playhead_after_id = self.after(33, self._update_playhead)

    def _stop_loop_if_playing(self):
        if not self._is_looping:
            return
        try:
            sd.stop()
        except Exception:
            pass
        self._is_looping = False
        if self._playhead_after_id is not None:
            try:
                self.after_cancel(self._playhead_after_id)
            except Exception:
                pass
            self._playhead_after_id = None
        if self._playhead_line is not None:
            try:
                self._playhead_line.remove()
            except (ValueError, AttributeError):
                pass
            self._playhead_line = None
            self.canvas.draw_idle()
        self.loop_btn.config(text="▶ Loop", bg="#1976D2",
                             activebackground="#1565C0")
        self.status.config(text="Loop stopped")

    def _commit_deletions(self):
        """Applique les suppressions marquées : retire l'audio des sections
        rouges du buffer et reconstruit les onsets en conservant les autres
        cuts à leur position relative dans le nouveau timeline."""
        if not self._deleted_indices or self.y_mono is None:
            self.status.config(text="No section marked for deletion.")
            return
        self._stop_loop_if_playing()
        n = len(self.onsets)
        deleted = {i for i in self._deleted_indices if 0 <= i < n}
        keep = [i for i in range(n) if i not in deleted]
        if not keep:
            self.status.config(text="Cannot delete all sections.")
            return

        # Reconstruction du buffer audio (mono ou stéréo) en concaténant
        # uniquement les slices non supprimées
        audio_chunks = []
        y_chunks = []
        new_onsets = [0]
        cumulative = 0
        for k_pos, idx in enumerate(keep):
            start = int(self.onsets[idx])
            end = int(self.onsets[idx + 1]) if idx + 1 < n else len(self.y_mono)
            audio_chunks.append(self.audio[start:end])
            y_chunks.append(self.y_mono[start:end])
            cumulative += end - start
            if k_pos < len(keep) - 1:
                new_onsets.append(cumulative)
        self.audio = np.concatenate(audio_chunks, axis=0)
        self.y_mono = np.concatenate(y_chunks, axis=0)
        self.onsets = np.array(new_onsets, dtype=np.int64)

        n_deleted = len(deleted)
        self._deleted_indices.clear()
        self._selected_indices.clear()
        self._cycle_idx = -1

        # Redraw waveform + onset lines avec les nouveaux onsets (preservés)
        self._draw_waveform()
        for line in self._onset_lines:
            try:
                line.remove()
            except (ValueError, AttributeError):
                pass
        self._onset_lines = [
            self.ax.axvline(s / self.sr, color=ONSET_COLOR, linewidth=0.8, alpha=0.85)
            for s in self.onsets
        ]
        self.count_label.config(text=f"{len(self.onsets)} transients detected")
        self._render_selection_cells()
        self.canvas.draw_idle()

        new_dur = len(self.y_mono) / self.sr if self.sr else 0
        self.status.config(
            text=f"Deleted {n_deleted} section(s). New duration: {new_dur:.2f}s"
        )

    def _render_selection_cells(self):
        for cell in self._selection_cells:
            try:
                cell.remove()
            except (ValueError, AttributeError):
                pass
        self._selection_cells = []

        if self.y_mono is None or len(self.onsets) == 0:
            self.canvas.draw_idle()
            return

        # Filtre les indices encore valides
        n_slices = len(self.onsets)
        self._selected_indices = {i for i in self._selected_indices if 0 <= i < n_slices}
        self._deleted_indices = {i for i in self._deleted_indices if 0 <= i < n_slices}
        selected_indices = self._selected_indices
        deleted_indices = self._deleted_indices

        DELETE_COLOR = "#d04848"  # rouge pour slices supprimées
        for i in range(n_slices):
            start = int(self.onsets[i])
            end = int(self.onsets[i + 1]) if i + 1 < n_slices else len(self.y_mono)
            x0 = start / self.sr
            w = max(1.0 / self.sr, (end - start) / self.sr)
            if i in deleted_indices:
                facecolor, edgecolor = DELETE_COLOR, DELETE_COLOR
            elif i in selected_indices:
                facecolor, edgecolor = SELECTION_COLOR, SELECTION_COLOR
            else:
                facecolor, edgecolor = "#3a3a3a", "#555"
            rect = Rectangle(
                (x0, 0.12), w, 0.76,
                facecolor=facecolor, edgecolor=edgecolor,
                linewidth=0.8,
            )
            self.sel_ax.add_patch(rect)
            self._selection_cells.append(rect)

        self.canvas.draw_idle()

    def _clear_selection(self):
        self._selected_indices.clear()
        self._deleted_indices.clear()
        self._render_selection_cells()

    def _select_all(self):
        if self.y_mono is None or len(self.onsets) == 0:
            return
        self._selected_indices = set(range(len(self.onsets)))
        self._render_selection_cells()
        self.status.config(text=f"All {len(self.onsets)} slices selected")

    def _deselect_all(self):
        if not self._selected_indices:
            return
        self._selected_indices.clear()
        self._render_selection_cells()
        self.status.config(text="Selection cleared (export will include all slices)")

    def _reset_to_default(self):
        """Remet sliders à leurs valeurs par défaut ET recharge l'audio depuis
        le fichier d'origine (annule les "Delete marked" appliqués)."""
        if self.input_path is None:
            self.status.config(text="Load a WAV first.")
            return
        self.sensitivity.set(1.0)
        self.min_gap.set(30)
        self.sens_label.config(text="1.00")
        self.gap_label.config(text="30")
        self._selected_indices.clear()
        self._deleted_indices.clear()
        # Reload depuis le disque : restore l'audio source intact + redétecte
        # avec les params défaut (le worker thread lit self.sensitivity/min_gap
        # qu'on vient de remettre à 1.0 / 30)
        self._load_path(self.input_path)

    def _add_onset_at(self, x_sec):
        if self.y_mono is None:
            return
        sample = int(np.clip(x_sec * self.sr, 1, len(self.y_mono) - 1))
        insert_idx = int(np.searchsorted(self.onsets, sample))
        if insert_idx < len(self.onsets) and int(self.onsets[insert_idx]) == sample:
            return
        self.onsets = np.insert(self.onsets, insert_idx, sample)
        line = self.ax.axvline(
            sample / self.sr, color=ONSET_COLOR, linewidth=0.8, alpha=0.85,
        )
        self._onset_lines.insert(insert_idx, line)
        # Décale les indices sélectionnés / supprimés ≥ insert_idx
        self._selected_indices = {
            (i + 1 if i >= insert_idx else i) for i in self._selected_indices
        }
        self._deleted_indices = {
            (i + 1 if i >= insert_idx else i) for i in self._deleted_indices
        }
        self.count_label.config(text=f"{len(self.onsets)} transients detected")
        self.status.config(text=f"+ cut at {sample / self.sr:.3f}s")
        self._render_selection_cells()
        self.canvas.draw_idle()

    def _delete_onset(self, idx):
        if idx <= 0 or idx >= len(self.onsets):
            return
        try:
            self._onset_lines[idx].remove()
        except (ValueError, AttributeError):
            pass
        self._onset_lines.pop(idx)
        removed_t = float(self.onsets[idx]) / self.sr
        self.onsets = np.delete(self.onsets, idx)
        # Décale les indices sélectionnés / supprimés > idx (la slice idx-1
        # absorbe l'ancienne idx)
        self._selected_indices = {
            (i - 1 if i > idx else i)
            for i in self._selected_indices
            if i != idx
        }
        self._deleted_indices = {
            (i - 1 if i > idx else i)
            for i in self._deleted_indices
            if i != idx
        }
        self.count_label.config(text=f"{len(self.onsets)} transients detected")
        self.status.config(text=f"− cut at {removed_t:.3f}s removed")
        self._render_selection_cells()
        self.canvas.draw_idle()

    def _on_motion(self, event):
        # Drag-select dans le bandeau supérieur : étend la sélection / le
        # marquage aux cellules adjacentes pendant qu'on maintient le clic
        if self._mode in ("select_drag", "delete_drag"):
            if event.inaxes is self.sel_ax and event.xdata is not None:
                info = self._slice_range_at(float(event.xdata))
                if info is not None:
                    idx = info[0]
                    if idx not in self._drag_visited:
                        self._drag_visited.add(idx)
                        if self._mode == "select_drag":
                            self._deleted_indices.discard(idx)
                            if self._drag_state:
                                self._selected_indices.add(idx)
                            else:
                                self._selected_indices.discard(idx)
                        else:  # delete_drag
                            self._selected_indices.discard(idx)
                            if self._drag_state:
                                self._deleted_indices.add(idx)
                            else:
                                self._deleted_indices.discard(idx)
                        self._render_selection_cells()
            return
        if event.inaxes != self.ax:
            return
        if self._mode is None:
            idx = self._nearest_onset(event)
            cursor = "sb_h_double_arrow" if idx is not None else ""
            try:
                self.canvas.get_tk_widget().config(cursor=cursor)
            except tk.TclError:
                pass
            return
        if event.x is None:
            return
        if self._mode == "drag_cut":
            if event.xdata is None or self.y_mono is None:
                return
            x = float(np.clip(event.xdata, 0.0, len(self.y_mono) / self.sr))
            new_sample = int(np.clip(x * self.sr, 0, len(self.y_mono) - 1))
            self.onsets[self._drag_idx] = new_sample
            self._onset_lines[self._drag_idx].set_xdata([x, x])
            # Mise à jour live des selection cells (sinon elles ne suivent pas
            # le déplacement du cut tant que l'utilisateur n'a pas relâché)
            self._render_selection_cells()
            self.status.config(text=f"Cut #{self._drag_idx + 1} → {x:.3f}s")
            self.canvas.draw_idle()
            return
        if self._mode == "click_or_pan":
            if abs(event.x - self._press_x_pixel) <= self._click_motion_threshold_px:
                return
            self._mode = "pan"
        if self._mode == "pan" and self.y_mono is not None:
            ax_width = max(1.0, self.ax.bbox.width)
            xmin0, xmax0 = self._press_xlim
            data_per_px = (xmax0 - xmin0) / ax_width
            delta_data = -(event.x - self._press_x_pixel) * data_per_px
            new_min = xmin0 + delta_data
            new_max = xmax0 + delta_data
            max_t = len(self.y_mono) / self.sr
            if new_min < 0:
                new_max -= new_min
                new_min = 0
            if new_max > max_t:
                shift = new_max - max_t
                new_min = max(0, new_min - shift)
                new_max = max_t
            self.ax.set_xlim(new_min, new_max)
            self.canvas.draw_idle()

    def _on_release(self, event):
        mode = self._mode
        self._mode = None
        if mode in ("select_drag", "delete_drag"):
            self._drag_visited = set()
            return
        if mode == "drag_cut":
            self._drag_idx = None
            order = np.argsort(self.onsets)
            # Si le tri a changé l'ordre (drag à travers un autre cut), il
            # faut remapper les indices sélectionnés vers leur nouvelle position
            if not np.array_equal(order, np.arange(len(self.onsets))):
                old_to_new = {int(old): new for new, old in enumerate(order)}
                self._selected_indices = {
                    old_to_new[i] for i in self._selected_indices if i in old_to_new
                }
                self._deleted_indices = {
                    old_to_new[i] for i in self._deleted_indices if i in old_to_new
                }
            self.onsets = self.onsets[order]
            self._onset_lines = [self._onset_lines[i] for i in order]
            self._render_selection_cells()
            self.canvas.draw_idle()
        elif mode == "click_or_pan" and event.xdata is not None:
            self._play_slice_at(float(event.xdata))

    def _play_slice_at(self, x_sec):
        if self.audio is None or self.sr is None:
            return
        if not AUDIO_AVAILABLE:
            self.status.config(text="Playback unavailable (install sounddevice)")
            return
        if len(self.onsets) == 0:
            return
        sample_pos = int(x_sec * self.sr)
        idx = int(np.searchsorted(self.onsets, sample_pos, side="right") - 1)
        idx = max(0, idx)
        start = int(self.onsets[idx])
        end = int(self.onsets[idx + 1]) if idx + 1 < len(self.onsets) else len(self.y_mono)
        chunk = self.audio[start:end]
        try:
            sd.stop()
            sd.play(chunk, self.sr)
        except Exception as e:
            self.status.config(text=f"Playback error: {e}")
            return
        self._highlight_slice(start, end)
        self.status.config(
            text=f"▶ Slice #{idx + 1}  ·  {start/self.sr:.3f}s → {end/self.sr:.3f}s  ({(end-start)/self.sr:.3f}s)"
        )

    def _highlight_slice(self, start, end):
        if self._playing_span is not None:
            try:
                self._playing_span.remove()
            except (ValueError, AttributeError):
                pass
        self._playing_span = self.ax.axvspan(
            start / self.sr, end / self.sr,
            color="#ffd866", alpha=0.18, zorder=0,
        )
        self.canvas.draw_idle()

    # ── Rendu waveform ───────────────────────────────────────────────────────

    def _setup_axes_styling(self):
        self.ax.set_facecolor(BG_PLOT)
        self.ax.tick_params(colors="white")
        for spine in self.ax.spines.values():
            spine.set_color("white")
        self.ax.set_ylim(-1.05, 1.05)
        self.ax.set_xlabel("Time (s)", color="white")
        self.ax.grid(True, axis="x", alpha=0.15)

    def _clear_plot_artists(self):
        for art in self._wave_artists:
            try:
                art.remove()
            except (ValueError, AttributeError):
                pass
        self._wave_artists = []
        for ln in self._onset_lines:
            try:
                ln.remove()
            except (ValueError, AttributeError):
                pass
        self._onset_lines = []
        for cell in self._selection_cells:
            try:
                cell.remove()
            except (ValueError, AttributeError):
                pass
        self._selection_cells = []
        if self._playing_span is not None:
            try:
                self._playing_span.remove()
            except (ValueError, AttributeError):
                pass
            self._playing_span = None
        if self._hint_artist is not None:
            try:
                self._hint_artist.remove()
            except (ValueError, AttributeError):
                pass
            self._hint_artist = None

    def _show_drop_hint(self):
        self._clear_plot_artists()
        self.ax.set_xlim(0, 1)
        self.ax.set_xticks([])
        self.ax.set_yticks([])
        msg = "Drop a WAV file here" if DND_AVAILABLE else "Open a WAV file to begin"
        self._hint_artist = self.ax.text(
            0.5, 0.5, msg,
            transform=self.ax.transAxes, ha="center", va="center",
            color="#888", fontsize=14,
        )
        self.canvas.draw_idle()

    def _on_xlim_changed(self, _ax):
        if self._rendering or self.y_mono is None:
            return
        xmin, xmax = self.ax.get_xlim()
        max_t = len(self.y_mono) / self.sr
        c_min = max(0.0, xmin)
        c_max = min(max_t, xmax)
        if (c_min, c_max) != (xmin, xmax) and c_max > c_min:
            self._rendering = True
            try:
                self.ax.set_xlim(c_min, c_max)
            finally:
                self._rendering = False
        self._render_visible_waveform()

    def _on_sens_change(self, _):
        self.sens_label.config(text=f"{self.sensitivity.get():.2f}")
        self._refresh_onsets()

    def _on_gap_change(self, _):
        self.gap_label.config(text=f"{int(self.min_gap.get())}")
        self._refresh_onsets()

    def open_file(self):
        path = filedialog.askopenfilename(
            title="Choose a WAV file",
            filetypes=[("WAV", "*.wav"), ("All", "*.*")],
        )
        if path:
            self._load_path(path)

    def _load_path(self, path):
        """Lance la lecture du WAV en background pour ne pas freezer l'UI
        (sf.read + numba JIT de librosa.onset_detect peuvent prendre 20-30s
        au cold-start dans le bundle PyInstaller)."""
        self._stop_loop_if_playing()
        path = Path(path)
        self.status.config(text=f"Loading {path.name}… (decoding + transient detection)")
        self.file_label.config(text=_truncate_name(path.name, 42))
        try:
            self.canvas.get_tk_widget().config(cursor="watch")
        except tk.TclError:
            pass
        import threading
        threading.Thread(
            target=self._load_path_worker, args=(path,), daemon=True,
        ).start()

    def _load_path_worker(self, path: Path) -> None:
        try:
            audio, sr = sf.read(str(path), always_2d=False)
            y_mono = (librosa.to_mono(audio.T) if audio.ndim > 1 else audio).astype(np.float32)
            onsets = detect_transients(
                y_mono, sr,
                sensitivity=float(self.sensitivity.get()),
                min_gap_ms=float(self.min_gap.get()),
            )
        except Exception as e:
            err = str(e)
            self.after(0, lambda: self._on_load_error(path, err))
            return
        self.after(0, lambda: self._on_load_done(path, audio, sr, y_mono, onsets))

    def _on_load_error(self, path: Path, err: str) -> None:
        try:
            self.canvas.get_tk_widget().config(cursor="")
        except tk.TclError:
            pass
        self.status.config(text=f"Failed to load {path.name}")
        messagebox.showerror("Error", f"Cannot read file:\n{path}\n\n{err}")

    def _on_load_done(self, path: Path, audio, sr, y_mono, onsets) -> None:
        self.audio = audio
        self.sr = sr
        self.y_mono = y_mono
        self.input_path = path
        self.onsets = onsets
        self._cycle_idx = -1
        ch = 1 if audio.ndim == 1 else audio.shape[1]
        short = _truncate_name(path.name, 24)
        self.file_label.config(
            text=f"{short}  ·  {sr} Hz  ·  {ch} ch  ·  {len(y_mono)/sr:.2f}s"
        )
        self._draw_waveform()
        for line in self._onset_lines:
            line.remove()
        self._onset_lines = [
            self.ax.axvline(s / self.sr, color=ONSET_COLOR, linewidth=0.8, alpha=0.85)
            for s in self.onsets
        ]
        self.count_label.config(text=f"{len(self.onsets)} transients detected")
        self._clear_selection()
        self._render_selection_cells()
        self.canvas.draw_idle()
        try:
            self.canvas.get_tk_widget().config(cursor="")
        except tk.TclError:
            pass
        self.status.config(text=f"Loaded: {path}")

    def _draw_waveform(self):
        self._clear_plot_artists()
        if self.y_mono is None:
            self._show_drop_hint()
            return
        self.ax.tick_params(axis="both", which="both", colors="white")
        self.ax.xaxis.set_major_locator(mticker.AutoLocator())
        self.ax.yaxis.set_major_locator(mticker.AutoLocator())
        self.ax.set_ylim(-1.05, 1.05)
        self._rendering = True
        try:
            self.ax.set_xlim(0, len(self.y_mono) / self.sr)
        finally:
            self._rendering = False
        self._render_visible_waveform()

    def _render_visible_waveform(self):
        if self.y_mono is None or self._rendering:
            return
        self._rendering = True
        try:
            for art in self._wave_artists:
                try:
                    art.remove()
                except (ValueError, AttributeError):
                    pass
            self._wave_artists = []

            xmin, xmax = self.ax.get_xlim()
            n = len(self.y_mono)
            max_t = n / self.sr
            xmin = max(0.0, xmin)
            xmax = min(max_t, xmax)
            if xmax - xmin < 1e-9:
                self.canvas.draw_idle()
                return

            s_lo = max(0, int(np.floor(xmin * self.sr)))
            s_hi = min(n, int(np.ceil(xmax * self.sr)) + 1)
            visible = self.y_mono[s_lo:s_hi]
            if len(visible) < 2:
                self.canvas.draw_idle()
                return

            ax_width_px = max(1.0, self.ax.bbox.width)
            spp = len(visible) / ax_width_px

            if spp >= 4:
                bins = max(1, int(ax_width_px * 2))
                bin_size = max(1, len(visible) // bins)
                usable = (len(visible) // bin_size) * bin_size
                if usable < bin_size:
                    self.canvas.draw_idle()
                    return
                blk = visible[:usable].reshape(-1, bin_size)
                mins = blk.min(axis=1)
                maxs = blk.max(axis=1)
                rms = np.sqrt(np.mean(blk.astype(np.float64) ** 2, axis=1)).astype(np.float32)
                t = (s_lo + np.arange(len(mins)) * bin_size + bin_size / 2.0) / self.sr
                peak = self.ax.fill_between(
                    t, mins, maxs,
                    color=WAVE_COLOR, linewidth=0, alpha=WAVE_PEAK_ALPHA,
                )
                rms_fill = self.ax.fill_between(
                    t, -rms, rms,
                    color=WAVE_COLOR, linewidth=0,
                )
                self._wave_artists.extend([peak, rms_fill])
            else:
                t = (s_lo + np.arange(len(visible))) / self.sr
                line, = self.ax.plot(t, visible, color=WAVE_COLOR, linewidth=1.2)
                self._wave_artists.append(line)
                if spp < 0.3:
                    dots, = self.ax.plot(
                        t, visible, "o",
                        color=WAVE_COLOR, markersize=3, markeredgewidth=0,
                    )
                    self._wave_artists.append(dots)

            zero_line = self.ax.axhline(0, color="#ffffff", linewidth=0.4, alpha=0.15, zorder=0)
            self._wave_artists.append(zero_line)

            self.canvas.draw_idle()
        finally:
            self._rendering = False

    def _refresh_onsets(self):
        if self.y_mono is None:
            return
        self.onsets = detect_transients(
            self.y_mono, self.sr,
            sensitivity=float(self.sensitivity.get()),
            min_gap_ms=float(self.min_gap.get()),
        )
        self._cycle_idx = -1
        for line in self._onset_lines:
            line.remove()
        self._onset_lines = [
            self.ax.axvline(s / self.sr, color=ONSET_COLOR, linewidth=0.8, alpha=0.85)
            for s in self.onsets
        ]
        self.count_label.config(text=f"{len(self.onsets)} transients detected")
        self._clear_selection()
        self._render_selection_cells()
        self.canvas.draw_idle()

    # ── Export ───────────────────────────────────────────────────────────────

    def _selected_slice_indices(self):
        """Retourne la liste d'indices à exporter, ou None pour "tout".

        Logique :
        - Si des slices sont marquées "deleted" : exporte tout SAUF elles
          (en respectant aussi la sélection si non vide)
        - Sinon, si une sélection existe : exporte uniquement les sélectionnées
        - Sinon : retourne None → exporte tout
        """
        if len(self.onsets) == 0:
            return None
        n = len(self.onsets)
        deleted = {i for i in self._deleted_indices if 0 <= i < n}
        selected = {i for i in self._selected_indices if 0 <= i < n}
        if not deleted and not selected:
            return None
        if selected:
            base = selected
        else:
            base = set(range(n))
        result = base - deleted
        return sorted(result) if result else None

    def _slice_to(self, output_dir: Path) -> list[Path]:
        """Génère les slices dans output_dir et retourne la liste de paths."""
        selected = self._selected_slice_indices()
        manifest = slice_wave(
            self.input_path, output_dir,
            onsets=self.onsets,
            slice_indices=selected,
        )
        return [output_dir / s["file"] for s in manifest["slices"]]

    def export_slices(self):
        if self.input_path is None:
            messagebox.showwarning("No file", "Load a WAV first.")
            return
        out = filedialog.askdirectory(title="Output folder")
        if not out:
            return
        try:
            paths = self._slice_to(Path(out))
        except Exception as e:
            messagebox.showerror("Error", str(e))
            return
        n = len(paths)
        scope = "selected" if self._selected_slice_indices() else "all"
        self.status.config(text=f"{n} {scope} slice(s) written to {out}")
        messagebox.showinfo("Done", f"{n} {scope} slice(s) written to:\n{out}")

    # ── MIDI export ──────────────────────────────────────────────────────────

    def _generate_midi_temp(self) -> Optional[Path]:
        """Génère un fichier .mid temp : 1 note par slice, chromatique C2+.

        Le tempo BPM est calculé pour que la durée totale du sample = N beats
        (où N est lu depuis self.beats_var). Comme ça le DAW aligne les notes
        sur sa grille de beats à l'import."""
        if not MIDI_AVAILABLE:
            self.status.config(text="MIDI export unavailable (mido not installed)")
            return None
        if self.input_path is None or len(self.onsets) == 0 or self.sr is None:
            self.status.config(text="Load a WAV and detect transients first.")
            return None

        ppq = 480
        total_dur_sec = len(self.y_mono) / self.sr if self.sr else 0.0
        try:
            n_beats = max(1, int(self.beats_var.get()))
        except (tk.TclError, ValueError):
            n_beats = 16
        if total_dur_sec > 0:
            bpm = n_beats * 60.0 / total_dur_sec
        else:
            bpm = 120.0
        tempo = mido.bpm2tempo(bpm)  # microsec per beat
        # 1 second = bpm/60 beats = bpm/60 * ppq ticks
        sec_to_ticks = lambda s: int(round(s * bpm / 60 * ppq))

        mid = mido.MidiFile(ticks_per_beat=ppq)
        track = mido.MidiTrack()
        mid.tracks.append(track)
        track.append(mido.MetaMessage("set_tempo", tempo=tempo, time=0))
        track.append(mido.MetaMessage("track_name", name=self.input_path.stem[:32], time=0))

        base_note = 36  # C2
        last_tick = 0
        total_samples = len(self.y_mono)
        for i, onset in enumerate(self.onsets):
            start_sec = float(onset) / self.sr
            end_sample = int(self.onsets[i + 1]) if i + 1 < len(self.onsets) else total_samples
            end_sec = float(end_sample) / self.sr
            start_tick = sec_to_ticks(start_sec)
            end_tick = sec_to_ticks(end_sec)
            note = min(127, base_note + i)
            delta_on = max(0, start_tick - last_tick)
            delta_off = max(1, end_tick - start_tick)
            track.append(mido.Message("note_on", note=note, velocity=100, time=delta_on))
            track.append(mido.Message("note_off", note=note, velocity=0, time=delta_off))
            last_tick = end_tick

        out_dir = Path(tempfile.mkdtemp(prefix="a3000_slicer_midi_"))
        midi_path = out_dir / f"{self.input_path.stem}_slices.mid"
        mid.save(str(midi_path))
        return midi_path

    def _on_midi_drag_init(self, event):
        """Handler tkinterdnd2 : génère le .mid et le passe en drag-out OLE."""
        midi_path = self._generate_midi_temp()
        if midi_path is None:
            return None
        self.status.config(text=f"Dragging {midi_path.name} → drop in your DAW")
        # Wrap dans {} : Tcl list syntax pour échapper () et espaces dans le path
        # (sans ça, tkdnd parse mal et le DAW refuse le drop)
        return (("copy",), (DND_FILES,), "{" + str(midi_path) + "}")

    def _on_send_to_upload_click(self):
        if self.input_path is None or len(self.onsets) == 0:
            self.status.config(text="Load a WAV and detect transients first.")
            return
        if self.on_send_to_upload is None:
            self.status.config(text="No upload handler configured.")
            return
        try:
            out_dir = Path(tempfile.mkdtemp(prefix="a3000_slicer_"))
            paths = self._slice_to(out_dir)
        except Exception as e:
            messagebox.showerror("Error", str(e))
            return
        if not paths:
            self.status.config(text="No slices generated.")
            return
        self.status.config(text=f"Sent {len(paths)} slice(s) to Upload tab.")
        self.on_send_to_upload(paths)
