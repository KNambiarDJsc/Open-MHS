//! A simulated Modbus TCP device serving a profile's register map.
//!
//! Written from the Modbus specification, independently of the client library, so the
//! two check each other. It answers function codes 1–6, 15 and 16, and misbehaves on
//! demand: refusals are exception 03, garbled answers carry the wrong function code, and
//! a dead link drops connections.

use std::collections::BTreeMap;
use std::sync::Arc;

use openmhs_profile::{Outcome, VirtualDevice, codec};
use openmhs_types::Value;
use openmhs_types::descriptor::CapabilityKind;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::{Profile, Register, Table};

const ILLEGAL_FUNCTION: u8 = 0x01;
const ILLEGAL_ADDRESS: u8 = 0x02;
const ILLEGAL_VALUE: u8 = 0x03;
/// A user-defined function code, used to garble answers.
const GARBLED_FUNCTION: u8 = 0x41;

pub struct ModbusSim {
    profile: Profile,
    device: Arc<VirtualDevice>,
}

/// What to send back for one request PDU.
enum Answer {
    Pdu(Vec<u8>),
    Silent,
}

impl ModbusSim {
    pub fn new(profile: Profile) -> Arc<Self> {
        let device = VirtualDevice::new(profile.descriptor.clone(), &profile.simulator.initial);
        Arc::new(Self { profile, device })
    }

    /// The device behind the protocol: its state, and the conformance fixture.
    pub fn device(&self) -> Arc<VirtualDevice> {
        self.device.clone()
    }

    /// Serve on `listener` until the task is dropped.
    pub async fn serve(self: Arc<Self>, listener: TcpListener) {
        while let Ok((stream, _)) = listener.accept().await {
            if self.device.link_down() {
                continue;
            }
            let sim = self.clone();
            tokio::spawn(async move { sim.connection(stream).await });
        }
    }

    async fn connection(&self, mut stream: TcpStream) {
        loop {
            let mut header = [0u8; 7];
            if stream.read_exact(&mut header).await.is_err() {
                return;
            }
            let len = usize::from(u16::from_be_bytes([header[4], header[5]]));
            if !(2..=254).contains(&len) {
                return;
            }
            let mut pdu = vec![0u8; len - 1];
            if stream.read_exact(&mut pdu).await.is_err() || self.device.link_down() {
                return;
            }
            let reply = match self.answer(&pdu) {
                Answer::Silent => continue,
                Answer::Pdu(p) => p,
            };
            let mut frame = Vec::with_capacity(7 + reply.len());
            frame.extend_from_slice(&header[..4]);
            frame.extend_from_slice(&u16::try_from(reply.len() + 1).unwrap_or(0).to_be_bytes());
            frame.push(header[6]);
            frame.extend_from_slice(&reply);
            if stream.write_all(&frame).await.is_err() {
                return;
            }
        }
    }

    /// The register covering `address` in `table`, if one starts there or spans it.
    fn find(&self, table: Table, address: u16) -> Option<(&str, &Register)> {
        self.profile.mapping.iter().find_map(|(name, reg)| {
            let end = u32::from(reg.address) + u32::from(reg.width());
            (reg.table == table
                && u32::from(address) >= u32::from(reg.address)
                && u32::from(address) < end)
                .then_some((name.as_str(), reg))
        })
    }

    fn answer(&self, pdu: &[u8]) -> Answer {
        let Some((&fc, body)) = pdu.split_first() else {
            return Answer::Silent;
        };
        let word = |i: usize| body.get(i..i + 2).map(|b| u16::from_be_bytes([b[0], b[1]]));
        let exception = |code: u8| Answer::Pdu(vec![fc | 0x80, code]);
        match fc {
            0x01..=0x04 => {
                let (Some(start), Some(count)) = (word(0), word(2)) else {
                    return exception(ILLEGAL_VALUE);
                };
                let table = match fc {
                    0x01 => Table::Coil,
                    0x02 => Table::DiscreteInput,
                    0x03 => Table::Holding,
                    _ => Table::Input,
                };
                match self.read_block(table, start, count) {
                    Ok(data) => Answer::Pdu([vec![fc], data].concat()),
                    Err(code) => exception(code),
                }
            }
            0x05 | 0x06 => {
                let (Some(address), Some(raw)) = (word(0), word(2)) else {
                    return exception(ILLEGAL_VALUE);
                };
                let words = if fc == 0x05 {
                    match raw {
                        0xFF00 => vec![1],
                        0x0000 => vec![0],
                        _ => return exception(ILLEGAL_VALUE),
                    }
                } else {
                    vec![raw]
                };
                let table = if fc == 0x05 {
                    Table::Coil
                } else {
                    Table::Holding
                };
                self.write(table, address, &words, || pdu.to_vec())
            }
            0x10 => {
                let (Some(address), Some(count)) = (word(0), word(2)) else {
                    return exception(ILLEGAL_VALUE);
                };
                let words: Option<Vec<u16>> =
                    (0..usize::from(count)).map(|i| word(5 + 2 * i)).collect();
                let Some(words) = words else {
                    return exception(ILLEGAL_VALUE);
                };
                self.write(Table::Holding, address, &words, || pdu[..5].to_vec())
            }
            _ => exception(ILLEGAL_FUNCTION),
        }
    }

