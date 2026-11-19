# 0018: Upload ownership belongs to accepted calls

Accepted locally, 17 November 2026. See SPEC-mtproto-agent-files.md.

M2 document intents accept a stable owner through pinned Bytes::from_owner. Keep
Vec retained-capacity checks and wiping, two-pass integrity and existing part
bounds. An absolute cap covers verified cache lookup and the complete upload.
Bot API and MTProto reuse one private immutable snapshot registry with no copied
metadata/permit implementation. The registry extraction changed no schema. The
following document-provider integration exposed the old Bot-feed FK and requires
the v9 profile-FK migration recorded in SPEC-mtproto-agent-files.md.

The accepted Call owns a Bytes guard before identity or business-action polling.
Move it into CallState on actor admission. Keeping it only in Prepared is too
short: failed read_next consumes Prepared before part drain, and finish consumes
it before final SendMedia. Keeping it only in the action also misses queued and
identity-wait cancellation. CallState survives through driver and native joins.

Normal completion disposes source ownership on serialized native work before
reply/capacity release. Shutdown joins driver, async wrappers and native work,
then disposes retained sources and queued actions before session finalization.
Queued native cleanup must execute even if its action deadline has passed.
Stale queued actions cannot dispatch and do not arm generation timeout; their
owned cleanup continues without stopping an otherwise healthy driver.

Contain host destructor panics in native disposal. Forget a failed panic payload
because its destructor is arbitrary too; stop the bounded client on cleanup
failure. The intentional payload leak occurs only after a trusted host destructor
failure, instead of risking recursive panic and skipped session close. Before
admission, ordinary Rust caller ownership applies. Failed enqueue disposal is
caller-owned native work whose reserved permit persists through cancellation;
it has no accepted protocol effect or promise of inclusion in actor close.

An RPC failure fencing one action is not another queued call's RPC result.
Affected unstarted calls report Closed/NotSent or their own elapsed Timeout.
The initiating action keeps its own conservative error. This preserves the
existing DC-import waiter fence independently of scheduling.

Pre-fix tests reproduce early source release, skipped session finalization,
queued-owner panic and enqueue rejection. Additional checks cover nested panic
payload, zero-copy identity, read error retention, absolute identity/part/cache
caps, healthy-driver stale rejection and existing file behaviors. Registry,
source, disposal and queue boundaries were reviewed separately; actionable
findings have passing regressions and scoped re-reviews. Cross-model skipped:
non-interactive continuation. The eighth executor action remains separate work.
