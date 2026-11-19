# Explicit retry policy

Status: accepted for the unpublished alpha, 16 November 2026.

The architecture's failure table describes safe backoff/retry behavior. The prototype deliberately made one attempt. Keep that observable default and expose the complete bounded behavior through an explicit RetryPolicy on generated requests.

This keeps an ordinary Rust call and an agent action's request budget predictable. It also avoids silently replaying streams, credential/session operations or short-lived remote identifiers. The HTTP transport itself keeps retries disabled.

Reviewed reads can recover from transport/timeouts and HTTP 5xx. Confirmed ordinary JSON 429 refusals may retry only with an additional write opt-in. Unknown writes, target migration and temporal references never replay. Server delay, elapsed time, jitter and attempt caps are enforced by one policy. Streaming downloads do not retry into partially written local destinations.

An application can attach RetryPolicy::default for bounded read recovery without changing other requests. The agent executor must authorize and budget any retries it enables; metadata is not a permission grant. Durable polling recovery remains a separate journal/runtime contract.
