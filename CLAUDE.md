# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project context

Windows-only utility for transferring WAV samples to a Yamaha A3000/A4000/A5000 sampler over SCSI (SMDI). The repo holds **two parallel prototypes**:

- `python/` — **active MVP**. Validates SPTI scanning and WAV input. Targets the fastest path to confirm device detection on Windows 11.
- `src/` — earlier C# prototype kept as reference. Mirrors the Python design but is no longer the focus.

Neither prototype implements the actual SMDI sample transfer yet. Both currently stop at "scan + WAV validation".

The text in `README.md` and `python/README.md` is in French — match that language when editing those files.

## Common commands

### Python MVP (primary)

Run from the repo root or `python/` (no install needed, plain stdlib):

```powershell
python -m a3000_transfer scan
python -m a3000_transfer scan --json
python -m a3000_transfer send --wave C:\samples\kick.wav --ha 0 --bus 0 --target 5 --lun 0
```

Requires Python 3.11+ and a **PowerShell/CMD launched as Administrator** — `\\.\ScsiN:` returns `ERROR_ACCESS_DENIED` otherwise. The code detects this case and raises `PermissionError` with a hint.

There is no test suite, no linter config, and no install step (`pyproject.toml` declares the package but it is invoked directly via `-m`).

### C# prototype (legacy)

```powershell
dotnet build src/A3000Transfer.Windows/A3000Transfer.Windows.csproj
dotnet run --project src/A3000Transfer.Windows/A3000Transfer.Windows.csproj -- scan
dotnet run --project src/A3000Transfer.Windows/A3000Transfer.Windows.csproj -- send --wave <file.wav> --ha 0 --target 5
```

Requires .NET 8 SDK. `A3000Transfer.Core` targets `net8.0`, `A3000Transfer.Windows` targets `net8.0-windows`.

## Architecture

### SPTI scan flow (the load-bearing piece in both prototypes)

The scan is the same algorithm in Python (`python/a3000_transfer/spti.py`) and C# (`src/A3000Transfer.Windows/Scsi/SptiScsiTransport.cs`):

1. Iterate host adapters `\\.\Scsi0:` through `\\.\Scsi15:` via `CreateFileW`.
2. For each opened handle, send `IOCTL_SCSI_GET_INQUIRY_DATA` (0x0004100C) with a doubling buffer (16 KiB → 256 KiB) until the call succeeds or the cap is hit.
3. Parse the returned `SCSI_ADAPTER_BUS_INFO` blob: header → per-bus `SCSI_BUS_DATA` → linked list of `SCSI_INQUIRY_DATA` records via `NextInquiryDataOffset`.
4. From each inquiry payload, read the standard SCSI INQUIRY fields: device type (byte 0, low 5 bits), vendor (8..16), product (16..32), revision (32..36).

The Python version exposes `path_id` (SCSI bus) on `ScsiTargetInfo`; the C# `ScsiTargetInfo` record does not. If you align them, update `display_name`/`DisplayName` and the `send` lookup that filters on `bus` (Python) vs. only `HostAdapter`/`TargetId` (C#).

### Layering (C#)

- `A3000Transfer.Core` — pure logic. `IScsiTransport`, `IWaveReader`, `SampleTransferService` (orchestration + MVP validation rules: 16-bit PCM, mono/stereo, non-empty).
- `A3000Transfer.Windows` — Win32 interop (`Interop/SptiNative.cs`, `Interop/AspiNative.cs`), two `IScsiTransport` implementations (`SptiScsiTransport`, `AspiScsiTransport`), CLI (`Program.cs` + `Commands/CommandLine.cs`).
- `Program.cs` wires `SptiScsiTransport` by default. `AspiScsiTransport` is the older WNASPI32 path, kept but no longer the recommended route.

### Python module map

`python/a3000_transfer/` is a flat package:
- `spti.py` — SPTI scan via ctypes/`kernel32`. The CTypes `argtypes` are configured in `_load_kernel32` for `CreateFileW`/`DeviceIoControl`/`CloseHandle`.
- `wav_reader.py` — wraps stdlib `wave`, raises `WaveValidationError` for non-PCM, non-16-bit, or non-mono/stereo input.
- `models.py` — `ScsiTargetInfo` and `WavePayload` dataclasses (both `slots=True`).
- `cli.py` — argparse subcommands `scan` and `send`. The `send` command currently only validates and prints — it does **not** transfer.

### What's intentionally not implemented

