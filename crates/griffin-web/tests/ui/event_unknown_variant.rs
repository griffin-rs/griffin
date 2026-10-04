use griffin_web::html;
use griffin_web::live::Event;
use griffin_web::template::Rendered;

#[derive(Event)]
enum CounterEvent {
    Increment,
    Add { by: i32 },
}

fn render() -> Rendered {
    html! {
        <p>
            <button phx-click={CounterEvent::Incremnt}>One more</button>
            <button phx-click={CounterEvent::Add { amount: 2 }}>Two more</button>
            <button phx-click={CounterEvent::Add { by: "two" }}>Two more</button>
        </p>
    }
}

fn main() {}
