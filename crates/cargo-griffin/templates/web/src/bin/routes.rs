//! `cargo griffin routes` runs this: it prints the route table.

fn main() {
    print!(
        "{}",
        griffin::router::format_routes(__app___web::routes::Routes::LIST)
    );
}
