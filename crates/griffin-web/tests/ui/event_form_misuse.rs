use griffin_web::live::{Event, FormParams};

// A typed field beside the form can never be decoded.
#[derive(Event)]
enum Mixed {
    Save {
        id: u32,
        #[form]
        params: FormParams,
    },
}

// Nor can two forms.
#[derive(Event)]
enum Twice {
    Save {
        #[form]
        first: FormParams,
        #[form]
        second: FormParams,
    },
}

fn main() {}
