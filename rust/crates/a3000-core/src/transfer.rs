//! Orchestrateur SMDI : envoi / réception complets d'un sample sur un slot.
//!
//! Référence Python : `python/a3000_transfer/transfer.py`
//!
//! Séquence transfer (master → slave) :
//!   1. Drain défensif
//!   2. SEND Sample Header → RECEIVE BSTA
//!   3. SEND Begin Sample Transfer
//!   4. Loop : RECEIVE Send Next Packet → SEND Data Packet
//!   5. RECEIVE End Of Procedure
//!   6. Settle post-EoP

#![cfg(windows)]

use std::thread;
use std::time::Duration;

use thiserror::Error;

use crate::scsi::{PassThroughResult, ScsiError, ScsiHandle};
use crate::smdi::{
    build_scsi_receive_cdb, build_scsi_send_cdb, decode_begin_sample_transfer_ack,
    decode_data_packet, decode_message, decode_message_reject, decode_sample_header,
    decode_send_next_packet, encode_abort_procedure, encode_begin_sample_transfer,
    encode_data_packet, encode_sample_header, encode_sample_header_request,
    encode_send_next_packet, period_ns_for_rate, SampleHeader, SmdiError, SmdiMessage,
    LOOP_DISABLED, MSG_BEGIN_SAMPLE_TRANSFER_ACK, MSG_DATA_PACKET, MSG_END_OF_PROCEDURE,
    MSG_REJECT, MSG_SAMPLE_HEADER, MSG_SEND_NEXT_PACKET, MSG_WAIT, PITCH_DEFAULT_FRACTION,
    PITCH_MIDDLE_C_INTEGER, SMDI_MIN_TRANSFER,
};

const DEFAULT_TIMEOUT_S: u32 = 30;
const MAX_WAIT_MESSAGES: u32 = 10;
const RECEIVE_RETRIES: u32 = 5;

#[derive(Debug, Error)]
pub enum TransferError {
    #[error("SCSI error: {0}")]
    Scsi(#[from] ScsiError),
    #[error("SMDI error: {0}")]
    Smdi(#[from] SmdiError),
    #[error("WAV non supporté : {0}")]
    UnsupportedWav(String),
    #[error("Le slave a rejeté : 0x{code:04X}/0x{sub:04X}")]
    Rejected { code: u16, sub: u16 },
    #[error("Réponse inattendue : 0x{id:04X}/0x{sub:04X}")]
    UnexpectedReply { id: u16, sub: u16 },
    #[error("Trop de Wait messages reçus ({0})")]
    TooManyWaits(u32),
    #[error(
        "RECEIVE échec après {retries} essais : ScsiStatus=0x{status:02X}, sense_key=0x{key:02X}, ASC=0x{asc:02X}"
    )]
    ReceiveFailed { retries: u32, status: u8, key: u8, asc: u8 },
    #[error("End Of Procedure prématuré à offset {offset}/{total}")]
    PrematureEop { offset: usize, total: usize },
    #[error("Numéro de packet incohérent : slave demande {requested}, attendu {expected}")]
    PacketNumberMismatch { requested: u32, expected: u32 },
    #[error("BSTA pointe vers sample {got} au lieu de {expected}")]
    BstaWrongSample { got: u32, expected: u32 },
    #[error("Data Packet Length négocié trop petit ({0}, frame_size requis)")]
    DplTooSmall(u32),
    #[error("Message SMDI < {SMDI_MIN_TRANSFER} octets : rejeté slave (sense 82H)")]
    SmdiTooShort,
    #[error("Internal protocol error: {0}")]
    Internal(String),
}

/// Triplet d'adresse SCSI au-dessus du handle ouvert.
#[derive(Debug, Clone, Copy)]
pub struct ScsiTarget {
    pub path_id: u8,
    pub target_id: u8,
    pub lun: u8,
}

#[derive(Debug, Clone)]
pub struct TransferStats {
    pub sample_number: u32,
    pub bytes_sent: usize,
    pub packet_count: usize,
    pub packet_length: u32,
}

