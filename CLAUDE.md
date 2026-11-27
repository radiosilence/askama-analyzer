# askama-analyzer

A language server for [askama](https://askama.rs) templates, backed by rust-analyzer. It answers template requests by running askama's own code generator and asking the user's rust-analyzer about its output. The repository ships two things at one version:

- `crates/`: `askama-analyzer` (the server, `check` and `fmt`) and `askama-analyzer-codegen` (askama's generator run on one item, with a map back to the template);
- `editors/claude-code/`: a Claude Code plugin that gives Claude Code's LSP tool the server.

Its main client is the Zed extension in [radiosilence/zed-askama](https://github.com/radiosilence/zed-askama), which downloads this repository's release archives and also holds the tree-sitter grammar. The server speaks plain LSP, so any editor can run it. radiosilence/zed-askama#1 holds the plan and the reasons behind it. The [kōan standard](https://github.com/radiosilence/koan/blob/main/STANDARD.md) applies throughout.

## Git

Every change is a squash-merged pull request with signed commits. `main` is merged into a branch; a pushed branch is never rebased or force-pushed. Titles say what the change does, in sentence case.

## Changelog

`CHANGELOG.md` is generated from `changelog.d/` by `just changelog`; never edit it by hand. Each pull request adds one fragment, `changelog.d/unreleased/<section>/<pr>-<slug>.md`, holding one `- **Lead.** Detail.` line. `scripts/changelog.py` and `scripts/test_changelog.py` are koan's, copied verbatim: fix them in koan. 0.1.0 and the fragments unreleased at the move were made in zed-askama, so their pull request numbers point there.

## Releasing

`just release patch|minor|major|X.Y.Z` (or the **Release** workflow) opens a `release-vX.Y.Z` pull request that sets every version file and cuts the changelog. Merging it releases: CI builds `askama-analyzer` for six platforms (macOS signed and notarised) and publishes the GitHub release with checksums and provenance. [docs/releasing.md](docs/releasing.md) has the steps and the secrets.

## CI/CD

One workflow, `.github/workflows/ci-cd.yml`, ending in `Checks`, the only required status. `changes` decides which domains a diff touches (`rust`, `vendor`, `changelog`, `workflows`, `action`, `deps`), each an allowlist of paths; a new top-level source directory must be added there. It also decides whether the run releases: a release pull request (branch `release-v*`) builds every platform without publishing, and a push to `main` that changes the workspace version publishes it once `Checks` has passed. `versions` runs on every run. `release.yml` only opens the release pull request.

## Check

- `just check`: everything CI checks but the advisory audit.
- `just fast`: the checks that take seconds, which lefthook runs before a push.

## Invariants

- **askama's code generation is never reimplemented.** The analyser runs askama's generator, from the `tooling` branch of [radiosilence/askama](https://github.com/radiosilence/askama/compare/v0.16.1...tooling), so what rust-analyzer analyses is what `cargo build` compiles.
- **The fork's `tooling` branch is askama's release plus the `tooling` feature, and nothing else.** The change is one reviewable diff against the release tag; ARCHITECTURE.md lists what it does and why. A new askama release is merged into the branch, and `askama_derive` and `askama_parser` move here together to the new commit (`rev` in `Cargo.toml`).
- **Formatting never touches `vendor/`.** `cargo fmt` (never `--all`) in the justfile, CI, lefthook and the agents' push guard; `vendor/askama-testing` is askama's code.
- **Every thread that parses with proc-macro2 frees its source map afterwards** (`proc_macro2::extra::invalidate_current_thread_spans`), since `span-locations` keeps every parsed text for the life of the thread. `askama_analyzer_codegen::generate` does this itself.
- **Every version file states one version.** The workspace `Cargo.toml` and `editors/claude-code/.claude-plugin/plugin.json` move together through `scripts/versions.py --set`; CI fails when they disagree.
- **The release archive names are a contract with the editors that download them.** zed-askama's `server_asset` downloads `askama-analyzer-<target>.tar.gz` (or `.zip` on Windows) from this repository's latest release by name; the `build` job in `ci-cd.yml` must produce exactly those.
- **The initialization options are a contract with the Zed extension.** `rustAnalyzer: { path, args, env, initializationOptions, settings }` (`ra::Config`) is read leniently, so the extension and the server release independently; a field is added, never renamed.
- **`vendor/askama-testing` is askama's test suite at the pinned release**, written by `scripts/vendor-askama-testing.py`; the generator's and the formatter's corpus tests read it.
- **The formatter changes only whitespace HTML does not render.** Whitespace inside askama tags; and in HTML templates, the whitespace between a start tag's attributes and the amount of leading whitespace on existing lines, outside `<pre>`, `<textarea>`, `<script>`, `<style>`, `raw` blocks, `filter` and block `let` bodies, and blocks a parent renders in one of those (every block when a parent cannot be read). A macro or include rendered inside another file's `<pre>` is the one case it cannot see; `format.indentHtml: false` and `--no-indent-html` turn indentation off. Indentation comes from `.editorconfig` for `fmt` and the editor alike. It checks the tree, the stripped lines, the text between tags and a second pass, and refuses otherwise.
- **`askama-analyzer-codegen` does no I/O.** Files reach it through its `Files` trait, so unsaved buffers are what askama reads.

## Where things live

| Path | What |
|---|---|
| `crates/askama-analyzer-codegen` | askama's generator run on one item, and the map from each generated token to its template range; `tests/askama_corpus.rs` runs it over askama's passing tests, with a floor on how many template names map back |
| `crates/askama-analyzer` | The server (`server.rs`, `workspace.rs`, `ra.rs`, `features.rs`), `check` and `fmt`; `tests/rust_analyzer.rs` runs it against the `shop` fixture with a real rust-analyzer |
| `crates/askama-analyzer/src/format/` | The template formatter (`askama-analyzer fmt`, and the LSP's formatting): `mod.rs` splits tags and checks the result against `askama_parser`'s tree, `layout.rs` lays out expressions as rustfmt would, `html.rs` re-indents HTML templates by changing only leading whitespace; `tests/format_corpus.rs` runs it over askama's suite and, with `ASKAMA_TEMPLATE_DIRS`, real projects |
| `editors/claude-code/`, `.claude-plugin/marketplace.json` | The Claude Code plugin, which gives Claude Code's LSP tool `askama-analyzer`, and the marketplace that installs it from this repository |
| `scripts/release.sh`, `scripts/versions.py`, `docs/releasing.md` | The release pull request, the one-version check, and how they fit |
| `vendor/askama-testing`, `scripts/vendor-askama-testing.py` | askama's test suite and the script that vendors it |
| `ARCHITECTURE.md` | How the pieces fit and the rules they keep |
