use std::path::PathBuf;

fn main() {
    let celt_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("lib/celt")
        .canonicalize()
        .expect("lib/celt directory not found");

    println!("cargo:rustc-link-search=native={}", celt_dir.display());
    // vaudio_celt_client.so has no "lib" prefix, so it can't be linked with
    // a plain -l flag; -l: takes the filename verbatim (same as the gcc
    // command used to build celt_convert/csgo.c).
    println!("cargo:rustc-link-arg=-l:vaudio_celt_client.so");
    // Bake in an rpath (old-style, transitive) so the resulting binary can
    // find vaudio_celt_client.so and its libtier0_client.so dependency at
    // runtime without needing LD_LIBRARY_PATH set.
    println!(
        "cargo:rustc-link-arg=-Wl,--disable-new-dtags,-rpath,{}",
        celt_dir.display()
    );
}
