#!/usr/bin/env python3
"""Zcash dependency-version policy checker for zcash/wallet.

The source of truth for a governed library's version is the library's own
upstream repository at its default-branch HEAD. A consumer (this repo) can
depend on a governed library in one of two ways, and each is checked
accordingly:

  * Registry version pin (e.g. ``orchard = "0.14"`` in
    ``[workspace.dependencies]``): compared against the library's
    ``[package] version`` on its ``ref`` branch.

  * Git pin via ``[patch.crates-io]`` (e.g. the librustzcash crates pinned to
    a fixed ``rev``): the pinned commit is compared against the branch HEAD
    commit, so a pin that has fallen behind ``main`` is reported as drift.

Subcommands:

  check            Report drift for every governed library. Warn-by-default;
                   set ``severity = "error"`` on a library to make it block.

  rewrite-to-head  Repoint the governed ``[patch.crates-io]`` git pins to
                   ``branch = "<ref>"`` so a follow-up ``cargo check`` builds
                   the workspace against upstream HEAD (early-warning job).

Stdlib only (tomllib + urllib + the ``git`` CLI for ``ls-remote``); no
third-party Python packages, so it runs with a bare ``python3`` on a stock
GitHub runner.
"""

from __future__ import annotations

import argparse
import re
import subprocess
import sys
import tomllib
import urllib.error
import urllib.request
from dataclasses import dataclass

RAW = "https://raw.githubusercontent.com/{repo}/{ref}/{manifest}"
VERSION_RE = re.compile(r"\d+(?:\.\d+)*")


@dataclass
class Library:
    crate: str
    repo: str  # owner/name slug on github.com
    ref: str = "main"
    manifest: str = "Cargo.toml"
    severity: str = "warn"  # "warn" or "error"


@dataclass
class Declared:
    key: str
    package: str
    req: str | None
    kind: str  # version | git | path | workspace | unknown
    spec: dict | str


@dataclass
class Patch:
    key: str
    package: str
    repo: str  # normalized owner/name
    pin_kind: str  # rev | tag | branch
    pin: str


@dataclass
class Result:
    lib: Library
    status: str  # OK | MISMATCH | REV_OK | REV_BEHIND | MISSING | ERROR
    via: str = ""  # "version" | "patch"
    pinned: str = ""
    canonical: str = ""
    detail: str = ""
    severity: str = "warn"


# --------------------------------------------------------------------------
# Helpers
# --------------------------------------------------------------------------
def load_toml(path: str) -> dict:
    with open(path, "rb") as fh:
        return tomllib.load(fh)


def normalize_repo(url_or_slug: str) -> str:
    s = url_or_slug.strip()
    s = re.sub(r"^https?://github\.com/", "", s)
    s = re.sub(r"^git@github\.com:", "", s)
    s = re.sub(r"\.git$", "", s)
    return s.strip("/")


def table(data: dict, dotted: str) -> dict:
    node: object = data
    for part in dotted.split("."):
        if not isinstance(node, dict):
            return {}
        node = node.get(part, {})
    return node if isinstance(node, dict) else {}


def version_tuple(s: str) -> tuple[int, ...]:
    m = VERSION_RE.search(s)
    return tuple(int(p) for p in m.group(0).split(".")) if m else ()


def same_version(req: str, canonical: str) -> bool:
    rt, ct = version_tuple(req), version_tuple(canonical)
    if not rt or not ct:
        return False
    n = min(len(rt), len(ct))
    return rt[:n] == ct[:n] and len(rt) <= len(ct)


# --------------------------------------------------------------------------
# Consumer manifest parsing
# --------------------------------------------------------------------------
def parse_declared(data: dict, tables: list[str]) -> dict[str, Declared]:
    out: dict[str, Declared] = {}
    for dotted in tables:
        for key, val in table(data, dotted).items():
            if isinstance(val, str):
                d = Declared(key, key, val, "version", val)
            elif isinstance(val, dict):
                package = val.get("package", key)
                if "git" in val:
                    kind, req = "git", None
                elif "path" in val:
                    kind, req = "path", None
                elif val.get("workspace"):
                    kind, req = "workspace", None
                elif "version" in val:
                    kind, req = "version", val["version"]
                else:
                    kind, req = "unknown", None
                d = Declared(key, package, req, kind, val)
            else:
                continue
            out.setdefault(d.package, d)
    return out


