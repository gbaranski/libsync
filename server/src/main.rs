use libsync::{AtomicSequenceNumber, DeserializeError, Frame, SequenceNumber, SerializeError};
use std::{
    collections::BTreeMap,
    net::{IpAddr, SocketAddr},
    str::FromStr,
    sync::Arc,
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

                // TODO: Verify checksum
                // let new_checksum = libsync::checksum(&*deltas);
                // if new_checksum != checksum {
                //     todo!(
                //         "checksum don't match. Expected: {}, Received: {}",
                //         new_checksum,
                //         checksum
                //     );
                // }
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

    async fn write(&self, address: &SocketAddr, bytes: &[u8]) -> Result<(), Error> {
        let last_acknowledged_seqn = self
            .last_acknowledged_seqn
            .load(std::sync::atomic::Ordering::Relaxed);
        let mut deltas = self.deltas.lock().await;
        let seqn = match deltas.keys().rev().next() {
            Some(seqn) => seqn + 1,
            None => 0,
        };
        deltas.insert(seqn, bytes.to_vec());
        let checksum = libsync::checksum(resolve_deltas(&deltas));
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
    let run_server_task = {
        let server = server.clone();
        tokio::spawn(async move { server.run().await.expect("error when running a server") })
    };
    let send_new_deltas_task = {
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(512)).await;
                // if let Some(address) = &*server.address.read().await {
                //     server
                //         .write(address, &[])
                //         .await
                //         .expect("error when sending new deltas");
                // }
            }
        })
    };

    tokio::select! {
        value = run_server_task => { value? },
        value = send_new_deltas_task => { value? },
    }
    Ok(())
}
