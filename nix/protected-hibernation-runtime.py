"""Run the complete protected chain outside Nix, with disposable runtime keys.

The Nix driver owns QEMU/swtpm. Only public signed artifacts and a disposable
ZFS/LUKS credential enter the guest. Signing keys stay in the parent process.
"""
import datetime
import hashlib
import importlib.util
import json
import os
from pathlib import Path
from collections import deque
import select
import shlex
import shutil
import subprocess
import sys
import time

sys.dont_write_bytecode = True


def load(path, name):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def command(arguments, **kwargs):
    result = subprocess.run([str(value) for value in arguments], check=True,
                            stdout=subprocess.PIPE, **kwargs)
    return result.stdout


def wait(check, timeout=90):
    deadline = time.monotonic() + timeout
    last = None
    while time.monotonic() < deadline:
        try:
            result = check()
            if result:
                return result
        except (AssertionError, subprocess.CalledProcessError, ValueError, OSError, KeyError, TypeError) as error:
            last = error
        time.sleep(0.2)
    raise AssertionError(f"Protected-chain condition timed out: {last}")


def screen(machine, offset=0):
    for line in reversed(machine.get_console_log()[offset:].splitlines()):
        try:
            event = json.loads(line)
            if isinstance(event, dict) and "event" in event:
                return event
        except ValueError:
            pass
    raise ValueError("No observed loader UI state")


def password(machine, text):
    # Neither characters nor their count enter the driver's per-key log.
    for character in text:
        machine.send_key(character, log=False)
    machine.send_key("ret", log=False)


def select_generation(machine, generation):
    for _ in range(12):
        state = screen(machine)
        if state["ui"].get("selected_row", {}).get("Generation") == generation:
            return
        previous = state["ui"].get("selected_row")
        machine.send_key("down")
        wait(lambda: screen(machine)["ui"].get("selected_row") != previous, 10)
    raise AssertionError(f"Generation {generation} is not selectable")


def guest_store_file(root, guest):
    """Resolve a generated store link inside the private extracted guest tree.

    Absolute guest links start at root, never at the host's /nix/store. The
    extraction tree is owned exclusively by this process while preparing it.
    """
    root = root.resolve(strict=True)
    path = Path(guest)
    if not str(guest).startswith("/nix/store/") or ".." in path.parts:
        raise ValueError("Expected generated Nix store artifact")
    pending = deque(path.parts[1:])
    resolved = []
    links = 0
    while pending:
        part = pending.popleft()
        if part == "..":
            if not resolved:
                raise ValueError("Guest artifact escapes its root")
            resolved.pop()
            continue
        if part in ("", "."):
            continue
        candidate = root.joinpath(*resolved, part)
        if candidate.is_symlink():
            links += 1
            if links > 40:
                raise ValueError("Guest artifact has a symlink loop")
            target = candidate.readlink()
            if target.is_absolute():
                resolved = []
                parts = target.parts[1:]
            else:
                parts = target.parts
            pending.extendleft(reversed(parts))
        else:
            candidate.lstat()  # Missing guest files must never fall back to host.
            resolved.append(part)
    file = root.joinpath(*resolved)
    if resolved[:2] != ["nix", "store"] or not file.is_file():
        raise ValueError("Expected owned regular Nix artifact")
    return file


