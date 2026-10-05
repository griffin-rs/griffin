use griffin_web::template::{Rendered, SlotEntries};
use griffin_web::{component, html};

#[component]
fn Card(
    #[slot] inner_block: SlotEntries<'_>,
    #[slot] footer: SlotEntries<'_>,
    #[slot]
    #[default]
    header: SlotEntries<'_>,
) -> Rendered {
    html! { <section>{header}{inner_block}{footer}</section> }
}

fn main() {
    html! {
        <div>
            <Card><:footer>Bye</:footer></Card>
            <Card>Hello</Card>
            <Card><:footer>Bye</:footer>Hello</Card>
        </div>
    };
}
