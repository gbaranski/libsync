use libsync::server::Server;
use std::net::IpAddr;
use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::Arc;

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

    let server = Arc::new(Server::new(SocketAddr::new(ip, port)).await?);
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