def prepare_root(source, bundle, work, signing):
    fixture = load(source / "xtask/fixtures/authorize_nixos.py", "nixos_fixture")
    publisher = load(source / "nix/publish-loader.py", "publisher")
    root = work / "root"
    root.mkdir()
    fixture.unpack_tar(Path(bundle["root"]) / "root.tar", root)
    target = str(bundle["target"])
    document = json.loads((root / target.lstrip("/") / "boot.json").read_text())
    spec = document["org.nixos.bootspec.v1"]
    prepared = root / "boot/zbm-rs/nixos" / Path(target).name
    prepared.mkdir(parents=True)
    for name in ("kernel", "initrd"):
        shutil.copyfile(guest_store_file(root, spec[name]), prepared / name)
    # Do not repack/re-sign the approved initrd module tree. The native secret
    # appender must preserve the exact Nix-built base checked by the publisher.
    if appender := spec.get("initrdSecrets"):
        command([appender, prepared / "initrd"])
    command(["sbverify", "--cert", signing["certificate"], prepared / "kernel"])
    publisher.ima_sign(prepared / "initrd", signing["ima_key"], signing["ima_certificate"],
                       Path(bundle["evmctl"]), os.environ.copy())
    records = []
    for generation in (1, 2, 3):
        current = target if generation == 1 else f"/nix/store/zbm-hibernation-fixture-{generation}"
        if generation != 1:
            variant = root / current.lstrip("/")
            variant.mkdir()
            changed = json.loads(json.dumps(document))
            changed_spec = changed["org.nixos.bootspec.v1"]
            changed_spec["toplevel"] = current
            changed_spec["init"] = current + "/init"
            (variant / "boot.json").write_text(json.dumps(changed))
            (variant / "init").symlink_to(target + "/init")
            (variant / "etc").symlink_to(target + "/etc")
            (root / f"nix/var/nix/profiles/system-{generation}-link").symlink_to(current)
            copy = root / "boot/zbm-rs/nixos" / Path(current).name
            copy.mkdir()
            for name in ("kernel", "initrd", "initrd.sig"):
                shutil.copyfile(prepared / name, copy / name)
        else:
            copy = prepared
        bootspec = root / current.lstrip("/") / "boot.json"
        (copy / "prepared.json").write_text(json.dumps({
            "version": 1, "bootspec_sha256": hashlib.sha256(bootspec.read_bytes()).hexdigest(),
        }))
        plans = json.loads(command([bundle["binary"], "--nixos-plans", root, "testpool/root", bundle["config"], current]))
        assert len(plans) == 1
        arguments = work / f"arguments-{generation}.json"
        arguments.write_text(json.dumps(plans[0]["cmdline"]))
        invoke = ["uv", "run", "--offline", "--no-project", "python", source / "nix/authorize-boot.py",
                  "--kernel", copy / "kernel", "--initramfs", copy / "initrd",
                  "--ima-signature", copy / "initrd.sig", "--arguments", arguments,
                  "--dataset", "testpool/root", "--root-prefix", "root=ZFS=",
                  "--authority-key", signing["key"], "--authority-cert", signing["certificate"],
                  "--output", root / "boot/zbm-rs/authorizations"]
        if generation != 2:
            invoke.extend(["--protected-contract", Path(bundle["root"]) / "contract.json"])
        result = json.loads(command(invoke))
        if generation == 3:
            payload = Path(result["authorization"])
            policy = json.loads(payload.read_text())
            policy["protected_hibernation"]["resume_policy_sha256"] = "00" * 32
            payload.write_text(json.dumps(policy, separators=(",", ":")))
            command(["openssl", "cms", "-sign", "-binary", "-md", "sha256", "-in", payload,
                     "-signer", signing["certificate"], "-inkey", signing["key"],
                     "-outform", "DER", "-out", result["cms"]])
        records.append({"generation": generation, "authorization": result})
    archive = work / "prepared-root.tar"
    command(["tar", "--numeric-owner", "--owner=0", "--group=0", "-cf", archive, "-C", root, "."])
    (work / "target-authorizations.json").write_text(json.dumps(records, indent=2))
    return archive


