"""
Orchestrateur du transfert d'un sample WAV vers un slot du Yamaha A3000 via SMDI.

Séquence (master → slave) :
  1. SEND     Sample Header (0x0121)
  2. RECEIVE  Begin Sample Transfer Acknowledge (0x0122/0x0001)
  3. SEND     Begin Sample Transfer (0x0122/0x0000)
  4. boucle :
       RECEIVE  Send Next Packet (0x0103) → numéro du packet attendu
       SEND     Data Packet (0x0110) avec ce numéro et les octets PCM big-endian
  5. RECEIVE  End Of Procedure (0x0104)

En cas d'erreur à n'importe quelle étape, on tente un Abort Procedure pour ne
pas laisser le slot dans un état pendant.
"""
from __future__ import annotations

import array
import time
from dataclasses import dataclass
from typing import Callable, Optional

from .models import WavePayload
from .smdi import (
    LOOP_DISABLED,
    MSG_BEGIN_SAMPLE_TRANSFER_ACK,
    MSG_DATA_PACKET,
    MSG_END_OF_PROCEDURE,
    MSG_REJECT,
    MSG_SAMPLE_HEADER,
    MSG_SEND_NEXT_PACKET,
    PITCH_DEFAULT_FRACTION,
    PITCH_MIDDLE_C_INTEGER,
    SampleHeader,
    decode_begin_sample_transfer_ack,
    decode_data_packet,
    decode_message_reject,
    decode_sample_header,
    decode_send_next_packet,
    drain_pending_reply,
    encode_abort_procedure,
    encode_begin_sample_transfer,
    encode_data_packet,
    encode_sample_header,
    encode_sample_header_request,
    encode_send_next_packet,
    period_ns_for_rate,
    receive_smdi,
    send_smdi,
)


class SampleTransferError(Exception):
    pass


@dataclass(slots=True)
class TransferStats:
    sample_number: int
    bytes_sent: int
    packet_count: int
    packet_length: int


def pcm16_le_to_be(pcm_data: bytes) -> bytes:
    """Inverse l'endianness de chaque mot 16 bits (WAV LE → SMDI BE)."""
    if len(pcm_data) % 2:
        raise ValueError("PCM 16 bits attendu : longueur paire.")
    arr = array.array("h")
    arr.frombytes(pcm_data)
    arr.byteswap()
    return arr.tobytes()


def pcm16_be_to_le(pcm_data: bytes) -> bytes:
    """Inverse SMDI BE → WAV LE (même opération, byteswap)."""
    return pcm16_le_to_be(pcm_data)


