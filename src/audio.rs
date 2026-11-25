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
#[uniffi::export]
pub fn is_wav(data: Vec<u8>) -> bool {
    data.len() >= 12 && &data[..4] == b"RIFF" && &data[8..12] == b"WAVE"
}
#[uniffi::export]
pub fn is_ogg_speex(data: Vec<u8>) -> bool {
    data.len() >= 36
        && data.starts_with(b"OggS")
        && data[..data.len().min(128)]
            .windows(8)
            .any(|w| w == b"Speex   ")
}
fn speex_block(data: &[u8], max_packet_size: usize) -> Option<Vec<Vec<u8>>> {
    let mut packets = Vec::new();
    let mut offset = 0;
    while data.len() - offset >= 2 {
        let size = u16::from_le_bytes([data[offset], data[offset + 1]]) as usize;
        if size == 0 {
            break;
        }
        if size > max_packet_size || size > data.len() - offset - 2 {
            return None;
        }
        offset += 2;
        packets.push(data[offset..offset + size].to_vec());
        offset += size;
    }
    if packets.is_empty() || data[offset..].iter().any(|b| ![0, 255].contains(b)) {
        return None;
    }
    Some(packets)
}
#[uniffi::export]
pub fn parse_packetized_speex_stream(
    data: Vec<u8>,
    max_packet_size: u32,
    framed_block_size: Option<u32>,
) -> Result<Option<Vec<Vec<u8>>>> {
    if max_packet_size == 0 || framed_block_size == Some(0) {
        return Err(invalid("Speex packet and block sizes must be positive"));
    }
    if let Some(packets) = speex_block(&data, max_packet_size as usize) {
        return Ok(Some(packets));
    }
    let Some(block_size) = framed_block_size.map(|v| v as usize) else {
        return Ok(None);
    };
    if data.len() <= block_size {
        return Ok(None);
    }
    let mut packets = Vec::new();
    for block in data.chunks(block_size) {
        let Some(mut block_packets) = speex_block(block, max_packet_size as usize) else {
            return Ok(None);
        };
        packets.append(&mut block_packets);
    }
    Ok(Some(packets))
}
#[uniffi::export]
pub fn speex_packet_size(quality: u8, bits_size: Option<u32>) -> Result<u32> {
    if let Some(size) = bits_size {
        if size == 0 {
            return Err(invalid("Speex packet size must be positive"));
        }
        return Ok(size);
    }
    Ok(match quality {
        1 => 10,
        2 => 15,
        5 | 6 => 28,
        7 | 8 => 38,
        9 | 10 => 46,
        _ => 20,
    })
}
#[uniffi::export]
pub fn split_raw_speex_packets(
    data: Vec<u8>,
    quality: u8,
    bits_size: Option<u32>,
) -> Result<Option<Vec<Vec<u8>>>> {
    let size = speex_packet_size(quality, bits_size)? as usize;
    if data.len() < size || !data.len().is_multiple_of(size) {
        return Ok(None);
    }
    Ok(Some(data.chunks(size).map(|p| p.to_vec()).collect()))
}
#[uniffi::export]
pub fn normalize_decoded_speex_pcm(
    data: Vec<u8>,
    packet_count: u32,
    format: PcmFormat,
) -> Result<Vec<u8>> {
    let samples = match format.sample_rate_hz {
        8000 => 160usize,
        16000 => 320,
        32000 => 640,
        _ => return Err(invalid("Speex supports 8000, 16000 or 32000 Hz")),
    };
    if !(1..=2).contains(&format.channels) || ![8, 16].contains(&format.bit_depth) {
        return Err(invalid("Unsupported PCM format"));
    }
    let frame_bytes = samples * format.channels as usize * (format.bit_depth as usize / 8);
    let count = packet_count as usize;
    if count == 0 || data.len() / frame_bytes <= count || !data.len().is_multiple_of(count) {
        return Ok(data);
    }
    let decoded_stride = data.len() / count;
    if !decoded_stride.is_multiple_of(frame_bytes) {
        return Ok(data);
    }
    Ok(data
        .chunks(decoded_stride)
        .flat_map(|p| p[..frame_bytes].iter().copied())
        .collect())
}
const RING_AUDIO_RATE: u32 = 16_000;
const RING_FRAME_SAMPLES: usize = 320;

