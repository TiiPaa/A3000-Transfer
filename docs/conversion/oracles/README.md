# Oracles de conversion

Phase 4 du protocole. **Tests de parité bit-à-bit** entre l'implémentation
Python source et l'implémentation Rust cible. Les golden files capturés ici
servent de référence inviolable pendant le port.

## Principe

1. **Capture** : `generate_oracles.py` prend des entrées de test fixées,
   appelle les fonctions Python, sérialise les sorties dans `golden/`
   (JSON pour les bytes, binary pour le PCM, etc.)
2. **Validation** : pendant le développement Rust, des tests d'intégration
   (`cargo test`) chargent les golden files, exécutent la même entrée sur
   le code Rust, et comparent
3. **Re-capture** : si on change un comportement intentionnellement (cf.
   `DECISIONS.md`), on re-run `generate_oracles.py` pour mettre à jour les
   goldens, et on commit le diff explicitement

## Couverture par phase

| Phase | Module | Oracle | Type |
|---|---|---|---|
| 1 | `core::smdi` | `encode_sample_header_request`, `encode_data_packet`, `encode_master_identify`, `encode_begin_sample_transfer`, `encode_send_next_packet`, `encode_end_of_procedure`, `encode_abort_procedure`, `decode_*` round-trip | bit-à-bit |
| 1 | `core::wav` | `peek_wave_metadata` (header parsing) ; `load_wave` (full PCM 16-bit avec dither, seed fixe) | bit-à-bit |
| 1 | `core::midi` | génération SMF pour différents (onsets, sr, n_beats) | bit-à-bit |
| 2 | `onset::detect_transients` | indices d'onsets sur N WAVs de test | tolérance ≤1 frame (~11 ms à 44.1k) |

## Structure

```
oracles/
├── README.md                  (ce fichier)
├── generate_oracles.py        Script principal de capture
├── inputs/                    Entrées de test (WAVs, configs)
│   ├── wavs/                  Petits WAVs représentatifs (drum loop, voix, etc.)
│   └── ...
└── golden/                    Sorties Python attendues
    ├── smdi/
    │   ├── encode_sample_header_request.json
    │   ├── encode_master_identify.json
    │   └── ...
    ├── wav/
    │   ├── manifest.json      (path WAV → métadonnées attendues)
    │   └── pcm_*.bin          (PCM 16-bit LE attendu après load_wave)
    ├── midi/
    │   ├── manifest.json
    │   └── slf_*.mid          (bytes SMF attendus)
    └── onset/
        ├── manifest.json      (path WAV → liste d'onset indices)
```

## Convention pour le seed des PRNG

Pour reproductibilité du dither TPDF (`load_wave` 24→16-bit) :
- Python : `random.Random(42)`
- Rust : `mt19937::MT19937::new_with_slice_seed(&[42])` (mt19937 crate)

Le seed `42` est codé en dur dans `generate_oracles.py` et dans les tests
Rust. Si on doit le changer, c'est une **décision** à logger dans
`DECISIONS.md`.

## Lancer la capture

Depuis la racine du projet :

```powershell
cd python
pip install -e .
cd ../docs/conversion/oracles
python generate_oracles.py
```

## Lancer la validation côté Rust

(Une fois le crate Rust démarré)

```powershell
cd rust
cargo test --workspace
```

Les tests Rust chargent depuis `../docs/conversion/oracles/golden/` (chemin
relatif depuis `rust/`).
