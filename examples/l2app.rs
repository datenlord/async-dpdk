//! A simple example

use async_dpdk::eal::{self, Eal};
use async_dpdk::eth_dev::{EthDev, EthRxQueue, EthTxQueue};
use async_dpdk::ether::EthPacket;
use dpdk_sys::RTE_ETHER_TYPE_IPV4;
use std::sync::Arc;

const SERVER_PORT: u16 = 0;
const CLIENT_PORT: u16 = 1;

async fn server(ctx: Arc<Eal>) {
    eprintln!("server running");
    let dev = EthDev::new(&ctx, SERVER_PORT, 1, 1).unwrap();
    let rx = EthRxQueue::init(&dev, 0).unwrap();
    let _tx = EthTxQueue::init(&dev, 0).unwrap();

    dev.start().unwrap();

    let _msg = rx.recv::<EthPacket>().await.unwrap();
    eprintln!("server pkt recv!");
    dev.stop().await.unwrap();
}

async fn client(ctx: Arc<Eal>) {
    let dev = EthDev::new(&ctx, CLIENT_PORT, 1, 1).unwrap();
    let _rx = EthRxQueue::init(&dev, 0).unwrap();
    let tx = EthTxQueue::init(&dev, 0).unwrap();
    dev.start().unwrap();

    // Eth packet initialization
    let src = EthDev::mac_addr(CLIENT_PORT).unwrap();
    let dst = EthDev::mac_addr(SERVER_PORT).unwrap();
    let msg = EthPacket::new(src, dst, RTE_ETHER_TYPE_IPV4 as u16);

    tx.send(msg).await.unwrap();
    eprintln!("client pkt sent!");
    dev.stop().await.unwrap();
}

#[tokio::main]
async fn main() {
    let eal = eal::Builder::new().build().unwrap();
    let srv = tokio::task::spawn(server(eal.clone()));
    client(eal).await;
    let _ = srv.await;
}
