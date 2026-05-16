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
- [x] Rust Phase 5a — Icône Windows embedded via winres (build.rs) + metadata FileVersion/CompanyName/ProductName
- [x] Rust Phase 5b — Profile release optimisé (lto=true + codegen-units=1 + strip=symbols + panic=abort) → binaire 7.1 → 5.8 MB
- [x] Rust Phase 5c — 55 tests OK en release LTO (7 ipc + 18 smdi + 1 scsi + 9 wav + 3 midi + 17 onset)
- [x] Slicer → Upload : bouton "Send to Upload (N)" qui découpe le buffer audio aux onsets et écrit chaque slice non marquée comme WAV mono 16-bit dans %TEMP%/a3000_slicer_slices/, ajoute aux items du tab Upload, switche sur Upload.

**🎉 PORT RUST COMPLET — 5 phases terminées.** Binaire 5.8 MB (vs 311 MB Python).
Démarrage instantané (pas de JIT numba à warmup). Tous les chemins Python portés et validés.

- [x] Bug UI overlap rows / header / footer : la helper `cell()` faisait `child.set_clip_rect(rect)` qui REMPLACE le clip parent au lieu d'INTERSECTER. Pour les rows hors-viewport ScrollArea, le clip viewport hérité était écrasé → la cellule pouvait peindre partout dans son rect propre, y compris par-dessus header/footer. Fix : `child.set_clip_rect(rect.intersect(ui.clip_rect()))`. (Diagnostic : `examples/scroll_repro.rs` montrait que les patterns externes — TopBottomPanel, allocate_ui, child_ui — fonctionnaient en isolation, donc le bug était au niveau plus bas.)
- [x] Auto-probe SMDI (Master Identify → Slave Identify) après UAC connect : top bar affiche `Sampler : sondage… / OK / non détecté (msg)` + bouton `Probe` pour relancer après ajustement Settings ou hardware
- [x] Slicer footer 2 lignes (info / boutons) ; spinbox `Beats` placé à côté des boutons MIDI ; couleur `BUTTON_MIDI` (cuivre/ambre) plus douce que `ACCENT_YELLOW`
- [x] Tokens design centralisés dans `theme.rs` (`palette` étendu BUTTON_PRIMARY/MIDI/DANGER/BUSY, `size`, `font`, `space`)
- [x] Slicer cells selectors alignés sur la sémantique Python : `selected: Vec<bool>` (vert, à garder pour export, mode filter) + `marked: Vec<bool>` (rouge, à supprimer) mutuellement exclusifs ; click gauche/drag → selected, click droit/drag → marked
- [x] Slicer : click sur la waveform (hors d'un onset) → preview de la slice en oneshot ; pas de playhead pendant un preview ; cellule highlight au contour jaune
- [x] Upload : import drag-IN d'archives `.zip` / `.tar.gz` / `.tgz` / `.tar` (module `archive.rs` + crates `zip`, `flate2`, `tar`) — extraction dans `%TEMP%\a3000_extracted\<stem>_<pid>_<nano>\`, walk récursif, ajout des `.wav` à la queue
- [x] Module `audio.rs` partagé Slicer ↔ Upload (`Playback::start_loop` / `start_oneshot` + `pcm16_le_to_mono_f32`)
- [x] Upload : preview audio par item — bouton play/stop par row (icône peinte triangle/carré pour centrage pixel-perfect, glyphs Unicode `▶`/`■` ont des galleys asymétriques) à droite du Sample name ; row highlight jaune pendant la lecture ; oneshot, stop auto à la fin
- [x] Upload : preview audio via click sur le nom du fichier (curseur PointingHand + tooltip Play/Stop) ; colonne Play séparée supprimée
- [x] Fix UI : checkbox tronquée à gauche dans tables Upload/Download — le focus stroke / hover ring d'egui dépasse de ~2 px le box visible et était rogné par le `clip_rect` strict du cell. Fix : `CHECKBOX_LEFT_PAD = 6.0` + `ui.add_space(CHECKBOX_LEFT_PAD)` au début de chaque cell checkbox (header + rows)
- [x] Fix Slicer : conversion 24→16 perdait la stéréo (sample sur A3000 en 1ch). Le Slicer Rust convertissait en mono à `load` puis exportait depuis `audio.mono`. Python (`engine.py:174`) garde l'audio original et n'utilise mono que pour la détection d'onsets. Fix : ajout de `AudioData.pcm16_le: Vec<u8>` (interleaved bytes du source), export depuis `pcm16_le[start*ch*2..end*ch*2]` avec `channels: audio.channels` ; `delete_marked` rebuild les deux buffers en parallèle
- [x] Preview audio stéréo : `Playback` accepte maintenant un buffer interleaved f32 + `src_channels` ; routage mono→repli sur tous les canaux device / stéréo + device ≥2ch → L=ch0 R=ch1 / stéréo + device mono → downmix (L+R)/2. Slicer Loop, preview slice et Upload preview passent par `pcm16_le_to_interleaved_f32` / `pcm16_le_bytes_to_interleaved_f32`

## Moyen terme (Sprint)

## Long terme (Backlog)
- [ ] Distribuer le .exe (Python) avec un installeur signé (élimine l'avertissement Defender)
- [ ] Support multi-sampler (A4000, A5000) si demandes utilisateurs
- [ ] CI GitHub Actions : cargo test + cargo clippy sur les 3 crates Rust

## Bugs à corriger
- (aucun connu côté Python à ce stade)
