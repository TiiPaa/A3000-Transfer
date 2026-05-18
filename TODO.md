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
- [x] Slicer : click droit sur la waveform → près d'un onset (< 5 px) = supprime la séparation (slice idx-1 absorbe idx) / ailleurs = ajoute une séparation à la position du clic (split la slice). Mirror Python view.py:407-415. L'onset 0 (début audio) ne peut pas être supprimé. Indices `dragging_onset` / `current_onset` / `previewing_slice` décalés pour rester valides
- [x] Slicer : slider Sensitivity (0.2 → 3.0, défaut 1.0) sur ligne 1 du footer (à droite des compteurs). Re-détection en temps réel pendant le drag (`resp.changed()`) ; wipe les sélections/marquages utilisateur car indices invalidés. Mirror Python view.py:209-212
- [x] Slicer Remix : pipeline 3 étages (Shuffle → Repeat → Stutter) avec intensités indépendantes (sliders Sh/Rp/St). Sequence rendue en strip colorée sous la waveform (HSV par slice_idx, angle d'or). Régénération temps réel sur changement, déterministe via `remix_seed` (ChaCha8). Auto-restart de la lecture si Play actif. Sync remix après modifs onsets (delete_marked / redetect / add_onset / delete_onset).
- [x] Slicer Remix : 4 modes de Shuffle (Random / Beat-aligned via `n_beats` / Pair-swap / Block-reorder taille 2 ou 4 selon intensity) sélectionnables via ComboBox.
- [x] Slicer Remix : preview audio en boucle via `Playback::start_loop` sur le buffer rendu (interleaved f32, préserve la stéréo) + playhead orange sur la strip. Bouton Play/Stop dédié (vert/orange).
- [x] Slicer Remix : drag-out MIDI via OLE (`generate_midi_sequence` ajouté dans `a3000-core::midi` — note = `C2 + slice_idx`, tempo BPM = `n_beats × 60 / total_remix_duration`, filename tag `_remix_S30_R40_T15_<mode>.mid`)
- [x] Upload : bouton × peint manuellement via `Painter` (rect 17×17 centré exactement dans la cell de 40 px, glyph 14 px centré via `Align2::CENTER_CENTER`). Cause racine du bug "tronqué à gauche" : le `Layout::left_to_right` du cell helper plaçait le Button d'egui à `cell.left()`, et son focus stroke (qui déborde de 1-2 px) tombait à `x < cell.left()` → coupé par le `clip_rect` strict du cell. Le painter direct contourne le problème (interact + rect + text gérés à la main).
- [x] Upload : click droit sur le nom du fichier → menu contextuel "Send to Slicer" + "Remove". `UploadState.request_send_to_slicer: Option<PathBuf>` drainé par `app::poll_upload_send_to_slicer()` qui charge le WAV dans le Slicer + bascule sur Tab::Slicer.
- [x] Slicer : bouton "Beat slice" sur footer ligne 1 (à côté de Sensitivity) + ComboBox subdivisions `1/2/3/4/6/8/16 per beat` → remplace les onsets par `n_beats × slices_per_beat` positions équidistantes. Utile pour loops en grille rythmique stricte.
- [x] Slicer Remix : bouton "Reset" à côté de ↻ → reset les 3 intensités à 0 + mode Random, préserve le seed.
- [x] Slicer Remix : layout `horizontal_wrapped` avec sections distinctes (Shuffle / Repeat / Stutter en vert gras + séparateurs entre les groupes) → lisibilité claire de quel slider contrôle quoi. Drag MIDI Remix inline (right_to_left ne marche pas en wrapped).
- [x] Slicer Remix Stutter : refactor — le slider = **probabilité** qu'une slice soit stutterée (plus on pousse, plus on a de stutters dans la loop). Le K (nb de retriggers par slice stutterée) = aléatoire dans `K_CHOICES = [2, 3, 4, 6, 8, 12, 16]` → multiples de 2 + triolets, pas de 5/7 (découpes non-musicales).
- [x] Slicer Remix Play : playhead orange sur la waveform originale **suit la slice en cours de lecture** (= `onsets[slice_idx] + offset_dans_step`, pas la position linéaire du buffer rendu). + highlight jaune de la cell active. Implémentation : helper `state.remix_current_play_info() -> Option<(step_idx, slice_idx, sample_offset)>` qui convertit `pb.position_fraction() × total_buffer` → step en cours + offset interne.
- [x] Fix Drag MIDI Remix : le glyph `↓` ne rend pas dans la police default d'egui → carré blanc dans le bouton. Retiré, bouton devient `"Drag MIDI Remix"` aligné sur le pattern du bouton MIDI standard du slicer.
- [x] Slicer Remix : regen par étage du pipeline. Refactor : 3 seeds indépendants (`shuffle_seed`, `repeat_seed`, `stutter_seed`) + 3 RNG ChaCha8 séparés dans `regenerate_remix`. UI : bouton ↻ après chaque slider (re-roll uniquement cet étage) + bouton ↻ All (re-roll les 3).
- [x] Slicer Remix : layout vertical 4 lignes (header + Shuffle + Repeat + Stutter). Sliders alignés verticalement, labels `Shuffle`/`Repeat`/`Stutter` largeur fixe 60 px. ComboBox shuffle mode placée APRÈS le slider Shuffle pour que les sliders restent alignés. REMIX_CONTROLS_H bumpé de 32 → 120.
- [x] Slicer : selection range via Shift+drag sur la waveform (overlay bleu translucide). Drag des extrémités gauche/droite pour resize (cursor ResizeHorizontal). Ctrl+click sur une slice pour étendre la sélection avec cette slice. Échap ou bouton "Clear sel" pour reset. Touche Échap aussi.
- [x] Slicer : bouton Crop → tronque audio à la sélection (mono + pcm16_le + onsets shiftés, push_undo). Bouton Clear sel à côté.
- [x] Slicer : bouton Loop devient "Loop sel" quand sélection active → playback boucle uniquement la plage sélectionnée. Playhead orange parcourt UNIQUEMENT la plage (mapping `sel.start + frac × sel_len`).
- [x] Slicer : restart automatique du Playback (loop) quand la sélection change (drag end, edge resize, Ctrl+click extend, Clear sel). Au release du drag, pas pendant — évite les clicks audio.
- [x] Slicer Undo : pile snapshots audio + onsets + marked + selected (max 20 entrées) avec push_undo() avant chaque opération destructive (delete_marked / redetect / slice_by_beats / add_onset / delete_onset / crop / warp). Bouton Undo + Ctrl+Z keyboard shortcut. AudioData impl Clone manuel.
- [x] Slicer Time-stretching : Alt+drag d'un onset → warp time-stretch des 2 slices voisines (ancres gauche/droite fixes, durée totale préservée). Snapshot capturé au press, commit au release. Nouveau module `time_stretch.rs` (5 tests) : `stretch_linear` (resampling lerp, pitch change avec ratio) + `stretch_wsola` (Waveform Similarity Overlap-Add, frame Hann 1024 / hop 256, cross-correlation search ±128, pitch-preserving, fallback Linear si input < 2048 frames). UI ComboBox `[Linear ▾]` / `[WSOLA ▾]` dans footer ligne 1. Restart auto du Playback après commit.
- [x] Fix edge grab impossible en zoom : les `?` early-returns dans `edge_hover` et `edge_hover_for_cursor` rejetaient toute la fonction dès qu'UNE extrémité était hors view. Chaque edge maintenant testée indépendamment → on peut grabber l'edge visible même si l'autre est hors view.
- [ ] **REPRENDRE ICI** Slicer : refonte cohérence UI (devient bordélique avec : top bar, cells strip, waveform, remix strip, remix controls 4 lignes, footer ligne 1 stats + Sensitivity + Beat slice + Stretch mode, footer ligne 2 ~9 boutons). Idées : regroupement visuel par fonction, panneau latéral pour les contrôles, sections collapsibles, hiérarchie typographique.
- [ ] Améliorer les algorithmes Remix : design à creuser. Pistes — Shuffle "musical" (preserve groove via beat-grouping correct), Repeat avec patterns rythmiques (pas juste random), Stutter avec accélération/décélération (sweep rythmique), nouveau étage "Reverse" (renverse certaines slices), nouveau étage "Drop" (silence sur certains beats).

## Moyen terme (Sprint)

## Long terme (Backlog)
- [ ] Distribuer le .exe (Python) avec un installeur signé (élimine l'avertissement Defender)
- [ ] Support multi-sampler (A4000, A5000) si demandes utilisateurs
- [ ] CI GitHub Actions : cargo test + cargo clippy sur les 3 crates Rust

## Bugs à corriger
- (aucun connu côté Python à ce stade)
