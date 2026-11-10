use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=c_src/npu_hw_driver.c");
    println!("cargo:rerun-if-changed=c_src/panic_decoder.c");
    println!("cargo:rerun-if-changed=src/arch/x86_64/switch.s");
    println!("cargo:rerun-if-changed=src/arch/x86_64/trap.s");

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let c_lib_dir = manifest_dir.join("c_src");

    if cfg!(target_os = "windows") {
        // When invoked from Windows host Cargo, compile C11 kernel modules via WSL gcc + ar
        let status = Command::new("wsl")
            .args([
                "-e",
                "bash",
                "-c",
                "cd /mnt/c/FipsSOs/ostd && \
                 gcc -std=c11 -ffreestanding -fno-stack-protector -mno-red-zone -msse2 -mfpmath=sse -mcmodel=kernel -fno-pic -fno-pie -ffunction-sections -fdata-sections -O3 -Wall -Wextra -Werror -c c_src/npu_hw_driver.c -o c_src/npu_hw_driver.o && \
                 gcc -std=c11 -ffreestanding -fno-stack-protector -mno-red-zone -msse2 -mfpmath=sse -mcmodel=kernel -fno-pic -fno-pie -ffunction-sections -fdata-sections -O3 -Wall -Wextra -Werror -c c_src/panic_decoder.c -o c_src/panic_decoder.o && \
                 ar rcs c_src/libnpu_hw_driver.a c_src/npu_hw_driver.o c_src/panic_decoder.o",
            ])
            .status()
            .expect("Fallo al invocar WSL gcc para compilar C kernel modules");
        assert!(status.success(), "Error compilando ostd/c_src C modules con gcc");
    } else {
        let npu_obj = c_lib_dir.join("npu_hw_driver.o");
        let panic_obj = c_lib_dir.join("panic_decoder.o");
        let lib_path = c_lib_dir.join("libnpu_hw_driver.a");

        let c_flags = [
            "-std=c11",
            "-ffreestanding",
            "-fno-stack-protector",
            "-mno-red-zone",
            "-msse2",
            "-mfpmath=sse",
            "-mcmodel=kernel",
            "-fno-pic",
            "-fno-pie",
            "-ffunction-sections",
            "-fdata-sections",
            "-O3",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-c",
        ];

        let s1 = Command::new("gcc")
            .args(&c_flags)
            .arg(c_lib_dir.join("npu_hw_driver.c"))
            .arg("-o")
            .arg(&npu_obj)
            .status()
            .expect("Fallo al ejecutar gcc para compilar npu_hw_driver.c");
        assert!(s1.success(), "gcc fallo al compilar npu_hw_driver.c");

        let s2 = Command::new("gcc")
            .args(&c_flags)
            .arg(c_lib_dir.join("panic_decoder.c"))
            .arg("-o")
            .arg(&panic_obj)
            .status()
            .expect("Fallo al ejecutar gcc para compilar panic_decoder.c");
        assert!(s2.success(), "gcc fallo al compilar panic_decoder.c");

        let ar_status = Command::new("ar")
            .arg("rcs")
            .arg(&lib_path)
            .arg(&npu_obj)
            .arg(&panic_obj)
            .status()
            .expect("Fallo al empaquetar libnpu_hw_driver.a con ar");
        assert!(ar_status.success(), "ar fallo al crear libnpu_hw_driver.a");
    }

    println!("cargo:rustc-link-search=native={}", c_lib_dir.display());
    println!("cargo:rustc-link-lib=static=npu_hw_driver");
}
