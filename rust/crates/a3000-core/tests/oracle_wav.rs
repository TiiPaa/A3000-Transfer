//! Oracle WAV : compare metadata + PCM bytes contre les goldens Python.
//!
//! Stratégie (cf. `docs/conversion/DECISIONS.md`) :
//! - PCM_16 input → bit-à-bit
//! - autres formats → tolérance |rust - no_dither| ≤ 1 LSB
//!
//! Les WAV de test sont dans `docs/conversion/oracles/inputs/wavs/`,
//! générés par `python generate_oracles.py --wav`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]

use std::fs;
use std::path::PathBuf;

use a3000_core::wav::{load_wave, peek_wave_metadata};
use serde_json::Value;

fn manifest_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../docs/conversion/oracles")
}

fn load_manifest() -> Value {
    let path = manifest_root().join("golden/wav/manifest.json");
    let raw = fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!("Cannot read {} : {e}. Run generate_oracles.py --wav.", path.display())
    });
    serde_json::from_str(&raw).expect("invalid manifest JSON")
}

#[test]
fn oracle_peek_metadata_all_formats() {
    let manifest = load_manifest();
    let cases = manifest["cases"].as_array().expect("array");
    for case in cases {
        let input = case["input_filename"].as_str().unwrap();
        let path = manifest_root().join("inputs/wavs").join(input);
        let meta = peek_wave_metadata(&path)
            .unwrap_or_else(|e| panic!("peek failed on {input}: {e}"));
        let exp = &case["expected_metadata"];
        assert_eq!(u64::from(meta.channels), exp["channels"].as_u64().unwrap(),
                   "channels mismatch on {input}");
        assert_eq!(u64::from(meta.sample_rate), exp["sample_rate"].as_u64().unwrap(),
                   "sample_rate mismatch on {input}");
        assert_eq!(u64::from(meta.bits_per_sample), exp["bits_per_sample"].as_u64().unwrap(),
                   "bits_per_sample mismatch on {input}");
        assert_eq!(meta.frame_count, exp["frame_count"].as_u64().unwrap(),
                   "frame_count mismatch on {input}");
        assert_eq!(meta.byte_count, exp["byte_count"].as_u64().unwrap(),
                   "byte_count mismatch on {input}");
    }
}

#[test]
fn oracle_load_pcm16_bit_exact() {
    let manifest = load_manifest();
    let cases = manifest["cases"].as_array().expect("array");
    for case in cases {
        if case["subtype"].as_str() != Some("PCM_16") {
            continue;
        }
        let input = case["input_filename"].as_str().unwrap();
        let path = manifest_root().join("inputs/wavs").join(input);
        let payload = load_wave(&path)
            .unwrap_or_else(|e| panic!("load_wave failed on {input}: {e}"));
        let expected_path = manifest_root().join("golden/wav").join(case["pcm16_filename"].as_str().unwrap());
        let expected = fs::read(&expected_path).expect("read golden bytes");
        assert_eq!(
            payload.pcm_data, expected,
            "PCM_16 bit-à-bit mismatch on {input}"
        );
    }
}

#[test]
fn oracle_load_dithered_within_1_lsb() {
    let manifest = load_manifest();
    let cases = manifest["cases"].as_array().expect("array");
    for case in cases {
        if case["subtype"].as_str() == Some("PCM_16") {
            continue;
        }
        let input = case["input_filename"].as_str().unwrap();
        let path = manifest_root().join("inputs/wavs").join(input);
        let payload = load_wave(&path)
            .unwrap_or_else(|e| panic!("load_wave failed on {input}: {e}"));

        let nd_filename = case["no_dither_filename"].as_str().expect("no_dither_filename present");
        let nd_path = manifest_root().join("golden/wav").join(nd_filename);
        let no_dither_bytes = fs::read(&nd_path).expect("read no_dither bytes");

        assert_eq!(
            payload.pcm_data.len(),
            no_dither_bytes.len(),
            "byte count mismatch on {input}"
        );

        // Compare i16 samples avec tolérance ±1 LSB
        let rust_samples: Vec<i16> = payload.pcm_data
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]))
            .collect();
        let nd_samples: Vec<i16> = no_dither_bytes
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]))
            .collect();

        let mut max_diff = 0i32;
        for (i, (&r, &nd)) in rust_samples.iter().zip(&nd_samples).enumerate() {
            let diff = (i32::from(r) - i32::from(nd)).abs();
            if diff > max_diff { max_diff = diff; }
            assert!(
                diff <= 1,
                "{input} sample[{i}] : rust={r} no_dither={nd} diff={diff} > 1 LSB"
            );
        }
        eprintln!("[oracle_wav] {input} OK (max_diff={max_diff} LSB)");
    }
}
