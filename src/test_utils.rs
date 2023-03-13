#![cfg(test)]

use std::sync::Once;

use crate::eal;

static SETUP: Once = Once::new();

pub(crate) fn dpdk_setup() {
    SETUP.call_once(|| {
        env_logger::init();
        eal::Config::new().enter().unwrap();
    })
}
