fn main() {
    // Resolve imported DLLs from System32 only, not from whatever folder the exe sits in.
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        println!("cargo:rustc-link-arg-bins=/DEPENDENTLOADFLAG:0x800");
    }
    println!("cargo:rerun-if-changed=build.rs");
}
