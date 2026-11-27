//! The template formatter behind `askama-analyzer fmt` and `textDocument/formatting`.
//!
//! Inside tags, expressions are laid out as rustfmt lays out Rust (see `layout`): one space
//! inside `{{ }}`, `{% %}` and `{# #}`, canonical spacing between tokens (`a + b`, `f(a, b)`,
//! `x|filter(y)`, `a..b`), and an expression too long for the line broken over lines indented
//! from the line the tag starts on. Whitespace inside a tag is never rendered. Everything askama
//! renders byte for byte stays as written: text outside tags, `raw` blocks, comments' text,
//! whitespace-control markers and Rust macro arguments (which askama keeps as text).
//!
//! HTML templates are also re-indented by nesting depth (see `html`), which only ever changes
//! the amount of leading whitespace on a line: HTML renders any amount of it the same.
//!
//! The formatter checks its own work. An expression it lays out must parse back to the same
//! `askama_parser` tree, or its tag keeps its tokens' order and line breaks. Indentation must
//! leave every line the same once its leading whitespace is stripped, and the tree the same
//! once template text is set aside. The result must parse to the same tree as the indented
//! template, spans aside, and formatting it again must change nothing. Otherwise it refuses and
//! says where.

mod html;
mod layout;

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use askama_parser::{Ast, Syntax};

/// How a template is formatted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    /// The width lines are kept to, as rustfmt's `max_width`.
    pub max_width: usize,
    /// For an HTML template, the text one level of nesting is indented by; `None` leaves the
    /// indentation outside tags as written.
    pub html_indent: Option<String>,
    /// How many columns a tab takes when measuring a line.
    pub tab_width: usize,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            max_width: 100,
            html_indent: None,
            tab_width: 4,
        }
    }
}

/// Whether the template at `path` is HTML by askama's extensions: `.html` or `.htm`, possibly
/// followed by one of the Jinja suffixes askama looks through.
pub fn is_html(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let name = [".j2", ".jinja", ".jinja2", ".rinja"]
        .iter()
        .find_map(|suffix| name.strip_suffix(suffix))
        .unwrap_or(&name);
    name.ends_with(".html") || name.ends_with(".htm")
}

/// Why a template was not formatted.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum FormatError {
    /// The template does not parse, so there is no tree to keep. Fix the template first.
    Parse(String),
    /// Formatting the tag or line at this position changed what the template means, or would
    /// change again on a second pass. A bug in the formatter; the template is left as it was.
    Unsafe {
        line: usize,
        column: usize,
        tag: String,
    },
}

impl fmt::Display for FormatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Parse(message) => write!(f, "the template does not parse: {message}"),
            Self::Unsafe { line, column, tag } => write!(
                f,
                "formatting at {line}:{column} would change the template, so it was left as written: {tag}"
            ),
        }
    }
}

impl std::error::Error for FormatError {}

/// Formats `source`, a template in `syntax`, returning the formatted text.
pub fn format(source: &str, syntax: &Syntax<'_>, options: &Options) -> Result<String, FormatError> {
    let original = tree(source, syntax).map_err(FormatError::Parse)?;
    let indented = indent(source, syntax, options);
    let base = if indented == source {
        original
    } else {
        if let Some(line) = stripped_difference(source, &indented) {
            return Err(unsafe_at_line(source, line));
        }
        let base = tree(&indented, syntax).map_err(FormatError::Parse)?;
        if blank_strings_after(&base, "Lit(Lit { ", 3)
            != blank_strings_after(&original, "Lit(Lit { ", 3)
        {
            return Err(unsafe_at(source, 0..source.len().min(40)));
        }
        base
    };
    let segments = segments(&indented, syntax);
    let formatted = render(&indented, &segments, options, None);
    let again = indent(&formatted, syntax, options);
    let second = render(&again, &self::segments(&again, syntax), options, None);
    if tree(&formatted, syntax).as_ref() == Ok(&base) && second == formatted {
        return Ok(formatted);
    }
    // Find the first tag that, formatted on its own, changes the tree.
    for (index, segment) in segments.iter().enumerate() {
        if !matches!(segment, Segment::Tag { .. }) {
            continue;
        }
        let alone = render(&indented, &segments, options, Some(index));
        if tree(&alone, syntax).as_ref() != Ok(&base) {
            return Err(unsafe_at(&indented, segment.range()));
        }
    }
    // Then the first line a second pass changes.
    let line = second
        .split('\n')
        .zip(formatted.split('\n'))
        .position(|(a, b)| a != b)
        .unwrap_or_default();
    Err(unsafe_at_line(&formatted, line))
}

/// `source` re-indented, when `options` asks for HTML indentation.
fn indent(source: &str, syntax: &Syntax<'_>, options: &Options) -> String {
    match &options.html_indent {
        Some(unit) => html::reindent(source, &segments(source, syntax), unit),
        None => source.to_owned(),
    }
}

/// The index of the first line that differs between `a` and `b` once each line's leading
/// whitespace is stripped.
fn stripped_difference(a: &str, b: &str) -> Option<usize> {
    let strip = |text: &str| -> Vec<String> {
        text.split('\n')
            .map(|line| line.trim_start_matches([' ', '\t']).to_owned())
            .collect()
    };
    let (a, b) = (strip(a), strip(b));
    (0..a.len().max(b.len())).find(|&index| a.get(index) != b.get(index))
}

fn unsafe_at_line(source: &str, line: usize) -> FormatError {
    let text = source.split('\n').nth(line).unwrap_or_default();
    FormatError::Unsafe {
        line: line + 1,
        column: 1,
        tag: text.trim().chars().take(80).collect(),
    }
}

