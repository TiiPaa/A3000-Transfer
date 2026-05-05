# A3000-Transfer — Journal des écarts assumés (Python → Rust)

Phase 6 du protocole de conversion. Documenter chaque décision où l'implémentation
Rust **diverge intentionnellement** de l'implémentation Python source. Sert à :
- Empêcher un relecteur de "corriger" silencieusement vers le comportement Python
- Garder une trace décisionnelle pour les futurs maintainers
- Faciliter la code review

## Format par entrée

```
### [Module] — [Titre court]
**Source (Python)** : comment c'était fait
**Cible (Rust)** : comment c'est fait maintenant
**Pourquoi** : justification
**Impact** : ce que ça change pour les consumers ou la prod
**Alternative envisagée** : ce qui a été rejeté et pourquoi
```

---

## Décisions actées avant code

### `core::scsi` — Type-safe handle au lieu de `int`
**Source (Python)** : `scsi_passthrough.py` retourne un `int` (HANDLE Win32) traité comme opaque
**Cible (Rust)** : type `ScsiHandle` (newtype wrapper sur `windows::Win32::Foundation::HANDLE`) avec `Drop` qui CloseHandle
**Pourquoi** : RAII garantit la fermeture du handle même en cas d'erreur ou panic. Le `int` Python doit être close manuellement (et c'est fait dans des try/finally à la main)
**Impact** : pas de fuite de handle possible. API publique idiomatique
**Alternative envisagée** : exposer le HANDLE brut. Rejeté car non-idiomatique Rust et risque de fuite

### `core::smdi` — Erreurs typées au lieu d'exceptions hiérarchisées
**Source (Python)** : hierarchie `SmdiProtocolError` < `Exception`, sous-classes `SmdiNoReplyPendingError`, etc.
**Cible (Rust)** : `enum SmdiError { Reject{code, sub}, NoReply, Protocol(String), Io(io::Error) }` avec `thiserror`
**Pourquoi** : exhaustivité forcée par le compilateur, pattern matching explicite, performance (pas d'unwinding)
**Impact** : les call sites doivent matcher explicitement les variants — c'est voulu (cf. handler Bulk Protect dans worker)
**Alternative envisagée** : `Box<dyn Error>` partout. Rejeté car perd la richesse du pattern match

### `app` — Pas de PyO3, full rewrite
**Source (Python)** : monolithe Python
**Cible (Rust)** : binaire Rust autonome dans `rust/`, Python conservé intact dans `python/` pendant la transition
**Pourquoi** : objectif final est un binaire 25 MB autonome. PyO3 hybride retarderait sans bénéfice puisque le but est de remplacer Python complètement
**Impact** : pas d'interop. Les deux versions tournent en parallèle, on bascule à la fin quand Rust est validé bout-en-bout
**Alternative envisagée** : approche hybride PyO3 (port progressif du core Rust avec GUI Python). Rejetée car ajoute de la complexité de packaging et la transition aurait été reportée

### `onset` — Port pur Rust de librosa, pas de FFI aubio
**Source (Python)** : `librosa.onset.onset_detect` (algo spectral flux + peak picking + backtrack)
**Cible (Rust)** : crate `a3000-onset` qui port l'algo ligne à ligne
**Pourquoi** : `aubio-rs` (FFI vers libaubio C) utilise un algo différent (energy-based bandes de fréquence) → qualité de détection différente sur drums/transients. On veut une parité ≤1 frame, donc même algo
**Impact** : ~700 LOC Rust à écrire et tester, mais aucun .so/.dll C à packager
**Alternative envisagée** : `aubio-rs` pour MVP rapide. Rejetée car le test A/B vs Python échouerait probablement

### `wav` — `mt19937` Rust crate pour TPDF dither reproductible
**Source (Python)** : `random.Random()` (Mersenne Twister, seed implicite timestamp)
**Cible (Rust)** : crate `mt19937` (Mersenne Twister 19937) avec seed fixe pour les tests, seed time-based en runtime
**Pourquoi** : reproductibilité oracle. Comparer bytes Python ↔ Rust nécessite le même PRNG bit-à-bit
**Impact** : le dither produit est identique au Python pour des seeds identiques. En runtime user-facing aucun impact perceptible (le dither est ±1 LSB)
**Alternative envisagée** : ChaCha8 (PRNG cryptographique rapide). Rejetée pour la reproductibilité oracle. ChaCha pourrait être ré-évalué après que le port soit validé

### `gui` — Persistance config compatible avec Python
**Source (Python)** : `%APPDATA%/a3000_transfer/config.json` avec `{"ha", "bus", "target", "lun"}`
**Cible (Rust)** : même path, même format JSON, même clés
**Pourquoi** : un user qui passe de la version Python à Rust garde sa config sans la retaper
**Impact** : compatibilité forward et backward avec versions Python existantes
**Alternative envisagée** : nouveau format TOML / RON. Rejeté pour préserver l'expérience user

### Concurrence — std::thread + mpsc, pas de tokio
**Source (Python)** : `threading` + `queue`
**Cible (Rust)** : `std::thread::spawn` + `std::sync::mpsc::channel`
**Pourquoi** : aucune opération IO async dans l'app (tout est CPU-bound ou IOCTL sync). tokio ajouterait une dep lourde et de la complexité sans bénéfice
**Impact** : architecture concurrente identique au Python
**Alternative envisagée** : tokio (overkill), rayon (pas adapté pour event loop GUI)

---

## Décisions à prendre ultérieurement (TBD)

Ces points seront tranchés au fur et à mesure du port et ajoutés ici.

- Cross-tab navigation : rester sur tabs egui standard ou implémenter une nav custom ?
- Settings dialog : `egui::Window` modal ou popup persistant ?
- Worker process : conserver le JSON line-protocol exact ou simplifier (RON, MessagePack) ?
- Onset detection : implémenter `librosa.onset_strength_multi` complet ou la version simplifiée single-band suffit ?
- Audio playback ▶ Loop : positions samplerate-accurate via `cpal` callback ou polling clock simple ?

---

## Comment ajouter une décision

Quand pendant le port on choisit de diverger volontairement du Python :
1. Ouvrir `DECISIONS.md`
2. Ajouter une section avec le format ci-dessus
3. Mentionner la décision dans le message de commit du module concerné
