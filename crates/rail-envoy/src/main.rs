use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    rail_envoy::run().await
}
