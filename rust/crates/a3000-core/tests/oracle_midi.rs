//! Oracle MIDI : compare bytes SMF Rust vs Python (mido) bit-à-bit.
//!
//! Source des fixtures : `docs/conversion/oracles/golden/midi/*.mid`
//! Générées via `python generate_oracles.py --midi`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]

use std::fs;
use std::path::PathBuf;

use a3000_core::midi::generate_midi;
use serde_json::Value;

fn manifest_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../docs/conversion/oracles")
}

fn load_manifest() -> Value {
    let path = manifest_root().join("golden/midi/manifest.json");
    let raw = fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!("Cannot read {} : {e}. Run generate_oracles.py --midi.", path.display())
    });
    serde_json::from_str(&raw).expect("invalid manifest JSON")
}

#[test]
fn oracle_midi_bit_exact() {
    let manifest = load_manifest();
    let cases = manifest["cases"].as_array().expect("array");
    for case in cases {
        let onsets: Vec<i64> = case["onsets"].as_array().unwrap().iter()
            .map(|v| v.as_i64().unwrap()).collect();
        let total_samples = case["total_samples"].as_u64().unwrap();
        let sr = case["sample_rate"].as_u64().unwrap() as u32;
        let n_beats = case["n_beats"].as_u64().unwrap() as u32;
        let track_name = case["track_name"].as_str().unwrap();
        let fname = case["midi_filename"].as_str().unwrap();

        let actual = generate_midi(&onsets, total_samples, sr, n_beats, track_name)
            .unwrap_or_else(|e| panic!("generate_midi failed on {fname}: {e}"));

        let expected_path = manifest_root().join("golden/midi").join(fname);
        let expected = fs::read(&expected_path).expect("read golden bytes");

        if actual != expected {
            // Diagnostic utile : afficher les premières divergences
            let max_show = 64.min(actual.len()).min(expected.len());
            eprintln!(
                "MIDI mismatch on {fname}\nactual ({} bytes)  : {:02X?}\nexpected ({} bytes): {:02X?}",
                actual.len(),
                &actual[..max_show],
                expected.len(),
                &expected[..max_show],
            );
        }
        assert_eq!(actual, expected, "MIDI bytes mismatch on {fname}");
    }
}