/// PCM 16-bit little-endian → big-endian (byteswap par mots de 2 octets).
///
/// # Errors
/// `Internal` si la longueur n'est pas paire.
pub fn pcm16_le_to_be(pcm: &[u8]) -> Result<Vec<u8>, TransferError> {
    if pcm.len() % 2 != 0 {
        return Err(TransferError::Internal(
            "PCM 16-bit attendu : longueur paire".into(),
        ));
    }
    let mut out = Vec::with_capacity(pcm.len());
    for chunk in pcm.chunks_exact(2) {
        out.push(chunk[1]);
        out.push(chunk[0]);
    }
    Ok(out)
}

/// PCM 16-bit big-endian → little-endian (même opération que LE→BE).
///
/// # Errors
/// `Internal` si la longueur n'est pas paire.
pub fn pcm16_be_to_le(pcm: &[u8]) -> Result<Vec<u8>, TransferError> {
    pcm16_le_to_be(pcm)
}

/// Envoie un message SMDI complet via SCSI SEND.
fn send_smdi(
    handle: &ScsiHandle,
    target: ScsiTarget,
    message: &[u8],
    timeout: u32,
) -> Result<PassThroughResult, TransferError> {
    if message.len() < SMDI_MIN_TRANSFER {
        return Err(TransferError::SmdiTooShort);
    }
    let cdb = build_scsi_send_cdb(message.len() as u32)?;
    Ok(handle.send_cdb(target.path_id, target.target_id, target.lun, &cdb, 0, Some(message), timeout)?)
}

/// Lit un message SMDI via SCSI RECEIVE et le décode. Boucle sur Wait
/// (slave demande plus de temps) et retry sur sense key 0x0B (ABORTED COMMAND).
fn receive_smdi(
    handle: &ScsiHandle,
    target: ScsiTarget,
    allocation_length: u32,
    timeout: u32,
) -> Result<(PassThroughResult, SmdiMessage), TransferError> {
    if (allocation_length as usize) < SMDI_MIN_TRANSFER {
        return Err(TransferError::SmdiTooShort);
    }
    let cdb = build_scsi_receive_cdb(allocation_length)?;
    let mut wait_count: u32 = 0;
    let mut last_status: u8 = 0;
    let mut last_sense: [u8; 32] = [0; 32];

    loop {
        for attempt in 0..RECEIVE_RETRIES {
            let result = handle.send_cdb(
                target.path_id, target.target_id, target.lun,
                &cdb, allocation_length, None, timeout,
            )?;
            if result.scsi_status == 0 {
                let message = decode_message(&result.data)?;
                if message.code() == MSG_WAIT {
                    wait_count += 1;
                    if wait_count > MAX_WAIT_MESSAGES {
                        return Err(TransferError::TooManyWaits(wait_count));
                    }
                    thread::sleep(Duration::from_millis(500));
                    break; // re-RECEIVE
                }
                return Ok((result, message));
            }
            last_status = result.scsi_status;
            last_sense = result.sense;
            let sense_key = result.sense.get(2).copied().unwrap_or(0) & 0x0F;
            let asc = result.sense.get(12).copied().unwrap_or(0);
            if asc == 0x81 {
                return Err(TransferError::Smdi(SmdiError::NoReplyPending));
            }
            if sense_key == 0x0B {
                thread::sleep(Duration::from_millis(50 * u64::from(attempt + 1)));
                continue; // retry intérieur
            }
            return Err(TransferError::ReceiveFailed {
                retries: attempt + 1,
                status: last_status,
                key: sense_key,
                asc,
            });
        }
        // Boucle interne épuisée sur sense key 0x0B → erreur
        let sense_key = last_sense.get(2).copied().unwrap_or(0) & 0x0F;
        let asc = last_sense.get(12).copied().unwrap_or(0);
        if sense_key == 0x0B {
            return Err(TransferError::ReceiveFailed {
                retries: RECEIVE_RETRIES,
                status: last_status,
                key: sense_key,
                asc,
            });
        }
        // Sinon (Wait reçu) : continue la boucle externe pour re-RECEIVE
    }
}

