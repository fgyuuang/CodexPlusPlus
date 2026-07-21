# GPT-5.6 Model Catalog Acceptance

The model catalog must return `modelMetadata` for every route that supplies model IDs: saved relay profiles, aggregate relay profiles, and `config.toml` discovery. This preserves GPT-5.6 reasoning controls and the Fast service tier in the injected Codex UI.

Model IDs remain plain slugs such as `gpt-5.6-sol`. The `[272K]` / `[1M]` notation is configuration syntax only: Codex++ parses it into `modelWindows` and writes the numeric context window to `model_catalog_json`, so the request sent to a provider never contains the brackets.

Model discovery must append `/models` directly when a provider Base URL already ends in any version segment, for example `/api/coding/v3`; otherwise it appends the standard `/v1/models` endpoint.

For aggregate `gpt-*` and `codex-*` entries, the catalog displays each member in one label. A member whose upstream model is identical to the aggregate model contributes only its provider name; other members use `provider:upstream-model`. The display label is normalized to the plain model ID before the request is routed upstream.

The protocol proxy resolves the selected relay's mapping with the full display label first and the normalized model ID second, then rewrites the upstream request body's `model` field to that relay's real model name.
