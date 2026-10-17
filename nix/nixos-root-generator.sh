#!@bash@
# shellcheck shell=bash disable=SC2239
set -euo pipefail
export PATH=@path@
early=${2:?missing early generator directory}
dataset=
read -r -a words < /proc/cmdline
for word in "${words[@]}"; do
    case "$word" in root=ZFS=*) dataset=${word#root=ZFS=} ;; esac
done
if [[ $dataset != '@pool@/'* || ! $dataset =~ ^[A-Za-z0-9_./:-]+$ || $dataset == *..* ]]; then
    echo 'Missing or invalid zbm-rs ZFS root selection' >&2
    dataset=/invalid-zbm-root
fi
mkdir -p "$early/initrd-root-fs.target.requires"
cat > "$early/sysroot.mount" <<EOF
[Unit]
Description=ZFS root selected by zbm-rs
DefaultDependencies=no
Requires=zfs-import-@pool@.service
After=zfs-import-@pool@.service
Before=initrd-root-fs.target
[Mount]
What=$dataset
Where=/sysroot
Type=zfs
Options=zfsutil,rw
EOF
ln -s ../sysroot.mount "$early/initrd-root-fs.target.requires/sysroot.mount"
