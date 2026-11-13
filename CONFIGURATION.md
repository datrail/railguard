# Configuration

Every setting `rail-apigee` and `rail-envoy` read, in one place. See the [main README](README.md) for what each host
does.

Each host reads its settings from environment variables. A blank value is unset; a value is trimmed, since one written
to a file keeps its trailing newline. Every refusal is printed at once and the host exits 2, so all of them are fixed in
one round. Each setting it uses is logged at startup, a secret only as `set` or `unset`, and a `RAIL_…` variable that
nothing reads is logged as a likely typo. A boolean is `true` or `false`, in any case.

## Both hosts

### `RAIL_DISABLED`

Runs the host without Rail Center: nothing is fetched from it and nothing reported to it.

- **Default:** `false`.
- **Accepted:** a boolean.
- **Read by:** both hosts.
- Both hosts are then ready at once. `rail-apigee` allows every call it can answer. `rail-envoy`'s proxy still routes
  the protected hosts, but sets neither `x-rail` nor `x-rail-status`; its gateway forwards every call, with `x-rail`,
  `x-rail-*` and `_` headers removed.
- The other settings are still judged if they're set, so a mistake in one shows before Rail Center is turned on. It's
  logged as a warning when `true`.

### `RAIL_CENTER_URL`

Where Rail Center is: the base URL each route, such as `/v1/policy-bundle`, is appended to.

- **Required** unless `RAIL_DISABLED=true`.
- **Accepted:** an `http://` or `https://` URL naming a host, in printable ASCII, with no credential, query or fragment,
  and no percent-encoded host. A trailing `/` is dropped.
- **Read by:** both hosts.
- **`http://` is refused** unless `RAIL_CENTER_ALLOW_HTTP=true`: the credential and what Rail Center returns would cross
  in the clear.
- A host other URL parsers would read differently (`0x7f.1`, which is `127.0.0.1`) is refused, so the host reached is
  the one read in the value. A refusal never quotes the value, which may hold a credential.

### `RAIL_CENTER_ALLOW_HTTP`

Allows an `http://` `RAIL_CENTER_URL`, for a local stack or a network that's private end to end.

- **Default:** `false`.
- **Accepted:** a boolean.
- **Read by:** both hosts.
- An `http://` URL is still logged as a warning.

### `RAIL_AUTH_MODE`

Which credential is sent to Rail Center.

- **Default:** `none`.
- **Accepted:** `none` or `bearer`, in any case.
- **Read by:** both hosts.
- `none` with `RAIL_AUTH_TOKEN` set is refused, so a token isn't ignored by mistake. Another value is refused without
  being quoted, since it may be a token pasted into the wrong variable.

### `RAIL_AUTH_TOKEN`

**A secret.** The Rail Center service token sent as a bearer credential under `RAIL_AUTH_MODE=bearer`.

- **Required** under `RAIL_AUTH_MODE=bearer`, unless `RAIL_DISABLED=true`.
- **Accepted:** what Rail Center mints for a component: `rc_svc_` then 43 URL-safe base64 characters.
- **Read by:** both hosts.
- A refusal says where the token is wrong without quoting any of it.

### `RAIL_LOG_LEVEL`

The level of the hosts' own log lines, on stderr.

- **Default:** `info`.
- **Accepted:** `error`, `warn`, `info`, `debug` or `trace`, in any case.
- **Read by:** both hosts.
- The libraries' lines stay at `warn`: below it they log the wire itself, reqwest's and rustls' included. At `debug`,
  `rail-envoy` logs each request's decision: the host, and for a protected one the header set (`x-rail`, or
  `x-rail-status` with its reason), the route and where it connects; and the headers removed. Never the ticket.

### `RAIL_LOG_FORMAT`

How each log line is written, on stderr.

- **Default:** `json`, or `gcp` on Cloud Run (`K_SERVICE` or `CLOUD_RUN_JOB` set), logged as `source="detected"`. A
  value set always wins. Nothing else is detected: on GKE or GCE, set `gcp` for Cloud Logging to read each line's
  severity.
- **Accepted:** `text`, `json` or `gcp`, in any case.
- **Read by:** both hosts, before anything is logged, so the `starting` line is already in it; an invalid value is
  refused, its refusal written in the default format. `rail probe` uses it too, but doesn't judge it: it falls back to
  the default.
