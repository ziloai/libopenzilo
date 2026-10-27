// SPDX-License-Identifier: MPL-2.0
//! Sequential bounded transfer. Hosts acknowledge each sink write before another
//! device read is issued; bytes are never retained by this state machine.
use crate::*;
use std::sync::Mutex;
#[derive(Clone, Debug, uniffi::Record)]
pub struct AudioChunk {
    pub offset: u32,
    pub data: Vec<u8>,
}
#[derive(Clone, Debug, uniffi::Record)]
pub struct AudioSinkStep {
    pub request: Option<RequestSpec>,
    pub chunk: Option<AudioChunk>,
    pub committed: u32,
    pub total: u32,
    pub complete: bool,
}
struct SinkState {
    phase: DownloadPhase,
    info: Option<AudioFileInfo>,
    offset: u32,
    pending: Option<(u32, u32)>,
    deadline: u64,
    now: u64,
}
#[derive(uniffi::Object)]
pub struct AudioSinkTransfer {
    state: Mutex<SinkState>,
    file_index: u32,
    max_bytes: u32,
    total_timeout_ms: u64,
}
#[uniffi::export]
impl AudioSinkTransfer {
    #[uniffi::constructor]
    pub fn new(file_index: u32, max_bytes: u32, total_timeout_ms: u64) -> Result<Self> {
        if max_bytes == 0 || total_timeout_ms == 0 {
            return Err(invalid("Invalid streaming download limits"));
        }
        Ok(Self {
            state: Mutex::new(SinkState {
                phase: DownloadPhase::Ready,
                info: None,
                offset: 0,
                pending: None,
                deadline: 0,
                now: 0,
            }),
            file_index,
            max_bytes,
            total_timeout_ms,
        })
    }
    pub fn begin(&self, now_ms: u64) -> Result<AudioSinkStep> {
        let mut s = self.state.lock().unwrap();
        if s.phase != DownloadPhase::Ready {
            return Err(invalid("Download already started"));
        }
        s.deadline = now_ms
            .checked_add(self.total_timeout_ms)
            .ok_or_else(|| invalid("Download deadline overflow"))?;
        s.now = now_ms;
        s.phase = DownloadPhase::Metadata;
        self.step(
            &s,
            Some(DeviceRequest::AudioInfo {
                file_index: self.file_index,
            }),
            None,
        )
    }
    pub fn accept(&self, packet: Packet, now_ms: u64) -> Result<AudioSinkStep> {
        let mut s = self.state.lock().unwrap();
        let result = (|| {
            check(&mut s, now_ms)?;
            match (s.phase, packet.command) {
                (DownloadPhase::Metadata, 0x504) => {
                    let info = parse_audio_info(packet.body)?;
                    if info.file_index != self.file_index || info.data_size > self.max_bytes {
                        return Err(invalid("Audio identity or size exceeds sink limits"));
                    }
                    let empty = info.data_size == 0;
                    s.info = Some(info);
                    s.phase = if empty {
                        DownloadPhase::Finishing
                    } else {
                        DownloadPhase::Receiving
                    };
                    self.step(
                        &s,
                        Some(if empty {
                            DeviceRequest::AudioEnd {
                                file_index: self.file_index,
                            }
                        } else {
                            DeviceRequest::AudioRead {
                                file_index: self.file_index,
                                offset: 0,
                            }
                        }),
                        None,
                    )
                }
                (DownloadPhase::Receiving, 0x505) => {
                    let frame = parse_audio_frame(packet.body)?;
                    if s.pending.is_some()
                        || frame.file_index != self.file_index
                        || frame.frame_offset != s.offset
                        || frame.data.is_empty()
                    {
                        return Err(invalid(
                            "Unexpected sink frame or sink write not acknowledged",
                        ));
                    }
                    let end = s
                        .offset
                        .checked_add(frame.frame_size)
                        .ok_or_else(|| invalid("Audio sink offset overflow"))?;
                    if end > s.info.as_ref().unwrap().data_size {
                        return Err(invalid("Audio sink frame exceeds announced size"));
                    }
                    s.pending = Some((s.offset, end));
                    self.step(
                        &s,
                        None,
                        Some(AudioChunk {
                            offset: s.offset,
                            data: frame.data,
                        }),
                    )
                }
                (DownloadPhase::Finishing, 0x508) => {
                    if parse_audio_end(packet.body)? != self.file_index {
                        return Err(invalid("Audio completion identity differs"));
                    }
                    s.phase = DownloadPhase::Complete;
                    self.step(&s, None, None)
                }
                _ => Err(invalid("Unexpected audio sink phase or command")),
            }
        })();
        if result.is_err() {
            s.phase = DownloadPhase::Failed;
        }
        result
    }
    pub fn acknowledge_chunk(
        &self,
        offset: u32,
        accepted: bool,
        now_ms: u64,
    ) -> Result<AudioSinkStep> {
        let mut s = self.state.lock().unwrap();
        let result = (|| {
            check(&mut s, now_ms)?;
            let (start, end) = s.pending.ok_or_else(|| invalid("No sink write pending"))?;
            if !accepted || offset != start {
                return Err(invalid(
                    "Audio sink write failed or acknowledged wrong offset",
                ));
            }
            s.offset = end;
            s.pending = None;
            let request = if end == s.info.as_ref().unwrap().data_size {
                s.phase = DownloadPhase::Finishing;
                DeviceRequest::AudioEnd {
                    file_index: self.file_index,
                }
            } else {
                DeviceRequest::AudioRead {
                    file_index: self.file_index,
                    offset: end,
                }
            };
            self.step(&s, Some(request), None)
        })();
        if result.is_err() {
            s.phase = DownloadPhase::Failed;
        }
        result
    }
    pub fn tick(&self, now_ms: u64) -> Result<()> {
        let mut s = self.state.lock().unwrap();
        let result = check(&mut s, now_ms);
        if result.is_err() {
            s.phase = DownloadPhase::Failed;
        }
        result
    }
    pub fn info(&self) -> Option<AudioFileInfo> {
        self.state.lock().unwrap().info.clone()
    }
    pub fn cancel(&self) {
        let mut s = self.state.lock().unwrap();
        s.phase = DownloadPhase::Cancelled;
        s.pending = None;
    }
}
impl AudioSinkTransfer {
    fn step(
        &self,
        s: &SinkState,
        request: Option<DeviceRequest>,
        chunk: Option<AudioChunk>,
    ) -> Result<AudioSinkStep> {
        Ok(AudioSinkStep {
            request: request.map(prepare_request).transpose()?,
            chunk,
            committed: s.offset,
            total: s.info.as_ref().map(|v| v.data_size).unwrap_or(0),
            complete: s.phase == DownloadPhase::Complete,
        })
    }
}
fn check(s: &mut SinkState, now: u64) -> Result<()> {
    if !matches!(
        s.phase,
        DownloadPhase::Metadata | DownloadPhase::Receiving | DownloadPhase::Finishing
    ) {
        return Err(invalid("Audio sink transfer is not active"));
    }
    if now < s.now {
        return Err(invalid("Monotonic clock moved backwards"));
    }
    s.now = now;
    if now >= s.deadline {
        return Err(CoreError::Timeout);
    }
    Ok(())
}
