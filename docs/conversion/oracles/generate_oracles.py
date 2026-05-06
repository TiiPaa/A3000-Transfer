"""Capture les sorties de l'implémentation Python comme golden files pour
les tests de parité de l'implémentation Rust.

Lancer depuis la racine du projet :
    cd python && pip install -e . && cd ../docs/conversion/oracles
    python generate_oracles.py [--smdi] [--wav] [--midi] [--onset]

Sans flag : capture tous les modules.
"""
from __future__ import annotations

import argparse
import json
import random
import sys
import wave as wave_stdlib
from pathlib import Path

# Permet d'importer a3000_transfer même si pas pip install -e
ROOT = Path(__file__).resolve().parent.parent.parent.parent
sys.path.insert(0, str(ROOT / "python"))

from a3000_transfer.smdi import (  # noqa: E402
    encode_sample_header_request,
    master_identify_message,
    encode_begin_sample_transfer,
    encode_send_next_packet,
    encode_data_packet,
    encode_end_of_procedure,
    encode_abort_procedure,
    encode_delete_sample,
    encode_message,
    encode_sample_header,
    SampleHeader,
)


HERE = Path(__file__).resolve().parent
GOLDEN = HERE / "golden"


# ──────────────────────────────────────────────────────────────────────────
# SMDI codec oracles
# ──────────────────────────────────────────────────────────────────────────

def capture_smdi() -> None:
    out = GOLDEN / "smdi"
    out.mkdir(parents=True, exist_ok=True)

    def _hex(b: bytes) -> str:
        return b.hex()

    # Sample Header Request (codec encode-only ; decode testé via round-trip)
    cases = []
    for sn in [0, 1, 7, 100, 300, 1023, 65535]:
        cases.append({
            "sample_number": sn,
            "expected_hex": _hex(encode_sample_header_request(sn)),
        })
    (out / "encode_sample_header_request.json").write_text(
        json.dumps(cases, indent=2),
    )

    # Master Identify (deterministic, no input)
    (out / "master_identify.json").write_text(json.dumps({
        "expected_hex": _hex(master_identify_message()),
    }, indent=2))

    # Begin Sample Transfer (sample_number, data_packet_length)
    cases = []
    for sn, plen in [(0, 4096), (100, 8192), (1023, 1024), (65535, 16384)]:
        cases.append({
            "sample_number": sn,
            "data_packet_length": plen,
            "expected_hex": _hex(encode_begin_sample_transfer(sn, plen)),
        })
    (out / "encode_begin_sample_transfer.json").write_text(
        json.dumps(cases, indent=2),
    )

    # Send Next Packet (packet_number)
    cases = []
    for pn in [0, 1, 100, 0xFFFF, 0xFFFFFF]:
        cases.append({
            "packet_number": pn,
            "expected_hex": _hex(encode_send_next_packet(pn)),
        })
    (out / "encode_send_next_packet.json").write_text(
        json.dumps(cases, indent=2),
    )

    # Data Packet (packet_number, data)
    cases = []
    for pn, data in [
        (0, b"\x00\x01\x02\x03"),
        (1, bytes(range(64))),
        (100, b"\xff" * 1024),
        (65535, b"\x55\xaa" * 512),
    ]:
        cases.append({
            "packet_number": pn,
            "data_hex": _hex(data),
            "expected_hex": _hex(encode_data_packet(pn, data)),
        })
    (out / "encode_data_packet.json").write_text(
        json.dumps(cases, indent=2),
    )

    # End Of Procedure
    (out / "end_of_procedure.json").write_text(json.dumps({
        "expected_hex": _hex(encode_end_of_procedure()),
    }, indent=2))

    # Abort Procedure
    (out / "abort_procedure.json").write_text(json.dumps({
        "expected_hex": _hex(encode_abort_procedure()),
    }, indent=2))

    # Delete Sample
    cases = []
    for sn in [0, 100, 1023, 65535]:
        cases.append({
            "sample_number": sn,
            "expected_hex": _hex(encode_delete_sample(sn)),
        })
    (out / "encode_delete_sample.json").write_text(
        json.dumps(cases, indent=2),
    )

    # Sample Header (full encoding round-trip)
    headers = [
        SampleHeader(
            sample_number=100,
            bits_per_word=16,
            channels=1,
            sample_period_ns=22675,  # ~44100 Hz
            sample_length_words=44100,
            loop_start=0,
            loop_end=44100,
            loop_control=0,
            pitch_integer=60,
            pitch_fraction=0,
            name="test_sample",
        ),
        SampleHeader(
            sample_number=300,
            bits_per_word=16,
            channels=2,  # stéréo
            sample_period_ns=20833,  # ~48000 Hz
            sample_length_words=159784,
            loop_start=12345,
            loop_end=159784,
            loop_control=1,
            pitch_integer=64,
            pitch_fraction=12345,
            name="loop01",
        ),
    ]
    cases = []
    for h in headers:
        cases.append({
            "input": {
                "sample_number": h.sample_number,
                "bits_per_word": h.bits_per_word,
                "channels": h.channels,
                "sample_period_ns": h.sample_period_ns,
                "sample_length_words": h.sample_length_words,
                "loop_start": h.loop_start,
                "loop_end": h.loop_end,
                "loop_control": h.loop_control,
                "pitch_integer": h.pitch_integer,
                "pitch_fraction": h.pitch_fraction,
                "name": h.name,
            },
            "expected_hex": _hex(encode_sample_header(h)),
        })
    (out / "encode_sample_header.json").write_text(
        json.dumps(cases, indent=2),
    )

    print(f"[smdi] wrote {len(list(out.glob('*.json')))} fixture files to {out}")


