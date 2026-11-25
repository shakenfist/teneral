//! Compiles the control plane's schema into Rust.
//!
//! The output goes to `OUT_DIR` and is never committed. A generated
//! file in the tree is a second copy of the schema that can disagree
//! with the first, and the disagreement is invisible until somebody
//! reads both; compiling at build time makes that drift impossible
//! rather than merely detectable.
//!
//! This runs `protoc`, which the build image carries. That is also why
//! it can run with no network and against a read-only cargo cache,
//! like every other compile in this repository: the compiler is in the
//! image, so nothing here fetches anything.
//!
//! Nothing is configured beyond the paths. Type attributes, extra
//! derives and bytes substitutions are all ways of making the
//! generated types something other than what the schema says, and the
//! point of having a schema is that both ends come from it unaltered.

/// The schema, which is the whole of the wire.
const SCHEMA: &str = "proto/teneral.v1.proto";

/// What an `import` in it would be resolved against. One directory,
/// because the schema is one file: a wire that needed several would be
/// a wire nobody reads in one sitting.
const DIRECTORY: &str = "proto";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // `prost-build` emits its own `cargo:rerun-if-changed` for the
    // schema and for anything it includes, which is why there is none
    // written here: naming the file twice would be two lists to keep in
    // step, and the one cargo read would be the one that forgot an
    // include.
    prost_build::compile_protos(&[SCHEMA], &[DIRECTORY])?;

    Ok(())
}
