"""Checks rail-apigee on its own, then through the stand-in for an Apigee proxy running apiproxy/;
or, with E2E_TARGET=live (live/session.sh test), through real Apigee, followed by what Apigee is
seen to do where the stand-in can't say.

Assertions read WireMock's request journals, not logs: a log says what rail-apigee believes it
did, a journal says it happened. "live §9" is what the Python gateway's runs on real Apigee found.
"""

import base64
import gzip
import http.client
import json
import os
import ssl
import time
import urllib.error
import urllib.parse
import urllib.request

import grpc
from external_callout_pb2 import MessageContext, Request, Strings
from external_callout_pb2_grpc import ExternalCalloutServiceStub
from grpc_health.v1 import health_pb2, health_pb2_grpc

_LIVE = os.environ.get("E2E_TARGET", "local") == "live"
_UPSTREAM = os.environ.get("E2E_UPSTREAM", "http://upstream:8080")
_RAIL_CENTER = os.environ.get("E2E_RAIL_CENTER", "http://rail-center:8080")
_CALLOUT = "callout:8080"
_UNREACHABLE = "callout-unreachable:9100"
_ENFORCE = os.environ.get("E2E_ENFORCE_URL", "http://apigee:8080/mcp")
_NO_CALLOUT = os.environ.get("E2E_NO_CALLOUT_URL", "http://apigee:8080/no-callout")
_READY = "datrail.gateway.Ready"
_TIMEOUT_SECONDS = 15
# http.client sends a body in one call, which the timeout bounds whole.
_UPLOAD_TIMEOUT_SECONDS = 300

# Unsigned base64url JSON, expiring in 2100, in the nested shape Rail Center stores; its skill is
# the full key Rail Center mints, `<slug>#<path>#<method>#<call>`.
_GOOD_TICKET = (
    "eyJpZCI6ICI1YzhmMWU0Mi0wMDAwLTQwMDAtODAwMC0wMDAwMDAwMDAwYzEiLCAiYWdlbnRfaWQiOiAiNWM4ZjFlND"
    "ItMDAwMC00MDAwLTgwMDAtMDAwMDAwMDAwMGUyIiwgInBvc3R1cmUiOiB7Im92ZXJhbGwiOiB7InNjb3JlIjogOTAs"
    "ICJsZXZlbCI6ICJsb3cifSwgImNhdGVnb3JpZXMiOiB7fX0sICJhbGlnbm1lbnQiOiB7ImRyaWZ0ZWQiOiBbXX0sIC"
    "Jza2lsbHMiOiBbImRlbGl2ZXJ5Iy9tY3AjdG9vbHMvY2FsbCN0cmFja19wYWNrYWdlIl0sICJleHAiOiA0MTAyNDQ0"
    "ODAwfQ"
)
# P2 in rail-center-mappings/01-policy-bundle.json: the endpoint rule.
_P2 = "11111111-0000-4000-8000-000000000002"
# A fake agent credential: Apigee must forward it as sent.
_AGENT = {"Authorization": "Bearer e2e-agent"}
_REFUSED = '{"error": "denied by policy"}'
_CALLOUT_FAILED = '{"error": "policy ruleset cannot be applied"}'
_OPERATOR_FAULT = '{"error": "e2e operator fault"}'
# Apigee's own fault for a body past its limit.
_TOO_BIG = "protocol.http.TooBigBody"
_MCP_HEADERS = {
    "Accept": "application/json, text/event-stream",
    "Content-Type": "application/json",
}
_INITIALIZE = {
    "jsonrpc": "2.0",
    "id": 1,
    "method": "initialize",
    "params": {
        "protocolVersion": "2025-06-18",
        "capabilities": {},
        "clientInfo": {"name": "e2e-driver", "version": "1"},
    },
}
_CALL = {"jsonrpc": "2.0", "id": 9, "method": "tools/call", "params": {"name": "track_package"}}

# The live load balancer's certificate is self-signed; the stubs' are publicly trusted.
_TLS = ssl.create_default_context()
if os.environ.get("E2E_CA_FILE"):
    _TLS.load_verify_locations(os.environ["E2E_CA_FILE"])
# The live stubs lock /__admin with basic auth.
_ADMIN_HEADERS = {}
if os.environ.get("E2E_STUB_ADMIN_PASSWORD"):
    _credential = base64.b64encode(f"e2e:{os.environ['E2E_STUB_ADMIN_PASSWORD']}".encode())
    _ADMIN_HEADERS["Authorization"] = f"Basic {_credential.decode()}"

