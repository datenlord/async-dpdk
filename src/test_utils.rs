#![cfg(test)]

use std::sync::Once;

use crate::eal;

static SETUP: Once = Once::new();

pub(crate) fn dpdk_setup() {
    SETUP.call_once(|| {
        eal::Config::new().no_hugepages(true).enter().unwrap();
    })
}
