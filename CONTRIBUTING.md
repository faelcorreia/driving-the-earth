# Contributing

The whole repository is in English (code, identifiers, logs, comments, commit messages, docs), which
keeps it easy to extend with LLM-assisted code generation. Internationalization is core: every text shown
in the interface goes through i18n keys (en, pt-BR), never hardcoded strings.
Never commit personal data (addresses, exact home coordinates, `home.local.json`, `cache/`).

## Versioning

The whole workspace has one version: `[workspace.package] version` in the root `Cargo.toml`.
Every crate inherits it with `version.workspace = true`. The version follows semver with three numbers
(`MAJOR.MINOR.PATCH`, no pre-release or build suffix):

- **Major** stays at `0` until the first stable release, `1.0.0`. After that it stays `1`
  (`1.y.z`). Major `2` or higher is not used. Major stays 1; raising it is a deliberate project
  decision, done by changing `MAX_MAJOR` in `scripts/check_version.py` in the same PR, so it is
  visible in the diff and goes through the owner's review.
- **Minor and patch have no upper limit.** `1.9999.99999999` is valid; the next minor after it is
  `1.10000.0`. Only the major number is capped.
- **Minor** for a new feature, or a change to the tile or cache format. A format change that breaks
  existing data is a minor bump, never a major one.
- **Patch** for fixes and adjustments that add no feature and change no format.

Before the first release tag, the version is not constrained beyond the format and the major limit.
The first release is `v0.0.1`. Once a `v*` tag exists on `main`, the rules below apply in full.

Bumps are exact: patch goes up by 1; a minor bump sets patch to `0`; a major bump sets minor and patch
to `0`. The version never decreases.

A PR that changes code (`crates/`, `godot/`, `Cargo.toml`, `Cargo.lock`) must bump the version.
Changes only to docs or `.github/` do not need a bump.

The `version-check` workflow runs `scripts/check_version.py` on every PR to `main`. To run it locally:

```sh
python3 -m unittest discover -s scripts -p 'test_*.py'
python3 scripts/check_version.py --base origin/main
```