# ──────────────────────────────────────────────────────────────────────────
# WAV reader oracles (Phase 1 — load_wave + dither TPDF reproductible)
# ──────────────────────────────────────────────────────────────────────────

def capture_wav() -> None:
    """Capture peek_wave_metadata + load_wave (path PCM_16 uniquement bit-à-bit).

    Pour les paths dithered, on capture pour information mais le test Rust
    utilisera une tolérance ±1 LSB (cf. DECISIONS.md).
    """
    import numpy as np
    import soundfile as sf

    from a3000_transfer.wav_reader import peek_wave_metadata, load_wave

    out = GOLDEN / "wav"
    out.mkdir(parents=True, exist_ok=True)
    inputs = HERE / "inputs" / "wavs"
    inputs.mkdir(parents=True, exist_ok=True)

    # Génère un set de WAV de test avec contenu déterministe
    cases_def = [
        # (filename, channels, sample_rate, frames, subtype)
        ("silence_mono_16_44100.wav",   1, 44100, 1024, "PCM_16"),
        ("silence_stereo_16_44100.wav", 2, 44100, 1024, "PCM_16"),
        ("sine_440_mono_16_44100.wav",  1, 44100, 4410, "PCM_16"),
        ("sine_440_stereo_16_48000.wav", 2, 48000, 4800, "PCM_16"),
        ("sine_440_mono_24_44100.wav",  1, 44100, 4410, "PCM_24"),
        ("sine_440_stereo_24_44100.wav", 2, 44100, 4410, "PCM_24"),
        ("sine_440_mono_32_44100.wav",  1, 44100, 4410, "PCM_32"),
        ("sine_440_stereo_32f_44100.wav", 2, 44100, 4410, "FLOAT"),
        ("sine_440_mono_8_44100.wav",   1, 44100, 4410, "PCM_U8"),
    ]

    manifest = {"cases": []}
    for name, ch, sr, frames, subtype in cases_def:
        path = inputs / name
        # Génère le contenu : silence si nom contient "silence", sine 440 Hz sinon
        t = np.arange(frames) / sr
        if "silence" in name:
            wave_data = np.zeros((frames, ch), dtype=np.float64)
        else:
            sine = 0.5 * np.sin(2 * np.pi * 440.0 * t)
            wave_data = np.tile(sine[:, None], (1, ch))
        # Conversion vers le subtype voulu
        if subtype == "FLOAT":
            data = wave_data.astype(np.float32)
        elif subtype == "PCM_U8":
            # 8-bit unsigned : soundfile veut int16 + subtype="PCM_U8" pour write
            data = (wave_data * 32767).astype(np.int16)
        elif subtype == "PCM_16":
            data = (wave_data * 32767).astype(np.int16)
        elif subtype == "PCM_24":
            data = (wave_data * 8388607).astype(np.int32)  # soundfile gère 24-bit depuis int32
        elif subtype == "PCM_32":
            data = (wave_data * 2147483647).astype(np.int32)
        else:
            raise ValueError(f"unknown subtype {subtype}")
        sf.write(str(path), data, sr, subtype=subtype)

        # Capture peek_wave_metadata
        meta = peek_wave_metadata(path)
        meta_dict = {
            "channels": meta.channels,
            "sample_rate": meta.sample_rate,
            "bits_per_sample": meta.bits_per_sample,
            "frame_count": meta.frame_count,
            "byte_count": meta.byte_count,
            "duration_s": meta.duration_s,
        }
        # Capture load_wave PCM 16-bit LE bytes (avec dither, pour info)
        wave_payload = load_wave(path)
        pcm_bin = out / f"{path.stem}.pcm16"
        pcm_bin.write_bytes(wave_payload.pcm_data)

        # Pour les paths dithered, on stocke aussi la version SANS dither
        # (round simple de float×32767). Le test Rust check |rust - no_dither| ≤ 1,
        # le dither ne peut que rajouter ±1 LSB à round(float×32767).
        if subtype != "PCM_16":
            data_f32, _ = sf.read(str(path), dtype="float32", always_2d=True)
            no_dither = np.clip(data_f32 * 32767.0, -32768.0, 32767.0).astype(np.int16)
            pcm_bin_nd = out / f"{path.stem}.no_dither.pcm16"
            pcm_bin_nd.write_bytes(no_dither.tobytes())
        else:
            pcm_bin_nd = None

        manifest["cases"].append({
            "input_filename": name,
            "subtype": subtype,
            "expected_metadata": meta_dict,
            "pcm16_filename": pcm_bin.name,
            "pcm16_byte_count": len(wave_payload.pcm_data),
            "no_dither_filename": pcm_bin_nd.name if pcm_bin_nd else None,
            "oracle_strategy": "exact" if subtype == "PCM_16" else "tolerance_1_lsb_vs_no_dither",
        })

    (out / "manifest.json").write_text(json.dumps(manifest, indent=2))
    print(f"[wav] wrote {len(cases_def)} WAVs in {inputs} + {out}/manifest.json")


