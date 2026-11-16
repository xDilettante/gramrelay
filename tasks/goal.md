# Full completion goal

User instruction: "Ставь цель до полного завершения" (16 November 2026).

Deliver the complete architecture in docs/ARCHITECTURE.md, in module dependency order. Do not mark the goal complete at prototype, Bot API beta, or an intermediate milestone.

## Completion gates

1. All methods and types of the pinned Bot API snapshot have a reviewed, reproducibly generated Rust surface. Normal application code retains ergonomic helpers.
2. Polling and authenticated webhooks feed bounded execution, preserve per-chat admission order and acknowledge only successfully processed or durably stored updates. Shutdown, overload, duplicates and recovery are tested.
3. Persistent operation/update journals enforce atomic claims, payload consistency, ownership, retained results and Unknown recovery without blindly replaying external effects.
4. CLI and MCP share one authorized executor, typed schemas and safe outputs. Grants, bot/chat scopes, file handles, cached-result access, budgets and no raw-call bypass are tested.
5. Rich text, files, response compatibility, optional integrations, examples, versioned agent docs, migration notes and dependency policy are finished.
6. A separate MTProto client has a documented backend choice, session/auth lifecycle, common operations and low-level access, recovery and mock tests. It reuses maintained protocol/crypto implementations.
7. Reproducible correctness, safety and performance checks meet the architecture targets. Cross-platform and MSRV claims have evidence; release artifacts and local packaging are ready.

Actual registry publication, remote push and live Telegram actions require explicit authorization. Their absence must not block local implementation, tests, packaging or documentation. Performance targets are not achieved until measured. A lack of test credentials is not a reason to stop mock-driven development.