/// Drain défensif : consume une réponse pending si elle traîne sur le bus.
/// Tolère les sense 0x81 (rien à drainer, OK) et 0x0B (transitoire).
pub fn drain_pending_reply(
    handle: &ScsiHandle,
    target: ScsiTarget,
) -> Result<Option<SmdiMessage>, TransferError> {
    let cdb = build_scsi_receive_cdb(4096)?;
    for attempt in 0..3 {
        let result = handle.send_cdb(
            target.path_id, target.target_id, target.lun,
            &cdb, 4096, None, 5,
        )?;
        if result.scsi_status == 0 {
            return Ok(decode_message(&result.data).ok());
        }
        let sense_key = result.sense.get(2).copied().unwrap_or(0) & 0x0F;
        let asc = result.sense.get(12).copied().unwrap_or(0);
        if asc == 0x81 {
            return Ok(None); // bus propre
        }
        if sense_key == 0x0B {
            thread::sleep(Duration::from_millis(50));
            continue;
        }
        let _ = attempt;
        break;
    }
    // Drain ne doit jamais bloquer un transfert : on retourne None si pas réussi
    Ok(None)
}

/// Vue du WAV à transférer (ce qu'attend `transfer_sample`).
///
/// Mirror simplifié de `WavePayload` Python : pcm_data en LE 16-bit (sera
/// byteswapé en BE par l'orchestrateur).
#[derive(Debug, Clone)]
pub struct WaveInput<'a> {
    pub channels: u16,
    pub sample_rate: u32,
    pub bits_per_sample: u16,
    pub frame_count: u32,
    pub pcm_data_le: &'a [u8],
}

/// Options pour `transfer_sample`.
#[derive(Debug, Clone)]
pub struct TransferOptions {
    pub loop_control: u8,
    pub pitch_integer: u8,
    pub pitch_fraction: u32,
    pub preferred_packet_length: u32,
    pub timeout_seconds: u32,
    pub dry_run: bool,
    pub settle_seconds: f32,
    /// Si `true`, skip le drain défensif au début. Économise 1 RECEIVE
    /// (~50 ms) sur les transferts consécutifs dans une même session.
    pub skip_initial_drain: bool,
}

impl Default for TransferOptions {
    fn default() -> Self {
        Self {
            loop_control: LOOP_DISABLED,
            pitch_integer: PITCH_MIDDLE_C_INTEGER,
            pitch_fraction: PITCH_DEFAULT_FRACTION,
            preferred_packet_length: 4096,
            timeout_seconds: DEFAULT_TIMEOUT_S,
            dry_run: false,
            settle_seconds: 1.0,
            skip_initial_drain: false,
        }
    }
}

/// Callback de progression : `(packets_sent, bytes_sent, total_bytes)`.
pub type ProgressFn<'a> = &'a mut dyn FnMut(usize, usize, usize);

