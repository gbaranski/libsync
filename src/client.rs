use crate::delta;
use crate::delta::Delta;
use crate::delta::Deltas;
use crate::frame::AtomicSequenceNumber;
use crate::frame::DeserializeError;
use crate::frame::Frame;
use crate::frame::SequenceNumber;
use crate::frame::SerializeError;
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
    // TODO: Consider changing to Vec instead of BTreeMap
    deltas: Arc<Mutex<Deltas>>,
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
            deltas: Arc::new(Mutex::new(Deltas::default())),
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
                delta,
                checksum,
                seqn,
            } => {
                let mut deltas = self.deltas.lock().await;
                tracing::debug!("Current state: {:?}", deltas);
                deltas.insert(seqn, delta);
                tracing::debug!("State after write: {:?}", deltas);
                let new_checksum = deltas.checksum();
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

    #[tracing::instrument(skip(self))]
    async fn send(&self, frame: Frame) -> Result<(), Error> {
        self.socket.send(frame.serialize()?.as_bytes()).await?;
        tracing::debug!("Sent frame");
        Ok(())
    }

    #[tracing::instrument(skip(self))]
    pub async fn write(&self, delta: Delta) -> Result<(), Error> {
        let last_acknowledged_seqn = self
            .last_acknowledged_seqn
            .load(std::sync::atomic::Ordering::Relaxed);
        let mut deltas = self.deltas.lock().await;
        let seqn = deltas.latest_seqn() + 1;
        deltas.insert(seqn, delta);
        tracing::debug!("State: {:?}", deltas);
        let unacknowledged_deltas = deltas.since(&last_acknowledged_seqn);
        tracing::debug!("Unacknowledged deltas: {:?}", unacknowledged_deltas);
        let unacknowledged_delta = delta::merge(unacknowledged_deltas.iter());

        self.send(Frame::Write {
            delta: unacknowledged_delta,
            checksum: deltas.checksum(),
            seqn,
        })
        .await?;
        Ok(())
    }
}
