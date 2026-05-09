fn main() {
    // Tell cargo to recompile if the build-time persist toggle flips,
    // so observe-only and normal images produce distinct binaries.
    println!("cargo:rerun-if-env-changed=ECPHORY_PERSIST");
}
