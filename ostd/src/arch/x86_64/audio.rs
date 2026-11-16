//! x86_64 Audio Subsystem backed exclusively by the real Intel HDA DMA driver.
//!
//! Zero simulated audio or PIT speaker fallbacks: if no Intel HDA controller is present,
//! `write_audio()` and `read_audio()` return `Err(-19)` (`-ENODEV`).

use alloc::format;
use alloc::string::String;

/// Initializes the real Intel HDA PCIe DMA audio controller.
pub fn init() {
    crate::bus::intel_hda::init();
}

/// Plays raw PCM samples via Intel HDA DMA (`Err(-19)` / `-ENODEV` if no HDA hardware).
pub fn write_audio(data: &[u8]) -> Result<usize, i64> {
    crate::bus::intel_hda::play_pcm(data)
}

/// Captures PCM samples from Intel HDA input stream (`Err(-19)` / `-ENODEV` if no capture stream).
pub fn read_audio(dst: &mut [u8]) -> Result<usize, i64> {
    crate::bus::intel_hda::read_pcm(dst)
}

/// Returns a hardware status summary string for `/dev/audio`.
pub fn status_summary() -> String {
    let hda = crate::bus::intel_hda::info();
    if hda.present {
        format!(
            "FipSOs /dev/audio | Controller=Intel-HDA ({:#06x}:{:#06x}) | Codec={} DAC={} Pin={} | DMA_Bytes={}\n",
            hda.vendor_id, hda.device_id, hda.active_codec, hda.dac_nid, hda.pin_nid, hda.frames_played
        )
    } else {
        String::from("FipSOs /dev/audio | Controller=NotPresent (-ENODEV)\n")
    }
}
