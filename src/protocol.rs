// SPDX-License-Identifier: MPL-2.0
use crate::{invalid, Result};
use std::sync::Mutex;
pub const MAX_BODY: usize = 5120;
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct Packet {
    pub command: u16,
    pub body: Vec<u8>,
    pub version: u16,
    pub body_crc: u16,
}
#[uniffi::export]
pub fn crc16(data: Vec<u8>, initial: u16) -> u16 {
    crc(&data, initial)
}
pub(crate) fn crc(data: &[u8], mut c: u16) -> u16 {
    for b in data {
        c = c.rotate_left(8) ^ *b as u16;
        c ^= (c & 255) >> 4;
        c ^= c.wrapping_shl(12);
        c ^= (c & 255).wrapping_shl(5);
    }
    c
}
#[uniffi::export]
pub fn encode_packet(command: u16, body: Vec<u8>) -> Result<Vec<u8>> {
    if body.len() > MAX_BODY {
        return Err(invalid(format!("Body too large: {} bytes", body.len())));
    }
    let sum = if body.is_empty() {
        0
    } else {
        crc(&body, 65535)
    };
    let mut out = vec![0x3f, 0, 4];
    out.extend(command.to_be_bytes());
    out.extend((body.len() as u32).to_be_bytes());
    out.extend(sum.to_be_bytes());
    out.extend(body);
    Ok(out)
}
#[uniffi::export]
pub fn decode_packet(data: Vec<u8>) -> Result<Packet> {
    decode(&data)
}
fn decode(d: &[u8]) -> Result<Packet> {
    let packet = parse_frame(d)?;
    if !valid_body_crc(&packet) {
        return Err(invalid(format!(
            "Body CRC mismatch: expected 0x{:04X}, got 0x{:04X}",
            packet.body_crc,
            crc(&packet.body, 65535)
        )));
    }
    Ok(packet)
}
fn valid_body_crc(packet: &Packet) -> bool {
    packet.body.is_empty() || packet.body_crc == crc(&packet.body, 65535)
}
fn parse_frame(d: &[u8]) -> Result<Packet> {
    if d.len() < 11 {
        return Err(invalid(format!("Packet too short: {} bytes", d.len())));
    }
    if d[0] != 0x3f {
        return Err(invalid(format!("Invalid packet magic: 0x{:02X}", d[0])));
    }
    let version = u16::from_be_bytes([d[1], d[2]]);
    if version != 4 {
        return Err(invalid(format!("Unsupported protocol version: {version}")));
    }
    let n = u32::from_be_bytes(d[5..9].try_into().unwrap()) as usize;
    if n > MAX_BODY {
        return Err(invalid(format!("Body too large: {n} bytes")));
    }
    if d.len() < 11 + n {
        return Err(invalid(format!(
            "Incomplete packet: need {}, got {}",
            11 + n,
            d.len()
        )));
    }
    let sum = u16::from_be_bytes([d[9], d[10]]);
    let body = d[11..11 + n].to_vec();
    Ok(Packet {
        command: u16::from_be_bytes([d[3], d[4]]),
        body,
        version,
        body_crc: sum,
    })
}
#[derive(Clone, Debug, uniffi::Record)]
pub struct DecodeBatch {
    pub packets: Vec<Packet>,
    pub fault: Option<String>,
    pub dropped_crc_packets: u32,
}
#[derive(uniffi::Object, Default)]
pub struct PacketDecoder {
    buffer: Mutex<Vec<u8>>,
}
#[uniffi::export]
impl PacketDecoder {
    #[uniffi::constructor]
    pub fn new() -> Self {
        Self::default()
    }
    pub fn clear(&self) {
        self.buffer.lock().unwrap().clear();
    }
    pub fn buffer_len(&self) -> u64 {
        self.buffer.lock().unwrap().len() as u64
    }
    pub fn feed(&self, data: Vec<u8>) -> Result<Vec<Packet>> {
        let batch = self.feed_batch(data);
        match batch.fault {
            Some(detail) => Err(invalid(detail)),
            None => Ok(batch.packets),
        }
    }
    /// Drop complete CRC-invalid frames without discarding their valid neighbors.
    pub fn feed_batch(&self, data: Vec<u8>) -> DecodeBatch {
        let mut b = self.buffer.lock().unwrap();
        let mut packets = Vec::new();
        let mut dropped_crc_packets = 0u32;
        // Bounded chunks avoid allocating arbitrarily large incomplete frames.
        for part in data.chunks(MAX_BODY + 11).chain(std::iter::once(&[][..])) {
            b.extend(part);
            loop {
                match b.iter().position(|x| *x == 0x3f) {
                    Some(i) => {
                        b.drain(..i);
                    }
                    None => {
                        b.clear();
                        break;
                    }
                }
                if b.len() < 11 {
                    break;
                }
                let n = u32::from_be_bytes(b[5..9].try_into().unwrap()) as usize;
                if n > MAX_BODY {
                    b.clear();
                    return DecodeBatch {
                        packets,
                        fault: Some(format!("Body too large: {n} bytes")),
                        dropped_crc_packets,
                    };
                }
                if b.len() < 11 + n {
                    break;
                }
                let raw: Vec<u8> = b.drain(..11 + n).collect();
                match parse_frame(&raw) {
                    Ok(p) if valid_body_crc(&p) => packets.push(p),
                    Ok(_) => dropped_crc_packets = dropped_crc_packets.saturating_add(1),
                    Err(e) => {
                        return DecodeBatch {
                            packets,
                            fault: Some(e.to_string()),
                            dropped_crc_packets,
                        };
                    }
                }
            }
        }
        DecodeBatch {
            packets,
            fault: None,
            dropped_crc_packets,
        }
    }
}
pub(crate) struct Reader<'a> {
    pub d: &'a [u8],
    pub i: usize,
}
impl<'a> Reader<'a> {
    pub fn new(d: &'a [u8]) -> Self {
        Self { d, i: 0 }
    }
    pub fn remaining(&self) -> usize {
        self.d.len() - self.i
    }
    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        if n > self.remaining() {
            return Err(invalid(format!(
                "Need {n} bytes, only {} left",
                self.remaining()
            )));
        }
        let r = &self.d[self.i..self.i + n];
        self.i += n;
        Ok(r)
    }
    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.bytes(1)?[0])
    }
    pub fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_be_bytes(self.bytes(2)?.try_into().unwrap()))
    }
    pub fn i16(&mut self) -> Result<i16> {
        Ok(i16::from_be_bytes(self.bytes(2)?.try_into().unwrap()))
    }
    pub fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_be_bytes(self.bytes(4)?.try_into().unwrap()))
    }
    pub fn string(&mut self) -> Result<String> {
        let n = self.u16()? as usize;
        String::from_utf8(self.bytes(n)?.to_vec()).map_err(|_| invalid("Invalid UTF-8 string"))
    }
    pub fn success(&mut self) -> Result<()> {
        let code = self.u16()?;
        if code != 0 {
            return Err(crate::CoreError::Device { code });
        }
        Ok(())
    }
    pub fn end(&self) -> Result<()> {
        if self.remaining() != 0 {
            return Err(invalid(format!(
                "Unexpected trailing bytes: {}",
                self.remaining()
            )));
        }
        Ok(())
    }
}
