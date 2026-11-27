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
- **Anything else** that speaks LSP: run `askama-analyzer` on stdio for template files. rust-analyzer's path, arguments, environment and settings can be passed as `initializationOptions.rustAnalyzer` (`path`, `args`, `env`, `initializationOptions`, `settings`), and the formatter's settings as `initializationOptions.format`: `maxWidth` (100 by default) and `indentHtml` (`true` by default; `false` leaves HTML templates' indentation outside tags as written).

## On the command line

- `askama-analyzer check [--rust-analyzer PATH] [--annotate] [PATH]` prints every template error in a Cargo workspace and exits 1 if there are any. askama's own errors need nothing else; given a rust-analyzer, it also reports the type errors rust-analyzer finds in the generated code, as an editor shows them.
- `askama-analyzer fmt [--check [--annotate]] [--max-width N] [--no-indent-html] [PATH...]` formats templates, the files given or every file in or under a `templates` directory. Expressions inside tags are laid out as rustfmt lays out Rust, broken over lines when they pass `--max-width` (100). A line still too long breaks inside the tags that run past the width: an askama tag too long for a line of its own puts its contents on lines of their own, and in HTML templates a start tag that begins its line puts one attribute per line. A dense line with nothing long enough to break stays long. Whitespace inside a tag is never rendered, and nothing between tags or in text changes. Other templates' text is never touched, since askama renders it byte for byte. Arrays of short simple items are packed onto lines, as rustfmt packs them.
- `--annotate` prints `check`'s errors, and each run of lines `fmt --check` would change, as GitHub Actions annotations, so a workflow shows them on the pull request's diff; `fmt`'s hold the formatted text.
- `askama-analyzer completions SHELL` prints a completion script for bash, zsh, fish, elvish or PowerShell.

### HTML indentation

HTML templates (`.html`, `.htm`, also with askama's `.askama`, `.j2`, `.jinja`, `.jinja2` or `.rinja` suffix) are re-indented by the nesting of elements and block tags. One level is what the file's `.editorconfig` says (`indent_style`, `indent_size`, `tab_width`), or 2 spaces when it says nothing; `fmt` and the editor both read it, so a file the editor formatted passes `fmt --check`.

Only the amount of whitespace starting each line changes, which HTML renders the same whatever its amount. `<pre>`, `<textarea>`, `<script>`, `<style>`, `raw` blocks, and the bodies of `filter` and block `let` tags are left as written, as are the blocks a parent template renders inside one of those elements (or every block, when the parent cannot be read). What a file cannot see is the one way the output can change: a macro or an included template rendered inside another file's `<pre>`, or an element styled `white-space: pre`. Where that matters, turn it off with `--no-indent-html` or `format.indentHtml: false`.

## How it is put together

| Path | What |
|---|---|
| `crates/askama-analyzer` | The language server, `check` and `fmt` |
| `crates/askama-analyzer-codegen` | askama's generator run on one template item, with a map from every generated token to its template range |
| `editors/claude-code` | The Claude Code plugin |
| [radiosilence/askama](https://github.com/radiosilence/askama/compare/v0.16.1...tooling) | askama with a small `tooling` feature for long-running use, described in [ARCHITECTURE.md](ARCHITECTURE.md), which the crates build from |
| `vendor/askama-testing` | askama's own test suite at the pinned release, which the generator and the formatter are tested against |
| `docs/releasing.md` | How a release goes and the secrets it needs |

## Licence

MIT, in [LICENSE](LICENSE). `vendor/askama-testing` is askama's code under its own MIT or Apache-2.0 licence, kept beside it.
