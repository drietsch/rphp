//! `cargo xtask <subcommand>` — repository automation. Each subcommand lives in
//! its own module so parallel work never touches a shared file; this file is
//! only the dispatcher.

mod corpus;
mod coverage;
mod fetch_php_src;
mod gen;
mod ladder;
mod missing;
mod native_params;
mod string_params;
mod parse_sweep;
mod phpt;
mod token_ids;

pub type XtaskResult = Result<(), Box<dyn std::error::Error>>;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (cmd, rest) = match args.split_first() {
        Some((c, r)) => (c.as_str(), r.to_vec()),
        None => {
            usage();
            std::process::exit(2);
        }
    };
    let result = match cmd {
        "gen" => gen::run(&rest),
        "coverage" => coverage::run(&rest),
        "phpt" => phpt::run(&rest),
        "ladder" => ladder::run(&rest),
        "missing" => missing::run(&rest),
        "string-params" => string_params::run(&rest),
        "native-params" => native_params::run(&rest),
        "corpus" => corpus::run(&rest),
        "token-ids" => token_ids::run(&rest),
        "parse-sweep" => parse_sweep::run(&rest),
        "fetch-php-src" => fetch_php_src::run(&rest),
        "-h" | "--help" | "help" => {
            usage();
            Ok(())
        }
        other => {
            eprintln!("xtask: unknown subcommand `{other}`");
            usage();
            std::process::exit(2);
        }
    };
    if let Err(e) = result {
        eprintln!("xtask {cmd}: {e}");
        std::process::exit(1);
    }
}

fn usage() {
    eprintln!(
        "usage: cargo xtask <subcommand> [args]\n\
         \n\
         gen            generate arginfo/constants/ini tables from manifest/ into the ext crates (--check)\n\
         coverage       recompute crates/rphp-stdlib/COVERAGE.md vs the manifest (gates regressions)\n\
         phpt           run php-src .phpt slices (--ext <name>, --filter <glob>, --gate, --update-baseline)\n\
         ladder         run a Symfony ladder rung under php and rphp (--rung L<n>)\n\
         missing        list internal functions/classes a PHP tree uses that rphp lacks (--dir <path>)\n\
         corpus         parse-parity job over a PHP corpus vs `php -l` (--dir <path>)\n\
         token-ids      regenerate crates/rphp-tokenizer/src/ids.rs from the installed php\n\
         parse-sweep    parse-only sweep over php-src Zend/tests + tests/lang\n\
         fetch-php-src  sparse checkout of the pinned php-src test corpus"
    );
}
