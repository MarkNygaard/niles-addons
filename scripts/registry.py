"""The add-on library's registry: builds index.json, and checks addons.toml.

    python scripts/registry.py build --out site
    python scripts/registry.py added --base origin/main

`build` writes the file Niles reads (the format is in Niles's
`niles_plugins::library` docs): every repo in addons.toml as listed, every
public repo with the topic `niles-addon` as unreviewed, and every
[[removed]] entry. It reads GitHub's public data only, with the workflow's
own token for the API's higher limit.

A repo can hold several add-ons. Each one's releases are tagged with its
id, a `-v` and the version (`pool-v0.2.0`), and each is its own entry,
with `addon` set to that id. Such a file is index version 2; a file without
one is version 1, which every Niles with the library reads.

`added` checks addons.toml's shape and prints, one per line, what a pull
request adds to [[addon]] (`owner/repo`, or `owner/repo/<addon>`), for the
listing check to run `niles plugin check --listing` on.

Standard library only, so the workflows need nothing installed.
"""

from __future__ import annotations

import argparse
import datetime
import json
import os
import re
import subprocess
import sys
import tomllib
import urllib.error
import urllib.request
from pathlib import Path

API = "https://api.github.com"
TOPIC = "niles-addon"
MANIFEST = "niles-integration.toml"
COMPONENT = "integration.wasm"
MARKER = "niles-installed"

# Niles ignores anything bigger, so there is no point carrying it.
MAX_MANIFEST = 64 * 1024
MAX_LOGO = 32 * 1024
MAX_README = 1000
MAX_REASON = 300

REPO = re.compile(r"^[A-Za-z0-9-]{1,39}/[A-Za-z0-9._-]{1,100}$")
# An add-on's id, as Niles's manifest check has it.
ADDON = re.compile(r"^[a-z][a-z0-9_]{1,31}$")
# A release version: major.minor.patch, without a pre-release part.
VERSION = re.compile(r"^(\d+)\.(\d+)\.(\d+)(?:\+[0-9A-Za-z.-]+)?$")
PRE_VERSION = re.compile(r"^\d+\.\d+\.\d+-[0-9A-Za-z.-]+(?:\+[0-9A-Za-z.-]+)?$")


def fail(message: str) -> None:
    print(f"error: {message}", file=sys.stderr)
    sys.exit(1)


# A listed or removed entry: the repo as written, and the add-on in it for a
# repo of several (None for a repo of one).
Key = tuple[str, "str | None"]


def key_of(repo: str, addon: str | None) -> tuple[str, str | None]:
    return repo.lower(), addon


def address(repo: str, addon: str | None) -> str:
    return f"{repo}/{addon}" if addon else repo


# ---------------------------------------------------------------- addons.toml


def read_list(text: str) -> tuple[list[Key], dict[Key, str]]:
    """The listed entries and the removed ones with their reasons, checked."""
    try:
        doc = tomllib.loads(text)
    except tomllib.TOMLDecodeError as e:
        fail(f"addons.toml is not TOML: {e}")
    unknown = set(doc) - {"addon", "removed"}
    if unknown:
        fail(f"addons.toml has tables it doesn't use: {', '.join(sorted(unknown))}")

    seen: dict[tuple[str, str | None], str] = {}

    def entry_of(entry: object, table: str) -> Key:
        if not isinstance(entry, dict) or not isinstance(entry.get("repo"), str):
            fail(f"every [[{table}]] needs repo = \"owner/name\"")
        repo = entry["repo"].strip()
        if not REPO.match(repo):
            fail(f"[[{table}]] repo {repo!r} is not owner/name")
        addon = entry.get("addon")
        if addon is not None and (not isinstance(addon, str) or not ADDON.match(addon)):
            fail(
                f"[[{table}]] {repo}: addon {addon!r} is not an add-on's id "
                "(2 to 32 lowercase letters, digits and _, starting with a letter)"
            )
        key = key_of(repo, addon)
        if key in seen:
            fail(f"{address(repo, addon)} is in addons.toml twice ([[{seen[key]}]] and [[{table}]])")
        seen[key] = table
        return repo, addon

    listed = [entry_of(e, "addon") for e in doc.get("addon", [])]
    removed: dict[Key, str] = {}
    for entry in doc.get("removed", []):
        repo, addon = entry_of(entry, "removed")
        where = address(repo, addon)
        reason = entry.get("reason")
        if not isinstance(reason, str) or not reason.strip():
            fail(f"[[removed]] {where} needs a reason: one sentence the household is shown")
        if len(reason) > MAX_REASON:
            fail(f"[[removed]] {where}'s reason is over {MAX_REASON} characters")
        removed[(repo, addon)] = reason.strip()

    # A repo is listed whole or by its add-ons, not both; and a repo
    # removed whole has none of its add-ons listed.
    whole = {repo.lower() for repo, addon in listed if addon is None}
    gone = {repo.lower() for repo, addon in removed if addon is None}
    for repo, addon in listed:
        if addon is not None and repo.lower() in whole:
            fail(f"{repo} is listed whole and as {address(repo, addon)}; list one or the other")
        if addon is not None and repo.lower() in gone:
            fail(f"{repo} is in [[removed]] whole, so {address(repo, addon)} can't be listed")
    return listed, removed


