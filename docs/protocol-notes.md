# Protocol notes

## Ce qu’on sait

- le Yamaha A3000 peut échanger des samples via SCSI dans un workflow de type SMDI
- la carte Adaptec 2940U peut encore fonctionner sous Windows 11 avec des drivers communautaires selon plusieurs retours terrain
- les vieux logiciels de transfert s’appuient souvent sur `WNASPI32.DLL`

## Ce qu’il faut confirmer en labo

- méthode la plus fiable pour détecter le device sur le bus
- signature INQUIRY exacte renvoyée par le Yamaha A3000
- séquence de commandes SMDI exacte acceptée par l’appareil
- gestion du mono/stéréo, sample rate, nom du sample, accusé de réception

## Approche retenue

1. faire d’abord une détection SCSI propre
2. journaliser les réponses INQUIRY
3. implémenter un envoi minimal de sample court
4. ne pas élargir à l’édition de programmes avant d’avoir un transfert stable