_fails = 0
# Set by any sweep: each reset sweeps first, so the last check covers the whole run.
_unmatched = {_UPSTREAM: 0, _RAIL_CENTER: 0}


def _report_ok(what):
    print(f"  ok    {what}")


def _report_fail(message):
    global _fails
    _fails += 1
    print(f"  FAIL  {message}")


def _expect(what, want, got):
    if got is not None and str(got) == str(want):
        _report_ok(what)
    else:
        _report_fail(f"{what}: wanted {want}, got {'<empty>' if got is None else got}")


# What Apigee was seen to do, which no assertion holds it to yet.
def _report_seen(what, seen):
    print(f"  seen  {what}: {seen}")


def _start_block(title):
    print(f"\n== {title} ==")


def _send_json(method, url, body=None, headers=None):
    """The response, open. Raises `HTTPError` on anything but a 2xx."""
    request = urllib.request.Request(
        url,
        data=None if body is None else json.dumps(body).encode(),
        method=method,
        headers={"Content-Type": "application/json", **(headers or {})},
    )
    return urllib.request.urlopen(request, timeout=_TIMEOUT_SECONDS, context=_TLS)


# Raises on failure: a reset that silently failed would leave the last block's traffic behind.
def _ask_admin(method, base, path, body=None):
    with _send_json(method, base + path, body, _ADMIN_HEADERS) as response:
        text = response.read().decode()
    return json.loads(text) if text else None


def _count(base, pattern):
    """How many requests the stub at `base` answered that match `pattern`, or None."""
    try:
        return _ask_admin("POST", base, "/__admin/requests/count", pattern)["count"]
    except (OSError, ValueError, KeyError):
        return None


# Session messages are forwarded unjudged; a refusal must stop the `tools/call`.
def _count_forwarded_tool_calls():
    return _count(
        _UPSTREAM,
        {
            "method": "POST",
            "urlPath": "/mcp",
            "bodyPatterns": [{"matchesJsonPath": "$[?(@.method == 'tools/call')]"}],
        },
    )


def _count_upstream_requests():
    return _count(_UPSTREAM, {"method": "ANY", "urlPattern": ".*"})


def _count_denials():
    return _count(_RAIL_CENTER, {"method": "POST", "urlPath": "/v1/denials"})


def _count_bundle_fetches():
    return _count(_RAIL_CENTER, {"method": "GET", "urlPath": "/v1/policy-bundle"})


def _count_denials_naming(policy):
    pattern = f"$[?(@.policy_id == '{policy}')]"
    return _count(
        _RAIL_CENTER,
        {
            "method": "POST",
            "urlPath": "/v1/denials",
            "bodyPatterns": [{"matchesJsonPath": pattern}],
        },
    )


# A fallback refusal omits `policy_id`; this matches its absence, not a null.
def _count_denials_naming_no_policy():
    return _count(
        _RAIL_CENTER,
        {
            "method": "POST",
            "urlPath": "/v1/denials",
            "bodyPatterns": [{"matchesJsonPath": "$[?(!@.policy_id)]"}],
        },
    )


def _read_denials():
    """The denial reports Rail Center's stub received, newest first; [] if it couldn't be asked."""
    try:
        requests = _ask_admin("GET", _RAIL_CENTER, "/__admin/requests")["requests"]
    except (OSError, ValueError, KeyError):
        return []
    return [
        json.loads(r["request"]["body"]) for r in requests if r["request"]["url"] == "/v1/denials"
    ]


def _wait_for(read, want, tries=40, interval=0.25):
    """What `read` last returned, once it reached `want` or `tries` ran out."""
    for _ in range(tries):
        got = read()
        if (got or 0) >= want:
            return got
        time.sleep(interval)
    return got


# Denials are reported after the caller is answered, so each is waited for.
def _wait_for_last_denial():
    """The newest denial report, or {} if none arrived."""
    _wait_for(lambda: len(_read_denials()), 1)
    return (_read_denials() or [{}])[0]


def _read_received_headers():
    """The headers of each request the upstream logged, newest first, names lower-cased."""
    try:
        requests = _ask_admin("GET", _UPSTREAM, "/__admin/requests")["requests"]
    except (OSError, ValueError, KeyError):
        return None
    return [
        {name.lower(): value for name, value in r["request"]["headers"].items()} for r in requests
    ]


