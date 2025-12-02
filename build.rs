use std::env;
use std::process::Command;

fn main() {
    let out = std::env::var("TMPDIR").unwrap();
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();

    assert!(Command::new("make")
	    .arg("-f")
	    .arg("_Makefile")
	    .arg("clean")
	    .env("TMPDIR", &out)
	    .env("UACPI_SOURCE", format!("{}/uacpi",  manifest_dir))
	    .status()
	    .unwrap()
	    .success());
    assert!(Command::new("make")
	    .arg("-f")
	    .arg("_Makefile")
//	    .arg("-d")
	    .env("TMPDIR", &out)
	    .env("UACPI_SOURCE", format!("{}/uacpi",  manifest_dir))
	    .status()
	    .unwrap()
	    .success());
    let linker_script = std::env::current_dir().unwrap().join("linker.ld");

    println!("cargo:rustc-link-lib=static=uacpi");
    println!("cargo:rustc-link-search=native={}", env::var("TMPDIR").unwrap());
    println!("cargo:rustc-link-arg=-T{}", linker_script.display());
    println!("cargo:rustc-link-arg=-static");
    println!("cargo:rustc-link-arg=-nostdlib");
}
