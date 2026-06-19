use clap::Parser;

#[derive(Parser)]
#[command(name = "aether-lspd")]
#[command(about = "LSP daemon for sharing language servers across multiple agents")]
struct Args {
    #[command(flatten)]
    inner: aether_lspd::LspdArgs,
}

fn main() {
    let args = Args::parse();
    if let Err(e) = aether_lspd::run_lspd(args.inner) {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}
