# État du projet A3000-transfert

## ✅ RÉSOLU — Transfert SMDI complet fonctionnel (2026-04-27)

**Validation labo** : transfert de `loop01.wav` (159784 frames stéréo 44.1 kHz = 639136 octets PCM BE) vers slot #300 du Yamaha A3000 v0200 via SPTI Win11 + djsvs.sys + Adaptec 2940UW. **313 packets de 2048 octets, EoP propre, 100% transféré.**

### Le bug et le fix

Notre encodeur Sample Header avait **Period sur 4 octets et Pitch Fraction sur 2 octets**, alors que la spec Peavey/OpenSMDI demande **Period sur 3 octets et Pitch Fraction sur 3 octets**. Le total restait 26 octets fixed mais tous les champs après Period étaient décalés d'1 octet → le slave lisait Length à un mauvais offset (≈ 624 mots au lieu de 159784) et committait après ~4 KB.

Toute la conclusion précédente *"limite ferme à 4 KB côté firmware"* était **erronée**. Le firmware fait son boulot correctement, c'est nous qui lui mentions sur la longueur du sample.

### Découverte par comparaison avec le projet jumeau

Le projet `I:\Dev\Sampletrans` (qui marchait déjà) utilise les bons offsets. La comparaison ligne à ligne avec `src/sampletrans/smdi.py` a révélé la différence d'encodage. Sources documentaires utilisées par Sampletrans (qu'on n'avait pas trouvées) : OpenSMDI sur https://www.chn-dev.net/Projects/OpenSMDI/ — confirme le format avec Period sur 3 octets BE.

### Améliorations également appliquées (Sampletrans-inspired)

1. **Data buffer aligné sur 512 octets** dans `send_cdb` (DMA-friendly, évite que storport bounce)
2. **Gestion du message Wait (0x0102)** dans `receive_smdi` : si reçu, sleep 0.5s et re-receive
3. Pas de `multi_bst`, pas de `force_packet_length`, pas de `link_intermediate` activés par défaut — toutes ces options sont restées comme flags optionnels mais inutiles maintenant que le format est correct

## Ce qui marche

- `python -m a3000_transfer scan` — scan SCSI multi-adaptateurs
- `python scripts\probe_inquiry.py` — INQUIRY direct via pass-through
- `python scripts\probe_smdi_identify.py` — handshake Master/Slave Identify
- `python scripts\probe_sample_header_request.py --sample N` — lecture du Sample Header d'un slot
- `python scripts\probe_send_sample.py --commit --sample N` — **transfert WAV complet vers le sampler**

## Limitations observées

- Quelques blocages occasionnels après 2-3 transferts consécutifs (probablement état slave non drainé entre transferts) — à investiguer si gênant
- Délai entre envois recommandé pour stabilité

## Tentatives faites avant le fix correct (gardées en flags pour debug)

| Tentative | Résultat |
|---|---|
| Format BST AL=8 + Channels/BPW | Rejeté sense 0x86 (correct, AL=6 est le bon format) |
| LINK bit (Control byte 0x01) | Strippé par storport, sans effet |
| `IOCTL_SCSI_PASS_THROUGH` non-direct | Pas de différence |
| Pre-encoding tous les Data Packets | Pas de différence (latency Python pas en cause) |
| DPL forcé à 4096+ dans BST | Rejeté (slave annonce 2048 max) |
| Multi-BST sans re-SH | Sense 0x81 (séquence non valide pour la spec) |
| Multi-BST avec re-SH | 156 samples séparés créés (bug Sample Header → mauvais Length lu) |
| Test LUNs 1-7 | Tous miroirs identiques |
| Test BST avec format extension | Aucune différence |

## Ce qui marche

- `python -m a3000_transfer scan` — scan SCSI multi-adaptateurs
- `python scripts\probe_inquiry.py` — INQUIRY direct via pass-through
- `python scripts\probe_smdi_identify.py` — handshake Master/Slave Identify
- `python scripts\probe_sample_header_request.py --sample N` — lecture du Sample Header d'un slot existant
- `python scripts\probe_send_sample.py --commit` — création d'un sample mais limité à 4096 octets

## Ce qui ne marche pas

- Transfert d'un sample WAV complet (> 4 KB) en SMDI single-BST
- Multi-BST avec re-SH crée des samples séparés (le firmware A3000 v0200 ignore le `sample_number` envoyé et alloue un nouveau slot à chaque Sample Header)

