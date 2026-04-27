# MVP plan

## V1 stricte

La v1 ne cherche pas à faire un éditeur complet Yamaha.

Elle vise uniquement :

- sélection d’un device SCSI cible
- import d’un WAV PCM
- normalisation du format si besoin
- transfert d’un sample unitaire
- log détaillé

## Hors scope pour l’instant

- édition de programmes/keymaps Yamaha
- dump complet de mémoire sampler
- browsing de volumes/disques Yamaha
- batch avancé avec mapping multisample
- packaging/installateur

## Première validation terrain

Pour déclarer le prototype utile, il faut réussir ce scénario :

1. la carte 2940U est vue par l’app
2. le Yamaha A3000 apparaît avec son ID SCSI
3. un WAV court est envoyé sans erreur
4. le sample est visible et jouable sur le sampler

## Risques

- protocole SMDI Yamaha partiellement documenté
- backend ASPI variable selon DLL utilisée
- comportement différent selon câble, terminaison, ID SCSI et ordre d’allumage
