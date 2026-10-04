//! The button, flash, input, label and error Components: ordinary Components with slots, fed by a
//! `Form` from `griffin-domain`. They are yours: nothing here is in the framework, so
//! an application restyles or replaces them.

use griffin::template::{Rendered, SlotEntries};
use griffin::{component, html};
use griffin_domain::Form;

/// A button. Its text is the default slot. `kind` is the HTML `type`: `submit` by default.
#[component]
pub fn Button(#[default("submit")] kind: &str, #[slot] inner_block: SlotEntries<'_>) -> Rendered {
    html! { <button type={kind}>{inner_block}</button> }
}

/// The one-time notice left by the request before, or nothing when there is none. The
/// text is escaped by the template.
#[component]
pub fn Flash(message: Option<&str>) -> Rendered {
    html! { <p :if={message.is_some()} class="flash" role="status">{message.unwrap_or_default()}</p> }
}

/// The label of an input. Its text is the default slot.
#[component]
pub fn Label(target: &str, #[slot] inner_block: SlotEntries<'_>) -> Rendered {
    html! { <label for={target}>{inner_block}</label> }
}

/// An error message under an input. Its text is the default slot.
#[component]
pub fn FieldError(#[slot] inner_block: SlotEntries<'_>) -> Rendered {
    html! { <p class="error" role="alert">{inner_block}</p> }
}

/// The errors of the form as a whole, at the top of the form: "the form is too large".
/// The text is escaped by the template. Nothing is rendered when there are none.
#[component]
pub fn FormErrors(form: &Form) -> Rendered {
    html! {
        <div class="form-errors" role="alert" :if={!form.form_errors.is_empty()}>
            <p :for={message in &form.form_errors}>{message}</p>
        </div>
    }
}

/// A label, an input and its errors for the field `field` of `form`. The label's text
/// is the slot. It writes the field's name as the browser sends it back, `user[email]`
/// for the form `user`, and what the user typed as the value, whether or not it is valid.
///
/// An error is shown only for an input the user has used: the client marks the others
/// `_unused_<name>`, and the Changeset's `touched` is that. So a form does not scold a
/// field the user has not reached, and shows every error once it is submitted.
///
/// The value and the messages are what the user typed: the template escapes them.
///
/// # Panics
///
/// If the Changeset the form came from declares no field of that name: a mistake in
/// the template, found by the first render.
#[component]
pub fn Input(
    form: &Form,
    field: &str,
    #[default("text")] kind: &str,
    #[default("off")] autocomplete: &str,
    #[slot] inner_block: SlotEntries<'_>,
) -> Rendered {
    let data = form
        .field(field)
        .expect("the form's Changeset declares this field");
    let name = input_name(form, field);
    let id = input_id(&name);
    let errors = if data.touched { &data.errors[..] } else { &[] };
    let error_id = format!("{id}_error");
    let invalid = !errors.is_empty();
    html! {
        <div class="field">
            <Label target={id.as_str()}>{inner_block}</Label>
            <input id={id.as_str()} name={name.as_str()} type={kind} value={data.value.as_str()}
                autocomplete={autocomplete} aria-invalid={invalid.then_some("true")}
                aria-describedby={invalid.then_some(error_id.as_str())}>
            <div id={error_id.as_str()} :if={invalid}>
                <FieldError :for={message in errors}>{message}</FieldError>
            </div>
        </div>
    }
}

/// A group of checkboxes for a list field (`user[topics][]`): one for each of `choices`,
/// checked if the user ticked it. The label of the group is the slot.
///
/// It writes two hidden inputs before the boxes: an empty item under the list's name,
/// and a `_sent_<name>` marker (spelled like the client's `_unused_<name>`). A browser
/// sends no unchecked box, so a group with nothing ticked would not be sent at all.
/// The item is what makes the client treat the group as an input that can be unused
/// (it writes `_unused_<name>` only for names with a visible input, and the boxes are
/// visible); the marker tells the Changeset that item is the group's and no data, so
/// it drops it. This is Griffin's own convention: Phoenix has no such sentinel. Its
/// `core_components` checkbox writes a scalar hidden `value="false"`, LiveView's
/// multi-checkbox fixture has none, and the only hidden empty list item is the
/// dedicated `emails_drop[]` name of `inputs_for`.
#[component]
pub fn Checkboxes(
    form: &Form,
    field: &str,
    choices: &[&str],
    #[slot] inner_block: SlotEntries<'_>,
) -> Rendered {
    let data = form
        .field(field)
        .expect("the form's Changeset declares this field");
    let base = input_name(form, field);
    let name = format!("{base}[]");
    let marker = match base.rsplit_once('[') {
        Some((head, last)) => format!("{head}[_sent_{last}"),
        None => format!("_sent_{base}"),
    };
    let errors = if data.touched { &data.errors[..] } else { &[] };
    html! {
        <fieldset class="field">
            <legend>{inner_block}</legend>
            <input type="hidden" name={name.as_str()} value="">
            <input type="hidden" name={marker.as_str()} value="">
            <label :for={choice in choices} class="choice">
                <input type="checkbox" name={name.as_str()} value={*choice}
                    checked={data.values.iter().any(|value| value == choice)}>
                {*choice}
            </label>
            <FieldError :for={message in errors}>{message}</FieldError>
        </fieldset>
    }
}

/// `user[email]` for the field `email` of the form `user`, and `email` for a form with
/// no name.
fn input_name(form: &Form, field: &str) -> String {
    if form.name.is_empty() {
        field.to_owned()
    } else {
        format!("{}[{field}]", form.name)
    }
}

/// An id for an input, made of its name: the label points at it, and the client
/// recovers a form by the ids of its inputs.
fn input_id(name: &str) -> String {
    name.replace(['[', ']'], "_")
        .trim_end_matches('_')
        .to_owned()
}
