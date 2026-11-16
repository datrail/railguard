# The gateway's decisions

What the gateway, `rail-apigee` or `rail-envoy` as `rail gateway`, does with each request, in the order it decides. Each
host first checks what only it can, then hands the call to the steps both share. See the [main README](README.md) for
the system around it, and [`CONFIGURATION.md`](CONFIGURATION.md) for the settings named.

Outcomes are coloured by what happens to the call: green forwarded, red denied (403), orange the rules can't be applied
(503), yellow too large (413), grey a gRPC error the host acts on.

## rail-envoy

```mermaid
flowchart TD
  arrive([A request arrives])
  grpc{"Its gRPC stream fails?"}
  disabled{RAIL_DISABLED}
  whole{"The whole body gathered?"}
  both([Both hosts, below])
  e1["A gRPC error:<br>Envoy's failure_mode_allow decides"]:::grpcError
  f1[Forwarded unjudged]:::forwarded
  r1[503]:::cannotApply
  r2[413]:::tooLarge

  arrive --> grpc
  grpc -->|"yes: a message past RAIL_CALLOUT_GRPC_MAX_MESSAGE_MB,<br>ext_proc in observability mode,<br>or a stream ending before its answer"| e1
  grpc -->|no| disabled
  disabled -->|true| f1
  disabled -->|false| whole
  whole -->|"no: ext_proc's body mode isn't full duplex or buffered,<br>ext_proc sent the body before its headers,<br>or ext_authz sent a POST without its body"| r1
  whole -->|"no: ext_authz sent part of the body,<br>or a body streamed past RAIL_MAX_BODY_MB"| r2
  whole -->|yes| both

  classDef forwarded fill:#d4edda,stroke:#2e7d32,color:#1b5e20
  classDef denied fill:#f8d7da,stroke:#c62828,color:#7f0000
  classDef cannotApply fill:#ffe0b2,stroke:#ef6c00,color:#7a3e00
  classDef tooLarge fill:#fff9c4,stroke:#f9a825,color:#5d4a00
  classDef grpcError fill:#e0e0e0,stroke:#616161,color:#212121
```

## rail-apigee

```mermaid
flowchart TD
  arrive([A request arrives])
  grpc{"Its message past<br>RAIL_CALLOUT_GRPC_MAX_MESSAGE_MB?"}
  answerable{"The whole request past 4 MiB less 1 KiB,<br>more than an answer can carry back?"}
  disabled{RAIL_DISABLED}
  both([Both hosts, below])
  e1["A gRPC error:<br>the proxy's FaultRule answers 503"]:::grpcError
  r1[413]:::tooLarge
  f1[Forwarded unjudged]:::forwarded

  arrive --> grpc
  grpc -->|yes| e1
  grpc -->|no| answerable
  answerable -->|yes| r1
  answerable -->|no| disabled
  disabled -->|true| f1
  disabled -->|false| both

  classDef forwarded fill:#d4edda,stroke:#2e7d32,color:#1b5e20
  classDef denied fill:#f8d7da,stroke:#c62828,color:#7f0000
  classDef cannotApply fill:#ffe0b2,stroke:#ef6c00,color:#7a3e00
  classDef tooLarge fill:#fff9c4,stroke:#f9a825,color:#5d4a00
  classDef grpcError fill:#e0e0e0,stroke:#616161,color:#212121
```

## Both hosts: whether a call is judged

```mermaid
flowchart TD
  start([From either host])
  cap{"Body past RAIL_MAX_BODY_MB?"}
  post{POST?}
  subgraph deadline["Within 5 s from the bundle wait on, a wait for a free judging slot and the judging below included"]
    bundle{"A bundle held,<br>waiting up to 2 s?"}
    key["Read x-rail; for a signed ticket, wait up to 2 s for its key,<br>if the bundle names an issuer and an audience<br>and enforcement isn't none"]
    session{"A session message?<br>initialize, ping, tools/list, prompts/list,<br>resources/list, resources/templates/list, notifications/…"}
    mode{enforcement}
    judged([Judged, below])
  end
  r1[413]:::tooLarge
  f1[Forwarded unjudged]:::forwarded
  f2["Forwarded unjudged,<br>logged as an error"]:::forwarded
  f3[Forwarded unjudged]:::forwarded
  f4[Forwarded unjudged]:::forwarded
  r2["503, logged as an error"]:::cannotApply
  f5["Forwarded, logged as an error"]:::forwarded

  start --> cap
  cap -->|yes| r1
  cap -->|no| post
  post -->|no| f1
  post -->|yes| bundle
  bundle -->|no| f2
  bundle -->|yes| key
  key --> session
  session -->|yes| f3
  session -->|no| mode
  mode -->|none| f4
  mode -->|observe or enforce| judged
  deadline -->|"past it, or judging panicked,<br>at enforce"| r2
  deadline -->|"past it, or judging panicked,<br>at observe or none"| f5

  style deadline fill:#eceff1,stroke:#90a4ae,color:#263238

  classDef forwarded fill:#d4edda,stroke:#2e7d32,color:#1b5e20
  classDef cannotApply fill:#ffe0b2,stroke:#ef6c00,color:#7a3e00
  classDef tooLarge fill:#fff9c4,stroke:#f9a825,color:#5d4a00
```

## Both hosts: how a call is judged

```mermaid
flowchart TD
  subgraph deadline["Within the same 5 s, as above"]
    start([Judged, at observe or enforce])
    ticket{"A valid ticket?"}
    endpoint{"What the call names"}
    bf{binding_fallback}
    chain["Every enabled policy"]
    bound["The binding's enabled policies"]
    walk{"Walked in priority order,<br>each alert policy that holds logged"}
    deny{"Denied, at"}
    refused{"Refused, at"}
    allow[Allowed]
  end
  r1["403, reported to Rail Center"]:::denied
  f1["Forwarded, logged as it would be denied"]:::forwarded
  r2["503, logged as an error"]:::cannotApply
  f2["Forwarded, logged as an error"]:::forwarded
  f3[Forwarded]:::forwarded

  start --> ticket
  ticket -->|"no, and ticket_fallback is block"| deny
  ticket -->|"yes, or ticket_fallback is pass"| endpoint
  endpoint -->|"another method, naming no tool"| chain
  endpoint -->|"a tool whose key is bound, gated"| bound
  endpoint -->|"a tool whose key is bound, open"| allow
  endpoint -->|"a tool with no binding,<br>or nothing readable"| bf
  bf -->|block| deny
  bf -->|pass| chain
  chain --> walk
  bound --> walk
  walk -->|"a block policy holds"| deny
  walk -->|"a condition can't be read,<br>or 100,000 steps spent"| refused
  walk -->|"no block policy holds"| allow
  deny -->|enforce| r1
  deny -->|observe| f1
  refused -->|enforce| r2
  refused -->|observe| f2
  allow --> f3

  style deadline fill:#f8f9fa,stroke:#cfd8dc,color:#455a64

  classDef forwarded fill:#d4edda,stroke:#2e7d32,color:#1b5e20
  classDef denied fill:#f8d7da,stroke:#c62828,color:#7f0000
  classDef cannotApply fill:#ffe0b2,stroke:#ef6c00,color:#7a3e00
```

A call forwarded goes on without the agent's `x-rail`, `x-rail-*` and `_` headers. A refusal names no policy: 403
`{"error": "denied by policy"}`, 503 `{"error": "policy ruleset cannot be applied"}`, 413
`{"error": "request body too large"}`.
