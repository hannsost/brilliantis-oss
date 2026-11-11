#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    brilliantis::runtime::run(brilliantis::profile::Profile::standalone()).await
}
