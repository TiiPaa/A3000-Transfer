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
    out = GOLDEN / "wav"
    out.mkdir(parents=True, exist_ok=True)
    inputs = HERE / "inputs" / "wavs"
    inputs.mkdir(parents=True, exist_ok=True)

    # TODO Phase 1 : générer WAVs de test (mono/stéréo, 8/16/24/32-bit, float)
    # avec contenus déterministes (sine + impulse), puis appeler peek_wave_metadata
    # et load_wave (avec random.seed(42) avant) et dump bytes PCM attendus.
    print(f"[wav] TODO — placeholder. Inputs WAVs à générer dans {inputs}")


# ──────────────────────────────────────────────────────────────────────────
# MIDI generator oracles (Phase 1)
# ──────────────────────────────────────────────────────────────────────────

def capture_midi() -> None:
    out = GOLDEN / "midi"
    out.mkdir(parents=True, exist_ok=True)
    # TODO Phase 1 : pour différents (onsets, sr, n_beats), appeler la logique
    # de _generate_midi_temp et dump les bytes SMF attendus
    print(f"[midi] TODO — placeholder. Capturer bytes SMF dans {out}")


# ──────────────────────────────────────────────────────────────────────────
# Onset detection oracles (Phase 2)
# ──────────────────────────────────────────────────────────────────────────

def capture_onset() -> None:
    out = GOLDEN / "onset"
    out.mkdir(parents=True, exist_ok=True)
    # TODO Phase 2 : sur un set de WAVs de test (drum loops, samples vocaux),
    # appeler detect_transients et dump les listes d'onset indices
    print(f"[onset] TODO — placeholder. Lancer detect_transients sur N WAVs de test")


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
