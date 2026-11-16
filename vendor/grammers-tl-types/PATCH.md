# Canonical incoming TL byte strings

Pinned upstream grammers-tl-types 0.12.0 archive:
https://static.crates.io/crates/grammers-tl-types/grammers-tl-types-0.12.0.crate
SHA-256 `eb4da570f8dbcea3e0d0a1746240045555bacc56b4d57d3a81204ee0e84736bd`.
Original copyright headers retained; distributed under Apache-2.0.

Only Cargo.toml and src/deserialize.rs change. Publishing is disabled. Byte-string
decoding rejects reserved length marker 255, noncanonical long lengths and
nonzero padding. It verifies the entire field is available before allocating
owned bytes. Error::InvalidEncoding is mapped by the patched MTProto core into
a static decode failure, so malformed RPC errors cannot become refusals.
String UTF-8 handling, serializers, generator/build script, schema files and
generated API definitions are unchanged.

These rules follow the base-type serialization in
https://core.telegram.org/mtproto/serialize . Three local tests cover bad forms
and all canonical short lengths plus the long-length boundary.

`python tools/verify_grammers.py check` verifies all three pinned archives and
their exact recorded patches. Standalone tests use root target/vendor-tl and
`cargo test --locked --manifest-path vendor/grammers-tl-types/Cargo.toml --lib`.
They perform no network or account actions.
