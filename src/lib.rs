pub mod client;
pub mod server;
pub mod session;
pub mod delta;
pub mod frame;

pub const PORT: u16 = 7070;
pub type SequenceNumber = u32;
pub type AtomicSequenceNumber = std::sync::atomic::AtomicU32;
