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
Continue all Q1–Q4 documentation, evaluation, measurement, platform and security
acceptance. The user chose to keep original code closed; record private licensing
separately, preserving third-party licenses and publish=false. This does not
remove or weaken any remaining completion gate.

Q1 task documentation and the shared command/callback example now have local
MSRV/stable acceptance. The 30-task corpus remains draft, with no LLM score.
Its feasibility review exposed the missing public one-shot AsyncRead upload;
implement that separate client capability before reference fixture acceptance
and evaluation freeze, retaining the original task and full goal scope.