def run(machine, *, source, tools):
    if os.geteuid() == 0:
        raise ValueError("Run the disposable runtime driver as the regular repository owner")
    source = Path(source)
    work = Path(os.environ["ZBM_RESUME_VM_WORKDIR"]).resolve()
    work.mkdir(parents=True, exist_ok=True)
    staging = work / "authoring"
    staging.mkdir()  # Never reuse a partially prepared run.
    (work / "tmp").mkdir()
    os.environ["TMPDIR"] = str(work / "tmp")
    os.environ["PATH"] = tools + "/bin:" + os.environ["PATH"]
    os.environ["UV_NO_MANAGED_PYTHON"] = "1"
    os.environ["UV_OFFLINE"] = "1"
    signing = {
        "key": Path(os.environ["ZBM_RESUME_VM_KEY"]).resolve(strict=True),
        "certificate": Path(os.environ["ZBM_RESUME_VM_CERT"]).resolve(strict=True),
        "ima_key": Path(os.environ["ZBM_RESUME_VM_IMA_KEY"]).resolve(strict=True),
        "ima_certificate": Path(os.environ["ZBM_RESUME_VM_IMA_CERT"]).resolve(strict=True),
    }
    for name in ("key", "ima_key"):
        if signing[name].is_relative_to("/nix/store") or not signing[name].is_file():
            raise ValueError("Disposable private keys must remain outside Nix")
    compressed = os.environ.get("ZBM_RESUME_VM_COMPRESSED") == "1"
    report = {"complete": False, "checks": [], "source": str(source),
              "host_disks_attached": False, "compressed_image": compressed}
    (work / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    def passed(name):
        report["checks"].append(name)
        (work / "report.json").write_text(json.dumps(report, indent=2) + "\n")
        print("PASSED", name, flush=True)
    shared_secret = "disposable-vm-passphrase"
    secret = work / "disposable.key"
    secret.write_text(shared_secret)
    secret.chmod(0o600)
    machine.start()
    machine.wait_for_unit("multi-user.target")
    machine.succeed("mkdir -p /etc/zfs; printf disposable-vm-passphrase > /etc/zfs/test.key; chmod 600 /etc/zfs/test.key")
    machine.succeed("sgdisk --zap-all --new=1:2048:0 --partition-guid=1:12345678-1234-1234-1234-123456789abc /dev/vdd; udevadm settle")
    machine.succeed("cryptsetup luksFormat --batch-mode --type luks2 --pbkdf pbkdf2 --pbkdf-force-iterations 1000 --uuid ca55a30a-636e-4e69-9e68-bb36bf380408 --key-file /etc/zfs/test.key /dev/vdd1; cryptsetup open --key-file /etc/zfs/test.key /dev/vdd1 resume-swap; mkswap /dev/mapper/resume-swap; cryptsetup close resume-swap")
    machine.succeed("zpool create -o cachefile=none -O compression=lz4 -O mountpoint=none -O encryption=aes-256-gcm -O keyformat=passphrase -O keylocation=file:///etc/zfs/test.key testpool /dev/vdc; zfs create -u -o mountpoint=/ -o canmount=noauto testpool/root; zfs set org.zfsbootmenu:active=on testpool/root; zpool set bootfs=testpool/root testpool; mkdir /mnt; mount -t zfs -o zfsutil testpool/root /mnt")
    pool_guid = machine.succeed("zpool get -Hp -o value guid testpool").strip()
    public = {
        "source": str(source), "poolGuid": pool_guid, "secretSource": str(secret),
        "compressedImage": compressed,
        "certificate": str(signing["certificate"]), "imaCertificate": str(signing["ima_certificate"]),
        "signedKernel": os.environ["ZBM_RESUME_VM_SIGNED_KERNEL"],
        "signedModules": os.environ["ZBM_RESUME_VM_SIGNED_MODULES"],
    }
    # Explicit public recipe override for backport regression diagnosis. The
    # normal acceptance run uses this same source for loader and target.
    if kernel_source := os.environ.get("ZBM_RESUME_VM_KERNEL_SOURCE"):
        public["kernelSource"] = str(Path(kernel_source).resolve(strict=True))
    parameters = work / "public-inputs.json"
    parameters.write_text(json.dumps(public))
    print("Building immutable protected loader and native target; see nix-build.log", flush=True)
    with (work / "nix-build.log").open("wb") as log:
        built = json.loads(command(["nix", "build", "--out-link", work / "built-inputs", "--json", "--impure", "--file",
                                    source / "nix/protected-hibernation-fixture.nix", "--argstr", "parameters", parameters,
                                    "bundle", "--cores", "4", "--max-jobs", "1"], stderr=log))
    bundle = json.loads(Path(built[0]["outputs"]["out"]).read_text())
    (work / "bundle.json").write_text(json.dumps(bundle, indent=2))
    deploy = work / "signed-loader"
    with (work / "loader-publish.log").open("wb") as log:
        command(["uv", "run", "--offline", "--no-project", "python", source / "nix/publish-loader.py",
                 bundle["loader"], deploy, "--efi-key", signing["key"], "--efi-cert", signing["certificate"],
                 "--module-key", signing["key"], "--module-cert", signing["certificate"],
                 "--ima-key", signing["ima_key"], "--ima-cert", signing["ima_certificate"],
                 "--sign-file", bundle["sign_file"], "--evmctl", bundle["evmctl"],
                 "--ukify", bundle["ukify"], "--allow-fixture"], stderr=log)
    published = json.loads((deploy / "manifest.json").read_text())
    for name, relative in (("efi", "esp/EFI/BOOT/BOOTX64.EFI"),
                           ("kernel", "vmlinuz"), ("initramfs", "initramfs.img")):
        if hashlib.sha256((deploy / relative).read_bytes()).hexdigest() != published[f"{name}_sha256"]:
            raise ValueError(f"Published {name} hash differs from the signed deployment")
    archive = prepare_root(source, bundle, staging, signing)
    ssh_key = work / "ssh.key"
    shutil.copyfile(deploy / "id_ed25519", ssh_key)
    ssh_key.chmod(0o600)
    ssh_public = command(["ssh-keygen", "-y", "-f", ssh_key]).decode().strip()
    machine.succeed("mkdir -p /root/.ssh; chmod 700 /root/.ssh; printf '%s\\n' " + shlex.quote(ssh_public) + " > /root/.ssh/authorized_keys; chmod 600 /root/.ssh/authorized_keys")
    ssh_options = ["-i", str(ssh_key), "-o", "IdentitiesOnly=yes", "-o", "BatchMode=yes", "-o", "StrictHostKeyChecking=no",
                   "-o", "UserKnownHostsFile=/dev/null", "-o", "ConnectTimeout=2", "-o", "LogLevel=ERROR"]
    def ssh(script):
        return command(["ssh", *ssh_options, "-p", "22941", "root@127.0.0.1", "set -eu\n" + script], stderr=subprocess.PIPE).decode().strip()
    def upload(path, destination):
        command(["scp", *ssh_options, "-P", "22941", str(path), "root@127.0.0.1:" + destination], stderr=subprocess.PIPE)
    wait(lambda: ssh("test $(hostname) != resume-target; echo ready") == "ready")
    # The bootstrap's writable root is RAM-backed. Stream directly to the
    # disposable ZFS dataset instead of staging a multi-GiB duplicate there.
    with archive.open("rb") as stream:
        command(["ssh", *ssh_options, "-p", "22941", "root@127.0.0.1",
                 "set -eu; tar -xf - -C /mnt"], stdin=stream, stderr=subprocess.PIPE)
    upload(deploy / "esp/EFI/BOOT/BOOTX64.EFI", "/root/protected-loader.EFI")
    machine.succeed("mkdir -p /mnt/etc/zfs; cp /etc/zfs/test.key /mnt/etc/zfs/test.key; umount /mnt; zpool export testpool")
    machine.succeed("set -eu; test -d /boot/EFI; mkdir -p /boot/EFI/zbm-test; cp /root/protected-loader.EFI /boot/EFI/zbm-test/main.EFI; efibootmgr -c -d /dev/vda -p 1 -L ZBM-Protected-Hibernation -l '\\EFI\\zbm-test\\main.EFI'; sync")
    machine.shutdown()
    variables = Path(machine.efi_vars_path)
    assert variables.is_relative_to(machine.state_dir) and variables.is_file() and not variables.is_symlink()
    enrolled = work / "secure-vars.fd"
    owner = "8a8e09ee-f748-4bac-b6eb-28ff9922c625"
    with (work / "firmware-enrollment.log").open("wb") as log:
        command(["virt-fw-vars", "--input", variables, "--output", enrolled,
                 "--set-pk", owner, signing["certificate"], "--add-kek", owner, signing["certificate"],
                 "--add-db", owner, signing["certificate"], "--secure-boot"], stderr=log)
    shutil.copyfile(enrolled, variables)
    passed("disposable_setup_and_runtime_only_signing")

    def loader_boot(wanted):
        machine.start()
        # start() clears the driver's boot-scoped console buffer. An offset
        # from the previous boot would hide this boot's UI events indefinitely.
        wait(lambda: screen(machine)["ui"]["panel"] == "Unlock", 150)
        password(machine, shared_secret)
        wait(lambda: screen(machine)["event"] == "catalog-ready" and screen(machine)["state"]["resume"]["state"] == wanted, 60)
        evidence = json.loads(ssh("cat /run/zbm-rs/security.json"))
        assert evidence["firmware"] == "enabled" and evidence["mode"] == "enforce" and evidence["ima_policy_loaded"] is True, evidence
        assert ssh("zpool get -H -o value readonly testpool") == "on"
        # OVMF also writes menu text to the virtio console. This loader has no
        # backdoor shell to consume it. Drain only its pending output now,
        # before handing off: a restored shell emits no new ready handshake
        # and would otherwise inherit those bytes ahead of its base64 replies.
        assert machine.shell is not None
        while select.select([machine.shell], [], [], 0)[0]:
            if not machine.shell.recv(65536):
                raise ValueError("Disconnected VM console before target handoff")
        return screen(machine)

    def target_boot(*, restored=False):
        # RAM restoration resumes the original backdoor shell; it does not
        # produce the fresh-boot handshake expected by connect().
        if restored:
            # Do not send shell protocol bytes into the loader or target
            # initramfs while the kernel handoff/restoration is still pending.
            machine.wait_for_console_text("PM: hibernation: hibernation exit", timeout=120)
            if compressed:
                assert "Loading and decompressing image data" in machine.get_console_log()
        machine.connected = restored
        if not restored:
            machine.connect()
        machine.wait_for_unit("multi-user.target")
        machine.succeed("test $(findmnt -n -o SOURCE /) = testpool/root; grep -q '\\[integrity\\]' /sys/kernel/security/lockdown; test $(cat /proc/sys/kernel/kexec_load_disabled) = 1; zpool status -x | grep -q 'all pools are healthy'")
        machine.wait_for_unit("systemd-tpm2-setup.service")
        machine.succeed("test -s /var/lib/systemd/tpm2-srk-public-key.pem; tpm2_nvreadpublic 0x014c4853")

    loader_boot("inactive")
    machine.send_key("k")
    wait(lambda: screen(machine)["ui"]["view"] == "Generations")
    for generation, reason in ((2, "lacks owner-authorized protected hibernation"), (3, "resume device or credential policy differs")):
        select_generation(machine, generation)
        machine.send_key("ret")
        wait(lambda: screen(machine)["event"] == "boot-error" and reason in screen(machine)["state"]["error"])
        assert ssh("cat /sys/kernel/kexec_loaded") == "0"
        passed(f"cold_boot_refuses_generation_{generation}")
    select_generation(machine, 1)
    machine.send_key("ret")
    target_boot()
    original_boot = machine.succeed("cat /proc/sys/kernel/random/boot_id").strip()
    machine.succeed("printf preserved-protected-ram > /run/protected-resume-proof; echo 67108864 > /sys/power/image_size")
    machine.execute("systemctl --no-block hibernate", check_return=False)
    machine.wait_for_shutdown(timeout=datetime.timedelta(seconds=90))
    if compressed:
        assert "Compressing and saving image data" in machine.get_console_log()
    pending = loader_boot("pending")
    session = pending["state"]["resume"]["session"]["id"]
    before_swap = ssh("sha256sum /dev/vdd")
    machine.screenshot(str(work / "protected-pending.png"))
    machine.send_key("f10")
    machine.wait_for_shutdown(timeout=datetime.timedelta(seconds=45))
    pending = loader_boot("pending")
    assert pending["state"]["resume"]["session"]["id"] == session
    assert ssh("sha256sum /dev/vdd") == before_swap
    passed("pending_poweroff_preserves_image_without_confirmation")
    machine.send_key("ret")
    target_boot(restored=True)
    assert machine.succeed("cat /proc/sys/kernel/random/boot_id").strip() == original_boot
    machine.succeed("test $(cat /run/protected-resume-proof) = preserved-protected-ram")
    machine.wait_until_succeeds("test $(systemctl show -p ActiveState --value systemd-hibernate.service) = inactive", timeout=30)
    machine.succeed("test $(systemctl show -p Result --value systemd-hibernate.service) = success")
    assert json.loads(machine.succeed("zbm-rs --resume-status-config /etc/zbm-rs/test-resume-policy.json"))["state"] == "inactive"
    passed("secure_boot_ima_cms_luks_zfs_authenticated_ram_restore")
    # Explicit cancellation is exercised through the unprivileged manager and
    # privileged broker, rather than calling the swap writer directly.
    machine.execute("systemctl --no-block hibernate", check_return=False)
    machine.wait_for_shutdown(timeout=datetime.timedelta(seconds=90))
    loader_boot("pending")
    loader_id = ssh("cat /proc/sys/kernel/random/boot_id")
    header_before = ssh("dd if=/dev/mapper/resume-swap bs=4096 count=1 status=none | sha256sum")
    machine.send_key("f2")
    wait(lambda: screen(machine)["ui"]["panel"] == "Actions")
    for _ in range(24):
        if screen(machine)["ui"].get("selected_action") == "CancelResume":
            break
        previous = screen(machine)["ui"].get("selected_action")
        machine.send_key("down")
        wait(lambda: screen(machine)["ui"].get("selected_action") != previous, 10)
    else:
        raise AssertionError("Missing discard-only action")
    machine.send_key("ret")
    wait(lambda: screen(machine)["ui"]["panel"] == "ConfirmResume")
    machine.screenshot(str(work / "protected-discard-confirmation.png"))
    machine.send_key("esc")
    wait(lambda: screen(machine)["ui"]["panel"] != "ConfirmResume" and screen(machine)["state"]["resume"]["state"] == "pending")
    assert ssh("dd if=/dev/mapper/resume-swap bs=4096 count=1 status=none | sha256sum") == header_before
    machine.send_key("f2")
    wait(lambda: screen(machine)["ui"]["panel"] == "Actions")
    for _ in range(24):
        if screen(machine)["ui"].get("selected_action") == "CancelResume":
            break
        previous = screen(machine)["ui"].get("selected_action")
        machine.send_key("down")
        wait(lambda: screen(machine)["ui"].get("selected_action") != previous, 10)
    else:
        raise AssertionError("Missing discard-only action after declining consent")
    machine.send_key("ret")
    wait(lambda: screen(machine)["ui"]["panel"] == "ConfirmResume")
    machine.send_key("ret")
    wait(lambda: screen(machine)["state"]["resume"]["state"] == "inactive")
    assert ssh("cat /proc/sys/kernel/random/boot_id") == loader_id
    assert ssh("dd if=/dev/mapper/resume-swap bs=1 skip=4086 count=10 status=none") == "SWAPSPACE2"
    assert ssh("cat /sys/kernel/kexec_loaded") == "0"
    passed("broker_discard_only_requires_consent_and_stays_in_menu")
    machine.send_key("ret")
    target_boot()
    assert machine.succeed("cat /proc/sys/kernel/random/boot_id").strip() != original_boot
    machine.succeed("test ! -e /run/protected-resume-proof")
    passed("explicit_discard_then_ordinary_protected_cold_boot")
    machine.shutdown()
    report["complete"] = True
    (work / "report.json").write_text(json.dumps(report, indent=2) + "\n")
