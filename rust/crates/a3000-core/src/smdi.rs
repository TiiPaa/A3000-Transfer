//! Codec SMDI Peavey (encode/decode messages SCSI Musical Data Interchange).
//!
//! Référence Python : `python/a3000_transfer/smdi.py`
//! Référence spec : Peavey 1992 v0.03 + corrections projet (cf. `docs/conversion/INVENTORY.md`)
//!
//! ⚠️ Format Sample Header A3000 corrigé :
//! - Period sur 3 octets BE (pas 4)
//! - Pitch Fraction sur 3 octets BE (pas 2)
//!
//! Format wire d'un message SMDI :
//! ```text
//! "SMDI"            (4 octets ASCII)
//! Message ID        (2 octets, big-endian)
//! Message Sub-ID    (2 octets, big-endian)
//! Additional Length (3 octets, big-endian) — taille du payload qui suit
//! payload           (Additional Length octets)
//! ```

use thiserror::Error;

// ─── Constantes ───────────────────────────────────────────────────────────

pub const SMDI_TAG: &[u8; 4] = b"SMDI";
pub const SMDI_HEADER_SIZE: usize = 11; // tag(4) + msg_id(2) + sub_id(2) + add_len(3)
pub const SMDI_MIN_TRANSFER: usize = 11;
pub const SAMPLE_HEADER_FIXED_PAYLOAD: usize = 26;

/// Code de message = `(message_id, sub_id)`.
pub type MessageCode = (u16, u16);

// ─── Message IDs (Table 1 de la spec, p39) ────────────────────────────────

pub const MSG_NO_MESSAGE: MessageCode = (0x0000, 0x0000);
pub const MSG_MASTER_IDENTIFY: MessageCode = (0x0001, 0x0000);
pub const MSG_SLAVE_IDENTIFY: MessageCode = (0x0001, 0x0001);
pub const MSG_REJECT: MessageCode = (0x0002, 0x0000);
pub const MSG_ACK: MessageCode = (0x0100, 0x0000);
pub const MSG_NAK: MessageCode = (0x0101, 0x0000);
pub const MSG_WAIT: MessageCode = (0x0102, 0x0000);
pub const MSG_SEND_NEXT_PACKET: MessageCode = (0x0103, 0x0000);
pub const MSG_END_OF_PROCEDURE: MessageCode = (0x0104, 0x0000);
pub const MSG_ABORT_PROCEDURE: MessageCode = (0x0105, 0x0000);
pub const MSG_DATA_PACKET: MessageCode = (0x0110, 0x0000);
pub const MSG_SAMPLE_HEADER_REQUEST: MessageCode = (0x0120, 0x0000);
pub const MSG_SAMPLE_HEADER: MessageCode = (0x0121, 0x0000);
pub const MSG_BEGIN_SAMPLE_TRANSFER: MessageCode = (0x0122, 0x0000);
pub const MSG_BEGIN_SAMPLE_TRANSFER_ACK: MessageCode = (0x0122, 0x0001);
pub const MSG_SAMPLE_NAME: MessageCode = (0x0123, 0x0000);
pub const MSG_DELETE_SAMPLE: MessageCode = (0x0124, 0x0000);
pub const MSG_TRANSMIT_MIDI: MessageCode = (0x0200, 0x0000);

// Loop control values
pub const LOOP_FORWARD: u8 = 0x00;
pub const LOOP_BACKWARD: u8 = 0x01;
pub const LOOP_DISABLED: u8 = 0x7F;

// Pitch defaults
pub const PITCH_MIDDLE_C_INTEGER: u8 = 0x3C;
pub const PITCH_DEFAULT_FRACTION: u32 = 0x0000;

// Rejection codes (Table 2, p40)
pub const REJECT_GENERAL: u16 = 0x0002;
pub const REJECT_DEVICE_BUSY: u16 = 0x0005;
pub const REJECT_PACKET_NUMBER_MISMATCH: u16 = 0x0011;
pub const REJECT_SAMPLE: u16 = 0x0020;
pub const REJECT_BEGIN_TRANSFER: u16 = 0x0022;

const MAX_24BIT: u32 = 0x00FF_FFFF;

// ─── Erreurs ──────────────────────────────────────────────────────────────

