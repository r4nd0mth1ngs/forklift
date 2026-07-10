//! The staging verifier Lambda: an S3 object-created event → `verify_and_promote`.
//!
//! The large-blob half of the split-verify decision (`store.rs`). A client `PUT`s a large
//! object straight to `staging/{session}/{hash}` via a presigned URL, which the control plane
//! never sees; S3 fires an object-created event, and this function runs the *same*
//! [`ObjectStore::verify_and_promote`] the control plane runs synchronously for small objects —
//! it stream-hashes the staged bytes and, only if `Blake3(bytes) == hash`, server-side-copies
//! them to the canonical key. Promotion is the single moment the object becomes fetchable
//! (invariant 1), and it is idempotent, so this racing the control plane's `commit_lift` on one
//! hash is safe.
//!
//! Same R4 blocking discipline as the control plane: `verify_and_promote` bridges S3 futures,
//! so it runs on a blocking thread. It needs only the object store — no ref store, no warehouse
//! id — because promotion is content-addressed, not warehouse-scoped.
//!
//! Build with `cargo build -p forklift-aws-lambda --features lambda --release`.

use aws_lambda_events::event::s3::S3Event;
use forklift_aws_lambda::aws::build_clients;
use forklift_aws_lambda::store::{ObjectStore, PromoteOutcome};
use forklift_aws_lambda::{config_from_env, AsyncBridge, S3ObjectStore};
use lambda_runtime::{run, service_fn, Error, LambdaEvent};
use tokio::sync::OnceCell;

/// The object store, built once per cold start (the bucket is warehouse-agnostic).
static OBJECTS: OnceCell<S3ObjectStore> = OnceCell::const_new();

/// Resolve the object store, building the S3 client and capturing the bridge on first use.
async fn objects() -> Result<&'static S3ObjectStore, Error> {
    OBJECTS
        .get_or_try_init(|| async {
            let (config, _routing) = config_from_env().map_err(Error::from)?;
            let (s3, _dynamodb) = build_clients(&config).await.map_err(Error::from)?;
            let bridge = AsyncBridge::current().map_err(Error::from)?;

            Ok(S3ObjectStore::new(s3, config.bucket, bridge))
        })
        .await
}

/// Verify-and-promote every staged object an S3 event names.
async fn handler(event: LambdaEvent<S3Event>) -> Result<(), Error> {
    let objects = objects().await?;

    for record in event.payload.records {
        let Some(key) = record.s3.object.key else {
            continue;
        };

        let Some((session, hash)) = parse_staging_key(&key) else {
            // The trigger should be scoped to the `staging/` prefix, but a stray event is
            // skipped rather than failed — a failure would have S3/EventBridge retry forever.
            eprintln!("verifier: ignoring an S3 event that is not a staging object: {}", key);
            continue;
        };

        let (session, hash) = (session.to_string(), hash.to_string());

        // A storage error propagates (the event retries); a semantic outcome is logged and the
        // event is consumed — a corrupt object was discarded, not promoted, and retrying it
        // would only rediscard it.
        let outcome =
            tokio::task::spawn_blocking(move || objects.verify_and_promote(&session, &hash))
                .await
                .map_err(|e| Error::from(format!("The verifier task panicked: {}", e)))?
                .map_err(Error::from)?;

        match outcome {
            PromoteOutcome::Promoted | PromoteOutcome::AlreadyPresent => {}
            PromoteOutcome::Missing => {
                eprintln!("verifier: {} vanished from staging before verification", key)
            }
            PromoteOutcome::Corrupt { actual } => {
                eprintln!("verifier: discarded corrupt staged object {} (hashes to {})", key, actual)
            }
        }
    }

    Ok(())
}

/// Parse a `staging/{session}/{hash}` key into its parts. `None` for any other key shape.
fn parse_staging_key(key: &str) -> Option<(&str, &str)> {
    let rest = key.strip_prefix("staging/")?;
    let (session, hash) = rest.split_once('/')?;

    // A well-formed staging key is exactly two more segments; a nested slash is not one.
    if session.is_empty() || hash.is_empty() || hash.contains('/') {
        return None;
    }

    Some((session, hash))
}

/// Multi-thread on purpose, exactly as the control plane: [`AsyncBridge`] refuses a
/// single-threaded runtime.
#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Error> {
    run(service_fn(handler)).await
}

#[cfg(test)]
mod tests {
    use super::parse_staging_key;

    #[test]
    fn a_staging_key_splits_into_session_and_hash() {
        assert_eq!(parse_staging_key("staging/lift-1/abc123"), Some(("lift-1", "abc123")));
    }

    #[test]
    fn a_non_staging_key_is_rejected() {
        assert_eq!(parse_staging_key("objects/abc123"), None);
        assert_eq!(parse_staging_key("staging/lift-1"), None, "no hash segment");
        assert_eq!(parse_staging_key("staging//abc"), None, "empty session");
        assert_eq!(parse_staging_key("staging/lift-1/"), None, "empty hash");
        assert_eq!(parse_staging_key("staging/lift-1/a/b"), None, "extra segment");
    }
}
