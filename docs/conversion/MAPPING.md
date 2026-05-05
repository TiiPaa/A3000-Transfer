# A3000-Transfer — Cartographie des équivalences (Python → Rust)

Phase 2 du protocole de conversion. Pour chaque dépendance et pattern non
trivial, choix Rust + justification. Les `🚩` marquent les zones d'attention
particulière.

## Dépendances directes

| Source (Python) | Cible (Rust) | Justification |
|---|---|---|
| `numpy` | `ndarray` 0.15+ | Numpy-like API, mature, interop facile avec rustfft |
| `numpy` (slices PCM) | `&[i16]` / `Vec<i16>` natifs | Pas besoin de `ndarray` pour PCM 1D, slices stdlib suffisent |
| `soundfile` | `symphonia` 0.5+ | Lecture WAV multi-format (8/16/24/32-bit int + float). Pure Rust, pas de DLL |
| `soundfile` (écriture WAV 16-bit) | `hound` 3.5 | Plus simple que symphonia pour writing. Limité au PCM mais c'est notre cas |
| `mido` | `midly` 0.5 | Standard de fait pour SMF en Rust. API claire, write SMF Type 1 OK |
| `tkinterdnd2` (drag-IN) | `egui::Context::input.raw.dropped_files` | Natif egui. Plus simple que tk |
| `tkinterdnd2` (drag-OUT) 🚩 | `windows::Win32::System::Com` + `IDataObject` custom | **Pas natif egui**. ~300-500 LOC pour OLE drag source |
| `tkinter` + `ttk` | `egui` + `eframe` | Choix justifié dans le plan principal |
| `matplotlib` (waveform canvas) | Custom `egui::Painter` dans widget Rust | Pas d'équivalent matplotlib en Rust ; mais on dessine manuellement (peaks/RMS bins) |
| `sounddevice` | `cpal` 0.15 ou `rodio` 0.17 | `cpal` pour contrôle bas niveau (callback de mix audio + position de lecture pour playhead). `rodio` plus haut niveau mais moins de contrôle |
| `librosa.onset.onset_detect` 🚩 | **Port custom** dans crate `a3000-onset` | **Aucun équivalent mature en Rust**. Port ligne à ligne (cf. plan principal Phase 2) |
| `librosa.to_mono` | `(L+R)/2` direct | Trivial, 5 lignes |
| `numba` (transitif) | N/A | Rust est compilé AOT, pas de JIT |
| `scipy` (transitif via librosa) | `rustfft` + `ndarray-stats` | Ce qu'on utilise vraiment : FFT, max_filter1d. Implémentables sans dep lourde |
| `pyinstaller` | `cargo build --release` + `winres` (icône) | Binaire Rust unique nativement |

## Stdlib Python → équivalents Rust

| Python | Rust | Notes |
|---|---|---|
| `wave` | `hound` ou `symphonia` | déjà couvert |
| `socket` (TCP localhost) | `std::net::{TcpListener, TcpStream}` | Direct, pas besoin de tokio pour ce use case |
| `ctypes` + `wintypes` | `windows` crate (windows-rs) | Bindings officiels Microsoft, type-safe |
| `tkinter` | `egui` | déjà couvert |
| `tempfile.mkdtemp` | `tempfile::TempDir` | API similaire |
| `tarfile` / `zipfile` | `tar` + `flate2` (gz) ; `zip` | Pure Rust |
| `threading` + `queue` | `std::thread` + `std::sync::mpsc` ou `crossbeam-channel` | mpsc OK pour 1-1 ; crossbeam pour multi-producteurs |
| `json` | `serde_json` | Standard de fait |
| `argparse` | `clap` 4.x | Standard de fait |
| `dataclass` | `struct` + `#[derive(Debug, Clone, Serialize, Deserialize)]` | Idiomatique |
| `random.Random` (TPDF dither) | `rand` + `rand::SeedableRng` | Préciser le PRNG (Pcg, ChaCha) pour reproductibilité oracles |

## Patterns Python → idiomatique Rust

| Pattern source | Cible Rust |
|---|---|
| Exceptions `raise ScsiError(...)` | `Result<T, ScsiError>` avec `thiserror` (lib) / `anyhow` (binaire) |
| Hierarchie d'exceptions (`SmdiProtocolError` < `Exception`) | `enum SmdiError { Reject(...), NoReply, Protocol(...) }` |
| `with` context manager (file / handle) | RAII via `Drop` impl |
| Dataclass `WavePayload` (frozen) | `#[derive(Debug, Clone)] struct WavePayload { ... }` (immutable par défaut) |
| Generators / `yield` | trait `Iterator` |
| `threading.Lock` | `Mutex<T>` (parking_lot pour perf si besoin) |
| Type hints optionnels | Types statiques obligatoires (le compilo force) |
| Duck typing | Traits + `impl Trait` ou génériques bornés |
| `**kwargs` | Pas d'équivalent direct → builder pattern ou struct config |
| String formatting `f"...{x}..."` | `format!()` macro |

## Choix structurants (à graver dans le marbre)

### Concurrence : threads + channels, pas de tokio

