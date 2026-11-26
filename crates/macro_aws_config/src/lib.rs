#![deny(missing_docs)]

//! This crate creates a standard way to make AWS configs.

pub use aws_config::SdkConfig;
use macro_env_var::maybe_env_var;

maybe_env_var! {
    #[derive(Clone)]
    pub struct LocalAwsUrl;
}

maybe_env_var! {
    /// Browser-facing LocalStack endpoint, including any reverse-proxy path prefix.
    #[derive(Clone)]
    pub struct LocalAwsPublicUrl;
}

/// Select a dedicated SNS profile when notification delivery uses an AWS account
/// separate from the workspace queues and storage. Only SNS clients use the result.
pub async fn sns_sdk_config(workspace: &SdkConfig) -> std::io::Result<SdkConfig> {
    match std::env::var("MACRO_SNS_PROFILE") {
        Err(std::env::VarError::NotPresent) => Ok(workspace.clone()),
        Err(_) => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "invalid MACRO_SNS_PROFILE",
        )),
        Ok(profile) => named_profile_sdk_config(&profile).await,
    }
}

/// Resolve credentials and region exclusively from the selected AWS profile.
pub async fn named_profile_sdk_config(profile: &str) -> std::io::Result<SdkConfig> {
    use aws_config::meta::region::ProvideRegion;
    use aws_config::profile::{ProfileFileCredentialsProvider, ProfileFileRegionProvider};
    use std::io::{Error, ErrorKind};

    if profile.trim().is_empty() {
        return Err(Error::new(ErrorKind::InvalidInput, "empty AWS profile"));
    }
    let region = ProfileFileRegionProvider::builder()
        .profile_name(profile)
        .build()
        .region()
        .await
        .ok_or_else(|| Error::new(ErrorKind::InvalidInput, "AWS profile requires a region"))?;
    let credentials = ProfileFileCredentialsProvider::builder()
        .profile_name(profile)
        .build();
    Ok(aws_config::defaults(aws_config::BehaviorVersion::latest())
        .profile_name(profile)
        .region(region)
        .credentials_provider(credentials)
        .load()
        .await)
}

/// Creates an S3 client
#[cfg(feature = "s3")]
pub async fn s3_client() -> aws_sdk_s3::Client {
    let s3_config = aws_sdk_s3::config::Builder::from(&get_macro_aws_config().await)
        .force_path_style(is_local_aws())
        .build();
    aws_sdk_s3::Client::from_conf(s3_config)
}

/// Build the client used by document storage, preserving the default workspace
/// configuration unless a dedicated signing profile and endpoints are supplied.
#[cfg(feature = "s3")]
pub async fn document_s3_client() -> std::io::Result<aws_sdk_s3::Client> {
    use std::env::{VarError, var};
    use std::io::{Error, ErrorKind};
    let (profile, internal, public) = match (
        var("MACRO_DOCUMENT_S3_PROFILE"),
        var("MACRO_DOCUMENT_S3_ENDPOINT"),
        var("MACRO_DOCUMENT_S3_PUBLIC_URL"),
    ) {
        (Err(VarError::NotPresent), Err(VarError::NotPresent), Err(VarError::NotPresent)) => {
            return Ok(s3_client().await);
        }
        (Ok(profile), Ok(internal), Ok(public)) => (profile, internal, public),
        _ => {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "document storage needs a profile and both endpoints",
            ));
        }
    };
    for (endpoint, external) in [(&internal, false), (&public, true)] {
        let valid = url::Url::parse(endpoint).is_ok_and(|u| {
            u.host().is_some()
                && u.username().is_empty()
                && u.password().is_none()
                && u.query().is_none()
                && u.fragment().is_none()
                && (u.scheme() == "https" || (!external && u.scheme() == "http"))
                && (external || u.path() == "/")
        });
        if !valid {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "invalid document storage endpoint",
            ));
        }
    }
    let sdk = named_profile_sdk_config(&profile).await?;
    let config = aws_sdk_s3::config::Builder::from(&sdk)
        .endpoint_url(internal.trim_end_matches('/'))
        .force_path_style(true)
        .build();
    Ok(aws_sdk_s3::Client::from_conf(config))
}

/// Whether this service uses a dedicated document object store for download grants.
pub fn has_document_store() -> bool {
    std::env::var_os("MACRO_DOCUMENT_S3_PROFILE").is_some()
}

/// Sign a read for one object after its caller has authorized the document.
#[cfg(feature = "s3")]
pub async fn document_download_url(
    client: &aws_sdk_s3::Client,
    bucket: &str,
    key: &str,
    seconds: u64,
) -> Result<String, aws_sdk_s3::error::SdkError<aws_sdk_s3::operation::get_object::GetObjectError>>
{
    use aws_sdk_s3::presigning::PresigningConfig;
    let signing = PresigningConfig::expires_in(std::time::Duration::from_secs(seconds))
        .map_err(aws_sdk_s3::error::SdkError::construction_failure)?;
    let request = client
        .get_object()
        .bucket(bucket)
        .key(key)
        .presigned(signing)
        .await?;
    Ok(transform_aws_url(request.uri()))
}

/// Whether local development uses its document-storage fixture.
pub fn is_local_document_fixture() -> bool {
    is_local_aws() && !has_document_store()
}

fn replace_storage_prefix(url: &str, from: &str, to: &str) -> Option<String> {
    let tail = url
        .strip_prefix(from.trim_end_matches('/'))?
        .strip_prefix('/')?;
    Some(format!("{}/{tail}", to.trim_end_matches('/')))
}

