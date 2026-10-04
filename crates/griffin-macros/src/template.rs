//! The template compiler (HEEx's shape, with real Rust expressions in `{}`): [`parse`] turns template source into a syntax
//! tree, [`codegen`] turns the tree into calls on `griffin_web::template` (every macro lowers to a public API).
//!
//! Both work on a [`Source`], not on tokens, so that they serve two front ends:
//! [`expand`] for the inline `html!` macro and [`expand_file`] for a template file.

mod codegen;
mod parse;

use std::collections::HashMap;
use std::iter::repeat_n;
use std::path::Path;
use std::{env, fs};

use proc_macro2::{Delimiter, Group, Ident, LineColumn, Span, TokenStream, TokenTree};
use quote::{quote, quote_spanned};

/// Template source as the parser reads it: the text, and where its Rust is.
#[derive(Default)]
pub struct Source {
    pub text: String,
    /// Each `{...}` holding a Rust expression, by the offset of its `{`: the offset
    /// after its `}`, and the braces with the tokens between them. The inline front
    /// end finds them, because only a Rust lexer knows where an expression ends. A
    /// file has none here: its braces are lexed where the parser asks for them.
    pub exprs: HashMap<usize, (usize, Group)>,
    /// The offset at which each token starts, in order, with its span. Empty for a
    /// file, which has no tokens, and then everything is at the macro call.
    pub spans: Vec<(usize, Span)>,
    /// Set for a template file, whose places are named in words because no span can
    /// point into it.
    pub file: Option<File>,
}

pub struct File {
    /// The path the file was asked for by, which is relative to the templates directory.
    name: String,
    /// The whitespace that `Source::text` lost at its start. Lines and columns count it.
    leading: String,
}

impl Source {
    /// The span of the token that covers `offset`.
    pub fn span_at(&self, offset: usize) -> Span {
        let after = self.spans.partition_point(|(start, _)| *start <= offset);
        let token = self.spans.get(after.saturating_sub(1));
        token.map_or_else(Span::call_site, |(_, span)| *span)
    }

    /// The `{...}` that starts at `offset`, if it holds Rust: the offset after its
    /// `}`, and the braces with the tokens between them.
    pub fn expr_at(&self, offset: usize) -> Option<(usize, Group)> {
        if self.file.is_none() {
            return self.exprs.get(&offset).cloned();
        }
        // Only a Rust lexer knows which `}` ends the expression (one may be in a
        // string), so each is tried in turn until the text up to it is one group.
        // Lexes the start of a long expression once for each `}` in it, and
        // again each time the parser asks. Write a brace matcher that knows Rust's
        // strings, chars, lifetimes and comments if a profile ever shows this.
        let text = &self.text[offset..];
        if !text.starts_with('{') {
            return None;
        }
        let mut ends = text.match_indices('}').map(|(index, _)| index + 1);
        ends.find_map(|end| {
            let mut tokens = text[..end].parse::<TokenStream>().ok()?.into_iter();
            match (tokens.next(), tokens.next()) {
                (Some(TokenTree::Group(group)), None) => Some((offset + end, group)),
                _ => None,
            }
        })
    }

    /// The line and column of `offset` in a template file, both counted from one.
    fn line_column(&self, offset: usize) -> (usize, usize) {
        let leading = self.file.as_ref().map_or("", |file| &file.leading);
        let before = format!("{leading}{}", &self.text[..offset]);
        let line_start = before.rfind('\n').map_or(0, |newline| newline + 1);
        (
            before.matches('\n').count() + 1,
            before[line_start..].chars().count() + 1,
        )
    }

