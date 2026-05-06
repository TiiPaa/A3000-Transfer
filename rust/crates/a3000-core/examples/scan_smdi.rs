//! Smoke test bout-en-bout : open_adapter + Master Identify SMDI.
//!
//! Run :
//!   cd rust
//!   cargo run -p a3000-core --example scan_smdi --release -- [HA] [BUS] [TARGET] [LUN]
//!
//! Defaults : HA=1 BUS=0 TARGET=0 LUN=0 (test bench Yamaha typique).
//! Requiert PowerShell **Administrator** (sinon ERROR_ACCESS_DENIED sur \\.\ScsiN:).

#![cfg(windows)]
#![allow(clippy::expect_used, clippy::print_stdout, clippy::print_stderr)]

use std::env;

use a3000_core::scsi::ScsiHandle;
use a3000_core::smdi::{
    build_scsi_receive_cdb, build_scsi_send_cdb, decode_message, master_identify_message,
    MSG_SLAVE_IDENTIFY, SMDI_MIN_TRANSFER,
};

fn main() {
    let args: Vec<String> = env::args().collect();
    let ha: u32 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(1);
    let bus: u8 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
    let target: u8 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(0);
    let lun: u8 = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(0);

    println!("=== a3000-core smoke test ===");
    println!("Cible : HA{ha} BUS{bus} ID{target} LUN{lun}");

    println!("→ Ouverture \\\\.\\Scsi{ha}:");
    let handle = match ScsiHandle::open(ha) {
        Ok(h) => {
            println!("  ✓ handle ouvert");
            h
        }
        Err(e) => {
            eprintln!("  ✗ {e}");
            std::process::exit(1);
        }
    };

    // 1. SCSI SEND : envoi du Master Identify
    let identify = master_identify_message();
    let send_cdb = build_scsi_send_cdb(identify.len() as u32).expect("send CDB");
    println!("→ SEND Master Identify ({} bytes)", identify.len());
    let send_result = match handle.send_cdb(bus, target, lun, &send_cdb, 0, Some(&identify), 30) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("  ✗ {e}");
            std::process::exit(2);
        }
    };
    println!(
        "  ScsiStatus = 0x{:02X}, sense = {:02X?}",
        send_result.scsi_status,
        &send_result.sense[..14],
    );

    // 2. SCSI RECEIVE : récupère le Slave Identify
    let recv_cdb = build_scsi_receive_cdb(64).expect("recv CDB");
    println!("→ RECEIVE (alloc 64 bytes)");
    let recv_result = match handle.send_cdb(bus, target, lun, &recv_cdb, 64, None, 30) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("  ✗ {e}");
            std::process::exit(3);
        }
    };
    println!(
        "  ScsiStatus = 0x{:02X}, transferred = {} bytes",
        recv_result.scsi_status, recv_result.transferred,
    );
    println!("  data = {:02X?}", recv_result.data);
    println!("  sense = {:02X?}", &recv_result.sense[..14]);

    if recv_result.scsi_status != 0 {
        eprintln!(
            "✗ SCSI RECEIVE non-zero status (0x{:02X}) — sense key 0x{:02X} ASC 0x{:02X}",
            recv_result.scsi_status,
            recv_result.sense.get(2).copied().unwrap_or(0) & 0x0F,
            recv_result.sense.get(12).copied().unwrap_or(0),
        );
        std::process::exit(4);
    }

    if recv_result.data.len() < SMDI_MIN_TRANSFER {
        eprintln!("✗ Réponse trop courte ({} octets)", recv_result.data.len());
        std::process::exit(5);
    }

    // 3. Decode
    let message = match decode_message(&recv_result.data) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("✗ decode_message : {e}");
            std::process::exit(6);
        }
    };
    println!(
        "→ Message reçu : id=0x{:04X}/sub=0x{:04X}, payload {} bytes",
        message.message_id,
        message.sub_id,
        message.payload.len(),
    );

    if message.code() == MSG_SLAVE_IDENTIFY {
        println!("✓✓✓ SLAVE IDENTIFY reçu — handshake SMDI OK sur Yamaha A3000");
        std::process::exit(0);
    } else {
        eprintln!(
            "✗ Code message inattendu : attendu 0x0001/0x0001 (Slave Identify)"
        );
        std::process::exit(7);
    }
}