#[derive(uniffi::Object)]
pub struct RingOpusDecoder {
    channels: u16,
    decoder: std::sync::Mutex<opus1::OpusDecoder>,
}

#[uniffi::export]
impl RingOpusDecoder {
    #[uniffi::constructor]
    pub fn new(channels: u16) -> Result<Self> {
        let layout = match channels {
            1 => opus1::Channels::Mono,
            2 => opus1::Channels::Stereo,
            _ => return Err(invalid("Ring Opus requires one or two channels")),
        };
        let decoder = opus1::OpusDecoder::new(opus1::SampleRate::Hz16000, layout)
            .map_err(|err| invalid(format!("Could not initialize Opus decoder: {err}")))?;
        Ok(Self {
            channels,
            decoder: std::sync::Mutex::new(decoder),
        })
    }

    pub fn format(&self) -> PcmFormat {
        PcmFormat {
            sample_rate_hz: RING_AUDIO_RATE,
            channels: self.channels,
            bit_depth: 16,
        }
    }

    /// Decode one in-order ring packet to 20 ms of PCM16LE, interleaved L/R for stereo.
    pub fn decode_packet(&self, packet: Vec<u8>) -> Result<Vec<u8>> {
        validate_opus(&packet, self.channels)?;
        let mut output = [0i16; RING_FRAME_SAMPLES * 2];
        let mut decoder = self.decoder.lock().unwrap();
        let count = decoder
            .decode_i16(
                &packet,
                RING_FRAME_SAMPLES,
                &mut output[..RING_FRAME_SAMPLES * usize::from(self.channels)],
            )
            .map_err(|err| invalid(format!("Invalid Opus payload: {err}")))?;
        if count != RING_FRAME_SAMPLES {
            return Err(invalid("Opus decoder returned an unexpected sample count"));
        }
        Ok(output[..count * usize::from(self.channels)]
            .iter()
            .flat_map(|sample| sample.to_le_bytes())
            .collect())
    }
}

/// Decode an OP2C recording to 16 kHz PCM16LE without changing its channel count.
#[uniffi::export]
pub fn decode_opus_pcm(data: Vec<u8>) -> Result<Vec<u8>> {
    let recording = parse_opus_recording(data)?;
    let decoder = RingOpusDecoder::new(recording.channels)?;
    let mut pcm = Vec::new();
    for packet in recording.packets {
        pcm.extend(decoder.decode_packet(packet)?);
    }
    Ok(pcm)
}

#[derive(uniffi::Object)]
pub struct DualMicDenoiser {
    gain: std::sync::Mutex<Option<f32>>,
}

#[uniffi::export]
impl DualMicDenoiser {
    #[uniffi::constructor]
    pub fn new() -> Self {
        Self::default()
    }

    /// Process whole 20 ms frames of synchronized 16 kHz stereo PCM16LE into mono PCM16LE.
    pub fn process(&self, pcm: Vec<u8>) -> Result<Vec<u8>> {
        if !pcm.len().is_multiple_of(RING_FRAME_SAMPLES * 4) {
            return Err(invalid(
                "Dual-mic input must contain whole 20 ms stereo PCM16LE frames",
            ));
        }
        let mut result = Vec::with_capacity(pcm.len() / 2);
        let mut previous_gain = self.gain.lock().unwrap();
        for frame in pcm.as_chunks::<{ RING_FRAME_SAMPLES * 4 }>().0 {
            let mut mid = [0f32; RING_FRAME_SAMPLES];
            let mut mid_energy = 0f32;
            let mut side_energy = 0f32;
            for (index, pair) in frame.as_chunks::<4>().0.iter().enumerate() {
                let left = f32::from(i16::from_le_bytes([pair[0], pair[1]]));
                let right = f32::from(i16::from_le_bytes([pair[2], pair[3]]));
                mid[index] = (left + right) * 0.5;
                let side = (left - right) * 0.5;
                mid_energy += mid[index] * mid[index];
                side_energy += side * side;
            }
            // The side channel estimates noise only when the desired signal is aligned in both mics.
            let target = (1.0 - side_energy / mid_energy.max(1.0))
                .clamp(0.25, 1.0)
                .sqrt();
            let start = previous_gain.unwrap_or(target);
            for (index, sample) in mid.into_iter().enumerate() {
                let gain =
                    start + (target - start) * (index + 1) as f32 / RING_FRAME_SAMPLES as f32;
                result.extend(((sample * gain).round() as i16).to_le_bytes());
            }
            *previous_gain = Some(target);
        }
        Ok(result)
    }
}

