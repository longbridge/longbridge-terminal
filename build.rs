fn main() {
    // Windows default stack is 1MB, too small for debug async futures.
    #[cfg(windows)]
    println!("cargo:rustc-link-arg=/STACK:8388608");
}
