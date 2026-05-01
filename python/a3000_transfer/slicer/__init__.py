"""Slicer embarqué : découpe un long WAV en samples par détection de transients.

Vendoré depuis https://github.com/.../Simpler-Slicer (I:\\Dev\\Simpler-Slicer).
Adapté pour s'embarquer comme onglet dans la GUI A3000-Transfer.
"""
from .view import SlicerView

__all__ = ["SlicerView"]
