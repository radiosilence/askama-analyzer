# askama-analyzer

A language server for [askama](https://askama.rs) templates: completion, hover, go to definition, references, renames, errors and formatting inside templates, from the rust-analyzer you already run.

## Why

Askama compiles a template into Rust inside `#[derive(Template)]`. rust-analyzer analyses that expansion but cannot map it back to the template, so a broken template shows up as one error on the struct, and the template itself gets no completion or navigation. Existing editor support either highlights only, or approximates askama's code generation by hand and misses whatever the approximation does not cover.

`askama-analyzer` runs askama's own code generator, keeps the template position of every token it generates, and asks rust-analyzer about the result. Answers come back in template positions, and they are the compiler's answers, because the code is the compiler's code.

## Installing

Each [release](https://github.com/radiosilence/askama-analyzer/releases) has archives for macOS (signed and notarised), Linux (static) and Windows, on x86_64 and arm64. Or build it:

```sh
cargo install --locked --git https://github.com/radiosilence/askama-analyzer askama-analyzer
```

It needs `rust-analyzer`, from the client's initialization options or the `PATH`.

## Editors

- **Zed:** the [Askama extension](https://github.com/radiosilence/zed-askama) downloads the server and passes it your rust-analyzer settings.
- **Claude Code:** `editors/claude-code` gives Claude Code's LSP tool the server. After each edit to a template, Claude sees askama's and rust-analyzer's errors in it, and it can ask for definitions, references and types there. With `askama-analyzer` and `rust-analyzer` on the `PATH` of the shell that starts `claude`:

  ```
  /plugin install askama --marketplace radiosilence/askama-analyzer
  ```

  The plugin attaches to `.html`, `.htm`, `.txt` and `.md` files as well as `.askama`, since that is how templates are named; in a file no template struct reads, the server does nothing. It leaves `.rs` files to rust-analyzer.
- **Anything else** that speaks LSP: run `askama-analyzer` on stdio for template files. rust-analyzer's path, arguments, environment and settings can be passed as `initializationOptions.rustAnalyzer` (`path`, `args`, `env`, `initializationOptions`, `settings`).

## On the command line

- `askama-analyzer check [--rust-analyzer PATH] [PATH]` prints every template error in a Cargo workspace and exits 1 if there are any. askama's own errors need nothing else; given a rust-analyzer, it also reports the type errors rust-analyzer finds in the generated code, as an editor shows them.
- `askama-analyzer fmt [--check] [PATH...]` formats templates, the files given or every file under a `templates` directory, changing only the spacing inside tags: text outside them is what askama renders, so it is never touched.

## How it is put together

| Path | What |
|---|---|
| `crates/askama-analyzer` | The language server, `check` and `fmt` |
| `crates/askama-analyzer-codegen` | askama's generator run on one template item, with a map from every generated token to its template range |
| `editors/claude-code` | The Claude Code plugin |
| `vendor/askama_derive` | askama's generator with a small patch for long-running use, described in [ARCHITECTURE.md](ARCHITECTURE.md) |
| `vendor/askama-testing` | askama's own test suite at the pinned release, which the generator and the formatter are tested against |
| `docs/releasing.md` | How a release goes and the secrets it needs |

## Licence

MIT, in [LICENSE](LICENSE). `vendor/askama_derive` is askama's code under its own MIT or Apache-2.0 licence, kept beside it.
