//! Indentation of HTML templates by nesting depth.
//!
//! The rule that keeps it safe: only the amount of leading whitespace on an existing line
//! changes. No line break is added or removed and nothing inside a line moves. In HTML a line
//! break followed by any amount of whitespace renders as the same single space. The first line,
//! which no line break precedes, keeps its whitespace, and so do the places where whitespace is
//! kept as written, line by line: `<pre>`, `<textarea>`, `<script>` and `<style>` contents,
//! HTML comments, `raw` blocks, the bodies of `filter` and block `let` tags (which capture
//! rather than render), the bodies of blocks the templates this one extends render in one of
//! those places (all of them, when those templates cannot be read), and lines that continue an
//! askama tag, which are laid out with its expression.
//!
//! What a file cannot see is the one way the output can change: a macro or an included
//! template rendered inside another file's `<pre>`, or an element styled `white-space: pre`. Lines that continue an askama comment,
//! or an HTML tag (unless they start inside an attribute's value), move with the line the
//! comment or tag starts on.
//!
//! `start_tags` finds the HTML start tags a long line may break between attributes, which is
//! whitespace HTML never renders either.

use std::collections::HashSet;
use std::ops::Range;

use super::{Segment, TagKind, first_word};

const VOID: &[&str] = &[
    "area", "base", "br", "col", "command", "embed", "hr", "img", "input", "keygen", "link",
    "meta", "param", "source", "track", "wbr",
];

/// Elements whose contents keep their whitespace (or are not HTML at all).
const VERBATIM: &[&str] = &["pre", "textarea", "script", "style", "xmp", "listing"];

/// Elements an author may leave unclosed, closed by the next one of the same name.
const SELF_ENDING: &[&str] = &["li", "option", "tr", "td", "th", "dt", "dd", "p"];