# 1 if any request found no stub, which no count of matched requests would show.
def _has_unmatched(base):
    try:
        requests = _ask_admin("GET", base, "/__admin/requests/unmatched")["requests"]
    except (OSError, ValueError, KeyError):
        return 1
    return 0 if not requests else 1


def _sweep_unmatched():
    for base in _unmatched:
        _unmatched[base] |= _has_unmatched(base)


def _reset_journals():
    _sweep_unmatched()
    _clear_journals()


# Without the sweep: what's only seen can't fail the run.
def _clear_journals():
    _ask_admin("DELETE", _UPSTREAM, "/__admin/requests")
    _ask_admin("DELETE", _RAIL_CENTER, "/__admin/requests")


def _read_last_received_body():
    """The body of the newest request the upstream logged, as bytes, or None."""
    try:
        requests = _ask_admin("GET", _UPSTREAM, "/__admin/requests")["requests"]
        return base64.b64decode(requests[0]["request"]["bodyAsBase64"])
    except (OSError, ValueError, KeyError, IndexError):
        return None


def _send_raw_post(url, body, headers, timeout=_TIMEOUT_SECONDS):
    """The status and body of a POST of `body` (bytes) with `headers`, a list of (name, value)
    pairs, so a header can repeat; (None, "") if nothing answered."""
    parts = urllib.parse.urlsplit(url)
    if parts.scheme == "https":
        connection = http.client.HTTPSConnection(parts.netloc, timeout=timeout, context=_TLS)
    else:
        connection = http.client.HTTPConnection(parts.netloc, timeout=timeout)
    try:
        connection.putrequest("POST", parts.path + (f"?{parts.query}" if parts.query else ""))
        for name, value in [*_MCP_HEADERS.items(), *headers]:
            connection.putheader(name, value)
        connection.putheader("Content-Length", str(len(body)))
        connection.endheaders(body)
        response = connection.getresponse()
        return response.status, response.read().decode(errors="replace")
    except OSError:
        return None, ""
    finally:
        connection.close()


def _post_mcp(url, payload, headers):
    """The status, body and headers of one MCP POST, whatever the status."""
    try:
        with _send_json("POST", url, payload, {**_MCP_HEADERS, **headers}) as response:
            return response.status, response.read().decode(), response.headers
    except urllib.error.HTTPError as error:
        return error.code, error.read().decode(), error.headers


# The upstream is stateful: every call after `initialize` carries its session id.
def _open_session(url, headers):
    """A session id from a full handshake at `url`, or None if it failed."""
    try:
        status, _, answer = _post_mcp(url, _INITIALIZE, headers)
        sid = answer.get("Mcp-Session-Id") if status == 200 else None
        if not sid:
            return None
        status, _, _ = _post_mcp(
            url,
            {"jsonrpc": "2.0", "method": "notifications/initialized"},
            {"Mcp-Session-Id": sid, **headers},
        )
    except OSError:
        return None
    return sid if 200 <= status < 300 else None


# `initialize` names no endpoint, so it's forwarded without a policy walk.
def _get_handshake_status(url, headers):
    try:
        return _post_mcp(url, _INITIALIZE, headers)[0]
    except OSError:
        return None


def _call_tool(url, tool, arguments=None, headers=None, request_id=9):
    """The status and body of a `tools/call`; the body is SSE-framed."""
    payload = {
        "jsonrpc": "2.0",
        "id": request_id,
        "method": "tools/call",
        "params": {"name": tool, "arguments": arguments or {}},
    }
    try:
        status, body, _ = _post_mcp(url, payload, headers or {})
    except OSError:
        return None, ""
    return status, body


# No session needed: refusals are answered before the MCP layer.
def _get_call_status(url, tool, headers=None):
    return _call_tool(url, tool, headers=headers)[0]


def _check_health(target, service=""):
    """The status's name, or the gRPC error's code."""
    with grpc.insecure_channel(target) as channel:
        stub = health_pb2_grpc.HealthStub(channel)
        try:
            answer = stub.Check(health_pb2.HealthCheckRequest(service=service), timeout=5)
        except grpc.RpcError as error:
            return error.code().name
    return health_pb2.HealthCheckResponse.ServingStatus.Name(answer.status)


def _process(target, verb, content="", uri="/mcp", ticket=None):
    """The flow variables rail-apigee set."""
    headers = {} if ticket is None else {"x-rail": Strings(strings=[ticket])}
    context = MessageContext(request=Request(verb=verb, uri=uri, content=content, headers=headers))
    with grpc.insecure_channel(target) as channel:
        answer = ExternalCalloutServiceStub(channel).ProcessMessage(context, timeout=5)
    return {name: value.string for name, value in answer.additional_flow_variables.items()}


