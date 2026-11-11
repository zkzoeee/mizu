#!/usr/bin/env python3
"""Identity of a CUDA build, so a volume's cached binaries can be reused.

A cached binary is only reusable if it would be byte-for-byte pointless to
rebuild it. That needs more than "a file exists": the GPU architecture it was
compiled for, the CUDA toolkit and OS it links against, the Rust toolchain and
every source file that feeds the build all have to match. Each goes into this
key, so a miss can say which component moved rather than just "miss".

Reads nothing but the filesystem; prints JSON. With --compare it also explains
a mismatch on stderr and exits 1, which is what the pod driver branches on.

Usage:
  cache-key.py --root /workspace/mizu --archs 86 [--toolchain 1.92.0]
               [--write KEY.json] [--compare KEY.json]
"""
import argparse
import hashlib
import json
import os
import subprocess
import sys

# Only what the build actually consumes. `vendor/sp1/sp1-gpu` is deliberately
# absent: the CUDA crates come from crates.io at pinned versions recorded in
# Cargo.lock, which is hashed, so the fork's own copy cannot affect the build.
SOURCES = [
    "tools/pasta-proof/Cargo.toml",
    "tools/pasta-proof/Cargo.lock",
    "tools/pasta-proof/src",
    "vendor/sp1/Cargo.toml",
    "vendor/sp1/crates",
]
SKIP_DIRS = {"target", ".git", "book", "node_modules", "__pycache__"}

# What decides whether an ALREADY-BUILT binary is the right one and will run
# here. `nvcc`, `rustc` and `toolchain` are build identity: they determined what
# the binary is, and they are recorded in the stored key for that reason, but a
# path that compiles nothing does not need them installed to use the result. A
# slim CUDA image has no nvcc at all, so keying a prove-only run on it would
# force a 10 GB toolchain image to re-derive a fact already written down.
# `os` stays: it is the glibc the binary is linked against.
RUNTIME_COMPONENTS = ("os", "cuda_archs", "source_fingerprint")


