from __future__ import annotations

from dataclasses import asdict, dataclass


@dataclass(slots=True)
class ScsiTargetInfo:
    host_adapter: int
    path_id: int
    target_id: int
    lun: int
    vendor: str
    product: str
    revision: str
    device_type: int
    device_claimed: bool = False

    @property
    def display_name(self) -> str:
        claimed = " claimed" if self.device_claimed else ""
        return (
            f"HA{self.host_adapter} BUS{self.path_id} ID{self.target_id} "
            f"LUN{self.lun} {self.vendor} {self.product} {self.revision}{claimed}"
        ).strip()

    def to_dict(self) -> dict:
        data = asdict(self)
        data["display_name"] = self.display_name
        return data


@dataclass(slots=True)
class WavePayload:
    path: str
    channels: int
    sample_rate: int
    bits_per_sample: int
    frame_count: int
    byte_count: int
    pcm_data: bytes

    def to_dict(self) -> dict:
        return {
            "path": self.path,
            "channels": self.channels,
            "sample_rate": self.sample_rate,
            "bits_per_sample": self.bits_per_sample,
            "frame_count": self.frame_count,
            "byte_count": self.byte_count,
        }
