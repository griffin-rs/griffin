//! The starter Components, rendered on their own.

extern crate griffin as griffin_web;

use griffin::html;

use __app___web::components::{Button, Flash};

#[test]
fn a_button_is_a_submit_button_unless_told_otherwise_and_its_text_is_the_slot() {
    assert_eq!(
        html! { <Button>Save</Button> }.to_html(),
        "<button type=\"submit\">Save</button>"
    );
    assert_eq!(
        html! { <Button kind="button">Cancel</Button> }.to_html(),
        "<button type=\"button\">Cancel</button>"
    );
}

#[test]
fn a_flash_shows_its_message_escaped_and_nothing_without_one() {
    let shown = html! { <Flash message={Some("Saved <b>")} /> }.to_html();
    assert!(shown.contains("class=\"flash\""), "{shown}");
    assert!(shown.contains("Saved &lt;b&gt;"), "{shown}");
    assert!(
        !html! { <Flash message={None} /> }
            .to_html()
            .contains("flash")
    );
}