/// Erreur de protocole SMDI.
///
/// `NoReplyPending` correspond au sense byte 0x81 retourné par le slave
/// (cf. Python `SmdiNoReplyPendingError`). Sur Yamaha A3000, ce code apparaît
/// pour un slot vide ou (pendant un transfert) un Bulk Protect activé.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum SmdiError {
    #[error("Trame SMDI trop courte : {0} octets, minimum {SMDI_HEADER_SIZE}.")]
    FrameTooShort(usize),
    #[error("Tag SMDI absent (reçu {0:?}).")]
    MissingTag([u8; 4]),
    #[error("Trame tronquée : header annonce {expected} octets de payload, reçu {actual}.")]
    PayloadTruncated { expected: usize, actual: usize },
    #[error("Payload SMDI trop grand (max 0xFF_FFFF octets).")]
    PayloadTooLarge,
    #[error("Pas un Sample Header (id=0x{0:04X}/0x{1:04X}).")]
    NotSampleHeader(u16, u16),
    #[error("Sample Header trop court : {0} octets, attendu ≥ {SAMPLE_HEADER_FIXED_PAYLOAD}.")]
    SampleHeaderTooShort(usize),
    #[error("Sample Header tronqué : nom annoncé {name_len} octets, payload total {total}.")]
    SampleHeaderNameTruncated { name_len: usize, total: usize },
    #[error("Pas un Message Reject (id=0x{0:04X}/0x{1:04X}).")]
    NotReject(u16, u16),
    #[error("Message Reject incomplet : {0} octets, attendu 4.")]
    RejectTooShort(usize),
    #[error("Pas un BSTA (id=0x{0:04X}/0x{1:04X}).")]
    NotBsta(u16, u16),
    #[error("BSTA tronqué : {0} octets, attendu ≥ 6.")]
    BstaTooShort(usize),
    #[error("Pas un Send Next Packet (id=0x{0:04X}/0x{1:04X}).")]
    NotSendNextPacket(u16, u16),
    #[error("Send Next Packet tronqué : {0} octets, attendu 3.")]
    SendNextPacketTooShort(usize),
    #[error("Pas un Data Packet (id=0x{0:04X}/0x{1:04X}).")]
    NotDataPacket(u16, u16),
    #[error("Data Packet trop court : {0} octets, attendu ≥ 3.")]
    DataPacketTooShort(usize),
    #[error("Nom de sample trop long : {0} octets, max 255.")]
    NameTooLong(usize),
    #[error("Valeur 24-bit hors plage : {0} (max 0xFF_FFFF).")]
    Value24BitOutOfRange(u32),

    /// Sense 0x81 — RECEIVE rejected, no slave reply pending.
    /// Sur Yamaha A3000 : slot vide ou Bulk Protect activé.
    #[error("Pas de réponse slave pending (sense 0x81) — slot vide ou Bulk Protect activé sur le sampler.")]
    NoReplyPending,
}

// ─── Helpers d'encodage 24-bit BE ─────────────────────────────────────────

#[inline]
fn write_u24_be(buf: &mut Vec<u8>, v: u32) -> Result<(), SmdiError> {
    if v > MAX_24BIT {
        return Err(SmdiError::Value24BitOutOfRange(v));
    }
    buf.push(((v >> 16) & 0xFF) as u8);
    buf.push(((v >> 8) & 0xFF) as u8);
    buf.push((v & 0xFF) as u8);
    Ok(())
}

#[inline]
fn read_u24_be(b: &[u8]) -> u32 {
    debug_assert!(b.len() >= 3);
    (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2])
}

// ─── SmdiMessage ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmdiMessage {
    pub message_id: u16,
    pub sub_id: u16,
    pub payload: Vec<u8>,
}

impl SmdiMessage {
    #[must_use]
    pub fn code(&self) -> MessageCode {
        (self.message_id, self.sub_id)
    }

    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        // SAFETY : `SmdiMessage` ne peut être construit que via `decode_message`
        // (qui valide la taille) ou littéralement (utilisateur responsable).
        // Si un user crée un SmdiMessage avec un payload > 0xFFFFFF directement,
        // c'est un bug appelant et on panique.
        #[allow(clippy::expect_used)]
        encode_message(self.message_id, self.sub_id, &self.payload)
            .expect("SmdiMessage payload exceeds 0xFFFFFF — invalid construction")
    }
}

