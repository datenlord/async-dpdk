#![cfg(test)]
#![feature(new_uninit)]

use async_dpdk::{
    eal::{self, IovaMode},
    lcore,
};
// use std::sync::Once;

// static INIT: Once = Once::new();
// static EAL: Option<Eal> = None;


mod mbuf {
    use super::*;
    use async_dpdk::mbuf::Mbuf;

    #[test]
    fn test() {
        let _eal = eal::Builder::new().iova_mode(IovaMode::VA).build().unwrap();

        // Create a packet mempool.
        let mp = Mbuf::create_mp(10, 0, lcore::socket_id() as _).unwrap();
        let mut mbuf = Mbuf::new(&mp).unwrap();
        assert!(mbuf.is_contiguous());
        assert_eq!(mbuf.data_len(), 0);

        // Read and write from mbuf.
        let data = mbuf.append(10).unwrap();
        data.copy_from_slice(&[0, 1, 2, 3, 4, 5, 6, 7, 8, 9]);
        assert_eq!(mbuf.data_len(), 10);
        assert_eq!(mbuf.data_slice(), &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9]);
        mbuf.trim(5).unwrap();
        assert_eq!(mbuf.data_len(), 5);
        assert_eq!(mbuf.data_slice(), &[0, 1, 2, 3, 4]);

        // Mbuf chaining.
        let mut mbuf1 = Mbuf::new(&mp).unwrap();
        mbuf1.append(5).unwrap();
        mbuf.chain(mbuf1).unwrap();
        assert_eq!(mbuf.num_segs(), 2);
        assert_eq!(mbuf.pkt_len(), 10);

        // Indirect mbuf.
        let mbuf2 = mbuf.pktmbuf_clone(&mp).unwrap();
        assert_eq!(mbuf2.data_slice(), &[0, 1, 2, 3, 4]);
    }
}

mod mempool {
    use super::*;
    use async_dpdk::mempool::{self, Mempool};

    #[test]
    fn test() {
        let _eal = eal::Builder::new().iova_mode(IovaMode::VA).build().unwrap();

        let mp = Mempool::create(
            "mempool",
            64,
            16,
            0,
            0,
            lcore::socket_id() as _,
            mempool::MEMPOOL_SINGLE_CONSUMER | mempool::MEMPOOL_SINGLE_PRODUCER,
        )
        .unwrap();
        assert!(mp.is_full());
        assert_eq!(mp.in_use(), 0);
        assert_eq!(mp.available(), 64);

        let mp1 = Mempool::lookup("mempool").unwrap();
        assert!(mp1.is_full());
        assert_eq!(mp1.in_use(), 0);
        assert_eq!(mp1.available(), 64);
    }
}

mod alloc {
    use super::*;
    use async_dpdk::alloc;

    #[test]
    fn test() {
        #[derive(Default)]
        struct Test {
            x: i32,
            y: i64,
        }

        let _eal = eal::Builder::new().iova_mode(IovaMode::VA).build().unwrap();

        let t = alloc::malloc::<Test>();
        assert_eq!(t.x, 0);
        assert_eq!(t.y, 0);

        alloc::free(t);
    }
}
