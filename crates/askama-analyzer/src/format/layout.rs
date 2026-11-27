//! Expressions inside tags, laid out as rustfmt lays out Rust.
//!
//! The printer works over `askama_parser`'s expression tree. Every construct either fits on
//! the rest of its line or breaks in one consistent way, chosen as rustfmt chooses with its
//! default settings: argument lists break one item per line with the closing delimiter on a
//! line of its own (or, for a single call-like argument, let that argument break instead),
//! method chains break one call per line, and binary operators break before the operator.
//! Continuation lines are indented in steps of four spaces from the indentation of the line
//! the tag starts on, as rustfmt indents blocks from the statement's indentation.

use askama_parser::expr::{AssociatedItem, Call, ExprStruct};
use askama_parser::{
    Ast, CharPrefix, Expr, Filter, Node, PathComponent, PathOrIdentifier, Syntax, TyGenerics,
    TyGenericsKind, WithSpan,
};

/// rustfmt's block indent.
pub(super) const INDENT: usize = 4;

/// The widest a chain's root can be and still have its first link joined to it, as rustfmt
/// keeps `self.field` or `x.method(` together rather than putting `self` on a line alone.
const SHORT_ROOT: usize = INDENT;

/// An expression parsed on its own, from `{{ text }}`.
pub(super) struct Parsed {
    source: String,
}

impl Parsed {
    pub(super) fn new(text: &str) -> Self {
        Self {
            source: format!("{{{{ {text} }}}}"),
        }
    }

    /// The expression's tree, without spans, or `None` when the text is not one expression.
    pub(super) fn tree(&self) -> Option<String> {
        self.with_expr(|expr, _| format!("{expr:?}"))
    }

    pub(super) fn with_expr<T>(&self, f: impl FnOnce(&Expr<'_>, &str) -> T) -> Option<T> {
        let ast = Ast::from_str(&self.source, None, &Syntax::default()).ok()?;
        match ast.nodes() {
            [node] => match &**node {
                Node::Expr(_, expr) => Some(f(expr, &self.source)),
                _ => None,
            },
            _ => None,
        }
    }
}

/// Where an expression is laid out.
pub(super) struct Printer<'s> {
    /// The leading whitespace of the line the tag starts on, which continuation lines repeat.
    base: &'s str,
    /// How many columns `base` takes.
    base_width: usize,
    max_width: usize,
    /// The text the expression was parsed from, which holds Rust macros' delimiters.
    source: &'s str,
}

pub(super) fn width(text: &str) -> usize {
    text.chars().count()
}

enum Item<'e, 'a> {
    Expr(&'e Expr<'a>),
    Field(&'a str, Option<&'e Expr<'a>>),
    Base(&'e Expr<'a>),
}

/// A link of a method chain after its root.
enum Link<'e, 'a> {
    Field(&'e AssociatedItem<'a>),
    Method(&'e AssociatedItem<'a>, &'e Call<'a>),
    Index(&'e Expr<'a>),
    Try,
}

impl<'s> Printer<'s> {
    pub(super) fn new(base: &'s str, base_width: usize, max_width: usize, source: &'s str) -> Self {
        Self {
            base,
            base_width,
            max_width,
            source,
        }
    }

    pub(super) fn newline(&self, level: usize) -> String {
        format!("\n{}{}", self.base, " ".repeat(INDENT * level))
    }

    pub(super) fn column(&self, level: usize) -> usize {
        self.base_width + INDENT * level
    }

    /// The column after `text` when it starts at `col`.
    pub(super) fn end_column(&self, text: &str, col: usize) -> usize {
        match text.rsplit_once('\n') {
            Some((_, last)) => self.line_width(last),
            None => col + width(text),
        }
    }

    fn line_width(&self, line: &str) -> usize {
        match line.strip_prefix(self.base) {
            Some(rest) => self.base_width + width(rest),
            None => width(line),
        }
    }

    /// Whether `text`, starting at `col` and followed by `trail` columns, stays in the width.
    fn fits(&self, text: &str, col: usize, trail: usize) -> bool {
        let mut lines = text.split('\n');
        let first = lines.next().unwrap_or_default();
        let mut last = col + width(first);
        if last > self.max_width {
            return false;
        }
        for line in lines {
            last = self.line_width(line);
            if last > self.max_width {
                return false;
            }
        }
        last + trail <= self.max_width
    }

