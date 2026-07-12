use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use implib::{Flavor, ImportLibrary, MachineType};

fn main() {
    let celt_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("lib/celt")
        .canonicalize()
        .expect("lib/celt directory not found");
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap();
    let runtime_libs: &[&str] = if target_os == "windows" {
        &["vaudio_celt.dll", "tier0.dll"]
    } else {
        &["vaudio_celt_client.so", "libtier0_client.so"]
    };

    // Copy the runtime libraries next to the produced binary so `cargo
    // run`/`cargo build` yield an immediately runnable executable, and so
    // the binary can be distributed by copying it out of target/<profile>/
    // together with these files.
    let bin_dir = output_dir();
    for lib in runtime_libs {
        let src = celt_dir.join(lib);
        println!("cargo:rerun-if-changed={}", src.display());
        fs::copy(&src, bin_dir.join(lib)).unwrap_or_else(|e| {
            panic!("failed to copy {} to {}: {}", src.display(), bin_dir.display(), e)
        });
    }

    if target_os == "windows" {
        link_windows();
    } else {
        link_unix(&celt_dir);
    }
}

/// OUT_DIR is target/<profile>/build/<pkg>-<hash>/out; the binary lands
/// three directories up, in target/<profile>.
fn output_dir() -> PathBuf {
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    out_dir
        .ancestors()
        .nth(3)
        .expect("OUT_DIR has unexpected layout")
        .to_path_buf()
}

fn link_unix(celt_dir: &Path) {
    println!("cargo:rustc-link-search=native={}", celt_dir.display());
    // vaudio_celt_client.so has no "lib" prefix, so it can't be linked with
    // a plain -l flag; -l: takes the filename verbatim (same as the gcc
    // command used to build celt_convert/csgo.c).
    println!("cargo:rustc-link-arg=-l:vaudio_celt_client.so");
    // Bake in an rpath (old-style, transitive) so the resulting binary can
    // find vaudio_celt_client.so and its libtier0_client.so dependency at
    // runtime without needing LD_LIBRARY_PATH set. $ORIGIN is resolved by
    // the dynamic linker relative to the binary's own location, so the
    // binary and .so files can be copied together to another machine as
    // long as they stay in the same directory.
    println!("cargo:rustc-link-arg=-Wl,--disable-new-dtags,-rpath,$ORIGIN");
}

fn link_windows() {
    let arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap();
    assert_eq!(
        arch, "x86_64",
        "only x86_64 vaudio_celt.dll/tier0.dll are vendored; got target arch {arch}"
    );
    let flavor = match env::var("CARGO_CFG_TARGET_ENV").as_deref() {
        Ok("msvc") => Flavor::Msvc,
        Ok("gnu") => Flavor::Gnu,
        Ok(env) => panic!("unsupported target env {env}; expected msvc or gnu"),
        Err(_) => panic!("CARGO_CFG_TARGET_ENV not set"),
    };

    // vaudio_celt.dll ships with no import library, so generate a minimal
    // one from a .def file listing just the symbols src/celt.rs's
    // `extern "C"` block actually calls. tier0.dll needs no import library:
    // nothing here calls into it directly, it's only vaudio_celt.dll's own
    // runtime dependency, resolved by the loader as long as tier0.dll sits
    // next to the .exe (handled by the copy step in `main`).
    let def = "\
LIBRARY vaudio_celt.dll
EXPORTS
celt_mode_create
celt_mode_destroy
celt_decoder_create_custom
celt_decoder_destroy
celt_decode
";
    let import_lib = ImportLibrary::new(def, MachineType::AMD64, flavor)
        .expect("failed to parse generated .def file");

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let lib_path = match flavor {
        Flavor::Msvc => out_dir.join("vaudio_celt.lib"),
        Flavor::Gnu => out_dir.join("libvaudio_celt.dll.a"),
    };
    let mut file = fs::File::create(&lib_path)
        .unwrap_or_else(|e| panic!("failed to create {}: {}", lib_path.display(), e));
    import_lib
        .write_to(&mut file)
        .expect("failed to write import library");

    println!("cargo:rustc-link-search=native={}", out_dir.display());
    println!("cargo:rustc-link-lib=dylib=vaudio_celt");
}
