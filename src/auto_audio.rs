// SPDX-License-Identifier: MPL-2.0
use crate::*;
use std::sync::Mutex;

#[derive(Clone, Debug, uniffi::Record)]
pub struct AutoAudioStep {
    pub request: Option<RequestSpec>,
    pub file_index: Option<u32>,
    pub received: u32,
    pub total: Option<u32>,
    pub phase: DownloadPhase,
}

struct AutoState {
    phase: DownloadPhase,
    file_index: Option<u32>,
    download: Option<DownloadTransfer>,
    data: Vec<u8>,
    present: Vec<bool>,
    received: u32,
    total: Option<u32>,
    metadata_known: bool,
    retries: u8,
    deadline: u64,
    idle_deadline: u64,
    now: u64,
}

#[derive(uniffi::Object)]
pub struct AutoAudioTransfer {
    state: Mutex<AutoState>,
    max_bytes: u32,
    timeout_ms: u64,
    total_timeout_ms: u64,
}

#[uniffi::export]
impl AutoAudioTransfer {
    #[uniffi::constructor]
    pub fn new(max_bytes: u32, timeout_ms: u64, total_timeout_ms: u64) -> Result<Self> {
        if max_bytes == 0
            || max_bytes > 64 * 1024 * 1024
            || timeout_ms == 0
            || total_timeout_ms == 0
        {
            return Err(invalid("Invalid automatic audio transfer limits"));
        }
        Ok(Self {
            state: Mutex::new(AutoState {
                phase: DownloadPhase::Ready,
                file_index: None,
                download: None,
                data: Vec::new(),
                present: Vec::new(),
                received: 0,
                total: None,
                metadata_known: false,
                retries: 0,
                deadline: 0,
                idle_deadline: 0,
                now: 0,
            }),
            max_bytes,
            timeout_ms,
            total_timeout_ms,
        })
    }

    pub fn begin(&self, now_ms: u64) -> Result<AutoAudioStep> {
        let mut s = self.state.lock().unwrap();
        if s.phase != DownloadPhase::Ready {
            return Err(invalid("Automatic audio transfer already started"));
        }
        let Some(deadline) = now_ms.checked_add(self.total_timeout_ms) else {
            s.phase = DownloadPhase::Failed;
            return Err(invalid("Automatic audio deadline overflow"));
        };
        s.deadline = deadline;
        s.now = now_ms;
        s.idle_deadline = now_ms.saturating_add(self.timeout_ms);
        s.phase = DownloadPhase::Receiving;
        step(&s, None)
    }

    pub fn accept(&self, packet: Packet, now_ms: u64) -> Result<AutoAudioStep> {
        let mut s = self.state.lock().unwrap();
        let result = self.accept_inner(&mut s, packet, now_ms);
        if result.is_err() && active(s.phase) {
            fail(&mut s);
        }
        result
    }

    pub fn tick(&self, now_ms: u64) -> Result<AutoAudioStep> {
        let mut s = self.state.lock().unwrap();
        let result = (|| {
            check_time(&mut s, now_ms)?;
            if let Some(download) = &s.download {
                let next = download.tick(now_ms)?;
                return delegated_step(&mut s, next);
            }
            if now_ms < s.idle_deadline {
                return step(&s, None);
            }
            if s.file_index.is_none() || s.phase == DownloadPhase::Metadata {
                return Err(CoreError::Timeout);
            }
            self.recover(&mut s, now_ms)
        })();
        if result.is_err() && active(s.phase) {
            fail(&mut s);
        }
        result
    }

    pub fn next_deadline(&self) -> Option<u64> {
        let s = self.state.lock().unwrap();
        if !active(s.phase) {
            return None;
        }
        let idle = s
            .download
            .as_ref()
            .and_then(|d| d.next_deadline())
            .unwrap_or(s.idle_deadline);
        Some(idle.min(s.deadline))
    }

    pub fn finish(&self) -> Result<Vec<u8>> {
        let s = self.state.lock().unwrap();
        if s.phase != DownloadPhase::Complete {
            return Err(invalid("Automatic audio transfer is not complete"));
        }
        if let Some(download) = &s.download {
            download.finish()
        } else {
            Ok(s.data.clone())
        }
    }

    pub fn cancel(&self) {
        let mut s = self.state.lock().unwrap();
        if s.phase == DownloadPhase::Ready || active(s.phase) {
            if let Some(download) = &s.download {
                download.cancel();
            }
            s.download = None;
            s.data.clear();
            s.present.clear();
            s.phase = DownloadPhase::Cancelled;
        }
    }
}

