use griffin_web::html;

fn main() {
    let user: Option<&str> = None;
    html! {
        <p :if={user}>Signed in</p>
    };
}