    fn read_block(&self, table: Table, start: u16, count: u16) -> Result<Vec<u8>, u8> {
        if count == 0 || count > 125 {
            return Err(ILLEGAL_VALUE);
        }
        let mut words = Vec::new();
        let mut a = u32::from(start);
        let end = a + u32::from(count);
        while a < end {
            let address = u16::try_from(a).map_err(|_| ILLEGAL_ADDRESS)?;
            let (name, reg) = self.find(table, address).ok_or(ILLEGAL_ADDRESS)?;
            if reg.address != address {
                return Err(ILLEGAL_ADDRESS); // reading half of a 32-bit value
            }
            let cap = self
                .profile
                .descriptor
                .capability(name)
                .ok_or(ILLEGAL_ADDRESS)?;
            let value = self.device.read(name).ok_or(ILLEGAL_ADDRESS)?;
            let spec = cap.value.as_ref().ok_or(ILLEGAL_ADDRESS)?;
            let x = codec::to_number(spec, &value).map_err(|_| ILLEGAL_VALUE)?;
            if table.is_bit() {
                words.push(u16::from(x != 0.0));
            } else {
                words.extend(reg.encode(x).map_err(|_| ILLEGAL_VALUE)?);
            }
            a += u32::from(reg.width());
        }
        if a != end {
            return Err(ILLEGAL_ADDRESS);
        }
        Ok(if table.is_bit() {
            let mut bytes = vec![0u8; words.len().div_ceil(8)];
            for (i, w) in words.iter().enumerate() {
                if *w != 0 {
                    bytes[i / 8] |= 1 << (i % 8);
                }
            }
            [vec![u8::try_from(bytes.len()).unwrap_or(0)], bytes].concat()
        } else {
            let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_be_bytes()).collect();
            [vec![u8::try_from(bytes.len()).unwrap_or(0)], bytes].concat()
        })
    }

    /// One write: exactly one mapped value, starting at its first register.
    fn write(
        &self,
        table: Table,
        address: u16,
        words: &[u16],
        echo: impl FnOnce() -> Vec<u8>,
    ) -> Answer {
        let fc = match (table, words.len()) {
            (Table::Coil, _) => 0x05,
            (_, 1) => 0x06,
            _ => 0x10,
        };
        let exception = |code: u8| Answer::Pdu(vec![fc | 0x80, code]);
        let Some((name, reg)) = self.find(table, address) else {
            return exception(ILLEGAL_ADDRESS);
        };
        if reg.address != address || usize::from(reg.width()) != words.len() {
            return exception(ILLEGAL_ADDRESS);
        }
        let Some(cap) = self.profile.descriptor.capability(name) else {
            return exception(ILLEGAL_ADDRESS);
        };
        let x = if table.is_bit() {
            f64::from(words[0])
        } else {
            match reg.decode(words) {
                Ok(x) => x,
                Err(_) => return exception(ILLEGAL_VALUE),
            }
        };
        let outcome = if cap.kind == CapabilityKind::Action {
            let mut params = BTreeMap::new();
            if let (Some(p), Some(spec)) = (&reg.param, reg.spec(cap)) {
                match codec::from_number(spec, x) {
                    Ok(v) => {
                        params.insert(p.clone(), v);
                    }
                    Err(_) => return exception(ILLEGAL_VALUE),
                }
            }
            self.device.command(name, None, &params)
        } else {
            let value: Option<Value> = reg
                .spec(cap)
                .and_then(|spec| codec::from_number(spec, x).ok());
            match value {
                Some(v) => self.device.command(name, Some(v), &BTreeMap::new()),
                None => return exception(ILLEGAL_VALUE),
            }
        };
        match outcome {
            Outcome::Applied => Answer::Pdu(echo()),
            Outcome::Refused(_) => exception(ILLEGAL_VALUE),
            Outcome::Silent => Answer::Silent,
            Outcome::Garbled => Answer::Pdu(vec![GARBLED_FUNCTION, 0xDE, 0xAD]),
        }
    }
}
