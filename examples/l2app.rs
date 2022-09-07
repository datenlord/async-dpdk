//! A simple example

use async_dpdk::eal;
use async_dpdk::eth_dev::{EthDev, EthRxQueue, EthTxQueue};
use async_dpdk::mbuf::Mbuf;

// const SERVER_PORT: u16 = 0;
// const CLIENT_PORT: u16 = 1;

#[allow(dead_code)]
async fn server() {
    let mut dev = EthDev::new(0, 1, 1).unwrap();
    println!("server running");
    let mut rx = EthRxQueue::init(&mut dev, 0).unwrap();
    let _tx = EthTxQueue::init(&mut dev, 0).unwrap();
    dev.enable_promiscuous().unwrap();
    dev.start().unwrap();

    let _msg = rx.recv0().await.unwrap();
    println!("server pkt recv!");
    dev.stop().await.unwrap();
}

#[allow(dead_code)]
async fn client() {
    let mut dev = EthDev::new(0, 1, 1).unwrap();
    // println!("client mac {:?}", dev.mac_addr());
    let mp_send = Mbuf::create_mp("client_send", 10, 0, dev.socket_id() as _).unwrap();
    let mut rx = EthRxQueue::init(&mut dev, 0).unwrap();
    let mut tx = EthTxQueue::init(&mut dev, 0).unwrap();
    dev.start().unwrap();

    // Eth packet initialization
    let mut msg = Mbuf::new(&mp_send).unwrap();
    // let _ether_hdr = msg.eth_init().unwrap();
    // ether_hdr.ether_type = (RTE_ETHER_TYPE_IPV4 as u16).to_be();
    // ether_hdr.src_addr = *EthDev::mac_addr(CLIENT_PORT).unwrap();
    // ether_hdr.dst_addr = *EthDev::mac_addr(SERVER_PORT).unwrap();

    let data = msg.append(5).unwrap();
    data.copy_from_slice(&"Hello".as_bytes());
    tx.send0(msg).await.unwrap();
    println!("client pkt sent!");
    let ack = rx.recv0().await.unwrap();
    let data = ack.data_slice();
    assert_eq!(data, "Hello ack".as_bytes());
    dev.stop().await.unwrap();
}

#[tokio::main]
async fn main() {
    let _eal = eal::Builder::new().build().unwrap();
    tokio::task::spawn(server());
    client().await;
}
