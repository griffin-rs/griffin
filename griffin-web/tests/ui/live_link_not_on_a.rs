use griffin_web::html;

fn main() {
    let path = "/users/7".to_owned();
    html! {
        <button navigate={path}>Go</button>
    };
    html! {
        <div patch="/users">Go</div>
    };
}
