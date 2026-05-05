//! Lecture WAV multi-format avec conversion vers PCM 16-bit + dither TPDF.
//!
//! Référence Python : `python/a3000_transfer/wav_reader.py`
//! Lib : `symphonia` pour le decode multi-format, `mt19937` pour le PRNG du dither
//! (matche le `random.Random` Python pour reproductibilité oracle).

// TODO Phase 1 : peek_wave_metadata + load_wave + tests d'oracle bit-à-bit
