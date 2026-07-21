# GPT-5.6 Model Catalog Acceptance

The model catalog must return `modelMetadata` for every route that supplies model IDs: saved relay profiles, aggregate relay profiles, and `config.toml` discovery. This preserves GPT-5.6 reasoning controls and the Fast service tier in the injected Codex UI.

Model IDs remain plain slugs such as `gpt-5.6-sol`. The `[272K]` / `[1M]` notation is configuration syntax only: Codex++ parses it into `modelWindows` and writes the numeric context window to `model_catalog_json`, so the request sent to a provider never contains the brackets.

Model discovery must append `/models` directly when a provider Base URL already ends in any version segment, for example `/api/coding/v3`; otherwise it appends the standard `/v1/models` endpoint.

For aggregate `gpt-*` and `codex-*` entries, the catalog keeps the official model ID and appends one provider suffix to the existing frontend display name. A member whose upstream model is identical to the aggregate model contributes only its provider name; other members use `provider:upstream-model`. For example, the single official entry can display as `GPT-5.4(ProviderA|ProviderB:vendor-gpt-5.4)` without adding a duplicate model ID.

The frontend suffix is display-only. Requests keep the official model ID, and the protocol proxy rewrites the request body's `model` field to the selected relay's real model name. Normalization of legacy display labels remains supported for compatibility.