# ──────────────────────────────────────────────────────────────────────────
# MIDI generator oracles (Phase 1)
# ──────────────────────────────────────────────────────────────────────────

def capture_midi() -> None:
    """Capture bytes SMF générés selon la logique de _generate_midi_temp.

    Reproduit ici la logique du slicer pour pouvoir feeder des fixtures
    déterministes (onsets, sr, n_beats, track_name) sans dépendre de la GUI.
    """
    import mido

    out = GOLDEN / "midi"
    out.mkdir(parents=True, exist_ok=True)

    # (onsets en samples, total_samples, sr, n_beats, track_name, output_filename)
    cases = [
        ([0, 22050, 44100], 88200, 44100, 4, "test_loop", "case_3_slices_4_beats.mid"),
        ([0, 5000, 10000, 15000, 20000], 25000, 22050, 8, "drum_break",
         "case_5_slices_8_beats.mid"),
        ([0], 100000, 44100, 16, "single_slice", "case_1_slice_16_beats.mid"),
        # Edge case : total_samples=0 → BPM fallback 120
        ([0, 1000], 0, 44100, 16, "edge_zero_dur", "case_zero_dur.mid"),
        # Cas A3000 typique : 16 slices de drum break à 120 BPM (16 beats sur 8s)
        ([i * 22050 for i in range(16)], 16 * 22050, 44100, 16, "drumloop16",
         "case_16_slices.mid"),
    ]

    manifest = {"cases": []}
    for onsets, total_samples, sr, n_beats, track_name, fname in cases:
        ppq = 480
        total_dur_sec = total_samples / sr if total_samples > 0 else 0.0
        bpm = (n_beats * 60.0 / total_dur_sec) if total_dur_sec > 0 else 120.0
        tempo = mido.bpm2tempo(bpm)

        def sec_to_ticks(s: float) -> int:
            return int(round(s * bpm / 60 * ppq))

        mid = mido.MidiFile(ticks_per_beat=ppq)
        track = mido.MidiTrack()
        mid.tracks.append(track)
        track.append(mido.MetaMessage("set_tempo", tempo=tempo, time=0))
        track.append(mido.MetaMessage("track_name", name=track_name[:32], time=0))

        base_note = 36
        last_tick = 0
        for i, onset in enumerate(onsets):
            start_sec = float(onset) / sr
            end_sample = onsets[i + 1] if i + 1 < len(onsets) else total_samples
            end_sec = float(end_sample) / sr
            start_tick = sec_to_ticks(start_sec)
            end_tick = sec_to_ticks(end_sec)
            note = min(127, base_note + i)
            delta_on = max(0, start_tick - last_tick)
            delta_off = max(1, end_tick - start_tick)
            track.append(mido.Message("note_on", note=note, velocity=100, time=delta_on))
            track.append(mido.Message("note_off", note=note, velocity=0, time=delta_off))
            last_tick = end_tick

        path = out / fname
        mid.save(str(path))

        manifest["cases"].append({
            "onsets": onsets,
            "total_samples": total_samples,
            "sample_rate": sr,
            "n_beats": n_beats,
            "track_name": track_name,
            "midi_filename": fname,
            "midi_byte_count": path.stat().st_size,
        })

    (out / "manifest.json").write_text(json.dumps(manifest, indent=2))
    print(f"[midi] wrote {len(cases)} MIDI fixtures in {out}")


