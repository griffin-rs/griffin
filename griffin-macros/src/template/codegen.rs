//! Syntax tree to Rust: a call to `griffin_web::template::Rendered::new`, with the
//! tree flattened into static parts and the slots between them. An element with `:if`
//! is a slot holding a nested template of its own, one with `:for` a slot holding a
//! comprehension, and a Component tag a slot holding the Component's template. What
//! is between the tags of a Component is given to it as templates of their own, each
//! in a closure: one for each `<:name>` and one for the rest.

use proc_macro2::{Delimiter, Group, Ident, Literal, Span, TokenStream, TokenTree};
use quote::{quote, quote_spanned};

use super::Source;
use super::parse::{Attr, Braces, Element, Node, Repeat};

pub fn generate(nodes: &[Node], source: &Source) -> TokenStream {
    let mut template = Template::new(source);
    template.nodes(nodes);
    let rendered = template.rendered();
    // Phoenix's rule: the template is exactly one HTML element, and one that is
    // always there, once.
    let single_root = matches!(nodes, [Node::Element(element)]
        if element.condition.is_none() && element.repeat.is_none() && !element.is_component())
    .then(|| quote!(.single_root()));
    quote!(#rendered #single_root)
}

struct Template<'a> {
    /// Where the tokens of a tag or an attribute are, by offset.
    source: &'a Source,
    /// Always one more than `slots`: the last one is still being written.
    statics: Vec<String>,
    slots: Vec<TokenStream>,
}

