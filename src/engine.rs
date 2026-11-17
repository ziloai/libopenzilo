// SPDX-License-Identifier: MPL-2.0
//! Portable policy and request engine. The host serializes calls and executes
//! complete RequestTicket frames on its own transport and monotonic clock.
use crate::*;
use std::{collections::BTreeMap, sync::Mutex};
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum ActivityState {
    Idle,
    Active,
    Finalizing,
    Unknown,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum SubscriptionState {
    Disabled,
    Enabled,
    Unknown,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum ExclusiveOperation {
    AudioDownload,
    FirmwareUpdate,
}
#[derive(Clone, Debug, uniffi::Record)]
pub struct DeviceState {
    pub recording: ActivityState,
    pub session_id: Option<u32>,
    pub live: SubscriptionState,
    pub imu: SubscriptionState,
}
struct PendingOperation {
    command: u16,
    response: u16,
    body: Vec<u8>,
    before: DeviceState,
}
struct EngineState {
    generation: Option<u64>,
    info: Option<DeviceInfo>,
    profile: CapabilityProfile,
    activity: DeviceState,
    pending: BTreeMap<u64, PendingOperation>,
}
fn unknown_profile() -> CapabilityProfile {
    CapabilityProfile {
        source: CapabilitySource::Unknown,
        capabilities: None,
        reason: None,
    }
}
fn initial_state() -> DeviceState {
    DeviceState {
        recording: ActivityState::Unknown,
        session_id: None,
        live: SubscriptionState::Disabled,
        imu: SubscriptionState::Disabled,
    }
}
#[derive(uniffi::Object)]
pub struct DeviceEngine {
    session: DeviceSession,
    state: Mutex<EngineState>,
}
#[uniffi::export]
impl DeviceEngine {
    #[uniffi::constructor]
    pub fn new(event_capacity: u32) -> Result<Self> {
        Ok(Self {
            session: DeviceSession::new(event_capacity)?,
            state: Mutex::new(EngineState {
                generation: None,
                info: None,
                profile: unknown_profile(),
                activity: initial_state(),
                pending: BTreeMap::new(),
            }),
        })
    }
    pub fn open(&self) -> Result<u64> {
        let generation = self.session.open()?;
        let mut s = self.state.lock().unwrap();
        s.generation = Some(generation);
        s.info = None;
        s.profile = unknown_profile();
        s.activity = initial_state();
        s.pending.clear();
        Ok(generation)
    }
    pub fn profile(&self) -> CapabilityProfile {
        self.state.lock().unwrap().profile.clone()
    }
    pub fn device_state(&self) -> DeviceState {
        self.state.lock().unwrap().activity.clone()
    }
    /// Explicit compatibility opt-in is supplied by the host, never inferred.
    pub fn check_access(&self, command: u16, allow_unverified: bool) -> Result<()> {
        let s = self.state.lock().unwrap();
        check_command_contract(
            command,
            s.profile
                .capabilities
                .as_ref()
                .map(|c| c.features.clone())
                .unwrap_or_default(),
            s.profile.source,
            allow_unverified,
        )
    }
    pub fn set_capabilities(&self, info: DeviceInfo, caps: Capabilities) -> Result<()> {
        verify_capability_identity(info.clone(), caps.clone())?;
        let mut s = self.state.lock().unwrap();
        s.info = Some(info);
        s.profile = CapabilityProfile {
            source: CapabilitySource::Device,
            capabilities: Some(caps),
            reason: None,
        };
        Ok(())
    }
    pub fn use_legacy_profile(&self, info: DeviceInfo, reason: String) -> CapabilityProfile {
        let mut s = self.state.lock().unwrap();
        s.profile = legacy_capability_profile(info.clone(), reason);
        s.info = Some(info);
        s.profile.clone()
    }
    /// Call after capability admission. Also supports raw legacy commands.
    pub fn begin(
        &self,
        command: u16,
        response: u16,
        body: Vec<u8>,
        now_ms: u64,
        timeout_ms: u64,
        owner: Option<String>,
    ) -> Result<RequestTicket> {
        let mut s = self.state.lock().unwrap();
        if command == 0x514 {
            // The outgoing wire body is parsed independently from readback.
            if let Some(caps) = &s.profile.capabilities {
                if let Some(limits) = &caps.recording_limits {
                    validate_recording_config(config_from_body(&body)?, limits.clone())?;
                }
            }
        }
        let ticket =
            self.session
                .begin(command, response, body.clone(), now_ms, timeout_ms, owner)?;
        let before = s.activity.clone();
        match command {
            0x105 => s.profile = unknown_profile(),
            0x516 | 0x518 => s.activity.recording = ActivityState::Unknown,
            0x50d => s.activity.live = SubscriptionState::Unknown,
            0x601 | 0x603 => s.activity.imu = SubscriptionState::Unknown,
            _ => (),
        }
        s.pending.insert(
            ticket.id,
            PendingOperation {
                command,
                response,
                body,
                before,
            },
        );
        Ok(ticket)
    }
    pub fn prepare_send(
        &self,
        command: u16,
        body: Vec<u8>,
        owner: Option<String>,
    ) -> Result<Vec<u8>> {
        self.session.prepare_send(command, body, owner)
    }
    pub fn receive(&self, data: Vec<u8>, generation: u64, now_ms: u64) -> Result<Vec<Delivery>> {
        let deliveries = self.session.receive_direct(data, generation, now_ms)?;
        Ok(self.apply(deliveries))
    }
    pub fn tick(&self, now_ms: u64) -> Vec<Delivery> {
        self.apply(self.session.tick(now_ms))
    }
    pub fn cancel(&self, request_id: u64) -> Option<Delivery> {
        self.session
            .cancel(request_id)
            .map(|d| self.apply(vec![d]).remove(0))
    }
    pub fn next_deadline(&self) -> Option<u64> {
        self.session.next_deadline()
    }
    pub fn reconnect_required(&self) -> bool {
        self.session.reconnect_required()
    }
    pub fn take_events(&self) -> Vec<Packet> {
        self.session.take_events()
    }
    pub fn take_dropped_crc_packets(&self) -> u64 {
        self.session.take_dropped_crc_packets()
    }
    pub fn interrupt_write(&self, generation: u64) {
        self.session.interrupt_write(generation);
    }
    pub fn disconnect(&self, generation: u64) -> Vec<Delivery> {
        if self.state.lock().unwrap().generation != Some(generation) {
            return Vec::new();
        }
        let deliveries = self.apply(self.session.disconnect(generation));
        let mut s = self.state.lock().unwrap();
        s.generation = None;
        s.profile = unknown_profile();
        s.info = None;
        s.activity = initial_state();
        deliveries
    }
    pub fn acquire_exclusive(&self, owner: String) -> Result<()> {
        self.session.acquire_exclusive(owner)
    }
    pub fn acquire_operation(&self, operation: ExclusiveOperation, owner: String) -> Result<()> {
        match operation {
            ExclusiveOperation::FirmwareUpdate => self.require_ota_idle()?,
            ExclusiveOperation::AudioDownload => {
                if self.device_state().live != SubscriptionState::Disabled {
                    return Err(CoreError::Busy {
                        detail: "Disable live audio before stored-file extraction".into(),
                    });
                }
            }
        }
        self.session.acquire_exclusive(owner)
    }
    pub fn release_exclusive(&self, owner: String) -> Result<()> {
        self.session.release_exclusive(owner)
    }
    pub fn require_ota_idle(&self) -> Result<()> {
        let s = self.state.lock().unwrap();
        let legacy_ota = s.profile.source == CapabilitySource::VerifiedLegacyProfile
            && s.profile
                .capabilities
                .as_ref()
                .is_some_and(|c| c.build_id == "bccb2ffcbbe18cc63f2b5d4954432d2c7046fe56");
        let a = &s.activity;
        // This legacy build has OTA but no recording-status command.
        if (!legacy_ota && a.recording != ActivityState::Idle)
            || a.live != SubscriptionState::Disabled
            || a.imu != SubscriptionState::Disabled
        {
            return Err(CoreError::Busy {
                detail: "Confirm recording stopped and disable live/IMU before OTA".into(),
            });
        }
        Ok(())
    }
}
impl DeviceEngine {
    fn apply(&self, deliveries: Vec<Delivery>) -> Vec<Delivery> {
        let mut s = self.state.lock().unwrap();
        deliveries
            .into_iter()
            .map(|d| {
                match &d {
                    Delivery::Response { request_id, packet } => {
                        if let Some(p) = s.pending.remove(request_id) {
                            // A normal response carrying a nonzero device status is a definite rejection.
                            if p.command != 0x107
                                && packet.body.len() >= 2
                                && packet.body[..2] != [0, 0]
                            {
                                s.activity = restore_changed(s.activity.clone(), &p);
                            } else if let Err(e) = apply_response(&mut s, &p, packet) {
                                self.session.quarantine(p.command, p.response);
                                return Delivery::Failure {
                                    request_id: *request_id,
                                    kind: "protocol".into(),
                                    code: ErrorCode::InvalidData,
                                    recovery: Recovery::ReconnectAndQueryStatus,
                                    message: e.to_string(),
                                    device_code: None,
                                };
                            }
                        }
                    }
                    Delivery::Failure {
                        request_id, code, ..
                    } => {
                        if let Some(p) = s.pending.remove(request_id) {
                            if *code == ErrorCode::DeviceRejected {
                                s.activity = restore_changed(s.activity.clone(), &p);
                            }
                        }
                    }
                    Delivery::Fault { .. } => {
                        s.profile = unknown_profile();
                        s.activity = initial_state();
                    }
                    _ => (),
                }
                d
            })
            .collect()
    }
}
fn restore_changed(mut a: DeviceState, p: &PendingOperation) -> DeviceState {
    match p.command {
        0x516 | 0x518 => {
            a.recording = p.before.recording;
            a.session_id = p.before.session_id;
        }
        0x50d => a.live = p.before.live,
        0x601 | 0x603 => a.imu = p.before.imu,
        _ => (),
    }
    a
}
fn apply_response(s: &mut EngineState, p: &PendingOperation, packet: &Packet) -> Result<()> {
    match p.command {
        0x101 => {
            let info = parse_system_info(packet.body.clone())?;
            if s.info.as_ref().is_some_and(|old| {
                old.model != info.model
                    || old.firmware_version != info.firmware_version
                    || old.cpuid != info.cpuid
                    || old.sn != info.sn
            }) {
                s.profile = unknown_profile();
                s.activity = DeviceState {
                    recording: ActivityState::Unknown,
                    session_id: None,
                    live: SubscriptionState::Unknown,
                    imu: SubscriptionState::Unknown,
                };
            }
            s.info = Some(info);
        }
        0x105 => {
            let caps = parse_capabilities(packet.body.clone())?;
            let info = s
                .info
                .clone()
                .ok_or_else(|| invalid("Query system information before capabilities"))?;
            verify_capability_identity(info, caps.clone())?;
            s.profile = CapabilityProfile {
                source: CapabilitySource::Device,
                capabilities: Some(caps),
                reason: None,
            };
        }
        0x107 => match parse_connection_rssi(packet.body.clone()) {
            Ok(_) | Err(CoreError::Device { .. }) => (),
            Err(e) => return Err(e),
        },
        0x514 => {
            if parse_recording_config(packet.body.clone())? != config_from_body(&p.body)? {
                return Err(invalid("Configuration readback differs"));
            }
        }
        0x516 => {
            s.activity.session_id = Some(parse_session_ack(packet.body.clone())?);
            s.activity.recording = ActivityState::Active;
        }
        0x518 => {
            let id = parse_session_ack(packet.body.clone())?;
            if p.body.len() != 5 || id != u32::from_be_bytes(p.body[..4].try_into().unwrap()) {
                return Err(invalid("Recording stop session differs"));
            }
            s.activity.session_id = Some(id);
            s.activity.recording = ActivityState::Finalizing;
        }
        0x51a => {
            let status = parse_recording_status(packet.body.clone())?;
            s.activity.session_id = (status.session_id != 0).then_some(status.session_id);
            s.activity.recording = match status.state {
                1 | 2 => ActivityState::Active,
                3 => ActivityState::Finalizing,
                _ => ActivityState::Idle,
            };
        }
        0x50d => {
            let enabled = parse_live_subscription(packet.body.clone())?;
            if p.body != [u8::from(enabled)] {
                return Err(invalid("Live subscription readback differs"));
            }
            s.activity.live = if enabled {
                SubscriptionState::Enabled
            } else {
                SubscriptionState::Disabled
            };
        }
        0x601 => {
            parse_imu_config(packet.body.clone())?;
            s.activity.imu = SubscriptionState::Enabled;
        }
        0x603 => {
            parse_success(packet.body.clone())?;
            s.activity.imu = SubscriptionState::Disabled;
        }
        _ => (),
    }
    Ok(())
}
fn config_from_body(body: &[u8]) -> Result<RecordingConfig> {
    if body.len() != 14 || body[0] != 1 {
        return Err(invalid("Invalid outgoing configuration"));
    }
    let c = RecordingConfig {
        output: body[1],
        disconnect_policy: body[2],
        channels: body[3],
        bitrate: u32::from_be_bytes(body[4..8].try_into().unwrap()),
        gain_left_q8: u16::from_be_bytes(body[8..10].try_into().unwrap()),
        gain_right_q8: u16::from_be_bytes(body[10..12].try_into().unwrap()),
        max_duration_ms: u16::from_be_bytes(body[12..14].try_into().unwrap()),
    };
    recording_config_body(c.clone())?;
    Ok(c)
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum RecordingCompletion {
    Pending,
    Complete,
    Cancelled,
}
#[uniffi::export]
pub fn recording_completion(
    status: RecordingStatus,
    session_id: u32,
) -> Result<RecordingCompletion> {
    if session_id == 0 || status.session_id != session_id {
        return Err(invalid("Recording status belongs to another session"));
    }
    if status.error_code != 0 {
        return Err(CoreError::Device {
            code: status.error_code,
        });
    }
    Ok(match status.state {
        4 => RecordingCompletion::Complete,
        6 => RecordingCompletion::Cancelled,
        _ => RecordingCompletion::Pending,
    })
}