def parse_patches(data: dict) -> dict[str, Patch]:
    out: dict[str, Patch] = {}
    for key, val in table(data, "patch.crates-io").items():
        if not isinstance(val, dict) or "git" not in val:
            continue
        package = val.get("package", key)
        for pin_kind in ("rev", "tag", "branch"):
            if pin_kind in val:
                out[package] = Patch(key, package, normalize_repo(val["git"]),
                                     pin_kind, val[pin_kind])
                break
    return out


# --------------------------------------------------------------------------
# Upstream resolution (cached)
# --------------------------------------------------------------------------
_ver_cache: dict[str, str] = {}
_sha_cache: dict[str, str] = {}


def canonical_version(lib: Library) -> str:
    url = RAW.format(repo=lib.repo, ref=lib.ref, manifest=lib.manifest)
    if url not in _ver_cache:
        with urllib.request.urlopen(url, timeout=30) as resp:  # noqa: S310
            data = tomllib.loads(resp.read().decode("utf-8"))
        _ver_cache[url] = data["package"]["version"]
    return _ver_cache[url]


def head_sha(repo: str, ref: str) -> str:
    cache_key = f"{repo}@{ref}"
    if cache_key not in _sha_cache:
        url = f"https://github.com/{repo}.git"
        proc = subprocess.run(
            ["git", "ls-remote", url, f"refs/heads/{ref}"],
            capture_output=True, text=True, timeout=60, check=True)
        line = proc.stdout.strip().split("\n")[0]
        if not line:
            raise ValueError(f"no ref refs/heads/{ref} in {repo}")
        _sha_cache[cache_key] = line.split()[0]
    return _sha_cache[cache_key]


# --------------------------------------------------------------------------
# check
# --------------------------------------------------------------------------
def evaluate(lib: Library, declared: dict[str, Declared],
             patches: dict[str, Patch], allow: set[str]) -> Result:
    sev = "warn" if lib.crate in allow else lib.severity
    patch = patches.get(lib.crate)

    if patch and normalize_repo(patch.repo) == normalize_repo(lib.repo):
        try:
            sha = head_sha(lib.repo, lib.ref)
        except (subprocess.SubprocessError, ValueError) as exc:
            return Result(lib, "ERROR", "patch", patch.pin, severity="warn",
                          detail=f"ls-remote failed: {exc}")
        if patch.pin_kind == "branch":
            ok = patch.pin == lib.ref
            return Result(lib, "REV_OK" if ok else "REV_BEHIND", "patch",
                          f"branch={patch.pin}", sha[:10], severity=sev,
                          detail="" if ok else f"tracks {patch.pin}, not {lib.ref}")
        n = min(len(patch.pin), len(sha))
        ok = patch.pin[:n] == sha[:n]
        return Result(lib, "REV_OK" if ok else "REV_BEHIND", "patch",
                      patch.pin[:10], sha[:10], severity=sev)

    dep = declared.get(lib.crate)
    if dep is None:
        return Result(lib, "MISSING", severity="warn",
                      detail="not a declared dependency or patch")
    if dep.kind != "version":
        return Result(lib, "MISSING", "version", severity="warn",
                      detail=f"{dep.kind} dependency, not a version pin")
    try:
        canon = canonical_version(lib)
    except (urllib.error.URLError, KeyError, ValueError) as exc:
        return Result(lib, "ERROR", "version", dep.req or "", severity="warn",
                      detail=str(exc))
    ok = same_version(dep.req or "", canon)
    return Result(lib, "OK" if ok else "MISMATCH", "version",
                  dep.req or "", canon, severity=sev)


