# Included in the encrypted-root test after the target has booted. All disks,
# fault injection and sleep operations belong to the disposable guest.
import time

unit = "systemd-suspend-then-hibernate.service"


def inactive_image():
    status = json.loads(machine.succeed(status_command))
    assert status["state"] == "inactive", status


def finished():
    machine.wait_until_succeeds(f"test $(systemctl show -p ActiveState --value {unit}) = inactive", timeout=30)
    machine.succeed(f"test $(systemctl show -p Result --value {unit}) = success")
    assert machine.succeed("cat /proc/sys/kernel/random/boot_id").strip() == before
    machine.succeed("test $(cat /run/resume-proof) = preserved-in-ram; zpool status -x | grep -q 'all pools are healthy'")
    inactive_image()


def wake_from_suspend(delay):
    # This Q35 fixture does not reliably wake s2idle from CMOS alarms. Inject a
    # real ACPI power-button wake instead. Waiting beyond HibernateDelaySec
    # exercises systemd's real timer-expiry branch, but does not certify RTC
    # delivery on hardware. Logind ignores the button only in this test target.
    machine.wait_for_console_text("PM: suspend entry \\(s2idle\\)", timeout=30)
    time.sleep(delay)
    assert machine.qmp_client is not None
    machine.qmp_client.send("system_powerdown")


# A production preparation error must abort before systemd-sleep runs. Foreign
# bootbits give the actual recorder a valid swap header it must not overwrite.
machine.succeed("swapoff /dev/mapper/resume-swap; dd if=/dev/mapper/resume-swap of=/run/original-header bs=4096 count=1 status=none; printf foreign-bootbits | dd of=/dev/mapper/resume-swap conv=notrunc status=none; swapon /dev/mapper/resume-swap")
machine.succeed("journalctl --sync")
sleep_count = machine.succeed("journalctl -b -u " + unit + " --no-pager | grep -c 'Performing sleep operation' || true").strip()
machine.fail("systemctl start " + unit)
assert machine.succeed("journalctl -b -u " + unit + " --no-pager | grep -c 'Performing sleep operation' || true").strip() == sleep_count
machine.succeed("grep -q /dev/dm- /proc/swaps; swapoff /dev/mapper/resume-swap; dd if=/run/original-header of=/dev/mapper/resume-swap conv=notrunc status=none; swapon /dev/mapper/resume-swap; systemctl reset-failed " + unit)
inactive_image()

# Simulate a leftover image on inactive disposable swap. The activation used in
# cleanup traps must refuse it byte-for-byte; util-linux swapon would rewrite it.
machine.succeed(record_command)
machine.succeed("swapoff /dev/mapper/resume-swap; dd if=/dev/mapper/resume-swap of=/run/clean-header bs=4096 count=1 status=none; dd if=/run/clean-header bs=1 skip=4086 count=10 status=none | dd of=/dev/mapper/resume-swap bs=1 seek=4076 conv=notrunc status=none; printf 'S1SUSPEND\\0' | dd of=/dev/mapper/resume-swap bs=1 seek=4086 conv=notrunc status=none; dd if=/dev/mapper/resume-swap of=/run/pending-header bs=4096 count=1 status=none")
machine.fail(activate_command)
machine.succeed("! grep -q /dev/dm- /proc/swaps; dd if=/dev/mapper/resume-swap of=/run/after-finish-header bs=4096 count=1 iflag=direct status=none; cmp /run/pending-header /run/after-finish-header")
assert json.loads(machine.succeed(status_command))["state"] == "pending"
# This is only a synthetic header, not a RAM image. Restore the fixture's exact
# normal page explicitly before continuing, never through activation recovery.
machine.succeed("dd if=/run/clean-header of=/dev/mapper/resume-swap conv=notrunc status=none; swapon /dev/mapper/resume-swap")
inactive_image()

# Manual wake before the timer: the prepared boot record alone is not a saved
# session, and finishing it must not require cancellation or cold boot.
machine.succeed("mkdir -p /etc/systemd/sleep.conf.d; printf '[Sleep]\\nHibernateDelaySec=120s\\n' > /etc/systemd/sleep.conf.d/90-zbm-test.conf")
machine.succeed("grep -q '\\[s2idle\\]' /sys/power/mem_sleep")
machine.execute("systemctl --no-block suspend-then-hibernate", check_return=False)
wake_from_suspend(2)
machine.wait_for_console_text("PM: suspend exit", timeout=30)
finished()

# Fail the real hibernate phase. Native systemd should return to suspend; after
# an explicit wake its normal finish hook must leave no invented pending image.
machine.succeed("printf '[Sleep]\\nHibernateDelaySec=5s\\n' > /etc/systemd/sleep.conf.d/90-zbm-test.conf; touch /run/zbm-test-fail-hibernate")
machine.execute("systemctl --no-block suspend-then-hibernate", check_return=False)
wake_from_suspend(8)
machine.wait_for_console_text("Couldn't hibernate, will try to suspend again", timeout=60)
wake_from_suspend(2)
machine.wait_for_console_text("PM: suspend exit", timeout=30)
finished()
machine.succeed("rm /run/zbm-test-fail-hibernate /etc/systemd/sleep.conf.d/90-zbm-test.conf")