    /// `expr` laid out starting at `col`, with continuation lines at `level`, leaving `trail`
    /// columns after it for what follows on its last line. `None` when the tree holds
    /// something the printer does not write.
    pub(super) fn layout(
        &self,
        expr: &Expr<'_>,
        level: usize,
        col: usize,
        trail: usize,
    ) -> Option<String> {
        let flat = self.flat(expr)?;
        if self.fits(&flat, col, trail) {
            return Some(flat);
        }
        Some(match expr {
            Expr::Call(_) | Expr::AssociatedItem(..) | Expr::Index(..) | Expr::Try(_) => {
                self.chain(expr, level, col, trail)?
            }
            Expr::BinOp(_) | Expr::Concat(_) => self.binary(expr, level, col, trail)?,
            Expr::Filter(filter) => self.filter(filter, level, col, trail)?,
            Expr::Array(items) => {
                let items: Vec<_> = items.iter().map(|item| Item::Expr(item)).collect();
                self.list("[", &items, "]", false, level, col, trail)?
            }
            Expr::Tuple(items) if items.len() > 1 => {
                let items: Vec<_> = items.iter().map(|item| Item::Expr(item)).collect();
                self.list("(", &items, ")", false, level, col, trail)?
            }
            Expr::Struct(structure) => self.structure(structure, level, col, trail)?,
            Expr::Group(inner) => {
                format!("({})", self.layout(inner, level, col + 1, trail + 1)?)
            }
            Expr::Unary(op, inner) => {
                format!("{op}{}", self.layout(inner, level, col + width(op), trail)?)
            }
            Expr::NamedArgument(name, value) => {
                let head = format!("{} = ", **name);
                let value = self.layout(value, level, col + width(&head), trail)?;
                format!("{head}{value}")
            }
            Expr::As(inner, ty) => {
                let tail = format!(" as {}", **ty);
                let inner = self.layout(inner, level, col, trail + width(&tail))?;
                format!("{inner}{tail}")
            }
            _ => flat,
        })
    }

    /// `expr` on one line.
    pub(super) fn flat(&self, expr: &Expr<'_>) -> Option<String> {
        Some(match expr {
            Expr::BoolLit(value) => value.to_string(),
            Expr::NumLit(text, _) => (*text).to_owned(),
            Expr::StrLit(lit) => {
                let prefix = lit.prefix.map(|p| p.to_char().to_string());
                format!("{}\"{}\"", prefix.unwrap_or_default(), lit.content)
            }
            Expr::CharLit(lit) => {
                let prefix = if lit.prefix == Some(CharPrefix::Binary) {
                    "b"
                } else {
                    ""
                };
                format!("{prefix}'{}'", lit.content)
            }
            Expr::Var(name) => (*name).to_owned(),
            Expr::Path(path) => self.path(path),
            Expr::Array(items) => format!("[{}]", self.flat_all(items)?),
            Expr::ArrayRepeat(item, count) => {
                format!("[{}; {}]", self.flat(item)?, self.flat(count)?)
            }
            Expr::AssociatedItem(receiver, item) => {
                format!("{}{}", self.flat(receiver)?, self.field(item))
            }
            Expr::Index(receiver, index) => {
                format!("{}[{}]", self.flat(receiver)?, self.flat(index)?)
            }
            Expr::Filter(filter) => {
                let (input, args) = filter.arguments.split_first()?;
                let mut out = format!("{}|{}", self.flat(input)?, filter_name(&filter.name));
                if !args.is_empty() {
                    out.push_str(&format!("({})", self.flat_all(args)?));
                }
                out
            }
            Expr::As(inner, ty) => format!("{} as {}", self.flat(inner)?, **ty),
            Expr::NamedArgument(name, value) => format!("{} = {}", **name, self.flat(value)?),
            Expr::Unary(op, inner) => format!("{op}{}", self.flat(inner)?),
            Expr::BinOp(binop) => format!(
                "{} {} {}",
                self.flat(&binop.lhs)?,
                operator(binop.op),
                self.flat(&binop.rhs)?
            ),
            Expr::Range(range) => {
                let lhs = match &range.lhs {
                    Some(lhs) => self.flat(lhs)?,
                    None => String::new(),
                };
                let rhs = match &range.rhs {
                    Some(rhs) => self.flat(rhs)?,
                    None => String::new(),
                };
                format!("{lhs}{}{rhs}", range.op)
            }
            Expr::Group(inner) => format!("({})", self.flat(inner)?),
            Expr::Tuple(items) => match items.as_slice() {
                [item] => format!("({},)", self.flat(item)?),
                _ => format!("({})", self.flat_all(items)?),
            },
            Expr::Call(call) => format!(
                "{}{}({})",
                self.flat(&call.path)?,
                turbofish(call.generics.as_deref()),
                self.flat_all(&call.args)?
            ),
            Expr::RustMacro(path, args) => {
                let offset = (args.as_ptr() as usize).checked_sub(self.source.as_ptr() as usize)?;
                let open = self.source.get(..offset)?.chars().next_back()?;
                let close = self.source.get(offset + args.len()..)?.chars().next()?;
                let path: Vec<&str> = path.iter().map(|name| **name).collect();
                format!("{}!{open}{}{close}", path.join("::"), **args)
            }
            Expr::Try(inner) => format!("{}?", self.flat(inner)?),
            Expr::Struct(structure) => {
                let items = struct_items(structure);
                let fields = items
                    .iter()
                    .map(|item| self.flat_item(item))
                    .collect::<Option<Vec<_>>>()?;
                if fields.is_empty() {
                    format!("{} {{}}", self.flat(&structure.path)?)
                } else {
                    format!(
                        "{} {{ {} }}",
                        self.flat(&structure.path)?,
                        fields.join(", ")
                    )
                }
            }
            Expr::IsDefined(name) => format!("{name} is defined"),
            Expr::IsNotDefined(name) => format!("{name} is not defined"),
            Expr::Concat(items) => items
                .iter()
                .map(|item| self.flat(item))
                .collect::<Option<Vec<_>>>()?
                .join(" ~ "),
            Expr::LetCond(_) | Expr::FilterSource | Expr::ArgumentPlaceholder => return None,
        })
    }

