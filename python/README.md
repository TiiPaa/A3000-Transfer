# A3000 Transfer Python MVP

MVP Python pour explorer un Yamaha A3000 via **SPTI natif Windows**.

## Ce que fait déjà ce MVP

- scan des adaptateurs `\\.\\ScsiN:`
- appel à `IOCTL_SCSI_GET_INQUIRY_DATA`
- parsing des buses, targets et LUNs
- extraction de `vendor / product / revision`
- validation d’un fichier WAV 16 bits PCM pour préparer la suite

## Ce que ça ne fait pas encore

- transfert SMDI réel vers le Yamaha
- création automatique d’un programme / keymap côté sampler
- UI graphique

## Pré-requis

- Windows 11
- Python 3.11+
- terminal PowerShell ou CMD, idéalement **en administrateur**
- carte SCSI fonctionnelle et visible par Windows
- Yamaha A3000 branché, avec ID SCSI valide et terminaison correcte

## Lancer sans installation

Depuis ce dossier :

```powershell
cd a3000-transfer\python
python -m a3000_transfer scan
```

## Sortie attendue

```text
HA0 BUS0 ID5 LUN0 YAMAHA A3000 1.00
```

## Sortie JSON

```powershell
python -m a3000_transfer scan --json
```

## Validation d’un WAV

```powershell
python -m a3000_transfer send --wave C:\samples\kick.wav --ha 0 --bus 0 --target 5 --lun 0
```

Pour l’instant, cette commande :
- vérifie que la cible existe
- lit le WAV
- valide qu’il est en PCM 16 bits mono/stéréo
- prépare la prochaine étape

Le transfert SMDI réel reste à coder.
