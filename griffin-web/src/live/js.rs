//! Client commands: what a binding does in the browser, built in Rust.

use super::Event;
use crate::template::AttributeValue;
use serde_json::{Value, json};

/// Client commands: a list of operations the Phoenix client carries out in the browser
/// when the binding they are attached to fires, with no round trip to the server for
/// any but [`push`](Js::push). It is Phoenix's `JS` (the wire protocol is Phoenix's, unchanged): bound to an element, it
/// is written into the attribute as the JSON the unmodified client executes.
///
/// Each method appends one operation and gives the commands back, so they chain, and
/// run in the order written:
///
/// ```
/// use griffin_web::html;
/// use griffin_web::live::{Event, Js};
///
/// #[derive(Event)]
/// enum MenuEvent {
///     Opened { by: String },
/// }
///
/// let open = Js::new()
///     .toggle("#menu")
///     .push(&MenuEvent::Opened { by: "button".into() });
/// let button = html! { <button phx-click={open}>Menu</button> };
///
/// assert_eq!(
///     button.to_html(),
///     "<button phx-click=\"[[&quot;toggle&quot;,{&quot;to&quot;:&quot;#menu&quot;}],\
///      [&quot;push&quot;,{&quot;event&quot;:&quot;opened&quot;,\
///      &quot;value&quot;:{&quot;by&quot;:&quot;button&quot;}}]]\">Menu</button>"
/// );
/// ```
///
/// # Targets
///
/// `to` is a CSS selector for the elements an operation works on, as in `"#menu"`, or
/// `None` for the element the binding is on.
///
/// # Trust
///
/// Commands reach the page as an attribute's value, which is escaped like any other,
/// so text from a user in a class name, an attribute's value or an Event's field
/// cannot leave the attribute. It can still be a selector or a name the developer did
/// not mean: do not take selectors, attribute names or event names from user input.
// An operation has its target and nothing else. Phoenix's options (`time`,
// `display`, `blocking`, a transition on show, hide and toggle or with start and end
// classes, `detail` and `bubbles` on dispatch, `target` and `loading` on push) are
// more keys in the same maps: add each when something needs it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Js(Vec<Value>);

impl Js {
    /// No commands yet.
    pub fn new() -> Js {
        Js::default()
    }

    /// Shows the elements.
    pub fn show<'a>(self, to: impl Into<Option<&'a str>>) -> Js {
        self.operation("show", to.into(), json!({}))
    }

    /// Hides the elements.
    pub fn hide<'a>(self, to: impl Into<Option<&'a str>>) -> Js {
        self.operation("hide", to.into(), json!({}))
    }

    /// Shows the elements if they are hidden and hides them if they are shown.
    pub fn toggle<'a>(self, to: impl Into<Option<&'a str>>) -> Js {
        self.operation("toggle", to.into(), json!({}))
    }

    /// Adds the classes in `names`, separated by spaces, to the elements.
    pub fn add_class<'a>(self, names: &str, to: impl Into<Option<&'a str>>) -> Js {
        self.operation("add_class", to.into(), json!({"names": classes(names)}))
    }

    /// Removes the classes in `names`, separated by spaces, from the elements.
    pub fn remove_class<'a>(self, names: &str, to: impl Into<Option<&'a str>>) -> Js {
        let names = json!({"names": classes(names)});
        self.operation("remove_class", to.into(), names)
    }

    /// Sets the attribute `name` of the elements to `value`.
    pub fn set_attribute<'a>(self, name: &str, value: &str, to: impl Into<Option<&'a str>>) -> Js {
        self.operation("set_attr", to.into(), json!({"attr": [name, value]}))
    }

    /// Removes the attribute `name` from the elements.
    pub fn remove_attribute<'a>(self, name: &str, to: impl Into<Option<&'a str>>) -> Js {
        self.operation("remove_attr", to.into(), json!({"attr": name}))
    }

    /// Gives the elements the classes in `names`, separated by spaces, for the time of
    /// a transition (the client's default, 200 milliseconds), and removes them again.
    pub fn transition<'a>(self, names: &str, to: impl Into<Option<&'a str>>) -> Js {
        // The classes during the transition, at its start, and at its end.
        let transition = json!({"transition": [classes(names), [], []]});
        self.operation("transition", to.into(), transition)
    }

    /// Dispatches the DOM event `event` on the elements, for JavaScript to listen to.
    pub fn dispatch<'a>(self, event: &str, to: impl Into<Option<&'a str>>) -> Js {
        self.operation("dispatch", to.into(), json!({"event": event}))
    }

    /// Moves the focus to the element.
    pub fn focus<'a>(self, to: impl Into<Option<&'a str>>) -> Js {
        self.operation("focus", to.into(), json!({}))
    }

    /// Sends `event` to the LiveView, as a binding of the Event itself would: this is
    /// how one binding does something in the browser and tells the server too.
    ///
    /// The Event's fields go with the operation, not into `phx-value-*` attributes, so
    /// two Events pushed from one element do not share them.
    pub fn push(self, event: &impl Event) -> Js {
        let (name, fields) = event.encode();
        let mut push = json!({"event": name});
        if !fields.is_empty() {
            let fields = fields.into_iter();
            let fields = fields.map(|(field, value)| (field.to_owned(), Value::from(value)));
            push["value"] = Value::Object(fields.collect());
        }
        self.operation("push", None, push)
    }

    /// Appends the operation `kind`: its arguments, and its target unless it is the
    /// element itself. Phoenix leaves out what is not given, and so does this.
    fn operation(mut self, kind: &str, to: Option<&str>, mut arguments: Value) -> Js {
        if let Some(to) = to {
            arguments["to"] = to.into();
        }
        self.0.push(json!([kind, arguments]));
        self
    }
}

fn classes(names: &str) -> Vec<&str> {
    names.split_whitespace().collect()
}

/// What binds commands to an element: `phx-click={commands}` in a template. The value
/// is the operations as JSON, which is escaped when written, as every attribute is.
impl From<&Js> for AttributeValue {
    fn from(commands: &Js) -> AttributeValue {
        AttributeValue::Value(Value::from(commands.0.as_slice()).to_string())
    }
}
