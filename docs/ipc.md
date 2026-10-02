# Thallium Store IPC v1

Transport: newline-delimited JSON-RPC 2.0 over a Unix socket.

Default socket:

```text
$XDG_RUNTIME_DIR/thallium-store/backend.sock
```

The socket directory must be `0700`; the socket must be `0600`.

## Requests

Every request is one JSON object followed by `\n`.

```json
{"jsonrpc":"2.0","id":1,"method":"system.health","params":{}}
```

## Methods

### system.hello

Returns protocol and backend information.

### system.health

Returns backend readiness and UNI status.

### catalog.search

Request:

```json
{"query":"gimp","sources":["system","flathub","github"],"limit":30,"protocol_version":1}
```

Response:

```json
{"query":"gimp","results":[],"providers":[]}
```

Provider states: `loading`, `ready`, `stale`, `offline`, `rate-limited`, `misconfigured`, `failed`.

### catalog.appDetails

Request:

```json
{"id":"org.gimp.GIMP"}
```

Returns a canonical app object or `null`.

### operations.enqueue

Request:

```json
{"app_id":"org.gimp.GIMP","variant_id":"flatpak:org.gimp.GIMP","action":"install"}
```

Actions: `install`, `update`, `reinstall`, `remove`.

Returns the created operation.

### operations.list

Returns all persisted operations.

### operations.logs

Request:

```json
{"operationId":"01..."}
```

Returns sanitized operation logs.

### operations.cancel

MVP returns a structured unsupported response for active fake jobs.

### installed.list

MVP returns an empty list until UNI structured installed-state support is added.

### updates.list

MVP returns an empty list until UNI structured update support is added.

### apps.launch

MVP rejects launch requests until trusted desktop-entry lookup is implemented.

## Events

Long-lived clients receive notifications:

```json
{"jsonrpc":"2.0","method":"event.operationProgress","params":{"id":"01...","state":"downloading","percent":35}}
```

The browser frontend polls `operations.list`; long-lived Unix-socket clients can
consume the progress notifications directly.

## Native Qt transport

The Qt client keeps one `QLocalSocket` connection open for the lifetime of the
application. Its C++ bridge assigns request IDs, queues requests until the Rust
backend is ready, matches responses to QML callbacks, and forwards
`event.operationProgress` notifications. The client starts
`thallium-store-backend` as a child when no daemon is available; a helper exits
cleanly when another process already owns the per-user socket.

## Browser transport

`thallium-store-backend --web` embeds the desktop-independent frontend and
serves it on a random `127.0.0.1` port. `POST /api` accepts the same JSON-RPC
request objects documented above. A per-launch token, injected into the served
HTML and required in the `X-Thallium-Token` header, prevents unrelated web pages
from submitting package operations. The server sends no CORS opt-in headers.

When another backend already owns the Unix socket, the browser server proxies
requests to it. Otherwise it owns the Unix socket itself, so `--request` clients
and the browser share one catalog, operation scheduler, and SQLite connection.
