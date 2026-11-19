"""Prepare a pinned, project-local TDLib C ABI spike; no Telegram credentials."""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tarfile
import urllib.request

ROOT = Path(__file__).resolve().parents[2]
WORK = (ROOT / "target/mtproto-spike").resolve()
TD_COMMIT = "03220f6b3043c6fd7785fbf0901d63f19ad19974"
PACKAGES = {
    "mingw-w64-ucrt-x86_64-openssl-3.6.5-1-any.pkg.tar.zst": "773e021ebad83f14b5c103db467411e8ff064663e1c7e7f730e2be72f4117064",
    "mingw-w64-ucrt-x86_64-zlib-1.4-2-any.pkg.tar.zst": "172541cf5e2391a402f092104988699b0163129d22be1d6b4b201bfb84af6fcf",
    "mingw-w64-ucrt-x86_64-gperf-3.3-1-any.pkg.tar.zst": "d502483b5b0229546d245022be86a10af922aabdbdd5b1eae64f2fbcf4101c91",
}


def run(*args, env=None):
    subprocess.run(args, check=True, cwd=ROOT, env=env)


def contained(path):
    resolved = path.resolve()
    if not resolved.is_relative_to(WORK):
        raise RuntimeError("native artifact escaped spike directory")
    return resolved


def verify_tree(path):
    """Do not let existing nested reparse points redirect native build tools."""
    pending = [contained(path)]
    while pending:
        current = pending.pop()
        if not current.exists():
            continue
        with os.scandir(current) as entries:
            for entry in entries:
                child = Path(entry.path)
                if child.is_symlink() or child.is_junction():
                    raise RuntimeError("native artifact tree contains a filesystem link")
                if entry.is_dir(follow_symlinks=False):
                    pending.append(child)


def main():
    if not WORK.is_relative_to(ROOT.resolve() / "target"):
        raise RuntimeError("spike directory escaped repository target")
    WORK.mkdir(parents=True, exist_ok=True)
    downloads = contained(WORK / "downloads")
    downloads.mkdir(exist_ok=True)
    dependencies = contained(WORK / "dependencies")
    dependencies.mkdir(exist_ok=True)
    verify_tree(downloads)
    verify_tree(dependencies)
    for filename, digest in PACKAGES.items():
        archive = contained(downloads / filename)
        if not archive.exists():
            with urllib.request.urlopen(f"https://mirror.msys2.org/mingw/ucrt64/{filename}", timeout=30) as response, archive.open("xb") as output:
                total = 0
                while chunk := response.read(1024 * 1024):
                    total += len(chunk)
                    if total > 128 * 1024 * 1024:
                        raise RuntimeError("native package exceeded download bound")
                    output.write(chunk)
        if not archive.is_file() or archive.stat().st_size > 128 * 1024 * 1024:
            raise RuntimeError("invalid cached native package")
        if hashlib.sha256(archive.read_bytes()).hexdigest() != digest:
            raise RuntimeError(f"SHA-256 mismatch: {filename}")
        with tarfile.open(archive) as contents:
            contents.extractall(dependencies, members=(member for member in contents if member.name.startswith("ucrt64/")), filter="data")
    source = contained(WORK / "tdlib-source")
    verify_tree(source)
    if not source.exists():
        run("git", "init", str(source))
        run("git", "-C", str(source), "remote", "add", "origin", "https://github.com/tdlib/td.git")
        run("git", "-C", str(source), "fetch", "--depth=1", "origin", TD_COMMIT)
        run("git", "-C", str(source), "checkout", "--detach", "FETCH_HEAD")
    actual = subprocess.check_output(["git", "-C", str(source), "rev-parse", "HEAD"], text=True).strip()
    if actual != TD_COMMIT:
        raise RuntimeError("existing TDLib source has a different revision; refusing to replace it")
    run("git", "-C", str(source), "diff", "--exit-code", "HEAD", "--")
    untracked = subprocess.check_output(["git", "-C", str(source), "ls-files", "--others", "--exclude-standard"], text=True)
    if untracked.strip():
        raise RuntimeError("unexpected untracked TDLib source inputs")
    native = dependencies / "ucrt64"
    environment = os.environ.copy()
    environment["PATH"] = str(native / "bin") + os.pathsep + environment["PATH"]
    build = contained(WORK / "tdlib-build")
    verify_tree(build)
    run("cmake", "-S", str(source), "-B", str(build), "-G", "Ninja",
        "-DCMAKE_BUILD_TYPE=Release", "-DCMAKE_CXX_FLAGS_RELEASE=-O1 -DNDEBUG",
        "-DCMAKE_C_FLAGS_RELEASE=-O1 -DNDEBUG", "-DTD_ENABLE_PCH=OFF", "-DTD_ENABLE_LTO=OFF",
        "-DTD_INSTALL_STATIC_LIBRARIES=OFF", f"-DCMAKE_PREFIX_PATH={native}",
        f"-DOPENSSL_ROOT_DIR={native}", "-DOPENSSL_USE_STATIC_LIBS=ON", "-DZLIB_USE_STATIC_LIBS=ON",
        env=environment)
    manifest = {"tdlib_commit": TD_COMMIT, "packages": PACKAGES, "build": str(build), "compiler": "MinGW UCRT C++ from existing PATH", "optimization": "O1 (API spike, not benchmark)"}
    contained(WORK / "native-inputs.json").write_text(json.dumps(manifest, indent=2)+"\n", encoding="utf-8")
    print("Pinned TDLib sources and hash-verified native dependencies configured; no Telegram client started.")


if __name__ == "__main__":
    main()
