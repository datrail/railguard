"""The gateway's cases any host runs the same, through an MCP URL, and what its drivers share: the
reporting, the stubs' journals, and MCP calls.

Assertions read WireMock's request journals, not logs: a log says what the gateway believes it
did, a journal says it happened. "Seen live" is what runs on real Apigee found.
"""

import base64
import http.client
import json
import os
import ssl
import time
import urllib.error
import urllib.parse
import urllib.request

UPSTREAM = os.environ.get("E2E_UPSTREAM", "http://upstream:8080")
RAIL_CENTER = os.environ.get("E2E_RAIL_CENTER", "http://rail-center:8080")
TIMEOUT_SECONDS = 15
# http.client sends a body in one call, which the timeout bounds whole.
UPLOAD_TIMEOUT_SECONDS = 300

# Unsigned base64url JSON, expiring in 2100, in the nested shape Rail Center stores; its skill is
# the full key Rail Center mints, `<slug>#<path>#<method>#<call>`.
GOOD_TICKET = (
    "eyJpZCI6ICI1YzhmMWU0Mi0wMDAwLTQwMDAtODAwMC0wMDAwMDAwMDAwYzEiLCAiYWdlbnRfaWQiOiAiNWM4ZjFlND"
    "ItMDAwMC00MDAwLTgwMDAtMDAwMDAwMDAwMGUyIiwgInBvc3R1cmUiOiB7Im92ZXJhbGwiOiB7InNjb3JlIjogOTAs"
    "ICJsZXZlbCI6ICJsb3cifSwgImNhdGVnb3JpZXMiOiB7fX0sICJhbGlnbm1lbnQiOiB7ImRyaWZ0ZWQiOiBbXX0sIC"
    "Jza2lsbHMiOiBbImRlbGl2ZXJ5Iy9tY3AjdG9vbHMvY2FsbCN0cmFja19wYWNrYWdlIl0sICJleHAiOiA0MTAyNDQ0"
    "ODAwfQ"
)
# P2 in rail-center-mappings/01-policy-bundle.json: the endpoint rule.
P2 = "11111111-0000-4000-8000-000000000002"
# A fake agent credential: the host must forward it as sent.
AGENT = {"Authorization": "Bearer e2e-agent"}
REFUSED = '{"error": "denied by policy"}'
CALLOUT_FAILED = '{"error": "policy ruleset cannot be applied"}'
TOO_LARGE = '{"error": "request body too large"}'
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
CALL = {
    "jsonrpc": "2.0",
    "id": 9,
    "method": "tools/call",
    "params": {"name": "track_package"},
}

# The live load balancer's certificate is self-signed; the stubs' are publicly trusted.
_TLS = ssl.create_default_context()
if os.environ.get("E2E_CA_FILE"):
    _TLS.load_verify_locations(os.environ["E2E_CA_FILE"])
# The live stubs lock /__admin with basic auth.
_ADMIN_HEADERS = {}
if os.environ.get("E2E_STUB_ADMIN_PASSWORD"):
    _credential = base64.b64encode(
        f"e2e:{os.environ['E2E_STUB_ADMIN_PASSWORD']}".encode()
    )
    _ADMIN_HEADERS["Authorization"] = f"Basic {_credential.decode()}"

_fails = 0
# Set by any sweep: each reset sweeps first, so the last check covers the whole run.
_unmatched = {UPSTREAM: 0, RAIL_CENTER: 0}


def report_ok(what):
    print(f"  ok    {what}")


def report_fail(message):
    global _fails
    _fails += 1
    print(f"  FAIL  {message}")


def expect(what, want, got):
    if got is not None and str(got) == str(want):
        report_ok(what)
    else:
        report_fail(f"{what}: wanted {want}, got {'<empty>' if got is None else got}")


# What a host was seen to do, which no assertion holds it to yet.
def report_seen(what, seen):
    print(f"  seen  {what}: {seen}")


def start_block(title):
    print(f"\n== {title} ==")


def _send_json(method, url, body=None, headers=None):
    """The response, open. Raises `HTTPError` on anything but a 2xx."""
    request = urllib.request.Request(
        url,
        data=None if body is None else json.dumps(body).encode(),
        method=method,
        headers={"Content-Type": "application/json", **(headers or {})},
    )
    return urllib.request.urlopen(request, timeout=TIMEOUT_SECONDS, context=_TLS)


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
def count_forwarded_tool_calls():
    return _count(
        UPSTREAM,
        {
            "method": "POST",
            "urlPath": "/mcp",
            "bodyPatterns": [{"matchesJsonPath": "$[?(@.method == 'tools/call')]"}],
        },
    )


