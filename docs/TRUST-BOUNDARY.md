# Trust boundary and operational safety (0.1.0)

What **cryptographic erasure** and **sync** guarantee inside a wisp bundle, and what remains **operator responsibility**.

## Erasure ([docs/ERASE.md](ERASE.md))

| Inside boundary | Outside boundary |
|-----------------|------------------|
| Cold payloads shredded; scope keys removed from keystore | Plaintext copied to clipboards, logs, non-wisp disks |
| Hot tier and search indexes purged for erased scopes | `export --git`, tar backups **without** `.wisp/`, third-party mirrors |
| **Managed** checkouts (primary + `wisp worktree`) rewritten after shred | Unmanaged copies of `.md` files; editor local history |
| `erase` blocks on **unrecorded** dirty managed trees unless `--discard` | Agents that read then cache content in an external vector DB |

Erasure is **scoped** (section, concept, asset, actor, group). It does not revoke data already exported under another system.

## HTTP cold pack (`wisp serve --http`, sync over HTTPS)

| Default | Risk if misconfigured |
|---------|------------------------|
| Packs **omit** `keys/` | `WISP_PACK_INCLUDE_KEYS=1` embeds keystore material in the tarball |
| Bearer auth via `WISP_HTTP_TOKEN` (optional) | Cleartext HTTP on a LAN exposes pack bytes |
| Loopback bind is typical for dev | Non-loopback + keys in pack = **cleartext key material** |

**0.1.0 guard:** `serve --http` **refuses** to start when `WISP_PACK_INCLUDE_KEYS=1` and the bind address is **not** loopback, unless `WISP_PACK_INCLUDE_KEYS_INSECURE=1` (explicit unsafe override).

**Production:** terminate **TLS** in front (reverse proxy); do not expose dumb HTTP pack endpoints on untrusted networks. See [README.md](../README.md) M8 row.

## Stdio cold pack (`stdio://` remotes, `wisp push` / `wisp pull`)

| Property | Implication |
|----------|-------------|
| Remote URL is a **shell command** | `wisp push` / `wisp pull` run it with the **caller’s OS user** (via `sh -c` / `cmd /C`). |
| Manifest `remotes[].url` | Treat like executable config: anyone who can edit the bundle can point a named remote at arbitrary commands. |
| Channel security | SSH/TLS must come from the command you choose (e.g. `ssh host wisp serve --stdio`); the JSON-lines protocol does not authenticate the peer by itself. |
| Failed exchange | A protocol error does not prove the remote replica was unchanged; the subprocess is terminated on failure but remote side effects depend on how far `serve --stdio` got. |

Prefer one-off `stdio://…` URLs for ad hoc sync; use **trusted** remotes only for fixed infrastructure you control.

## Git interop

`import --git` / `export --git` are **interop tools**, not a guarantee that arbitrary Git merges preserve OKF section anchors. Editing markdown outside wisp then re-importing uses **heuristic** section matching (`DECISIONS.md` §7.5). See [docs/MERGE.md](MERGE.md) (ref merge vs record-time matching).

## Keys and backups

- `wisp backup --with-keys` copies keystore material; treat retention like any secret backup.
- `WISP_MASTER_KEY` and `.wisp/keys/` must stay out of VCS (see `demo/check-no-secrets.sh` patterns).

## Compliance framing

For diligence: wisp provides **provable destruction inside the store** and **audit-friendly provenance** on changes; **data lifecycle outside the bundle** still requires process controls (export policy, managed-checkout hygiene, replica trust).
