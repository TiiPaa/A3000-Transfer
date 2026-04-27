r"""
Supprime un sample du Yamaha A3000 via SMDI Delete Sample From Memory (0x0124).

Usage (en admin) :
    python scripts\probe_delete_sample.py --sample 7

ATTENTION : suppression définitive côté sampler. Les slots ROM (0-6 sur A3000)
seront refusés par le slave avec un Reject.
"""
from __future__ import annotations

import argparse
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from a3000_transfer.scsi_passthrough import close_handle, open_adapter  # noqa: E402
from a3000_transfer.smdi import (  # noqa: E402
    MSG_END_OF_PROCEDURE,
    MSG_REJECT,
    MSG_WAIT,
    SmdiProtocolError,
    decode_message_reject,
    drain_pending_reply,
    encode_delete_sample,
    receive_smdi,
    send_smdi,
)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--ha", type=int, default=1)
    parser.add_argument("--bus", type=int, default=0)
    parser.add_argument("--target", type=int, default=0)
    parser.add_argument("--lun", type=int, default=0)
    parser.add_argument("--sample", type=int, required=True)
    args = parser.parse_args()

    common = dict(path_id=args.bus, target_id=args.target, lun=args.lun)

    handle = open_adapter(args.ha)
    try:
        drained = drain_pending_reply(handle, **common)
        if drained is not None:
            print(f"(drain: réponse pending consommée code=0x{drained.message_id:04X}/0x{drained.sub_id:04X})")

        msg = encode_delete_sample(args.sample)
        print(f"--> SEND Delete Sample #{args.sample} ({len(msg)} octets) {msg.hex(' ')}")
        send_result = send_smdi(handle, message=msg, **common)
        if send_result.scsi_status != 0:
            print(f"SEND échoué: sense={send_result.sense[:18].hex(' ')}")
            return 1

        # Lire la réponse, en gérant un Wait intermédiaire (spec p32)
        for _ in range(3):
            _, reply = receive_smdi(handle, allocation_length=64, **common)
            if reply.code == MSG_WAIT:
                print("    Wait reçu, on relit la réponse réelle...")
                continue
            break

        if reply.code == MSG_END_OF_PROCEDURE:
            print(f"OK Sample #{args.sample} supprimé.")
            return 0

        if reply.code == MSG_REJECT:
            rej_code, rej_sub = decode_message_reject(reply)
            print(f"Rejet: 0x{rej_code:04X}/0x{rej_sub:04X}")
            if rej_code == 0x0020 and rej_sub == 0x0000:
                print("  → Sample Number out of range")
            elif rej_code == 0x0020 and rej_sub == 0x0002:
                print("  → no sample at this Sample Number (déjà vide)")
            return 2

        print(f"Réponse inattendue: 0x{reply.message_id:04X}/0x{reply.sub_id:04X}")
        return 3

    except SmdiProtocolError as exc:
        print(f"Erreur protocole SMDI: {exc}")
        return 4
    finally:
        close_handle(handle)


if __name__ == "__main__":
    sys.exit(main())