    fn flat_all(&self, items: &[WithSpan<Box<Expr<'_>>>]) -> Option<String> {
        Some(
            items
                .iter()
                .map(|item| self.flat(item))
                .collect::<Option<Vec<_>>>()?
                .join(", "),
        )
    }

    fn path(&self, path: &[PathComponent<'_>]) -> String {
        path.iter()
            .map(|component| {
                format!(
                    "{}{}",
                    *component.name,
                    turbofish(component.generics.as_deref())
                )
            })
            .collect::<Vec<_>>()
            .join("::")
    }

    fn field(&self, item: &AssociatedItem<'_>) -> String {
        format!(".{}{}", *item.name, turbofish(item.generics.as_deref()))
    }

    fn flat_item(&self, item: &Item<'_, '_>) -> Option<String> {
        match item {
            Item::Expr(expr) => self.flat(expr),
            Item::Field(name, None) => Some((*name).to_owned()),
            Item::Field(name, Some(value)) => Some(format!("{name}: {}", self.flat(value)?)),
            Item::Base(base) => Some(format!("..{}", self.flat(base)?)),
        }
    }

    fn layout_item(
        &self,
        item: &Item<'_, '_>,
        level: usize,
        col: usize,
        trail: usize,
    ) -> Option<String> {
        match item {
            Item::Expr(expr) => self.layout(expr, level, col, trail),
            Item::Field(name, None) => Some((*name).to_owned()),
            Item::Field(name, Some(value)) => {
                let head = format!("{name}: ");
                let value = self.layout(value, level, col + width(&head), trail)?;
                Some(format!("{head}{value}"))
            }
            Item::Base(base) => Some(format!("..{}", self.layout(base, level, col + 2, trail)?)),
        }
    }

    /// A delimited, comma-separated list: on one line; or its one call-like item broken in
    /// place, as rustfmt overflows a last argument; or one item per line, each followed by a
    /// comma (askama accepts a trailing one in every list this prints), with the closing
    /// delimiter on a line of its own.
    #[allow(clippy::too_many_arguments)]
    fn list(
        &self,
        open: &str,
        items: &[Item<'_, '_>],
        close: &str,
        padded: bool,
        level: usize,
        col: usize,
        trail: usize,
    ) -> Option<String> {
        let flat = items
            .iter()
            .map(|item| self.flat_item(item))
            .collect::<Option<Vec<_>>>()?
            .join(", ");
        let flat = if padded && !flat.is_empty() {
            format!("{open} {flat} {close}")
        } else {
            format!("{open}{flat}{close}")
        };
        if items.is_empty() || self.fits(&flat, col, trail) {
            return Some(flat);
        }
        if let [item @ Item::Expr(expr)] = items
            && overflows(expr)
        {
            let inner = self.layout_item(item, level, col + width(open), trail + width(close))?;
            let opens_block = inner
                .split('\n')
                .next()
                .is_some_and(|first| first.ends_with(['(', '[', '{']));
            let overflowed = format!("{open}{inner}{close}");
            if inner.contains('\n') && opens_block && self.fits(&overflowed, col, trail) {
                return Some(overflowed);
            }
        }
        let mut out = open.to_owned();
        for (index, item) in items.iter().enumerate() {
            let comma = index + 1 < items.len() || !matches!(item, Item::Base(_));
            out.push_str(&self.newline(level + 1));
            out.push_str(&self.layout_item(
                item,
                level + 1,
                self.column(level + 1),
                usize::from(comma),
            )?);
            if comma {
                out.push(',');
            }
        }
        out.push_str(&self.newline(level));
        out.push_str(close);
        Some(out)
    }

    fn structure(
        &self,
        structure: &ExprStruct<'_>,
        level: usize,
        col: usize,
        trail: usize,
    ) -> Option<String> {
        let path = format!("{} ", self.flat(&structure.path)?);
        let items = struct_items(structure);
        let fields = self.list("{", &items, "}", true, level, col + width(&path), trail)?;
        Some(format!("{path}{fields}"))
    }

    /// A call that is not a method call: its callee on one line, then its arguments.
    fn call(&self, call: &Call<'_>, level: usize, col: usize, trail: usize) -> Option<String> {
        let head = format!(
            "{}{}",
            self.flat(&call.path)?,
            turbofish(call.generics.as_deref())
        );
        let args: Vec<_> = call.args.iter().map(|arg| Item::Expr(arg)).collect();
        let args = self.list("(", &args, ")", false, level, col + width(&head), trail)?;
        Some(format!("{head}{args}"))
    }

    /// A method chain: its root and the links joined to it, then one link per line, indented
    /// one level, or at the chain's own level when the root already broke over lines.
    fn chain(&self, expr: &Expr<'_>, level: usize, col: usize, trail: usize) -> Option<String> {
        let (root, links) = flatten_chain(expr);
        // Each step is a field or method with the indexing and `?` that follow it.
        let mut steps: Vec<Vec<&Link<'_, '_>>> = Vec::new();
        for link in &links {
            match (link, steps.last_mut()) {
                (Link::Index(_) | Link::Try, Some(step)) => step.push(link),
                _ => steps.push(vec![link]),
            }
        }
        // Indexing or `?` straight after the root stays with it.
        let mut root_suffix = String::new();
        while let Some([Link::Index(_) | Link::Try, ..]) = steps.first().map(Vec::as_slice) {
            for link in steps.remove(0) {
                root_suffix.push_str(&self.flat_link(link)?);
            }
        }
        if steps.is_empty() {
            return match root {
                Expr::Call(call) if root_suffix.is_empty() => self.call(call, level, col, trail),
                _ => Some(format!(
                    "{}{root_suffix}",
                    self.layout(root, level, col, trail + width(&root_suffix))?
                )),
            };
        }

        // A short root takes links onto its line while what it holds is still short.
        let mut probe = format!("{}{root_suffix}", self.flat(root)?);
        let mut head = String::new();
        let mut joined = 0;
        while joined < steps.len() && !probe.contains('\n') && width(&probe) <= SHORT_ROOT {
            head.clone_from(&probe);
            probe.push_str(&self.flat_step(&steps[joined])?);
            joined += 1;
        }
        let rest = &steps[joined..];
        let head_trail = if rest.is_empty() { trail } else { 0 };
        let parent = if joined == 0 {
            match root {
                Expr::Call(call) if root_suffix.is_empty() => {
                    self.call(call, level, col, head_trail)?
                }
                _ => format!(
                    "{}{root_suffix}",
                    self.layout(root, level, col, head_trail + width(&root_suffix))?
                ),
            }
        } else {
            let step = self.layout_step(
                &steps[joined - 1],
                level,
                self.end_column(&head, col),
                head_trail,
            )?;
            format!("{head}{step}")
        };
        let level = if parent.contains('\n') {
            level
        } else {
            level + 1
        };
        let mut out = parent;
        for (index, step) in rest.iter().enumerate() {
            let step_trail = if index + 1 == rest.len() { trail } else { 0 };
            out.push_str(&self.newline(level));
            out.push_str(&self.layout_step(step, level, self.column(level), step_trail)?);
        }
        Some(out)
    }

    fn flat_link(&self, link: &Link<'_, '_>) -> Option<String> {
        Some(match link {
            Link::Field(item) => self.field(item),
            Link::Method(item, call) => format!(
                "{}{}({})",
                self.field(item),
                turbofish(call.generics.as_deref()),
                self.flat_all(&call.args)?
            ),
            Link::Index(index) => format!("[{}]", self.flat(index)?),
            Link::Try => "?".to_owned(),
        })
    }

    fn flat_step(&self, step: &[&Link<'_, '_>]) -> Option<String> {
        step.iter().map(|link| self.flat_link(link)).collect()
    }

    fn layout_step(
        &self,
        step: &[&Link<'_, '_>],
        level: usize,
        col: usize,
        trail: usize,
    ) -> Option<String> {
        let (first, suffixes) = step.split_first()?;
        let suffix = self.flat_step(suffixes)?;
        let first = match first {
            Link::Method(item, call) => {
                let head = format!(
                    "{}{}",
                    self.field(item),
                    turbofish(call.generics.as_deref())
                );
                let args: Vec<_> = call.args.iter().map(|arg| Item::Expr(arg)).collect();
                let args = self.list(
                    "(",
                    &args,
                    ")",
                    false,
                    level,
                    col + width(&head),
                    trail + width(&suffix),
                )?;
                format!("{head}{args}")
            }
            link => self.flat_link(link)?,
        };
        Some(format!("{first}{suffix}"))
    }

    /// Operators of one precedence: the first operand, then each operator and operand on a
    /// line of its own, one level in.
    fn binary(&self, expr: &Expr<'_>, level: usize, col: usize, trail: usize) -> Option<String> {
        let (first, rest) = flatten_binary(expr);
        let mut out = self.layout(first, level, col, 0)?;
        for (index, (op, operand)) in rest.iter().enumerate() {
            let operand_trail = if index + 1 == rest.len() { trail } else { 0 };
            out.push_str(&self.newline(level + 1));
            out.push_str(op);
            out.push(' ');
            let start = self.column(level + 1) + width(op) + 1;
            out.push_str(&self.layout(operand, level + 1, start, operand_trail)?);
        }
        Some(out)
    }

    /// A filter, written as askama writes it, `input|name(args)`, with its input laid out
    /// first and then its arguments.
    fn filter(
        &self,
        filter: &Filter<'_>,
        level: usize,
        col: usize,
        trail: usize,
    ) -> Option<String> {
        let (input, args) = filter.arguments.split_first()?;
        let name = format!("|{}", filter_name(&filter.name));
        if args.is_empty() {
            let input = self.layout(input, level, col, trail + width(&name))?;
            return Some(format!("{input}{name}"));
        }
        let flat_args = format!("({})", self.flat_all(args)?);
        let input = self.layout(input, level, col, trail + width(&name) + width(&flat_args))?;
        let items: Vec<_> = args.iter().map(|arg| Item::Expr(arg)).collect();
        let start = self.end_column(&input, col) + width(&name);
        let args = self.list("(", &items, ")", false, level, start, trail)?;
        Some(format!("{input}{name}{args}"))
    }
}

/// Whether rustfmt lets this expression, as a list's only item, break in place of the list.
fn overflows(expr: &Expr<'_>) -> bool {
    match expr {
        Expr::Call(_) | Expr::Struct(_) | Expr::Array(_) => true,
        Expr::Tuple(items) => items.len() > 1,
        _ => false,
    }
}

fn struct_items<'e, 'a>(structure: &'e ExprStruct<'a>) -> Vec<Item<'e, 'a>> {
    let mut items: Vec<_> = structure
        .fields
        .iter()
        .map(|field| Item::Field(*field.name, field.value.as_ref().map(|value| &***value)))
        .collect();
    if let Some(base) = &structure.base {
        items.push(Item::Base(base));
    }
    items
}

fn flatten_chain<'e, 'a>(expr: &'e Expr<'a>) -> (&'e Expr<'a>, Vec<Link<'e, 'a>>) {
    let (receiver, link) = match expr {
        Expr::Call(call) => match &**call.path {
            Expr::AssociatedItem(receiver, item) => (&***receiver, Link::Method(item, call)),
            _ => return (expr, Vec::new()),
        },
        Expr::AssociatedItem(receiver, item) => (&***receiver, Link::Field(item)),
        Expr::Index(receiver, index) => (&***receiver, Link::Index(index)),
        Expr::Try(receiver) => (&***receiver, Link::Try),
        _ => return (expr, Vec::new()),
    };
    let (root, mut links) = flatten_chain(receiver);
    links.push(link);
    (root, links)
}

/// A binary expression as its operands of one precedence and the operators between them.
fn flatten_binary<'e, 'a>(expr: &'e Expr<'a>) -> (&'e Expr<'a>, Vec<(&'static str, &'e Expr<'a>)>) {
    match expr {
        Expr::Concat(items) => match items.split_first() {
            Some((first, rest)) => (first, rest.iter().map(|item| ("~", &***item)).collect()),
            None => (expr, Vec::new()),
        },
        Expr::BinOp(binop) => {
            let precedence = precedence(binop.op);
            let mut rest = vec![(operator(binop.op), &**binop.rhs)];
            let mut lhs: &Expr<'_> = &binop.lhs;
            while let Expr::BinOp(inner) = lhs
                && self::precedence(inner.op) == precedence
            {
                rest.push((operator(inner.op), &**inner.rhs));
                lhs = &inner.lhs;
            }
            rest.reverse();
            (lhs, rest)
        }
        _ => (expr, Vec::new()),
    }
}

fn precedence(op: &str) -> u8 {
    match op {
        "||" => 1,
        "&&" => 2,
        "==" | "!=" | "<" | ">" | "<=" | ">=" => 3,
        "|" => 4,
        "^" => 5,
        "&" => 6,
        "<<" | ">>" => 7,
        "+" | "-" => 8,
        _ => 9,
    }
}

/// The operator as askama spells it: its bitwise operators are words, since `|` is a filter.
fn operator(op: &str) -> &'static str {
    match op {
        "|" => "bitor",
        "^" => "xor",
        "&" => "bitand",
        "||" => "||",
        "&&" => "&&",
        "==" => "==",
        "!=" => "!=",
        "<" => "<",
        ">" => ">",
        "<=" => "<=",
        ">=" => ">=",
        "<<" => "<<",
        ">>" => ">>",
        "+" => "+",
        "-" => "-",
        "*" => "*",
        "/" => "/",
        _ => "%",
    }
}

fn filter_name(name: &PathOrIdentifier<'_>) -> String {
    match name {
        PathOrIdentifier::Identifier(name) => (**name).to_owned(),
        PathOrIdentifier::Path(path) => path
            .iter()
            .map(|component| {
                format!(
                    "{}{}",
                    *component.name,
                    turbofish(component.generics.as_deref())
                )
            })
            .collect::<Vec<_>>()
            .join("::"),
    }
}

fn turbofish(generics: Option<&Vec<WithSpan<TyGenerics<'_>>>>) -> String {
    generics.map_or_else(String::new, |generics| format!("::{}", type_args(generics)))
}

fn type_args(generics: &[WithSpan<TyGenerics<'_>>]) -> String {
    let args: Vec<String> = generics.iter().map(|ty| type_name(ty)).collect();
    format!("<{}>", args.join(", "))
}

fn type_name(ty: &TyGenerics<'_>) -> String {
    let kind = match &*ty.kind {
        TyGenericsKind::Path { path, args } => {
            let path: Vec<&str> = path.iter().map(|name| **name).collect();
            format!(
                "{}{}",
                path.join("::"),
                args.as_deref()
                    .map(|args| type_args(args))
                    .unwrap_or_default()
            )
        }
        TyGenericsKind::Tuple(items) => match items.as_slice() {
            [item] => format!("({},)", type_name(item)),
            _ => format!(
                "({})",
                items
                    .iter()
                    .map(|item| type_name(item))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        },
        TyGenericsKind::Array { ty, nb_elems } => match nb_elems {
            Some(count) => format!("[{}; {count}]", type_name(ty)),
            None => format!("[{}]", type_name(ty)),
        },
    };
    format!("{}{kind}", "&".repeat(ty.refs))
}
