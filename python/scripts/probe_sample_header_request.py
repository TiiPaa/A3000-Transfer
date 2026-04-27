r"""
Test : envoie un SMDI Sample Header Request (0x0120) pour interroger un slot
sample du Yamaha A3000 sans rien écrire.

A lancer en Administrateur :
    python scripts\probe_sample_header_request.py
    python scripts\probe_sample_header_request.py --sample 5

Trois issues possibles :
  - Sample Header (0x0121/0x0000) : le slot contient un sample, on parse et affiche
  - Message Reject (0x0002/...)   : code 0x0020/0x0002 = "no sample at this number" (slot vide, OK)
  - Autre                          : on dump brut

Aucun écrit côté sampler.
"""
from __future__ import annotations

import argparse
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from a3000_transfer.scsi_passthrough import close_handle, open_adapter  # noqa: E402
from a3000_transfer.smdi import (  # noqa: E402
    MSG_REJECT,
    MSG_SAMPLE_HEADER,
    REJECT_SAMPLE,
    SmdiProtocolError,
    decode_message_reject,
    decode_sample_header,
    drain_pending_reply,
    encode_sample_header_request,
    receive_smdi,
    send_smdi,
)


SAMPLE_REJECTION_REASONS = {
    0x0000: "Sample Number is out of range",
    0x0002: "no sample at this Sample Number",
    0x0004: "insufficient sample memory available",
    0x0005: "insufficient param memory available",
    0x0006: "can't accommodate Sample Format - Bits Per Word",
    0x0007: "can't accommodate Sample Format - Number Of Channels",
}


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--ha", type=int, default=1)
    parser.add_argument("--bus", type=int, default=0)
    parser.add_argument("--target", type=int, default=0)
    parser.add_argument("--lun", type=int, default=0)
    parser.add_argument("--sample", type=int, default=0, help="Sample number à interroger (0 par défaut)")
    args = parser.parse_args()

    handle = open_adapter(args.ha)
    try:
        # Drain défensif : si une session précédente a laissé une réponse pending
        drained = drain_pending_reply(handle, path_id=args.bus, target_id=args.target, lun=args.lun)
        if drained is not None:
            print(f"(drain: réponse pending consommée code=0x{drained.message_id:04X}/0x{drained.sub_id:04X})")

        request = encode_sample_header_request(args.sample)
        print(f"--> SEND Sample Header Request sample#{args.sample} ({len(request)} octets)")
        print(f"    {request.hex(' ')}")

        send_result = send_smdi(
            handle,
            path_id=args.bus,
            target_id=args.target,
            lun=args.lun,
            message=request,
        )
        if send_result.scsi_status != 0:
            print(f"    SEND ScsiStatus=0x{send_result.scsi_status:02X}, sense={send_result.sense[:18].hex(' ')}")
            return 1
        print(f"    SEND OK ({send_result.transferred} octets transférés)")

        print("<-- RECEIVE")
        recv_result, reply = receive_smdi(
            handle,
            path_id=args.bus,
            target_id=args.target,
            lun=args.lun,
            allocation_length=4096,
        )
        print(f"    {recv_result.transferred} octets reçus, code=0x{reply.message_id:04X}/0x{reply.sub_id:04X}, payload={len(reply.payload)} octets")
        print(f"    Raw payload hex: {reply.payload.hex(' ')}")
    except SmdiProtocolError as exc:
        print(f"Erreur protocole SMDI: {exc}")
        return 4
    finally:
        close_handle(handle)

    if reply.code == MSG_SAMPLE_HEADER:
        header = decode_sample_header(reply)
        print()
        print(f"Sample Header reçu pour slot #{header.sample_number} :")
        print(f"  bits/word     : {header.bits_per_word}")
        print(f"  channels      : {header.channels}")
        print(f"  period (ns)   : {header.sample_period_ns}  (sample rate {header.sample_rate_hz:.0f} Hz)")
        print(f"  length (words): {header.sample_length_words}")
        print(f"  loop start    : {header.loop_start}")
        print(f"  loop end      : {header.loop_end}")
        print(f"  loop control  : 0x{header.loop_control:02X}")
        print(f"  pitch         : 0x{header.pitch_integer:04X}.{header.pitch_fraction:04X}")
        print(f"  name          : {header.name!r}")
        return 0

    if reply.code == MSG_REJECT:
        rej_code, rej_sub = decode_message_reject(reply)
        reason = ""
        if rej_code == REJECT_SAMPLE:
            reason = " — " + SAMPLE_REJECTION_REASONS.get(rej_sub, "(sub-code inconnu)")
        elif rej_code == 0x0002:
            reason = " — Last message rejected (general)"
        elif rej_code == 0x0005:
            reason = " — Device busy"
        print(f"Message Reject: code=0x{rej_code:04X} sub=0x{rej_sub:04X}{reason}")
        if rej_code == REJECT_SAMPLE and rej_sub == 0x0002:
            print("OK Slot vide, c'est attendu si tu n'as pas chargé de sample dans ce slot.")
            return 0
        return 2

    print(f"Réponse inattendue: id=0x{reply.message_id:04X}/0x{reply.sub_id:04X}")
    print(f"Raw: {reply.payload.hex(' ')}")
    return 3


if __name__ == "__main__":
    sys.exit(main())
