pub mod error;
pub mod reflection;

use bytes::Bytes;
use error::{CallError, ClientError};
use http::uri::PathAndQuery;
use prost::Message;
use prost_reflect::DynamicMessage;
use serde_json::Value;
use tonic::transport::Channel;

/// A dynamic gRPC client that discovers services via server reflection
/// and builds messages from JSON at runtime.
pub struct GrpcClient {
    channel: Channel,
    pool: prost_reflect::DescriptorPool,
}

impl GrpcClient {
    /// Connect to a gRPC server and fetch its service descriptors via reflection.
    pub async fn connect(address: &str) -> Result<Self, ClientError> {
        let channel = Channel::from_shared(address.to_string())
            .map_err(|_| ClientError::InvalidUri)?
            .connect()
            .await?;

        let file_descriptor_set = reflection::fetch_all_descriptors(&channel).await?;
        let pool = prost_reflect::DescriptorPool::from_file_descriptor_set(file_descriptor_set)?;

        Ok(Self { channel, pool })
    }

    /// List all available services and their methods.
    pub fn services(&self) -> Vec<ServiceInfo> {
        self.pool
            .services()
            .map(|svc| ServiceInfo {
                name: svc.full_name().to_string(),
                methods: svc
                    .methods()
                    .map(|m| MethodInfo {
                        name: m.name().to_string(),
                        input_type: m.input().full_name().to_string(),
                        output_type: m.output().full_name().to_string(),
                    })
                    .collect(),
            })
            .collect()
    }

    /// Call a unary RPC method with a JSON payload and optional metadata headers.
    ///
    /// `method` should be in the format `"package.Service/Method"`.
    /// `json_data` is the request body as a JSON string.
    /// `headers` are optional `(key, value)` pairs inserted into the gRPC request metadata.
    ///
    /// Returns the response as a `serde_json::Value`.
    pub async fn unary(
        &self,
        method: &str,
        json_data: &str,
        headers: Option<&[(&str, &str)]>,
    ) -> Result<Value, CallError> {
        let prepared = self.prepare_request(method, json_data, headers)?;

        let mut grpc_client = tonic::client::Grpc::new(self.channel.clone());
        grpc_client.ready().await?;

        let codec = DynamicCodec::new(prepared.method_desc.clone());
        let response = grpc_client
            .unary(prepared.request, prepared.path, codec)
            .await?;

        let response_msg = response.into_inner();
        let value = serde_json::to_value(&response_msg)?;
        Ok(value)
    }

    /// Call a server-streaming RPC method with a JSON payload and optional metadata headers.
    ///
    /// Returns a stream that yields each response message as a `serde_json::Value`.
    pub async fn stream(
        &self,
        method: &str,
        json_data: &str,
        headers: Option<&[(&str, &str)]>,
    ) -> Result<ResponseStream, CallError> {
        let prepared = self.prepare_request(method, json_data, headers)?;

        let mut grpc_client = tonic::client::Grpc::new(self.channel.clone());
        grpc_client.ready().await?;

        let codec = DynamicCodec::new(prepared.method_desc.clone());
        let response = grpc_client
            .server_streaming(prepared.request, prepared.path, codec)
            .await?;

        Ok(ResponseStream {
            inner: response.into_inner(),
        })
    }

    /// Get the underlying descriptor pool for advanced use cases.
    pub fn descriptor_pool(&self) -> &prost_reflect::DescriptorPool {
        &self.pool
    }

    fn prepare_request(
        &self,
        method: &str,
        json_data: &str,
        headers: Option<&[(&str, &str)]>,
    ) -> Result<PreparedRequest, CallError> {
        let (service_name, method_name) = method
            .rsplit_once('/')
            .ok_or(CallError::InvalidMethodFormat)?;

        let service = self
            .pool
            .get_service_by_name(service_name)
            .ok_or_else(|| CallError::ServiceNotFound(service_name.to_string()))?;

        let method_desc = service
            .methods()
            .find(|m| m.name() == method_name)
            .ok_or_else(|| CallError::MethodNotFound(method_name.to_string()))?;

        let json: Value = serde_json::from_str(json_data)?;
        let json_str = serde_json::to_string(&json)?;
        let mut deserializer = serde_json::Deserializer::from_str(&json_str);
        let request_msg = DynamicMessage::deserialize(method_desc.input(), &mut deserializer)
            .map_err(CallError::MessageBuild)?;

        let encoded: Bytes = request_msg.encode_to_vec().into();
        let path: PathAndQuery = format!("/{}/{}", service_name, method_name)
            .parse()
            .map_err(|_| CallError::InvalidMethodFormat)?;

        let mut request = tonic::Request::new(encoded);
        for (key, value) in headers.unwrap_or_default() {
            let meta_key: tonic::metadata::MetadataKey<tonic::metadata::Ascii> = key
                .parse()
                .map_err(|_| CallError::InvalidHeader(key.to_string()))?;
            let meta_value: tonic::metadata::AsciiMetadataValue = value
                .parse()
                .map_err(|_| CallError::InvalidHeader(format!("{}: {}", key, value)))?;
            request.metadata_mut().insert(meta_key, meta_value);
        }

        Ok(PreparedRequest {
            request,
            path,
            method_desc,
        })
    }
}

struct PreparedRequest {
    request: tonic::Request<Bytes>,
    path: PathAndQuery,
    method_desc: prost_reflect::MethodDescriptor,
}

/// A lazy stream of server-streaming response messages as JSON values.
pub struct ResponseStream {
    inner: tonic::Streaming<DynamicMessage>,
}

impl ResponseStream {
    /// Get the next message from the stream, or `None` if the stream is done.
    pub async fn next(&mut self) -> Option<Result<Value, CallError>> {
        use futures_util::StreamExt;
        match self.inner.next().await {
            Some(Ok(msg)) => Some(
                serde_json::to_value(&msg).map_err(CallError::from),
            ),
            Some(Err(status)) => Some(Err(CallError::Grpc(status))),
            None => None,
        }
    }
}

pub struct ServiceInfo {
    pub name: String,
    pub methods: Vec<MethodInfo>,
}

pub struct MethodInfo {
    pub name: String,
    pub input_type: String,
    pub output_type: String,
}

// --- Codec internals ---

#[derive(Clone)]
struct DynamicCodec {
    method: prost_reflect::MethodDescriptor,
}

impl DynamicCodec {
    fn new(method: prost_reflect::MethodDescriptor) -> Self {
        Self { method }
    }
}

impl tonic::codec::Codec for DynamicCodec {
    type Encode = Bytes;
    type Decode = DynamicMessage;
    type Encoder = DynamicEncoder;
    type Decoder = DynamicDecoder;

    fn encoder(&mut self) -> Self::Encoder {
        DynamicEncoder
    }

    fn decoder(&mut self) -> Self::Decoder {
        DynamicDecoder {
            output_desc: self.method.output(),
        }
    }
}

#[derive(Clone)]
struct DynamicEncoder;

impl tonic::codec::Encoder for DynamicEncoder {
    type Item = Bytes;
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
struct DynamicDecoder {
    output_desc: prost_reflect::MessageDescriptor,
}

impl tonic::codec::Decoder for DynamicDecoder {
    type Item = DynamicMessage;
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
        let bytes = src.copy_to_bytes(remaining);
        let msg = DynamicMessage::decode(self.output_desc.clone(), bytes)
            .map_err(|e| tonic::Status::internal(format!("Failed to decode response: {}", e)))?;
        Ok(Some(msg))
    }
}
