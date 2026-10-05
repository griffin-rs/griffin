use griffin_web::component;

#[component]
fn Generic<T: ToString>(label: T) -> griffin_web::template::Rendered {
    griffin_web::html! { <span>{label.to_string()}</span> }
}

#[component]
fn Pattern((label, count): (&str, u32)) -> griffin_web::template::Rendered {
    griffin_web::html! { <span>{label} {count}</span> }
}

#[component(cached)]
fn Arguments(label: &str) -> griffin_web::template::Rendered {
    griffin_web::html! { <span>{label}</span> }
}

#[component]
fn lower_case(label: &str) -> griffin_web::template::Rendered {
    griffin_web::html! { <span>{label}</span> }
}

#[component]
fn Globals(
    #[global] one: griffin_web::template::Attributes,
    #[global] two: griffin_web::template::Attributes,
) -> griffin_web::template::Rendered {
    griffin_web::html! { <span {one} {two}></span> }
}

struct Page;

impl Page {
    #[component]
    fn Method(&self, label: &str) -> griffin_web::template::Rendered {
        griffin_web::html! { <span>{label}</span> }
    }
}

fn main() {}
