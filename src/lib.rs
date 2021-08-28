pub mod client;
pub mod server;
pub mod session;

pub const PORT: u16 = 7070;
pub type SequenceNumber = u32;
pub type AtomicSequenceNumber = std::sync::atomic::AtomicU32;

#[derive(Debug, thiserror::Error)]
pub enum SerializeError {
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, thiserror::Error)]
pub enum DeserializeError {
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
}

use serde::Deserialize;
use serde::Serialize;

#[derive(Debug, PartialEq, Eq, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Frame {
    Write {
        bytes: Vec<u8>,
        checksum: u32,
        seqn: SequenceNumber,
    },
    WriteAck {
        seqn: SequenceNumber,
    },
}

impl Frame {
    pub fn serialize(&self) -> Result<String, SerializeError> {
        serde_json::to_string(&self).map_err(SerializeError::Json)
    }

    pub fn deserialize(bytes: &[u8]) -> Result<Self, DeserializeError> {
        serde_json::from_slice(bytes).map_err(DeserializeError::Json)
    }
}

pub fn checksum(value: impl std::hash::Hash) -> u32 {
    let mut hasher = crc32fast::Hasher::new();
    value.hash(&mut hasher);
    hasher.finalize()
}
