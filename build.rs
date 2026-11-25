use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-env-changed=CPR_AUTHOR_MANIFEST");
    let manifest = env::var_os("CPR_AUTHOR_MANIFEST")
        .map_or_else(|| PathBuf::from("plugin.json"), PathBuf::from);
    println!("cargo:rerun-if-changed={}", manifest.display());
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo OUT_DIR"));
    fs::copy(manifest, out.join("plugin.json")).expect("copy author manifest");
}
