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

M3 is complete locally: shared typed MT dispatch/extended schemas, trusted Host
v2 and standalone CLI/MCP lifecycle fixtures. The original Bot v1 discovery is
unchanged. Q3's chrono remediation, inherited hardened dependency graph and local
source-candidate acceptance are complete. Windows MSRV1.89/current1.100 runtime
and feature matrices pass; Linux x86_64 GNU repeats both compiler matrices,
stable lint and relocated source consumer acceptance. Both macOS cells remain.
Local normalized private-package preparation is also accepted on Windows1.100:
eight exact private registry nodes, offline independent runtime, models and lock.
The user chose to keep original code closed. Private licensing is recorded
alongside third-party licenses and publish=false. The closed candidate 94269339316a
passed source ZIP and eight-package local registry acceptance on Windows.

Q1 task documentation and the shared command/callback example have local
MSRV/stable acceptance. The first-check evaluation passed 27 of 30 tasks and
the MCP archive evaluation passed ten questions. Q1 still needs the observed
new-human live timing. Q2 benchmark and Q4 source-review evidence are recorded
in tasks/todo.md. Both native macOS cells remain open under Q3.