fn document_storage_url(url: &str, internal: bool) -> Option<String> {
    let service = std::env::var("MACRO_DOCUMENT_S3_ENDPOINT").ok()?;
    let browser = std::env::var("MACRO_DOCUMENT_S3_PUBLIC_URL").ok()?;
    if internal {
        replace_storage_prefix(url, &browser, &service)
    } else {
        replace_storage_prefix(url, &service, &browser)
    }
}

/// Creates an SQS client
#[cfg(feature = "sqs")]
pub async fn sqs_client() -> aws_sdk_sqs::Client {
    aws_sdk_sqs::Client::new(&get_macro_aws_config().await)
}

/// Creates a aws_config to use.
/// If you provide `LOCAL_AWS_URL` environment variable we create a local aws
/// config with test credentials.
/// Otherwise we load normally.
pub async fn get_macro_aws_config() -> aws_config::SdkConfig {
    if let Some(local_aws_url) = LocalAwsUrl::new() {
        local_aws_config(local_aws_url.as_ref()).await
    } else {
        aws_config::defaults(aws_config::BehaviorVersion::latest())
            .region("us-east-1")
            .load()
            .await
    }
}

/// Creates an AWS SDK config pointed at a LocalStack endpoint.
pub async fn local_aws_config(local_aws_url: &str) -> aws_config::SdkConfig {
    aws_config::defaults(aws_config::BehaviorVersion::latest())
        .region("us-east-1")
        .test_credentials()
        .endpoint_url(local_aws_url)
        .load()
        .await
}

/// Returns if the aws config is local or not
pub fn is_local_aws() -> bool {
    LocalAwsUrl::new().is_some()
}

/// internal method to transform the local aws url
fn transform_local_url(url: &str, public_url: Option<&str>) -> String {
    // NOTE: it is ok to use expect as this is only run locally
    let parsed = url::Url::parse(url).expect("valid url");
    let host = parsed.host_str().unwrap();
    let port = parsed.port().unwrap_or(4566);
    let path = parsed.path();
    let query = parsed.query().map(|q| format!("?{q}")).unwrap_or_default();

    let origin = public_url
        .map(|url| url.trim_end_matches('/').to_owned())
        .unwrap_or_else(|| format!("http://localhost:{port}"));

    // Path-style LocalStack URLs generated inside Docker use `localstack` as
    // the host, which the browser on the host machine cannot resolve. Keep the
    // existing path (`/{bucket}/{key}`) and use the browser-facing origin.
    if host == "localstack" || host == "localhost" {
        return format!("{origin}{path}{query}");
    }

    // hostname should be in the form {asset}.localstack or {asset}.localhost;
    // any other endpoint (a test's own, by address) is reachable as it is.
    let Some(asset) = host
        .strip_suffix(".localstack")
        .or_else(|| host.strip_suffix(".localhost"))
    else {
        return url.to_owned();
    };

    format!("{origin}/{asset}{path}{query}")
}

/// Transforms a localstack url into one that will work within the app
/// For example, presigned urls for localstack come out as `http://{BUCKET_NAME}.localstack:{PORT}`
/// but we need them to be formulated as `http://localhost:{PORT}/bucket-name`.
/// `LOCAL_AWS_PUBLIC_URL`, when set by the local stack, supplies the browser-facing
/// endpoint so named instances can use their HTTPS storage proxy.
pub fn transform_aws_url(url: &str) -> String {
    if let Some(mapped) = document_storage_url(url, false) {
        return mapped;
    }
    if is_local_aws() {
        let public_url = LocalAwsPublicUrl::new();
        return transform_local_url(url, public_url.as_ref().map(|url| url.as_ref()));
    }
    url.to_string()
}

/// internal method to transform a browser-facing local url into one reachable
/// from inside the docker network
fn transform_internal_url(url: &str, local_aws_url: &str, public_url: Option<&str>) -> String {
    // NOTE: it is ok to use expect as this is only run locally
    let parsed = url::Url::parse(url).expect("valid url");
    let host = parsed.host_str().unwrap();
    let path = parsed.path();
    let query = parsed.query().map(|q| format!("?{q}")).unwrap_or_default();

    // The HTTPS app proxy exposes storage below a path prefix. Match only
    // that configured endpoint and remove its prefix before calling S3.
    if let Some(public) = public_url.and_then(|url| url::Url::parse(url).ok())
        && parsed.origin() == public.origin()
        && let Some(key) = path.strip_prefix(&format!("{}/", public.path().trim_end_matches('/')))
    {
        return format!("{}/{key}{query}", local_aws_url.trim_end_matches('/'));
    }

    // Browser-facing local URLs use `localhost`, which inside a container
    // resolves to the container itself. Use the SDK endpoint, including its
    // internal port, so service-to-service fetches reach LocalStack. Leave any other
    // host untouched.
    if host == "localhost" || host == "localstack" {
        return format!("{}{path}{query}", local_aws_url.trim_end_matches('/'));
    }

    url.to_string()
}

/// Transforms a browser-facing local URL into one reachable from inside the
/// app's own containers when fetching an object server-side.
///
/// The inverse of [`transform_aws_url`]: remove the configured public endpoint's
/// origin and proxy prefix so container clients reach LocalStack directly.
/// Legacy localhost URLs are also supported. No-op outside local AWS.
pub fn transform_aws_url_for_internal_fetch(url: &str) -> String {
    if let Some(mapped) = document_storage_url(url, true) {
        return mapped;
    }
    if let Some(local_aws_url) = LocalAwsUrl::new() {
        let public_url = LocalAwsPublicUrl::new();
        return transform_internal_url(
            url,
            local_aws_url.as_ref(),
            public_url.as_ref().map(|url| url.as_ref()),
        );
    }
    url.to_string()
}

#[cfg(test)]
mod test;
