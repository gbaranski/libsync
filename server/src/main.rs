use libsync::{ClientFrame, DeserializeError, SerializeError, ServerFrame};
use std::{
    net::{IpAddr, SocketAddr},
    str::FromStr,
    sync::Arc,
    time::Duration,
};
use tokio::{
    net::UdpSocket,
    sync::{Mutex, RwLock},
};

#[derive(Debug, thiserror::Error)]
enum Error {
    #[error("session: {0}")]
    IO(#[from] std::io::Error),
    #[error("serialize: {0}")]
    Serialize(#[from] SerializeError),
    #[error("serialize: {0}")]
    Deserialize(#[from] DeserializeError),
}

#[derive(Clone)]
struct Server {
    socket: Arc<UdpSocket>,
    address: Arc<RwLock<Option<SocketAddr>>>,
    state: Arc<Mutex<Vec<u8>>>,
    prev_state: Arc<Mutex<Vec<u8>>>,
}

impl Server {
    pub async fn new(address: SocketAddr) -> Result<Self, Error> {
        let socket = UdpSocket::bind(address).await?;
        Ok(Self {
            address: Arc::new(RwLock::new(None)),
            state: Arc::new(Mutex::new(Vec::with_capacity(1024))),
            prev_state: Arc::new(Mutex::new(Vec::with_capacity(1024))),
            socket: Arc::new(socket),
        })
    }

    pub async fn run(self) -> Result<(), Error> {
        let send_state_task = {
            let this = self.clone();
            tokio::spawn(async move {
                this.send_state().await.unwrap();
            })
        };

        let read_packets_task = {
            let this = self.clone();
            tokio::spawn(async move {
                this.read_packets().await.unwrap();
            })
        };

        tokio::select! {
            value = send_state_task => value.unwrap(),
            value = read_packets_task => value.unwrap(),
        };

        Ok(())
    }

    async fn read_packets(&self) -> Result<(), Error> {
        let mut buf = vec![0; 1024];
        loop {
            let (n, from) = self.socket.recv_from(&mut buf).await?;
            if n == 0 {
                return Ok(());
            }
            if *self.address.read().await != Some(from) {
                *self.address.write().await = Some(from);
            }
            let bytes = &buf[..n];
            let frame = ClientFrame::deserialize(bytes)?;
            tracing::debug!("Received: {:?}", frame);
            if let Some(frame) = self.handle_message(frame, from).await? {
                tracing::debug!("Sent: {:?}", frame);
                self.socket
                    .send_to(frame.serialize()?.as_bytes(), from)
                    .await?;
            }
        }
    }

    #[tracing::instrument(skip(self, frame))]
    async fn handle_message(
        &self,
        frame: ClientFrame,
        from: SocketAddr,
    ) -> Result<Option<ServerFrame>, Error> {
        let frame = match frame {
            ClientFrame::Write { bytes, checksum } => {
                let mut state = self.state.lock().await;
                let mut prev_state = self.prev_state.lock().await;
                state.extend(&bytes);
                prev_state.extend(&bytes);
                if libsync::checksum(&state) != checksum {
                    todo!(
                        "checksum don't match. Expected: {}, Received: {}",
                        libsync::checksum(&state),
                        checksum
                    );
                }
                None
            }
        };
        Ok(frame)
    }

    async fn send_state(&self) -> Result<(), Error> {
        loop {
            if let Some(address) = *self.address.read().await {
                let state = self.state.lock().await;
                let mut prev_state = self.prev_state.lock().await;
                let stripped_state = state.strip_prefix(prev_state.as_slice()).unwrap_or(&[]);
                *prev_state = state.to_vec();
                let frame = ServerFrame::Write {
                    bytes: stripped_state.to_vec(),
                    checksum: libsync::checksum(&state),
                };
                self.socket
                    .send_to(frame.serialize()?.as_bytes(), address)
                    .await?;
                tracing::debug!("Sent: {:?}", frame);
            }
            tokio::time::sleep(Duration::from_millis(64)).await;
        }
    }
}

fn init_logging() {
    const LOG_ENV: &str = "RUST_LOG";
    use tracing::Level;
    use tracing_subscriber::EnvFilter;

    let filter = std::env::var(LOG_ENV)
        .map(|env| {
            EnvFilter::from_str(env.to_uppercase().as_str())
                .unwrap_or_else(|err| panic!("invalid `{}` environment variable {}", LOG_ENV, err))
        })
        .unwrap_or(EnvFilter::default().add_directive(Level::INFO.into()));

    tracing_subscriber::fmt().with_env_filter(filter).init();
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    init_logging();

    let matches = clap::App::new(clap::crate_name!())
        .bin_name(clap::crate_name!())
        .version(clap::crate_version!())
        .author(clap::crate_authors!())
        .about("Synchronization protocol server")
        .arg(
            clap::Arg::with_name("address")
                .help("Target address")
                .default_value("127.0.0.1")
                .takes_value(true),
        )
        .get_matches();
    let address = matches.value_of("address").unwrap();
    let (ip, port) = match address.split_once(':') {
        Some((host, port)) => (host, port.parse().expect("invalid port")),
        None => (address, libsync::PORT),
    };
    let ip = IpAddr::from_str(ip).expect("invalid ip address");

    let server = Server::new(SocketAddr::new(ip, port)).await?;
    tracing::info!("Starting server");
    server.run().await.unwrap();
    Ok(())
}
