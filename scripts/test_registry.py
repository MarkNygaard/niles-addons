"""The registry builder against made-up GitHub data, without the network.

    python -I -m unittest discover -s scripts
"""

from __future__ import annotations

import io
import json
import unittest
from contextlib import redirect_stderr

import registry


def release(tag: str, *, prerelease: bool = False, installs: int = 0, wasm: int = 0) -> dict:
    return {
        "tag_name": tag,
        "prerelease": prerelease,
        "published_at": "2026-10-20T12:00:00Z",
        "assets": [
            {"name": "niles-integration.toml", "browser_download_url": f"dl/{tag}/manifest",
             "download_count": 0},
            {"name": "integration.wasm", "browser_download_url": f"dl/{tag}/wasm",
             "download_count": wasm},
            {"name": "niles-installed", "browser_download_url": f"dl/{tag}/marker",
             "download_count": installs},
        ],
    }


class FakeGitHub:
    """Repos, their releases and the topic search, as registry.GitHub
    answers them. A release's manifest names the tag it came from."""

    def __init__(self, repos: dict[str, list[dict]], topic: list[str] = ()) -> None:
        self.repos = repos
        self.topic = list(topic)
        self.calls = 0

    def json(self, path: str) -> object:
        name = path.removeprefix("/repos/")
        if name in self.repos:
            return {"full_name": name, "description": f"{name} on GitHub", "stargazers_count": 9}
        return None

    def raw(self, path: str) -> str | None:
        return "The add-ons.\n"

    def download(self, url: str, cap: int) -> bytes | None:
        tag = url.split("/")[1]
        return f'id = "x"\nversion = "{tag}"\n'.encode()

    def topic_repos(self) -> list[str]:
        return self.topic

    def releases(self, repo: str) -> list[dict]:
        return self.repos[repo]


# Newest first, as the API lists them.
SEVERAL = [
    release("bravo-v2.0.0", installs=5, wasm=8),
    release("alpha-v1.2.0-beta.1", prerelease=True, installs=1),
    release("alpha-v1.1.0", installs=3, wasm=4),
    release("alpha-v1.0.0", installs=2, wasm=2),
    release("alpha-v0.9.0"),
]
ONE = [release("v0.2.0", installs=7), release("v0.1.0", installs=1)]


def build(github: FakeGitHub, toml: str) -> dict:
    listed, removed = registry.read_list(toml)
    with redirect_stderr(io.StringIO()):
        return registry.index_of(github, listed, removed)


def by_address(index: dict) -> dict[str, dict]:
    return {registry.address(e["repo"], e.get("addon")): e for e in index["addons"]}


class Tags(unittest.TestCase):
    def test_a_tag_names_its_add_on(self) -> None:
        self.assertEqual(registry.addon_of_tag("spot_price-v1.2.0"), "spot_price")
        self.assertEqual(registry.addon_of_tag("alpha-v1.2.0-beta.1"), "alpha")
        for tag in ["v1.2.0", "1.2.0", "alpha-1.2.0", "Alpha-v1.2.0", "alpha-vnext", "a-v1.0.0"]:
            self.assertIsNone(registry.addon_of_tag(tag), tag)

    def test_the_latest_of_an_add_on_is_its_highest_version(self) -> None:
        alpha = registry.releases_of(SEVERAL, "alpha")
        self.assertEqual(registry.latest(alpha, "alpha")["tag_name"], "alpha-v1.1.0")
        self.assertEqual(registry.addons_in(SEVERAL), ["alpha", "bravo"])
        self.assertEqual(registry.releases_of(SEVERAL, None), [])


