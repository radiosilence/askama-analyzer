# Changelog

## Unreleased

### Changed

- **Typed macro parameters, `filter` tags and let chains are laid out.** A `macro` tag's typed parameters go one per line when they do not fit, a `filter` tag's filters are laid out like an expression's, and an `if` condition chained with `&& let` breaks before each `&&`; before, these tags only had their spacing normalised.

## 0.5.0

### Changed

- **Every broken tag has its delimiters on lines of their own.** A tag that starts its line and does not fit on it no longer breaks in place around its delimiters: like a tag broken after other text, its expression goes one level in from the line's indentation and its closing delimiter returns to that indentation.

## 0.4.0

### Fixed

- **The formatter writes `Vec<u8> = v` and keeps a start tag whole before dense content.** A typed `let` written `Vec<u8>= v` came out as `Vec<u8 >= v`, which reads as a comparison, and a start tag that begins a long line was split onto attribute lines even when the content after its `>` stayed as long as before.

## 0.3.0

### Added

- **The formatter lays out long expressions and indents HTML.** Expressions inside tags are broken over lines as rustfmt breaks Rust, to `--max-width` or `format.maxWidth` (100); a line still too long breaks inside the tags that run past the width, and in HTML between a start tag's attributes; and HTML templates are re-indented by nesting depth, by the file's `.editorconfig`, unless `--no-indent-html` or `format.indentHtml: false` says not to. Only whitespace that HTML does not render changes, except in a macro or include another file renders inside `<pre>`.

### Fixed

- **Renames reach templates, and a rename that cannot work says why.** Renaming a field from a template now renames its uses in templates as well as in Rust, and a name rust-analyzer will not rename, such as one from a dependency, is refused with its reason instead of opening a rename that does nothing. Errors on a field now sit on its name rather than the whole access.

## 0.2.1

### Fixed

- **askama's vendored test suite is complete again.** Three of its `.gitignore` files were left out when the server moved from zed-askama, which failed the suite's check.
- **Hover, definition and rename work on every segment of a path.** On `Button` or `primary` in `kit::Button::primary(…)` they found nothing, because rust-analyzer was asked about the `::` before the segment rather than the segment itself.

## 0.2.0

### Added

- **Completion offers loop variables, tags, filters and template paths, and go to definition follows includes, macros and blocks.** Completion of a bare name offered only the template struct's fields; it now offers the loop variables and `let` bindings in scope too, tag keywords with the closing tag of the innermost open block first, askama's built-in filters after a pipe, and template files inside `extends`, `include` and `import`. Go to definition takes a path to its file, a `call` to its macro and an overriding block to the parent's. Zed shows rust-analyzer loading as progress, and a Claude Code plugin gives Claude Code's LSP tool the same analysis.

### Changed

- **askama-analyzer is released from its own repository.** Releases, the Claude Code plugin and `cargo install --git` now come from radiosilence/askama-analyzer, so the server ships on its own schedule and any LSP client can use it; the tree-sitter grammar and the Zed extension stay in radiosilence/zed-askama.

### Internal

- **askama's generator is built from the radiosilence/askama fork.** The tooling change is one commit on askama's release tag instead of a vendored copy and a patch file, so it reads as an ordinary diff and a new askama release is a merge.

## 0.1.0

### Added

- **Askama templates get completion, navigation and errors in Zed.** Askama turns a template into Rust inside a derive macro, so rust-analyzer could only report a broken template as an error on the struct. The new Zed extension highlights templates with a grammar of its own and runs `askama-analyzer`, which feeds askama's generated code to the user's own rust-analyzer, as Zed's `lsp.rust-analyzer` settings configure it, and maps completion, hover, definitions, references, renames and errors back to the template. The extension downloads askama-analyzer from this repository's releases and keeps it current, unless `lsp.askama-analyzer.binary` names one.
- **Templates can be formatted.** `askama-analyzer fmt` (and formatting in the editor) gives tags one space inside their delimiters and Rust-style spacing in expressions, filters, patterns and arguments, leaving text outside tags, raw blocks, comments' text and line breaks inside tags as written; it refuses any change that would alter what askama parses, and `--check` lists unformatted templates for CI.
