//! The `rphp` binary. Thin wrapper over the CLI SAPI.

/// The process allocator (see the workspace manifest): every PHP string,
/// array and object is a heap allocation of its own here.
#[global_allocator]
static ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(rphp_sapi_cli::run(args));
}
