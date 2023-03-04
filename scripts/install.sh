#!/bin/bash

# Install DPDK on ubuntu.

wget https://fast.dpdk.org/rel/dpdk-21.11.1.tar.xz
tar xf dpdk-21.11.1.tar.xz
cd dpdk-stable-21.11.1
mkdir build
meson -Denable_kmods=true build
ninja -C build
cd build
sudo ninja install

