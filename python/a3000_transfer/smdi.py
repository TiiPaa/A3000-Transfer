"""
Encodeur/décodeur de messages SMDI (Peavey 1992 v0.03) et helpers SCSI
SEND (0x0A) / RECEIVE (0x08) au-dessus du pass-through.

Format d'un message SMDI :
    "SMDI"            (4 octets ASCII)
    Message ID        (2 octets, big-endian)
    Message Sub-ID    (2 octets, big-endian)
    Additional Length (3 octets, big-endian) — taille du payload qui suit
    payload           (Additional Length octets)

Tous les multi-octets sont en big-endian (= ordre SCSI standard).
"""
from __future__ import annotations

import time
from dataclasses import dataclass

from .scsi_passthrough import PassThroughResult, send_cdb

SMDI_TAG = b"SMDI"
SMDI_HEADER_SIZE = 11  # tag(4) + msg_id(2) + sub_id(2) + add_len(3)
SMDI_MIN_TRANSFER = 11  # spec p41: SEND avec Transfer Length < 11 est rejeté

# Message ID / Sub-ID (Table 1, p39)
MSG_NO_MESSAGE = (0x0000, 0x0000)
MSG_MASTER_IDENTIFY = (0x0001, 0x0000)
MSG_SLAVE_IDENTIFY = (0x0001, 0x0001)
MSG_REJECT = (0x0002, 0x0000)
MSG_ACK = (0x0100, 0x0000)
MSG_NAK = (0x0101, 0x0000)
MSG_WAIT = (0x0102, 0x0000)
MSG_SEND_NEXT_PACKET = (0x0103, 0x0000)
MSG_END_OF_PROCEDURE = (0x0104, 0x0000)
MSG_ABORT_PROCEDURE = (0x0105, 0x0000)
MSG_DATA_PACKET = (0x0110, 0x0000)
MSG_SAMPLE_HEADER_REQUEST = (0x0120, 0x0000)
MSG_SAMPLE_HEADER = (0x0121, 0x0000)
MSG_BEGIN_SAMPLE_TRANSFER = (0x0122, 0x0000)
MSG_BEGIN_SAMPLE_TRANSFER_ACK = (0x0122, 0x0001)
MSG_SAMPLE_NAME = (0x0123, 0x0000)
MSG_DELETE_SAMPLE = (0x0124, 0x0000)
MSG_TRANSMIT_MIDI = (0x0200, 0x0000)


@dataclass(slots=True)
class SmdiMessage:
    message_id: int
    sub_id: int
    payload: bytes

    @property
    def code(self) -> tuple[int, int]:
        return (self.message_id, self.sub_id)

    def to_bytes(self) -> bytes:
        return encode_message(self.message_id, self.sub_id, self.payload)


class SmdiProtocolError(Exception):
    pass


class SmdiNoReplyPendingError(SmdiProtocolError):
    """Sense 0x81 = "RECEIVE rejected - no slave reply message is pending".
    Sur Yamaha A3000 c'est le code retourné quand on interroge un slot vide
    (au lieu d'un Reject 0x0020/0x0002 standard SMDI)."""
    pass


def encode_message(message_id: int, sub_id: int, payload: bytes = b"") -> bytes:
    add_len = len(payload)
    if add_len > 0xFFFFFF:
        raise ValueError("Payload SMDI trop grand (max 0xFFFFFF octets).")
    buf = bytearray()
    buf += SMDI_TAG
    buf += message_id.to_bytes(2, "big")
    buf += sub_id.to_bytes(2, "big")
    buf += add_len.to_bytes(3, "big")
    buf += payload
    return bytes(buf)


def decode_message(raw: bytes) -> SmdiMessage:
    if len(raw) < SMDI_HEADER_SIZE:
        raise SmdiProtocolError(f"Trame SMDI trop courte: {len(raw)} octets, minimum {SMDI_HEADER_SIZE}.")
    if raw[:4] != SMDI_TAG:
        raise SmdiProtocolError(f"Tag SMDI absent (reçu {raw[:4]!r}).")
    message_id = int.from_bytes(raw[4:6], "big")
    sub_id = int.from_bytes(raw[6:8], "big")
    add_len = int.from_bytes(raw[8:11], "big")
    payload_end = SMDI_HEADER_SIZE + add_len
    if len(raw) < payload_end:
        raise SmdiProtocolError(
            f"Trame tronquée: header annonce {add_len} octets de payload, reçu {len(raw) - SMDI_HEADER_SIZE}."
        )
    payload = raw[SMDI_HEADER_SIZE:payload_end]
    return SmdiMessage(message_id=message_id, sub_id=sub_id, payload=payload)


