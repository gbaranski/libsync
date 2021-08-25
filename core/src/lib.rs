pub mod session;

pub const PORT: u16 = 7070;

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

use serde::{Deserialize, Serialize};

#[derive(Debug, PartialEq, Eq, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ServerFrame {
    Write { bytes: Vec<u8>, checksum: u32 },
}

#[derive(Debug, PartialEq, Eq, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ClientFrame {
    Write { bytes: Vec<u8>, checksum: u32 },
}

macro_rules! frame_impl {
    ($item:ident) => {
        impl $item {
            pub fn serialize(&self) -> Result<String, SerializeError> {
                serde_json::to_string(&self).map_err(SerializeError::Json)
            }

            pub fn deserialize(bytes: &[u8]) -> Result<Self, DeserializeError> {
                serde_json::from_slice(bytes).map_err(DeserializeError::Json)
            }
        }
    };
}

frame_impl!(ServerFrame);
frame_impl!(ClientFrame);

pub fn checksum(bytes: &[u8]) -> u32 {
    let mut hasher = crc32fast::Hasher::new();
    hasher.update(bytes);
    hasher.finalize()
}
