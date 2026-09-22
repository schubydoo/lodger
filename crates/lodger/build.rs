// rust-embed reads web/build at compile time in release builds. Tell cargo to
// rebuild when that folder changes, so a fresh `pnpm run build` always lands in
// the next binary.
fn main() {
    println!("cargo:rerun-if-changed=../../web/build");
}
