# Protocol schema and generation

Module ID: protocol. This slice implements P1; P2 remains responsible for the complete callable typed Rust API.

## P2: typed wire surface

Generate every pinned object, named union and method from the canonical schema. `api::types` is the complete wire model; the initial root DTOs remain convenience projections until runtime migration. Never label the projections complete. Normal builds use checked-in Rust files only.

Objects retain unknown fields in a read-only map. Required fields remain required, optional fields omit `None`, and literal tags/True/zero facts cannot be replaced with arbitrary values. Constructors accept required fields and fill literals; optional setters are lazy local changes. Chat/user/message/file/callback identifiers use separate reviewed Rust types. The JSON spelling remains Telegram's spelling.

Tagged unions decode known tags strictly and preserve new string tags in an opaque `UnknownVariant`. Missing/invalid tags and malformed known variants fail. Repeated tags and untagged object unions select exactly one complete candidate; a known field belonging exclusively to another candidate prevents a less-specific match. New unknown fields remain compatible. `MaybeInaccessibleMessage` selects its inaccessible variant only for integer date zero. RichText accepts strings, arrays and tagged objects. Named union payloads and optional object fields are boxed to bound storage (Update must not reserve a large inline Message for every possible event). Constructors/setters and `From<Payload>` hide these allocations. Additional recursive value edges are boxed only where required to break size cycles; arrays already provide indirection. Allocation and throughput tradeoffs must be measured at Q2.

All generated methods use one sealed typed request path and the existing bounded, redacted transport. Wire methods support all pinned parameters; convenience helpers remain available. Effect/replay metadata requires explicit review, with conservative defaults never granting agent permissions. Files, retries and agent authorization are separate gates, not implicit consequences of generation.

Verify strict known/unknown union behavior, required/literal fields, nested recursion, Unicode, ID separation, reproducibility, complete method/type coverage and representative exact JSON requests against localhost mocks. No live Telegram calls are needed.

## Inputs and authority

The pinned source is the official Telegram Bot API HTML at https://core.telegram.org/bots/api. Import works from an explicit local file. A developer may download it into target/schema-cache (ignored by Git). Normal Cargo builds never download or generate API code.

The canonical schema records API version, source URL/SHA-256, all object fields, required flags, literal discriminator facts, union variants and method parameter/return types. It stores structural API facts and source links, not copied prose. The import must fail rather than silently replacing an unknown type with JSON Value or dropping an unsupported row.

## Outputs

- schema/bot-api.json: sorted, pinned structural schema, independent of filesystem timestamps.
- schema/overrides.json: explicit reviewed corrections for return/type ambiguities; stale or unused overrides fail.
- src/generated/catalog.rs: reproducible method/type inventory and schema identity, later extended with typed code by P2.
- Coverage report: every recognized official type/method appears once; all named references resolve. The initial 10.4 source contains 193 methods and 425 types; count changes require deliberate review.

Use Python's standard library for development-only HTML extraction/generation, with unittest fixtures and no additional runtime dependencies. The library and callable generated surface remain Rust. Generated code and the canonical schema are checked in.

## Checks

python -m unittest discover -s tools/tests -v
python tools/schema.py import --source target/schema-cache/api.html
python tools/schema.py generate
python tools/schema.py check
cargo test --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings

The check command performs no network I/O and does not modify files. Importing identical source plus overrides produces byte-identical JSON and Rust artifacts. Type/return ambiguities must be explicit; conservative effect metadata starts unreviewed and is not permission for agent tool exposure.