impl AutoAudioTransfer {
    fn accept_inner(&self, s: &mut AutoState, packet: Packet, now: u64) -> Result<AutoAudioStep> {
        check_time(s, now)?;
        if let Some(download) = &s.download {
            let next = download.accept(packet, now)?;
            return delegated_step(s, next);
        }
        if s.file_index.is_none() && packet.command == 0x50a {
            let index = parse_audio_record_result(packet.body)?;
            let download = DownloadTransfer::new(
                index,
                self.max_bytes,
                false,
                self.timeout_ms,
                s.deadline - now,
            )?;
            let next = download.begin(now)?;
            s.file_index = Some(index);
            s.download = Some(download);
            return delegated_step(s, next);
        }
        if packet.command == 0x1ff {
            let mut reader = crate::protocol::Reader::new(&packet.body);
            let command = reader.u16()?;
            let code = reader.u16()?;
            reader.end()?;
            let expected = if s.phase == DownloadPhase::Metadata {
                0x503
            } else {
                0x506
            };
            if s.file_index.is_some() && command == expected && code != 0 {
                return Err(CoreError::Device { code });
            }
            return Err(invalid("Unexpected automatic audio error response"));
        }
        match packet.command {
            0x505 => {
                let frame = parse_audio_frame(packet.body)?;
                let previous = s.received;
                self.push(s, frame)?;
                if s.received > previous {
                    s.retries = 0;
                    if s.phase != DownloadPhase::Metadata {
                        s.idle_deadline = now.saturating_add(self.timeout_ms);
                    }
                }
                if s.phase == DownloadPhase::Metadata {
                    return step(s, None);
                }
                if s.total == Some(s.received) {
                    s.phase = DownloadPhase::Complete;
                    return step(s, None);
                }
                if s.data.len() > s.received as usize {
                    return self.recover(s, now);
                }
                step(s, None)
            }
            0x504 if s.phase == DownloadPhase::Metadata => {
                let info = parse_audio_info(packet.body)?;
                if Some(info.file_index) != s.file_index
                    || info.data_size > self.max_bytes
                    || info.data_size < s.data.len() as u32
                    || s.total.is_some_and(|total| total != info.data_size)
                {
                    return Err(invalid("Automatic audio metadata identity or size differs"));
                }
                s.total = Some(info.data_size);
                s.metadata_known = true;
                s.idle_deadline = now.saturating_add(self.timeout_ms);
                if s.received == info.data_size {
                    s.phase = DownloadPhase::Complete;
                    step(s, None)
                } else {
                    s.phase = DownloadPhase::Receiving;
                    step(
                        s,
                        Some(DeviceRequest::AudioRead {
                            file_index: info.file_index,
                            offset: s.received,
                        }),
                    )
                }
            }
            _ => Err(invalid("Unexpected automatic audio phase or command")),
        }
    }

    fn push(&self, s: &mut AutoState, frame: AudioFrame) -> Result<()> {
        if s.file_index.is_some_and(|index| index != frame.file_index) {
            return Err(invalid("Automatic audio frame belongs to another file"));
        }
        let end = frame
            .frame_offset
            .checked_add(frame.frame_size)
            .ok_or_else(|| invalid("Automatic audio frame offset overflow"))?;
        if end > self.max_bytes
            || s.total.is_some_and(|total| end > total)
            || (frame.data.is_empty() && !frame.is_end)
        {
            return Err(invalid("Automatic audio frame exceeds limits or is empty"));
        }
        if frame.is_end {
            if s.total.is_some_and(|total| total != end) || end < s.data.len() as u32 {
                return Err(invalid("Conflicting automatic audio end offset"));
            }
            s.total = Some(end);
        }
        s.file_index = Some(frame.file_index);
        s.data.resize(s.data.len().max(end as usize), 0);
        s.present.resize(s.data.len(), false);
        for (i, value) in frame.data.into_iter().enumerate() {
            let offset = frame.frame_offset as usize + i;
            if s.present[offset] && s.data[offset] != value {
                return Err(invalid("Conflicting automatic audio overlap"));
            }
            s.data[offset] = value;
            s.present[offset] = true;
        }
        while (s.received as usize) < s.present.len() && s.present[s.received as usize] {
            s.received += 1;
        }
        Ok(())
    }

    fn recover(&self, s: &mut AutoState, now: u64) -> Result<AutoAudioStep> {
        if s.retries >= 3 {
            return Err(invalid("Automatic audio recovery retry budget exhausted"));
        }
        s.retries += 1;
        s.idle_deadline = now.saturating_add(self.timeout_ms);
        let index = s.file_index.unwrap();
        if !s.metadata_known {
            s.phase = DownloadPhase::Metadata;
            step(s, Some(DeviceRequest::AudioInfo { file_index: index }))
        } else {
            step(
                s,
                Some(DeviceRequest::AudioRead {
                    file_index: index,
                    offset: s.received,
                }),
            )
        }
    }
}

fn active(phase: DownloadPhase) -> bool {
    matches!(
        phase,
        DownloadPhase::Metadata | DownloadPhase::Receiving | DownloadPhase::Finishing
    )
}

fn check_time(s: &mut AutoState, now: u64) -> Result<()> {
    if !active(s.phase) {
        return Err(invalid("Automatic audio transfer is not active"));
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

fn fail(s: &mut AutoState) {
    if let Some(download) = &s.download {
        download.cancel();
    }
    s.download = None;
    s.data.clear();
    s.present.clear();
    s.phase = DownloadPhase::Failed;
}

fn step(s: &AutoState, request: Option<DeviceRequest>) -> Result<AutoAudioStep> {
    Ok(AutoAudioStep {
        request: request.map(prepare_request).transpose()?,
        file_index: s.file_index,
        received: s.received,
        total: s.total,
        phase: s.phase,
    })
}

fn delegated_step(s: &mut AutoState, next: DownloadStep) -> Result<AutoAudioStep> {
    s.phase = next.phase;
    s.received = next.received;
    s.total = s
        .download
        .as_ref()
        .and_then(|download| download.info())
        .map(|info| info.data_size);
    Ok(AutoAudioStep {
        request: next.request,
        file_index: s.file_index,
        received: s.received,
        total: s.total,
        phase: s.phase,
    })
}
