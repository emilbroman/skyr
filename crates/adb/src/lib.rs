use std::time::Duration;

use aws_sdk_s3::error::ProvideErrorMetadata;
use aws_sdk_s3::{
    Client as S3Client,
    config::{Builder as S3ConfigBuilder, Credentials, Region},
    error::SdkError,
    operation::{
        create_bucket::CreateBucketError, get_object::GetObjectError, head_object::HeadObjectError,
        list_objects_v2::ListObjectsV2Error, put_object::PutObjectError,
    },
    presigning::{PresigningConfig, PresigningConfigError},
    primitives::ByteStream,
    types::BucketCannedAcl,
};
use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};
use thiserror::Error;
use url::Url;

const DEFAULT_REGION: &str = "us-east-1";
const DEFAULT_MEDIA_TYPE: &str = "application/octet-stream";

/// Maximum allowed presigned URL expiration (1 hour).
const MAX_PRESIGN_EXPIRATION: Duration = Duration::from_secs(3600);

pub use aws_sdk_s3::primitives::ByteStream as ArtifactBody;

#[derive(Debug, Clone)]
pub struct ArtifactHeader {
    namespace: String,
    name: String,
    media_type: String,
}

impl ArtifactHeader {
    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn media_type(&self) -> &str {
        &self.media_type
    }
}

#[derive(Debug)]
pub struct Artifact {
    pub header: ArtifactHeader,
    pub body: ByteStream,
}

#[derive(Clone)]
pub struct Client {
    s3: S3Client,
    presign_s3: S3Client,
    bucket: String,
    external_url: Option<String>,
    force_path_style: bool,
}

#[derive(Default)]
pub struct ClientBuilder {
    bucket: Option<String>,
    endpoint_url: Option<String>,
    external_url: Option<String>,
    region: Option<String>,
    access_key_id: Option<String>,
    secret_access_key: Option<String>,
    force_path_style: bool,
    create_bucket_if_missing: bool,
}

impl ClientBuilder {
    pub fn new() -> Self {
        Self {
            force_path_style: true,
            create_bucket_if_missing: false,
            ..Self::default()
        }
    }

    pub fn bucket(mut self, bucket: impl Into<String>) -> Self {
        self.bucket = Some(bucket.into());
        self
    }

    pub fn endpoint_url(mut self, endpoint_url: impl Into<String>) -> Self {
        self.endpoint_url = Some(endpoint_url.into());
        self
    }

    pub fn external_url(mut self, external_url: impl Into<String>) -> Self {
        self.external_url = Some(external_url.into());
        self
    }

    pub fn region(mut self, region: impl Into<String>) -> Self {
        self.region = Some(region.into());
        self
    }

    pub fn access_key_id(mut self, access_key_id: impl Into<String>) -> Self {
        self.access_key_id = Some(access_key_id.into());
        self
    }

    pub fn secret_access_key(mut self, secret_access_key: impl Into<String>) -> Self {
        self.secret_access_key = Some(secret_access_key.into());
        self
    }

    pub fn force_path_style(mut self, force_path_style: bool) -> Self {
        self.force_path_style = force_path_style;
        self
    }

    pub fn create_bucket_if_missing(mut self, create_bucket_if_missing: bool) -> Self {
        self.create_bucket_if_missing = create_bucket_if_missing;
        self
    }

    pub async fn build(self) -> Result<Client, ConnectError> {
        let bucket = self
            .bucket
            .ok_or_else(|| ConnectError::MissingConfig("bucket".to_owned()))?;

        let region = Region::new(self.region.unwrap_or_else(|| DEFAULT_REGION.to_owned()));

        let credentials = match (self.access_key_id, self.secret_access_key) {
            (Some(access_key_id), Some(secret_access_key)) => Some(Credentials::new(
                access_key_id,
                secret_access_key,
                None,
                None,
                "adb",
            )),
            (None, None) => None,
            _ => {
                return Err(ConnectError::MissingConfig(
                    "both access_key_id and secret_access_key".to_owned(),
                ));
            }
        };

        let external_url = self.external_url.or_else(|| self.endpoint_url.clone());

        let s3 = build_s3_client(
            &region,
            credentials.clone(),
            self.endpoint_url,
            self.force_path_style,
        );

        let presign_s3 = build_s3_client(
            &region,
            credentials,
            external_url.clone(),
            self.force_path_style,
        );

        let client = Client {
            s3,
            presign_s3,
            bucket,
            external_url,
            force_path_style: self.force_path_style,
        };

        if self.create_bucket_if_missing {
            client.create_bucket_if_missing().await?;
        }

        Ok(client)
    }
}

