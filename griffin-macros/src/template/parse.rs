//! Template source to syntax tree. Every error is recorded with its place in the
//! source and parsing carries on, so that one compile reports all of them.

use std::ops::{Deref, Range};

use proc_macro2::{Group, TokenStream, TokenTree};

use super::Source;

pub enum Node {
    /// Literal HTML text, sent as written.
    Text(String),
    /// `{expression}` as element content.
    Expr(Braces),
    Element(Box<Element>),
}

/// A `{...}` holding Rust: the braces with the tokens between them, which it stands
/// in for, and the offset of its `{`.
pub struct Braces {
    group: Group,
    pub at: usize,
}

impl Deref for Braces {
    type Target = Group;

    fn deref(&self) -> &Group {
        &self.group
    }
}

pub struct Element {
    pub name: String,
    /// The offset of the name in the opening tag.
    pub at: usize,
    pub attrs: Vec<Attr>,
    /// `:if={condition}`: the element is only rendered when the condition holds.
    pub condition: Option<Braces>,
    /// `:for={pattern in iterable}`: the element is rendered once for each item.
    pub repeat: Option<Repeat>,
    /// `:key={expression}`: what tells the items of `repeat` apart.
    pub key: Option<Braces>,
    /// `:let={pattern}`: what the argument of a slot is bound to in its content. On a
    /// Component tag it is the argument of the default slot.
    pub argument: Option<Braces>,
    pub children: Vec<Node>,
    /// A void element such as `<br>` has no content and no closing tag.
    pub void: bool,
}

pub struct Repeat {
    /// The braces of the attribute.
    pub braces: Braces,
    pub pattern: TokenStream,
    pub iterable: TokenStream,
}

pub enum Attr {
    /// `name` or `name="value"`, and the offsets it is at.
    Static {
        name: String,
        value: Option<String>,
        at: Range<usize>,
    },
    /// `name={expression}`, and the offset of the name.
    Dynamic {
        name: String,
        expr: Braces,
        at: usize,
    },
    /// `{expression}` in a tag: attributes decided at run time.
    Spread(Braces),
}

impl Element {
    /// A capitalised tag calls a Component (a PascalCase tag tells it from an HTML tag); any other is an HTML element.
    pub fn is_component(&self) -> bool {
        self.name.starts_with(char::is_uppercase)
    }

    /// `<:name>` is an entry of the slot `name` of the Component it is inside.
    pub fn slot(&self) -> Option<&str> {
        self.name.strip_prefix(':')
    }
}

impl Node {
    /// Text that is only whitespace, which between the tags of a Component is no
    /// content.
    pub fn is_blank(&self) -> bool {
        matches!(self, Node::Text(text) if text.trim().is_empty())
    }
}

pub struct Error {
    /// The offsets in the source text that are at fault. Never empty.
    pub at: Range<usize>,
    pub message: String,
}

/// The tree for `source`, and its syntax errors. The tree is whatever could be made
/// of the source, so it is only good for finding more errors when there are any.
pub fn parse(source: &Source) -> (Vec<Node>, Vec<Error>) {
    let mut parser = Parser {
        source,
        at: 0,
        open: Vec::new(),
        roots: Vec::new(),
        errors: Vec::new(),
    };
    while parser.at < source.text.len() {
        parser.node();
    }
    while !parser.open.is_empty() {
        parser.unclosed();
    }
    (parser.roots, parser.errors)
}

/// Elements that never have content. HTML's list, and the obsolete ones Phoenix adds.
const VOID: [&str; 16] = [
    "area", "base", "br", "col", "command", "embed", "hr", "img", "input", "keygen", "link",
    "meta", "param", "source", "track", "wbr",
];

/// Elements whose content is CSS or JavaScript, where `<` and `{` are not ours. It is
/// sent as written, as in Phoenix.
const RAW_TEXT: [&str; 2] = ["script", "style"];

const EXPECTED_FOR: &str =
    "expected a pattern and what to iterate over in braces: `:for={pattern in iterable}`";

/// Splits the `pattern in iterable` between the braces of a `:for`, which is `None`
/// if it does not have that form. `in` is a keyword, so the first one ends the pattern.
fn repeat(braces: Braces) -> Option<Repeat> {
    let mut tokens = braces.stream().into_iter();
    let is_in = |token: &TokenTree| matches!(token, TokenTree::Ident(ident) if ident == "in");
    let pattern: TokenStream = tokens.by_ref().take_while(|token| !is_in(token)).collect();
    let iterable: TokenStream = tokens.collect();
    (!pattern.is_empty() && !iterable.is_empty()).then_some(Repeat {
        braces,
        pattern,
        iterable,
    })
}

