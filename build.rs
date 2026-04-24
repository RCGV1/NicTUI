use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=macos/Info.plist");

    let target = env::var("TARGET").unwrap_or_default();
    if !target.contains("apple-darwin") {
        return;
    }

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".into()));
    let template_path = manifest_dir.join("macos").join("Info.plist");
    let plist_template = fs::read_to_string(&template_path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", template_path.display()));
    let plist = plist_template.replace(
        "@VERSION@",
        &env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".into()),
    );

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap_or_else(|_| ".".into()));
    let plist_path = out_dir.join("nictui-Info.plist");
    fs::write(&plist_path, plist)
        .unwrap_or_else(|error| panic!("failed to write {}: {error}", plist_path.display()));

    println!(
        "cargo:rustc-link-arg-bin=nictui=-Wl,-sectcreate,__TEXT,__info_plist,{}",
        plist_path.display()
    );
}
