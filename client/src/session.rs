use libsync::{ClientFrame, DeserializeError, SerializeError, ServerFrame};
use std::{
    net::{Ipv4Addr, SocketAddr, SocketAddrV4},
    sync::Arc,
};
use tokio::{net::UdpSocket, sync::Mutex};

#[derive(Debug, Clone)]
pub struct Session {
    socket: Arc<UdpSocket>,
    state: Arc<Mutex<Vec<u8>>>,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io: {0}")]
    IO(#[from] std::io::Error),
    #[error("serialize: {0}")]
    Serialize(#[from] SerializeError),
    #[error("deserialize: {0}")]
    Deserialize(#[from] DeserializeError),
}

async fn send(socket: &UdpSocket, frame: ClientFrame) -> Result<(), Error> {
    tracing::debug!("Sending frame: {:?}", frame);
    let string = frame.serialize()?;
    socket.send(string.as_bytes()).await?;
    Ok(())
}

async fn recv(socket: &UdpSocket, buf: &mut [u8]) -> Result<ServerFrame, Error> {
    let n = socket.recv(buf).await?;
    let frame: ServerFrame = ServerFrame::deserialize(&buf[..n])?;
    tracing::debug!("Received frame: {:?}", frame);
    Ok(frame)
}

impl Session {
    #[tracing::instrument]
    pub async fn new(address: SocketAddr) -> Result<Self, Error> {
        let socket = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0)).await?;
        socket.connect(address).await?;

        Ok(Self {
            socket: Arc::new(socket),
            state: Arc::new(Mutex::new(Vec::new())),
        })
    }

    pub async fn run(&self) -> Result<(), Error> {
        const BUFFER_SIZE: u16 = 65535;
        let mut buf = vec![0; BUFFER_SIZE as usize];
        loop {
            let frame = recv(&self.socket, &mut buf).await?;
            match frame {
                ServerFrame::Write { bytes, checksum } => {
                    let mut state = self.state.lock().await;
                    tracing::debug!("State: {:?}", state);
                    state.extend(bytes);
                    tracing::debug!("State after write: {:?}", state);
                    if libsync::checksum(&state) != checksum {
                        todo!(
                            "checksum don't match. Expected: {}, Received: {}",
                            libsync::checksum(&state),
                            checksum
                        );
                    }
                }
            };
        }
    }

    async fn send(&self, frame: ClientFrame) -> Result<(), Error> {
        send(&self.socket, frame).await
    }

    #[tracing::instrument(skip(self))]
    pub async fn write(&self, bytes: &[u8]) -> Result<(), Error> {
        let mut state = self.state.lock().await;
        state.extend(bytes);
        let checksum = libsync::checksum(&state);
        self.send(ClientFrame::Write {
            bytes: bytes.to_owned(),
            checksum,
        })
        .await?;
        Ok(())
    }
}