def build_scsi_send_cdb(transfer_length: int) -> bytes:
    """SCSI SEND (opcode 0x0A), 6-byte CDB."""
    if transfer_length < 0 or transfer_length > 0xFFFFFF:
        raise ValueError("Transfer length doit tenir sur 24 bits.")
    return bytes([
        0x0A,
        0x00,
        (transfer_length >> 16) & 0xFF,
        (transfer_length >> 8) & 0xFF,
        transfer_length & 0xFF,
        0x00,
    ])


def build_scsi_receive_cdb(allocation_length: int) -> bytes:
    """SCSI RECEIVE (opcode 0x08), 6-byte CDB."""
    if allocation_length < 0 or allocation_length > 0xFFFFFF:
        raise ValueError("Allocation length doit tenir sur 24 bits.")
    return bytes([
        0x08,
        0x00,
        (allocation_length >> 16) & 0xFF,
        (allocation_length >> 8) & 0xFF,
        allocation_length & 0xFF,
        0x00,
    ])


def send_smdi(
    handle: int,
    *,
    path_id: int,
    target_id: int,
    lun: int,
    message: bytes,
    timeout_seconds: int = 30,
) -> PassThroughResult:
    """Envoie un message SMDI complet via SCSI SEND."""
    if len(message) < SMDI_MIN_TRANSFER:
        raise ValueError(f"Message SMDI < {SMDI_MIN_TRANSFER} octets: rejeté par le slave (sense 82H).")
    cdb = build_scsi_send_cdb(len(message))
    return send_cdb(
        handle,
        path_id=path_id,
        target_id=target_id,
        lun=lun,
        cdb=cdb,
        data_out=message,
        timeout_seconds=timeout_seconds,
    )


def receive_smdi(
    handle: int,
    *,
    path_id: int,
    target_id: int,
    lun: int,
    allocation_length: int = 4096,
    timeout_seconds: int = 30,
    retries: int = 5,
    wait_seconds: float = 0.5,
    max_wait_messages: int = 10,
) -> tuple[PassThroughResult, SmdiMessage]:
    """Lit un message SMDI via SCSI RECEIVE et le décode.

    Retry automatique sur sense key 0x0B (ABORTED COMMAND) — transitoire slave.
    Boucle sur message Wait (0x0102) — slave demande plus de temps (spec p21).
    """
    if allocation_length < SMDI_MIN_TRANSFER:
        raise ValueError(f"Allocation length < {SMDI_MIN_TRANSFER}: rejeté par le slave (sense 83H).")
    cdb = build_scsi_receive_cdb(allocation_length)
    last_status = 0
    last_sense = b""
    wait_count = 0

    while True:
        for attempt in range(retries):
            result = send_cdb(
                handle,
                path_id=path_id,
                target_id=target_id,
                lun=lun,
                cdb=cdb,
                data_in_length=allocation_length,
                timeout_seconds=timeout_seconds,
            )
            if result.scsi_status == 0:
                message = decode_message(result.data)
                if message.code == MSG_WAIT:
                    wait_count += 1
                    if wait_count > max_wait_messages:
                        raise SmdiProtocolError(
                            f"Trop de Wait messages reçus ({wait_count}), abandon."
                        )
                    time.sleep(wait_seconds)
                    break  # break inner retry loop, redo outer to receive again
                return result, message

            last_status = result.scsi_status
            last_sense = result.sense
            sense_key = result.sense[2] & 0x0F if len(result.sense) >= 3 else 0
            asc = result.sense[12] if len(result.sense) >= 13 else 0
            if asc == 0x81:
                raise SmdiNoReplyPendingError(
                    f"RECEIVE: pas de réponse pending (sense 0x81). "
                    f"Sur Yamaha A3000 indique typiquement un slot vide."
                )
            if sense_key == 0x0B:
                time.sleep(0.05 * (attempt + 1))
                continue
            raise SmdiProtocolError(
                f"RECEIVE a retourné ScsiStatus 0x{last_status:02X} après {attempt + 1} essais, "
                f"sense={last_sense[:14].hex(' ')}"
            )
        else:
            # Loop ended without break → all retries exhausted on sense 0x0B
            raise SmdiProtocolError(
                f"RECEIVE a retourné ScsiStatus 0x{last_status:02X} après {retries} essais (sense 0x0B), "
                f"sense={last_sense[:14].hex(' ')}"
            )


