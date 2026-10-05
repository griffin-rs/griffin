use griffin_web::template::Rendered;
use griffin_web::{component, html};

#[component]
fn Badge(label: &str) -> Rendered {
    html! { <span>{label}</span> }
}

fn main() {
    let rest = [("id", "inbox")];
    html! {
        <p>
            <Badge label="new" {rest} />
            <Badge-big label="new" />
        </p>
    };
}
