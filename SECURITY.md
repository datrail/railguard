# Security Policy

RailGuard enforces DatRail's rules on the network. rail-apigee, the gateway, judges each call to an MCP server against
Rail Center's rules; rail-envoy, the proxy, attaches its own ticket from Rail Center to the agent's calls. Two things
are worth attacking:

- **A call past the gateway that its rules refuse.** The highest-severity class of report we accept: every other
  component exists to put a trustworthy ticket and a policy in front of it.
- **A call attributed to an agent it didn't come from, or a ticket that escapes.** The gateway then enforces the wrong
  policy, and the audit trail records the wrong agent.

## Reporting a vulnerability

**Please don't open a public issue for a security problem.**

Use the repository's **Security** tab to open a private vulnerability report.

Include what an attacker can do (not only what is wrong), the version or commit, the smallest reproduction you have, and
whether you've told anyone else.

## What to expect

|                  |                                                |
| ---------------- | ---------------------------------------------- |
| Acknowledgement  | within 3 working days                          |
| First assessment | within 10 working days                         |
| Progress         | at least every 10 working days until it closes |

We ask for **90 days** before public disclosure, and will usually be much faster. You'll be credited unless you'd rather
not be. If we disagree that a report is a vulnerability, we'll say so.

## The gateway (rail-apigee)

Anything that gets a call past rail-apigee that its rules refuse:

- **a ticket read as what it isn't:** a signed ticket accepted though expired, malformed, or signed with a key Rail
  Center didn't publish, or for another issuer or audience than the bundle names;
- **a parser disagreement:** the gateway reading a ticket, a bundle or a call differently from Rail Center, or from the
  MCP server behind it, so what's judged isn't what's sent (Apigee splits a header on commas, so a ticket holding one
  reads as undecodable);
- **a hostile call or bundle** that crashes the gateway, or holds it past its deadline or its step budget (a judgement
  past 100,000 steps is refused, 503, at `enforce`);
- **a call forwarded unjudged** other than in the cases below;
- **header smuggling:** a call that presents differently to the gateway than to the MCP server. The gateway removes
  every `x-rail` and `x-rail-*` header, and every header whose name has an `_`, whatever the verdict, so an MCP server
  never sees the ticket;
- **the denial reports:** anything that suppresses or forges them, since they're the audit trail. Past 64 reports in
  flight, 16 of them naming no policy, a report is dropped and logged; the call stays denied.

A refusal names no policy: 403 for a denial, 503 for a call the rules can't be applied to. On Apigee, a request past 4
MiB less 1 KiB is refused unjudged, 503. If the callout is down or answers no decision, the reference Apigee proxy
refuses the call, 503.

**The callout's port.** It receives every header and the body, bearer token included, and logs no header value.
rail-apigee authenticates no caller, so only Apigee may reach it: a caller could otherwise ask for verdicts, and send
denial reports under this gateway's name. On Cloud Run, grant `roles/run.invoker` to the Apigee proxy's service account
alone. Elsewhere, keep it reachable from Apigee only.

**Not a report: forwarded unjudged, by design.**

- every call while `RAIL_DISABLED=true`, at enforcement `none`, or at `observe`, which only logs what it would deny;
- a request other than a `POST`, and a session message (`initialize`, `tools/list`, a notification);
- a call that arrives before the first bundle is held, after waiting 2 s for it, logged as an error;
- a call not judged within 5 s, or whose judgement panicked, logged as an error;
- an unsigned ticket, read as it is even where tickets are signed: anything that reaches the gateway can present one, so
  it's only as good as the path from the proxy.

## The proxy (rail-envoy)

- **The identity boundary.** No `x-rail` or `x-rail-*` header the agent wrote, nor any header whose name has an `_`
  (many servers read `x_rail` as `x-rail`), may reach an upstream: rail-envoy removes them from every request. The
  agent's other headers, `Authorization` included, are forwarded as sent, by design. An Envoy config that fails open
  must remove them too, before the callout, as the reference configs in `e2e/envoy` do. An agent-written `x-rail` header
  reaching an upstream is a report.
- **The ticket.** It's a bearer credential for its lifetime. It must not reach a log, an error, or any host but a
  protected one. It's attached only for a host in `RAIL_PROXY_PROTECTED_HOSTS`, at the address Envoy resolves for that
  name, over TLS unless the entry says `http://`; the reference configs check the certificate against that name. An
  entry without a port sends it to whatever port the agent names on that host. A ticket reaching a host the config
  didn't name is a report.
- **The socket.** Whatever can connect to it can ask what rail-envoy would attach to a request, ticket included. It's
  made `0660`, for rail-envoy's own user and group, and there's no TCP port, since anything on localhost in a pod is
  reachable by the agent. Envoy reaches it by sharing the group: don't put the agent in it.

## Both hosts

- **The Rail Center fetch.** It carries the service token and the denial reports out, and a ticket, a bundle or the
  signing keys back. Its client refuses an `http://` Rail Center unless `RAIL_CENTER_ALLOW_HTTP=true`, reads no proxy
  setting from the environment, follows no redirect, caps what it reads, before and after decoding, and bounds the whole
  fetch. A way past one of these is a report.
- **The log.** Each field is quoted, so a call's text can't forge a line, and a secret is logged only as `set` or
  `unset`. The ticket is logged only as a fingerprint. A ticket, a token, or a header value other than the host reaching
  the log is a report.

## Scope

In scope: this repository, its images, and the `datrail-policy` package built from it.

Out of scope:

- vulnerabilities in our dependencies: report those upstream, and we'll help;
- rules that are permissive because they were written that way;
- a setting that weakens a host by design (`RAIL_DISABLED=true`, an `http://` Rail Center or protected host), each
  logged as a warning at startup, or an Envoy or Apigee config that leaves out what the hosts' READMEs say it must do.
