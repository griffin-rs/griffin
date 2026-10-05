use griffin_web::html;

fn main() {
    let show = true;
    html! {
        <p :if>bare</p>
        <p :if="show">quoted</p>
        <p :if={show} :if={!show}>twice</p>
    };
}