/// An askama tag with a body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tag {
    If,
    For,
    Match,
    When,
    Block,
    Macro,
    Filter,
    Call,
    Let,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Entry {
    Element(String),
    Block(Tag),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Line {
    /// Left exactly as written.
    Keep,
    /// Indented this many levels.
    Depth(usize),
    /// Moved with the line at this index, where the tag it continues starts.
    Shift(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Html {
    Text,
    /// Inside an HTML tag; `line` is the index of the line it starts on.
    Tag {
        line: usize,
        quote: Option<u8>,
        /// The element the tag opens, once its `>` is reached; `None` for end tags,
        /// doctypes and the like.
        opens: Option<String>,
        last: u8,
    },
    Comment,
    /// Inside an element whose contents are kept, until its end tag.
    Verbatim(String),
}

/// The `{% block %}` bodies of a template whose whitespace may be rendered as written, where
/// the template that renders them puts them: inside `<pre>`, `<textarea>`, `<script>` or
/// `<style>`, or in a `filter` or block `let`. `All` when that template could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Blocks {
    All,
    Named(HashSet<String>),
}

impl Blocks {
    fn contains(&self, name: &str) -> bool {
        match self {
            Self::All => true,
            Self::Named(names) => names.contains(name),
        }
    }
}

struct Indenter<'b> {
    stack: Vec<Entry>,
    html: Html,
    lines: Vec<Line>,
    /// Whether everything on the current line so far is whitespace or closes something, so a
    /// closer there takes the line out a level.
    head: bool,
    /// The blocks whose bodies are kept as written.
    kept_blocks: &'b Blocks,
    /// The depth of the stack at the block, `filter` or `let` whose body is being kept.
    kept_from: Option<usize>,
    /// The blocks opened where their bodies are kept, for templates that extend this one.
    kept_names: HashSet<String>,
    /// Where the kept bodies are in the template.
    kept_bodies: Vec<Range<usize>>,
}

impl Indenter<'_> {
    fn keeping(&self) -> bool {
        self.kept_from.is_some_and(|depth| self.stack.len() > depth)
    }

    fn new_line(&mut self, inside_tag: bool) {
        let line = if inside_tag || self.keeping() {
            Line::Keep
        } else {
            match &self.html {
                Html::Text => Line::Depth(self.stack.len()),
                Html::Tag {
                    line, quote: None, ..
                } => Line::Shift(*line),
                Html::Tag { .. } | Html::Comment | Html::Verbatim(_) => Line::Keep,
            }
        };
        self.head = matches!(line, Line::Depth(_));
        self.lines.push(line);
    }

    fn content(&mut self) {
        self.head = false;
    }

    /// After something at the start of the line closed a level.
    fn dedent(&mut self) {
        if self.head
            && let Some(line @ Line::Depth(_)) = self.lines.last_mut()
        {
            *line = Line::Depth(self.stack.len());
        }
    }

    fn open(&mut self, entry: Entry) {
        self.content();
        self.stack.push(entry);
    }

    fn find_block(&self, kinds: &[Tag]) -> Option<usize> {
        self.stack
            .iter()
            .rposition(|entry| matches!(entry, Entry::Block(kind) if kinds.contains(kind)))
    }

    fn close_block(&mut self, kind: Tag) {
        if let Some(index) = self.find_block(&[kind]) {
            self.stack.truncate(index);
        }
        self.dedent();
    }

    fn close_element(&mut self, name: &str) {
        let found = self.stack.iter().rposition(|entry| match entry {
            Entry::Element(open) => open == name,
            Entry::Block(_) => true,
        });
        if let Some(index) = found
            && self.stack[index] == Entry::Element(name.to_owned())
        {
            self.stack.truncate(index);
        }
        self.dedent();
    }

    /// `else`, `elif` and `when`: the end of one branch and the start of the next, one level
    /// out from their bodies.
    fn branch(&mut self, word: &str) {
        let kinds: &[Tag] = if word == "when" {
            &[Tag::When, Tag::Match]
        } else {
            &[Tag::If, Tag::For, Tag::When, Tag::Match]
        };
        let Some(index) = self.find_block(kinds) else {
            self.content();
            return;
        };
        let reopen = match self.stack[index] {
            Entry::Block(Tag::Match) => {
                self.stack.truncate(index + 1);
                Tag::When
            }
            Entry::Block(kind) => {
                self.stack.truncate(index);
                kind
            }
            Entry::Element(_) => return,
        };
        self.dedent();
        self.open(Entry::Block(reopen));
    }

    fn askama_block(&mut self, body: &str) {
        let word = first_word(body);
        let opens = match word {
            "if" => Some(Tag::If),
            "for" => Some(Tag::For),
            "match" => Some(Tag::Match),
            "block" => Some(Tag::Block),
            "macro" => Some(Tag::Macro),
            "filter" => Some(Tag::Filter),
            "call" => Some(Tag::Call),
            "let" | "set" if !super::assigns(body) => Some(Tag::Let),
            _ => None,
        };
        // A `filter` or block `let` captures its body rather than rendering it, and a block
        // may be rendered elsewhere where whitespace matters, so their bodies are kept.
        let keeps = match opens {
            Some(Tag::Filter | Tag::Let) => true,
            Some(Tag::Block) => {
                let name = first_word(body.trim_start()[word.len()..].trim_start());
                let kept = self.keeping()
                    || matches!(
                        self.html,
                        Html::Verbatim(_) | Html::Comment | Html::Tag { quote: Some(_), .. }
                    )
                    || self.kept_blocks.contains(name);
                if kept {
                    self.kept_names.insert(name.to_owned());
                }
                kept
            }
            _ => false,
        };
        if !self.keeping() {
            self.kept_from = keeps.then_some(self.stack.len());
        }
        let closes = match word {
            "endif" => Some(Tag::If),
            "endfor" => Some(Tag::For),
            "endmatch" => Some(Tag::Match),
            "endwhen" => Some(Tag::When),
            "endblock" => Some(Tag::Block),
            "endmacro" => Some(Tag::Macro),
            "endfilter" => Some(Tag::Filter),
            "endcall" => Some(Tag::Call),
            "endlet" | "endset" => Some(Tag::Let),
            _ => None,
        };
        if let Some(kind) = opens {
            self.open(Entry::Block(kind));
        } else if let Some(kind) = closes {
            self.close_block(kind);
        } else if matches!(word, "else" | "elif" | "when") {
            self.branch(word);
        } else {
            self.content();
        }
    }

    /// Reads the HTML in `text`, opening a line after each line break.
    fn text(&mut self, text: &str) {
        let bytes = text.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            let b = bytes[i];
            if b == b'\n' {
                i += 1;
                self.new_line(false);
                continue;
            }
            match &mut self.html {
                Html::Text => {
                    if b == b' ' || b == b'\t' || b == b'\r' {
                        i += 1;
                    } else if bytes[i..].starts_with(b"<!--") {
                        self.content();
                        self.html = Html::Comment;
                        i += 4;
                    } else if b == b'<' && bytes.get(i + 1) == Some(&b'/') {
                        let name = tag_name(&text[i + 2..]);
                        if name.is_empty() {
                            self.content();
                            i += 1;
                            continue;
                        }
                        i += 2 + name.len();
                        self.close_element(&name);
                        self.html = Html::Tag {
                            line: self.lines.len() - 1,
                            quote: None,
                            opens: None,
                            last: 0,
                        };
                    } else if b == b'<' && bytes.get(i + 1).is_some_and(u8::is_ascii_alphabetic) {
                        let name = tag_name(&text[i + 1..]);
                        i += 1 + name.len();
                        self.content();
                        self.html = Html::Tag {
                            line: self.lines.len() - 1,
                            quote: None,
                            opens: Some(name),
                            last: 0,
                        };
                    } else if b == b'<' && matches!(bytes.get(i + 1), Some(b'!' | b'?')) {
                        self.content();
                        self.html = Html::Tag {
                            line: self.lines.len() - 1,
                            quote: None,
                            opens: None,
                            last: 0,
                        };
                        i += 2;
                    } else {
                        self.content();
                        i += 1;
                    }
                }
                Html::Tag {
                    quote, opens, last, ..
                } => {
                    i += 1;
                    match *quote {
                        Some(q) if b == q => *quote = None,
                        Some(_) => {}
                        None if b == b'"' || b == b'\'' => *quote = Some(b),
                        None if b == b'>' => {
                            let self_closing = *last == b'/';
                            let opens = opens.take();
                            self.html = Html::Text;
                            if let Some(name) = opens
                                && !self_closing
                                && !VOID.contains(&name.as_str())
                            {
                                self.open_element(name);
                            }
                        }
                        None if !b.is_ascii_whitespace() => *last = b,
                        None => {}
                    }
                }
                Html::Comment => {
                    if bytes[i..].starts_with(b"-->") {
                        self.html = Html::Text;
                        i += 3;
                    } else {
                        i += 1;
                    }
                }
                Html::Verbatim(name) => {
                    if b == b'<'
                        && bytes.get(i + 1) == Some(&b'/')
                        && tag_name(&text[i + 2..]) == *name
                    {
                        let name = name.clone();
                        i += 2 + name.len();
                        self.html = Html::Tag {
                            line: self.lines.len() - 1,
                            quote: None,
                            opens: None,
                            last: 0,
                        };
                        self.close_element(&name);
                    } else {
                        i += 1;
                    }
                }
            }
        }
    }

    fn open_element(&mut self, name: String) {
        if SELF_ENDING.contains(&name.as_str())
            && self.stack.last() == Some(&Entry::Element(name.clone()))
        {
            self.stack.pop();
        }
        if VERBATIM.contains(&name.as_str()) {
            self.html = Html::Verbatim(name.clone());
        }
        self.stack.push(Entry::Element(name));
    }
}

