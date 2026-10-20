//! Feature `call`: libopus, vendored in `opus/` (`opus/README` says from
//! where), built by `cc`. Every C file there is one libopus's own build
//! lists, so all of them are compiled. Floating point, no SIMD: what
//! libopus's `configure --disable-intrinsics --disable-rtcd` builds.
//! ponytail: no NEON or SSE; libopus's runtime-detected intrinsics are the
//! upgrade if a phone's profile ever shows the voice codec.
fn main() {
    #[cfg(feature = "call")]
    opus();
}

#[cfg(feature = "call")]
fn opus() {
    println!("cargo:rerun-if-changed=opus");
    let dirs = ["opus/celt", "opus/silk", "opus/silk/float", "opus/src"];
    let mut files = Vec::new();
    for dir in dirs {
        for entry in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{dir}: {e}")) {
            let path = entry.expect("a vendored libopus file").path();
            if path.extension().is_some_and(|ext| ext == "c") {
                files.push(path);
            }
        }
    }
    files.sort();
    cc::Build::new()
        .files(files)
        .include("opus/include")
        .includes(dirs)
        .define("OPUS_BUILD", None)
        .define("VAR_ARRAYS", None)
        .define("HAVE_LRINT", None)
        .define("HAVE_LRINTF", None)
        .define("FLOAT_APPROX", None)
        .define("ENABLE_HARDENING", None)
        .define("PACKAGE_VERSION", "\"1.5.2\"")
        .opt_level(2)
        .warnings(false)
        .compile("opus");
}
