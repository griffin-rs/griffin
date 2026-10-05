use griffin_web::template::{Rendered, SlotEntries};
use griffin_web::{component, html};

#[derive(Default)]
struct Column<'a> {
    label: &'a str,
}

#[component]
fn Table(rows: &[u32], #[slot] col: SlotEntries<'_, &u32, Column<'_>>) -> Rendered {
    html! { <table><tr :for={row in rows}><td :for={col in &col}>{col.label} {col.render(row)}</td></tr></table> }
}

fn main() {
    let rows = [1, 2];
    html! {
        <div>
            <Table rows={&rows}><:col label="N">n</:col><:column>no such slot</:column></Table>
            <Table rows={&rows}><:col label="N">n</:col>no default slot</Table>
            <Table rows={&rows}><:col label={7} title="N">n</:col></Table>
            <Table rows={&rows}><:col :let={(a, b)}>{a}{b}</:col></Table>
        </div>
    };
}
