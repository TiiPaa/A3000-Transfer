r"""
Test du handshake SMDI : envoie Master Identify (0001/0000) et lit la réponse,
qui doit être Slave Identify (0001/0001).

A lancer en Administrateur :
    python scripts\probe_smdi_identify.py
    python scripts\probe_smdi_identify.py --ha 1 --bus 0 --target 0 --lun 0

Si le sampler ne répond pas correctement, vérifie dans son menu UTILITY → MIDI/SCSI
que SMDI est bien activé.
"""
from __future__ import annotations

import argparse
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from a3000_transfer.scsi_passthrough import close_handle, open_adapter  # noqa: E402
from a3000_transfer.smdi import (  # noqa: E402
    MSG_SLAVE_IDENTIFY,
    SmdiProtocolError,
    master_identify_message,
    receive_smdi,
    send_smdi,
)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--ha", type=int, default=1)
    parser.add_argument("--bus", type=int, default=0)
    parser.add_argument("--target", type=int, default=0)
    parser.add_argument("--lun", type=int, default=0)
    args = parser.parse_args()

    handle = open_adapter(args.ha)
    try:
        msg = master_identify_message()
        print(f"--> SEND Master Identify ({len(msg)} octets): {msg.hex(' ')}")

        send_result = send_smdi(
            handle,
            path_id=args.bus,
            target_id=args.target,
            lun=args.lun,
            message=msg,
        )
        print(f"    SEND ScsiStatus=0x{send_result.scsi_status:02X}  transferred={send_result.transferred}")
        if send_result.scsi_status != 0:
            print(f"    Sense: {send_result.sense[:18].hex(' ')}")
            print("--> SEND a échoué, on n'essaie pas le RECEIVE.")
            return 1

        print("<-- RECEIVE (allocation 4096)")
        recv_result, reply = receive_smdi(
            handle,
            path_id=args.bus,
            target_id=args.target,
            lun=args.lun,
            allocation_length=4096,
        )
        print(f"    ScsiStatus=0x{recv_result.scsi_status:02X}  transferred={recv_result.transferred}")
        print(f"    Raw: {recv_result.data[:32].hex(' ')}{'...' if len(recv_result.data) > 32 else ''}")
        print(f"    Decoded: id=0x{reply.message_id:04X} sub=0x{reply.sub_id:04X} payload={len(reply.payload)} octets")

        if reply.code == MSG_SLAVE_IDENTIFY:
            print("OK Slave Identify reçu — le sampler parle SMDI.")
            return 0

        if reply.code == (0x0002, 0x0000):
            rej_code = int.from_bytes(reply.payload[:2], "big") if len(reply.payload) >= 2 else 0
            rej_sub = int.from_bytes(reply.payload[2:4], "big") if len(reply.payload) >= 4 else 0
            print(f"NOK Slave a renvoyé Message Reject: code=0x{rej_code:04X} sub=0x{rej_sub:04X}")
            return 2

        print(f"NOK Réponse inattendue (code 0x{reply.message_id:04X}/0x{reply.sub_id:04X}).")
        return 3

    except SmdiProtocolError as exc:
        print(f"Erreur protocole SMDI: {exc}")
        return 4
    finally:
        close_handle(handle)


if __name__ == "__main__":
    sys.exit(main())