# ──────────────────────────────────────────────────────────────────────────
# Onset detection oracles (Phase 2)
# ──────────────────────────────────────────────────────────────────────────

def capture_onset() -> None:
    """Capture les sorties librosa pour la pipeline onset detection.

    Pour chaque cas (synthétique + drum loops réels), dumpe :
      - y.f32        : input mono float32 LE (à feed verbatim depuis Rust)
      - stft_mag.f32 : |S| magnitude, shape (n_freq, n_frames), row-major f32 LE
      - mel.f32      : mel power spectrogram, shape (n_mels, n_frames)
      - mel_db.f32   : power_to_db(mel), shape (n_mels, n_frames)
      - onset_env.f32: librosa.onset.onset_strength(y, sr), shape (n_frames,)
      - peaks.json          : peak_pick(env) indices de frames (sans backtrack)
      - backtracked.json    : onset_backtrack(peaks, env) indices de frames
      - onset_detect.json   : onset_detect(..., backtrack=True) indices de frames
      - engine_samples.json : engine.py:detect_transients() indices de samples (final)

    Tous les paramètres sont ceux par défaut au sr=44100 hop=512 (cf. plan).
    """
    import numpy as np
    import soundfile as sf
    import librosa

    from a3000_transfer.slicer.engine import detect_transients

    out = GOLDEN / "onset"
    out.mkdir(parents=True, exist_ok=True)
    inputs = HERE / "inputs" / "wavs_onset"
    inputs.mkdir(parents=True, exist_ok=True)

    # ── Génère un cas synthétique : impulse train avec décroissance exponentielle.
    # 4 transients à 0.0s / 0.25s / 0.5s / 0.75s sur 1.0s à 44100 Hz.
    sr_synth = 44100
    y_synth = np.zeros(sr_synth, dtype=np.float32)
    for start in [0, 11025, 22050, 33075]:
        n = min(2205, len(y_synth) - start)
        decay = np.exp(-np.arange(n) / 1000.0).astype(np.float32)
        y_synth[start:start + n] += decay
    synth_path = inputs / "impulses_4_44100.wav"
    sf.write(str(synth_path), y_synth, sr_synth, subtype="PCM_16")

    # ── Cas réels (drum loops + reese du dossier python/samples).
    repo_samples = ROOT / "python" / "samples"

    cases = [
        ("impulses_4_44100", synth_path),
        ("loop01", repo_samples / "loop01.wav"),
        ("loop02", repo_samples / "loop02.wav"),
        ("loop03", repo_samples / "loop03.wav"),
        ("reese02", repo_samples / "reese02.wav"),
    ]

    n_fft = 2048
    hop_length = 512

    manifest = {
        "params": {
            "n_fft": n_fft,
            "hop_length": hop_length,
            "n_mels": 128,
            "fmin": 0.0,
            "fmax_factor": 0.5,  # fmax = sr * 0.5
            "power": 2.0,
            "win": "hann_periodic",
            "center": True,
            "pad_mode": "constant",
            "mel_norm": "slaney",
            "mel_htk": False,
            "power_to_db": {"ref": 1.0, "amin": 1e-10, "top_db": 80.0},
            "onset_strength": {"lag": 1, "max_size": 1, "aggregate": "mean"},
            "peak_pick_default": "0.03/0.00/0.10/0.10/0.03 sec → frames",
            "delta_default": 0.07,
            "engine": {"sensitivity": 1.0, "min_gap_ms": 30, "snap_ms": 50, "backtrack": True},
        },
        "cases": [],
    }

    for case_name, wav_path in cases:
        # Load + mono float32 (même logique que slicer/engine.py)
        audio, sr = sf.read(str(wav_path), always_2d=False)
        if audio.ndim > 1:
            y = librosa.to_mono(audio.T)
        else:
            y = audio
        y = y.astype(np.float32)

        case_dir = out / case_name
        case_dir.mkdir(parents=True, exist_ok=True)

        # Dump y verbatim
        (case_dir / "y.f32").write_bytes(y.tobytes())

        # Stage 1 : STFT magnitude
        S = librosa.stft(y, n_fft=n_fft, hop_length=hop_length,
                         window="hann", center=True, pad_mode="constant")
        stft_mag = np.abs(S).astype(np.float32)
        (case_dir / "stft_mag.f32").write_bytes(stft_mag.tobytes())

        # Stage 2 : Mel power spectrogram (utilise S² en interne)
        mel = librosa.feature.melspectrogram(
            y=y, sr=sr, n_fft=n_fft, hop_length=hop_length,
            n_mels=128, fmin=0.0, fmax=sr / 2, power=2.0,
            window="hann", center=True, pad_mode="constant",
        ).astype(np.float32)
        (case_dir / "mel.f32").write_bytes(mel.tobytes())

        # Stage 3 : power_to_db
        mel_db = librosa.power_to_db(mel).astype(np.float32)
        (case_dir / "mel_db.f32").write_bytes(mel_db.tobytes())

        # Stage 4 : onset envelope
        env = librosa.onset.onset_strength(
            y=y, sr=sr, hop_length=hop_length, n_fft=n_fft,
        ).astype(np.float32)
        (case_dir / "onset_env.f32").write_bytes(env.tobytes())

        # Stage 5 : peaks (sparse frame indices, paramètres librosa par défaut)
        pre_max = max(1, int(0.03 * sr / hop_length))
        post_max = max(1, int(0.00 * sr / hop_length) + 1)
        pre_avg = max(1, int(0.10 * sr / hop_length))
        post_avg = max(1, int(0.10 * sr / hop_length) + 1)
        wait = max(1, int(0.03 * sr / hop_length))
        delta = 0.07

        # Note : librosa.onset_detect normalize=True par défaut :
        #   env_n = (env - min(env)) / (max(env - min(env)) + tiny)
        # puis peak_pick(env_n, ...).
        from librosa.util import tiny as _ltiny
        env_shifted = env - env.min()
        env_normalized = env_shifted / (env_shifted.max() + _ltiny(env_shifted))
        (case_dir / "onset_env_normalized.f32").write_bytes(
            env_normalized.astype(np.float32).tobytes()
        )
        peaks = librosa.util.peak_pick(
            env_normalized,
            pre_max=pre_max, post_max=post_max,
            pre_avg=pre_avg, post_avg=post_avg,
            delta=delta, wait=wait,
        )
        (case_dir / "peaks.json").write_text(json.dumps(peaks.tolist()))

        # Stage 6 : backtracked (utilise env normalized, comme onset_detect)
        backtracked = librosa.onset.onset_backtrack(peaks, env_normalized)
        (case_dir / "backtracked.json").write_text(json.dumps(backtracked.tolist()))

        # Stage 7 : onset_detect (officiel, backtrack=True comme dans engine.py)
        od_frames = librosa.onset.onset_detect(
            y=y, sr=sr, hop_length=hop_length,
            backtrack=True, delta=delta, units="frames",
        )
        (case_dir / "onset_detect.json").write_text(json.dumps(od_frames.tolist()))

        # Stage 8 : engine.detect_transients (final API publique)
        engine_samples = detect_transients(y, sr, sensitivity=1.0,
                                           min_gap_ms=30, snap_ms=50,
                                           hop_length=hop_length, backtrack=True)
        (case_dir / "engine_samples.json").write_text(
            json.dumps(engine_samples.tolist())
        )

        manifest["cases"].append({
            "name": case_name,
            "input_wav": str(wav_path.relative_to(ROOT)).replace("\\", "/"),
            "sample_rate": int(sr),
            "n_samples": int(len(y)),
            "y_byte_count": len(y) * 4,
            "stft_mag_shape": list(stft_mag.shape),
            "mel_shape": list(mel.shape),
            "mel_db_shape": list(mel_db.shape),
            "onset_env_len": int(len(env)),
            "peaks_count": int(len(peaks)),
            "backtracked_count": int(len(backtracked)),
            "onset_detect_count": int(len(od_frames)),
            "engine_samples_count": int(len(engine_samples)),
            "peak_pick_params": {
                "pre_max": pre_max, "post_max": post_max,
                "pre_avg": pre_avg, "post_avg": post_avg,
                "delta": delta, "wait": wait,
            },
        })

        print(f"[onset] {case_name}: {len(y)} samples ; "
              f"env len={len(env)}, peaks={len(peaks)}, "
              f"backtracked={len(backtracked)}, engine_samples={len(engine_samples)}")

    (out / "manifest.json").write_text(json.dumps(manifest, indent=2))
    print(f"[onset] wrote manifest + {len(cases)} fixture dirs to {out}")


# ──────────────────────────────────────────────────────────────────────────
# Main
# ──────────────────────────────────────────────────────────────────────────

def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--smdi", action="store_true")
    parser.add_argument("--wav", action="store_true")
    parser.add_argument("--midi", action="store_true")
    parser.add_argument("--onset", action="store_true")
    args = parser.parse_args()

    # Si aucun flag, on capture tout
    do_all = not (args.smdi or args.wav or args.midi or args.onset)

    if do_all or args.smdi:
        capture_smdi()
    if do_all or args.wav:
        capture_wav()
    if do_all or args.midi:
        capture_midi()
    if do_all or args.onset:
        capture_onset()

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