/// Envoie un sample dans le slot `sample_number` du sampler.
///
/// # Errors
/// Voir `TransferError`. En cas d'erreur après le BST, un Abort Procedure est
/// envoyé pour libérer le slot.
#[allow(clippy::too_many_lines)] // orchestration multi-étapes, séquentielle, lisible
pub fn transfer_sample(
    handle: &ScsiHandle,
    target: ScsiTarget,
    sample_number: u32,
    wave: WaveInput<'_>,
    name: &str,
    opts: &TransferOptions,
    mut progress: Option<ProgressFn<'_>>,
) -> Result<TransferStats, TransferError> {
    if wave.bits_per_sample != 16 {
        return Err(TransferError::UnsupportedWav(format!(
            "{} bits — sampler attend 16-bit words",
            wave.bits_per_sample
        )));
    }
    if wave.channels != 1 && wave.channels != 2 {
        return Err(TransferError::UnsupportedWav(format!(
            "{} canaux — mono ou stéréo seulement",
            wave.channels
        )));
    }

    let smdi_data = pcm16_le_to_be(wave.pcm_data_le)?;
    let header = SampleHeader {
        sample_number,
        bits_per_word: wave.bits_per_sample as u8,
        channels: wave.channels as u8,
        sample_period_ns: period_ns_for_rate(wave.sample_rate),
        sample_length_words: wave.frame_count,
        loop_start: 0,
        loop_end: wave.frame_count.saturating_sub(1),
        loop_control: opts.loop_control,
        pitch_integer: opts.pitch_integer,
        pitch_fraction: opts.pitch_fraction,
        name: name.into(),
    };

    // Helper closure interne pour wrapper le déroulement avec abort en cas d'erreur
    let mut aborted = false;
    let mut do_abort = |handle: &ScsiHandle| {
        if aborted {
            return;
        }
        aborted = true;
        let _ = send_smdi(handle, target, &encode_abort_procedure(), opts.timeout_seconds);
        let _ = drain_pending_reply(handle, target);
    };

    // Wrapper pour propager les erreurs avec abort
    let run = |progress: &mut Option<ProgressFn<'_>>| -> Result<TransferStats, TransferError> {
        // 0. Drain défensif (ne bloque jamais)
        if !opts.skip_initial_drain {
            let _ = drain_pending_reply(handle, target);
        }

        // 1. SEND Sample Header → RECEIVE BSTA
        let sh_msg = encode_sample_header(&header)?;
        let send_result = send_smdi(handle, target, &sh_msg, opts.timeout_seconds)?;
        if send_result.scsi_status != 0 {
            return Err(TransferError::ReceiveFailed {
                retries: 1,
                status: send_result.scsi_status,
                key: send_result.sense.get(2).copied().unwrap_or(0) & 0x0F,
                asc: send_result.sense.get(12).copied().unwrap_or(0),
            });
        }
        let (_, reply) = receive_smdi(handle, target, 64, opts.timeout_seconds)?;
        if reply.code() == MSG_REJECT {
            let (code, sub) = decode_message_reject(&reply)?;
            return Err(TransferError::Rejected { code, sub });
        }
        if reply.code() != MSG_BEGIN_SAMPLE_TRANSFER_ACK {
            return Err(TransferError::UnexpectedReply {
                id: reply.message_id,
                sub: reply.sub_id,
            });
        }
        let (ack_sn, slave_max_dpl) = decode_begin_sample_transfer_ack(&reply)?;
        if ack_sn != sample_number {
            return Err(TransferError::BstaWrongSample {
                got: ack_sn,
                expected: sample_number,
            });
        }

        let frame_size = u32::from(wave.channels) * (u32::from(wave.bits_per_sample) / 8);
        let mut packet_length = opts.preferred_packet_length.min(slave_max_dpl);
        packet_length -= packet_length % frame_size;
        if packet_length < frame_size {
            return Err(TransferError::DplTooSmall(packet_length));
        }

        if opts.dry_run {
            return Ok(TransferStats {
                sample_number, bytes_sent: 0, packet_count: 0, packet_length,
            });
        }

        // 2. SEND Begin Sample Transfer
        let bst_msg = encode_begin_sample_transfer(sample_number, packet_length)?;
        let send_result = send_smdi(handle, target, &bst_msg, opts.timeout_seconds)?;
        if send_result.scsi_status != 0 {
            return Err(TransferError::ReceiveFailed {
                retries: 1,
                status: send_result.scsi_status,
                key: send_result.sense.get(2).copied().unwrap_or(0) & 0x0F,
                asc: send_result.sense.get(12).copied().unwrap_or(0),
            });
        }

        // 3. Pré-encode des Data Packets pour réduire la latence
        let mut packets: Vec<Vec<u8>> = Vec::new();
        let plen = packet_length as usize;
        let mut offset = 0;
        let mut pn: u32 = 0;
        while offset < smdi_data.len() {
            let end = (offset + plen).min(smdi_data.len());
            packets.push(encode_data_packet(pn, &smdi_data[offset..end])?);
            offset = end;
            pn += 1;
        }

        // 4. Boucle SNP / DP
        let total = smdi_data.len();
        let mut sent_offset: usize = 0;
        let mut packets_sent: usize = 0;
        let mut expected_pn: u32 = 0;

        loop {
            let (_, reply) = receive_smdi(handle, target, 64, opts.timeout_seconds)?;
            if reply.code() == MSG_END_OF_PROCEDURE {
                if sent_offset < total {
                    return Err(TransferError::PrematureEop { offset: sent_offset, total });
                }
                break;
            }
            if reply.code() == MSG_REJECT {
                let (code, sub) = decode_message_reject(&reply)?;
                return Err(TransferError::Rejected { code, sub });
            }
            if reply.code() != MSG_SEND_NEXT_PACKET {
                return Err(TransferError::UnexpectedReply {
                    id: reply.message_id, sub: reply.sub_id,
                });
            }
            let snp = decode_send_next_packet(&reply)?;
            if snp != expected_pn {
                return Err(TransferError::PacketNumberMismatch {
                    requested: snp, expected: expected_pn,
                });
            }
            if sent_offset >= total {
                break; // tout envoyé : EoP attendu au prochain RECEIVE
            }
            let pre_encoded = packets.get(snp as usize)
                .ok_or_else(|| TransferError::Internal(format!(
                    "Slave demande packet#{snp} hors pool ({} pré-encodés)", packets.len()
                )))?;
            let chunk_size = pre_encoded.len() - 14; // 11 SMDI hdr + 3 packet#
            let send_result = send_smdi(handle, target, pre_encoded, opts.timeout_seconds)?;
            if send_result.scsi_status != 0 {
                return Err(TransferError::ReceiveFailed {
                    retries: 1, status: send_result.scsi_status,
                    key: send_result.sense.get(2).copied().unwrap_or(0) & 0x0F,
                    asc: send_result.sense.get(12).copied().unwrap_or(0),
                });
            }
            sent_offset += chunk_size;
            packets_sent += 1;
            expected_pn += 1;
            if let Some(cb) = progress.as_deref_mut() {
                cb(packets_sent, sent_offset, total);
            }
        }

        if opts.settle_seconds > 0.0 {
            thread::sleep(Duration::from_millis((opts.settle_seconds * 1000.0) as u64));
        }

        Ok(TransferStats {
            sample_number, bytes_sent: sent_offset, packet_count: packets_sent, packet_length,
        })
    };

    match run(&mut progress) {
        Ok(stats) => Ok(stats),
        Err(e) => {
            do_abort(handle);
            Err(e)
        }
    }
}