# rail-apigee on its own: its health, its port, and a call judged.
def _run_callout_cases():
    _start_block("enrolled: holds the bundle and judges a call")
    _expect("a bundle was fetched at startup", 1, _wait_for(_count_bundle_fetches, 1))
    _expect("liveness", "SERVING", _check_health(_CALLOUT))
    _expect("readiness", "SERVING", _check_health(_CALLOUT, _READY))
    _expect("a GET is allowed", "allow", _process(_CALLOUT, "GET").get("rail.decision"))
    refused = _process(_CALLOUT, "POST", json.dumps(_CALL))
    _expect("a call with no ticket is refused", "refuse", refused.get("rail.decision"))
    _expect("with 403", "403", refused.get("rail.status"))
    _expect("and the refusal's body", _REFUSED, refused.get("rail.body"))
    _expect(
        "the denial is reported, naming no policy", 1, _wait_for(_count_denials_naming_no_policy, 1)
    )

    forbidden = json.dumps({**_CALL, "params": {"name": "forbidden_tool"}})
    decoded = _process(_CALLOUT, "POST", forbidden, "/m%63p", _GOOD_TICKET)
    _expect("a percent-encoded path is decoded, so P2 denies it", "403", decoded.get("rail.status"))
    broken = json.dumps({**_CALL, "params": {"name": "broken_tool"}})
    broken = _process(_CALLOUT, "POST", broken, ticket=_GOOD_TICKET)
    _expect("a policy it can't apply is refused", "refuse", broken.get("rail.decision"))
    _expect("with 503", "503", broken.get("rail.status"))
    _expect("and the refusal's body", _CALLOUT_FAILED, broken.get("rail.body"))

    _start_block("unreachable: serves on the port it was given, and isn't ready")
    _expect("liveness on 9100", "SERVING", _check_health(_UNREACHABLE))
    _expect("readiness on 9100", "NOT_SERVING", _check_health(_UNREACHABLE, _READY))


