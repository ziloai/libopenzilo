// SPDX-License-Identifier: MPL-2.0
use crate::{encode_packet, invalid, CoreError, Packet, PacketDecoder, Result};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::Mutex,
};
#[derive(Clone, Debug, uniffi::Record)]
pub struct RequestTicket {
    pub id: u64,
    pub generation: u64,
    pub frame: Vec<u8>,
}
#[derive(Clone, Debug, uniffi::Enum)]
pub enum Delivery {
    Response {
        request_id: u64,
        packet: Packet,
    },
    Event {
        packet: Packet,
    },
    Failure {
        request_id: u64,
        kind: String,
        code: crate::ErrorCode,
        recovery: crate::Recovery,
        message: String,
        device_code: Option<u16>,
    },
    Fault {
        code: crate::ErrorCode,
        message: String,
        recovery: crate::Recovery,
    },
}
struct Pending {
    id: u64,
    command: u16,
    response: u16,
    deadline: u64,
}
#[derive(Default)]
struct State {
    generation: u64,
    next_id: u64,
    connected: bool,
    write_failed: bool,
    pending: BTreeMap<u64, Pending>,
    events: VecDeque<Packet>,
    exclusive: Option<String>,
    quarantined: BTreeSet<u16>,
    dropped_crc_packets: u64,
}
#[derive(uniffi::Object)]
pub struct DeviceSession {
    state: Mutex<State>,
    decoder: PacketDecoder,
    event_capacity: u32,
}
fn failure(id: u64, kind: &str, message: &str, code: Option<u16>) -> Delivery {
    Delivery::Failure {
        request_id: id,
        kind: kind.into(),
        code: match kind {
            "device" => crate::ErrorCode::DeviceRejected,
            "timeout" => crate::ErrorCode::Timeout,
            "cancelled" => crate::ErrorCode::Cancelled,
            "overflow" => crate::ErrorCode::Overflow,
            _ => crate::ErrorCode::Disconnected,
        },
        recovery: if kind == "device" {
            crate::Recovery::None
        } else if matches!(kind, "timeout" | "cancelled") {
            crate::Recovery::ReconnectAndQueryStatus
        } else {
            crate::Recovery::Reconnect
        },
        message: message.into(),
        device_code: code,
    }
}
#[uniffi::export]
impl DeviceSession {
    pub(crate) fn quarantine(&self, command: u16, response: u16) {
        let mut s = self.state.lock().unwrap();
        s.quarantined.insert(command);
        s.quarantined.insert(response);
    }