# ------------------------------------------------------------------- releases


def addon_of_tag(tag: str) -> str | None:
    """The add-on a tag like `pool-v1.2.0` belongs to; None for `v1.2.0`."""
    name, dash, rest = tag.partition("-")
    if not dash or not rest.startswith("v") or not ADDON.match(name):
        return None
    version = rest[1:]
    return name if VERSION.match(version) or PRE_VERSION.match(version) else None


def releases_of(releases: list[dict], addon: str | None) -> list[dict]:
    """`addon`'s releases, or for a repo of one add-on, those not tagged for one."""
    return [r for r in releases if addon_of_tag(r.get("tag_name") or "") == addon]


def addons_in(releases: list[dict]) -> list[str]:
    """The add-ons a repo's releases are tagged for, sorted."""
    return sorted({a for r in releases if (a := addon_of_tag(r.get("tag_name") or ""))})


def latest(releases: list[dict], addon: str | None) -> dict | None:
    """The release Niles installs: GitHub's latest for a repo of one add-on
    (the newest that isn't a pre-release, as the API lists them), and for
    one of several, its highest version that isn't a pre-release."""
    published = [r for r in releases if not r.get("prerelease")]
    if addon is None:
        return published[0] if published else None
    versioned = []
    for r in published:
        match = VERSION.match(r["tag_name"][len(addon) + 2 :])
        if match:
            versioned.append((tuple(int(n) for n in match.groups()), r))
    return max(versioned, key=lambda pair: pair[0])[1] if versioned else None


def counted(releases: list[dict], name: str) -> int | None:
    counts = [
        a.get("download_count", 0)
        for r in releases
        for a in r.get("assets", [])
        if a.get("name") == name
    ]
    return sum(counts) if counts else None


# --------------------------------------------------------------------- GitHub


class GitHub:
    def __init__(self, token: str | None) -> None:
        self.token = token
        self.calls = 0

    def _open(self, url: str, accept: str) -> bytes | None:
        headers = {
            "Accept": accept,
            "User-Agent": "niles-addons-registry",
            "X-GitHub-Api-Version": "2022-11-28",
        }
        if self.token and url.startswith(API):
            headers["Authorization"] = f"Bearer {self.token}"
        request = urllib.request.Request(url, headers=headers)
        self.calls += 1
        try:
            with urllib.request.urlopen(request, timeout=30) as response:
                return response.read()
        except urllib.error.HTTPError as e:
            if e.code == 404:
                return None
            raise

    def json(self, path: str) -> object:
        body = self._open(f"{API}{path}", "application/vnd.github+json")
        return None if body is None else json.loads(body)

    def raw(self, path: str) -> str | None:
        body = self._open(f"{API}{path}", "application/vnd.github.raw")
        return None if body is None else body.decode("utf-8", errors="replace")

    def download(self, url: str, cap: int) -> bytes | None:
        """A release asset, or None when it is missing or over `cap`."""
        body = self._open(url, "application/octet-stream")
        if body is None or len(body) > cap:
            return None
        return body

    def topic_repos(self) -> list[str]:
        """Public repos with the topic; the search API stops at 1,000."""
        found: list[str] = []
        for page in range(1, 11):
            result = self.json(
                f"/search/repositories?q=topic:{TOPIC}+is:public&per_page=100&page={page}"
            )
            items = (result or {}).get("items", [])
            found += [item["full_name"] for item in items if not item.get("private")]
            if len(items) < 100:
                break
        return found

    def releases(self, repo: str) -> list[dict]:
        out: list[dict] = []
        for page in range(1, 11):
            batch = self.json(f"/repos/{repo}/releases?per_page=100&page={page}") or []
            out += batch
            if len(batch) < 100:
                break
        return [r for r in out if not r.get("draft")]


