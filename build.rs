use std::process::Command;
use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    let out = std::env::var("OUT_DIR").unwrap();
    let manifest_dir = std::env::current_dir().unwrap();

    build_uacpi(&out, manifest_dir.to_str().unwrap());
    generate_bindings(&out, &manifest_dir);
    let linker_script = manifest_dir.join("linker.ld");

    println!("cargo:rustc-link-lib=static=uacpi");
    println!("cargo:rustc-link-search=native={}", out);
    println!("cargo:rustc-link-arg=-T{}", linker_script.display());
    println!("cargo:rustc-link-arg=-static");
    println!("cargo:rustc-link-arg=-nostdlib");
}

fn build_uacpi(out: &str, manifest_dir: &str) {
    assert!(Command::new("make")
	    .arg("-f")
	    .arg("_Makefile")
	    .arg("clean")
	    .env("TMPDIR", out)
	    .env("UACPI_SOURCE", format!("{}/uacpi",  manifest_dir))
	    .status()
	    .unwrap()
	    .success());
    assert!(Command::new("make")
	    .arg("-f")
	    .arg("_Makefile")
	    .env("TMPDIR", out)
	    .env("UACPI_SOURCE", format!("{}/uacpi",  manifest_dir))
	    .status()
	    .unwrap()
	    .success());
}

fn generate_bindings(out: &str, manifest_dir: &Path) {
    let include_dir = manifest_dir.join("uacpi/include");
    let header = manifest_dir.join("uacpi/wrapper.h");
    let out_path = PathBuf::from(out);

    let bindings = bindgen::Builder::default()
	.use_core()
	.header(header.to_str().unwrap())
	.clang_arg(format!("-I{}", include_dir.display()))
	.allowlist_type("uacpi_.*")
	.allowlist_type("acpi_.*")
	.allowlist_function("uacpi_.*")
	.allowlist_var("UACPI_.*")
	.blocklist_function("uacpi_kernel_.*")
	.derive_default(true)
	.layout_tests(false)
	.rustified_enum("uacpi_.*")
	.rustified_enum("acpi_.*")
	.generate()
	.expect("Unable to generate bindings for uACPI");

    let mut output = bindings
	.to_string()
	.replace(r#"extern "C" {"#, r#"unsafe extern "C" {"#);

    output.push('\n');

    fs::write(out_path.join("uacpi_bindings.rs"), output).expect("Couldn't write bindings for uACPI");
}
