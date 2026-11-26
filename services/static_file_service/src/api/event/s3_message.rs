use super::message::{S3EventKind::*, *};
use super::s3_create;
use crate::service::dynamodb::client::DynamodbClient;
use anyhow::{Context, Result};

pub async fn handle_s3_message(text: &str, metadata_client: DynamodbClient) -> Result<()> {
    let notification: S3EventNotification =
        serde_json::from_str(text).context("invalid static-file storage event")?;
    for event in notification.records {
        match event.event_kind {
            ObjectCreatedAll
            | ObjectCreatedCompleteMultipartUpload
            | ObjectCreatedPut
            | ObjectCreatedCopy
            | ObjectCreatedPost => {
                s3_create::handle_s3_create(event, metadata_client.clone()).await?;
            }
            _ => {}
        }
    }
    Ok(())
}