def drain_pending_reply(
    handle: int,
    *,
    path_id: int,
    target_id: int,
    lun: int,
    allocation_length: int = 4096,
    timeout_seconds: int = 5,
    retries: int = 3,
) -> SmdiMessage | None:
    """RECEIVE défensif pour vider une réponse pending côté slave.

    Tolère :
      - sense 0x81 (SMDI table 3) : "no slave reply message is pending" — bus propre
      - sense key 0x0B (SCSI standard) : "ABORTED COMMAND" — slave en cleanup transitoire
    Avec retries pour absorber un état transitoire après échec précédent.
    """
    cdb = build_scsi_receive_cdb(allocation_length)
    last_status = 0
    last_sense = b""

    for attempt in range(retries):
        result = send_cdb(
            handle,
            path_id=path_id,
            target_id=target_id,
            lun=lun,
            cdb=cdb,
            data_in_length=allocation_length,
            timeout_seconds=timeout_seconds,
        )
        if result.scsi_status == 0:
            try:
                return decode_message(result.data)
            except SmdiProtocolError:
                return None

        last_status = result.scsi_status
        last_sense = result.sense
        sense_key = result.sense[2] & 0x0F if len(result.sense) >= 3 else 0
        asc = result.sense[12] if len(result.sense) >= 13 else 0

        if asc == 0x81:
            return None
        if sense_key == 0x0B:
            time.sleep(0.05)
            continue
        break

    raise SmdiProtocolError(
        f"drain RECEIVE échec inattendu après {retries} essais: "
        f"status=0x{last_status:02X} sense={last_sense[:14].hex(' ')}"
    )


def master_identify_message() -> bytes:
    msg_id, sub_id = MSG_MASTER_IDENTIFY
    return encode_message(msg_id, sub_id)


# === Sample Header (Message ID 0x0121) ===========================================
# Layout du payload (26 octets fixes + nom) — spec Peavey + OpenSMDI, big-endian partout.
# Confirmé par projet jumeau Sampletrans qui réussit le transfert complet.
#
#   off  size  field
#   0    3     Sample Number (24-bit BE)
#   3    1     Bits Per Word
#   4    1     Number Of Channels
#   5    3     Sample Period (24-bit BE, ns)  ← 3 octets, pas 4
#   8    4     Sample Length (words par canal)
#   12   4     Sample Loop Start (word number)
#   16   4     Sample Loop End (word number)
#   20   1     Sample Loop Control
#   21   1     Sample Pitch Integer  (note MIDI 0..127, 0x3C = middle C)
#   22   3     Sample Pitch Fraction (24-bit BE)  ← 3 octets, pas 2
#   25   1     Sample Name Length (n)
#   26   n     Sample Name (ASCII)
#
# Erreur historique du projet : on lisait Period sur 4 octets et Pitch Fraction sur 2.
# Le total restait 26 octets, mais tous les champs après Period étaient décalés d'1
# octet, ce qui faisait que le slave lisait Length à un mauvais offset (≈ 624 mots
# au lieu des 159784 qu'on déclarait), expliquant l'EoP prématuré à ~4 KB.

SAMPLE_HEADER_FIXED_PAYLOAD = 26

LOOP_FORWARD = 0x00
LOOP_BACKWARD = 0x01
LOOP_DISABLED = 0x7F

PITCH_MIDDLE_C_INTEGER = 0x3C  # 1 octet, note MIDI
PITCH_DEFAULT_FRACTION = 0x0000

# Rejection codes (Table 2, p40)
REJECT_GENERAL = 0x0002
REJECT_DEVICE_BUSY = 0x0005
REJECT_PACKET_NUMBER_MISMATCH = 0x0011
REJECT_SAMPLE = 0x0020
REJECT_BEGIN_TRANSFER = 0x0022


@dataclass(slots=True)
class SampleHeader:
    sample_number: int
    bits_per_word: int
    channels: int
    sample_period_ns: int  # 24-bit BE, période entre 2 samples en nanosecondes
    sample_length_words: int
    loop_start: int
    loop_end: int
    loop_control: int
    pitch_integer: int
    pitch_fraction: int
    name: str

    @property
    def sample_rate_hz(self) -> float:
        return 1_000_000_000 / self.sample_period_ns if self.sample_period_ns else 0.0


# Le champ Sample Period dans le Sample Header est documenté par la spec Peavey
# comme "nanoseconds" sur 24 bits BE (3 octets). Pour 44.1 kHz : 22676 ns. Pour
# 48 kHz : 20833 ns.
def period_ns_for_rate(sample_rate_hz: int) -> int:
    return round(1_000_000_000 / sample_rate_hz)