/// The lower-cased element name at the start of `text`.
fn tag_name(text: &str) -> String {
    let end = text
        .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '-' | ':' | '_' | '.')))
        .unwrap_or(text.len());
    text[..end].to_ascii_lowercase()
}

/// An HTML start tag on one line, with the places it may break: the whitespace before each
/// attribute, and before its `>` or `/>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct StartTag {
    /// From its `<` to just after its `>`.
    pub(super) range: Range<usize>,
    /// The whitespace before each attribute, outside quotes and outside any askama block
    /// opened inside the tag.
    pub(super) gaps: Vec<Range<usize>>,
    /// Where its `>` or `/>` starts, and the whitespace before it, if any.
    pub(super) close: usize,
    pub(super) close_gap: Option<Range<usize>>,
}

/// Every HTML start tag in `source` that has attributes and sits on one line, outside the
/// elements whose contents are not HTML and outside comments; and the askama tags, by segment,
/// that sit inside a start tag's quoted attribute value.
pub(super) fn start_tags(source: &str, segments: &[Segment]) -> (Vec<StartTag>, HashSet<usize>) {
    struct Open {
        start: usize,
        name: String,
        gaps: Vec<Range<usize>>,
        quote: Option<u8>,
        depth: usize,
        last: Option<usize>,
        valid: bool,
    }
    let mut found = Vec::new();
    let mut open: Option<Open> = None;
    let mut verbatim: Option<String> = None;
    let mut comment = false;
    let mut in_values = HashSet::new();
    for (index, segment) in segments.iter().enumerate() {
        let range = match segment {
            Segment::Verbatim(range) => range.clone(),
            Segment::Raw(range) | Segment::Tag { range, .. } => {
                if let Some(tag) = &mut open {
                    if tag.quote.is_some() {
                        in_values.insert(index);
                    }
                    tag.last = None;
                    if source[range.clone()].contains('\n') || matches!(segment, Segment::Raw(_)) {
                        tag.valid = false;
                    }
                    if let Segment::Tag {
                        kind: TagKind::Block,
                        inner,
                        ..
                    } = segment
                    {
                        let word = first_word(&source[inner.clone()]);
                        if matches!(
                            word,
                            "if" | "for" | "match" | "block" | "macro" | "filter" | "call"
                        ) {
                            tag.depth += 1;
                        } else if word.starts_with("end") {
                            tag.depth = tag.depth.saturating_sub(1);
                        }
                    }
                }
                continue;
            }
        };
        let bytes = source.as_bytes();
        let mut i = range.start;
        while i < range.end {
            let b = bytes[i];
            if comment {
                if bytes[i..range.end].starts_with(b"-->") {
                    comment = false;
                    i += 3;
                } else {
                    i += 1;
                }
                continue;
            }
            if let Some(name) = &verbatim {
                if b == b'<'
                    && bytes.get(i + 1) == Some(&b'/')
                    && tag_name(&source[i + 2..]) == *name
                {
                    verbatim = None;
                }
                i += 1;
                continue;
            }
            let Some(tag) = &mut open else {
                if bytes[i..range.end].starts_with(b"<!--") {
                    comment = true;
                    i += 4;
                } else if b == b'<' && bytes.get(i + 1).is_some_and(u8::is_ascii_alphabetic) {
                    let name = tag_name(&source[i + 1..]);
                    open = Some(Open {
                        start: i,
                        gaps: Vec::new(),
                        quote: None,
                        depth: 0,
                        last: None,
                        valid: true,
                        name,
                    });
                    i += 1 + open.as_ref().map_or(0, |tag| tag.name.len());
                } else {
                    i += 1;
                }
                continue;
            };
            match tag.quote {
                Some(q) => {
                    if b == q {
                        tag.quote = None;
                    }
                    tag.last = Some(i);
                    i += 1;
                }
                None if b == b'"' || b == b'\'' => {
                    tag.quote = Some(b);
                    tag.last = Some(i);
                    i += 1;
                }
                None if b == b'\n' => {
                    tag.valid = false;
                    i += 1;
                }
                None if b == b' ' || b == b'\t' => {
                    let start = i;
                    while i < range.end && matches!(bytes[i], b' ' | b'\t') {
                        i += 1;
                    }
                    if tag.depth == 0 {
                        tag.gaps.push(start..i);
                    }
                }
                None if b == b'>' => {
                    let close = match tag.last {
                        Some(slash) if bytes[slash] == b'/' && slash + 1 == i => slash,
                        _ => i,
                    };
                    let mut gaps = std::mem::take(&mut tag.gaps);
                    let close_gap = gaps.pop_if(|gap| gap.end == close);
                    if tag.valid && !gaps.is_empty() {
                        found.push(StartTag {
                            range: tag.start..i + 1,
                            gaps,
                            close,
                            close_gap,
                        });
                    }
                    if VERBATIM.contains(&tag.name.as_str()) {
                        verbatim = Some(tag.name.clone());
                    }
                    open = None;
                    i += 1;
                }
                None => {
                    tag.last = Some(i);
                    i += 1;
                }
            }
        }
    }
    (found, in_values)
}

