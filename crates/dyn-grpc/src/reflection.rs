use crate::error::ReflectionError;
use futures_util::StreamExt;
use prost::Message;
use prost_types::FileDescriptorProto;
use prost_types::FileDescriptorSet;
use std::collections::HashMap;
use tonic::transport::Channel;

const REFLECTION_V1: &str = "/grpc.reflection.v1.ServerReflection/ServerReflectionInfo";
const REFLECTION_V1ALPHA: &str =
    "/grpc.reflection.v1alpha.ServerReflection/ServerReflectionInfo";

/// Fetches all file descriptors from the server via gRPC reflection.
/// Tries v1 first, falls back to v1alpha.
pub async fn fetch_all_descriptors(
    channel: &Channel,
) -> Result<FileDescriptorSet, ReflectionError> {
    let services = list_services(channel).await?;
    let path = detect_reflection_path(channel).await?;

    let mut all_fds: HashMap<String, FileDescriptorProto> = HashMap::new();

    for service in &services {
        let fds = get_file_descriptors_for_symbol(channel, service, &path).await?;
        for fd in fds {
            let name = fd.name().to_string();
            all_fds.entry(name).or_insert(fd);
        }
    }

    // Resolve transitive dependencies
    let mut missing: Vec<String> = Vec::new();
    loop {
        missing.clear();
        for fd in all_fds.values() {
            for dep in &fd.dependency {
                if !all_fds.contains_key(dep) {
                    missing.push(dep.clone());
                }
            }
        }
        if missing.is_empty() {
            break;
        }
        for filename in &missing {
            let fds = get_file_descriptors_by_filename(channel, filename, &path).await?;
            for fd in fds {
                let name = fd.name().to_string();
                all_fds.entry(name).or_insert(fd);
            }
        }
    }

    Ok(FileDescriptorSet {
        file: all_fds.into_values().collect(),
    })
}

async fn detect_reflection_path(
    channel: &Channel,
) -> Result<String, ReflectionError> {
    match reflection_call_list(channel, REFLECTION_V1).await {
        Ok(_) => Ok(REFLECTION_V1.to_string()),
        Err(_) => {
            reflection_call_list(channel, REFLECTION_V1ALPHA)
                .await
                .map_err(|_| ReflectionError::NotSupported)?;
            Ok(REFLECTION_V1ALPHA.to_string())
        }
    }
}

async fn list_services(
    channel: &Channel,
) -> Result<Vec<String>, ReflectionError> {
    match reflection_call_list(channel, REFLECTION_V1).await {
        Ok(services) => Ok(services),
        Err(_) => reflection_call_list(channel, REFLECTION_V1ALPHA).await,
    }
}

async fn reflection_call_list(
    channel: &Channel,
    path_str: &str,
) -> Result<Vec<String>, ReflectionError> {
    use http::uri::PathAndQuery;

    let path: PathAndQuery = path_str.parse().expect("hardcoded path is valid");

    let mut client = tonic::client::Grpc::new(channel.clone());
    client.ready().await?;

    let request = ServerReflectionRequest {
        host: String::new(),
        message_request: Some(MessageRequest::ListServices(String::new())),
    };

    let encoded = request.encode_to_vec();

    let (tx, rx) = tokio::sync::mpsc::channel(1);
    tx.send(encoded.into()).await.map_err(|_| ReflectionError::Send)?;
    drop(tx);

    let input_stream = tokio_stream::wrappers::ReceiverStream::new(rx);
    let request = tonic::Request::new(input_stream);

    let codec = RawCodec;
    let response = client.streaming(request, path, codec).await?;

    let mut stream = response.into_inner();
    let mut services = Vec::new();

    while let Some(msg) = stream.next().await {
        let bytes = msg?;
        let resp = ServerReflectionResponse::decode(bytes)?;
        match resp.message_response {
            Some(MessageResponse::ListServicesResponse(list)) => {
                for svc in list.service {
                    if !svc.name.contains("reflection") {
                        services.push(svc.name);
                    }
                }
            }
            Some(MessageResponse::ErrorResponse(err)) => {
                return Err(ReflectionError::Server(err.error_message));
            }
            _ => {}
        }
    }

    Ok(services)
}

async fn get_file_descriptors_for_symbol(
    channel: &Channel,
    symbol: &str,
    path_str: &str,
) -> Result<Vec<FileDescriptorProto>, ReflectionError> {
    fetch_file_descriptors(
        channel,
        path_str,
        MessageRequest::FileContainingSymbol(symbol.to_string()),
    )
    .await
}

async fn get_file_descriptors_by_filename(
    channel: &Channel,
    filename: &str,
    path_str: &str,
) -> Result<Vec<FileDescriptorProto>, ReflectionError> {
    fetch_file_descriptors(
        channel,
        path_str,
        MessageRequest::FileByFilename(filename.to_string()),
    )
    .await
}