def file_sha(path):
    h = hashlib.sha256()
    with open(path, "rb") as fh:
        for chunk in iter(lambda: fh.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def source_fingerprint(root):
    """One hash over (relative path, file hash) for every build input."""
    entries = []
    for rel in SOURCES:
        base = os.path.join(root, rel)
        if os.path.isfile(base):
            entries.append((rel, file_sha(base)))
            continue
        if not os.path.isdir(base):
            raise SystemExit(f"missing build input: {base}")
        for dirpath, dirnames, filenames in os.walk(base):
            dirnames[:] = sorted(d for d in dirnames if d not in SKIP_DIRS)
            for name in sorted(filenames):
                if name == ".DS_Store" or name.startswith("._"):
                    continue
                full = os.path.join(dirpath, name)
                if os.path.islink(full) or not os.path.isfile(full):
                    continue
                entries.append((os.path.relpath(full, root), file_sha(full)))
    entries.sort()
    h = hashlib.sha256()
    for rel, digest in entries:
        h.update(rel.encode())
        h.update(b"\0")
        h.update(digest.encode())
        h.update(b"\n")
    return h.hexdigest(), len(entries)


def run(*argv):
    try:
        out = subprocess.run(argv, capture_output=True, text=True, timeout=60)
    except (OSError, subprocess.SubprocessError):
        return "unknown"
    return (out.stdout + out.stderr).strip() or "unknown"


def os_release():
    try:
        with open("/etc/os-release") as fh:
            kv = dict(
                line.rstrip("\n").split("=", 1)
                for line in fh
                if "=" in line and not line.startswith("#")
            )
    except OSError:
        return "unknown"
    return f"{kv.get('ID', '?').strip(chr(34))}-{kv.get('VERSION_ID', '?').strip(chr(34))}"


def nvcc_release():
    text = run("nvcc", "--version")
    for token in text.replace(",", " ").split():
        if token.count(".") == 1 and token[0].isdigit():
            return token
    return "unknown"


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--root", help="unpacked bundle root; omit with --source-fingerprint")
    p.add_argument("--archs", help="CUDA_ARCHS this build targets")
    p.add_argument("--toolchain", default=os.environ.get("RUSTUP_TOOLCHAIN", "unknown"))
    p.add_argument("--write")
    p.add_argument("--compare")
    p.add_argument(
        "--fingerprint-only",
        action="store_true",
        help="print just the source fingerprint and exit. It is the only "
        "component that needs the sources, so a launcher can compute it at home "
        "and ask the pod whether it already holds that build before shipping a "
        "72 MB bundle the pod may not need.",
    )
    p.add_argument(
        "--runtime",
        action="store_true",
        help="compare only the components that matter to a prebuilt binary "
        f"({', '.join(RUNTIME_COMPONENTS)}), ignoring the build toolchain. For "
        "a prove-only run on an image that has no compiler.",
    )
    p.add_argument(
        "--source-fingerprint",
        help="use this instead of hashing --root, so the key can be computed on "
        "a pod that never received the sources",
    )
    args = p.parse_args()

    if args.fingerprint_only:
        if not args.root:
            raise SystemExit("--fingerprint-only needs --root")
        print(source_fingerprint(args.root)[0])
        return 0

    if not args.archs:
        raise SystemExit("--archs is required")
    if args.source_fingerprint:
        fingerprint, counted = args.source_fingerprint, -1
    elif args.root:
        fingerprint, counted = source_fingerprint(args.root)
    else:
        raise SystemExit("either --root or --source-fingerprint is required")
    components = {
        "os": os_release(),
        "nvcc": nvcc_release(),
        "rustc": run("rustc", "--version"),
        "cuda_archs": args.archs,
        "toolchain": args.toolchain,
        "source_fingerprint": fingerprint,
    }
    key = hashlib.sha256(
        json.dumps(components, sort_keys=True).encode()
    ).hexdigest()
    record = {"key": key, "files_hashed": counted, "components": components}
    if args.runtime:
        record["scope"] = "runtime"
        record["compared_components"] = list(RUNTIME_COMPONENTS)

    if args.write:
        os.makedirs(os.path.dirname(os.path.abspath(args.write)), exist_ok=True)
        with open(args.write, "w") as fh:
            json.dump(record, fh, indent=2, sort_keys=True)
            fh.write("\n")

    print(json.dumps(record, indent=2, sort_keys=True))

    if args.compare:
        try:
            with open(args.compare) as fh:
                stored = json.load(fh)
        except (OSError, ValueError) as error:
            print(f"cache miss: no usable key at {args.compare} ({error})", file=sys.stderr)
            return 1
        old = stored.get("components", {})
        if args.runtime:
            ignored = [n for n in sorted(components) if n not in RUNTIME_COMPONENTS]
            print(
                "runtime scope: requiring "
                + ", ".join(RUNTIME_COMPONENTS)
                + "; ignoring "
                + ", ".join(ignored)
                + " because this path compiles nothing",
                file=sys.stderr,
            )
            moved = [
                n for n in RUNTIME_COMPONENTS
                if old.get(n, "<absent>") != components[n]
            ]
            for name in RUNTIME_COMPONENTS:
                was, now = old.get(name, "<absent>"), components[name]
                mark = "   same" if was == now else "CHANGED"
                print(f"  {mark} {name}: {was} -> {now}", file=sys.stderr)
            if moved:
                print("cache miss in runtime scope", file=sys.stderr)
                return 1
            print(
                f"cache hit (runtime scope); the build's own key is {stored.get('key')}",
                file=sys.stderr,
            )
            return 0
        if stored.get("key") == key:
            print(f"cache hit: {key}", file=sys.stderr)
            return 0
        print("cache miss, by component:", file=sys.stderr)
        for name in sorted(components):
            was, now = old.get(name, "<absent>"), components[name]
            mark = "   same" if was == now else "CHANGED"
            print(f"  {mark} {name}: {was} -> {now}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
