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
        self._selected_midpoints = set()
        self._selection_cells = []
        self._cycle_idx = -1  # index du cut actuellement centré (Space pour cycler)

        self._build_ui()
        self._wire_canvas_events()

    # ── Construction UI ──────────────────────────────────────────────────────

    def _build_ui(self):
        top = ttk.Frame(self, padding=8)
        top.pack(side=tk.TOP, fill=tk.X)

        ttk.Button(top, text="Open WAV…", command=self.open_file).pack(side=tk.LEFT)
        self.file_label = ttk.Label(top, text="No file loaded")
        self.file_label.pack(side=tk.LEFT, padx=10)

        # Boutons d'export à droite : Send to Upload (bleu) + Export to folder
        self.send_btn = tk.Button(
            top, text="Send to Upload", command=self._on_send_to_upload_click,
            bg="#1976D2", fg="white",
            activebackground="#1565C0", activeforeground="white",
            disabledforeground="#BDBDBD",
            font=("", 9, "bold"),
            relief="raised", borderwidth=1, padx=14, pady=2,
            cursor="hand2",
        )
        self.send_btn.pack(side=tk.RIGHT)
        ttk.Button(top, text="Export to folder…", command=self.export_slices).pack(
            side=tk.RIGHT, padx=(0, 8),
        )
        ttk.Button(top, text="Deselect all", command=self._deselect_all).pack(
            side=tk.RIGHT, padx=(0, 8),
        )
        ttk.Button(top, text="Select all", command=self._select_all).pack(
            side=tk.RIGHT, padx=(0, 4),
        )

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
            "Wheel: zoom  ·  Right click: add/remove cut  ·  "
            "Top strip: toggle slice export"
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
            if event.button == 1 and event.xdata is not None and self.y_mono is not None:
                self._toggle_selection_at(float(event.xdata))
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
        _, start, end = info
        existing = next(
            (m for m in self._selected_midpoints if start <= m < end),
            None,
        )
        if existing is not None:
            self._selected_midpoints.discard(existing)
        else:
            self._selected_midpoints.add((start + end) // 2)
        self._render_selection_cells()
        n = len(self._selected_midpoints)
        if n == 0:
            self.status.config(text="No slice selected — export will include all")
        else:
            self.status.config(text=f"{n} slice{'s' if n > 1 else ''} selected")

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

        selected_indices = set()
        valid_midpoints = set()
        for m in self._selected_midpoints:
            info = self._slice_range_at(m / self.sr) if self.sr else None
            if info is None:
                continue
            idx, start, end = info
            if start <= m < end:
                selected_indices.add(idx)
                valid_midpoints.add(m)
        self._selected_midpoints = valid_midpoints

        for i in range(len(self.onsets)):
            start = int(self.onsets[i])
            end = int(self.onsets[i + 1]) if i + 1 < len(self.onsets) else len(self.y_mono)
            x0 = start / self.sr
            w = max(1.0 / self.sr, (end - start) / self.sr)
            is_sel = i in selected_indices
            rect = Rectangle(
                (x0, 0.12), w, 0.76,
                facecolor=SELECTION_COLOR if is_sel else "#3a3a3a",
                edgecolor=SELECTION_COLOR if is_sel else "#555",
                linewidth=0.8,
            )
            self.sel_ax.add_patch(rect)
            self._selection_cells.append(rect)

        self.canvas.draw_idle()

    def _clear_selection(self):
        self._selected_midpoints.clear()
        self._render_selection_cells()

    def _select_all(self):
        if self.y_mono is None or len(self.onsets) == 0:
            return
        self._selected_midpoints.clear()
        for i in range(len(self.onsets)):
            start = int(self.onsets[i])
            end = int(self.onsets[i + 1]) if i + 1 < len(self.onsets) else len(self.y_mono)
            self._selected_midpoints.add((start + end) // 2)
        self._render_selection_cells()
        self.status.config(text=f"All {len(self.onsets)} slices selected")

    def _deselect_all(self):
        if not self._selected_midpoints:
            return
        self._selected_midpoints.clear()
        self._render_selection_cells()
        self.status.config(text="Selection cleared (export will include all slices)")

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
        self.count_label.config(text=f"{len(self.onsets)} transients detected")
        self.status.config(text=f"− cut at {removed_t:.3f}s removed")
        self._render_selection_cells()
        self.canvas.draw_idle()

    def _on_motion(self, event):
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
        if mode == "drag_cut":
            self._drag_idx = None
            order = np.argsort(self.onsets)
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
        path = Path(path)
        self.status.config(text=f"Loading {path.name}… (decoding + transient detection)")
        self.file_label.config(text=path.name)
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
        self.file_label.config(
            text=f"{path.name}  ·  {sr} Hz  ·  {ch} ch  ·  {len(y_mono)/sr:.2f}s"
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
        if not self._selected_midpoints or len(self.onsets) == 0:
            return None
        indices = set()
        for m in self._selected_midpoints:
            idx = int(np.searchsorted(self.onsets, m, side="right") - 1)
            idx = max(0, idx)
            start = int(self.onsets[idx])
            end = int(self.onsets[idx + 1]) if idx + 1 < len(self.onsets) else len(self.y_mono)
            if start <= m < end:
                indices.add(idx)
        return sorted(indices) if indices else None

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
