use crate::delta::Delta;

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
        new_deltas: Vec<(SequenceNumber, Delta)>,
        state_checksum: u32,
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
