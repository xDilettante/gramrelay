# ADR 0021: committed source candidates with bounded verification

Status: accepted for the local source-distribution increment; release acceptance
is recorded separately in SPEC-source-distribution.md.

The hardened dependencies are supplied as one complete source tree (ADR0020).
Export a single captured commit through native Git ZIP and independently compare
every exported blob and mode with that tree. Disable replacement objects,
line-ending conversion, global attributes, fsmonitor helpers and automatic GC.
Untracked info/attributes can still affect Git archive; the independent tree
comparison detects those changes instead of trusting an archive-derived manifest.

Use a small explicit ZIP profile: bounded input/expanded bytes, entry counts,
paths and metadata; store/deflate only; no links, ZIP64, encryption or multi-disk.
Check central framing before ZipFile allocation, then local headers, contiguous
record extents, bounded genuine deflate end, actual sizes and CRC. Normalize
after casefold when excluding path aliases, including implicit parents. Write
only validated immutable file bytes into a new directory in an owned parent.

The initial TAR/PAX design was rejected because extension parsing preceded
ordinary member limits. ZipExtFile alone was also insufficient: it can truncate
to an advertised size. Observed RED-to-GREEN regressions cover those ZIP framing
and output assumptions. Three scoped review cycles completed with no remaining
concrete findings. Cross-model skipped: autonomous continuation.

The digest manifest detects corruption, not provenance. Plain check executes no
archived code; --consumer explicitly executes a trusted candidate's relocated
verifiers and public-API fixture. Preserve separate build evidence and clean
completed alternate Cargo caches. This does not establish registry packaging,
platform/MSRV compatibility or fully vendored offline dependency availability.
