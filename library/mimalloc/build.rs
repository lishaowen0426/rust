#![allow(dead_code)]
#![allow(unused_imports)]
use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

const MIMALLOC_ROOT: &str = "/home/swli/inprocess/mimalloc";
const MIMALLOC_SRC: &str = "/home/swli/inprocess/mimalloc/src";

fn rerun_if_changed_anything_in_dir(dir: &Path) {
    let mut stack = dir
        .read_dir()
        .unwrap()
        .map(|e| e.unwrap())
        .filter(|e| &*e.file_name() != ".git")
        .collect::<Vec<_>>();
    while let Some(entry) = stack.pop() {
        let path = entry.path();
        if entry.file_type().unwrap().is_dir() {
            stack.extend(path.read_dir().unwrap().map(|e| e.unwrap()));
        } else {
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }
}

fn build_mimalloc() {
    println!("cargo::rustc-link-search=native=/home/swli/inprocess/mimalloc/out/secure");
    println!("cargo:rustc-link-lib=static=mimalloc");
}

fn main() {
    build_mimalloc();
}