impl Default for DualMicDenoiser {
    fn default() -> Self {
        Self {
            gain: std::sync::Mutex::new(None),
        }
    }
}

fn check_pcm_format(f: &PcmFormat) -> Result<()> {
    if f.sample_rate_hz == 0 || !(1..=2).contains(&f.channels) || ![8, 16].contains(&f.bit_depth) {
        return Err(invalid("Unsupported PCM format"));
    }
    Ok(())
}
fn resample_interleaved(samples: &[i16], channels: usize, src: u32, dst: u32) -> Vec<i16> {
    if src == dst || samples.is_empty() {
        return samples.to_vec();
    }
    let in_frames = samples.len() / channels;
    let out_frames = (in_frames as u128 * u128::from(dst) / u128::from(src)) as usize;
    let mut out = Vec::with_capacity(out_frames * channels);
    for i in 0..out_frames {
        let pos = i as f64 * f64::from(src) / f64::from(dst);
        let idx = (pos as usize).min(in_frames - 1);
        let next = (idx + 1).min(in_frames - 1);
        let frac = pos - idx as f64;
        for c in 0..channels {
            let a = f64::from(samples[idx * channels + c]);
            let b = f64::from(samples[next * channels + c]);
            out.push(
                (a * (1.0 - frac) + b * frac)
                    .round()
                    .clamp(f64::from(i16::MIN), f64::from(i16::MAX)) as i16,
            );
        }
    }
    out
}
fn encode_samples(samples: &[i16], bit_depth: u16) -> Vec<u8> {
    if bit_depth == 16 {
        samples.iter().flat_map(|s| s.to_le_bytes()).collect()
    } else {
        samples
            .iter()
            .map(|s| ((i32::from(*s) >> 8) + 128).clamp(0, 255) as u8)
            .collect()
    }
}
fn speex_error(e: impl std::fmt::Display) -> CoreError {
    invalid(format!("Invalid Speex payload: {e}"))
}
/// Decode one packet; legacy packets with trailing padding that decode extra frames keep only
/// the first 20 ms frame.
fn decode_speex_frame(
    decoder: &mut oxideav_speex::SpeexStreamDecoder,
    packet: &[u8],
    channels: usize,
) -> Result<Vec<i16>> {
    if packet.is_empty() {
        return Err(invalid("Empty Speex packet"));
    }
    let mut samples = if channels == 2 {
        decoder.decode_packet_stereo_i16(packet)
    } else {
        decoder.decode_packet_pcm_i16(packet)
    }
    .map_err(speex_error)?;
    let expected = decoder.frame_samples() * channels;
    if samples.is_empty() {
        return Err(invalid("Speex packet decoded zero samples"));
    }
    if samples.len() > expected && samples.len().is_multiple_of(expected) {
        samples.truncate(expected);
    }
    Ok(samples)
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct SpeexDecodedAudio {
    pub pcm_bytes: Vec<u8>,
    pub format: PcmFormat,
    pub source_type: String,
    pub packet_count: u32,
}

/// Stateful pure-Rust decoder for ordered legacy Speex packets (20 ms each).
#[derive(uniffi::Object)]
pub struct RingSpeexDecoder {
    channels: u16,
    rate: u32,
    decoder: std::sync::Mutex<oxideav_speex::SpeexStreamDecoder>,
}

#[uniffi::export]
impl RingSpeexDecoder {
    #[uniffi::constructor]
    pub fn new(sample_rate_hz: u32, channels: u16) -> Result<Self> {
        if !(1..=2).contains(&channels) {
            return Err(invalid("Speex requires one or two output channels"));
        }
        let decoder = oxideav_speex::SpeexStreamDecoder::for_sample_rate(sample_rate_hz)
            .map_err(|_| invalid("Speex supports 8000, 16000 or 32000 Hz"))?;
        Ok(Self {
            channels,
            rate: sample_rate_hz,
            decoder: std::sync::Mutex::new(decoder),
        })
    }
    pub fn format(&self) -> PcmFormat {
        PcmFormat {
            sample_rate_hz: self.rate,
            channels: self.channels,
            bit_depth: 16,
        }
    }
    /// Decode one in-order packet to PCM16LE (mono, or duplicated/intensity stereo).
    pub fn decode_packet(&self, packet: Vec<u8>) -> Result<Vec<u8>> {
        let mut decoder = self.decoder.lock().unwrap();
        let samples = decode_speex_frame(&mut decoder, &packet, usize::from(self.channels))?;
        Ok(encode_samples(&samples, 16))
    }
}

/// Decode an Ogg Speex file to PCM in the requested format.
#[uniffi::export]
pub fn decode_ogg_speex_pcm(data: Vec<u8>, target_format: PcmFormat) -> Result<Vec<u8>> {
    check_pcm_format(&target_format)?;
    let packets = oxideav_ogg::framing::pages_to_packets(&data)
        .map_err(|e| invalid(format!("Invalid Ogg Speex container: {e}")))?;
    let header = packets
        .first()
        .ok_or_else(|| invalid("Ogg Speex stream has no packets"))
        .and_then(|p| {
            oxideav_speex::SpeexHeader::parse(p)
                .map_err(|e| invalid(format!("Invalid Speex header: {e}")))
        })?;
    let audio_start = 2usize
        .checked_add(header.extra_headers as usize)
        .filter(|start| *start <= packets.len())
        .ok_or_else(|| invalid("Ogg Speex stream is missing header packets"))?;
    let mut decoder = oxideav_speex::SpeexStreamDecoder::for_header(&header)
        .map_err(|e| invalid(format!("Unsupported Speex header: {e}")))?;
    let source_channels = if header.nb_channels == 2 { 2 } else { 1 };
    let mut samples = Vec::new();
    for packet in &packets[audio_start..] {
        samples.extend(decode_speex_frame(&mut decoder, packet, source_channels)?);
    }
    let target = usize::from(target_format.channels);
    if source_channels == 2 && target == 1 {
        samples = samples
            .as_chunks::<2>()
            .0
            .iter()
            .map(|p| ((i32::from(p[0]) + i32::from(p[1])) / 2) as i16)
            .collect();
    } else if source_channels == 1 && target == 2 {
        samples = samples.iter().flat_map(|s| [*s, *s]).collect();
    }
    let samples = resample_interleaved(
        &samples,
        target,
        decoder.output_rate_hz(),
        target_format.sample_rate_hz,
    );
    Ok(encode_samples(&samples, target_format.bit_depth))
}

/// Decode legacy Speex (Ogg, length-prefixed packets, or fixed-size raw packets).
#[uniffi::export]
pub fn decode_speex_pcm(
    data: Vec<u8>,
    target_format: PcmFormat,
    quality: u8,
    bits_size: Option<u32>,
    max_packet_size: u32,
    framed_block_size: Option<u32>,
) -> Result<SpeexDecodedAudio> {
    if data.is_empty() {
        return Err(invalid("Speex data is empty"));
    }
    check_pcm_format(&target_format)?;
    if is_ogg_speex(data.clone()) {
        return Ok(SpeexDecodedAudio {
            pcm_bytes: decode_ogg_speex_pcm(data, target_format.clone())?,
            format: target_format,
            source_type: "ogg-speex".into(),
            packet_count: 0,
        });
    }
    let (source_type, packets) =
        match parse_packetized_speex_stream(data.clone(), max_packet_size, framed_block_size)? {
            Some(packets) => ("packet-speex", packets),
            None => match split_raw_speex_packets(data, quality, bits_size)? {
                Some(packets) => ("raw-speex", packets),
                None => {
                    return Err(invalid(
                        "bin data does not look like packetized or raw Speex",
                    ))
                }
            },
        };
    let rate = match target_format.sample_rate_hz {
        0..=8000 => 8000,
        8001..=16000 => 16000,
        _ => 32000,
    };
    let mut decoder = oxideav_speex::SpeexStreamDecoder::for_sample_rate(rate)
        .map_err(|e| invalid(e.to_string()))?;
    let channels = usize::from(target_format.channels);
    let mut samples = Vec::new();
    for packet in &packets {
        samples.extend(decode_speex_frame(&mut decoder, packet, channels)?);
    }
    let samples = resample_interleaved(&samples, channels, rate, target_format.sample_rate_hz);
    Ok(SpeexDecodedAudio {
        pcm_bytes: encode_samples(&samples, target_format.bit_depth),
        format: target_format,
        source_type: source_type.into(),
        packet_count: u32::try_from(packets.len())
            .map_err(|_| invalid("Too many Speex packets"))?,
    })
}
