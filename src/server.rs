use crate::delta::Delta;
use crate::delta::Deltas;
use crate::frame::AtomicSequenceNumber;
use crate::frame::ClientFrame;
use crate::frame::DeserializeError;
use crate::frame::Frame;
use crate::frame::SerializeError;
use crate::frame::ServerFrame;
use crate::frame::WriteAckFrame;
use crate::frame::WriteFrame;
use dashmap::DashMap;
use std::net::SocketAddr;
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

pub struct Server {
    socket: UdpSocket,
    sessions: DashMap<crate::session::ID, Session>,
}

pub struct Session {
    pub deltas: Mutex<Deltas>,
    pub write_notify: Notify,
    remote_address: RwLock<SocketAddr>,
    last_acknowledged_seqn: AtomicSequenceNumber,
}

impl Session {
    pub fn new(remote_address: SocketAddr) -> Self {
        Self {
            deltas: Mutex::new(Deltas::new()),
            write_notify: Notify::new(),
            remote_address: RwLock::new(remote_address),
            last_acknowledged_seqn: AtomicSequenceNumber::new(0),
        }
    }

    pub async fn on_write(&self, frame: WriteFrame) -> Result<Option<ServerFrame>, Error> {
        let mut deltas = self.deltas.lock().await;
        tracing::debug!("State: {:?}", deltas);
        deltas.insert_many(frame.new_deltas);
        tracing::debug!("State after update: {:?}", deltas);
        let new_checksum = deltas.checksum();
        if new_checksum != frame.state_checksum {
            todo!(
                "checksum don't match. Expected: {}, Received: {}",
                new_checksum,
                frame.state_checksum
            );
        }
        self.write_notify.notify_one();
        Ok(Some(ServerFrame {
            inner: Frame::WriteAck(WriteAckFrame {
                seqn: deltas.latest_seqn(),
            }),
        }))
    }

    pub async fn on_write_ack(&self, frame: WriteAckFrame) -> Result<Option<ServerFrame>, Error> {
        if self
            .last_acknowledged_seqn
            .load(std::sync::atomic::Ordering::Relaxed)
            < frame.seqn
        {}
        self.last_acknowledged_seqn
            .store(frame.seqn, std::sync::atomic::Ordering::Relaxed);
        Ok(None)
    }

    pub async fn write(&self, delta: Delta) -> Result<WriteFrame, Error> {
        let last_acknowledged_seqn = self
            .last_acknowledged_seqn
            .load(std::sync::atomic::Ordering::Relaxed);
        let mut deltas = self.deltas.lock().await;
        let seqn = deltas.latest_seqn();
        deltas.insert(seqn, delta);
        tracing::debug!("State: {:?}", deltas);
        let unacknowledged_deltas = deltas.since(&last_acknowledged_seqn);

        Ok(WriteFrame {
            new_deltas: unacknowledged_deltas,
            state_checksum: deltas.checksum(),
        })
    }
}

impl Server {
    pub async fn new(address: SocketAddr) -> Result<Self, Error> {
        let socket = UdpSocket::bind(address).await?;
        Ok(Self::with_socket(socket))
    }

    pub fn with_socket(socket: UdpSocket) -> Self {
        Self {
            socket,
            sessions: DashMap::new(),
        }
    }

    pub async fn run(&self) -> Result<(), Error> {
        let mut buf = vec![0; 1024];
        loop {
            let (n, from) = self.socket.recv_from(&mut buf).await?;
            if n == 0 {
                return Ok(());
            }
            let client_frame = ClientFrame::deserialize(&buf[..n])?;
            let session = match self.sessions.get(&client_frame.session_id) {
                Some(session) => session,
                None => {
                    let session = Session::new(from);
                    self.sessions.insert(client_frame.session_id, session);
                    self.sessions.get(&client_frame.session_id).unwrap()
                }
            };
            if *session.remote_address.read().await != from {
                *session.remote_address.write().await = from;
            }
            tracing::debug!("Received: {:?}", client_frame);
            let server_frame = match client_frame.inner {
                Frame::Write(write) => session.on_write(write).await,
                Frame::WriteAck(write_ack) => session.on_write_ack(write_ack).await,
            }?;
            if let Some(server_frame) = server_frame {
                self.socket
                    .send_to(server_frame.serialize()?.as_bytes(), from)
                    .await?;
                tracing::debug!("Sent: {:?}", server_frame);
            }
        }
    }

    async fn send(&self, address: &SocketAddr, frame: ServerFrame) -> Result<(), Error> {
        self.socket
            .send_to(frame.serialize()?.as_bytes(), address)
            .await?;
        tracing::debug!("Sent {:?}", frame);
        Ok(())
    }

    pub async fn write(&self, delta: Delta, session: &Session) -> Result<(), Error> {
        let frame = session.write(delta).await?;
        let remote_address = session.remote_address.read().await;
        self.send(
            &remote_address,
            ServerFrame {
                inner: Frame::Write(frame),
            },
        )
        .await?;
        Ok(())
    }
}
