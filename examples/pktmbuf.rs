//! An example using Mempool and Mbuf.

use async_dpdk::{
    eal,
    mbuf::Mbuf,
    mempool::{Mempool, PktMempool},
};

fn main() {
    // Enter DPDK EAL (Environment Abstract Layer).
    eal::Config::new().enter().unwrap();
    // Create a `Mempool` with capacity of 512 `Mbuf`s, and cache size of 16 `Mbuf`s.
    let mp = PktMempool::create("pktmbuf", 512).unwrap();
    // Allocate a `Mbuf` from the `Mempool`.
    let mut mbuf = Mbuf::new(&mp).unwrap();
    // Append 10 bytes to `Mbuf`.
    let data = mbuf.append(10).unwrap();
    // Write to `Mbuf`.
    data.copy_from_slice("HelloWorld".as_bytes());
}
