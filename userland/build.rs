#![forbid(unsafe_code)]

fn main() {
    println!("cargo:rerun-if-changed=../linker_user_x86_64.ld");
    println!("cargo:rustc-link-arg-bin=npu_infer=-Tlinker_user_x86_64.ld");
    println!("cargo:rustc-link-arg-bin=npu_infer=--gc-sections");
    println!("cargo:rustc-link-arg-bin=npu_infer=--strip-all");
}
