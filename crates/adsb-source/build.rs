//! Find librtlsdr, and only when the `usb` feature asks for it.
//!
//! Without `--features usb` this script does nothing at all, which is the
//! point: the default build of this crate has no native dependency, so
//! `cargo build --target aarch64-unknown-linux-gnu` needs no sysroot, no
//! cross-compiler and no libusb headers.
//!
//! # Static on macOS, dynamic on Linux
//!
//! Homebrew builds `librtlsdr.0.dylib` with an install name of
//! `@rpath/librtlsdr.0.dylib`. A Rust binary linked against it records that
//! literal string and then fails at startup with "Library not loaded" unless
//! the caller exports `DYLD_LIBRARY_PATH=/opt/homebrew/lib` — which is a
//! footgun to hand someone whose actual goal is to look at aeroplanes. So on
//! macOS we link the static `librtlsdr.a` that Homebrew ships beside it and
//! let libusb, whose install name *is* absolute, stay dynamic.
//!
//! On Linux the distribution packages have ordinary SONAMEs in the ordinary
//! places, so the ordinary dynamic link is right. `apt install librtlsdr-dev`
//! is all it takes.
//!
//! Override the search path with `RTLSDR_LIB_DIR`, and force one linkage or
//! the other with `RTLSDR_STATIC=1` / `RTLSDR_STATIC=0`.

use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-env-changed=RTLSDR_LIB_DIR");
    println!("cargo:rerun-if-env-changed=RTLSDR_STATIC");
    println!("cargo:rerun-if-changed=build.rs");

    if std::env::var_os("CARGO_FEATURE_USB").is_none() {
        return;
    }

    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let macos = target_os == "macos";

    let dir = locate(&target_os);

    let static_link = match std::env::var("RTLSDR_STATIC").ok().as_deref() {
        Some("1") | Some("true") => true,
        Some("0") | Some("false") => false,
        // Default to whatever avoids the rpath trap on this platform.
        _ => macos && dir.as_ref().is_some_and(|d| d.join("librtlsdr.a").exists()),
    };

    // `-l static=rtlsdr` is only a hint on macOS: ld64 has no `-Bstatic`, so
    // given a directory holding both `librtlsdr.a` and `librtlsdr.dylib` it
    // takes the dylib and we are back to the rpath failure. Staging the
    // archive alone in OUT_DIR and searching there first leaves the linker no
    // choice. (Verified: without this the test binary aborts at startup with
    // "Library not loaded: @rpath/librtlsdr.0.dylib".)
    let staged = static_link
        .then(|| dir.as_ref().and_then(|d| stage_archive(d)))
        .flatten();
    if let Some(staged) = &staged {
        println!("cargo:rustc-link-search=native={}", staged.display());
    }
    if let Some(dir) = &dir {
        println!("cargo:rustc-link-search=native={}", dir.display());
    }

    if static_link {
        println!("cargo:rustc-link-lib=static=rtlsdr");
        // A static librtlsdr carries no record of what it needs, so its own
        // dependencies have to be named here.
        println!("cargo:rustc-link-lib=dylib=usb-1.0");
        if macos {
            // libusb's macOS backend talks to IOKit, and IOKit's device
            // matching goes through CoreFoundation. Omitting either produces a
            // page of undefined symbols at final link, not at compile.
            println!("cargo:rustc-link-lib=framework=IOKit");
            println!("cargo:rustc-link-lib=framework=CoreFoundation");
            println!("cargo:rustc-link-lib=framework=Security");
            println!("cargo:rustc-link-lib=dylib=objc");
        }
    } else {
        println!("cargo:rustc-link-lib=dylib=rtlsdr");
    }

    if dir.is_none() {
        println!(
            "cargo:warning=librtlsdr was not found in any of the usual places. \
             Install it (macOS: `brew install librtlsdr`; Debian/Raspberry Pi OS: \
             `sudo apt install librtlsdr-dev`) or set RTLSDR_LIB_DIR to the \
             directory holding librtlsdr."
        );
    }
}

/// Copy `librtlsdr.a` into OUT_DIR so it is the only candidate there.
fn stage_archive(dir: &Path) -> Option<PathBuf> {
    let archive = dir.join("librtlsdr.a");
    if !archive.exists() {
        return None;
    }
    let out = PathBuf::from(std::env::var_os("OUT_DIR")?).join("rtlsdr-static");
    std::fs::create_dir_all(&out).ok()?;
    std::fs::copy(&archive, out.join("librtlsdr.a")).ok()?;
    println!("cargo:rerun-if-changed={}", archive.display());
    Some(out)
}

/// The directory holding librtlsdr, if one of the usual ones has it.
fn locate(target_os: &str) -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("RTLSDR_LIB_DIR") {
        return Some(PathBuf::from(dir));
    }

    // Homebrew first on macOS, then the multiarch and plain paths Debian and
    // Raspberry Pi OS use. Nothing here is a fallback for a *wrong* answer:
    // the linker takes the first directory that actually contains the library.
    let candidates: &[&str] = if target_os == "macos" {
        &["/opt/homebrew/lib", "/usr/local/lib"]
    } else {
        &[
            "/usr/lib/aarch64-linux-gnu",
            "/usr/lib/arm-linux-gnueabihf",
            "/usr/lib/x86_64-linux-gnu",
            "/usr/local/lib",
            "/usr/lib",
        ]
    };

    candidates
        .iter()
        .map(Path::new)
        .find(|dir| {
            dir.join("librtlsdr.a").exists()
                || dir.join("librtlsdr.so").exists()
                || dir.join("librtlsdr.dylib").exists()
        })
        .map(Path::to_path_buf)
}
