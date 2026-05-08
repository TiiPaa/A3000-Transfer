# TODO

## Court terme (En cours)
- [x] Rust Phase 1 — `a3000-core::wav.rs` : port wav_reader.py + oracles (bit-à-bit PCM_16, tolérance ±1 LSB dithered)
- [x] Rust Phase 1 — `a3000-core::midi.rs` : port _generate_midi_temp + oracles bit-à-bit
- [x] Rust Phase 1 — `a3000-core::scsi.rs` : port scsi_passthrough.py via windows crate (RAII handle, retry ERROR_IO_DEVICE, buffer 512-aligned)
- [x] Rust Phase 1 — `a3000-core::transfer.rs` : port transfer.py (orchestrateur SMDI)
- [x] Test bout-en-bout Rust scan/identify/transfer sur Yamaha A3000 réel (loop01.wav + RawCutz_*.wav OK sur slot 300)
- [x] Rust Phase 2 — `a3000-onset` : port librosa.onset_detect (STFT + Mel + flux + peak + backtrack), 17 tests passent
- [x] Rust Phase 2 — A/B test ≤1 frame tolérance vs librosa : **150/150 onsets matched (100%)** sur 5 WAVs (synthétique + 3 drum loops + reese)
- [x] Rust Phase 3a — Scaffolding GUI egui : eframe::App + 3 tabs vides + theme dark + IPC types Cmd/Event
- [x] Rust Phase 3b — Worker process port _worker.py (TCP localhost JSON line, find_free_slot/list_samples/receive/transfer/exit)
- [x] Rust Phase 3c — UAC elevation via ShellExecuteExW (WorkerClient bind port + spawn worker élevé + handshake "ready")
- [x] Rust Phase 3d.1 — WorkerClient lifecycle dans App (WorkerState Idle/Connecting/Connected/Error + bouton Connect)
- [x] Rust Phase 3d.2 — Settings dialog modal HA/Bus/Target/LUN + auto/manual start slot
- [x] Rust Phase 3d.3 — Upload tab : table colonnes + drag-IN WAV + batch via WorkerSender (FindFreeSlot → Transfer séquentiel + progress live)
- [x] Rust Phase 3d.4 — Download tab : Scan → ListSamples → table samples + Download batch séquentiel
- [x] Fix alignement colonnes tables Upload/Download : `allocate_exact_size` + `child_ui` + `TextWrapMode::Truncate` (les labels longs ne poussent plus le layout)
- [x] Fix UI : footer ancré bas, progress bar fluide (ctx.request_repaint() depuis reader thread), padding boutons + ROW_H 28 px, glyphs Unicode broken remplacés
- [x] Fix UI : ordre boutons footer (Upload primaire flush right)
- [x] Fix UI : alignement vertical boutons footer Upload/Download — tous les boutons passent par `add_sized([_, 32], …)` (chemin layout `centered_and_justified` uniforme) au lieu de mélanger `min_size` et `add_sized` ; retrait de `RichText::strong()` et du glyph `▶` (galley asymétrique) sur Upload/Download
- [x] Rust Phase 4a — Slicer : drop WAV → waveform peaks + onsets auto-détectés via a3000-onset
- [x] Rust Phase 4b — Slicer : selection cells (click + drag-select range) + drag onsets (capture onset au press, pas au drag_started)
- [x] Rust Phase 4c — Slicer : Delete marked (rebuild audio + onsets) + audio playback cpal (resampling fixed-point 32.32 préserve la hauteur) + playhead animée + zoom (molette anchor curseur, factor 1.1×) + pan (drag souris OU Shift+molette) + navigation Space / Ctrl+Space par onset
- [x] Rust Phase 4d — Slicer : Beats spinbox + Save MIDI button (a3000_core::midi::generate_midi → %TEMP%/a3000_slicer_midi/<stem>.mid) + ligne de message à hauteur fixe (allocate_exact_size, ne pousse pas la waveform)
- [x] Rust Phase 4e — Slicer : drag-OUT MIDI vers DAW via OLE (#[implement] IDataObject + IDropSource + IEnumFORMATETC fournissant CF_HDROP, DoDragDrop synchrone)
- [ ] **REPRENDRE ICI** Rust Phase 5 — Polish + design dark theme + packaging release

## Moyen terme (Sprint)

## Long terme (Backlog)
- [ ] Distribuer le .exe (Python) avec un installeur signé (élimine l'avertissement Defender)
- [ ] Support multi-sampler (A4000, A5000) si demandes utilisateurs
- [ ] CI GitHub Actions : cargo test + cargo clippy sur les 3 crates Rust

## Bugs à corriger
- (aucun connu côté Python à ce stade)
