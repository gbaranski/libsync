use crate::AtomicSequenceNumber;
use crate::DeserializeError;
use crate::Frame;
use crate::SequenceNumber;
use crate::SerializeError;
use std::collections::BTreeMap;
use std::net::Ipv4Addr;
use std::net::SocketAddr;
use std::net::SocketAddrV4;
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::Mutex;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io: {0}")]
    IO(#[from] std::io::Error),
    #[error("serialize: {0}")]
    Serialize(#[from] SerializeError),
    #[error("deserialize: {0}")]
    Deserialize(#[from] DeserializeError),
}

#[derive(Debug, Clone)]
pub struct Session {
    socket: Arc<UdpSocket>,
    seqn: Arc<AtomicSequenceNumber>,
    // TODO: Consider changing to Vec instead of BTreeMap
    deltas: Arc<Mutex<BTreeMap<SequenceNumber, Vec<u8>>>>,
    /// Last acknowledged sequence number
    last_acknowledged_seqn: Arc<AtomicSequenceNumber>,
}

pub fn resolve_deltas(deltas: &BTreeMap<SequenceNumber, Vec<u8>>) -> Vec<u8> {
    deltas.values().flatten().cloned().collect()
}

impl Session {
    #[tracing::instrument]
    pub async fn new(address: SocketAddr) -> Result<Self, Error> {
        let socket = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0)).await?;
        socket.connect(address).await?;

        Ok(Self {
            socket: Arc::new(socket),
            seqn: Arc::new(AtomicSequenceNumber::new(0)),
            deltas: Arc::new(Mutex::new(BTreeMap::new())),
            last_acknowledged_seqn: Arc::new(AtomicSequenceNumber::new(0)),
        })
    }

    pub async fn run(&self) -> Result<(), Error> {
        self.read_packets().await
    }

    async fn read_packets(&self) -> Result<(), Error> {
        let mut buf = vec![0; 1024];
        loop {
            let n = self.socket.recv(&mut buf).await?;
            if n == 0 {
                return Ok(());
            }
            let frame = Frame::deserialize(&buf[..n])?;
            tracing::debug!("Received: {:?}", frame);
            if let Some(frame) = self.handle_frame(frame).await? {
                tracing::debug!("Sent: {:?}", frame);
                self.socket.send(frame.serialize()?.as_bytes()).await?;
            }
        }
    }

    async fn handle_frame(&self, frame: Frame) -> Result<Option<Frame>, Error> {
        Ok(match frame {
            Frame::Write {
                bytes,
                checksum,
                seqn,
            } => {
                self.seqn.store(seqn, std::sync::atomic::Ordering::Relaxed);
                let mut deltas = self.deltas.lock().await;
                tracing::debug!("Current state: {:?}", resolve_deltas(&deltas));
                deltas.insert(seqn, bytes);
                tracing::debug!("State after write: {:?}", resolve_deltas(&deltas));
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

    async fn send(&self, frame: Frame) -> Result<(), Error> {
        self.socket.send(frame.serialize()?.as_bytes()).await?;
        tracing::debug!("Sent {:?}", frame);
        Ok(())
    }

    #[tracing::instrument(skip(self))]
    pub async fn write(&self, bytes: &[u8]) -> Result<(), Error> {
        let last_acknowledged_seqn = self
            .last_acknowledged_seqn
            .load(std::sync::atomic::Ordering::Relaxed);
        let seqn = self.seqn.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        let mut deltas = self.deltas.lock().await;
        deltas.insert(seqn, bytes.to_vec());
        let checksum = crate::checksum(&*deltas);
        tracing::debug!("Current state: {:?}", resolve_deltas(&deltas));
        let total_bytes = deltas
            .range((last_acknowledged_seqn + 1)..)
            .map(|(_, delta)| delta)
            .flatten()
            .cloned()
            .collect();

        self.send(Frame::Write {
            bytes: total_bytes,
            checksum,
            seqn,
        })
        .await?;
        Ok(())
    }
}
