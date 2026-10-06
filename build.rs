fn main() {
    // Clap's derived command builders have large debug stack frames (the root
    // and nested key builder together exceed 750 KiB). Discovery builds this
    // grammar inside the async dispatcher, exceeding MSVC's 1 MiB default.
    // Reserve the usual Linux main-stack size for the CLI on Windows too;
    // pages are committed on demand. Keep this in the crate so source builds
    // and packaged executables use the same setting.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let flag = if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
            "/STACK:8388608"
        } else {
            "-Wl,--stack,8388608"
        };
        println!("cargo:rustc-link-arg-bin=monica-pass={flag}");
    }
}
