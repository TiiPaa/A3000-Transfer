"""Worker process : tourne en admin (élevé via UAC par la GUI non-admin).

Communique avec la GUI via une socket TCP localhost (la GUI bind un port,
lance le worker en lui passant le port, le worker s'y connecte). Pas de pipe
stdin/stdout parce qu'UIPI bloque le redirect cross-privilege ; la socket
localhost est cross-privilege OK.

Protocol — lignes JSON :

  GUI → Worker (commandes) :
    {"cmd": "find_free_slot", "ha": 1, "bus": 0, "target": 0, "lun": 0,
     "start": 7}
    {"cmd": "transfer", "wave_path": "C:/...", "sample_number": 100,
     "name": "...", "ha": 1, "bus": 0, "target": 0, "lun": 0}
    {"cmd": "exit"}

  Worker → GUI (événements) :
    {"event": "ready"}
    {"event": "free_slot", "slot": 100}
    {"event": "progress", "sent": 12345, "total": 67890}
    {"event": "done", "sample_number": 100, "bytes_sent": ..., "packet_count": ...}
    {"event": "error", "msg": "..."}
"""
from __future__ import annotations

import argparse
import datetime
import json
import os
import socket
import sys
import tempfile
import traceback

from .scsi_passthrough import close_handle, open_adapter
from .smdi import (
    MSG_REJECT,
    MSG_SAMPLE_HEADER,
    SmdiNoReplyPendingError,
    SmdiProtocolError,
    decode_message_reject,
    decode_sample_header,
    encode_sample_header_request,
    find_first_free_sample_number,
    receive_smdi,
    send_smdi,
)
from .transfer import SampleTransferError, receive_sample, save_smdi_to_wav, transfer_sample
from .wav_reader import WaveValidationError, load_wave


_LOG_PATH = os.path.join(tempfile.gettempdir(), "a3000_worker.log")


def _log(msg: str) -> None:
    """Log à un fichier (le stderr n'est pas redirigé via ShellExecuteW UAC)."""
    try:
        with open(_LOG_PATH, "a", encoding="utf-8") as f:
            ts = datetime.datetime.now().isoformat(timespec="milliseconds")
            f.write(f"[{ts}] {msg}\n")
    except Exception:
        pass