# Through the stand-in: what Apigee does with rail-apigee's answers.
def _run_apigee_cases():
    good = {"x-rail": _GOOD_TICKET, **_AGENT}
    call = json.dumps(_CALL).encode()

    _start_block("enforce: discovery passes, and a call with no ticket is refused")
    _reset_journals()
    _expect("initialize isn't refused", 200, _get_handshake_status(_ENFORCE, _AGENT))
    code, body = _call_tool(_ENFORCE, "track_package", headers=_AGENT)
    _expect("the call is refused 403", 403, code)
    _expect("with the refusal's body", _REFUSED, body)
    _expect("no call reached the upstream", 0, _count_forwarded_tool_calls())
    _expect(
        "the ticket fallback refused it, naming no policy",
        1,
        _wait_for(_count_denials_naming_no_policy, 1),
    )

    _start_block("enforce: a good ticket is forwarded, with only the agent's headers")
    _reset_journals()
    sid = _open_session(_ENFORCE, good)
    if sid is None:
        _report_fail("the handshake didn't return a session id")
        sid = "none"
    code, body = _call_tool(
        _ENFORCE,
        "track_package",
        {"tracking_number": "pkg-1"},
        {"Mcp-Session-Id": sid, "x-rail-status": "not-found", **good},
        request_id=2,
    )
    if code == 200 and '"text":"delivered"' in body:
        _report_ok("the tool call returns the upstream's answer")
    else:
        _report_fail(f"the tool call returns the upstream's answer: got {code} {body}")
    received = (_read_received_headers() or [{}])[0]
    _expect(
        "no x-rail* header reaches the upstream",
        "[]",
        sorted(name for name in received if name.startswith("x-rail")),
    )
    _expect(
        "accept reaches it whole", "application/json, text/event-stream", received.get("accept")
    )
    _expect(
        "the agent's Authorization reaches it as sent",
        "Bearer e2e-agent",
        received.get("authorization"),
    )

    _start_block("enforce: a ticket Apigee splits on its comma is undecodable")
    _reset_journals()
    split = {**good, "x-rail": _GOOD_TICKET + ",x"}
    _expect("the call is refused 403", 403, _get_call_status(_ENFORCE, "track_package", split))
    _expect(
        "the denial reads the ticket as undecodable",
        "undecodable",
        _wait_for_last_denial().get("metadata", {}).get("x-rail-status"),
    )

    # live §9 #1: a ticket sent twice is refused, as one sent once by a standalone gateway isn't.
    _start_block("enforce: a ticket sent on two header lines is undecodable")
    _reset_journals()
    code, _ = _send_raw_post(
        _ENFORCE, call, [("x-rail", _GOOD_TICKET), ("x-rail", _GOOD_TICKET), *_AGENT.items()]
    )
    _expect("the call is refused 403", 403, code)
    _expect(
        "the denial reads the ticket as undecodable",
        "undecodable",
        _wait_for_last_denial().get("metadata", {}).get("x-rail-status"),
    )

    # live §9 #2: Apigee splits the claim on its comma, so it reads as two and is dropped. No
    # real claim has a comma.
    _start_block("enforce: a claimed status is recorded, and one with a comma dropped")
    _reset_journals()
    claimed = {"x-rail-status": "not-found", **_AGENT}
    _expect("the call is refused 403", 403, _get_call_status(_ENFORCE, "track_package", claimed))
    metadata = _wait_for_last_denial().get("metadata", {})
    _expect("the denial reads no ticket", "absent", metadata.get("x-rail-status"))
    _expect(
        "and records a claim without a comma", "not-found", metadata.get("claimed-x-rail-status")
    )
    _reset_journals()
    claimed = {"x-rail-status": "not-found, expired", **_AGENT}
    _expect("the call is refused 403", 403, _get_call_status(_ENFORCE, "track_package", claimed))
    metadata = _wait_for_last_denial().get("metadata", {})
    _expect("the denial reads no ticket", "absent", metadata.get("x-rail-status"))
    _expect("and records no claim", "<none>", metadata.get("claimed-x-rail-status", "<none>"))

    # live §9 #5: the endpoint key is the path without the query.
    _start_block("enforce: the query string stays out of the endpoint key")
    _reset_journals()
    _expect(
        "the forbidden call is refused 403",
        403,
        _get_call_status(_ENFORCE + "?e2e=1", "forbidden_tool", good),
    )
    _expect("it named P2, the endpoint rule", 1, _wait_for(lambda: _count_denials_naming(_P2), 1))

    # live §9 #6: Apigee may send such content; whatever answers, it mustn't be forwarded.
    _start_block("enforce: a body that isn't UTF-8 never reaches the upstream")
    _reset_journals()
    code, body = _send_raw_post(
        _ENFORCE, b"\xff\xfe{}", [("x-rail", _GOOD_TICKET), *_AGENT.items()]
    )
    if code in (403, 503):
        _report_ok(f"the call is refused ({code}: {body})")
    else:
        _report_fail(f"the call is refused 403 or 503: got {code} {body}")
    _expect("nothing reached the upstream", 0, _count_upstream_requests())

    # live §9 #8: Apigee takes a callout answer of at most 4 MiB, and the answer echoes the body,
    # so rail-apigee refuses a larger call; Apigee answers one past 10 MB itself.
    _start_block("enforce: a 3.5 MiB call is forwarded, a 5 MiB one refused, an 11 MiB one too big")
    _reset_journals()
    code, body = _call_tool(_ENFORCE, "track_package", {"pad": "x" * 7 * 512 * 1024}, good)
    if code == 200 and '"text":"delivered"' in body:
        _report_ok("the 3.5 MiB call returns the upstream's answer")
    else:
        _report_fail(f"the 3.5 MiB call returns the upstream's answer: got {code} {body[:300]}")
    _reset_journals()
    code, body = _call_tool(_ENFORCE, "track_package", {"pad": "x" * 5 * 1024 * 1024}, good)
    _expect("the 5 MiB call is answered 503", 503, code)
    _expect("with the refusal's body", _CALLOUT_FAILED, body)
    _expect("nothing reached the upstream", 0, _count_upstream_requests())
    _reset_journals()
    arguments = {"pad": "x" * 11 * 1024 * 1024}
    payload = {**_CALL, "params": {"name": "track_package", "arguments": arguments}}
    body = json.dumps(payload).encode()
    code, body = _send_raw_post(_ENFORCE, body, list(good.items()), _UPLOAD_TIMEOUT_SECONDS)
    _expect("the 11 MiB call is answered 413", 413, code)
    _expect("by Apigee, as too big", True, _TOO_BIG in body)
    _expect("nothing reached the upstream", 0, _count_upstream_requests())

    # RF-Refuse answers rail.status: a policy that can't be applied, 503.
    _start_block("enforce: a policy rail-apigee can't apply answers 503")
    _reset_journals()
    code, body = _call_tool(_ENFORCE, "broken_tool", headers=good)
    _expect("the call is answered 503", 503, code)
    _expect("with the refusal's body", _CALLOUT_FAILED, body)
    _expect("nothing reached the upstream", 0, _count_upstream_requests())

    _start_block("no-callout: the proxy fails closed")
    _reset_journals()
    code, body = _call_tool(_NO_CALLOUT, "track_package", headers=good)
    _expect("the call is answered 503", 503, code)
    _expect("with RF-CalloutFailed's body", _CALLOUT_FAILED, body)
    _expect("nothing reached the upstream", 0, _count_upstream_requests())

    # The FaultRule names EC-Rail's fault, so an operator's policy failing before it
    # (RF-E2E-OperatorFault, test only) keeps its own error.
    _start_block("an operator's fault before the callout keeps its own error")
    _reset_journals()
    fault = {"x-e2e-operator-fault": "1", **_AGENT}
    for name, url in (("enforce", _ENFORCE), ("no-callout", _NO_CALLOUT)):
        code, body = _call_tool(url, "track_package", headers=fault)
        _expect(f"{name}: the call is answered 418", 418, code)
        _expect(f"{name}: with the operator's body", _OPERATOR_FAULT, body)
    _expect("nothing reached the upstream", 0, _count_upstream_requests())
    # A refused call after it: once its denial is in, the first would be too.
    _expect(
        "a call with no ticket is refused", 403, _get_call_status(_ENFORCE, "track_package", _AGENT)
    )
    _expect("only that call reached rail-apigee", 1, _wait_for(_count_denials_naming_no_policy, 1))
    # Given time for a second report to arrive, were there one.
    _expect("and it alone was reported", 1, _wait_for(_count_denials, 2, tries=8))


