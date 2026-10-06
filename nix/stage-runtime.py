"""Select ELF dependencies from explicit Nix paths, with no host library fallback."""
import json
from pathlib import Path
import shutil
import subprocess
import sys
root, specification = Path(sys.argv[1]), json.loads(Path(sys.argv[2]).read_text())
expected_libc = sys.argv[3] if len(sys.argv) > 3 else None
records = []
seen = set()
known = {}
def query(path, option):
    return subprocess.check_output(["patchelf", option, str(path)], text=True, stderr=subprocess.DEVNULL).strip()
def copy(source, target=None, inherited=()):
    source = Path(source)
    real = source.resolve(strict=True)
    known[source.name] = real
    known[real.name] = real
    destination = root / str(target or source).lstrip("/")
    destination.parent.mkdir(parents=True, exist_ok=True)
    if source.is_symlink() and target is None:
        if not destination.exists(): destination.symlink_to(str(real))
        copy(real, inherited=inherited)
        return
    if not destination.exists(): shutil.copy2(real, destination)
    if real in seen: return
    seen.add(real)
    if real.read_bytes()[:4] != b"\x7fELF": return
    try: interpreter = query(real, "--print-interpreter")
    except subprocess.CalledProcessError: interpreter = ""
    if expected_libc == "musl":
        if "glibc" in str(real) or (interpreter and "ld-musl" not in interpreter):
            raise RuntimeError(f"Non-musl ELF in musl userspace: {real}, interpreter={interpreter}")
    if interpreter:
        copy(interpreter)
        inherited = (*inherited, Path(interpreter).parent)
    try:
        search = [Path(p.replace("$ORIGIN", str(real.parent))) for p in query(real, "--print-rpath").split(":") if p] + list(inherited)
        needed = query(real, "--print-needed").splitlines()
    except subprocess.CalledProcessError:
        records.append({"source": str(real), "interpreter": interpreter, "needed": [], "static": True})
        return  # statically linked ELF
    if expected_libc == "musl" and "libc.so.6" in needed:
        raise RuntimeError(f"glibc dependency in musl userspace: {real}")
    records.append({"source": str(real), "interpreter": interpreter, "needed": needed, "static": not interpreter and not needed})
    for name in needed:
        candidate = next((p / name for p in search if (p / name).exists()), None)
        if candidate is None: candidate = known.get(name)
        if candidate is None: raise RuntimeError(f"Unresolved ELF dependency {name} of {real}")
        copy(candidate, inherited=search)
for item in specification:
    copy(item["source"], item.get("target"))
    if item.get("link"):
        link = root / item["link"].lstrip("/")
        link.parent.mkdir(parents=True, exist_ok=True)
        link.symlink_to(item["source"])

Path("runtime-audit.json").write_text(json.dumps({"libc": expected_libc, "elfs": records}, indent=2) + "\n")
