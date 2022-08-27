#!/bin/bash

# run as root
mkdir -p /dev/hugepages
mountpoint -q /dev/hugepages || mount -t hugetlbfs nodev /dev/hugepages
echo 1 > /sys/devices/system/node/node0/hugepages/hugepages-1048576kB/nr_hugepages

chmod 600 /sys/bus/pci/drivers/e1000/bind
chmod 600 /sys/bus/pci/drivers/e1000/unbind
chmod 600 /sys/bus/pci/drivers/vfio-pci/bind
chmod 600 /sys/bus/pci/drivers/vfio-pci/unbind

modprobe vfio vfio-pci
echo 1 > /sys/module/vfio/parameters/enable_unsafe_noiommu_mode
# modprobe vfio enable_unsafe_noiommu_mode=1

/home/luo/dpdk/usertools/dpdk-devbind.py -b=vfio-pci 02:02.0 02:03.0

# /home/luo/dpdk/usertools/dpdk-devbind.py -u 02:02.0 02:03.0