    #[uniffi::constructor]
    pub fn new(event_capacity: u32) -> Result<Self> {
        if !(1..=65536).contains(&event_capacity) {
            return Err(invalid("event_capacity must be 1..65536"));
        }
        Ok(Self {
            state: Mutex::new(State::default()),
            decoder: PacketDecoder::new(),
            event_capacity,
        })
    }
    pub fn open(&self) -> Result<u64> {
        let mut s = self.state.lock().unwrap();
        if s.connected {
            return Err(CoreError::Busy {
                detail: "Disconnect the previous session first".into(),
            });
        }
        s.generation = s
            .generation
            .checked_add(1)
            .ok_or_else(|| invalid("Session generation exhausted"))?;
        s.connected = true;
        s.write_failed = false;
        s.events.clear();
        s.pending.clear();
        s.quarantined.clear();
        s.dropped_crc_packets = 0;
        s.exclusive = None;
        self.decoder.clear();
        Ok(s.generation)
    }
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
        if !s.connected || s.write_failed {
            return Err(CoreError::Disconnected);
        }
        if s.exclusive.is_some() && s.exclusive != owner && command != 0x402 {
            return Err(CoreError::Busy {
                detail: "An exclusive operation owns this connection".into(),
            });
        }
        if s.quarantined.contains(&command) || s.quarantined.contains(&response) {
            return Err(CoreError::OutcomeUnknown);
        }
        if s.pending.len() >= 256 {
            return Err(CoreError::Busy {
                detail: "Too many pending requests".into(),
            });
        }
        if s.pending
            .values()
            .any(|p| p.response == response || p.command == command)
        {
            return Err(CoreError::Busy {
                detail: "A request with the same response or command is already pending".into(),
            });
        }
        if timeout_ms == 0 {
            return Err(CoreError::Timeout);
        }
        let deadline = now_ms
            .checked_add(timeout_ms)
            .ok_or_else(|| invalid("Timeout overflow"))?;
        let frame = encode_packet(command, body)?;
        s.next_id = s
            .next_id
            .checked_add(1)
            .ok_or_else(|| invalid("Request ID exhausted"))?;
        let id = s.next_id;
        s.pending.insert(
            id,
            Pending {
                id,
                command,
                response,
                deadline,
            },
        );
        Ok(RequestTicket {
            id,
            generation: s.generation,
            frame,
        })
    }
    pub fn receive(&self, data: Vec<u8>, generation: u64, now_ms: u64) -> Result<Vec<Delivery>> {
        self.receive_with_buffer(data, generation, now_ms, true)
    }
    /// Direct delivery for hosts that own bounded queues; avoids duplicate event storage.
    pub fn receive_direct(
        &self,
        data: Vec<u8>,
        generation: u64,
        now_ms: u64,
    ) -> Result<Vec<Delivery>> {
        self.receive_with_buffer(data, generation, now_ms, false)
    }
    pub fn next_deadline(&self) -> Option<u64> {
        self.state
            .lock()
            .unwrap()
            .pending
            .values()
            .map(|p| p.deadline)
            .min()
    }
    pub fn reconnect_required(&self) -> bool {
        let s = self.state.lock().unwrap();
        s.write_failed || !s.quarantined.is_empty()
    }
    /// Encode a command without a response while respecting connection ownership.
    pub fn prepare_send(
        &self,
        command: u16,
        body: Vec<u8>,
        owner: Option<String>,
    ) -> Result<Vec<u8>> {
        let s = self.state.lock().unwrap();
        if !s.connected || s.write_failed {
            return Err(CoreError::Disconnected);
        }
        if s.exclusive.is_some() && s.exclusive != owner && command != 0x402 {
            return Err(CoreError::Busy {
                detail: "An exclusive operation owns this connection".into(),
            });
        }
        encode_packet(command, body)
    }
    pub fn tick(&self, now_ms: u64) -> Vec<Delivery> {
        expire(&mut self.state.lock().unwrap(), now_ms)
    }
    pub fn cancel(&self, request_id: u64) -> Option<Delivery> {
        let mut s = self.state.lock().unwrap();
        s.pending.remove(&request_id).map(|p| {
            s.quarantined.insert(p.command);
            s.quarantined.insert(p.response);
            failure(request_id, "cancelled", "Operation cancelled", None)
        })
    }
    pub fn disconnect(&self, generation: u64) -> Vec<Delivery> {
        let mut s = self.state.lock().unwrap();
        if s.generation != generation {
            return Vec::new();
        }
        s.connected = false;
        s.events.clear();
        s.exclusive = None;
        self.decoder.clear();
        let out = s
            .pending
            .keys()
            .map(|id| failure(*id, "disconnected", "Transport disconnected", None))
            .collect();
        s.pending.clear();
        out
    }
    pub fn interrupt_write(&self, generation: u64) {
        let mut s = self.state.lock().unwrap();
        if s.generation == generation {
            s.write_failed = true;
        }
    }
    pub fn take_events(&self) -> Vec<Packet> {
        self.state.lock().unwrap().events.drain(..).collect()
    }
    pub fn take_dropped_crc_packets(&self) -> u64 {
        std::mem::take(&mut self.state.lock().unwrap().dropped_crc_packets)
    }
    pub fn acquire_exclusive(&self, owner: String) -> Result<()> {
        let mut s = self.state.lock().unwrap();
        if owner.is_empty() {
            return Err(invalid("Exclusive owner must be nonempty"));
        }
        if s.exclusive.is_some() || !s.pending.is_empty() {
            return Err(CoreError::Busy {
                detail: "Connection has an active operation".into(),
            });
        }
        s.exclusive = Some(owner);
        Ok(())
    }
    pub fn release_exclusive(&self, owner: String) -> Result<()> {
        let mut s = self.state.lock().unwrap();
        if s.exclusive.as_ref() != Some(&owner) {
            return Err(CoreError::Busy {
                detail: "Exclusive operation owner differs".into(),
            });
        }
        s.exclusive = None;
        Ok(())
    }
}
fn expire(s: &mut State, now: u64) -> Vec<Delivery> {
    let ids: Vec<u64> = s
        .pending
        .values()
        .filter(|p| p.deadline <= now)
        .map(|p| p.id)
        .collect();
    ids.into_iter()
        .map(|id| {
            if let Some(p) = s.pending.remove(&id) {
                s.quarantined.insert(p.command);
                s.quarantined.insert(p.response);
            }
            failure(id, "timeout", "Request timed out", None)
        })
        .collect()
}

