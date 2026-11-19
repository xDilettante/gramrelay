# Host-authorized shared agent tools

Status: accepted for A1; execution/storage integration is incremental.

Rust, CLI and MCP must share one finite typed action surface. Generated Bot API
coverage is not a permission grant, and Telegram message contents cannot supply
principal, profile credentials, grants, API endpoints or confirmation.

The host authenticates a Principal and installs one current Grant per
principal/profile pair. A grant binds an expected bot identity, explicit actions,
chat/file scopes, expiry, confirmation policy and stable budget identity. One
grant avoids ambiguous unions that could select a different quota. Immutable
validated Request values keep approved intent unchanged. Numeric Telegram IDs
are canonical decimal JSON strings, including signed 64-bit extremes.

Approval is an in-process, non-deserializable capability. It binds the issuing
PolicyStore instance, principal, profile, monotonic grant revision, the entire
normalized versioned request including operation_id, and expiry. Replacement or
revoke/re-add invalidates it. Reusing a confirmation under a new operation key
would authorize a second write, so the operation key belongs in the fingerprint.
Confirmed NotSent retries retain the original key and intent.

PolicyStore::dry_run has no I/O or durable side effects and is never an execution
permit. The current policy must be checked again at admission and disclosure.
Callback and operation lookup need trusted observed/persisted scope, so the first
policy-only increment rejects them with ContextRequired rather than trusting
scope claimed in arguments. File grants alone do not prove snapshot availability.
Budget configuration alone does not enforce a quota. Subsequent A1 increments
add the durable binding/admission and reference/file registries from
SPEC-executor.md before exposing executable CLI/MCP actions.

Strict parsing rejects unknown fields, positional array aliases, invalid
versions, noncanonical IDs and inappropriate operation_id fields. Untrusted
adapters use bounded Request::from_json. Debug and executor error chains carry
no input content. Tests cover privilege injection, current policy, approval
replay, revocation, expiry, numeric precision and JSON boundary aliases.
