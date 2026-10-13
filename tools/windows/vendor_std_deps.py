#!/usr/bin/env python3
"""Build a cargo directory source for the standard library's crates.io dependencies:
real sources (git clones at the published tags) for the crates a Windows std build compiles,
manifest-only stubs for the rest (they only need to exist for dependency resolution)."""
import json, os, re, shutil, subprocess, sys

WORK = os.environ.get("CURUST_WIN_WORK", "/tmp/curust-win")
LIB = os.path.join(WORK, "rustsrc/library")
OUT = os.path.join(WORK, "vendor")
CLONES = os.path.join(WORK, "clones")

# Versions/tags as locked in library/Cargo.lock of rustc 1.97.0; adjust for other compilers.
REAL = {
    "cfg-if": ("rust-lang/cfg-if", "v1.0.4", ""),
    "hashbrown": ("rust-lang/hashbrown", "v0.17.1", ""),
    "rustc-demangle": ("rust-lang/rustc-demangle", "rustc-demangle-v0.1.27", ""),
    "libc": ("rust-lang/libc", "0.2.185", ""),
    "rustc-literal-escaper": ("rust-lang/literal-escaper", "v0.0.7", ""),
}

lock = open(os.path.join(LIB, "Cargo.lock")).read()
packages = []
for block in lock.split("[[package]]")[1:]:
    name = re.search(r'name = "([^"]+)"', block).group(1)
    version = re.search(r'version = "([^"]+)"', block).group(1)
    src = re.search(r'source = "([^"]+)"', block)
    ck = re.search(r'checksum = "([^"]+)"', block)
    deps = re.search(r'dependencies = \[(.*?)\]', block, re.S)
    deps = re.findall(r'"([^"]+)"', deps.group(1)) if deps else []
    if src and src.group(1).startswith("registry"):
        packages.append((name, version, ck.group(1), deps))

# Every feature any manifest requests of a dependency, so stubs accept them all.
os.makedirs(CLONES, exist_ok=True)
for name, (repo, tag, sub) in REAL.items():
    c = os.path.join(CLONES, name)
    if not os.path.exists(c):
        subprocess.run(["git", "clone", "-q", "-c", "advice.detachedHead=false", "--depth", "1", "--branch", tag, f"https://github.com/{repo}", c], check=True)
feats = {}
for root, _, files in [w for top in (LIB, CLONES) for w in os.walk(top)]:
    for f in files:
        if f == "Cargo.toml":
            t = open(os.path.join(root, f)).read()
            for m in re.finditer(r'^\s*([A-Za-z0-9_-]+)\s*=\s*\{[^}]*features\s*=\s*\[([^\]]*)\]', t, re.M | re.S):
                feats.setdefault(m.group(1), set()).update(re.findall(r"['\"]([^'\"]+)['\"]", m.group(2)))
            for m in re.finditer(r"['\"]([A-Za-z0-9_-]+)\??/([A-Za-z0-9_-]+)['\"]", t):
                feats.setdefault(m.group(1), set()).add(m.group(2))

shutil.rmtree(OUT, ignore_errors=True)
os.makedirs(OUT)
os.makedirs(CLONES, exist_ok=True)
for name, version, ck, deps in packages:
    d = os.path.join(OUT, f"{name}-{version}")
    if name in REAL:
        repo, tag, sub = REAL[name]
        c = os.path.join(CLONES, name)
        if not os.path.exists(c):
            subprocess.run(["git", "clone", "-q", "--depth", "1", "--branch", tag, f"https://github.com/{repo}", c], check=True)
        shutil.copytree(os.path.join(c, sub), d, ignore=shutil.ignore_patterns(".git"))
        mp = os.path.join(d, "Cargo.toml")
        # Published manifests carry no [workspace] table.
        text = open(mp).read()
        open(mp, "w").write(re.sub(r'(?ms)^\[workspace\]\n.*?(?=^\[(?!workspace)|\Z)', '', text))
        kind = "real"
    else:
        os.makedirs(os.path.join(d, "src"))
        open(os.path.join(d, "src", "lib.rs"), "w").write("#![no_std]\n")
        lines = [f'[package]\nname = "{name}"\nversion = "{version}"\nedition = "2015"\n\n[lib]\npath = "src/lib.rs"\n\n[dependencies]\n']
        lines.append("\n[features]\n")
        for f in sorted(feats.get(name, set()) | {"default", "std", "alloc", "core", "compiler_builtins", "rustc-dep-of-std"}):
            lines.append(f'"{f}" = []\n')
        open(os.path.join(d, "Cargo.toml"), "w").write("".join(lines))
        kind = "stub"
    json.dump({"files": {}, "package": ck}, open(os.path.join(d, ".cargo-checksum.json"), "w"))
    print(kind, name, version)
