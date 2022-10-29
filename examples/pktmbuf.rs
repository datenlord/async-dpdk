//! An example using Mempool and Mbuf.

use async_dpdk::{eal, lcore, mbuf::Mbuf};

fn main() {
    eal::Config::new().enter().unwrap();
    let mp = Mbuf::create_mp("pktmbuf", 512, 16, lcore::socket_id() as _).unwrap();
    let mut mbuf = Mbuf::new(&mp).unwrap();
    let data = mbuf.append(10).unwrap();
    data.copy_from_slice("HelloWorld".as_bytes());
}