/// Whether `name` can be a Rust identifier, a raw one if it is a keyword such as `type`.
fn is_identifier(name: &str) -> bool {
    let is = |name: &str| syn::parse_str::<syn::Ident>(name).is_ok();
    is(name) || is(&format!("r#{name}"))
}

struct Parser<'a> {
    source: &'a Source,
    /// The offset of the next character to read.
    at: usize,
    /// The elements whose closing tag has not been read yet, outermost first, each
    /// with where its `<name` is.
    open: Vec<(Element, Range<usize>)>,
    roots: Vec<Node>,
    errors: Vec<Error>,
}

impl<'a> Parser<'a> {
    fn node(&mut self) {
        if self.rest().starts_with("<!") {
            // A comment or a doctype is sent as written, as in Phoenix, and what
            // looks like a tag or an expression inside it is not one.
            let (start, rest) = (self.at, self.rest());
            let comment = rest.starts_with("<!--");
            let (open, end) = if comment {
                ("<!--", "-->")
            } else {
                ("<!", ">")
            };
            match rest.find(end) {
                Some(index) => self.at += index + end.len(),
                None => {
                    self.at += rest.len();
                    let message = format!("`{open}` is missing the `{end}` that ends it");
                    self.error(start..start + open.len(), message);
                }
            }
            self.push(Node::Text(self.source.text[start..self.at].to_owned()));
        } else if self.eat("</") {
            self.closing_tag();
        } else if self.eat("<") {
            self.opening_tag();
        } else if self.is_expr() {
            if let Some(expr) = self.expr() {
                self.push(Node::Expr(expr));
            }
        } else {
            // Text runs to the next tag or expression. A `{` that is not an
            // expression, such as one inside a string literal, is text. A file has
            // no string literals, so there it was meant to be an expression.
            let (rest, file) = (self.rest(), self.source.file.is_some());
            if file && rest.starts_with('{') {
                let message = "expected a Rust expression and its closing `}` after `{`: \
                               write `&lbrace;` for a brace that is text";
                self.error(self.at..self.at + 1, message);
            }
            let length = rest
                .match_indices(['<', '{'])
                .map(|(index, _)| index)
                .find(|&index| {
                    index > 0
                        && (rest.as_bytes()[index] == b'<'
                            || file
                            || self.source.expr_at(self.at + index).is_some())
                })
                .unwrap_or(rest.len());
            let text = self.rest()[..length].to_owned();
            self.push(Node::Text(text));
            self.at += length;
        }
    }

    /// Whether an `{expression}` starts here.
    fn is_expr(&self) -> bool {
        self.source.expr_at(self.at).is_some()
    }

    /// Reads the `{expression}` here, which is `None` if it is empty.
    fn expr(&mut self) -> Option<Braces> {
        let at = self.at;
        let (end, group) = self.source.expr_at(at).unwrap();
        self.at = end;
        if group.stream().is_empty() {
            self.error(at..end, "expected a Rust expression between the braces");
            return None;
        }
        Some(Braces { group, at })
    }

    fn opening_tag(&mut self) {
        let start = self.at - 1;
        let name = self.name();
        let tag = start..self.at;
        if name.is_empty() {
            return self.error(tag, "expected a tag name after `<`");
        }
        let mut element = Element {
            void: VOID.contains(&name),
            name: name.to_owned(),
            at: start + 1,
            attrs: Vec::new(),
            condition: None,
            repeat: None,
            key: None,
            argument: None,
            children: Vec::new(),
        };
        let self_closed = self.attributes(&mut element, &tag);
        if RAW_TEXT.contains(&name) && !self_closed {
            // Up to the closing tag, which is then read as any other.
            let rest = self.rest();
            let length = rest.find(&format!("</{name}")).unwrap_or(rest.len());
            element.children.push(Node::Text(rest[..length].to_owned()));
            self.at += length;
        }
        if element.void || self_closed {
            self.element(element, tag);
        } else {
            self.open.push((element, tag));
        }
    }