def count_upstream_requests():
    return _count(UPSTREAM, {"method": "ANY", "urlPattern": ".*"})


def count_denials():
    return _count(RAIL_CENTER, {"method": "POST", "urlPath": "/v1/denials"})


def count_bundle_fetches():
    return _count(RAIL_CENTER, {"method": "GET", "urlPath": "/v1/policy-bundle"})


def count_denials_naming(policy):
    pattern = f"$[?(@.policy_id == '{policy}')]"
    return _count(
        RAIL_CENTER,
        {
            "method": "POST",
            "urlPath": "/v1/denials",
            "bodyPatterns": [{"matchesJsonPath": pattern}],
        },
    )


# A fallback refusal omits `policy_id`; this matches its absence, not a null.
def count_denials_naming_no_policy():
    return _count(
        RAIL_CENTER,
        {
            "method": "POST",
            "urlPath": "/v1/denials",
            "bodyPatterns": [{"matchesJsonPath": "$[?(!@.policy_id)]"}],
        },
    )


def _read_denials():
    """The denial reports Rail Center's stub received, newest first; [] if it couldn't be asked."""
    try:
        requests = _ask_admin("GET", RAIL_CENTER, "/__admin/requests")["requests"]
    except (OSError, ValueError, KeyError):
        return []
    return [
        json.loads(r["request"]["body"])
        for r in requests
        if r["request"]["url"] == "/v1/denials"
    ]


def wait_for(read, want, tries=40, interval=0.25):
    """What `read` last returned, once it reached `want` or `tries` ran out."""
    for _ in range(tries):
        got = read()
        if (got or 0) >= want:
            return got
        time.sleep(interval)
    return got


# Denials are reported after the caller is answered, so each is waited for.
def wait_for_last_denial():
    """The newest denial report, or {} if none arrived."""
    wait_for(lambda: len(_read_denials()), 1)
    return (_read_denials() or [{}])[0]


def read_received_headers():
    """The headers of each request the upstream logged, newest first, names lower-cased."""
    try:
        requests = _ask_admin("GET", UPSTREAM, "/__admin/requests")["requests"]
    except (OSError, ValueError, KeyError):
        return None
    return [
        {name.lower(): value for name, value in r["request"]["headers"].items()}
        for r in requests
    ]


# 1 if any request found no stub, which no count of matched requests would show.
def _has_unmatched(base):
    try:
        requests = _ask_admin("GET", base, "/__admin/requests/unmatched")["requests"]
    except (OSError, ValueError, KeyError):
        return 1
    return 0 if not requests else 1


def sweep_unmatched():
    for base in _unmatched:
        _unmatched[base] |= _has_unmatched(base)


def reset_journals():
    sweep_unmatched()
    clear_journals()


# Without the sweep: what's only seen can't fail the run.
def clear_journals():
    _ask_admin("DELETE", UPSTREAM, "/__admin/requests")
    _ask_admin("DELETE", RAIL_CENTER, "/__admin/requests")


def read_last_received_body():
    """The body of the newest request the upstream logged, as bytes, or None."""
    try:
        requests = _ask_admin("GET", UPSTREAM, "/__admin/requests")["requests"]
        return base64.b64decode(requests[0]["request"]["bodyAsBase64"])
    except (OSError, ValueError, KeyError, IndexError):
        return None


def send_raw_post(url, body, headers, timeout=TIMEOUT_SECONDS):
    """The status and body of a POST of `body` (bytes) with `headers`, a list of (name, value)
    pairs, so a header can repeat; (None, "") if nothing answered."""
    parts = urllib.parse.urlsplit(url)
    if parts.scheme == "https":
        connection = http.client.HTTPSConnection(
            parts.netloc, timeout=timeout, context=_TLS
        )
    else:
        connection = http.client.HTTPConnection(parts.netloc, timeout=timeout)
    try:
        connection.putrequest(
            "POST", parts.path + (f"?{parts.query}" if parts.query else "")
        )
        for name, value in [*_MCP_HEADERS.items(), *headers]:
            connection.putheader(name, value)
        connection.putheader("Content-Length", str(len(body)))
        connection.endheaders(body)
        response = connection.getresponse()
        return response.status, response.read().decode(errors="replace")
    except (OSError, http.client.HTTPException):
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
def open_session(url, headers):
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


