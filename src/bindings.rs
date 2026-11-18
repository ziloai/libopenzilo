// SPDX-License-Identifier: MPL-2.0
use crate::{invalid, CoreError, Delivery, DeviceEngine, DeviceInfo, Result};
use std::{
    collections::{btree_map::Entry, BTreeMap},
    sync::{Arc, Mutex},
};

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum RingIdentity {
    CpuId { value: String },
    Serial { value: String },
}

impl RingIdentity {
    fn from_info(info: &DeviceInfo) -> Result<Self> {
        let cpu = info.cpuid.trim();
        let serial = info.sn.trim();
        if !cpu.is_empty() {
            Ok(Self::CpuId { value: cpu.into() })
        } else if !serial.is_empty() {
            Ok(Self::Serial {
                value: serial.into(),
            })
        } else {
            Err(invalid("Ring did not report a CPU ID or serial number"))
        }
    }

    fn matches(&self, info: &DeviceInfo) -> bool {
        match self {
            Self::CpuId { value } => value == info.cpuid.trim(),
            Self::Serial { value } => value == info.sn.trim(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct RingBinding {
    pub id: String,
    pub identity: RingIdentity,
}

#[derive(Clone, uniffi::Record)]
pub struct BoundRingSession {
    pub id: String,
    pub attachment: u64,
    pub generation: u64,
    pub engine: Arc<DeviceEngine>,
}

struct BoundRing {
    binding: RingBinding,
    session: Option<BoundRingSession>,
}

#[derive(Default)]
struct Registry {
    rings: BTreeMap<String, BoundRing>,
    attachment: u64,
}

#[derive(uniffi::Object)]
pub struct RingBindings {
    registry: Mutex<Registry>,
    event_capacity: u32,
}

#[uniffi::export]
impl RingBindings {
    #[uniffi::constructor]
    pub fn new(event_capacity: u32) -> Result<Self> {
        DeviceEngine::new(event_capacity)?;
        Ok(Self {
            registry: Mutex::new(Registry::default()),
            event_capacity,
        })
    }

    pub fn bind(&self, id: String, info: DeviceInfo) -> Result<RingBinding> {
        let binding = RingBinding {
            id,
            identity: RingIdentity::from_info(&info)?,
        };
        self.insert(binding.clone(), Some(&info))?;
        Ok(binding)
    }

    pub fn restore(&self, binding: RingBinding) -> Result<()> {
        self.insert(binding, None)
    }

    pub fn list(&self) -> Vec<RingBinding> {
        self.registry
            .lock()
            .unwrap()
            .rings
            .values()
            .map(|ring| ring.binding.clone())
            .collect()
    }

    /// Attach only after reading system information from this transport. Use the
    /// returned engine for this link; this registry owns its open/close lifecycle.
    pub fn attach(&self, id: String, info: DeviceInfo) -> Result<BoundRingSession> {
        let mut registry = self.registry.lock().unwrap();
        let ring = registry
            .rings
            .get(&id)
            .ok_or_else(|| invalid("Unknown ring binding"))?;
        if !ring.binding.identity.matches(&info) {
            return Err(CoreError::Incompatible {
                detail: "Ring does not match its binding".into(),
            });
        }
        if ring.session.is_some()
            || registry.rings.values().any(|candidate| {
                candidate.session.is_some() && candidate.binding.identity.matches(&info)
            })
        {
            return Err(CoreError::Busy {
                detail: "Ring already has an attached transport".into(),
            });
        }
        let attachment = registry
            .attachment
            .checked_add(1)
            .ok_or_else(|| invalid("Attachment counter exhausted"))?;
        let engine = Arc::new(DeviceEngine::new(self.event_capacity)?);
        let generation = engine.open()?;
        engine.seed_device_info(info);
        let session = BoundRingSession {
            id: id.clone(),
            attachment,
            generation,
            engine,
        };
        registry.attachment = attachment;
        registry.rings.get_mut(&id).unwrap().session = Some(session.clone());
        Ok(session)
    }

    pub fn session(&self, id: String) -> Option<BoundRingSession> {
        self.registry
            .lock()
            .unwrap()
            .rings
            .get(&id)?
            .session
            .clone()
    }

    pub fn detach(&self, id: String, attachment: u64) -> Vec<Delivery> {
        let mut registry = self.registry.lock().unwrap();
        let Some(ring) = registry.rings.get_mut(&id) else {
            return Vec::new();
        };
        if ring
            .session
            .as_ref()
            .is_none_or(|session| session.attachment != attachment)
        {
            return Vec::new();
        }
        let session = ring.session.take().unwrap();
        session.engine.disconnect(session.generation)
    }

    pub fn unbind(&self, id: String) -> Result<()> {
        let mut registry = self.registry.lock().unwrap();
        if let Entry::Occupied(ring) = registry.rings.entry(id) {
            if ring.get().session.is_some() {
                return Err(CoreError::Busy {
                    detail: "Detach the ring before unbinding".into(),
                });
            }
            ring.remove();
        }
        Ok(())
    }
}

impl RingBindings {
    fn insert(&self, binding: RingBinding, info: Option<&DeviceInfo>) -> Result<()> {
        if binding.id.trim().is_empty() {
            return Err(invalid("Ring binding ID cannot be empty"));
        }
        let (RingIdentity::CpuId { value } | RingIdentity::Serial { value }) = &binding.identity;
        if value.is_empty() || value.trim() != value {
            return Err(invalid(
                "Ring binding identity must be nonempty and trimmed",
            ));
        }
        let mut registry = self.registry.lock().unwrap();
        if let Some(existing) = registry.rings.get(&binding.id) {
            if existing.binding == binding {
                return Ok(());
            }
            return Err(invalid("Ring binding ID is already assigned"));
        }
        if registry.rings.values().any(|ring| {
            ring.binding.identity == binding.identity
                || info.is_some_and(|info| ring.binding.identity.matches(info))
        }) {
            return Err(invalid("Ring identity is already bound"));
        }
        registry.rings.insert(
            binding.id.clone(),
            BoundRing {
                binding,
                session: None,
            },
        );
        Ok(())
    }
}
