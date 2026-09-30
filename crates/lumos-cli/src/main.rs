//! `lumos` developer binary: thin entry point over [`lumos_cli::run`].

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(lumos_cli::run(&argv));
}
