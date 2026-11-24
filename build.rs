fn main() {
    println!("cargo:rustc-check-cfg=cfg(tessera_wpe)");
    println!("cargo:rerun-if-changed=src/wpe_platform.c");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("linux") {
        return;
    }
    if let Ok(platform) = pkg_config::Config::new()
        .atleast_version("2.54")
        .cargo_metadata(false)
        .probe("wpe-platform-2.0")
    {
        cc::Build::new()
            .file("src/wpe_platform.c")
            .includes(platform.include_paths)
            .compile("tessera_wpe_platform");
        println!("cargo:rustc-cfg=tessera_wpe");
    }
}
