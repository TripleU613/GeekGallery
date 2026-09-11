//! Cloudflare R2 backend, over R2's S3-compatible API.
//!
//! Objects are read by the browser straight from `R2_PUBLIC_BASE`
//! (media.geekgallery.com), never proxied through this app — a gallery
//! streaming every thumbnail through the Rust process would waste bandwidth on
//! bytes a CDN edge already caches.

use aws_sdk_s3::config::retry::RetryConfig;
use aws_sdk_s3::config::timeout::TimeoutConfig;
use aws_sdk_s3::config::{BehaviorVersion, Credentials, Region};
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::Client;

use super::Backend;

pub struct R2 {
    client: Client,
    bucket: String,
    public_base: String,
}

impl R2 {
    /// Build a client if every required variable is present.
    ///
    /// `Ok(None)` means "not configured, use local disk" — distinct from
    /// `Err`, which means configured but broken and worth logging loudly.
    pub async fn from_env() -> anyhow::Result<Option<Self>> {
        let (Ok(account), Ok(key_id), Ok(secret), Ok(bucket)) = (
            std::env::var("CF_ACCOUNT_ID"),
            std::env::var("R2_ACCESS_KEY_ID"),
            std::env::var("R2_SECRET_ACCESS_KEY"),
            std::env::var("R2_BUCKET"),
        ) else {
            return Ok(None);
        };

        if account.is_empty() || key_id.is_empty() || secret.is_empty() || bucket.is_empty() {
            return Ok(None);
        }

        // Without a public base URL the app would hand out unreachable links, so
        // treat a missing one as a configuration error rather than falling back.
        let public_base = std::env::var("R2_PUBLIC_BASE")
            .map_err(|_| anyhow::anyhow!("R2_PUBLIC_BASE must be set when R2 is configured"))?;

        let endpoint = format!("https://{account}.r2.cloudflarestorage.com");

        let cfg = aws_sdk_s3::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            // R2 ignores region but the SDK requires one.
            .region(Region::new("auto"))
            .endpoint_url(&endpoint)
            .credentials_provider(Credentials::new(
                key_id,
                secret,
                None,
                None,
                "geekgallery-env",
            ))
            // R2 does not support virtual-host-style addressing for these keys.
            .force_path_style(true)
            // Retries, because a single failed PUT used to lose an upload.
            //
            // An upload came back "failed to upload" with `R2 put ... failed:
            // dispatch failure` -- a transient DNS failure inside the container
            // network, at the same moment the Gemini sidecar could not resolve
            // gemini.google.com either. A dispatch failure is exactly the class of
            // error worth retrying: nothing reached R2, so there is nothing to be
            // idempotent about, and by then the upload has already cost a Gemini
            // image generation. The default policy gave up too early for a network
            // that hiccups.
            .retry_config(RetryConfig::standard().with_max_attempts(5))
            // A bounded per-attempt timeout, so a stalled connection is retried
            // rather than hanging the whole upload job behind it. Sized for a
            // 60MB video PUT from a host with an ordinary uplink, not for the
            // 12MB stills the numbers used to assume.
            .timeout_config(
                TimeoutConfig::builder()
                    .operation_attempt_timeout(std::time::Duration::from_secs(180))
                    .operation_timeout(std::time::Duration::from_secs(600))
                    .build(),
            )
            .build();

        Ok(Some(Self {
            client: Client::from_conf(cfg),
            bucket,
            public_base: public_base.trim_end_matches('/').to_string(),
        }))
    }

    /// Confirm the bucket is reachable and the credentials are accepted, so
    /// misconfiguration surfaces at startup instead of on a user's first upload.
    pub async fn check(&self) -> anyhow::Result<()> {
        self.client
            .head_bucket()
            .bucket(&self.bucket)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("cannot reach R2 bucket {}: {e}", self.bucket))?;
        Ok(())
    }
}

#[async_trait::async_trait]
impl Backend for R2 {
    async fn put(&self, key: &str, bytes: Vec<u8>, content_type: &str) -> anyhow::Result<()> {
        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(key)
            .content_type(content_type)
            // Immutable: keys are UUIDs, so a given key's bytes never change.
            .cache_control("public, max-age=31536000, immutable")
            .body(ByteStream::from(bytes))
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("R2 put {key} failed: {e}"))?;
        Ok(())
    }

    async fn get(&self, key: &str) -> anyhow::Result<Vec<u8>> {
        let resp = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("R2 get {key} failed: {e}"))?;
        let bytes = resp
            .body
            .collect()
            .await
            .map_err(|e| anyhow::anyhow!("R2 get {key}: could not read body: {e}"))?;
        Ok(bytes.to_vec())
    }

    async fn delete(&self, key: &str) {
        if let Err(e) = self
            .client
            .delete_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
        {
            tracing::warn!("R2 delete {key} failed: {e}");
        }
    }

    fn public_url(&self, key: &str) -> String {
        format!("{}/{}", self.public_base, key)
    }

    fn name(&self) -> String {
        format!("R2 ({} → {})", self.bucket, self.public_base)
    }
}
