use std::path::PathBuf;

fn main() {
    let linker_script = PathBuf::from(std::env::current_dir().unwrap()).join("linker.ld");

    println!("cargo:rustc-link-arg=-T{}", linker_script.display());
    println!("cargo:rustc-link-arg=-static");
}
