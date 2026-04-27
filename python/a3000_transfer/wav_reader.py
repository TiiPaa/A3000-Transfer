from __future__ import annotations

import random
import struct
import wave
from pathlib import Path

from .models import WavePayload


class WaveValidationError(Exception):
    pass


def load_wave(path: str | Path) -> WavePayload:
    """Charge un WAV PCM (8, 16 ou 24 bits) et retourne un WavePayload 16 bits.

    Conversion automatique vers 16 bits :
    - 8 bits unsigned → 16 bits signed (passage signé + shift left 8)
    - 24 bits signed → 16 bits signed avec TPDF dither (qualité pro)
    - 16 bits → tel quel
    """
    source = Path(path)
    if not source.exists():
        raise WaveValidationError(f"Fichier introuvable: {source}")

    try:
        with wave.open(str(source), "rb") as wav:
            channels = wav.getnchannels()
            sample_width = wav.getsampwidth()
            sample_rate = wav.getframerate()
            frame_count = wav.getnframes()
            comp_type = wav.getcomptype()
            pcm_data = wav.readframes(frame_count)
    except wave.Error as exc:
        raise WaveValidationError(f"WAV invalide: {exc}") from exc

    if comp_type != "NONE":
        raise WaveValidationError("Seuls les WAV PCM non compressés sont supportés.")
    if channels not in (1, 2):
        raise WaveValidationError(f"Seuls les WAV mono ou stéréo sont acceptés (reçu {channels} canaux).")
    if sample_width not in (1, 2, 3):
        raise WaveValidationError(
            f"Profondeur PCM non supportée: {sample_width * 8} bits (attendu 8, 16 ou 24)."
        )
    if frame_count <= 0 or not pcm_data:
        raise WaveValidationError("Le fichier WAV ne contient pas d'audio exploitable.")

    if sample_width == 1:
        pcm16 = pcm8u_to_pcm16le(pcm_data)
    elif sample_width == 2:
        pcm16 = pcm_data
    else:  # sample_width == 3
        pcm16 = pcm24le_to_pcm16le_tpdf(pcm_data)

    return WavePayload(
        path=str(source),
        channels=channels,
        sample_rate=sample_rate,
        bits_per_sample=16,
        frame_count=frame_count,
        byte_count=len(pcm16),
        pcm_data=pcm16,
    )


def pcm8u_to_pcm16le(pcm8: bytes) -> bytes:
    """8-bit unsigned PCM (0..255, centré sur 128) → 16-bit signed LE."""
    out = bytearray(len(pcm8) * 2)
    for i, b in enumerate(pcm8):
        signed_8 = b - 128
        # Shift left 8 pour mapper -128..127 vers -32768..32512
        signed_16 = signed_8 << 8
        unsigned_16 = signed_16 & 0xFFFF
        out[i * 2] = unsigned_16 & 0xFF
        out[i * 2 + 1] = (unsigned_16 >> 8) & 0xFF
    return bytes(out)


def pcm24le_to_pcm16le_tpdf(pcm24: bytes) -> bytes:
    """24-bit signed PCM LE → 16-bit signed PCM LE avec TPDF dithering.

    TPDF (Triangular Probability Density Function) = somme de deux distributions
    rectangulaires uniformes. Standard pro pour quantization noise shaping :
    décorrèle le bruit de troncature du signal, le rend audible comme un noise
    floor blanc plutôt que comme une distorsion harmonique.
    """
    if len(pcm24) % 3:
        raise WaveValidationError("PCM 24 bits doit être un multiple de 3 octets.")

    n_samples = len(pcm24) // 3
    out = bytearray(n_samples * 2)
    rng = random.Random()  # local instance, plus rapide que le module global

    for i in range(n_samples):
        in_off = i * 3
        b0 = pcm24[in_off]
        b1 = pcm24[in_off + 1]
        b2 = pcm24[in_off + 2]
        # 24-bit signed LE
        v = b0 | (b1 << 8) | (b2 << 16)
        if v & 0x800000:
            v -= 0x1000000

        # TPDF dither : ±256 sur l'échelle 24-bit (= ±1 LSB sur l'échelle 16-bit cible)
        # randint(-128, 128) somme deux fois → distribution triangulaire [-256, 256]
        v += rng.randint(-128, 128) + rng.randint(-128, 128)

        # Troncature 24→16 par shift right 8 (équivalent à diviser par 256)
        v16 = v >> 8
        if v16 > 32767:
            v16 = 32767
        elif v16 < -32768:
            v16 = -32768

        # Pack LE
        v16u = v16 & 0xFFFF
        out[i * 2] = v16u & 0xFF
        out[i * 2 + 1] = (v16u >> 8) & 0xFF

    return bytes(out)
