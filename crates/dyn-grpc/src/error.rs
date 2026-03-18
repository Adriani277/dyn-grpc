use thiserror::Error;

#[derive(Debug, Error)]
pub enum ClientError {
    #[error("invalid server address")]
    InvalidUri,

    #[error("failed to connect to server: {0}")]
    Connect(#[from] tonic::transport::Error),

    #[error(transparent)]
    Reflection(#[from] ReflectionError),

    #[error("failed to build descriptor pool: {0}")]
    DescriptorPool(#[from] prost_reflect::DescriptorError),

    #[error(transparent)]
    Call(#[from] CallError),
}

#[derive(Debug, Error)]
pub enum CallError {
    #[error("invalid method format, expected 'package.Service/Method'")]
    InvalidMethodFormat,

    #[error("service '{0}' not found")]
    ServiceNotFound(String),

    #[error("method '{0}' not found in service")]
    MethodNotFound(String),

    #[error("invalid JSON input: {0}")]
    InvalidJson(#[from] serde_json::Error),

    #[error("failed to build request message: {0}")]
    MessageBuild(#[source] serde_json::Error),

    #[error("invalid header: {0}")]
    InvalidHeader(String),

    #[error("gRPC transport error: {0}")]
    Transport(#[from] tonic::transport::Error),

    #[error("gRPC call failed: {0}")]
    Grpc(#[from] tonic::Status),
}

#[derive(Debug, Error)]
pub enum ReflectionError {
    #[error("server does not support reflection (tried v1 and v1alpha)")]
    NotSupported,

    #[error("reflection transport error: {0}")]
    Transport(#[from] tonic::transport::Error),

    #[error("reflection stream failed: {0}")]
    Stream(#[from] tonic::Status),

    #[error("failed to decode reflection response: {0}")]
    Decode(#[from] prost::DecodeError),

    #[error("reflection server returned error: {0}")]
    Server(String),

    #[error("failed to send reflection request")]
    Send,
}
