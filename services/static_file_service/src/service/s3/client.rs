use anyhow::{Context, Result};
use aws_sdk_s3::{self, presigning::PresigningConfig};
use std::time::Duration;

#[derive(Debug)]
pub struct S3Client {
    inner: aws_sdk_s3::Client,
    storage_bucket: String,
    endpoints: Option<(String, String)>,
}

impl S3Client {
    pub fn new(inner: aws_sdk_s3::Client, storage_bucket: String) -> Self {
        S3Client {
            inner,
            storage_bucket,
            endpoints: None,
        }
    }

    /// Use the dedicated object store when all three Studio settings are present.
    /// Metadata and queues continue to use the workspace AWS configuration.
    pub async fn from_env(storage_bucket: String) -> Result<Self> {
        use std::env::{VarError, var};
        let settings = (
            var("MACRO_STATIC_S3_PROFILE"),
            var("MACRO_STATIC_S3_ENDPOINT"),
            var("MACRO_STATIC_S3_PUBLIC_URL"),
        );
        match settings {
            (Err(VarError::NotPresent), Err(VarError::NotPresent), Err(VarError::NotPresent)) => {
                Ok(Self::new(
                    macro_aws_config::s3_client().await,
                    storage_bucket,
                ))
            }
            (Ok(profile), Ok(internal), Ok(public)) => {
                let internal = internal.trim_end_matches('/');
                let public = public.trim_end_matches('/');
                anyhow::ensure!(
                    public.starts_with("https://")
                        && (internal.starts_with("http://") || internal.starts_with("https://")),
                    "invalid static S3 endpoints"
                );
                let sdk = macro_aws_config::named_profile_sdk_config(&profile).await?;
                let config = aws_sdk_s3::config::Builder::from(&sdk)
                    .endpoint_url(internal)
                    .force_path_style(true)
                    .build();
                Ok(Self {
                    inner: aws_sdk_s3::Client::from_conf(config),
                    storage_bucket,
                    endpoints: Some((format!("{internal}/"), format!("{public}/"))),
                })
            }
            _ => anyhow::bail!("static S3 requires a profile, endpoint and public URL together"),
        }
    }

    fn public_url(&self, signed: &str) -> Result<String> {
        if let Some((internal, public)) = &self.endpoints {
            // Keep the original escaped path and query byte-for-byte. The proxy
            // supplies the signed host when forwarding to this private endpoint.
            let path = signed
                .strip_prefix(internal.as_str())
                .context("unexpected static S3 signing endpoint")?;
            return Ok(format!("{public}{path}"));
        }
        Ok(macro_aws_config::transform_aws_url(signed))
    }

    #[tracing::instrument(skip(self))]
    pub async fn put_presigned_url(
        &self,
        key: String,
        content_type: String,
        content_length: Option<u64>,
    ) -> Result<String> {
        let duration = Duration::from_secs(2 * 60);
        let content_length = content_length
            .map(i64::try_from)
            .transpose()
            .context("content length is out of range")?;
        let presigned_url = self
            .inner
            .put_object()
            .key(key)
            .content_type(content_type)
            .set_content_length(content_length)
            .bucket(self.storage_bucket.clone())
            .presigned(PresigningConfig::expires_in(duration)?)
            .await
            .context("failed to create presigned url")?;

        self.public_url(presigned_url.uri())
    }

    #[tracing::instrument(skip(self))]
    pub async fn hard_delete_object(&self, key: String) -> Result<()> {
        self.inner
            .delete_object()
            .key(key)
            .bucket(self.storage_bucket.clone())
            .send()
            .await
            .context("failed to delete object")?;
        Ok(())
    }

    #[tracing::instrument(skip(self))]
    pub async fn get_presigned_url(&self, key: String, head_only: bool) -> Result<String> {
        let config = PresigningConfig::expires_in(std::time::Duration::from_secs(3600))
            .context("failed to create presigning config")?;
        let presigned_url = if head_only {
            self.inner
                .head_object()
                .bucket(&self.storage_bucket)
                .key(key)
                .presigned(config)
                .await
                .context("failed to sign HEAD")?
        } else {
            self.inner
                .get_object()
                .bucket(&self.storage_bucket)
                .key(key)
                .presigned(config)
                .await
                .context("failed to sign GET")?
        };
        self.public_url(presigned_url.uri())
    }