async fn fetch_file_descriptors(
    channel: &Channel,
    path_str: &str,
    request_type: MessageRequest,
) -> Result<Vec<FileDescriptorProto>, ReflectionError> {
    use http::uri::PathAndQuery;

    let path: PathAndQuery = path_str.parse().expect("hardcoded path is valid");

    let mut client = tonic::client::Grpc::new(channel.clone());
    client.ready().await?;

    let request = ServerReflectionRequest {
        host: String::new(),
        message_request: Some(request_type),
    };

    let encoded = request.encode_to_vec();

    let (tx, rx) = tokio::sync::mpsc::channel(1);
    tx.send(encoded.into()).await.map_err(|_| ReflectionError::Send)?;
    drop(tx);

    let input_stream = tokio_stream::wrappers::ReceiverStream::new(rx);
    let request = tonic::Request::new(input_stream);

    let codec = RawCodec;
    let response = client.streaming(request, path, codec).await?;

    let mut stream = response.into_inner();
    let mut descriptors = Vec::new();

    while let Some(msg) = stream.next().await {
        let bytes = msg?;
        let resp = ServerReflectionResponse::decode(bytes)?;
        match resp.message_response {
            Some(MessageResponse::FileDescriptorResponse(fd_resp)) => {
                for fd_bytes in fd_resp.file_descriptor_proto {
                    let fd = FileDescriptorProto::decode(fd_bytes.as_slice())?;
                    descriptors.push(fd);
                }
            }
            Some(MessageResponse::ErrorResponse(err)) => {
                return Err(ReflectionError::Server(err.error_message));
            }
            _ => {}
        }
    }

    Ok(descriptors)
}

// Manual protobuf types for the reflection protocol (same wire format for v1 and v1alpha)

#[derive(Clone, prost::Message)]
struct ServerReflectionRequest {
    #[prost(string, tag = "1")]
    host: String,
    #[prost(oneof = "MessageRequest", tags = "3, 4, 5, 6, 7")]
    message_request: Option<MessageRequest>,
}

#[derive(Clone, prost::Oneof)]
enum MessageRequest {
    #[prost(string, tag = "3")]
    FileByFilename(String),
    #[prost(string, tag = "4")]
    FileContainingSymbol(String),
    #[prost(message, tag = "5")]
    FileContainingExtension(ExtensionRequest),
    #[prost(string, tag = "6")]
    AllExtensionNumbersOfType(String),
    #[prost(string, tag = "7")]
    ListServices(String),
}

#[derive(Clone, prost::Message)]
struct ExtensionRequest {
    #[prost(string, tag = "1")]
    containing_type: String,
    #[prost(int32, tag = "2")]
    extension_number: i32,
}

#[derive(Clone, prost::Message)]
struct ServerReflectionResponse {
    #[prost(string, tag = "1")]
    valid_host: String,
    #[prost(message, optional, tag = "2")]
    original_request: Option<ServerReflectionRequest>,
    #[prost(oneof = "MessageResponse", tags = "4, 5, 6, 7")]
    message_response: Option<MessageResponse>,
}

#[derive(Clone, prost::Oneof)]
enum MessageResponse {
    #[prost(message, tag = "4")]
    FileDescriptorResponse(FileDescriptorResponse),
    #[prost(message, tag = "5")]
    AllExtensionNumbersResponse(ExtensionNumberResponse),
    #[prost(message, tag = "6")]
    ListServicesResponse(ListServiceResponse),
    #[prost(message, tag = "7")]
    ErrorResponse(ErrorResponse),
}

#[derive(Clone, prost::Message)]
struct FileDescriptorResponse {
    #[prost(bytes = "vec", repeated, tag = "1")]
    file_descriptor_proto: Vec<Vec<u8>>,
}

#[derive(Clone, prost::Message)]
struct ExtensionNumberResponse {
    #[prost(string, tag = "1")]
    base_type_name: String,
    #[prost(int32, repeated, tag = "2")]
    extension_number: Vec<i32>,
}

#[derive(Clone, prost::Message)]
struct ListServiceResponse {
    #[prost(message, repeated, tag = "1")]
    service: Vec<ServiceResponse>,
}

#[derive(Clone, prost::Message)]
struct ServiceResponse {
    #[prost(string, tag = "1")]
    name: String,
}

#[derive(Clone, prost::Message)]
struct ErrorResponse {
    #[prost(int32, tag = "1")]
    error_code: i32,
    #[prost(string, tag = "2")]
    error_message: String,
}

// Raw codec that passes bytes through
#[derive(Clone)]
struct RawCodec;

impl tonic::codec::Codec for RawCodec {
    type Encode = bytes::Bytes;
    type Decode = bytes::Bytes;
    type Encoder = RawEncoder;
    type Decoder = RawDecoder;

    fn encoder(&mut self) -> Self::Encoder {
        RawEncoder
    }

    fn decoder(&mut self) -> Self::Decoder {
        RawDecoder
    }
}

#[derive(Clone)]
struct RawEncoder;

impl tonic::codec::Encoder for RawEncoder {
    type Item = bytes::Bytes;
    type Error = tonic::Status;

    fn encode(
        &mut self,
        item: Self::Item,
        dst: &mut tonic::codec::EncodeBuf<'_>,
    ) -> Result<(), Self::Error> {
        use bytes::BufMut;
        dst.put(item);
        Ok(())
    }
}

#[derive(Clone)]
struct RawDecoder;

impl tonic::codec::Decoder for RawDecoder {
    type Item = bytes::Bytes;
    type Error = tonic::Status;

    fn decode(
        &mut self,
        src: &mut tonic::codec::DecodeBuf<'_>,
    ) -> Result<Option<Self::Item>, Self::Error> {
        use bytes::Buf;
        let remaining = src.remaining();
        if remaining == 0 {
            return Ok(None);
        }
        Ok(Some(src.copy_to_bytes(remaining)))
    }
}
