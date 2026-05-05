# a3000-transfer (Rust port)

Port Rust en cours de l'app Python `python/a3000_transfer/`. La version Python
reste fonctionnelle pendant tout le port (rewrite parallèle, pas big-bang).

## Structure

```
rust/
├── Cargo.toml          Workspace avec deps partagées
└── crates/
    ├── a3000-core/     SCSI / SMDI / WAV / MIDI (Phase 1)
    ├── a3000-onset/    Détection de transients (Phase 2, port librosa)
    └── a3000-app/      GUI egui + worker UAC (Phases 3-4)
```

## Documentation de la conversion

Les artefacts de la méthode de conversion sont dans `docs/conversion/` :

- `INVENTORY.md` — snapshot du code Python source
- `MAPPING.md` — équivalences Python → Rust avec justifications
- `DECISIONS.md` — journal des écarts assumés (à enrichir au fil du port)
- `oracles/` — golden files pour tests de parité bit-à-bit

## Build

```powershell
cd rust
cargo build --release
```

## Tests

```powershell
cargo test --workspace
```

Les tests d'oracle chargent les goldens depuis `../docs/conversion/oracles/golden/`.

## État actuel

**Phase 0 — Préparation : DONE**
- Inventaire, mapping, décisions
- Workspace Cargo scaffold
- Capture oracles SMDI fonctionnelle (9 fichiers JSON)

**Phase 1 — Core SCSI/SMDI/WAV/MIDI : TODO**
**Phase 2 — Détection de transients : TODO**
**Phase 3 — GUI shell + tabs Upload/Download : TODO**
**Phase 4 — Slicer + drag-out MIDI : TODO**
**Phase 5 — Polish + packaging : TODO**

## Plan détaillé

`C:\Users\baboost\.claude\plans\peppy-zooming-knuth.md`
