// SPDX-License-Identifier: MPL-2.0
use crate::{encode_packet, invalid, CoreError, Packet, PacketDecoder, Result};
use std::{
    collections::{BTreeMap, VecDeque},
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
        message: String,
        device_code: Option<u16>,
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
        message: message.into(),
        device_code: code,
    }
}
#[uniffi::export]
impl DeviceSession {
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
        if s.exclusive.is_some() && s.exclusive != owner {
            return Err(CoreError::Busy {
                detail: "An exclusive operation owns this connection".into(),
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
        let mut s = self.state.lock().unwrap();
        if !s.connected || s.generation != generation {
            return Ok(Vec::new());
        }
        let mut out = expire(&mut s, now_ms);
        let packets = match self.decoder.feed(data) {
            Ok(p) => p,
            Err(e) => {
                self.decoder.clear();
                return Err(e);
            }
        };
        for packet in packets {
            if packet.command == 0x1ff {
                if packet.body.len() != 4 {
                    return Err(invalid("Invalid standard error response"));
                }
                let command = u16::from_be_bytes([packet.body[0], packet.body[1]]);
                let code = u16::from_be_bytes([packet.body[2], packet.body[3]]);
                if code == 0 {
                    return Err(invalid("Invalid standard error response"));
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
                if s.events.len() >= self.event_capacity as usize {
                    return Err(CoreError::Overflow);
                }
                s.events.push_back(packet.clone());
                out.push(Delivery::Event { packet });
            }
        }
        Ok(out)
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
        if s.exclusive.is_some() && s.exclusive != owner {
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
        s.pending
            .remove(&request_id)
            .map(|_| failure(request_id, "cancelled", "Operation cancelled", None))
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
            s.pending.remove(&id);
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
    complete: bool,
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
                complete: false,
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
            s.received[j] = true;
        }
        if frame.is_end {
            s.complete = true;
        }
        Ok(s.received.iter().filter(|v| **v).count() as u32)
    }
    pub fn finish(&self) -> Result<Vec<u8>> {
        let s = self.state.lock().unwrap();
        if s.received.iter().any(|v| !*v) {
            return Err(invalid("Audio file is incomplete"));
        }
        Ok(s.data.clone())
    }
    pub fn missing_offset(&self) -> Option<u32> {
        self.state
            .lock()
            .unwrap()
            .received
            .iter()
            .position(|v| !*v)
            .map(|i| i as u32)
    }
}
