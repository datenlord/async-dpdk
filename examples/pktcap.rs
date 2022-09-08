//! This is a simple application listening to the traffic and print stats periodically.

use async_dpdk::{
    eal,
    eth_dev::{EthDev, EthRxQueue, EthTxQueue},
};
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::task;

#[tokio::main]
async fn main() {
    let eal = eal::Builder::new().build().unwrap();
    let mut dev = EthDev::new(&eal, 0, 1, 1).unwrap();
    let rx = EthRxQueue::init(&mut dev, 0).unwrap();
    let _tx = EthTxQueue::init(&mut dev, 0).unwrap();
    dev.start().unwrap();
    dev.enable_promiscuous().unwrap();

    let rx_count = Arc::new(AtomicUsize::new(0));
    let count = rx_count.clone();

    task::spawn(async move {
        loop {
            let _pkt = rx.recv_m().await.unwrap();
            count.fetch_add(1, Ordering::Relaxed);
        }
    });

    for _ in 0..20 {
        println!("Packets received: {:?}", rx_count.load(Ordering::Relaxed));
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    dev.stop().await.unwrap();
}
