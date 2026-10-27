// SPDX-License-Identifier: MPL-2.0
use crate::*;
use std::sync::Mutex;
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum DownloadPhase {
    Ready,
    Metadata,
    Receiving,
    Finishing,
    Complete,
    Failed,
    Cancelled,
}
#[derive(Clone, Debug, uniffi::Record)]
pub struct DownloadStep {
    pub request: Option<RequestSpec>,
    pub received: u32,
    pub total: u32,
    pub phase: DownloadPhase,
}
struct State {
    phase: DownloadPhase,
    info: Option<AudioFileInfo>,
    assembler: Option<AudioAssembler>,
    received: u32,
    retries: u8,
    deadline: u64,
    idle_deadline: u64,
    now: u64,
}
#[derive(uniffi::Object)]
pub struct DownloadTransfer {
    state: Mutex<State>,
    file_index: u32,
    max_bytes: u32,
    quick: bool,
    timeout_ms: u64,
    total_timeout_ms: u64,
}
#[uniffi::export]
impl DownloadTransfer {
    #[uniffi::constructor]
    pub fn new(
        file_index: u32,
        max_bytes: u32,
        quick: bool,
        timeout_ms: u64,
        total_timeout_ms: u64,
    ) -> Result<Self> {
        if max_bytes == 0
            || max_bytes > 64 * 1024 * 1024
            || timeout_ms == 0
            || total_timeout_ms == 0
        {
            return Err(invalid("Invalid download limits"));
        }
        Ok(Self {
            state: Mutex::new(State {
                phase: DownloadPhase::Ready,
                info: None,
                assembler: None,
                received: 0,
                retries: 0,
                deadline: 0,
                idle_deadline: 0,
                now: 0,
            }),
            file_index,
            max_bytes,
            quick,
            timeout_ms,
            total_timeout_ms,
        })
    }
    pub fn begin(&self, now_ms: u64) -> Result<DownloadStep> {
        let mut s = self.state.lock().unwrap();
        if s.phase != DownloadPhase::Ready {
            return Err(invalid("Download already started"));
        }
        s.deadline = now_ms
            .checked_add(self.total_timeout_ms)
            .ok_or_else(|| invalid("Download deadline overflow"))?;
        s.now = now_ms;
        s.idle_deadline = now_ms.saturating_add(self.timeout_ms);
        s.phase = DownloadPhase::Metadata;
        self.step(
            &s,
            Some(if self.quick {
                DeviceRequest::AudioQuick {
                    file_index: self.file_index,
                }
            } else {
                DeviceRequest::AudioInfo {
                    file_index: self.file_index,
                }
            }),
        )
    }
    pub fn accept(&self, packet: Packet, now_ms: u64) -> Result<DownloadStep> {
        let mut s = self.state.lock().unwrap();
        let result = self.accept_inner(&mut s, packet, now_ms);
        if result.is_err() {
            s.phase = DownloadPhase::Failed;
        }
        result
    }
    pub fn tick(&self, now_ms: u64) -> Result<DownloadStep> {
        let mut s = self.state.lock().unwrap();
        let result = (|| {
            self.check_time(&mut s, now_ms)?;
            if now_ms < s.idle_deadline {
                return self.step(&s, None);
            }
            if s.phase != DownloadPhase::Receiving {
                return Err(CoreError::Timeout);
            }
            self.retry(&mut s, now_ms)
        })();
        if result.is_err() {
            s.phase = DownloadPhase::Failed;
        }
        result
    }
    pub fn next_deadline(&self) -> Option<u64> {
        let s = self.state.lock().unwrap();
        if matches!(
            s.phase,
            DownloadPhase::Metadata | DownloadPhase::Receiving | DownloadPhase::Finishing
        ) {
            Some(s.idle_deadline.min(s.deadline))
        } else {
            None
        }
    }
    pub fn info(&self) -> Option<AudioFileInfo> {
        self.state.lock().unwrap().info.clone()
    }
    pub fn finish(&self) -> Result<Vec<u8>> {
        let s = self.state.lock().unwrap();
        if s.phase != DownloadPhase::Complete {
            return Err(invalid("Download completion unconfirmed"));
        }
        s.assembler.as_ref().unwrap().finish()
    }
    pub fn cancel(&self) {
        let mut s = self.state.lock().unwrap();
        s.phase = DownloadPhase::Cancelled;
        s.assembler = None;
    }
}
impl DownloadTransfer {
    fn check_time(&self, s: &mut State, now: u64) -> Result<()> {
        if !matches!(
            s.phase,
            DownloadPhase::Metadata | DownloadPhase::Receiving | DownloadPhase::Finishing
        ) {
            return Err(invalid("Download is not active"));
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
    fn step(&self, s: &State, request: Option<DeviceRequest>) -> Result<DownloadStep> {
        Ok(DownloadStep {
            request: request.map(prepare_request).transpose()?,
            received: s.received,
            total: s.info.as_ref().map(|i| i.data_size).unwrap_or(0),
            phase: s.phase,
        })
    }
    fn retry(&self, s: &mut State, now: u64) -> Result<DownloadStep> {
        if s.retries >= 3 {
            return Err(invalid("Audio stream stalled: retry budget exhausted"));
        }
        s.retries += 1;
        s.idle_deadline = now.saturating_add(self.timeout_ms);
        self.step(
            s,
            Some(DeviceRequest::AudioRead {
                file_index: self.file_index,
                offset: s.assembler.as_ref().unwrap().missing_offset().unwrap_or(0),
            }),
        )
    }
    fn accept_inner(&self, s: &mut State, p: Packet, now: u64) -> Result<DownloadStep> {
        self.check_time(s, now)?;
        if p.command == 0x1ff {
            let mut r = crate::protocol::Reader::new(&p.body);
            let command = r.u16()?;
            let code = r.u16()?;
            r.end()?;
            if [0x503, 0x506, 0x507, 0x509].contains(&command) && code != 0 {
                return Err(CoreError::Device { code });
            }
            return Err(invalid("Unexpected download error response"));
        }
        match (s.phase, p.command) {
            (DownloadPhase::Metadata, 0x504) => {
                let info = parse_audio_info(p.body)?;
                if info.file_index != self.file_index {
                    return Err(invalid("Audio metadata index mismatch"));
                }
                s.assembler = Some(AudioAssembler::new(
                    self.file_index,
                    info.data_size,
                    self.max_bytes,
                )?);
                s.info = Some(info);
                s.phase = DownloadPhase::Receiving;
                s.idle_deadline = now.saturating_add(self.timeout_ms);
                if s.info.as_ref().unwrap().data_size == 0 {
                    s.phase = DownloadPhase::Finishing;
                    return self.step(
                        s,
                        Some(DeviceRequest::AudioEnd {
                            file_index: self.file_index,
                        }),
                    );
                }
                self.step(
                    s,
                    if self.quick {
                        None
                    } else {
                        Some(DeviceRequest::AudioRead {
                            file_index: self.file_index,
                            offset: 0,
                        })
                    },
                )
            }
            (DownloadPhase::Receiving, 0x505) => {
                let frame = parse_audio_frame(p.body)?;
                let is_end = frame.is_end;
                if frame.data.is_empty() {
                    return Err(invalid("Empty audio frame before completion"));
                }
                let previous = s.received;
                s.received = s.assembler.as_ref().unwrap().push(frame)?;
                if s.received > previous {
                    s.retries = 0;
                    s.idle_deadline = now.saturating_add(self.timeout_ms);
                }
                let missing = s.assembler.as_ref().unwrap().missing_offset();
                if missing.is_none() {
                    s.phase = DownloadPhase::Finishing;
                    s.idle_deadline = now.saturating_add(self.timeout_ms);
                    return self.step(
                        s,
                        Some(DeviceRequest::AudioEnd {
                            file_index: self.file_index,
                        }),
                    );
                }
                if is_end || s.received == previous {
                    return self.retry(s, now);
                }
                self.step(
                    s,
                    if self.quick {
                        None
                    } else {
                        Some(DeviceRequest::AudioRead {
                            file_index: self.file_index,
                            offset: missing.unwrap(),
                        })
                    },
                )
            }
            (DownloadPhase::Finishing, 0x508) => {
                if parse_audio_end(p.body)? != self.file_index {
                    return Err(invalid("Audio completion identity differs"));
                }
                s.assembler.as_ref().unwrap().finish()?;
                s.phase = DownloadPhase::Complete;
                self.step(s, None)
            }
            // Late duplicate frames are harmless during END confirmation; they must agree.
            (DownloadPhase::Finishing, 0x505) => {
                s.assembler
                    .as_ref()
                    .unwrap()
                    .push(parse_audio_frame(p.body)?)?;
                self.step(s, None)
            }
            _ => Err(invalid("Unexpected download phase or command")),
        }
    }
}
