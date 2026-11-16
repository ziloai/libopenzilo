// SPDX-License-Identifier: MPL-2.0
use crate::{invalid, recording_config_body, RecordingConfig, Result};
#[derive(Clone, Debug, uniffi::Enum)]
pub enum DeviceRequest {
    SystemInfo,
    Capabilities,
    ConnectionRssi,
    AudioCount,
    AudioInfo {
        file_index: u32,
    },
    AudioQuick {
        file_index: u32,
    },
    AudioRead {
        file_index: u32,
        offset: u32,
    },
    AudioEnd {
        file_index: u32,
    },
    ClearAudio {
        confirmed: bool,
    },
    LiveSubscribe {
        enabled: bool,
    },
    GetRecordingConfig,
    SetRecordingConfig {
        config: RecordingConfig,
    },
    StartRecording,
    StopRecording {
        session_id: u32,
        cancel: bool,
    },
    RecordingStatus,
    StartImu,
    StopImu,
    LogStorage,
    ReadLog {
        file_index: u32,
        offset: u32,
        length: u32,
    },
    TimeReply {
        request_time: u32,
        response_time: u32,
        send_time: u32,
    },
}
#[derive(Clone, Debug, uniffi::Record)]
pub struct RequestSpec {
    pub command: u16,
    pub response: Option<u16>,
    pub body: Vec<u8>,
    pub feature: Option<String>,
}
#[uniffi::export]
pub fn prepare_request(request: DeviceRequest) -> Result<RequestSpec> {
    let mut body = Vec::new();
    let (command, response, feature) = match request {
        DeviceRequest::SystemInfo => (0x101, Some(0x102), None),
        DeviceRequest::Capabilities => (0x105, Some(0x106), None),
        DeviceRequest::ConnectionRssi => (0x107, Some(0x108), Some("connection_rssi")),
        DeviceRequest::AudioCount => (0x501, Some(0x502), Some("audio_files")),
        DeviceRequest::AudioInfo { file_index } => {
            body.extend([0, 0]);
            body.extend(file_index.to_be_bytes());
            (0x503, Some(0x504), Some("audio_files"))
        }
        DeviceRequest::AudioQuick { file_index } => {
            body.extend([0, 0]);
            body.extend(file_index.to_be_bytes());
            (0x509, None, Some("audio_files"))
        }
        DeviceRequest::AudioRead { file_index, offset } => {
            body.extend([0, 0]);
            body.extend(file_index.to_be_bytes());
            body.extend(offset.to_be_bytes());
            body.extend([0, 0]);
            (0x506, Some(0x505), Some("audio_files"))
        }
        DeviceRequest::AudioEnd { file_index } => {
            body.extend([0, 0]);
            body.extend(file_index.to_be_bytes());
            (0x507, Some(0x508), Some("audio_files"))
        }
        DeviceRequest::ClearAudio { confirmed } => {
            if !confirmed {
                return Err(invalid(
                    "Deleting recordings requires explicit confirmation",
                ));
            }
            (0x50b, Some(0x50c), Some("audio_files"))
        }
        DeviceRequest::LiveSubscribe { enabled } => {
            body.push(enabled as u8);
            (0x50d, Some(0x50e), Some("live_audio"))
        }
        DeviceRequest::GetRecordingConfig => (0x512, Some(0x513), Some("recording_control")),
        DeviceRequest::SetRecordingConfig { config } => {
            body = recording_config_body(config)?;
            (0x514, Some(0x515), Some("recording_control"))
        }
        DeviceRequest::StartRecording => (0x516, Some(0x517), Some("recording_control")),
        DeviceRequest::StopRecording { session_id, cancel } => {
            if session_id == 0 {
                return Err(invalid("session_id must be nonzero"));
            }
            body.extend(session_id.to_be_bytes());
            body.push(cancel as u8);
            (0x518, Some(0x519), Some("recording_control"))
        }
        DeviceRequest::RecordingStatus => (0x51a, Some(0x51b), Some("recording_control")),
        DeviceRequest::StartImu => (0x601, Some(0x602), Some("imu")),
        DeviceRequest::StopImu => (0x603, Some(0x604), Some("imu")),
        DeviceRequest::LogStorage => (0x301, Some(0x302), Some("system")),
        DeviceRequest::ReadLog {
            file_index,
            offset,
            length,
        } => {
            body.extend(file_index.to_be_bytes());
            body.extend(offset.to_be_bytes());
            body.extend(length.to_be_bytes());
            (0x303, Some(0x304), Some("system"))
        }
        DeviceRequest::TimeReply {
            request_time,
            response_time,
            send_time,
        } => {
            body.extend(request_time.to_be_bytes());
            body.extend(response_time.to_be_bytes());
            body.extend(send_time.to_be_bytes());
            (0x402, None, Some("time_sync"))
        }
    };
    Ok(RequestSpec {
        command,
        response,
        body,
        feature: feature.map(String::from),
    })
}
