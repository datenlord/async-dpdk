//! An udp example

use std::net::IpAddr;
use std::time::Duration;

use async_dpdk::udp::UdpSocket;
use async_dpdk::{eal, net_dev};

const MSG: &str = "Calling server from client!";

async fn client() {
    // Bind the client socket to the second NIC.
    let socket = UdpSocket::bind("10.2.3.1:0").unwrap();
    let data = MSG.as_bytes();
    tokio::time::sleep(Duration::from_millis(10)).await;
    // Send message to server.
    let sz = socket.send_to(data, "10.2.3.0:1234").await.unwrap();
    assert_eq!(sz, MSG.len());
}

async fn server() {
    // Bind the server socket to the first NIC.
    let socket = UdpSocket::bind("10.2.3.0:1234").unwrap();
    let mut data = vec![0; 40];
    // Receive from client.
    let (sz, addr) = socket.recv_from(&mut data[..]).await.unwrap();
    assert_eq!(sz, MSG.len());
    assert_eq!(addr.ip(), IpAddr::from([10, 2, 3, 1]));
    assert_eq!(&data[..sz], MSG.as_bytes());
}

#[tokio::main]
async fn main() {
    // Enter DPDK EAL.
    eal::Config::new()
        // Assign IP addresses for two of the NICs.
        .device_probe(&["10.2.3.0", "10.2.3.1"])
        .unwrap()
        .enter()
        .unwrap();
    // Let the devices start polling.
    net_dev::device_start().unwrap();
    let srv = tokio::task::spawn(server());
    client().await;
    srv.await.unwrap();
    // Stop the polling threads.
    net_dev::device_stop().unwrap();
}
