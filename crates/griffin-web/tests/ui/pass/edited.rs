//! Run by `editing_a_template_file_recompiles_the_code_that_uses_it`, which rewrites
//! the template between two runs and touches nothing else.

use griffin_web::html_file;

fn main() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/templates/edited.html.griffin");
    let on_disk = std::fs::read_to_string(path).unwrap();

    assert_eq!(html_file!("edited.html.griffin").to_html(), on_disk);
}