def _send(fp, obj: dict) -> None:
    fp.write(json.dumps(obj).encode("utf-8") + b"\n")
    fp.flush()


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--port", type=int, required=True)
    parser.add_argument("--host", default="127.0.0.1")
    args = parser.parse_args()

    _log(f"=== Worker démarre, connect to {args.host}:{args.port} (PID={os.getpid()}) ===")

    try:
        sock = socket.create_connection((args.host, args.port), timeout=10)
    except OSError as exc:
        _log(f"FATAL: connect failed: {exc}")
        return 1

    # IMPORTANT : retirer le timeout pour que readline() bloque indéfiniment
    # entre deux commandes. Sinon le worker crash après 10 s d'inactivité.
    sock.settimeout(None)

    fp = sock.makefile("rwb", buffering=0)
    _send(fp, {"event": "ready"})
    _log("Connecté à la GUI, ready envoyé.")

    handle: int | None = None
    current_ha: int | None = None

    try:
        while True:
            line = fp.readline()
            if not line:
                _log("readline a retourné vide → la GUI a fermé la connexion.")
                break
            try:
                cmd = json.loads(line)
            except json.JSONDecodeError as exc:
                _send(fp, {"event": "error", "msg": f"bad JSON: {exc}"})
                continue

            kind = cmd.get("cmd")
            _log(f"Commande reçue: {kind}")
            if kind == "exit":
                _log("Exit demandé par la GUI.")
                break

            try:
                ha = int(cmd.get("ha", 1))
                if handle is None or current_ha != ha:
                    if handle is not None:
                        close_handle(handle)
                    handle = open_adapter(ha)
                    current_ha = ha

                common = dict(
                    path_id=int(cmd.get("bus", 0)),
                    target_id=int(cmd.get("target", 0)),
                    lun=int(cmd.get("lun", 0)),
                )

                if kind == "find_free_slot":
                    slot = find_first_free_sample_number(
                        handle, start=int(cmd.get("start", 7)), **common,
                    )
                    _send(fp, {"event": "free_slot", "slot": slot})

                elif kind == "list_samples":
                    start = int(cmd.get("start", 0))
                    limit = int(cmd.get("limit", 128))
                    samples = []
                    for n in range(start, start + limit):
                        try:
                            send_smdi(handle, message=encode_sample_header_request(n), **common)
                            _, reply = receive_smdi(handle, allocation_length=4096, **common)
                        except SmdiNoReplyPendingError:
                            continue  # slot vide
                        except SmdiProtocolError as exc:
                            _log(f"list_samples: erreur slot {n}: {exc}")
                            continue
                        if reply.code == MSG_REJECT:
                            try:
                                rej_code, rej_sub = decode_message_reject(reply)
                            except SmdiProtocolError:
                                continue
                            if (rej_code, rej_sub) == (0x0020, 0x0000):
                                break  # out of range, on s'arrête
                            continue  # autres rejets = slot vide ou autre, skip
                        if reply.code == MSG_SAMPLE_HEADER:
                            try:
                                hdr = decode_sample_header(reply)
                            except SmdiProtocolError:
                                continue
                            sr = (int(round(1_000_000_000 / hdr.sample_period_ns))
                                  if hdr.sample_period_ns else 0)
                            duration = hdr.sample_length_words / sr if sr else 0
                            samples.append({
                                "slot": n,
                                "name": hdr.name,
                                "channels": hdr.channels,
                                "bits": hdr.bits_per_word,
                                "sample_rate": sr,
                                "frames": hdr.sample_length_words,
                                "duration": duration,
                            })
                            # Reporter la progression du scan
                            _send(fp, {"event": "scan_progress", "scanned": n - start + 1, "found": len(samples)})
                    _send(fp, {"event": "samples_list", "samples": samples})

                elif kind == "receive":
                    sample_number = int(cmd["sample_number"])
                    output_path = cmd["output_path"]

                    def progress(packet_count: int, sent: int, total: int) -> None:
                        _send(fp, {"event": "progress", "sent": sent, "total": total})

                    header, data = receive_sample(
                        handle,
                        sample_number=sample_number,
                        progress=progress,
                        **common,
                    )
                    save_smdi_to_wav(output_path, header, data)
                    sr = int(round(1_000_000_000 / header.sample_period_ns)) if header.sample_period_ns else 0
                    _send(fp, {
                        "event": "received",
                        "sample_number": sample_number,
                        "output_path": output_path,
                        "name": header.name,
                        "channels": header.channels,
                        "bits_per_word": header.bits_per_word,
                        "frames": header.sample_length_words,
                        "sample_rate": sr,
                        "bytes_received": len(data),
                    })

                elif kind == "transfer":
                    wave_path = cmd["wave_path"]
                    sample_number = int(cmd["sample_number"])
                    name = cmd.get("name", "")
                    wave = load_wave(wave_path)

                    def progress(packet_count: int, sent: int, total: int) -> None:
                        _send(fp, {"event": "progress", "sent": sent, "total": total})

                    try:
                        stats = transfer_sample(
                            handle,
                            sample_number=sample_number,
                            wave=wave,
                            name=name,
                            progress=progress,
                            **common,
                        )
                    except SmdiNoReplyPendingError:
                        # Pendant un write, "no reply pending" = sampler refuse l'écriture.
                        # Cause typique sur A3000 : Bulk Protect activé.
                        _send(fp, {
                            "event": "error",
                            "msg": (
                                "BULK_PROTECT:Le sampler refuse l'écriture (no reply pending). "
                                "Probablement Bulk Protect activé sur l'A3000. "
                                "Désactive-le sur le sampler (UTILITY → MIDI/SAMPLE → "
                                "BulkProtect = off), puis réessaie."
                            ),
                        })
                    else:
                        _send(fp, {
                            "event": "done",
                            "sample_number": stats.sample_number,
                            "bytes_sent": stats.bytes_sent,
                            "packet_count": stats.packet_count,
                        })

                else:
                    _send(fp, {"event": "error", "msg": f"unknown cmd: {kind!r}"})

            except (WaveValidationError, SampleTransferError, OSError) as exc:
                _log(f"Exception attendue: {type(exc).__name__}: {exc}")
                _send(fp, {"event": "error", "msg": str(exc)})
            except Exception as exc:
                tb = traceback.format_exc()
                _log(f"Exception INATTENDUE: {exc!r}\n{tb}")
                _send(fp, {
                    "event": "error",
                    "msg": f"worker exception: {exc!r}",
                    "traceback": tb,
                })

    except Exception as outer_exc:
        _log(f"Exception FATALE dans le main loop: {outer_exc!r}\n{traceback.format_exc()}")

    finally:
        _log("Cleanup worker...")
        if handle is not None:
            try:
                close_handle(handle)
            except Exception:
                pass
        try:
            sock.close()
        except Exception:
            pass
        _log("=== Worker terminé ===")

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
