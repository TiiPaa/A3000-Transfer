"""CLI principal : a3000-transfer scan / identify / list / send / delete."""
from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

from .scsi_passthrough import close_handle, open_adapter
from .smdi import (
    MSG_REJECT,
    MSG_SAMPLE_HEADER,
    MSG_SLAVE_IDENTIFY,
    SmdiNoReplyPendingError,
    SmdiProtocolError,
    decode_message_reject,
    decode_sample_header,
    encode_sample_header_request,
    find_first_free_sample_number,
    master_identify_message,
    receive_smdi,
    send_smdi,
)
from .spti import scan_scsi_targets
from .transfer import SampleTransferError, transfer_sample
from .wav_reader import WaveValidationError, load_wave


def _build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="a3000-transfer")
    sub = parser.add_subparsers(dest="command", required=True)

    p_scan = sub.add_parser("scan", help="Liste les adaptateurs et devices SCSI vus par Windows")
    p_scan.add_argument("--max-adapters", type=int, default=16)
    p_scan.add_argument("--json", action="store_true")

    p_id = sub.add_parser("identify", help="Vérifie qu'un sampler répond au handshake SMDI")
    _add_target_args(p_id)

    p_list = sub.add_parser("list", help="Liste les samples présents dans le sampler")
    _add_target_args(p_list)
    p_list.add_argument("--start", type=int, default=0, help="Premier slot à interroger")
    p_list.add_argument("--limit", type=int, default=64, help="Nombre max de slots à scanner")

    p_send = sub.add_parser("send", help="Transfère un ou plusieurs WAV vers le sampler")
    _add_target_args(p_send)
    p_send.add_argument("waves", nargs="+", help="Fichiers WAV à transférer (8/16/24 bits supportés)")
    p_send.add_argument("--start-slot", type=int, default=None,
                        help="Slot de départ (auto-find si non spécifié)")
    p_send.add_argument("--name", default=None,
                        help="Nom du sample (par défaut : nom du fichier sans .wav)")
    p_send.add_argument("--packet-length", type=int, default=4096)
    p_send.add_argument("--verbose", action="store_true")
    p_send.add_argument("--dry-run", action="store_true",
                        help="Valide jusqu'au BSTA mais n'écrit pas en mémoire")

    p_recv = sub.add_parser("receive", help="Télécharge un sample du sampler vers un WAV")
    _add_target_args(p_recv)
    p_recv.add_argument("--sample", type=int, required=True)
    p_recv.add_argument("--output", required=True, help="Fichier WAV de sortie")
    p_recv.add_argument("--packet-length", type=int, default=4096)
    p_recv.add_argument("--verbose", action="store_true")

    p_del = sub.add_parser("delete", help="Supprime un sample (souvent silencieusement ignoré sur A3000)")
    _add_target_args(p_del)
    p_del.add_argument("--sample", type=int, required=True)

    sub.add_parser("gui", help="Lance la fenêtre tkinter avec drag'n'drop")

    return parser


