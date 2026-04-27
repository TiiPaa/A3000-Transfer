r"""
Interroge les capacités de l'adaptateur SCSI via IOCTL_STORAGE_QUERY_PROPERTY.
Réplique la fonction QueryPropertyForDevice du sample Microsoft SPTI.

Affiche notamment :
  - MaximumTransferLength : limite de transfert annoncée par l'adapter
  - MaximumPhysicalPages : nombre max de pages physiques par transfer
  - AlignmentMask : alignement buffer requis
  - SrbType : 0=legacy SCSI_REQUEST_BLOCK, 1=STORAGE_REQUEST_BLOCK (moderne)
  - CommandQueueing : adapter supporte command queueing
  - BusType : type de bus

Si MaximumPhysicalPages=2 et MaximumTransferLength=4096, on a trouvé l'origine
de notre limite dure des 4 KB.
"""
from __future__ import annotations

import ctypes
from ctypes import wintypes
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from a3000_transfer.scsi_passthrough import close_handle, open_adapter, _load_kernel32  # noqa: E402

IOCTL_STORAGE_QUERY_PROPERTY = 0x002D1400

# STORAGE_PROPERTY_ID
StorageAdapterProperty = 1
StorageDeviceProperty = 0

# STORAGE_QUERY_TYPE
PropertyStandardQuery = 0


class STORAGE_PROPERTY_QUERY(ctypes.Structure):
    _fields_ = [
        ("PropertyId", ctypes.c_ulong),
        ("QueryType", ctypes.c_ulong),
        ("AdditionalParameters", ctypes.c_ubyte * 1),
    ]


class STORAGE_DESCRIPTOR_HEADER(ctypes.Structure):
    _fields_ = [
        ("Version", ctypes.c_ulong),
        ("Size", ctypes.c_ulong),
    ]


class STORAGE_ADAPTER_DESCRIPTOR(ctypes.Structure):
    _fields_ = [
        ("Version", ctypes.c_ulong),
        ("Size", ctypes.c_ulong),
        ("MaximumTransferLength", ctypes.c_ulong),
        ("MaximumPhysicalPages", ctypes.c_ulong),
        ("AlignmentMask", ctypes.c_ulong),
        ("AdapterUsesPio", ctypes.c_ubyte),
        ("AdapterScansDown", ctypes.c_ubyte),
        ("CommandQueueing", ctypes.c_ubyte),
        ("AcceleratedTransfer", ctypes.c_ubyte),
        ("BusType", ctypes.c_ubyte),
        ("BusMajorVersion", ctypes.c_ushort),
        ("BusMinorVersion", ctypes.c_ushort),
        ("SrbType", ctypes.c_ubyte),
        ("AddressType", ctypes.c_ubyte),
    ]


BUS_TYPES = {
    0: "Unknown", 1: "Scsi", 2: "Atapi", 3: "Ata", 4: "1394",
    5: "Ssa", 6: "Fibre", 7: "Usb", 8: "RAID",
}


def query_adapter(handle: int) -> STORAGE_ADAPTER_DESCRIPTOR | None:
    k = _load_kernel32()
    query = STORAGE_PROPERTY_QUERY()
    query.PropertyId = StorageAdapterProperty
    query.QueryType = PropertyStandardQuery

    # Step 1: get header to know required size
    header = STORAGE_DESCRIPTOR_HEADER()
    returned = wintypes.DWORD(0)
    ok = k.DeviceIoControl(
        handle, IOCTL_STORAGE_QUERY_PROPERTY,
        ctypes.byref(query), ctypes.sizeof(query),
        ctypes.byref(header), ctypes.sizeof(header),
        ctypes.byref(returned), None,
    )
    if not ok and ctypes.get_last_error() not in (122, 234):  # ERROR_MORE_DATA-ish OK
        err = ctypes.get_last_error()
        print(f"QueryProperty header step failed: {err} ({ctypes.FormatError(err)})")
        return None
    if header.Size == 0:
        print("Adapter property : pas supporté par ce driver.")
        return None
    print(f"Adapter descriptor size annoncé : {header.Size} octets")

    # Step 2: allocate full size and read full descriptor
    buf = (ctypes.c_ubyte * header.Size)()
    ok = k.DeviceIoControl(
        handle, IOCTL_STORAGE_QUERY_PROPERTY,
        ctypes.byref(query), ctypes.sizeof(query),
        ctypes.byref(buf), header.Size,
        ctypes.byref(returned), None,
    )
    if not ok:
        err = ctypes.get_last_error()
        print(f"QueryProperty data step failed: {err} ({ctypes.FormatError(err)})")
        return None

    desc = ctypes.cast(buf, ctypes.POINTER(STORAGE_ADAPTER_DESCRIPTOR)).contents
    # Copy en tant que valeur stable (le buf est local)
    out = STORAGE_ADAPTER_DESCRIPTOR()
    ctypes.memmove(ctypes.byref(out), ctypes.byref(buf), min(ctypes.sizeof(out), header.Size))
    return out


def main() -> int:
    ha = int(sys.argv[1]) if len(sys.argv) > 1 else 1
    handle = open_adapter(ha)
    try:
        desc = query_adapter(handle)
        if desc is None:
            return 1

        page_size = 4096
        max_xfer_per_pages = max(1, desc.MaximumPhysicalPages - 1) * page_size
        true_max = min(desc.MaximumTransferLength, max_xfer_per_pages)

        print()
        print(f"=== Adaptateur HA{ha} ({BUS_TYPES.get(desc.BusType, '?')}) ===")
        print(f"  Version              : 0x{desc.Version:08X}")
        print(f"  MaximumTransferLength: {desc.MaximumTransferLength} octets ({desc.MaximumTransferLength // 1024} KB)")
        print(f"  MaximumPhysicalPages : {desc.MaximumPhysicalPages}")
        print(f"  TrueMaxTransfer      : {true_max} octets ({true_max // 1024} KB)")
        print(f"  AlignmentMask        : 0x{desc.AlignmentMask:08X}")
        print(f"  CommandQueueing      : {bool(desc.CommandQueueing)}")
        print(f"  AdapterUsesPio       : {bool(desc.AdapterUsesPio)}")
        print(f"  AcceleratedTransfer  : {bool(desc.AcceleratedTransfer)}")
        print(f"  SrbType              : {desc.SrbType} ({'STORAGE_REQUEST_BLOCK (moderne)' if desc.SrbType == 1 else 'SCSI_REQUEST_BLOCK (legacy)'})")
        print(f"  BusType              : {desc.BusType} ({BUS_TYPES.get(desc.BusType, '?')})")
        print(f"  BusMajor.Minor       : {desc.BusMajorVersion}.{desc.BusMinorVersion}")

        print()
        if desc.MaximumTransferLength <= 4096 or true_max <= 4096:
            print("⚠  L'adaptateur annonce une limite de transfert ≤ 4 KB.")
            print("   C'est probablement la cause de notre EoP prématuré à 4096 octets :")
            print("   storport ne laisse pas passer plus que ce qu'annonce l'adapter.")
        else:
            print("ℹ  L'adapter accepte des transferts plus gros — la limite des 4 KB")
            print("   vient probablement d'ailleurs (firmware slave, storport, etc.).")
        return 0
    finally:
        close_handle(handle)


if __name__ == "__main__":
    sys.exit(main())
