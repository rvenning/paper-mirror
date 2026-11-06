//! Regenerates the committed test corpus in tests/fixtures/.
fn main() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    paper_mirror::fixtures::write_all(&dir).expect("write fixtures");
    println!("fixtures written to {}", dir.display());
}
