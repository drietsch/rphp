//! The `rphp` binary. Thin wrapper over the CLI SAPI.
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(rphp_sapi_cli::run(args));
}
