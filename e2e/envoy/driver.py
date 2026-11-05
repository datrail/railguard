"""Sends requests through each Envoy, and asserts what reached each upstream."""

import json
import sys
import time
import urllib.error
import urllib.request
import uuid

# Each upstream's journal: the plain one serves upstream.test and upstream-plain.test, the TLS one
# upstream-tls.test and upstream-agent-port.test.
_PLAIN = "upstream:8080"
_TLS = "upstream-tls:8080"
_BODY = b'{"jsonrpc":"2.0","id":1,"method":"tools/list"}'
_MARKER = "x-e2e-id"
_RAIL_CENTER = "rail-center:8080"
_HOST_ID = "host-e2e"
_TICKET = {"x-rail": "e2e-opaque-token"}
# The agent's own x-rail headers, which must never reach an upstream.
_FORGED = {
    "x-rail": "forged",
    "x-rail-status": "forged",
    "x-rail-upstream": "tls",
    "x-rail-other": "forged",
}
# Some upstreams read `_` and `-` alike, so every name with `_` is kept away too.
_UNDERSCORED = {"x_rail": "forged"}
# Not an x-rail header, so it must arrive: the journal would show a forged one left in place.
_NEAR_MISS = {"x-railing": "near-miss"}
# The first fetch of each extension's ticket is in flight as the stack comes up.
_SETTLE_SECONDS = 15

# (Through, the agent's `Host`, its extension's sandbox, whether it fetches, the status, where it
# arrives as (journal, scheme, `Host`) or None, the x-rail and `_` headers that arrive)
_CASES = [
    # Straight to the upstream, past every Envoy: the journal shows what a forgery looks like.
    (None, "upstream.test:8080", None, False, 200, (_PLAIN, "http", "upstream.test:8080"), {**_FORGED, **_UNDERSCORED}),
    # Protected with a port: the ticket, over HTTPS, there whatever port the agent sends.
    ("envoy-ext-authz", "upstream-tls.test:8443", "e2e-sandbox", True, 200, (_TLS, "https", "upstream-tls.test:8443"), _TICKET),
    ("envoy-ext-authz", "upstream-tls.test", "e2e-sandbox", True, 200, (_TLS, "https", "upstream-tls.test:8443"), _TICKET),
    ("envoy-ext-authz", "upstream-tls.test:9999", "e2e-sandbox", True, 200, (_TLS, "https", "upstream-tls.test:8443"), _TICKET),
    # Protected without one: at the agent's port.
    ("envoy-ext-authz", "upstream-agent-port.test:8443", "e2e-sandbox", True, 200, (_TLS, "https", "upstream-agent-port.test:8443"), _TICKET),
    # Protected as http://: the ticket, over plain HTTP.
    ("envoy-ext-authz", "upstream-plain.test:8080", "e2e-sandbox", True, 200, (_PLAIN, "http", "upstream-plain.test:8080"), _TICKET),
    # Not protected: as the agent sent it, with neither header, and the agent's still removed.
    ("envoy-ext-authz", "upstream.test:8080", "e2e-sandbox", True, 200, (_PLAIN, "http", "upstream.test:8080"), {}),
    # Protected, and doesn't resolve: Envoy fails it, and nothing reaches an upstream.
    ("envoy-ext-authz", "nowhere.test:8443", "e2e-sandbox", True, 503, None, None),
    # The same through ext_proc, which sets the headers in its answer to the request's headers.
    ("envoy-ext-proc", "upstream-tls.test:8443", "e2e-sandbox", True, 200, (_TLS, "https", "upstream-tls.test:8443"), _TICKET),
    ("envoy-ext-proc", "upstream-tls.test", "e2e-sandbox", True, 200, (_TLS, "https", "upstream-tls.test:8443"), _TICKET),
    ("envoy-ext-proc", "upstream-tls.test:9999", "e2e-sandbox", True, 200, (_TLS, "https", "upstream-tls.test:8443"), _TICKET),
    ("envoy-ext-proc", "upstream-agent-port.test:8443", "e2e-sandbox", True, 200, (_TLS, "https", "upstream-agent-port.test:8443"), _TICKET),
    ("envoy-ext-proc", "upstream-plain.test:8080", "e2e-sandbox", True, 200, (_PLAIN, "http", "upstream-plain.test:8080"), _TICKET),
    ("envoy-ext-proc", "upstream.test:8080", "e2e-sandbox", True, 200, (_PLAIN, "http", "upstream.test:8080"), {}),
    ("envoy-ext-proc", "nowhere.test:8443", "e2e-sandbox", True, 503, None, None),
    # Rail Center answers with no ticket for the sandbox, so x-rail-status says so.
    ("envoy-not-found", "upstream-tls.test", "not-a-registered-sandbox", True, 200, (_TLS, "https", "upstream-tls.test:8443"), {"x-rail-status": "not-found"}),
    # Rail Center serves a ticket already past expires_at, which is never presented.
    ("envoy-expired", "upstream-tls.test", "e2e-expired", True, 200, (_TLS, "https", "upstream-tls.test:8443"), {"x-rail-status": "expired"}),
    # Rail Center fails the fetch, and no ticket was ever held.
    ("envoy-issuer-down", "upstream-tls.test", "e2e-issuer-down", True, 200, (_TLS, "https", "upstream-tls.test:8443"), {"x-rail-status": "issuer-unreachable"}),
    # Rail Center is set up but disabled: never reached, neither header, still routed.
    ("envoy-disabled", "upstream-tls.test", "e2e-disabled", False, 200, (_TLS, "https", "upstream-tls.test:8443"), {}),
    # The callout fails and Envoy fails open: forwarded as sent, Envoy having removed the agent's
    # x-rail and `_` headers.
    ("envoy-no-ext-authz", "upstream.test:8080", None, False, 200, (_PLAIN, "http", "upstream.test:8080"), {}),
    ("envoy-no-ext-proc", "upstream.test:8080", None, False, 200, (_PLAIN, "http", "upstream.test:8080"), {}),
    # Nor is a protected host routed over HTTPS, despite the forged x-rail-upstream: plain HTTP to
    # its TLS port fails.
    ("envoy-no-ext-authz", "upstream-tls.test:8443", None, False, 502, None, None),
    ("envoy-no-ext-proc", "upstream-tls.test:8443", None, False, 502, None, None),
]


