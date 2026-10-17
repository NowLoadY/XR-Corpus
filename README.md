# XR Corpus

XR Corpus is a local, session-aware terminology service for speech recognition and translation.
It selects vocabulary from a user-editable SQLite concept graph within each model's budget
and exposes HTTP and Rust client APIs.

## Design

- Domains form a tree, so games and their term categories can be managed independently. Each
  vocabulary node has a required domain and sixteen ordered language values. Domain topic words
  are configured once and inherited by member terms and child domains. Individual overrides
  remain available for exceptions. Directed
  trigger and context edges connect nodes; an edge with a missing or disabled endpoint stays
  stored but does not activate vocabulary.
- The editable graph lives in `runtime/xr-corpus.sqlite`. On first launch, the service copies
  `corpora/default.sqlite` into that location. Later launches and ordinary application updates
  keep the user's database. SQLite schema changes are governed by `PRAGMA user_version`.
- The database uses rollback journaling, so a stopped application's graph can be shared by
  copying one file.
- Activation state, bounded recognition evidence and vocabulary snapshots belong to a server-side session.
  Callers own conversation memory and provide relevant context when selecting terminology.
- The persistent graph and short-lived runtime providers enter one selection pipeline.
- Callers receive vocabulary, recognition corrections and provenance spans as structured data.
  Prompt composition and translation history remain with the translation executor.
- Idle sessions, snapshots, and dynamic data are bounded and expire automatically.

## Run

```sh
cargo run -p xr-corpus-server -- --config config.example.json
```

The server listens on `127.0.0.1:7766` by default. `GET /healthz` reports readiness.

Start integrations with the typed Rust client:

```rust
let corpus = xr_corpus_client::CorpusClient::connect("http://127.0.0.1:7766").await?;
let mut session = corpus.create_session().await?;
```

`connect` verifies API compatibility before returning. See [API.md](API.md) for the complete
session lifecycle, dynamic-provider contract, error format, and curl examples. A compilable runtime
provider is included at [`crates/client/examples/publish_runtime.rs`](crates/client/examples/publish_runtime.rs).

## Vocabulary graph

The [graph API](API.md#vocabulary-graph) manages domains, nodes and directed edges. A node
contains one concept, with values in the fixed language order
`zh,en,fr,pt,es,ja,ru,ko,th,it,de,vi,id,pl,cs,nl`. Missing translations are empty strings.
Disabling a domain or any ancestor, a node, or an edge immediately removes its effect from
subsequent selections; the stored content remains available for later editing.

## Attribution

The automatic VRCX runtime provider was informed by [febilly/Yakutan](https://github.com/febilly/Yakutan).
Its source file retains SPDX attribution.

## License

GNU Affero General Public License v3.0 only (`AGPL-3.0-only`).

Recognition aliases are term attributes, independent of domain activation. The editor can attach
`fanboy` to the English canonical word `femboy` without graph wiring; domain inheritance and
moves retain this correction. Only canonical words enter ASR vocabulary. The v6 database migration
preserves existing corrections while keeping activation edges compatible.
