# dyn-grpc

A Rust library and CLI for making dynamic gRPC calls using server reflection. Build and send protobuf messages from JSON at runtime — no code generation needed.

## Features

- Server reflection with automatic v1/v1alpha fallback
- Transitive proto dependency resolution
- JSON-based message building and response parsing
- Unary and server-streaming RPCs
- Custom metadata headers
- Typed errors via `thiserror`

## Library Usage

```rust
use dyn_grpc::GrpcClient;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = GrpcClient::connect("http://localhost:8080").await?;

    // List available services
    for svc in client.services() {
        println!("{}", svc.name);
    }

    // Unary call
    let response = client.unary(
        "package.Service/Method",
        r#"{"field": "value"}"#,
        None,
    ).await?;
    println!("{}", response);

    // Unary call with headers
    let response = client.unary(
        "package.Service/Method",
        r#"{"field": "value"}"#,
        Some(&[("authorization", "Bearer token")]),
    ).await?;

    // Server streaming
    let mut stream = client.stream(
        "package.Service/StreamMethod",
        r#"{"field": "value"}"#,
        None,
    ).await?;
    while let Some(result) = stream.next().await {
        println!("{}", result?);
    }

    Ok(())
}
```

## CLI Usage

```bash
# List services
dyn-grpc --list

# Describe a service
dyn-grpc --describe package.Service

# Unary call
dyn-grpc -m "package.Service/Method" -d '{"field": "value"}'

# With headers
dyn-grpc -m "package.Service/Method" -d '{"field": "value"}' -H "authorization:Bearer token"

# Server streaming
dyn-grpc -m "package.Service/StreamMethod" -d '{}' --stream

# Custom address
dyn-grpc -a http://localhost:9090 --list

# JSON from file
dyn-grpc -m "package.Service/Method" -d "$(cat request.json)"
```

## Project Structure

```
crates/
  dyn-grpc/       # Library crate
  dyn-grpc-cli/   # CLI binary
```

## Building

```bash
# Build everything
cargo build

# Build release binary
cargo build --release -p dyn-grpc-cli

# Run CLI
cargo run -p dyn-grpc-cli -- --list
```
