r"""
Diagnostic: envoie une commande SCSI INQUIRY (CDB 0x12) directement à la cible
via IOCTL_SCSI_PASS_THROUGH_DIRECT et affiche vendor/product/revision.

A lancer en Administrateur :
    python scripts\probe_inquiry.py
    python scripts\probe_inquiry.py --ha 1 --bus 0 --target 0 --lun 0

Ciblage par défaut : HA1 / BUS0 / ID0 / LUN0 (Yamaha A3000 du banc).
"""
from __future__ import annotations

import argparse
import ctypes
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from a3000_transfer.scsi_passthrough import (  # noqa: E402
    SCSI_PASS_THROUGH_DIRECT,
    close_handle,
    open_adapter,
    send_cdb,
)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--ha", type=int, default=1, help="Host adapter")
    parser.add_argument("--bus", type=int, default=0, help="PathId / bus SCSI")
    parser.add_argument("--target", type=int, default=0, help="Target ID")
    parser.add_argument("--lun", type=int, default=0, help="LUN")
    args = parser.parse_args()

    expected = 56
    actual = ctypes.sizeof(SCSI_PASS_THROUGH_DIRECT)
    print(f"sizeof(SCSI_PASS_THROUGH_DIRECT) = {actual} (attendu {expected} sur x64)")
    if actual != expected:
        print("WARNING: layout de struct inattendu, le pass-through risque d'échouer.")

    handle = open_adapter(args.ha)
    try:
        # Standard SCSI INQUIRY: opcode 0x12, allocation length 36 (0x24)
        cdb = bytes([0x12, 0x00, 0x00, 0x00, 0x24, 0x00])
        result = send_cdb(
            handle,
            path_id=args.bus,
            target_id=args.target,
            lun=args.lun,
            cdb=cdb,
            data_in_length=36,
        )
    finally:
        close_handle(handle)

    print(f"ScsiStatus      : 0x{result.scsi_status:02X}")
    print(f"Bytes transférés: {result.transferred}")
    print(f"Data (hex)      : {result.data.hex(' ')}")

    if result.scsi_status != 0:
        print("Sense (premiers 18 octets):", result.sense[:18].hex(' '))
        print("--> commande NON OK, pas de parsing INQUIRY.")
        return 1

    if len(result.data) < 36:
        print(f"--> moins de 36 octets retournés ({len(result.data)}), parsing partiel.")

    if len(result.data) >= 1:
        device_type = result.data[0] & 0x1F
        peripheral_qualifier = (result.data[0] >> 5) & 0x07
        print(f"Peripheral qual : 0x{peripheral_qualifier:X}  device type: 0x{device_type:02X}")

    def ascii_clean(b: bytes) -> str:
        return b.decode("ascii", errors="replace").rstrip(" \0")

    if len(result.data) >= 36:
        vendor = ascii_clean(result.data[8:16])
        product = ascii_clean(result.data[16:32])
        revision = ascii_clean(result.data[32:36])
        print(f"Vendor          : {vendor!r}")
        print(f"Product         : {product!r}")
        print(f"Revision        : {revision!r}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
