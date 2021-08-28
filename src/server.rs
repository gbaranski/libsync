use crate::AtomicSequenceNumber;
use crate::DeserializeError;
use crate::Frame;
use crate::SequenceNumber;
use crate::SerializeError;
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::Mutex;
use tokio::sync::RwLock;

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
    deltas: Arc<Mutex<BTreeMap<SequenceNumber, Vec<u8>>>>,
    /// Last acknowledged sequence number
    last_acknowledged_seqn: Arc<AtomicSequenceNumber>,
}

pub fn resolve_deltas(deltas: &BTreeMap<SequenceNumber, Vec<u8>>) -> Vec<u8> {
    deltas.values().flatten().cloned().collect()
}

impl Server {
    pub async fn new(address: SocketAddr) -> Result<Self, Error> {
        let socket = UdpSocket::bind(address).await?;
        Ok(Self {
            address: Arc::new(RwLock::new(None)),
            socket: Arc::new(socket),
            deltas: Arc::new(Mutex::new(BTreeMap::new())),
            last_acknowledged_seqn: Arc::new(AtomicSequenceNumber::new(0)),
        })
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

    #[tracing::instrument(skip(self, frame))]
    async fn handle_frame(&self, frame: Frame, from: SocketAddr) -> Result<Option<Frame>, Error> {
        Ok(match frame {
            Frame::Write {
                bytes,
                checksum,
                seqn,
            } => {
                let mut deltas = self.deltas.lock().await;
                tracing::debug!("Current state: {:?}", resolve_deltas(&deltas));
                let latest_seqn = deltas.keys().rev().next();
                match latest_seqn {
                    Some(latest_seqn) if *latest_seqn >= seqn => {
                        tracing::debug!(
                            "Received seqn: {} is lower than latest seqn: {}. Skipping update",
                            seqn,
                            latest_seqn
                        );
                    }
                    _ => {
                        deltas.insert(seqn, bytes);
                        tracing::debug!("State after update: {:?}", resolve_deltas(&deltas));
                    }
                };

                let new_checksum = crate::checksum(&*deltas);
                if new_checksum != checksum {
                    todo!(
                        "checksum don't match. Expected: {}, Received: {}",
                        new_checksum,
                        checksum
                    );
                }
                Some(Frame::WriteAck { seqn })
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

    pub async fn write(&self, address: &SocketAddr, bytes: &[u8]) -> Result<(), Error> {
        let last_acknowledged_seqn = self
            .last_acknowledged_seqn
            .load(std::sync::atomic::Ordering::Relaxed);
        let mut deltas = self.deltas.lock().await;
        let seqn = match deltas.keys().rev().next() {
            Some(seqn) => seqn + 1,
            None => 0,
        };
        deltas.insert(seqn, bytes.to_vec());
        let checksum = crate::checksum(&*deltas);
        let unacknowledged_deltas = deltas
            .range(last_acknowledged_seqn..)
            .map(|(_, delta)| delta)
            .flatten()
            .cloned()
            .collect::<Vec<_>>();
        self.send(
            &address,
            Frame::Write {
                bytes: unacknowledged_deltas,
                checksum,
                seqn,
            },
        )
        .await?;
        Ok(())
    }
}
