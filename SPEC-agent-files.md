# Authorized file snapshots (A1 increment 4)

Implement the file boundary of SPEC-executor.md before marking A1 complete.
The trusted host supplies owned bytes, a plain filename, principal/profile and
absolute UTC expiry. Tool arguments accept only an opaque file_handle and plain
caption; no paths, URLs, Telegram file IDs or supplied hashes. Host registration
does not grant agent permission. The host can grant a returned handle afterward.

## Durable identity

Add schema v5 executor_files without modifying v1 through v4 DDL. A random opaque
handle is globally unique within a journal and never reused/deleted. Bind exact
profile/principal, SHA-256, byte size, filename, creation and expiry permanently.
Use the existing SQLite thread, parameterized SQL and immediate transactions.
Default max_file_handles is 100000 across the journal; capacity rejects new
metadata instead of evicting it. Validate all old DDL before migration and new
DDL before commit; reject malformed/newer schemas atomically. Metadata is not
file content or a local path. Protect filenames and hashes as private metadata;
Debug and errors redact them. Validate names before storage/multipart: nonempty,
at most 255 UTF-8 bytes, no controls, slash or backslash, and neither '.' nor '..'.

register_file returns immutable RegisteredFile metadata. restore_file can attach
owned bytes only after owner/profile/handle lookup and exact hash/size comparison.
Metadata lookup never trusts claims from a tool. Wrong ownership does not disclose
another principal's metadata. No overwrite API exists. A cancelled registration
may leave retained metadata without active bytes; it cannot change an old handle.

## Memory and upload ownership

Host FileLimits default to 50 MiB per file, 100 MiB total resident snapshots,
1024 resident/pending snapshots and 50 MiB active uploads. Limits are finite and
validated. Reserve resident byte/count capacity before accepting a snapshot or
starting hash work. Charge the actual retained Vec capacity, including excess
capacity beyond its wire length; a short file cannot pin an uncharged allocation.
The host's preexisting input allocation is outside executor ownership until
accepted; oversized capacity is rejected before hash/native work. Hash owned data
on bounded blocking work; cancellation retains
permits until native work/owned bytes actually finish. No live entry is evicted.

Snapshots are immutable shared owners. Uploads reuse those bytes, without a
second full file copy. SDK InputFile::owned_bytes uses bytes::Bytes::from_owner
and a known-length multipart Body to retain the owner through actual body lifetime.
The upload owner carries both the snapshot and its active-upload byte permit;
dropping the registry entry or caller cannot free capacity while transport still
owns bytes. Discard/prune only removes resident map references; concurrent uploads
retain ownership. Empty files still consume a snapshot slot. SDK retains its own
upload and transport limits. No unsafe code or new transport is introduced.

## Execution

Executor authorization/dry-run/approval resolves immutable owner-checked metadata
and current file/chat/action grants. Canonical intent includes handle/hash/size/
filename/expiry; persisted operation scope includes handle/hash/size. A host
approval binds the immutable handle in the exact request. Registration never
substitutes approval or agent grants.

After prepare, an existing operation returns its currently authorized minimal
receipt even if bytes are absent or file expiry passed. get_operation also uses
persisted original file scope independently of live bytes/expiry. A new executable
attempt requires unexpired metadata, a matching resident snapshot and upload
capacity before identity RPC or begin. Missing/expired bytes never free the key
or create a new intent. Restart requires explicit matching restoration for new
uploads; it never replays Unknown. File expiry uses the host UTC clock, like grant
expiry; the host is responsible for clock correctness across restarts.

Recheck file expiry and current policy inside atomic begin and immediately before
polling the lazy multipart call. Quota charges the immutable full byte size with
write/cost admission, including a same-key retry after confirmed NotSent; cache
reads do not charge upload again. Cancellation after begin and receipt failures
keep existing conservative Unknown semantics. Use the shared absolute deadline,
one SDK attempt and allowlisted exact-target message receipt. No raw remote file
ID, document body or response description is stored/disclosed in that receipt.

## Verification

Observe RED before implementation: migration v4 to v5 and malformed-v4 rollback;
metadata nonreuse/capacity/ownership; exact multipart bytes and safe receipt;
restart/cache/missing bytes/restoration; expiry/current grant/approval checks;
size/hash/filename/TOCTOU abuse; active ownership after discard/cancellation;
atomic upload quota and Unknown/NotSent behavior. Run full repository gates and
offline authorized_document example before completing A1. CLI/MCP remain A2/A3.
