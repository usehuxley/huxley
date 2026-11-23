// `sqlx::migrate!` embeds the migrations at compile time. Rebuild when they
// change, or a new migration is silently missing from the binary.
fn main() {
    println!("cargo:rerun-if-changed=../../migrations");
}