fn unsafe_at(source: &str, range: std::ops::Range<usize>) -> FormatError {
    let before = &source[..range.start];
    let line = before.matches('\n').count() + 1;
    let column = before.len() - before.rfind('\n').map_or(0, |newline| newline + 1) + 1;
    let tag: String = source[range].chars().take(80).collect();
    FormatError::Unsafe { line, column, tag }
}

/// The template's tree without spans, which `Debug` leaves out, and without comments' text,
/// whose spacing the formatter may change.
fn tree(source: &str, syntax: &Syntax<'_>) -> Result<String, String> {
    let ast = Ast::from_str(source, None, syntax).map_err(|error| error.to_string())?;
    Ok(blank_strings_after(
        &format!("{:?}", ast.nodes()),
        "Comment { ws: ",
        1,
    ))
}

/// `debug` with the first `count` string literals after each `marker` emptied.
fn blank_strings_after(debug: &str, marker: &str, count: usize) -> String {
    let mut out = String::with_capacity(debug.len());
    let mut rest = debug;
    while let Some(start) = rest.find(marker) {
        out.push_str(&rest[..start + marker.len()]);
        rest = &rest[start + marker.len()..];
        for _ in 0..count {
            let Some(quote) = rest.find('"') else {
                break;
            };
            out.push_str(&rest[..=quote]);
            let bytes = rest.as_bytes();
            let mut end = quote + 1;
            while end < bytes.len() && bytes[end] != b'"' {
                end += if bytes[end] == b'\\' { 2 } else { 1 };
            }
            rest = &rest[end.min(rest.len())..];
            if let Some(after) = rest.strip_prefix('"') {
                out.push('"');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// A part of the template: text and raw blocks copied as they are, tags formatted.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Segment {
    Verbatim(std::ops::Range<usize>),
    /// A `raw` block, its tags included.
    Raw(std::ops::Range<usize>),
    Tag {
        kind: TagKind,
        /// The whole tag, delimiters included.
        range: std::ops::Range<usize>,
        /// Between the delimiters and any whitespace-control markers.
        inner: std::ops::Range<usize>,
    },
}

impl Segment {
    fn range(&self) -> std::ops::Range<usize> {
        match self {
            Self::Verbatim(range) | Self::Raw(range) | Self::Tag { range, .. } => range.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TagKind {
    Expression,
    Block,
    Comment,
}

fn is_marker(byte: u8) -> bool {
    matches!(byte, b'-' | b'+' | b'~')
}

/// Splits `source` into text, raw blocks and tags, by the delimiters of `syntax`.
fn segments(source: &str, syntax: &Syntax<'_>) -> Vec<Segment> {
    let bytes = source.as_bytes();
    let mut out = Vec::new();
    let mut text_start = 0;
    let mut i = 0;
    while i < bytes.len() {
        let rest = &source[i..];
        let (kind, open, close) = if rest.starts_with(syntax.expr_start) {
            (TagKind::Expression, syntax.expr_start, syntax.expr_end)
        } else if rest.starts_with(syntax.block_start) {
            (TagKind::Block, syntax.block_start, syntax.block_end)
        } else if rest.starts_with(syntax.comment_start) {
            (TagKind::Comment, syntax.comment_start, syntax.comment_end)
        } else {
            i += rest.chars().next().map_or(1, char::len_utf8);
            continue;
        };
        let found = match kind {
            TagKind::Comment => comment_end(source, i + open.len(), syntax),
            _ => tag_end(source, i + open.len(), close),
        };
        let Some(end) = found else {
            // An unclosed tag: the rest is left alone (the template does not parse anyway).
            break;
        };
        let range = i..end;
        if i > text_start {
            out.push(Segment::Verbatim(text_start..i));
        }
        let mut inner = i + open.len()..end - close.len();
        if inner.start < inner.end && is_marker(bytes[inner.start]) {
            inner.start += 1;
        }
        if inner.start < inner.end && is_marker(bytes[inner.end - 1]) {
            inner.end -= 1;
        }
        if kind == TagKind::Block && first_word(&source[inner.clone()]) == "raw" {
            let end = raw_end(source, end, syntax).unwrap_or(source.len());
            out.push(Segment::Raw(i..end));
            text_start = end;
            i = end;
            continue;
        }
        out.push(Segment::Tag { kind, range, inner });
        text_start = end;
        i = end;
    }
    if text_start < source.len() {
        out.push(Segment::Verbatim(text_start..source.len()));
    }
    out
}

fn first_word(text: &str) -> &str {
    let text = text.trim_start();
    let end = text
        .find(|c: char| !(c == '_' || c.is_alphanumeric()))
        .unwrap_or(text.len());
    &text[..end]
}

/// The end of a tag closed by `close`, after its contents: string and character literals may
/// hold the closing delimiter.
fn tag_end(source: &str, start: usize, close: &str) -> Option<usize> {
    let bytes = source.as_bytes();
    let mut i = start;
    while i < bytes.len() {
        if source[i..].starts_with(close) {
            return Some(i + close.len());
        }
        match bytes[i] {
            b'"' => i = skip_string(bytes, i),
            b'\'' => i = skip_char(bytes, i),
            _ => i += 1,
        }
    }
    None
}

fn skip_string(bytes: &[u8], quote: usize) -> usize {
    let mut i = quote + 1;
    while i < bytes.len() && bytes[i] != b'"' {
        i += if bytes[i] == b'\\' { 2 } else { 1 };
    }
    i + 1
}

/// A character literal (`'x'`, `'\n'`, `'\u{1F600}'`), or else one byte: a lifetime in a
/// macro's arguments.
fn skip_char(bytes: &[u8], quote: usize) -> usize {
    let mut i = quote + 1;
    if bytes.get(i) == Some(&b'\\') {
        i += 2;
        while i < bytes.len() && bytes[i] != b'\'' && i - quote < 12 {
            i += 1;
        }
        return if bytes.get(i) == Some(&b'\'') {
            i + 1
        } else {
            quote + 1
        };
    }
    // One character, which may be several bytes.
    let width = std::str::from_utf8(&bytes[i..bytes.len().min(i + 4)])
        .ok()
        .or_else(|| std::str::from_utf8(&bytes[i..bytes.len().min(i + 1)]).ok())
        .and_then(|text| text.chars().next())
        .map_or(1, char::len_utf8);
    if bytes.get(i + width) == Some(&b'\'') {
        i + width + 1
    } else {
        quote + 1
    }
}

/// The end of a comment whose opening delimiter ends at `start`; comments nest.
fn comment_end(source: &str, start: usize, syntax: &Syntax<'_>) -> Option<usize> {
    let mut depth = 0usize;
    let mut i = start;
    while i < source.len() {
        let rest = &source[i..];
        if rest.starts_with(syntax.comment_start) {
            depth += 1;
            i += syntax.comment_start.len();
        } else if rest.starts_with(syntax.comment_end) {
            i += syntax.comment_end.len();
            if depth == 0 {
                return Some(i);
            }
            depth -= 1;
        } else {
            i += rest.chars().next().map_or(1, char::len_utf8);
        }
    }
    None
}

/// The end of the `endraw` tag closing a raw block whose opening tag ends at `start`, found as
/// askama finds it: the block start, an optional marker, spaces, `endraw`, spaces, an optional
/// marker and the block end.
fn raw_end(source: &str, start: usize, syntax: &Syntax<'_>) -> Option<usize> {
    let mut from = start;
    while let Some(found) = source[from..].find(syntax.block_start) {
        let open = from + found;
        let mut i = open + syntax.block_start.len();
        let bytes = source.as_bytes();
        if i < bytes.len() && is_marker(bytes[i]) {
            i += 1;
        }
        let after = source[i..].trim_start_matches(|c: char| c.is_ascii_whitespace());
        if let Some(after) = after.strip_prefix("endraw") {
            let after = after.trim_start_matches(|c: char| c.is_ascii_whitespace());
            let after = after.strip_prefix(['-', '+', '~']).unwrap_or(after);
            if after.starts_with(syntax.block_end) {
                return Some(source.len() - after.len() + syntax.block_end.len());
            }
        }
        from = open + syntax.block_start.len();
    }
    None
}

/// The template with its tags formatted: all of them, or only the one at `only`.
fn render(source: &str, segments: &[Segment], options: &Options, only: Option<usize>) -> String {
    let mut out = String::with_capacity(source.len());
    for (index, segment) in segments.iter().enumerate() {
        match segment {
            Segment::Tag { kind, range, inner } if only.is_none_or(|only| only == index) => {
                let open = &source[range.start..inner.start];
                let close = &source[inner.end..range.end];
                let body = &source[inner.clone()];
                let line = &out[out.rfind('\n').map_or(0, |at| at + 1)..];
                let laid = match kind {
                    TagKind::Comment => None,
                    TagKind::Expression | TagKind::Block => {
                        lay_out(*kind, body, open, close, line, options)
                    }
                };
                out.push_str(open);
                match (kind, laid) {
                    (_, Some(laid)) => out.push_str(&laid),
                    (TagKind::Comment, None) => out.push_str(&pad(body, body.trim())),
                    (_, None) => match format_tokens(body, true) {
                        Some(tokens) => out.push_str(&pad(body, &tokens)),
                        None => out.push_str(body),
                    },
                }
                out.push_str(close);
            }
            other => out.push_str(&source[other.range()]),
        }
    }
    out
}

/// A part of a tag: its own words, spaced as tokens, or an expression, laid out.
enum Piece {
    Text(std::ops::Range<usize>),
    Expr(std::ops::Range<usize>),
}

/// The inside of a tag that starts after `line` (the text before it on its line), with its
/// expressions laid out; `None` when the tag holds no expression the printer lays out, or one
/// it cannot print back to the same tree, so the tag keeps its tokens' layout.
fn lay_out(
    kind: TagKind,
    body: &str,
    open: &str,
    close: &str,
    line: &str,
    options: &Options,
) -> Option<String> {
    let tokens = tokenize(body)?;
    let pieces = pieces(kind, &tokens)?;
    let columns = |text: &str| -> usize {
        text.chars()
            .map(|c| if c == '\t' { options.tab_width } else { 1 })
            .sum()
    };
    let base = &line[..line.len() - line.trim_start_matches([' ', '\t']).len()];
    let base_width = columns(base);
    let max_width = options.max_width;

    let mut parts = Vec::with_capacity(pieces.len());
    for piece in &pieces {
        parts.push(match piece {
            Piece::Text(range) => (None, format_tokens(&body[range.clone()], false)?),
            Piece::Expr(range) => {
                let parsed = layout::Parsed::new(&body[range.clone()]);
                let flat = parsed.with_expr(|expr, source| {
                    layout::Printer::new(base, base_width, max_width, source).flat(expr)
                })??;
                (Some(parsed), flat)
            }
        });
    }

    let mut out = String::from(" ");
    let mut col = columns(line) + columns(open) + 1;
    for (index, (parsed, flat)) in parts.iter().enumerate() {
        if index > 0 {
            out.push(' ');
            col += 1;
        }
        let trail = parts[index + 1..]
            .iter()
            .map(|(_, flat)| 1 + layout::width(flat))
            .sum::<usize>()
            + 1
            + columns(close);
        let text = match parsed {
            None => flat.clone(),
            Some(parsed) => {
                let laid = parsed.with_expr(|expr, source| {
                    layout::Printer::new(base, base_width, max_width, source)
                        .layout(expr, 0, col, trail)
                })??;
                if layout::Parsed::new(&laid).tree()? != parsed.tree()? {
                    return None;
                }
                laid
            }
        };
        col = layout::Printer::new(base, base_width, max_width, "").end_column(&text, col);
        out.push_str(&text);
    }
    out.push(' ');
    Some(out)
}

/// A tag's tokens split into its words and the expressions it holds, or `None` for a tag
/// that holds none the printer lays out.
fn pieces(kind: TagKind, tokens: &[Token<'_>]) -> Option<Vec<Piece>> {
    let n = tokens.len();
    let span =
        |from: usize, to: usize| (from < to).then(|| tokens[from].start..tokens[to - 1].end());
    let is = |index: usize, kind: Kind, text: &str| {
        tokens
            .get(index)
            .is_some_and(|token| token.kind == kind && token.text == text)
    };
    // The first token at or after `from`, outside any group, that is `text`.
    let find = |from: usize, kind: Kind, text: &str| {
        let mut depth = 0usize;
        for (index, token) in tokens.iter().enumerate().skip(from) {
            if depth == 0 && token.kind == kind && token.text == text {
                return Some(index);
            }
            match token.text {
                "(" | "[" | "{" => depth += 1,
                ")" | "]" | "}" => depth = depth.saturating_sub(1),
                _ => {}
            }
        }
        None
    };
    // A text and the expression after it.
    let after = |at: usize| Some(vec![Piece::Text(span(0, at)?), Piece::Expr(span(at, n)?)]);
    let condition = |at: usize| match is(at, Kind::Word, "let") {
        true => after(find(at, Kind::Punct, "=")? + 1),
        false => after(at),
    };
    if kind == TagKind::Expression {
        return Some(vec![Piece::Expr(span(0, n)?)]);
    }
    match tokens.first()?.text {
        "if" | "elif" => condition(1),
        "else" if is(1, Kind::Word, "if") => condition(2),
        "for" => {
            let at = find(1, Kind::Word, "in")? + 1;
            match find(at, Kind::Word, "if") {
                Some(cond) => Some(vec![
                    Piece::Text(span(0, at)?),
                    Piece::Expr(span(at, cond)?),
                    Piece::Text(span(cond, cond + 1)?),
                    Piece::Expr(span(cond + 1, n)?),
                ]),
                None => after(at),
            }
        }
        "let" | "set" => after(find(1, Kind::Punct, "=")? + 1),
        "match" | "macro" => after(1),
        "call" if is(1, Kind::Punct, "(") => after(find(2, Kind::Punct, ")")? + 1),
        "call" => after(1),
        _ => None,
    }
}

/// `formatted` with one space on each side, except where the original's whitespace on that
/// side holds a line break, which is kept. An empty tag stays empty.
fn pad(original: &str, formatted: &str) -> String {
    if formatted.is_empty() {
        return original.to_owned();
    }
    let leading = &original[..original.len() - original.trim_start().len()];
    let trailing = &original[original.trim_end().len()..];
    let side = |space: &str| {
        if space.contains('\n') {
            space.to_owned()
        } else {
            " ".to_owned()
        }
    };
    format!("{}{}{}", side(leading), formatted, side(trailing))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Word,
    Literal,
    Punct,
    /// A Rust macro's arguments, delimiters included, kept as written.
    Verbatim,
}

#[derive(Debug, Clone, Copy)]
struct Token<'a> {
    kind: Kind,
    text: &'a str,
    /// Where the token starts in the tag's body.
    start: usize,
    /// The whitespace before the token, when it holds a line break and so is kept.
    kept_gap: Option<&'a str>,
}

impl Token<'_> {
    fn end(&self) -> usize {
        self.start + self.text.len()
    }
}

const PUNCTUATION: &[&str] = &[
    "..=", "<<=", ">>=", "...", "::", "..", "==", "!=", "<=", ">=", "&&", "||", "<<", ">>", "+=",
    "-=", "*=", "/=", "%=", "&=", "|=", "^=", "=>", "->", "+", "-", "*", "/", "%", "&", "|", "^",
    "!", "=", "<", ">", "(", ")", "[", "]", "{", "}", ",", ";", ":", ".", "?", "@", "~", "#",
];

/// The tag's tokens, or `None` when something in it is not understood, in which case the tag
/// is left as written.
fn tokenize(body: &str) -> Option<Vec<Token<'_>>> {
    let bytes = body.as_bytes();
    let mut tokens: Vec<Token<'_>> = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let gap_start = i;
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i == bytes.len() {
            break;
        }
        let gap = &body[gap_start..i];
        let kept_gap = (gap.contains('\n') && !tokens.is_empty()).then_some(gap);
        let start = i;
        let c = bytes[i];
        let rest = &body[i..];
        let kind = if c == b'"' || (matches!(c, b'b' | b'c') && bytes.get(i + 1) == Some(&b'"')) {
            i = skip_string(bytes, i + usize::from(c != b'"'));
            Kind::Literal
        } else if c == b'\'' || (c == b'b' && bytes.get(i + 1) == Some(&b'\'')) {
            let quote = i + usize::from(c == b'b');
            i = skip_char(bytes, quote);
            if i == quote + 1 {
                return None;
            }
            Kind::Literal
        } else if c.is_ascii_digit() {
            i = number_end(bytes, i);
            Kind::Literal
        } else if c == b'_' || rest.chars().next().is_some_and(char::is_alphabetic) {
            if rest.starts_with("r#") {
                i += 2;
            }
            i += body[i..]
                .find(|ch: char| !(ch == '_' || ch.is_alphanumeric()))
                .unwrap_or(body.len() - i);
            Kind::Word
        } else {
            // Anything else is not understood, and the tag is left as written.
            let punct = PUNCTUATION.iter().find(|p| rest.starts_with(**p))?;
            i += punct.len();
            // A Rust macro call: `name!` then a group, kept as written.
            if *punct == "!" && tokens.last().is_some_and(|t| t.kind == Kind::Word) {
                let after = body[i..].trim_start();
                if after.starts_with(['(', '[', '{']) {
                    tokens.push(Token {
                        kind: Kind::Punct,
                        text: "!",
                        start,
                        kept_gap,
                    });
                    let group_start = body.len() - after.len();
                    let group_end = group_end(body, group_start)?;
                    tokens.push(Token {
                        kind: Kind::Verbatim,
                        text: &body[group_start..group_end],
                        start: group_start,
                        kept_gap: None,
                    });
                    i = group_end;
                    continue;
                }
            }
            Kind::Punct
        };
        tokens.push(Token {
            kind,
            text: &body[start..i],
            start,
            kept_gap,
        });
    }
    Some(tokens)
}

fn number_end(bytes: &[u8], start: usize) -> usize {
    let hex = bytes
        .get(start + 1)
        .is_some_and(|b| matches!(b, b'x' | b'X'))
        && bytes[start] == b'0';
    let mut i = start;
    while i < bytes.len() {
        let b = bytes[i];
        // Digits, a suffix, a fraction (`1.5` but not `1..`), and an exponent's sign.
        let fraction = b == b'.' && bytes.get(i + 1).is_some_and(u8::is_ascii_digit);
        let exponent_sign = matches!(b, b'+' | b'-')
            && !hex
            && matches!(bytes[i - 1], b'e' | b'E')
            && bytes
                .get(i + 1)
                .is_some_and(|n| n.is_ascii_digit() || *n == b'_');
        if b.is_ascii_alphanumeric() || b == b'_' || fraction || exponent_sign {
            i += 1;
        } else {
            break;
        }
    }
    i
}

/// The end of the balanced group opening at `start`, string and character literals skipped.
fn group_end(body: &str, start: usize) -> Option<usize> {
    let bytes = body.as_bytes();
    let mut depth = 0usize;
    let mut i = start;
    while i < bytes.len() {
        match bytes[i] {
            b'(' | b'[' | b'{' => {
                depth += 1;
                i += 1;
            }
            b')' | b']' | b'}' => {
                depth -= 1;
                i += 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            b'"' => i = skip_string(bytes, i),
            b'\'' => i = skip_char(bytes, i),
            _ => i += 1,
        }
    }
    None
}

/// Words after which an operand starts, so a following `-`, `*`, `&` or `!` is a prefix
/// operator and `(` or `[` opens a group rather than a call or an index.
const LEADING_KEYWORDS: &[&str] = &[
    "if", "elif", "else", "for", "in", "match", "when", "let", "set", "mut", "is", "not", "as",
    "or", "with", "xor", "bitand", "bitor", "filter", "extends", "include", "import", "block",
    "macro", "decl", "declare", "return",
];

fn is_leading_keyword(token: &Token<'_>) -> bool {
    token.kind == Kind::Word && LEADING_KEYWORDS.contains(&token.text)
}

/// Whether `token` ends an operand, so what follows is a suffix or a binary operator.
fn ends_operand(token: &Token<'_>, generic_close: bool) -> bool {
    match token.kind {
        Kind::Word => !is_leading_keyword(token),
        Kind::Literal | Kind::Verbatim => true,
        Kind::Punct => matches!(token.text, ")" | "]" | "}" | "?") || generic_close,
    }
}

/// The tag's tokens with canonical spacing, or `None` when the tag is not understood. With
/// `keep_breaks`, whitespace between tokens that holds a line break is kept as written.
fn format_tokens(body: &str, keep_breaks: bool) -> Option<String> {
    let tokens = tokenize(body)?;
    let tag = tokens
        .first()
        .filter(|t| t.kind == Kind::Word)
        .map(|t| t.text);
    let mut out = String::new();
    // Open groups, to tell a `when` arm's `|` alternatives from a filter's `|`.
    let mut depth = 0usize;
    // Generic arguments: after `::<`, or in a type after `:` in `let` and `macro` tags.
    let mut generics = 0usize;
    let mut in_type = false;
    let mut type_depth = 0usize;
    // Whether the previous token was a prefix operator, and whether it closed generics.
    let mut prefix = false;
    let mut generic_close = false;

    for (index, token) in tokens.iter().enumerate() {
        let previous = index.checked_sub(1).map(|i| tokens[i]);
        let text = token.text;

        let opens_generics = text == "<"
            && (previous.is_some_and(|p| p.text == "::")
                || (in_type && generics == 0)
                || generics > 0);
        let closes_generics = generics > 0 && matches!(text, ">" | ">>");
        let is_prefix = token.kind == Kind::Punct
            && matches!(text, "-" | "*" | "&" | "&&" | "!")
            && previous.is_none_or(|p| !ends_operand(&p, generic_close));

        let space = match previous {
            None => false,
            Some(previous) => {
                let p = previous.text;
                if prefix {
                    false
                } else if matches!(text, "," | ";" | ")" | "]" | "?" | "." | "::")
                    || matches!(p, "(" | "[" | "." | "::")
                {
                    // `::std` after a keyword keeps its space.
                    text == "::" && is_leading_keyword(&previous)
                } else if matches!(p, "," | ";") || text == "{" {
                    true
                } else if p == "{" {
                    text != "}"
                } else if text == "}" {
                    true
                } else if text == ":"
                    || opens_generics
                    || closes_generics
                    || (generic_close && matches!(text, "(" | "::"))
                    || (p == "<" && generics > 0)
                    || matches!(text, ".." | "..=")
                    || matches!(p, ".." | "..=")
                {
                    // Tight: a type or field's colon, generics, ranges.
                    false
                } else if text == "|" || p == "|" {
                    tag == Some("when") && depth == 0
                } else if text == "(" || text == "[" {
                    !ends_operand(&previous, generic_close) && !(p == "call" && text == "(")
                } else if text == "!"
                    && previous.kind == Kind::Word
                    && tokens
                        .get(index + 1)
                        .is_some_and(|n| n.kind == Kind::Verbatim)
                {
                    false
                } else {
                    !(p == "!" && token.kind == Kind::Verbatim)
                }
            }
        };

        if let Some(gap) = token.kept_gap.filter(|_| keep_breaks) {
            out.push_str(gap);
        } else if space {
            out.push(' ');
        }
        out.push_str(text);

        match text {
            "(" | "[" | "{" => depth += 1,
            ")" | "]" | "}" => depth = depth.saturating_sub(1),
            _ => {}
        }
        if text == ":" && matches!(tag, Some("let" | "set" | "macro")) {
            in_type = true;
            type_depth = depth;
        } else if in_type && depth <= type_depth && generics == 0 && matches!(text, "=" | "," | ")")
        {
            in_type = false;
        }
        if opens_generics {
            generics += 1;
        }
        generic_close = closes_generics;
        if closes_generics {
            generics = generics.saturating_sub(if text == ">>" { 2 } else { 1 });
        }
        prefix = is_prefix;
    }
    Some(out)
}

/// `askama-analyzer fmt`: formats the templates at `paths` in place or, with `check`, lists
/// those that are not formatted. A directory means every file under a `templates` directory
/// in it. Templates use askama's default delimiters. Lines are kept to `max_width`, and HTML
/// templates are indented by `indent` per level.
pub fn run(
    check: bool,
    paths: &[PathBuf],
    max_width: usize,
    indent: &str,
) -> anyhow::Result<ExitCode> {
    let mut files = Vec::new();
    for path in paths {
        if path.is_dir() {
            templates_under(path, false, &mut files);
        } else {
            files.push(path.clone());
        }
    }
    files.sort();
    let syntax = Syntax::default();
    let mut failed = false;
    for file in &files {
        let source = match fs::read_to_string(file) {
            Ok(source) => source,
            Err(error) => {
                eprintln!("{}: {error}", file.display());
                failed = true;
                continue;
            }
        };
        let options = Options {
            max_width,
            html_indent: is_html(file).then(|| indent.to_owned()),
            ..Options::default()
        };
        match format(&source, &syntax, &options) {
            Ok(formatted) if formatted == source => {}
            Ok(_) if check => {
                println!("{}", file.display());
                failed = true;
            }
            Ok(formatted) => {
                if let Err(error) = fs::write(file, formatted) {
                    eprintln!("{}: {error}", file.display());
                    failed = true;
                }
            }
            Err(error) => {
                eprintln!("{}: {error}", file.display());
                failed = true;
            }
        }
    }
    Ok(if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

fn templates_under(directory: &Path, inside: bool, found: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        if path.is_dir() {
            if matches!(name.to_str(), Some("target" | "node_modules" | ".git")) {
                continue;
            }
            templates_under(&path, inside || name == "templates", found);
        } else if inside {
            found.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn formatted_with(source: &str, options: &Options) -> String {
        let once = format(source, &Syntax::default(), options)
            .unwrap_or_else(|error| panic!("{source:?} should format: {error}"));
        let twice = format(&once, &Syntax::default(), options)
            .unwrap_or_else(|error| panic!("{once:?} should format again: {error}"));
        assert_eq!(twice, once, "formatting twice changes {source:?}");
        once
    }

    fn formatted(source: &str) -> String {
        formatted_with(source, &Options::default())
    }

    fn html(source: &str) -> String {
        formatted_with(
            source,
            &Options {
                html_indent: Some("  ".to_owned()),
                ..Options::default()
            },
        )
    }

    #[test]
    fn tags_get_one_space_inside_their_delimiters() {
        assert_eq!(
            formatted("{{x}}{%if a%}{%endif%}{#note#}"),
            "{{ x }}{% if a %}{% endif %}{# note #}"
        );
    }

    #[test]
    fn whitespace_control_markers_stay_against_their_delimiters() {
        assert_eq!(
            formatted("{{-x+}}{%~ if a-%}{%endif%}{#-c-#}"),
            "{{- x +}}{%~ if a -%}{% endif %}{#- c -#}"
        );
    }

    #[test]
    fn text_outside_tags_is_never_touched() {
        let source = "  <p class=\"a  b\">\n\t{{ x }}  </p>  {  }} %} ";
        assert_eq!(formatted(source), source);
    }

    #[test]
    fn binary_operators_get_spaces_and_prefix_operators_none() {
        assert_eq!(formatted("{{a+b*-c}}"), "{{ a + b * -c }}");
        assert_eq!(
            formatted("{% if !a&&b==c||d %}{% endif %}"),
            "{% if !a && b == c || d %}{% endif %}"
        );
        assert_eq!(formatted("{{ a   ~   b }}"), "{{ a ~ b }}");
        assert_eq!(formatted("{{ & x }}{{ * y }}"), "{{ &x }}{{ *y }}");
    }

    #[test]
    fn calls_arguments_and_filters_are_spaced_as_rust_writes_them() {
        assert_eq!(
            formatted("{{ f ( a ,b , c = 1 ) }}"),
            "{{ f(a, b, c = 1) }}"
        );
        assert_eq!(
            formatted("{{ x | upper | truncate( 3 ) }}"),
            "{{ x|upper|truncate(3) }}"
        );
        assert_eq!(formatted("{{ a . b [ 0 ] ? . c }}"), "{{ a.b[0]?.c }}");
    }

    #[test]
    fn ranges_paths_and_turbofish_are_tight() {
        assert_eq!(
            formatted("{{ x[ 1 .. ] }}{{ a ..= b }}"),
            "{{ x[1..] }}{{ a..=b }}"
        );
        assert_eq!(
            formatted("{{ Vec :: < u8 > :: new ( ) }}"),
            "{{ Vec::<u8>::new() }}"
        );
    }

    #[test]
    fn struct_literals_and_patterns_get_rusts_spacing() {
        assert_eq!(formatted("{{ P{x:1,y} }}"), "{{ P { x: 1, y } }}");
        assert_eq!(
            formatted("{% match v %}{% when Some with(x)|None %}{% when P{a,..} %}{% endmatch %}"),
            "{% match v %}{% when Some with (x) | None %}{% when P { a, .. } %}{% endmatch %}"
        );
    }

    #[test]
    fn is_defined_and_casts_keep_their_words_apart() {
        assert_eq!(
            formatted("{% if x  is  not   defined %}{% endif %}"),
            "{% if x is not defined %}{% endif %}"
        );
        assert_eq!(formatted("{{ ( a ) as u8 }}"), "{{ (a) as u8 }}");
    }

    #[test]
    fn types_in_let_and_macro_tags_keep_generics_tight() {
        assert_eq!(
            formatted("{% let x : Vec < u8 > = v %}"),
            "{% let x: Vec<u8> = v %}"
        );
        assert_eq!(
            formatted("{% macro m( a : & str , b = 1 ) %}{% endmacro %}"),
            "{% macro m(a: &str, b = 1) %}{% endmacro %}"
        );
    }

    #[test]
    fn rust_macro_arguments_are_kept_as_written() {
        assert_eq!(
            formatted("{{ format !( \"{}\" ,x ) }}"),
            "{{ format!( \"{}\" ,x ) }}"
        );
    }

    #[test]
    fn raw_blocks_and_comment_text_are_kept_as_written() {
        let raw = "{%raw%}{{ a+b }}{%endraw%}";
        assert_eq!(formatted(raw), raw);
        assert_eq!(formatted("{#  a  +  b  #}"), "{# a  +  b #}");
    }

    #[test]
    fn a_short_tag_laid_out_over_lines_is_joined_onto_one() {
        let source = "{% call kit::card(\n    title ,\n    body\n) %}{% endcall %}";
        assert_eq!(
            formatted(source),
            "{% call kit::card(title, body) %}{% endcall %}"
        );
        assert_eq!(formatted("{{\n  a\n  + b\n}}"), "{{ a + b }}");
    }

    /// rustfmt's layout of the same expression as a statement indented four spaces.
    #[test]
    fn a_long_call_breaks_its_arguments_and_chains_as_rustfmt_does() {
        let source = "    {{ kit::Combo::new(kit::Input::url(\"url\").label(\"Download link\").placeholder(\"Download link, such as a Bandcamp purchase\").autocomplete(\"off\").required(), import.submit(kit::Button::primary(\"Import\"))).compact() }}\n";
        let expected = r#"    {{ kit::Combo::new(
        kit::Input::url("url")
            .label("Download link")
            .placeholder("Download link, such as a Bandcamp purchase")
            .autocomplete("off")
            .required(),
        import.submit(kit::Button::primary("Import")),
    )
    .compact() }}
"#;
        assert_eq!(formatted(source), expected);
    }

    #[test]
    fn a_lone_call_argument_breaks_in_place() {
        let source = "{{ foo(aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa(bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb, ccccccccccccccccccccccccc, d)) }}";
        let expected = "{{ foo(aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa(\n    bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb,\n    ccccccccccccccccccccccccc,\n    d,\n)) }}";
        assert_eq!(formatted(source), expected);
    }

    #[test]
    fn a_chain_on_a_short_root_keeps_its_first_link_on_the_root_line() {
        let source = "{{ self.foooooooooooooooooooooooooo.barrrrrrrrrrrrrrrrrrrrrrrrrrrrr().bazzzzzzzzzzzzzzzzzzzzzzzzzzzzzz() }}";
        let expected = "{{ self.foooooooooooooooooooooooooo\n    .barrrrrrrrrrrrrrrrrrrrrrrrrrrrr()\n    .bazzzzzzzzzzzzzzzzzzzzzzzzzzzzzz() }}";
        assert_eq!(formatted(source), expected);
        let source = "{{ x.y(aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa, bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb) }}";
        let expected = "{{ x.y(\n    aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa,\n    bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb,\n) }}";
        assert_eq!(formatted(source), expected);
    }

    #[test]
    fn binary_operators_break_before_the_operator() {
        let source = "{% if aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa && bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb || cccccccccccccccccccccc %}{% endif %}";
        let expected = "{% if aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa && bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\n    || cccccccccccccccccccccc %}{% endif %}";
        assert_eq!(formatted(source), expected);
        let source = "{{ aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa ~ bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb ~ ccccccccccccccc }}";
        let expected = "{{ aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n    ~ bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\n    ~ ccccccccccccccc }}";
        assert_eq!(formatted(source), expected);
    }

    #[test]
    fn askama_syntax_is_written_as_askama_writes_it() {
        assert_eq!(
            formatted("{{ a bitor b xor c bitand d }}{{ x  |  f ( y = 1 ) }}"),
            "{{ a bitor b xor c bitand d }}{{ x|f(y = 1) }}"
        );
        let source = "{{ value|truncate(aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa, bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb)|safe }}";
        let expected = "{{ value|truncate(\n    aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa,\n    bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb,\n)|safe }}";
        assert_eq!(formatted(source), expected);
    }

    #[test]
    fn block_tags_lay_out_their_expressions() {
        let source = "{% for item in items.iter().filter(aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa).map(bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb) %}{% endfor %}";
        let expected = "{% for item in items\n    .iter()\n    .filter(aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa)\n    .map(bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb) %}{% endfor %}";
        assert_eq!(formatted(source), expected);
        assert_eq!(
            formatted("{% let  x = f( a ) %}{% if let Some( y ) = x %}{% endif %}"),
            "{% let x = f(a) %}{% if let Some(y) = x %}{% endif %}"
        );
        assert_eq!(
            formatted(
                "{% macro m( a , b = 1 ) %}{% endmacro %}{% call( c ) m( 1 ) %}{% endcall %}"
            ),
            "{% macro m(a, b = 1) %}{% endmacro %}{% call(c) m(1) %}{% endcall %}"
        );
    }

    #[test]
    fn whitespace_control_markers_stay_on_a_broken_tag() {
        let source = "{{- f(aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa, bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb) ~}}";
        let expected = "{{- f(\n    aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa,\n    bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb,\n) ~}}";
        assert_eq!(formatted(source), expected);
    }

    #[test]
    fn max_width_is_configurable() {
        let options = Options {
            max_width: 20,
            ..Options::default()
        };
        assert_eq!(
            formatted_with("{{ f(alpha, beta, gamma) }}", &options),
            "{{ f(\n    alpha,\n    beta,\n    gamma,\n) }}"
        );
    }

    #[test]
    fn html_is_indented_by_nesting_and_kept_where_whitespace_matters() {
        let source = "\
<div>
<ul>
{% for x in xs %}
<li>{{ x }}</li>
{% endfor %}
</ul>
        <pre>
  keep
    this
</pre>
{% raw %}
   {{ raw }}
{% endraw %}
{% if a %}
<p class=\"x\"
   id=\"y\">
text
</p>
{% else if b %}
<br>
<img src=\"a\" />
{% else %}
{# a
   comment #}
{% endif %}
{% match v %}
{% when Some(x) %}
{{ x }}
{% when None %}
{% endmatch %}
<script>
    if (a < b) {}
</script>
</div>
";
        let expected = "\
<div>
  <ul>
    {% for x in xs %}
      <li>{{ x }}</li>
    {% endfor %}
  </ul>
  <pre>
  keep
    this
</pre>
  {% raw %}
   {{ raw }}
{% endraw %}
  {% if a %}
    <p class=\"x\"
       id=\"y\">
      text
    </p>
  {% else if b %}
    <br>
    <img src=\"a\" />
  {% else %}
    {# a
   comment #}
  {% endif %}
  {% match v %}
    {% when Some(x) %}
      {{ x }}
    {% when None %}
  {% endmatch %}
  <script>
    if (a < b) {}
</script>
</div>
";
        assert_eq!(html(source), expected);
    }

    #[test]
    fn html_indentation_never_adds_or_removes_a_line_break() {
        let source = "<div><p>{% if a %}x{% endif %}</p>\n<span>y</span></div>\n\n   \n<i>z</i>";
        assert_eq!(
            html(source),
            "<div><p>{% if a %}x{% endif %}</p>\n  <span>y</span></div>\n\n\n<i>z</i>"
        );
    }

    #[test]
    fn a_broken_tag_is_indented_from_its_lines_new_indentation() {
        let source = "<div>\n{{ f(aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa, bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb) }}\n</div>";
        let expected = "<div>\n  {{ f(\n      aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa,\n      bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb,\n  ) }}\n</div>";
        assert_eq!(html(source), expected);
    }

    #[test]
    fn templates_that_are_not_html_keep_their_indentation() {
        let source = "{% if a %}\nx\n    {% endif %}\n";
        assert_eq!(formatted(source), source);
        assert!(is_html(Path::new("templates/a.html")));
        assert!(is_html(Path::new("a.HTM.j2")));
        assert!(!is_html(Path::new("a.txt")));
        assert!(!is_html(Path::new("a.md")));
        assert!(!is_html(Path::new("a.xml")));
    }

    #[test]
    fn a_template_that_does_not_parse_is_refused() {
        let error = format("{{ a +", &Syntax::default(), &Options::default())
            .expect_err("an unclosed tag does not parse");
        assert!(
            matches!(error, FormatError::Parse(_)),
            "expected a parse error, got {error:?}"
        );
    }

    #[test]
    fn formatting_twice_changes_nothing() {
        let once = formatted("{%for x in xs if x>0%}{{x|f(1,2)}}{%endfor%}");
        assert_eq!(formatted(&once), once);
    }
}
