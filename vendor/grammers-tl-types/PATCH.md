# Bounded incoming TL collections

Pinned upstream grammers-tl-types 0.12.0 archive:
https://static.crates.io/crates/grammers-tl-types/grammers-tl-types-0.12.0.crate
SHA-256 `eb4da570f8dbcea3e0d0a1746240045555bacc56b4d57d3a81204ee0e84736bd`.
Original copyright headers retained; distributed under Apache-2.0.

The manifest and decoder are patched, and a local Cargo build driver is added.
Publishing is disabled. Byte-string
decoding rejects reserved length marker 255, noncanonical long lengths and
nonzero padding. It verifies the entire field is available before allocating
owned bytes. Strings decode the validated borrowed bytes directly and preserve
the standard lossy UTF-8 result, counting replacement characters before
allocating.

A cursor shares a 64 MiB budget for requested allocation capacity and a
1,048,576-item budget for boxed, bare and manually decoded vectors. Vector
growth charges each requested capacity, including capacity allocated before an
earlier block can be released. Collection allocation uses `try_reserve_exact`.
The same allocation budget charges each generated decoder box by the size of its
value before decoding it. Each box and each nonempty vector consumes one of 24
nesting levels while decoding its contents. Sibling values reuse the depth
levels and share the cumulative allocation and item budgets.

The pinned generator and original `build.rs` run unchanged. The driver rewrites
the decoder allocation sites in `OUT_DIR`, checks decoder signatures and counts
against the declarations, and matches each rewritten arm to its boxed variant.
It requires exactly 37 recognized sites in API mode or zero with only MTProto or
neither schema. Format or inventory mismatches fail the build. The serializers
and schema files retain their upstream bytes.

The allocation ledger measures requested capacity. The host allocator, the
borrowed wire buffer and caller-provided decoders are outside that ledger. The
nesting limit bounds recursive descents, with representative generated paths
tested on a 2 MiB thread stack. Native stack frame sizes depend on the build.

The patched MTProto core maps the resource limit to `LimitExceeded`, and the
client returns `Capacity` with an `Unknown` outcome for a matching RPC reply.
Malformed RPC errors still use the static decode failure.

These rules follow the base-type serialization in
https://core.telegram.org/mtproto/serialize . Local tests cover canonical
boundaries, shared allocation and item limits, zero-sized bare items, lossy UTF-8
and byte reads that fail before allocating. Seeded cases cover encoded byte
lengths, vector cardinality and recursive generated values, including Box-only,
vector-only and mixed paths.

`python tools/verify_grammers.py check` verifies the pinned archives and
their exact recorded patches. Standalone tests use root target/vendor-tl and
`cargo test --locked --manifest-path vendor/grammers-tl-types/Cargo.toml --lib`.
They perform no network or account actions.
