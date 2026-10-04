use griffin_web::html;

fn main() {
    html! {
        <a navigate="javascript:alert(1)">x</a>
        <a patch="//evil.example">y</a>
        <a navigate="/fine">z</a>
    };
}
