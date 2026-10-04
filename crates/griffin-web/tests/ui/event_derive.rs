use griffin_web::live::Event;

#[derive(Event)]
struct Increment {
    by: i32,
}

#[derive(Event)]
enum Unnamed {
    Add(i32),
    Reset,
    Move(i32, i32),
}

struct NoParser;

#[derive(Event)]
enum Unparsed {
    Pick { what: NoParser },
}

fn main() {}