def encode_sample_header(header: SampleHeader) -> bytes:
    name_bytes = header.name.encode("ascii", errors="replace")
    if len(name_bytes) > 255:
        raise ValueError("Nom de sample trop long (max 255 octets ASCII).")

    payload = bytearray()
    payload += header.sample_number.to_bytes(3, "big")
    payload += header.bits_per_word.to_bytes(1, "big")
    payload += header.channels.to_bytes(1, "big")
    payload += header.sample_period_ns.to_bytes(3, "big")        # 3 octets BE
    payload += header.sample_length_words.to_bytes(4, "big")
    payload += header.loop_start.to_bytes(4, "big")
    payload += header.loop_end.to_bytes(4, "big")
    payload += header.loop_control.to_bytes(1, "big")
    payload += header.pitch_integer.to_bytes(1, "big")
    payload += header.pitch_fraction.to_bytes(3, "big")          # 3 octets BE
    payload += len(name_bytes).to_bytes(1, "big")
    payload += name_bytes

    msg_id, sub_id = MSG_SAMPLE_HEADER
    return encode_message(msg_id, sub_id, bytes(payload))


def decode_sample_header(message: SmdiMessage) -> SampleHeader:
    if message.code != MSG_SAMPLE_HEADER:
        raise SmdiProtocolError(
            f"Pas un Sample Header (id=0x{message.message_id:04X}/0x{message.sub_id:04X})."
        )
    p = message.payload
    if len(p) < SAMPLE_HEADER_FIXED_PAYLOAD:
        raise SmdiProtocolError(
            f"Sample Header trop court: {len(p)} octets, attendu ≥ {SAMPLE_HEADER_FIXED_PAYLOAD}."
        )
    name_length = p[25]
    if len(p) < SAMPLE_HEADER_FIXED_PAYLOAD + name_length:
        raise SmdiProtocolError(
            f"Sample Header tronqué: nom annoncé {name_length} octets, payload total {len(p)}."
        )
    return SampleHeader(
        sample_number=int.from_bytes(p[0:3], "big"),
        bits_per_word=p[3],
        channels=p[4],
        sample_period_ns=int.from_bytes(p[5:8], "big"),          # 3 octets BE
        sample_length_words=int.from_bytes(p[8:12], "big"),
        loop_start=int.from_bytes(p[12:16], "big"),
        loop_end=int.from_bytes(p[16:20], "big"),
        loop_control=p[20],
        pitch_integer=p[21],
        pitch_fraction=int.from_bytes(p[22:25], "big"),          # 3 octets BE
        name=p[26:26 + name_length].decode("ascii", errors="replace"),
    )


def encode_sample_header_request(sample_number: int) -> bytes:
    msg_id, sub_id = MSG_SAMPLE_HEADER_REQUEST
    return encode_message(msg_id, sub_id, sample_number.to_bytes(3, "big"))


def is_sample_slot_occupied(
    handle: int,
    *,
    path_id: int,
    target_id: int,
    lun: int,
    sample_number: int,
) -> bool:
    """Vérifie si un slot contient un sample en faisant un Sample Header Request.

    Renvoie True si le slot est occupé, False s'il est vide.
    Sur Yamaha A3000 v0200, un slot vide se manifeste par sense 0x81 (no slave
    reply pending) plutôt que par un Reject standard SMDI.
    """
    send_smdi(
        handle, message=encode_sample_header_request(sample_number),
        path_id=path_id, target_id=target_id, lun=lun,
    )
    try:
        _, reply = receive_smdi(
            handle, allocation_length=4096,
            path_id=path_id, target_id=target_id, lun=lun,
        )
    except SmdiNoReplyPendingError:
        return False
    if reply.code == MSG_REJECT:
        try:
            rej_code, rej_sub = decode_message_reject(reply)
        except SmdiProtocolError:
            return False
        if (rej_code, rej_sub) == (0x0020, 0x0002):  # no sample at this Sample Number
            return False
    return reply.code == MSG_SAMPLE_HEADER


def find_first_free_sample_number(
    handle: int,
    *,
    path_id: int,
    target_id: int,
    lun: int,
    start: int = 7,
    limit: int = 256,
) -> int:
    """Scanne les slots à partir de `start` et retourne le 1er libre.

    Default `start=7` car les slots 0..6 sont des samples factory ROM sur A3000.
    """
    for n in range(start, start + limit):
        if not is_sample_slot_occupied(
            handle, path_id=path_id, target_id=target_id, lun=lun, sample_number=n
        ):
            return n
    raise SmdiProtocolError(
        f"Aucun slot libre dans la plage {start}..{start + limit - 1}."
    )


