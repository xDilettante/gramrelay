# Trusted MTProto host bootstrap (M3)

## Objective and interface

Finish SPEC-mtproto-agent-tools increment 3: the production `tg-agent run`,
`serve` and `mcp` commands load exact MT profiles through the same Host parser,
PolicyStore, ToolSession and executors. Preserve Bot configuration v1 unchanged.
Config v2 requires an explicit nonempty `mtproto` object and the existing
`profiles` array (possibly empty). Without feature `mtproto`, v2/MT fields fail.
No startup/login/identity/polling RPC, raw endpoint, automatic authentication or
model-provided authority. Standalone read_updates reads the existing journal;
continuous remote intake remains an explicitly managed Rust host capability.

```
schema_version: 2
principal, journal_path, profiles: existing Bot fields
callback_clock_policy: existing shared policy
mtproto: {
  profiles: [{name, account_kind: "bot"|"user", account_id: decimal string,
    session_path: absolute existing regular file, session_binding, api_id,
    session_key_env, grant, update_options?, request_timeout_ms?, max_pending?}],
  approvals?: [{request: exact MT Request, expires_at_ms}],
  file_restorations?: [{profile, file_handle, path: absolute}],
  callback_observations?: [{profile, cursor: exact MT cursor,
    reference: "mtcb_" followed by 32 lowercase hex digits}]
}
```

The operator provisions the encrypted Session using the normal M2 Rust auth API.
The configured binding/API ID, key and remembered exact account kind/ID must all
match. Reject absent, unprovisioned, quarantined, symbolic-link or nonregular
stores; do not create an empty session as an authentication side effect. Read the
32-byte session key only from the named environment variable as 64 hex digits;
zeroize the decoded and temporary owned secret. Never print it or accept literals
in config/tool input. api_hash/phone/password/bot token are unnecessary here.

Validate all static fields/grants/cursors/approval limits before opening storage.
Shared maxima across both protocols: 64 profiles, 256 approvals, 1024 restorations
and 1024 observations. Names are unique globally; expected identities are unique
per exact kind. Duplicate lists, unknown/duplicate fields, positional objects,
null MT extension, contradictory peer/all_peers scopes and invalid resource names
fail closed. MT grants use the same BudgetConfig with explicit mtproto actions,
peers/all_peers, files, unscoped_updates, expiry, approval actions and costs.
Default actions/resources deny. Request timeout is 1..30000 ms (default 30000),
max_pending 1..1024 (default 16); the common 60-second lease retains its existing
five-second confirmation margin. Keep remaining reviewed SDK defaults.

Before journal creation, canonicalize and validate all existing session paths,
refuse path aliases/shared journal paths (including native file identities for
hard links), then open/verify the provisioned
Sessions through a noninitializing `Session::open_existing` native path and
construct lazy Clients. Missing/empty stores cannot be initialized even after
external preflight races. Close every accepted session/client on a
subsequent startup failure. Register the exact Feed/Client pairs in one MT
Executor sharing the Bot PolicyStore. Restore immutable files, observe exact
retained callbacks, then issue configured exact approvals. The existing explicit
TrustedWallClock choice governs cross-lifetime callback provenance for both.
MT callback observations explicitly install a trusted alias for that actual
event, retaining original source/scope/time checks. Alias uniqueness is global;
both alias-to-context and context-to-alias conflicts refuse atomically. This
lets exact configured approvals refer to a known alias without letting tool JSON
mint authority, disclosing the raw query or refreshing the observation deadline.
Use the host-only `observe_callback_as` API; across registry lifetimes the host
must reuse an alias only for its original event. This trusted assignment is not
a durable alias-history ledger or permission to retarget an old reference.

## Lifecycle and boundaries

Host keeps the MT Executor and all Clients. Caller/adapter work must drain before
close; `Host::close(&self)` can resume after a cancelled wait. Always join MT
Executor, then every Client/session native cleanup, then Journal, even if one
close reports a failure. Return only static safe diagnostics. Drop/startup-future
cancellation requests ordinary resource cleanup; it does not claim joined close
or immediately reusable session ownership. Explicit close is the guarantee.

