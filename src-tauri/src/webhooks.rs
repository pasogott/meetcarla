use anyhow::Result;
use hmac::{Hmac, Mac};
use sha2::Sha256;

use crate::database::Database;
use crate::types::Webhook;

type HmacSha256 = Hmac<Sha256>;

pub async fn dispatch_webhook_event(
    database: &Database,
    event_type: &str,
    payload: &serde_json::Value,
) -> Result<()> {
    let webhooks = database.list_webhooks_for_event(event_type)?;
    if webhooks.is_empty() {
        return Ok(());
    }

    let payload_str = serde_json::to_string(payload)?;

    for webhook in webhooks {
        let (response_status, response_body, success) =
            send_to_webhook(&webhook, event_type, &payload_str).await;

        let _ = database.save_webhook_delivery(
            &webhook.id,
            event_type,
            &payload_str,
            response_status,
            response_body.as_deref(),
            success,
        );
    }

    Ok(())
}

pub async fn dispatch_to_webhook(webhook: &Webhook, payload: &serde_json::Value) -> Result<()> {
    let payload_str = serde_json::to_string(payload)?;
    let _ = send_to_webhook(webhook, "webhook.test", &payload_str).await;
    Ok(())
}

async fn send_to_webhook(
    webhook: &Webhook,
    event_type: &str,
    payload_str: &str,
) -> (Option<i32>, Option<String>, bool) {
    let signature = webhook
        .secret
        .as_deref()
        .map(|secret| compute_signature(secret, payload_str));

    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
    {
        Ok(c) => c,
        Err(err) => return (None, Some(err.to_string()), false),
    };

    let mut request = client
        .post(&webhook.url)
        .header("Content-Type", "application/json")
        .header("X-Carla-Event", event_type)
        .body(payload_str.to_string());

    if let Some(sig) = &signature {
        request = request.header("X-Carla-Signature", format!("sha256={sig}"));
    }

    match request.send().await {
        Ok(response) => {
            let status = response.status().as_u16() as i32;
            let body = response.text().await.unwrap_or_default();
            let ok = (200..300).contains(&(status as u16));
            (Some(status), Some(body), ok)
        }
        Err(err) => (None, Some(err.to_string()), false),
    }
}

fn compute_signature(secret: &str, payload: &str) -> String {
    let mut mac =
        HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key length");
    mac.update(payload.as_bytes());
    let result = mac.finalize();
    hex::encode(result.into_bytes())
}
