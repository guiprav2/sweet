use std::env;
use std::path::PathBuf;

fn main() {
    napi_build::setup();

    if env::var_os("CARGO_FEATURE_ANDROID_APK").is_some() {
        let root = PathBuf::from(env::var_os("NODE_MOBILE_ROOT").expect(
            "NODE_MOBILE_ROOT must point to the extracted nodejs-mobile Android release",
        ));
        let abi = env::var("ANDROID_ABI").unwrap_or_else(|_| "arm64-v8a".into());
        cc::Build::new()
            .cpp(true)
            .file("src/node_host.cc")
            .include(root.join("include/node"))
            .flag_if_supported("-std=c++17")
            .compile("sweet_node_host");
        println!("cargo:rustc-link-search=native={}", root.join("bin").join(&abi).display());
        println!("cargo:rustc-link-lib=dylib=node");
        println!("cargo:rerun-if-env-changed=NODE_MOBILE_ROOT");
        println!("cargo:rerun-if-env-changed=ANDROID_ABI");
    }
}