def run_check(libs: list[Library], data: dict, tables: list[str],
              allow: set[str]) -> int:
    declared = parse_declared(data, tables)
    patches = parse_patches(data)
    results = [evaluate(lib, declared, patches, allow) for lib in libs]

    drift = {"MISMATCH", "REV_BEHIND"}
    width = max((len(r.lib.crate) for r in results), default=10)
    print("\nZcash dependency version policy")
    print("(truth: each library's upstream HEAD; version pins vs [package] "
          "version, git pins vs branch HEAD commit)\n")
    print(f"  {'STATUS':<11}{'VIA':<9}{'CRATE':<{width}}  PINNED -> UPSTREAM HEAD")
    print(f"  {'-'*11}{'-'*9}{'-'*width}  {'-'*26}")

    failures = 0
    for r in sorted(results, key=lambda x: (x.status not in drift, x.lib.crate)):
        arrow = f"{r.pinned or '-'} -> {r.canonical or '-'}"
        note = f"  ({r.detail})" if r.detail else ""
        print(f"  {r.status:<11}{r.via:<9}{r.lib.crate:<{width}}  {arrow}{note}")
        if r.status in drift:
            msg = (f"{r.lib.crate}: {r.via} pin {r.pinned} is behind "
                   f"{r.lib.repo}@{r.lib.ref} ({r.canonical})")
            loc = f"file=Cargo.toml"
            if r.severity == "error" and r.lib.crate not in allow:
                print(f"::error {loc}::{msg}")
                failures += 1
            else:
                tag = " [allowlisted]" if r.lib.crate in allow else ""
                print(f"::warning {loc}::{msg}{tag}")
        elif r.status == "ERROR":
            print(f"::warning file=Cargo.toml::{r.lib.crate}: {r.detail}")

    insync = sum(1 for r in results if r.status in ("OK", "REV_OK"))
    drifted = sum(1 for r in results if r.status in drift)
    print(f"\n  {insync} in sync, {drifted} drifted, {failures} blocking\n")
    return 1 if failures else 0


# --------------------------------------------------------------------------
# rewrite-to-head: repoint governed patch.crates-io git pins to branch HEAD
# --------------------------------------------------------------------------
def section_span(text: str, header: str) -> tuple[int, int] | None:
    m = re.search(rf"(?m)^\[{re.escape(header)}\]\s*$", text)
    if not m:
        return None
    start = m.end()
    nxt = re.search(r"(?m)^\[", text[start:])
    end = start + nxt.start() if nxt else len(text)
    return start, end


def find_value_span(text: str, key: str, base: int = 0) -> tuple[int, int] | None:
    m = re.search(rf"(?m)^\s*{re.escape(key)}\s*=\s*", text[base:])
    if not m:
        return None
    i = base + m.end()
    if i < len(text) and text[i] == "{":
        depth, j, in_str = 0, i, False
        while j < len(text):
            c = text[j]
            if in_str:
                if c == '"':
                    in_str = False
            elif c == '"':
                in_str = True
            elif c == "{":
                depth += 1
            elif c == "}":
                depth -= 1
                if depth == 0:
                    return base + m.start(), j + 1
            j += 1
    return None


def inline_table(parts: dict) -> str:
    items = []
    for k, v in parts.items():
        if isinstance(v, bool):
            items.append(f"{k} = {'true' if v else 'false'}")
        elif isinstance(v, list):
            arr = ", ".join(f'"{x}"' for x in v)
            items.append(f"{k} = [{arr}]")
        else:
            items.append(f'{k} = "{v}"')
    return "{ " + ", ".join(items) + " }"


