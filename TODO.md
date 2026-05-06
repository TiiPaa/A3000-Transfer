# TODO

## Court terme (En cours)
- [x] Rust Phase 1 — `a3000-core::wav.rs` : port wav_reader.py + oracles (bit-à-bit PCM_16, tolérance ±1 LSB dithered)
- [x] Rust Phase 1 — `a3000-core::midi.rs` : port _generate_midi_temp + oracles bit-à-bit
- [ ] **REPRENDRE ICI** Rust Phase 1 — `a3000-core::scsi.rs` : port scsi_passthrough.py via windows crate (RAII handle)
- [ ] Rust Phase 1 — `a3000-core::transfer.rs` : port transfer.py (orchestrateur SMDI)

## Moyen terme (Sprint)
- [ ] Rust Phase 2 — `a3000-onset` : port librosa.onset_detect (STFT + Mel + flux + peak + backtrack)
- [ ] Rust Phase 2 — A/B test automatisé tolérance ≤1 frame sur N WAVs de test
- [ ] Rust Phase 3 — Scaffolding GUI egui + worker UAC + tabs Upload/Download
- [ ] Rust Phase 4 — Slicer egui + custom waveform widget + drag-out OLE MIDI
- [ ] Rust Phase 5 — Polish + design dark theme + packaging release

## Long terme (Backlog)
- [ ] Distribuer le .exe (Python) avec un installeur signé (élimine l'avertissement Defender)
- [ ] Support multi-sampler (A4000, A5000) si demandes utilisateurs
- [ ] CI GitHub Actions : cargo test + cargo clippy sur les 3 crates Rust

## Bugs à corriger
- (aucun connu côté Python à ce stade)
