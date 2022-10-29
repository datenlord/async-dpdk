//! An udp example

use std::net::IpAddr;

use async_dpdk::udp::UdpSocket;
use async_dpdk::{eal, net_dev};

const MSG: &'static str = "Calling server from client!";

async fn client() {
    let socket = UdpSocket::bind("10.2.3.1:0").unwrap();
    let data = MSG.as_bytes();
    let sz = socket.send_to(data, "10.2.3.0:1234").await.unwrap();
    assert_eq!(sz, MSG.len());
}

async fn server() {
    let socket = UdpSocket::bind("10.2.3.0:1234").unwrap();
    let mut data = vec![0; 40];
    let (sz, addr) = socket.recv_from(&mut data[..]).await.unwrap();
    assert_eq!(sz, MSG.len());
    assert_eq!(addr.ip(), IpAddr::from([10, 2, 3, 1]));
    assert_eq!(&data[..sz], MSG.as_bytes());
}

#[tokio::main]
async fn main() {
    eal::Config::new()
        .device_probe(&["10.2.3.0", "10.2.3.1"])
        .enter()
        .unwrap();
    net_dev::device_start().unwrap();
    let srv = tokio::task::spawn(server());
    client().await;
    srv.await.unwrap();
    net_dev::device_stop().unwrap();
}
