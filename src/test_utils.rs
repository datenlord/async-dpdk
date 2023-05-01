use std::sync::Once;

use crate::eal::{self, Vdev};

static SETUP: Once = Once::new();

pub(crate) fn dpdk_setup() {
    SETUP.call_once(|| {
        env_logger::init();
        eal::Config::new()
            .no_hugepages(true)
            .vdev(Vdev::Ring(0))
            .vdev(Vdev::Ring(1))
            .enter()
            .unwrap();
    })
}