def _add_target_args(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("--ha", type=int, default=1, help="Host adapter (default 1)")
    parser.add_argument("--bus", type=int, default=0, help="PathId / bus SCSI")
    parser.add_argument("--target", type=int, default=0, help="Target ID SCSI")
    parser.add_argument("--lun", type=int, default=0, help="LUN")


def main(argv: list[str] | None = None) -> int:
    parser = _build_parser()
    args = parser.parse_args(argv)

    try:
        if args.command == "scan":
            return _cmd_scan(args)
        if args.command == "identify":
            return _cmd_identify(args)
        if args.command == "list":
            return _cmd_list(args)
        if args.command == "send":
            return _cmd_send(args)
        if args.command == "receive":
            return _cmd_receive(args)
        if args.command == "delete":
            return _cmd_delete(args)
        if args.command == "gui":
            return _cmd_gui(args)
        parser.error(f"commande inconnue: {args.command}")
        return 2
    except (PermissionError, OSError) as exc:
        print(f"Erreur système: {exc}", file=sys.stderr)
        return 1
    except WaveValidationError as exc:
        print(f"WAV invalide: {exc}", file=sys.stderr)
        return 1
    except (SmdiProtocolError, SampleTransferError) as exc:
        print(f"Erreur SMDI: {exc}", file=sys.stderr)
        return 2


def _cmd_scan(args) -> int:
    targets = scan_scsi_targets(max_adapters=args.max_adapters)
    if args.json:
        print(json.dumps([t.to_dict() for t in targets], indent=2, ensure_ascii=False))
        return 0
    if not targets:
        print("Aucune cible SCSI détectée.")
        return 0
    for t in targets:
        print(t.display_name)
    return 0


def _cmd_identify(args) -> int:
    common = dict(path_id=args.bus, target_id=args.target, lun=args.lun)
    handle = open_adapter(args.ha)
    try:
        send_smdi(handle, message=master_identify_message(), **common)
        _, reply = receive_smdi(handle, allocation_length=64, **common)
    finally:
        close_handle(handle)
    if reply.code == MSG_SLAVE_IDENTIFY:
        print(f"OK Slave Identify reçu (HA{args.ha} BUS{args.bus} ID{args.target} LUN{args.lun})")
        return 0
    print(f"Réponse inattendue: 0x{reply.message_id:04X}/0x{reply.sub_id:04X}", file=sys.stderr)
    return 2


def _cmd_list(args) -> int:
    common = dict(path_id=args.bus, target_id=args.target, lun=args.lun)
    handle = open_adapter(args.ha)
    try:
        for n in range(args.start, args.start + args.limit):
            send_smdi(handle, message=encode_sample_header_request(n), **common)
            try:
                _, reply = receive_smdi(handle, allocation_length=4096, **common)
            except SmdiNoReplyPendingError:
                print(f"  #{n:>4}: <vide>")
                continue
            if reply.code == MSG_REJECT:
                rej_code, rej_sub = decode_message_reject(reply)
                if (rej_code, rej_sub) == (0x0020, 0x0000):
                    print(f"  #{n:>4}: <out of range>")
                    break
                if (rej_code, rej_sub) == (0x0020, 0x0002):
                    print(f"  #{n:>4}: <vide>")
                    continue
                print(f"  #{n:>4}: rejet 0x{rej_code:04X}/0x{rej_sub:04X}")
                continue
            if reply.code == MSG_SAMPLE_HEADER:
                hdr = decode_sample_header(reply)
                ch = "stéréo" if hdr.channels == 2 else "mono"
                rate = hdr.sample_rate_hz
                duration_s = hdr.sample_length_words / rate if rate else 0
                print(f"  #{n:>4}: {hdr.name!r:<20} "
                      f"{hdr.bits_per_word}b {ch} {rate:.0f}Hz "
                      f"{hdr.sample_length_words}w ({duration_s:.2f}s)")
            else:
                print(f"  #{n:>4}: réponse 0x{reply.message_id:04X}/0x{reply.sub_id:04X}")
    finally:
        close_handle(handle)
    return 0


def _cmd_send(args) -> int:
    waves = [Path(p) for p in args.waves]
    for w in waves:
        if not w.exists():
            print(f"WAV introuvable: {w}", file=sys.stderr)
            return 1

    common = dict(path_id=args.bus, target_id=args.target, lun=args.lun)
    handle = open_adapter(args.ha)
    try:
        if args.start_slot is None:
            slot = find_first_free_sample_number(handle, **common)
            print(f"Auto-find: 1er slot libre = #{slot}")
        else:
            slot = args.start_slot

        for wave_path in waves:
            wave = load_wave(wave_path)
            sample_name = args.name if (args.name and len(waves) == 1) else wave_path.stem

            print(f"\n→ {wave_path.name}  ({wave.channels}ch {wave.sample_rate}Hz "
                  f"{wave.frame_count} frames)  → slot #{slot} '{sample_name}'")

            def progress(packet_count: int, sent: int, total: int) -> None:
                pct = (sent / total * 100) if total else 0
                if packet_count % 20 == 0 or sent == total:
                    print(f"  packet #{packet_count:>4}  {sent:>8}/{total} ({pct:5.1f}%)")

            stats = transfer_sample(
                handle,
                sample_number=slot,
                wave=wave,
                name=sample_name,
                preferred_packet_length=args.packet_length,
                progress=progress,
                dry_run=args.dry_run,
                verbose=args.verbose,
                **common,
            )
            print(f"  ✓ {stats.bytes_sent} octets en {stats.packet_count} packets")
            slot += 1
    finally:
        close_handle(handle)
    return 0


def _cmd_gui(args) -> int:
    from .gui import main as gui_main
    return gui_main()


def _cmd_receive(args) -> int:
    from .transfer import receive_sample, save_smdi_to_wav
    common = dict(path_id=args.bus, target_id=args.target, lun=args.lun)
    handle = open_adapter(args.ha)
    try:
        def progress(packet_count: int, sent: int, total: int) -> None:
            pct = (sent / total * 100) if total else 0
            if packet_count % 20 == 0 or sent == total:
                print(f"  packet #{packet_count:>4}  {sent:>8}/{total} ({pct:5.1f}%)")

        print(f"→ Téléchargement sample #{args.sample} → {args.output}")
        header, data = receive_sample(
            handle,
            sample_number=args.sample,
            preferred_packet_length=args.packet_length,
            progress=progress,
            verbose=args.verbose,
            **common,
        )
        save_smdi_to_wav(args.output, header, data)
        sr = int(round(1_000_000_000 / header.sample_period_ns)) if header.sample_period_ns else 0
        print(f"OK '{header.name}' {header.channels}ch {sr}Hz {header.sample_length_words} frames "
              f"→ {len(data)} octets écrits dans {args.output}")
    finally:
        close_handle(handle)
    return 0


def _cmd_delete(args) -> int:
    from .smdi import encode_delete_sample, drain_pending_reply
    common = dict(path_id=args.bus, target_id=args.target, lun=args.lun)
    handle = open_adapter(args.ha)
    try:
        drain_pending_reply(handle, **common)
        send_smdi(handle, message=encode_delete_sample(args.sample), **common)
        try:
            _, reply = receive_smdi(handle, allocation_length=64, **common)
        except SmdiNoReplyPendingError:
            print(f"Sample #{args.sample}: pas de réponse (probablement déjà vide)")
            return 0
        print(f"Sample #{args.sample}: réponse 0x{reply.message_id:04X}/0x{reply.sub_id:04X}")
    finally:
        close_handle(handle)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
