{ pkgs, source, runtimePkgs ? pkgs, kernelPackages ? pkgs.linuxPackages
, zfsUserspace ? (import ./lean-userspace.nix { pkgs = runtimePkgs; }).zfs
, testProfile ? false, profile ? "portable", hardwareManifest ? null
, extraModules ? [], forcedModules ? [], loaderConfig ? null
, kernelPolicy ? "validate", kernelTrustedCertificates ? []
, targetAuthorities ? [], imaCertificate ? null
, pcrPublicKey ? null, tpmProvider ? (import ./tpm-provider.nix { inherit pkgs; })
, initramfsCompression ? "zstd", rustProfile ? {}
, udevPackage ? (import ./lean-userspace.nix { pkgs = runtimePkgs; }).udev }:
let
  lib = pkgs.lib;
  rawConfig = if loaderConfig == null then builtins.fromJSON (builtins.readFile
    (source + (if testProfile then "/config/test.json" else "/config/default.json")))
    else loaderConfig;
  security = rawConfig.security or {};
  enforced = (security.mode or "off") == "enforce";
  tpmConfig = security.tpm or {};
  tpmEnabled = (tpmConfig.policy or "off") != "off";
  nvpcrs = tpmConfig.nvpcrs or [];
  publicInput = path:
    let content = builtins.readFile path;
    in assert lib.assertMsg (builtins.stringLength content <= 65536 && !(lib.hasInfix "PRIVATE KEY" content))
      "Trust inputs must be bounded public certificates/keys, never private keys";
    path;
  publicImaCertificate = if imaCertificate == null then null else publicInput imaCertificate;
  publicPcrKey = if pcrPublicKey == null then null else publicInput pcrPublicKey;
  selectedKernelPackages = if enforced && kernelPolicy == "configure" then
    import ./security-kernel.nix { inherit pkgs kernelPackages;
      trustedCertificates = map publicInput kernelTrustedCertificates; }
    else kernelPackages;
  authorities = lib.imap0 (index: certificate: {
    name = "authority-${toString index}.pem"; certificate = publicInput certificate;
  }) targetAuthorities;
  trustedConfig = rawConfig // lib.optionalAttrs (enforced || targetAuthorities != [] || imaCertificate != null) {
    security = security // {
      target_authorities = map (authority: "/etc/zbm-rs/trust/${authority.name}") authorities;
      ima_certificate = if imaCertificate == null then null else "/etc/zbm-rs/trust/ima.der";
    };
  };
  hardware = if hardwareManifest == null then {} else builtins.fromJSON (builtins.readFile hardwareManifest);
  portable = [ "nvme" "ahci" "sd_mod" "usb_storage" "uas" "usbhid" "hid_generic"
    "xhci_pci" "ehci_pci" "atkbd" "i8042" "virtio_pci" "virtio_blk" "virtio_scsi" "virtio_gpu" ];
  roots = lib.unique ([ "zfs" ] ++ (if profile == "portable" then portable else hardware.modules or [])
    ++ extraModules ++ forcedModules ++ lib.optionals testProfile [ "e1000" ]
    ++ lib.optionals tpmEnabled [ "tpm_crb" "tpm_tis" ]);
  kernel = selectedKernelPackages.kernel;
  zfsModule = builtins.getAttr zfsUserspace.kernelModuleAttribute selectedKernelPackages;
  kernelCheck = pkgs.runCommand "zbm-loader-security-capabilities" {
    nativeBuildInputs = [ pkgs.uv pkgs.python3 ];
  } ''
    export UV_CACHE_DIR="$TMPDIR/uv-cache"
    uv run --offline --no-project --python ${pkgs.python3}/bin/python \
      ${source}/nix/check-security-kernel.py ${kernel.configfile} > $out
  '';
  imaPolicy = pkgs.writeText "zbm-ima-policy" ''
    appraise func=KEXEC_INITRAMFS_CHECK appraise_type=imasig
  '';
  moduleTree = pkgs.aggregateModules [ kernel.modules zfsModule ];
  modules = pkgs.makeModulesClosure {
    kernel = moduleTree;
    firmware = pkgs.linux-firmware;
    rootModules = roots;
    allowMissing = false;
    extraFirmwarePaths = hardware.firmwarePaths or [];
  };
  configJson = pkgs.writeText "zbm-rs-config.json" (builtins.toJSON trustedConfig);
  rustSource = lib.cleanSourceWith {
    src = source;
    filter = path: type:
      let relative = lib.removePrefix (toString source + "/") path;
      in path == toString source || lib.any (name: relative == name || lib.hasPrefix (name + "/") relative) [ "Cargo.toml" "Cargo.lock" "crates" "xtask" "config" ];
  };
  binary = runtimePkgs.rustPlatform.buildRustPackage {
    ZBM_RS_CONFIG = configJson;
    pname = "zbm-rs";
    version = "0.1.0";
    src = rustSource;
    cargoLock.lockFile = rustSource + "/Cargo.lock";
    nativeBuildInputs = [ runtimePkgs.pkg-config ];
    buildInputs = [ runtimePkgs.openssl ];
    cargoBuildFlags = [ "-p" "zbm-rs" ];
    cargoTestFlags = [ "-p" "zbm-rs" "-p" "zbm-core" ];
    buildFeatures = lib.optional testProfile "vm-test";
    env = lib.optionalAttrs (rustProfile ? optLevel) { CARGO_PROFILE_RELEASE_OPT_LEVEL = toString rustProfile.optLevel; }
      // lib.optionalAttrs (rustProfile ? lto) { CARGO_PROFILE_RELEASE_LTO = rustProfile.lto; }
      // lib.optionalAttrs (rustProfile ? codegenUnits) { CARGO_PROFILE_RELEASE_CODEGEN_UNITS = toString rustProfile.codegenUnits; }
      # Share the musl already needed by C tools instead of embedding another copy.
      // lib.optionalAttrs (runtimeLibc == "musl") { RUSTFLAGS = "-C target-feature=-crt-static"; };
  };
  udevDaemon = if (udevPackage.pname or "") == "eudev" then "${udevPackage}/bin/udevd"
    else "${udevPackage}/lib/systemd/systemd-udevd";
  udevRules = if (udevPackage.pname or "") == "eudev" then "${udevPackage}/var/lib/udev/rules.d"
    else "${udevPackage}/lib/udev/rules.d";
  tools = [
    { source = "${binary}/bin/zbm-rs"; target = "/bin/zbm-rs"; }
    { source = "${runtimePkgs.busybox}/bin/busybox"; link = "/bin/busybox"; }
    { source = "${runtimePkgs.kmod}/bin/modprobe"; link = "/usr/bin/modprobe"; }
    { source = "${zfsUserspace}/bin/zfs"; link = "/usr/bin/zfs"; }
    { source = "${zfsUserspace}/bin/zpool"; link = "/usr/bin/zpool"; }
    { source = "${zfsUserspace}/bin/mount.zfs"; link = "/usr/sbin/mount.zfs"; }
    { source = "${udevDaemon}"; link = "/usr/lib/systemd/systemd-udevd"; }
    { source = "${udevPackage}/bin/udevadm"; link = "/usr/bin/udevadm"; }
    { source = "${udevPackage}/lib/udev/ata_id"; link = "/usr/lib/udev/ata_id"; }
    { source = "${udevPackage}/lib/udev/scsi_id"; link = "/usr/lib/udev/scsi_id"; }
    { source = "${lib.getLib runtimePkgs.kmod}/lib/libkmod.so.2"; link = "/usr/lib/libkmod.so.2"; }
    { source = "${lib.getLib runtimePkgs.util-linux}/lib/libblkid.so.1"; link = "/usr/lib/libblkid.so.1"; }
  ] ++ lib.optionals testProfile [
    { source = "${runtimePkgs.openssh}/bin/sshd"; link = "/usr/bin/sshd"; }
    { source = "${runtimePkgs.openssh}/bin/ssh-keygen"; link = "/usr/bin/ssh-keygen"; }
    { source = "${runtimePkgs.openssh}/libexec/sshd-session"; }
    { source = "${runtimePkgs.openssh}/libexec/sshd-auth"; }
  ] ++ lib.optionals tpmEnabled [
    { source = "${tpmProvider}/lib/systemd/systemd-tpm2-setup"; target = "/usr/lib/systemd/systemd-tpm2-setup"; }
    { source = "${tpmProvider}/lib/systemd/systemd-pcrextend"; target = "/usr/lib/systemd/systemd-pcrextend"; }
    # systemd loads TSS libraries dynamically; DT_NEEDED scanning alone misses them.
    { source = "${lib.getLib pkgs.tpm2-tss}/lib/libtss2-esys.so.0"; link = "/usr/lib/libtss2-esys.so.0"; }
    { source = "${lib.getLib pkgs.tpm2-tss}/lib/libtss2-mu.so.0"; link = "/usr/lib/libtss2-mu.so.0"; }
    { source = "${lib.getLib pkgs.tpm2-tss}/lib/libtss2-rc.so.0"; link = "/usr/lib/libtss2-rc.so.0"; }
    { source = "${lib.getLib pkgs.tpm2-tss}/lib/libtss2-tcti-device.so.0"; link = "/usr/lib/libtss2-tcti-device.so.0"; }
    { source = "${lib.getLib pkgs.tpm2-tss}/lib/libtss2-tctildr.so.0"; link = "/usr/lib/libtss2-tctildr.so.0"; }
    { source = "${lib.getLib pkgs.openssl}/lib/libcrypto.so.3"; link = "/usr/lib/libcrypto.so.3"; }
  ];
  setup = pkgs.writeText "setup.sh" (lib.replaceStrings
    [ "# NIX_FORCED_MODULES" ]
    [ (lib.concatMapStringsSep "\n" (name: "/usr/bin/modprobe " + lib.escapeShellArg name) (lib.remove "zfs" forcedModules)) ]
    (builtins.readFile (source + "/boot/setup.sh")));
  runtimeLibc = runtimePkgs.stdenv.hostPlatform.libc;
  runtime = pkgs.runCommand "zbm-runtime-${profile}" {
    nativeBuildInputs = [ pkgs.uv pkgs.python3 pkgs.patchelf pkgs.cpio pkgs.openssh pkgs.openssl ];
  } ''
    export UV_CACHE_DIR="$TMPDIR/uv-cache"
    mkdir -p root/{bin,usr/bin,usr/lib,usr/sbin,etc/zbm-rs,root,proc,sys,dev,run,tmp,var/empty,usr/share/empty.sshd}
    uv run --offline --no-project --python ${pkgs.python3}/bin/python ${source}/nix/stage-runtime.py root ${pkgs.writeText "tools.json" (builtins.toJSON tools)} ${runtimeLibc}
    ln -s /bin/zbm-rs root/init
    ln -s ${runtimePkgs.busybox}/bin/busybox root/bin/sh
    ln -s usr/lib root/lib
    ln -s usr/sbin root/sbin
    cp -r ${modules}/lib/. root/usr/lib/
    cp ${setup} root/etc/zbm-rs/setup.sh
    ${lib.optionalString tpmEnabled ''
      printf 'ID=zbm-rs\n' > root/etc/initrd-release
      mkdir -p root/etc/nvpcr
    ''}
    ${lib.concatMapStringsSep "\n" (name: ''
      uv run --offline --no-project --python ${pkgs.python3}/bin/python - \
        ${tpmProvider}/lib/nvpcr/${name}.nvpcr root/etc/nvpcr/${name}.nvpcr ${name} <<'PY'
    import json, sys
    definition = json.load(open(sys.argv[1]))
    definition["priority"] = {"hardware": 100, "cryptsetup": 200, "login": 300, "verity": 800}[sys.argv[3]]
    json.dump(definition, open(sys.argv[2], "w"))
    PY
    '') nvpcrs}
    ${lib.optionalString (pcrPublicKey != null) ''
      install -Dm444 ${publicPcrKey} root/etc/systemd/tpm2-pcr-public-key.pem
    ''}
    install -Dm444 ${configJson} root${configJson}
    ln -s ${configJson} root/etc/zbm-rs/config.json
    ${lib.optionalString enforced ''
      install -Dm444 ${kernelCheck} root/etc/zbm-rs/kernel-capabilities.json
      install -Dm444 ${imaPolicy} root/etc/zbm-rs/security/ima-policy
    ''}
    ${lib.concatMapStringsSep "\n" (authority: ''
      mkdir -p root/etc/zbm-rs/trust
      openssl x509 -in ${lib.escapeShellArg "${authority.certificate}"} \
        -out root/etc/zbm-rs/trust/${authority.name}
    '') authorities}
    ${lib.optionalString (imaCertificate != null) ''
      uv run --offline --no-project --python ${pkgs.python3}/bin/python \
        ${source}/nix/check-certificate.py ${lib.escapeShellArg "${publicImaCertificate}"}
      mkdir -p root/etc/zbm-rs/trust
      openssl x509 -in ${lib.escapeShellArg "${publicImaCertificate}"} -outform DER \
        -out root/etc/zbm-rs/trust/ima.der
    ''}
    printf 'root:x:0:0:root:/root:/bin/sh\nsshd:x:74:74:sshd:/var/empty:/bin/false\nnobody:x:65534:65534:nobody:/:/bin/false\n' > root/etc/passwd
    printf 'root::0:0:99999:7:::\nsshd:!:0:0:99999:7:::\n' > root/etc/shadow
    printf 'root:x:0:\nsshd:x:74:\nnobody:x:65534:\n' > root/etc/group
    mkdir -p root/usr/lib/udev/rules.d
    # eudev's vendor rules path is compiled against its original Nix prefix.
    # This common path is searched by both eudev and systemd-udevd.
    mkdir -p root/etc/udev
    ln -s /usr/lib/udev/rules.d root/etc/udev/rules.d
    cp ${udevRules}/{60-persistent-storage,80-drivers}.rules root/usr/lib/udev/rules.d/
    substituteInPlace root/usr/lib/udev/rules.d/60-persistent-storage.rules \
      --replace-quiet '${udevPackage}/lib/udev' '/usr/lib/udev'
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
    cp runtime-audit.json $out/runtime-audit.json
    (cd root; find . -exec touch -h -d '@1' {} +; find . -print0 | LC_ALL=C sort -z | cpio --null --quiet -o -H newc --owner=0:0 --reproducible) > $out/root.cpio
    ${lib.optionalString testProfile "cp client-key $out/id_ed25519"}
  '';
  initramfs = pkgs.runCommand "zbm-initramfs-${profile}" { nativeBuildInputs = [ pkgs.gzip pkgs.xz pkgs.zstd ]; } ''
    mkdir -p $out
    ${if initramfsCompression == "gzip" then "gzip -n -9"
      else if initramfsCompression == "xz" then "xz --check=crc32 -6"
      else "zstd -q -T1 -19"} < ${runtime}/root.cpio > $out/initramfs.img
  '';
  osRelease = pkgs.writeText "zbm-os-release" "ID=zbm-rs\nNAME=zbm-rs\nVERSION_ID=0.1.0\n";
  cmdline = "console=ttyS0,115200 console=tty0 loglevel=3 panic=${if enforced then "0" else "-1"}";
  image = pkgs.runCommand "zbm-rs-efi-${profile}${lib.optionalString testProfile "-test"}" {
    nativeBuildInputs = [ pkgs.systemdUkify pkgs.python3 pkgs.uv ];
    passthru = { inherit binary kernel zfsModule zfsUserspace udevPackage modules initramfs configJson runtime runtimeLibc; inherit sizeCheck kernelCheck; };
  } ''
    export UV_CACHE_DIR="$TMPDIR/uv-cache"
    mkdir -p $out/esp/EFI/BOOT
    cp ${kernel}/bzImage $out/vmlinuz
    cp ${initramfs}/initramfs.img $out/initramfs.img
    cp ${runtime}/runtime-audit.json $out/runtime-audit.json
    ${lib.optionalString enforced ''
      cp ${runtime}/root.cpio $out/root.cpio
      cp ${kernel.configfile} $out/kernel.config
      cp ${kernelCheck} $out/kernel-capabilities.json
      cp ${imaPolicy} $out/ima-policy
      cp ${pkgs.systemd}/lib/systemd/boot/efi/linuxx64.efi.stub $out/stub.efi
      cp ${osRelease} $out/os-release
    ''}
    ${lib.optionalString (pcrPublicKey != null) ''
      cp ${publicPcrKey} $out/pcr-public.pem
    ''}
    echo ${lib.escapeShellArg cmdline} > $out/cmdline
    ukify build --linux $out/vmlinuz --initrd $out/initramfs.img \
      --uname ${lib.escapeShellArg kernel.modDirVersion} --cmdline @${pkgs.writeText "cmdline" cmdline} \
      --os-release @${osRelease} --stub ${pkgs.systemd}/lib/systemd/boot/efi/linuxx64.efi.stub \
      --output $out/esp/EFI/BOOT/BOOTX64.EFI
    ${lib.optionalString testProfile "cp ${runtime}/id_ed25519 $out/id_ed25519"}
    uv run --offline --no-project --python ${pkgs.python3}/bin/python ${source}/nix/image-metadata.py \
      $out ${lib.escapeShellArg kernel.modDirVersion} ${lib.escapeShellArg zfsModule.version} \
      ${if testProfile then "yes" else "no"} ${lib.escapeShellArg profile} ${source}/nix/image-size-policy.json ${initramfsCompression}
  '';
  sizeCheck = pkgs.runCommand "zbm-image-size-check" {} ''
    test -s ${image}/sizes.json
    cp ${image}/sizes.json $out
  '';
in
assert lib.assertMsg (builtins.elem initramfsCompression [ "gzip" "xz" "zstd" ]) "Unknown initramfs compressor";
assert lib.assertMsg (builtins.elem profile [ "portable" "host-only" ]) "Unknown image profile";
assert lib.assertMsg (builtins.elem kernelPolicy [ "validate" "configure" ]) "Unknown loader kernel policy";
assert lib.assertMsg (!enforced || (targetAuthorities != [] && imaCertificate != null)) "Enforced image needs public authorization and IMA certificates";
assert lib.assertMsg (nvpcrs == [] || (tpmEnabled && pcrPublicKey != null)) "NvPCR setup needs a public PCR policy key";
assert lib.assertMsg (!tpmEnabled || runtimeLibc == "glibc") "Current TPM provider requires the glibc userspace profile";
assert lib.assertMsg (profile != "host-only" || hardwareManifest != null || extraModules != [] || forcedModules != []) "Host-only requires an explicit manifest or NixOS module lists";
assert lib.assertMsg (zfsModule.kernel.modDirVersion == kernel.modDirVersion) "ZFS kernel module ABI mismatch";
assert lib.assertMsg (zfsModule.version == zfsUserspace.version) "ZFS kernel and userspace version mismatch";
image
