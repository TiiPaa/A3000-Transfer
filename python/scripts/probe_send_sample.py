r"""
Test du transfert SMDI master→slave.

PAR DÉFAUT --dry-run : envoie Sample Header, lit BSTA, envoie Abort Procedure.
Aucune écriture mémoire côté sampler — valide le 2e handshake.

Pour le vrai transfert, ajouter --commit :
    python scripts\probe_send_sample.py --wave samples\loop01.wav --sample 100 --commit

Cible par défaut : HA1/BUS0/ID0/LUN0.
Note : sur A3000, les slots 0-6 sont des samples factory ROM ; choisir un slot
plus haut (ex. 100, 300) pour les transferts.
"""
from __future__ import annotations

import argparse
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from a3000_transfer.scsi_passthrough import close_handle, open_adapter  # noqa: E402
from a3000_transfer.transfer import SampleTransferError, transfer_sample  # noqa: E402
from a3000_transfer.wav_reader import WaveValidationError, load_wave  # noqa: E402


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--wave", default="samples/loop01.wav")
    parser.add_argument("--sample", type=int, default=100, help="Slot sample dans le sampler")
    parser.add_argument("--name", default="loop01")
    parser.add_argument("--ha", type=int, default=1)
    parser.add_argument("--bus", type=int, default=0)
    parser.add_argument("--target", type=int, default=0)
    parser.add_argument("--lun", type=int, default=0)
    parser.add_argument("--packet-length", type=int, default=4096)
    parser.add_argument("--commit", action="store_true",
                        help="Effectue le vrai transfert (sinon dry-run + Abort).")
    parser.add_argument("--verbose", action="store_true",
                        help="Logs détaillés de chaque échange SCSI/SMDI.")
    args = parser.parse_args()

    try:
        wave = load_wave(args.wave)
    except WaveValidationError as exc:
        print(f"WAV invalide: {exc}", file=sys.stderr)
        return 1

    print(f"Wave: {args.wave}  channels={wave.channels} sr={wave.sample_rate} "
          f"bits={wave.bits_per_sample} frames={wave.frame_count} bytes={wave.byte_count}")
    print(f"Mode: {'COMMIT (transfert réel)' if args.commit else 'DRY-RUN (Abort après BSTA)'}")
    print(f"Cible: HA{args.ha} BUS{args.bus} ID{args.target} LUN{args.lun}, slot #{args.sample}, name={args.name!r}")

    def progress(packet_count: int, sent: int, total: int) -> None:
        pct = (sent / total * 100) if total else 0
        if packet_count % 10 == 0 or sent == total:
            print(f"  packet #{packet_count}  {sent}/{total} octets  ({pct:.1f}%)")

    handle = open_adapter(args.ha)
    try:
        stats = transfer_sample(
            handle,
            path_id=args.bus,
            target_id=args.target,
            lun=args.lun,
            sample_number=args.sample,
            wave=wave,
            name=args.name,
            preferred_packet_length=args.packet_length,
            progress=progress,
            dry_run=not args.commit,
            verbose=args.verbose,
        )
    except SampleTransferError as exc:
        print(f"Transfert échoué: {exc}", file=sys.stderr)
        return 2
    finally:
        close_handle(handle)

    print()
    print("OK")
    print(f"  sample_number = {stats.sample_number}")
    print(f"  packet_length négocié = {stats.packet_length}")
    print(f"  packets envoyés       = {stats.packet_count}")
    print(f"  octets envoyés        = {stats.bytes_sent}")
    if not args.commit:
        print("  (dry-run — Abort Procedure envoyé après BSTA, slot non écrit)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
