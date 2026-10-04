use griffin_web::template::{Rendered, SlotEntries};
use griffin_web::{component, html};

#[component]
fn List(
    #[slot]
    #[default]
    inner_block: SlotEntries<'_, u32>,
    #[slot]
    #[default]
    item: SlotEntries<'_>,
) -> Rendered {
    html! { <ul>{inner_block.render(1)}<li :for={item in &item}>{item}</li></ul> }
}

fn main() {
    let (items, rest) = ([1, 2], [("id", "a")]);
    html! {
        <div>
            <:item>outside a Component</:item>
            <List>
                <p><:item>not directly inside</:item></p>
                <:item :for={item in items} :if={items.len() > 1}>many</:item>
                <:item data-id="1" {rest}>one</:item>
                <:item-big>two</:item-big>
                <:item :let>three</:item>
            </List>
            <List :let={count}><:item>four</:item></List>
        </div>
    };
}