fn build_s3_client(
    region: &Region,
    credentials: Option<Credentials>,
    endpoint_url: Option<String>,
    force_path_style: bool,
) -> S3Client {
    let mut builder = S3ConfigBuilder::new()
        .region(region.clone())
        .force_path_style(force_path_style);

    if let Some(endpoint_url) = endpoint_url {
        builder = builder.endpoint_url(endpoint_url);
    }

    if let Some(credentials) = credentials {
        builder = builder.credentials_provider(credentials);
    }

    S3Client::from_conf(builder.build())
}

/// Validate that a media type string does not contain control characters
/// (newlines, carriage returns, null bytes, etc.) that could enable header
/// injection when passed to S3 `content_type()`.
fn validate_media_type(media_type: &str) -> Result<(), WriteError> {
    if media_type
        .bytes()
        .any(|b| b.is_ascii_control() && b != b'\t')
    {
        return Err(WriteError::InvalidMediaType(media_type.to_owned()));
    }
    Ok(())
}

impl Client {
    pub async fn create_bucket_if_missing(&self) -> Result<(), ConnectError> {
        let result = self.s3.head_bucket().bucket(&self.bucket).send().await;
        if result.is_ok() {
            return Ok(());
        }

        let create_result = self
            .s3
            .create_bucket()
            .bucket(&self.bucket)
            .acl(BucketCannedAcl::Private)
            .send()
            .await;

        match create_result {
            Ok(_) => Ok(()),
            Err(error) => match &error {
                SdkError::ServiceError(service_error)
                    if service_error.err().is_bucket_already_owned_by_you()
                        || service_error.err().is_bucket_already_exists() =>
                {
                    Ok(())
                }
                _ => Err(ConnectError::CreateBucket(error.into())),
            },
        }
    }

    pub async fn write(
        &self,
        namespace: impl AsRef<str>,
        name: impl AsRef<str>,
        media_type: Option<&str>,
        body: ByteStream,
    ) -> Result<ArtifactHeader, WriteError> {
        let namespace = namespace.as_ref();
        let name = name.as_ref();
        let media_type = media_type.unwrap_or(DEFAULT_MEDIA_TYPE);
        validate_media_type(media_type)?;
        let key = key(namespace, name);

        let result = self
            .s3
            .put_object()
            .bucket(&self.bucket)
            .key(key)
            .if_none_match("*")
            .content_type(media_type)
            .body(body)
            .send()
            .await;

        match result {
            Ok(_) => Ok(ArtifactHeader {
                namespace: namespace.to_owned(),
                name: name.to_owned(),
                media_type: media_type.to_owned(),
            }),
            Err(error) => match error {
                SdkError::ServiceError(service_error)
                    if matches!(
                        service_error.err().code(),
                        Some("PreconditionFailed" | "ConditionalRequestConflict")
                    ) =>
                {
                    Err(WriteError::AlreadyExists {
                        namespace: namespace.to_owned(),
                        name: name.to_owned(),
                    })
                }
                other => Err(WriteError::PutObject(other.into())),
            },
        }
    }

    pub async fn read(
        &self,
        namespace: impl AsRef<str>,
        name: impl AsRef<str>,
    ) -> Result<Option<Artifact>, ReadError> {
        let namespace = namespace.as_ref();
        let name = name.as_ref();
        let key = key(namespace, name);

        let result = self
            .s3
            .get_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await;

        match result {
            Ok(response) => Ok(Some(Artifact {
                header: ArtifactHeader {
                    namespace: namespace.to_owned(),
                    name: name.to_owned(),
                    media_type: response
                        .content_type
                        .unwrap_or_else(|| DEFAULT_MEDIA_TYPE.to_owned()),
                },
                body: response.body,
            })),
            Err(error) => match error {
                SdkError::ServiceError(service_error) if service_error.err().is_no_such_key() => {
                    Ok(None)
                }
                other => Err(ReadError::GetObject(other.into())),
            },
        }
    }

    pub async fn read_to_bytes(
        &self,
        namespace: impl AsRef<str>,
        name: impl AsRef<str>,
    ) -> Result<Option<Vec<u8>>, ReadError> {
        let Some(artifact) = self.read(namespace, name).await? else {
            return Ok(None);
        };

        let collected = artifact
            .body
            .collect()
            .await
            .map_err(|error| ReadError::ReadBody(error.to_string()))?;
        Ok(Some(collected.to_vec()))
    }

