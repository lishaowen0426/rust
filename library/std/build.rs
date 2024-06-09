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
    let mut out_dir = PathBuf::new();
    out_dir.push(env::var("OUT_DIR").unwrap());
    let build_dir = out_dir.join("mimalloc/build");
    let install_dir = out_dir.join("mimalloc/install");
    Command::new("cmake")
        .args(["-S", MIMALLOC_ROOT])
        .args(["-B", build_dir.to_str().unwrap()])
        .args([
            "-DMI_SECURE=ON",
            "-DMI_BUILD_SHARED=OFF",
            "-DMI_BUILD_OBJECT=ON",
            "-DMI_BUILD_TESTS=OFF",
            "-DMI_INSTALL_TOPLEVEL=ON",
            "-DCMAKE_BUILD_TYPE=Release",
        ])
        .arg(format!("-DCMAKE_INSTALL_PREFIX={}", install_dir.to_str().unwrap()))
        .output()
        .expect("mimalloc cmake configure failed");

    Command::new("make")
        .args(["-C", build_dir.to_str().unwrap()])
        .arg("install")
        .output()
        .expect("make mimalloc filed");

    let lib_dir = install_dir.join("lib");
    println!("cargo::rustc-link-search=native={}", lib_dir.to_str().unwrap());
    println!("cargo:rustc-link-lib=static=mimalloc");
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").expect("CARGO_CFG_TARGET_ARCH was not set");
    let target_os = env::var("CARGO_CFG_TARGET_OS").expect("CARGO_CFG_TARGET_OS was not set");
    let target_vendor =
        env::var("CARGO_CFG_TARGET_VENDOR").expect("CARGO_CFG_TARGET_VENDOR was not set");
    let target_env = env::var("CARGO_CFG_TARGET_ENV").expect("CARGO_CFG_TARGET_ENV was not set");
    if target_os == "netbsd" && env::var("RUSTC_STD_NETBSD10").is_ok() {
        println!("cargo:rustc-cfg=netbsd10");
    }
    if target_os == "linux"
        || target_os == "android"
        || target_os == "netbsd"
        || target_os == "dragonfly"
        || target_os == "openbsd"
        || target_os == "freebsd"
        || target_os == "solaris"
        || target_os == "illumos"
        || target_os == "macos"
        || target_os == "ios"
        || target_os == "tvos"
        || target_os == "watchos"
        || target_os == "windows"
        || target_os == "fuchsia"
        || (target_vendor == "fortanix" && target_env == "sgx")
        || target_os == "hermit"
        || target_os == "l4re"
        || target_os == "redox"
        || target_os == "haiku"
        || target_os == "vxworks"
        || target_arch == "wasm32"
        || target_arch == "wasm64"
        || target_os == "espidf"
        || target_os.starts_with("solid")
        || (target_vendor == "nintendo" && target_env == "newlib")
        || target_os == "vita"
        || target_os == "aix"
        || target_os == "nto"
        || target_os == "xous"
        || target_os == "hurd"
        || target_os == "uefi"
        || target_os == "teeos"
        || target_os == "zkvm"

        // See src/bootstrap/src/core/build_steps/synthetic_targets.rs
        || env::var("RUSTC_BOOTSTRAP_SYNTHETIC_TARGET").is_ok()
    {
        // These platforms don't have any special requirements.
    } else {
        // This is for Cargo's build-std support, to mark std as unstable for
        // typically no_std platforms.
        // This covers:
        // - os=none ("bare metal" targets)
        // - mipsel-sony-psp
        // - nvptx64-nvidia-cuda
        // - arch=avr
        // - JSON targets
        // - Any new targets that have not been explicitly added above.
        println!("cargo:rustc-cfg=feature=\"restricted-std\"");
    }
    println!("cargo:rustc-env=STD_ENV_ARCH={}", env::var("CARGO_CFG_TARGET_ARCH").unwrap());
    println!("cargo:rustc-cfg=backtrace_in_libstd");

    {
        //compiler mimalloc

        rerun_if_changed_anything_in_dir(Path::new(MIMALLOC_SRC));
        // build_mimalloc();
    }
}
