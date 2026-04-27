# A3000 Transfer MVP

Prototype d’application Windows pour transférer des fichiers WAV vers un Yamaha A3000 via SCSI.

## Direction actuelle

Le MVP actif est maintenant **en Python** dans `python/`, parce que c’est la voie la plus rapide pour valider :

- le scan **SPTI** sous Windows 11
- la détection du Yamaha sur le bus SCSI
- la validation des WAV avant d’attaquer le vrai transfert SMDI

Le prototype C# reste dans `src/` comme base de travail antérieure, mais la suite du MVP part côté Python.

## Objectif v1

Créer un utilitaire simple qui permet de :

- détecter un hôte SCSI compatible (ex. Adaptec 2940U)
- lister les cibles SCSI visibles
- identifier un sampler Yamaha A3000/A4000/A5000 compatible SMDI
- charger un fichier WAV PCM
- convertir le WAV si nécessaire (mono/stéréo, 16 bits, fréquence)
- envoyer un sample au sampler
- afficher des logs exploitables

## Hypothèses techniques

- Windows 11 x64
- carte SCSI déjà reconnue par le système
- le sampler est visible sur le bus SCSI
- le transfert vise désormais en priorité **SPTI** (voie native Windows), sans dépendance à ASPI

## Choix de conception

Je pars sur un MVP en C# avec une séparation nette :

- `A3000Transfer.Core` : logique métier, parsing WAV, orchestration
- `A3000Transfer.Windows` : accès Windows, interop SPTI/SCSI, CLI de test

Je commence volontairement par une version outillage / console :

- plus rapide à déboguer
- meilleure visibilité sur les échanges SCSI
- GUI possible ensuite, une fois le chemin de transfert validé

## Ce qui est dur

Le point le plus risqué n’est pas l’UI mais :

- la détection fiable du sampler
- l’accès fiable au bus SCSI via l’API Windows native
- l’implémentation exacte du flux SMDI attendu par Yamaha

## Roadmap courte

1. Détection carte + bus SCSI
2. Liste des devices et identification Yamaha
3. Envoi d’un WAV mono simple
4. Logs et erreurs propres
5. Interface graphique légère si la pile de transfert est stable

## Statut

Le projet contient maintenant un premier scan **SPTI/SCSI** réel :

- ouverture des adaptateurs `\\.\ScsiN:`
- appel à `IOCTL_SCSI_GET_INQUIRY_DATA`
- parsing des bus / targets / LUNs détectés
- lecture des champs `vendor / product / revision`

## Tester sous Windows

### Option recommandée, MVP Python

```powershell
cd a3000-transfer\python
python -m a3000_transfer scan
```

Documentation Python détaillée : `python/README.md`

### Option historique, prototype C#

### 1. Pré-requis

- .NET 8 SDK installé
- carte SCSI fonctionnelle
- Yamaha A3000 branché, avec ID SCSI valide et terminaison correcte
- si besoin, terminal lancé en administrateur

### 2. Build

```powershell
cd a3000-transfer
dotnet build src/A3000Transfer.Windows/A3000Transfer.Windows.csproj
```

### 3. Scan des périphériques

```powershell
dotnet run --project src/A3000Transfer.Windows/A3000Transfer.Windows.csproj -- scan
```

Si tout se passe bien, l’app doit afficher des lignes du genre :

```text
HA0 ID5 LUN0 YAMAHA A3000 1.00
```

### 4. Limite actuelle

Le `scan` passe maintenant par **SPTI natif Windows**, donc sans installation ASPI.
L’envoi de sample (`send`) reste encore à implémenter côté protocole SMDI/Yamaha.
