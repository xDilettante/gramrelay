# Authenticated upstream comparison inputs

These unmodified published release archives let the source verifiers compare
the downstream SDKs without a pre-existing Cargo cache or network access. They
are comparison data, not selected Cargo packages. Their original licenses and
notices remain inside the archives and in the corresponding vendored sources.
The project's private notice does not replace those licenses.

| Release archive | SHA-256 |
|---|---|
| grammers-session-0.12.0.crate | 221a58eaf6bfb61ecd1cb8bc58c20d2bc7ec3749b968bc90d52bcf9a8547d4e6 |
| grammers-mtsender-0.12.0.crate | 9a6339a64b7b70ae4447ae4ae7b1b5a1e2e15649882bf26550df356e7a8bb1a4 |
| grammers-mtproto-0.12.0.crate | fc95dd02ac37101d8121e5098706a9c9b342e855ca7d7b8b5daa91ec7b119649 |
| grammers-tl-types-0.12.0.crate | eb4da570f8dbcea3e0d0a1746240045555bacc56b4d57d3a81204ee0e84736bd |
| rmcp-3.11.0.crate | 391f153431ab68664d946513ea0cb3edbca78eaf99c5a983036cacab6220d85b |

Canonical download URLs have the form
`https://static.crates.io/crates/<name>/<name>-<version>.crate`.
Each verifier authenticates its pinned checksum before parsing any archive.
The five files total less than 1 MiB. Do not regenerate or replace them to hide
a source-diff failure. Updating a release requires deliberate checksum/version
review and corresponding verification; no public publication is authorized.
