# A3000-Transfer — Conversion Inventory (Python → Rust)

Phase 1 du protocole de conversion. Snapshot exact de l'application source
au moment où on démarre le port. À valider par l'utilisateur avant tout code
Rust.

## Métadonnées projet

- **Nom** : `a3000-transfer`
- **Version Python** : 0.2.0 (cf. `python/pyproject.toml`)
- **Python requis** : ≥ 3.11 (testé sur CPython 3.12.x sous Windows 11)
- **OS cible** : Windows 11 x64 uniquement (utilise `IOCTL_SCSI_PASS_THROUGH_DIRECT`,
  OLE drag-drop, ShellExecuteEx/UAC)
- **Hardware cible** : Yamaha A3000 / A4000 / A5000 sampler via SCSI
  (Adaptec 2940UW + driver djsvs.sys validé en lab)

## Arborescence + LOC

```
python/a3000_transfer/
├── __init__.py             6 LOC   (vide ou métadonnées)
├── __main__.py            42 LOC   Entry point + routing --worker + cache numba
├── cli.py                274 LOC   Sous-commandes argparse
├── gui.py              1 688 LOC   GUI tkinter principale
├── _worker.py            275 LOC   Worker admin (UAC), socket TCP localhost
├── models.py              50 LOC   Dataclasses ScsiTargetInfo, WavePayload
├── scsi_passthrough.py   216 LOC   IOCTL_SCSI_PASS_THROUGH_DIRECT via ctypes
├── smdi.py               557 LOC   Codec SMDI Peavey (encode/decode messages)
├── spti.py               232 LOC   Scan SCSI devices (IOCTL_SCSI_GET_INQUIRY_DATA)
├── transfer.py           435 LOC   Orchestrateur transfer_sample / receive_sample
├── wav_reader.py         121 LOC   Lecture WAV multi-format → PCM 16-bit + dither
└── slicer/
    ├── __init__.py         8 LOC
    ├── engine.py         194 LOC   detect_transients (librosa wrapper)
    └── view.py         1 270 LOC   GUI slicer (tkinter + matplotlib)
                       ━━━━━━━━━
TOTAL                  5 368 LOC
```

## Points d'entrée

| Entrée | Mode | Déclenche |
|---|---|---|
| `python -m a3000_transfer gui` | GUI principale | `gui.A3000TransferApp` |
| `python -m a3000_transfer scan/list/send/receive/...` | CLI | `cli.main()` |
| `python -m a3000_transfer --worker --port N` | Worker admin (lancé via UAC depuis GUI) | `_worker.main()` |
| `dist/A3000Transfer/A3000Transfer.exe` | .exe PyInstaller (mêmes routes selon argv) | idem ci-dessus |

## Dépendances tierces (versions installées au moment du port)

| Package | Version | Usage réel | Criticité |
|---|---|---|---|
| `tkinterdnd2` | 0.4.3 | Drag-drop IN files (root); drag-drop OUT MIDI vers DAW (slicer) | **Critique** — pas d'équivalent Rust direct, drag-OUT OLE à réimplémenter |
| `numpy` | 2.4.4 | Buffers audio, dither TPDF, conversions PCM | Critique (port → `ndarray`) |
| `librosa` | 0.11.0 | `onset.onset_detect`, `to_mono` | **Critique — aucun équivalent Rust mature** |
| `soundfile` | 0.13.1 | Lecture WAV multi-format (libsndfile) | Critique (port → `symphonia`) |
| `matplotlib` | 3.10.9 | Waveform canvas dans le slicer (TkAgg backend) | Important (port → custom egui canvas) |
| `mido` | 1.3.3 | Génération SMF (MidiFile + MetaMessage + Message) | Simple (port → `midly`) |
| `sounddevice` | 0.5.5 | Audio playback (slicer ▶ Loop) | Important (port → `cpal` ou `rodio`) |
| `numba` | 0.65.1 | JIT compile pour librosa (transitif) | N/A (disparaît avec le port de librosa) |
| `scipy` | 1.17.1 | Transitif (librosa interne, signal processing) | N/A |
| `pyinstaller` | 6.20.0 | Build du .exe | N/A (Rust → cargo build) |
| `pillow` | 12.2.0 | Génération icône | N/A (un script one-shot) |

**Deps stdlib utilisées non triviales** :
- `wave` (avant le port `soundfile` complet — encore référencé dans `transfer.py:save_smdi_to_wav`)
- `socket` (TCP localhost worker IPC)
- `ctypes` + `ctypes.wintypes` (Win32 IOCTL, ShellExecuteEx, OleInitialize)
- `tkinter` + `tkinter.ttk` (GUI)
- `tempfile`, `tarfile`, `zipfile` (extraction archives upload)
- `threading`, `queue` (UI thread + worker threads)
- `json` (config persisted, IPC worker)

## I/O externes

### Système de fichiers
- Lecture WAV : n'importe où sur disque (drag'n'drop ou file picker)
- Écriture WAV : sortie download + slicer export
- Écriture MIDI : `%TEMP%/a3000_slicer_midi_xxx/*.mid` (drag-out)
- Extraction archives : `%TEMP%/a3000_transfer_xxx/` (cleanup à fermeture)
- Config persistée : `%APPDATA%/a3000_transfer/config.json` (HA/Bus/Target/LUN)
- Cache numba : `%APPDATA%/a3000_transfer/numba_cache/` (set via env var)
- Logs worker : `%TEMP%/a3000_worker.log`

### Network
- Aucun accès réseau externe
- Socket TCP localhost (loopback) entre GUI et worker admin

