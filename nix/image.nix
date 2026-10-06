{ pkgs, source, zfsSource, kernelPackages ? pkgs.linuxPackages
, zfsUserspace ? pkgs.zfs_2_4.override { enablePython = false; }
, testProfile ? false, profile ? "portable", hardwareManifest ? null
, extraModules ? [], forcedModules ? [], loaderConfig ? null }:
let
  lib = pkgs.lib;
  hardware = if hardwareManifest == null then {} else builtins.fromJSON (builtins.readFile hardwareManifest);
  portable = [ "nvme" "ahci" "sd_mod" "usb_storage" "uas" "usbhid" "hid_generic"
    "xhci_pci" "ehci_pci" "atkbd" "i8042" "virtio_pci" "virtio_blk" "virtio_scsi" "virtio_gpu" ];
  roots = lib.unique ([ "zfs" ] ++ (if profile == "portable" then portable else hardware.modules or [])
    ++ extraModules ++ forcedModules ++ lib.optionals testProfile [ "e1000" ]);
  kernel = kernelPackages.kernel;
  zfsModule = builtins.getAttr zfsUserspace.kernelModuleAttribute kernelPackages;
  moduleTree = pkgs.aggregateModules [ kernel.modules zfsModule ];
  modules = pkgs.makeModulesClosure {
    kernel = moduleTree;
    firmware = pkgs.linux-firmware;
    rootModules = roots;
    allowMissing = false;
    extraFirmwarePaths = hardware.firmwarePaths or [];
  };
  configJson = pkgs.writeText "zbm-rs-config.json" (builtins.toJSON (
    if loaderConfig == null then builtins.fromJSON (builtins.readFile
      (source + (if testProfile then "/config/test.json" else "/config/default.json")))
    else loaderConfig
  ));
  rustSource = lib.cleanSourceWith {
    src = source;
    filter = path: type:
      let relative = lib.removePrefix (toString source + "/") path;
      in path == toString source || lib.any (name: relative == name || lib.hasPrefix (name + "/") relative) [ "Cargo.toml" "Cargo.lock" "crates" "xtask" "config" ];
  };
  combinedSource = pkgs.runCommand "zbm-workspace-source" {} ''
    mkdir -p $out/zbm-rs $out/zfskit
    cp -r ${rustSource}/Cargo.toml ${rustSource}/Cargo.lock ${rustSource}/crates ${rustSource}/xtask ${rustSource}/config $out/zbm-rs/
    cp -r ${zfsSource}/. $out/zfskit/
  '';
  binary = pkgs.rustPlatform.buildRustPackage {
    ZBM_RS_CONFIG = configJson;
    pname = "zbm-rs";
    version = "0.1.0";
    src = combinedSource;
    sourceRoot = "zbm-workspace-source/zbm-rs";
    cargoLock.lockFile = rustSource + "/Cargo.lock";
    cargoBuildFlags = [ "-p" "zbm-rs" ];
    cargoTestFlags = [ "-p" "zbm-rs" "-p" "zbm-core" ];
    buildFeatures = lib.optional testProfile "vm-test";
  };
  tools = [
    { source = "${binary}/bin/zbm-rs"; target = "/bin/zbm-rs"; }
    { source = "${pkgs.busybox}/bin/busybox"; link = "/bin/busybox"; }
    { source = "${pkgs.kmod}/bin/modprobe"; link = "/usr/bin/modprobe"; }
    { source = "${zfsUserspace}/bin/zfs"; link = "/usr/bin/zfs"; }
    { source = "${zfsUserspace}/bin/zpool"; link = "/usr/bin/zpool"; }
    { source = "${pkgs.systemdMinimal}/lib/systemd/systemd-udevd"; link = "/usr/lib/systemd/systemd-udevd"; }
    { source = "${pkgs.systemdMinimal}/bin/udevadm"; link = "/usr/bin/udevadm"; }
    { source = "${pkgs.systemdMinimal}/lib/udev/ata_id"; link = "/usr/lib/udev/ata_id"; }
    { source = "${pkgs.systemdMinimal}/lib/udev/scsi_id"; link = "/usr/lib/udev/scsi_id"; }
    { source = "${lib.getLib pkgs.kmod}/lib/libkmod.so.2"; link = "/usr/lib/libkmod.so.2"; }
    { source = "${lib.getLib pkgs.util-linux}/lib/libblkid.so.1"; link = "/usr/lib/libblkid.so.1"; }
  ] ++ lib.optionals testProfile [
    { source = "${pkgs.openssh}/bin/sshd"; link = "/usr/bin/sshd"; }
    { source = "${pkgs.openssh}/bin/ssh-keygen"; link = "/usr/bin/ssh-keygen"; }
    { source = "${pkgs.openssh}/libexec/sshd-session"; }
    { source = "${pkgs.openssh}/libexec/sshd-auth"; }
  ];
  setup = pkgs.writeText "setup.sh" (lib.replaceStrings
    [ "# NIX_FORCED_MODULES" ]
    [ (lib.concatMapStringsSep "\n" (name: "/usr/bin/modprobe " + lib.escapeShellArg name) (lib.remove "zfs" forcedModules)) ]
    (builtins.readFile (source + "/boot/setup.sh")));
  runtime = pkgs.runCommand "zbm-runtime-${profile}" {
    nativeBuildInputs = [ pkgs.uv pkgs.python3 pkgs.patchelf pkgs.cpio pkgs.openssh ];
  } ''
    export UV_CACHE_DIR="$TMPDIR/uv-cache"
    mkdir -p root/{bin,usr/bin,usr/lib,usr/sbin,etc/zbm-rs,root,proc,sys,dev,run,tmp,var/empty,usr/share/empty.sshd}
    uv run --offline --no-project --python ${pkgs.python3}/bin/python ${source}/nix/stage-runtime.py root ${pkgs.writeText "tools.json" (builtins.toJSON tools)}
    ln -s /bin/zbm-rs root/init
    ln -s ${pkgs.busybox}/bin/busybox root/bin/sh
    ln -s usr/lib root/lib
    ln -s usr/sbin root/sbin
    cp -r ${modules}/lib/. root/usr/lib/
    cp ${setup} root/etc/zbm-rs/setup.sh
    install -Dm444 ${configJson} root${configJson}
    ln -s ${configJson} root/etc/zbm-rs/config.json
    printf 'root:x:0:0:root:/root:/bin/sh\nsshd:x:74:74:sshd:/var/empty:/bin/false\nnobody:x:65534:65534:nobody:/:/bin/false\n' > root/etc/passwd
    printf 'root::0:0:99999:7:::\nsshd:!:0:0:99999:7:::\n' > root/etc/shadow
    printf 'root:x:0:\nsshd:x:74:\nnobody:x:65534:\n' > root/etc/group
    mkdir -p root/usr/lib/udev/rules.d
    cp ${pkgs.systemdMinimal}/lib/udev/rules.d/{60-persistent-storage,80-drivers}.rules root/usr/lib/udev/rules.d/
    substituteInPlace root/usr/lib/udev/rules.d/60-persistent-storage.rules \
      --replace-quiet '${pkgs.systemdMinimal}/lib/udev' '/usr/lib/udev'
    ${lib.optionalString testProfile ''
      mkdir -p root/etc/ssh root/root/.ssh
      ssh-keygen -q -t ed25519 -N "" -C zbm-disposable-test -f client-key
      cp client-key.pub root/root/.ssh/authorized_keys
      chmod 700 root/root/.ssh
      chmod 600 root/root/.ssh/authorized_keys
      cp ${source}/boot/sshd_config root/etc/ssh/sshd_config
      touch root/etc/zbm-test-ssh
    ''}
    mkdir -p $out
    (cd root; find . -exec touch -h -d '@1' {} +; find . -print0 | LC_ALL=C sort -z | cpio --null --quiet -o -H newc --owner=0:0 --reproducible) > $out/root.cpio
    ${lib.optionalString testProfile "cp client-key $out/id_ed25519"}
  '';
  initramfs = pkgs.runCommand "zbm-initramfs-${profile}" { nativeBuildInputs = [ pkgs.gzip ]; } ''
    mkdir -p $out
    gzip -n -9 < ${runtime}/root.cpio > $out/initramfs.img
  '';
  osRelease = pkgs.writeText "zbm-os-release" "ID=zbm-rs\nNAME=zbm-rs\nVERSION_ID=0.1.0\n";
  cmdline = "console=ttyS0,115200 console=tty0 loglevel=3 panic=-1";
  image = pkgs.runCommand "zbm-rs-efi-${profile}${lib.optionalString testProfile "-test"}" {
    nativeBuildInputs = [ pkgs.systemdUkify pkgs.python3 pkgs.uv ];
    passthru = { inherit binary kernel zfsModule modules initramfs configJson; inherit sizeCheck; };
  } ''
    export UV_CACHE_DIR="$TMPDIR/uv-cache"
    mkdir -p $out/esp/EFI/BOOT
    cp ${kernel}/bzImage $out/vmlinuz
    cp ${initramfs}/initramfs.img $out/initramfs.img
    echo ${lib.escapeShellArg cmdline} > $out/cmdline
    ukify build --linux $out/vmlinuz --initrd $out/initramfs.img \
      --uname ${lib.escapeShellArg kernel.modDirVersion} --cmdline @${pkgs.writeText "cmdline" cmdline} \
      --os-release @${osRelease} --stub ${pkgs.systemd}/lib/systemd/boot/efi/linuxx64.efi.stub \
      --output $out/esp/EFI/BOOT/BOOTX64.EFI
    ${lib.optionalString testProfile "cp ${runtime}/id_ed25519 $out/id_ed25519"}
    uv run --offline --no-project --python ${pkgs.python3}/bin/python ${source}/nix/image-metadata.py \
      $out ${lib.escapeShellArg kernel.modDirVersion} ${lib.escapeShellArg zfsModule.version} \
      ${if testProfile then "yes" else "no"} ${lib.escapeShellArg profile} ${source}/nix/image-size-policy.json
  '';
  sizeCheck = pkgs.runCommand "zbm-image-size-check" {} ''
    test -s ${image}/sizes.json
    cp ${image}/sizes.json $out
  '';
in
assert lib.assertMsg (builtins.elem profile [ "portable" "host-only" ]) "Unknown image profile";
assert lib.assertMsg (profile != "host-only" || hardwareManifest != null || extraModules != [] || forcedModules != []) "Host-only requires an explicit manifest or NixOS module lists";
assert lib.assertMsg (zfsModule.kernel.modDirVersion == kernel.modDirVersion) "ZFS kernel module ABI mismatch";
assert lib.assertMsg (zfsModule.version == zfsUserspace.version) "ZFS kernel and userspace version mismatch";
image
