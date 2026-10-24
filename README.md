# Niles add-ons

The add-on library for [Niles](https://github.com/MarkNygaard/niles): what Niles shows under **Add-ons → Browse**. It's one file, [`index.json`](https://addons.getniles.app/index.json), rebuilt from GitHub's public data every six hours and whenever this repo changes. Niles reads it, keeps it for six hours, and makes no GitHub API calls of its own to browse.

Writing an add-on is described in Niles's [add-on author guide](https://getniles.app/docs/writing-add-ons).

## Being found

Give your repo the topic `niles-addon` (About → Topics, or `gh repo edit --add-topic niles-addon`). Within six hours it appears in Browse as **Unreviewed**. Unreviewed add-ons are folded away below the reviewed ones, with a line saying nobody has looked at them.

## Several add-ons in one repo

A repo can hold several add-ons. Tag each one's releases with its id, a `-v` and the version: `spotprice-v0.2.0`, `pool-v1.0.0`. Niles installs one of them as `owner/repo/<id>`, from its newest release with that tag. The library lists each add-on its releases are tagged for as its own entry, with its own install count summed over its own releases. Stars belong to the repo, so its add-ons share them, and Browse says so. The [plugin author guide](https://getniles.app/docs/writing-add-ons/publishing) has the details.

## Getting listed

Open a pull request adding your repo to [`addons.toml`](addons.toml):

```toml
[[addon]]
repo = "you/your-repo"
```

For one add-on of a repo that holds several, add `addon` with its id:

```toml
[[addon]]
repo = "you/your-addons"
addon = "spotprice"
```

A repo is listed whole or by its add-ons, not both.

The check runs `niles plugin check --listing you/your-repo` (or `you/your-addons/spotprice`) on your latest release. That is everything an install checks, plus a `niles-installed` asset of at most 1 KiB. The run's summary lists what your add-on would be allowed to do, in the words the household's approval screen uses. Mark reads that and merges, and your add-on is **Listed**.

Listed means it passed the checks and someone read what it asks for. It isn't an audit of your code, later releases aren't reviewed again, and every household still approves its permissions when they install it.

## Install counts

Attach a file named `niles-installed` (any content, up to 1 KiB) to each release. Niles downloads it once per house, after a fresh install, and the library shows GitHub's download count of it, summed over your releases. Previews, updates and reinstalls don't count, and nothing about the household is sent. Without it the library shows your component's downloads instead, labelled as downloads.

## Leaving the library

Remove the topic, or open a pull request removing your entry. To take an add-on off the list, move it to `[[removed]]` with a one-sentence reason:

```toml
[[removed]]
repo = "someone/their-repo"
reason = "Its author archived it."
```

With `addon`, only that add-on of the repo is removed. Without it, every add-on in the repo is.

A removed add-on is hidden from Browse. Where it's installed it keeps working, and its panel says it is no longer listed, and why.

## How it's built

- [`scripts/registry.py`](scripts/registry.py) writes `index.json`, using only the Python standard library. The format is Niles's, documented in `niles_plugins::library`. The file is version 2 once an entry has `addon`, and version 1 until then, so a Niles from before version 2 keeps browsing. [`scripts/test_registry.py`](scripts/test_registry.py) tests it against made-up GitHub data: `python -I -m unittest discover -s scripts`.
- [`.github/workflows/build.yml`](.github/workflows/build.yml) runs it on a schedule and on every merge, and deploys to GitHub Pages at `addons.getniles.app`.
- [`.github/workflows/check.yml`](.github/workflows/check.yml) checks a listing pull request with Niles's own image.

## First-party add-ons

Niles's own add-ons live here, in [`addons/`](addons), one folder each:

- [`spotprice`](addons/spotprice): Danish electricity spot prices.
- [`bambulab`](addons/bambulab): a Bambu Lab 3D printer on the home network. It moved here from [MarkNygaard/niles-bambulab](https://github.com/MarkNygaard/niles-bambulab), with its history, and carries on from its 0.1.1. [`fake-printer`](addons/bambulab/fake-printer) beside it is a stand-in printer for trying it, a native program outside the add-ons' workspace.

They are built like anybody's: against the published interface, copied into [`addons/wit/`](addons/wit), never against Niles's own crates. A change to the interface lands in Niles first, and an add-on that uses it follows with a `min_niles` that has it.

Build and check one locally, with the `wasm32-wasip2` target installed (`rustup target add wasm32-wasip2`):

```sh
cd addons
cargo fmt --all --check
cargo clippy --target wasm32-wasip2 --all-targets -- -D warnings
./build.sh spotprice                  # or ./build.ps1; no name builds them all
niles plugin check spotprice
```

`build.sh` copies each component to `addons/<name>/integration.wasm`, where `niles plugin check` looks. [`.github/workflows/addons.yml`](.github/workflows/addons.yml) does the same on every pull request that touches `addons/`, checking with the published Niles image.

Each add-on is released on its own. Pull request titles name it as the scope, `feat(spotprice): ...` or `fix(spotprice): ...`, and release-please keeps a release pull request per add-on (`chore(spotprice): release 0.2.0`). Merging that tags `spotprice-v0.2.0`, and [`.github/workflows/release-please.yml`](.github/workflows/release-please.yml) attaches `niles-integration.toml`, `integration.wasm`, the logo and `niles-installed` to the release. Households install it as `MarkNygaard/niles-addons/spotprice`.

A new add-on also needs a package in [`release-please-config.json`](release-please-config.json), copied from `addons/spotprice`'s with the name changed, and `# x-release-please-version` after the `version` line in its `Cargo.toml` and `niles-integration.toml`.

The add-ons here are **Listed** automatically: the index builder lists every add-on released from this repo, without an entry in `addons.toml`, since they're reviewed in this repo's own pull requests. A `[[removed]]` entry with `addon = "<name>"` still takes one off. Listing one makes the index version 2, which Niles reads from 0.6.0.
