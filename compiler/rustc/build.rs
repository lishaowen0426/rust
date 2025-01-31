use std::env;
use std::fs::{metadata, read_dir};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

const MIMALLOC_SRC: &str = "/home/sw/rust-isolation/mimalloc/src";
const MIMALLOC_OUT: &str = "/home/sw/rust-isolation/mimalloc/out/debug";
fn main() {
    let target_os = env::var("CARGO_CFG_TARGET_OS");
    let target_env = env::var("CARGO_CFG_TARGET_ENV");
    if Ok("windows") == target_os.as_deref() && Ok("msvc") == target_env.as_deref() {
        set_windows_exe_options();
    } else {
        // Avoid rerunning the build script every time.
        println!("cargo:rerun-if-changed=build.rs");
    }

    link_mimalloc();
}

// Add a manifest file to rustc.exe.
fn set_windows_exe_options() {
    static WINDOWS_MANIFEST_FILE: &str = "Windows Manifest.xml";

    let mut manifest = env::current_dir().unwrap();
    manifest.push(WINDOWS_MANIFEST_FILE);

    println!("cargo:rerun-if-changed={WINDOWS_MANIFEST_FILE}");
    // Embed the Windows application manifest file.
    println!("cargo:rustc-link-arg-bin=rustc-main=/MANIFEST:EMBED");
    println!("cargo:rustc-link-arg-bin=rustc-main=/MANIFESTINPUT:{}", manifest.to_str().unwrap());
    // Turn linker warnings into errors.
    println!("cargo:rustc-link-arg-bin=rustc-main=/WX");
}

fn _list_files(vec: &mut Vec<PathBuf>, path: &Path) {
    if metadata(&path).unwrap().is_dir() {
        let paths = read_dir(&path).unwrap();
        for path_result in paths {
            let full_path = path_result.unwrap().path();
            if metadata(&full_path).unwrap().is_dir() {
                _list_files(vec, &full_path);
            } else {
                vec.push(full_path);
            }
        }
    }
}

fn list_files(path: &Path) -> Vec<PathBuf> {
    let mut vec = Vec::new();
    _list_files(&mut vec, &path);
    vec
}

fn link_mimalloc() {
    let dir = Path::new(MIMALLOC_SRC);
    let stack = list_files(dir);
    for entry in stack.iter() {
        println!("cargo:rerun-if-changed={}", entry.display());
    }

    let output = Command::new("make")
        .arg("-j8")
        .current_dir(MIMALLOC_OUT)
        .output()
        .expect("compile mimalloc failed");

    io::stdout().write_all(&output.stdout).unwrap();
    io::stderr().write_all(&output.stderr).unwrap();

    println!("cargo::rustc-link-search=native={}", MIMALLOC_OUT);
}
