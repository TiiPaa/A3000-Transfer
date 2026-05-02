from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path

import numpy as np
import soundfile as sf

from .models import WavePayload


class WaveValidationError(Exception):
    pass


@dataclass(slots=True)
class WaveMetadata:
    """Métadonnées d'un WAV sans charger les données PCM. Utilisé par la GUI
    pour afficher format/durée/taille sans le coût de la conversion."""
    channels: int
    sample_rate: int
    bits_per_sample: int
    frame_count: int
    byte_count: int       # taille de la sortie 16-bit après conversion
    duration_s: float


# Mapping subtype soundfile → bits affichés à l'utilisateur
_SUBTYPE_BITS = {
    "PCM_S8": 8,
    "PCM_U8": 8,
    "PCM_16": 16,
    "PCM_24": 24,
    "PCM_32": 32,
    "FLOAT": 32,    # 32-bit IEEE 754 float
    "DOUBLE": 64,   # rare mais soundfile le lit
}


def _validate_info(info) -> int:
    """Retourne les bits effectifs ou raise WaveValidationError."""
    bits = _SUBTYPE_BITS.get(info.subtype)
    if bits is None:
        raise WaveValidationError(
            f"Format PCM non supporté: subtype={info.subtype!r} "
            f"(formats acceptés : 8/16/24/32-bit int, 32-bit float)."
        )
    if info.channels not in (1, 2):
        raise WaveValidationError(
            f"Seuls les WAV mono ou stéréo sont acceptés (reçu {info.channels} canaux)."
        )
    if info.frames <= 0:
        raise WaveValidationError("Le fichier WAV ne contient pas d'audio exploitable.")
    return bits


def peek_wave_metadata(path: str | Path) -> WaveMetadata:
    """Lit uniquement le header du WAV via soundfile.info(). Très rapide,
    indépendant de la taille du fichier."""
    source = Path(path)
    if not source.exists():
        raise WaveValidationError(f"Fichier introuvable: {source}")
    try:
        info = sf.info(str(source))
    except Exception as exc:
        raise WaveValidationError(f"WAV invalide: {exc}") from exc

    bits = _validate_info(info)
    duration = info.frames / info.samplerate if info.samplerate else 0.0
    return WaveMetadata(
        channels=info.channels,
        sample_rate=info.samplerate,
        bits_per_sample=bits,
        frame_count=info.frames,
        byte_count=info.frames * info.channels * 2,  # sortie 16-bit
        duration_s=duration,
    )


def load_wave(path: str | Path) -> WavePayload:
    """Charge un WAV (8/16/24/32-bit PCM ou 32-bit float) et retourne un
    WavePayload 16 bits LE prêt à envoyer au sampler.

    - 16-bit int  → lecture directe, pas de conversion
    - autres      → lecture en float32, dither TPDF, quantification 16-bit
    """
    source = Path(path)
    if not source.exists():
        raise WaveValidationError(f"Fichier introuvable: {source}")

    try:
        info = sf.info(str(source))
    except Exception as exc:
        raise WaveValidationError(f"WAV invalide: {exc}") from exc
    _validate_info(info)

    if info.subtype == "PCM_16":
        # Pas de conversion → lecture directe en int16, déjà LE sur Windows
        data, sr = sf.read(str(source), dtype="int16", always_2d=True)
        pcm_data = data.tobytes()  # interleaved frames, LE
    else:
        # Lit en float32 normalisé [-1.0, 1.0], applique TPDF dither, quantifie
        data, sr = sf.read(str(source), dtype="float32", always_2d=True)
        rng = np.random.default_rng()
        # TPDF dither = somme de 2 distributions uniformes ±0.5 LSB
        # En espace float : ±1/(2*32768) chacune → ±1/32768 total
        d1 = rng.random(data.shape, dtype=np.float32)
        d2 = rng.random(data.shape, dtype=np.float32)
        dithered = data + (d1 - d2) / 32768.0
        pcm16 = np.clip(dithered * 32767.0, -32768.0, 32767.0).astype(np.int16)
        pcm_data = pcm16.tobytes()

    return WavePayload(
        path=str(source),
        channels=info.channels,
        sample_rate=int(sr),
        bits_per_sample=16,
        frame_count=info.frames,
        byte_count=len(pcm_data),
        pcm_data=pcm_data,
    )