/// Encode un message SMDI complet.
///
/// # Errors
/// `SmdiError::PayloadTooLarge` si le payload dépasse 0xFF_FFFF octets.
pub fn encode_message(message_id: u16, sub_id: u16, payload: &[u8]) -> Result<Vec<u8>, SmdiError> {
    let add_len = payload.len();
    if add_len > MAX_24BIT as usize {
        return Err(SmdiError::PayloadTooLarge);
    }
    let mut buf = Vec::with_capacity(SMDI_HEADER_SIZE + add_len);
    buf.extend_from_slice(SMDI_TAG);
    buf.extend_from_slice(&message_id.to_be_bytes());
    buf.extend_from_slice(&sub_id.to_be_bytes());
    write_u24_be(&mut buf, add_len as u32)?;
    buf.extend_from_slice(payload);
    Ok(buf)
}

/// Décode un message SMDI brut.
///
/// # Errors
/// - `SmdiError::FrameTooShort` si moins de 11 octets
/// - `SmdiError::MissingTag` si les 4 premiers octets ne sont pas "SMDI"
/// - `SmdiError::PayloadTruncated` si le buffer ne contient pas tout le payload annoncé
pub fn decode_message(raw: &[u8]) -> Result<SmdiMessage, SmdiError> {
    if raw.len() < SMDI_HEADER_SIZE {
        return Err(SmdiError::FrameTooShort(raw.len()));
    }
    // raw.len() ≥ SMDI_HEADER_SIZE (= 11) vérifié juste au-dessus
    let tag: [u8; 4] = [raw[0], raw[1], raw[2], raw[3]];
    if &tag != SMDI_TAG {
        return Err(SmdiError::MissingTag(tag));
    }
    let message_id = u16::from_be_bytes([raw[4], raw[5]]);
    let sub_id = u16::from_be_bytes([raw[6], raw[7]]);
    let add_len = read_u24_be(&raw[8..11]) as usize;
    let payload_end = SMDI_HEADER_SIZE + add_len;
    if raw.len() < payload_end {
        return Err(SmdiError::PayloadTruncated {
            expected: add_len,
            actual: raw.len() - SMDI_HEADER_SIZE,
        });
    }
    Ok(SmdiMessage {
        message_id,
        sub_id,
        payload: raw[SMDI_HEADER_SIZE..payload_end].to_vec(),
    })
}

// ─── SCSI CDBs ────────────────────────────────────────────────────────────

/// Construit le CDB SCSI SEND (opcode 0x0A, 6-byte CDB).
///
/// # Errors
/// `Value24BitOutOfRange` si transfer_length > 0xFF_FFFF.
pub fn build_scsi_send_cdb(transfer_length: u32) -> Result<[u8; 6], SmdiError> {
    if transfer_length > MAX_24BIT {
        return Err(SmdiError::Value24BitOutOfRange(transfer_length));
    }
    Ok([
        0x0A,
        0x00,
        ((transfer_length >> 16) & 0xFF) as u8,
        ((transfer_length >> 8) & 0xFF) as u8,
        (transfer_length & 0xFF) as u8,
        0x00,
    ])
}

/// Construit le CDB SCSI RECEIVE (opcode 0x08, 6-byte CDB).
///
/// # Errors
/// `Value24BitOutOfRange` si allocation_length > 0xFF_FFFF.
pub fn build_scsi_receive_cdb(allocation_length: u32) -> Result<[u8; 6], SmdiError> {
    if allocation_length > MAX_24BIT {
        return Err(SmdiError::Value24BitOutOfRange(allocation_length));
    }
    Ok([
        0x08,
        0x00,
        ((allocation_length >> 16) & 0xFF) as u8,
        ((allocation_length >> 8) & 0xFF) as u8,
        (allocation_length & 0xFF) as u8,
        0x00,
    ])
}

// ─── Master Identify ──────────────────────────────────────────────────────

#[must_use]
#[allow(clippy::expect_used)] // payload vide → encode_message infaillible
pub fn master_identify_message() -> Vec<u8> {
    encode_message(MSG_MASTER_IDENTIFY.0, MSG_MASTER_IDENTIFY.1, &[])
        .expect("empty payload always fits")
}

// ─── Sample Header ────────────────────────────────────────────────────────

/// Sample Header SMDI (Message ID 0x0121).
///
/// Layout du payload : 26 octets fixes + nom variable, big-endian partout.
/// Voir docstring du module pour le détail des offsets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SampleHeader {
    pub sample_number: u32,
    pub bits_per_word: u8,
    pub channels: u8,
    /// Période entre 2 samples en nanosecondes (24-bit BE).
    pub sample_period_ns: u32,
    pub sample_length_words: u32,
    pub loop_start: u32,
    pub loop_end: u32,
    pub loop_control: u8,
    pub pitch_integer: u8,
    pub pitch_fraction: u32, // 24-bit BE
    pub name: String,
}

