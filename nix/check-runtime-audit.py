"""A glibc executable must be rejected when staging a whole-musl image."""
import json
from pathlib import Path
import shutil
import subprocess
import tempfile

script = Path(__file__).with_name("stage-runtime.py").resolve()
with tempfile.TemporaryDirectory(prefix="zbm-libc-audit-") as folder:
    root = Path(folder)
    candidate = shutil.which("true")
    assert candidate, "ELF test fixture 'true' is missing"
    interpreter = subprocess.check_output(["patchelf", "--print-interpreter", candidate], text=True)
    assert "ld-linux" in interpreter, "Run this negative test on a glibc build host"
    specification = root / "tools.json"
    specification.write_text(json.dumps([{"source": candidate}]))
    result = subprocess.run(["uv", "run", "--no-project", "--offline", "python", str(script),
        str(root / "image"), str(specification), "musl"], cwd=root, capture_output=True, text=True)
    assert result.returncode != 0, "Wrong-libc ELF was accepted"
    assert "Non-musl ELF" in result.stderr, result.stderr
print("Wrong-libc runtime ELF rejection passed")
