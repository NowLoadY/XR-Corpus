# XR Corpus API v1

XR Corpus is a loopback HTTP service. The default base URL is `http://127.0.0.1:7766`.
All JSON endpoints live under `/v1`; readiness is available at `/healthz`.

## Compatibility and errors

Call `GET /healthz` before creating sessions and require `api_version: 1`. The Rust SDK does this
with `CorpusClient::connect`.

```sh
curl http://127.0.0.1:7766/healthz
curl -X POST -H "Content-Type: application/json" -d "{}" http://127.0.0.1:7766/v1/sessions
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

1. `POST /v1/sessions` with `{}`.
2. Before ASR, `POST /v1/sessions/{id}/asr` with languages and model-derived token budgets.
3. After ASR, `POST /v1/sessions/{id}/translation` with the recognition and its segments.
4. Translate each returned segment using its structured `context_data`, `prompt_terms`, and
   shared `context_id`. `context_data` exposes bounded recent turns, the previous streaming
   revision, and source text surrounding that exact segment. `prompt_terms` supplies relevant
   structured terminology. These fields are data only; XR Corpus does not own user templates,
   block ordering, or provider message roles.
5. Join the successful segments in source order and `POST /v1/sessions/{id}/results` once for the
   logical speech turn. Send the same `turn_id` on continuous-window revisions; the latest window
   updates that turn instead of appending overlapping text. `speaker_id` is optional neutral
   recognition metadata used to label dialogue context.
6. `DELETE /v1/sessions/{id}` when finished. Abandoned sessions expire automatically.

`GET /v1/sessions/{id}` exposes active corpus IDs and retained snapshot count for diagnostics.
Snapshots are immutable and bounded; clients must not reuse old context IDs indefinitely.

## Vocabulary graph

`GET /v1/graph` returns the editable persistent graph as `domains`, `nodes`, and `edges`.
Changes take effect in new ASR and translation selections without restarting the service.

- `PUT /v1/graph/domains/{id}` saves a domain; `DELETE` removes an empty domain.
- `PUT /v1/graph/nodes/{id}` saves a node; `DELETE` removes a node but leaves its edges
  dormant so they can reconnect if that ID appears again.
- `PUT /v1/graph/edges` saves an edge; `DELETE /v1/graph/edges` with the edge as JSON
  removes it.

Mutations return the current complete graph. IDs in URL paths must match the JSON body.
Nodes require an existing domain and exactly sixteen ordered language values. `activation`
is `always` or `on-evidence`; `promptable` controls whether a node can enter recognition
and translation context. An `on-evidence` node needs an enabled incoming `trigger` edge.
Incoming `context` edges express a second requirement: one enabled context source must also
appear in the current evidence. Both endpoint nodes and their domains must be enabled.
An edge whose source or target does not exist remains stored and becomes effective when
that node appears again. The graph editor visualizes these edges as dormant.

The database is `runtime/xr-corpus.sqlite`, initialized once from the packaged
`corpora/default.sqlite`. Stop XRTranslate before copying the user database to another
installation. The service never replaces an existing user database merely because the
application version changed.

## Runtime providers

External programs publish current room names, player names, project terminology, or similar
short-lived information with:

```text
PUT /v1/providers/{provider-id}
DELETE /v1/providers/{provider-id}
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
- Session history and context snapshots are bounded.
- Prompt budgets come from the active ASR and translation models, not hard-coded model names.
- Persistent graph nodes and dynamic providers enter the same ranking and activation pipeline.
