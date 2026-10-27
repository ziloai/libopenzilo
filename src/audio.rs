// SPDX-License-Identifier: MPL-2.0
use crate::*;
#[derive(Clone, Debug, uniffi::Record)]
pub struct AudioSummary {
    pub channels: u16,
    pub sample_rate_hz: u32,
    pub samples_per_channel: u64,
    pub duration_ms: u64,
    pub packets: u32,
}
#[uniffi::export]
pub fn opus_recording_info(data: Vec<u8>) -> Result<AudioSummary> {
    let r = parse_opus_recording(data)?;
    let n = r.packets.len();
    Ok(AudioSummary {
        channels: r.channels,
        sample_rate_hz: r.sample_rate as u32,
        samples_per_channel: n as u64 * 320,
        duration_ms: n as u64 * 20,
        packets: u32::try_from(n).map_err(|_| invalid("Too many Opus packets"))?,
    })
}
fn ogg_page(packet: &[u8], flags: u8, granule: u64, serial: u32, sequence: u32) -> Result<Vec<u8>> {
    let segments = packet.len() / 255 + 1;
    if segments > 255 {
        return Err(invalid("Ogg packet exceeds page limit"));
    }
    let mut page = b"OggS\0".to_vec();
    page.push(flags);
    page.extend(granule.to_le_bytes());
    page.extend(serial.to_le_bytes());
    page.extend(sequence.to_le_bytes());
    page.extend([0; 4]);
    page.push(segments as u8);
    page.extend(std::iter::repeat_n(255, segments - 1));
    page.push((packet.len() % 255) as u8);
    page.extend(packet);
    let mut crc = 0u32;
    for b in &page {
        crc ^= (*b as u32) << 24;
        for _ in 0..8 {
            crc = if crc & 0x80000000 != 0 {
                (crc << 1) ^ 0x04c11db7
            } else {
                crc << 1
            };
        }
    }
    page[22..26].copy_from_slice(&crc.to_le_bytes());
    Ok(page)
}
#[uniffi::export]
pub fn export_ogg_opus(data: Vec<u8>, serial: u32) -> Result<Vec<u8>> {
    let r = parse_opus_recording(data)?;
    // OP2C has no encoder pre-skip metadata. Preserve all decoded samples.
    let mut head = b"OpusHead".to_vec();
    head.extend([1, r.channels as u8]);
    head.extend(0u16.to_le_bytes());
    head.extend(16000u32.to_le_bytes());
    head.extend([0, 0, 0]);
    let vendor = b"libopenzilo";
    let mut tags = b"OpusTags".to_vec();
    tags.extend((vendor.len() as u32).to_le_bytes());
    tags.extend(vendor);
    tags.extend(0u32.to_le_bytes());
    let mut result = ogg_page(&head, 2, 0, serial, 0)?;
    result.extend(ogg_page(
        &tags,
        if r.packets.is_empty() { 4 } else { 0 },
        0,
        serial,
        1,
    )?);
    for (i, p) in r.packets.iter().enumerate() {
        result.extend(ogg_page(
            p,
            if i + 1 == r.packets.len() { 4 } else { 0 },
            (i as u64 + 1) * 960,
            serial,
            (i + 2) as u32,
        )?);
    }
    Ok(result)
}

/// Legacy Speex packets are mono; platform decoders may resample/upmix output.
#[uniffi::export]
pub fn export_ogg_speex(
    packets: Vec<Vec<u8>>,
    sample_rate_hz: u32,
    serial: u32,
) -> Result<Vec<u8>> {
    let (mode, frame_samples) = match sample_rate_hz {
        8000 => (0i32, 160i32),
        16000 => (1, 320),
        32000 => (2, 640),
        _ => return Err(invalid("Speex supports 8000, 16000 or 32000 Hz")),
    };
    let mut header = b"Speex   speex-1.2.1".to_vec();
    header.resize(28, 0);
    for value in [
        1,
        80,
        sample_rate_hz as i32,
        mode,
        4,
        1,
        -1,
        frame_samples,
        0,
        1,
        0,
        0,
        0,
    ] {
        header.extend(value.to_le_bytes());
    }
    let vendor = b"libopenzilo";
    let mut tags = (vendor.len() as u32).to_le_bytes().to_vec();
    tags.extend(vendor);
    tags.extend(0u32.to_le_bytes());
    let mut result = ogg_page(&header, 2, 0, serial, 0)?;
    result.extend(ogg_page(
        &tags,
        if packets.is_empty() { 4 } else { 0 },
        0,
        serial,
        1,
    )?);
    for (i, packet) in packets.iter().enumerate() {
        if packet.is_empty() {
            return Err(invalid("Empty Speex packet"));
        }
        let sequence = u32::try_from(i + 2).map_err(|_| invalid("Too many Speex packets"))?;
        result.extend(ogg_page(
            packet,
            if i + 1 == packets.len() { 4 } else { 0 },
            (i as u64 + 1) * frame_samples as u64,
            serial,
            sequence,
        )?);
    }
    Ok(result)
}
#[derive(Clone, Debug, uniffi::Record)]
pub struct PcmFormat {
    pub sample_rate_hz: u32,
    pub channels: u16,
    pub bit_depth: u16,
}
#[uniffi::export]
pub fn export_wav_pcm(data: Vec<u8>, format: PcmFormat) -> Result<Vec<u8>> {
    if format.sample_rate_hz == 0
        || !(1..=2).contains(&format.channels)
        || ![8, 16].contains(&format.bit_depth)
    {
        return Err(invalid("Unsupported PCM format"));
    }
    let align = format.channels * format.bit_depth / 8;
    let padding = data.len() % 2;
    if !data.len().is_multiple_of(align as usize) || data.len() > u32::MAX as usize - 36 - padding {
        return Err(invalid("Incomplete PCM frame or WAV exceeds RIFF size"));
    }
    let byte_rate = format
        .sample_rate_hz
        .checked_mul(align as u32)
        .ok_or_else(|| invalid("PCM byte rate exceeds WAV format"))?;
    let mut out = b"RIFF".to_vec();
    out.extend((data.len() as u32 + 36 + padding as u32).to_le_bytes());
    out.extend(b"WAVEfmt ");
    out.extend(16u32.to_le_bytes());
    out.extend(1u16.to_le_bytes());
    out.extend(format.channels.to_le_bytes());
    out.extend(format.sample_rate_hz.to_le_bytes());
    out.extend(byte_rate.to_le_bytes());
    out.extend(align.to_le_bytes());
    out.extend(format.bit_depth.to_le_bytes());
    out.extend(b"data");
    out.extend((data.len() as u32).to_le_bytes());
    out.extend(data);
    if padding != 0 {
        out.push(0);
    }
    Ok(out)
}
/// Stable optional entry point; no native decoder is bundled in this build.
#[uniffi::export]
pub fn decode_opus_pcm(_data: Vec<u8>) -> Result<Vec<u8>> {
    Err(CoreError::Unavailable {
        detail: "opus_pcm_decoder".into(),
    })
}