impl SampleHeader {
    /// Sample rate en Hz dérivé de la période.
    #[must_use]
    pub fn sample_rate_hz(&self) -> f64 {
        if self.sample_period_ns == 0 {
            0.0
        } else {
            1_000_000_000.0 / f64::from(self.sample_period_ns)
        }
    }
}

#[must_use]
pub fn period_ns_for_rate(sample_rate_hz: u32) -> u32 {
    if sample_rate_hz == 0 {
        0
    } else {
        ((1_000_000_000.0 / f64::from(sample_rate_hz)).round()) as u32
    }
}

/// Encode un Sample Header complet (header SMDI + payload 26+n octets).
///
/// # Errors
/// - `NameTooLong` si le nom ASCII dépasse 255 octets
/// - `Value24BitOutOfRange` si un champ 24-bit dépasse 0xFF_FFFF
pub fn encode_sample_header(header: &SampleHeader) -> Result<Vec<u8>, SmdiError> {
    // ASCII strict ; tout caractère non-ASCII est remplacé par '?' comme Python.
    let name_bytes: Vec<u8> = header
        .name
        .chars()
        .map(|c| if c.is_ascii() { c as u8 } else { b'?' })
        .collect();
    if name_bytes.len() > 255 {
        return Err(SmdiError::NameTooLong(name_bytes.len()));
    }

    let mut payload = Vec::with_capacity(SAMPLE_HEADER_FIXED_PAYLOAD + name_bytes.len());
    write_u24_be(&mut payload, header.sample_number)?;
    payload.push(header.bits_per_word);
    payload.push(header.channels);
    write_u24_be(&mut payload, header.sample_period_ns)?;
    payload.extend_from_slice(&header.sample_length_words.to_be_bytes());
    payload.extend_from_slice(&header.loop_start.to_be_bytes());
    payload.extend_from_slice(&header.loop_end.to_be_bytes());
    payload.push(header.loop_control);
    payload.push(header.pitch_integer);
    write_u24_be(&mut payload, header.pitch_fraction)?;
    payload.push(name_bytes.len() as u8);
    payload.extend_from_slice(&name_bytes);

    encode_message(MSG_SAMPLE_HEADER.0, MSG_SAMPLE_HEADER.1, &payload)
}

/// Décode un Sample Header depuis un message SMDI.
///
/// # Errors
/// - `NotSampleHeader` si le code message n'est pas (0x0121, 0x0000)
/// - `SampleHeaderTooShort` si payload < 26 octets
/// - `SampleHeaderNameTruncated` si le payload n'est pas assez grand pour le nom annoncé
pub fn decode_sample_header(message: &SmdiMessage) -> Result<SampleHeader, SmdiError> {
    if message.code() != MSG_SAMPLE_HEADER {
        return Err(SmdiError::NotSampleHeader(message.message_id, message.sub_id));
    }
    let p = &message.payload;
    if p.len() < SAMPLE_HEADER_FIXED_PAYLOAD {
        return Err(SmdiError::SampleHeaderTooShort(p.len()));
    }
    let name_length = p[25] as usize;
    if p.len() < SAMPLE_HEADER_FIXED_PAYLOAD + name_length {
        return Err(SmdiError::SampleHeaderNameTruncated {
            name_len: name_length,
            total: p.len(),
        });
    }
    let name_bytes = &p[26..26 + name_length];
    // Python utilise `.decode("ascii", errors="replace")` qui remplace les
    // octets non-ASCII par U+FFFD. On reproduit en remplaçant chaque octet
    // > 0x7F par '?' (le character de remplacement Python est U+FFFD mais
    // cette fonction sert pour decode bytes → display ; les octets non-ASCII
    // ne devraient pas exister dans un nom A3000 valide).
    let name: String = name_bytes
        .iter()
        .map(|&b| if b.is_ascii() { b as char } else { '?' })
        .collect();
    Ok(SampleHeader {
        sample_number: read_u24_be(&p[0..3]),
        bits_per_word: p[3],
        channels: p[4],
        sample_period_ns: read_u24_be(&p[5..8]),
        sample_length_words: u32::from_be_bytes([p[8], p[9], p[10], p[11]]),
        loop_start: u32::from_be_bytes([p[12], p[13], p[14], p[15]]),
        loop_end: u32::from_be_bytes([p[16], p[17], p[18], p[19]]),
        loop_control: p[20],
        pitch_integer: p[21],
        pitch_fraction: read_u24_be(&p[22..25]),
        name,
    })
}