#[derive(uniffi::Object)]
pub struct AudioAssembler {
    state: Mutex<AudioState>,
}
struct AudioState {
    index: u32,
    size: u32,
    data: Vec<u8>,
    received: Vec<bool>,
    received_bytes: u32,
    missing: usize,
}
#[uniffi::export]
impl AudioAssembler {
    #[uniffi::constructor]
    pub fn new(file_index: u32, data_size: u32, max_size: u32) -> Result<Self> {
        if data_size > max_size || max_size > 64 * 1024 * 1024 {
            return Err(invalid("Audio size exceeds limit"));
        }
        Ok(Self {
            state: Mutex::new(AudioState {
                index: file_index,
                size: data_size,
                data: vec![0; data_size as usize],
                received: vec![false; data_size as usize],
                received_bytes: 0,
                missing: 0,
            }),
        })
    }
    pub fn push(&self, frame: crate::AudioFrame) -> Result<u32> {
        let mut s = self.state.lock().unwrap();
        if frame.file_index != s.index || frame.frame_size as usize != frame.data.len() {
            return Err(invalid("Audio frame identity or size differs"));
        }
        let start = frame.frame_offset as usize;
        let end = start
            .checked_add(frame.data.len())
            .ok_or_else(|| invalid("Audio offset overflow"))?;
        if end > s.size as usize {
            return Err(invalid("Audio frame exceeds file"));
        }
        for (i, &b) in frame.data.iter().enumerate() {
            let j = start + i;
            if s.received[j] && s.data[j] != b {
                return Err(invalid("Conflicting audio retransmission"));
            }
        }
        for (i, &b) in frame.data.iter().enumerate() {
            let j = start + i;
            s.data[j] = b;
            if !s.received[j] {
                s.received_bytes += 1;
            }
            s.received[j] = true;
        }
        while s.missing < s.received.len() && s.received[s.missing] {
            s.missing += 1;
        }
        Ok(s.received_bytes)
    }
    pub fn finish(&self) -> Result<Vec<u8>> {
        let s = self.state.lock().unwrap();
        if s.received_bytes != s.size {
            return Err(invalid("Audio file is incomplete"));
        }
        Ok(s.data.clone())
    }
    pub fn missing_offset(&self) -> Option<u32> {
        let s = self.state.lock().unwrap();
        (s.missing < s.size as usize).then_some(s.missing as u32)
    }
}

impl DeviceSession {
    fn receive_with_buffer(
        &self,
        data: Vec<u8>,
        generation: u64,
        now_ms: u64,
        buffered: bool,
    ) -> Result<Vec<Delivery>> {
        let mut s = self.state.lock().unwrap();
        if !s.connected || s.generation != generation {
            return Ok(Vec::new());
        }
        let mut out = expire(&mut s, now_ms);
        let batch = self.decoder.feed_batch(data);
        s.dropped_crc_packets = s
            .dropped_crc_packets
            .saturating_add(u64::from(batch.dropped_crc_packets));
        let mut fault = batch
            .fault
            .map(|message| (crate::ErrorCode::InvalidData, message));
        for packet in batch.packets {
            if packet.command == 0x1ff {
                if packet.body.len() != 4 {
                    fault = Some((
                        crate::ErrorCode::InvalidData,
                        "Invalid standard error response".into(),
                    ));
                    break;
                }
                let command = u16::from_be_bytes([packet.body[0], packet.body[1]]);
                let code = u16::from_be_bytes([packet.body[2], packet.body[3]]);
                if code == 0 {
                    fault = Some((
                        crate::ErrorCode::InvalidData,
                        "Invalid standard error response".into(),
                    ));
                    break;
                }
                if let Some(id) = s
                    .pending
                    .values()
                    .find(|p| p.command == command)
                    .map(|p| p.id)
                {
                    s.pending.remove(&id);
                    out.push(failure(id, "device", "Device rejected request", Some(code)));
                    continue;
                }
            }
            if let Some(id) = s
                .pending
                .values()
                .find(|p| p.response == packet.command)
                .map(|p| p.id)
            {
                s.pending.remove(&id);
                out.push(Delivery::Response {
                    request_id: id,
                    packet,
                });
            } else {
                if buffered && s.events.len() >= self.event_capacity as usize {
                    fault = Some((crate::ErrorCode::Overflow, "Receive queue overflow".into()));
                    break;
                }
                if buffered {
                    s.events.push_back(packet.clone());
                }
                out.push(Delivery::Event { packet });
            }
        }
        if let Some((code, message)) = fault {
            // All removed requests have a delivery, even when a later frame fails.
            for id in s.pending.keys() {
                out.push(Delivery::Failure {
                    request_id: *id,
                    kind: "protocol".into(),
                    code,
                    recovery: crate::Recovery::Reconnect,
                    message: message.clone(),
                    device_code: None,
                });
            }
            s.pending.clear();
            s.connected = false;
            s.events.clear();
            s.exclusive = None;
            self.decoder.clear();
            out.push(Delivery::Fault {
                code,
                message,
                recovery: crate::Recovery::Reconnect,
            });
        }
        Ok(out)
    }
}
