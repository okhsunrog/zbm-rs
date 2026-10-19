{ pkgs }:
pkgs.testers.runNixOSTest {
  name = "zbm-efi-install";
  globalTimeout = 300;
  nodes.machine = { ... }: {
    boot.loader.systemd-boot.enable = true;
    boot.loader.grub.enable = false;
    virtualisation = {
      useBootLoader = true;
      useEFIBoot = true;
      emptyDiskImages = [ 256 ];
      memorySize = 1536;
      additionalPaths = [ ./install-efi.py pkgs.systemd ];
    };
    environment.systemPackages = [
      pkgs.uv pkgs.python3 pkgs.efibootmgr pkgs.gptfdisk
      pkgs.dosfstools pkgs.openssl pkgs.sbsigntool pkgs.util-linux
    ];
  };
  testScript = ''
    import json, shlex
    machine.start()
    machine.wait_for_unit("multi-user.target")
    machine.succeed("sgdisk --new=1:0:0 --typecode=1:ef00 --partition-guid=1:ed5d4b72-0aac-48d3-8f89-43362d952ca0 /dev/vdb; blockdev --rereadpt /dev/vdb; udevadm settle; mkfs.vfat /dev/vdb1")
    machine.succeed("mkdir -p /root/efi-test /mnt/esp; mount /dev/vdb1 /mnt/esp; mkdir -p /mnt/esp/EFI/foreign; printf keep > /mnt/esp/EFI/foreign/existing.EFI")
    machine.succeed("openssl req -new -x509 -newkey rsa:2048 -nodes -subj /CN=disposable-efi-test/ -keyout /root/efi-test/key.pem -out /root/efi-test/cert.pem -days 1 >/dev/null 2>&1")
    machine.succeed("cp ${pkgs.systemd}/lib/systemd/boot/efi/linuxx64.efi.stub /root/efi-test/rescue.efi; sbsign --key /root/efi-test/key.pem --cert /root/efi-test/cert.pem --output /root/efi-test/main.efi /root/efi-test/rescue.efi")
    original_order = machine.succeed("efibootmgr | grep '^BootOrder:'").strip().split(": ")[1].split(",")
    def install(label, path, image, cert):
        config = {"partuuid": "ed5d4b72-0aac-48d3-8f89-43362d952ca0", "label": label,
                  "path": path, "image": "/root/efi-test/" + image, "certificate": cert}
        machine.succeed("printf %s " + shlex.quote(json.dumps(config)) + " > /root/efi-test/config.json")
        return "uv run --offline --no-project python ${./install-efi.py} /root/efi-test/config.json"
    main = install("zbm-rs", "EFI/zbm-rs/zbm-rs.EFI", "main.efi", "/root/efi-test/cert.pem")
    machine.succeed(main)
    machine.succeed("sha256sum /mnt/esp/EFI/zbm-rs/zbm-rs.EFI > /root/efi-test/main.sha; stat -c %Y /mnt/esp/EFI/zbm-rs/zbm-rs.EFI > /root/efi-test/mtime")
    machine.succeed(main)
    machine.succeed("stat -c %Y /mnt/esp/EFI/zbm-rs/zbm-rs.EFI | cmp /root/efi-test/mtime")
    rescue = install("zbm-rs (rescue)", "EFI/zbm-rs/rescue.EFI", "rescue.efi", None)
    machine.succeed(rescue)
    machine.succeed("sha256sum -c /root/efi-test/main.sha; cmp /root/efi-test/rescue.efi /mnt/esp/EFI/zbm-rs/rescue.EFI; test $(cat /mnt/esp/EFI/foreign/existing.EFI) = keep")
    machine.succeed("cp /root/efi-test/rescue.efi /root/efi-test/second.efi; printf v2 >> /root/efi-test/second.efi; sbsign --key /root/efi-test/key.pem --cert /root/efi-test/cert.pem --output /root/efi-test/main2.efi /root/efi-test/second.efi")
    machine.succeed(install("zbm-rs", "EFI/zbm-rs/zbm-rs.EFI", "main2.efi", "/root/efi-test/cert.pem"))
    machine.succeed("cmp /root/efi-test/main2.efi /mnt/esp/EFI/zbm-rs/zbm-rs.EFI; cmp /root/efi-test/rescue.efi /mnt/esp/EFI/zbm-rs/rescue.EFI; sha256sum /mnt/esp/EFI/zbm-rs/zbm-rs.EFI > /root/efi-test/main.sha")
    order = machine.succeed("efibootmgr | grep '^BootOrder:'").strip().split(": ")[1].split(",")
    assert order[:len(original_order)] == original_order and len(order) == len(original_order) + 2
    # A rescue update can never accept an image trusted by Secure Boot.
    machine.fail(install("zbm-rs (rescue)", "EFI/zbm-rs/rescue.EFI", "main.efi", None))
    # Main publication must reject unsigned inputs without touching either entry.
    machine.fail(install("zbm-rs", "EFI/zbm-rs/zbm-rs.EFI", "rescue.efi", "/root/efi-test/cert.pem"))
    machine.fail(install("zbm-rs", "EFI/foreign/other.EFI", "main.efi", "/root/efi-test/cert.pem"))
    machine.succeed("sha256sum -c /root/efi-test/main.sha; cmp /root/efi-test/rescue.efi /mnt/esp/EFI/zbm-rs/rescue.EFI")
    # Also exercise the installer's own mount/unmount path.
    machine.succeed("umount /mnt/esp")
    machine.succeed(install("zbm-rs (rescue)", "EFI/zbm-rs/rescue.EFI", "rescue.efi", None))
    machine.fail("findmnt --source /dev/vdb1")
    machine.shutdown()
  '';
}
