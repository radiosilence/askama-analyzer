//! Indentation of HTML templates by nesting depth.
//!
//! The rule that keeps it safe: only the amount of leading whitespace on an existing line
//! changes. No line break is added or removed and nothing inside a line moves. In HTML a line
//! break followed by any amount of whitespace renders as the same single space, so the page
//! cannot change. The places where whitespace is kept as written are left alone line by line:
//! `<pre>`, `<textarea>`, `<script>` and `<style>` contents, HTML comments, `raw` blocks,
//! askama comments, and lines that continue a tag (an askama tag's are laid out with its
//! expression; an HTML tag's move with the line the tag starts on, unless they start inside
//! an attribute's value).

use super::{Segment, TagKind, first_word};

const VOID: &[&str] = &[
    "area", "base", "br", "col", "command", "embed", "hr", "img", "input", "keygen", "link",
    "meta", "param", "source", "track", "wbr",
];

/// Elements whose contents keep their whitespace (or are not HTML at all).
const VERBATIM: &[&str] = &["pre", "textarea", "script", "style", "xmp", "listing"];

/// Elements an author may leave unclosed, closed by the next one of the same name.
const SELF_ENDING: &[&str] = &["li", "option", "tr", "td", "th", "dt", "dd", "p"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Block {
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
    Block(Block),
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

struct Indenter {
    stack: Vec<Entry>,
    html: Html,
    lines: Vec<Line>,
    /// Whether everything on the current line so far is whitespace or closes something, so a
    /// closer there takes the line out a level.
    head: bool,
}

impl Indenter {
    fn new_line(&mut self, inside_tag: bool) {
        let line = if inside_tag {
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

    fn find_block(&self, kinds: &[Block]) -> Option<usize> {
        self.stack
            .iter()
            .rposition(|entry| matches!(entry, Entry::Block(kind) if kinds.contains(kind)))
    }

    fn close_block(&mut self, kind: Block) {
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
        let kinds: &[Block] = if word == "when" {
            &[Block::When, Block::Match]
        } else {
            &[Block::If, Block::For, Block::When, Block::Match]
        };
        let Some(index) = self.find_block(kinds) else {
            self.content();
            return;
        };
        let reopen = match self.stack[index] {
            Entry::Block(Block::Match) => {
                self.stack.truncate(index + 1);
                Block::When
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
            "if" => Some(Block::If),
            "for" => Some(Block::For),
            "match" => Some(Block::Match),
            "block" => Some(Block::Block),
            "macro" => Some(Block::Macro),
            "filter" => Some(Block::Filter),
            "call" => Some(Block::Call),
            "let" | "set" if !assigns(body) => Some(Block::Let),
            _ => None,
        };
        let closes = match word {
            "endif" => Some(Block::If),
            "endfor" => Some(Block::For),
            "endmatch" => Some(Block::Match),
            "endwhen" => Some(Block::When),
            "endblock" => Some(Block::Block),
            "endmacro" => Some(Block::Macro),
            "endfilter" => Some(Block::Filter),
            "endcall" => Some(Block::Call),
            "endlet" | "endset" => Some(Block::Let),
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

    /// Reads HTML in `text`, which starts at `offset` in the template, opening a line after
    /// each line break.
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
                    } else if text[i..].starts_with("<!--") {
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
                    if text[i..].starts_with("-->") {
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

/// Whether a `let` or `set` tag assigns a value, rather than opening a block that does.
fn assigns(body: &str) -> bool {
    let mut depth = 0usize;
    let bytes = body.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth = depth.saturating_sub(1),
            b'"' => {
                i = super::skip_string(bytes, i);
                continue;
            }
            b'=' if depth == 0
                && bytes.get(i + 1) != Some(&b'=')
                && i.checked_sub(1)
                    .is_none_or(|p| !matches!(bytes[p], b'=' | b'!' | b'<' | b'>')) =>
            {
                return true;
            }
            _ => {}
        }
        i += 1;
    }
    false
}

/// The lower-cased element name at the start of `text`.
fn tag_name(text: &str) -> String {
    let end = text
        .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '-' | ':' | '_' | '.')))
        .unwrap_or(text.len());
    text[..end].to_ascii_lowercase()
}

/// `source` with each line's leading whitespace set by its nesting depth, `unit` per level.
pub(super) fn reindent(source: &str, segments: &[Segment], unit: &str) -> String {
    let mut indenter = Indenter {
        stack: Vec::new(),
        html: Html::Text,
        lines: Vec::new(),
        head: true,
    };
    indenter.new_line(false);
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
                if *kind == TagKind::Block {
                    indenter.askama_block(&source[inner.clone()]);
                } else {
                    indenter.content();
                }
                for _ in source[range.clone()].matches('\n') {
                    indenter.new_line(true);
                }
            }
        }
    }

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
