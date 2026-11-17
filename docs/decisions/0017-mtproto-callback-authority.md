# 0017: Original callback observations and durable semantic identity

Accepted locally, 17 November 2026. Extends 0011 and SPEC-mtproto-agent-callback.md.

MTProto query IDs are opaque 64-bit wire data. Agent requests use a random alias
issued only from an authorized original event in the same journal instance and
profile. Bind principal, exact bot, primary peer or explicit unscoped boundary,
original UTC receipt and monotonic collection time. Read authority may issue an
alias; answer authority and any approval are checked separately on every use.

Canonical admission hashes a domain separator plus query bits, independent of
owner, alias or timestamps. The existing v8 partial index seals one operation key
per admitted query/profile in this database. Durable intent replaces only the
visible alias with that hash and retains all answer fields and original times.
This permits an explicitly restored alias to resume its own confirmed NotSent
key without silently changing semantics. Approvals still bind the entire visible
request and alias. Unknown/Rejected cannot replay; another key cannot bypass any
previous admission, including NotSent. Separate databases/forks share no fence.

Registry entries and every copied authorization share an irreversible UTC
high-water in addition to immutable monotonic deadlines. Once a forward clock
sample expires a reference, wall rollback cannot revive a buffered copy. Runtime
policy decisions sample UTC after acquiring the policy lock; otherwise a blocked
check could accept an expired grant or approval. Pure explicit-time policy APIs
remain local calculations, not execution permits. Both fixes apply to Bot API.

Original monotonic proof disappears on journal restart. Default recovery refuses
new answers; explicit host TrustedWallClock permits restoration with original
times and host clock responsibility. Neither mode proves server creation age.
Source-watermark replay retains provenance; a distinct later server batch can
contain a new observation, without an unbounded all-time observation ledger.
Previously admitted queries remain durably sealed regardless of RAM expiry.

Only Bool true produces acknowledged=true. Absolute deadlines cover identity and
target RPC, while owned jobs retain capacity through cancellation/confirmation.
Current original scope remains necessary for retained get_operation; a live
reference is not necessary for that lookup. Final caller-side revision/deadline
checks withhold buffered results after revocation or expiry.

Verification uses synthetic encrypted sessions, production TL codecs, actual M2
intake, durable restarts, false/RPC/timeout/cancellation and lock/rollback RED→GREEN
regressions. Fresh focused contract, registry and execution re-reviews found no
remaining substantive findings. Cross-model skipped: non-interactive context.
