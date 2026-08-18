# XR Corpus

XR Corpus is a local, session-aware terminology service for speech recognition and translation.
It loads versioned Markdown corpora, selects relevant terminology within each model's budget,
keeps bounded bilingual conversation history, and exposes stable HTTP and Rust client APIs.

## Design

- Markdown remains the source of truth for static corpora.
- Activation state and context snapshots belong to a server-side session.
- Static corpora and short-lived runtime providers use one catalog contract.
- Callers receive stable, neutral context data and provenance spans, not
  rendered translation prompts or internal catalog/UI template objects.
- Idle sessions, snapshots, and dynamic data are bounded and expire automatically.

## Run

```sh
cargo run -p xr-corpus-server -- --config config.example.json
```

The server listens on `127.0.0.1:7766` by default. `GET /healthz` reports readiness.

Start integrations with the typed Rust client:

```rust
let corpus = xr_corpus_client::CorpusClient::connect("http://127.0.0.1:7766").await?;
let session = corpus.create_session().await?;
```

`connect` verifies API compatibility before returning. See [API.md](API.md) for the complete
session lifecycle, dynamic-provider contract, error format, and curl examples. A compilable runtime
provider is included at [`crates/client/examples/publish_runtime.rs`](crates/client/examples/publish_runtime.rs).

## Corpus format

See [`corpora/v1/SCHEMA.md`](corpora/v1/SCHEMA.md). Each terminology row uses the fixed language
order declared in the file and leaves unavailable translations empty between English commas.

## Attribution

The automatic VRCX runtime provider was informed by [febilly/Yakutan](https://github.com/febilly/Yakutan).
Its source file retains SPDX attribution.

## License

GNU Affero General Public License v3.0 only (`AGPL-3.0-only`).
