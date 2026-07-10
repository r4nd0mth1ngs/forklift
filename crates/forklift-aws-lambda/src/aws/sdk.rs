//! Shared SDK-error plumbing for the S3 and DynamoDB stores.
//!
//! The two traits return `Result<_, String>`, and the [`Head`](crate::Head) turns every
//! `Err` into a `500`. The *semantic* outcomes a store must distinguish — a CAS conflict, a
//! not-found, an already-present, a corrupt promote — are trait enum variants, **not**
//! errors: `head.rs` branches on them. So the whole job of error mapping here is to notice
//! the specific SDK failure that corresponds to one of those semantic outcomes (a `404`, a
//! `412` precondition failure, a DynamoDB `ConditionalCheckFailedException`) and hand back
//! the matching variant, while every *other* SDK failure becomes an opaque `Err(String)`
//! that reads as `500` — exactly as the in-memory fakes behave for equivalent inputs.
//!
//! Two primitives do that work: [`http_status`], which reads the transport status a store
//! keys those decisions off (`404`/`412`), and [`describe`], which renders a genuine failure
//! into a message. Neither ever surfaces credentials — the SDK error chain carries an AWS
//! error code and request id, never the signing key.

use aws_smithy_runtime_api::client::orchestrator::HttpResponse;
use aws_smithy_runtime_api::client::result::SdkError;
use aws_smithy_types::error::display::DisplayErrorContext;

/// The HTTP status of a failed SDK call, when the failure reached the service and came back
/// with a response (a modeled or unmodeled service error). `None` for a failure that never
/// got a response — a dispatch, timeout, or construction error — which is always a genuine
/// `500`, never a semantic outcome.
///
/// Stores key their semantic branches off this: a `404` on a `GET`/`HEAD` is "absent"
/// (`Ok(None)` / `Ok(false)`), and a `412` on an `If-None-Match: *` write is "already there"
/// — not an error either.
pub(crate) fn http_status<E>(err: &SdkError<E, HttpResponse>) -> Option<u16> {
    err.raw_response().map(|response| response.status().as_u16())
}

/// Whether an S3 error is a `404` — the key is absent. Used to fold a missing object into
/// `Ok(None)`/`Ok(false)` rather than an error, mirroring the fake's `HashMap` miss.
pub(crate) fn is_not_found<E>(err: &SdkError<E, HttpResponse>) -> bool {
    http_status(err) == Some(404)
}

/// Whether an S3 error is a `412 Precondition Failed` — the response to an
/// `If-None-Match: *` write when the object already exists. This is the conditional-write
/// CAS the byte plane leans on: it is how `put_verified` reports `AlreadyPresent` and how
/// `verify_and_promote` serializes two racing promoters onto one canonical key.
pub(crate) fn is_precondition_failed<E>(err: &SdkError<E, HttpResponse>) -> bool {
    http_status(err) == Some(412)
}

/// Render a genuine SDK failure into the opaque `Err(String)` the head reports as `500`.
/// [`DisplayErrorContext`] walks the whole source chain (AWS error code, message, request
/// id) — diagnostic, and free of secrets, which the SDK never puts in an error.
pub(crate) fn describe<E, R>(operation: &str, err: SdkError<E, R>) -> String
where
    SdkError<E, R>: std::error::Error,
{
    format!("{} failed: {}", operation, DisplayErrorContext(&err))
}