**Justification** : l'app n'est pas IO-bound (pas de réseau, pas de DB). Les
opérations long-running sont :
- SCSI transfer : sync IOCTL, pas async
- Audio decoding : CPU-bound
- Onset detection : CPU-bound

L'overhead d'un runtime async (tokio) serait inutile. `std::thread` + `std::sync::mpsc`
suffit pour la communication GUI ↔ workers ↔ worker process.

### Gestion d'erreurs : `thiserror` + `anyhow`

- Crates lib (`a3000-core`, `a3000-onset`) : `thiserror` pour des erreurs typées
  (utiles aux callers, exhaustivité dans les `match`)
- Crate binaire (`a3000-app`) : `anyhow` pour propager facilement avec `?`

### Logging : `tracing`

Plus expressif que `log` (spans, structured fields). Sortie console + fichier
rotatif via `tracing-appender`.

### IPC GUI ↔ worker : JSON line-protocol via `serde_json`

Garder le contrat existant (cf. `_worker.py` docstring). Strict compat : un
worker Python pourrait rester compatible avec une GUI Rust ou inverse pendant
la transition (même si on ne s'en sert pas, c'est un filet de sécurité).

### Tests : `cargo test` + harness oracles dédiés

- Unitaires : dans `mod tests` à côté des fonctions
- Intégration : `tests/` au niveau crate
- Oracles : un binaire séparé `xtask/oracle.rs` (ou script Python)
  qui compare sorties Rust et sorties Python sur les mêmes entrées

## 🚩 Zones d'attention identifiées

### 1. `librosa.onset.onset_detect` — port from scratch

**Pas d'équivalent Rust mature**. Détaillé dans plan Phase 2. La qualité de
détection est le critère bloquant pour valider cette phase.

Décision : port pur Rust de l'algo (rustfft + ndarray + custom code), pas
de FFI vers aubio (différent algo, qualité non équivalente).

### 2. OLE drag-OUT MIDI

**Pas natif egui**. Implémentation manuelle via `windows` crate :
- Implémenter `IDataObject` COM interface
- Format CF_HDROP avec DROPFILES struct + path UTF-16 + `\0\0`
- `DoDragDrop` au moment du press-and-drag du bouton "↓ Drag MIDI"
- Génération du `.mid` à `IDataObject::GetData()` (lazy, juste avant le drop)

Risque : COM en Rust est verbeux ; complexité ~300-500 LOC. Fallback prévu :
bouton "Save MIDI to..." avec file dialog si bloqué.

### 3. Cold-load tkdnd OLE → équivalent Rust ?

`windows-rs` `RegisterDragDrop` peut avoir un cold-load similaire. À mesurer
en Phase 4 / 5. Si problème : pré-warmer en background thread au démarrage
(comme on le fait en Python).

### 4. TPDF dither reproductibilité

Le code Python utilise `random.Random()` (Mersenne Twister) avec une seed
implicite (timestamp). Pour les tests d'oracle, on doit fixer la seed côté
Python ET côté Rust :

- Côté Python (oracle generation) : `random.Random(seed=42)` → fixed
- Côté Rust : `rand::SeedableRng::seed_from_u64(42)` avec un PRNG aux propriétés
  équivalentes (Mersenne Twister via `mt19937` crate, ou ChaCha8 si on accepte
  une distribution différente mais bornée)

**Décision à graver** : on utilise **mt19937** crate pour matcher exactement
le PRNG Python sur les tests d'oracle.

### 5. Format Sample Header A3000

À reproduire **bit-for-bit** dans `a3000-core::smdi`. Les 26 octets fixes :
SN(3) + Bits(1) + Channels(1) + Period(3) + Length(4) + LoopStart(4) +
LoopEnd(4) + LoopCtrl(1) + PitchInt(1) + PitchFrac(3) + NameLen(1) puis Name.

Test d'oracle : feed identical inputs to Python `encode_sample_header_request`
and Rust equivalent → bytes identiques.

## Décisions explicites flaggées (récap)

- **Stratégie** : Rewrite parallèle module-par-module commit (pas big-bang)
- **Codebase Python conservé intact** dans `python/` pendant tout le port
- **Codebase Rust** dans `rust/` (workspace Cargo)
- **Pas de PyO3** : full Rust, pas d'interop hybride pendant la transition
  (un binaire Rust autonome à terme)
- **Stack GUI** : egui + eframe (pas tauri)
- **PRNG dither** : mt19937 (pas ChaCha) pour reproductibilité oracle Python

## À valider avec l'utilisateur avant Phase 1 du plan principal

- [ ] OK pour `mt19937` crate Rust (à confirmer cross-compile sans souci sur Windows MSVC)
- [ ] OK pour conserver le contrat IPC JSON line-protocol identique (interop possible Python ↔ Rust pendant la transition)
- [ ] OK pour utiliser `cpal` (low-level) plutôt que `rodio` (le slicer a besoin de la position de lecture pour la playhead)
- [ ] OK pour la stratégie de Phase 4 oracle : on génère les golden depuis Python et on compare bit-à-bit où c'est possible (SMDI codec, MIDI gen, WAV PCM 16-bit), tolérance de ≤1 frame pour onset detection (subjective)
