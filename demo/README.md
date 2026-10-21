# wisp demos

Runnable demos live here (not under `contrib/`, which is batteries and tooling).

| Demo | Path | Purpose |
|------|------|---------|
| **Desk** | [`desk/`](desk/) | Two agents, one book — merge, erase, sync, pitch PDF |

## Secrets

Never commit API tokens. Copy [`demo/.env.example`](.env.example) to **`demo/.env`** (gitignored) and set:

```sh
SCRYPTED_BEARER_TOKEN=scrypted_your_token_here
```

Optional: `OPENAI_API_KEY` for Act 1 LLM fallback.

Check before push:

```sh
./demo/check-no-secrets.sh
```

## Workspaces

Desk demo clones OKF bundles into **`demo/workspaces/`** (gitignored). Seed content still comes from [`docs/test/`](../docs/SCRATCH-STORE.md).

Migrate old clones (optional):

```sh
mkdir -p demo/workspaces
mv docs/demo demo/workspaces/primary 2>/dev/null || true
mv docs/demo-replica demo/workspaces/replica 2>/dev/null || true
```