### Variables d'environnement
- `APPDATA` (lu, défini par Windows)
- `TEMP` (via `tempfile`)
- `NUMBA_CACHE_DIR` (set par nous dans `__main__.py`)
- `PYTHONPATH` (implicite mode dev)

### Périphériques
- `\\.\Scsi0:` à `\\.\Scsi15:` via `CreateFileW` (admin requis)
- IOCTL : `IOCTL_SCSI_GET_INQUIRY_DATA` (0x0004100C),
  `IOCTL_SCSI_PASS_THROUGH_DIRECT` (0x0004D014)
- Audio output device (sounddevice/PortAudio) pour playback
- Clipboard Windows (Ctrl+V dans upload tab : `CF_HDROP`)

## Modèle de concurrence

### Threads
- **Main thread** : Tk event loop (`root.mainloop`)
- **Worker GUI thread** : un par opération (upload batch, download batch, scan)
  Communique via `queue.Queue` posté vers le main thread (`_drain_ui_events`)
- **Slicer load thread** : daemon thread pour `sf.read` + `librosa.to_mono` +
  `detect_transients`
- **Peek thread** : daemon thread pour lire les headers WAV des fichiers droppés
- **Prewarm thread** : daemon, démarre numba JIT au lancement

### Sub-process
- **Worker admin** : `_worker.py` lancé via `ShellExecuteExW` avec verb `runas`
  (UAC popup), communique via socket TCP localhost JSON line-protocol
- Architecture split : GUI non-admin (drag'n'drop OK), worker admin (SCSI OK)

### Pas d'asyncio
- Tout en `threading` + `queue` standard

## Contrats publics (ne PAS casser)

### CLI
Sous-commandes : `scan`, `identify`, `list`, `send`, `receive`, `delete`, `gui`
Arguments cohérents : `--ha`, `--bus`, `--target`, `--lun`, `--start`, `--limit`, etc.
Réf : `cli.py:_build_parser()`

### Format Sample Header SMDI (Yamaha A3000 spec corrigée)

26 octets fixes + nom variable. **Important** : ce format a été DEBUG durant le
projet — Period sur 3 octets (pas 4), Pitch Fraction sur 3 octets (pas 2). Ne
pas régresser ce détail. Réf docstring `CLAUDE.md` lignes 130-150.

### Config persistée (compatibilité avec versions futures)
`%APPDATA%/a3000_transfer/config.json` :
```json
{"ha": 1, "bus": 0, "target": 0, "lun": 0}
```

### Worker IPC (JSON line-protocol)
Réf `_worker.py` docstring début. Commands : `find_free_slot`, `list_samples`,
`receive`, `transfer`, `exit`. Events : `ready`, `progress`, `done`, `error`,
`free_slot`, `samples_list`, `received`, `scan_progress`, `cancelled`.

### Format MIDI export (slicer)
- 1 note par slice, chromatique ascendante depuis C2 (note 36)
- Tempo calculé : BPM = N_beats × 60 / total_duration_sec
- PPQ = 480
- Réf `slicer/view.py:_generate_midi_temp`

## Comportements implicites / pièges connus

1. **Patch scipy post-build** (`python/patch_scipy.py`) : workaround
   `_distn_infrastructure.py` ligne ~369 NameError. À reproduire si on garde
   un build Python en parallèle ; non applicable au port Rust (pas de scipy).

2. **Cache numba dans `%APPDATA%`** : sans NUMBA_CACHE_DIR set tôt, le bundle
   PyInstaller (read-only) ne peut pas cacher → 20-30s à chaque lancement.
   Spécifique au port Python ; le port Rust n'a pas ce problème (compile
   avant runtime).

3. **Format Sample Header A3000** (déjà mentionné) : Period sur 3 octets,
   pas 4 ; Pitch Fraction sur 3 octets, pas 2.

4. **Iomega Zip drive sur la chaîne SCSI** perturbe les transferts longs
   → désactiver dans le menu sampler ou dans Device Manager Windows.

5. **Bulk Protect A3000** : si activé, les transferts SMDI échouent
   silencieusement avec sense 0x81 (no reply pending). Le port Rust doit
   reproduire la détection + popup explicite (cf. `_worker.py` handler
   `transfer` qui catch `SmdiNoReplyPendingError`).

6. **Tkdnd cold-load OLE Windows** au 1er drop : ~20-30s la 1ère fois
   après lancement. Mitigé par drop target unique sur root + prewarm.
   Le port Rust a son propre cold-load potentiel (windows-rs OLE
   `RegisterDragDrop` à warmer ?).

7. **WAV "1 fichier = 1 sample" mapping** : `wav_reader` accepte
   8/16/24/32-bit int + 32-bit float, mono ou stéréo, n'importe quel SR.
   Sortie toujours 16-bit signed LE pour le sampler. TPDF dither pour
   les conversions de bit-depth.

## Couverture de tests existante

- **Aucun test automatisé** dans `python/`. Pas de pytest, pas de framework.
- Validation manuelle : binaire `cli scan` sur Yamaha A3000 réel + drag'n'drop GUI.
- Scripts probes dans `python/scripts/` (pas dans le code prod).

**Implication pour la conversion** : le port Rust ne peut pas s'appuyer sur
des tests existants. Phase 4 (oracles) doit créer les comparateurs depuis zéro.

## Out of scope pour le port Rust

- Le prototype C# legacy `src/` reste tel quel (référence historique uniquement)
- Les manuels A3000 dans `docs/` (PDF, copyright)
- Les samples de test `python/samples/` (audio binaires, regenerable)