# --------------------------------------------------------------------- README


def plain_readme(markdown: str) -> str:
    """The README's opening paragraphs as plain text, at most 1,000 characters."""
    text = re.sub(r"```.*?```", "", markdown, flags=re.S)
    text = re.sub(r"<!--.*?-->", "", text, flags=re.S)
    text = re.sub(r"<[^>]+>", "", text)
    text = re.sub(r"!\[[^\]]*\]\([^)]*\)", "", text)
    text = re.sub(r"\[([^\]]*)\]\([^)]*\)", r"\1", text)
    text = re.sub(r"\[([^\]]*)\]\[[^\]]*\]", r"\1", text)
    paragraphs: list[str] = []
    for block in re.split(r"\n\s*\n", text):
        lines = []
        for line in block.splitlines():
            line = line.strip()
            if not line or line.startswith("#") or re.match(r"^[-=*_]{3,}$", line):
                continue
            line = re.sub(r"^(>|\d+\.|[-*+])\s+", "", line)
            line = re.sub(r"[*_`~]", "", line)
            lines.append(line)
        if lines:
            paragraphs.append(" ".join(lines))
    out = ""
    for paragraph in paragraphs:
        joined = f"{out}\n\n{paragraph}" if out else paragraph
        if len(joined) > MAX_README:
            if not out:
                out = paragraph[: MAX_README - 1].rsplit(" ", 1)[0] + "…"
            break
        out = joined
    return out


# ---------------------------------------------------------------------- build


class Repo:
    """One repo as GitHub describes it, fetched once for all its add-ons."""

    def __init__(self, github: GitHub, info: dict) -> None:
        self.info = info
        self.name = info["full_name"]
        self.releases = github.releases(self.name)
        readme = github.raw(f"/repos/{self.name}/readme")
        self.readme = (plain_readme(readme) or None) if readme else None


def fetch_repo(github: GitHub, repo: str) -> Repo | None:
    info = github.json(f"/repos/{repo}")
    if not isinstance(info, dict) or info.get("private"):
        print(f"  skipped {repo}: not a public repo", file=sys.stderr)
        return None
    return Repo(github, info)


def release_text(github: GitHub, release: dict) -> dict:
    """The release as the index carries it: its manifest and logo as text."""
    assets = {a["name"]: a for a in release.get("assets", [])}
    manifest_text = None
    logo_text = None
    if MANIFEST in assets:
        raw = github.download(assets[MANIFEST]["browser_download_url"], MAX_MANIFEST)
        if raw is not None:
            manifest_text = raw.decode("utf-8", errors="replace")
    if manifest_text is not None:
        try:
            logo = tomllib.loads(manifest_text).get("logo")
        except tomllib.TOMLDecodeError:
            logo = None
        if isinstance(logo, str):
            name = logo.rsplit("/", 1)[-1]
            if name in assets:
                raw = github.download(assets[name]["browser_download_url"], MAX_LOGO)
                if raw is not None:
                    logo_text = raw.decode("utf-8", errors="replace")
    return {
        "tag": release.get("tag_name"),
        "published_at": release.get("published_at"),
        "manifest": manifest_text,
        "logo": logo_text,
    }


def entry(github: GitHub, repo: Repo, addon: str | None, standing: str) -> dict:
    """One add-on's entry: a repo of one, or `addon` of several in the repo.
    Its counts and release are its own; its stars are the repo's."""
    releases = releases_of(repo.releases, addon)
    out: dict = {"repo": repo.name}
    if addon is not None:
        out["addon"] = addon
    out |= {
        "standing": standing,
        "description": repo.info.get("description"),
        "stars": repo.info.get("stargazers_count", 0),
        "installs": counted(releases, MARKER),
        "downloads": counted(releases, COMPONENT) or 0,
        "release": None,
        "readme": repo.readme,
    }
    newest = latest(releases, addon)
    if newest is not None:
        out["release"] = release_text(github, newest)
    return out


