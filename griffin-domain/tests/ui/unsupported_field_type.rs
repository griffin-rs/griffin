use griffin_domain::Changeset;

struct Coordinates;

#[derive(Changeset)]
struct Venue {
    name: String,
    location: Coordinates,
}

fn main() {}
