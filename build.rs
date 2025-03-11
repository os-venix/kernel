use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    assert!(Command::new("make")
	    .arg("-f")
	    .arg("_Makefile")
	    .arg("clean")
	    .status()
	    .unwrap()
	    .success());
    assert!(Command::new("make")
	    .arg("-f")
	    .arg("_Makefile")
	    .status()
	    .unwrap()
	    .success());
    let linker_script = PathBuf::from(std::env::current_dir().unwrap()).join("linker.ld");

    println!("cargo:rustc-link-lib=static=uacpi");
    println!("cargo:rustc-link-search=native={}", env::var("OUT_DIR").unwrap());
    println!("cargo:rustc-link-arg=-T{}", linker_script.display());
    println!("cargo:rustc-link-arg=-static");
}
