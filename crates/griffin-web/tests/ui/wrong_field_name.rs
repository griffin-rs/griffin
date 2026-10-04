use griffin_web::html;
use griffin_web::template::Rendered;

struct Counter {
    count: u32,
}

impl Counter {
    fn render(&self) -> Rendered {
        html! {
            <p>Count: {@cuont}</p>
        }
    }
}

fn main() {}
