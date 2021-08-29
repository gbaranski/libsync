use libsync::client::Session;
use libsync::delta::Delta;
use std::net::ToSocketAddrs;
use std::sync::Arc;

fn init_logging() {
    const LOG_ENV: &str = "RUST_LOG";
    use std::str::FromStr;
    use tracing::Level;

    let level = std::env::var(LOG_ENV)
        .map(|env| {
            Level::from_str(env.to_uppercase().as_str())
                .unwrap_or_else(|err| panic!("invalid `{}` environment variable {}", LOG_ENV, err))
        })
        .unwrap_or(Level::INFO);

    tracing_subscriber::fmt()
        .with_writer(|| {
            let log_file_path = xdg::BaseDirectories::with_prefix("libsync")
                .unwrap()
                .get_cache_home()
                .join("libsync.log");
            if !log_file_path.exists() {
                std::fs::create_dir_all(&log_file_path.parent().unwrap()).unwrap();
            }
            let log_file = std::fs::OpenOptions::new()
                .read(true)
                .append(true)
                .create(true)
                .open(log_file_path)
                .unwrap();
            log_file
        })
        .with_max_level(level)
        .init();
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    init_logging();
    let matches = clap::App::new(clap::crate_name!())
        .bin_name(clap::crate_name!())
        .version(clap::crate_version!())
        .author(clap::crate_authors!())
        .about("Synchronization protocol client")
        .arg(
            clap::Arg::with_name("address")
                .help("Target address")
                .default_value("127.0.0.1")
                .takes_value(true),
        )
        .get_matches();
    let address = matches.value_of("address").unwrap();
    let address = match address.split_once(':') {
        Some((host, port)) => (host, port.parse().expect("invalid port")),
        None => (address, libsync::server::DEFAULT_PORT),
    }
    .to_socket_addrs()
    .expect("invalid address")
    .next()
    .unwrap();

    let session = Arc::new(Session::new(address).await?);
    let run_session_task = {
        let session = session.clone();
        tokio::spawn(async move { session.run().await.unwrap() })
    };
    let read_input_task = {
        let session = session.clone();
        tokio::spawn(async move { read_input(&session).await.unwrap() })
    };
    tokio::select! {
        value = run_session_task => value.unwrap(),
        value = read_input_task => value.unwrap(),
    };
    Ok(())
}

fn get_input<'a>(term: &console::Term, buf: &'a mut [u8]) -> &'a [u8] {
    use console::Key;

    let key = term.read_key().unwrap();
    match key {
        Key::Char(char) => {
            let bytes = char.encode_utf8(buf);
            bytes.as_bytes()
        }
        Key::UnknownEscSeq(_) => &buf[0..1],
        Key::Unknown => unimplemented!(),
        key => {
            let byte = match key {
                Key::Enter => 0x0A,
                Key::Escape => todo!(),
                Key::Backspace => 0x08,
                Key::Home => todo!(),
                Key::End => todo!(),
                Key::Tab => 0x09,
                Key::BackTab => todo!(),
                Key::Del => todo!(),
                Key::Insert => todo!(),
                Key::PageUp => todo!(),
                Key::PageDown => todo!(),
                _ => unreachable!(),
            };
            buf[0] = byte;
            &buf[0..1]
        }
    }
}

async fn read_input(session: &Session) -> Result<(), Box<dyn std::error::Error>> {
    let mut buf = vec![0; 8];
    let term = console::Term::stdout();
    loop {
        let input = get_input(&term, &mut buf);
        session.write(Delta::Append(input.to_vec())).await?;
    }
}