// ─── Encoders simples ─────────────────────────────────────────────────────

/// Encode un Sample Header Request (master demande au slave les métadonnées d'un slot).
///
/// # Errors
/// `Value24BitOutOfRange` si sample_number > 0xFF_FFFF.
pub fn encode_sample_header_request(sample_number: u32) -> Result<Vec<u8>, SmdiError> {
    let mut payload = Vec::with_capacity(3);
    write_u24_be(&mut payload, sample_number)?;
    encode_message(MSG_SAMPLE_HEADER_REQUEST.0, MSG_SAMPLE_HEADER_REQUEST.1, &payload)
}

/// Encode un Begin Sample Transfer (BST).
///
/// # Errors
/// `Value24BitOutOfRange` si sample_number ou data_packet_length > 0xFF_FFFF.
pub fn encode_begin_sample_transfer(
    sample_number: u32,
    data_packet_length: u32,
) -> Result<Vec<u8>, SmdiError> {
    if data_packet_length == 0 || data_packet_length > MAX_24BIT {
        return Err(SmdiError::Value24BitOutOfRange(data_packet_length));
    }
    let mut payload = Vec::with_capacity(6);
    write_u24_be(&mut payload, sample_number)?;
    write_u24_be(&mut payload, data_packet_length)?;
    encode_message(MSG_BEGIN_SAMPLE_TRANSFER.0, MSG_BEGIN_SAMPLE_TRANSFER.1, &payload)
}

/// Décode un Begin Sample Transfer Acknowledge.
///
/// # Errors
/// `NotBsta`, `BstaTooShort`.
pub fn decode_begin_sample_transfer_ack(message: &SmdiMessage) -> Result<(u32, u32), SmdiError> {
    if message.code() != MSG_BEGIN_SAMPLE_TRANSFER_ACK {
        return Err(SmdiError::NotBsta(message.message_id, message.sub_id));
    }
    if message.payload.len() < 6 {
        return Err(SmdiError::BstaTooShort(message.payload.len()));
    }
    let sn = read_u24_be(&message.payload[0..3]);
    let dpl = read_u24_be(&message.payload[3..6]);
    Ok((sn, dpl))
}

/// Encode un Send Next Packet (master demande au slave d'envoyer le packet N).
///
/// # Errors
/// `Value24BitOutOfRange` si packet_number > 0xFF_FFFF.
pub fn encode_send_next_packet(packet_number: u32) -> Result<Vec<u8>, SmdiError> {
    let mut payload = Vec::with_capacity(3);
    write_u24_be(&mut payload, packet_number)?;
    encode_message(MSG_SEND_NEXT_PACKET.0, MSG_SEND_NEXT_PACKET.1, &payload)
}

/// Décode un Send Next Packet → numéro du packet demandé.
///
/// # Errors
/// `NotSendNextPacket`, `SendNextPacketTooShort`.
pub fn decode_send_next_packet(message: &SmdiMessage) -> Result<u32, SmdiError> {
    if message.code() != MSG_SEND_NEXT_PACKET {
        return Err(SmdiError::NotSendNextPacket(message.message_id, message.sub_id));
    }
    if message.payload.len() < 3 {
        return Err(SmdiError::SendNextPacketTooShort(message.payload.len()));
    }
    Ok(read_u24_be(&message.payload[0..3]))
}

/// Encode un Data Packet (numéro + data).
///
/// # Errors
/// `Value24BitOutOfRange` si packet_number > 0xFF_FFFF, `PayloadTooLarge` si total > 0xFF_FFFF.
pub fn encode_data_packet(packet_number: u32, data: &[u8]) -> Result<Vec<u8>, SmdiError> {
    let mut payload = Vec::with_capacity(3 + data.len());
    write_u24_be(&mut payload, packet_number)?;
    payload.extend_from_slice(data);
    encode_message(MSG_DATA_PACKET.0, MSG_DATA_PACKET.1, &payload)
}