def call_tool(url, tool, arguments=None, headers=None, request_id=9):
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
def get_call_status(url, tool, headers=None):
    return call_tool(url, tool, headers=headers)[0]


def run_gateway_cases(mcp):
    """The cases every host's gateway passes alike, through the MCP endpoint at `mcp`."""
    good = {"x-rail": GOOD_TICKET, **AGENT}
    call = json.dumps(CALL).encode()

    start_block("mcp: a call with no ticket is refused")
    reset_journals()
    code, body = call_tool(mcp, "track_package", headers=AGENT)
    expect("the call is refused 403", 403, code)
    expect("with the refusal's body", REFUSED, body)
    expect("no call reached the upstream", 0, count_forwarded_tool_calls())
    expect(
        "the ticket fallback refused it, naming no policy",
        1,
        wait_for(count_denials_naming_no_policy, 1),
    )

    start_block("mcp: a good ticket is forwarded, with only the agent's headers")
    reset_journals()
    sid = open_session(mcp, good)
    if sid is None:
        report_fail("the handshake didn't return a session id")
        sid = "none"
    code, body = call_tool(
        mcp,
        "track_package",
        {"tracking_number": "pkg-1"},
        {"Mcp-Session-Id": sid, "x-rail-status": "not-found", "x_team": "a", **good},
        request_id=2,
    )
    if code == 200 and '"text":"delivered"' in body:
        report_ok("the tool call returns the upstream's answer")
    else:
        report_fail(f"the tool call returns the upstream's answer: got {code} {body}")
    received = (read_received_headers() or [{}])[0]
    expect(
        "no x-rail* or _ header reaches the upstream",
        "[]",
        sorted(name for name in received if name.startswith("x-rail") or "_" in name),
    )
    expect(
        "accept reaches it whole",
        "application/json, text/event-stream",
        received.get("accept"),
    )
    expect(
        "the agent's Authorization reaches it as sent",
        "Bearer e2e-agent",
        received.get("authorization"),
    )

    # Seen live: a ticket sent twice is refused.
    start_block("mcp: a ticket sent on two header lines is undecodable")
    reset_journals()
    code, _ = send_raw_post(
        mcp, call, [("x-rail", GOOD_TICKET), ("x-rail", GOOD_TICKET), *AGENT.items()]
    )
    expect("the call is refused 403", 403, code)
    expect(
        "the denial reads the ticket as undecodable",
        "undecodable",
        wait_for_last_denial().get("metadata", {}).get("x-rail-status"),
    )

    start_block("mcp: a claimed status is recorded")
    reset_journals()
    claimed = {"x-rail-status": "not-found", **AGENT}
    expect(
        "the call is refused 403", 403, get_call_status(mcp, "track_package", claimed)
    )
    metadata = wait_for_last_denial().get("metadata", {})
    expect("the denial reads no ticket", "absent", metadata.get("x-rail-status"))
    expect("and records the claim", "not-found", metadata.get("claimed-x-rail-status"))

    # Seen live: the endpoint key is the path without the query.
    start_block("mcp: the query string stays out of the endpoint key")
    reset_journals()
    expect(
        "the forbidden call is refused 403",
        403,
        get_call_status(mcp + "?e2e=1", "forbidden_tool", good),
    )
    expect(
        "it named P2, the endpoint rule",
        1,
        wait_for(lambda: count_denials_naming(P2), 1),
    )

    # Apigee replaces such bytes with U+FFFD (seen live), Envoy passes them as sent: either way
    # the body isn't JSON, and is refused.
    start_block("mcp: a body that isn't UTF-8 never reaches the upstream")
    reset_journals()
    code, body = send_raw_post(
        mcp, b"\xff\xfe{}", [("x-rail", GOOD_TICKET), *AGENT.items()]
    )
    if code in (403, 503):
        report_ok(f"the call is refused ({code}: {body})")
    else:
        report_fail(f"the call is refused 403 or 503: got {code} {body}")
    expect("nothing reached the upstream", 0, count_upstream_requests())


def check_every_request_matched():
    start_block("every request found a stub")
    sweep_unmatched()
    expect("no unmatched request reached the upstream", 0, _unmatched[UPSTREAM])
    expect("no unmatched request reached rail-center", 0, _unmatched[RAIL_CENTER])


def exit_with_result():
    print()
    if _fails:
        print(f"e2e: {_fails} assertion(s) failed")
        raise SystemExit(1)
    print("e2e: all assertions passed")