## Diagnostic final

La limite à exactement 4096 octets vient de la combinaison **`djsvs.sys` + storport Windows 11** qui ne préserve pas la session bus SCSI entre commandes. Le firmware A3000 v0200 a une SMDI stateful qui commit son buffer interne (1 page = 4 KB) à chaque bus disconnect entre commandes successives.

Sous **ASPI legacy (Win9x/XP)**, le driver Adaptec officiel maintenait probablement le bus connecté entre commandes successives (LINK bit honoré au niveau bus, pas de bus free entre SEND et RECEIVE). C'est ce qui permettait à des softs comme Awave Studio, SoundForge, TWE de transférer des samples complets — pas un secret de SMDI, juste une différence de gestion bus.

Sous **Windows 11 + storport + djsvs.sys**, chaque IOCTL = bus arbitration → command → bus free. Le LINK bit qu'on set dans le CDB Control byte est strippé par storport.

## Tentatives faites (toutes négatives)

| Tentative | Résultat |
|---|---|
| Format BST AL=6 (mirror BSTA) | Accepté mais EoP à 4096 |
| Format BST AL=8 + Channels/BPW | Rejeté sense 0x86 |
| Format BST AL=8 + padding zéro | Rejeté sense 0x86 |
| LINK bit (Control byte 0x01) sur tous les CDBs | Aucun changement (strippé) |
| `IOCTL_SCSI_PASS_THROUGH_EX` (non-direct) | Aucun changement |
| Pre-encoding tous les Data Packets | Aucun changement (latency Python pas en cause) |
| DPL forcé à 4096+ dans BST | Rejeté silencieusement (sense 0x81) |
| Multi-BST sans re-SH | Sense 0x81 sur 2e BST |
| Multi-BST avec re-SH | 156 samples séparés créés (sample_number ignoré) |
| Test LUNs 1-7 | Tous miroirs identiques de LUN 0 |
| Différents sample_number cibles | Comportement identique (slave alloue son propre slot) |
| Delete Sample SMDI (0x0124) | Silencieusement ignoré |

## Capabilities adaptateur (`probe_adapter_caps.py 1`)

```
MaximumTransferLength: 4 GB (illimité)
TrueMaxTransfer      : 64 KB (16 pages × 4 KB)
CommandQueueing      : True
AdapterUsesPio       : True
SrbType              : 0 (SCSI_REQUEST_BLOCK legacy)
BusType              : Scsi
```

→ La limite des 4 KB **ne vient pas** de Windows/storport. L'adapter accepte 64 KB par command. Confirmé : c'est l'interaction firmware ↔ bus management qui est en cause.

## Découvertes protocole Yamaha A3000

Documentées en détail dans la mémoire `protocol_yamaha_quirks.md`. En résumé :

