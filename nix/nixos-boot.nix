# Native NixOS generation installation; independent from EFI image construction.
{ self }:
{
  config,
  lib,
  pkgs,
  utils,
  ...
}:
let
  cfg = config.boot.loader.zbm-rs;
  pool = builtins.head (lib.splitString "/" config.fileSystems."/".device);
  loader = self.packages.${pkgs.stdenv.hostPlatform.system}.zbm-rs;
  support = pkgs.runCommand "zbm-rs-nixos-installer" { } (
    ''
      mkdir -p $out
    ''
    +
      lib.concatMapStringsSep "\n"
        (
          name:
          let
            file = builtins.path {
              path = ./. + "/${name}";
              inherit name;
            };
          in
          "cp ${file} $out/${name}"
        )
        [
          "install-nixos.py"
          "publish-loader.py"
          "authorize-linux.py"
          "authorize-boot.py"
        ]
  );
  loaderConfig = pkgs.writeText "zbm-rs-target-config.json" (
    builtins.toJSON { kernel_args = cfg.kernelArgs; }
  );
  installerConfig = pkgs.writeText "zbm-rs-install-config.json" (
    builtins.toJSON {
      inherit pool loaderConfig;
      loader = "${loader}/bin/zbm-rs";
      signing =
        if cfg.signing.enable then lib.filterAttrs (name: _: name != "enable") cfg.signing else null;
    }
  );
  installer = pkgs.writeShellScript "install-zbm-rs-generation" ''
    export PATH=${
      lib.makeBinPath [
        pkgs.coreutils
        pkgs.util-linux
        pkgs.openssl
        pkgs.sbsigntool
        pkgs.uv
        pkgs.python3
      ]
    }:$PATH
    export UV_PYTHON=${pkgs.python3}/bin/python
    export UV_NO_MANAGED_PYTHON=1
    export UV_OFFLINE=1
    exec ${pkgs.uv}/bin/uv run --offline --no-project --python ${pkgs.python3}/bin/python ${support}/install-nixos.py --config ${installerConfig} "$@"
  '';
  generator = pkgs.replaceVarsWith {
    src = builtins.path {
      path = ./nixos-root-generator.sh;
      name = "nixos-root-generator.sh";
    };
    isExecutable = true;
    replacements = {
      bash = "${pkgs.bash}/bin/bash";
      path = lib.makeBinPath [ pkgs.coreutils ];
      inherit pool;
    };
  };
  # Keep stock NixOS formatting/options for early data mounts, but root is
  # selected explicitly. Leaving it here duplicates systemd's cmdline root.
  escapeFstab = value: builtins.replaceStrings [ " " "\t" ] [ "\\040" "\\011" ] value;
  earlyMounts = pkgs.writeText "zbm-rs-early-mounts" (
    lib.concatMapStrings (fs: escapeFstab fs.mountPoint + "\n") (
      lib.filter (fs: fs.mountPoint != "/" && utils.fsNeededForBoot fs) (
        lib.attrValues config.fileSystems
      )
    )
  );
  earlyFstab = pkgs.runCommand "zbm-rs-initrd-fstab" { nativeBuildInputs = [ pkgs.gawk ]; } ''
    awk 'NR == FNR { keep[$0] = 1; next } $2 in keep' ${earlyMounts} ${config.environment.etc.fstab.source} > $out
  '';
  runtimePath =
    description:
    lib.mkOption {
      type = lib.types.str;
      inherit description;
    };
in
{
  options.boot.loader.zbm-rs = {
    enable = lib.mkEnableOption "native NixOS generation publication for zbm-rs";
    kernelArgs = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      default = [ ];
      description = "Must match kernel_args in the installed zbm-rs image.";
    };
    signing = {
      enable = lib.mkEnableOption "owner signing of kernel, final initramfs and boot authorization";
      kernelKey = runtimePath "Runtime EFI private key path, never a Nix path literal.";
      kernelCertificate = runtimePath "Runtime EFI public certificate path.";
      imaKey = runtimePath "Runtime IMA private key path, never a Nix path literal.";
      imaCertificate = runtimePath "Runtime IMA public certificate path.";
      authorityKey = runtimePath "Runtime boot authorization private key path.";
      authorityCertificate = runtimePath "Runtime boot authorization public certificate path.";
      evmctl = lib.mkOption {
        type = lib.types.str;
        default = "${pkgs.ima-evm-utils}/bin/evmctl";
      };
    };
  };
  config = lib.mkIf cfg.enable {
    assertions = [
      {
        assertion = config.fileSystems."/".fsType == "zfs" && !(config.fileSystems ? "/nix");
        message = "zbm-rs requires a ZFS root with /nix inside that dataset.";
      }
      {
        assertion = !config.boot.loader.grub.enable && !config.boot.loader.systemd-boot.enable;
        message = "Native zbm-rs publication owns installBootLoader; disable other publishers.";
      }
    ];
    boot.bootspec.enable = lib.mkIf (lib.versionOlder config.system.nixos.release "26.11") true;
    boot.loader.grub.enable = lib.mkDefault false;
    boot.loader.systemd-boot.enable = lib.mkDefault false;
    boot.loader.efi.canTouchEfiVariables = false;
    boot.loader.external = {
      enable = true;
      installHook = installer;
    };
    boot.loader.supportsInitrdSecrets = true;
    boot.supportedFilesystems = [ "zfs" ];
    boot.zfs.forceImportRoot = false;
    boot.initrd.systemd.enable = true;
    boot.initrd.systemd.root = null;
    boot.initrd.systemd.storePaths = [ earlyFstab ];
    boot.initrd.systemd.managerEnvironment.SYSTEMD_SYSROOT_FSTAB = lib.mkForce earlyFstab;
    boot.initrd.systemd.services.initrd-parse-etc.environment.SYSTEMD_SYSROOT_FSTAB =
      lib.mkForce earlyFstab;
    boot.initrd.systemd.contents."/etc/zbm-rs/root-selection-v1".text = "zfs-pool-root-v1\n";
    boot.initrd.systemd.contents."/etc/systemd/system-generators/zbm-rs-root".source = generator;
    # Explicit fileSystems mounts only, never bulk-mount foreign boot environments.
    systemd.services.zfs-mount.enable = false;
    systemd.services.zfs-share.enable = false;
  };
}