- `text` is the message then `name="value"` fields. `json` and `gcp` write one JSON object per line: `timestamp` (RFC
  3339, UTC; `time` in `gcp`, the name Cloud Logging reads it by), the level, `target`, `message`, then each field, a
  number or a bool bare. `json` names the level `level` (`ERROR`, `WARN`, `INFO`, `DEBUG`, `TRACE`); `gcp` names it
  `severity`, as Cloud Logging reads it (`WARN` as `WARNING`, `TRACE` as `DEBUG`).
- In either JSON format, hidden characters and `\` are escaped as in `text`, `"` is left to JSON, and a field is cut at
  1024 characters, but not the message. A field named `message`, `timestamp`, `level`, `severity`, `target`, or one
  Cloud Logging reads (`time`, `timestampSeconds`, `timestampNanos`, `httpRequest`, or under `logging.googleapis.com/`),
  or already starting `field.`, is written `field.<name>`. Each refusal is an `ERROR` line of its own,
  `invalid configuration` with the reason in `refusal`, and a usage error is one too, `invalid arguments` with the
  reason in `reason`.

### `RAIL_CALLOUT_PORT`

The TCP port the callout and its health checks are served on, on every interface; `rail probe` asks `127.0.0.1`.

- **Default:** `8080` in `rail-apigee`; none in `rail-envoy`, which needs exactly one of it and `RAIL_EXT_SOCKET`.
- **Accepted:** `1` to `65535`.
- **Read by:** both hosts; it's what `rail probe` judges to ask the server (with `RAIL_EXT_SOCKET` in `rail-envoy`).
- Whatever reaches it reaches the callout: in `rail-envoy`'s proxy role, that's the ticket, so only Envoy may reach it.

## The gateway

Read by `rail-apigee`, and by `rail-envoy` as the gateway (`rail gateway`).

### `RAIL_CALLOUT_GRPC_MAX_MESSAGE_MB`

The largest gRPC message read, in MiB: from Apigee, a whole request; from Envoy, one ext_authz request, or one ext_proc
message, which holds a whole buffered body.

- **Default:** `16`, above Apigee's 10 MB request limit.
- **Accepted:** `1` to `100`, a whole number of MiB; the maximum is high enough that no limit a deployment already sets
  is refused.
- **Read by:** both hosts' gateways.
- **In `rail-apigee`, past 4 MiB less 1 KiB, a request is refused unjudged** (413, with a warning logged), whatever this
  allows: Apigee takes a callout's answer of at most 4 MiB, and the answer echoes the body. An answer may be up to 1 KiB
  larger than the request.
- **In `rail-envoy`, a message past it** fails its stream while the call is unanswered, so Envoy fails closed
  (`failure_mode_allow: false`).

### `RAIL_GATEWAY_SLUG`

Which gateway this is in Rail Center, and so whose policy bundle it fetches.

- **Required** unless `RAIL_DISABLED=true`.
- **Accepted:** ASCII letters, digits, `-` and `_`, up to 64 characters, as Rail Center's slugs are; another it would
  refuse on each fetch.
- **Read by:** both hosts' gateways.

### `RAIL_GATEWAY_BUNDLE_REFRESH_SECONDS`

How often the bundle held is fetched again.

- **Default:** `60`, so a rule changed in Rail Center reaches every gateway within about a minute.
- **Accepted:** `5` to `86400`, whole seconds: a bundle polled less often than daily is surely a mistake.
- **Read by:** both hosts' gateways.
- Each wait is scaled by 0.8 to 1.2, so a fleet doesn't poll in step. After a failure, the next attempt comes in 5 s,
  doubling to at most 320 s or the interval, whichever is less. The 5 s floor isn't a setting: a lower one would let a
  fleet hammer Rail Center.

### `RAIL_MAX_BODY_MB`