def save_smdi_to_wav(path, header: SampleHeader, smdi_data: bytes) -> None:
    """Sauvegarde un sample SMDI (PCM 16-bit BE interleaved) en fichier WAV."""
    import wave
    if header.bits_per_word != 16:
        raise SampleTransferError(
            f"Export WAV: seul 16-bit supporté pour l'instant ({header.bits_per_word} bits reçus)."
        )
    le_data = pcm16_be_to_le(smdi_data)
    sample_rate = int(round(1_000_000_000 / header.sample_period_ns)) if header.sample_period_ns else 44100
    with wave.open(str(path), "wb") as w:
        w.setnchannels(header.channels)
        w.setsampwidth(header.bits_per_word // 8)
        w.setframerate(sample_rate)
        w.writeframes(le_data)


def transfer_sample(
    handle: int,
    *,
    path_id: int,
    target_id: int,
    lun: int,
    sample_number: int,
    wave: WavePayload,
    name: str = "",
    loop_control: int = LOOP_DISABLED,
    pitch_integer: int = PITCH_MIDDLE_C_INTEGER,
    pitch_fraction: int = PITCH_DEFAULT_FRACTION,
    preferred_packet_length: int = 4096,
    timeout_seconds: int = 30,
    progress: Optional[Callable[[int, int, int], None]] = None,
    dry_run: bool = False,
    verbose: bool = False,
    settle_seconds: float = 1.0,
) -> TransferStats:
    """Envoie `wave` dans le slot `sample_number` du sampler.

    Avec `dry_run=True`, s'arrête après réception du BSTA et envoie un Abort
    pour libérer le slot (validation du 2e handshake sans écriture mémoire).
    """
    if wave.bits_per_sample != 16:
        raise SampleTransferError("Seuls les WAV 16 bits sont supportés (le sampler attend 16-bit words).")
    if wave.channels not in (1, 2):
        raise SampleTransferError("Seuls les WAV mono ou stéréo sont supportés.")

    smdi_data = pcm16_le_to_be(wave.pcm_data)
    sample_length_words = wave.frame_count
    period_ns = period_ns_for_rate(wave.sample_rate)

    header = SampleHeader(
        sample_number=sample_number,
        bits_per_word=wave.bits_per_sample,
        channels=wave.channels,
        sample_period_ns=period_ns,
        sample_length_words=sample_length_words,
        loop_start=0,
        loop_end=max(0, sample_length_words - 1),
        loop_control=loop_control,
        pitch_integer=pitch_integer,
        pitch_fraction=pitch_fraction,
        name=name,
    )

    common = dict(path_id=path_id, target_id=target_id, lun=lun, timeout_seconds=timeout_seconds)
    aborted = False

    def _log(msg: str) -> None:
        if verbose:
            print(f"  [transfer] {msg}")

    def _abort() -> None:
        nonlocal aborted
        if aborted:
            return
        aborted = True
        try:
            send_smdi(handle, message=encode_abort_procedure(), **common)
            # Spec p24 : le slave répond par un Ack à un Abort Procedure du master.
            # On doit drainer cette réponse, sinon le slave reste avec une réponse
            # pending et rejette le prochain SEND avec sense 0x80.
            drain_pending_reply(handle, path_id=path_id, target_id=target_id, lun=lun)
        except Exception:
            pass

    try:
        # 0. Drain défensif : si une session précédente s'est mal terminée,
        # une réponse pending peut bloquer notre premier SEND avec sense 0x80.
        drained = drain_pending_reply(handle, path_id=path_id, target_id=target_id, lun=lun)
        if drained is not None:
            _log(f"drain consommé: code=0x{drained.message_id:04X}/0x{drained.sub_id:04X}")

        # 1. Sample Header → BSTA
        sh_msg = encode_sample_header(header)
        _log(f"SEND Sample Header ({len(sh_msg)} octets)")
        send_result = send_smdi(handle, message=sh_msg, **common)
        if send_result.scsi_status != 0:
            raise SampleTransferError(
                f"SEND Sample Header rejeté: ScsiStatus=0x{send_result.scsi_status:02X}, "
                f"sense={send_result.sense[:18].hex(' ')}"
            )

        _, reply = receive_smdi(handle, allocation_length=64, **common)
        if reply.code == MSG_REJECT:
            rej_code, rej_sub = decode_message_reject(reply)
            raise SampleTransferError(
                f"Sample Header refusé par le slave: reject 0x{rej_code:04X}/0x{rej_sub:04X}"
            )
        if reply.code != MSG_BEGIN_SAMPLE_TRANSFER_ACK:
            raise SampleTransferError(
                f"Réponse inattendue après Sample Header: 0x{reply.message_id:04X}/0x{reply.sub_id:04X}"
            )
        ack_sample_number, slave_max_dpl = decode_begin_sample_transfer_ack(reply)
        if ack_sample_number != sample_number:
            raise SampleTransferError(
                f"BSTA pointe vers sample {ack_sample_number} au lieu de {sample_number}."
            )
        _log(f"BSTA reçu: slave_max_dpl={slave_max_dpl}")

        # DPL : ≤ ce que le slave annonce, multiple de la frame size pour ne pas
        # split un sample word entre packets
        frame_size = wave.channels * (wave.bits_per_sample // 8)
        packet_length = min(preferred_packet_length, slave_max_dpl)
        packet_length -= packet_length % frame_size
        if packet_length < frame_size:
            raise SampleTransferError(
                f"Data Packet Length négocié trop petit ({packet_length}, frame_size={frame_size})."
            )

        if dry_run:
            _abort()
            return TransferStats(
                sample_number=sample_number,
                bytes_sent=0,
                packet_count=0,
                packet_length=packet_length,
            )

        # 2. Begin Sample Transfer
        bst_msg = encode_begin_sample_transfer(sample_number, packet_length)
        _log(f"SEND Begin Sample Transfer (DPL={packet_length})")
        send_result = send_smdi(handle, message=bst_msg, **common)
        if send_result.scsi_status != 0:
            raise SampleTransferError(
                f"SEND Begin Sample Transfer rejeté: sense={send_result.sense[:18].hex(' ')}"
            )

        # 3. Pré-encoder tous les Data Packets pour minimiser la latence dans la boucle
        data_packet_pool: list[bytes] = []
        offset = 0
        packet_num = 0
        while offset < len(smdi_data):
            chunk = smdi_data[offset:offset + packet_length]
            data_packet_pool.append(encode_data_packet(packet_num, chunk))
            offset += len(chunk)
            packet_num += 1
        _log(f"Pool de {len(data_packet_pool)} Data Packets pré-encodé")

        # 4. Boucle SNP / DP
        total = len(smdi_data)
        offset = 0
        packets_sent = 0
        expected_packet_num = 0

        while True:
            _, reply = receive_smdi(handle, allocation_length=64, **common)

            if reply.code == MSG_END_OF_PROCEDURE:
                if offset < total:
                    raise SampleTransferError(
                        f"End Of Procedure prématuré à l'offset {offset}/{total}."
                    )
                break

            if reply.code == MSG_REJECT:
                rej_code, rej_sub = decode_message_reject(reply)
                raise SampleTransferError(
                    f"Slave a rejeté pendant le transfert (offset {offset}): "
                    f"0x{rej_code:04X}/0x{rej_sub:04X}"
                )

            if reply.code != MSG_SEND_NEXT_PACKET:
                raise SampleTransferError(
                    f"Attendu Send Next Packet ou End Of Procedure, "
                    f"reçu 0x{reply.message_id:04X}/0x{reply.sub_id:04X}"
                )

            packet_num = decode_send_next_packet(reply)
            if packet_num != expected_packet_num:
                raise SampleTransferError(
                    f"Numéro de packet incohérent: slave demande #{packet_num}, "
                    f"attendu #{expected_packet_num}."
                )

            if offset >= total:
                # Tout envoyé : on attend EoP au prochain RECEIVE
                break

            if packet_num >= len(data_packet_pool):
                raise SampleTransferError(
                    f"Slave demande packet#{packet_num} hors pool ({len(data_packet_pool)} pré-encodés)."
                )
            pre_encoded = data_packet_pool[packet_num]
            chunk_size = len(pre_encoded) - 14  # 11 SMDI header + 3 packet#
            send_result = send_smdi(handle, message=pre_encoded, **common)
            if send_result.scsi_status != 0:
                raise SampleTransferError(
                    f"SEND Data Packet #{packet_num} rejeté: sense={send_result.sense[:18].hex(' ')}"
                )
            offset += chunk_size
            packets_sent += 1
            expected_packet_num += 1
            if progress:
                progress(packets_sent, offset, total)

        # 5. Settle post-EoP : laisser le slave finaliser l'écriture en RAM
        if settle_seconds > 0:
            _log(f"settle {settle_seconds}s post-EoP")
            time.sleep(settle_seconds)

        return TransferStats(
            sample_number=sample_number,
            bytes_sent=offset,
            packet_count=packets_sent,
            packet_length=packet_length,
        )

    except Exception:
        _abort()
        raise


def receive_sample(
    handle: int,
    *,
    path_id: int,
    target_id: int,
    lun: int,
    sample_number: int,
    preferred_packet_length: int = 4096,
    timeout_seconds: int = 30,
    progress: Optional[Callable[[int, int, int], None]] = None,
    verbose: bool = False,
    settle_seconds: float = 0.5,
) -> tuple[SampleHeader, bytes]:
    """Récupère un sample depuis le sampler. Retourne (SampleHeader, PCM 16-bit BE).

    Séquence (slave → master) :
      1. SEND     Sample Header Request → slave répond Sample Header (metadata)
      2. SEND     Begin Sample Transfer (DPL = max acceptable par master)
      3. RECEIVE  Begin Sample Transfer Acknowledge (DPL effectif côté slave)
      4. boucle :
           SEND     Send Next Packet
           RECEIVE  Data Packet
      5. quand on a length × channels × bytes_per_word octets, on s'arrête
    """
    common = dict(path_id=path_id, target_id=target_id, lun=lun, timeout_seconds=timeout_seconds)

    def _log(msg: str) -> None:
        if verbose:
            print(f"  [receive] {msg}")

    drain_pending_reply(handle, path_id=path_id, target_id=target_id, lun=lun)

    # 1. Sample Header Request
    send_smdi(handle, message=encode_sample_header_request(sample_number), **common)
    _, reply = receive_smdi(handle, allocation_length=4096, **common)
    if reply.code == MSG_REJECT:
        rej_code, rej_sub = decode_message_reject(reply)
        raise SampleTransferError(
            f"Sample Header Request refusé : 0x{rej_code:04X}/0x{rej_sub:04X}"
        )
    if reply.code != MSG_SAMPLE_HEADER:
        raise SampleTransferError(
            f"Réponse inattendue (Sample Header attendu) : "
            f"0x{reply.message_id:04X}/0x{reply.sub_id:04X}"
        )
    header = decode_sample_header(reply)
    _log(f"Header reçu : {header.name!r}, {header.channels}ch, "
         f"{header.bits_per_word}b, {header.sample_length_words} words")

    if header.bits_per_word != 16:
        raise SampleTransferError(
            f"Réception : seul 16-bit supporté ({header.bits_per_word}b reçu)."
        )
    if header.channels not in (1, 2):
        raise SampleTransferError(f"Réception : seuls mono/stéréo supportés ({header.channels} canaux).")

    frame_size = header.channels * (header.bits_per_word // 8)
    total_bytes = header.sample_length_words * frame_size

    # 2. Begin Sample Transfer (master demande le max qu'il peut prendre)
    requested_dpl = preferred_packet_length - (preferred_packet_length % frame_size)
    if requested_dpl < frame_size:
        requested_dpl = frame_size
    send_smdi(handle, message=encode_begin_sample_transfer(sample_number, requested_dpl), **common)

    # 3. BSTA avec DPL effectif
    _, reply = receive_smdi(handle, allocation_length=64, **common)
    if reply.code == MSG_REJECT:
        rej_code, rej_sub = decode_message_reject(reply)
        raise SampleTransferError(f"BST refusé : 0x{rej_code:04X}/0x{rej_sub:04X}")
    if reply.code != MSG_BEGIN_SAMPLE_TRANSFER_ACK:
        raise SampleTransferError(
            f"BSTA attendu, reçu 0x{reply.message_id:04X}/0x{reply.sub_id:04X}"
        )
    ack_sample_number, slave_dpl = decode_begin_sample_transfer_ack(reply)
    if ack_sample_number != sample_number:
        raise SampleTransferError(
            f"BSTA pointe vers sample {ack_sample_number} au lieu de {sample_number}."
        )
    actual_dpl = min(requested_dpl, slave_dpl)
    actual_dpl -= actual_dpl % frame_size
    if actual_dpl < frame_size:
        raise SampleTransferError(f"DPL négocié trop petit : {actual_dpl}.")
    _log(f"BSTA: slave_dpl={slave_dpl}, actual_dpl={actual_dpl}")

    # 4. Boucle SNP master / DP slave
    received = bytearray()
    packet_num = 0
    receive_alloc = 14 + actual_dpl + 16  # 11 SMDI hdr + 3 packet# + dpl + safety

    while len(received) < total_bytes:
        send_smdi(handle, message=encode_send_next_packet(packet_num), **common)
        _, reply = receive_smdi(handle, allocation_length=receive_alloc, **common)

        if reply.code == MSG_END_OF_PROCEDURE:
            _log(f"EoP reçu à offset {len(received)}/{total_bytes}")
            break
        if reply.code == MSG_REJECT:
            rej_code, rej_sub = decode_message_reject(reply)
            raise SampleTransferError(
                f"Slave a rejeté pendant la réception : 0x{rej_code:04X}/0x{rej_sub:04X}"
            )
        if reply.code != MSG_DATA_PACKET:
            raise SampleTransferError(
                f"Data Packet attendu, reçu 0x{reply.message_id:04X}/0x{reply.sub_id:04X}"
            )
        recv_packet_num, chunk = decode_data_packet(reply)
        if recv_packet_num != packet_num:
            raise SampleTransferError(
                f"Numéro de packet incohérent : reçu #{recv_packet_num}, attendu #{packet_num}."
            )

        # Tronquer si on dépasse total_bytes (dernière trame potentiellement partielle)
        remaining = total_bytes - len(received)
        if len(chunk) > remaining:
            chunk = chunk[:remaining]
        received.extend(chunk)
        packet_num += 1
        if progress:
            progress(packet_num, len(received), total_bytes)

    if settle_seconds > 0:
        time.sleep(settle_seconds)

    return header, bytes(received)
