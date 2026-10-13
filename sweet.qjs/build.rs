fn main() {
    let target = std::env::var("TARGET").unwrap_or_default();
    let windows = target.contains("windows");
    let include = std::env::var("QUICKJS_INCLUDE")
        .unwrap_or_else(|_| "/home/camila/prog/quickjs".to_string());
    let mut build = cc::Build::new();
    build.file("module.c").include(&include);
    if !windows {
        build.flag("-fPIC");
    }
    build.compile("sweet_qjs_module");
    println!("cargo:rerun-if-changed=module.c");
    println!("cargo:rerun-if-env-changed=QUICKJS_INCLUDE");
    println!("cargo:rerun-if-env-changed=QJS_IMPLIB");
    if windows {
        let implib = std::env::var("QJS_IMPLIB").expect("QJS_IMPLIB must point at the qjs import library");
        println!("cargo:rustc-link-arg-cdylib={implib}");
    } else {
        println!("cargo:rustc-link-arg-cdylib=-Wl,--unresolved-symbols=ignore-all");
    }
}