/// The blocks a template that extends this one, `source`, defines whose bodies this one
/// renders where whitespace is kept: `inherited` (this one's own, from the template it
/// extends) and those it opens in such a place.
pub(super) fn kept_blocks(source: &str, segments: &[Segment], inherited: &Blocks) -> Blocks {
    match inherited {
        Blocks::All => Blocks::All,
        Blocks::Named(names) => {
            let mut kept = scan(source, segments, inherited).kept_names;
            kept.extend(names.iter().cloned());
            Blocks::Named(kept)
        }
    }
}

/// Where in `source` the bodies of `filter` and block `let` tags, and of `kept_blocks`, are:
/// text that is captured, or rendered where its whitespace is kept.
pub(super) fn kept_bodies(
    source: &str,
    segments: &[Segment],
    kept_blocks: &Blocks,
) -> Vec<Range<usize>> {
    scan(source, segments, kept_blocks).kept_bodies
}

fn scan<'b>(source: &str, segments: &[Segment], kept_blocks: &'b Blocks) -> Indenter<'b> {
    let mut indenter = Indenter {
        stack: Vec::new(),
        html: Html::Text,
        lines: Vec::new(),
        head: true,
        kept_blocks,
        kept_from: None,
        kept_names: HashSet::new(),
        kept_bodies: Vec::new(),
    };
    // No line break precedes the first line's whitespace, so it would render.
    indenter.new_line(true);
    for segment in segments {
        match segment {
            Segment::Verbatim(range) => indenter.text(&source[range.clone()]),
            Segment::Raw(range) => {
                indenter.content();
                for _ in source[range.clone()].matches('\n') {
                    indenter.new_line(true);
                }
            }
            Segment::Tag { kind, range, inner } => {
                let start = indenter.lines.len() - 1;
                if *kind == TagKind::Block {
                    let was = indenter.keeping();
                    indenter.askama_block(&source[inner.clone()]);
                    match (was, indenter.keeping()) {
                        (false, true) => indenter.kept_bodies.push(range.end..source.len()),
                        (true, false) => {
                            if let Some(body) = indenter.kept_bodies.last_mut() {
                                body.end = range.start;
                            }
                        }
                        _ => {}
                    }
                } else {
                    indenter.content();
                }
                for _ in source[range.clone()].matches('\n') {
                    if *kind == TagKind::Comment && !indenter.keeping() {
                        // A comment's text is never rendered, so it moves with its first line.
                        indenter.lines.push(Line::Shift(start));
                        indenter.head = false;
                    } else {
                        indenter.new_line(true);
                    }
                }
            }
        }
    }
    indenter
}

/// `source` with each line's leading whitespace set by its nesting depth, `unit` per level,
/// except in the bodies of `kept_blocks`.
pub(super) fn reindent(
    source: &str,
    segments: &[Segment],
    unit: &str,
    kept_blocks: &Blocks,
) -> String {
    let indenter = scan(source, segments, kept_blocks);
    let mut out = String::with_capacity(source.len());
    let mut leading: Vec<(&str, String)> = Vec::new();
    for (index, line) in source.split('\n').enumerate() {
        if index > 0 {
            out.push('\n');
        }
        let rest = line.trim_start_matches([' ', '\t']);
        let old = &line[..line.len() - rest.len()];
        let new = match indenter.lines.get(index).copied().unwrap_or(Line::Keep) {
            Line::Keep => old.to_owned(),
            Line::Depth(_) if rest.is_empty() || rest == "\r" => String::new(),
            Line::Depth(depth) => unit.repeat(depth),
            Line::Shift(start) => match leading.get(start) {
                Some((start_old, start_new)) if old.starts_with(start_old) => {
                    format!("{start_new}{}", &old[start_old.len()..])
                }
                _ => old.to_owned(),
            },
        };
        out.push_str(&new);
        out.push_str(rest);
        leading.push((old, new));
    }
    out
}