def run_rewrite(libs: list[Library], data: dict, manifest_path: str,
                tables: list[str], rewrite_deps: bool) -> int:
    """Point each governed library's *effective* source at its branch HEAD.

    A library pinned through ``[patch.crates-io]`` (wallet's librustzcash
    crates) has its patch entry repointed from a fixed ``rev`` to
    ``branch = "<ref>"``. A library expressed only as a registry version pin is
    rewritten in place to a git dependency, but only when ``rewrite_deps`` is
    set, so a repo that deliberately tracks released crates is left untouched.
    """
    governed_repos = {normalize_repo(lib.repo): lib.ref for lib in libs}
    with open(manifest_path, encoding="utf-8") as fh:
        text = fh.read()

    edits: list[tuple[int, int, str]] = []
    touched: list[str] = []
    patched_repos: set[str] = set()

    # (1) Repoint governed [patch.crates-io] git pins to branch HEAD.
    span = section_span(text, "patch.crates-io")
    if span is not None:
        sec_start, sec_end = span
        for key, val in table(data, "patch.crates-io").items():
            if not isinstance(val, dict) or "git" not in val:
                continue
            repo = normalize_repo(val["git"])
            if repo not in governed_repos:
                continue
            vspan = find_value_span(text, key, base=sec_start)
            if vspan is None or vspan[0] >= sec_end:
                continue
            parts: dict = {"git": val["git"]}
            if "package" in val:
                parts["package"] = val["package"]
            parts["branch"] = governed_repos[repo]
            edits.append((vspan[0], vspan[1], f"{key} = {inline_table(parts)}"))
            touched.append(key)
            patched_repos.add(repo)

    # (2) Optionally rewrite governed registry version pins to git deps, but
    #     not for repos already handled by a patch entry above.
    if rewrite_deps:
        declared = parse_declared(data, tables)
        for lib in libs:
            if normalize_repo(lib.repo) in patched_repos:
                continue
            dep = declared.get(lib.crate)
            if dep is None or dep.kind != "version":
                continue
            vspan = find_value_span(text, dep.key)
            if vspan is None:
                continue
            parts = {"git": f"https://github.com/{lib.repo}",
                     "branch": lib.ref}
            if dep.key != dep.package:
                parts["package"] = dep.package
            if isinstance(dep.spec, dict):
                if "default-features" in dep.spec:
                    parts["default-features"] = dep.spec["default-features"]
                if "features" in dep.spec:
                    parts["features"] = dep.spec["features"]
            edits.append((vspan[0], vspan[1],
                          f"{dep.key} = {inline_table(parts)}"))
            touched.append(dep.key)

    for start, end, repl in sorted(edits, reverse=True):
        text = text[:start] + repl + text[end:]

    with open(manifest_path, "w", encoding="utf-8") as fh:
        fh.write(text)

    if touched:
        print(f"Repointed {len(touched)} governed crate(s) to branch HEAD: "
              f"{', '.join(sorted(set(touched)))}")
    else:
        print("No governed pins matched; nothing repointed.")
    return 0


# --------------------------------------------------------------------------
# entry
# --------------------------------------------------------------------------
def load_config(path: str):
    data = load_toml(path)
    cfg = {
        "manifest": data.get("consumer_manifest", "Cargo.toml"),
        "tables": data.get("consumer_tables", ["dependencies"]),
        "rewrite_deps": data.get("head_rewrite_deps", True),
    }
    libs = [
        Library(
            crate=e["crate"], repo=e["repo"], ref=e.get("ref", "main"),
            manifest=e.get("manifest", "Cargo.toml"),
            severity=e.get("severity", "warn"),
        )
        for e in data.get("library", [])
    ]
    return libs, cfg


def load_allowlist(path: str | None) -> set[str]:
    if not path:
        return set()
    try:
        data = load_toml(path)
    except FileNotFoundError:
        return set()
    return {e["crate"] for e in data.get("allow", []) if "crate" in e}


def main(argv: list[str]) -> int:
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("command", choices=["check", "rewrite-to-head"])
    p.add_argument("--config", default=".github/dep-policy/governed-libs.toml")
    p.add_argument("--manifest", help="override consumer manifest path")
    p.add_argument("--allow", default=".github/dep-policy/allow.toml")
    args = p.parse_args(argv)

    libs, cfg = load_config(args.config)
    manifest = args.manifest or cfg["manifest"]
    data = load_toml(manifest)

    if args.command == "check":
        return run_check(libs, data, cfg["tables"], load_allowlist(args.allow))
    return run_rewrite(libs, data, manifest, cfg["tables"], cfg["rewrite_deps"])


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
