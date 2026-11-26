use super::message::{S3EventKind::*, *};
use super::s3_create;
use crate::service::dynamodb::client::DynamodbClient;
use anyhow::{Context, Result};

// S3 also sends a one-off destination handshake without a Records array.
#[derive(serde::Deserialize)]
#[serde(untagged)]
enum QueueEvent {
    Notification(S3EventNotification),
    Test {
        #[serde(rename = "Event")]
        _event: TestEventName,
    },
}

#[derive(serde::Deserialize)]
enum TestEventName {
    #[serde(rename = "s3:TestEvent")]
    Test,
}

pub async fn handle_s3_message(text: &str, metadata_client: DynamodbClient) -> Result<()> {
    let message: QueueEvent =
        serde_json::from_str(text).context("invalid static-file storage event")?;
    let QueueEvent::Notification(notification) = message else {
        return Ok(());
    };
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

#[cfg(test)]
mod test {
    use super::QueueEvent;

    #[test]
    fn recognizes_the_s3_destination_handshake() {
        let sample = r#"{"Service":"Amazon S3","Event":"s3:TestEvent","Bucket":"files"}"#;
        assert!(matches!(
            serde_json::from_str(sample),
            Ok(QueueEvent::Test { .. })
        ));
        assert!(serde_json::from_str::<QueueEvent>(r#"{"Event":"unknown"}"#).is_err());
        assert!(serde_json::from_str::<QueueEvent>(r#"{"Bucket":"files"}"#).is_err());
    }
}