- **Period field encodé en `period_ns × 256`** (8 bits fractionnaires zéros), pas pure ns comme la spec Peavey
- **Sample Number = 24-bit BE** partout (pas 16-bit comme suggère l'art ASCII)
- **Pitch Integer = 1 octet** (note MIDI), Pitch Fraction = 2 octets BE
- **Trailing null** ajouté après le sample name dans la réponse Yamaha
- **DPL max = 2048** annoncé dans BSTA, strict (DPL > 2048 → sense 0x81)
- **Sense 0x81** sur Sample Header Request d'un slot vide (au lieu de Reject 0x0020/0x0002)
- **Sample Number ignoré** côté slave : chaque Sample Header crée un nouveau sample auto-nommé
- **`sample_number=0` à 6** = samples factory ROM (sine wave, saw up, triangle, square, pulse 1/2/3)

## Hardware testé

- PC : Windows 11 Pro 24H2 x64
- Carte SCSI 1 : **Adaptec AHA-2940UW** (driver community `djsvs.sys` de savagetaylor.com — port Win98 vers x64)
- Carte SCSI 2 (à tester) : **LSI Logic SYM8952U** (chipset Symbios 53C895, driver Microsoft inbox `symc8xx.sys` ou `symmpi.sys`)
- Sampler : Yamaha A3000 firmware v0200, ID SCSI=0, BUS=0, 8 LUNs (LUN 0 actif, autres miroirs)

## Prochaine étape (révisée 2026-04-27)

**Priorité 1** : étudier `I:\Dev\Sampletrans` qui marche déjà sur le même hardware.
- Lire `src/sampletrans/scsi_win.py` (couche SCSI passthrough avec alignement 512)
- Lire `src/sampletrans/smdi.py` (codec + client SMDI avec gestion Wait)
- Comparer ligne à ligne avec notre `a3000_transfer/scsi_passthrough.py` et `transfer.py`
- Identifier précisément quelle différence débloque le transfert

**Décision stratégique à prendre** :
- Soit **adopter Sampletrans** comme la solution principale et archiver A3000-transfert (plus simple)
- Soit **fusionner** les deux projets (récupérer ce qui marche dans Sampletrans, garder nos probes utiles)

**Priorité 2 (si pertinent)** : tester avec LSI SYM8952U pour voir si on peut résoudre les "blocages occasionnels après 2-3 transferts" mentionnés par l'user.

## Prochaine étape précédente (LSI SYM8952U) — probablement obsolète

C'est la piste la plus prometteuse. Si le driver Microsoft natif `symc8xx.sys` gère le bus différemment de `djsvs.sys`, on pourrait débloquer le transfert complet.

### Procédure

1. Power off complet le PC
2. Remplacer (ou ajouter à côté) la 2940UW par la LSI SYM8952U
3. Vérifier la connectique SCSI : la SYM8952U a typiquement un connecteur 68-pin Wide externe — le A3000 a un **50-pin half-pitch externe**, donc adaptateur 68→50 pin probablement nécessaire
4. Allumer le sampler **avant** le PC (règle SCSI standard)
5. Boot Windows : il devrait installer automatiquement le driver inbox

### Tests à lancer

```powershell
python scripts\probe_scsi.py
python scripts\probe_adapter_caps.py <HA>
python scripts\probe_smdi_identify.py --ha <HA>
python scripts\probe_send_sample.py --commit --ha <HA> --sample 200 --verbose 2>&1 > out.txt
```

### Ce qu'on regarde dans la sortie

- Driver utilisé (devrait être `symc8xx.sys` ou similaire, pas `djsvs.sys`)
- `SrbType` : si **= 1** (STORAGE_REQUEST_BLOCK moderne), on pourra tester `IOCTL_SCSI_PASS_THROUGH_EX` qu'on n'a pas pu utiliser sous djsvs.sys
- Sur le test transfert : si on dépasse 4096 octets dans un seul BST → débloqué !

### Si la SYM8952U ne change rien

→ La limite est confirmée firmware-side. Solutions : BlueSCSI/ZuluSCSI (recommandé) ou MIDI/SDS via le projet jumeau `A3000-editor`.

## Sources / Références

- Spec SMDI Peavey 1992 v0.03 : `I:\Dev\A3000-editor\docs\smdi_spec_pages16-41.txt`
- Manuel A3000 (PDF + extraction texte) : `I:\Dev\A3000-editor\docs\A3000E.pdf` et `I:\Dev\A3000-transfert\docs\a3000_manual.txt`
- Manuel A3000 v2 supplément : `I:\Dev\A3000-editor\docs\A3000V2E.pdf`
- Sample Microsoft SPTI : `I:\Dev\A3000-transfert\docs\spti_microsoft_sample.c` et `.h` (clonés depuis github.com/microsoft/windows-driver-samples/storage/tools/spti)
- Origine du driver djsvs.sys : https://www.savagetaylor.com/2018/02/11/scsi-on-windows-10-adaptec-aha-2940-adaptec-29xx-ultra-or-aic-7870-adaptec-78xx/
- Pdftotext (poppler) : installé via `winget install --id oschwartz10612.Poppler` dans `C:\Users\baboost\AppData\Local\Microsoft\WinGet\Packages\oschwartz10612.Poppler_*\poppler-25.07.0\Library\bin`

## Voies de résolution alternatives (si SPTI continue de coincer)

1. **BlueSCSI v2 / ZuluSCSI** (~30-80€) : émulation SCSI disque via SD card, contournement complet du protocole SMDI. Le sampler lit les WAV via son menu IMPORT (manuel p272). Solution recommandée.
2. **SDS via MIDI** (projet jumeau `A3000-editor`) : protocole documenté p353+ du manuel, lent (~50 KB/s) mais garanti.
3. **Driver kernel custom** : techniquement possible mais disproportionné (signature Microsoft Dev Portal ~300€/an + 3-6 mois de dev kernel).
