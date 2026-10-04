use griffin_web::template::Rendered;
use griffin_web::{component, html, html_file};

struct Opaque;

struct Counter {
    title: String,
    count: u32,
}

#[component]
fn Badge(label: &str) -> Rendered {
    html! { <b>{label}</b> }
}

impl Counter {
    fn render(&self) -> Rendered {
        html_file!("type_errors.html.griffin")
    }
}

fn main() {}
