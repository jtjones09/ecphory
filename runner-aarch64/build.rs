use std::path::PathBuf;
use std::process::Command;

fn main() {
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    let efi = PathBuf::from(
        std::env::var_os("CARGO_BIN_FILE_KERNEL_AARCH64_kernel-aarch64").unwrap(),
    );
    let img_path = out_dir.join("ecphory-aarch64.img");

    // Use the shell script in scripts/ to wrap the .efi in a GPT+ESP image.
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let script = manifest.join("..").join("scripts").join("mkimg-aarch64.sh");
    let status = Command::new("bash")
        .arg(&script)
        .arg(&efi)
        .arg(&img_path)
        .status()
        .expect("failed to invoke mkimg-aarch64.sh");
    if !status.success() {
        panic!("mkimg-aarch64.sh failed: {:?}", status);
    }

    println!("cargo:rustc-env=AARCH64_IMG_PATH={}", img_path.display());
    println!("cargo:rustc-env=AARCH64_EFI_PATH={}", efi.display());
}
