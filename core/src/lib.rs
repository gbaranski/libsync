pub mod session;

pub const PORT: u16 = 7070;

use serde::{Deserialize, Serialize};

#[derive(Debug, PartialEq, Eq, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ServerFrame {
    Write { bytes: Vec<u8> },
}

#[derive(Debug, PartialEq, Eq, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ClientFrame {
    Write { bytes: Vec<u8> },
}
