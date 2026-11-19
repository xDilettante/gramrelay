# Bounded incoming TL collections

Pinned upstream grammers-tl-types 0.12.0 archive:
https://static.crates.io/crates/grammers-tl-types/grammers-tl-types-0.12.0.crate
SHA-256 `eb4da570f8dbcea3e0d0a1746240045555bacc56b4d57d3a81204ee0e84736bd`.
Original copyright headers retained; distributed under Apache-2.0.

Only Cargo.toml and src/deserialize.rs change. Publishing is disabled. Byte-string
decoding rejects reserved length marker 255, noncanonical long lengths and
nonzero padding. It verifies the entire field is available before allocating
owned bytes. Strings decode the validated borrowed bytes directly and preserve
the standard lossy UTF-8 result, counting replacement characters before
allocating.

A cursor shares a 64 MiB budget for requested allocation capacity and a
1,048,576-item budget for boxed, bare and manually decoded vectors. Vector
growth charges each requested capacity, including capacity allocated before an
earlier block can be released. Allocation attempts use `try_reserve_exact`.
These limits cover the built-in collections and owned byte/string fields.
Generated recursive boxes, stack depth and caller-provided decoders require
separate bounds. Allocator rounding and the borrowed wire buffer are outside
the requested-capacity ledger.

The patched MTProto core maps the resource limit to `LimitExceeded`, and the
client returns `Capacity` with an `Unknown` outcome for a matching RPC reply.
Malformed RPC errors still use the static decode failure. Serializer, schema,
generator and original build script bytes are retained.

These rules follow the base-type serialization in
https://core.telegram.org/mtproto/serialize . Local tests cover canonical
boundaries, shared allocation and item limits, zero-sized bare items, lossy UTF-8
and byte reads that fail before allocating.

`python tools/verify_grammers.py check` verifies the pinned archives and
their exact recorded patches. Standalone tests use root target/vendor-tl and
`cargo test --locked --manifest-path vendor/grammers-tl-types/Cargo.toml --lib`.
They perform no network or account actions.
