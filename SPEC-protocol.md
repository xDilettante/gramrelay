# Protocol schema and generation

Module ID: protocol. This slice implements P1; P2 remains responsible for the complete callable typed Rust API.

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
