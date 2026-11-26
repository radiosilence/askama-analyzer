# askama-analyzer: a language server for askama templates, backed by rust-analyzer

# Everything CI checks short of the audit and the release builds: formatting,
# clippy with warnings as errors, the tests, the vendored test suite, the
# changelog, the version files and the workflows.
check:
    cargo fmt -- --check
    cargo clippy --workspace --all-targets --locked -- -D warnings
    cargo test --workspace --locked
    python3 scripts/vendor-askama-testing.py --check
    python3 -m unittest scripts/test_changelog.py
    python3 scripts/changelog.py --lint
    python3 scripts/versions.py
    actionlint

# The checks that take seconds: the lockfile matches the manifests, and formatting.
fast:
    cargo metadata --locked --format-version 1 > /dev/null
    cargo fmt -- --check

# Format the Rust code.
fmt:
    cargo fmt

# Fail unless every version file states the one version the repository ships at.
versions:
    python3 scripts/versions.py

# Open the release PR: `just release patch|minor|major`, or an exact version.
# Merging it publishes the GitHub release; tags are never pushed by hand (docs/releasing.md).
release version *args:
    scripts/release.sh {{version}} {{args}}

# Install the git hooks.
hooks:
    lefthook install

# Write CHANGELOG.md from the fragments in changelog.d/. `--stdout` prints it instead.
changelog *args:
    python3 scripts/changelog.py {{args}}