/// Décode un Data Packet → (packet_number, data slice).
///
/// # Errors
/// `NotDataPacket`, `DataPacketTooShort`.
pub fn decode_data_packet(message: &SmdiMessage) -> Result<(u32, &[u8]), SmdiError> {
    if message.code() != MSG_DATA_PACKET {
        return Err(SmdiError::NotDataPacket(message.message_id, message.sub_id));
    }
    if message.payload.len() < 3 {
        return Err(SmdiError::DataPacketTooShort(message.payload.len()));
    }
    let pn = read_u24_be(&message.payload[0..3]);
    Ok((pn, &message.payload[3..]))
}

#[must_use]
#[allow(clippy::expect_used)] // payload vide → encode_message infaillible
pub fn encode_abort_procedure() -> Vec<u8> {
    encode_message(MSG_ABORT_PROCEDURE.0, MSG_ABORT_PROCEDURE.1, &[])
        .expect("empty payload always fits")
}

#[must_use]
#[allow(clippy::expect_used)] // payload vide → encode_message infaillible
pub fn encode_end_of_procedure() -> Vec<u8> {
    encode_message(MSG_END_OF_PROCEDURE.0, MSG_END_OF_PROCEDURE.1, &[])
        .expect("empty payload always fits")
}

/// Encode un Delete Sample From Memory.
///
/// # Errors
/// `Value24BitOutOfRange` si sample_number > 0xFF_FFFF.
pub fn encode_delete_sample(sample_number: u32) -> Result<Vec<u8>, SmdiError> {
    let mut payload = Vec::with_capacity(3);
    write_u24_be(&mut payload, sample_number)?;
    encode_message(MSG_DELETE_SAMPLE.0, MSG_DELETE_SAMPLE.1, &payload)
}

/// Décode un Message Reject → (rejection_code, rejection_sub).
///
/// # Errors
/// `NotReject`, `RejectTooShort`.
pub fn decode_message_reject(message: &SmdiMessage) -> Result<(u16, u16), SmdiError> {
    if message.code() != MSG_REJECT {
        return Err(SmdiError::NotReject(message.message_id, message.sub_id));
    }
    if message.payload.len() < 4 {
        return Err(SmdiError::RejectTooShort(message.payload.len()));
    }
    let code = u16::from_be_bytes([message.payload[0], message.payload[1]]);
    let sub = u16::from_be_bytes([message.payload[2], message.payload[3]]);
    Ok((code, sub))
}

// ─── Tests unitaires ──────────────────────────────────────────────────────

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn encode_decode_roundtrip() {
        let raw = encode_message(0x0120, 0x0000, b"\x00\x00\x07").unwrap();
        let m = decode_message(&raw).unwrap();
        assert_eq!(m.message_id, 0x0120);
        assert_eq!(m.sub_id, 0x0000);
        assert_eq!(m.payload, b"\x00\x00\x07");
    }

    #[test]
    fn decode_rejects_truncated() {
        let too_short = b"SMDI\x01\x20\x00\x00\x00\x00\x05";
        let r = decode_message(too_short);
        assert!(matches!(r, Err(SmdiError::PayloadTruncated { .. })));
    }

    #[test]
    fn decode_rejects_missing_tag() {
        let bad = b"NOPE\x01\x20\x00\x00\x00\x00\x00";
        let r = decode_message(bad);
        assert!(matches!(r, Err(SmdiError::MissingTag(_))));
    }

    #[test]
    fn write_u24_rejects_overflow() {
        let mut buf = Vec::new();
        let r = write_u24_be(&mut buf, 0x0100_0000);
        assert!(matches!(r, Err(SmdiError::Value24BitOutOfRange(_))));
    }

    #[test]
    fn sample_header_roundtrip() {
        let h = SampleHeader {
            sample_number: 100,
            bits_per_word: 16,
            channels: 1,
            sample_period_ns: 22675,
            sample_length_words: 44100,
            loop_start: 0,
            loop_end: 44100,
            loop_control: 0,
            pitch_integer: 60,
            pitch_fraction: 0,
            name: "test_sample".into(),
        };
        let raw = encode_sample_header(&h).unwrap();
        let m = decode_message(&raw).unwrap();
        let h2 = decode_sample_header(&m).unwrap();
        assert_eq!(h, h2);
    }

    #[test]
    fn period_ns_44100() {
        // librosa-equivalent : 1_000_000_000 / 44100 ≈ 22675.7 → arrondi à 22676
        assert_eq!(period_ns_for_rate(44100), 22676);
    }
}
