#!/bin/sh
/bin/busybox --install -s /bin
/usr/bin/modprobe virtio_pci
/usr/bin/modprobe virtio_blk
/usr/bin/modprobe virtio_gpu
/usr/bin/modprobe i8042
/usr/bin/modprobe atkbd
/usr/bin/modprobe usbhid
# NIX_FORCED_MODULES
/usr/lib/systemd/systemd-udevd --daemon --resolve-names=never
/usr/bin/udevadm trigger --type=subsystems --action=add
/usr/bin/udevadm trigger --type=devices --action=add
/usr/bin/udevadm settle --timeout=10
if [ -f /etc/zbm-test-ssh ]; then
    /usr/bin/modprobe e1000
    /bin/busybox ip link set eth0 up
    /bin/busybox ip addr add 10.0.2.15/24 dev eth0
    /bin/busybox ip route add default via 10.0.2.2
    mkdir -p /run/sshd /etc/ssh
    ssh-keygen -A
    /usr/bin/sshd -E /dev/ttyS0
fi
