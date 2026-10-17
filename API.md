# XR Corpus API v3

XR Corpus is a loopback HTTP service. The default base URL is `http://127.0.0.1:7766`.
All JSON endpoints live under `/v3`; readiness is available at `/healthz`.

## Compatibility and errors

Call `GET /healthz` before creating sessions and require `api_version: 3`. The Rust SDK does this
with `CorpusClient::connect`.

```sh
curl http://127.0.0.1:7766/healthz
curl -X POST -H "Content-Type: application/json" -d "{}" http://127.0.0.1:7766/v3/sessions
```

Errors have a stable machine-readable code and a human-readable message:

```json
{
  "code": "context_expired",
  "error": "ASR context snapshot has expired"
}
```

Do not branch on the English `error` text.

## Inference session lifecycle

1. `POST /v3/sessions` with `{}`.
2. Before ASR, `POST /v3/sessions/{id}/asr` with languages and model-derived vocabulary token budgets.
3. After ASR, `POST /v3/sessions/{id}/translation` with the recognition and its segments.
   Optional `context_evidence` supplies relevant caller-owned text for vocabulary ranking.
   New topic activation requires current recognition or live runtime facts; external context is
   neither a new topic trigger nor retained conversation history.
4. Translate each segment using its `prompt_terms` and `source_corrections`. `activation_matches`
   and `context_matches` identify the source spans that activated or matched vocabulary.
   Prompt composition, conversation memory and completed translation results belong to the caller.
5. `DELETE /v3/sessions/{id}` when finished. Abandoned sessions expire automatically.

Reuse the same `turn_id` for recognition revisions. Topic decay advances once per logical turn,
and its recognition evidence replaces the preceding revision. `context_evidence` defaults to an
empty array; a new session has no shared conversation identity.

The Rust SDK's `prepare_asr` takes a mutable session handle. If the server reports
HTTP 404 with `session_not_found`, it creates a fresh session and retries this
preparation once. Retain that handle for later turns. Clones already handed to
in-flight work keep the old session identity; translation preparations are never
replayed into a new session with stale context IDs. Other errors return without retrying.

`GET /v3/sessions/{id}` exposes active corpus IDs and retained snapshot count for diagnostics.
Snapshots are immutable and bounded; clients must not reuse old context IDs indefinitely.

## Vocabulary graph

`GET /v3/graph` returns the editable persistent graph as `domains`, `nodes`,
and `edges`. Node positions are arranged automatically by the client and are
not stored in the corpus database.
Changes take effect in new ASR and translation selections without restarting the service.

- `PUT /v3/graph/domains/{id}` saves a domain with an optional `parent_id` (`null` for a root)
  and `cue_words`: `null` inherits the nearest ancestor's rules, `[]` uses direct matching,
  and a nonempty list activates the domain when any topic word occurs, in any language.
  Optional `inherit_existing: true` makes all terms in this domain and its descendants
  use their domain rules, in the same transaction. Descendants' explicit domain rules remain in place;
  `DELETE` removes a domain only when it has no nodes or children.
- `PUT /v3/graph/nodes/{id}` saves a node; `DELETE` removes a node but leaves its edges
  dormant so they can reconnect if that ID appears again.
- `PUT /v3/graph/nodes` updates `enabled`, `domain_id`, and/or `inherit_domain` for a nonempty
  `ids` array in one transaction. Unknown or duplicate IDs and unknown domains reject
  the entire batch.
- `PUT /v3/graph/terms/{id}` atomically saves `{node, domain, incoming_edges}`.
  `domain` is an optional new domain containing the term; an existing domain ID is rejected.
  `incoming_edges` replaces that term's incoming edges only. Failure rolls back the entire edit.
- `POST /v3/graph/terms` imports an array of nodes in one transaction, with a 32 MiB request limit.
  Existing or repeated IDs reject the whole batch; existing terms are never overwritten.
- `PUT /v3/graph/edges` saves an edge; `DELETE /v3/graph/edges` with the edge as JSON
  removes it.