def wanted_entries(
    listed: list[Key],
    removed: dict[Key, str],
    topic: list[str],
    releases_for,
    own: str | None = None,
) -> list[tuple[str, str | None, str]]:
    """What goes in the index besides the removals, as (repo, addon,
    standing): every listed entry; every add-on released from `own`, this
    registry's repo, as listed; and every topic repo's add-ons that aren't
    listed or removed. A topic repo whose releases are tagged for add-ons
    is those add-ons; otherwise it is one. `releases_for(repo)` gives a
    repo's releases."""
    out: list[tuple[str, str | None, str]] = [(r, a, "listed") for r, a in listed]
    named = {key_of(r, a) for r, a in [*listed, *removed]}
    whole = {r.lower() for r, a in [*listed, *removed] if a is None}
    # The first-party add-ons beside this registry are reviewed in its own
    # pull requests, so each one released is listed without an entry.
    if own is not None and own.lower() not in whole:
        for addon in addons_in(releases_for(own)):
            if key_of(own, addon) not in named:
                named.add(key_of(own, addon))
                out.append((own, addon, "listed"))
    for repo in topic:
        if repo.lower() in whole:
            continue
        addons = addons_in(releases_for(repo))
        for addon in addons or [None]:
            if key_of(repo, addon) not in named:
                named.add(key_of(repo, addon))
                out.append((repo, addon, "unreviewed"))
    return sorted(out, key=lambda e: (e[0].lower(), e[1] or ""))


def index_of(
    github: GitHub,
    listed: list[Key],
    removed: dict[Key, str],
    own: str | None = None,
) -> dict:
    repos: dict[str, Repo | None] = {}

    def repo_of(name: str) -> Repo | None:
        if name.lower() not in repos:
            try:
                repos[name.lower()] = fetch_repo(github, name)
            except (urllib.error.URLError, TimeoutError, ValueError) as e:
                print(f"  skipped {name}: {e}", file=sys.stderr)
                repos[name.lower()] = None
        return repos[name.lower()]

    def releases_for(name: str) -> list[dict]:
        repo = repo_of(name)
        return repo.releases if repo else []

    addons = []
    for name, addon, standing in wanted_entries(
        listed, removed, github.topic_repos(), releases_for, own
    ):
        print(f"{standing:10} {address(name, addon)}", file=sys.stderr)
        repo = repo_of(name)
        if repo is None:
            continue
        try:
            addons.append(entry(github, repo, addon, standing))
        except (urllib.error.URLError, TimeoutError, ValueError) as e:
            print(f"  skipped {address(name, addon)}: {e}", file=sys.stderr)
    for (name, addon), reason in sorted(
        removed.items(), key=lambda kv: (kv[0][0].lower(), kv[0][1] or "")
    ):
        gone: dict = {"repo": name}
        if addon is not None:
            gone["addon"] = addon
        addons.append(gone | {"standing": "removed", "removed_reason": reason})

    return {
        # Version 1 while no entry needs `addon`, so a Niles that reads
        # only 1 keeps browsing until a repo of several add-ons is in.
        "version": 2 if any("addon" in a for a in addons) else 1,
        "generated_at": datetime.datetime.now(datetime.UTC).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "addons": addons,
    }


def build(out_dir: Path, own: str | None) -> None:
    listed, removed = read_list(Path("addons.toml").read_text(encoding="utf-8"))
    github = GitHub(os.environ.get("GITHUB_TOKEN"))
    index = index_of(github, listed, removed, own)
    out_dir.mkdir(parents=True, exist_ok=True)
    (out_dir / "index.json").write_text(
        json.dumps(index, ensure_ascii=False, indent=1) + "\n", encoding="utf-8"
    )
    print(f"{len(index['addons'])} entries, {github.calls} requests", file=sys.stderr)


# ---------------------------------------------------------------------- added


def added(base: str) -> None:
    listed, _ = read_list(Path("addons.toml").read_text(encoding="utf-8"))
    try:
        before_text = subprocess.run(
            ["git", "show", f"{base}:addons.toml"],
            check=True,
            capture_output=True,
            text=True,
        ).stdout
        before, _ = read_list(before_text)
    except subprocess.CalledProcessError:
        before = []
    had = {key_of(r, a) for r, a in before}
    for repo, addon in listed:
        if key_of(repo, addon) not in had:
            print(address(repo, addon))


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = parser.add_subparsers(dest="command", required=True)
    b = sub.add_parser("build", help="write index.json")
    b.add_argument("--out", type=Path, default=Path("site"))
    b.add_argument(
        "--self",
        dest="own",
        default=os.environ.get("GITHUB_REPOSITORY"),
        help="this registry's repo, whose add-ons are listed (default: $GITHUB_REPOSITORY)",
    )
    a = sub.add_parser("added", help="the repos and add-ons a pull request adds")
    a.add_argument("--base", required=True, help="the commit to compare with")
    args = parser.parse_args()
    if args.command == "build":
        build(args.out, args.own)
    else:
        added(args.base)


if __name__ == "__main__":
    main()
