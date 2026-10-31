use clankerdiff_server::{DiffServer, ServerOptions, StreamTransport};

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let repository = std::env::args().nth(1).expect("the repository to serve");
    let server = DiffServer::open(repository, ServerOptions::default()).await.expect("open the repository");
    server.serve(StreamTransport::new(tokio::io::stdin(), tokio::io::stdout())).await.expect("serve stdin and stdout");
}
