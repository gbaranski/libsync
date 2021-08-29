use crate::delta::Delta;
use crate::delta::Deltas;
use crate::frame::AtomicSequenceNumber;
use crate::frame::ClientFrame;
use crate::frame::DeserializeError;
use crate::frame::Frame;
use crate::frame::SequenceNumber;
use crate::frame::SerializeError;
use crate::frame::ServerFrame;
use crate::frame::WriteAckFrame;
use crate::frame::WriteFrame;
use std::collections::BTreeMap;
use std::net::Ipv4Addr;
use std::net::SocketAddr;
use std::net::SocketAddrV4;
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

#[derive(Debug)]
pub struct Session {
    id: crate::session::ID,
    socket: UdpSocket,
    deltas: Mutex<Deltas>,
    /// Last acknowledged sequence number
    last_acknowledged_seqn: AtomicSequenceNumber,
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
            socket,
            id: crate::session::ID::new_v4(),
            deltas: Mutex::new(Deltas::default()),
            last_acknowledged_seqn: AtomicSequenceNumber::new(0),
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
            let frame = ServerFrame::deserialize(&buf[..n])?;
            tracing::debug!("Received: {:?}", frame);
            if let Some(frame) = self.on_frame(frame).await? {
                self.send(frame).await?;
            }
        }
    }

    async fn on_write(&self, frame: WriteFrame) -> Result<WriteAckFrame, Error> {
        let mut deltas = self.deltas.lock().await;
        tracing::debug!("Current state: {:?}", deltas);
        deltas.insert_many(frame.new_deltas);
        tracing::debug!("State after write: {:?}", deltas);
        let new_checksum = deltas.checksum();
        if new_checksum != frame.state_checksum {
            todo!(
                "checksum don't match. Expected: {}, Received: {}",
                new_checksum,
                frame.state_checksum
            );
        }
        Ok(WriteAckFrame {
            seqn: deltas.latest_seqn(),
        })
    }

    async fn on_write_ack(&self, frame: WriteAckFrame) -> Result<(), Error> {
        if self
            .last_acknowledged_seqn
            .load(std::sync::atomic::Ordering::Relaxed)
            < frame.seqn
        {}
        self.last_acknowledged_seqn
            .store(frame.seqn, std::sync::atomic::Ordering::Relaxed);

        Ok(())
    }

    async fn on_frame(&self, frame: ServerFrame) -> Result<Option<Frame>, Error> {
        Ok(match frame.inner {
            Frame::Write(write) => Some(Frame::WriteAck(self.on_write(write).await?)),
            Frame::WriteAck(write_ack) => {
                self.on_write_ack(write_ack).await?;
                None
            }
        })
    }

    async fn send(&self, frame: Frame) -> Result<(), Error> {
        let client_frame = ClientFrame {
            session_id: self.id,
            inner: frame,
        };
        self.socket
            .send(client_frame.serialize()?.as_bytes())
            .await?;
        tracing::debug!("Sent: {:?}", client_frame);
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

        self.send(Frame::Write(WriteFrame {
            new_deltas: unacknowledged_deltas,
            state_checksum: deltas.checksum(),
        }))
        .await?;
        Ok(())
    }
}