class AddonsToml(unittest.TestCase):
    def test_addon_names_one_add_on_of_a_repo(self) -> None:
        listed, removed = registry.read_list(
            '[[addon]]\nrepo = "jane/addons"\naddon = "alpha"\n'
            '[[addon]]\nrepo = "jane/addons"\naddon = "bravo"\n'
            '[[removed]]\nrepo = "jane/addons"\naddon = "charlie"\nreason = "Broken."\n'
        )
        self.assertEqual(listed, [("jane/addons", "alpha"), ("jane/addons", "bravo")])
        self.assertEqual(removed, {("jane/addons", "charlie"): "Broken."})

    def test_a_wrong_or_repeated_addon_is_refused(self) -> None:
        for toml in [
            '[[addon]]\nrepo = "jane/addons"\naddon = "Alpha"\n',
            '[[addon]]\nrepo = "jane/addons"\naddon = 7\n',
            '[[addon]]\nrepo = "jane/addons"\naddon = "alpha"\n'
            '[[addon]]\nrepo = "Jane/Addons"\naddon = "alpha"\n',
            '[[addon]]\nrepo = "jane/addons"\n[[addon]]\nrepo = "jane/addons"\naddon = "alpha"\n',
            '[[addon]]\nrepo = "jane/addons"\naddon = "alpha"\n'
            '[[removed]]\nrepo = "jane/addons"\nreason = "Gone."\n',
        ]:
            with self.subTest(toml=toml), redirect_stderr(io.StringIO()):
                with self.assertRaises(SystemExit):
                    registry.read_list(toml)


class Index(unittest.TestCase):
    def test_a_listed_repo_of_several_has_an_entry_per_add_on(self) -> None:
        github = FakeGitHub({"jane/addons": SEVERAL})
        index = build(
            github,
            '[[addon]]\nrepo = "jane/addons"\naddon = "alpha"\n'
            '[[addon]]\nrepo = "jane/addons"\naddon = "bravo"\n',
        )
        self.assertEqual(index["version"], 2)
        entries = by_address(index)
        alpha = entries["jane/addons/alpha"]
        self.assertEqual(alpha["addon"], "alpha")
        self.assertEqual(alpha["standing"], "listed")
        self.assertEqual(alpha["release"]["tag"], "alpha-v1.1.0")
        self.assertEqual(alpha["installs"], 1 + 3 + 2, "its own releases, the pre-release too")
        self.assertEqual(alpha["downloads"], 4 + 2)
        self.assertEqual(alpha["stars"], 9)
        bravo = entries["jane/addons/bravo"]
        self.assertEqual((bravo["release"]["tag"], bravo["installs"]), ("bravo-v2.0.0", 5))

    def test_a_topic_repo_of_several_lists_each_add_on_its_tags_name(self) -> None:
        github = FakeGitHub({"jane/addons": SEVERAL}, topic=["jane/addons"])
        index = build(
            github,
            '[[addon]]\nrepo = "jane/addons"\naddon = "alpha"\n',
        )
        entries = by_address(index)
        self.assertEqual(set(entries), {"jane/addons/alpha", "jane/addons/bravo"})
        self.assertEqual(entries["jane/addons/alpha"]["standing"], "listed")
        self.assertEqual(entries["jane/addons/bravo"]["standing"], "unreviewed")

    def test_a_repo_of_one_reads_as_before_in_version_1(self) -> None:
        github = FakeGitHub({"jane/hello": ONE, "bob/pool": ONE}, topic=["bob/pool"])
        index = build(
            github,
            '[[addon]]\nrepo = "jane/hello"\n'
            '[[removed]]\nrepo = "eve/gone"\nreason = "Gone."\n',
        )
        self.assertEqual(index["version"], 1)
        hello = by_address(index)["jane/hello"]
        self.assertNotIn("addon", hello)
        self.assertEqual((hello["release"]["tag"], hello["installs"]), ("v0.2.0", 8))
        self.assertEqual(by_address(index)["bob/pool"]["standing"], "unreviewed")
        self.assertEqual(
            index["addons"][-1],
            {"repo": "eve/gone", "standing": "removed", "removed_reason": "Gone."},
        )
        json.dumps(index)

    def test_a_removed_add_on_is_left_out_and_said_so(self) -> None:
        github = FakeGitHub({"jane/addons": SEVERAL}, topic=["jane/addons"])
        index = build(
            github,
            '[[removed]]\nrepo = "jane/addons"\naddon = "bravo"\nreason = "Broken."\n',
        )
        self.assertEqual(index["version"], 2)
        entries = by_address(index)
        self.assertEqual(entries["jane/addons/alpha"]["standing"], "unreviewed")
        self.assertEqual(
            entries["jane/addons/bravo"],
            {"repo": "jane/addons", "addon": "bravo", "standing": "removed",
             "removed_reason": "Broken."},
        )


if __name__ == "__main__":
    unittest.main()