    pub async fn read_header(
        &self,
        namespace: impl AsRef<str>,
        name: impl AsRef<str>,
    ) -> Result<Option<ArtifactHeader>, HeadError> {
        let namespace = namespace.as_ref();
        let name = name.as_ref();
        let key = key(namespace, name);

        let result = self
            .s3
            .head_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await;

        match result {
            Ok(response) => Ok(Some(ArtifactHeader {
                namespace: namespace.to_owned(),
                name: name.to_owned(),
                media_type: response
                    .content_type
                    .unwrap_or_else(|| DEFAULT_MEDIA_TYPE.to_owned()),
            })),
            Err(error) => match error {
                SdkError::ServiceError(service_error)
                    if matches!(service_error.err().code(), Some("NotFound" | "NoSuchKey")) =>
                {
                    Ok(None)
                }
                other => Err(HeadError::HeadObject(other.into())),
            },
        }
    }

    pub async fn list(&self, namespace: impl AsRef<str>) -> Result<Vec<ArtifactHeader>, ListError> {
        let namespace = namespace.as_ref();
        let prefix = format!("{namespace}/");
        let mut continuation_token = None;
        let mut names = Vec::new();

        loop {
            let mut request = self
                .s3
                .list_objects_v2()
                .bucket(&self.bucket)
                .prefix(&prefix)
                .max_keys(1000);

            if let Some(token) = continuation_token.take() {
                request = request.continuation_token(token);
            }

            let response = request
                .send()
                .await
                .map_err(|error| ListError::ListObjects(error.into()))?;

            if let Some(contents) = response.contents {
                for object in contents {
                    let Some(key) = object.key else {
                        continue;
                    };

                    if let Some(name) = name_from_key(namespace, &key) {
                        names.push(name);
                    }
                }
            }

            if !response.is_truncated.unwrap_or(false) {
                break;
            }

            continuation_token = response.next_continuation_token;
            if continuation_token.is_none() {
                break;
            }
        }

        let mut headers = Vec::with_capacity(names.len());
        for name in names {
            match self.read_header(namespace, name.as_str()).await {
                Ok(Some(header)) => headers.push(header),
                Ok(None) => {
                    // Artifact was deleted between list and head; skip it.
                }
                Err(error) => {
                    tracing::warn!(
                        namespace,
                        name,
                        %error,
                        "skipping artifact: failed to read header during list",
                    );
                }
            }
        }

        Ok(headers)
    }

    pub async fn presign_read_url(
        &self,
        namespace: impl AsRef<str>,
        name: impl AsRef<str>,
        expires_in: Duration,
    ) -> Result<String, PresignError> {
        if expires_in > MAX_PRESIGN_EXPIRATION {
            return Err(PresignError::ExpirationTooLong {
                requested: expires_in,
                max: MAX_PRESIGN_EXPIRATION,
            });
        }
        let key = key(namespace.as_ref(), name.as_ref());
        let config = PresigningConfig::expires_in(expires_in)?;
        let request = self
            .presign_s3
            .get_object()
            .bucket(&self.bucket)
            .key(key)
            .presigned(config)
            .await
            .map_err(PresignError::Presign)?;
        Ok(request.uri().to_string())
    }

    pub fn read_url(
        &self,
        namespace: impl AsRef<str>,
        name: impl AsRef<str>,
    ) -> Result<String, UrlError> {
        let endpoint_url = self
            .external_url
            .as_ref()
            .ok_or(UrlError::MissingEndpoint)?;
        let key = key(namespace.as_ref(), name.as_ref());

        let encoded_key = encode_key_path(&key);

        if self.force_path_style {
            let mut endpoint = endpoint_url.trim_end_matches('/').to_owned();
            endpoint.push('/');
            endpoint.push_str(&self.bucket);
            endpoint.push('/');
            endpoint.push_str(&encoded_key);
            return Ok(endpoint);
        }

        let mut endpoint = Url::parse(endpoint_url).map_err(UrlError::Parse)?;
        let host = endpoint.host_str().ok_or(UrlError::MissingHost)?.to_owned();
        endpoint
            .set_host(Some(&format!("{}.{}", self.bucket, host)))
            .map_err(|_| UrlError::InvalidHost)?;
        let path = endpoint.path().trim_end_matches('/');

        let mut url = endpoint.origin().ascii_serialization();
        if !path.is_empty() && path != "/" {
            url.push_str(path);
        }
        url.push('/');
        url.push_str(&encoded_key);
        Ok(url)
    }
}

#[derive(Debug, Error)]
pub enum ConnectError {
    #[error("missing adb config: {0}")]
    MissingConfig(String),