def decode_message_reject(message: SmdiMessage) -> tuple[int, int]:
    if message.code != MSG_REJECT:
        raise SmdiProtocolError(
            f"Pas un Message Reject (id=0x{message.message_id:04X}/0x{message.sub_id:04X})."
        )
    if len(message.payload) < 4:
        raise SmdiProtocolError(f"Message Reject incomplet: {len(message.payload)} octets, attendu 4.")
    rejection_code = int.from_bytes(message.payload[0:2], "big")
    rejection_sub = int.from_bytes(message.payload[2:4], "big")
    return rejection_code, rejection_sub


# === Begin Sample Transfer (master→slave) ========================================
# AL=6. Même structure que BSTA (0x0122/0x0001), juste un sub-ID différent : SN(3)+DPL(3).
# La spec p33 annonce AL=8 mais le slave Yamaha A3000 rejette avec sense 0x86 sur cette
# longueur ; AL=6 (mirror du BSTA) passe.
def encode_begin_sample_transfer(sample_number: int, data_packet_length: int) -> bytes:
    if data_packet_length < 1 or data_packet_length > 0xFFFFFF:
        raise ValueError("data_packet_length doit tenir sur 24 bits.")
    payload = sample_number.to_bytes(3, "big") + data_packet_length.to_bytes(3, "big")
    msg_id, sub_id = MSG_BEGIN_SAMPLE_TRANSFER
    return encode_message(msg_id, sub_id, payload)


# === Begin Sample Transfer Acknowledge (slave→master) ============================
# AL=6. Layout : SN(3) + DPL(3).
def decode_begin_sample_transfer_ack(message: SmdiMessage) -> tuple[int, int]:
    if message.code != MSG_BEGIN_SAMPLE_TRANSFER_ACK:
        raise SmdiProtocolError(
            f"Pas un BSTA (id=0x{message.message_id:04X}/0x{message.sub_id:04X})."
        )
    if len(message.payload) < 6:
        raise SmdiProtocolError(f"BSTA tronqué: {len(message.payload)} octets, attendu ≥ 6.")
    sample_number = int.from_bytes(message.payload[0:3], "big")
    data_packet_length = int.from_bytes(message.payload[3:6], "big")
    return sample_number, data_packet_length


# === Send Next Packet ============================================================
# AL=3. Payload : packet_number(3 octets BE).
def encode_send_next_packet(packet_number: int) -> bytes:
    msg_id, sub_id = MSG_SEND_NEXT_PACKET
    return encode_message(msg_id, sub_id, packet_number.to_bytes(3, "big"))


def decode_send_next_packet(message: SmdiMessage) -> int:
    if message.code != MSG_SEND_NEXT_PACKET:
        raise SmdiProtocolError(
            f"Pas un Send Next Packet (id=0x{message.message_id:04X}/0x{message.sub_id:04X})."
        )
    if len(message.payload) < 3:
        raise SmdiProtocolError(f"Send Next Packet tronqué: {len(message.payload)} octets, attendu 3.")
    return int.from_bytes(message.payload[0:3], "big")


# === Data Packet =================================================================
# AL = 3 + n. Payload : packet_number(3 octets BE) + data(n octets).
def decode_data_packet(message: SmdiMessage) -> tuple[int, bytes]:
    """Décode un Data Packet : retourne (packet_number, data)."""
    if message.code != MSG_DATA_PACKET:
        raise SmdiProtocolError(
            f"Pas un Data Packet (id=0x{message.message_id:04X}/0x{message.sub_id:04X})."
        )
    if len(message.payload) < 3:
        raise SmdiProtocolError(f"Data Packet trop court: {len(message.payload)} octets, attendu ≥ 3.")
    packet_number = int.from_bytes(message.payload[0:3], "big")
    return packet_number, bytes(message.payload[3:])


def encode_data_packet(packet_number: int, data: bytes) -> bytes:
    payload = bytearray()
    payload += packet_number.to_bytes(3, "big")
    payload += data
    msg_id, sub_id = MSG_DATA_PACKET
    return encode_message(msg_id, sub_id, bytes(payload))


# === Abort Procedure / End Of Procedure ==========================================
def encode_abort_procedure() -> bytes:
    return encode_message(*MSG_ABORT_PROCEDURE)


def encode_end_of_procedure() -> bytes:
    return encode_message(*MSG_END_OF_PROCEDURE)


# === Delete Sample From Memory (master→slave) ====================================
# Spec p32 : AL=3, payload = Sample Number (24-bit BE).
# Réponse attendue : End Of Procedure (avec éventuel Wait intermédiaire).
def encode_delete_sample(sample_number: int) -> bytes:
    msg_id, sub_id = MSG_DELETE_SAMPLE
    return encode_message(msg_id, sub_id, sample_number.to_bytes(3, "big"))