# Live only: what Apigee hands the callout, which the callout echoes, so the upstream receives
# what the callout read. Each prints what was seen, for the stand-in and rail-apigee to follow.
def _run_live_observations():
    good = [("x-rail", _GOOD_TICKET), *_AGENT.items()]

    _start_block("seen: what Apigee sends the callout for content that isn't UTF-8")
    _clear_journals()
    arguments = b'{"note": "a\xffb"}'
    body = b'{"jsonrpc": "2.0", "id": 9, "method": "tools/call", "params": '
    body += b'{"name": "track_package", "arguments": ' + arguments + b"}}"
    code, answer = _send_raw_post(_ENFORCE, body, good)
    _report_seen("the answer", f"{code} {answer[:200]}")
    received = _read_last_received_body()
    marker = b'"note": "'
    if received is None:
        _report_seen("the upstream received", "nothing")
    elif marker not in received:
        _report_seen("the upstream received, without the note", received[:200])
    else:
        # From the note's value on: a, then U+FFFD (ef bf bd), nothing, or ff, then b.
        note = received[received.index(marker) + len(marker) :][:6]
        _report_seen("the upstream received the note as", note.hex(" "))

    _start_block("seen: whether Apigee decompresses a body before the callout")
    _clear_journals()
    forbidden = json.dumps({**_CALL, "params": {"name": "forbidden_tool"}}).encode()
    code, answer = _send_raw_post(
        _ENFORCE, gzip.compress(forbidden), [("Content-Encoding", "gzip"), *good]
    )
    _report_seen("the answer", f"{code} {answer[:200]}")
    # P2 names the tool, so only a decompressed body can match it.
    named_p2 = _wait_for(lambda: _count_denials_naming(_P2), 1)
    _report_seen("P2 denied it, so it was decompressed", "yes" if named_p2 else "no")


def main():
    # First: the stand-in's cases reset the journals, and these count a fetch at startup.
    if not _LIVE:
        _run_callout_cases()
    _run_apigee_cases()

    _start_block("every request found a stub")
    _sweep_unmatched()
    _expect("no unmatched request reached the upstream", 0, _unmatched[_UPSTREAM])
    _expect("no unmatched request reached rail-center", 0, _unmatched[_RAIL_CENTER])

    if _LIVE:
        try:
            _run_live_observations()
        except OSError as error:
            _report_seen("the observations stopped", error)

    print()
    if _fails:
        print(f"e2e: {_fails} assertion(s) failed")
        raise SystemExit(1)
    print("e2e: all assertions passed")


if __name__ == "__main__":
    main()
