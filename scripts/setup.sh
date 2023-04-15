#!/bin/bash

env_setup() {
    mkdir -p /dev/hugepages
    mountpoint -q /dev/hugepages || mount -t hugetlbfs nodev /dev/hugepages
    echo 32 > /sys/devices/system/node/node0/hugepages/hugepages-2048kB/nr_hugepages
    
    modprobe vfio
    echo 1 > /sys/module/vfio/parameters/enable_unsafe_noiommu_mode
    modprobe e1000
    modprobe vfio-pci
    
    chmod 600 /sys/bus/pci/drivers/e1000/bind
    chmod 600 /sys/bus/pci/drivers/e1000/unbind
    chmod 600 /sys/bus/pci/drivers/vfio-pci/bind
    chmod 600 /sys/bus/pci/drivers/vfio-pci/unbind
    
    modprobe vfio enable_unsafe_noiommu_mode=1
    
    PCI_NUM=$(lspci | grep -i 'eth' | sed '1d' | sed -r 's/\s.*//g' | tr -s '\n' ' ')
    usertools/dpdk-devbind.py -b=vfio-pci $PCI_NUM
}

env_teardown() {
    PCI_NUM=$(lspci | grep -i 'eth' | sed '1d' | sed -r 's/\s.*//g' | tr -s '\n' ' ')
    usertools/dpdk-devbind.py -u $PCI_NUM
}

command=$1

case $command in
    "setup") env_setup ;;
    "teardown") env_teardown ;;
esac