Mutations return the current complete graph. IDs in URL paths must match the JSON body.
Nodes belong to one domain in the tree and require exactly sixteen ordered language values.
The runtime corpus `subdomain` is derived from the first child below the root domain. `activation`
is `always` or `on-evidence`; `promptable` controls whether a node can enter recognition
and translation context. A node with `inherit_domain: true` uses the effective domain rules
instead of its individual activation or incoming edges. Moving it changes its inherited rules.
`recognition_aliases` belongs to the term independently: a map of language codes to spellings,
for example `{"en":["fanboy","fem boy"]}` on a term whose English value is `femboy`.
Every populated alias language needs a canonical value in that same language. Saving trims and
deduplicates spellings; aliases never become hotwords or bypass domain activation. Changing
inheritance, moving terms, or applying domain rules preserves aliases. An empty map removes them.
The editor defaults new and imported terms to inheritance. Omitted `inherit_domain` is `false`
for compatibility with existing clients. Existing stored nodes keep their individual rules on upgrade.
An independent `on-evidence` node needs an enabled incoming `trigger` edge.
Incoming `context` edges express a second requirement: one enabled context source must also
appear in the current evidence. Both endpoint nodes and every ancestor domain must be enabled.
An edge whose source or target does not exist remains stored and becomes effective when
that node appears again. The graph editor visualizes these edges as dormant.

Inherited terms are compiled into shared domain/priority collections, without generating edges
between their members. A domain may contain more than the 512-term limit used for external provider
payloads. Topic words enter activation through `topic_words` only; they are never offered as
translations or recognition corrections. Prompt vocabulary remains relevance- and budget-limited.

The database is `runtime/xr-corpus.sqlite`, initialized once from the packaged
`corpora/default.sqlite`. Stop XRTranslate before copying the user database to another
installation. The service keeps the user database across ordinary application updates.
When the database format changes, it migrates the existing runtime database in one
transaction before opening it. Schema v6 adds per-term recognition aliases and migrates enabled alias-node corrections;
v3/v4/v5 user databases migrate transactionally. Existing activation edges remain intact.
The v4/v5 distribution seeds are accepted without modification.

## Runtime providers

External programs publish current room names, player names, project terminology, or similar
short-lived information with:

```text
PUT /v3/providers/{provider-id}
DELETE /v3/providers/{provider-id}
```

Each `PUT` atomically replaces that provider's complete snapshot. Runtime snapshots require a TTL
between 5 and 3600 seconds, so a crashed provider cannot leave stale context behind.

```json
{
  "ttl_seconds": 30,
  "corpora": [
    {
      "schema": "xrtranslate-corpus/v1",
      "id": "runtime.example.room",
      "domain": "virtual-worlds",
      "subdomain": "example",
      "title": "Current room",
      "priority": 100,
      "activation": "always",
      "triggers": [],
      "trigger_aliases": [],
      "activation_context": [],
      "terms": [
        {"ordered_values":["","Player One","","","","","","","","","","","","","",""]}
      ]
    }
  ]
}
```

Term columns always follow `zh,en,fr,pt,es,ja,ru,ko,th,it,de,vi,id,pl,cs,nl`. Empty translations
remain empty columns. The Rust types validate schema IDs, column count, limits, and duplicates.

## Operational guarantees

- The service binds to loopback only.
- Provider replacement is atomic.
- Dynamic provider data expires by TTL.
- Recognition evidence and vocabulary snapshots are bounded.
- Prompt budgets come from the active ASR and translation models, not hard-coded model names.
- Persistent graph nodes and dynamic providers enter the same ranking and activation pipeline.

### Recognition alias provider compatibility

Runtime corpora may supply `recognition_aliases` as
`[{"language":"en","canonical":"femboy","variants":["fanboy"]}]`.
The canonical word must occur in the corpus terminology. An explicit empty array disables
correction aliases; omission preserves the legacy `trigger_aliases` interpretation. Persisted
graph projections always provide this field, so removing a migrated alias cannot resurrect it
through old activation edges. Activation topics, canonical hotwords and correction spellings
remain separate. Ambiguous active mappings are not applied. Canonical words alone enter the recognition vocabulary.
