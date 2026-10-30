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
  h = cfg.hibernation;
  encryptedResume = h.enable && h.swap != null && h.swap.type == "luks2";
  cryptName = if encryptedResume && h.swap.luksDevice != null then h.swap.luksDevice else "";
  crypt = config.boot.initrd.luks.devices.${cryptName} or null;
  luksUuid = if crypt == null then "" else lib.removePrefix "/dev/disk/by-uuid/" crypt.device;
  resumePolicy = if !h.enable then { } else if h.swap == null then {
    swap_partuuid = h.swapPartuuid;
  } else {
    device = { inherit (h.swap) type partuuid; } // lib.optionalAttrs encryptedResume {
      luks_uuid = luksUuid;
      mapping = cryptName;
    };
    credential = if h.credential == null then null else {
      type = h.credential.type;
      pool_guid = h.credential.poolGuid;
      encryption_root = h.credential.encryptionRoot;
    };
  };
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
          "prune-nixos.py"
          "publish-loader.py"
          "authorize-linux.py"
          "authorize-boot.py"
          "install-efi.py"
        ]
  );
  rawLoaderConfig = pkgs.writeText "zbm-rs-target-config-input.json" (
    builtins.toJSON { kernel_args = cfg.kernelArgs; resume = h.resumeConfig; }
  );
  loaderConfig = if h.enable then pkgs.runCommand "zbm-rs-target-config.json" { } ''
    ${loader}/bin/zbm-rs --validate-config ${rawLoaderConfig} > $out
  '' else rawLoaderConfig;
  installerConfig = pkgs.writeText "zbm-rs-install-config.json" (
    builtins.toJSON {
      inherit pool loaderConfig;
      loader = "${loader}/bin/zbm-rs";
      signing =
        if cfg.signing.enable then lib.filterAttrs (name: _: name != "enable") cfg.signing else null;
      efi = if cfg.efi.enable then {
        inherit (cfg.efi) image certificate partuuid path label;
      } else null;
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
        pkgs.efibootmgr
        pkgs.dosfstools
      ]
    }:$PATH
    export UV_PYTHON=${pkgs.python3}/bin/python
    export UV_NO_MANAGED_PYTHON=1
    export UV_OFFLINE=1
    exec ${pkgs.uv}/bin/uv run --offline --no-project --python ${pkgs.python3}/bin/python ${support}/install-nixos.py --config ${installerConfig} "$@"
  '';
  prune = pkgs.writeShellApplication {
    name = "zbm-prune-generations";
    runtimeInputs = [ pkgs.uv pkgs.python3 ];
    text = ''
      export UV_NO_CACHE=1
      exec uv run --offline --no-project --no-managed-python --python ${pkgs.python3}/bin/python ${support}/prune-nixos.py "$@"
    '';
  };
  pruneAfterGc = pkgs.writeShellScript "zbm-prune-after-gc" ''
    set -eu
    removed="$(${prune}/bin/zbm-prune-generations --apply)"
    if [ -n "$removed" ]; then
      printf '%s\n' "$removed"
      exec ${config.nix.package.out}/bin/nix-store --gc
    fi
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
  resumeDevice = if encryptedResume then "/dev/mapper/${cryptName}"
    else "/dev/disk/by-partuuid/${if h.swap == null then h.swapPartuuid else h.swap.partuuid}";
  resumeDeviceUnit = "${utils.escapeSystemdPath resumeDevice}.device";
  cryptUnit = "systemd-cryptsetup@${utils.escapeSystemdPath cryptName}.service";
  resumePools = lib.unique (
    [ pool ] ++ config.boot.zfs.extraPools ++ map
      (fs: builtins.head (lib.splitString "/" fs.device))
      (builtins.filter (fs: fs.fsType == "zfs") (builtins.attrValues config.fileSystems))
  );
  recordResume = pkgs.writeShellScript "zbm-record-resume" ''
    set -eu
    export PATH=${lib.makeBinPath [ pkgs.util-linux pkgs.cryptsetup pkgs.lvm2 config.boot.zfs.package ]}
    swapoff ${lib.escapeShellArg resumeDevice}
    trap 'swapon ${lib.escapeShellArg resumeDevice}' EXIT
    ${loader}/bin/zbm-rs --resume-record-current-config \
      ${lib.escapeShellArg config.fileSystems."/".device} ${loaderConfig}
    swapon ${lib.escapeShellArg resumeDevice}
    trap - EXIT
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
    hibernation = {
      enable = lib.mkEnableOption "experimental native hibernation integration (requires a matching loader image)";
      swapPartuuid = lib.mkOption {
        type = lib.types.str;
        default = "";
        description = "Dedicated unencrypted swap partition; must match loader resume.swap_partuuid.";
      };
      swap = lib.mkOption {
        default = null;
        description = "Dedicated resume device; replaces legacy swapPartuuid.";
        type = lib.types.nullOr (lib.types.submodule {
          options = {
            type = lib.mkOption { type = lib.types.enum [ "partition" "luks2" ]; };
            partuuid = lib.mkOption { type = lib.types.str; };
            luksDevice = lib.mkOption { type = lib.types.nullOr lib.types.str; default = null;
              description = "Name in boot.initrd.luks.devices; its device must use a LUKS UUID."; };
          };
        });
      };
      credential = lib.mkOption {
        default = null;
        description = "Loader credential source; zfs-keyfile is not implemented.";
        type = lib.types.nullOr (lib.types.submodule {
          options = {
            type = lib.mkOption { type = lib.types.enum [ "zfs-unlock" ]; };
            poolGuid = lib.mkOption { type = lib.types.str; };
            encryptionRoot = lib.mkOption { type = lib.types.str; };
          };
        });
      };
      resumeConfig = lib.mkOption {
        type = lib.types.attrs; readOnly = true; default = resumePolicy;
        description = "Resolved non-secret policy to pass to mkImage loaderConfig.resume.";
      };
    };
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
    efi = {
      enable = lib.mkEnableOption "install a separately prepared, signed EFI image after generation publication";
      image = runtimePath "Runtime signed EFI image; prepare it before invoking the installation hook.";
      certificate = runtimePath "Public certificate required to verify the main EFI image.";
      partuuid = runtimePath "Explicit GPT partition UUID of the ESP.";
      path = lib.mkOption { type = lib.types.str; default = "EFI/zbm-rs/zbm-rs.EFI"; };
      label = lib.mkOption { type = lib.types.str; default = "zbm-rs"; };
    };
  };
  config = lib.mkIf cfg.enable {
    environment.systemPackages = [ prune ];
    systemd.services.nix-gc.serviceConfig.ExecStartPost = lib.mkIf config.nix.gc.automatic [ pruneAfterGc ];
    assertions = [
      {
        assertion = !cfg.hibernation.enable || builtins.all
          (fs: !(utils.fsNeededForBoot fs) || builtins.elem fs.fsType [ "zfs" "tmpfs" "ramfs" ])
          (builtins.attrValues config.fileSystems);
        message = "Native resume currently requires all persistent initramfs filesystems to use gated ZFS imports.";
      }
      {
        assertion = !h.enable || (builtins.match "[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}" (if h.swap == null then h.swapPartuuid else h.swap.partuuid) != null
          && (h.swap == null || h.swapPartuuid == ""));
        message = "zbm-rs hibernation requires a dedicated swap PARTUUID.";
      }
      {
        assertion = !encryptedResume || (crypt != null && lib.hasPrefix "/dev/disk/by-uuid/" crypt.device
          && crypt.keyFile != null && builtins.hasAttr crypt.keyFile config.boot.initrd.secrets
          && h.credential != null);
        message = "Encrypted resume requires a named LUKS UUID device, an initramfs secret key file and an explicit loader credential source.";
      }
      {
        assertion = !h.enable || encryptedResume || (h.credential == null && (h.swap == null || h.swap.luksDevice == null));
        message = "Plain resume partitions cannot use LUKS devices or credential sources.";
      }
      {
        assertion = !cfg.hibernation.enable || config.boot.zfs.unsafeAllowHibernation;
        message = "ZFS hibernation remains experimental; opt in explicitly with boot.zfs.unsafeAllowHibernation.";
      }
      {
        assertion = config.fileSystems."/".fsType == "zfs" && !(config.fileSystems ? "/nix");
        message = "zbm-rs requires a ZFS root with /nix inside that dataset.";
      }
      {
        assertion = !config.boot.loader.grub.enable && !config.boot.loader.systemd-boot.enable;
        message = "Native zbm-rs publication owns installBootLoader; disable other publishers.";
      }
      {
        assertion = !cfg.efi.enable || config.boot.loader.efi.canTouchEfiVariables;
        message = "zbm-rs EFI publication needs boot.loader.efi.canTouchEfiVariables.";
      }
    ];
    boot.bootspec.enable = lib.mkIf (lib.versionOlder config.system.nixos.release "26.11") true;
    boot.loader.grub.enable = lib.mkDefault false;
    boot.loader.systemd-boot.enable = lib.mkDefault false;
    boot.loader.efi.canTouchEfiVariables = lib.mkDefault cfg.efi.enable;
    boot.loader.external = {
      enable = true;
      installHook = installer;
    };
    boot.loader.supportsInitrdSecrets = true;
    boot.supportedFilesystems = [ "zfs" ];
    boot.zfs.forceImportRoot = false;
    boot.initrd.systemd.enable = true;
    boot.kernelParams = lib.mkIf cfg.hibernation.enable [ "hibernate=noresume" ];
    boot.initrd.systemd.suppressedUnits = lib.mkIf cfg.hibernation.enable [ "systemd-hibernate-resume.service" "systemd-hibernate-resume@.service" ];
    boot.initrd.systemd.suppressedStorePaths = lib.mkIf cfg.hibernation.enable [
      "${config.boot.initrd.systemd.package}/lib/systemd/system-generators/systemd-hibernate-resume-generator"
    ];
    # All imports require successful completion, not just ordering after a failed
    # guard. Suppress stock automatic resume without disabling systemd sleep.
    boot.initrd.systemd.services = {
      initrd-parse-etc.environment.SYSTEMD_SYSROOT_FSTAB = lib.mkForce earlyFstab;
    } // lib.optionalAttrs cfg.hibernation.enable (
      {
        zbm-resume = {
          description = "Restore saved session before importing ZFS";
          wantedBy = [ "initrd.target" ];
          requires = [ resumeDeviceUnit ] ++ lib.optional encryptedResume cryptUnit;
          after = [ resumeDeviceUnit ] ++ lib.optional encryptedResume cryptUnit;
          environment.PATH = lib.makeBinPath [ pkgs.cryptsetup pkgs.lvm2 ];
          before = [ "initrd-root-fs.target" ];
          unitConfig.DefaultDependencies = false;
          serviceConfig = {
            Type = "oneshot";
            RemainAfterExit = true;
            ExecStart = "${loader}/bin/zbm-rs --resume-restore-config ${loaderConfig}";
          };
        };
      } // lib.optionalAttrs encryptedResume {
        "systemd-cryptsetup@${utils.escapeSystemdPath cryptName}" = {
          overrideStrategy = "asDropin";
          requires = [ "initrd-nixos-copy-secrets.service" ];
          after = [ "initrd-nixos-copy-secrets.service" ];
        };
      } // lib.genAttrs (map (name: "zfs-import-${name}") resumePools) (_: {
        requires = [ "zbm-resume.service" ];
        after = [ "zbm-resume.service" ];
      })
    );
    systemd.services.systemd-hibernate = lib.mkIf cfg.hibernation.enable {
      preStart = "${recordResume}";
    };
    systemd.sleep.settings.Sleep = lib.mkIf cfg.hibernation.enable {
      AllowHybridSleep = false;
      AllowSuspendThenHibernate = false;
    };
    boot.initrd.systemd.root = null;
    boot.initrd.systemd.storePaths = [ earlyFstab ] ++ lib.optionals h.enable [ loader loaderConfig (lib.getBin pkgs.cryptsetup) (lib.getBin pkgs.lvm2) ];
    boot.initrd.systemd.managerEnvironment.SYSTEMD_SYSROOT_FSTAB = lib.mkForce earlyFstab;
    boot.initrd.systemd.contents."/etc/zbm-rs/root-selection-v1".text = "zfs-pool-root-v1\n";
    boot.initrd.systemd.contents."/etc/systemd/system-generators/zbm-rs-root".source = generator;
    # Explicit fileSystems mounts only, never bulk-mount foreign boot environments.
    systemd.services.zfs-mount.enable = false;
    systemd.services.zfs-share.enable = false;
  };
}
