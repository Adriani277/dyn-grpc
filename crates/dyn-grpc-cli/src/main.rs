use clap::Parser;
use dyn_grpc::GrpcClient;

#[derive(Parser)]
#[command(name = "dyn-grpc", about = "Dynamic gRPC client using server reflection")]
struct Cli {
    /// gRPC server address (e.g. http://localhost:8080)
    #[arg(short, long, default_value = "http://localhost:8080")]
    address: String,

    /// Fully qualified method name (e.g. package.Service/Method)
    #[arg(short, long)]
    method: Option<String>,

    /// JSON request body
    #[arg(short, long, default_value = "{}")]
    data: String,

    /// List all available services
    #[arg(short, long)]
    list: bool,

    /// Describe a service or method
    #[arg(long)]
    describe: Option<String>,

    /// Add metadata header (repeatable, format: key:value)
    #[arg(short = 'H', long = "header")]
    headers: Vec<String>,

    /// Use server-streaming instead of unary
    #[arg(short, long)]
    stream: bool,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    let client = GrpcClient::connect(&cli.address).await?;

    if cli.list {
        println!("Available services:");
        for svc in client.services() {
            println!("  {}", svc.name);
            for m in &svc.methods {
                println!("    - {}/{}", svc.name, m.name);
            }
        }
        return Ok(());
    }

    if let Some(ref name) = cli.describe {
        describe(client.descriptor_pool(), name)?;
        return Ok(());
    }

    let method = cli.method.as_deref().ok_or("--method is required for calls")?;

    let headers: Vec<(&str, &str)> = cli
        .headers
        .iter()
        .map(|h| {
            h.split_once(':')
                .map(|(k, v)| (k.trim(), v.trim()))
                .expect("Header must be in format 'key:value'")
        })
        .collect();

    let hdrs = if headers.is_empty() { None } else { Some(headers.as_slice()) };

    if cli.stream {
        let mut stream = client.stream(method, &cli.data, hdrs).await?;
        while let Some(result) = stream.next().await {
            let value = result?;
            println!("{}", serde_json::to_string_pretty(&value)?);
        }
    } else {
        let response = client.unary(method, &cli.data, hdrs).await?;
        println!("{}", serde_json::to_string_pretty(&response)?);
    }

    Ok(())
}

fn describe(
    pool: &prost_reflect::DescriptorPool,
    name: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(service) = pool.get_service_by_name(name) {
        println!("Service: {}", service.full_name());
        for method in service.methods() {
            println!(
                "  rpc {}({}) returns ({})",
                method.name(),
                method.input().full_name(),
                method.output().full_name()
            );
        }
        return Ok(());
    }

    if let Some((svc, mtd)) = name.rsplit_once('/') {
        if let Some(service) = pool.get_service_by_name(svc) {
            if let Some(method) = service.methods().find(|m| m.name() == mtd) {
                println!("Method: {}/{}", svc, method.name());
                println!("  Input:  {}", method.input().full_name());
                for field in method.input().fields() {
                    println!("    {:?} {} = {}", field.kind(), field.name(), field.number());
                }
                println!("  Output: {}", method.output().full_name());
                for field in method.output().fields() {
                    println!("    {:?} {} = {}", field.kind(), field.name(), field.number());
                }
                return Ok(());
            }
        }
    }

    Err(format!("'{}' not found", name).into())
}
