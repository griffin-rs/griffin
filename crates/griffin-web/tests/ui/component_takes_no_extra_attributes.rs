use griffin_web::template::Rendered;
use griffin_web::{component, html};

#[component]
fn Badge(label: &str) -> Rendered {
    html! { <span>{label}</span> }
}

fn main() {
    html! {
        <p><Badge label="new" data-id="7" /><Badge label="new" class="big" /></p>
    };
}
