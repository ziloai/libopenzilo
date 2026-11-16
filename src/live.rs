// SPDX-License-Identifier: MPL-2.0
use crate::{
    device::validate_opus, invalid, protocol::Reader, recording_config_body, Packet,
    RecordingConfig, Result,
};
use std::sync::Mutex;
#[derive(Clone, Debug, uniffi::Record)]
pub struct LiveStart {
    pub session_id: u32,
    pub record_time: u32,
    pub sample_rate: u16,
    pub channels: u8,
    pub codec: u8,
}
#[derive(Clone, Debug, uniffi::Record)]
pub struct LiveData {
    pub session_id: u32,
    pub sequence: u32,
    pub byte_offset: u32,
    pub data: Vec<u8>,
}
#[derive(Clone, Debug, uniffi::Record)]
pub struct LiveEnd {
    pub error_code: u16,
    pub session_id: u32,
    pub produced_bytes: u32,
    pub sent_bytes: u32,
    pub dropped_frames: u32,
}
#[derive(Clone, Debug, uniffi::Enum)]
pub enum LiveEvent {
    Start { value: LiveStart },
    Data { value: LiveData },
    End { value: LiveEnd },
}
#[uniffi::export]
pub fn parse_live_start(body: Vec<u8>) -> Result<LiveStart> {
    if body.len() != 20 {
        return Err(invalid("Invalid live START length"));
    }
    let mut r = Reader::new(&body);
    let id = r.u32()?;
    let a = r.u32()?;
    let time = r.u32()?;
    let b = r.u32()?;
    let rate = r.u16()?;
    let channels = r.u8()?;
    let codec = r.u8()?;
    if id == 0 || a != 0 || b != 0 || rate != 16000 || !(1..=2).contains(&channels) || codec != 3 {
        return Err(invalid("Unsupported live START format"));
    }
    Ok(LiveStart {
        session_id: id,
        record_time: time,
        sample_rate: rate,
        channels,
        codec,
    })
}
#[uniffi::export]
pub fn parse_live_data(body: Vec<u8>, channels: u8) -> Result<LiveData> {
    if body.len() < 15 {
        return Err(invalid("Truncated live DATA"));
    }
    let mut r = Reader::new(&body);
    let id = r.u32()?;
    let seq = r.u32()?;
    let off = r.u32()?;
    let n = r.u16()? as usize;
    if id == 0 || !(1..=100).contains(&n) || r.remaining() != n {
        return Err(invalid("Invalid live DATA length or session"));
    }
    let data = r.bytes(n)?.to_vec();
    validate_opus(&data, channels as u16)?;
    Ok(LiveData {
        session_id: id,
        sequence: seq,
        byte_offset: off,
        data,
    })
}
#[uniffi::export]
pub fn parse_live_end(body: Vec<u8>) -> Result<LiveEnd> {
    if body.len() != 22 {
        return Err(invalid("Invalid live END length"));
    }
    let mut r = Reader::new(&body);
    let error = r.u16()?;
    let id = r.u32()?;
    let reserved = r.u32()?;
    let produced = r.u32()?;
    let sent = r.u32()?;
    let dropped = r.u32()?;
    if id == 0 || reserved != 0 || sent > produced {
        return Err(invalid("Invalid live END counters or session"));
    }
    Ok(LiveEnd {
        error_code: error,
        session_id: id,
        produced_bytes: produced,
        sent_bytes: sent,
        dropped_frames: dropped,
    })
}
#[derive(Default)]
struct State {
    start: Option<LiveStart>,
    sequence: u32,
    bytes: u32,
    ended: bool,
    failed: bool,
}
/// Validates provisional live frames. Only a successful END confirms completeness.
#[derive(uniffi::Object, Default)]
pub struct LiveValidator {
    state: Mutex<State>,
}
#[uniffi::export]
impl LiveValidator {
    #[uniffi::constructor]
    pub fn new() -> Self {
        Self::default()
    }
    pub fn accept(&self, packet: Packet) -> Result<LiveEvent> {
        let mut s = self.state.lock().unwrap();
        if s.ended || s.failed {
            return Err(invalid("Live stream is already closed"));
        }
        let result = validate(&mut s, packet);
        if result.is_err() {
            s.failed = true;
        }
        result
    }
}
fn validate(s: &mut State, p: Packet) -> Result<LiveEvent> {
    match p.command {
        0x50f => {
            if s.start.is_some() {
                return Err(invalid("Unexpected START during live recording"));
            }
            let v = parse_live_start(p.body)?;
            s.start = Some(v.clone());
            Ok(LiveEvent::Start { value: v })
        }
        0x510 => {
            let start = s
                .start
                .as_ref()
                .ok_or_else(|| invalid("Live recording did not begin with START"))?;
            let v = parse_live_data(p.body, start.channels)?;
            if v.session_id != start.session_id
                || v.sequence != s.sequence
                || v.byte_offset != s.bytes
            {
                return Err(invalid(
                    "Live recording session, sequence or offset mismatch",
                ));
            }
            if s.sequence >= 3000 {
                return Err(invalid("Live recording exceeds firmware's 60-second limit"));
            }
            s.sequence += 1;
            s.bytes += v.data.len() as u32;
            Ok(LiveEvent::Data { value: v })
        }
        0x511 => {
            let start = s
                .start
                .as_ref()
                .ok_or_else(|| invalid("Live recording did not begin with START"))?;
            let v = parse_live_end(p.body)?;
            if v.session_id != start.session_id {
                return Err(invalid("Live END belongs to another recording"));
            }
            if v.error_code != 0 {
                return Err(crate::CoreError::Device { code: v.error_code });
            }
            if v.dropped_frames != 0 || v.produced_bytes != s.bytes || v.sent_bytes != s.bytes {
                return Err(invalid(
                    "Live recording is incomplete according to END counters",
                ));
            }
            s.ended = true;
            Ok(LiveEvent::End { value: v })
        }
        _ => Err(invalid("Unexpected live command")),
    }
}
#[uniffi::export]
pub fn parse_recording_config(body: Vec<u8>) -> Result<RecordingConfig> {
    let mut r = Reader::new(&body);
    r.success()?;
    if body.len() != 34 {
        return Err(invalid("Invalid audio configuration response length"));
    }
    if r.u8()? != 1 {
        return Err(invalid("Unsupported audio configuration version"));
    }
    let c = RecordingConfig {
        output: r.u8()?,
        disconnect_policy: r.u8()?,
        channels: r.u8()?,
        bitrate: r.u32()?,
        gain_left_q8: r.u16()?,
        gain_right_q8: r.u16()?,
        max_duration_ms: r.u16()?,
    };
    recording_config_body(c.clone())?;
    if r.bytes(18)?
        != [
            0x3e, 0x80, 0, 20, 3, 1, 1, 1, 0, 0, 0, 0x0b, 0xb9, 16, 0, 100, 8, 0,
        ]
    {
        return Err(invalid("Unsupported fixed Opus profile"));
    }
    Ok(c)
}
#[uniffi::export]
pub fn parse_session_ack(body: Vec<u8>) -> Result<u32> {
    let mut r = Reader::new(&body);
    r.success()?;
    let id = r.u32()?;
    r.end()?;
    if id == 0 {
        return Err(invalid("Invalid recording session response"));
    }
    Ok(id)
}
#[uniffi::export]
pub fn parse_success(body: Vec<u8>) -> Result<()> {
    let mut r = Reader::new(&body);
    r.success()?;
    r.end()
}
#[uniffi::export]
pub fn parse_audio_count(body: Vec<u8>) -> Result<u32> {
    let mut r = Reader::new(&body);
    r.success()?;
    let n = r.u32()?;
    r.end()?;
    Ok(n)
}
#[derive(Clone, Debug, uniffi::Record)]
pub struct ImuConfig {
    pub sample_rate_hz: u16,
    pub accel_range_g: u16,
    pub gyro_range_dps: u16,
}
#[uniffi::export]
pub fn parse_imu_config(body: Vec<u8>) -> Result<ImuConfig> {
    let mut r = Reader::new(&body);
    r.success()?;
    let v = ImuConfig {
        sample_rate_hz: r.u16()?,
        accel_range_g: r.u16()?,
        gyro_range_dps: r.u16()?,
    };
    r.end()?;
    Ok(v)
}
#[uniffi::export]
pub fn parse_live_subscription(body: Vec<u8>) -> Result<bool> {
    let mut r = Reader::new(&body);
    r.success()?;
    let v = r.u8()?;
    r.end()?;
    if v > 1 {
        return Err(invalid("Invalid live subscription response"));
    }
    Ok(v != 0)
}
#[uniffi::export]
pub fn parse_audio_record_result(body: Vec<u8>) -> Result<u32> {
    parse_audio_end(body)
}
#[uniffi::export]
pub fn parse_audio_end(body: Vec<u8>) -> Result<u32> {
    let mut r = Reader::new(&body);
    r.success()?;
    let index = r.u32()?;
    r.end()?;
    Ok(index)
}