impl<'a> Template<'a> {
    fn new(source: &'a Source) -> Template<'a> {
        Template {
            source,
            statics: vec![String::new()],
            slots: Vec::new(),
        }
    }

    fn rendered(self) -> TokenStream {
        let Template { statics, slots, .. } = self;
        let fingerprint = fingerprint(&statics);
        quote! {
            ::griffin_web::template::Rendered::new(
                #fingerprint,
                &[#(#statics),*],
                ::std::vec![#(#slots),*],
            )
        }
    }

    fn nodes<'n>(&mut self, nodes: impl IntoIterator<Item = &'n Node>) {
        for node in nodes {
            match node {
                Node::Text(text) => self.text(text),
                Node::Expr(expr) => self.slot(self.source.located(expr.at, text_slot(expr))),
                Node::Element(element) => match (&element.repeat, &element.condition) {
                    (Some(repeat), condition) => {
                        let mut entry = Template::new(self.source);
                        entry.element(element);
                        self.slot(comprehension(entry, element, repeat, condition.as_ref()));
                    }
                    (None, None) => self.element(element),
                    // As in Phoenix, the element is a template of its own, which is
                    // not declared a single root, and is empty text when left out.
                    (None, Some(condition)) => {
                        let mut nested = Template::new(self.source);
                        nested.element(element);
                        let nested = nested.rendered();
                        let holds = lower(condition.stream());
                        let holds = self.source.located(condition.at, holds);
                        self.slot(quote_spanned! {at(condition.span())=>
                            if #holds {
                                ::griffin_web::template::Slot::template(#nested)
                            } else {
                                ::griffin_web::template::Slot::text("")
                            }
                        });
                    }
                },
            }
        }
    }

    fn element(&mut self, element: &Element) {
        if element.is_component() {
            let slot = component(element, self.source);
            return self.slot(self.source.located(element.at, slot));
        }
        self.text("<");
        self.text(&element.name);
        element.attrs.iter().for_each(|attr| self.attr(attr));
        self.text(">");
        if !element.void {
            self.nodes(&element.children);
            self.text("</");
            self.text(&element.name);
            self.text(">");
        }
    }

    fn attr(&mut self, attr: &Attr) {
        match attr {
            Attr::Static {
                name, value: None, ..
            } => self.text(&format!(" {name}")),
            // The same live link, written out: the parser has seen it is on an `<a>`.
            Attr::Static {
                name,
                value: Some(value),
                at,
            } if name == "navigate" || name == "patch" => {
                let link = Ident::new(name, Span::call_site());
                let slot = quote! {
                    ::griffin_web::template::Slot::attributes(
                        ::griffin_web::live::Link::#link(#value)
                    )
                };
                self.slot(self.source.located(at.start, slot));
            }
            Attr::Static {
                name,
                value: Some(value),
                ..
            } => {
                // Only a value given in single quotes can have a double quote in it.
                let quote = if value.contains('"') { '\'' } else { '"' };
                self.text(&format!(" {name}={quote}{value}{quote}"));
            }
            // As in Phoenix, these two are always present, so only the value is a slot.
            Attr::Dynamic { name, expr, .. } if name == "class" || name == "style" => {
                self.text(&format!(" {name}=\""));
                self.slot(self.source.located(expr.at, text_slot(expr)));
                self.text("\"");
            }
            // Live links: `<a navigate={path}>` and `<a patch={path}>` write the `href`
            // and what the Phoenix client reads to follow it over the connection.
            Attr::Dynamic { name, expr, .. } if name == "navigate" || name == "patch" => {
                let value = lower(expr.stream());
                let link = Ident::new(name, Span::call_site());
                let slot = quote_spanned! {at(expr.span())=>
                    ::griffin_web::template::Slot::attributes(
                        ::griffin_web::live::Link::#link(&(#value))
                    )
                };
                self.slot(self.source.located(expr.at, slot));
            }
            Attr::Dynamic { name, expr, .. } => {
                let value = lower(expr.stream());
                let slot = quote_spanned! {at(expr.span())=>
                    ::griffin_web::template::Slot::attribute(#name, &(#value))
                };
                self.slot(self.source.located(expr.at, slot));
            }
            Attr::Spread(expr) => {
                let attributes = lower(expr.stream());
                let slot = quote_spanned! {at(expr.span())=>
                    ::griffin_web::template::Slot::attributes(#attributes)
                };
                self.slot(self.source.located(expr.at, slot));
            }
        }
    }

    fn text(&mut self, text: &str) {
        self.statics.last_mut().unwrap().push_str(text);
    }

    fn slot(&mut self, slot: TokenStream) {
        self.slots.push(slot);
        self.statics.push(String::new());
    }
}

/// The slot for an element with `:for`: the element once for each item, as the
/// entries of a comprehension. As in Phoenix, an `:if` beside the `:for` can read what
/// the pattern binds and leaves items out, and the element is the entry itself, not a
/// template of its own.
fn comprehension(
    entry: Template,
    element: &Element,
    repeat: &Repeat,
    condition: Option<&Braces>,
) -> TokenStream {
    let Template {
        statics,
        slots,
        source,
    } = entry;
    let fingerprint = fingerprint(&statics);
    let Repeat {
        braces, pattern, ..
    } = repeat;
    let mut iterable = lower(repeat.iterable.clone());
    if source.file.is_some() {
        // rustc reports what cannot be iterated at the loop it rewrites `for` into,
        // and an error there loses the note that `locate` is for. The call the loop
        // would make, made here, is reported as any other.
        iterable = quote!(::std::iter::IntoIterator::into_iter(#iterable));
    }
    // Not in a block of its own: what the iterable borrows has to live through the loop.
    let (iterable_at, iterable) = source.locate(braces.at, iterable);
    // Not a name the user's expressions can see, so it shadows nothing of theirs.
    let entries = Ident::new("entries", Span::mixed_site());

    let entry = quote!(::std::vec![#(#slots),*]);
    let (constructor, entry) = match &element.key {
        None => (quote!(comprehension), entry),
        Some(key) => {
            let value = lower(key.stream());
            let text =
                quote_spanned!(at(key.span())=> ::std::string::ToString::to_string(&(#value)));
            let key = source.located(key.at, text);
            (quote!(keyed_comprehension), quote!((#key, #entry)))
        }
    };
    let push = quote!(#entries.push(#entry););
    let push = match condition {
        None => push,
        Some(condition) => {
            let holds = source.located(condition.at, lower(condition.stream()));
            quote_spanned!(at(condition.span())=> if #holds { #push })
        }
    };
    quote_spanned! {at(braces.span())=> {
        let mut #entries = ::std::vec::Vec::new();
        #iterable_at
        for #pattern in #iterable {
            #push
        }
        ::griffin_web::template::Slot::#constructor(#fingerprint, &[#(#statics),*], #entries)
    }}
}

/// The slot for a Component tag: the Component's template, nested. What a tag expands
/// to is documented on `griffin_web::template::Component`, for whoever writes a
/// Component by hand (every macro lowers to a public API).
///
/// Whether the name is a Component is not known here, since a macro cannot look a
/// name up: it is used as one, at the tag, and rustc reports a name that is not.
fn component(element: &Element, source: &Source) -> TokenStream {
    let tag_at = source.span_at(element.at);
    let Some(tag) = ident(&element.name, tag_at) else {
        // Not a name, which the parser has reported.
        return quote!(::griffin_web::template::Slot::text(""));
    };
    // Not a name the user's expressions can see, so it shadows nothing of theirs.
    // Said to be at the tag or attribute it is used for, where rustc then reports it.
    let component = |at: Span| Ident::new("component", Span::mixed_site().located_at(at));

    let mut given = Vec::new();
    let set = element.attrs.iter().filter_map(|attr| {
        let (name, name_at, value) = attribute(attr, source)?;
        let component = component(name_at);
        // A name that cannot be a method is an extra HTML attribute.
        Some(match ident(name, name_at) {
            Some(method) => {
                given.push(name);
                quote_spanned!(at(name_at)=> let #component = #component.#method(#value);)
            }
            None => quote_spanned! {at(name_at)=>
                let #component = ::griffin_web::template::GlobalAttributes::attribute(
                    #component, #name, #value
                );
            },
        })
    });
    let mut set: Vec<_> = set.collect();

    // Each `<:name>` is an entry of the slot `name`, and what content is left is one
    // of the default slot, `inner_block`.
    let mut content = Template::new(source);
    let (mut slots, mut blank, mut after_entry) = (Vec::new(), true, false);
    for node in &element.children {
        if let Node::Element(entry) = node
            && let Some(name) = entry.slot()
        {
            // The name after the `:`, which the parser has reported if it is not one.
            let name_at = source.span_at(entry.at + 1);
            if let Some(method) = ident(name, name_at) {
                let mut content = Template::new(source);
                content.nodes(&entry.children);
                slots.push(name);
                set.push(slot_entry(component(name_at), method, entry, content));
            }
            after_entry = true;
            continue;
        }
        match node {
            // As in Phoenix, text after a slot entry loses its leading whitespace.
            Node::Text(text) if after_entry => content.text(text.trim_start()),
            node => content.nodes([node]),
        }
        (blank, after_entry) = (blank && node.is_blank(), false);
    }
    if !blank {
        let method = Ident::new("inner_block", tag_at);
        slots.push("inner_block");
        set.push(slot_entry(component(tag_at), method, element, content));
    }
    let component = component(tag_at);

    quote_spanned! {at(tag_at)=>
        ::griffin_web::template::Slot::template({
            // Its methods are the extra HTML attributes that are identifiers.
            #[allow(unused_imports)]
            use ::griffin_web::template::GlobalAttributes as _;
            #[allow(elided_lifetimes_in_paths)]
            const _: () = ::griffin_web::template::require(
                "attribute",
                <#tag as ::griffin_web::template::Component>::REQUIRED,
                &[#(#given),*],
            );
            #[allow(elided_lifetimes_in_paths)]
            const _: () = ::griffin_web::template::require(
                "slot",
                <#tag as ::griffin_web::template::Component>::REQUIRED_SLOTS,
                &[#(#slots),*],
            );
            #[allow(elided_lifetimes_in_paths)]
            let #component = <#tag as ::std::default::Default>::default();
            #(#set)*
            ::griffin_web::template::Component::render(#component)
        })
    }
}

/// An attribute of a Component tag or a slot entry: its name, where the name is, and
/// its value as it is given, by value, a bare attribute being `true`. Each is where
/// the user wrote it, so that is where rustc reports one of the wrong type.
fn attribute<'a>(attr: &'a Attr, source: &Source) -> Option<(&'a str, Span, TokenStream)> {
    Some(match attr {
        Attr::Static {
            name,
            value: None,
            at: place,
        } => {
            let name_at = source.span_at(place.start);
            (name, name_at, quote_spanned!(at(name_at)=> true))
        }
        Attr::Static {
            name,
            value: Some(value),
            at: place,
        } => {
            let mut value = Literal::string(value);
            value.set_span(source.span_at(place.end - 1));
            (name, source.span_at(place.start), quote!(#value))
        }
        Attr::Dynamic {
            name,
            expr,
            at: place,
        } => {
            let value = source.located(expr.at, lower(expr.stream()));
            (name, source.span_at(*place), value)
        }
        // Reported by the parser.
        Attr::Spread(_) => return None,
    })
}

/// The call that gives a slot one entry: the method named after the slot, with a
/// closure that pushes the content onto the slot and assigns the entry's attributes.
/// `given` is the slot entry, or for the default slot the Component tag.
fn slot_entry(component: Ident, method: Ident, given: &Element, content: Template) -> TokenStream {
    let source = content.source;
    let content = content.rendered();
    // Not names the user's expressions can see. The attributes are said to be at
    // the one being given, where rustc then reports a value of the wrong type.
    let slot = Ident::new("slot", Span::mixed_site());
    let attributes = |at: Span| Ident::new("attributes", Span::mixed_site().located_at(at));
    // `:let={pattern}` binds the argument the Component renders the content with.
    let argument = given.argument.as_ref();
    let pattern = argument.map_or_else(|| quote!(_), |braces| braces.stream());
    let push = quote!(#slot.push(|#pattern| #content));
    // An attribute of the Component tag is the Component's, not the default slot's.
    let assign = given.attrs.iter().filter(|_| given.slot().is_some());
    let assign: Vec<_> = assign
        .filter_map(|attr| {
            let (name, name_at, value) = attribute(attr, source)?;
            let (field, attributes) = (ident(name, name_at)?, attributes(name_at));
            Some(quote_spanned!(at(name_at)=> #attributes.#field = #value;))
        })
        .collect();
    let body = if assign.is_empty() {
        quote!(#push;)
    } else {
        let attributes = attributes(method.span());
        quote!(let #attributes = #push; #(#assign)*)
    };
    quote_spanned!(at(method.span())=> let #component = #component.#method(|#slot| { #body });)
}

/// `name` as a Rust identifier, a raw one if it is a keyword such as `type`. `None`
/// for a name that cannot be one, such as `data-id`.
fn ident(name: &str, span: Span) -> Option<Ident> {
    if syn::parse_str::<Ident>(name).is_ok() {
        return Some(Ident::new(name, span));
    }
    let raw = syn::parse_str::<Ident>(&format!("r#{name}"));
    raw.is_ok().then(|| Ident::new_raw(name, span))
}

/// Where generated code that wraps what the user wrote is said to be: at the user's
/// tokens, so that rustc reports a value of the wrong type there, but still marked
/// as macro output, so that lints do not blame the user for how it is written.
fn at(span: Span) -> Span {
    Span::call_site().located_at(span)
}

/// The slot for `{expression}` as content. The expression is borrowed, because a
/// template reads its state and must not move out of it.
///
/// An `if` with no final `else` has no value in Rust. In a template it is, as in
/// Phoenix, empty text when no branch is taken, so each branch becomes a slot by itself.
fn text_slot(expr: &Group) -> TokenStream {
    let tokens: Vec<TokenTree> = lower(expr.stream()).into_iter().collect();
    let is_keyword =
        |token: &TokenTree, keyword| matches!(token, TokenTree::Ident(ident) if ident == keyword);
    let is_block = |token: &TokenTree| {
        let TokenTree::Group(group) = token else {
            return false;
        };
        group.delimiter() == Delimiter::Brace
    };
    // `if .. {..} else if .. {..}`: every part between two `else` is an `if` and its block.
    let branches = tokens.split(|token| is_keyword(token, "else"));
    let lacks_else = branches.clone().all(|branch| {
        branch.first().is_some_and(|token| is_keyword(token, "if"))
            && branch.last().is_some_and(is_block)
    });
    if !lacks_else {
        return quote_spanned!(at(expr.span())=> ::griffin_web::template::Slot::from(&(#(#tokens)*)));
    }
    let branches = branches.map(|branch| {
        let (block, condition) = branch.split_last().unwrap();
        quote_spanned!(at(expr.span())=> #(#condition)* { ::griffin_web::template::Slot::from(&#block) })
    });
    quote_spanned!(at(expr.span())=> #(#branches else)* { ::griffin_web::template::Slot::text("") })
}

/// The user's expression with each `@name` replaced by `self.name`, and every other
/// token untouched, so that rustc reports errors at the user's code.
fn lower(expr: TokenStream) -> TokenStream {
    let mut lowered = TokenStream::new();
    let mut tokens = expr.into_iter().peekable();
    // In `name @ pattern` the `@` is Rust's own, and it follows a name.
    let mut after_name = false;
    while let Some(token) = tokens.next() {
        let is_name = matches!(&token, TokenTree::Ident(ident)
            if syn::parse_str::<syn::Ident>(&ident.to_string()).is_ok());
        match token {
            TokenTree::Punct(at)
                if at.as_char() == '@'
                    && !after_name
                    && matches!(tokens.peek(), Some(TokenTree::Ident(_))) =>
            {
                lowered.extend(quote_spanned!(at.span()=> self.));
            }
            TokenTree::Group(group) => {
                let mut inner = Group::new(group.delimiter(), lower(group.stream()));
                inner.set_span(group.span());
                lowered.extend([TokenTree::from(inner)]);
            }
            other => lowered.extend([other]),
        }
        after_name = is_name;
    }
    lowered
}

/// Names a template by its statics. FNV-1a, spelled out here because the same
/// template must get the same fingerprint from every compiler run and every Rust
/// release, which std's hashers do not promise.
fn fingerprint(statics: &[String]) -> u64 {
    // 0xff is in no UTF-8 string, so it keeps ["ab", ""] apart from ["a", "b"].
    let bytes = statics.iter().flat_map(|part| part.bytes().chain([0xff]));
    bytes.fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    })
}