    #[error("failed to create bucket: {0}")]
    CreateBucket(#[from] Box<SdkError<CreateBucketError>>),
}

#[derive(Debug, Error)]
pub enum WriteError {
    #[error("artifact already exists: {namespace}/{name}")]
    AlreadyExists { namespace: String, name: String },

    #[error("invalid media type (contains control characters): {0:?}")]
    InvalidMediaType(String),

    #[error("failed to write artifact object: {0}")]
    PutObject(#[from] Box<SdkError<PutObjectError>>),
}

#[derive(Debug, Error)]
pub enum ReadError {
    #[error("failed to read artifact object: {0}")]
    GetObject(#[from] Box<SdkError<GetObjectError>>),

    #[error("failed to read artifact body: {0}")]
    ReadBody(String),
}

#[derive(Debug, Error)]
pub enum HeadError {
    #[error("failed to read artifact metadata: {0}")]
    HeadObject(#[from] Box<SdkError<HeadObjectError>>),
}

#[derive(Debug, Error)]
pub enum ListError {
    #[error("failed to list artifact objects: {0}")]
    ListObjects(#[from] Box<SdkError<ListObjectsV2Error>>),
}

#[derive(Debug, Error)]
pub enum PresignError {
    #[error("invalid presign expiration: {0}")]
    InvalidExpiry(#[from] PresigningConfigError),

    #[error("presign expiration too long: requested {requested:?}, maximum allowed is {max:?}")]
    ExpirationTooLong { requested: Duration, max: Duration },

    #[error("failed to create presigned URL: {0}")]
    Presign(#[source] SdkError<GetObjectError>),
}

#[derive(Debug, Error)]
pub enum UrlError {
    #[error("failed to build object URL because adb endpoint_url is not configured")]
    MissingEndpoint,

    #[error("failed to parse adb endpoint_url: {0}")]
    Parse(#[source] url::ParseError),

    #[error("failed to parse adb endpoint_url: missing host")]
    MissingHost,

    #[error("failed to build adb endpoint_url: invalid host")]
    InvalidHost,
}

fn key(namespace: &str, name: &str) -> String {
    format!("{namespace}/{name}")
}

fn name_from_key(namespace: &str, key: &str) -> Option<String> {
    key.strip_prefix(namespace)?
        .strip_prefix('/')
        .map(str::to_owned)
}

/// Percent-encoding set for embedding S3 object keys into URL paths.
///
/// Preserves characters that are safe in URL path segments per RFC 3986
/// (`pchar = unreserved / sub-delims / ":" / "@"`) plus the `/` separator,
/// and percent-encodes everything else (controls, whitespace, fragment/query
/// delimiters, and bracket/brace/backtick characters that some servers reject).
const KEY_PATH_ENCODE_SET: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'\\')
    .add(b'^')
    .add(b'`')
    .add(b'{')
    .add(b'|')
    .add(b'}');

fn encode_key_path(key: &str) -> String {
    utf8_percent_encode(key, KEY_PATH_ENCODE_SET).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_concatenates_raw_namespace_and_name() {
        assert_eq!(
            key(
                "Demo/HelloService::main",
                "ip-9283206015f9865eae5471979fe60583"
            ),
            "Demo/HelloService::main/ip-9283206015f9865eae5471979fe60583",
        );
    }

    #[test]
    fn name_from_key_strips_namespace_prefix() {
        let key = "Demo/HelloService::main/ip-9283206015f9865eae5471979fe60583";
        assert_eq!(
            name_from_key("Demo/HelloService::main", key).as_deref(),
            Some("ip-9283206015f9865eae5471979fe60583"),
        );
    }

    #[test]
    fn name_from_key_rejects_non_matching_prefix() {
        assert_eq!(name_from_key("Other", "Demo/foo"), None);
        assert_eq!(name_from_key("Demo", "DemoFoo/bar"), None);
    }

    #[test]
    fn encode_key_path_preserves_path_safe_characters() {
        assert_eq!(
            encode_key_path("Demo/HelloService::main/ip-9283206015f9865eae5471979fe60583"),
            "Demo/HelloService::main/ip-9283206015f9865eae5471979fe60583",
        );
    }

    #[test]
    fn encode_key_path_encodes_unsafe_characters() {
        assert_eq!(encode_key_path("a b"), "a%20b");
        assert_eq!(encode_key_path("a?b"), "a%3Fb");
        assert_eq!(encode_key_path("a#b"), "a%23b");
        assert_eq!(encode_key_path("100%"), "100%25");
    }
}
