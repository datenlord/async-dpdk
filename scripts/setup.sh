#!/bin/bash

if [ "$1" == "setup" ]; then
    mkdir -p /dev/hugepages
    mountpoint -q /dev/hugepages || mount -t hugetlbfs nodev /dev/hugepages
    echo 32 > /sys/devices/system/node/node0/hugepages/hugepages-2048kB/nr_hugepages
    # modprobe e1000 vfio vfio-pci
    
    # chmod 600 /sys/bus/pci/drivers/e1000/bind
    # chmod 600 /sys/bus/pci/drivers/e1000/unbind
    # chmod 600 /sys/bus/pci/drivers/vfio-pci/bind
    # chmod 600 /sys/bus/pci/drivers/vfio-pci/unbind

    # echo 1 > /sys/module/vfio/parameters/enable_unsafe_noiommu_mode
    # modprobe vfio enable_unsafe_noiommu_mode=1
    
    # PCI_NUM=$(lspci | grep -i 'eth' | sed '1d' | sed -r 's/\s.*//g' | tr -s '\n' ' ')
    # usertools/dpdk-devbind.py -b=vfio-pci $PCI_NUM
elif [ "$1" == "teardown" ]; then
    PCI_NUM=$(lspci | grep -i 'eth' | sed '1d' | sed -r 's/\s.*//g' | tr -s '\n' ' ')
    usertools/dpdk-devbind.py -b=e1000 $PCI_NUM
fi