    #[tracing::instrument(skip(self), err)]
    pub async fn delete_objects_by_prefix(&self, prefix: &str) -> Result<()> {
        let mut continuation_token: Option<String> = None;

        loop {
            let mut req = self
                .inner
                .list_objects_v2()
                .bucket(&self.storage_bucket)
                .prefix(prefix);

            if let Some(token) = &continuation_token {
                req = req.continuation_token(token);
            }

            let response = req.send().await.context("failed to list objects")?;
            let keys: Vec<String> = response
                .contents()
                .iter()
                .filter_map(|obj| obj.key().map(String::from))
                .collect();

            if !keys.is_empty() {
                self.bulk_hard_delete_objects(keys).await;
            }

            match response.next_continuation_token() {
                Some(token) => continuation_token = Some(token.to_string()),
                None => break,
            }
        }

        Ok(())
    }

    #[tracing::instrument(skip(self), fields(count = keys.len()))]
    pub async fn bulk_hard_delete_objects(&self, keys: Vec<String>) -> Vec<Result<()>> {
        if keys.is_empty() {
            return Vec::new();
        }

        // S3 delete_objects has a limit of 1000 keys per request
        let chunk_size = 1000;
        let mut all_results = Vec::with_capacity(keys.len());

        for chunk in keys.chunks(chunk_size) {
            let mut object_identifiers = Vec::new();
            for key in chunk {
                object_identifiers.push(
                    aws_sdk_s3::types::ObjectIdentifier::builder()
                        .key(key.clone())
                        .build()
                        .expect("failed to build ObjectIdentifier"),
                );
            }

            let delete_request = aws_sdk_s3::types::Delete::builder()
                .set_objects(Some(object_identifiers))
                .build()
                .context("failed to build Delete request");

            match delete_request {
                Ok(delete_req) => {
                    match self
                        .inner
                        .delete_objects()
                        .bucket(self.storage_bucket.clone())
                        .delete(delete_req)
                        .send()
                        .await
                    {
                        Ok(output) => {
                            // Build a map of error keys to their error messages
                            let errors = output.errors();
                            let error_map: std::collections::HashMap<&str, String> = errors
                                .iter()
                                .filter_map(|e| {
                                    e.key().map(|key| {
                                        let msg =
                                            e.message().unwrap_or("unknown error").to_string();
                                        (key, msg)
                                    })
                                })
                                .collect();

                            // Log errors
                            for error in errors {
                                if let Some(key) = error.key() {
                                    tracing::warn!(
                                        key = key,
                                        error = ?error,
                                        "failed to delete object from S3"
                                    );
                                }
                            }

                            // Add results in the same order as the input keys
                            for key in chunk {
                                if let Some(error_msg) = error_map.get(key.as_str()) {
                                    all_results.push(Err(anyhow::anyhow!(
                                        "failed to delete {}: {}",
                                        key,
                                        error_msg
                                    )));
                                } else {
                                    all_results.push(Ok(()));
                                }
                            }
                        }
                        Err(e) => {
                            tracing::error!(error = ?e, "failed to delete batch of objects from S3");
                            // Add error for each key in this chunk
                            for _ in chunk {
                                all_results
                                    .push(Err(anyhow::anyhow!("batch delete failed: {}", e)));
                            }
                        }
                    }
                }
                Err(e) => {
                    let err_msg = e.to_string();
                    tracing::error!(error = ?e, "failed to build delete request");
                    for _ in chunk {
                        all_results.push(Err(anyhow::anyhow!(
                            "failed to build delete request: {}",
                            err_msg
                        )));
                    }
                }
            }
        }

        all_results
    }
}

#[cfg(test)]
mod test {
    use aws_sdk_s3::config::{BehaviorVersion, Credentials, Region};

    use super::*;

    fn client() -> S3Client {
        let config = aws_sdk_s3::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .region(Region::new("us-east-1"))
            .credentials_provider(Credentials::new("id", "secret", None, None, "test"))
            .build();
        S3Client::new(aws_sdk_s3::Client::from_conf(config), "bucket".to_string())
    }

    #[tokio::test]
    async fn a_declared_length_is_signed_so_storage_rejects_other_sizes() {
        let url = client()
            .put_presigned_url("key".to_string(), "image/png".to_string(), Some(5))
            .await
            .unwrap();

        assert!(
            url.to_lowercase()
                .contains("x-amz-signedheaders=content-length"),
            "{url}"
        );
    }

    #[tokio::test]
    async fn no_declared_length_leaves_the_size_unsigned() {
        let url = client()
            .put_presigned_url("key".to_string(), "image/png".to_string(), None)
            .await
            .unwrap();

        assert!(!url.to_lowercase().contains("content-length"), "{url}");
    }
}
