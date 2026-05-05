//! Tests de parité bit-à-bit avec les goldens Python.
//!
//! Source des fixtures : `docs/conversion/oracles/golden/smdi/*.json`
//! Générées via `python docs/conversion/oracles/generate_oracles.py --smdi`.
//!
//! Si un test échoue : la divergence est probablement un bug du port Rust,
//! sauf si une décision a été actée dans `docs/conversion/DECISIONS.md`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![allow(clippy::cast_possible_truncation)]

use std::fs;
use std::path::PathBuf;

use a3000_core::smdi::{
    encode_abort_procedure, encode_begin_sample_transfer, encode_data_packet,
    encode_delete_sample, encode_end_of_procedure, encode_sample_header,
    encode_sample_header_request, encode_send_next_packet, master_identify_message,
    SampleHeader,
};
use serde_json::Value;

fn golden_dir() -> PathBuf {
    // Tests run from rust/crates/a3000-core/, goldens in ../../../docs/conversion/oracles/golden/smdi/
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../docs/conversion/oracles/golden/smdi")
}

fn load_json(name: &str) -> Value {
    let path = golden_dir().join(name);
    let raw = fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!("Cannot read golden {} : {e}. Did you run generate_oracles.py ?", path.display())
    });
    serde_json::from_str(&raw).expect("invalid golden JSON")
}

fn expect_hex(case: &Value) -> Vec<u8> {
    let h = case["expected_hex"].as_str().expect("expected_hex missing");
    hex_decode(h)
}

fn hex_decode(h: &str) -> Vec<u8> {
    (0..h.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&h[i..i + 2], 16).expect("invalid hex"))
        .collect()
}

#[test]
fn oracle_encode_sample_header_request() {
    let cases = load_json("encode_sample_header_request.json");
    for case in cases.as_array().expect("array") {
        let sn = case["sample_number"].as_u64().unwrap() as u32;
        let expected = expect_hex(case);
        let actual = encode_sample_header_request(sn).expect("encode");
        assert_eq!(
            actual, expected,
            "encode_sample_header_request({}) mismatch", sn
        );
    }
}

#[test]
fn oracle_master_identify() {
    let case = load_json("master_identify.json");
    let expected = expect_hex(&case);
    let actual = master_identify_message();
    assert_eq!(actual, expected);
}

#[test]
fn oracle_encode_begin_sample_transfer() {
    let cases = load_json("encode_begin_sample_transfer.json");
    for case in cases.as_array().expect("array") {
        let sn = case["sample_number"].as_u64().unwrap() as u32;
        let dpl = case["data_packet_length"].as_u64().unwrap() as u32;
        let expected = expect_hex(case);
        let actual = encode_begin_sample_transfer(sn, dpl).expect("encode");
        assert_eq!(actual, expected, "encode_begin_sample_transfer({sn}, {dpl}) mismatch");
    }
}

#[test]
fn oracle_encode_send_next_packet() {
    let cases = load_json("encode_send_next_packet.json");
    for case in cases.as_array().expect("array") {
        let pn = case["packet_number"].as_u64().unwrap() as u32;
        let expected = expect_hex(case);
        let actual = encode_send_next_packet(pn).expect("encode");
        assert_eq!(actual, expected, "encode_send_next_packet({pn}) mismatch");
    }
}

#[test]
fn oracle_encode_data_packet() {
    let cases = load_json("encode_data_packet.json");
    for case in cases.as_array().expect("array") {
        let pn = case["packet_number"].as_u64().unwrap() as u32;
        let data = hex_decode(case["data_hex"].as_str().unwrap());
        let expected = expect_hex(case);
        let actual = encode_data_packet(pn, &data).expect("encode");
        assert_eq!(actual, expected, "encode_data_packet({pn}, {} bytes) mismatch", data.len());
    }
}

#[test]
fn oracle_end_of_procedure() {
    let case = load_json("end_of_procedure.json");
    assert_eq!(encode_end_of_procedure(), expect_hex(&case));
}

#[test]
fn oracle_abort_procedure() {
    let case = load_json("abort_procedure.json");
    assert_eq!(encode_abort_procedure(), expect_hex(&case));
}

#[test]
fn oracle_encode_delete_sample() {
    let cases = load_json("encode_delete_sample.json");
    for case in cases.as_array().expect("array") {
        let sn = case["sample_number"].as_u64().unwrap() as u32;
        let expected = expect_hex(case);
        let actual = encode_delete_sample(sn).expect("encode");
        assert_eq!(actual, expected, "encode_delete_sample({sn}) mismatch");
    }
}

#[test]
fn oracle_encode_sample_header() {
    let cases = load_json("encode_sample_header.json");
    for case in cases.as_array().expect("array") {
        let inp = &case["input"];
        let h = SampleHeader {
            sample_number: inp["sample_number"].as_u64().unwrap() as u32,
            bits_per_word: inp["bits_per_word"].as_u64().unwrap() as u8,
            channels: inp["channels"].as_u64().unwrap() as u8,
            sample_period_ns: inp["sample_period_ns"].as_u64().unwrap() as u32,
            sample_length_words: inp["sample_length_words"].as_u64().unwrap() as u32,
            loop_start: inp["loop_start"].as_u64().unwrap() as u32,
            loop_end: inp["loop_end"].as_u64().unwrap() as u32,
            loop_control: inp["loop_control"].as_u64().unwrap() as u8,
            pitch_integer: inp["pitch_integer"].as_u64().unwrap() as u8,
            pitch_fraction: inp["pitch_fraction"].as_u64().unwrap() as u32,
            name: inp["name"].as_str().unwrap().to_string(),
        };
        let expected = expect_hex(case);
        let actual = encode_sample_header(&h).expect("encode");
        assert_eq!(actual, expected, "encode_sample_header({}) mismatch", h.name);
    }
}
