use griffin_web::html;

fn main() {
    let items = [1, 2, 3];
    html! {
        <li :for>bare</li>
        <li :for="item in items">quoted</li>
        <li :for={items}>no pattern</li>
        <li :for={item <- items}>an arrow</li>
        <li :for={in items}>no pattern before in</li>
        <li :for={item in}>nothing after in</li>
        <li :for={_ in &items} :for={_ in &items}>twice</li>
        <li :for={_ in &items} :key>bare key</li>
        <li :key={1}>key alone</li>
    };
}