/// Récupère un sample depuis le sampler. Retourne (header, PCM 16-bit BE).
///
/// # Errors
/// Voir `TransferError`.
#[allow(clippy::too_many_lines)]
pub fn receive_sample(
    handle: &ScsiHandle,
    target: ScsiTarget,
    sample_number: u32,
    preferred_packet_length: u32,
    timeout_seconds: u32,
    settle_seconds: f32,
    mut progress: Option<ProgressFn<'_>>,
) -> Result<(SampleHeader, Vec<u8>), TransferError> {
    let _ = drain_pending_reply(handle, target);

    // 1. Sample Header Request
    send_smdi(handle, target, &encode_sample_header_request(sample_number)?, timeout_seconds)?;
    let (_, reply) = receive_smdi(handle, target, 4096, timeout_seconds)?;
    if reply.code() == MSG_REJECT {
        let (code, sub) = decode_message_reject(&reply)?;
        return Err(TransferError::Rejected { code, sub });
    }
    if reply.code() != MSG_SAMPLE_HEADER {
        return Err(TransferError::UnexpectedReply { id: reply.message_id, sub: reply.sub_id });
    }
    let header = decode_sample_header(&reply)?;
    if header.bits_per_word != 16 {
        return Err(TransferError::UnsupportedWav(format!(
            "Réception : {} bits, seul 16-bit supporté", header.bits_per_word
        )));
    }
    if header.channels != 1 && header.channels != 2 {
        return Err(TransferError::UnsupportedWav(format!(
            "Réception : {} canaux, mono/stéréo seulement", header.channels
        )));
    }

    let frame_size = u32::from(header.channels) * (u32::from(header.bits_per_word) / 8);
    let total_bytes = (header.sample_length_words as u64 * u64::from(frame_size)) as usize;

    // 2. Begin Sample Transfer
    let mut requested_dpl = preferred_packet_length - (preferred_packet_length % frame_size);
    if requested_dpl < frame_size {
        requested_dpl = frame_size;
    }
    send_smdi(handle, target, &encode_begin_sample_transfer(sample_number, requested_dpl)?, timeout_seconds)?;

    // 3. BSTA
    let (_, reply) = receive_smdi(handle, target, 64, timeout_seconds)?;
    if reply.code() == MSG_REJECT {
        let (code, sub) = decode_message_reject(&reply)?;
        return Err(TransferError::Rejected { code, sub });
    }
    if reply.code() != MSG_BEGIN_SAMPLE_TRANSFER_ACK {
        return Err(TransferError::UnexpectedReply { id: reply.message_id, sub: reply.sub_id });
    }
    let (ack_sn, slave_dpl) = decode_begin_sample_transfer_ack(&reply)?;
    if ack_sn != sample_number {
        return Err(TransferError::BstaWrongSample { got: ack_sn, expected: sample_number });
    }
    let mut actual_dpl = requested_dpl.min(slave_dpl);
    actual_dpl -= actual_dpl % frame_size;
    if actual_dpl < frame_size {
        return Err(TransferError::DplTooSmall(actual_dpl));
    }

    // 4. Loop SNP master / DP slave
    let mut received: Vec<u8> = Vec::with_capacity(total_bytes);
    let mut packet_num: u32 = 0;
    let receive_alloc = 14 + actual_dpl + 16; // 11 hdr + 3 packet# + dpl + safety

    while received.len() < total_bytes {
        send_smdi(handle, target, &encode_send_next_packet(packet_num)?, timeout_seconds)?;
        let (_, reply) = receive_smdi(handle, target, receive_alloc, timeout_seconds)?;
        if reply.code() == MSG_END_OF_PROCEDURE {
            break;
        }
        if reply.code() == MSG_REJECT {
            let (code, sub) = decode_message_reject(&reply)?;
            return Err(TransferError::Rejected { code, sub });
        }
        if reply.code() != MSG_DATA_PACKET {
            return Err(TransferError::UnexpectedReply { id: reply.message_id, sub: reply.sub_id });
        }
        let (recv_pn, chunk) = decode_data_packet(&reply)?;
        if recv_pn != packet_num {
            return Err(TransferError::PacketNumberMismatch { requested: recv_pn, expected: packet_num });
        }
        let remaining = total_bytes.saturating_sub(received.len());
        let copy_len = chunk.len().min(remaining);
        received.extend_from_slice(&chunk[..copy_len]);
        packet_num += 1;
        if let Some(cb) = progress.as_deref_mut() {
            cb(packet_num as usize, received.len(), total_bytes);
        }
    }

    if settle_seconds > 0.0 {
        thread::sleep(Duration::from_millis((settle_seconds * 1000.0) as u64));
    }

    Ok((header, received))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn pcm16_byteswap_roundtrip() {
        let le: Vec<u8> = vec![0x01, 0x02, 0x03, 0x04, 0x05, 0x06];
        let be = pcm16_le_to_be(&le).unwrap();
        assert_eq!(be, vec![0x02, 0x01, 0x04, 0x03, 0x06, 0x05]);
        let back = pcm16_be_to_le(&be).unwrap();
        assert_eq!(back, le);
    }

    #[test]
    fn pcm16_rejects_odd_length() {
        let r = pcm16_le_to_be(&[0x01, 0x02, 0x03]);
        assert!(r.is_err());
    }
}