The largest request body judged, in MiB, as sent (an encoded body isn't decoded).

- **Default:** `15`.
- **Accepted:** `1` to `100`, a whole number of MiB; in `rail-envoy`, below `RAIL_CALLOUT_GRPC_MAX_MESSAGE_MB`, since a
  buffered body arrives as one message, with the request's headers.
- **Read by:** both hosts' gateways.
- **Past it, a call is refused unjudged,** 413 `{"error": "request body too large"}`, with a warning logged, and isn't
  reported; with `RAIL_DISABLED=true`, nothing is judged, and this isn't applied. Apigee's own limits come first: past 4
  MiB less 1 KiB rail-apigee refuses a request (above), and past 10 MB Apigee answers 413 itself, so a value of 4 or
  more is logged with the limit in force. In `rail-envoy`, a body streamed in full duplex is refused as soon as it's
  past it.

## rail-envoy

### `RAIL_EXT_SOCKET`

The unix socket ext_authz, ext_proc and the health checks are served on, for an Envoy sharing it.

- **Required** unless `RAIL_CALLOUT_PORT` is set, and refused with it: exactly one says where it listens. No default, so
  a deployment can't serve on a socket where it meant a port; the image sets none either. To share it with Envoy in the
  image's `/run/rail` volume, set `/run/rail/ext.sock`.
- **Accepted:** an absolute path.
- **Read by:** `rail-envoy`, either role; with `RAIL_CALLOUT_PORT`, the only settings `rail probe` judges, to ask the
  server.

### `RAIL_PROXY_PROTECTED_HOSTS`

The hosts the ticket is attached for, and how a request to each is routed.

- **Required** unless `RAIL_DISABLED=true`: the proxy attaches the agent's identity, so it must name where to send it.
  With Rail Center disabled, it's still read and routed.
- **Accepted:** comma-separated entries, each `[http(s)://]host[:port]`: HTTPS unless it says `http://`, and the entry's
  port if it names one, else the agent's. A host is a name or an IP address; a port is `1` to `65535`.
- **Read by:** `rail-envoy` as the proxy (`rail proxy`).
- **Refused:** an empty entry, a path, another scheme, and a host listed twice, since a host is matched without its
  scheme or port. A credential, query or fragment is refused without being quoted, since it may hold a secret.
- Over `http://`, the ticket crosses in the clear.

### `RAIL_HOST_ID`

Which host this proxy is in Rail Center, for the ticket fetch.

- **Required** unless `RAIL_DISABLED=true`.
- **Accepted:** up to 64 characters, Rail Center's bound.
- **Read by:** `rail-envoy` as the proxy (`rail proxy`).

### `RAIL_SANDBOX_NAME`

The one sandbox this proxy serves, for the ticket fetch.

- **Required** unless `RAIL_DISABLED=true`.
- **Accepted:** up to 255 characters, Rail Center's bound.
- **Read by:** `rail-envoy` as the proxy (`rail proxy`).

### `RAIL_TICKET_REFRESH_SECONDS`

The longest a valid ticket goes without being fetched again.

- **Default:** `3600`.
- **Accepted:** `5` to `86400`, whole seconds: a ticket lives no longer than a day.
- **Read by:** `rail-envoy` as the proxy (`rail proxy`).
- A ticket is fetched again at half its remaining life when that comes sooner, so a short one is replaced before it
  lapses. The waits are scaled and backed off as the bundle's are (`RAIL_GATEWAY_BUNDLE_REFRESH_SECONDS`).

## Retired names

Each is refused when set, rather than ignored, so a deployment carrying one over can't quietly take the default instead.

| Name                                   | Instead                                                          | Read by     |
| -------------------------------------- | ---------------------------------------------------------------- | ----------- |
| `RAIL_GATEWAY_PORT`                    | `RAIL_CALLOUT_PORT`                                              | rail-apigee |
| `RAIL_GATEWAY_GRPC_MAX_MESSAGE_MB`     | `RAIL_CALLOUT_GRPC_MAX_MESSAGE_MB`                               | rail-apigee |
| `RAIL_GATEWAY_LOG_LEVEL`               | `RAIL_LOG_LEVEL`                                                 | both hosts  |
| `RAIL_PROXY_LOG_LEVEL`                 | `RAIL_LOG_LEVEL`                                                 | both hosts  |
| `RAIL_PLUGIN_ENABLED`                  | `RAIL_DISABLED`: Rail Center is used unless it's `true`          | both hosts  |
| `RAIL_PROXY_ALLOW_INSECURE_CREDENTIAL` | `RAIL_CENTER_ALLOW_HTTP`: `true` allows an `http://` Rail Center | both hosts  |
