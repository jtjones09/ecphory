use std::path::PathBuf;
use std::process::Command;

fn main() {
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    let efi = PathBuf::from(
        std::env::var_os("CARGO_BIN_FILE_KERNEL_X86_64_kernel-x86_64").unwrap(),
    );
    let img_path = out_dir.join("ecphory-x86_64.img");

    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let script = manifest.join("..").join("scripts").join("mkimg-x86_64.sh");
    let status = Command::new("bash")
        .arg(&script)
        .arg(&efi)
        .arg(&img_path)
        .status()
        .expect("failed to invoke mkimg-x86_64.sh");
    if !status.success() {
        panic!("mkimg-x86_64.sh failed: {:?}", status);
    }

    println!("cargo:rustc-env=X86_64_IMG_PATH={}", img_path.display());
    println!("cargo:rustc-env=X86_64_EFI_PATH={}", efi.display());
}
