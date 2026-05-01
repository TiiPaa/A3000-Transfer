"""Moteur de découpe par transients.

Vendoré depuis I:\\Dev\\Simpler-Slicer\\slice_transients.py.

Fonctions exposées :
- detect_transients(y, sr, ...) -> np.ndarray d'indices d'échantillons
- slice_wave(input_path, output_dir, ..., onsets=..., slice_indices=...) -> manifest dict
"""
from __future__ import annotations

import json
from pathlib import Path

import numpy as np
import librosa
import soundfile as sf


def refine_onset(y, onset, sr, max_samples, smooth_ms=1,
                 bg_factor=5.0, abs_floor=0.005):
    """Avance depuis `onset` jusqu'au premier instant où l'amplitude dépasse
    significativement le bruit de fond local. Voir docstring source pour détails."""
    n = len(y)
    if onset >= n - 1 or max_samples < 2:
        return int(onset)
    hi = min(n, onset + max_samples + 1)
    region = np.abs(y[onset:hi]).astype(np.float64)
    if len(region) < 8:
        return int(onset)

    bg_samples = max(8, min(int(0.005 * sr), len(region) // 4))
    background = float(np.median(region[:bg_samples]))
    threshold = max(background * bg_factor, abs_floor)

    win = max(2, int(smooth_ms * sr / 1000))
    if len(region) > win:
        cumsum = np.concatenate(([0.0], np.cumsum(region)))
        smoothed = (cumsum[win:] - cumsum[:-win]) / win
    else:
        smoothed = region

    above = np.where(smoothed > threshold)[0]
    if len(above) == 0:
        return int(onset)
    return int(onset) + int(above[0])


def detect_transients(y, sr, sensitivity=1.0, hop_length=512,
                      backtrack=True, min_gap_ms=30, snap_ms=50):
    """Détecte les transients et retourne les indices d'échantillons.

    sensitivity : >1.0 => plus de transients, <1.0 => moins
    backtrack   : ramène l'onset au début de la montée d'énergie
    min_gap_ms  : écart minimum entre deux slices
    snap_ms     : fenêtre de recherche du début d'attaque en avant. 0 = désactivé.
    """
    delta = 0.07 / max(sensitivity, 1e-3)

    onset_frames = librosa.onset.onset_detect(
        y=y, sr=sr,
        hop_length=hop_length,
        backtrack=backtrack,
        delta=delta,
        units="frames",
    )
    onset_samples = librosa.frames_to_samples(onset_frames, hop_length=hop_length)

    if len(onset_samples) == 0 or onset_samples[0] > int(sr * 0.01):
        onset_samples = np.concatenate([[0], onset_samples])

    min_gap = int(sr * min_gap_ms / 1000)
    filtered = [int(onset_samples[0])]
    for s in onset_samples[1:]:
        if s - filtered[-1] >= min_gap:
            filtered.append(int(s))

    if snap_ms > 0:
        snap_samples = int(snap_ms * sr / 1000)
        buffer_samples = int(0.005 * sr)
        refined = [filtered[0]]
        for i in range(1, len(filtered)):
            if i + 1 < len(filtered):
                room = filtered[i + 1] - filtered[i] - buffer_samples
            else:
                room = len(y) - filtered[i] - 1
            max_fwd = max(0, min(room, snap_samples))
            if max_fwd < 2:
                refined.append(filtered[i])
            else:
                refined.append(refine_onset(y, filtered[i], sr, max_samples=max_fwd))
        deduped = [refined[0]]
        for s in refined[1:]:
            if s - deduped[-1] >= min_gap:
                deduped.append(s)
        filtered = deduped

    return np.array(filtered, dtype=np.int64)


def apply_fades(slice_audio, sr, fade_ms=2):
    """Petit fade in/out pour éviter les clicks aux jonctions."""
    fade_samples = int(sr * fade_ms / 1000)
    fade_samples = min(fade_samples, len(slice_audio) // 4)
    if fade_samples < 2:
        return slice_audio

    out = slice_audio.astype(np.float32, copy=True)
    fade_in = np.linspace(0.0, 1.0, fade_samples, dtype=np.float32)
    fade_out = np.linspace(1.0, 0.0, fade_samples, dtype=np.float32)

    if out.ndim == 1:
        out[:fade_samples] *= fade_in
        out[-fade_samples:] *= fade_out
    else:
        out[:fade_samples] *= fade_in[:, np.newaxis]
        out[-fade_samples:] *= fade_out[:, np.newaxis]
    return out


def slice_wave(input_path, output_dir, sensitivity=1.0, min_gap_ms=30,
               fade_ms=2, prefix=None, min_slice_ms=5, snap_ms=50,
               onsets=None, slice_indices=None):
    """Découpe le WAV et écrit les slices + un manifest JSON.

    Si `onsets` est fourni (array d'indices d'échantillons), il est utilisé
    tel quel et la détection automatique est court-circuitée.

    Si `slice_indices` est fourni (iterable d'indices 0-based), seules ces
    slices sont écrites. Sinon toutes les slices sont écrites.
    """
    input_path = Path(input_path)
    output_dir = Path(output_dir)
    output_dir.mkdir(parents=True, exist_ok=True)
    if prefix is None:
        prefix = input_path.stem

    audio, sr = sf.read(str(input_path), always_2d=False)

    y_mono = librosa.to_mono(audio.T) if audio.ndim > 1 else audio
    y_mono = y_mono.astype(np.float32)

    if onsets is None:
        onsets = detect_transients(
            y_mono, sr,
            sensitivity=sensitivity,
            min_gap_ms=min_gap_ms,
            snap_ms=snap_ms,
        )
    else:
        onsets = np.sort(np.asarray(onsets, dtype=np.int64))

    slices = []
    for i, start in enumerate(onsets):
        end = int(onsets[i + 1]) if i + 1 < len(onsets) else len(y_mono)
        slices.append((int(start), end))

    manifest = {
        "source": input_path.name,
        "sample_rate": int(sr),
        "total_samples": int(len(y_mono)),
        "duration_sec": round(len(y_mono) / sr, 6),
        "slices": [],
    }

    n_digits = max(3, len(str(len(slices))))
    min_slice_samples = int(sr * min_slice_ms / 1000)
    selected = set(slice_indices) if slice_indices is not None else None

    for i, (start, end) in enumerate(slices):
        if selected is not None and i not in selected:
            continue
        if end - start < min_slice_samples:
            continue
        chunk = audio[start:end]
        chunk = apply_fades(chunk, sr, fade_ms=fade_ms)

        out_name = f"{prefix}_slice_{str(i + 1).zfill(n_digits)}.wav"
        out_path = output_dir / out_name
        sf.write(str(out_path), chunk, sr)

        manifest["slices"].append({
            "index": i + 1,
            "file": out_name,
            "start_sample": start,
            "end_sample": end,
            "start_sec": round(start / sr, 6),
            "duration_sec": round((end - start) / sr, 6),
        })

    manifest_path = output_dir / f"{prefix}_slices.json"
    with open(manifest_path, "w", encoding="utf-8") as f:
        json.dump(manifest, f, indent=2, ensure_ascii=False)

    return manifest
