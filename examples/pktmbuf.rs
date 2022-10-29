//! An example using Mempool and Mbuf.

use async_dpdk::{eal, lcore, mbuf::Mbuf};

fn main() {
    // Enter DPDK EAL (Environment Abstract Layer).
    eal::Config::new().enter().unwrap();
    // Create a `Mempool` with capacity of 512 `Mbuf`s, and cache size of 16 `Mbuf`s.
    let mp = Mbuf::create_mp("pktmbuf", 512, 16, lcore::socket_id() as _).unwrap();
    // Allocate a `Mbuf` from the `Mempool`.
    let mut mbuf = Mbuf::new(&mp).unwrap();
    // Append 10 bytes to `Mbuf`.
    let data = mbuf.append(10).unwrap();
    // Write to `Mbuf`.
    data.copy_from_slice("HelloWorld".as_bytes());
}