def _send_through(via, host):
    # The agent's request: plain HTTP to the listener, with the target in `Host`.
    marker = str(uuid.uuid4())
    headers = {
        "Host": host,
        "Content-Type": "application/json",
        _MARKER: marker,
        **_FORGED,
        **_UNDERSCORED,
        **_NEAR_MISS,
    }
    address = host if via is None else f"{via}:15001"
    request = urllib.request.Request(f"http://{address}/mcp", data=_BODY, headers=headers, method="POST")
    try:
        with urllib.request.urlopen(request, timeout=10) as response:
            return response.status, marker
    except urllib.error.HTTPError as error:
        return error.code, marker


def _find_received(marker):
    """Where each request with `marker` arrived, and its x-rail and underscored headers."""
    received = []
    for journal in [_PLAIN, _TLS]:
        with urllib.request.urlopen(f"http://{journal}/__admin/requests", timeout=10) as response:
            entries = json.load(response)["requests"]
        for entry in entries:
            request = entry["request"]
            # WireMock keeps some header names as sent and capitalises others.
            headers = {name.lower(): value for name, value in request["headers"].items()}
            if headers.get(_MARKER) == marker:
                rail = {name: value for name, value in headers.items() if name.startswith("x-rail") or "_" in name}
                received.append(((journal, request["scheme"], headers.get("host")), rail))
    return received


def _count_fetches(sandbox):
    """How many ticket fetches Rail Center got for this host and `sandbox`."""
    with urllib.request.urlopen(f"http://{_RAIL_CENTER}/__admin/requests", timeout=10) as response:
        journal = json.load(response)
    return sum(
        1
        for entry in journal["requests"]
        if entry["request"]["url"].startswith("/v1/tickets?")
        and entry["request"]["queryParams"].get("host_id", {}).get("values") == [_HOST_ID]
        and entry["request"]["queryParams"].get("sandbox_name", {}).get("values") == [sandbox]
    )


def _wait_for_fetch(sandbox):
    deadline = time.monotonic() + _SETTLE_SECONDS
    while _count_fetches(sandbox) == 0 and time.monotonic() < deadline:
        time.sleep(0.2)


def _run_case(via, host, expected_status, arrives, expected_headers):
    expected = [] if arrives is None else [(arrives, {**expected_headers, **_NEAR_MISS})]
    deadline = time.monotonic() + _SETTLE_SECONDS
    while True:
        status, marker = _send_through(via, host)
        received = _find_received(marker)
        if (status, received) == (expected_status, expected) or time.monotonic() > deadline:
            return status == expected_status and received == expected, status, received
        time.sleep(1)


def main():
    passed = True
    for via, host, sandbox, fetches_ticket, expected_status, arrives, expected_headers in _CASES:
        # Each state is the answer to a real fetch, not the state before the first one.
        if fetches_ticket:
            _wait_for_fetch(sandbox)
        ok, status, received = _run_case(via, host, expected_status, arrives, expected_headers)
        fetches = None if sandbox is None else _count_fetches(sandbox)
        ok = ok and (fetches != 0 if fetches_ticket else not fetches)
        passed = passed and ok
        print(f"{'ok  ' if ok else 'FAIL'} {via or 'direct'} {host}: {status}, received {received}, {fetches} fetches")
    return 0 if passed else 1


if __name__ == "__main__":
    sys.exit(main())
