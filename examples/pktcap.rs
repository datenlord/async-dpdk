//! Packet capture app.
use async_dpdk::{
    eal,
    eth_dev::{EthDev, EthRxQueue, EthTxQueue},
    mbuf::Mbuf,
};

#[tokio::main]
async fn main() {
    let _eal = eal::Builder::new().build().unwrap();
    let mut dev = EthDev::new(0, 1, 1).unwrap();
    let mp = Mbuf::create_mp("mbuf_pool", 8192, 256, dev.socket_id() as _).unwrap();
    let mut rx = EthRxQueue::init(&mut dev, 0, mp).unwrap();
    let _tx = EthTxQueue::init(&mut dev, 0).unwrap();
    dev.start().unwrap();
    dev.enable_promiscuous().unwrap();

    let _pkt = rx.recv().await.unwrap();
    println!("receive packet!");

    dev.stop().await.unwrap();
}
