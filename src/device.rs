// SPDX-License-Identifier: MPL-2.0
use crate::{invalid, protocol::Reader, CoreError, Result};
use std::collections::BTreeMap;
macro_rules! record {($name:ident {$($f:ident:$t:ty),*$(,)?})=>{#[derive(Clone,Debug,PartialEq,uniffi::Record)] pub struct $name {$(pub $f:$t),*}}}
record!(DeviceInfo {
    firmware_version: String,
    system_time: u32,
    audio_storage_total: u32,
    audio_storage_available: u32,
    battery_percent: u16,
    battery_charging: bool,
    sn: String,
    cpuid: String,
    model: String
});
#[uniffi::export]
pub fn parse_system_info(body: Vec<u8>) -> Result<DeviceInfo> {
    let mut r = Reader::new(&body);
    r.success()?;
    Ok(DeviceInfo {
        firmware_version: r.string()?,
        system_time: r.u32()?,
        audio_storage_total: r.u32()?,
        audio_storage_available: r.u32()?,
        battery_percent: r.u16()?,
        battery_charging: r.u8()? != 0,
        sn: r.string()?,
        cpuid: r.string()?,
        model: r.string()?,
    })
}
#[uniffi::export]
pub fn parse_connection_rssi(body: Vec<u8>) -> Result<i8> {
    let mut r = Reader::new(&body);
    let code = r.u16()?;
    if code != 0 {
        if r.remaining() != 0 {
            return Err(invalid("Invalid RSSI error response length"));
        }
        return Err(CoreError::Device { code });
    }
    if r.remaining() != 1 {
        return Err(invalid("Invalid RSSI response length"));
    }
    let rssi = r.u8()? as i8;
    if !(-127..=20).contains(&rssi) {
        return Err(invalid(
            "RSSI is unavailable or outside the supported range",
        ));
    }
    Ok(rssi)
}
record!(FeatureVersion {
    name: String,
    major: u8,
    minor: u8
});
record!(RecordingLimits {
    sample_rate_hz: u16,
    frame_duration_ms: u16,
    channels_mask: u8,
    outputs_mask: u8,
    continue_outputs_mask: u8,
    bitrate_min: u32,
    bitrate_max: u32,
    gain_min_q8: u16,
    gain_max_q8: u16,
    duration_min_ms: u16,
    duration_max_ms: u16,
    duration_step_ms: u16
});
record!(Capabilities{firmware_version:String,model:String,board_target:String,build_id:String,boot_id:String,protocol_versions:Vec<u16>,features:Vec<FeatureVersion>,recording_limits:Option<RecordingLimits>});
pub(crate) fn feature_name(id: u16) -> String {
    match id {
        1 => "system",
        2 => "time_sync",
        3 => "audio_files",
        4 => "recording_control",
        5 => "live_audio",
        6 => "imu",
        7 => "key_events",
        8 => "double_tap",
        9 => "ota",
        10 => "gesture",
        11 => "imu_calibration",
        12 => "connection_rssi",
        _ => return format!("unknown_{id}"),
    }
    .into()
}
#[uniffi::export]
pub fn supports_feature(
    features: Vec<FeatureVersion>,
    name: String,
    major: u8,
    min_minor: u8,
) -> bool {
    features
        .iter()
        .any(|f| f.name == name && f.major == major && f.minor >= min_minor)
}
#[uniffi::export]
pub fn parse_capabilities(body: Vec<u8>) -> Result<Capabilities> {
    let mut r = Reader::new(&body);
    r.success()?;
    if r.u8()? != 1 {
        return Err(CoreError::Incompatible {
            detail: "Unsupported capability schema; SDK upgrade required".into(),
        });
    }
    let mut fields = BTreeMap::new();
    while r.remaining() > 0 {
        let tag = r.u16()?;
        let n = r.u16()? as usize;
        let value = r.bytes(n)?.to_vec();
        if tag & 0x8000 != 0 {
            return Err(CoreError::Incompatible {
                detail: format!("Unknown required capability tag 0x{tag:04X}"),
            });
        }
        if fields.insert(tag, value).is_some() {
            return Err(invalid(format!("Duplicate capability tag 0x{tag:04X}")));
        }
    }
    for tag in 1..=6 {
        if fields.get(&tag).is_none_or(|v| v.is_empty()) {
            return Err(invalid(format!("Missing capability tag {tag}")));
        }
    }
    let text = |tag| {
        String::from_utf8(fields[&tag].clone())
            .map_err(|_| invalid("Invalid UTF-8 capability identity"))
    };
    if fields[&4].len() != 8 || fields[&6].len() % 2 != 0 {
        return Err(invalid("Invalid boot identity or protocol list"));
    }
    let protocols: Vec<u16> = fields[&6]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|v| u16::from_be_bytes([v[0], v[1]]))
        .collect();
    if !protocols.contains(&4) {
        return Err(CoreError::Incompatible {
            detail: "Device does not advertise protocol v4".into(),
        });
    }
    let mut features = Vec::new();
    for (&tag, v) in &fields {
        if tag > 0x100 && tag <= 0x1ff {
            if v.len() != 2 || v[0] == 0 {
                return Err(invalid("Invalid feature contract"));
            }
            features.push(FeatureVersion {
                name: feature_name(tag - 0x100),
                major: v[0],
                minor: v[1],
            });
        }
    }
    let limits = if let Some(v) = fields.get(&0x204) {
        if v.len() != 26 {
            return Err(invalid("Invalid recording limits length"));
        }
        let mut r = Reader::new(v);
        let rate = r.u16()?;
        let frame = r.u16()?;
        let channels = r.u8()?;
        let outputs = r.u8()?;
        let continuing = r.u8()?;
        let reserved = r.u8()?;
        let l = RecordingLimits {
            sample_rate_hz: rate,
            frame_duration_ms: frame,
            channels_mask: channels,
            outputs_mask: outputs,
            continue_outputs_mask: continuing,
            bitrate_min: r.u32()?,
            bitrate_max: r.u32()?,
            gain_min_q8: r.u16()?,
            gain_max_q8: r.u16()?,
            duration_min_ms: r.u16()?,
            duration_max_ms: r.u16()?,
            duration_step_ms: r.u16()?,
        };
        if reserved != 0
            || rate == 0
            || frame == 0
            || channels == 0
            || outputs == 0
            || continuing & !outputs != 0
            || l.duration_step_ms == 0
            || l.bitrate_min > l.bitrate_max
            || l.gain_min_q8 > l.gain_max_q8
            || l.duration_min_ms > l.duration_max_ms
        {
            return Err(invalid("Invalid recording limits"));
        }
        Some(l)
    } else {
        None
    };
    if features
        .iter()
        .any(|f| f.name == "recording_control" && f.major == 1)
    {
        let l = limits
            .as_ref()
            .ok_or_else(|| invalid("Recording control v1 requires limits"))?;
        if l.sample_rate_hz != 16000 || l.frame_duration_ms != 20 {
            return Err(CoreError::Incompatible {
                detail: "Recording contract v1 requires 16 kHz / 20 ms".into(),
            });
        }
    }
    Ok(Capabilities {
        firmware_version: text(1)?,
        model: text(2)?,
        board_target: text(3)?,
        build_id: text(5)?,
        boot_id: fields[&4].iter().map(|b| format!("{b:02x}")).collect(),
        protocol_versions: protocols,
        features,
        recording_limits: limits,
    })
}
record!(ImuSample {
    timestamp_ms: u32,
    accel_x: i16,
    accel_y: i16,
    accel_z: i16,
    gyro_x: i16,
    gyro_y: i16,
    gyro_z: i16
});
record!(ImuBatch{sequence_start:u32,frame_count:u16,sample_size:u16,samples:Vec<ImuSample>});
#[uniffi::export]
pub fn parse_imu(body: Vec<u8>) -> Result<ImuBatch> {
    let mut r = Reader::new(&body);
    r.success()?;
    let sequence_start = r.u32()?;
    let (count, size) = if body.len() == 22 {
        (1, 16)
    } else {
        (r.u16()?, r.u16()?)
    };
    if size != 16 {
        return Err(invalid(format!("Unsupported sensor sample size: {size}")));
    }
    if r.remaining() != count as usize * 16 {
        return Err(invalid(format!(
            "Sensor batch length mismatch: expected {} sample bytes, got {}",
            count as usize * 16,
            r.remaining()
        )));
    }
    let mut samples = Vec::new();
    for _ in 0..count {
        samples.push(ImuSample {
            timestamp_ms: r.u32()?,
            accel_x: r.i16()?,
            accel_y: r.i16()?,
            accel_z: r.i16()?,
            gyro_x: r.i16()?,
            gyro_y: r.i16()?,
            gyro_z: r.i16()?,
        });
    }
    Ok(ImuBatch {
        sequence_start,
        frame_count: count,
        sample_size: size,
        samples,
    })
}
record!(AudioFileInfo {
    file_index: u32,
    record_time: u32,
    data_size: u32
});
#[uniffi::export]
pub fn parse_audio_info(body: Vec<u8>) -> Result<AudioFileInfo> {
    let mut r = Reader::new(&body);
    r.success()?;
    let v = AudioFileInfo {
        file_index: r.u32()?,
        record_time: r.u32()?,
        data_size: r.u32()?,
    };
    if r.remaining() > 0 {
        return Err(invalid(format!(
            "Unexpected trailing audio file info bytes: {}",
            r.remaining()
        )));
    }
    Ok(v)
}
record!(AudioFrame{file_index:u32,frame_offset:u32,frame_size:u32,is_end:bool,data:Vec<u8>});
#[uniffi::export]
pub fn parse_audio_frame(body: Vec<u8>) -> Result<AudioFrame> {
    let mut r = Reader::new(&body);
    r.success()?;
    let index = r.u32()?;
    let offset = r.u32()?;
    let n = r.u32()?;
    let end = r.u8()? != 0;
    if r.remaining() != n as usize {
        return Err(invalid(format!(
            "Audio frame size mismatch: declared {n}, got {}",
            r.remaining()
        )));
    }
    Ok(AudioFrame {
        file_index: index,
        frame_offset: offset,
        frame_size: n,
        is_end: end,
        data: r.bytes(n as usize)?.to_vec(),
    })
}
record!(RecordingConfig {
    output: u8,
    disconnect_policy: u8,
    channels: u8,
    bitrate: u32,
    gain_left_q8: u16,
    gain_right_q8: u16,
    max_duration_ms: u16
});
#[uniffi::export]
pub fn recording_config_body(c: RecordingConfig) -> Result<Vec<u8>> {
    if c.output > 3
        || c.disconnect_policy > 1
        || !(1..=2).contains(&c.channels)
        || !(8000..=32000).contains(&c.bitrate)
        || c.gain_left_q8 > 1024
        || c.gain_right_q8 > 1024
        || (c.max_duration_ms != 0 && !(20..=60000).contains(&c.max_duration_ms))
        || !c.max_duration_ms.is_multiple_of(20)
        || (c.disconnect_policy == 1 && c.output != 1 && c.output != 3)
    {
        return Err(invalid("Invalid recording configuration"));
    }
    let mut b = vec![1, c.output, c.disconnect_policy, c.channels];
    b.extend(c.bitrate.to_be_bytes());
    b.extend(c.gain_left_q8.to_be_bytes());
    b.extend(c.gain_right_q8.to_be_bytes());
    b.extend(c.max_duration_ms.to_be_bytes());
    Ok(b)
}
record!(RecordingStatus{session_id:u32,state:u8,saved:bool,error_code:u16,file_index:Option<u32>});
#[uniffi::export]
pub fn parse_recording_status(body: Vec<u8>) -> Result<RecordingStatus> {
    let mut r = Reader::new(&body);
    r.success()?;
    if body.len() != 14 {
        return Err(invalid("Invalid recording status length"));
    }
    let session = r.u32()?;
    let state = r.u8()?;
    let saved = r.u8()?;
    let error = r.u16()?;
    let index = r.u32()?;
    if state > 6 || saved > 1 || (state == 0) != (session == 0) {
        return Err(invalid("Invalid recording status values"));
    }
    if (saved == 0 && index != 0)
        || (state == 4 && error != 0)
        || (state == 5 && error == 0)
        || ([0, 6].contains(&state) && (saved != 0 || error != 0))
    {
        return Err(invalid("Inconsistent recording status"));
    }
    Ok(RecordingStatus {
        session_id: session,
        state,
        saved: saved != 0,
        error_code: error,
        file_index: if saved != 0 { Some(index) } else { None },
    })
}
#[uniffi::export]
pub fn sensor_gesture_name(gesture_id: u8) -> String {
    match gesture_id {
        0 => "idle".into(),
        1 => "rotate_back".into(),
        2 => "rotate_front".into(),
        3 => "wave".into(),
        value => format!("unknown({value})"),
    }
}
record!(DeviceEvent{command:u16,timestamp:u32,timestamp_unit:String,gesture_id:Option<u8>});
#[uniffi::export]
pub fn parse_device_event(command: u16, body: Vec<u8>) -> Result<DeviceEvent> {
    if !(0x701..=0x704).contains(&command) {
        return Err(invalid("Not a device event"));
    }
    let mut r = Reader::new(&body);
    let timestamp = r.u32()?;
    let gesture_id = if command == 0x702 {
        Some(r.u8()?)
    } else {
        None
    };
    r.end()?;
    Ok(DeviceEvent {
        command,
        timestamp,
        timestamp_unit: if command <= 0x702 {
            "milliseconds"
        } else {
            "seconds"
        }
        .into(),
        gesture_id,
    })
}
#[derive(Clone, Debug, uniffi::Record)]
pub struct OpusRecording {
    pub channels: u16,
    pub sample_rate: u16,
    pub frame_samples: u16,
    pub packets: Vec<Vec<u8>>,
}
pub(crate) fn validate_opus(p: &[u8], channels: u16) -> Result<()> {
    if !(1..=100).contains(&p.len()) {
        return Err(invalid("Opus packet length must be 1..100 bytes"));
    }
    let toc = p[0];
    if !(1..=2).contains(&channels) || (toc & 4 != 0) != (channels == 2) {
        return Err(invalid("Opus packet channels do not match the recording"));
    }
    let samples = if toc & 128 != 0 {
        (16000u32 << ((toc >> 3) & 3)) / 400
    } else if toc & 0x60 == 0x60 {
        if toc & 8 != 0 {
            320
        } else {
            160
        }
    } else {
        let mode = (toc >> 3) & 3;
        if mode == 3 {
            960
        } else {
            (16000u32 << mode) / 100
        }
    };
    let code = toc & 3;
    if code == 3 && p.len() < 2 {
        return Err(invalid("Truncated Opus frame count"));
    }
    let frames = if code == 0 {
        1
    } else if code == 3 {
        (p[1] & 63) as u32
    } else {
        2
    };
    if samples * frames != 320 {
        return Err(invalid("Opus packet must contain 20 ms per channel"));
    }
    Ok(())
}
#[uniffi::export]
pub fn parse_opus_recording(data: Vec<u8>) -> Result<OpusRecording> {
    if data.len() < 12 || &data[..4] != b"OP2C" {
        return Err(invalid("Unsupported or truncated OP2C header"));
    }
    let field = |i| u16::from_le_bytes([data[i], data[i + 1]]);
    let channels = field(6);
    if field(4) != 1 || !(1..=2).contains(&channels) || field(8) != 16000 || field(10) != 320 {
        return Err(invalid("Unsupported OP2C header"));
    }
    let mut packets = Vec::new();
    let mut offset = 12;
    while offset < data.len() {
        if data.len() - offset < 2 {
            return Err(invalid("Truncated Opus packet length"));
        }
        let n = field(offset) as usize;
        offset += 2;
        if n == 0 || n > 100 || n > data.len() - offset {
            return Err(invalid("Invalid or truncated Opus packet"));
        }
        let p = data[offset..offset + n].to_vec();
        validate_opus(&p, channels)?;
        packets.push(p);
        offset += n;
    }
    Ok(OpusRecording {
        channels,
        sample_rate: 16000,
        frame_samples: 320,
        packets,
    })
}
#[uniffi::export]
pub fn build_opus_recording(packets: Vec<Vec<u8>>, channels: u16) -> Result<Vec<u8>> {
    if !(1..=2).contains(&channels) {
        return Err(invalid("channels must be 1 or 2"));
    }
    let mut out = b"OP2C".to_vec();
    for n in [1, channels, 16000, 320] {
        out.extend(n.to_le_bytes())
    }
    for p in packets {
        validate_opus(&p, channels)?;
        out.extend((p.len() as u16).to_le_bytes());
        out.extend(p)
    }
    Ok(out)
}
/// Validate both the SDK profile and the limits actually advertised by this device.
#[uniffi::export]
pub fn validate_recording_config(config: RecordingConfig, limits: RecordingLimits) -> Result<()> {
    recording_config_body(config.clone())?;
    let c = config;
    let l = limits;
    if c.channels >= 8
        || l.channels_mask & (1 << c.channels) == 0
        || c.output >= 8
        || l.outputs_mask & (1 << c.output) == 0
        || c.bitrate < l.bitrate_min
        || c.bitrate > l.bitrate_max
        || c.gain_left_q8 < l.gain_min_q8
        || c.gain_left_q8 > l.gain_max_q8
        || c.gain_right_q8 < l.gain_min_q8
        || c.gain_right_q8 > l.gain_max_q8
        || (c.max_duration_ms != 0
            && (c.max_duration_ms < l.duration_min_ms || c.max_duration_ms > l.duration_max_ms))
        || l.duration_step_ms == 0
        || !c.max_duration_ms.is_multiple_of(l.duration_step_ms)
        || (c.disconnect_policy != 0 && l.continue_outputs_mask & (1 << c.output) == 0)
        || l.sample_rate_hz != 16000
        || l.frame_duration_ms != 20
    {
        return Err(CoreError::Incompatible {
            detail: "Recording configuration exceeds device capabilities".into(),
        });
    }
    Ok(())
}
#[uniffi::export]
pub fn validate_opus_packet(packet: Vec<u8>, channels: u16) -> Result<()> {
    validate_opus(&packet, channels)
}

record!(LogStorage {
    page_size: u32,
    total_len: u32
});
#[uniffi::export]
pub fn parse_log_storage(body: Vec<u8>) -> Result<LogStorage> {
    let mut r = Reader::new(&body);
    r.success()?;
    let value = LogStorage {
        page_size: r.u32()?,
        total_len: r.u32()?,
    };
    r.end()?;
    Ok(value)
}
#[uniffi::export]
pub fn parse_log_chunk(body: Vec<u8>, max_length: u32) -> Result<Vec<u8>> {
    let mut r = Reader::new(&body);
    r.success()?;
    let n = r.u32()?;
    if n > max_length || n as usize != r.remaining() {
        return Err(invalid(
            "Log chunk length differs from response or requested limit",
        ));
    }
    Ok(r.bytes(n as usize)?.to_vec())
}
#[uniffi::export]
pub fn parse_time_request(body: Vec<u8>) -> Result<u32> {
    let mut r = Reader::new(&body);
    let value = r.u32()?;
    r.end()?;
    Ok(value)
}