The actual SMDI command sequence (CDB layout, sample header, ACK handling) is the next chunk of work. `SendSampleAsync` returns `false` (C#) and the Python `send` command prints "Transfert SMDI non implémenté". Don't claim the transfer works without verifying against a real sampler — see `docs/protocol-notes.md` for the open protocol questions.

## Working notes

- Targeting Windows 11 x64 with an Adaptec 2940U-class card. The expected first successful detection prints e.g. `HA0 BUS0 ID5 LUN0 YAMAHA A3000 1.00`.
- The 2940U + Win11 driver situation is fragile (community drivers); cabling, terminator, SCSI ID, and power-on order all affect whether the device shows up. Treat empty scan output as a hardware-side question first.
- When extending either prototype, keep both in lockstep on data shape if practical, but the Python side is where new behavior should land first.

## Current status — RESOLVED (2026-04-27)

Full SMDI sample transfer to Yamaha A3000 works under Win11 + Adaptec 2940UW + djsvs.sys. Validated in lab : `loop01.wav` (159784 frames stereo 44.1 kHz = 639136 bytes PCM BE) transferred to slot #300, 313 packets, EoP clean, 100%.

The "4 KB hard limit" we hit for weeks was **a Sample Header encoding bug on our side** : Period was being encoded on 4 bytes instead of 3, Pitch Fraction on 2 bytes instead of 3. Total remained 26 fixed bytes but every field after Period was offset by 1 byte → slave read Length = 624 words instead of 159784 → committed after ~4 KB. No firmware quirks involved.

The discovery came from comparing with the sister project `I:\Dev\Sampletrans` which already worked. Sources Sampletrans uses : **OpenSMDI** (https://www.chn-dev.net/Projects/OpenSMDI/).

## Sample Header layout (correct)

```
off  size  field
0    3     Sample Number (24-bit BE)
3    1     Bits Per Word
4    1     Number Of Channels
5    3     Sample Period (24-bit BE, ns)        ← 3 bytes, not 4
8    4     Sample Length (words/channel, BE)
12   4     Sample Loop Start (BE)
16   4     Sample Loop End (BE)
20   1     Sample Loop Control
21   1     Sample Pitch Integer (MIDI note)
22   3     Sample Pitch Fraction (24-bit BE)    ← 3 bytes, not 2
25   1     Sample Name Length (n)
26   n     Sample Name (ASCII)
```

## Probe scripts (`python/scripts/`)

All require Administrator PowerShell. Common targets default to HA1/BUS0/ID0/LUN0 (the test bench Yamaha).

- `probe_scsi.py` — list all `\\.\ScsiN:` adapters and their bus topology
- `probe_inquiry.py` — direct SCSI INQUIRY to the sampler via `IOCTL_SCSI_PASS_THROUGH_DIRECT`
- `probe_smdi_identify.py` — Master Identify → Slave Identify handshake
- `probe_sample_header_request.py [--sample N]` — read an existing slot's Sample Header
- `probe_delete_sample.py --sample N` — try Delete Sample From Memory (currently silently ignored on A3000)
- `probe_send_sample.py [--commit] [--sample N] [--verbose] [--multi-bst] [--link] [--non-direct] [--force-packet-length] [--packet-length N]` — full sample transfer attempt with various experimental flags
- `probe_bst_al8.py` — one-shot test of Begin Sample Transfer with AL=8 format variants
- `probe_adapter_caps.py [HA]` — query `IOCTL_STORAGE_QUERY_PROPERTY` for adapter capabilities (MaximumTransferLength, SrbType, etc.)

## Code layers (Python)

- `a3000_transfer/spti.py` — SCSI scan via `IOCTL_SCSI_GET_INQUIRY_DATA`
- `a3000_transfer/scsi_passthrough.py` — pass-through SCSI primitives, both `IOCTL_SCSI_PASS_THROUGH_DIRECT` and `IOCTL_SCSI_PASS_THROUGH` (non-direct), with module-level toggle `USE_NONDIRECT_BY_DEFAULT`
- `a3000_transfer/smdi.py` — SMDI message codec : encode/decode for Sample Header (Request), Begin Sample Transfer (Acknowledge), Send Next Packet, Data Packet, Abort, EoP, Delete Sample. Includes `yamaha_period_field()` helper for the Yamaha-specific period × 256 quirk and `drain_pending_reply()` for defensive bus cleanup.
- `a3000_transfer/transfer.py` — orchestrator with multi-BST opt-in, LINK bit option, force packet length option, pre-encoded data packet pool