    /// Reads the attributes of an opening tag and the `>` or `/>` that ends it.
    /// Returns whether it was `/>`.
    fn attributes(&mut self, element: &mut Element, tag: &Range<usize>) -> bool {
        // Where `:key` is, and whether there is a `:for` for it, well-formed or not.
        let (mut key_at, mut repeated) = (None, false);
        let self_closed = loop {
            self.whitespace();
            if self.eat("/>") {
                break true;
            }
            if self.eat(">") {
                break false;
            }
            if self.rest().is_empty() || self.rest().starts_with('<') {
                let message = format!("`<{}` is missing the `>` that ends the tag", element.name);
                self.error(tag.clone(), message);
                break false;
            }
            if self.is_expr() {
                let start = self.at;
                element.attrs.extend(self.expr().map(Attr::Spread));
                if element.is_component() {
                    // No spread onto a Component. Lower it to a method of
                    // `GlobalAttributes` when a Component has to pass its own on.
                    self.error(
                        start..self.at,
                        "attributes cannot be spread onto a Component",
                    );
                } else if element.slot().is_some() {
                    self.error(
                        start..self.at,
                        "attributes cannot be spread onto a slot entry",
                    );
                }
                continue;
            }

            let (start, rest) = (self.at, self.rest());
            let length = rest
                .find(|char: char| char.is_whitespace() || "=/><\"'{}".contains(char))
                .unwrap_or(rest.len());
            let name = rest[..length].to_owned();
            if name.is_empty() {
                // Skip to where the next attribute could start.
                let first = rest.chars().next().map_or(0, char::len_utf8);
                let junk = rest
                    .find(|char: char| char.is_whitespace() || char == '>')
                    .map_or(rest.len(), |length| length.max(first));
                self.at += junk;
                let message = format!("expected an attribute name, found `{}`", &rest[..junk]);
                self.error(start..self.at, message);
                continue;
            }
            self.at += length;
            // These are not attributes: nothing of them reaches the HTML.
            let expected = match name.as_str() {
                ":if" => Some("expected a condition in braces: `:if={expression}`"),
                ":for" => Some(EXPECTED_FOR),
                ":key" => Some("expected a key in braces: `:key={expression}`"),
                ":let" => Some("expected a pattern in braces: `:let={pattern}`"),
                _ => None,
            };
            if name.starts_with(':') && expected.is_none() {
                // Phoenix's other special attributes.
                self.error(start..self.at, format!("unsupported attribute `{name}`"));
            }
            let at = start..self.at;
            if (name == "navigate" || name == "patch")
                && !element.is_component()
                && element.slot().is_none()
                && element.name != "a"
            {
                let message = format!(
                    "`{name}` makes a live link, so it goes on an `<a>` tag, not on `<{}>`",
                    element.name
                );
                self.error(at.clone(), message);
            }
            repeated |= name == ":for";
            if name == ":let" {
                if !element.is_component() && element.slot().is_none() {
                    let message = "`:let` binds the argument of a slot, so it goes on a \
                                   Component tag or a slot entry, not on an HTML element";
                    self.error(at.clone(), message);
                }
            } else if expected.is_some() && element.slot().is_some() {
                // A slot entry is given once, always. Lower `:if` and `:for`
                // to an `if` and a `for` around the call that gives it when needed.
                let message = format!("`{name}` cannot be on a slot entry yet");
                self.error(at.clone(), message);
            } else if !name.starts_with(':') && element.slot().is_some() && !is_identifier(&name) {
                let message = format!(
                    "`{name}` cannot be an attribute of a slot entry: \
                     its name has to be a Rust identifier"
                );
                self.error(at.clone(), message);
            }

            self.whitespace();
            let valued = self.eat("=");
            let equals = self.at;
            self.whitespace();
            if let Some(expected) = expected {
                let given = match name.as_str() {
                    ":if" => element.condition.is_some(),
                    ":for" => element.repeat.is_some(),
                    ":key" => element.key.is_some(),
                    _ => element.argument.is_some(),
                };
                if !valued || !self.is_expr() {
                    self.error(at.clone(), expected);
                } else if given {
                    self.error(at.clone(), format!("`{name}` is given twice"));
                }
            }
            if !valued {
                let value = None;
                element.attrs.push(Attr::Static { name, value, at });
                continue;
            }
            if self.is_expr() {
                let expr = self.expr();
                match name.as_str() {
                    ":if" => element.condition = expr,
                    ":for" => {
                        // Empty braces have been reported by `expr`.
                        let braces = expr.is_some();
                        element.repeat = expr.and_then(repeat);
                        if braces && element.repeat.is_none() {
                            self.error(at, EXPECTED_FOR);
                        }
                    }
                    ":key" => (element.key, key_at) = (expr, Some(at)),
                    ":let" => element.argument = expr,
                    _ => element.attrs.extend(expr.map(|expr| {
                        let at = start;
                        Attr::Dynamic { name, expr, at }
                    })),
                }
            } else if let Some(length) = self
                .rest()
                .strip_prefix(['"', '\''])
                .and_then(|rest| rest.find(&self.rest()[..1]))
            {
                let value = Some(self.rest()[1..=length].to_owned());
                self.at += length + 2;
                let at = start..self.at;
                if (name == "navigate" || name == "patch")
                    && !element.is_component()
                    && element.slot().is_none()
                    && value.as_deref().is_some_and(|to| !is_local_path(to))
                {
                    let message = format!(
                        "`{name}` takes a path on this server, as `/users/7`: no scheme, \
                         host, `//` or `/\\` start, space or control character"
                    );
                    self.error(at.clone(), message);
                }
                element.attrs.push(Attr::Static { name, value, at });
            } else {
                let message = format!(
                    "expected a value in double quotes or an `{{expression}}` after `{name}=`"
                );
                self.error(start..equals, message);
            }
        };
        if let Some(at) = key_at.filter(|_| !repeated) {
            self.error(
                at,
                "`:key` tells the items of a `:for` apart, and this element has none",
            );
        }
        self_closed
    }

