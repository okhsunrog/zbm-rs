"""Driver assertions shared by direct and composite encrypted-root VM tests."""
pressure_program = open(pressure_program_path).read()
machine.succeed("printf %s " + shlex.quote(pressure_program) + " > /run/boot-record-pressure.py")
machine.succeed("systemd-run --unit=boot-record-pressure --property=MemoryMax=192M --property=MemorySwapMax=768M "
                "uv run --offline --no-project --python " + python_command + " /run/boot-record-pressure.py")
machine.wait_for_file("/run/boot-record-pressure-ready", timeout=90)
machine.wait_until_succeeds("test $(cat /sys/fs/cgroup/system.slice/boot-record-pressure.service/memory.swap.current) -gt 268435456", timeout=30)
machine.succeed("grep -q /dev/dm- /proc/swaps")

# The restoring initramfs has a separate tracing buffer. Inspect the sleeping
# kernel's buffer to distinguish its hooks from legitimate early activation.
machine.succeed("mountpoint -q /sys/kernel/tracing || mount -t tracefs tracefs /sys/kernel/tracing; "
                "echo 1 > /sys/kernel/tracing/events/syscalls/sys_enter_swapoff/enable; "
                "echo 1 > /sys/kernel/tracing/events/syscalls/sys_enter_swapon/enable; "
                "echo > /sys/kernel/tracing/trace")
trace_probe = "import ctypes; libc = ctypes.CDLL(None); assert libc.swapoff(b'/dev/zbm-absent-trace-probe') == -1; assert libc.swapon(b'/dev/zbm-absent-trace-probe', 0) == -1"
machine.succeed("test ! -e /dev/zbm-absent-trace-probe; uv run --offline --no-project --python "
                + python_command + " python -c " + shlex.quote(trace_probe))
machine.succeed("grep -q swapoff /sys/kernel/tracing/trace; grep -q swapon /sys/kernel/tracing/trace; echo > /sys/kernel/tracing/trace")
pressure_round = 0


def check_live_pressure():
    global pressure_round
    pressure_round += 1
    machine.succeed("test $(cat /sys/fs/cgroup/system.slice/boot-record-pressure.service/memory.swap.current) -gt 268435456")
    machine.succeed("touch /run/boot-record-pressure-check")
    machine.wait_until_succeeds("test $(cat /run/boot-record-pressure-verified 2>/dev/null || echo 0) = " + str(pressure_round), timeout=60)
    machine.succeed("systemctl is-active boot-record-pressure.service; "
                    "! grep -E 'swap(off|on)' /sys/kernel/tracing/trace")
    machine.succeed("dd if=/dev/mapper/resume-swap of=/run/resume-header-direct bs=4096 count=1 iflag=direct status=none; "
                    "dd if=/dev/mapper/resume-swap of=/run/resume-header-cached bs=4096 count=1 status=none; "
                    "cmp /run/resume-header-direct /run/resume-header-cached")
    machine.log("Verified live swap, unchanged anonymous data and no swapoff/swapon in native hooks")


# The old implementation would have to drain the cgroup's swap here.
machine.succeed(record_command, timeout=15)
machine.succeed(finish_command, timeout=15)
check_live_pressure()
