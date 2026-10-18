"""The add-on library's registry: builds index.json, and checks addons.toml.

    python scripts/registry.py build --out site
    python scripts/registry.py added --base origin/main

`build` writes the file Niles reads (the format is in Niles's
`niles_plugins::library` docs): every repo in addons.toml as listed, every
public repo with the topic `niles-addon` as unreviewed, and every
[[removed]] entry. It reads GitHub's public data only, with the workflow's
own token for the API's higher limit.

`added` checks addons.toml's shape and prints, one per line, the repos a
pull request adds to [[addon]], for the listing check to run
`niles plugin check --listing` on.

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


def fail(message: str) -> None:
    print(f"error: {message}", file=sys.stderr)
    sys.exit(1)


# ---------------------------------------------------------------- addons.toml


def read_list(text: str) -> tuple[list[str], dict[str, str]]:
    """The listed repos and the removed ones with their reasons, checked."""
    try:
        doc = tomllib.loads(text)
    except tomllib.TOMLDecodeError as e:
        fail(f"addons.toml is not TOML: {e}")
    unknown = set(doc) - {"addon", "removed"}
    if unknown:
        fail(f"addons.toml has tables it doesn't use: {', '.join(sorted(unknown))}")

    seen: dict[str, str] = {}

    def repo_of(entry: object, table: str) -> str:
        if not isinstance(entry, dict) or not isinstance(entry.get("repo"), str):
            fail(f"every [[{table}]] needs repo = \"owner/name\"")
        repo = entry["repo"].strip()
        if not REPO.match(repo):
            fail(f"[[{table}]] repo {repo!r} is not owner/name")
        key = repo.lower()
        if key in seen:
            fail(f"{repo} is in addons.toml twice ([[{seen[key]}]] and [[{table}]])")
        seen[key] = table
        return repo

    listed = [repo_of(e, "addon") for e in doc.get("addon", [])]
    removed: dict[str, str] = {}
    for entry in doc.get("removed", []):
        repo = repo_of(entry, "removed")
        reason = entry.get("reason")
        if not isinstance(reason, str) or not reason.strip():
            fail(f"[[removed]] {repo} needs a reason: one sentence the household is shown")
        if len(reason) > MAX_REASON:
            fail(f"[[removed]] {repo}'s reason is over {MAX_REASON} characters")
        removed[repo] = reason.strip()
    return listed, removed


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


def entry(github: GitHub, repo: str, standing: str) -> dict | None:
    info = github.json(f"/repos/{repo}")
    if not isinstance(info, dict) or info.get("private"):
        print(f"  skipped {repo}: not a public repo", file=sys.stderr)
        return None
    releases = github.releases(info["full_name"])

    def counted(name: str) -> int | None:
        counts = [
            a.get("download_count", 0)
            for r in releases
            for a in r.get("assets", [])
            if a.get("name") == name
        ]
        return sum(counts) if counts else None

    out: dict = {
        "repo": info["full_name"],
        "standing": standing,
        "description": info.get("description"),
        "stars": info.get("stargazers_count", 0),
        "installs": counted(MARKER),
        "downloads": counted(COMPONENT) or 0,
        "release": None,
        "readme": None,
    }

    latest = next((r for r in releases if not r.get("prerelease")), None)
    if latest is not None:
        assets = {a["name"]: a for a in latest.get("assets", [])}
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
        out["release"] = {
            "tag": latest.get("tag_name"),
            "published_at": latest.get("published_at"),
            "manifest": manifest_text,
            "logo": logo_text,
        }

    readme = github.raw(f"/repos/{info['full_name']}/readme")
    if readme:
        out["readme"] = plain_readme(readme) or None
    return out


def build(out_dir: Path) -> None:
    listed, removed = read_list(Path("addons.toml").read_text(encoding="utf-8"))
    github = GitHub(os.environ.get("GITHUB_TOKEN"))
    removed_keys = {r.lower() for r in removed}
    listed_keys = {r.lower() for r in listed}

    wanted: dict[str, tuple[str, str]] = {}
    for repo in listed:
        wanted[repo.lower()] = (repo, "listed")
    for repo in github.topic_repos():
        key = repo.lower()
        if key not in listed_keys and key not in removed_keys:
            wanted.setdefault(key, (repo, "unreviewed"))

    addons = []
    for _, (repo, standing) in sorted(wanted.items()):
        print(f"{standing:10} {repo}", file=sys.stderr)
        try:
            made = entry(github, repo, standing)
        except (urllib.error.URLError, TimeoutError, ValueError) as e:
            print(f"  skipped {repo}: {e}", file=sys.stderr)
            continue
        if made is not None:
            addons.append(made)
    for repo, reason in sorted(removed.items(), key=lambda kv: kv[0].lower()):
        addons.append({"repo": repo, "standing": "removed", "removed_reason": reason})

    index = {
        "version": 1,
        "generated_at": datetime.datetime.now(datetime.UTC).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "addons": addons,
    }
    out_dir.mkdir(parents=True, exist_ok=True)
    (out_dir / "index.json").write_text(
        json.dumps(index, ensure_ascii=False, indent=1) + "\n", encoding="utf-8"
    )
    print(f"{len(addons)} entries, {github.calls} requests", file=sys.stderr)


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
    had = {r.lower() for r in before}
    for repo in listed:
        if repo.lower() not in had:
            print(repo)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = parser.add_subparsers(dest="command", required=True)
    b = sub.add_parser("build", help="write index.json")
    b.add_argument("--out", type=Path, default=Path("site"))
    a = sub.add_parser("added", help="the repos a pull request adds")
    a.add_argument("--base", required=True, help="the commit to compare with")
    args = parser.parse_args()
    if args.command == "build":
        build(args.out)
    else:
        added(args.base)


if __name__ == "__main__":
    main()
