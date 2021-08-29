use libsync::client;
use libsync::server;
use std::net::Ipv4Addr;
use std::net::SocketAddrV4;
use std::sync::Arc;
use tokio::net::UdpSocket;

pub async fn setup() -> (Arc<server::Server>, Arc<client::Session>) {
    let server_socket = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("error when binding server UDP Socket");
    let server_address = server_socket.local_addr().unwrap();
    let server = server::Server::with_socket(server_socket);
    let session = client::Session::new(server_address)
        .await
        .expect("error when creating Client Session");
    (Arc::new(server), Arc::new(session))
}
