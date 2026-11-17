# Full implementation plan

See tasks/goal.md for completion gates and docs/ARCHITECTURE.md for module boundaries.

1. bot-runtime: getMe/getUpdates/webhook info, typed contexts/router, bounded per-chat polling, graceful stop and failure recovery.
2. protocol: snapshot official API documentation, build a reproducible schema importer/generator, classify method effects and verify all types/methods.
3. client and test-kit: full request surface, rich text/entities, upload/download variants, safe retry metadata, limits, observation hooks and the public in-memory scripted transport.
4. journal: SQLite operation/update records, atomic claims, payload hashes, authorization ownership, durable acknowledgement and retention.
5. bot-runtime: authenticated webhooks, durable ingestion, duplicate handling and multiple-executor safety.
6. agent-runtime: shared authorized executor, CLI JSON and MCP stdio, file handles, scoped grants and cost/quantity limits.
7. mtproto: evaluate maintained backend, implement separate client/session facade, common operations, raw access and recovery.
8. release: checked examples, recipes/migration guide, agent-task evaluation, comparable benchmarks, compatibility matrix, audit and packaging.

Each module has a concrete spec before its implementation. Use small verified commits, keep the shared request path conservative, and update tasks/todo.md with evidence. The full goal remains active across milestones.

Current M3 order: shared typed MT tool dispatch and opt-in extended schemas /
embedded CLI/MCP adapters (SPEC-mtproto-agent-tools.md), then trusted standalone
MT profile bootstrap and executable lifecycle fixtures; only then close M3 and
run Q1–Q4. The original Bot v1 discovery remains unchanged.