    fn closing_tag(&mut self) {
        let start = self.at - 2;
        let name = self.name();
        self.whitespace();
        if !self.eat(">") {
            let message = format!("`</{name}` is missing the `>` that ends the tag");
            self.error(start..self.at, message);
        }
        let tag = start..self.at;

        if let Some(index) = self.open.iter().rposition(|(open, _)| open.name == name) {
            // Whatever was opened inside it should have been closed by now.
            while self.open.len() > index + 1 {
                self.unclosed();
            }
            self.close();
        } else if let Some((open, _)) = self.open.last() {
            // Taken to be a misspelling of the tag that belongs here.
            let message = format!(
                "expected `</{0}>` to close `<{0}>`, found `</{name}>`",
                open.name
            );
            self.error(tag, message);
            self.close();
        } else {
            self.error(tag, format!("`</{name}>` has no opening tag"));
        }
    }

    fn unclosed(&mut self) {
        let (open, tag) = self.open.last().unwrap();
        let message = format!("`<{}>` is never closed", open.name);
        self.error(tag.clone(), message);
        self.close();
    }

    /// Ends the innermost open element, which makes it a child of the one around it.
    fn close(&mut self) {
        let (element, tag) = self.open.pop().unwrap();
        self.element(element, tag);
    }

    /// Adds a finished element to the tree. `tag` is where its `<name` is.
    fn element(&mut self, element: Element, tag: Range<usize>) {
        if element.is_component() {
            let name = &element.name;
            if syn::parse_str::<syn::Ident>(name).is_err() {
                let message = format!(
                    "`<{name}>` is capitalised, so it calls a Component, \
                     whose name has to be a Rust identifier"
                );
                self.error(tag.clone(), message);
            }
            let is_content = |node: &Node| {
                !node.is_blank() && !matches!(node, Node::Element(entry) if entry.slot().is_some())
            };
            if element.argument.is_some() && !element.children.iter().any(is_content) {
                let message = format!(
                    "`:let` binds the argument of the default slot, \
                     and `<{name}>` has no content of its own"
                );
                self.error(tag.clone(), message);
            }
        }
        if let Some(slot) = element.slot() {
            let in_component =
                matches!(self.open.last(), Some((parent, _)) if parent.is_component());
            if !is_identifier(slot) {
                let message = format!(
                    "`<:{slot}>` gives a slot of a Component, \
                     whose name has to be a Rust identifier"
                );
                self.error(tag, message);
            } else if !in_component {
                let message = format!(
                    "`<:{slot}>` gives a slot of a Component, \
                     so it goes directly inside the tags of one"
                );
                self.error(tag, message);
            }
        }
        self.push(Node::Element(Box::new(element)));
    }

    fn error(&mut self, at: Range<usize>, message: impl Into<String>) {
        let message = message.into();
        self.errors.push(Error { at, message });
    }

    fn whitespace(&mut self) {
        self.at += self.rest().len() - self.rest().trim_start().len();
    }

    fn push(&mut self, node: Node) {
        match self.open.last_mut() {
            Some((parent, _)) => parent.children.push(node),
            None => self.roots.push(node),
        }
    }

    /// Reads a tag name, which is empty if there is none here.
    fn name(&mut self) -> &'a str {
        let rest = self.rest();
        let length = rest
            .find(|char: char| !(char.is_alphanumeric() || "-_:.".contains(char)))
            .unwrap_or(rest.len());
        self.at += length;
        &rest[..length]
    }

    fn rest(&self) -> &'a str {
        &self.source.text[self.at..]
    }

    fn eat(&mut self, text: &str) -> bool {
        let found = self.rest().starts_with(text);
        if found {
            self.at += text.len();
        }
        found
    }
}

/// The rule of `griffin_web::live`'s `local_path`, for a target known at compile time:
/// a path, not a URL that leaves this server. (The run-time one also asks that `http`
/// parses it as a URI, which this does not repeat: it is the narrower check here.)
// Two copies of one rule, as this crate cannot depend on griffin-web.
fn is_local_path(to: &str) -> bool {
    let mut chars = to.chars();
    chars.next() == Some('/')
        && !matches!(chars.next(), Some('/' | '\\'))
        && !to
            .chars()
            .any(|char| char.is_control() || matches!(char, '\\' | ' ' | '"' | '<' | '>' | '`'))
}
