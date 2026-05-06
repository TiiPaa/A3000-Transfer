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
- [ ] **REPRENDRE ICI** Rust Phase 3d — Wiring tabs Upload/Download au WorkerClient + table complète + drag-IN files/archives

## Moyen terme (Sprint)
- [ ] Rust Phase 4 — Slicer egui + custom waveform widget + drag-out OLE MIDI
- [ ] Rust Phase 5 — Polish + design dark theme + packaging release

## Long terme (Backlog)
- [ ] Distribuer le .exe (Python) avec un installeur signé (élimine l'avertissement Defender)
- [ ] Support multi-sampler (A4000, A5000) si demandes utilisateurs
- [ ] CI GitHub Actions : cargo test + cargo clippy sur les 3 crates Rust

## Bugs à corriger
- (aucun connu côté Python à ce stade)
