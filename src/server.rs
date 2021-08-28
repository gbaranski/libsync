use crate::delta;
use crate::delta::Delta;
use crate::delta::Deltas;
use crate::frame::AtomicSequenceNumber;
use crate::frame::DeserializeError;
use crate::frame::Frame;
use crate::frame::SequenceNumber;
use crate::frame::SerializeError;
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::Mutex;
use tokio::sync::Notify;
use tokio::sync::RwLock;

pub const DEFAULT_PORT: u16 = 7070;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("session: {0}")]
    IO(#[from] std::io::Error),
    #[error("serialize: {0}")]
    Serialize(#[from] SerializeError),
    #[error("serialize: {0}")]
    Deserialize(#[from] DeserializeError),
}

#[derive(Clone)]
pub struct Server {
    socket: Arc<UdpSocket>,
    address: Arc<RwLock<Option<SocketAddr>>>,
    // TODO: Consider changing to Vec instead of BTreeMap
    pub deltas: Arc<Mutex<Deltas>>,
    pub write_notify: Arc<Notify>,
    /// Last acknowledged sequence number
    last_acknowledged_seqn: Arc<AtomicSequenceNumber>,
}

pub fn resolve_deltas(deltas: &BTreeMap<SequenceNumber, Vec<u8>>) -> Vec<u8> {
    deltas.values().flatten().cloned().collect()
}

impl Server {
    pub async fn new(address: SocketAddr) -> Result<Self, Error> {
        let socket = UdpSocket::bind(address).await?;
        Ok(Self::with_socket(socket))
    }

    pub fn with_socket(socket: UdpSocket) -> Self {
        Self {
            address: Arc::new(RwLock::new(None)),
            socket: Arc::new(socket),
            deltas: Arc::new(Mutex::new(Deltas::new())),
            last_acknowledged_seqn: Arc::new(AtomicSequenceNumber::new(0)),
            write_notify: Arc::new(Notify::new()),
        }
    }

    pub async fn run(&self) -> Result<(), Error> {
        let mut buf = vec![0; 1024];
        loop {
            let (n, from) = self.socket.recv_from(&mut buf).await?;
            if n == 0 {
                return Ok(());
            }
            if *self.address.read().await != Some(from) {
                *self.address.write().await = Some(from);
            }
            let frame = Frame::deserialize(&buf[..n])?;
            tracing::debug!("Received: {:?}", frame);
            if let Some(frame) = self.handle_frame(frame, from).await? {
                tracing::debug!("Sent: {:?}", frame);
                self.socket
                    .send_to(frame.serialize()?.as_bytes(), from)
                    .await?;
            }
        }
    }

    #[tracing::instrument(skip(self))]
    async fn handle_frame(&self, frame: Frame, from: SocketAddr) -> Result<Option<Frame>, Error> {
        Ok(match frame {
            Frame::Write {
                delta,
                checksum,
                seqn,
            } => {
                let mut deltas = self.deltas.lock().await;
                tracing::debug!("State: {:?}", deltas);
                let latest_seqn = deltas.latest_seqn();
                let response = if deltas.latest_seqn() > seqn {
                    tracing::debug!(
                        "Received seqn: {} is lower than latest seqn: {}. Skipping update",
                        seqn,
                        latest_seqn
                    );
                    None
                } else {
                    deltas.insert(seqn, delta);
                    tracing::debug!("State after update: {:?}", deltas);
                    let new_checksum = deltas.checksum();
                    if new_checksum != checksum {
                        todo!(
                            "checksum don't match. Expected: {}, Received: {}",
                            new_checksum,
                            checksum
                        );
                    }
                    Some(Frame::WriteAck { seqn })
                };
                self.write_notify.notify_one();
                response
            }
            Frame::WriteAck { seqn } => {
                self.last_acknowledged_seqn
                    .store(seqn, std::sync::atomic::Ordering::Relaxed);
                None
            }
        })
    }

    async fn send(&self, address: &SocketAddr, frame: Frame) -> Result<(), Error> {
        self.socket
            .send_to(frame.serialize()?.as_bytes(), address)
            .await?;
        tracing::debug!("Sent {:?}", frame);
        Ok(())
    }

    pub async fn write(&self, address: &SocketAddr, delta: Delta) -> Result<(), Error> {
        let last_acknowledged_seqn = self
            .last_acknowledged_seqn
            .load(std::sync::atomic::Ordering::Relaxed);
        let mut deltas = self.deltas.lock().await;
        let seqn = deltas.latest_seqn();
        deltas.insert(seqn, delta);
        tracing::debug!("State: {:?}", deltas);
        let unacknowledged_deltas = deltas.since(&last_acknowledged_seqn);
        let unacknowledged_delta = delta::merge(unacknowledged_deltas.iter());

        self.send(
            address,
            Frame::Write {
                delta: unacknowledged_delta,
                checksum: deltas.checksum(),
                seqn,
            },
        )
        .await?;
        Ok(())
    }
}
