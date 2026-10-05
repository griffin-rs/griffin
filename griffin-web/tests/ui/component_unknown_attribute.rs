use griffin_web::template::Rendered;
use griffin_web::{component, html};

#[component]
fn Badge(label: &str, #[default(0)] count: u32) -> Rendered {
    html! { <span>{label} {count}</span> }
}

fn main() {
    html! {
        <p><Badge label="new" colour="red" /></p>
    };
}