The existing per-protocol FileLimits remain bounded: at most two 100-MiB resident
pools, with one bounded restoration input at a time; they are not per profile.
No tool input grants access to source paths or config. The MT extension has no
endpoint override or fixture selector; existing Bot operator endpoint controls
remain unchanged. A dev-only host factory may swap
only lazy M2 Client construction using the existing test-kit; production load
always uses Client::new. No second executor/codec/transport path.

## Structure, evidence and commands

`host.rs` retains shared Bot config/limits and common bootstrap. `host/mtproto.rs`
owns MT-specific config, validation, resources and restoration. Reuse Strict/map
visitors, common BudgetConfig/FeedConfig and existing policy/provider methods.
Tests use synthetic encrypted sessions, local SQLite and real TL test-kit. The
ordinary executable proves no-RPC dry run, retained reads, invalid config/key/
binding/account refusal and session reuse after close. A separate dev-only
executable fixture exercises writes/callback/files through production Host
bootstrap, CLI and official MCP without real credentials/network.

Prove current grants/approvals, same-key/restart/Unknown, file hash restoration,
callback scope/expiry, static preflight before journal creation, partial startup
cleanup, joined/resumable close and secret-free errors. Preserve Bot v1/v2 schema
artifacts and run full gates before marking M3 complete. A narrower successful
bootstrap increment alone does not close M3 without executable acceptance.

```
cargo test --locked -p tg-agent --all-features --test mtproto_host
cargo test --locked --workspace --all-features --all-targets
cargo test --locked --workspace --all-targets
cargo test --locked --workspace --all-features --doc
cargo clippy --locked --workspace --all-features --all-targets -- -D warnings
cargo fmt --all -- --check
python -m unittest discover -s tools/tests -v
python tools/schema.py check
python tools/verify_grammers.py check
python tools/agent_schema.py check
python tools/agent_schema.py check --mtproto
cargo audit
```

Always: static redacted failures, exact existing capabilities, bounded owned work.
Never: live Telegram verification, publishing/push, config-selected RPC fixture,
editing generated Rust or treating a stored identity as live proof. Q1–Q4 remain
separate required completion gates after M3.

## Implemented acceptance evidence

Production Host v2 and the explicit development-only SDK factory are implemented.
Fourteen executable cases use normal tg-agent for no-RPC dry runs, seeded journal
reads/operation lookup, mixed Bot/MT configuration, discovery and startup refusal;
the synthetic executable changes only Client construction for actual TL actions.
Every configured DC is persisted to a bound loopback observer in these synthetic
Sessions, including IPv6-mapped endpoints, so normal executable tests observe
zero TCP connections without risking external Telegram traffic.

Both CLI/MCP action loops require succeeded/stored receipts with exact peer and
message IDs or callback acknowledgement; cached CLI receipts compare equal.
Unknown survives actual process restart with zero new SDK calls. Changed payload/
key approvals, hash restoration and wrong peer/clock/expired callback provenance
fail closed. Extra Client clones remain alive inside the fixture while Sessions
are reopened after partial startup and normal/resumed close, proving cleanup
before process exit. The close wait is cancelled while an owned SDK write is
blocked, then resumed and its confirmed receipt checked from storage.

New foundations have two existing-only Session cases, one loopback-route
ownership case, one alias-conflict unit and one actual event/approval integration,
plus native file-identity preflight. The undeclared-alias approval regression
failed because Journal was created, then passes after static mapping validation.
The first bootstrap test failed at the original Bot-only config parser.
The hard-link startup refusal was already safe before stronger native-identity
preflight; its initial passing test is not claimed as RED→GREEN evidence.

Fresh configuration/lifecycle/execution reviews are clear after addressing
explicit alias binding, noninitializing opening, hard links and the stricter
acceptance evidence. Cross-model review was skipped in this autonomous run.
Full workspace/default/docs/feature/schema/audit results are recorded in
tasks/todo.md at the final checkpoint; Q1–Q4 remain independent required gates.
