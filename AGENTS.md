# AGENTS.md

How to work in this repo, for any coding agent (Claude Code, Codex, Hermes, Cursor) and the people who run one. `CLAUDE.md` imports this file.

## What this repo is

Two things for [Niles](https://github.com/MarkNygaard/niles), the self-hosted home assistant:

- **The add-on library's registry.** `addons.toml` lists the reviewed add-ons. `scripts/registry.py` builds `index.json` from it and GitHub's public data, and `.github/workflows/build.yml` publishes that at `https://addons.getniles.app/index.json`. Niles reads that file under Add-ons → Browse. The format is Niles's, documented in `niles_plugins::library` in the Niles repo.
- **Niles's first-party add-ons** in `addons/<name>/`. They're WebAssembly components, installed as `MarkNygaard/niles-addons/<name>` and released one by one as `<name>-vX.Y.Z`.

How add-ons work, for authors: [getniles.app/docs/writing-add-ons](https://getniles.app/docs/writing-add-ons).

## Layout

- `addons/`: its own Cargo workspace (`addons/Cargo.toml`), built to `wasm32-wasip2` (`addons/rust-toolchain.toml`).
  - `addons/<name>/` holds one add-on: `Cargo.toml`, `niles-integration.toml`, its logo, `src/`, `README.md` and `CHANGELOG.md`.
  - `addons/wit/` holds frozen copies of the published Niles interface (WIT), copied unchanged from the Niles repo's `schemas/`.
- `addons.toml`: the reviewed list, plus `[[removed]]` entries.
- `scripts/registry.py`: the index builder, Python standard library only. Its tests are in `scripts/test_registry.py`.
- `site-static/`: the front page next to `index.json`.

## Commands

```sh
# Add-ons
rustup target add wasm32-wasip2
cd addons
cargo fmt --all --check
cargo clippy --target wasm32-wasip2 -- -D warnings
./build.sh            # or .\build.ps1; builds each and writes addons/<name>/integration.wasm

# Check one as an install would, with a Niles binary (from a Niles checkout):
cargo run -p niles-bin -- plugin check <path>/addons/<name>

# Registry
python -I -m unittest discover -s scripts
python scripts/registry.py build --out site    # needs GITHUB_TOKEN for the API's higher limit
```

CI runs all of this. `addons.yml` checks each add-on with the published Niles image, the Niles households actually run. `build.yml` runs the registry tests before every build.

## Rules for add-ons

- **An add-on depends only on the WIT, never on Niles's crates.** It builds against a copy in `addons/wit/`. Don't edit those copies. A new interface version gets its own file, copied once Niles has released it.
- **An add-on that needs something new from Niles waits for Niles.** The interface change is a Niles issue and ships in a Niles release first. Then the add-on moves to it and sets `min_niles` in its manifest to that release.
- **The manifest is honest.** It declares every host the add-on calls, and nothing more, plus every setting and permission. The household approves exactly what it says.
- **No secrets in code, tests or logs.** Keys come through the secrets interface, which the household fills in.
- Comments explain why. Never restate the code.

## Rules for the registry

- `addons.toml` is reviewed by Mark. A listing PR is checked by `check.yml` (`niles plugin check --listing`), and Mark reads the permissions in its summary before merging.
- `registry.py` stays standard-library only, so the workflows install nothing.
- Each `index.json` field is read by Niles as a stranger's data, so keep the format as `niles_plugins::library` documents it. A change an older Niles would misread bumps `version`.

## Branches, PRs and releases

1. Branch off `main` as `feat/`, `fix/`, `docs/` or `chore/`, named after the issue (`feat/nil-53-weather`).
2. Open a PR into `main`, with `Fixes NIL-N` in the body. Its title is a Conventional Commit with the add-on as scope, `feat(weather): …` or `fix(spotprice): …`, because release-please reads the titles. Registry changes have no add-on scope.
3. Wait for CI, then squash-merge.

release-please (`release-please-config.json`, `.github/workflows/release-please.yml`) keeps one release PR per add-on with changes, titled `chore(<name>): release X.Y.Z`. Merging it tags `<name>-vX.Y.Z`, and the workflow attaches `niles-integration.toml`, `integration.wasm`, the logo and `niles-installed`, so the release installs and is counted. A new add-on gets a package in `release-please-config.json` (copy spotprice's) with `initial-version`.

## Linear

Issues for this repo go in the `niles` team's **Add-ons** project. The ai-harness picks this repo from that project. Anything in the Niles repo, including the installer, the interface and Browse, goes in *Niles software*.

- Anyone but Mark files a new issue into Triage. Only Mark moves an issue to Todo, and only he hands one to the ai-harness. An agent never triggers the harness.
- Before filing, search for the same thing. If one already covers it, comment there instead.
- Work only on an issue in Todo that nobody else is assigned to and that isn't delegated to the AI Harness. Assign it to yourself and move it to In Progress, then to In Review once the PR is open. The rest is Mark's.

## Things to avoid

- `git add -A` or `git add .`. Stage files by name.
- Skipping hooks (`--no-verify`).
- Printing secrets anywhere.
- A dependency on Niles's crates in an add-on.
- A third-party Python package in `scripts/`.