    /// Marks generated code as coming from `offset`, for rustc to report an error in
    /// it there. The code is one expression. What comes back is a definition to put
    /// in a statement before it, and the expression to put in its place.
    ///
    /// Inline, the code already has the spans of the user's tokens, and comes back
    /// as it is. A stable proc macro has no span for a place in another file, so for
    /// a file the code becomes the body of a `macro_rules!` named after the file,
    /// line and column, called on the spot: rustc ends every error in the code with
    /// "this error originates in the macro `inbox_html_griffin_line_3_column_8`".
    /// A `macro_rules!` body sees the names in scope where it is defined, `self`
    /// included, so the code means what it did.
    pub fn locate(&self, offset: usize, code: TokenStream) -> (TokenStream, TokenStream) {
        let Some(file) = &self.file else {
            return (TokenStream::new(), code);
        };
        let (line, column) = self.line_column(offset);
        let name = file
            .name
            .replace(|char: char| !char.is_ascii_alphanumeric(), "_");
        let name = format!("{name}_line_{line}_column_{column}");
        // A file name may start with a digit, and an identifier may not.
        let name = syn::parse_str::<Ident>(&name)
            .unwrap_or_else(|_| Ident::new(&format!("_{name}"), Span::call_site()));
        (
            quote!(macro_rules! #name { () => { #code } }),
            quote!(#name!()),
        )
    }

    /// As [`Source::locate`], for where no statement can come before the code.
    pub fn located(&self, offset: usize, code: TokenStream) -> TokenStream {
        match self.locate(offset, code) {
            (definition, code) if definition.is_empty() => code,
            (definition, code) => quote!({ #definition #code }),
        }
    }
}

/// What `html! { input }` expands to: the `Rendered` expression, preceded by a
/// `compile_error!` for each syntax error, spanned at the tokens at fault.
pub fn expand(input: TokenStream) -> TokenStream {
    let mut inline = Inline::default();
    inline.read(input);
    let (nodes, errors) = parse::parse(&inline.source);
    let errors = errors.into_iter().map(|error| {
        let (start, end) = (
            inline.source.span_at(error.at.start),
            inline.source.span_at(error.at.end - 1),
        );
        // Two tokens, because a stable proc macro cannot join spans: syn underlines
        // from the first to the last.
        let at = quote_spanned!(start=> a)
            .into_iter()
            .chain(quote_spanned!(end=> b));
        syn::Error::new_spanned(at.collect::<TokenStream>(), error.message).into_compile_error()
    });
    let rendered = codegen::generate(&nodes, &inline.source);
    quote!({ #(#errors)* #rendered })
}

/// The directory of a crate that its template files are in.
const DIRECTORY: &str = "templates";

/// What `html_file!("name")` expands to: as [`expand`], for the text of the file
/// `name` in the `templates` directory of the crate being compiled.
///
/// The file front end. It has no tokens and so no spans: a syntax error names the
/// file, line and column in its message, and is reported at the macro call.
pub fn expand_file(input: TokenStream) -> TokenStream {
    let Ok(name) = syn::parse2::<syn::LitStr>(input.clone()) else {
        let message = "expected the name of a template file as a string literal: \
                       `html_file!(\"page.html.griffin\")`";
        return syn::Error::new_spanned(input, message).into_compile_error();
    };
    // Set by cargo for every crate it compiles.
    let root = env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
    let path = Path::new(&root).join(DIRECTORY).join(name.value());
    let path = path.to_string_lossy();
    let text = match fs::read_to_string(&*path) {
        Ok(text) => text,
        Err(error) => {
            let message = format!("cannot read the template file `{path}`: {error}");
            return syn::Error::new(name.span(), message).into_compile_error();
        }
    };

    // The same static parts whichever system and editor the file was saved with.
    let text = text.replace("\r\n", "\n");
    let text = text.trim_start_matches('\u{feff}');
    // What an editor leaves around the template is not part of it, as in Phoenix.
    let leading = text.len() - text.trim_start().len();
    let source = Source {
        text: text.trim().to_owned(),
        file: Some(File {
            name: name.value(),
            leading: text[..leading].to_owned(),
        }),
        ..Source::default()
    };
    let (nodes, errors) = parse::parse(&source);
    let errors = errors.into_iter().map(|error| {
        let (line, column) = source.line_column(error.at.start);
        let message = format!("{path}:{line}:{column}: {}", error.message);
        syn::Error::new(name.span(), message).into_compile_error()
    });
    let rendered = codegen::generate(&nodes, &source);
    // rustc knows of no file but those it reads itself, and cargo recompiles a crate
    // when one of them changes. A stable proc macro has no other way to say so.
    quote!({
        const _: &[u8] = ::core::include_bytes!(#path);
        #(#errors)*
        #rendered
    })
}

/// The inline front end: rebuilds the source text of a token stream.
///
/// Tokens carry no whitespace, but HTML text needs it, so the gaps between tokens are
/// filled in from their line and column. A Rust comment therefore reads as whitespace.
// Tokens without positions of their own (a template written inside a
// `macro_rules!` body, or expanded by rust-analyzer) lose their whitespace. Fall back
// to `Span::source_text` of the call site if that ever matters.
#[derive(Default)]
struct Inline {
    source: Source,
    /// Where the first token starts and the last one read ended.
    position: Option<(LineColumn, LineColumn)>,
}

impl Inline {
    fn read(&mut self, tokens: TokenStream) {
        for tree in tokens {
            match tree {
                TokenTree::Group(group) if group.delimiter() == Delimiter::Brace => {
                    // What is between the braces is text too, for where a brace is
                    // no expression: in a comment, a `<style>` or a `<script>`.
                    let start = self.token("{", group.span_open());
                    self.read(group.stream());
                    self.token("}", group.span_close());
                    let end = self.source.text.len();
                    self.source.exprs.insert(start, (end, group));
                }
                TokenTree::Group(group) => {
                    let (open, close) = match group.delimiter() {
                        Delimiter::Parenthesis => ("(", ")"),
                        Delimiter::Bracket => ("[", "]"),
                        _ => ("", ""),
                    };
                    self.token(open, group.span_open());
                    self.read(group.stream());
                    self.token(close, group.span_close());
                }
                leaf => {
                    self.token(&leaf.to_string(), leaf.span());
                }
            }
        }
    }

    /// Appends one token after the whitespace that separated it from the one before,
    /// and returns the offset it starts at.
    fn token(&mut self, text: &str, span: Span) -> usize {
        let (start, end) = (span.start(), span.end());
        let (first, last) = *self.position.get_or_insert((start, start));
        if start.line > last.line {
            // Indentation is counted from the column of the first token, so that
            // moving the whole template sideways changes nothing.
            let indent = start.column.saturating_sub(first.column);
            self.source
                .text
                .extend(repeat_n('\n', start.line - last.line));
            self.source.text.extend(repeat_n(' ', indent));
        } else if start.line == last.line && start.column > last.column {
            let gap = start.column - last.column;
            self.source.text.extend(repeat_n(' ', gap));
        }
        self.position = Some((first, end));

        let offset = self.source.text.len();
        self.source.spans.push((offset, span));
        self.source.text.push_str(text);
        offset
    }
}

#[cfg(test)]
mod tests {
    //! Snapshots of the expansion. They pin two things a user depends on: that it
    //! calls nothing but the documented `griffin_web::template` API (every macro lowers to a public API), and
    //! the fingerprint, which has to come out the same in every compiler run or a
    //! redeployed server would resend statics its clients already have.

    use super::expand;

    fn expansion(template: &str) -> String {
        expand(template.parse().unwrap()).to_string()
    }

    #[test]
    fn text_around_an_expression() {
        assert_eq!(
            expansion("<p>Hello, {name}!</p>"),
            "{ :: griffin_web :: template :: Rendered :: new (\
                10701045657905709241u64 , \
                & [\"<p>Hello, \" , \"!</p>\"] , \
                :: std :: vec ! [:: griffin_web :: template :: Slot :: from (& (name))] ,\
             ) . single_root () }"
        );
    }

    #[test]
    fn every_kind_of_attribute_on_a_self_closing_tag() {
        assert_eq!(
            expansion(
                r#"<input type="text" name={@name} disabled={@busy} class={@class} {@rest.iter()} />"#
            ),
            "{ :: griffin_web :: template :: Rendered :: new (\
                107579774985825999u64 , \
                & [\"<input type=\\\"text\\\"\" , \"\" , \" class=\\\"\" , \"\\\"\" , \">\"] , \
                :: std :: vec ! [\
                    :: griffin_web :: template :: Slot :: attribute (\"name\" , & (self . name)) , \
                    :: griffin_web :: template :: Slot :: attribute (\"disabled\" , & (self . busy)) , \
                    :: griffin_web :: template :: Slot :: from (& (self . class)) , \
                    :: griffin_web :: template :: Slot :: attributes (self . rest . iter ())\
                ] ,\
             ) . single_root () }"
        );
    }

    #[test]
    fn two_roots_over_several_lines() {
        assert_eq!(
            expansion("<h1>{@title}</h1>\n<ul>\n    <li>{@items.len() + 1} items<br></li>\n</ul>"),
            "{ :: griffin_web :: template :: Rendered :: new (\
                18151500310010583005u64 , \
                & [\"<h1>\" , \"</h1>\\n<ul>\\n    <li>\" , \" items<br></li>\\n</ul>\"] , \
                :: std :: vec ! [\
                    :: griffin_web :: template :: Slot :: from (& (self . title)) , \
                    :: griffin_web :: template :: Slot :: from (& (self . items . len () + 1))\
                ] ,\
             ) }"
        );
    }

    #[test]
    fn an_element_with_if_and_an_if_without_else() {
        assert_eq!(
            expansion("<p :if={@show}>Hello, {name}!</p>{if @admin { 1 }}"),
            "{ :: griffin_web :: template :: Rendered :: new (\
                17985182406306147860u64 , \
                & [\"\" , \"\" , \"\"] , \
                :: std :: vec ! [\
                    if self . show { \
                        :: griffin_web :: template :: Slot :: template (\
                            :: griffin_web :: template :: Rendered :: new (\
                                10701045657905709241u64 , \
                                & [\"<p>Hello, \" , \"!</p>\"] , \
                                :: std :: vec ! [:: griffin_web :: template :: Slot :: from (& (name))] ,\
                            )\
                        ) \
                    } else { :: griffin_web :: template :: Slot :: text (\"\") } , \
                    if self . admin { :: griffin_web :: template :: Slot :: from (& { 1 }) } \
                    else { :: griffin_web :: template :: Slot :: text (\"\") }\
                ] ,\
             ) }"
        );
    }

    #[test]
    fn an_element_with_for_keyed_and_filtered() {
        assert_eq!(
            expansion(
                "<ul><li :for={(id, name) in &@guests} :key={id} :if={@all || *id > 9}>{name}</li></ul>"
            ),
            "{ :: griffin_web :: template :: Rendered :: new (\
                66842448558921622u64 , \
                & [\"<ul>\" , \"</ul>\"] , \
                :: std :: vec ! [{ \
                    let mut entries = :: std :: vec :: Vec :: new () ; \
                    for (id , name) in &self . guests { \
                        if self . all || * id > 9 { \
                            entries . push ((\
                                :: std :: string :: ToString :: to_string (& (id)) , \
                                :: std :: vec ! [:: griffin_web :: template :: Slot :: from (& (name))]\
                            )) ; \
                        } \
                    } \
                    :: griffin_web :: template :: Slot :: keyed_comprehension (\
                        14384822050611926874u64 , & [\"<li>\" , \"</li>\"] , entries\
                    ) \
                }] ,\
             ) . single_root () }"
        );
    }

    #[test]
    fn a_component_tag() {
        assert_eq!(
            expansion(r#"<Badge label="new" count={@unread} highlighted data-id={7} />"#),
            "{ :: griffin_web :: template :: Rendered :: new (\
                763862646787557219u64 , \
                & [\"\" , \"\"] , \
                :: std :: vec ! [:: griffin_web :: template :: Slot :: template ({ \
                    # [allow (unused_imports)] \
                    use :: griffin_web :: template :: GlobalAttributes as _ ; \
                    # [allow (elided_lifetimes_in_paths)] \
                    const _ : () = :: griffin_web :: template :: require (\
                        \"attribute\" , \
                        < Badge as :: griffin_web :: template :: Component > :: REQUIRED , \
                        & [\"label\" , \"count\" , \"highlighted\"] ,\
                    ) ; \
                    # [allow (elided_lifetimes_in_paths)] \
                    const _ : () = :: griffin_web :: template :: require (\
                        \"slot\" , \
                        < Badge as :: griffin_web :: template :: Component > :: REQUIRED_SLOTS , \
                        & [] ,\
                    ) ; \
                    # [allow (elided_lifetimes_in_paths)] \
                    let component = < Badge as :: std :: default :: Default > :: default () ; \
                    let component = component . label (\"new\") ; \
                    let component = component . count (self . unread) ; \
                    let component = component . highlighted (true) ; \
                    let component = :: griffin_web :: template :: GlobalAttributes :: attribute (\
                        component , \"data-id\" , 7\
                    ) ; \
                    :: griffin_web :: template :: Component :: render (component) \
                })] ,\
             ) }"
        );
    }

    #[test]
    fn a_component_tag_with_a_slot_entry_and_content() {
        assert_eq!(
            expansion(
                r#"<Table rows={&@users}><:col label="Name" wide :let={user}>{user.name}</:col>None of {@kind}</Table>"#
            ),
            "{ :: griffin_web :: template :: Rendered :: new (\
                763862646787557219u64 , \
                & [\"\" , \"\"] , \
                :: std :: vec ! [:: griffin_web :: template :: Slot :: template ({ \
                    # [allow (unused_imports)] \
                    use :: griffin_web :: template :: GlobalAttributes as _ ; \
                    # [allow (elided_lifetimes_in_paths)] \
                    const _ : () = :: griffin_web :: template :: require (\
                        \"attribute\" , \
                        < Table as :: griffin_web :: template :: Component > :: REQUIRED , \
                        & [\"rows\"] ,\
                    ) ; \
                    # [allow (elided_lifetimes_in_paths)] \
                    const _ : () = :: griffin_web :: template :: require (\
                        \"slot\" , \
                        < Table as :: griffin_web :: template :: Component > :: REQUIRED_SLOTS , \
                        & [\"col\" , \"inner_block\"] ,\
                    ) ; \
                    # [allow (elided_lifetimes_in_paths)] \
                    let component = < Table as :: std :: default :: Default > :: default () ; \
                    let component = component . rows (&self . users) ; \
                    let component = component . col (| slot | { \
                        let attributes = slot . push (\
                            | user | :: griffin_web :: template :: Rendered :: new (\
                                763862646787557219u64 , \
                                & [\"\" , \"\"] , \
                                :: std :: vec ! [:: griffin_web :: template :: Slot :: from (& (user . name))] ,\
                            )\
                        ) ; \
                        attributes . label = \"Name\" ; \
                        attributes . wide = true ; \
                    }) ; \
                    let component = component . inner_block (| slot | { \
                        slot . push (\
                            | _ | :: griffin_web :: template :: Rendered :: new (\
                                14676726458371328866u64 , \
                                & [\"None of \" , \"\"] , \
                                :: std :: vec ! [:: griffin_web :: template :: Slot :: from (& (self . kind))] ,\
                            )\
                        ) ; \
                    }) ; \
                    :: griffin_web :: template :: Component :: render (component) \
                })] ,\
             ) }"
        );
    }
}
