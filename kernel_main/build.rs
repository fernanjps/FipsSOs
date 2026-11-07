#![forbid(unsafe_code)]

fn main() {
    println!("cargo:rerun-if-changed=../linker_x86_64.ld");
    println!("cargo:rustc-link-arg-bin=fipsos-kernel=-Tlinker_x86_64.ld");
}
